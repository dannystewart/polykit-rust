use std::io::Write as _;
use std::path::{Path, PathBuf};

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

use crate::log::builder::LogConfig;
use crate::log::{InitError, Level};

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_owned();
        }
    }
}

pub(crate) struct FileLayer {
    pub(crate) tz: jiff::tz::TimeZone,
    pub(crate) writer: tracing_appender::non_blocking::NonBlocking,
}

fn format_file(
    level: Level,
    ts: &str,
    target: &str,
    file_basename: &str,
    line_num: u32,
    msg: &str,
) -> Vec<u8> {
    let label = level.label();
    format!("[{ts}] {label} {target} {file_basename}:{line_num}: {msg}\n").into_bytes()
}

pub(crate) fn build_file_layer(
    config: &LogConfig,
    tz: jiff::tz::TimeZone,
) -> Result<(FileLayer, tracing_appender::non_blocking::WorkerGuard), InitError> {
    let Some(path) = config.log_file.as_ref() else {
        return Err(InitError::FileSetupFailed {
            path: PathBuf::new(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "log_file missing"),
        });
    };

    let (directory, filename): (PathBuf, PathBuf) = {
        let path_str = path.to_string_lossy();
        if path_str.ends_with('/') || path.is_dir() {
            (path.to_path_buf(), PathBuf::from("polykit.log"))
        } else {
            let dir = path
                .parent()
                .map(|p| if p == Path::new("") { Path::new(".") } else { p })
                .unwrap_or(Path::new("."))
                .to_path_buf();
            let fname =
                path.file_name().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("polykit.log"));
            (dir, fname)
        }
    };

    std::fs::create_dir_all(&directory)
        .map_err(|source| InitError::FileSetupFailed { path: path.clone(), source })?;

    let file_appender = tracing_appender::rolling::daily(&directory, &filename);
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    Ok((FileLayer { tz, writer }, guard))
}

impl FileLayer {
    fn render_event(&self, event: &tracing::Event<'_>) -> Vec<u8> {
        let meta = event.metadata();

        if *meta.level() == tracing::Level::TRACE {
            return Vec::new();
        }

        let Some(event_level) = Level::from_tracing(*meta.level()) else {
            return Vec::new();
        };
        if event_level < crate::log::init::current_min_level() {
            return Vec::new();
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let msg = visitor.0;

        let ts = jiff::Zoned::now()
            .with_time_zone(self.tz.clone())
            .strftime("%Y-%m-%d %H:%M:%S")
            .to_string();

        let (file_basename, line_num) = match (meta.file(), meta.line()) {
            (Some(f), Some(l)) => {
                let basename = Path::new(f).file_name().and_then(|n| n.to_str()).unwrap_or(f);
                (basename.to_owned(), l)
            }
            _ => ("<unknown>".to_owned(), 0),
        };

        let target = meta.target();
        format_file(event_level, &ts, target, &file_basename, line_num, &msg)
    }
}

impl<S> Layer<S> for FileLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let bytes = self.render_event(event);
        if bytes.is_empty() {
            return;
        }
        let mut w = self.writer.clone();
        let _ = w.write_all(&bytes);
        let _ = w.flush();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::log::builder::LogConfig;
    use crate::log::format::{ColorMode, FormatMode};

    fn make_layer() -> FileLayer {
        let dir = std::env::temp_dir().join("polykit-file-test-sink");
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let appender = tracing_appender::rolling::daily(&dir, "test.log");
        let (writer, _guard) = tracing_appender::non_blocking(appender);
        FileLayer { tz: jiff::tz::TimeZone::UTC, writer }
    }

    fn render_direct(
        _layer: &FileLayer,
        ts: &'static str,
        level: tracing::Level,
        target: &'static str,
        file: Option<&'static str>,
        line: Option<u32>,
        msg: &str,
    ) -> Vec<u8> {
        if level == tracing::Level::TRACE {
            return Vec::new();
        }

        let Some(event_level) = Level::from_tracing(level) else {
            return Vec::new();
        };

        let (file_basename, line_num) = match (file, line) {
            (Some(f), Some(l)) => {
                let basename =
                    std::path::Path::new(f).file_name().and_then(|n| n.to_str()).unwrap_or(f);
                (basename.to_owned(), l)
            }
            _ => ("<unknown>".to_owned(), 0),
        };

        format_file(event_level, ts, target, &file_basename, line_num, msg)
    }

    #[test]
    fn file_format_no_ansi() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::INFO,
            "myapp",
            Some("src/main.rs"),
            Some(42),
            "hello world",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [INFO] myapp main.rs:42: hello world\n".as_bytes().to_vec()
        );
    }

    #[test]
    fn file_format_iso_timestamp() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::INFO,
            "myapp",
            Some("src/main.rs"),
            Some(1),
            "ts test",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [INFO] myapp main.rs:1: ts test\n".as_bytes().to_vec()
        );
    }

    #[test]
    fn build_file_layer_creates_missing_parent_dir() {
        let rand: u64 = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos().into(),
            Err(_) => panic!("system time should be after UNIX_EPOCH"),
        };
        let log_path = PathBuf::from(format!("target/tmp/test-mkdir-{rand}/sub/log/app.log"));
        let config = LogConfig {
            level: Level::Info,
            format: FormatMode::Normal,
            color: ColorMode::Never,
            log_file: Some(log_path.clone()),
        };
        let result = build_file_layer(&config, jiff::tz::TimeZone::UTC);
        assert!(result.is_ok(), "expected Ok but got Err");
        let dir = match log_path.parent() {
            Some(dir) => dir,
            None => panic!("log path should have a parent"),
        };
        assert!(dir.exists(), "parent dir should have been created");
    }

    #[test]
    fn build_file_layer_returns_error_for_unwritable_path() {
        let config = LogConfig {
            level: Level::Info,
            format: FormatMode::Normal,
            color: ColorMode::Never,
            log_file: Some(PathBuf::from("/dev/null/sub")),
        };
        let result = build_file_layer(&config, jiff::tz::TimeZone::UTC);
        assert!(
            matches!(result, Err(InitError::FileSetupFailed { .. })),
            "expected FileSetupFailed"
        );
    }

    #[test]
    fn golden_file_info_no_ansi() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::INFO,
            "myapp::module",
            Some("src/lib.rs"),
            Some(99),
            "golden info message",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [INFO] myapp::module lib.rs:99: golden info message\n"
                .as_bytes()
                .to_vec()
        );
    }

    #[test]
    fn golden_file_warn() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::WARN,
            "myapp",
            Some("src/warn.rs"),
            Some(7),
            "something warned",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [WARN] myapp warn.rs:7: something warned\n".as_bytes().to_vec()
        );
    }

    #[test]
    fn golden_file_unknown_file_line() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::ERROR,
            "myapp",
            None,
            None,
            "no location",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [ERROR] myapp <unknown>:0: no location\n".as_bytes().to_vec()
        );
    }

    #[test]
    fn golden_file_unicode_preserved() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::INFO,
            "myapp",
            Some("src/unicode.rs"),
            Some(1),
            "héllo wörld 日本語",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [INFO] myapp unicode.rs:1: héllo wörld 日本語\n"
                .as_bytes()
                .to_vec()
        );
    }

    #[test]
    fn golden_file_error() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            "2024-01-02 03:04:05",
            tracing::Level::ERROR,
            "myapp::core",
            Some("src/error.rs"),
            Some(13),
            "boom",
        );
        assert_eq!(
            bytes,
            "[2024-01-02 03:04:05] [ERROR] myapp::core error.rs:13: boom\n".as_bytes().to_vec()
        );
    }
}
