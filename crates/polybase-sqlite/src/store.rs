//! [`LocalStore`] implementation over `sqlx::SqlitePool`.
//!
//! Speaks the canonical column names registered in `polybase::registry`. Apps must run their
//! own migrations to create the tables (or opt in to the built-in [`crate::MIGRATOR`] for the
//! `kvs` table).

use async_trait::async_trait;
use polybase::errors::PolyError;
use polybase::persistence::{LocalStore, Record, VersionRow};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{Column, Row, TypeInfo, ValueRef};

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
        let placeholders = std::iter::repeat_n("?", ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, version, deleted FROM {table} WHERE id IN ({placeholders})");
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
        Ok(Some(row_to_record(&row)))
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

/// Decode one [`SqliteRow`] into a polybase [`Record`], inspecting the runtime SQLite type
/// of each column so integers stay integers, booleans stay booleans (as 0/1 ints in SQLite),
/// reals stay reals, NULLs stay nulls, and TEXT columns stay strings (or, for legitimately
/// JSON-shaped TEXT, stay strings the consumer can `serde_json::from_str` themselves).
fn row_to_record(row: &SqliteRow) -> Record {
    let mut record = Record::new();
    for col in row.columns() {
        let name = col.name();
        let raw = match row.try_get_raw(name) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.is_null() {
            record.insert(name.into(), Value::Null);
            continue;
        }
        let type_name = raw.type_info().name().to_uppercase();
        let value = match type_name.as_str() {
            "INTEGER" | "INT" | "BIGINT" | "INT8" | "BOOLEAN" => {
                row.try_get::<i64, _>(name).map(|n| Value::Number(n.into())).unwrap_or(Value::Null)
            }
            "REAL" | "FLOAT" | "DOUBLE" => row
                .try_get::<f64, _>(name)
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            "BLOB" => Value::Null, // BLOBs aren't representable as JSON; consumers shouldn't read them this way.
            _ => row.try_get::<String, _>(name).map(Value::String).unwrap_or(Value::Null),
        };
        record.insert(name.into(), value);
    }
    record
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
        // Arrays and objects are serialized as JSON text — the local mirror's `value` column
        // for KVS, for example, is TEXT carrying JSON.
        other => q.bind(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use polybase::auth::{SessionPayload, SessionStore};
    use polybase::client::{Client, ClientConfig};
    use polybase::events::EventBus;
    use polybase::offline_queue::{MemoryQueue, OfflineQueue};
    use polybase::persistence::LocalStore;
    use polybase::registry::Registry;
    use polybase::sync::Coordinator;
    use polybase::{Kvs, kvs};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn fresh_session() -> SessionPayload {
        use std::time::{SystemTime, UNIX_EPOCH};
        let exp =
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
                + 3600;
        SessionPayload {
            user_id: "user-1".into(),
            access_token: "tok".into(),
            refresh_token: "rt".into(),
            expires_at: exp,
            updated_at: 0,
        }
    }

    async fn setup() -> (TempDir, SqliteLocalStore, Coordinator, Kvs) {
        let tmp = TempDir::new().expect("tempdir");
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();
        manager.switch_user("user-1").await.expect("switch user");
        let local: Arc<dyn LocalStore> = Arc::new(SqliteLocalStore::new(manager.clone()));

        let client = Client::new(ClientConfig {
            supabase_url: "https://example.supabase.co".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: None,
        })
        .unwrap();
        let bus = EventBus::new();
        let sessions = SessionStore::new(client.clone(), bus.clone());
        sessions.set_session(fresh_session()).await.unwrap();
        let registry = Arc::new(Registry::new());
        Kvs::register(&registry);
        let queue: Arc<dyn OfflineQueue> = Arc::new(MemoryQueue::new());
        let coord = Coordinator::new(client, sessions, registry, queue, bus, None, local.clone());
        let kvs = Kvs::new(coord.clone());

        (tmp, SqliteLocalStore::new(manager), coord, kvs)
    }

    #[tokio::test]
    async fn migrator_creates_kvs_table() {
        let tmp = TempDir::new().unwrap();
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();
        manager.switch_user("u1").await.unwrap();
        let pool = manager.pool().await.expect("pool");
        let row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM kvs").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 0);
    }

    #[tokio::test]
    async fn kvs_set_then_get_round_trips_through_local_mirror() {
        let (_tmp, _store, _coord, kvs) = setup().await;

        // First write — network is unreachable so this is the offline-replay path. The local
        // mirror still reflects the change so reads succeed.
        kvs.set("prism", "default_model", &json!({"id": "claude-sonnet-4-5"}), 1).await.unwrap();

        let value: Option<serde_json::Value> = kvs.get("prism", "default_model").await.unwrap();
        assert_eq!(value.as_ref().and_then(|v| v.get("id")), Some(&json!("claude-sonnet-4-5")));
    }

    #[tokio::test]
    async fn kvs_delete_makes_get_return_none() {
        let (_tmp, _store, _coord, kvs) = setup().await;
        kvs.set("prism", "ephemeral", &json!(true), 1).await.unwrap();
        kvs.delete("prism", "ephemeral", 2).await.unwrap();
        let value: Option<serde_json::Value> = kvs.get("prism", "ephemeral").await.unwrap();
        assert!(value.is_none());
    }

    #[tokio::test]
    async fn read_record_preserves_integer_columns() {
        let (_tmp, store, _coord, kvs) = setup().await;
        kvs.set("prism", "counter", &json!(42), 7).await.unwrap();

        let id = kvs::encode_id("prism", "counter");
        let record = store.read_record("kvs", &id).await.unwrap().expect("present");
        assert_eq!(record.get("version"), Some(&json!(7)));
        assert_eq!(record.get("deleted"), Some(&json!(0))); // SQLite booleans live as integers.
    }

    #[tokio::test]
    async fn switch_user_wipes_previous_user_data() {
        let tmp = TempDir::new().unwrap();
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();

        manager.switch_user("alice").await.unwrap();
        let alice_pool = manager.pool().await.expect("alice pool");
        sqlx::query("INSERT INTO kvs (id, namespace, key, value, version) VALUES ('a::b', 'a', 'b', '\"x\"', 1)")
            .execute(&alice_pool)
            .await
            .unwrap();
        assert!(tmp.path().join("alice").join("sync.db").exists());

        manager.switch_user("bob").await.unwrap();
        assert!(!tmp.path().join("alice").exists(), "alice's directory should be wiped");
        assert!(tmp.path().join("bob").join("sync.db").exists());
    }
}
