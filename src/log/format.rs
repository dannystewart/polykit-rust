/// How log lines are formatted on the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    /// Message only. Bold if level ≥ Warn.
    Simple,
    /// Timestamp + level label + message.
    Normal,
    /// Timestamp + level label + target + file:line + message.
    Context,
}

impl Default for FormatMode {
    fn default() -> Self {
        FormatMode::Normal
    }
}

/// When ANSI color escape codes should be emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Respect `NO_COLOR` / `FORCE_COLOR` env vars, then fall back to TTY detection.
    Auto,
    /// Always emit ANSI codes regardless of environment or TTY state.
    Always,
    /// Never emit ANSI codes.
    Never,
}

impl Default for ColorMode {
    fn default() -> Self {
        ColorMode::Auto
    }
}

impl ColorMode {
    /// Determine whether ANSI codes should be emitted for the given target.
    ///
    /// Rules:
    /// - `Always` → true
    /// - `Never` → false
    /// - `Auto` → `NO_COLOR` (non-empty) → false;
    ///             `FORCE_COLOR` (non-empty) → true;
    ///             else `target_is_tty`
    pub fn should_emit_ansi(self, target_is_tty: bool) -> bool {
        match self {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                if std::env::var_os("NO_COLOR")
                    .is_some_and(|v| !v.is_empty())
                {
                    false
                } else if std::env::var_os("FORCE_COLOR")
                    .is_some_and(|v| !v.is_empty())
                {
                    true
                } else {
                    target_is_tty
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_is_normal() {
        assert_eq!(FormatMode::default(), FormatMode::Normal);
    }

    #[test]
    fn default_color_is_auto() {
        assert_eq!(ColorMode::default(), ColorMode::Auto);
    }

    #[test]
    fn color_always_emits_ansi_even_when_not_tty() {
        assert!(ColorMode::Always.should_emit_ansi(false));
    }

    #[test]
    fn color_never_strips_ansi_even_on_tty() {
        assert!(!ColorMode::Never.should_emit_ansi(true));
    }

    #[test]
    fn auto_respects_no_color_env_var() {
        unsafe { std::env::set_var("NO_COLOR", "1") };
        unsafe { std::env::remove_var("FORCE_COLOR") };
        assert!(!ColorMode::Auto.should_emit_ansi(true));
        unsafe { std::env::remove_var("NO_COLOR") };
    }

    #[test]
    fn auto_respects_force_color_env_var() {
        unsafe { std::env::set_var("FORCE_COLOR", "1") };
        unsafe { std::env::remove_var("NO_COLOR") };
        assert!(ColorMode::Auto.should_emit_ansi(false));
        unsafe { std::env::remove_var("FORCE_COLOR") };
    }

    #[test]
    fn auto_falls_back_to_tty_detection() {
        unsafe { std::env::remove_var("NO_COLOR") };
        unsafe { std::env::remove_var("FORCE_COLOR") };
        assert!(ColorMode::Auto.should_emit_ansi(true));
        assert!(!ColorMode::Auto.should_emit_ansi(false));
    }
}
