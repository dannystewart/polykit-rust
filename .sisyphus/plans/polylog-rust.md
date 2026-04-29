# polykit-rust v0.1: Bootstrap `polykit` Crate + `log` Module (PolyLog Rust Port)

## TL;DR
> **Summary**: Bootstrap `/Users/danny/Developer/polykit-rust` as a proper Rust library crate named `polykit`, then build its first module `polykit::log` — a Python-parity port of `polykit.log` using tracing as the foundation.
> **Deliverables**:
> - Cargo package renamed `polykit-rust` → `polykit`, binary→library conversion
> - `polykit::log` module: builder API, 4 levels, 3 format modes, console + optional rolling file output, color modes, level override RAII guard, closure-based `catch` helper
> - Tracing macros (`tracing::info!` etc.) re-exported through `polykit::log`
> - README, expanded .gitignore, rust-toolchain.toml, GitHub Actions CI
> - Comprehensive tests: formatter golden tests, subprocess-based init tests
> **Effort**: Large
> **Parallel**: YES — 6 waves
> **Critical Path**: T1 (repo bootstrap) → T4-T6 (core types, parallel) → T7 (builder) → T8/T9 (console+file layers, parallel) → T10 (init) → T13 (module aggregator) → T15-T17 (tests)

## Context

### Original Request
> "I want to create a polykit-rust library in the noble traditions of my polykit and polykit-swift libraries... what I always need first is my logger. Can you take a look at /Users/danny/Developer/polykit/src/polykit/log (I think Python is a better fit for this one than Swift, most likely) and plan and build me a nice Rust logger?"

User explicitly identified Python as the primary reference.

### Interview Summary
Eight architectural decisions resolved with user (round 1 + Metis-surfaced round 2):
1. **Scope**: Python feature parity only. Defer Supabase remote, TimeAwareLogger, PolyEnv, all Swift-specific extras.
2. **API style**: Builder pattern + re-exported `tracing` macros. Crate re-exports `tracing` so consumers don't need a separate dep.
3. **Repo bootstrap**: Full bootstrap — convert binary→library, Cargo metadata, README, .gitignore, rust-toolchain, CI. **NO LICENSE in v0.1** (user explicitly excluded).
4. **Crate naming**: `polykit::log::` — single `polykit` crate, future modules become `polykit::time`, `polykit::env`, etc. Rename Cargo package from `polykit-rust` → `polykit`.
5. **CRITICAL level**: Dropped from v0.1. 4 levels (debug/info/warn/error), tracing-native.
6. **`show_context` mode format**: `{ts} [{level}] {target} {file}:{line} {msg}` — module + file:line, captured for free by tracing macros.
7. **File rotation**: Daily via `tracing-appender` (explicit divergence from Python's size-based 512KB).
8. **Init contract**: `init() -> Result<InitGuard, InitError>`. Second call returns `AlreadyInitialized` error. `InitGuard` must outlive program (holds `WorkerGuard` for file flush).
9. **Console+file routing**: When `log_file` set, BOTH console and file receive output (console colored AM/PM, file plain ISO). Matches Python.
10. **Color**: `ColorMode::{Auto, Always, Never}` enum, not a bool.
11. **catch/exception**: Closure-based for Result-returning code only. No `catch_unwind` in v0.1.
12. **Time library**: `jiff` (user's explicit choice).

### Metis Review (gaps addressed)
Metis flagged five major decision gates (crate naming, CRITICAL semantics, function-name fallback, file rotation, init idempotency) plus several smaller behavioral specs. All resolved in interview round 2 above. Metis-required hard exclusions captured in "Must NOT Have" below.

## Work Objectives

### Core Objective
Ship `polykit-rust` v0.1 as a proper, idiomatic, tested Rust library named `polykit` whose first module is `log` — providing a Python-parity logger built on `tracing` with builder configuration, colored console output, optional rolling file output, and ergonomic Rust-native helpers.

### Deliverables
- Renamed Cargo package: `polykit` (not `polykit-rust`)
- Library crate (no binary), edition 2024, MSRV 1.85
- `src/lib.rs` re-exporting `log` module
- `src/log/` module with: `level.rs`, `format.rs`, `error.rs`, `builder.rs`, `console.rs`, `file.rs`, `init.rs`, `level_override.rs`, `catch.rs`, `mod.rs`
- README.md (no LICENSE)
- `.gitignore` (Rust-standard)
- `rust-toolchain.toml` pinning stable
- `.github/workflows/ci.yml` with fmt + clippy + test + doc
- Unit tests in each module
- Formatter golden tests with deterministic time
- Subprocess-based integration tests for init idempotency
- README example compiling as doctest or examples/ binary

### Definition of Done (verifiable conditions with commands)
All of the following must succeed from `/Users/danny/Developer/polykit-rust`:
- `cargo fmt --all --check` → exits 0
- `cargo clippy --all-targets --all-features -- -D warnings` → exits 0
- `cargo test --all-targets --all-features` → all tests pass
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` → exits 0
- `cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].name'` → returns `polykit`
- `cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].targets[] | select(.kind[] == "lib") | .name'` → returns `polykit`
- `! [ -f src/main.rs ]` → main.rs deleted
- `[ -f src/lib.rs ]` → lib.rs exists
- The README example compiles (verified by `cargo test --doc` or `cargo build --examples`)

### Must Have
- Builder API: `polykit::log::init().level(...).format(...).color(...).log_file(...).init()`
- 4 levels rendered as `[DEBUG]` (gray), `[INFO]` (green), `[WARN]` (yellow), `[ERROR]` (red), matching Python's coloring exactly (Python's WARNING was abbreviated to `[WARN]` in the rendered label — preserve)
- 3 format modes: Simple (msg only, bold for warn+), Normal (`{ts} [{level}] {msg}`), Context (`{ts} [{level}] {target} {file}:{line} {msg}`)
- Timestamp format: console = "HH:MM:SS AM/PM" in gray, file = "YYYY-MM-DD HH:MM:SS"
- TZ env var support (default `America/New_York`); invalid TZ → fall back to default with one-time stderr warning
- ColorMode enum (Auto/Always/Never)
- File output rolls daily via tracing-appender; console + file both emit when `log_file` set
- `init()` returns `Result<InitGuard, InitError>`; second call returns `InitError::AlreadyInitialized`
- `InitGuard` holds `tracing_appender::WorkerGuard` so file output flushes on drop
- `LogLevelOverride` RAII guard for temporary level changes (scope-bound)
- `polykit::log::catch(closure)` helper that runs a Result-returning closure, logs the error chain on Err, returns the Result unchanged
- `tracing-log` bridge active so `log` crate calls from dependencies flow through
- Re-exports: `tracing` itself, plus `tracing::{debug, info, warn, error, trace, span, instrument}` at `polykit::log::` for convenience
- README example showing builder + a few log calls; example compiles as doctest

### Must NOT Have (guardrails — Metis-surfaced + scope boundaries)
- ❌ No `LICENSE` file in v0.1 (user explicit)
- ❌ No CRITICAL level / no custom `critical!` macro
- ❌ No Supabase remote handler
- ❌ No `TimeAwareLogger` / no `get_pretty_time` integration
- ❌ No `PolyEnv` integration / no env-driven config beyond `TZ`, `NO_COLOR`, `FORCE_COLOR`
- ❌ No Swift extras: no LogGroup, no in-memory capture buffer, no LogPersistence/zip export, no LogMeasurement, no observers
- ❌ No JSON formatter
- ❌ No span-tree / hierarchical / `tracing-tree`-style output
- ❌ No size-based file rotation (we chose daily time-based explicitly)
- ❌ No backtrace-parsing / stack-walking for function-name auto-detection (use `module_path!()` + `file!()` + `line!()` only)
- ❌ No `catch_unwind` panic catching helper
- ❌ No custom proc-macros / derive macros
- ❌ No workspace / multi-crate split (single `polykit` crate)
- ❌ No config file format / no env-parser abstraction
- ❌ No EnvFilter / `RUST_LOG` parsing — level is set by the builder only in v0.1
- ❌ No reload-layer / runtime subscriber swapping (level override uses a different mechanism)
- ❌ No silent "close enough" parity drift — every Python behavior either matches exactly or is documented as a deliberate divergence
- ❌ NO sycophantic comments or emojis in code/docs

## Verification Strategy

> ZERO HUMAN INTERVENTION — all verification is agent-executed.

- **Test framework**: Standard `cargo test` (built into Rust toolchain, no extra deps)
- **Test policy**: Tests live alongside implementation in same TODO ("Implementation + Test = ONE task" per template). Three classes of tests:
  1. **Inline unit tests** (`#[cfg(test)] mod tests`): per-module pure-function tests (level→label mapping, color mode resolution, format mode dispatch)
  2. **Formatter golden tests** (`tests/formatter_golden.rs`): deterministic timestamp + fixed `Vec<u8>` writer + controlled `ColorMode::Never` and `ColorMode::Always` → assert exact byte-for-byte expected output
  3. **Subprocess integration tests** (`tests/init_subprocess.rs`): spawn a small example binary multiple times to verify init idempotency, second-init error, pre-init log behavior, NO_COLOR env var, log_file output presence and absence of ANSI in file
- **Evidence**: Test output and inspection logs go to `.sisyphus/evidence/task-{N}-{slug}.txt`
- **CI**: GitHub Actions workflow runs the same Definition-of-Done commands on every push/PR

## Execution Strategy

### Parallel Execution Waves

> Target: 5-8 tasks per wave where possible. Some waves are smaller because of natural sequencing.

**Wave 1 — Repo bootstrap** (3 parallel):
- T1 [quick]: Bootstrap `polykit` library crate (Cargo.toml + lib.rs skeleton + delete main.rs + .gitignore + rust-toolchain.toml)
- T2 [writing]: README.md (no LICENSE)
- T3 [quick]: GitHub Actions CI workflow

**Wave 2 — Core types** (3 parallel, all depend on T1):
- T4 [quick]: `src/log/level.rs` — LogLevel + label/color tables
- T5 [quick]: `src/log/format.rs` — FormatMode + ColorMode enums
- T6 [quick]: `src/log/error.rs` — InitError + InitGuard

**Wave 3 — Builder & layers** (T7 first, then T8+T9 parallel):
- T7 [unspecified-low]: `src/log/builder.rs` — LogBuilder struct with setters
- T8 [unspecified-high]: `src/log/console.rs` — Custom tracing Layer for console (jiff timestamps, owo-colors, anstream, all 3 format modes)
- T9 [unspecified-low]: `src/log/file.rs` — File layer (tracing-appender daily rolling, plain formatter)

**Wave 4 — Init plumbing & helpers** (4 parallel, depend on T7-T9):
- T10 [unspecified-high]: `src/log/init.rs` — `init()` function, idempotency check, registers subscribers, tracing-log bridge
- T11 [quick]: `src/log/level_override.rs` — RAII level override guard
- T12 [quick]: `src/log/catch.rs` — closure-based catch helper

**Wave 5 — Aggregation** (sequenced after Wave 4):
- T13 [quick]: `src/log/mod.rs` — Module aggregator + re-exports (tracing macros, types, helpers)
- T14 [quick]: `src/lib.rs` — Top-level crate doc + log module re-export

**Wave 6 — Test suite** (3 parallel, after T14):
- T15 [unspecified-high]: `tests/formatter_golden.rs` — golden output tests for all 4 levels × 3 format modes × ColorMode variants
- T16 [unspecified-high]: `tests/init_subprocess.rs` — subprocess-based init/idempotency/pre-init/NO_COLOR/log_file tests
- T17 [unspecified-low]: README/example doctest — verify the README example compiles and runs

**Final Verification Wave** (4 parallel):
- F1 [oracle]: Plan compliance audit
- F2 [unspecified-high]: Code quality review
- F3 [unspecified-high]: Real manual QA (build + test + lint + doc + smoke run)
- F4 [deep]: Scope fidelity check

### Dependency Matrix

| Task | Depends On | Blocks |
|------|-----------|--------|
| T1   | (none)    | T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16, T17 |
| T2   | (none)    | T17 |
| T3   | (none)    | F3 |
| T4   | T1        | T7, T8, T11 |
| T5   | T1        | T7, T8, T9 |
| T6   | T1        | T10 |
| T7   | T4, T5, T6 | T8, T9, T10 |
| T8   | T7        | T10, T15 |
| T9   | T7        | T10, T15 |
| T10  | T6, T7, T8, T9 | T13, T16 |
| T11  | T4, T7    | T13 |
| T12  | T4, T7    | T13 |
| T13  | T10, T11, T12 | T14 |
| T14  | T13       | T15, T16, T17 |
| T15  | T8, T9, T14 | F-wave |
| T16  | T10, T14  | F-wave |
| T17  | T2, T14   | F-wave |
| F1-F4 | All implementation tasks | (none) |

### Agent Dispatch Summary

| Wave | Tasks | Categories Used |
|------|-------|----------------|
| 1    | 3     | quick × 2, writing × 1 |
| 2    | 3     | quick × 3 |
| 3    | 3     | unspecified-low × 2, unspecified-high × 1 |
| 4    | 3     | unspecified-high × 1, quick × 2 |
| 5    | 2     | quick × 2 |
| 6    | 3     | unspecified-high × 2, unspecified-low × 1 |
| Final | 4    | oracle × 1, unspecified-high × 2, deep × 1 |

## TODOs

