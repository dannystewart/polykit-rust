use std::path::PathBuf;

use crate::{ColorMode, FormatMode, InitError, InitGuard, Level};

/// Builder for configuring and installing the polylog logger.
///
/// Obtain one via [`crate::init`], then chain setters and call [`install`](LogBuilder::install).
pub struct LogBuilder {
    level: Level,
    format: FormatMode,
    color: ColorMode,
    log_file: Option<PathBuf>,
}

impl Default for LogBuilder {
    fn default() -> Self {
        Self {
            level: Level::Info,
            format: FormatMode::Normal,
            color: ColorMode::Auto,
            log_file: None,
        }
    }
}

impl LogBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the minimum log level.
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    /// Set the output format mode.
    pub fn format(mut self, format: FormatMode) -> Self {
        self.format = format;
        self
    }

    /// Set the color mode.
    pub fn color(mut self, color: ColorMode) -> Self {
        self.color = color;
        self
    }

    /// Enable file logging to the given path.
    pub fn log_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.log_file = Some(path.into());
        self
    }

    /// Install the logger and return a guard that must remain in scope.
    pub fn install(self) -> Result<InitGuard, InitError> {
        crate::init::install_with_config(self)
    }
}

/// Internal configuration derived from [`LogBuilder`].
pub(crate) struct LogConfig {
    pub(crate) level: Level,
    pub(crate) format: FormatMode,
    pub(crate) color: ColorMode,
    pub(crate) log_file: Option<PathBuf>,
}

impl From<LogBuilder> for LogConfig {
    fn from(b: LogBuilder) -> Self {
        Self { level: b.level, format: b.format, color: b.color, log_file: b.log_file }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn default_builder_has_expected_values() {
        let b = LogBuilder::default();
        assert_eq!(b.level, Level::Info);
        assert_eq!(b.format, FormatMode::Normal);
        assert_eq!(b.color, ColorMode::Auto);
        assert!(b.log_file.is_none());
    }

    #[test]
    fn setters_chain_correctly() {
        let b = LogBuilder::new()
            .level(Level::Debug)
            .format(FormatMode::Context)
            .color(ColorMode::Never);
        assert_eq!(b.level, Level::Debug);
        assert_eq!(b.format, FormatMode::Context);
        assert_eq!(b.color, ColorMode::Never);
        assert!(b.log_file.is_none());
    }

    #[test]
    fn log_file_accepts_str_pathbuf_path() {
        let b_str = LogBuilder::new().log_file("/tmp/test.log");
        assert_eq!(b_str.log_file, Some(PathBuf::from("/tmp/test.log")));

        let b_pathbuf = LogBuilder::new().log_file(PathBuf::from("/var/log/app.log"));
        assert_eq!(b_pathbuf.log_file, Some(PathBuf::from("/var/log/app.log")));

        let b_path = LogBuilder::new().log_file(std::path::Path::new("/etc/log/out.log"));
        assert_eq!(b_path.log_file, Some(PathBuf::from("/etc/log/out.log")));
    }
}
