//! Reconcile planner.
//!
//! The planner is a pure function: given two sets of `(id, version, deleted)` tuples (one local,
//! one remote), it produces a [`ReconcilePlan`] enumerating what each side needs to do. The
//! actual execution (issuing the pull/push) is the runtime's job.
//!
use std::collections::{HashMap, HashSet};

use crate::contract::{ReconcileAction, determine_reconcile_action};
use crate::events::ReconcileActionCounts;

/// Compact triple used as input to the planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionTriple {
    /// Primary key of the row.
    pub id: String,
    /// Current version (monotonic per the contract).
    pub version: i64,
    /// True for tombstoned rows.
    pub deleted: bool,
}

impl VersionTriple {
    /// Convenience constructor used by callers building local-side input lists.
    pub fn new(id: impl Into<String>, version: i64, deleted: bool) -> Self {
        Self { id: id.into(), version, deleted }
    }
}

/// Planned actions for a single table — buckets of ids that should pull, push, adopt the
/// remote tombstone, or skip. Ids in [`Self::create_local`] / [`Self::create_remote`] exist
/// only on one side and need to be materialized on the other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcilePlan {
    /// Local rows that should adopt the remote tombstone (fast path for in-window deletes).
    pub adopt_tombstone: Vec<String>,
    /// Ids whose remote version is newer than local and should be pulled.
    pub pull: Vec<String>,
    /// Ids whose local version is newer than remote and should be pushed.
    pub push: Vec<String>,
    /// Ids that match on both sides; nothing to do.
    pub skip: Vec<String>,
    /// Ids that exist remote-only — must be created locally.
    pub create_local: Vec<String>,
    /// Ids that exist local-only — must be created on remote.
    pub create_remote: Vec<String>,
}

impl ReconcilePlan {
    /// Bucket counts suitable for `PolyEvent::ReconcileProgress { action_counts: ... }`.
    pub fn counts(&self) -> ReconcileActionCounts {
        ReconcileActionCounts {
            adopt_tombstone: u32::try_from(self.adopt_tombstone.len()).unwrap_or(u32::MAX),
            pull: u32::try_from(self.pull.len() + self.create_local.len()).unwrap_or(u32::MAX),
            push: u32::try_from(self.push.len() + self.create_remote.len()).unwrap_or(u32::MAX),
            skip: u32::try_from(self.skip.len()).unwrap_or(u32::MAX),
        }
    }
}

/// Build a deterministic plan from local + remote version sets.
pub fn make_plan(local: &[VersionTriple], remote: &[VersionTriple]) -> ReconcilePlan {
    let local_index: HashMap<&str, &VersionTriple> =
        local.iter().map(|t| (t.id.as_str(), t)).collect();
    let remote_index: HashMap<&str, &VersionTriple> =
        remote.iter().map(|t| (t.id.as_str(), t)).collect();

    let local_ids: HashSet<&str> = local_index.keys().copied().collect();
    let remote_ids: HashSet<&str> = remote_index.keys().copied().collect();

    let mut plan = ReconcilePlan::default();

    let common: Vec<&str> = local_ids.intersection(&remote_ids).copied().collect();
    let mut sorted_common = common.clone();
    sorted_common.sort_unstable();
    for id in sorted_common {
        let local = local_index[id];
        let remote = remote_index[id];
        match determine_reconcile_action(
            local.version,
            local.deleted,
            remote.version,
            remote.deleted,
        ) {
            ReconcileAction::AdoptTombstone => plan.adopt_tombstone.push(id.into()),
            ReconcileAction::Pull => plan.pull.push(id.into()),
            ReconcileAction::Push => plan.push.push(id.into()),
            ReconcileAction::Skip => plan.skip.push(id.into()),
        }
    }

    let mut local_only: Vec<&str> = local_ids.difference(&remote_ids).copied().collect();
    local_only.sort_unstable();
    plan.create_remote.extend(local_only.into_iter().map(String::from));

    let mut remote_only: Vec<&str> = remote_ids.difference(&local_ids).copied().collect();
    remote_only.sort_unstable();
    plan.create_local.extend(remote_only.into_iter().map(String::from));

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, version: i64, deleted: bool) -> VersionTriple {
        VersionTriple { id: id.into(), version, deleted }
    }

    #[test]
    fn pull_when_remote_newer() {
        let plan = make_plan(&[t("a", 1, false)], &[t("a", 2, false)]);
        assert_eq!(plan.pull, vec!["a".to_string()]);
        assert!(plan.push.is_empty());
    }

    #[test]
    fn push_when_local_newer() {
        let plan = make_plan(&[t("a", 3, false)], &[t("a", 2, false)]);
        assert_eq!(plan.push, vec!["a".to_string()]);
    }

    #[test]
    fn adopt_tombstone_when_remote_deleted_within_threshold() {
        let plan = make_plan(&[t("a", 5, false)], &[t("a", 5, true)]);
        assert_eq!(plan.adopt_tombstone, vec!["a".to_string()]);
    }

    #[test]
    fn push_undelete_when_local_meets_threshold() {
        let plan = make_plan(&[t("a", 1006, false)], &[t("a", 5, true)]);
        assert_eq!(plan.push, vec!["a".to_string()]);
    }

    #[test]
    fn create_local_for_remote_only_ids() {
        let plan = make_plan(&[], &[t("a", 1, false), t("b", 1, false)]);
        assert_eq!(plan.create_local, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn create_remote_for_local_only_ids() {
        let plan = make_plan(&[t("c", 1, false)], &[]);
        assert_eq!(plan.create_remote, vec!["c".to_string()]);
    }

    #[test]
    fn skip_when_versions_match_and_neither_deleted() {
        let plan = make_plan(&[t("a", 4, false)], &[t("a", 4, false)]);
        assert_eq!(plan.skip, vec!["a".to_string()]);
    }
}
