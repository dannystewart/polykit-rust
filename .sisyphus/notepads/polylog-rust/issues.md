## 2026-04-29

- `tests/color_mode_env.rs` was flaky under parallel execution because it mutated `NO_COLOR` / `FORCE_COLOR` without locking.
- `tests/tz_env.rs` could not access `polykit::log::console::resolve_tz` from an integration test because `console` is private.

- Removed all `unwrap()` / `expect()` usage from `src/` and `examples/`, including test helpers, to satisfy review policy.
- Replaced emoji test strings with non-emoji Unicode (`日本語`) so unicode coverage remains without emoji characters.
- `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` all passed.
