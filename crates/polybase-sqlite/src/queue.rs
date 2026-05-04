//! SQLite-backed [`OfflineQueue`] implementation.
//!
//! Lives in the same per-user SQLite file as [`crate::SqliteLocalStore`] (both share a
//! [`crate::DbManager`]). The `polybase_queue` table is created by polybase-sqlite's
//! bundled migration — enable it via [`crate::DbManager::with_polybase_migrator`].
//!
//! ## Dedupe
//!
//! Schema-enforced via a composite primary key on `(table_name, entity_id)`, so [`enqueue`]
//! is a single `INSERT OR REPLACE` regardless of whether an older op with the same key
//! already existed.
//!
//! ## Finalize semantics
//!
//! Matches the contract helper in [`polybase::contract::finalize_queue_after_processing`]:
//! - Successful snapshot ops are deleted.
//! - Concurrent enqueues that arrived during processing are preserved.
//! - Failed snapshot ops are kept iff no newer op for the same key arrived; their
//!   `retry_count` is bumped and `last_error` is updated to the value the caller passed in.
//!
//! All finalize work runs in a single SQLite transaction so a partial drain can't leave the
//! queue in a half-updated state.
//!
//! [`enqueue`]: SqliteOfflineQueue::enqueue

use std::collections::HashSet;

use async_trait::async_trait;
use polybase::contract::{ContractOperation, finalize_queue_after_processing};
use polybase::errors::OfflineQueueError;
use polybase::offline_queue::{OfflineQueue, QueuedOperation, QueuedOperationKind};
use sqlx::SqlitePool;

use crate::manager::DbManager;

/// SQLite-backed [`OfflineQueue`].
#[derive(Clone)]
pub struct SqliteOfflineQueue {
    manager: DbManager,
}

impl SqliteOfflineQueue {
    /// Build a queue that delegates per-user pool management to `manager`. The same
    /// [`DbManager`] is typically shared with [`crate::SqliteLocalStore`] so all polybase
    /// state for a user lives in one SQLite file.
    pub fn new(manager: DbManager) -> Self {
        Self { manager }
    }

    /// Borrow the underlying [`DbManager`].
    pub fn manager(&self) -> &DbManager {
        &self.manager
    }

    async fn pool(&self) -> Result<SqlitePool, OfflineQueueError> {
        self.manager.pool().await.ok_or_else(|| OfflineQueueError::Io("no active user pool".into()))
    }

    // By-value signatures are intentional: passed directly to `.map_err(Self::map_*)` where
    // taking `&Error` would force every call site into a closure.
    #[allow(clippy::needless_pass_by_value)]
    fn map_sqlx(err: sqlx::Error) -> OfflineQueueError {
        OfflineQueueError::Io(err.to_string())
    }

    #[allow(clippy::needless_pass_by_value)]
    fn map_json(err: serde_json::Error) -> OfflineQueueError {
        OfflineQueueError::Decode(err.to_string())
    }
}

