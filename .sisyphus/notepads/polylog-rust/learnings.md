## 2026-04-29

- Prefer `#[derive(Default)]` + `#[default]` for enum defaults to keep clippy quiet.
- `std::str::FromStr` is the right home for string parsing helpers that look like `from_str`.
- Environment-variable tests need serialization; parallel Rust tests can race on process-global env state.
- F4 scope audit passed: deferred features did not leak; only `capture` matches were unrelated environment-test snapshot helpers, and `critical` matches were explicit no-CRITICAL documentation/tests.

- Removing dead fields from logger layers is easiest when `init.rs` owns shared min-level/timezone state and layers only render/write.
- `write!(&mut String, ...)` is a clean replacement for per-iteration `format!` allocations inside field loops.
- File-layer golden tests are easiest to make exact by extracting a pure `format_file(level, ts, target, file_basename, line_num, msg)` helper and passing fixed timestamps from tests.
