use std::error::Error;

/// Run a fallible closure and log any error chain before returning it.
pub fn catch<T, E, F>(context: &str, f: F) -> Result<T, E>
where
    E: Error,
    F: FnOnce() -> Result<T, E>,
{
    match f() {
        Ok(value) => Ok(value),
        Err(error) => {
            tracing::error!("{context}: {error}");

            let mut cause = error.source();
            while let Some(source) = cause {
                tracing::error!("  caused by: {source}");
                cause = source.source();
            }

            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq, Eq)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl Error for TestError {}

    #[derive(Debug)]
    struct CauseError {
        message: &'static str,
        source: TestError,
    }

    impl fmt::Display for CauseError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl Error for CauseError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.source)
        }
    }

    #[derive(Clone, Default)]
    struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn new() -> Self {
            Self::default()
        }

        fn snapshot(&self) -> String {
            let bytes = match self.0.lock() {
                Ok(guard) => guard.clone(),
                Err(_) => panic!("mutex poisoned"),
            };
            match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => panic!("buffer should be valid UTF-8"),
            }
        }
    }

    struct BufferWriter<'a> {
        inner: std::sync::MutexGuard<'a, Vec<u8>>,
    }

    impl Write for BufferWriter<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuffer {
        type Writer = BufferWriter<'a>;

        fn make_writer(&'a self) -> Self::Writer {
            BufferWriter {
                inner: match self.0.lock() {
                    Ok(guard) => guard,
                    Err(_) => panic!("mutex poisoned"),
                },
            }
        }
    }

    #[test]
    fn catch_passes_through_ok() {
        let result = catch("ctx", || Ok::<_, TestError>("ok"));
        assert_eq!(result, Ok("ok"));
    }

    #[test]
    fn catch_returns_err_unchanged() {
        let err = TestError("boom");
        let result = catch("ctx", || Err::<(), _>(TestError("boom")));

        assert_eq!(result, Err(err));
    }

    #[test]
    fn catch_logs_chain() {
        let buffer = SharedBuffer::new();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_level(false)
            .with_target(false)
            .without_time()
            .with_writer(buffer.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let err = CauseError { message: "top level", source: TestError("inner cause") };

            let result = catch("loading config", || Err::<(), _>(err));
            assert!(result.is_err());
        });

        let logs = buffer.snapshot();
        assert!(logs.contains("loading config: top level"));
        assert!(logs.contains("caused by: inner cause"));
    }
}
