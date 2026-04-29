# Learnings

## Conventions
- Crate name: `polykit` (renamed from `polykit-rust`)
- Edition 2024, MSRV 1.85
- No LICENSE file in v0.1 (user explicit)
- No unsafe code (`#![forbid(unsafe_code)]`)
- No emojis or sycophantic comments in code/docs
- Hand-roll Display/Error impls (no thiserror, no anyhow in public API)
- `#[must_use]` on guard types
- Process-global init idempotency via AtomicBool::compare_exchange
- Process-global MIN_LEVEL via AtomicU8
- Builder uses owned `self` (not &mut self)
- Single FormatMode enum (not conflicting booleans)
- ColorMode enum (Auto/Always/Never), not a bool
- Console output → stderr (not stdout)
- File output → plain ASCII, no ANSI, ISO 24-hour timestamp
- Daily file rotation via tracing-appender (documented divergence from Python's 512KB size-based)
- Time library: jiff (not chrono)
- TRACE events dropped (not rendered)
- No CRITICAL level in v0.1
- show_context format: `{ts} [{level}] {target} {file}:{line} {msg}`
- `catch` is closure-based, no catch_unwind
- catch walks Error::source() chain
- LogLevelOverride is process-global (documented divergence from Python per-logger)
- Tests that mutate global state must use --test-threads=1 or a static Mutex
- Subprocess-based integration tests for init behavior
- Formatter golden tests use exact assert_eq! on bytes, deterministic timestamps
- All verification is agent-executed (zero human intervention)

## Dependencies
- tracing = "0.1"
- tracing-subscriber = { version = "0.3", default-features = false, features = ["registry", "std"] }
- tracing-appender = "0.2"
- tracing-log = "0.2"
- owo-colors = { version = "4", features = ["supports-colors"] }
- anstream = "0.6"
- jiff = { version = "0.1", default-features = false, features = ["std", "tz-system"] }
- log = "0.4" (dev-dependency only, for bridge test)

## Rust Ecosystem Decisions
- Custom tracing_subscriber::Layer (not tracing_subscriber::fmt::Layer)
- No EnvFilter / RUST_LOG parsing
- No set_global_default (use try_init)
- No reload layer
- owo-colors for conditional coloring (supports NO_COLOR/FORCE_COLOR)
- anstream::stderr() for terminal-cap adaptation

## File Structure
src/lib.rs
src/log/mod.rs
src/log/level.rs
src/log/format.rs
src/log/error.rs
src/log/builder.rs
src/log/console.rs
src/log/file.rs
src/log/init.rs
src/log/level_override.rs
src/log/catch.rs

## Commit Messages (Conventional)
- Wave 1: `chore: bootstrap polykit crate with library scaffold and CI`
- Wave 2: `feat(log): add core types: Level, FormatMode, ColorMode, InitError, InitGuard`
- Wave 3: `feat(log): add builder, console layer, and file layer`
- Wave 4: `feat(log): wire init, level override, and catch helper`
- Wave 5: `feat(log): expose public API via module aggregator and lib root`
- Wave 6: `test(log): add formatter goldens, init subprocess tests, and smoke suite`

## CI Learnings
- GitHub Actions CI should stay minimal: fmt, clippy, test, doc only.
- Use `dtolnay/rust-toolchain@stable` plus `Swatinem/rust-cache@v2`; avoid extra actions.
- Keep warning policy strict in CI with `RUSTFLAGS=-D warnings` and `RUSTDOCFLAGS=-D warnings`.