- [x] 1. Bootstrap `polykit` library crate

  **What to do**:
  1. Edit `/Users/danny/Developer/polykit-rust/Cargo.toml`:
     - Change `name = "polykit-rust"` to `name = "polykit"`
     - Keep `edition = "2024"` and `version = "0.1.0"` as-is (edition 2024 IS valid as of Rust 1.85, Feb 2025)
     - Add `rust-version = "1.85"` (MSRV)
     - Add `description = "Polykit utility library for Rust"`
     - Add `repository = "https://github.com/dannystewart/polykit-rust"` (verify URL via `git remote -v` first; if unset, omit)
     - Add `[lib]` section with `name = "polykit"` (explicit, avoids any default-name confusion)
     - Add `[dependencies]` section with these crates (use latest published version constraints):
       - `tracing = "0.1"`
       - `tracing-subscriber = { version = "0.3", default-features = false, features = ["registry", "std"] }`
       - `tracing-appender = "0.2"`
       - `tracing-log = "0.2"`
       - `owo-colors = { version = "4", features = ["supports-colors"] }`
       - `anstream = "0.6"`
       - `jiff = { version = "0.1", default-features = false, features = ["std", "tz-system"] }` — if jiff has bumped to 0.2 by exec time, use latest stable; verify exact feature names with `cargo add --dry-run jiff` before locking
  2. Delete `/Users/danny/Developer/polykit-rust/src/main.rs`
  3. Create `/Users/danny/Developer/polykit-rust/src/lib.rs` with content:
     ```rust
     //! Polykit utility library for Rust.
     //!
     //! See module-level docs for details.

     pub mod log;
     ```
  4. Create directory `/Users/danny/Developer/polykit-rust/src/log/`
  5. Create `/Users/danny/Developer/polykit-rust/src/log/mod.rs` skeleton with content:
     ```rust
     //! Branded logging built on `tracing`.
     //!
     //! See [`init`] and the module-level documentation for usage.

     mod builder;
     mod catch;
     mod console;
     mod error;
     mod file;
     mod format;
     mod init;
     mod level;
     mod level_override;
     ```
     (NOTE: this will fail to compile until later tasks land; that's expected. Mark T1 acceptance with `cargo check` SKIPPED — re-check at end of Wave 5.)
  6. Replace `/Users/danny/Developer/polykit-rust/.gitignore` with:
     ```
     /target
     **/*.rs.bk
     .DS_Store
     .idea/
     .vscode/
     *.swp
     ```
     Do NOT add `Cargo.lock` — for library crates the modern guidance is to commit it for reproducibility of CI/dev workflows.
  7. Create `/Users/danny/Developer/polykit-rust/rust-toolchain.toml`:
     ```toml
     [toolchain]
     channel = "stable"
     components = ["clippy", "rustfmt"]
     profile = "minimal"
     ```

  **Must NOT do**:
  - Do NOT add a LICENSE file (user explicitly excluded)
  - Do NOT add any dependencies beyond the list above
  - Do NOT add `RUST_LOG`/`EnvFilter` parsing
  - Do NOT use `chrono` — we use `jiff`
  - Do NOT create a workspace `Cargo.toml`
  - Do NOT pre-implement module bodies — those are later tasks
  - Do NOT include emojis or sycophantic comments

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Mechanical scaffolding, all decisions made, no judgment needed
  - Skills: none required — Cargo.toml editing and small file creation are stock toolchain work

  **Parallelization**: Can Parallel: NO (foundational) | Wave 1 | Blocks: T2, T3 (technically T2/T3 don't depend on T1's contents but conceptually belong in same wave); Blocks ALL of T4-T17 | Blocked By: (none)

  **References**:
  - Sibling project Cargo.toml: `/Users/danny/Developer/polykit/pyproject.toml` (for description style and consistency)
  - Sibling project Cargo.toml: `/Users/danny/Developer/polykit-swift/Package.swift` (likewise)
  - Rust 2024 edition stabilization: Rust 1.85 release notes (Feb 2025)
  - jiff feature flags: <https://docs.rs/jiff/latest/jiff/> — verify `tz-system` feature name at exec time
  - tracing-subscriber registry pattern: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/registry/index.html>
  - Existing repo state confirmed: `Cargo.toml` exists with name `polykit-rust`, `src/main.rs` exists with hello-world, `.gitignore` has only `/target`. NO commits yet on `main` branch.

  **Acceptance Criteria**:
  - [ ] `cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].name'` outputs `polykit`
  - [ ] `cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].edition'` outputs `2024`
  - [ ] `cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].rust_version'` outputs `1.85`
  - [ ] `[ ! -f /Users/danny/Developer/polykit-rust/src/main.rs ]` succeeds (main.rs deleted)
  - [ ] `[ -f /Users/danny/Developer/polykit-rust/src/lib.rs ]` succeeds
  - [ ] `[ -d /Users/danny/Developer/polykit-rust/src/log ]` succeeds
  - [ ] `[ -f /Users/danny/Developer/polykit-rust/src/log/mod.rs ]` succeeds
  - [ ] `[ -f /Users/danny/Developer/polykit-rust/rust-toolchain.toml ]` succeeds
  - [ ] `[ ! -f /Users/danny/Developer/polykit-rust/LICENSE ]` succeeds (no LICENSE created)
  - [ ] `grep -q '^/target$' /Users/danny/Developer/polykit-rust/.gitignore` succeeds
  - [ ] All seven dependencies (tracing, tracing-subscriber, tracing-appender, tracing-log, owo-colors, anstream, jiff) appear in `cargo metadata` output

  **QA Scenarios**:
  ```
  Scenario: Library metadata is correct
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo metadata --format-version 1 --no-deps > .sisyphus/evidence/task-1-metadata.json
      jq -r '.packages[0] | "\(.name) \(.edition) \(.rust_version)"' .sisyphus/evidence/task-1-metadata.json
    Expected: Output is exactly `polykit 2024 1.85`
    Evidence: .sisyphus/evidence/task-1-metadata.json

  Scenario: Dependency list is exactly the seven crates we agreed to
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo metadata --format-version 1 --no-deps | \
        jq -r '.packages[0].dependencies[] | .name' | sort > .sisyphus/evidence/task-1-deps.txt
      cat .sisyphus/evidence/task-1-deps.txt
    Expected: Output is exactly these 7 lines (sorted): anstream, jiff, owo-colors, tracing, tracing-appender, tracing-log, tracing-subscriber
    Evidence: .sisyphus/evidence/task-1-deps.txt

  Scenario: No LICENSE file exists
    Tool: Bash
    Steps:
      ls /Users/danny/Developer/polykit-rust/LICENSE 2>&1 | tee .sisyphus/evidence/task-1-no-license.txt
    Expected: stderr contains "No such file or directory"; command exits non-zero
    Evidence: .sisyphus/evidence/task-1-no-license.txt
  ```

  **Commit**: NO (commit at end of Wave 1 along with T2 and T3 as a single "scaffold" commit)

- [x] 2. Write README.md

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/README.md` with this structure (no emojis, no marketing fluff):
     ```markdown
     # polykit

     Personal utility library for Rust, sibling to [polykit](https://github.com/dannystewart/polykit) (Python) and [polykit-swift](https://github.com/dannystewart/polykit-swift) (Swift).

     ## Status

     Early development. v0.1 ships the `log` module only.

     ## Modules

     - `polykit::log` — Branded logger built on [`tracing`](https://docs.rs/tracing). Console + optional rolling file output, three format modes, color modes, and ergonomic helpers.

     Future modules (not yet shipped): `polykit::time`, `polykit::env`, etc.

     ## Quickstart

         use polykit::log;

         fn main() -> anyhow::Result<()> {
             let _guard = log::init()
                 .level(log::Level::Info)
                 .format(log::FormatMode::Context)
                 .color(log::ColorMode::Auto)
                 .install()?;

             log::info!("hello from polykit");
             log::warn!(user_id = 42, "elevated event");

             Ok(())
         }

     The `_guard` must remain in scope for the lifetime of the program (it flushes the file writer on drop).

     ## Requirements

     - Rust 1.85+ (edition 2024)

     ## License

     TBD.
     ```
     (NOTE: example uses `anyhow::Result`; that's an example dependency, not a polykit dep. README is example-only — actual `Quickstart` doctest is verified separately in T17. The example here is illustrative narrative, not the doctest source.)
  2. Verify by reading back the file with `cat`.

  **Must NOT do**:
  - No emojis
  - No "✨ Features" / "🚀 Quickstart" decorations
  - No "Why polykit?" marketing section
  - Do NOT include actual LICENSE text — License section says "TBD"
  - Do NOT mention deferred features (Supabase, time-aware, etc.) as "coming soon" — only mention what v0.1 ships
  - Do NOT claim CRITICAL is supported — it is not

  **Recommended Agent Profile**:
  - Category: `writing` — Reason: Documentation prose, structure matters
  - Skills: none required

  **Parallelization**: Can Parallel: YES | Wave 1 | Blocks: T17 | Blocked By: (none — does not depend on T1; can run in parallel)

  **References**:
  - Sibling READMEs: `/Users/danny/Developer/polykit/README.md`, `/Users/danny/Developer/polykit-swift/README.md` (match structure but not feature claims — those are different libraries)
  - Quickstart code mirrors the planned T13 public API (subject to refinement)

  **Acceptance Criteria**:
  - [ ] `[ -f /Users/danny/Developer/polykit-rust/README.md ]` succeeds
  - [ ] `wc -l < /Users/danny/Developer/polykit-rust/README.md` outputs ≥ 30 (substantial content)
  - [ ] `! grep -E '[\x{1F300}-\x{1FAFF}]|[\x{2600}-\x{26FF}]' /Users/danny/Developer/polykit-rust/README.md` succeeds (no emoji unicode ranges)
  - [ ] `grep -q '^# polykit$' /Users/danny/Developer/polykit-rust/README.md` succeeds
  - [ ] `grep -q 'polykit::log' /Users/danny/Developer/polykit-rust/README.md` succeeds
  - [ ] `grep -q 'CRITICAL' /Users/danny/Developer/polykit-rust/README.md` returns no matches (we explicitly don't ship CRITICAL)
  - [ ] `grep -q 'Supabase' /Users/danny/Developer/polykit-rust/README.md` returns no matches (deferred feature not advertised)

  **QA Scenarios**:
  ```
  Scenario: README is well-formed and free of forbidden content
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cp README.md .sisyphus/evidence/task-2-readme.md
      grep -c '^##' README.md  # heading count
      grep -E '[\x{1F300}-\x{1FAFF}]|[\x{2600}-\x{26FF}]' README.md && echo "EMOJI FOUND" || echo "OK"
      grep -E 'CRITICAL|Supabase|TimeAware|PolyEnv' README.md && echo "FORBIDDEN FOUND" || echo "OK"
    Expected: heading count ≥ 4; both grep checks output "OK"
    Evidence: .sisyphus/evidence/task-2-readme.md
  ```

  **Commit**: NO (combined with T1 + T3)

- [x] 3. GitHub Actions CI workflow

  **What to do**:
  1. Create directory `/Users/danny/Developer/polykit-rust/.github/workflows/`
  2. Create `/Users/danny/Developer/polykit-rust/.github/workflows/ci.yml`:
     ```yaml
     name: CI

     on:
       push:
         branches: [main]
       pull_request:
         branches: [main]

     env:
       CARGO_TERM_COLOR: always
       RUSTFLAGS: -D warnings

     jobs:
       fmt:
         name: rustfmt
         runs-on: ubuntu-latest
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
             with:
               components: rustfmt
           - run: cargo fmt --all --check

       clippy:
         name: clippy
         runs-on: ubuntu-latest
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
             with:
               components: clippy
           - uses: Swatinem/rust-cache@v2
           - run: cargo clippy --all-targets --all-features -- -D warnings

       test:
         name: test
         runs-on: ${{ matrix.os }}
         strategy:
           matrix:
             os: [ubuntu-latest, macos-latest]
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
           - uses: Swatinem/rust-cache@v2
           - run: cargo test --all-targets --all-features

       doc:
         name: doc
         runs-on: ubuntu-latest
         env:
           RUSTDOCFLAGS: -D warnings
         steps:
           - uses: actions/checkout@v4
           - uses: dtolnay/rust-toolchain@stable
           - uses: Swatinem/rust-cache@v2
           - run: cargo doc --no-deps --all-features
     ```

  **Must NOT do**:
  - Do NOT add release/publish workflows in v0.1
  - Do NOT add Windows to test matrix in v0.1 (focus on macOS+Linux; Windows added later if needed)
  - Do NOT add codecov / coverage upload
  - Do NOT pin specific Rust versions in CI (use `stable` — repo's `rust-toolchain.toml` from T1 takes precedence anyway)
  - Do NOT add any third-party action that isn't actions/checkout, dtolnay/rust-toolchain, or Swatinem/rust-cache

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Standard CI yaml, well-known patterns
  - Skills: none required

  **Parallelization**: Can Parallel: YES | Wave 1 | Blocks: F3 (real QA wants CI workflow file present so it can be linted) | Blocked By: (none)

  **References**:
  - dtolnay/rust-toolchain: <https://github.com/dtolnay/rust-toolchain>
  - Swatinem/rust-cache: <https://github.com/Swatinem/rust-cache>
  - Sibling Python project workflows: `/Users/danny/Developer/polykit/.github/workflows/` (style reference only — different language, different commands)

  **Acceptance Criteria**:
  - [ ] `[ -f /Users/danny/Developer/polykit-rust/.github/workflows/ci.yml ]` succeeds
  - [ ] `python3 -c "import yaml; yaml.safe_load(open('/Users/danny/Developer/polykit-rust/.github/workflows/ci.yml'))"` succeeds (valid YAML)
  - [ ] `grep -q 'cargo fmt --all --check' /Users/danny/Developer/polykit-rust/.github/workflows/ci.yml` succeeds
  - [ ] `grep -q 'cargo clippy.*-D warnings' /Users/danny/Developer/polykit-rust/.github/workflows/ci.yml` succeeds
  - [ ] `grep -q 'cargo test --all-targets --all-features' /Users/danny/Developer/polykit-rust/.github/workflows/ci.yml` succeeds
  - [ ] `grep -q 'cargo doc --no-deps' /Users/danny/Developer/polykit-rust/.github/workflows/ci.yml` succeeds
  - [ ] Workflow defines exactly 4 jobs: fmt, clippy, test, doc

  **QA Scenarios**:
  ```
  Scenario: CI workflow is valid YAML and has expected jobs
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cp .github/workflows/ci.yml .sisyphus/evidence/task-3-ci.yml
      python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); jobs=sorted(d['jobs'].keys()); print('jobs:',jobs); assert jobs==['clippy','doc','fmt','test'], f'unexpected jobs: {jobs}'"
    Expected: prints `jobs: ['clippy', 'doc', 'fmt', 'test']`; no AssertionError
    Evidence: .sisyphus/evidence/task-3-ci.yml
  ```

  **Commit**: YES — at the END of Wave 1 (after T1, T2, T3 all complete) | Message: `chore: bootstrap polykit crate with library scaffold and CI` | Files: Cargo.toml, src/lib.rs, src/log/mod.rs, .gitignore, rust-toolchain.toml, README.md, .github/workflows/ci.yml; deleted: src/main.rs

- [x] 4. `src/log/level.rs` — LogLevel enum + label/color tables

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/level.rs` defining:
     - `pub enum Level { Debug, Info, Warn, Error }` (NB: `Warn` not `Warning` — the rendered label is `[WARN]` per Python parity)
     - `impl Level`:
       - `pub const fn label(self) -> &'static str` returning `"[DEBUG]"`, `"[INFO]"`, `"[WARN]"`, `"[ERROR]"`
       - `pub const fn as_tracing(self) -> tracing::Level` mapping Debug→TRACE? NO — Debug→DEBUG. Info→INFO. Warn→WARN. Error→ERROR.
       - `pub fn from_tracing(level: tracing::Level) -> Option<Self>` returning Some for the four mapped levels, None for TRACE (we don't expose TRACE in our public API but tracing TRACE events from libraries should still flow through — TRACE just won't render under our formatter; route TRACE→treat as Debug for rendering OR drop. Decision: drop TRACE entirely from our formatter — if we see a TRACE event, skip it. Document this in the rustdoc comment.)
       - `pub fn from_str(s: &str) -> Option<Self>` accepting "debug", "info", "warn"/"warning", "error" (case-insensitive)
     - `pub(crate) fn level_color(level: Level) -> owo_colors::AnsiColors`:
       - Debug → `AnsiColors::BrightBlack` (gray)
       - Info → `AnsiColors::Green`
       - Warn → `AnsiColors::Yellow`
       - Error → `AnsiColors::Red`
  2. Add inline unit tests:
     - `label_matches_python_parity` — assert exact strings `[DEBUG]`/`[INFO]`/`[WARN]`/`[ERROR]`
     - `from_str_case_insensitive` — assert "INFO", "info", "Info" all parse to Info; "warning" → Warn
     - `from_str_unknown_returns_none` — assert "critical", "fatal", "trace", "" return None
     - `as_tracing_round_trip` — assert from_tracing(level.as_tracing()) == Some(level) for all 4 levels
     - `from_tracing_trace_returns_none` — assert from_tracing(tracing::Level::TRACE) == None
  3. Update `src/log/mod.rs`: change `mod level;` to `pub use level::Level;` (re-export the type publicly via mod.rs at this stage; keep submodule private)

  **Must NOT do**:
  - Do NOT add a `Critical` / `Fatal` variant
  - Do NOT add a `Trace` variant — TRACE is dropped, not exposed
  - Do NOT use the literal string `"WARNING"` for the label (Python uses `[WARN]`, we match)
  - Do NOT use `chrono` colors / styles — `owo_colors::AnsiColors` only
  - Do NOT make `level_color` public (it's an internal formatter helper)

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Small enum + table + tests, all decisions made
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T5, T6) | Wave 2 | Blocks: T7, T8, T11 | Blocked By: T1

  **References**:
  - Python `LogLevel` and `LEVEL_COLORS`: `/Users/danny/Developer/polykit/src/polykit/log/types.py` (lines defining the enum and color mapping — labels and colors must match Python exactly except `[WARN]` for WARNING is preserved as Python rendered it)
  - `owo-colors` v4 colors: <https://docs.rs/owo-colors/4/owo_colors/enum.AnsiColors.html>
  - `tracing::Level`: <https://docs.rs/tracing/0.1/tracing/struct.Level.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/level.rs` exists with the `Level` enum and 4 variants
  - [ ] `cargo check -p polykit` succeeds (only after T5+T6 land if mod.rs references them; for T4 in isolation, run `cargo check` and confirm `level.rs` itself compiles — may need to temporarily comment out other module decls in mod.rs and revert)
  - [ ] `cargo test --lib level::tests` runs 5 tests, all pass
  - [ ] No occurrence of `Critical`, `Fatal`, or `Trace` as enum variant identifiers
  - [ ] No occurrence of the string `"WARNING"` as a label

  **QA Scenarios**:
  ```
  Scenario: Level labels match Python parity exactly
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib level::tests::label_matches_python_parity 2>&1 | tee .sisyphus/evidence/task-4-labels.txt
    Expected: test output contains "test result: ok. 1 passed"
    Evidence: .sisyphus/evidence/task-4-labels.txt

  Scenario: TRACE is not accepted as a level
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib level::tests::from_str_unknown_returns_none 2>&1 | tee .sisyphus/evidence/task-4-no-trace.txt
    Expected: test passes
    Evidence: .sisyphus/evidence/task-4-no-trace.txt

  Scenario: No forbidden identifiers in source
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E '\b(Critical|Fatal|Trace)\b' src/log/level.rs && echo "FORBIDDEN" || echo "OK"
      grep '"WARNING"' src/log/level.rs && echo "WARNING-LABEL FOUND" || echo "OK"
    Expected: both lines print "OK"
    Evidence: terminal output captured by agent
  ```

  **Commit**: NO (combine with T5, T6 as a single Wave 2 commit)

- [x] 5. `src/log/format.rs` — FormatMode + ColorMode enums

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/format.rs` defining:
     - `pub enum FormatMode { Simple, Normal, Context }` with `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`
     - `impl Default for FormatMode { fn default() -> Self { FormatMode::Normal } }` (matches Python default)
     - `pub enum ColorMode { Auto, Always, Never }` with `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`
     - `impl Default for ColorMode { fn default() -> Self { ColorMode::Auto } }`
     - `impl ColorMode`:
       - `pub fn should_emit_ansi(self, target_is_tty: bool) -> bool` returning:
         - `Always` → true unconditionally
         - `Never` → false unconditionally
         - `Auto` → defer to env: if `NO_COLOR` set non-empty → false; else if `FORCE_COLOR` set non-empty → true; else `target_is_tty`
       - Use `std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())` and same for FORCE_COLOR
  2. Add inline unit tests using `temp_env` pattern (or std::env directly with `Mutex`-guarded test if needed — for simplicity, wrap each test that touches env in a serialized `#[test]` and use `unsafe { std::env::set_var(...) }` / `remove_var` carefully; tests must remove vars at end):
     - `default_format_is_normal`
     - `default_color_is_auto`
     - `color_always_emits_ansi_even_when_not_tty`
     - `color_never_strips_ansi_even_on_tty`
     - `auto_respects_no_color_env_var`
     - `auto_respects_force_color_env_var`
     - `auto_falls_back_to_tty_detection`
  3. Update `src/log/mod.rs`: add `pub use format::{ColorMode, FormatMode};`

  **Must NOT do**:
  - Do NOT call `anstream` or `supports-color` directly inside `should_emit_ansi` — pass `target_is_tty: bool` in. The console layer (T8) supplies this from `std::io::IsTerminal::is_terminal(&io::stderr())`. This keeps `format.rs` deterministically testable.
  - Do NOT add additional FormatMode variants like `Json` or `Pretty`
  - Do NOT add `ColorMode::Detect` or other variants

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Small enums with focused logic, env-var rules well-defined
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T4, T6) | Wave 2 | Blocks: T7, T8, T9 | Blocked By: T1

  **References**:
  - Python format determinism: `/Users/danny/Developer/polykit/src/polykit/log/formatters.py` — `simple` and `show_context` boolean fields. We collapse to a single enum to make conflicting states (both true) unrepresentable per Metis directive.
  - NO_COLOR convention: <https://no-color.org>
  - FORCE_COLOR convention: <https://force-color.org> (less standardized but widely supported)
  - `std::io::IsTerminal` (Rust 1.70+): <https://doc.rust-lang.org/std/io/trait.IsTerminal.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/format.rs` exists with `FormatMode` and `ColorMode` enums
  - [ ] `FormatMode::default() == FormatMode::Normal` and `ColorMode::default() == ColorMode::Auto`
  - [ ] `cargo test --lib format::tests` runs 7 tests, all pass
  - [ ] No `Json`/`Pretty` FormatMode variants; no `Detect` ColorMode variant

  **QA Scenarios**:
  ```
  Scenario: ColorMode behaves correctly across env-var combinations
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib format::tests 2>&1 | tee .sisyphus/evidence/task-5-format-tests.txt
    Expected: "test result: ok. 7 passed"
    Evidence: .sisyphus/evidence/task-5-format-tests.txt

  Scenario: No forbidden FormatMode/ColorMode variants exist
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E '\b(Json|Pretty|Detect)\b' src/log/format.rs && echo "FORBIDDEN" || echo "OK"
    Expected: prints "OK"
    Evidence: captured stdout
  ```

  **Commit**: NO (combine with T4, T6)

