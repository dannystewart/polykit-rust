use std::io::Write as _;
use std::path::{Path, PathBuf};

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;

use crate::log::{InitError, Level};
use crate::log::builder::LogConfig;

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
    pub(crate) min_level: Level,
    pub(crate) tz: jiff::tz::TimeZone,
    pub(crate) writer: tracing_appender::non_blocking::NonBlocking,
}

pub(crate) fn build_file_layer(
    config: &LogConfig,
    tz: jiff::tz::TimeZone,
) -> Result<(FileLayer, tracing_appender::non_blocking::WorkerGuard), InitError> {
    let path = config
        .log_file
        .as_ref()
        .expect("file layer requires log_file");

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
            let fname = path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("polykit.log"));
            (dir, fname)
        }
    };

    std::fs::create_dir_all(&directory).map_err(|source| InitError::FileSetupFailed {
        path: path.clone(),
        source,
    })?;

    let file_appender = tracing_appender::rolling::daily(&directory, &filename);
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    Ok((FileLayer { min_level: config.level, tz, writer }, guard))
}

impl FileLayer {
    fn render_event(&self, event: &tracing::Event<'_>) -> Vec<u8> {
        let meta = event.metadata();

        if *meta.level() == tracing::Level::TRACE {
            return Vec::new();
        }

        let event_level = match Level::from_tracing(*meta.level()) {
            Some(l) => l,
            None => return Vec::new(),
        };
        if event_level < self.min_level {
            return Vec::new();
        }

        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let msg = visitor.0;

        let ts = jiff::Zoned::now()
            .with_time_zone(self.tz.clone())
            .strftime("%Y-%m-%d %H:%M:%S")
            .to_string();

        let (file_basename, line_str) = match (meta.file(), meta.line()) {
            (Some(f), Some(l)) => {
                let basename = Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(f);
                (basename.to_owned(), l.to_string())
            }
            _ => ("<unknown>".to_owned(), "0".to_owned()),
        };

        let target = meta.target();
        let label = event_level.label();

        let line = format!("[{ts}] {label} {target} {file_basename}:{line_str}: {msg}\n");
        line.into_bytes()
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
        std::fs::create_dir_all(&dir).unwrap();
        let appender = tracing_appender::rolling::daily(&dir, "test.log");
        let (writer, _guard) = tracing_appender::non_blocking(appender);
        FileLayer {
            min_level: Level::Debug,
            tz: jiff::tz::TimeZone::UTC,
            writer,
        }
    }

    fn render_direct(
        layer: &FileLayer,
        level: tracing::Level,
        target: &'static str,
        file: Option<&'static str>,
        line: Option<u32>,
        msg: &str,
    ) -> Vec<u8> {
        if level == tracing::Level::TRACE {
            return Vec::new();
        }

        let event_level = match Level::from_tracing(level) {
            Some(l) => l,
            None => return Vec::new(),
        };
        if event_level < layer.min_level {
            return Vec::new();
        }

        let ts = jiff::Zoned::now()
            .with_time_zone(layer.tz.clone())
            .strftime("%Y-%m-%d %H:%M:%S")
            .to_string();

        let (file_basename, line_str) = match (file, line) {
            (Some(f), Some(l)) => {
                let basename = std::path::Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(f);
                (basename.to_owned(), l.to_string())
            }
            _ => ("<unknown>".to_owned(), "0".to_owned()),
        };

        let label = event_level.label();
        let line_out = format!("[{ts}] {label} {target} {file_basename}:{line_str}: {msg}\n");
        line_out.into_bytes()
    }

    #[test]
    fn file_format_no_ansi() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            tracing::Level::INFO,
            "myapp",
            Some("src/main.rs"),
            Some(42),
            "hello world",
        );
        assert!(!bytes.is_empty());
        assert!(!bytes.contains(&0x1b), "output must not contain ANSI escape bytes");
    }

    #[test]
    fn file_format_iso_timestamp() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            tracing::Level::INFO,
            "myapp",
            Some("src/main.rs"),
            Some(1),
            "ts test",
        );
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.starts_with('['), "must start with '[': {s:?}");
        let bracket_end = s.find(']').expect("must have closing ']'");
        let ts_part = &s[1..bracket_end];
        assert_eq!(ts_part.len(), 19, "timestamp must be 19 chars: {ts_part:?}");
        assert_eq!(&ts_part[4..5], "-", "year-month separator: {ts_part:?}");
        assert_eq!(&ts_part[7..8], "-", "month-day separator: {ts_part:?}");
        assert_eq!(&ts_part[10..11], " ", "date-time separator: {ts_part:?}");
        assert_eq!(&ts_part[13..14], ":", "hour-min separator: {ts_part:?}");
        assert_eq!(&ts_part[16..17], ":", "min-sec separator: {ts_part:?}");
    }

    #[test]
    fn build_file_layer_creates_missing_parent_dir() {
        let rand: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            .into();
        let log_path = PathBuf::from(format!(
            "target/tmp/test-mkdir-{rand}/sub/log/app.log"
        ));
        let config = LogConfig {
            level: Level::Info,
            format: FormatMode::Normal,
            color: ColorMode::Never,
            log_file: Some(log_path.clone()),
        };
        let result = build_file_layer(&config, jiff::tz::TimeZone::UTC);
        assert!(result.is_ok(), "expected Ok but got Err");
        let dir = log_path.parent().unwrap();
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
            tracing::Level::INFO,
            "myapp::module",
            Some("src/lib.rs"),
            Some(99),
            "golden info message",
        );
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("[INFO]"), "missing [INFO]: {s:?}");
        assert!(s.contains("myapp::module"), "missing target: {s:?}");
        assert!(s.contains("lib.rs:99"), "missing file:line: {s:?}");
        assert!(s.contains("golden info message"), "missing msg: {s:?}");
        assert!(!s.contains('\x1b'), "must not contain ANSI: {s:?}");
    }

    #[test]
    fn golden_file_warn() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            tracing::Level::WARN,
            "myapp",
            Some("src/warn.rs"),
            Some(7),
            "something warned",
        );
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("[WARN]"), "missing [WARN]: {s:?}");
        assert!(s.contains("warn.rs:7"), "missing file:line: {s:?}");
        assert!(s.contains("something warned"), "missing msg: {s:?}");
    }

    #[test]
    fn golden_file_unknown_file_line() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            tracing::Level::ERROR,
            "myapp",
            None,
            None,
            "no location",
        );
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("<unknown>:0"), "missing <unknown>:0: {s:?}");
        assert!(s.contains("no location"), "missing msg: {s:?}");
    }

    #[test]
    fn golden_file_unicode_preserved() {
        let layer = make_layer();
        let bytes = render_direct(
            &layer,
            tracing::Level::INFO,
            "myapp",
            Some("src/unicode.rs"),
            Some(1),
            "héllo wörld 🦀",
        );
        let s = String::from_utf8(bytes).unwrap();
        assert!(s.contains("héllo wörld 🦀"), "unicode not preserved: {s:?}");
    }
}
