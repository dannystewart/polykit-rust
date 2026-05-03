use crate::Level;
use crate::init::{current_min_level, set_min_level};

/// RAII guard that temporarily overrides the global log level.
///
/// On construction, replaces the current minimum level with `level` and
/// remembers the previous value. On drop, restores the previous value.
///
/// Note: this affects the entire process, not a per-logger / per-thread
/// level. Concurrent threads will observe the override for its lifetime.
#[must_use = "LogLevelOverride must be held; dropping it ends the override"]
pub struct LogLevelOverride {
    previous: Level,
}

impl LogLevelOverride {
    /// Create a new override and apply it immediately.
    pub fn new(level: Level) -> Self {
        let previous = current_min_level();
        set_min_level(level);
        Self { previous }
    }
}

impl Drop for LogLevelOverride {
    fn drop(&mut self) {
        set_min_level(self.previous);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn override_changes_min_level_for_scope() {
        let _guard = match TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(_) => panic!("mutex poisoned"),
        };

        set_min_level(Level::Info);
        {
            let _override = LogLevelOverride::new(Level::Debug);
            assert_eq!(current_min_level(), Level::Debug);
        }

        assert_eq!(current_min_level(), Level::Info);
    }

    #[test]
    fn nested_overrides_unwind_in_lifo_order() {
        let _guard = match TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(_) => panic!("mutex poisoned"),
        };

        set_min_level(Level::Warn);
        {
            let _outer = LogLevelOverride::new(Level::Info);
            assert_eq!(current_min_level(), Level::Info);

            {
                let _inner = LogLevelOverride::new(Level::Debug);
                assert_eq!(current_min_level(), Level::Debug);
            }

            assert_eq!(current_min_level(), Level::Info);
        }

        assert_eq!(current_min_level(), Level::Warn);
    }

    #[test]
    fn override_works_across_threads() {
        use std::sync::mpsc;
        use std::thread;

        let _guard = match TEST_LOCK.lock() {
            Ok(guard) => guard,
            Err(_) => panic!("mutex poisoned"),
        };

        set_min_level(Level::Error);

        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let _override = LogLevelOverride::new(Level::Debug);
            let _ = ready_tx.send(());
            let _ = done_rx.recv();
        });

        let _ = ready_rx.recv();
        assert_eq!(current_min_level(), Level::Debug);

        let _ = done_tx.send(());
        let _ = handle.join();

        assert_eq!(current_min_level(), Level::Error);
    }
}
