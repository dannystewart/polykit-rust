//! Hybrid sync coordinator — top-level public API for persisting changes.
//!
//! Reads the registered [`crate::registry::WritePath`] for the entity's table to decide whether
//! the mutation flows through:
//! - [`crate::edge::EdgeClient`] (synced chat-style entities), or
//! - [`crate::sync::push::Pusher`] (KVS, device_tokens, other PostgREST-friendly tables).
//!
//! Either way, the [`crate::sync::echo::EchoTracker`] is marked before the network call so the
//! realtime subscriber suppresses the rebound event for our own write.
//!
//! When an [`Encryption`] is wired in, the coordinator transparently encrypts every column that
//! the registry flags as `encrypted = true` before pushing, and the matching
//! [`Coordinator::decode_remote_record`] helper decrypts those columns when consumers ingest a
//! row pulled from PostgREST or arriving via realtime.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::auth::SessionStore;
use crate::client::Client;
use crate::edge::{EdgeClient, EdgeRequest};
use crate::encryption::Encryption;
use crate::errors::{PolyError, RegistryError};
use crate::events::{EventBus, PolyEvent};
use crate::offline_queue::{OfflineQueue, QueuedOperation, QueuedOperationKind};
use crate::persistence::{LocalStore, Record, VersionRow};
use crate::registry::{EntityConfig, Registry, WritePath};
use crate::sync::echo::EchoTracker;
use crate::sync::pull::Puller;
use crate::sync::push::Pusher;
use crate::sync::reconcile::{ReconcilePlan, VersionTriple, make_plan};

/// Top-level coordinator. Cheap to clone (`Arc`).
///
/// The coordinator owns the network side (PostgREST + Edge dispatch + echo tracker), the
/// offline queue, and a [`LocalStore`] handle. It is the single source of truth for the
/// "write local, then push to network, retry on failure" lifecycle that all synced entities
/// share — consumers should NOT touch `LocalStore` directly for writes.
#[derive(Clone)]
pub struct Coordinator {
    inner: Arc<CoordinatorInner>,
}

struct CoordinatorInner {
    registry: Arc<Registry>,
    sessions: SessionStore,
    pusher: Pusher,
    puller: Puller,
    edge: EdgeClient,
    queue: Arc<dyn OfflineQueue>,
    events: EventBus,
    echo: EchoTracker,
    encryption: Option<Encryption>,
    local: Arc<dyn LocalStore>,
}

impl Coordinator {
    /// Build a coordinator from already-constructed pieces. Encryption is optional; when
    /// `None`, columns flagged `encrypted = true` are pushed and pulled in plaintext.
    pub fn new(
        client: Client,
        sessions: SessionStore,
        registry: Arc<Registry>,
        queue: Arc<dyn OfflineQueue>,
        events: EventBus,
        encryption: Option<Encryption>,
        local: Arc<dyn LocalStore>,
    ) -> Self {
        let echo = EchoTracker::with_default_window();
        let pusher = Pusher::new(client.clone(), echo.clone());
        let puller = Puller::new(client.clone());
        let edge = EdgeClient::new(client);
        Self {
            inner: Arc::new(CoordinatorInner {
                registry,
                sessions,
                pusher,
                puller,
                edge,
                queue,
                events,
                echo,
                encryption,
                local,
            }),
        }
    }

    /// Borrow the [`LocalStore`] for read-side queries. All write-side operations should go
    /// through [`Coordinator::persist_change`] / [`Coordinator::delete`] so the local mirror
    /// and the offline queue stay coherent.
    pub fn local(&self) -> &Arc<dyn LocalStore> {
        &self.inner.local
    }

    /// Borrow the [`EventBus`] for subscribing to sync/auth/realtime/queue events.
    pub fn events(&self) -> &EventBus {
        &self.inner.events
    }

    /// Borrow the echo tracker (used by the realtime subscriber to suppress self-rebound events).
    #[allow(dead_code)] // wired up when the realtime transport ships.
    pub(crate) fn echo_tracker(&self) -> &EchoTracker {
        &self.inner.echo
    }

