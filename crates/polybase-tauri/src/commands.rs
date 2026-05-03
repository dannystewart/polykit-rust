//! Tauri command surface.
//!
//! These commands are deliberately thin shims over the `polybase` core. They are wired into the
//! Tauri runtime by the [`crate::Builder::build`] method in `lib.rs`. App-specific commands
//! (e.g. `list_personas`, `list_messages_page`) stay in the host crate; this module only ships
//! the generic library surface.
//!
//! Lifecycle (host responsibility):
//! 1. Register the plugin with `tauri::Builder::default().plugin(polybase_tauri::Builder::new().build())`.
//! 2. In your `.setup(|app| { ... })`, build a `LocalStore` and an `OfflineQueue`, then call
//!    [`RuntimeHandle::attach`] on the plugin's state so the polybase coordinator has them.
//! 3. From JS, call `polybase_configure` once on app start with the Supabase URL, anon key,
//!    optional encryption secret, and storage bucket.
//! 4. From JS, call `polybase_set_session` whenever supabase-js issues a fresh session (sign-in,
//!    refresh, account switch).
//!
//! `unreachable_pub` is allowed at the file level: `#[tauri::command]` functions must be `pub`
//! for the macro's generated handler to find them, but they are not part of the Rust public API
//! consumers should call directly — they are invoked from JS via `invoke()`.
#![allow(unreachable_pub)]
#![allow(missing_docs)]

use std::sync::Arc;

use polybase::auth::{SessionPayload, SessionStore};
use polybase::client::{Client, ClientConfig};
use polybase::edge::{EdgeClient, EdgeRequest};
use polybase::encryption::Encryption;
use polybase::events::EventBus;
use polybase::kvs::Kvs;
use polybase::offline_queue::OfflineQueue;
use polybase::persistence::LocalStore;
use polybase::registry::Registry;
use polybase::storage::{Bucket, ListOptions, ListSort, ObjectEntry};
use polybase::sync::Coordinator;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// State held by the Tauri plugin.
///
/// Construct via [`RuntimeHandle::default`] when registering the plugin, then call
/// [`RuntimeHandle::attach`] during your `.setup(|app| { ... })` to plug in the `LocalStore`
/// and `OfflineQueue` implementations.
#[derive(Default)]
pub struct RuntimeHandle {
    pub(crate) inner: Arc<RwLock<RuntimeInner>>,
}

#[derive(Default)]
pub(crate) struct RuntimeInner {
    pub(crate) client: Option<Client>,
    pub(crate) sessions: Option<SessionStore>,
    pub(crate) encryption: Option<Encryption>,
    pub(crate) events: EventBus,
    pub(crate) registry: Arc<Registry>,
    pub(crate) local: Option<Arc<dyn LocalStore>>,
    pub(crate) queue: Option<Arc<dyn OfflineQueue>>,
    pub(crate) coordinator: Option<Coordinator>,
    pub(crate) kvs: Option<Kvs>,
}

impl RuntimeHandle {
    /// Build an empty runtime handle. The plugin uses this internally; consumers don't
    /// normally need to construct it directly.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the host-provided [`LocalStore`] and [`OfflineQueue`] implementations. Must
    /// be called BEFORE `polybase_configure` so the resulting coordinator has them in hand.
    /// Calling more than once replaces the previous attachment (useful for tests).
    pub async fn attach(&self, local: Arc<dyn LocalStore>, queue: Arc<dyn OfflineQueue>) {
        let mut guard = self.inner.write().await;
        guard.local = Some(local);
        guard.queue = Some(queue);
    }

    /// Borrow the shared [`EventBus`] so the host app can spawn its own subscribers
    /// (typically [`crate::EventForwarder::spawn`] to relay events to JS).
    pub async fn events(&self) -> EventBus {
        self.inner.read().await.events.clone()
    }

    /// Replace the entity registry. Use this if your app needs a custom registration set
    /// beyond the built-in KVS entity. The default registry already has KVS registered.
    pub async fn set_registry(&self, registry: Arc<Registry>) {
        self.inner.write().await.registry = registry;
    }

    /// Borrow the active [`Coordinator`] if one has been built (i.e. after both `attach`
    /// and `polybase_configure` have completed).
    pub async fn coordinator(&self) -> Option<Coordinator> {
        self.inner.read().await.coordinator.clone()
    }
}

