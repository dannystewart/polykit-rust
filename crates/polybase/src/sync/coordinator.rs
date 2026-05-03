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
use crate::registry::{EntityConfig, Registry, WritePath};
use crate::sync::echo::EchoTracker;
use crate::sync::pull::Puller;
use crate::sync::push::Pusher;
use crate::sync::reconcile::{ReconcilePlan, VersionTriple, make_plan};

/// Top-level coordinator. Cheap to clone (`Arc`).
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
            }),
        }
    }

    /// Borrow the echo tracker (used by the realtime subscriber to suppress self-rebound events).
    #[allow(dead_code)]
    pub(crate) fn echo_tracker(&self) -> &EchoTracker {
        &self.inner.echo
    }

    /// Borrow the entity registry.
    #[allow(dead_code)]
    pub(crate) fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    /// Borrow the event bus (used to subscribe to sync state changes).
    #[allow(dead_code)]
    pub(crate) fn events(&self) -> &EventBus {
        &self.inner.events
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
    /// other fields should follow the registered remote-column shape for the table.
    ///
    /// Pre-flight transformations the coordinator applies for you, in order:
    /// 1. If the entity is registered with `include_user_id = true`, injects the active user's
    ///    id under the configured `user_id_column` (default `user_id`).
    /// 2. If an [`Encryption`] is wired in and the entity has columns flagged `encrypted = true`,
    ///    encrypts the matching string fields (canonical or remote name) in place.
    ///
    /// Dispatch follows the registered [`WritePath`]:
    /// - `WritePath::PostgREST` → [`crate::sync::push::Pusher`] upsert.
    /// - `WritePath::Edge { function, default_op }` → [`EdgeClient`] call to
    ///   `{function}/v1/{op}`. The op is taken from `op` when supplied, falling back to
    ///   `default_op`. PostgREST entities ignore `op`.
    ///
    /// Either dispatch path marks the echo tracker BEFORE the network call (critical ordering —
    /// see [`EchoTracker`]). On transient failure (5xx, transport, forbidden), the operation
    /// is enqueued for offline replay and `Ok(())` is returned. On permanent failure, the
    /// error is returned and the operation is NOT enqueued.
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
            Ok(()) => Ok(()),
            Err(err) if is_transient(&err) => {
                let op = QueuedOperation {
                    table: table.into(),
                    entity_id,
                    kind: QueuedOperationKind::Write { payload: Value::Object(record) },
                    queued_at_micros: now_micros(),
                    retry_count: 0,
                    last_error: Some(err.to_string()),
                };
                self.inner.queue.enqueue(op).await?;
                let depth = self.inner.queue.depth().await.unwrap_or(0);
                self.inner
                    .events
                    .publish(PolyEvent::OfflineQueueChanged { depth, in_flight: false });
                Ok(())
            }
            Err(err) => Err(err),
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
        match config.write_path {
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
        }
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
    use super::*;
    use crate::client::ClientConfig;
    use crate::offline_queue::MemoryQueue;
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

    fn coordinator_with_encryption(secret: Option<&str>) -> (Coordinator, SessionStore) {
        let client = make_client();
        let events = EventBus::new();
        let sessions = SessionStore::new(client.clone(), events.clone());
        let registry = registry_with_messages();
        let queue: Arc<dyn OfflineQueue> = Arc::new(MemoryQueue::new());
        let encryption = secret.map(|s| Encryption::new(s).unwrap());
        let coord = Coordinator::new(client, sessions.clone(), registry, queue, events, encryption);
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
}
