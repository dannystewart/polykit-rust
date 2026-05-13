use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::Arc;

use owo_colors::OwoColorize;
use tracing_subscriber::Layer;

use crate::builder::{LogConfig, TargetOverride, effective_min_level};
use crate::format::{ColorMode, FormatMode};
use crate::level::{Level, level_color};

pub(crate) struct ConsoleLayer {
    format: FormatMode,
    color_mode: ColorMode,
    tz: jiff::tz::TimeZone,
    target_overrides: Arc<[TargetOverride]>,
    strip_target_prefix: Option<Box<str>>,
}

impl ConsoleLayer {
    pub(crate) fn new_with_tz(config: &LogConfig, tz: jiff::tz::TimeZone) -> Self {
        Self {
            format: config.format,
            color_mode: config.color,
            tz,
            target_overrides: config.target_overrides.clone(),
            strip_target_prefix: config.strip_target_prefix.as_deref().map(Box::from),
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
        // Per-target override (if any) replaces the global min for matched targets;
        // otherwise the global min applies.
        let target = event.metadata().target();
        let min_level =
            effective_min_level(target, &self.target_overrides, crate::init::current_min_level());
        if level < min_level {
            return Vec::new();
        }

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);
        let mut msg = visitor.message;
        for (k, v) in visitor.fields {
            let _ = write!(&mut msg, " {k}={v}");
        }

        let ts =
            jiff::Zoned::now().with_time_zone(self.tz.clone()).strftime("%-I:%M:%S %p").to_string();

        let use_ansi =
            self.color_mode.should_emit_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));

        match self.format {
            FormatMode::Simple => format_simple(level, &msg, use_ansi),
            FormatMode::Normal => format_normal(level, &ts, &msg, use_ansi),
            FormatMode::Context => {
                let meta = event.metadata();
                // For events bridged from the `log` crate, `tracing_log` sets
                // meta.target() to "log" and stores the real target/file/line
                // as fields. Prefer those when available.
                let raw_target = visitor.log_target.as_deref().unwrap_or_else(|| meta.target());
                let target = self
                    .strip_target_prefix
                    .as_deref()
                    .and_then(|prefix| strip_module_prefix(raw_target, prefix))
                    .unwrap_or(raw_target);
                let file = visitor.log_file.as_deref().or_else(|| meta.file());
                let line = visitor.log_line.or_else(|| meta.line());
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
            let mut stderr = io::stderr().lock();
            let _ = stderr.write_all(&bytes);
            let _ = stderr.flush();
        }
    }
}

/// Strip `prefix::` from the front of `target` on an exact module-path boundary.
///
/// Returns `Some(rest)` when `target` starts with `prefix` followed by `::`,
/// and `None` otherwise (leaving the caller to use the original target).
/// This prevents false matches: `"surfer_lib"` strips from `"surfer_lib::foo"`
/// but not from `"surfer_library::foo"`.
fn strip_module_prefix<'a>(target: &'a str, prefix: &str) -> Option<&'a str> {
    target.strip_prefix(prefix)?.strip_prefix("::")
}

fn format_simple(level: Level, msg: &str, use_ansi: bool) -> Vec<u8> {
    let line = if use_ansi { msg.color(level_color(level)).to_string() } else { msg.to_string() };
    format!("{line}\n").into_bytes()
}

fn format_normal(level: Level, ts: &str, msg: &str, use_ansi: bool) -> Vec<u8> {
    let ts_str = if use_ansi { ts.bright_black().to_string() } else { ts.to_string() };
    let label = level.label();
    let label_str =
        if use_ansi { label.color(level_color(level)).to_string() } else { label.to_string() };
    let msg_str =
        if use_ansi { msg.color(level_color(level)).to_string() } else { msg.to_string() };
    format!("{ts_str} {label_str} {msg_str}\n").into_bytes()
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
    let ts_str = if use_ansi { ts.bright_black().to_string() } else { ts.to_string() };
    let label = level.label();
    let label_str =
        if use_ansi { label.color(level_color(level)).to_string() } else { label.to_string() };
    let target_str = if use_ansi { target.blue().to_string() } else { target.to_string() };

    let file_basename = file.map_or("<unknown>", |f| {
        std::path::Path::new(f).file_name().and_then(|n| n.to_str()).unwrap_or(f)
    });
    let line_num = line.unwrap_or(0);

    let loc = if use_ansi {
        format!("{file_basename}:{}", line_num.to_string().cyan())
    } else {
        format!("{file_basename}:{line_num}")
    };

    let msg_str =
        if use_ansi { msg.color(level_color(level)).to_string() } else { msg.to_string() };

    format!("{ts_str} {label_str} {target_str} {loc} {msg_str}\n").into_bytes()
}

struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
    /// Target from `log.target` field, set by `tracing_log::LogTracer` when
    /// bridging `log` crate events whose tracing metadata target is `"log"`.
    log_target: Option<String>,
    /// Source file from `log.file`, bridged from the original `log!()` callsite.
    log_file: Option<String>,
    /// Source line from `log.line`, bridged from the original `log!()` callsite.
    log_line: Option<u32>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: Vec::new(),
            log_target: None,
            log_file: None,
            log_line: None,
        }
    }
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        // Intercept log.* bridge fields — they carry the original log crate
        // metadata and should not appear as key=value pairs in the output.
        if name.starts_with("log.") {
            match name {
                "log.target" => {
                    self.log_target = Some(format!("{value:?}").trim_matches('"').to_owned())
                }
                "log.file" => {
                    self.log_file = Some(format!("{value:?}").trim_matches('"').to_owned())
                }
                _ => {} // log.module_path and others: discard
            }
            return;
        }
        let val = format!("{value:?}");
        if name == "message" {
            self.message = val;
        } else {
            let cleaned = crate::clean::clean_debug_value(&val);
            // Reuse the original allocation when nothing changed; otherwise
            // take the newly-produced String from the Cow.
            let val = match cleaned {
                std::borrow::Cow::Borrowed(_) => val,
                std::borrow::Cow::Owned(s) => s,
            };
            self.fields.push((name.to_string(), val));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let name = field.name();
        if name.starts_with("log.") {
            match name {
                "log.target" => self.log_target = Some(value.to_owned()),
                "log.file" => self.log_file = Some(value.to_owned()),
                _ => {} // log.module_path and others: discard
            }
            return;
        }
        if name == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "log.line" {
            self.log_line = u32::try_from(value).ok();
        } else {
            self.fields.push((field.name().to_string(), value.to_string()));
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.push((field.name().to_string(), value.to_string()));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.fields.push((field.name().to_string(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.push((field.name().to_string(), value.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn golden_normal_info_ansi() {
        let level = Level::Info;
        let ts = "2:34:09 PM";
        let msg = "hello world";
        let result = format_normal(level, ts, msg, true);
        let expected = format!(
            "{} {} {}\n",
            ts.bright_black(),
            level.label().color(level_color(level)),
            msg.color(level_color(level))
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
        let expected = format!("{} {} {}\n", ts, level.label(), msg).into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_info_plain() {
        let result = format_simple(Level::Info, "info msg", false);
        let expected = "info msg\n".as_bytes().to_vec();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_warn_ansi_is_level_colored() {
        let result = format_simple(Level::Warn, "warning!", true);
        let expected = format!("{}\n", "warning!".color(level_color(Level::Warn))).into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_simple_info_ansi_is_level_colored() {
        let result = format_simple(Level::Info, "info msg", true);
        let expected = format!("{}\n", "info msg".color(level_color(Level::Info))).into_bytes();
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
            Level::Error.label().color(level_color(Level::Error)),
            "my_crate::module".blue(),
            format_args!("file.rs:{}", "42".cyan()),
            "boom".color(level_color(Level::Error))
        )
        .into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_normal_multiline_message_is_level_colored() {
        let level = Level::Debug;
        let ts = "2:34:09 PM";
        let msg = "line1\nline2";
        let result = format_normal(level, ts, msg, true);
        let expected = format!(
            "{} {} {}\n",
            ts.bright_black(),
            level.label().color(level_color(level)),
            msg.color(level_color(level))
        )
        .into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_context_missing_file_line() {
        let result =
            format_context(Level::Debug, "2:34:09 PM", "my_crate", None, None, "msg", false);
        let expected =
            format!("2:34:09 PM {} my_crate <unknown>:0 msg\n", Level::Debug.label()).into_bytes();
        assert_eq!(result, expected);
    }

    #[test]
    fn golden_unicode_preserved() {
        let result = format_simple(Level::Info, "こんにちは 日本語", false);
        assert_eq!(result, "こんにちは 日本語\n".as_bytes().to_vec());
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
            fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {
            }
            fn event(&self, event: &tracing::Event<'_>) {
                let bytes = self.layer.render_event(event);
                self.output.lock().expect("mutex poisoned").extend_from_slice(&bytes);
            }
            fn enter(&self, _span: &tracing::span::Id) {}
            fn exit(&self, _span: &tracing::span::Id) {}
        }

        let layer = ConsoleLayer {
            format: FormatMode::Normal,
            color_mode: ColorMode::Never,
            tz: jiff::tz::TimeZone::UTC,
            target_overrides: Arc::from(Vec::<TargetOverride>::new()),
            strip_target_prefix: None,
        };

        let sub = Arc::new(CapturingSubscriber { layer, output: Mutex::new(Vec::new()) });

        let dispatch = tracing::dispatcher::Dispatch::new(Arc::clone(&sub));
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::debug!("this should be filtered");
        });

        let Ok(output) = sub.output.lock() else { panic!("mutex poisoned") };
        assert!(output.is_empty());
    }

    #[test]
    fn strip_module_prefix_strips_on_exact_boundary() {
        assert_eq!(strip_module_prefix("surfer_lib::foo::bar", "surfer_lib"), Some("foo::bar"));
    }

    #[test]
    fn strip_module_prefix_strips_single_segment() {
        assert_eq!(strip_module_prefix("surfer_lib::foo", "surfer_lib"), Some("foo"));
    }

    #[test]
    fn strip_module_prefix_does_not_match_non_boundary() {
        assert_eq!(strip_module_prefix("surfer_library::foo", "surfer_lib"), None);
    }

    #[test]
    fn strip_module_prefix_does_not_match_unrelated() {
        assert_eq!(strip_module_prefix("other_crate::foo", "surfer_lib"), None);
    }

    #[test]
    fn strip_module_prefix_exact_target_returns_empty_string() {
        // "surfer_lib" with no sub-module: strip_prefix("surfer_lib") → "",
        // then strip_prefix("::") fails → None. Correct: the whole string IS
        // the prefix, not a sub-module of it.
        assert_eq!(strip_module_prefix("surfer_lib", "surfer_lib"), None);
    }
}
