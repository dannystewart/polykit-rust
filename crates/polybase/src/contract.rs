//! Frozen semantic invariants for the PolyBase Rust port.
//!
//! These constants and pure helpers lock in the contract that PolyBase Swift v1 (battle-tested) and
//! PolyBase Rust v2 must both honor. Keeping them in one place — and in pure code — means tests can
//! pin behavior independently of any runtime, and porting to other apps does not silently change
//! semantics like the version-step delta or echo-suppression window.
//!
//! Lifted from Tauri Prism's `src-tauri/src/sync/contract.rs`. Treat as append-only.

use std::collections::{HashMap, HashSet};

/// Stable name of the contract for identification in logs / diagnostics.
pub const CONTRACT_NAME: &str = "polybase-rust-v2";

// -- Per-user data partition ---------------------------------------------------------------------

/// Each authenticated user owns at most one local database file.
pub const SQLITE_DATABASES_PER_AUTHENTICATED_USER: usize = 1;
/// Each authenticated user owns at most one attachment cache root.
pub const ATTACHMENT_ROOTS_PER_AUTHENTICATED_USER: usize = 1;
/// Sign-out / account switch must wipe the active user's local mirror before activating a new user.
pub const WIPE_ACTIVE_USER_BEFORE_NEXT_ACTIVATION: bool = true;

// -- Version increment rules ---------------------------------------------------------------------

/// New synced rows start at version 1.
pub const INITIAL_VERSION: i64 = 1;
/// Ordinary local mutations bump version by +1.
pub const STANDARD_MUTATION_VERSION_STEP: i64 = 1;
/// Intentional undelete is the only allowed large jump and must use +1000.
pub const UNDELETE_VERSION_DELTA: i64 = 1000;

// -- Queue contract ------------------------------------------------------------------------------

/// Queue dedupe key fields.
pub const QUEUE_DEDUPE_KEY_FIELDS: [&str; 2] = ["table", "entity_id"];
/// `queued_at` must be monotonic; bump by 1µs when wall-clock does not advance.
pub const MONOTONIC_QUEUE_TIMESTAMP_BUMP_MICROS: i64 = 1;

// -- Reconnect / replay backoff ------------------------------------------------------------------

/// Reconnect processing is debounced by this many seconds before another replay attempt.
pub const RECONNECT_DEBOUNCE_SECS: u64 = 2;
/// Enqueue bursts are coalesced for this many milliseconds before draining.
pub const ENQUEUE_COALESCE_MS: u64 = 250;
/// If replay makes forward progress, keep draining after this many milliseconds.
pub const FORWARD_PROGRESS_RETRY_MS: u64 = 100;
/// Backoff ladder when replay makes no progress (seconds).
pub const RECONNECT_BACKOFF_LADDER_SECS: [u64; 6] = [2, 5, 10, 30, 60, 120];
/// Per-operation timeout when replaying offline queue items, to avoid long hangs.
pub const REPLAY_PER_OPERATION_TIMEOUT_SECS: u64 = 20;

// -- Realtime echo filtering ---------------------------------------------------------------------

/// Echo suppression remembers pushed IDs for this many seconds.
pub const ECHO_EXPIRY_WINDOW_SECS: u64 = 5;
/// Echo lookup key format: `table:id`.
pub const ECHO_KEY_FORMAT: &str = "table:id";

// -- Encryption wire format compatibility --------------------------------------------------------

/// Encrypted strings keep the `enc:` prefix.
pub const STRING_ENCRYPTION_PREFIX: &str = "enc:";
/// Encrypted binary payloads keep the `ENC\0` magic header.
pub const BINARY_ENCRYPTION_HEADER: [u8; 4] = *b"ENC\0";
/// HKDF salt rule (uppercased UUID bytes).
pub const HKDF_SALT_RULE: &str = "uppercased-uuid";
/// HKDF info string.
pub const HKDF_INFO: &str = "supabase-encryption";
/// Attachment storage path format.
pub const ATTACHMENT_STORAGE_PATH_FORMAT: &str = "user-id/conversation-id/attachment-id.ext";

