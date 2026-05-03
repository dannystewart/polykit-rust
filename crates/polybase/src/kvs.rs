//! Typed key-value store on top of the `kvs` table.
//!
//! Replaces iCloud KVS for cross-device user preferences. The `kvs` table uses `(user_id,
//! namespace, key)` as composite primary key with realtime publication enabled. Each row
//! carries `version`, `deleted`, and `updated_at` so it inherits the same conflict-resolution
//! machinery as every other synced entity.
//!
//! **Schema:** see `crates/polybase/migrations/0001_kvs.sql` (Supabase) and
//! `crates/polybase-sqlite/migrations/0001_kvs.sql` (local mirror).
//!
//! **Write path:** [`crate::registry::WritePath::PostgREST`]. The user's JWT can read/write
//! its own KVS rows directly; there is no Edge Function in front.
//!
//! **Reads:** served from the local mirror via the [`Coordinator`] so they are synchronous
//! and offline-tolerant.
//!
//! **Notifications:** every set/delete (local-side or remote-side via [`Coordinator::apply_remote_record`])
//! emits a [`PolyEvent::KvsChanged`] event. Subscribe via [`Kvs::subscribe`] for a typed
//! stream filtered to KVS events only.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::broadcast;

use crate::errors::PolyError;
use crate::events::PolyEvent;
use crate::registry::{ColumnDef, EntityConfig, Registry};
use crate::sync::Coordinator;

/// Canonical table name.
pub const KVS_TABLE: &str = "kvs";

/// One row in the KVS table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KvsRow {
    /// Namespace this key lives under (e.g. `"prism"`).
    pub namespace: String,
    /// Key name within the namespace.
    pub key: String,
    /// Stored JSON value. May be any JSON shape.
    pub value: Value,
    /// Monotonic version (per the contract, increments by 1 on every write; +1000 on undelete).
    pub version: i64,
    /// Tombstone flag.
    pub deleted: bool,
    /// ISO-8601 timestamp of last update.
    pub updated_at: String,
}

/// One side of a [`Kvs::subscribe`] payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvsChange {
    /// Namespace the change occurred under.
    pub namespace: String,
    /// Key the change occurred on.
    pub key: String,
    /// True when the change was a tombstone (delete) rather than a value update.
    pub deleted: bool,
}

/// Build the [`EntityConfig`] for the `kvs` table. Apps that want KVS must register this once
/// at startup (or call [`Kvs::register`] which is idempotent).
///
/// Writes go via PostgREST since RLS allows users to read/write their own KVS rows directly —
/// there is no Edge Function for KVS.
pub fn kvs_entity_config() -> EntityConfig {
    EntityConfig::synced(KVS_TABLE, "Kvs")
        .columns([
            // The local-mirror primary key is the synthetic `id` slot, encoded as
            // `{namespace}::{key}`. The remote table also has this column for consistency,
            // but Supabase enforces the composite `(user_id, namespace, key)` unique key
            // separately — see `crates/polybase/migrations/0001_kvs.sql`.
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

/// Encode a `(namespace, key)` pair into the synthetic `id` slot used by the local mirror.
pub fn encode_id(namespace: &str, key: &str) -> String {
    format!("{namespace}::{key}")
}

/// Decode a synthetic id back into `(namespace, key)`.
pub fn decode_id(id: &str) -> Option<(&str, &str)> {
    id.split_once("::")
}

/// Public KVS API. Cheap to clone (delegates to the [`Coordinator`]).
#[derive(Clone)]
pub struct Kvs {
    coordinator: Coordinator,
}

impl Kvs {
    /// Build a new KVS handle wrapping the shared coordinator.
    pub fn new(coordinator: Coordinator) -> Self {
        Self { coordinator }
    }

    /// Register the KVS entity if not already present. Idempotent.
    pub fn register(registry: &Registry) {
        if !registry.is_registered_table(KVS_TABLE) {
            registry.register(kvs_entity_config());
        }
    }

    /// Read a value out of the local mirror and decode it as `T`. Returns `None` when the
    /// key has never been set or has been tombstoned.
    pub async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<T>, PolyError> {
        let id = encode_id(namespace, key);
        let Some(row) = self.coordinator.read_record(KVS_TABLE, &id).await? else {
            return Ok(None);
        };
        // SQLite stores `deleted` as INTEGER (0/1), not a true bool — accept both shapes so
        // either kind of LocalStore can drive us.
        let deleted = row
            .get("deleted")
            .map(|v| v.as_bool().unwrap_or_else(|| v.as_i64().unwrap_or(0) != 0))
            .unwrap_or(false);
        if deleted {
            return Ok(None);
        }
        let Some(raw) = row.get("value") else {
            return Ok(None);
        };
        let value = match raw {
            Value::String(text) => {
                serde_json::from_str(text).unwrap_or(Value::String(text.clone()))
            }
            other => other.clone(),
        };
        let typed: T = serde_json::from_value(value)?;
        Ok(Some(typed))
    }

    /// Set a value. `version` should be the next version (`current + 1`) per the contract.
    /// On the very first write of a key, pass `version = 1`.
    ///
    /// On success, emits [`PolyEvent::KvsChanged { deleted: false }`] (via [`Coordinator`]).
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

    /// Mark a key as deleted (tombstone). `version` should be the next version per the
    /// contract. Subscribers receive [`PolyEvent::KvsChanged { deleted: true }`].
    pub async fn delete(&self, namespace: &str, key: &str, version: i64) -> Result<(), PolyError> {
        let id = encode_id(namespace, key);
        self.coordinator.delete(KVS_TABLE, &id, version).await
    }

    /// Subscribe to KVS change events (set + delete). Returns a tokio broadcast receiver
    /// already filtered to [`PolyEvent::KvsChanged`] events only — non-KVS events are
    /// silently dropped before reaching the consumer.
    ///
    /// The returned channel will lag if the consumer falls behind; callers should treat
    /// `RecvError::Lagged` as "refetch via [`Self::get`]".
    pub fn subscribe(&self) -> broadcast::Receiver<KvsChange> {
        let mut bus_rx = self.coordinator.events().subscribe();
        let (tx, rx) = broadcast::channel::<KvsChange>(64);
        tokio::spawn(async move {
            loop {
                match bus_rx.recv().await {
                    Ok(PolyEvent::KvsChanged { namespace, key, deleted }) => {
                        if tx.send(KvsChange { namespace, key, deleted }).is_err() {
                            // No more subscribers — exit the relay loop.
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        rx
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

    #[test]
    fn register_is_idempotent() {
        let reg = Registry::new();
        Kvs::register(&reg);
        Kvs::register(&reg);
        assert!(reg.is_registered_table(KVS_TABLE));
        assert_eq!(reg.registered_tables().iter().filter(|t| *t == KVS_TABLE).count(), 1);
    }
}