fn to_command_error<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// `polybase_configure` — initialize the client + session store + optional encryption from a
/// JSON configuration object provided by the frontend. Builds the [`Coordinator`] as a side
/// effect IF [`RuntimeHandle::attach`] has already supplied the LocalStore + OfflineQueue.
#[tauri::command]
pub async fn polybase_configure(
    config: ClientConfig,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let client = Client::new(config.clone()).map_err(to_command_error)?;
    let encryption = match &config.encryption_secret {
        Some(secret) => Some(Encryption::new(secret).map_err(to_command_error)?),
        None => None,
    };

    let mut guard = state.inner.write().await;
    let bus = std::mem::take(&mut guard.events);
    let sessions = SessionStore::new(client.clone(), bus.clone());

    // Make sure KVS is registered so polybase_kvs_* commands work out of the box.
    Kvs::register(&guard.registry);

    let coordinator = match (guard.local.clone(), guard.queue.clone()) {
        (Some(local), Some(queue)) => {
            let coord = Coordinator::new(
                client.clone(),
                sessions.clone(),
                guard.registry.clone(),
                queue,
                bus.clone(),
                encryption.clone(),
                local.clone(),
            );
            // Bridge LocalStore::switch_user into session-changed events so per-user mirrors
            // swap automatically on sign-in / account switch.
            let local_for_hook = local.clone();
            sessions.on_user_changed(move |new_user| {
                let local = local_for_hook.clone();
                let user_id = new_user.unwrap_or("").to_owned();
                tokio::spawn(async move {
                    let _ = local.switch_user(&user_id).await;
                });
            });
            Some(coord)
        }
        _ => None,
    };

    guard.client = Some(client);
    guard.sessions = Some(sessions);
    guard.encryption = encryption;
    guard.events = bus;
    guard.kvs = coordinator.as_ref().map(|c| Kvs::new(c.clone()));
    guard.coordinator = coordinator;
    Ok(())
}

/// `polybase_set_session` — accept a fresh session payload from supabase-js.
#[tauri::command]
pub async fn polybase_set_session(
    payload: SessionPayload,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let sessions = state
        .inner
        .read()
        .await
        .sessions
        .clone()
        .ok_or_else(|| "polybase not configured".to_string())?;
    sessions.set_session(payload).await.map_err(to_command_error)?;
    Ok(())
}

/// `polybase_clear_session` — sign-out from Rust's perspective.
#[tauri::command]
pub async fn polybase_clear_session(state: tauri::State<'_, RuntimeHandle>) -> Result<(), String> {
    let sessions = state
        .inner
        .read()
        .await
        .sessions
        .clone()
        .ok_or_else(|| "polybase not configured".to_string())?;
    sessions.clear_session().await.map_err(to_command_error)?;
    Ok(())
}