// -- Pure helpers --------------------------------------------------------------------------------

/// Minimum version value that constitutes an intentional undelete vs. a stale mutation.
pub const fn undelete_minimum_version(remote_version: i64) -> i64 {
    remote_version + UNDELETE_VERSION_DELTA
}

/// Compute the merged version when local logic decides the local row wins. Always advances
/// to `max(local, remote) + 1` so the next push is monotonic.
pub const fn local_winner_merged_version(local_version: i64, remote_version: i64) -> i64 {
    if local_version >= remote_version {
        local_version + STANDARD_MUTATION_VERSION_STEP
    } else {
        remote_version + STANDARD_MUTATION_VERSION_STEP
    }
}

/// Compute the next monotonic queue timestamp, bumping by 1µs if the wall clock did not advance.
pub const fn next_queued_at_micros(
    last_queued_at_micros: Option<i64>,
    candidate_micros: i64,
) -> i64 {
    match last_queued_at_micros {
        Some(last) if candidate_micros <= last => last + MONOTONIC_QUEUE_TIMESTAMP_BUMP_MICROS,
        _ => candidate_micros,
    }
}

/// Look up the next reconnect backoff. Index is `consecutive_failures - 1`, clamped to ladder.
pub const fn reconnect_backoff_seconds(consecutive_failures: u32) -> u64 {
    let index = consecutive_failures.saturating_sub(1) as usize;
    let last = RECONNECT_BACKOFF_LADDER_SECS.len() - 1;
    let clamped = if index >= RECONNECT_BACKOFF_LADDER_SECS.len() { last } else { index };
    RECONNECT_BACKOFF_LADDER_SECS[clamped]
}

/// Format the echo-tracker lookup key for `(table, entity_id)`.
pub fn echo_tracker_key(table: &str, entity_id: &str) -> String {
    format!("{table}:{entity_id}")
}

/// The dedupe key for offline-queue operations.
pub fn queue_dedupe_key<'a>(table: &'a str, entity_id: &'a str) -> (&'a str, &'a str) {
    (table, entity_id)
}

/// Format a Supabase Storage path for an attachment-style payload.
pub fn attachment_storage_path(
    user_id: &str,
    conversation_id: &str,
    attachment_id: &str,
    extension: &str,
) -> String {
    format!("{user_id}/{conversation_id}/{attachment_id}.{extension}")
}

/// Possible reconcile outcomes for a single row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileAction {
    /// Local must adopt the remote tombstone.
    AdoptTombstone,
    /// Pull the newer remote row into local.
    Pull,
    /// Push the newer local row to remote.
    Push,
    /// Both sides agree — no work.
    Skip,
}

/// Determine the reconcile action for a single row given the four key axes.
pub fn determine_reconcile_action(
    local_version: i64,
    local_deleted: bool,
    remote_version: i64,
    remote_deleted: bool,
) -> ReconcileAction {
    if remote_deleted && !local_deleted {
        if local_version >= undelete_minimum_version(remote_version) {
            return ReconcileAction::Push;
        }
        return ReconcileAction::AdoptTombstone;
    }

    if local_deleted && !remote_deleted {
        return ReconcileAction::Push;
    }

    if remote_version > local_version {
        return ReconcileAction::Pull;
    }
    if local_version > remote_version {
        return ReconcileAction::Push;
    }

    ReconcileAction::Skip
}

/// A queued operation under the contract dedupe rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractOperation {
    /// Table name the operation targets.
    pub table: String,
    /// Primary key value within the table.
    pub entity_id: String,
    /// Monotonic enqueue timestamp in microseconds.
    pub queued_at_micros: i64,
}

impl ContractOperation {
    /// Build a contract operation tuple from the canonical fields.
    pub fn new(
        table: impl Into<String>,
        entity_id: impl Into<String>,
        queued_at_micros: i64,
    ) -> Self {
        Self { table: table.into(), entity_id: entity_id.into(), queued_at_micros }
    }

    fn key(&self) -> (String, String) {
        (self.table.clone(), self.entity_id.clone())
    }
}

