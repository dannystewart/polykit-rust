- Added `src/log/format.rs` as the home for console format/color mode enums and their test coverage.
- Re-exported `ColorMode` and `FormatMode` from `src/log/mod.rs` so callers can import them from `polykit::log`.
- Keep level overrides process-global, not thread-local, to match the logger's shared MIN_LEVEL state and the plan's documented divergence.
- Nested overrides restore in strict LIFO order via Drop.
- Documented `InitError`, `Level`, and `LogLevelOverride::new` so crate-level missing-docs lint stays enabled without breaking docs generation.

- Deleted the duplicate timezone resolver from `console.rs`; `init.rs` remains the single source of truth for TZ resolution.
- Kept file/console layers focused on rendering and I/O; min-level filtering stays centralized in `init.rs`.
- Added a pure file formatter helper in `file.rs` so tests can assert exact bytes without `contains`/substring matching.
