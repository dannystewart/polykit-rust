//! Offline queue trait + persistent operation type.
//!
//! polybase does not own queue persistence directly. The [`OfflineQueue`] trait lets apps plug
//! in any backing store (file on disk, SQLite table, Tauri Store, etc.). polybase-tauri ships a
//! file-backed default; tests use the in-memory implementation in [`MemoryQueue`].

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::contract::{ContractOperation, finalize_queue_after_processing};
use crate::errors::OfflineQueueError;

/// One pending operation persisted to the queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuedOperation {
    /// Table the operation targets.
    pub table: String,
    /// Primary key value within the table.
    pub entity_id: String,
    /// Whether this is a write, tombstone, or hard delete.
    pub kind: QueuedOperationKind,
    /// Monotonic enqueue timestamp in microseconds since epoch.
    pub queued_at_micros: i64,
    /// Number of replay attempts so far.
    #[serde(default)]
    pub retry_count: u32,
    /// Last error message recorded (for diagnostics).
    #[serde(default)]
    pub last_error: Option<String>,
}

impl QueuedOperation {
    /// Project to the contract-level tuple used by the dedupe / finalize helpers.
    pub fn to_contract(&self) -> ContractOperation {
        ContractOperation::new(self.table.clone(), self.entity_id.clone(), self.queued_at_micros)
    }
}

/// What sort of operation the queue is replaying. The payload itself is opaque JSON so the queue
/// can persist Edge Function payloads, PostgREST upsert rows, or tombstone updates uniformly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueuedOperationKind {
    /// Generic upsert or write. The runtime decides whether to dispatch via Edge or PostgREST
    /// by consulting the registry's [`crate::registry::WritePath`].
    Write {
        /// JSON payload to push (PostgREST row or Edge Function body).
        payload: serde_json::Value,
    },
    /// Tombstone update (just `version` + `deleted` + `updated_at`).
    Tombstone {
        /// Tombstone version to publish.
        version: i64,
    },
    /// Hard delete (rare; usually for non-synced tables).
    HardDelete,
}

/// Persistent queue interface. Implementations must:
/// - dedupe by `(table, entity_id)` (newer operation replaces older);
/// - preserve monotonic `queued_at_micros` per the contract;
/// - guarantee that `finalize_after_processing` does not drop concurrent enqueues.
#[async_trait]
pub trait OfflineQueue: Send + Sync {
    /// Append an operation, replacing any existing operation with the same `(table, entity_id)`.
    async fn enqueue(&self, op: QueuedOperation) -> Result<(), OfflineQueueError>;

    /// Snapshot all queued operations in priority order (oldest first) for replay.
    async fn snapshot(&self) -> Result<Vec<QueuedOperation>, OfflineQueueError>;

    /// Finalize a replay batch. `successful` ids are dropped; `failed` ids are kept (or
    /// re-inserted if a newer op for the same key arrived during processing).
    async fn finalize_after_processing(
        &self,
        snapshot: &[QueuedOperation],
        failed: &[QueuedOperation],
    ) -> Result<(), OfflineQueueError>;

    /// Current depth (best-effort; some implementations may approximate).
    async fn depth(&self) -> Result<usize, OfflineQueueError>;
}

/// In-memory queue for tests and single-process embedding. Cheap to clone (`Arc<Mutex<...>>`).
#[derive(Debug, Clone, Default)]
pub struct MemoryQueue {
    inner: Arc<Mutex<VecDeque<QueuedOperation>>>,
}

impl MemoryQueue {
    /// Build an empty in-memory queue.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl OfflineQueue for MemoryQueue {
    async fn enqueue(&self, op: QueuedOperation) -> Result<(), OfflineQueueError> {
        let mut guard = self.inner.lock();
        // Dedupe by (table, entity_id): drop any older entry, then push the new one at the back.
        guard
            .retain(|existing| !(existing.table == op.table && existing.entity_id == op.entity_id));
        guard.push_back(op);
        Ok(())
    }

    async fn snapshot(&self) -> Result<Vec<QueuedOperation>, OfflineQueueError> {
        Ok(self.inner.lock().iter().cloned().collect())
    }