- [x] 6. `src/log/error.rs` — InitError + InitGuard

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/error.rs` defining:
     - `pub enum InitError` with variants:
       - `AlreadyInitialized` (no payload)
       - `FileSetupFailed { path: std::path::PathBuf, source: std::io::Error }` — used when log_file's parent directory cannot be created or the appender setup fails
       - `SetGlobalDefaultFailed(tracing::dispatcher::SetGlobalDefaultError)` — wraps tracing's own error if setting the global subscriber fails
     - `impl std::fmt::Display for InitError` with messages:
       - AlreadyInitialized → `"polykit::log already initialized; init() may only be called once per process"`
       - FileSetupFailed → `"failed to set up log file at {path}: {source}"`
       - SetGlobalDefaultFailed → `"failed to install tracing subscriber: {0}"`
     - `impl std::error::Error for InitError` with `source()` returning the inner error for the relevant variants
     - `pub struct InitGuard` holding `Option<tracing_appender::non_blocking::WorkerGuard>` (None when no log_file was configured)
     - `impl InitGuard`:
       - `pub(crate) fn empty() -> Self` returning `InitGuard { worker: None }`
       - `pub(crate) fn with_worker(g: tracing_appender::non_blocking::WorkerGuard) -> Self` returning `InitGuard { worker: Some(g) }`
     - Do NOT impl Drop manually for InitGuard — when the inner `WorkerGuard` is `Some`, its own Drop will flush. The struct is just a holder.
     - Mark `InitGuard` `#[must_use = "InitGuard must be held; dropping it flushes file output"]`
  2. Add inline unit tests:
     - `init_error_display_already_initialized` — asserts the exact display string
     - `init_error_is_send_sync` — `fn assert<T: Send + Sync>() {} assert::<InitError>();`
     - `init_guard_is_send` — same idea for InitGuard
  3. Update `src/log/mod.rs`: add `pub use error::{InitError, InitGuard};`

  **Must NOT do**:
  - Do NOT use `thiserror` (avoid adding the dep for a 3-variant enum; hand-roll Display/Error impls)
  - Do NOT use `anyhow` in the public API
  - Do NOT pre-implement Drop on InitGuard — the inner WorkerGuard's Drop is sufficient
  - Do NOT make `InitGuard::empty` or `with_worker` `pub` — internal-only constructors

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Two small types, mechanical
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T4, T5) | Wave 2 | Blocks: T10 | Blocked By: T1

  **References**:
  - Hand-rolling errors w/o thiserror: standard Rust pattern, see std library examples
  - tracing-appender WorkerGuard: <https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/struct.WorkerGuard.html>
  - tracing dispatcher SetGlobalDefaultError: <https://docs.rs/tracing/latest/tracing/dispatcher/struct.SetGlobalDefaultError.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/error.rs` exists with `InitError` (3 variants) and `InitGuard`
  - [ ] `cargo test --lib error::tests` runs ≥3 tests, all pass
  - [ ] `InitGuard` is annotated `#[must_use = ...]`
  - [ ] No `thiserror` or `anyhow` in `Cargo.toml` `[dependencies]` (verify after this task)
  - [ ] InitError implements `std::error::Error` (compile check via the test)

  **QA Scenarios**:
  ```
  Scenario: InitError types and traits are correct
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib error::tests 2>&1 | tee .sisyphus/evidence/task-6-error-tests.txt
      grep -E '^\s*(thiserror|anyhow)\s*=' Cargo.toml && echo "FORBIDDEN DEP" || echo "OK"
    Expected: tests pass; "OK" printed
    Evidence: .sisyphus/evidence/task-6-error-tests.txt

  Scenario: InitGuard is must_use
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -A1 'pub struct InitGuard' src/log/error.rs | head -5 > .sisyphus/evidence/task-6-mustuse.txt
      grep -B1 'pub struct InitGuard' src/log/error.rs | grep 'must_use' && echo "OK" || echo "MISSING"
    Expected: "OK"
    Evidence: .sisyphus/evidence/task-6-mustuse.txt
  ```

  **Commit**: YES — at end of Wave 2, single commit covering T4+T5+T6 | Message: `feat(log): add core types: Level, FormatMode, ColorMode, InitError, InitGuard` | Files: src/log/level.rs, src/log/format.rs, src/log/error.rs, src/log/mod.rs

