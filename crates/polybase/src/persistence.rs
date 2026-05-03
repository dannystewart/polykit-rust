//! Local persistence abstraction.
//!
//! polybase does not depend on `sqlx` directly. Instead, apps either implement [`LocalStore`]
//! themselves (for an existing storage layer) or pull in [`polybase-sqlite`] for a default
//! sqlx + SQLite implementation matching the Tauri Prism schema.
//!
//! The trait operates on opaque records (`serde_json::Map<String, Value>`) so it can serve as
//! the lingua franca between the registry/sync engine and any underlying store. Callers map
//! between domain types and `Map<String, Value>` at the registry boundary.

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::errors::PolyError;

/// One row's `(id, version, deleted)` triple — the minimum we need for reconcile decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    /// Primary key of the row.
    pub id: String,
    /// Current version (monotonic per the contract).
    pub version: i64,
    /// True for soft-deleted (tombstoned) rows.
    pub deleted: bool,
}

/// Opaque key/value record. Callers ensure the keys match the registered local column names.
pub type Record = Map<String, Value>;

/// Trait every local-mirror backend implements so the polybase sync engine can read/write rows
/// without knowing whether the underlying store is SQLite, Postgres, in-memory, or otherwise.
#[async_trait]
pub trait LocalStore: Send + Sync {
    /// Switch the active per-user store. Empty `user_id` means "no user" (e.g. after sign-out)
    /// and the implementation should drop / wipe any cached per-user state per the contract.
    async fn switch_user(&self, user_id: &str) -> Result<(), PolyError>;

    /// Return version + deletion state for the given ids on a table. Missing rows are simply
    /// omitted from the returned vector.
    async fn read_versions(
        &self,
        table: &str,
        ids: &[String],
    ) -> Result<Vec<VersionRow>, PolyError>;

    /// Read a single row. Returns `None` if not present (or hard-deleted).
    async fn read_record(&self, table: &str, id: &str) -> Result<Option<Record>, PolyError>;

    /// Read all ids on a table — used for reconcile planning.
    async fn read_all_ids(&self, table: &str) -> Result<Vec<String>, PolyError>;

    /// Insert-or-update a record by primary key (`id`).
    async fn upsert_record(&self, table: &str, record: Record) -> Result<(), PolyError>;

    /// Mark a row as deleted (soft delete) at the given version.
    async fn mark_deleted(&self, table: &str, id: &str, version: i64) -> Result<(), PolyError>;

    /// Hard-delete a row. Used by tombstone vacuum after remote convergence.
    async fn hard_delete(&self, table: &str, id: &str) -> Result<(), PolyError>;
}

/// Convenient zero-impl marker for tests / examples that want a no-op LocalStore.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullLocalStore;

#[async_trait]
impl LocalStore for NullLocalStore {
    async fn switch_user(&self, _user_id: &str) -> Result<(), PolyError> {
        Ok(())
    }

    async fn read_versions(
        &self,
        _table: &str,
        _ids: &[String],
    ) -> Result<Vec<VersionRow>, PolyError> {
        Ok(Vec::new())
    }

    async fn read_record(&self, _table: &str, _id: &str) -> Result<Option<Record>, PolyError> {
        Ok(None)
    }

    async fn read_all_ids(&self, _table: &str) -> Result<Vec<String>, PolyError> {
        Ok(Vec::new())
    }

    async fn upsert_record(&self, _table: &str, _record: Record) -> Result<(), PolyError> {
        Ok(())
    }

    async fn mark_deleted(&self, _table: &str, _id: &str, _version: i64) -> Result<(), PolyError> {
        Ok(())
    }

    async fn hard_delete(&self, _table: &str, _id: &str) -> Result<(), PolyError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::offline_queue::{MemoryQueue, OfflineQueue};

    /// Compile-time guard: any future change to the trait surface that breaks the reference
    /// impl will fail the build instead of silently shipping a broken contract.
    #[test]
    fn null_local_store_satisfies_trait_object() {
        let _store: Arc<dyn LocalStore> = Arc::new(NullLocalStore);
    }

    #[test]
    fn memory_queue_satisfies_trait_object() {
        let _queue: Arc<dyn OfflineQueue> = Arc::new(MemoryQueue::new());
    }

    #[tokio::test]
    async fn null_local_store_methods_are_callable_as_trait_object() {
        let store: Arc<dyn LocalStore> = Arc::new(NullLocalStore);
        store.switch_user("u1").await.unwrap();
        assert!(store.read_versions("messages", &[]).await.unwrap().is_empty());
        assert!(store.read_record("messages", "m1").await.unwrap().is_none());
        assert!(store.read_all_ids("messages").await.unwrap().is_empty());
        store.upsert_record("messages", Map::new()).await.unwrap();
        store.mark_deleted("messages", "m1", 1).await.unwrap();
        store.hard_delete("messages", "m1").await.unwrap();
    }
}