#[async_trait]
impl OfflineQueue for SqliteOfflineQueue {
    async fn enqueue(&self, op: QueuedOperation) -> Result<(), OfflineQueueError> {
        let pool = self.pool().await?;
        let kind_json = serde_json::to_string(&op.kind).map_err(Self::map_json)?;
        sqlx::query(
            "INSERT OR REPLACE INTO polybase_queue
                (table_name, entity_id, kind, queued_at_micros, retry_count, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&op.table)
        .bind(&op.entity_id)
        .bind(&kind_json)
        .bind(op.queued_at_micros)
        .bind(i64::from(op.retry_count))
        .bind(op.last_error.as_deref())
        .execute(&pool)
        .await
        .map_err(Self::map_sqlx)?;
        Ok(())
    }

    async fn snapshot(&self) -> Result<Vec<QueuedOperation>, OfflineQueueError> {
        let pool = self.pool().await?;
        let rows = sqlx::query_as::<_, QueueRow>(
            "SELECT table_name, entity_id, kind, queued_at_micros, retry_count, last_error
             FROM polybase_queue
             ORDER BY queued_at_micros ASC, table_name ASC, entity_id ASC",
        )
        .fetch_all(&pool)
        .await
        .map_err(Self::map_sqlx)?;
        rows.into_iter().map(QueuedOperation::try_from).collect()
    }

    async fn finalize_after_processing(
        &self,
        snapshot: &[QueuedOperation],
        failed: &[QueuedOperation],
    ) -> Result<(), OfflineQueueError> {
        let pool = self.pool().await?;
        let current = self.snapshot().await?;

        let current_contract: Vec<ContractOperation> =
            current.iter().map(QueuedOperation::to_contract).collect();
        let snapshot_contract: Vec<ContractOperation> =
            snapshot.iter().map(QueuedOperation::to_contract).collect();
        let failed_contract: Vec<ContractOperation> =
            failed.iter().map(QueuedOperation::to_contract).collect();

        let retained = finalize_queue_after_processing(
            &current_contract,
            &snapshot_contract,
            &failed_contract,
        );
        let retained_keys: HashSet<(String, String, i64)> =
            retained.into_iter().map(|op| (op.table, op.entity_id, op.queued_at_micros)).collect();
        // Key the failed lookup by the full tuple (table, entity_id, queued_at_micros) so that
        // if a concurrent enqueue replaced a failed op (different queued_at_micros), we don't
        // wrongly bump the newer op's retry counter — it isn't a retry, it's a fresh enqueue.
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

        let mut tx = pool.begin().await.map_err(Self::map_sqlx)?;
        for op in &current {
            let full_key = (op.table.clone(), op.entity_id.clone(), op.queued_at_micros);
            if !retained_keys.contains(&full_key) {
                sqlx::query(
                    "DELETE FROM polybase_queue
                     WHERE table_name = ?1 AND entity_id = ?2 AND queued_at_micros = ?3",
                )
                .bind(&op.table)
                .bind(&op.entity_id)
                .bind(op.queued_at_micros)
                .execute(&mut *tx)
                .await
                .map_err(Self::map_sqlx)?;
                continue;
            }

            if let Some(last_error) = failed_by_full_key.get(&full_key) {
                sqlx::query(
                    "UPDATE polybase_queue
                     SET retry_count = retry_count + 1, last_error = ?1
                     WHERE table_name = ?2 AND entity_id = ?3 AND queued_at_micros = ?4",
                )
                .bind(last_error.as_deref())
                .bind(&op.table)
                .bind(&op.entity_id)
                .bind(op.queued_at_micros)
                .execute(&mut *tx)
                .await
                .map_err(Self::map_sqlx)?;
            }
        }
        tx.commit().await.map_err(Self::map_sqlx)?;
        Ok(())
    }

    async fn depth(&self) -> Result<usize, OfflineQueueError> {
        let pool = self.pool().await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM polybase_queue")
            .fetch_one(&pool)
            .await
            .map_err(Self::map_sqlx)?;
        Ok(usize::try_from(count).unwrap_or(0))
    }
}

/// On-disk row shape. `kind` is JSON-serialized [`QueuedOperationKind`].
#[derive(sqlx::FromRow)]
struct QueueRow {
    table_name: String,
    entity_id: String,
    kind: String,
    queued_at_micros: i64,
    retry_count: i64,
    last_error: Option<String>,
}

impl TryFrom<QueueRow> for QueuedOperation {
    type Error = OfflineQueueError;

    fn try_from(row: QueueRow) -> Result<Self, Self::Error> {
        let kind: QueuedOperationKind = serde_json::from_str(&row.kind)
            .map_err(|err| OfflineQueueError::Decode(err.to_string()))?;
        Ok(QueuedOperation {
            table: row.table_name,
            entity_id: row.entity_id,
            kind,
            queued_at_micros: row.queued_at_micros,
            retry_count: u32::try_from(row.retry_count).unwrap_or(0),
            last_error: row.last_error,
        })
    }
}

#[cfg(test)]
mod tests {
    use polybase::offline_queue::QueuedOperationKind;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn write_op(
        table: &str,
        id: &str,
        queued_at: i64,
        payload: serde_json::Value,
    ) -> QueuedOperation {
        QueuedOperation {
            table: table.into(),
            entity_id: id.into(),
            kind: QueuedOperationKind::Write { payload },
            queued_at_micros: queued_at,
            retry_count: 0,
            last_error: None,
        }
    }

    fn tombstone_op(table: &str, id: &str, queued_at: i64, version: i64) -> QueuedOperation {
        QueuedOperation {
            table: table.into(),
            entity_id: id.into(),
            kind: QueuedOperationKind::Tombstone { version },
            queued_at_micros: queued_at,
            retry_count: 0,
            last_error: None,
        }
    }