- [x] 7. `src/log/builder.rs` — LogBuilder struct + setters

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/builder.rs` defining:
     - `pub struct LogBuilder` with private fields:
       - `level: Level` (default `Level::Info`)
       - `format: FormatMode` (default `FormatMode::Normal`)
       - `color: ColorMode` (default `ColorMode::Auto`)
       - `log_file: Option<std::path::PathBuf>` (default None)
     - `impl Default for LogBuilder` providing the defaults above
     - `impl LogBuilder`:
       - `pub fn new() -> Self` (calls Default)
       - `pub fn level(mut self, level: Level) -> Self`
       - `pub fn format(mut self, format: FormatMode) -> Self`
       - `pub fn color(mut self, color: ColorMode) -> Self`
       - `pub fn log_file(mut self, path: impl Into<std::path::PathBuf>) -> Self`
       - `pub fn install(self) -> Result<InitGuard, InitError>` — delegates to `crate::log::init::install_with_config(self)` (function defined in T10)
     - `pub(crate) struct LogConfig` (named carefully — this is the internal config snapshot consumed by init.rs):
       - all four fields above, but pub(crate)
       - `impl From<LogBuilder> for LogConfig`
  2. Inline unit tests:
     - `default_builder_has_expected_values` — assert all 4 default values
     - `setters_chain_correctly` — `LogBuilder::new().level(Level::Debug).format(FormatMode::Context).color(ColorMode::Never).log_file("/tmp/x.log")` produces a builder with those exact values (use `pub(crate)` accessors or expose via `LogConfig::from`)
     - `log_file_accepts_str_pathbuf_path` — verify the `Into<PathBuf>` bound works for `&str`, `String`, `PathBuf`, `&Path`
  3. Update `src/log/mod.rs`: add `pub use builder::LogBuilder;`

  **Must NOT do**:
  - Do NOT call `init()` from inside `install()` — the install method MUST delegate to the init module so test code can call init logic directly with a config without going through the builder
  - Do NOT make `LogConfig` `pub` — it's internal
  - Do NOT add fluent methods that return `&mut self` — use owned `self` consistently for ergonomic chaining
  - Do NOT add a `simple(bool)` or `show_context(bool)` setter — those are subsumed by `format(FormatMode::...)` per Metis directive (no conflicting boolean flags)
  - Do NOT add a `log_to_console(bool)` setter — console output is always on (matches Python where setting log_file does NOT disable console)

  **Recommended Agent Profile**:
  - Category: `unspecified-low` — Reason: Slightly more design surface than a quick task (Into<PathBuf> bound, into-from conversion to LogConfig)
  - Skills: none required

  **Parallelization**: Can Parallel: NO (gates Wave 3) | Wave 3a | Blocks: T8, T9, T10 | Blocked By: T4, T5, T6

  **References**:
  - Python `PolyLog.get_logger` signature: `/Users/danny/Developer/polykit/src/polykit/log/polylog.py` — note the v0.1 omits `time_aware`, `env`, `remote` per scope decisions
  - Builder pattern in Rust: standard, e.g. <https://rust-unofficial.github.io/patterns/patterns/creational/builder.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/builder.rs` exists with `LogBuilder` and internal `LogConfig`
  - [ ] No `simple` or `show_context` setter methods (only `format`)
  - [ ] No `log_to_console` setter (console always on)
  - [ ] `cargo test --lib builder::tests` runs ≥3 tests, all pass
  - [ ] `LogBuilder::default()` returns `level: Info, format: Normal, color: Auto, log_file: None`

  **QA Scenarios**:
  ```
  Scenario: Builder defaults and setter chaining work
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib builder::tests 2>&1 | tee .sisyphus/evidence/task-7-builder.txt
    Expected: "test result: ok. 3 passed" (or more)
    Evidence: .sisyphus/evidence/task-7-builder.txt

  Scenario: No forbidden setters exist
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E 'fn (simple|show_context|log_to_console)\b' src/log/builder.rs && echo "FORBIDDEN" || echo "OK"
    Expected: "OK"
    Evidence: captured stdout
  ```

  **Commit**: NO (combine with T8, T9 as Wave 3 commit)

