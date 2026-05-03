//! Realtime subscriber over Phoenix WebSocket.
//!
//! This module is feature-gated behind `realtime` (default-on). The transport is intentionally
//! abstracted behind [`RealtimeTransport`] so we can later swap in `realtime-rs` without
//! changing the high-level subscriber.
//!
//! For now this module ships the trait + a typed event surface. The `PhoenixTransport` impl
//! (lifted from Tauri Prism's `src-tauri/src/realtime/mod.rs`) lands in a follow-up — the
//! current Tauri Prism implementation continues to work unchanged in the meantime.
//!
//! Crate-internal until the transport implementation ships.
#![allow(unreachable_pub, missing_docs, dead_code)]

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::events::RealtimeOp;

/// Decoded `postgres_changes` payload, schema-agnostic.
#[derive(Debug, Clone)]
pub struct RealtimeChange {
    pub table: String,
    pub op: RealtimeOp,
    pub record: Value,
    pub old_record: Option<Value>,
}

/// Subscription handle. Drop to unsubscribe.
pub struct Subscription {
    rx: broadcast::Receiver<RealtimeChange>,
}

impl Subscription {
    pub fn new(rx: broadcast::Receiver<RealtimeChange>) -> Self {
        Self { rx }
    }

    /// Wait for the next change. Returns `None` if the underlying channel closes.
    pub async fn next(&mut self) -> Option<RealtimeChange> {
        loop {
            match self.rx.recv().await {
                Ok(change) => return Some(change),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

/// Pluggable transport for realtime delivery.
///
/// The default implementation in a future patch will be `PhoenixTransport`, which speaks the same
/// `realtime/v1/websocket` Phoenix protocol that Supabase exposes. Apps can ship custom impls
/// (e.g. polling, WebTransport) by implementing this trait.
#[async_trait]
pub trait RealtimeTransport: Send + Sync {
    /// Connect (or reconnect) and subscribe to `postgres_changes` on `tables`.
    async fn subscribe(
        &self,
        tables: &[&str],
        access_token: &str,
    ) -> Result<broadcast::Receiver<RealtimeChange>, RealtimeError>;
}

/// Error class for the realtime layer.
#[derive(Debug, thiserror::Error, Clone)]
pub enum RealtimeError {
    #[error("realtime connect failed: {0}")]
    ConnectFailed(String),
    #[error("realtime subscribe failed: {0}")]
    SubscribeFailed(String),
    #[error("realtime channel closed")]
    Closed,
}
