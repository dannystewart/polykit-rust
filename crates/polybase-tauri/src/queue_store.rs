//! File-backed [`OfflineQueue`] implementation matching Tauri Prism's `offline_queue.json`.
//!
//! Disabled in test/dev environments to avoid polluting on-disk state — set
//! `POLYBASE_DISABLE_OFFLINE_QUEUE=1` to opt out at runtime even outside `cfg!(test)`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use polybase::contract::{ContractOperation, finalize_queue_after_processing};
use polybase::errors::OfflineQueueError;
use polybase::offline_queue::{OfflineQueue, QueuedOperation};
use tokio::sync::Mutex;

const ENV_DISABLE: &str = "POLYBASE_DISABLE_OFFLINE_QUEUE";

/// Persistent JSON-file queue.
#[derive(Debug, Clone)]
pub struct FileBackedQueue {
    inner: Arc<FileBackedInner>,
}

#[derive(Debug)]
struct FileBackedInner {
    path: PathBuf,
    cache: Mutex<VecDeque<QueuedOperation>>,
    disabled: bool,
}

impl FileBackedQueue {
    /// Build a new file-backed queue. Reads `path` on first use; missing file is treated as empty.
    pub fn new(path: PathBuf) -> Self {
        let disabled = cfg!(test) || std::env::var(ENV_DISABLE).is_ok();
        Self {
            inner: Arc::new(FileBackedInner { path, cache: Mutex::new(VecDeque::new()), disabled }),
        }
    }

    /// File system path the queue persists to.
    pub fn path(&self) -> &PathBuf {
        &self.inner.path
    }

    /// True when the queue is disabled (cfg(test) or `POLYBASE_DISABLE_OFFLINE_QUEUE` set).
    /// Calls succeed but no work is persisted.
    pub fn is_disabled(&self) -> bool {
        self.inner.disabled
    }

    async fn load_into_cache(&self) -> Result<(), OfflineQueueError> {
        if self.inner.disabled {
            return Ok(());
        }
        let mut guard = self.inner.cache.lock().await;
        if !guard.is_empty() {
            return Ok(());
        }
        let bytes = match tokio::fs::read(&self.inner.path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(OfflineQueueError::Io(err.to_string())),
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let parsed: Vec<QueuedOperation> = serde_json::from_slice(&bytes)
            .map_err(|err| OfflineQueueError::Decode(err.to_string()))?;
        *guard = VecDeque::from(parsed);
        Ok(())
    }

    async fn flush_locked(
        &self,
        guard: &VecDeque<QueuedOperation>,
    ) -> Result<(), OfflineQueueError> {
        if self.inner.disabled {
            return Ok(());
        }
        if let Some(parent) = self.inner.path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| OfflineQueueError::Io(err.to_string()))?;
        }
        let serialized: Vec<&QueuedOperation> = guard.iter().collect();
        let bytes = serde_json::to_vec(&serialized)
            .map_err(|err| OfflineQueueError::Decode(err.to_string()))?;
        tokio::fs::write(&self.inner.path, bytes)
            .await
            .map_err(|err| OfflineQueueError::Io(err.to_string()))
    }
}

#[async_trait]
impl OfflineQueue for FileBackedQueue {
    async fn enqueue(&self, op: QueuedOperation) -> Result<(), OfflineQueueError> {
        self.load_into_cache().await?;
        let mut guard = self.inner.cache.lock().await;
        guard
            .retain(|existing| !(existing.table == op.table && existing.entity_id == op.entity_id));
        guard.push_back(op);
        self.flush_locked(&guard).await
    }

    async fn snapshot(&self) -> Result<Vec<QueuedOperation>, OfflineQueueError> {
        self.load_into_cache().await?;
        Ok(self.inner.cache.lock().await.iter().cloned().collect())
    }

    async fn finalize_after_processing(
        &self,
        snapshot: &[QueuedOperation],
        failed: &[QueuedOperation],
    ) -> Result<(), OfflineQueueError> {
        self.load_into_cache().await?;
        let mut guard = self.inner.cache.lock().await;
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
        guard.retain(|op| {
            retained_keys.contains(&(op.table.clone(), op.entity_id.clone(), op.queued_at_micros))
        });
        self.flush_locked(&guard).await
    }

    async fn depth(&self) -> Result<usize, OfflineQueueError> {
        self.load_into_cache().await?;
        Ok(self.inner.cache.lock().await.len())
    }
}