- [x] 8. `src/log/console.rs` — Custom tracing Layer for console output

  **What to do**:
  This is the largest single task. Implement a `tracing_subscriber::Layer` that renders events to stderr with the agreed format. Break into clear sections:

  1. Create `/Users/danny/Developer/polykit-rust/src/log/console.rs`.
  2. Define `pub(crate) struct ConsoleLayer` with fields:
     - `format: FormatMode`
     - `color_mode: ColorMode`
     - `min_level: Level`
     - `tz: jiff::tz::TimeZone` — resolved once at construction time
     - `tz_warning_emitted: std::sync::OnceLock<()>` — to ensure the "invalid TZ" warning is emitted at most once
  3. `impl ConsoleLayer`:
     - `pub(crate) fn new(config: &LogConfig) -> Self` — resolves TZ via `resolve_tz()` helper:
       - read `std::env::var("TZ")` — if set, try `jiff::tz::TimeZone::get(&tz_name)`. On error: emit one-time stderr warning `"polykit::log: invalid TZ env var '{name}'; falling back to America/New_York"` and use `jiff::tz::TimeZone::get("America/New_York")`. If THAT fails (extremely unlikely; would mean tzdata is missing), fall back to `jiff::tz::TimeZone::UTC` and emit second warning.
       - if `TZ` unset, default to `jiff::tz::TimeZone::get("America/New_York")` (with same fallback chain to UTC).
     - `fn render_event(&self, event: &tracing::Event<'_>) -> Vec<u8>` — pure function building the formatted bytes. Pure-function structure makes golden testing in T15 easy.
       - Drop tracing TRACE events: if `tracing::Level::from(event.metadata().level())` is TRACE, return empty vec (caller skips writing)
       - Map tracing level → our `Level`
       - If level < `self.min_level`, return empty vec
       - Read message: tracing Events carry the message in a field named `"message"`. Use a `MessageVisitor` (impl `tracing::field::Visit`) to extract `String` plus other fields formatted as ` field=value` after the message.
       - Resolve timestamp: `jiff::Zoned::now().with_time_zone(self.tz.clone())`
       - Format timestamp: `ts.strftime("%-I:%M:%S %p")` — produces e.g. "2:34:09 PM" (no leading zero on hour)
       - Get target: `event.metadata().target()`
       - Get file/line: `event.metadata().file()` (Option<&str>) and `event.metadata().line()` (Option<u32>)
       - Build the rendered bytes:
         - Determine `use_ansi = self.color_mode.should_emit_ansi(stderr_is_tty)` where `stderr_is_tty` is computed via `std::io::IsTerminal::is_terminal(&std::io::stderr())`
         - Use `owo_colors::OwoColorize` extension trait conditionally (use `if_supports_color`-style helpers OR conditionally apply colors based on `use_ansi`).
         - **Simple mode**: just message. Bold the entire line if level ≥ Warn (use `<msg>.bold()` when use_ansi is true). No timestamp, no level label.
         - **Normal mode**: `{ts in gray} {label colored} {msg}\n`. Timestamp colored with `AnsiColors::BrightBlack`. Level label colored with `level_color(level)` from level.rs. Message uncolored.
         - **Context mode**: `{ts in gray} {label colored} {target in blue} {file_basename}:{line in cyan} {msg}\n`. Target = module path (e.g. "polykit::log"). file_basename = `Path::new(file).file_name()` (just the filename, not the full path). Line number colored cyan. If file/line are None (rare), render as `<unknown>:0`.
         - Append a trailing `\n`. Multiline messages: if the message contains embedded `\n`, render verbatim — do NOT prefix each line with the formatter prefix in v0.1 (multiline messages will show subsequent lines without timestamps; document this as a known limitation in the rustdoc).
       - Use `Vec<u8>` writer + `write!` macros throughout for efficiency.
  4. `impl<S> tracing_subscriber::Layer<S> for ConsoleLayer where S: tracing::Subscriber`:
     - `fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>)`:
       - Call `self.render_event(event)` → bytes
       - If bytes is empty, return
       - Write bytes to `anstream::stderr()` (which strips ANSI when stderr isn't a TTY based on its own logic; we additionally control via `should_emit_ansi`).
       - Flush. If write fails, silently ignore (logging-during-logging recursion is bad).
  5. Inline unit tests for the pure helpers (the on_event Layer impl is exercised in T15 via golden tests):
     - `resolve_tz_with_invalid_falls_back` — set `TZ=Invalid/Bogus` (use `temp_env` pattern), call resolve_tz, assert returns America/New_York TimeZone
     - `resolve_tz_unset_uses_default` — clear TZ, call resolve_tz, assert returns America/New_York
     - `render_event_below_min_level_is_empty` — construct a ConsoleLayer with min_level=Warn, simulate an Info event (use `tracing::dispatcher::with_default` + a test subscriber that captures events; OR construct a mock Event — since constructing Events is hard, prefer a small integration approach: call layer.on_event in a dispatcher-set context. If too complex, defer all on_event-level testing to T15 golden tests.)
     - For the simpler render branches: extract a `format_normal` / `format_context` / `format_simple` pure-fn that takes (level, ts: &str, target: &str, file: Option<&str>, line: Option<u32>, msg: &str, use_ansi: bool) and assert exact bytes for known inputs.

  **Must NOT do**:
  - Do NOT use `chrono` — `jiff` only
  - Do NOT use `tracing_subscriber::fmt::Layer` — we are building a custom Layer because the default fmt layer cannot reproduce Python's exact format
  - Do NOT use ANSI escape codes literally — go through `owo-colors` for forward-compat with NO_COLOR/etc.
  - Do NOT write to stdout — logs go to stderr (matches Python convention)
  - Do NOT call `std::io::stderr()` directly — go through `anstream::stderr()` for terminal-cap adaptation
  - Do NOT add a `Json` format branch — only Simple/Normal/Context
  - Do NOT prefix multiline messages line-by-line in v0.1 (document as known limitation)
  - Do NOT panic on render failures (silently drop the event) — logging must never crash the program

  **Recommended Agent Profile**:
  - Category: `unspecified-high` — Reason: Most complex task. Custom Layer impl, jiff TZ handling, owo-colors conditional coloring, careful pure/impure separation for testability
  - Skills: none required (general Rust competence + reading the cited docs)

  **Parallelization**: Can Parallel: YES (with T9) | Wave 3b | Blocks: T10, T15 | Blocked By: T4, T5, T7

  **References**:
  - Custom Layer pattern: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/trait.Layer.html> and `tracing-bunyan-formatter` source on GitHub for a real-world example
  - `tracing::field::Visit`: <https://docs.rs/tracing/latest/tracing/field/trait.Visit.html> — used to extract message and structured fields from events
  - jiff TimeZone: <https://docs.rs/jiff/latest/jiff/tz/struct.TimeZone.html>
  - jiff Zoned + strftime: <https://docs.rs/jiff/latest/jiff/struct.Zoned.html#method.strftime>
  - owo-colors v4 OwoColorize trait: <https://docs.rs/owo-colors/4/owo_colors/trait.OwoColorize.html>
  - anstream stderr: <https://docs.rs/anstream/latest/anstream/fn.stderr.html>
  - Python formatter exact format strings: `/Users/danny/Developer/polykit/src/polykit/log/formatters.py`
  - `std::io::IsTerminal`: <https://doc.rust-lang.org/std/io/trait.IsTerminal.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/console.rs` exists with `ConsoleLayer` struct and `Layer` impl
  - [ ] Pure-function helpers `format_simple`, `format_normal`, `format_context` exist (or equivalent — testable without spinning up a subscriber)
  - [ ] `cargo test --lib console::tests` runs and passes
  - [ ] No occurrence of `chrono::` in console.rs
  - [ ] No occurrence of literal `\x1b[` ANSI escape sequences in console.rs (must go through owo-colors)
  - [ ] `cargo clippy -p polykit -- -D warnings` passes for src/log/console.rs

  **QA Scenarios**:
  ```
  Scenario: Console layer formatter pure functions produce exact expected bytes
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib console::tests 2>&1 | tee .sisyphus/evidence/task-8-console.txt
    Expected: tests pass
    Evidence: .sisyphus/evidence/task-8-console.txt

  Scenario: No forbidden imports / ANSI literals
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E 'use chrono::|chrono::' src/log/console.rs && echo "FORBIDDEN" || echo "OK"
      grep -E '\\x1b\[' src/log/console.rs && echo "RAW ANSI" || echo "OK"
    Expected: both print "OK"
    Evidence: captured stdout

  Scenario: Clippy is clean
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo clippy --all-targets -- -D warnings 2>&1 | tee .sisyphus/evidence/task-8-clippy.txt
    Expected: exit 0
    Evidence: .sisyphus/evidence/task-8-clippy.txt
  ```

  **Commit**: NO (combine with T7, T9)

- [x] 9. `src/log/file.rs` — File output layer (tracing-appender daily rolling)

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/file.rs`.
  2. Define `pub(crate) struct FileLayer` with fields:
     - `min_level: Level` (matches console min_level by default; for v0.1 they're the same — single level setting in the builder)
     - `tz: jiff::tz::TimeZone`
     - `writer: tracing_appender::non_blocking::NonBlocking`
  3. Define `pub(crate) fn build_file_layer(config: &LogConfig, tz: jiff::tz::TimeZone) -> Result<(FileLayer, tracing_appender::non_blocking::WorkerGuard), InitError>`:
     - `let path = config.log_file.as_ref().expect("file layer requires log_file");`
     - Resolve directory + filename:
       - If `path` is a directory (ends in `/` or is an existing dir), use `directory = path, filename = "polykit.log"`. Otherwise, `directory = path.parent().unwrap_or(Path::new("."))`, `filename = path.file_name().unwrap_or("polykit.log")`.
     - Create parent directory if it doesn't exist: `std::fs::create_dir_all(&directory)` — on error, return `InitError::FileSetupFailed { path, source }`
     - Construct appender: `let file_appender = tracing_appender::rolling::daily(&directory, &filename);`
     - Wrap in non-blocking: `let (writer, guard) = tracing_appender::non_blocking(file_appender);`
     - Return `(FileLayer { min_level: config.level, tz, writer }, guard)`
  4. `impl FileLayer`:
     - `fn render_event(&self, event: &tracing::Event<'_>) -> Vec<u8>` — pure function building plain-ASCII bytes (no ANSI):
       - Drop TRACE events
       - Filter by min_level
       - Extract message using same Visit pattern as ConsoleLayer (consider extracting the visitor to a shared `pub(super) struct MessageVisitor` in a new `src/log/visitor.rs` if it eliminates duplication; otherwise duplicate is fine for v0.1 — just keep them in sync)
       - Timestamp: `jiff::Zoned::now().with_time_zone(self.tz.clone()).strftime("%Y-%m-%d %H:%M:%S")` → e.g. "2026-04-28 14:09:02"
       - Format: `[{ts}] [{LEVEL_LABEL_NO_COLOR}] {target} {file_basename}:{line}: {msg}\n`
         - LEVEL_LABEL_NO_COLOR is the same `[DEBUG]`/`[INFO]`/`[WARN]`/`[ERROR]` strings, just no ANSI wrapping
         - file_basename + line same as console
       - This is a single format mode for files (not Simple/Normal/Context — files always get full context, matching Python's FileFormatter)
  5. `impl<S> tracing_subscriber::Layer<S> for FileLayer where S: tracing::Subscriber`:
     - `on_event` writes to `self.writer` (a `NonBlocking` impl `io::Write`); flush; ignore errors silently
  6. Inline unit tests:
     - `file_format_no_ansi` — invoke render_event-equivalent or the pure formatter, assert no `\x1b` bytes appear in output
     - `file_format_iso_timestamp` — assert output matches `^\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\]` regex
     - `build_file_layer_creates_missing_parent_dir` — point at `target/tmp/test-mkdir-{random}/sub/log/app.log`, call build_file_layer, assert success and dir exists
     - `build_file_layer_returns_error_for_unwritable_path` — try to write under `/dev/null/sub` (ENOTDIR), assert returns `InitError::FileSetupFailed`

  **Must NOT do**:
  - Do NOT include ANSI escape codes in file output
  - Do NOT use Simple/Normal/Context mode for files — files always render full context
  - Do NOT implement size-based rotation (we explicitly chose daily time-based)
  - Do NOT silently swallow `create_dir_all` failures — return InitError
  - Do NOT panic if log_file is None — caller must only invoke build_file_layer when log_file is Some (use `expect` with a clear message that indicates a programmer error if violated)

  **Recommended Agent Profile**:
  - Category: `unspecified-low` — Reason: Smaller than console.rs (single fixed format, no color logic), but real I/O + tracing-appender API to learn
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T8) | Wave 3b | Blocks: T10, T15 | Blocked By: T5, T6, T7

  **References**:
  - tracing-appender rolling: <https://docs.rs/tracing-appender/latest/tracing_appender/rolling/index.html>
  - tracing-appender non_blocking: <https://docs.rs/tracing-appender/latest/tracing_appender/non_blocking/index.html>
  - Python file formatter: `/Users/danny/Developer/polykit/src/polykit/log/formatters.py` `FileFormatter` class

  **Acceptance Criteria**:
  - [ ] File `src/log/file.rs` exists with `FileLayer` and `build_file_layer`
  - [ ] No ANSI bytes (`\x1b`) appear in file output (verified by test)
  - [ ] Timestamp format is exactly `YYYY-MM-DD HH:MM:SS` in 24-hour
  - [ ] `cargo test --lib file::tests` runs ≥4 tests, all pass
  - [ ] No size-based rotation logic (no references to byte counts, file sizes, etc.)

  **QA Scenarios**:
  ```
  Scenario: File output is plain ASCII with ISO timestamps
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib file::tests 2>&1 | tee .sisyphus/evidence/task-9-file.txt
    Expected: tests pass
    Evidence: .sisyphus/evidence/task-9-file.txt

  Scenario: No size-rotation references
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E 'rotate.*size|max_bytes|512.?KB|byte.?limit' src/log/file.rs && echo "SIZE-ROTATION FOUND" || echo "OK"
    Expected: "OK"
    Evidence: captured stdout
  ```

  **Commit**: YES — at end of Wave 3, single commit T7+T8+T9 | Message: `feat(log): add builder, console layer, and file layer` | Files: src/log/builder.rs, src/log/console.rs, src/log/file.rs, src/log/mod.rs

- [ ] 10. `src/log/init.rs` — `init()`, idempotency, subscriber wiring, tracing-log bridge

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/init.rs`.
  2. Define module-level shared state:
     - `static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);`
     - `pub(crate) static MIN_LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);` (initialized to Info's repr; we encode Level→u8 as Debug=0, Info=1, Warn=2, Error=3 — add `pub(crate) const fn as_u8` / `pub(crate) fn from_u8` to `level.rs` retroactively when T10 lands; this is a small, safe addition to T4)
     - `pub(crate) fn current_min_level() -> Level { Level::from_u8(MIN_LEVEL.load(Ordering::Relaxed)).unwrap_or(Level::Info) }`
     - `pub(crate) fn set_min_level(level: Level) { MIN_LEVEL.store(level.as_u8(), Ordering::Relaxed); }`
  3. Public API:
     - `pub fn init() -> LogBuilder` returns `LogBuilder::new()`. This is the canonical entry point users call: `polykit::log::init().level(...).install()?`
  4. Internal entry point:
     - `pub(crate) fn install_with_config(config: LogConfig) -> Result<InitGuard, InitError>`:
       1. **Idempotency check**: `INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).map_err(|_| InitError::AlreadyInitialized)?;`
       2. Set the dynamic min_level: `set_min_level(config.level);`
       3. Resolve TZ once: `let tz = console::resolve_tz();`
       4. Build console layer: `let console_layer = ConsoleLayer::new(&config, tz.clone());`
       5. Optionally build file layer:
          - If `config.log_file.is_some()`: `let (file_layer, worker_guard) = file::build_file_layer(&config, tz)?;` → on error, **roll back the INITIALIZED flag** (`INITIALIZED.store(false, Ordering::Release)`) before returning the error
          - Else: file_layer = None, worker_guard = None
       6. Compose the subscriber:
          ```rust
          use tracing_subscriber::layer::SubscriberExt;
          use tracing_subscriber::util::SubscriberInitExt;
          let registry = tracing_subscriber::registry().with(console_layer);
          if let Some(fl) = file_layer {
              registry.with(fl).init();
          } else {
              registry.init();
          }
          ```
          Note: `.init()` here is `SubscriberInitExt::init` which calls `set_global_default`. If it fails, return `InitError::SetGlobalDefaultFailed`. (In tracing 0.1+, `.init()` panics on failure rather than returning Result. To get Result, call `.try_init()` instead. **Use `.try_init()`** and map the error.)
       7. Bridge `log` crate calls: `tracing_log::LogTracer::init().map_err(|_| ...)?;` — actually LogTracer::init returns `Result<(), SetLoggerError>`; if it fails (because someone else set a logger), that's recoverable — log a warning to stderr and continue. Don't fail init for this.
       8. Return `Ok(InitGuard::with_worker_opt(worker_guard))` — add `with_worker_opt(Option<WorkerGuard>) -> InitGuard` constructor to error.rs (small T6 amendment, document here).
  5. Inline unit tests:
     - `init_atomics_default_to_info` — assert current_min_level() == Level::Info before any init
     - `set_and_get_min_level_round_trips` — call set_min_level(Level::Debug), assert current_min_level() == Debug; restore to Info
     - **NOTE on testing**: We cannot test `install_with_config` in-process because the global subscriber can only be set once per process. Real init testing happens in T16 via subprocess.

  **Must NOT do**:
  - Do NOT use `set_global_default` directly (panics) — use `try_init()` and convert to Result
  - Do NOT proceed if `INITIALIZED` is already true — return AlreadyInitialized immediately
  - Do NOT forget to roll back `INITIALIZED` on error after the compare_exchange — partial init must leave `INITIALIZED == false` so the caller can retry after fixing the error
  - Do NOT panic if `tracing_log::LogTracer::init()` fails — degrade gracefully with a stderr warning
  - Do NOT add an `EnvFilter` / `RUST_LOG` parsing layer
  - Do NOT add a `try_init()`-with-defaults convenience that bypasses the builder (just the documented API surface)

  **Recommended Agent Profile**:
  - Category: `unspecified-high` — Reason: Subtle concurrency (compare_exchange + rollback), composing tracing-subscriber Layered, error mapping for try_init, retroactive small edits to level.rs and error.rs
  - Skills: none required

  **Parallelization**: Can Parallel: NO (gates Wave 5) | Wave 4 | Blocks: T13, T16 | Blocked By: T6, T7, T8, T9

  **References**:
  - tracing-subscriber composition: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/layer/index.html>
  - SubscriberInitExt::try_init: <https://docs.rs/tracing-subscriber/latest/tracing_subscriber/util/trait.SubscriberInitExt.html>
  - tracing-log LogTracer: <https://docs.rs/tracing-log/latest/tracing_log/struct.LogTracer.html>
  - Atomic compare_exchange ordering: Rust nomicon / std::sync::atomic docs

  **Acceptance Criteria**:
  - [ ] File `src/log/init.rs` exists with `init()`, `install_with_config`, `current_min_level`, `set_min_level`, `MIN_LEVEL`, `INITIALIZED`
  - [ ] `Level::as_u8` and `Level::from_u8` added to `src/log/level.rs` (small amendment, document in commit message)
  - [ ] `InitGuard::with_worker_opt(Option<WorkerGuard>) -> InitGuard` added to `src/log/error.rs` (small amendment)
  - [ ] `cargo test --lib init::tests` runs the in-process tests successfully
  - [ ] `cargo build --lib --all-features` succeeds (whole crate compiles for the first time end-to-end)
  - [ ] No use of `set_global_default` (only `try_init`)
  - [ ] No `EnvFilter` import

  **QA Scenarios**:
  ```
  Scenario: Crate compiles end-to-end after init wiring
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo build --lib --all-features 2>&1 | tee .sisyphus/evidence/task-10-build.txt
    Expected: exit 0
    Evidence: .sisyphus/evidence/task-10-build.txt

  Scenario: Init module's in-process tests pass
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib init::tests 2>&1 | tee .sisyphus/evidence/task-10-init-tests.txt
    Expected: tests pass
    Evidence: .sisyphus/evidence/task-10-init-tests.txt

  Scenario: No forbidden APIs used
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E 'set_global_default|EnvFilter' src/log/init.rs && echo "FORBIDDEN" || echo "OK"
    Expected: "OK"
    Evidence: captured stdout
  ```

  **Commit**: NO (combine with T11, T12 as Wave 4 commit)

- [ ] 11. `src/log/level_override.rs` — RAII level override guard

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/level_override.rs`:
     ```rust
     use crate::log::Level;
     use crate::log::init::{current_min_level, set_min_level};

     /// RAII guard that temporarily overrides the global log level.
     ///
     /// On construction, replaces the current minimum level with `level` and
     /// remembers the previous value. On drop, restores the previous value.
     ///
     /// Note: this affects the entire process, not a per-logger / per-thread
     /// level. Concurrent threads will observe the override for its lifetime.
     #[must_use = "LogLevelOverride must be held; dropping it ends the override"]
     pub struct LogLevelOverride {
         previous: Level,
     }

     impl LogLevelOverride {
         pub fn new(level: Level) -> Self {
             let previous = current_min_level();
             set_min_level(level);
             Self { previous }
         }
     }

     impl Drop for LogLevelOverride {
         fn drop(&mut self) {
             set_min_level(self.previous);
         }
     }
     ```
  2. Inline unit tests (these CAN be in-process since they don't touch the subscriber):
     - `override_changes_min_level_for_scope`:
       ```rust
       set_min_level(Level::Info);  // baseline
       {
           let _g = LogLevelOverride::new(Level::Debug);
           assert_eq!(current_min_level(), Level::Debug);
       }
       assert_eq!(current_min_level(), Level::Info);  // restored
       ```
     - `nested_overrides_unwind_in_lifo_order` — verify two nested guards stack/unwind correctly
     - `override_works_across_threads` — note in test that this is process-global (NOT thread-local); document the behavior; test simply demonstrates that thread A's override is observed by thread B during its lifetime
     - **IMPORTANT**: tests must serialize via a shared `Mutex` because they mutate global state. Use `std::sync::Mutex<()>` declared as `static` and lock in each test.
  3. Update `src/log/mod.rs`: `pub use level_override::LogLevelOverride;`

  **Must NOT do**:
  - Do NOT make this thread-local (Python's behavior is logger-scoped, ours is process-global; document the divergence in the rustdoc)
  - Do NOT skip the must_use attribute
  - Do NOT take a `&Logger` parameter (we don't have a Logger type — the global state is the logger)

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Small RAII type
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T12) | Wave 4 | Blocks: T13 | Blocked By: T4, T7, T10

  **References**:
  - Python `LogLevelOverride`: `/Users/danny/Developer/polykit/src/polykit/log/polylog.py`
  - Rust RAII pattern: <https://rust-unofficial.github.io/patterns/idioms/dtor-finally.html>

  **Acceptance Criteria**:
  - [ ] File `src/log/level_override.rs` exists
  - [ ] Type marked `#[must_use = ...]`
  - [ ] `cargo test --lib level_override::tests` runs ≥3 tests, all pass
  - [ ] Test mutex visible (no concurrent test corruption of MIN_LEVEL)

  **QA Scenarios**:
  ```
  Scenario: Override sets level for scope and restores on drop
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib level_override::tests -- --test-threads=1 2>&1 | tee .sisyphus/evidence/task-11-override.txt
    Expected: tests pass; --test-threads=1 because of global state
    Evidence: .sisyphus/evidence/task-11-override.txt
  ```

  **Commit**: NO (combine with T10, T12)

- [x] 12. `src/log/catch.rs` — Closure-based error-catching helper

  **What to do**:
  1. Create `/Users/danny/Developer/polykit-rust/src/log/catch.rs`:
     ```rust
     use std::error::Error;

     /// Run a closure and log any error it returns, preserving the chain.
     ///
     /// On `Err(e)`, emits a single tracing ERROR event with the message
     /// `"{context}: {error}"` and walks `Error::source()` to log each cause
     /// on subsequent ERROR events at the same target.
     ///
     /// Returns the closure's result unchanged.
     ///
     /// Does NOT catch panics. v0.1 ships closure-based error logging only.
     pub fn catch<T, E, F>(context: &str, f: F) -> Result<T, E>
     where
         E: Error,
         F: FnOnce() -> Result<T, E>,
     {
         match f() {
             Ok(value) => Ok(value),
             Err(error) => {
                 tracing::error!("{context}: {error}");
                 let mut source = error.source();
                 while let Some(cause) = source {
                     tracing::error!("  caused by: {cause}");
                     source = cause.source();
                 }
                 Err(error)
             }
         }
     }
     ```
  2. Inline unit tests:
     - `catch_passes_through_ok` — `catch("test", || Ok::<_, std::io::Error>(42))` returns `Ok(42)`
     - `catch_returns_err_unchanged` — verify Err is returned (doesn't lose original error)
     - `catch_logs_chain` — set up a tracing subscriber that captures events (use `tracing_subscriber::fmt::TestWriter` or a simple Vec-backed Layer), invoke catch with a chained error (use a simple newtype that wraps another error), assert both messages were logged
       - For the test subscriber, it's acceptable to use `tracing_subscriber::with_default(...)` to scope a TEST-ONLY subscriber — this does NOT conflict with our global init because the test runs without calling install_with_config
  3. Update `src/log/mod.rs`: `pub use catch::catch;`

  **Must NOT do**:
  - Do NOT include `catch_unwind` panic-catching in v0.1
  - Do NOT use `anyhow::Error` — use `std::error::Error` for maximum compatibility
  - Do NOT attempt to format the error with `Debug` — `Display` is the contract here
  - Do NOT eat the error — always return it unchanged

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: ~30 LOC + tests
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T11) | Wave 4 | Blocks: T13 | Blocked By: T4, T7

  **References**:
  - Python `PolyLog.catch`: `/Users/danny/Developer/polykit/src/polykit/log/polylog.py` (the `@contextmanager` version — Rust translates the Python "context manager" pattern to a closure-based function, which is the idiomatic equivalent)
  - `std::error::Error::source`: <https://doc.rust-lang.org/std/error/trait.Error.html#method.source>

  **Acceptance Criteria**:
  - [ ] File `src/log/catch.rs` exists with `pub fn catch<T, E, F>(...)`
  - [ ] `cargo test --lib catch::tests` runs ≥3 tests, all pass
  - [ ] No `catch_unwind` reference in catch.rs
  - [ ] No `anyhow` reference

  **QA Scenarios**:
  ```
  Scenario: catch passes Ok through and logs Err chain
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib catch::tests 2>&1 | tee .sisyphus/evidence/task-12-catch.txt
      grep -E 'catch_unwind|anyhow' src/log/catch.rs && echo "FORBIDDEN" || echo "OK"
    Expected: tests pass; "OK" printed
    Evidence: .sisyphus/evidence/task-12-catch.txt
  ```

  **Commit**: YES — at end of Wave 4 | Message: `feat(log): wire init, level override, and catch helper` | Files: src/log/init.rs, src/log/level_override.rs, src/log/catch.rs, src/log/level.rs (as_u8/from_u8 amendment), src/log/error.rs (with_worker_opt amendment), src/log/mod.rs

- [ ] 13. `src/log/mod.rs` — Module aggregator + public re-exports

  **What to do**:
  1. Replace `/Users/danny/Developer/polykit-rust/src/log/mod.rs` with the final aggregator:
     ```rust
     //! Branded logger built on `tracing`.
     //!
     //! # Quickstart
     //! ```no_run
     //! use polykit::log;
     //!
     //! fn main() -> Result<(), Box<dyn std::error::Error>> {
     //!     let _guard = log::init()
     //!         .level(log::Level::Info)
     //!         .format(log::FormatMode::Context)
     //!         .install()?;
     //!
     //!     log::info!("hello from polykit");
     //!     Ok(())
     //! }
     //! ```
     //!
     //! The returned guard must remain in scope for the program lifetime;
     //! dropping it flushes any pending file output.
     //!
     //! # Levels
     //! Four levels: [`Level::Debug`], [`Level::Info`], [`Level::Warn`],
     //! [`Level::Error`]. The `tracing` TRACE level is dropped (not rendered).
     //! There is no CRITICAL level.

     mod builder;
     mod catch;
     mod console;
     mod error;
     mod file;
     mod format;
     pub(crate) mod init;
     mod level;
     mod level_override;

     pub use builder::LogBuilder;
     pub use catch::catch;
     pub use error::{InitError, InitGuard};
     pub use format::{ColorMode, FormatMode};
     pub use level::Level;
     pub use level_override::LogLevelOverride;

     /// Build a [`LogBuilder`] with default values.
     pub fn init() -> LogBuilder {
         init::init()
     }

     // Re-export tracing macros at polykit::log:: for convenience.
     pub use tracing::{debug, error, event, info, instrument, span, trace, warn};

     // Also re-export tracing itself for users who want full access.
     pub use tracing;
     ```
  2. Verify `cargo build --lib` and `cargo test --lib` and `cargo doc --no-deps`.

  **Must NOT do**:
  - Do NOT re-export `Level` aliases like `LogLevel` (single canonical name only)
  - Do NOT publicly expose `LogConfig`, `ConsoleLayer`, `FileLayer`, or `MIN_LEVEL`
  - Do NOT re-export `tracing::dispatcher` or other internals
  - Do NOT add a `prelude` module in v0.1 (defer; one canonical import path is enough)
  - Do NOT add convenience macros like `polykit::log::critical!` (no CRITICAL)

  **Recommended Agent Profile**:
  - Category: `quick` — Reason: Re-export plumbing
  - Skills: none required

  **Parallelization**: Can Parallel: NO | Wave 5 | Blocks: T14, T15, T16, T17 | Blocked By: T10, T11, T12

  **References**: None new (synthesizing prior work)

  **Acceptance Criteria**:
  - [ ] `cargo build --lib` succeeds
  - [ ] `cargo test --lib` runs all unit tests across all modules, all pass
  - [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` succeeds
  - [ ] `cargo doc --no-deps` produces docs that include `polykit::log::init`, `polykit::log::Level`, `polykit::log::LogBuilder`, `polykit::log::FormatMode`, `polykit::log::ColorMode`, `polykit::log::InitError`, `polykit::log::InitGuard`, `polykit::log::LogLevelOverride`, `polykit::log::catch`
  - [ ] `cargo doc` does NOT expose `LogConfig`, `ConsoleLayer`, `FileLayer`
  - [ ] Doctest in mod.rs compiles (verified via `cargo test --doc`)

  **QA Scenarios**:
  ```
  Scenario: Public API surface is exactly the agreed types
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo doc --no-deps 2>&1 | tee .sisyphus/evidence/task-13-doc.txt
      ls target/doc/polykit/log/ | sort > .sisyphus/evidence/task-13-public-items.txt
    Expected: target/doc/polykit/log/ contains expected items; no LogConfig/ConsoleLayer/FileLayer
    Evidence: .sisyphus/evidence/task-13-doc.txt + task-13-public-items.txt

  Scenario: Doctest compiles
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --doc 2>&1 | tee .sisyphus/evidence/task-13-doctest.txt
    Expected: doctest passes
    Evidence: .sisyphus/evidence/task-13-doctest.txt
  ```

  **Commit**: NO (combine with T14)

- [ ] 14. `src/lib.rs` — Top-level crate doc and module re-export

  **What to do**:
  Replace `/Users/danny/Developer/polykit-rust/src/lib.rs` with:
  ```rust
  //! Polykit utility library for Rust.
  //!
  //! Sibling to [`polykit`](https://github.com/dannystewart/polykit) (Python)
  //! and [`polykit-swift`](https://github.com/dannystewart/polykit-swift) (Swift).
  //!
  //! # Modules
  //! - [`log`] — Branded logger built on `tracing`.
  //!
  //! Future modules will be added here over time.

  #![forbid(unsafe_code)]
  #![warn(missing_docs)]

  pub mod log;
  ```
  - `forbid(unsafe_code)` — we don't need unsafe for v0.1; if a future module needs unsafe, downgrade per-module
  - `warn(missing_docs)` — gentle pressure to document; CI's `RUSTDOCFLAGS="-D warnings"` upgrades to error

  **Must NOT do**:
  - Do NOT add `#![deny(missing_docs)]` at the crate level (use warn + RUSTDOCFLAGS for CI escalation)
  - Do NOT import or re-export anything else at lib.rs level

  **Recommended Agent Profile**:
  - Category: `quick`
  - Skills: none

  **Parallelization**: Can Parallel: NO | Wave 5 | Blocks: T15, T16, T17 | Blocked By: T13

  **References**: None new

  **Acceptance Criteria**:
  - [ ] `cargo build` succeeds with no warnings
  - [ ] `cargo doc --no-deps` succeeds with `RUSTDOCFLAGS="-D warnings"`
  - [ ] `grep -q 'forbid(unsafe_code)' src/lib.rs` succeeds
  - [ ] `grep -q 'pub mod log;' src/lib.rs` succeeds

  **QA Scenarios**:
  ```
  Scenario: Crate builds clean with strict doc warnings
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo build --all-targets 2>&1 | tee .sisyphus/evidence/task-14-build.txt
      RUSTDOCFLAGS="-D warnings" cargo doc --no-deps 2>&1 | tee .sisyphus/evidence/task-14-doc.txt
    Expected: both exit 0, no warnings
    Evidence: .sisyphus/evidence/task-14-{build,doc}.txt
  ```

  **Commit**: YES — Wave 5 commit | Message: `feat(log): expose public API via module aggregator and lib root` | Files: src/log/mod.rs, src/lib.rs

- [ ] 15. `tests/formatter_golden.rs` — Deterministic byte-exact formatter golden tests

  **What to do**:
  Per Metis: formatter tests must be deterministic (fixed timestamp, fixed timezone, fixed writer, controlled color mode, exact expected output).

  Approach: T8 and T9's pure-function helpers (`format_simple`, `format_normal`, `format_context`, file render fn) take all the inputs explicitly. The integration test calls those helpers directly via the crate's `pub(crate)` visibility through a `cfg(test)`-only re-export, OR via a `pub(crate)` module that the integration test accesses via the `polykit::log::__test_helpers` (test-only) re-export.

  **Architecture decision**: add a single `#[cfg(any(test, feature = "test-helpers"))] pub mod __test_helpers;` to `src/log/mod.rs` that re-exports the formatter pure functions. Integration tests in `tests/` enable this via `[dev-dependencies]` and a `polykit = { path = ".", features = ["test-helpers"] }` self-reference IF needed. The simpler path: put these golden tests as `#[cfg(test)]` integration tests inline in `console.rs` and `file.rs` (which is already where T8/T9 put their unit tests). Pick the simpler path.

  **REVISED what to do** (simpler path):
  Add additional inline tests to `src/log/console.rs` and `src/log/file.rs` covering the full golden-test matrix. NO `tests/formatter_golden.rs` integration test file. The "tests/" directory is reserved for T16's subprocess tests.

  Required golden test cases (add inline to the relevant files):

  In **`src/log/console.rs`**:
  1. `golden_normal_info_ansi` — inputs: level=Info, ts="2:34:09 PM", target="my_app", file=Some("src/main.rs"), line=Some(42), msg="hello", use_ansi=true → expected exact bytes (compute by hand including ANSI codes from owo-colors; capture once and lock in) — store expected as a byte literal with explicit `\x1b[...m` sequences
  2. `golden_normal_info_plain` — same inputs, use_ansi=false → expected: `2:34:09 PM [INFO] hello\n`
  3. `golden_simple_info_plain` — Simple mode, msg="hello", use_ansi=false → expected: `hello\n`
  4. `golden_simple_warn_ansi_is_bold` — Simple mode, level=Warn, use_ansi=true → assert output starts with bold ANSI `\x1b[1m` and ends with reset
  5. `golden_simple_info_ansi_is_not_bold` — Simple mode, level=Info, use_ansi=true → assert output does NOT contain `\x1b[1m`
  6. `golden_context_full` — Context mode, target="my_app::sub", file=Some("/abs/path/src/lib.rs"), line=Some(10), msg="x" → expected: `2:34:09 PM [INFO] my_app::sub lib.rs:10 x\n` (basename only, no leading path)
  7. `golden_context_missing_file_line` — Context mode, file=None, line=None → expected: `... <unknown>:0 x\n`
  8. `golden_below_min_level_returns_empty` — already exists per T8 plan; ensure included
  9. `golden_unicode_preserved` — msg=`"héllo 🦀 wörld"`, assert bytes contain UTF-8 sequence verbatim (no escaping)
  10. `golden_multiline_message` — msg="line1\nline2", Normal mode → expected: `2:34:09 PM [INFO] line1\nline2\n` (NOT each line prefixed; documents v0.1 limitation)

  In **`src/log/file.rs`**:
  1. `golden_file_info_no_ansi` — level=Info, ts="2026-04-28 14:09:02", target="my_app", file=Some("src/main.rs"), line=Some(42), msg="hello" → expected exact: `[2026-04-28 14:09:02] [INFO] my_app main.rs:42: hello\n`
  2. `golden_file_warn` — same with level=Warn → `[2026-04-28 14:09:02] [WARN] ...`
  3. `golden_file_unknown_file_line` — file=None, line=None → `[2026-04-28 14:09:02] [INFO] my_app <unknown>:0: hello\n`
  4. `golden_file_no_ansi_bytes` — for any rendered output, `assert!(!output.contains(&0x1b))`
  5. `golden_file_unicode_preserved` — UTF-8 verbatim

  **Critical implementation detail**: pure formatter functions must take a `ts: &str` parameter (not look up `Zoned::now()` inside). T8 and T9's pure functions are already specified to support this — verify and amend if needed.

  **Must NOT do**:
  - Do NOT call `Zoned::now()` inside the test path — every test passes a fixed ts string
  - Do NOT use approximate matching (`contains`, `starts_with`) for the byte-exact golden tests; use `assert_eq!` on full bytes
  - Do NOT add `tests/formatter_golden.rs` (decided against — keep tests inline in the modules)
  - Do NOT skip the unicode and multiline cases; they document v0.1 behavior

  **Recommended Agent Profile**:
  - Category: `unspecified-low` — Reason: Many small precise tests, requires careful byte-exact assertions
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T16, T17) | Wave 6 | Blocks: F1-F4 | Blocked By: T13, T14

  **References**:
  - Python formatter exact strings: `/Users/danny/Developer/polykit/src/polykit/log/formatters.py`
  - owo-colors v4 ANSI sequences: `cargo expand` or run a small bin to capture the exact escape sequences for the chosen colors

  **Acceptance Criteria**:
  - [ ] At least 10 golden tests added to `console.rs` covering the matrix above
  - [ ] At least 5 golden tests added to `file.rs` covering the matrix above
  - [ ] `cargo test --lib console::tests` passes all golden cases
  - [ ] `cargo test --lib file::tests` passes all golden cases
  - [ ] No use of `contains`/`starts_with` in golden assertions (only `assert_eq!`)

  **QA Scenarios**:
  ```
  Scenario: All formatter goldens pass
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --lib 'console::tests::golden_' 2>&1 | tee .sisyphus/evidence/task-15-console-golden.txt
      cargo test --lib 'file::tests::golden_' 2>&1 | tee .sisyphus/evidence/task-15-file-golden.txt
    Expected: both pass; combined ≥15 tests
    Evidence: .sisyphus/evidence/task-15-{console,file}-golden.txt

  Scenario: Goldens use exact equality
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      grep -E 'fn golden_' src/log/console.rs src/log/file.rs > .sisyphus/evidence/task-15-golden-list.txt
      # Each golden test body must use assert_eq! (not contains/starts_with) on the full output
      # Verify by inspecting the file (manual review during F2)
    Expected: list shows ≥15 golden_* tests
    Evidence: .sisyphus/evidence/task-15-golden-list.txt
  ```

  **Commit**: NO (combine with T16, T17 as Wave 6)

- [ ] 16. `tests/init_behavior.rs` — Subprocess-based init/idempotency integration tests

  **What to do**:
  Per Metis: in-process tests cannot reliably exercise the global subscriber init. Each test case spawns a subprocess that exercises ONE init scenario.

  1. Create a small example binary `examples/init_harness.rs` that the integration tests will spawn:
     ```rust
     // examples/init_harness.rs
     //! Test harness: drives polykit::log init based on argv.
     //!
     //! Args:
     //!   1: scenario name (e.g. "first_init", "second_init", "pre_init", "file_init")
     //!   2: log file path (used by file_init scenarios; "-" if N/A)
     //!
     //! Exit codes:
     //!   0: scenario completed successfully
     //!   1: expected error occurred (test should assert on stderr/stdout)
     //!   2: unexpected panic / wrong behavior
     use polykit::log::{self, Level, FormatMode, ColorMode, InitError};

     fn main() {
         let args: Vec<String> = std::env::args().collect();
         let scenario = args.get(1).cloned().unwrap_or_default();
         let log_path = args.get(2).cloned().unwrap_or("-".to_string());

         match scenario.as_str() {
             "first_init" => {
                 let _g = log::init().level(Level::Info).install().expect("init failed");
                 log::info!("first init ok");
             }
             "second_init_returns_error" => {
                 let _g = log::init().install().expect("first init must succeed");
                 match log::init().install() {
                     Err(InitError::AlreadyInitialized) => println!("ALREADY_INITIALIZED"),
                     Ok(_) => { eprintln!("BUG: second init succeeded"); std::process::exit(2) }
                     Err(e) => { eprintln!("BUG: wrong error: {e}"); std::process::exit(2) }
                 }
             }
             "pre_init_logging_no_panic" => {
                 // Logging before init must not panic; events are dropped silently.
                 log::info!("before init");
                 log::warn!("still before init");
                 println!("PRE_INIT_OK");
             }
             "file_init_creates_dir" => {
                 if log_path == "-" { eprintln!("need log path"); std::process::exit(2); }
                 let _g = log::init().log_file(&log_path).install().expect("init failed");
                 log::info!("file write");
                 // Drop guard to flush
             }
             "file_init_unwritable_returns_error" => {
                 // Path that cannot be created (under /dev/null on unix)
                 let bad = "/dev/null/sub/app.log";
                 match log::init().log_file(bad).install() {
                     Err(InitError::FileSetupFailed { .. }) => println!("FILE_SETUP_FAILED"),
                     other => { eprintln!("BUG: expected FileSetupFailed, got {other:?}"); std::process::exit(2) }
                 }
             }
             "no_color_env_disables_ansi" => {
                 std::env::set_var("NO_COLOR", "1");
                 let _g = log::init().color(ColorMode::Auto).install().expect("init failed");
                 log::info!("no_color test");
             }
             "force_color_env_enables_ansi_when_piped" => {
                 std::env::set_var("FORCE_COLOR", "1");
                 let _g = log::init().color(ColorMode::Auto).install().expect("init failed");
                 log::info!("force_color test");
             }
             "log_crate_bridge" => {
                 let _g = log::init().level(Level::Info).install().expect("init failed");
                 ::log::info!("from log crate"); // Should appear via tracing-log bridge
             }
             other => { eprintln!("unknown scenario: {other}"); std::process::exit(2) }
         }
     }
     ```
  2. Create `tests/init_behavior.rs`:
     ```rust
     use std::process::Command;

     fn run_harness(args: &[&str]) -> std::process::Output {
         let mut cmd = Command::new(env!("CARGO"));
         cmd.args(["run", "--quiet", "--example", "init_harness", "--"]);
         cmd.args(args);
         cmd.output().expect("failed to spawn harness")
     }

     #[test]
     fn first_init_succeeds() {
         let out = run_harness(&["first_init"]);
         assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
         let stderr = String::from_utf8_lossy(&out.stderr);
         assert!(stderr.contains("first init ok"), "log line missing: {stderr}");
     }

     #[test]
     fn second_init_returns_already_initialized() {
         let out = run_harness(&["second_init_returns_error"]);
         assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
         let stdout = String::from_utf8_lossy(&out.stdout);
         assert!(stdout.contains("ALREADY_INITIALIZED"), "got: {stdout}");
     }

     #[test]
     fn pre_init_logging_does_not_panic() {
         let out = run_harness(&["pre_init_logging_no_panic"]);
         assert!(out.status.success());
         assert!(String::from_utf8_lossy(&out.stdout).contains("PRE_INIT_OK"));
     }

     #[test]
     fn file_init_creates_missing_directory() {
         let tmp = std::env::temp_dir().join(format!("polykit-test-{}", std::process::id()));
         let log = tmp.join("nested/sub/app.log");
         let _ = std::fs::remove_dir_all(&tmp);
         let out = run_harness(&["file_init_creates_dir", log.to_str().unwrap()]);
         assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
         // tracing-appender daily rolling appends a date suffix; just verify directory exists with a file
         let parent = log.parent().unwrap();
         assert!(parent.exists());
         let entries: Vec<_> = std::fs::read_dir(parent).unwrap().flatten().collect();
         assert!(!entries.is_empty(), "no log file created in {parent:?}");
         let _ = std::fs::remove_dir_all(&tmp);
     }

     #[test]
     fn file_init_unwritable_returns_error() {
         #[cfg(unix)]
         {
             let out = run_harness(&["file_init_unwritable_returns_error"]);
             assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
             assert!(String::from_utf8_lossy(&out.stdout).contains("FILE_SETUP_FAILED"));
         }
     }

     #[test]
     fn no_color_env_strips_ansi_in_auto_mode() {
         let out = run_harness(&["no_color_env_disables_ansi"]);
         assert!(out.status.success());
         let stderr = String::from_utf8_lossy(&out.stderr);
         assert!(!stderr.contains('\x1b'), "ANSI bytes leaked through NO_COLOR: {stderr:?}");
     }

     #[test]
     fn log_crate_calls_route_through_tracing_log_bridge() {
         let out = run_harness(&["log_crate_bridge"]);
         assert!(out.status.success());
         let stderr = String::from_utf8_lossy(&out.stderr);
         assert!(stderr.contains("from log crate"), "bridge missed: {stderr}");
     }
     ```
  3. Add `log = "0.4"` to `[dev-dependencies]` in Cargo.toml so the bridge test can call the `log` macros.

  **Must NOT do**:
  - Do NOT use a single test process to exercise multiple init scenarios — each scenario MUST spawn a fresh subprocess
  - Do NOT mock out the subscriber — these tests use the real subscriber pipeline
  - Do NOT skip the file-write-creates-dir scenario; this is the only way to verify create_dir_all works
  - Do NOT skip platform gating on `/dev/null/sub` (only valid on unix)

  **Recommended Agent Profile**:
  - Category: `unspecified-high` — Reason: Subprocess orchestration, real I/O, careful test isolation. Subtle.
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T15, T17) | Wave 6 | Blocks: F1-F4 | Blocked By: T13, T14

  **References**:
  - `env!("CARGO")` to spawn cargo: <https://doc.rust-lang.org/cargo/reference/environment-variables.html>
  - tracing-log `LogTracer`: covered in T10 references

  **Acceptance Criteria**:
  - [ ] `examples/init_harness.rs` exists and compiles
  - [ ] `tests/init_behavior.rs` exists with ≥7 test functions
  - [ ] `cargo test --test init_behavior` passes all tests on macOS (CI matrix: ubuntu + macos)
  - [ ] `log = "0.4"` added to `[dev-dependencies]`

  **QA Scenarios**:
  ```
  Scenario: All subprocess init tests pass
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --test init_behavior 2>&1 | tee .sisyphus/evidence/task-16-init.txt
    Expected: all tests pass; ≥7 results
    Evidence: .sisyphus/evidence/task-16-init.txt

  Scenario: Harness compiles standalone
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo build --example init_harness 2>&1 | tee .sisyphus/evidence/task-16-harness-build.txt
    Expected: exit 0
    Evidence: .sisyphus/evidence/task-16-harness-build.txt
  ```

  **Commit**: NO (combine with T15, T17 as Wave 6)