    /// Borrow the entity registry.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.inner.registry
    }

    /// Look up the registered [`WritePath`] for a table without dispatching anything. Useful
    /// for diagnostics and consumers that want to make routing decisions ahead of time.
    pub fn write_path_for(&self, table: &str) -> Option<WritePath> {
        self.inner.registry.config_for_table(table).map(|c| c.write_path)
    }

    /// Persist a change using the entity's registered default Edge op (or PostgREST upsert,
    /// when the entity is registered with `WritePath::PostgREST`).
    ///
    /// Equivalent to [`Self::persist_change_with_op`] with `op = None`.
    pub async fn persist_change(
        &self,
        table: &str,
        record: Map<String, Value>,
    ) -> Result<(), PolyError> {
        self.persist_change_with_op(table, record, None).await
    }

    /// Persist a change for the given table. The `record` map must include the `id` field; the
    /// other fields should follow the registered REMOTE column shape for the table — the
    /// coordinator will translate to local column names before writing the local mirror.
    ///
    /// Lifecycle (single source of truth — consumers should NOT touch [`LocalStore`] directly
    /// for writes):
    /// 1. If the entity is registered with `include_user_id = true`, injects the active user's
    ///    id under the configured `user_id_column` (default `user_id`).
    /// 2. If an [`Encryption`] is wired in and the entity has columns flagged `encrypted = true`,
    ///    encrypts the matching string fields in place. The local mirror stores the ENCRYPTED
    ///    value (matches the Tauri Prism contract).
    /// 3. Maps remote → local column names and writes to the local mirror first. If the local
    ///    write fails, the network call is NOT attempted and the error propagates.
    /// 4. Dispatches to the network per the registered [`WritePath`]:
    ///    - `WritePath::PostgREST` → [`crate::sync::push::Pusher`] upsert.
    ///    - `WritePath::Edge { function, default_op }` → [`EdgeClient`] call to
    ///      `{function}/v1/{op}`. The op is taken from `op` when supplied, falling back to
    ///      `default_op`. PostgREST entities ignore `op`.
    /// 5. Either dispatch path marks the echo tracker BEFORE the network call (critical
    ///    ordering — see [`EchoTracker`]).
    /// 6. On transient failure (5xx, transport, forbidden), the operation is enqueued for
    ///    offline replay and `Ok(())` is returned. On permanent failure, the error is
    ///    returned and the operation is NOT enqueued. Either way, the local mirror has
    ///    already absorbed the change so reads remain consistent.
    pub async fn persist_change_with_op(
        &self,
        table: &str,
        mut record: Map<String, Value>,
        op: Option<&str>,
    ) -> Result<(), PolyError> {
        let config =
            self.inner.registry.config_for_table(table).ok_or_else(|| {
                PolyError::Registry(RegistryError::TableNotRegistered(table.into()))
            })?;
        let entity_id = record
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                PolyError::Registry(RegistryError::MissingField {
                    table: table.into(),
                    field: "id".into(),
                })
            })?
            .to_string();
        let session = self.inner.sessions.current().await.ok_or(PolyError::NoSession)?;

        if config.include_user_id
            && let Some(column) = config.user_id_column
            && !record.contains_key(column)
        {
            record.insert(column.into(), Value::String(session.user_id.clone()));
        }

        if let Some(enc) = self.inner.encryption.as_ref() {
            encrypt_record_columns(&config, &mut record, enc, &session.user_id)?;
        }

        let local_record = config.map_remote_to_local(&record);
        self.inner.local.upsert_record(table, local_record).await?;

        let result = match config.write_path {
            WritePath::PostgREST => {
                self.inner.pusher.upsert(table, record.clone(), &session.access_token).await
            }
            WritePath::Edge { function, default_op } => {
                let chosen_op = op.unwrap_or(default_op);
                let req =
                    EdgeRequest::new(function, Value::Object(record.clone())).with_op(chosen_op);
                self.inner.echo.mark_pushed(table, &entity_id);
                self.inner.edge.call::<Value, Value>(req, &session.access_token).await.map(|_| ())
            }
        };

        match result {
            Ok(()) => {
                if table == crate::kvs::KVS_TABLE {
                    self.publish_kvs_event(&record, false);
                }
                Ok(())
            }
            Err(err) if is_transient(&err) => {
                let queued = QueuedOperation {
                    table: table.into(),
                    entity_id,
                    kind: QueuedOperationKind::Write { payload: Value::Object(record.clone()) },
                    queued_at_micros: now_micros(),
                    retry_count: 0,
                    last_error: Some(err.to_string()),
                };
                self.inner.queue.enqueue(queued).await?;
                let depth = self.inner.queue.depth().await.unwrap_or(0);
                self.inner
                    .events
                    .publish(PolyEvent::OfflineQueueChanged { depth, in_flight: false });
                if table == crate::kvs::KVS_TABLE {
                    self.publish_kvs_event(&record, false);
                }
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn publish_kvs_event(&self, record: &Map<String, Value>, deleted: bool) {
        let namespace = record.get("namespace").and_then(Value::as_str).map(str::to_owned);
        let key = record.get("key").and_then(Value::as_str).map(str::to_owned);
        if let (Some(namespace), Some(key)) = (namespace, key) {
            self.inner.events.publish(PolyEvent::KvsChanged { namespace, key, deleted });
        }
    }

    /// Decrypt a remote-shaped row in place using the registered encryption columns.
    ///
    /// Use this on every row pulled from PostgREST or delivered via realtime BEFORE handing
    /// it to a [`crate::registry::FactoryFn`] or merging it into the local mirror. No-op when
    /// the coordinator was constructed without an [`Encryption`] or the table is unregistered.
    pub fn decode_remote_record(
        &self,
        table: &str,
        record: &mut Map<String, Value>,
        user_id: &str,
    ) -> Result<(), PolyError> {
        let Some(enc) = self.inner.encryption.as_ref() else {
            return Ok(());
        };
        let Some(config) = self.inner.registry.config_for_table(table) else {
            return Ok(());
        };
        decrypt_record_columns(&config, record, enc, user_id)
    }

    /// Build a [`ReconcilePlan`] for `table` by combining caller-supplied local versions with a
    /// fresh remote `read_versions` probe. The actual pull / push execution is left to the
    /// caller (which owns the local store) — this method is the orchestration boundary that
    /// hides the puller and planner internals from consumers.
    pub async fn reconcile_plan_for(
        &self,
        table: &str,
        local_versions: &[VersionTriple],
    ) -> Result<ReconcilePlan, PolyError> {
        let _config =
            self.inner.registry.config_for_table(table).ok_or_else(|| {
                PolyError::Registry(RegistryError::TableNotRegistered(table.into()))
            })?;
        let session = self.inner.sessions.current().await.ok_or(PolyError::NoSession)?;
        let remote_rows = self.inner.puller.read_versions(table, &session.access_token).await?;
        let remote: Vec<VersionTriple> = remote_rows
            .into_iter()
            .filter_map(|row| {
                let map = row.as_object()?;
                let id = map.get("id")?.as_str()?.to_string();
                let version = map.get("version").and_then(Value::as_i64).unwrap_or(0);
                let deleted = map.get("deleted").and_then(Value::as_bool).unwrap_or(false);
                Some(VersionTriple { id, version, deleted })
            })
            .collect();
        Ok(make_plan(local_versions, &remote))
    }

    /// Soft-delete via tombstone UPDATE (PostgREST) or via Edge Function delete op.
    ///
    /// Local mirror is marked deleted FIRST so reads stay consistent before the network
    /// round-trip. Transient network failures enqueue a tombstone op for replay; permanent
    /// failures propagate (and the local row remains tombstoned, which is the correct
    /// final state since the user explicitly asked to delete).
    pub async fn delete(
        &self,
        table: &str,
        entity_id: &str,
        version: i64,
    ) -> Result<(), PolyError> {
        let config =
            self.inner.registry.config_for_table(table).ok_or_else(|| {
                PolyError::Registry(RegistryError::TableNotRegistered(table.into()))
            })?;
        let session = self.inner.sessions.current().await.ok_or(PolyError::NoSession)?;
        let updated_at = chrono::Utc::now().to_rfc3339();

        self.inner.local.mark_deleted(table, entity_id, version).await?;

        let result = match config.write_path {
            WritePath::PostgREST => {
                self.inner
                    .pusher
                    .update_tombstone(table, entity_id, version, &updated_at, &session.access_token)
                    .await
            }
            WritePath::Edge { function, .. } => {
                let payload = serde_json::json!({
                    "id": entity_id,
                    "version": version,
                    "deleted": true,
                });
                let req = EdgeRequest::new(function, payload).with_op("delete");
                self.inner.echo.mark_pushed(table, entity_id);
                self.inner.edge.call::<Value, Value>(req, &session.access_token).await.map(|_| ())
            }
        };

        match result {
            Ok(()) => {
                if table == crate::kvs::KVS_TABLE
                    && let Some((namespace, key)) = crate::kvs::decode_id(entity_id)
                {
                    self.inner.events.publish(PolyEvent::KvsChanged {
                        namespace: namespace.into(),
                        key: key.into(),
                        deleted: true,
                    });
                }
                Ok(())
            }
            Err(err) if is_transient(&err) => {
                let queued = QueuedOperation {
                    table: table.into(),
                    entity_id: entity_id.into(),
                    kind: QueuedOperationKind::Tombstone { version },
                    queued_at_micros: now_micros(),
                    retry_count: 0,
                    last_error: Some(err.to_string()),
                };
                self.inner.queue.enqueue(queued).await?;
                let depth = self.inner.queue.depth().await.unwrap_or(0);
                self.inner
                    .events
                    .publish(PolyEvent::OfflineQueueChanged { depth, in_flight: false });
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Apply a remote-shaped row to the local mirror. Used by realtime delivery and pull
    /// merge paths. Decrypts encrypted columns first, then maps remote → local field names
    /// before calling `LocalStore::upsert_record`. Emits `KvsChanged` for the `kvs` table.
    pub async fn apply_remote_record(
        &self,
        table: &str,
        mut record: Map<String, Value>,
        user_id: &str,
    ) -> Result<(), PolyError> {
        let config =
            self.inner.registry.config_for_table(table).ok_or_else(|| {
                PolyError::Registry(RegistryError::TableNotRegistered(table.into()))
            })?;

        if let Some(enc) = self.inner.encryption.as_ref() {
            decrypt_record_columns(&config, &mut record, enc, user_id)?;
        }

        let deleted = record.get("deleted").and_then(Value::as_bool).unwrap_or(false);
        let local_record = config.map_remote_to_local(&record);
        self.inner.local.upsert_record(table, local_record).await?;

        if table == crate::kvs::KVS_TABLE {
            self.publish_kvs_event(&record, deleted);
        }
        Ok(())
    }

    /// Convenience: read a row out of the local mirror.
    pub async fn read_record(&self, table: &str, id: &str) -> Result<Option<Record>, PolyError> {
        self.inner.local.read_record(table, id).await
    }

    /// Convenience: read all ids out of the local mirror (used for reconcile prep).
    pub async fn read_all_ids(&self, table: &str) -> Result<Vec<String>, PolyError> {
        self.inner.local.read_all_ids(table).await
    }

    /// Convenience: read `(id, version, deleted)` triples for the given ids.
    pub async fn read_versions(
        &self,
        table: &str,
        ids: &[String],
    ) -> Result<Vec<VersionRow>, PolyError> {
        self.inner.local.read_versions(table, ids).await
    }
}

fn is_transient(err: &PolyError) -> bool {
    use crate::errors::{EdgeError, PushError};
    match err {
        PolyError::Edge(EdgeError::Transient { .. } | EdgeError::Forbidden { .. }) => true,
        PolyError::Push(PushError::Transient { .. }) => true,
        PolyError::Http(_) => true,
        _ => false,
    }
}

fn now_micros() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or_default()
}

fn encrypt_record_columns(
    config: &EntityConfig,
    record: &mut Map<String, Value>,
    enc: &Encryption,
    user_id: &str,
) -> Result<(), PolyError> {
    let user_uuid = Encryption::key_user_uuid(user_id);
    for column in &config.columns {
        if !column.encrypted {
            continue;
        }
        let key = column.remote_name.unwrap_or(column.canonical_name);
        if let Some(Value::String(text)) = record.get(key).cloned()
            && !enc.is_encrypted(&text)
        {
            let cipher = enc.encrypt(&text, user_uuid)?;
            record.insert(key.into(), Value::String(cipher));
        }
    }
    Ok(())
}

fn decrypt_record_columns(
    config: &EntityConfig,
    record: &mut Map<String, Value>,
    enc: &Encryption,
    user_id: &str,
) -> Result<(), PolyError> {
    let user_uuid = Encryption::key_user_uuid(user_id);
    for column in &config.columns {
        if !column.encrypted {
            continue;
        }
        let key = column.remote_name.unwrap_or(column.canonical_name);
        if let Some(Value::String(text)) = record.get(key).cloned()
            && enc.is_encrypted(&text)
        {
            let plain = enc.decrypt(&text, user_uuid)?;
            record.insert(key.into(), Value::String(plain));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use parking_lot::Mutex as SyncMutex;

    use super::*;
    use crate::client::ClientConfig;
    use crate::offline_queue::MemoryQueue;
    use crate::persistence::NullLocalStore;
    use crate::registry::{ColumnDef, EntityConfig};
    use serde_json::json;

    fn make_client() -> Client {
        Client::new(ClientConfig {
            supabase_url: "https://example.supabase.co".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: None,
        })
        .unwrap()
    }

    fn registry_with_messages() -> Arc<Registry> {
        let registry = Registry::new();
        registry.register(
            EntityConfig::synced("messages", "Message")
                .columns([
                    ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
                    ColumnDef::synced("content", "content", "content", "TEXT", "string", true),
                    ColumnDef::synced("version", "version", "version", "INTEGER", "integer", false),
                ])
                .write_via_edge("messages-write", "send"),
        );
        Arc::new(registry)
    }

    /// Minimal in-memory `LocalStore` for coordinator tests — captures every write so the
    /// test can assert what the coordinator persisted before / instead of touching the network.
    #[derive(Debug, Default, Clone)]
    struct MemLocalStore {
        rows: Arc<SyncMutex<HashMap<(String, String), Record>>>,
        deleted: Arc<SyncMutex<HashMap<(String, String), i64>>>,
    }

    impl MemLocalStore {
        fn new() -> Self {
            Self::default()
        }

        fn snapshot(&self, table: &str, id: &str) -> Option<Record> {
            self.rows.lock().get(&(table.to_owned(), id.to_owned())).cloned()
        }

        fn deleted_version(&self, table: &str, id: &str) -> Option<i64> {
            self.deleted.lock().get(&(table.to_owned(), id.to_owned())).copied()
        }
    }

    #[async_trait]
    impl LocalStore for MemLocalStore {
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

        async fn read_record(&self, table: &str, id: &str) -> Result<Option<Record>, PolyError> {
            Ok(self.snapshot(table, id))
        }

        async fn read_all_ids(&self, table: &str) -> Result<Vec<String>, PolyError> {
            let guard = self.rows.lock();
            Ok(guard.keys().filter(|(t, _)| t == table).map(|(_, id)| id.clone()).collect())
        }

        async fn upsert_record(&self, table: &str, record: Record) -> Result<(), PolyError> {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| PolyError::Local("missing id".into()))?
                .to_owned();
            self.rows.lock().insert((table.to_owned(), id), record);
            Ok(())
        }

        async fn mark_deleted(&self, table: &str, id: &str, version: i64) -> Result<(), PolyError> {
            self.deleted.lock().insert((table.to_owned(), id.to_owned()), version);
            Ok(())
        }

        async fn hard_delete(&self, table: &str, id: &str) -> Result<(), PolyError> {
            self.rows.lock().remove(&(table.to_owned(), id.to_owned()));
            self.deleted.lock().remove(&(table.to_owned(), id.to_owned()));
            Ok(())
        }
    }

    fn coordinator_with_encryption(secret: Option<&str>) -> (Coordinator, SessionStore) {
        coordinator_with_local(secret, Arc::new(NullLocalStore))
    }

    fn coordinator_with_local(
        secret: Option<&str>,
        local: Arc<dyn LocalStore>,
    ) -> (Coordinator, SessionStore) {
        let client = make_client();
        let events = EventBus::new();
        let sessions = SessionStore::new(client.clone(), events.clone());
        let registry = registry_with_messages();
        let queue: Arc<dyn OfflineQueue> = Arc::new(MemoryQueue::new());
        let encryption = secret.map(|s| Encryption::new(s).unwrap());
        let coord =
            Coordinator::new(client, sessions.clone(), registry, queue, events, encryption, local);
        (coord, sessions)
    }

    #[tokio::test]
    async fn encrypt_helper_only_touches_registered_encrypted_columns() {
        let (coord, _) = coordinator_with_encryption(Some("secret"));
        let config = coord.inner.registry.config_for_table("messages").unwrap();
        let enc = coord.inner.encryption.as_ref().unwrap();

        let mut record = json!({
            "id": "m1",
            "content": "hello world",
            "version": 1,
        })
        .as_object()
        .unwrap()
        .clone();

        encrypt_record_columns(&config, &mut record, enc, "user-1").unwrap();

        let content = record["content"].as_str().unwrap();
        assert!(content.starts_with("enc:"), "expected encrypted content, got {content}");
        assert_eq!(record["id"].as_str().unwrap(), "m1");
        assert_eq!(record["version"].as_i64().unwrap(), 1);
    }

    #[tokio::test]
    async fn decrypt_helper_undoes_encrypt_helper() {
        let (coord, _) = coordinator_with_encryption(Some("secret"));
        let config = coord.inner.registry.config_for_table("messages").unwrap();
        let enc = coord.inner.encryption.as_ref().unwrap();

        let mut record = json!({
            "id": "m1",
            "content": "hello world",
            "version": 1,
        })
        .as_object()
        .unwrap()
        .clone();

        encrypt_record_columns(&config, &mut record, enc, "user-1").unwrap();
        decrypt_record_columns(&config, &mut record, enc, "user-1").unwrap();
        assert_eq!(record["content"].as_str().unwrap(), "hello world");
    }

    #[tokio::test]
    async fn encrypt_helper_skips_already_encrypted_values() {
        let (coord, _) = coordinator_with_encryption(Some("secret"));
        let config = coord.inner.registry.config_for_table("messages").unwrap();
        let enc = coord.inner.encryption.as_ref().unwrap();

        let user = "user-1";
        let pre_cipher = enc.encrypt("hello", Encryption::key_user_uuid(user)).unwrap();
        let mut record = json!({
            "id": "m1",
            "content": pre_cipher.clone(),
        })
        .as_object()
        .unwrap()
        .clone();

        encrypt_record_columns(&config, &mut record, enc, user).unwrap();
        assert_eq!(record["content"].as_str().unwrap(), pre_cipher);
    }

    #[tokio::test]
    async fn decode_remote_record_is_noop_without_encryption() {
        let (coord, _) = coordinator_with_encryption(None);
        let mut record = json!({
            "id": "m1",
            "content": "enc:doesnt_matter_no_engine",
        })
        .as_object()
        .unwrap()
        .clone();

        coord.decode_remote_record("messages", &mut record, "user-1").unwrap();
        assert_eq!(record["content"].as_str().unwrap(), "enc:doesnt_matter_no_engine");
    }

    #[tokio::test]
    async fn decode_remote_record_silently_skips_unknown_table() {
        let (coord, _) = coordinator_with_encryption(Some("secret"));
        let mut record = json!({"id": "x"}).as_object().unwrap().clone();
        coord.decode_remote_record("nope_not_registered", &mut record, "user-1").unwrap();
    }

    #[tokio::test]
    async fn reconcile_plan_for_unknown_table_errors_out() {
        let (coord, _) = coordinator_with_encryption(None);
        let err = coord.reconcile_plan_for("nope", &[]).await.unwrap_err();
        match err {
            PolyError::Registry(RegistryError::TableNotRegistered(_)) => {}
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_path_for_returns_registered_routing() {
        let (coord, _) = coordinator_with_encryption(None);
        let path = coord.write_path_for("messages").unwrap();
        assert!(matches!(path, WritePath::Edge { function: "messages-write", default_op: "send" }));

        // Register a PostgREST entity and confirm both routings co-exist.
        coord.inner.registry.register(
            EntityConfig::synced("kvs", "Kvs")
                .columns([ColumnDef::synced("id", "id", "id", "TEXT", "string", false)]),
        );
        let kvs = coord.write_path_for("kvs").unwrap();
        assert_eq!(kvs, WritePath::PostgREST);

        assert!(coord.write_path_for("not_registered").is_none());
    }

    fn live_session() -> SessionPayload {
        use std::time::{SystemTime, UNIX_EPOCH};
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default()
            + 3600;
        SessionPayload {
            user_id: "user-1".into(),
            access_token: "tok".into(),
            refresh_token: "rt".into(),
            expires_at: exp,
            updated_at: 0,
        }
    }

    use crate::auth::SessionPayload;

    #[tokio::test]
    async fn persist_change_writes_local_then_enqueues_when_network_unreachable() {
        let local = Arc::new(MemLocalStore::new());
        let (coord, sessions) = coordinator_with_local(None, local.clone());
        sessions.set_session(live_session()).await.unwrap();

        let record = json!({
            "id": "m1",
            "content": "hello",
            "version": 1,
        })
        .as_object()
        .unwrap()
        .clone();

        // No real network is available — example.supabase.co won't resolve. The coordinator
        // must still have written to local before the network call failed, and must enqueue
        // for replay rather than propagating the error.
        coord.persist_change("messages", record).await.unwrap();

        let local_row = local.snapshot("messages", "m1").expect("local row written first");
        assert_eq!(local_row["id"], json!("m1"));
        assert_eq!(local_row["content"], json!("hello"));

        assert!(coord.inner.queue.depth().await.unwrap() >= 1, "transient failure must enqueue");
    }

    #[tokio::test]
    async fn delete_marks_local_then_enqueues_when_network_unreachable() {
        let local = Arc::new(MemLocalStore::new());
        let (coord, sessions) = coordinator_with_local(None, local.clone());
        sessions.set_session(live_session()).await.unwrap();

        coord.delete("messages", "m1", 7).await.unwrap();
        assert_eq!(local.deleted_version("messages", "m1"), Some(7));
        assert!(coord.inner.queue.depth().await.unwrap() >= 1);
    }

    #[tokio::test]
    async fn apply_remote_record_decrypts_and_writes_local_with_local_field_names() {
        let local = Arc::new(MemLocalStore::new());
        let (coord, _sessions) = coordinator_with_local(Some("secret"), local.clone());

        // Build a remote-shaped record where `content` is encrypted by some other writer.
        let enc = coord.inner.encryption.as_ref().unwrap();
        let cipher = enc.encrypt("hello", Encryption::key_user_uuid("user-1")).unwrap();
        let mut remote = json!({
            "id": "m1",
            "content": cipher,
            "version": 4,
        })
        .as_object()
        .unwrap()
        .clone();
        // Ensure non-string keys survive.
        remote.insert("ignored_remote_only".into(), json!("nope"));

        coord.apply_remote_record("messages", remote, "user-1").await.unwrap();

        let local_row = local.snapshot("messages", "m1").expect("row written");
        assert_eq!(local_row["content"], json!("hello"));
        assert_eq!(local_row["version"], json!(4));
        assert!(local_row.get("ignored_remote_only").is_none());
    }

    #[tokio::test]
    async fn read_record_returns_what_persist_change_wrote() {
        let local = Arc::new(MemLocalStore::new());
        let (coord, sessions) = coordinator_with_local(None, local);
        sessions.set_session(live_session()).await.unwrap();

        let record = json!({
            "id": "m42",
            "content": "stored",
            "version": 1,
        })
        .as_object()
        .unwrap()
        .clone();
        coord.persist_change("messages", record).await.unwrap();

        let read = coord.read_record("messages", "m42").await.unwrap().expect("row present");
        assert_eq!(read["content"], json!("stored"));
    }
}
