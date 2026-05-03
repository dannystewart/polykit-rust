//! Observable events broadcast across the polybase runtime.
//!
//! The runtime publishes [`PolyEvent`] values on a [`tokio::sync::broadcast`] channel. The
//! `polybase-tauri` plugin subscribes and emits Tauri events to the JS frontend; in-process Rust
//! code can subscribe directly to react to sync state changes.

use serde::Serialize;
use tokio::sync::broadcast;

/// Maximum number of buffered events before slow subscribers start lagging.
const CHANNEL_CAPACITY: usize = 256;

/// Discriminated union of all events emitted by the polybase runtime.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolyEvent {
    /// Session payload changed (sign-in, refresh, change of user, sign-out).
    SessionChanged {
        /// User id after the change. `None` when the session was cleared.
        user_id: Option<String>,
        /// What kind of session change occurred.
        change: SessionChangeKind,
    },

    /// A realtime postgres_changes event was received and ready for merge.
    RealtimeChanged {
        /// Table the change applies to.
        table: String,
        /// Primary key of the affected row.
        entity_id: String,
        /// Insert / update / delete.
        op: RealtimeOp,
    },

    /// Offline queue depth changed.
    OfflineQueueChanged {
        /// Number of operations currently waiting in the queue.
        depth: usize,
        /// True if a replay batch is currently being processed.
        in_flight: bool,
    },

    /// A reconcile pass started or completed for a table.
    ReconcileProgress {
        /// Table being reconciled.
        table: String,
        /// Phase of the reconcile pass.
        phase: ReconcilePhase,
        /// Per-action counts; populated only on `Completed`.
        action_counts: Option<ReconcileActionCounts>,
    },

    /// Bulk pull (initial bootstrap or refresh) progress.
    PullProgress {
        /// Table being pulled.
        table: String,
        /// Number of rows received so far.
        rows_received: usize,
        /// True when the pull has finished.
        complete: bool,
    },

    /// A KVS key changed (locally or via realtime).
    KvsChanged {
        /// Namespace the key lives under.
        namespace: String,
        /// Key name within the namespace.
        key: String,
        /// True for tombstone events.
        deleted: bool,
    },
}

/// What kind of change happened to the active session.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionChangeKind {
    /// New user signed in (no prior session, or different user).
    UserChanged,
    /// Same user, fresh access/refresh tokens.
    CredentialsRefreshed,
    /// Session cleared (sign-out).
    Cleared,
}

/// PostgREST operation kind for a realtime row event.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeOp {
    /// New row was inserted.
    Insert,
    /// Existing row was updated.
    Update,
    /// Row was deleted (or tombstoned, if soft-delete is in use).
    Delete,
}

/// Phase of a reconcile pass.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconcilePhase {
    /// Reconcile pass started.
    Started,
    /// Reconcile pass completed successfully.
    Completed,
    /// Reconcile pass failed.
    Failed,
}

/// How many rows fell into each [`crate::contract::ReconcileAction`] bucket.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
pub struct ReconcileActionCounts {
    /// Local rows that adopted the remote tombstone.
    pub adopt_tombstone: u32,
    /// Rows pulled from remote (newer remote version, including create-local).
    pub pull: u32,
    /// Rows pushed to remote (newer local version, including create-remote).
    pub push: u32,
    /// Rows that already matched on both sides.
    pub skip: u32,
}

/// Sender + receiver factory for [`PolyEvent`].
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<PolyEvent>,
}

impl EventBus {
    /// Create a new bus with the default capacity.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Publish an event to all subscribers. Drops silently when no subscribers are listening.
    pub fn publish(&self, event: PolyEvent) {
        let _ = self.tx.send(event);
    }

    /// Subscribe to future events.
    pub fn subscribe(&self) -> broadcast::Receiver<PolyEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_and_receive() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(PolyEvent::OfflineQueueChanged { depth: 3, in_flight: false });
        let event = rx.recv().await.unwrap();
        match event {
            PolyEvent::OfflineQueueChanged { depth, in_flight } => {
                assert_eq!(depth, 3);
                assert!(!in_flight);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn publish_with_no_subscribers_is_fine() {
        let bus = EventBus::new();
        bus.publish(PolyEvent::OfflineQueueChanged { depth: 0, in_flight: false });
    }
}
