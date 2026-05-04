use std::path::PathBuf;
use std::sync::Arc;

use crate::{ColorMode, FormatMode, InitError, InitGuard, Level};

/// Builder for configuring and installing the polylog logger.
///
/// Obtain one via [`crate::init`], then chain setters and call [`install`](LogBuilder::install).
pub struct LogBuilder {
    level: Level,
    format: FormatMode,
    color: ColorMode,
    log_file: Option<PathBuf>,
    target_overrides: Vec<TargetOverride>,
}

impl Default for LogBuilder {
    fn default() -> Self {
        Self {
            level: Level::Info,
            format: FormatMode::Normal,
            color: ColorMode::Auto,
            log_file: None,
            target_overrides: Vec::new(),
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

    /// Set the minimum log level for events whose target matches `prefix`.
    ///
    /// Matches on module-path boundaries: `target_level("sqlx", Level::Warn)` affects
    /// `sqlx`, `sqlx::query`, `sqlx::pool`, etc., but NOT `sqlxie`. When multiple
    /// overrides match the same target, the longest matching prefix wins (most specific).
    ///
    /// The override fully replaces the global min level for matched targets — set the
    /// override above the global to silence a noisy dependency, or below the global to
    /// surface verbose output from a single area while keeping everything else quiet.
    ///
    /// Calling with the same `prefix` twice replaces the previous level for that prefix.
    ///
    /// # Examples
    ///
    /// Quiet sqlx's per-statement Debug spam while keeping its slow-statement WARNs:
    /// ```no_run
    /// # use polylog::Level;
    /// let _ = polylog::init()
    ///     .level(Level::Debug)
    ///     .target_level("sqlx", Level::Warn)
    ///     .install();
    /// ```
    pub fn target_level(mut self, prefix: impl Into<String>, level: Level) -> Self {
        let prefix = prefix.into();
        if let Some(existing) = self.target_overrides.iter_mut().find(|o| o.prefix == prefix) {
            existing.level = level;
        } else {
            self.target_overrides.push(TargetOverride { prefix, level });
        }
        self
    }

    /// Install the logger and return a guard that must remain in scope.
    pub fn install(self) -> Result<InitGuard, InitError> {
        crate::init::install_with_config(self)
    }
}

/// Single per-target level override stored on the builder. Crate-private; the public
/// surface is [`LogBuilder::target_level`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetOverride {
    pub(crate) prefix: String,
    pub(crate) level: Level,
}

/// Internal configuration derived from [`LogBuilder`].
pub(crate) struct LogConfig {
    pub(crate) level: Level,
    pub(crate) format: FormatMode,
    pub(crate) color: ColorMode,
    pub(crate) log_file: Option<PathBuf>,
    /// Pre-sorted overrides (longest prefix first) wrapped in an `Arc` so console + file
    /// layers share the same underlying slice without cloning per event.
    pub(crate) target_overrides: Arc<[TargetOverride]>,
}

impl From<LogBuilder> for LogConfig {
    fn from(b: LogBuilder) -> Self {
        let mut overrides = b.target_overrides;
        // Sort by descending prefix length so longest-prefix wins on first match in the
        // hot path. Stable sort preserves insertion order for equal-length prefixes.
        overrides.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()));
        Self {
            level: b.level,
            format: b.format,
            color: b.color,
            log_file: b.log_file,
            target_overrides: overrides.into(),
        }
    }
}

/// True when `target` matches `prefix` on a module-path boundary — exact match or
/// `prefix::*`. Avoids false positives like `"sqlxie"` matching the prefix `"sqlx"`.
pub(crate) fn target_matches(target: &str, prefix: &str) -> bool {
    if target == prefix {
        return true;
    }
    let prefix_len = prefix.len();
    target.len() > prefix_len + 1
        && target.as_bytes().get(..prefix_len) == Some(prefix.as_bytes())
        && target.as_bytes().get(prefix_len..prefix_len + 2) == Some(b"::")
}

