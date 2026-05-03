//! PolyBase v2 (Rust) — hybrid Supabase library.
//!
//! Sibling to [PolyBase (Swift)](https://github.com/dannystewart/polykit-swift) but rebuilt around a
//! hybrid write-path: synced entities (chat data, etc.) flow through Supabase Edge Functions, while
//! preferences-style data (KVS), device tokens, and other lightweight rows go through PostgREST with
//! the user JWT. Reads, realtime, reconcile, storage, and encryption all live in Rust regardless.
//!
//! # Modules
//! - [`auth`] — JWT session management with refresh loop and observable state.
//! - [`client`] — Configured Supabase HTTP client (URL, anon key, headers).
//! - [`contract`] — Frozen semantic invariants (version steps, backoff ladder, echo window, etc.).
//! - [`edge`] — Typed Edge Function client with idempotency, structured errors, and retry hints.
//! - [`encryption`] — AES-256-GCM with HKDF-SHA256, wire-compatible with PolyBase Swift.
//! - [`errors`] — Top-level [`PolyError`] plus per-subsystem error types.
//! - [`events`] — Broadcast channels for sync/auth/realtime/queue state changes.
//! - [`kvs`] — Typed key-value rows replacing iCloud KVS-style preferences.
//! - [`offline_queue`] — Trait + reducer for persistent retry queue.
//! - [`persistence`] — [`LocalStore`] trait so the core does not depend on sqlx.
//! - [`registry`] — Entity registration, field maps, parent relations, write-path policy.
//! - [`storage`] — Supabase Storage REST adapter.
//! - [`sync`] — Coordinator, push (PostgREST + Edge), pull, reconcile, echo tracker.
//! - `realtime` (feature `realtime`) — Phoenix WebSocket subscriber for `postgres_changes`
//!   (crate-internal until the transport ships).

pub mod auth;
pub mod client;
pub mod contract;
pub mod edge;
pub mod encryption;
pub mod errors;
pub mod events;
pub mod kvs;
pub mod offline_queue;
pub mod persistence;
pub mod registry;
pub mod storage;
pub mod sync;

#[cfg(feature = "realtime")]
pub(crate) mod realtime;

pub use client::{Client, ClientConfig};
pub use errors::{
    EdgeError, OfflineQueueError, PolyError, PullError, PushError, RegistryError, StorageError,
};
pub use kvs::{Kvs, KvsChange, KvsRow};
pub use offline_queue::{MemoryQueue, OfflineQueue, QueuedOperation, QueuedOperationKind};
pub use persistence::{LocalStore, NullLocalStore, Record, VersionRow};
pub use registry::{ColumnDef, EntityConfig, ParentRelation, Registry, WritePath};
pub use sync::Coordinator;
