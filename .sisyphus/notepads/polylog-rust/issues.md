## 2026-04-29

- `tests/color_mode_env.rs` was flaky under parallel execution because it mutated `NO_COLOR` / `FORCE_COLOR` without locking.
- `tests/tz_env.rs` could not access `polykit::log::console::resolve_tz` from an integration test because `console` is private.