/// `polybase_current_session` — read the active session payload from Rust. Used by the JS
/// layer to bootstrap UI state without reaching into supabase-js's storage directly.
#[tauri::command]
pub async fn polybase_current_session(
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<Option<SessionPayload>, String> {
    let sessions = state
        .inner
        .read()
        .await
        .sessions
        .clone()
        .ok_or_else(|| "polybase not configured".to_string())?;
    Ok(sessions.current().await)
}

/// `polybase_edge_call` — generic Edge Function call used for any `*-write` function.
#[derive(Debug, Deserialize)]
pub struct EdgeCallArgs {
    pub function: String,
    pub op: Option<String>,
    pub payload: serde_json::Value,
    pub idempotency_key: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EdgeCallResult {
    pub data: serde_json::Value,
    pub request_id: Option<String>,
}

#[tauri::command]
pub async fn polybase_edge_call(
    args: EdgeCallArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<EdgeCallResult, String> {
    let (client, sessions) = {
        let guard = state.inner.read().await;
        let client = guard.client.clone().ok_or_else(|| "polybase not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (client, sessions)
    };
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let mut req = EdgeRequest::new(args.function, args.payload);
    if let Some(op) = args.op {
        req = req.with_op(op);
    }
    if let Some(key) = args.idempotency_key {
        req = req.with_idempotency_key(key);
    }
    if let Some(id) = args.request_id {
        req = req.with_request_id(id);
    }
    let edge = EdgeClient::new(client);
    let result = edge
        .call::<serde_json::Value, serde_json::Value>(req, &session.access_token)
        .await
        .map_err(to_command_error)?;
    Ok(EdgeCallResult { data: result.data, request_id: result.request_id })
}

/// `polybase_encrypt` — encrypt a string with the configured encryption secret for the current user.
#[tauri::command]
pub async fn polybase_encrypt(
    plaintext: String,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let (encryption, sessions) = {
        let guard = state.inner.read().await;
        let encryption =
            guard.encryption.clone().ok_or_else(|| "encryption not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (encryption, sessions)
    };
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let user_uuid = Encryption::key_user_uuid(&session.user_id);
    encryption.encrypt(&plaintext, user_uuid).map_err(to_command_error)
}

/// `polybase_decrypt` — decrypt a string with the configured encryption secret for the current user.
#[tauri::command]
pub async fn polybase_decrypt(
    ciphertext: String,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let (encryption, sessions) = {
        let guard = state.inner.read().await;
        let encryption =
            guard.encryption.clone().ok_or_else(|| "encryption not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (encryption, sessions)
    };
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let user_uuid = Encryption::key_user_uuid(&session.user_id);
    encryption.decrypt(&ciphertext, user_uuid).map_err(to_command_error)
}

/// `polybase_kvs_get` — read a single KVS row from the local mirror. Returns `null` when the
/// key is unset or tombstoned.
#[derive(Debug, Deserialize)]
pub struct KvsGetArgs {
    pub namespace: String,
    pub key: String,
}

#[tauri::command]
pub async fn polybase_kvs_get(
    args: KvsGetArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<Option<serde_json::Value>, String> {
    let kvs = state
        .inner
        .read()
        .await
        .kvs
        .clone()
        .ok_or_else(|| "polybase coordinator not attached".to_string())?;
    kvs.get::<serde_json::Value>(&args.namespace, &args.key).await.map_err(to_command_error)
}

/// `polybase_kvs_set` — write a single KVS row (PostgREST upsert). The next version is
/// derived from the local mirror by [`polybase::Kvs::set`]; callers do not pass a version.
#[derive(Debug, Deserialize)]
pub struct KvsSetArgs {
    pub namespace: String,
    pub key: String,
    pub value: serde_json::Value,
}

#[tauri::command]
pub async fn polybase_kvs_set(
    args: KvsSetArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let kvs = state
        .inner
        .read()
        .await
        .kvs
        .clone()
        .ok_or_else(|| "polybase coordinator not attached".to_string())?;
    kvs.set(&args.namespace, &args.key, &args.value).await.map_err(to_command_error)
}

/// `polybase_kvs_delete` — soft-delete a KVS row. Version is derived from the local mirror.
#[derive(Debug, Deserialize)]
pub struct KvsDeleteArgs {
    pub namespace: String,
    pub key: String,
}

#[tauri::command]
pub async fn polybase_kvs_delete(
    args: KvsDeleteArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let kvs = state
        .inner
        .read()
        .await
        .kvs
        .clone()
        .ok_or_else(|| "polybase coordinator not attached".to_string())?;
    kvs.delete(&args.namespace, &args.key).await.map_err(to_command_error)
}

/// `polybase_storage_upload` — upload bytes to the configured bucket.
#[derive(Debug, Deserialize)]
pub struct StorageUploadArgs {
    pub path: String,
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub upsert: bool,
}

#[tauri::command]
pub async fn polybase_storage_upload(
    args: StorageUploadArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let (client, sessions) = require_client_and_session(&state).await?;
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    bucket
        .upload(
            &args.path,
            args.bytes.into(),
            &args.content_type,
            &session.access_token,
            args.upsert,
        )
        .await
        .map_err(to_command_error)
}

/// `polybase_storage_download` — fetch bytes from the configured bucket.
#[tauri::command]
pub async fn polybase_storage_download(
    path: String,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<Vec<u8>, String> {
    let (client, sessions) = require_client_and_session(&state).await?;
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    let bytes = bucket.download(&path, &session.access_token).await.map_err(to_command_error)?;
    Ok(bytes.to_vec())
}

/// `polybase_storage_delete` — delete an object from the configured bucket.
#[tauri::command]
pub async fn polybase_storage_delete(
    path: String,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let (client, sessions) = require_client_and_session(&state).await?;
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    bucket.delete(&path, &session.access_token).await.map_err(to_command_error)
}

/// `polybase_storage_list` — list objects under a prefix in the configured bucket.
#[derive(Debug, Deserialize, Default)]
pub struct StorageListArgs {
    pub prefix: String,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort_column: Option<String>,
    #[serde(default)]
    pub sort_order: Option<String>,
}

#[tauri::command]
pub async fn polybase_storage_list(
    args: StorageListArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<Vec<ObjectEntry>, String> {
    let (client, sessions) = require_client_and_session(&state).await?;
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    let sort_by = args
        .sort_column
        .map(|column| ListSort::new(column, args.sort_order.unwrap_or_else(|| "asc".into())));
    let options =
        ListOptions { limit: args.limit, offset: args.offset, search: args.search, sort_by };
    bucket.list(&args.prefix, options, &session.access_token).await.map_err(to_command_error)
}

/// `polybase_storage_signed_url` — mint a time-limited signed URL for a private object.
#[derive(Debug, Deserialize)]
pub struct StorageSignedUrlArgs {
    pub path: String,
    pub expires_in_seconds: u32,
}

#[tauri::command]
pub async fn polybase_storage_signed_url(
    args: StorageSignedUrlArgs,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<String, String> {
    let (client, sessions) = require_client_and_session(&state).await?;
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    bucket
        .create_signed_url(&args.path, args.expires_in_seconds, &session.access_token)
        .await
        .map_err(to_command_error)
}

async fn require_client_and_session(
    state: &tauri::State<'_, RuntimeHandle>,
) -> Result<(Client, SessionStore), String> {
    let guard = state.inner.read().await;
    let client = guard.client.clone().ok_or_else(|| "polybase not configured".to_string())?;
    let sessions = guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
    Ok((client, sessions))
}