/// Resolve the effective min level for an event with the given `target`. Returns the
/// most-specific (longest-prefix) override, or `global` when nothing matches.
pub(crate) fn effective_min_level(
    target: &str,
    overrides: &[TargetOverride],
    global: Level,
) -> Level {
    for entry in overrides {
        if target_matches(target, &entry.prefix) {
            return entry.level;
        }
    }
    global
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
        assert!(b.target_overrides.is_empty());
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

    #[test]
    fn target_level_appends_new_prefix() {
        let b = LogBuilder::new()
            .target_level("sqlx", Level::Warn)
            .target_level("hyper_util", Level::Warn);
        assert_eq!(b.target_overrides.len(), 2);
        assert_eq!(
            b.target_overrides[0],
            TargetOverride { prefix: "sqlx".into(), level: Level::Warn }
        );
        assert_eq!(
            b.target_overrides[1],
            TargetOverride { prefix: "hyper_util".into(), level: Level::Warn }
        );
    }

    #[test]
    fn target_level_replaces_existing_prefix_in_place() {
        let b =
            LogBuilder::new().target_level("sqlx", Level::Warn).target_level("sqlx", Level::Error);
        assert_eq!(b.target_overrides.len(), 1);
        assert_eq!(b.target_overrides[0].level, Level::Error);
    }

    #[test]
    fn config_sorts_overrides_by_descending_prefix_length() {
        let b = LogBuilder::new()
            .target_level("sqlx", Level::Warn)
            .target_level("sqlx::query", Level::Debug)
            .target_level("a", Level::Error);
        let config = LogConfig::from(b);
        let lengths: Vec<usize> = config.target_overrides.iter().map(|o| o.prefix.len()).collect();
        assert_eq!(lengths, vec!["sqlx::query".len(), "sqlx".len(), "a".len()]);
    }

    #[test]
    fn target_matches_exact_target() {
        assert!(target_matches("sqlx", "sqlx"));
    }

    #[test]
    fn target_matches_module_path_descendant() {
        assert!(target_matches("sqlx::query", "sqlx"));
        assert!(target_matches("sqlx::pool::inner", "sqlx"));
        assert!(target_matches("sqlx::pool::inner", "sqlx::pool"));
    }

    #[test]
    fn target_does_not_match_non_boundary_prefix() {
        assert!(!target_matches("sqlxie", "sqlx"));
        assert!(!target_matches("sqlx_extra", "sqlx"));
        assert!(!target_matches("sqlx:other", "sqlx"));
    }

    #[test]
    fn target_does_not_match_unrelated_target() {
        assert!(!target_matches("hyper_util::client", "sqlx"));
        assert!(!target_matches("", "sqlx"));
    }

    #[test]
    fn effective_min_level_falls_through_to_global_when_no_match() {
        let overrides: Vec<TargetOverride> =
            vec![TargetOverride { prefix: "sqlx".into(), level: Level::Warn }];
        assert_eq!(effective_min_level("prism::sync", &overrides, Level::Info), Level::Info);
    }

    #[test]
    fn effective_min_level_uses_override_when_target_matches() {
        let overrides: Vec<TargetOverride> =
            vec![TargetOverride { prefix: "sqlx".into(), level: Level::Warn }];
        assert_eq!(effective_min_level("sqlx::query", &overrides, Level::Debug), Level::Warn);
    }

    #[test]
    fn effective_min_level_picks_longest_matching_prefix() {
        // Overrides come pre-sorted by LogConfig::from; mimic that here.
        let overrides: Vec<TargetOverride> = vec![
            TargetOverride { prefix: "sqlx::query".into(), level: Level::Debug },
            TargetOverride { prefix: "sqlx".into(), level: Level::Warn },
        ];
        // sqlx::query matches both — the more specific (longer) prefix wins.
        assert_eq!(effective_min_level("sqlx::query", &overrides, Level::Info), Level::Debug);
        // sqlx::pool only matches "sqlx" — the less specific prefix is used.
        assert_eq!(effective_min_level("sqlx::pool", &overrides, Level::Info), Level::Warn);
    }

    #[test]
    fn effective_min_level_allows_override_below_global() {
        // Global is Warn but we want Debug from prism::sync specifically.
        let overrides: Vec<TargetOverride> =
            vec![TargetOverride { prefix: "prism::sync".into(), level: Level::Debug }];
        assert_eq!(
            effective_min_level("prism::sync::delta", &overrides, Level::Warn),
            Level::Debug
        );
    }
}
