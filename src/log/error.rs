use std::fmt;
use std::path::PathBuf;

/// Errors that can occur when initializing the logger.
#[derive(Debug)]
pub enum InitError {
    /// The logger has already been initialized for this process.
    AlreadyInitialized,
    /// Failed to create the log file or its parent directory.
    FileSetupFailed {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to install the global tracing subscriber.
    SetGlobalDefaultFailed(tracing::dispatcher::SetGlobalDefaultError),
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::AlreadyInitialized => {
                write!(f, "polykit::log already initialized; init() may only be called once per process")
            }
            InitError::FileSetupFailed { path, source } => {
                write!(f, "failed to set up log file at {}: {source}", path.display())
            }
            InitError::SetGlobalDefaultFailed(err) => {
                write!(f, "failed to install tracing subscriber: {err}")
            }
        }
    }
}

impl std::error::Error for InitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InitError::FileSetupFailed { source, .. } => Some(source),
            InitError::SetGlobalDefaultFailed(err) => Some(err),
            InitError::AlreadyInitialized => None,
        }
    }
}

/// Guard that must remain in scope for the lifetime of the program.
///
/// Dropping this guard flushes any pending file output.
#[must_use = "InitGuard must be held; dropping it flushes file output"]
pub struct InitGuard {
    pub(crate) _worker: Option<tracing_appender::non_blocking::WorkerGuard>,
}

#[allow(dead_code)]
impl InitGuard {
    /// Internal: guard with no worker (no file logging configured).
    pub(crate) fn empty() -> Self {
        Self { _worker: None }
    }

    /// Internal: guard that holds a file writer worker.
    pub(crate) fn with_worker(g: tracing_appender::non_blocking::WorkerGuard) -> Self {
        Self { _worker: Some(g) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_error_display_already_initialized() {
        let err = InitError::AlreadyInitialized;
        assert_eq!(
            err.to_string(),
            "polykit::log already initialized; init() may only be called once per process"
        );
    }

    #[test]
    fn init_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InitError>();
    }

    #[test]
    fn init_guard_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<InitGuard>();
    }
}