    async fn finalize_after_processing(
        &self,
        snapshot: &[QueuedOperation],
        failed: &[QueuedOperation],
    ) -> Result<(), OfflineQueueError> {
        let mut guard = self.inner.lock();
        let current: Vec<ContractOperation> =
            guard.iter().map(QueuedOperation::to_contract).collect();
        let snapshot_contract: Vec<ContractOperation> =
            snapshot.iter().map(QueuedOperation::to_contract).collect();
        let failed_contract: Vec<ContractOperation> =
            failed.iter().map(QueuedOperation::to_contract).collect();

        let retained =
            finalize_queue_after_processing(&current, &snapshot_contract, &failed_contract);
        let retained_keys: std::collections::HashSet<(String, String, i64)> =
            retained.into_iter().map(|op| (op.table, op.entity_id, op.queued_at_micros)).collect();

        // Key by the full (table, entity_id, queued_at_micros) tuple so a concurrent enqueue that
        // happened to share `(table, entity_id)` with a failed op (but a newer queued_at_micros)
        // is *not* treated as a retry — it's a fresh enqueue and should keep retry_count = 0.
        let failed_by_full_key: std::collections::HashMap<(String, String, i64), Option<String>> =
            failed
                .iter()
                .map(|f| {
                    (
                        (f.table.clone(), f.entity_id.clone(), f.queued_at_micros),
                        f.last_error.clone(),
                    )
                })
                .collect();

        guard.retain_mut(|op| {
            let full_key = (op.table.clone(), op.entity_id.clone(), op.queued_at_micros);
            if !retained_keys.contains(&full_key) {
                return false;
            }
            if let Some(last_error) = failed_by_full_key.get(&full_key) {
                op.retry_count = op.retry_count.saturating_add(1);
                op.last_error = last_error.clone();
            }
            true
        });
        Ok(())
    }

    async fn depth(&self) -> Result<usize, OfflineQueueError> {
        Ok(self.inner.lock().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(table: &str, id: &str, queued_at_micros: i64) -> QueuedOperation {
        QueuedOperation {
            table: table.into(),
            entity_id: id.into(),
            kind: QueuedOperationKind::Write { payload: serde_json::Value::Null },
            queued_at_micros,
            retry_count: 0,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn enqueue_dedupes_by_table_and_entity_id() {
        let q = MemoryQueue::new();
        q.enqueue(op("messages", "1", 100)).await.unwrap();
        q.enqueue(op("messages", "1", 200)).await.unwrap();
        let snap = q.snapshot().await.unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].queued_at_micros, 200);
    }

    #[tokio::test]
    async fn finalize_drops_completed_keeps_concurrent_enqueues() {
        let q = MemoryQueue::new();
        q.enqueue(op("messages", "1", 100)).await.unwrap();
        let snap = q.snapshot().await.unwrap();
        // Simulate concurrent enqueue arriving during processing.
        q.enqueue(op("messages", "2", 150)).await.unwrap();
        // Finalize: snapshot all completed, none failed.
        q.finalize_after_processing(&snap, &[]).await.unwrap();
        let after = q.snapshot().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].entity_id, "2");
    }

    #[tokio::test]
    async fn finalize_retains_failures_and_bumps_retry_count() {
        let q = MemoryQueue::new();
        q.enqueue(op("conversations", "c-1", 100)).await.unwrap();
        let snap = q.snapshot().await.unwrap();

        let mut failed = snap[0].clone();
        failed.last_error = Some("HTTP 503".into());

        q.finalize_after_processing(&snap, &[failed]).await.unwrap();

        let after = q.snapshot().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].retry_count, 1,
            "retry_count should be incremented on retained failures"
        );
        assert_eq!(after[0].last_error.as_deref(), Some("HTTP 503"));

        // Retrying again bumps it further so callers can implement exponential backoff against it.
        let snap2 = q.snapshot().await.unwrap();
        let mut failed2 = snap2[0].clone();
        failed2.last_error = Some("HTTP 504".into());
        q.finalize_after_processing(&snap2, &[failed2]).await.unwrap();

        let after2 = q.snapshot().await.unwrap();
        assert_eq!(after2[0].retry_count, 2);
        assert_eq!(after2[0].last_error.as_deref(), Some("HTTP 504"));
    }

    #[tokio::test]
    async fn finalize_drops_failure_when_superseded_by_newer_enqueue() {
        let q = MemoryQueue::new();
        q.enqueue(op("attachments", "a-1", 100)).await.unwrap();
        let snap = q.snapshot().await.unwrap();

        // A newer op for the same key arrives during processing.
        q.enqueue(op("attachments", "a-1", 200)).await.unwrap();

        let mut failed = snap[0].clone();
        failed.last_error = Some("transient".into());
        q.finalize_after_processing(&snap, &[failed]).await.unwrap();

        let after = q.snapshot().await.unwrap();
        assert_eq!(after.len(), 1, "newer enqueue should remain");
        assert_eq!(after[0].queued_at_micros, 200);
        assert_eq!(
            after[0].retry_count, 0,
            "newer enqueue is a fresh op, not a retry of the failed snapshot entry"
        );
        assert!(after[0].last_error.is_none());
    }

    #[tokio::test]
    async fn enqueue_replacement_resets_retry_count_and_clears_error() {
        let q = MemoryQueue::new();

        let mut original = op("personas", "p-1", 100);
        original.retry_count = 3;
        original.last_error = Some("network timeout".into());
        q.enqueue(original).await.unwrap();

        q.enqueue(op("personas", "p-1", 200)).await.unwrap();

        let snap = q.snapshot().await.unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].retry_count, 0, "fresh enqueue should reset retry counter");
        assert!(snap[0].last_error.is_none(), "fresh enqueue should clear last_error");
    }
}
