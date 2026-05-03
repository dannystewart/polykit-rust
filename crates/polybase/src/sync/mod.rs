//! Sync engine: hybrid coordinator (Edge + PostgREST), echo tracker, push, pull, reconcile.
//!
//! [`Coordinator`] and the reconcile planner ([`reconcile::ReconcilePlan`], [`reconcile::VersionTriple`],
//! [`reconcile::make_plan`]) are public. Push, pull, echo, and reducer internals remain
//! crate-internal — consumers drive them via the Coordinator and never reach in directly.

pub mod coordinator;
pub(crate) mod echo;
pub(crate) mod pull;
pub(crate) mod push;
pub mod reconcile;
pub(crate) mod reducer;

pub use coordinator::Coordinator;
pub use reconcile::{ReconcilePlan, VersionTriple, make_plan};
