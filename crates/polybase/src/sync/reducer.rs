//! Pure state machine for sync scheduling decisions.
//!
//! Direct port of Swift PolyBase's `Sync/Core.swift` reducer. The reducer is intentionally pure:
//! it accepts events (app launch, connectivity, auth, enqueue, processing started/completed) and
//! emits effects (`Schedule`, `ProcessNow`). The runtime executes the effects and feeds outcomes
//! back in.
//!
//! The state and reducer carry **gates** (`is_connected`, `is_signed_in`, `is_processing`,
//! `has_pending`) that decide when an event should produce a schedule effect. This is critical:
//! Tauri Prism's earlier `SyncRuntime` had to reinvent these gates around a leaner reducer; with
//! the gates promoted into polybase, consumers can delegate the entire scheduling decision and
//! only layer their own UI display state on top.
//!
//! Backoff ladder: 2s, 5s, 10s, 30s, 60s, 120s (see [`crate::contract::reconnect_backoff_seconds`]).

use std::time::Duration;

use crate::contract::{
    ENQUEUE_COALESCE_MS, FORWARD_PROGRESS_RETRY_MS, RECONNECT_DEBOUNCE_SECS,
    reconnect_backoff_seconds,
};

/// State carried across reducer steps. Mirrors Swift's `PolyBase.Sync.Core.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReducerState {
    /// Network reachability. False suppresses all schedule effects.
    pub is_connected: bool,
    /// Authenticated session present. False suppresses all schedule effects.
    pub is_signed_in: bool,
    /// True while a replay is in flight; suppresses re-entrant schedules until processing completes.
    pub is_processing: bool,
    /// True when the offline queue carries at least one operation.
    pub has_pending: bool,
    /// Number of consecutive replay attempts that drained zero operations. Drives the backoff ladder.
    pub consecutive_failures: u32,
}

impl Default for ReducerState {
    /// Optimistic defaults: assume connected and signed-in. The runtime feeds in the truth via
    /// `ConnectivityChanged` / `AuthStateChanged` events on startup.
    fn default() -> Self {
        Self {
            is_connected: true,
            is_signed_in: true,
            is_processing: false,
            has_pending: false,
            consecutive_failures: 0,
        }
    }
}

/// Inputs to the reducer. Mirrors Swift's `PolyBase.Sync.Core.Event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerEvent {
    /// App launched and the offline queue may carry persisted operations.
    AppLaunched {
        /// Whether the queue has anything queued at startup.
        has_pending: bool,
    },
    /// Network reachability changed.
    ConnectivityChanged {
        /// New reachability state.
        is_connected: bool,
    },
    /// Authentication state changed.
    AuthStateChanged {
        /// Whether a session is now active.
        is_signed_in: bool,
    },
    /// A new operation was enqueued (or replaced an existing one) while the app is running.
    OfflineOperationEnqueued,
    /// A replay is about to start. Marks `is_processing = true` so subsequent events do not
    /// schedule a parallel drain.
    OfflineQueueProcessingStarted,
    /// A replay completed.
    OfflineQueueProcessed {
        /// How many ops were successfully drained.
        processed_count: usize,
        /// Whether the queue still has anything pending after this drain.
        still_has_pending: bool,
    },
}

/// Effects the runtime should execute. Mirrors Swift's `PolyBase.Sync.Core.Effect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReducerEffect {
    /// Schedule a replay after the given delay.
    Schedule(Duration),
    /// Trigger a replay immediately. Equivalent to `Schedule(Duration::ZERO)` for runtimes that
    /// do not distinguish the two paths.
    ProcessNow,
}