/// After the worker drains a snapshot, finalize the queue: drop completed snapshot items, retain
/// concurrent enqueues, and re-add any failed items only if no newer op for the same key exists.
///
/// This is one of the hard-won lessons from PolyBase Swift — the snapshot+queued_at match prevents
/// dropping enqueues that arrived during processing.
pub fn finalize_queue_after_processing(
    current: &[ContractOperation],
    snapshot: &[ContractOperation],
    failed: &[ContractOperation],
) -> Vec<ContractOperation> {
    let snapshot_queued_at_by_key: HashMap<(String, String), i64> =
        snapshot.iter().map(|operation| (operation.key(), operation.queued_at_micros)).collect();

    let mut retained: Vec<ContractOperation> = current
        .iter()
        .filter(|operation| {
            snapshot_queued_at_by_key
                .get(&operation.key())
                .is_none_or(|snapshot_queued_at| *snapshot_queued_at != operation.queued_at_micros)
        })
        .cloned()
        .collect();

    let mut retained_keys: HashSet<(String, String)> =
        retained.iter().map(ContractOperation::key).collect();

    for operation in failed {
        let key = operation.key();
        if retained_keys.insert(key) {
            retained.push(operation.clone());
        }
    }

    retained
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undelete_threshold_uses_plus_1000() {
        assert_eq!(undelete_minimum_version(5), 1005);
    }

    #[test]
    fn reconnect_backoff_is_clamped_to_ladder_tail() {
        assert_eq!(reconnect_backoff_seconds(1), 2);
        assert_eq!(reconnect_backoff_seconds(6), 120);
        assert_eq!(reconnect_backoff_seconds(99), 120);
    }

    #[test]
    fn local_winner_advances_past_remote() {
        assert_eq!(local_winner_merged_version(5, 10), 11);
        assert_eq!(local_winner_merged_version(10, 5), 11);
    }

    #[test]
    fn monotonic_queued_at_bumps_when_clock_stalls() {
        assert_eq!(next_queued_at_micros(Some(100), 100), 101);
        assert_eq!(next_queued_at_micros(Some(100), 200), 200);
        assert_eq!(next_queued_at_micros(None, 50), 50);
    }

    #[test]
    fn echo_key_format_is_table_colon_id() {
        assert_eq!(echo_tracker_key("messages", "abc"), "messages:abc");
    }

    #[test]
    fn reconcile_local_winner_when_local_deleted_only() {
        assert_eq!(determine_reconcile_action(5, true, 5, false), ReconcileAction::Push,);
    }

    #[test]
    fn reconcile_adopts_remote_tombstone_unless_undelete_threshold() {
        assert_eq!(determine_reconcile_action(5, false, 5, true), ReconcileAction::AdoptTombstone,);
        assert_eq!(determine_reconcile_action(1006, false, 5, true), ReconcileAction::Push,);
    }

    #[test]
    fn reconcile_versions_drive_pull_or_push_or_skip() {
        assert_eq!(determine_reconcile_action(5, false, 6, false), ReconcileAction::Pull,);
        assert_eq!(determine_reconcile_action(7, false, 6, false), ReconcileAction::Push,);
        assert_eq!(determine_reconcile_action(6, false, 6, false), ReconcileAction::Skip,);
    }

    #[test]
    fn finalize_drops_completed_and_keeps_concurrent_enqueues() {
        let snapshot = vec![ContractOperation::new("t", "1", 100)];
        let current =
            vec![ContractOperation::new("t", "1", 100), ContractOperation::new("t", "2", 150)];
        let failed: Vec<ContractOperation> = vec![];
        let retained = finalize_queue_after_processing(&current, &snapshot, &failed);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].entity_id, "2");
    }

    #[test]
    fn finalize_does_not_reinsert_failed_when_newer_op_exists() {
        let snapshot = vec![ContractOperation::new("t", "1", 100)];
        let current = vec![ContractOperation::new("t", "1", 200)];
        let failed = vec![ContractOperation::new("t", "1", 100)];
        let retained = finalize_queue_after_processing(&current, &snapshot, &failed);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].queued_at_micros, 200);
    }
}