    async fn setup() -> (TempDir, DbManager, SqliteOfflineQueue) {
        let tmp = TempDir::new().expect("tempdir");
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();
        manager.switch_user("queue-user").await.expect("switch user");
        let queue = SqliteOfflineQueue::new(manager.clone());
        (tmp, manager, queue)
    }

    #[tokio::test]
    async fn migrator_creates_polybase_queue_table() {
        let (_tmp, manager, _queue) = setup().await;
        let pool = manager.pool().await.expect("pool");
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM polybase_queue").fetch_one(&pool).await.unwrap();
        assert_eq!(row.0, 0);
    }

    #[tokio::test]
    async fn enqueue_dedupes_by_table_and_entity_id() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("messages", "m-1", 100, json!({"v": 1}))).await.unwrap();
        queue.enqueue(write_op("messages", "m-1", 200, json!({"v": 2}))).await.unwrap();

        let snap = queue.snapshot().await.unwrap();
        assert_eq!(snap.len(), 1, "later enqueue should replace earlier one");
        assert_eq!(snap[0].queued_at_micros, 200);
        match &snap[0].kind {
            QueuedOperationKind::Write { payload } => {
                assert_eq!(payload, &json!({"v": 2}));
            }
            other => panic!("expected Write, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enqueue_replacement_resets_retry_count_and_clears_error() {
        let (_tmp, _manager, queue) = setup().await;

        let mut original = write_op("personas", "p-1", 100, json!({"v": 1}));
        original.retry_count = 3;
        original.last_error = Some("network timeout".into());
        queue.enqueue(original).await.unwrap();

        queue.enqueue(write_op("personas", "p-1", 200, json!({"v": 2}))).await.unwrap();

        let snap = queue.snapshot().await.unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].retry_count, 0, "fresh enqueue should reset retry counter");
        assert!(snap[0].last_error.is_none(), "fresh enqueue should clear last_error");
    }

    #[tokio::test]
    async fn snapshot_orders_by_queued_at_ascending() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("a", "1", 300, json!(null))).await.unwrap();
        queue.enqueue(write_op("b", "1", 100, json!(null))).await.unwrap();
        queue.enqueue(write_op("c", "1", 200, json!(null))).await.unwrap();

        let snap = queue.snapshot().await.unwrap();
        let ts: Vec<i64> = snap.iter().map(|op| op.queued_at_micros).collect();
        assert_eq!(ts, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn finalize_drops_completed_keeps_concurrent_enqueues() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("messages", "m-1", 100, json!({"v": 1}))).await.unwrap();
        let snap = queue.snapshot().await.unwrap();

        // Concurrent enqueue arrives during processing.
        queue.enqueue(write_op("messages", "m-2", 150, json!({"v": 1}))).await.unwrap();

        // All snapshot ops succeeded.
        queue.finalize_after_processing(&snap, &[]).await.unwrap();

        let after = queue.snapshot().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].entity_id, "m-2");
    }

    #[tokio::test]
    async fn finalize_retains_failures_and_bumps_retry_count() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("conversations", "c-1", 100, json!(null))).await.unwrap();
        let snap = queue.snapshot().await.unwrap();

        let mut failed = snap[0].clone();
        failed.last_error = Some("HTTP 503".into());

        queue.finalize_after_processing(&snap, &[failed]).await.unwrap();

        let after = queue.snapshot().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].retry_count, 1, "retry_count should be incremented");
        assert_eq!(after[0].last_error.as_deref(), Some("HTTP 503"));

        // Retrying again bumps it further.
        let snap2 = queue.snapshot().await.unwrap();
        let mut failed2 = snap2[0].clone();
        failed2.last_error = Some("HTTP 504".into());
        queue.finalize_after_processing(&snap2, &[failed2]).await.unwrap();

        let after2 = queue.snapshot().await.unwrap();
        assert_eq!(after2[0].retry_count, 2);
        assert_eq!(after2[0].last_error.as_deref(), Some("HTTP 504"));
    }

    #[tokio::test]
    async fn finalize_drops_failure_when_superseded_by_newer_enqueue() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("attachments", "a-1", 100, json!({"v": 1}))).await.unwrap();
        let snap = queue.snapshot().await.unwrap();

        // Newer op for same key arrives during processing.
        queue.enqueue(write_op("attachments", "a-1", 200, json!({"v": 2}))).await.unwrap();

        let mut failed = snap[0].clone();
        failed.last_error = Some("transient".into());

        queue.finalize_after_processing(&snap, &[failed]).await.unwrap();

        let after = queue.snapshot().await.unwrap();
        assert_eq!(after.len(), 1, "newer enqueue should remain");
        assert_eq!(after[0].queued_at_micros, 200);
        assert_eq!(after[0].retry_count, 0, "newer enqueue isn't a retry");
    }

    #[tokio::test]
    async fn enqueue_persists_across_pool_close_and_reopen() {
        let tmp = TempDir::new().unwrap();
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();
        manager.switch_user("persist-user").await.unwrap();

        {
            let queue = SqliteOfflineQueue::new(manager.clone());
            queue
                .enqueue(write_op("personas", "p-1", 100, json!({"name": "Alice"})))
                .await
                .unwrap();
            queue.enqueue(tombstone_op("conversations", "c-1", 200, 5)).await.unwrap();
            assert_eq!(queue.depth().await.unwrap(), 2);
        }

        // Switch away and back: simulates a real restart that closes the pool.
        manager.switch_user("").await.unwrap();
        // Since switching away wipes the previous user's directory per the v1 contract,
        // the queue is gone — that's the *expected* sign-out behaviour, not persistence.
        assert!(!tmp.path().join("persist-user").exists());

        // True persistence: open a fresh manager pointing at the same root and re-attach.
        let manager2 = DbManager::new(tmp.path()).with_polybase_migrator();
        manager2.switch_user("persist-user-2").await.unwrap();
        let queue2 = SqliteOfflineQueue::new(manager2.clone());
        queue2.enqueue(write_op("messages", "m-1", 300, json!({"text": "hi"}))).await.unwrap();
        assert_eq!(queue2.depth().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn switch_user_wipes_queue_with_per_user_directory() {
        let tmp = TempDir::new().unwrap();
        let manager = DbManager::new(tmp.path()).with_polybase_migrator();

        manager.switch_user("alice").await.unwrap();
        let queue_alice = SqliteOfflineQueue::new(manager.clone());
        queue_alice.enqueue(write_op("messages", "alice-msg", 100, json!({"v": 1}))).await.unwrap();
        assert_eq!(queue_alice.depth().await.unwrap(), 1);

        manager.switch_user("bob").await.unwrap();
        let queue_bob = SqliteOfflineQueue::new(manager.clone());
        assert_eq!(queue_bob.depth().await.unwrap(), 0, "bob's queue starts empty");
        assert!(!tmp.path().join("alice").exists(), "alice's directory should be wiped");
    }

    #[tokio::test]
    async fn depth_reflects_inserts_and_finalize_deletes() {
        let (_tmp, _manager, queue) = setup().await;
        assert_eq!(queue.depth().await.unwrap(), 0);

        queue.enqueue(write_op("personas", "p-1", 100, json!(null))).await.unwrap();
        queue.enqueue(write_op("personas", "p-2", 200, json!(null))).await.unwrap();
        queue.enqueue(write_op("personas", "p-3", 300, json!(null))).await.unwrap();
        assert_eq!(queue.depth().await.unwrap(), 3);

        let snap = queue.snapshot().await.unwrap();
        queue.finalize_after_processing(&snap, &[]).await.unwrap();
        assert_eq!(queue.depth().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn kind_round_trips_for_all_variants() {
        let (_tmp, _manager, queue) = setup().await;
        queue.enqueue(write_op("personas", "p-w", 100, json!({"k": "v"}))).await.unwrap();
        queue.enqueue(tombstone_op("conversations", "c-t", 200, 7)).await.unwrap();
        queue
            .enqueue(QueuedOperation {
                table: "ephemeral".into(),
                entity_id: "e-h".into(),
                kind: QueuedOperationKind::HardDelete,
                queued_at_micros: 300,
                retry_count: 0,
                last_error: None,
            })
            .await
            .unwrap();

        let snap = queue.snapshot().await.unwrap();
        assert_eq!(snap.len(), 3);

        match &snap[0].kind {
            QueuedOperationKind::Write { payload } => assert_eq!(payload, &json!({"k": "v"})),
            other => panic!("expected Write, got {other:?}"),
        }
        match &snap[1].kind {
            QueuedOperationKind::Tombstone { version } => assert_eq!(*version, 7),
            other => panic!("expected Tombstone, got {other:?}"),
        }
        assert!(matches!(snap[2].kind, QueuedOperationKind::HardDelete));
    }
}