/// Step the reducer. Returns the new state and any effects to execute.
///
/// All schedule effects honor the gate cascade: an event only produces a `Schedule` (or
/// `ProcessNow`) when `has_pending`, `is_connected`, `is_signed_in`, and `!is_processing` are
/// all true. This matches Swift PolyBase's `Core.Reducer.reduce` behavior exactly.
pub fn step(state: &ReducerState, event: ReducerEvent) -> (ReducerState, Vec<ReducerEffect>) {
    let mut next = *state;

    match event {
        ReducerEvent::AppLaunched { has_pending } => {
            next.has_pending = has_pending;
            if !has_pending || !next.is_connected || !next.is_signed_in || next.is_processing {
                return (next, vec![]);
            }
            (next, vec![ReducerEffect::Schedule(Duration::ZERO)])
        }
        ReducerEvent::ConnectivityChanged { is_connected } => {
            let was_connected = next.is_connected;
            next.is_connected = is_connected;
            if !is_connected || was_connected {
                return (next, vec![]);
            }
            if !next.has_pending || !next.is_signed_in || next.is_processing {
                return (next, vec![]);
            }
            (next, vec![ReducerEffect::Schedule(Duration::from_secs(RECONNECT_DEBOUNCE_SECS))])
        }
        ReducerEvent::AuthStateChanged { is_signed_in } => {
            let was_signed_in = next.is_signed_in;
            next.is_signed_in = is_signed_in;
            if !is_signed_in || was_signed_in {
                return (next, vec![]);
            }
            if !next.has_pending || !next.is_connected || next.is_processing {
                return (next, vec![]);
            }
            (next, vec![ReducerEffect::Schedule(Duration::ZERO)])
        }
        ReducerEvent::OfflineOperationEnqueued => {
            next.has_pending = true;
            if !next.is_connected || !next.is_signed_in || next.is_processing {
                return (next, vec![]);
            }
            (next, vec![ReducerEffect::Schedule(Duration::from_millis(ENQUEUE_COALESCE_MS))])
        }
        ReducerEvent::OfflineQueueProcessingStarted => {
            next.is_processing = true;
            (next, vec![])
        }
        ReducerEvent::OfflineQueueProcessed { processed_count, still_has_pending } => {
            next.is_processing = false;
            next.has_pending = still_has_pending;

            if !still_has_pending {
                next.consecutive_failures = 0;
                return (next, vec![]);
            }

            if processed_count > 0 {
                next.consecutive_failures = 0;
            } else {
                next.consecutive_failures = next.consecutive_failures.saturating_add(1);
            }

            if !next.is_connected || !next.is_signed_in {
                return (next, vec![]);
            }

            if processed_count > 0 {
                (
                    next,
                    vec![ReducerEffect::Schedule(Duration::from_millis(FORWARD_PROGRESS_RETRY_MS))],
                )
            } else {
                let secs = reconnect_backoff_seconds(next.consecutive_failures);
                (next, vec![ReducerEffect::Schedule(Duration::from_secs(secs))])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> ReducerState {
        ReducerState::default()
    }

    #[test]
    fn app_launched_with_pending_schedules_immediate_replay() {
        let (next, effects) = step(&fresh(), ReducerEvent::AppLaunched { has_pending: true });
        assert!(next.has_pending);
        assert_eq!(effects, vec![ReducerEffect::Schedule(Duration::ZERO)]);
    }

    #[test]
    fn app_launched_without_pending_emits_no_effects() {
        let (next, effects) = step(&fresh(), ReducerEvent::AppLaunched { has_pending: false });
        assert!(!next.has_pending);
        assert!(effects.is_empty());
    }

    #[test]
    fn app_launched_offline_emits_no_effects() {
        let mut state = fresh();
        state.is_connected = false;
        let (_, effects) = step(&state, ReducerEvent::AppLaunched { has_pending: true });
        assert!(effects.is_empty());
    }

    #[test]
    fn app_launched_signed_out_emits_no_effects() {
        let mut state = fresh();
        state.is_signed_in = false;
        let (_, effects) = step(&state, ReducerEvent::AppLaunched { has_pending: true });
        assert!(effects.is_empty());
    }

    #[test]
    fn connectivity_restored_with_pending_schedules_after_debounce() {
        let mut state = fresh();
        state.is_connected = false;
        state.has_pending = true;
        let (next, effects) =
            step(&state, ReducerEvent::ConnectivityChanged { is_connected: true });
        assert!(next.is_connected);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_secs(RECONNECT_DEBOUNCE_SECS))]
        );
    }

    #[test]
    fn connectivity_restored_with_no_change_emits_no_effects() {
        let mut state = fresh();
        state.has_pending = true;
        let (_, effects) = step(&state, ReducerEvent::ConnectivityChanged { is_connected: true });
        assert!(effects.is_empty());
    }

    #[test]
    fn connectivity_lost_emits_no_effects_and_disables_gate() {
        let state = fresh();
        let (next, effects) =
            step(&state, ReducerEvent::ConnectivityChanged { is_connected: false });
        assert!(!next.is_connected);
        assert!(effects.is_empty());
    }

    #[test]
    fn auth_signed_in_with_pending_schedules_immediate_replay() {
        let mut state = fresh();
        state.is_signed_in = false;
        state.has_pending = true;
        let (next, effects) = step(&state, ReducerEvent::AuthStateChanged { is_signed_in: true });
        assert!(next.is_signed_in);
        assert_eq!(effects, vec![ReducerEffect::Schedule(Duration::ZERO)]);
    }

    #[test]
    fn auth_signed_out_disables_gate() {
        let state = fresh();
        let (next, effects) = step(&state, ReducerEvent::AuthStateChanged { is_signed_in: false });
        assert!(!next.is_signed_in);
        assert!(effects.is_empty());
    }

    #[test]
    fn enqueue_coalesces_bursts() {
        let (next, effects) = step(&fresh(), ReducerEvent::OfflineOperationEnqueued);
        assert!(next.has_pending);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_millis(ENQUEUE_COALESCE_MS))]
        );
    }

    #[test]
    fn enqueue_while_processing_emits_no_effects() {
        let mut state = fresh();
        state.is_processing = true;
        let (next, effects) = step(&state, ReducerEvent::OfflineOperationEnqueued);
        assert!(next.has_pending);
        assert!(effects.is_empty());
    }

    #[test]
    fn processing_started_marks_processing_and_emits_no_effects() {
        let (next, effects) = step(&fresh(), ReducerEvent::OfflineQueueProcessingStarted);
        assert!(next.is_processing);
        assert!(effects.is_empty());
    }

    #[test]
    fn processed_drained_queue_resets_backoff_and_emits_no_effects() {
        let mut state = fresh();
        state.is_processing = true;
        state.has_pending = true;
        state.consecutive_failures = 3;
        let (next, effects) = step(
            &state,
            ReducerEvent::OfflineQueueProcessed { processed_count: 5, still_has_pending: false },
        );
        assert!(!next.is_processing);
        assert!(!next.has_pending);
        assert_eq!(next.consecutive_failures, 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn forward_progress_keeps_draining_aggressively() {
        let mut state = fresh();
        state.is_processing = true;
        state.has_pending = true;
        state.consecutive_failures = 3;
        let (next, effects) = step(
            &state,
            ReducerEvent::OfflineQueueProcessed { processed_count: 5, still_has_pending: true },
        );
        assert_eq!(next.consecutive_failures, 0);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_millis(FORWARD_PROGRESS_RETRY_MS))]
        );
    }

    #[test]
    fn no_forward_progress_walks_backoff_ladder() {
        let mut state = fresh();
        state.is_processing = true;
        state.has_pending = true;
        let (next, effects) = step(
            &state,
            ReducerEvent::OfflineQueueProcessed { processed_count: 0, still_has_pending: true },
        );
        assert_eq!(next.consecutive_failures, 1);
        assert_eq!(effects, vec![ReducerEffect::Schedule(Duration::from_secs(2))]);
    }

    #[test]
    fn no_forward_progress_offline_emits_no_effects_but_increments_failures() {
        let mut state = fresh();
        state.is_processing = true;
        state.has_pending = true;
        state.is_connected = false;
        let (next, effects) = step(
            &state,
            ReducerEvent::OfflineQueueProcessed { processed_count: 0, still_has_pending: true },
        );
        assert_eq!(next.consecutive_failures, 1);
        assert!(effects.is_empty());
    }

    #[test]
    fn backoff_clamps_at_top_of_ladder() {
        let mut state = fresh();
        state.is_processing = true;
        state.has_pending = true;
        state.consecutive_failures = 99;
        let (next, effects) = step(
            &state,
            ReducerEvent::OfflineQueueProcessed { processed_count: 0, still_has_pending: true },
        );
        assert_eq!(next.consecutive_failures, 100);
        assert_eq!(effects, vec![ReducerEffect::Schedule(Duration::from_secs(120))]);
    }
}
