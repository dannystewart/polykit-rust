//! `LocalStore` implementation over `sqlx::SqlitePool`.
//!
//! Speaks the canonical column names registered in `polybase::registry`. Apps must run their
//! own migrations to create the tables; this module does not own schema. (The host app is in a
//! better position to evolve its own schema than the library.)

use async_trait::async_trait;
use polybase::errors::PolyError;
use polybase::persistence::{LocalStore, Record, VersionRow};
use serde_json::Value;
use sqlx::{Column, Row};

use crate::manager::DbManager;

/// SQLite-backed [`LocalStore`].
#[derive(Clone)]
pub struct SqliteLocalStore {
    manager: DbManager,
}

impl SqliteLocalStore {
    /// Build a store that delegates per-user pool management to `manager`.
    pub fn new(manager: DbManager) -> Self {
        Self { manager }
    }

    /// Borrow the underlying [`DbManager`].
    pub fn manager(&self) -> &DbManager {
        &self.manager
    }
}

fn pool_required() -> PolyError {
    PolyError::Local("no active user pool".into())
}

#[async_trait]
impl LocalStore for SqliteLocalStore {
    async fn switch_user(&self, user_id: &str) -> Result<(), PolyError> {
        self.manager.switch_user(user_id).await.map_err(|err| PolyError::Local(err.to_string()))
    }

    async fn read_versions(
        &self,
        table: &str,
        ids: &[String],
    ) -> Result<Vec<VersionRow>, PolyError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        let mut placeholders = Vec::with_capacity(ids.len());
        for _ in 0..ids.len() {
            placeholders.push("?");
        }
        let sql = format!(
            "SELECT id, version, deleted FROM {table} WHERE id IN ({})",
            placeholders.join(",")
        );
        let mut q = sqlx::query(&sql);
        for id in ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&pool).await.map_err(|err| PolyError::Local(err.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id").map_err(|err| PolyError::Local(err.to_string()))?;
            let version: i64 =
                row.try_get("version").map_err(|err| PolyError::Local(err.to_string()))?;
            let deleted_int: i64 =
                row.try_get("deleted").map_err(|err| PolyError::Local(err.to_string()))?;
            out.push(VersionRow { id, version, deleted: deleted_int != 0 });
        }
        Ok(out)
    }

    async fn read_record(&self, table: &str, id: &str) -> Result<Option<Record>, PolyError> {
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        let sql = format!("SELECT * FROM {table} WHERE id = ?");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_optional(&pool)
            .await
            .map_err(|err| PolyError::Local(err.to_string()))?;
        let Some(row) = row else { return Ok(None) };
        let mut record = Record::new();
        for col in row.columns() {
            let name = col.name();
            // SQLite columns are typed dynamically; pull as string and best-effort decode.
            let value: Option<String> = row.try_get(name).ok();
            let json_value = match value {
                Some(s) => Value::String(s),
                None => Value::Null,
            };
            record.insert(name.into(), json_value);
        }
        Ok(Some(record))
    }

    async fn read_all_ids(&self, table: &str) -> Result<Vec<String>, PolyError> {
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        let sql = format!("SELECT id FROM {table}");
        let rows = sqlx::query(&sql)
            .fetch_all(&pool)
            .await
            .map_err(|err| PolyError::Local(err.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id").map_err(|err| PolyError::Local(err.to_string()))?;
            out.push(id);
        }
        Ok(out)
    }

    async fn upsert_record(&self, table: &str, record: Record) -> Result<(), PolyError> {
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        if record.is_empty() {
            return Ok(());
        }
        let columns: Vec<&String> = record.keys().collect();
        let placeholders = columns.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let column_list = columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",");
        let update_clause = columns
            .iter()
            .filter(|c| c.as_str() != "id")
            .map(|c| format!("{c} = excluded.{c}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "INSERT INTO {table} ({column_list}) VALUES ({placeholders}) ON CONFLICT(id) DO UPDATE SET {update_clause}"
        );
        let mut q = sqlx::query(&sql);
        for column in &columns {
            let value = record.get(*column).cloned().unwrap_or(Value::Null);
            q = bind_json_value(q, value);
        }
        q.execute(&pool).await.map_err(|err| PolyError::Local(err.to_string()))?;
        Ok(())
    }

    async fn mark_deleted(&self, table: &str, id: &str, version: i64) -> Result<(), PolyError> {
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        let sql = format!("UPDATE {table} SET deleted = 1, version = ? WHERE id = ?");
        sqlx::query(&sql)
            .bind(version)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(|err| PolyError::Local(err.to_string()))?;
        Ok(())
    }

    async fn hard_delete(&self, table: &str, id: &str) -> Result<(), PolyError> {
        let pool = self.manager.pool().await.ok_or_else(pool_required)?;
        let sql = format!("DELETE FROM {table} WHERE id = ?");
        sqlx::query(&sql)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(|err| PolyError::Local(err.to_string()))?;
        Ok(())
    }
}

fn bind_json_value<'q>(
    q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: Value,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match value {
        Value::Null => q.bind::<Option<String>>(None),
        Value::Bool(b) => q.bind(i64::from(b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => q.bind(s),
        other => q.bind(other.to_string()),
    }
}
