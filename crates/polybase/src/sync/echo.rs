//! Echo tracker — remembers recently-pushed `(table, id)` pairs so the realtime subscriber
//! can suppress server-rebound events for our own writes.
//!
//! Window default: [`crate::contract::ECHO_EXPIRY_WINDOW_SECS`] (5 seconds), matching Swift.
//!
//! Public so consumers can share a single tracker between the live writer
//! ([`crate::sync::SupabaseRemoteWriter`]) and any custom realtime subscription path —
//! a single tracker per process is the contract.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::contract::{ECHO_EXPIRY_WINDOW_SECS, echo_tracker_key};

/// Thread-safe echo tracker. Cheap to clone — the inner state is shared via `Arc`.
#[derive(Debug, Clone)]
pub struct EchoTracker {
    inner: Arc<Mutex<HashMap<String, Instant>>>,
    window: Duration,
}

impl EchoTracker {
    /// Tracker with the contract default window (5 seconds).
    pub fn with_default_window() -> Self {
        Self::with_window(Duration::from_secs(ECHO_EXPIRY_WINDOW_SECS))
    }

    /// Tracker with a custom window — tests may want a longer/shorter span.
    pub fn with_window(window: Duration) -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())), window }
    }

    /// Mark `(table, id)` as recently pushed. CRITICAL: must be called BEFORE the awaited write
    /// so the inbound realtime echo (which can arrive concurrently) is suppressed.
    pub fn mark_pushed(&self, table: &str, entity_id: &str) {
        self.inner.lock().insert(echo_tracker_key(table, entity_id), Instant::now());
    }

    /// True if `(table, id)` was pushed within the window.
    pub fn was_recently_pushed(&self, table: &str, entity_id: &str) -> bool {
        let key = echo_tracker_key(table, entity_id);
        let mut guard = self.inner.lock();
        match guard.get(&key) {
            Some(when) if when.elapsed() < self.window => true,
            Some(_) => {
                guard.remove(&key);
                false
            }
            None => false,
        }
    }

    /// Clear all entries. Used by sign-out / repair flows.
    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    /// Garbage-collect expired entries. Called opportunistically; not required for correctness.
    pub fn vacuum(&self) {
        let now = Instant::now();
        let window = self.window;
        self.inner.lock().retain(|_, when| now.duration_since(*when) < window);
    }
}

impl Default for EchoTracker {
    fn default() -> Self {
        Self::with_default_window()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_then_check_within_window() {
        let tracker = EchoTracker::with_window(Duration::from_secs(10));
        tracker.mark_pushed("messages", "abc");
        assert!(tracker.was_recently_pushed("messages", "abc"));
        assert!(!tracker.was_recently_pushed("messages", "other"));
    }

    #[test]
    fn expires_after_window() {
        let tracker = EchoTracker::with_window(Duration::from_millis(1));
        tracker.mark_pushed("messages", "abc");
        std::thread::sleep(Duration::from_millis(5));
        assert!(!tracker.was_recently_pushed("messages", "abc"));
    }

    #[test]
    fn clear_drops_all() {
        let tracker = EchoTracker::with_window(Duration::from_secs(10));
        tracker.mark_pushed("messages", "abc");
        tracker.clear();
        assert!(!tracker.was_recently_pushed("messages", "abc"));
    }
}