- [ ] 17. `tests/smoke.rs` — End-to-end smoke + LogLevelOverride + catch + concurrent logging

  **What to do**:
  Cover the remaining behaviors that don't fit naturally into golden or init categories.

  1. Create `tests/smoke.rs`. These tests CAN run in-process if they don't conflict with the global subscriber. To keep them compatible: do NOT call `init().install()` from this file — instead, use the same subprocess pattern via `examples/init_harness.rs` extended with extra scenarios.

  Add scenarios to `examples/init_harness.rs`:
  - `level_override_in_scope` — call init at Info, log info (visible) + debug (filtered), then `let _g = LogLevelOverride::new(Level::Debug); log::debug!("now visible"); drop _g; log::debug!("filtered again");` — print the captured behavior to stdout for assertion
  - `catch_logs_error_chain` — define a small error type with `.source()`, call `polykit::log::catch("ctx", || -> Result<(), _> { Err(my_chained_err) })`, verify stderr contains both error messages
  - `concurrent_logging_no_corruption` — spawn 8 threads each emitting 100 events with a known marker; each line should be a complete formatted line (no interleaving inside a single line); count distinct complete-line markers in stderr

  Then `tests/smoke.rs` spawns the harness for each:
  ```rust
  use std::process::Command;
  fn run(args: &[&str]) -> std::process::Output { /* same as T16 */ }

  #[test]
  fn level_override_changes_visibility_in_scope() {
      let out = run(&["level_override_in_scope"]);
      assert!(out.status.success());
      let stderr = String::from_utf8_lossy(&out.stderr);
      // Before override: debug filtered
      // During override: debug visible with marker "now visible"
      assert!(stderr.contains("now visible"), "override didn't enable debug: {stderr}");
      // After override: debug filtered again
      let after_marker_count = stderr.matches("filtered again").count();
      assert_eq!(after_marker_count, 0, "debug logged after override dropped");
  }

  #[test]
  fn catch_logs_error_chain_to_stderr() {
      let out = run(&["catch_logs_error_chain"]);
      assert!(out.status.success());
      let stderr = String::from_utf8_lossy(&out.stderr);
      assert!(stderr.contains("ctx:"));
      assert!(stderr.contains("caused by:"));
  }

  #[test]
  fn concurrent_logging_does_not_corrupt_lines() {
      let out = run(&["concurrent_logging_no_corruption"]);
      assert!(out.status.success());
      let stderr = String::from_utf8_lossy(&out.stderr);
      // Each event line should match the canonical pattern; count complete matches
      let line_count = stderr.lines()
          .filter(|l| l.contains("[INFO]") && l.contains("CONCURRENT_MARKER"))
          .count();
      assert_eq!(line_count, 800, "expected 8*100 lines, got {line_count}: {stderr}");
  }
  ```

  **Must NOT do**:
  - Do NOT call `init().install()` directly from `tests/smoke.rs` (use the harness)
  - Do NOT skip the concurrent test — line-corruption is a known logger failure mode
  - Do NOT use sleep-based synchronization in the concurrent test (rely on `JoinHandle::join`)

  **Recommended Agent Profile**:
  - Category: `unspecified-low`
  - Skills: none required

  **Parallelization**: Can Parallel: YES (with T15, T16) | Wave 6 | Blocks: F1-F4 | Blocked By: T13, T14

  **References**:
  - Same as T16 (reuses harness pattern)

  **Acceptance Criteria**:
  - [ ] `tests/smoke.rs` exists with ≥3 test functions
  - [ ] `examples/init_harness.rs` extended with the 3 new scenarios
  - [ ] `cargo test --test smoke` passes all tests
  - [ ] Concurrent test asserts exactly 800 well-formed lines (8 threads × 100 events)

  **QA Scenarios**:
  ```
  Scenario: Smoke + override + catch + concurrent tests pass
    Tool: Bash
    Steps:
      cd /Users/danny/Developer/polykit-rust
      cargo test --test smoke 2>&1 | tee .sisyphus/evidence/task-17-smoke.txt
    Expected: all tests pass
    Evidence: .sisyphus/evidence/task-17-smoke.txt
  ```

  **Commit**: YES — Wave 6 commit T15+T16+T17 | Message: `test(log): add formatter goldens, init subprocess tests, and smoke suite` | Files: src/log/console.rs (golden tests appended), src/log/file.rs (golden tests appended), tests/init_behavior.rs, tests/smoke.rs, examples/init_harness.rs, Cargo.toml (dev-dep `log`)

