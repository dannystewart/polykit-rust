//! PolyBase v2 (Rust) — hybrid Supabase library.
//!
//! Sibling to [PolyBase (Swift)](https://github.com/dannystewart/polykit-swift) but rebuilt around a
//! hybrid write-path: synced entities (chat data, etc.) flow through Supabase Edge Functions, while
//! preferences-style data (KVS), device tokens, and other lightweight rows go through PostgREST with
//! the user JWT. Reads, realtime, reconcile, storage, and encryption all live in Rust regardless.
//!
//! # Architecture
//!
//! Polybase is intentionally a layered library, not a framework. Each layer has one job and
//! talks to its neighbours through a small typed surface.
//!
//! ```text
//!  ┌─────────────────────────────────────────────────────────────────────────┐
//!  │ Host app (Tauri Prism, future apps, your code)                          │
//!  │   - Owns the UI, business logic, app-specific entities                  │
//!  │   - Drives [`Coordinator`] for persistence; subscribes to [`events`]    │
//!  └────────────────────────────┬────────────────────────────────────────────┘
//!                               │
//!  ┌────────────────────────────┴────────────────────────────────────────────┐
//!  │ [`Coordinator`] — single entry point for "save this row"                │
//!  │   Looks up [`Registry`] → [`WritePath`] → dispatches:                   │
//!  │     • [`WritePath::PostgREST`] → [`sync::push::Pusher::upsert`]         │
//!  │     • [`WritePath::Edge`]      → [`edge::EdgeClient::call`]             │
//!  │   Always writes [`LocalStore`] FIRST, then network. Failures enqueue    │
//!  │   into [`OfflineQueue`] for replay. Marks [`sync::echo`] before push    │
//!  │   to suppress realtime echoes of our own writes.                        │
//!  └──┬─────────────────────────────┬──────────────────────────┬─────────────┘
//!     │                             │                          │
//!  ┌──┴────────────┐  ┌─────────────┴────────────┐  ┌──────────┴───────────┐
//!  │ [`LocalStore`]│  │ [`Client`] + [`SessionStore`] │  │ [`OfflineQueue`]    │
//!  │   pluggable   │  │   reqwest + JWT + refresh     │  │   pluggable trait   │
//!  │ (polybase-    │  │   loop. user-changed hook.    │  │ (MemoryQueue +      │
//!  │  sqlite, or   │  │                               │  │  FileBackedQueue in │
//!  │  your own)    │  │                               │  │  tauri-plugin-      │
//!  │               │  │                               │  │  polybase)          │
//!  └───────────────┘  └───────────────────────────────┘  └─────────────────────┘
//! ```
//!
//! Built on top, the convenience modules:
//!
//! - [`Kvs`] — typed key-value rows on the `kvs` table (replaces iCloud KVS).
//! - [`encryption::Encryption`] — AES-256-GCM with HKDF-SHA256, wire-compatible with
//!   PolyBase Swift, applied transparently to columns marked `encrypted: true`.
//! - [`storage::Bucket`] — Supabase Storage REST adapter (upload, download, list, signed URLs).
//!
//! # Quickstart
//!
//! Polybase is a library, not a service — most apps wire it up through
//! [`tauri-plugin-polybase`](https://docs.rs/tauri-plugin-polybase) which handles the JS
//! command surface, `LocalStore` attach, event forwarding, and a default `FileBackedQueue`.
//! For non-Tauri or test code:
//!
//! ```no_run
//! use std::sync::Arc;
//! use polybase::{
//!     Client, ClientConfig, Coordinator, Kvs, MemoryQueue, NullLocalStore, Registry,
//!     auth::SessionStore, events::EventBus,
//! };
//!
//! # async fn build() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::new(ClientConfig {
//!     supabase_url: "https://xxxx.supabase.co".into(),
//!     supabase_anon_key: "anon-key".into(),
//!     encryption_secret: None,
//!     storage_bucket: None,
//! })?;
//! let bus = EventBus::default();
//! let sessions = SessionStore::new(client.clone(), bus.clone());
//! let registry = Arc::new(Registry::new());
//! Kvs::register(&registry); // idempotent; registers the built-in `kvs` entity.
//! let coord = Coordinator::new(
//!     client,
//!     sessions,
//!     registry,
//!     Arc::new(MemoryQueue::default()),
//!     bus,
//!     None,                          // encryption — None disables transparent crypto.
//!     Arc::new(NullLocalStore),      // swap with `polybase-sqlite::SqliteLocalStore` in real apps.
//! );
//! let kvs = Kvs::new(coord);
//! kvs.set("prism.settings", "show-archived-conversations", &true).await?;
//! # Ok(()) }
//! ```
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
