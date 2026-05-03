//! Typed key-value store on top of the `kvs` table.
//!
//! Replaces iCloud KVS. The `kvs` table uses `(user_id, namespace, key)` as composite primary
//! key with realtime publication enabled. Each row carries `version`, `deleted`, `updated_at`
//! so it inherits all the same conflict-resolution machinery as other synced entities.
//!
//! See `crates/polybase/migrations/0001_kvs.sql` for the canonical schema.
//!
//! Crate-internal until the `polybase-tauri` plugin exposes it through commands.
#![allow(unreachable_pub, missing_docs, dead_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::errors::PolyError;
use crate::registry::{ColumnDef, EntityConfig, Registry};
use crate::sync::Coordinator;

/// Canonical table name.
pub const KVS_TABLE: &str = "kvs";

/// One row in the KVS table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KvsRow {
    pub namespace: String,
    pub key: String,
    pub value: Value,
    pub version: i64,
    pub deleted: bool,
    pub updated_at: String,
}

/// Build the [`EntityConfig`] for the `kvs` table. Apps that want KVS must register this once
/// at startup. Writes go via PostgREST since RLS allows users to read/write their own KVS rows
/// directly — there is no Edge Function for KVS.
pub fn kvs_entity_config() -> EntityConfig {
    EntityConfig::synced(KVS_TABLE, "Kvs")
        .columns([
            // Composite key — the registry treats `id` as canonical primary key elsewhere, but
            // KVS encodes its key as `{namespace}::{key}` for the `id` slot when needed by the
            // coordinator. The persistence layer is also free to use the actual composite PK.
            ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
            ColumnDef::synced("namespace", "namespace", "namespace", "TEXT", "string", false),
            ColumnDef::synced("key", "key", "key", "TEXT", "string", false),
            ColumnDef::synced("value", "value", "value", "TEXT", "jsonb", false),
            ColumnDef::synced("version", "version", "version", "INTEGER", "integer", false),
            ColumnDef::synced("deleted", "deleted", "deleted", "INTEGER", "boolean", false),
            ColumnDef::synced("updated_at", "updated_at", "updated_at", "TEXT", "string", false),
        ])
        .user_id_column("user_id")
        .include_user_id(true)
}

/// Encode a `(namespace, key)` pair into the synthetic `id` slot.
pub fn encode_id(namespace: &str, key: &str) -> String {
    format!("{namespace}::{key}")
}

/// Decode a synthetic id back into `(namespace, key)`.
pub fn decode_id(id: &str) -> Option<(&str, &str)> {
    id.split_once("::")
}

/// Public KVS API. Cheap to clone (delegates to the Coordinator).
#[derive(Clone)]
pub struct Kvs {
    coordinator: Coordinator,
}

impl Kvs {
    pub fn new(coordinator: Coordinator) -> Self {
        Self { coordinator }
    }

    /// Register the KVS entity if not already present. Idempotent.
    pub fn register(registry: &Registry) {
        if !registry.is_registered_table(KVS_TABLE) {
            registry.register(kvs_entity_config());
        }
    }

    /// Set a value. `version` should be the next version (`current + 1`) per the contract.
    pub async fn set<T: Serialize>(
        &self,
        namespace: &str,
        key: &str,
        value: &T,
        version: i64,
    ) -> Result<(), PolyError> {
        let id = encode_id(namespace, key);
        let mut record: Map<String, Value> = Map::new();
        record.insert("id".into(), Value::String(id));
        record.insert("namespace".into(), Value::String(namespace.into()));
        record.insert("key".into(), Value::String(key.into()));
        record.insert("value".into(), serde_json::to_value(value)?);
        record.insert("version".into(), Value::Number(version.into()));
        record.insert("deleted".into(), Value::Bool(false));
        record.insert("updated_at".into(), Value::String(chrono::Utc::now().to_rfc3339()));
        self.coordinator.persist_change(KVS_TABLE, record).await
    }

    /// Mark a key as deleted (tombstone).
    pub async fn delete(&self, namespace: &str, key: &str, version: i64) -> Result<(), PolyError> {
        let id = encode_id(namespace, key);
        self.coordinator.delete(KVS_TABLE, &id, version).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        let id = encode_id("prism", "default_model");
        assert_eq!(decode_id(&id), Some(("prism", "default_model")));
    }

    #[test]
    fn entity_config_has_kvs_columns() {
        let cfg = kvs_entity_config();
        assert_eq!(cfg.table_name, KVS_TABLE);
        assert!(cfg.column_names().contains(&"namespace"));
        assert!(cfg.column_names().contains(&"value"));
    }
}
