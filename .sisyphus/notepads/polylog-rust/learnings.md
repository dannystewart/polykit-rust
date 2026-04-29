## 2026-04-29

- Prefer `#[derive(Default)]` + `#[default]` for enum defaults to keep clippy quiet.
- `std::str::FromStr` is the right home for string parsing helpers that look like `from_str`.
- Environment-variable tests need serialization; parallel Rust tests can race on process-global env state.