## Final Verification Wave (MANDATORY — after ALL implementation tasks)

> 4 review agents run in PARALLEL. ALL must APPROVE. Present consolidated results to user and get explicit "okay" before completing.
> **Do NOT auto-proceed after verification. Wait for user's explicit approval before marking work complete.**
> **Never mark F1-F4 as checked before getting user's okay.** Rejection or user feedback → fix → re-run → present again → wait for okay.

- [ ] F1. Plan Compliance Audit — oracle

  **What to do**: Oracle reads `.sisyphus/plans/polylog-rust.md` AND the actual implementation, then audits compliance task-by-task. Specifically:
  - Every "Must NOT" item in every task is verified absent from the implementation
  - Every "Acceptance Criteria" checkbox is independently re-verified
  - Public API surface matches T13's spec exactly (no extra/missing pub items)
  - The 12 user decisions (compressed in b2) are all honored
  - File rotation is daily (not size-based)
  - No `chrono`, `anyhow`, `thiserror`, `slog`, `fern`, `log` (except as dev-dep) in `[dependencies]`
  - No `set_global_default`, `EnvFilter`, `tracing_subscriber::fmt::Layer` references in `src/`
  - No `LICENSE` file exists in repo root (user explicitly excluded)
  - Cargo package name is `polykit` (not `polykit-rust`)
  - Edition is `2024`, MSRV pinned at `1.85`

  **Acceptance**: Oracle returns explicit `APPROVED` or `REJECTED: <list of violations>`. On rejection, fix violations and resubmit.

  **Tool**: `task(subagent_type="oracle", load_skills=[], run_in_background=true, prompt="Read .sisyphus/plans/polylog-rust.md and the implementation in src/, tests/, examples/, .github/, Cargo.toml. Audit task-by-task compliance. Return APPROVED or REJECTED with specific violations.")`

  **Evidence**: `.sisyphus/evidence/F1-oracle-compliance.txt`

