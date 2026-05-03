//! Tauri command surface.
//!
//! These commands are deliberately thin shims over the `polybase` core. They are wired into the
//! Tauri runtime by the `Builder::build` method in `lib.rs`. App-specific commands (e.g.
//! `list_personas`, `list_messages_page`) stay in the host crate; this module only ships the
//! generic library surface.
//!
//! Most commands require the host app to have called `polybase_configure` and
//! `polybase_set_session` first. The plugin maintains state via `tauri::State<RuntimeHandle>`.
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
use polybase::storage::Bucket;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// State held by the Tauri plugin.
#[derive(Default)]
pub struct RuntimeHandle {
    inner: Arc<RwLock<RuntimeInner>>,
}

#[derive(Default)]
struct RuntimeInner {
    client: Option<Client>,
    sessions: Option<SessionStore>,
    encryption: Option<Encryption>,
    events: EventBus,
}

impl RuntimeHandle {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Errors surfaced to the JS layer as strings.
fn to_command_error<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

/// `polybase_configure` — initialize the client + session store + optional encryption from a
/// JSON configuration object provided by the frontend.
#[tauri::command]
pub async fn polybase_configure(
    config: ClientConfig,
    state: tauri::State<'_, RuntimeHandle>,
) -> Result<(), String> {
    let client = Client::new(config.clone()).map_err(to_command_error)?;
    let bus = EventBus::new();
    let sessions = SessionStore::new(client.clone(), bus.clone());
    let encryption = match &config.encryption_secret {
        Some(secret) => Some(Encryption::new(secret).map_err(to_command_error)?),
        None => None,
    };
    let mut guard = state.inner.write().await;
    guard.client = Some(client);
    guard.sessions = Some(sessions);
    guard.encryption = encryption;
    guard.events = bus;
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

/// `polybase_kvs_set` — write a single KVS row (PostgREST upsert).
#[derive(Debug, Deserialize)]
pub struct KvsSetArgs {
    pub namespace: String,
    pub key: String,
    pub value: serde_json::Value,
    pub version: i64,
}

#[tauri::command]
pub async fn polybase_kvs_set(_args: KvsSetArgs) -> Result<(), String> {
    // Wired up once a Coordinator instance is plumbed through `RuntimeHandle`. For now this is
    // a placeholder that returns a clear error if invoked.
    Err("polybase_kvs_set not yet wired (pending Coordinator wiring in plugin state)".into())
}

/// `polybase_kvs_delete` — soft-delete a KVS row.
#[derive(Debug, Deserialize)]
pub struct KvsDeleteArgs {
    pub namespace: String,
    pub key: String,
    pub version: i64,
}

#[tauri::command]
pub async fn polybase_kvs_delete(_args: KvsDeleteArgs) -> Result<(), String> {
    Err("polybase_kvs_delete not yet wired (pending Coordinator wiring in plugin state)".into())
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
    let (client, sessions) = {
        let guard = state.inner.read().await;
        let client = guard.client.clone().ok_or_else(|| "polybase not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (client, sessions)
    };
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
    let (client, sessions) = {
        let guard = state.inner.read().await;
        let client = guard.client.clone().ok_or_else(|| "polybase not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (client, sessions)
    };
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
    let (client, sessions) = {
        let guard = state.inner.read().await;
        let client = guard.client.clone().ok_or_else(|| "polybase not configured".to_string())?;
        let sessions =
            guard.sessions.clone().ok_or_else(|| "polybase not configured".to_string())?;
        (client, sessions)
    };
    let session = sessions.current().await.ok_or_else(|| "no active session".to_string())?;
    let bucket_name = client.config().storage_bucket_or_default().to_string();
    let bucket = Bucket::new(client, bucket_name);
    bucket.delete(&path, &session.access_token).await.map_err(to_command_error)
}
