use owo_colors::AnsiColors;

/// Four log levels matching Python `polykit.log` parity.
///
/// `tracing::Level::TRACE` is not represented — TRACE events are dropped
/// by the formatter rather than rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// Bracketed label used in formatted output.
    pub const fn label(self) -> &'static str {
        match self {
            Level::Debug => "[DEBUG]",
            Level::Info => "[INFO]",
            Level::Warn => "[WARN]",
            Level::Error => "[ERROR]",
        }
    }

    /// Map to the corresponding `tracing::Level`.
    pub const fn as_tracing(self) -> tracing::Level {
        match self {
            Level::Debug => tracing::Level::DEBUG,
            Level::Info => tracing::Level::INFO,
            Level::Warn => tracing::Level::WARN,
            Level::Error => tracing::Level::ERROR,
        }
    }

    /// Parse from a `tracing::Level`.
    ///
    /// Returns `None` for `tracing::Level::TRACE`.
    pub fn from_tracing(level: tracing::Level) -> Option<Self> {
        match level {
            tracing::Level::DEBUG => Some(Level::Debug),
            tracing::Level::INFO => Some(Level::Info),
            tracing::Level::WARN => Some(Level::Warn),
            tracing::Level::ERROR => Some(Level::Error),
            tracing::Level::TRACE => None,
        }
    }

    /// Encode as a `u8` for atomic storage.
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Level::Debug => 0,
            Level::Info => 1,
            Level::Warn => 2,
            Level::Error => 3,
        }
    }

    /// Decode from a `u8`.
    pub(crate) fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Level::Debug),
            1 => Some(Level::Info),
            2 => Some(Level::Warn),
            3 => Some(Level::Error),
            _ => None,
        }
    }

    /// Parse from a string (case-insensitive).
    ///
    /// Accepts: "debug", "info", "warn", "warning", "error".
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

/// Internal helper: ANSI color for a given level.
pub(crate) fn level_color(level: Level) -> AnsiColors {
    match level {
        Level::Debug => AnsiColors::BrightBlack,
        Level::Info => AnsiColors::Green,
        Level::Warn => AnsiColors::Yellow,
        Level::Error => AnsiColors::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_matches_python_parity() {
        assert_eq!(Level::Debug.label(), "[DEBUG]");
        assert_eq!(Level::Info.label(), "[INFO]");
        assert_eq!(Level::Warn.label(), "[WARN]");
        assert_eq!(Level::Error.label(), "[ERROR]");
    }

    #[test]
    fn from_str_case_insensitive() {
        assert_eq!(Level::from_str("INFO"), Some(Level::Info));
        assert_eq!(Level::from_str("info"), Some(Level::Info));
        assert_eq!(Level::from_str("Info"), Some(Level::Info));
        assert_eq!(Level::from_str("warning"), Some(Level::Warn));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(Level::from_str("critical"), None);
        assert_eq!(Level::from_str("fatal"), None);
        assert_eq!(Level::from_str("trace"), None);
        assert_eq!(Level::from_str(""), None);
    }

    #[test]
    fn as_tracing_round_trip() {
        for level in [Level::Debug, Level::Info, Level::Warn, Level::Error] {
            assert_eq!(Level::from_tracing(level.as_tracing()), Some(level));
        }
    }

    #[test]
    fn from_tracing_trace_returns_none() {
        assert_eq!(Level::from_tracing(tracing::Level::TRACE), None);
    }
}