- [ ] F2. Code Quality Review — unspecified-high

  **What to do**: Read every Rust file in `src/`, `tests/`, `examples/` and review for:
  - Idiomatic Rust 2024 style (no leftover `match` where `if let` fits, etc.)
  - Clippy clean under `-D warnings` (re-run and verify)
  - No dead code, no commented-out code, no TODO/FIXME comments
  - No `unwrap()` / `expect()` outside test code (in production code, prefer `?` and proper error types)
  - Documentation completeness: every `pub` item has a `///` doc comment
  - Consistent naming and module organization
  - No emojis or sycophantic comments anywhere in the codebase
  - Hand-rolled error impls in `error.rs` are correct (Display formats, source chain works)

  **Acceptance**: Returns `APPROVED` or `REJECTED: <list of issues>`.

  **Tool**: `task(category="unspecified-high", load_skills=["simplify"], run_in_background=true, prompt="Review code quality of polykit-rust implementation. Read all files in src/, tests/, examples/. Check for idiomatic Rust 2024, clippy cleanliness, doc completeness on pub items, no unwrap/expect in production, no dead code. Return APPROVED or REJECTED with specifics.")`

  **Evidence**: `.sisyphus/evidence/F2-quality.txt`

- [ ] F3. Real Manual QA Execution — unspecified-high

  **What to do**: Actually execute the full verification matrix end-to-end (don't just read tests):
  1. Clone-fresh checkout simulation: `cargo clean && cargo build --all-targets`
  2. `cargo fmt --all --check`
  3. `cargo clippy --all-targets --all-features -- -D warnings`
  4. `cargo test --all-targets --all-features` — all tests including subprocess tests
  5. `cargo test --doc`
  6. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
  7. Run `cargo run --example init_harness first_init` and visually verify (well, programmatically via stderr capture) the output format matches: timestamp + level + message
  8. Run with `NO_COLOR=1 cargo run --example init_harness first_init 2>&1 | grep -c $'\x1b'` → should be 0
  9. Run with `FORCE_COLOR=1 cargo run --example init_harness first_init 2>&1 | grep -c $'\x1b'` → should be > 0
  10. Inspect a real log file: spawn the file_init_creates_dir scenario, then `cat` the resulting log file and assert the format `[YYYY-MM-DD HH:MM:SS] [LEVEL] target file:line: msg`

  **Acceptance**: Every step produces exit 0 and (where applicable) the asserted output. Returns `APPROVED` or `REJECTED: <step that failed + log>`.

  **Tool**: `task(category="unspecified-high", load_skills=[], run_in_background=true, prompt="Execute the full verification matrix for polykit-rust:\\n1. cargo clean && cargo build --all-targets\\n2. cargo fmt --all --check\\n3. cargo clippy --all-targets --all-features -- -D warnings\\n4. cargo test --all-targets --all-features\\n5. cargo test --doc\\n6. RUSTDOCFLAGS=\"-D warnings\" cargo doc --no-deps\\n7-10: see plan F3 step list. Capture all output. Return APPROVED if every step succeeds, REJECTED with specific step + log otherwise.")`

  **Evidence**: `.sisyphus/evidence/F3-manual-qa.txt`

- [ ] F4. Scope Fidelity Check — deep

  **What to do**: Verify that the implementation honors v0.1 scope decisions and Metis directives literally. Specifically check that NONE of the deferred features leaked in:
  - No Supabase handler module/file (`grep -r supabase src/` → nothing)
  - No TimeAwareLogger (`grep -r time_aware src/` / `grep -r TimeAware src/` → nothing)
  - No PolyEnv integration (`grep -r PolyEnv src/` → nothing)
  - No log groups / capture / persistence / measurements (Swift extras)
  - No JSON formatter (`grep -r json src/log/` → at most documentation comments, no code path)
  - No `critical!` macro / CRITICAL level (`grep -ri critical src/log/` → at most v0.1 explanation comments)
  - No size-based rotation
  - No `RUST_LOG` / `EnvFilter`
  - No reload layer (`grep -r reload src/log/` → nothing)
  - No unsafe code (`grep -r 'unsafe' src/` → only the `forbid(unsafe_code)` line)
  - No `LICENSE` file in repo root
  - README does not advertise CRITICAL or any deferred feature as "supported"

  **Acceptance**: All checks pass. Returns `APPROVED` or `REJECTED: <list of leaked features>`.

  **Tool**: `task(category="deep", load_skills=[], run_in_background=true, prompt="Scope fidelity audit for polykit-rust v0.1. Run the grep checks listed in F4 of .sisyphus/plans/polylog-rust.md. Verify NO deferred features leaked into src/, tests/, examples/, README.md, Cargo.toml. Return APPROVED or REJECTED with specific leaks.")`

  **Evidence**: `.sisyphus/evidence/F4-scope-fidelity.txt`

## Commit Strategy

One commit per wave (6 implementation commits + 0 verification commits). Verification fixes go into amending commits or follow-up commits as needed.

| Commit | Wave | Message | Files |
|--------|------|---------|-------|
| 1 | W1 | `chore: bootstrap polykit crate with library scaffold and CI` | Cargo.toml, src/lib.rs (skeleton), src/log/mod.rs (skeleton), .gitignore, rust-toolchain.toml, README.md, .github/workflows/ci.yml |
| 2 | W2 | `feat(log): add core types: Level, FormatMode, ColorMode, InitError, InitGuard` | src/log/level.rs, src/log/format.rs, src/log/error.rs, src/log/mod.rs |
| 3 | W3 | `feat(log): add builder, console layer, and file layer` | src/log/builder.rs, src/log/console.rs, src/log/file.rs, src/log/mod.rs |
| 4 | W4 | `feat(log): wire init, level override, and catch helper` | src/log/init.rs, src/log/level_override.rs, src/log/catch.rs, src/log/level.rs (amend), src/log/error.rs (amend), src/log/mod.rs |
| 5 | W5 | `feat(log): expose public API via module aggregator and lib root` | src/log/mod.rs, src/lib.rs |
| 6 | W6 | `test(log): add formatter goldens, init subprocess tests, and smoke suite` | src/log/console.rs (amend), src/log/file.rs (amend), tests/init_behavior.rs, tests/smoke.rs, examples/init_harness.rs, Cargo.toml (dev-dep) |

After F1-F4 all APPROVE and user explicitly says "okay", a tag commit:
- Tag: `v0.1.0`
- No additional commits unless verification surfaces defects

If F1-F4 surface defects, fix in scoped commits with conventional messages (`fix(log): ...`) and re-run the verification wave.

## Success Criteria

The plan is complete when ALL of the following are simultaneously true:

1. **Build & quality gates pass on a clean checkout**:
   - `cargo build --all-targets --all-features` exit 0
   - `cargo fmt --all --check` exit 0
   - `cargo clippy --all-targets --all-features -- -D warnings` exit 0
   - `cargo test --all-targets --all-features` exit 0, including all subprocess tests
   - `cargo test --doc` exit 0
   - `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` exit 0
   - CI workflow passes on both ubuntu-latest and macos-latest

2. **Public API surface is exactly the agreed shape**:
   - `polykit::log::init() -> LogBuilder`
   - `polykit::log::Level::{Debug, Info, Warn, Error}` (no Critical, no Trace)
   - `polykit::log::FormatMode::{Simple, Normal, Context}`
   - `polykit::log::ColorMode::{Auto, Always, Never}`
   - `polykit::log::LogBuilder` with `.level/.format/.color/.log_file/.install`
   - `polykit::log::{InitError, InitGuard}`
   - `polykit::log::LogLevelOverride`
   - `polykit::log::catch`
   - Re-exported tracing macros at `polykit::log::{debug, info, warn, error, trace, span, event, instrument}`
   - Re-exported `polykit::log::tracing` for full access
   - Nothing else publicly exposed; in particular no `LogConfig`, `ConsoleLayer`, `FileLayer`, `MIN_LEVEL`

3. **Behavior parity with Python where decided** (and explicit divergence where decided):
   - Console format strings match T8 golden tests byte-for-byte
   - File format string matches T9 golden test byte-for-byte
   - Level labels render exactly as `[DEBUG]` / `[INFO]` / `[WARN]` / `[ERROR]`
   - Console default to stderr, color via NO_COLOR/FORCE_COLOR/TTY rules
   - File output is plain ASCII (no ANSI), 24-hour ISO timestamp
   - File rotation is **daily** (documented divergence from Python's 512KB)
   - Second `init().install()` returns `InitError::AlreadyInitialized`
   - `LogLevelOverride` is process-global (documented divergence from Python's per-logger)
   - `catch` is closure-based and walks `Error::source()` chain
   - tracing-log bridge is active (calls to `log::info!` etc. flow through)

4. **Scope discipline** (F4 audit clean):
   - No Supabase, TimeAwareLogger, PolyEnv, log groups, capture, persistence, measurements
   - No JSON formatter, no `critical!`, no size-based rotation, no `RUST_LOG`/EnvFilter, no reload layer
   - No `LICENSE` file
   - No `unsafe` code

5. **Documentation**:
   - Crate-level rustdoc on `lib.rs` and `log/mod.rs` is complete and compiles
   - Doctest in `log/mod.rs` quickstart example compiles
   - README.md quickstart matches the actual API
   - Every `pub` item has a `///` doc comment
   - `cargo doc --no-deps` succeeds with `RUSTDOCFLAGS="-D warnings"`

6. **All four verification agents (F1-F4) return APPROVED**, and the user explicitly confirms "okay".
