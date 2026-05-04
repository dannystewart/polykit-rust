//! Sync engine: hybrid coordinator (Edge + PostgREST), echo tracker, push, pull, reconcile,
//! plus the public reducer / remote read-write protocols that consumers (Tauri Prism today,
//! future apps tomorrow) layer their own runtime on top of.
//!
//! Public surface:
//! - [`Coordinator`] for the high-level hybrid write path
//! - Reconcile planner ([`reconcile::ReconcilePlan`], [`reconcile::VersionTriple`],
//!   [`reconcile::make_plan`])
//! - Reducer ([`reducer::ReducerState`], [`reducer::ReducerEvent`], [`reducer::ReducerEffect`],
//!   [`reducer::step`]) — port of Swift PolyBase's `Sync/Core.swift`. Consumers feed events,
//!   honor effects, and layer their own UI display state on top.
//! - Remote read/write protocols ([`remote::RemoteWriter`], [`remote::RemoteReader`]) plus
//!   memory test impls and Supabase live impls — port of Swift PolyBase's
//!   `Sync/RemoteWriter.swift` + `Sync/RemoteReader.swift`.
//! - Push error message classifier ([`push::is_permanent_push_error_message`]) — Swift's exact
//!   10-pattern list. Use to decide whether a remote rejection should drop from the offline
//!   queue or be retried.
//!
//! Push, pull, and echo internals remain crate-internal — consumers drive them via the
//! Coordinator and never reach in directly.

pub mod coordinator;
pub mod echo;
pub(crate) mod pull;
pub mod push;
pub mod reconcile;
pub mod reducer;
pub mod remote;

pub use coordinator::Coordinator;
pub use echo::EchoTracker;
pub use push::is_permanent_push_error_message;
pub use reconcile::{ReconcilePlan, VersionTriple, make_plan};
pub use reducer::{ReducerEffect, ReducerEvent, ReducerState, step as reducer_step};
pub use remote::{
    MemoryReader, MemoryWriter, RemoteFilter, RemoteReader, RemoteWriter, SupabaseRemoteReader,
    SupabaseRemoteWriter,
};
