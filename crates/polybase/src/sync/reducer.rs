//! Pure reducer for the sync runtime's offline-replay state machine.
//!
//! Lifted in spirit from PolyBase Swift's `Sync/Core.swift`. The reducer is intentionally pure:
//! it accepts events (enqueue, reconnect, replay outcome) and emits effects (`Schedule`,
//! `ProcessQueueNow`, `Reset`). The runtime executes the effects and feeds outcomes back in.
//!
//! Crate-internal until the offline-queue runtime that drives it is wired up externally.

use std::time::Duration;

use crate::contract::{
    ENQUEUE_COALESCE_MS, FORWARD_PROGRESS_RETRY_MS, RECONNECT_DEBOUNCE_SECS,
    reconnect_backoff_seconds,
};

/// Inputs to the reducer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ReducerEvent {
    /// New operation arrived.
    Enqueued,
    /// Network came back online.
    Reconnected,
    /// Auth state changed (sign-in, refresh).
    SessionChanged,
    /// Replay finished.
    ReplayCompleted {
        /// How many ops were successfully drained.
        drained: usize,
        /// How many ops remain in the queue.
        remaining: usize,
    },
    /// Replay failed (transient).
    ReplayFailed,
    /// Caller requested a reset (e.g. sign-out).
    Reset,
}

/// Effects the runtime should execute.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum ReducerEffect {
    /// Wait this long, then trigger a replay.
    Schedule(Duration),
    /// Drain the queue immediately.
    ProcessQueueNow,
    /// Cancel any pending replay.
    Cancel,
}

/// Reducer state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct ReducerState {
    pub(crate) consecutive_failures: u32,
    pub(crate) pending: bool,
    pub(crate) last_drained: usize,
}

impl Default for ReducerState {
    fn default() -> Self {
        Self { consecutive_failures: 0, pending: false, last_drained: 0 }
    }
}

/// Step the reducer. Returns the new state and any effects to execute.
#[allow(dead_code)]
pub(crate) fn step(
    state: &ReducerState,
    event: ReducerEvent,
) -> (ReducerState, Vec<ReducerEffect>) {
    let mut next = state.clone();
    match event {
        ReducerEvent::Enqueued => {
            next.pending = true;
            (next, vec![ReducerEffect::Schedule(Duration::from_millis(ENQUEUE_COALESCE_MS))])
        }
        ReducerEvent::Reconnected | ReducerEvent::SessionChanged => {
            next.pending = true;
            (next, vec![ReducerEffect::Schedule(Duration::from_secs(RECONNECT_DEBOUNCE_SECS))])
        }
        ReducerEvent::ReplayCompleted { drained, remaining } => {
            next.last_drained = drained;
            if drained > 0 {
                next.consecutive_failures = 0;
            }
            if remaining > 0 {
                next.pending = true;
                let delay = if drained > 0 {
                    Duration::from_millis(FORWARD_PROGRESS_RETRY_MS)
                } else {
                    next.consecutive_failures = next.consecutive_failures.saturating_add(1);
                    Duration::from_secs(reconnect_backoff_seconds(next.consecutive_failures))
                };
                (next, vec![ReducerEffect::Schedule(delay)])
            } else {
                next.pending = false;
                (next, vec![])
            }
        }
        ReducerEvent::ReplayFailed => {
            next.consecutive_failures = next.consecutive_failures.saturating_add(1);
            next.pending = true;
            let delay = Duration::from_secs(reconnect_backoff_seconds(next.consecutive_failures));
            (next, vec![ReducerEffect::Schedule(delay)])
        }
        ReducerEvent::Reset => {
            next.consecutive_failures = 0;
            next.pending = false;
            next.last_drained = 0;
            (next, vec![ReducerEffect::Cancel])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueued_schedules_coalesce_window() {
        let (next, effects) = step(&ReducerState::default(), ReducerEvent::Enqueued);
        assert!(next.pending);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_millis(ENQUEUE_COALESCE_MS))]
        );
    }

    #[test]
    fn reconnect_uses_default_debounce() {
        let (_, effects) = step(&ReducerState::default(), ReducerEvent::Reconnected);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_secs(RECONNECT_DEBOUNCE_SECS))]
        );
    }

    #[test]
    fn forward_progress_keeps_draining() {
        let state = ReducerState { consecutive_failures: 3, pending: true, last_drained: 0 };
        let (next, effects) =
            step(&state, ReducerEvent::ReplayCompleted { drained: 5, remaining: 2 });
        assert_eq!(next.consecutive_failures, 0);
        assert_eq!(
            effects,
            vec![ReducerEffect::Schedule(Duration::from_millis(FORWARD_PROGRESS_RETRY_MS))]
        );
    }

    #[test]
    fn no_progress_uses_backoff_ladder() {
        let state = ReducerState { consecutive_failures: 0, pending: true, last_drained: 0 };
        let (next, effects) =
            step(&state, ReducerEvent::ReplayCompleted { drained: 0, remaining: 1 });
        assert_eq!(next.consecutive_failures, 1);
        assert_eq!(effects, vec![ReducerEffect::Schedule(Duration::from_secs(2))]);
    }

    #[test]
    fn reset_cancels_and_clears_failures() {
        let state = ReducerState { consecutive_failures: 5, pending: true, last_drained: 0 };
        let (next, effects) = step(&state, ReducerEvent::Reset);
        assert_eq!(next, ReducerState::default());
        assert_eq!(effects, vec![ReducerEffect::Cancel]);
    }

    #[test]
    fn empty_queue_after_replay_is_idle() {
        let state = ReducerState { consecutive_failures: 1, pending: true, last_drained: 0 };
        let (next, effects) =
            step(&state, ReducerEvent::ReplayCompleted { drained: 1, remaining: 0 });
        assert!(!next.pending);
        assert!(effects.is_empty());
    }
}
