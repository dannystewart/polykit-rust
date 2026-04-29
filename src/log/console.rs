use std::io::Write;
use std::sync::OnceLock;

use owo_colors::OwoColorize;
use tracing_subscriber::Layer;

use crate::log::builder::LogConfig;
use crate::log::format::{ColorMode, FormatMode};
use crate::log::level::{level_color, Level};

pub(crate) struct ConsoleLayer {
    format: FormatMode,
    color_mode: ColorMode,
    min_level: Level,
    tz: jiff::tz::TimeZone,
    #[allow(dead_code)]
    tz_warning_emitted: OnceLock<()>,
}

impl ConsoleLayer {
    #[allow(dead_code)]
    pub(crate) fn new(config: &LogConfig) -> Self {
        let tz_warning_emitted = OnceLock::new();
        let tz = resolve_tz(&tz_warning_emitted);
        Self {
            format: config.format,
            color_mode: config.color,
            min_level: config.level,
            tz,
            tz_warning_emitted,
        }
    }

    pub(crate) fn new_with_tz(config: &LogConfig, tz: jiff::tz::TimeZone) -> Self {
        Self {
            format: config.format,
            color_mode: config.color,
            min_level: config.level,
            tz,
            tz_warning_emitted: OnceLock::new(),
        }
    }

    pub(crate) fn render_event(&self, event: &tracing::Event<'_>) -> Vec<u8> {
        let tracing_level = event.metadata().level();
        if *tracing_level == tracing::Level::TRACE {
            return Vec::new();
        }

        let Some(level) = Level::from_tracing(*tracing_level) else {
            return Vec::new();
        };
        if level < self.min_level {
            return Vec::new();
        }

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);
        let mut msg = visitor.message;
        for (k, v) in visitor.fields {
            msg.push_str(&format!(" {}={}", k, v));
        }

        let ts = jiff::Zoned::now()
            .with_time_zone(self.tz.clone())
            .strftime("%-I:%M:%S %p")
            .to_string();

        let use_ansi = self
            .color_mode
            .should_emit_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));

        match self.format {
            FormatMode::Simple => format_simple(level, &msg, use_ansi),
            FormatMode::Normal => format_normal(level, &ts, &msg, use_ansi),
            FormatMode::Context => {
                let meta = event.metadata();
                let target = meta.target();
                let file = meta.file();
                let line = meta.line();
                format_context(level, &ts, target, file, line, &msg, use_ansi)
            }
        }
    }
}

impl<S> Layer<S> for ConsoleLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let bytes = self.render_event(event);
        if !bytes.is_empty() {
            let mut stderr = anstream::stderr();
            let _ = stderr.write_all(&bytes);
            let _ = stderr.flush();
        }
    }
}

fn resolve_tz(warning_lock: &OnceLock<()>) -> jiff::tz::TimeZone {
    if let Ok(tz_name) = std::env::var("TZ") {
        if !tz_name.is_empty() {
            match jiff::tz::TimeZone::get(&tz_name) {
                Ok(tz) => return tz,
                Err(_) => {
                    warning_lock.get_or_init(|| {
                        eprintln!(
                            "polykit::log: invalid TZ env var '{}'; falling back to America/New_York",
                            tz_name
                        );
                    });
                }
            }
        }
    }

    match jiff::tz::TimeZone::get("America/New_York") {
        Ok(tz) => tz,
        Err(_) => {
            eprintln!(
                "polykit::log: failed to load America/New_York timezone; falling back to UTC"
            );
            jiff::tz::TimeZone::UTC
        }
    }
}

fn format_simple(level: Level, msg: &str, use_ansi: bool) -> Vec<u8> {
    let line = if use_ansi && level >= Level::Warn {
        msg.bold().to_string()
    } else {
        msg.to_string()
    };
    format!("{}\n", line).into_bytes()
}

fn format_normal(level: Level, ts: &str, msg: &str, use_ansi: bool) -> Vec<u8> {
    let ts_str = if use_ansi {
        ts.bright_black().to_string()
    } else {
        ts.to_string()
    };
    let label = level.label();
    let label_str = if use_ansi {
        label.color(level_color(level)).to_string()
    } else {
        label.to_string()
    };
    format!("{} {} {}\n", ts_str, label_str, msg).into_bytes()
}

fn format_context(
    level: Level,
    ts: &str,
    target: &str,
    file: Option<&str>,
    line: Option<u32>,
    msg: &str,
    use_ansi: bool,
) -> Vec<u8> {
    let ts_str = if use_ansi {
        ts.bright_black().to_string()
    } else {
        ts.to_string()
    };
    let label = level.label();
    let label_str = if use_ansi {
        label.color(level_color(level)).to_string()
    } else {
        label.to_string()
    };
    let target_str = if use_ansi {
        target.blue().to_string()
    } else {
        target.to_string()
    };

    let file_basename = file
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(f)
        })
        .unwrap_or("<unknown>");
    let line_num = line.unwrap_or(0);

    let loc = if use_ansi {
        format!("{}:{}", file_basename, line_num.to_string().cyan())
    } else {
        format!("{}:{}", file_basename, line_num)
    };

    format!("{} {} {} {} {}\n", ts_str, label_str, target_str, loc, msg).into_bytes()
}

struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: Vec::new(),
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let val = format!("{:?}", value);
        if field.name() == "message" {
            self.message = val;
        } else {
            self.fields.push((field.name().to_string(), val));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    static TZ_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_tz_with_invalid_falls_back() {
        let _guard = TZ_TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("TZ", "Invalid/Bogus") };
        let warning_lock = OnceLock::new();
        let tz = resolve_tz(&warning_lock);
        // If the system has a tz database, we expect America/New_York;
        // otherwise resolve_tz falls back to UTC.
        let expected = jiff::tz::TimeZone::get("America/New_York")
            .unwrap_or_else(|_| jiff::tz::TimeZone::UTC);
        assert_eq!(tz.iana_name(), expected.iana_name());
        unsafe { std::env::remove_var("TZ") };
    }

    #[test]
    fn resolve_tz_unset_uses_default() {
        let _guard = TZ_TEST_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("TZ") };
        let warning_lock = OnceLock::new();
        let tz = resolve_tz(&warning_lock);
        let expected = jiff::tz::TimeZone::get("America/New_York")
            .unwrap_or_else(|_| jiff::tz::TimeZone::UTC);
        assert_eq!(tz.iana_name(), expected.iana_name());
    }

    #[test]
    fn golden_normal_info_ansi() {
        let level = Level::Info;
        let ts = "2:34:09 PM";
        let msg = "hello world";
        let result = format_normal(level, ts, msg, true);
        let expected = format!(
            "{} {} {}\n",
            ts.bright_black(),
            "[INFO]".color(level_color(level)),
            msg
        )
        .into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_normal_info_plain() {
        let level = Level::Info;
        let ts = "2:34:09 PM";
        let msg = "hello world";
        let result = format_normal(level, ts, msg, false);
        let expected = format!("{} {} {}\n", ts, "[INFO]", msg).into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_info_plain() {
        let result = format_simple(Level::Info, "info msg", false);
        let expected = "info msg\n".as_bytes().to_vec();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_warn_ansi_is_bold() {
        let result = format_simple(Level::Warn, "warning!", true);
        let expected = format!("{}\n", "warning!".bold()).into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_info_ansi_is_not_bold() {
        let result = format_simple(Level::Info, "info msg", true);
        let expected = "info msg\n".as_bytes().to_vec();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_context_full() {
        let result = format_context(
            Level::Error,
            "2:34:09 PM",
            "my_crate::module",
            Some("/path/to/file.rs"),
            Some(42),
            "boom",
            true,
        );
        let expected = format!(
            "{} {} {} {} {}\n",
            "2:34:09 PM".bright_black(),
            "[ERROR]".color(level_color(Level::Error)),
            "my_crate::module".blue(),
            format!("file.rs:{}", "42".cyan()),
            "boom"
        )
        .into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_context_missing_file_line() {
        let result = format_context(
            Level::Debug,
            "2:34:09 PM",
            "my_crate",
            None,
            None,
            "msg",
            false,
        );
        let expected = "2:34:09 PM [DEBUG] my_crate <unknown>:0 msg\n".as_bytes().to_vec();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_unicode_preserved() {
        let result = format_simple(Level::Info, "こんにちは 🎉", false);
        assert_eq!(result, "こんにちは 🎉\n".as_bytes().to_vec());
    }

    #[test]
    fn golden_multiline_message() {
        let result = format_simple(Level::Info, "line1\nline2", false);
        assert_eq!(result, "line1\nline2\n".as_bytes().to_vec());
    }

    #[test]
    fn golden_below_min_level_returns_empty() {
        struct CapturingSubscriber {
            layer: ConsoleLayer,
            output: Mutex<Vec<u8>>,
        }

        impl tracing::Subscriber for CapturingSubscriber {
            fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                let bytes = self.layer.render_event(event);
                self.output.lock().unwrap().extend_from_slice(&bytes);
            }
            fn enter(&self, _span: &tracing::span::Id) {}
            fn exit(&self, _span: &tracing::span::Id) {}
        }

        let layer = ConsoleLayer {
            format: FormatMode::Normal,
            color_mode: ColorMode::Never,
            min_level: Level::Warn,
            tz: jiff::tz::TimeZone::UTC,
            tz_warning_emitted: OnceLock::new(),
        };

        let sub = Arc::new(CapturingSubscriber {
            layer,
            output: Mutex::new(Vec::new()),
        });

        let dispatch = tracing::dispatcher::Dispatch::new(Arc::clone(&sub));
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("this should be filtered");
        });

        let output = sub.output.lock().unwrap();
        assert!(output.is_empty());
    }
}
