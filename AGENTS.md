# polykit-rust

## Project Context

This is a Cargo workspace centered on **PolyBase v2 (Rust)** — a hybrid Supabase library that
sits underneath my Rust / Tauri apps the same way [polykit-swift's PolyBase](https://github.com/dannystewart/polykit-swift)
sits under my Swift apps. The Rust side exists because I'm increasingly building Tauri apps
(starting with [Prism Tauri](../Prism/Tauri/)) and rewriting the Swift PolyBase patterns piece
by piece in Rust as I cut Tauri Prism over to it.

The reference consumer is **Tauri Prism** (`../Prism/Tauri/`). When in doubt about how
something should integrate, check there first.

## Status

Per-subsystem cutover, in progress:

- ✅ KVS (preferences-style key/value rows) — live in Tauri Prism, replacing iCloud KVS.
- 🚧 Sync / realtime / storage / auth / encryption — wired in the library, partial cutover in Prism.
- 🚧 Realtime transport — feature-gated, crate-internal until the WebSocket subscriber ships.

## Crates

| Crate | What it does | When you'd touch it |
|-------|-------------|---------------------|
| [`polylog`](crates/polylog/) | Branded `tracing`-based logger. | Adding a log level/format/feature. Independent of polybase. |
| [`polybase`](crates/polybase/) | Core Supabase library: client, registry, sync coordinator, KVS, encryption, edge calls, storage, sessions. | Most data-layer work. Read its README first. |
| [`polybase-sqlite`](crates/polybase-sqlite/) | Default `LocalStore` implementation using `sqlx` + SQLite. Per-user mirror DB layout. | Schema changes to the local mirror, or migration story. |
| [`tauri-plugin-polybase`](crates/tauri-plugin-polybase/) | Tauri 2 plugin wrapping polybase. Exposes `plugin:polybase|*` commands and `polybase:*` events. | JS surface changes, Tauri permissions, plugin lifecycle. |

Each crate has its own `README.md` with the high-level map and links into rustdoc for detail.

## Approach

**Architecture before fixes.** The library is layered intentionally — host app → Coordinator
→ pluggable LocalStore / SessionStore / OfflineQueue. Surgical fixes are preferred. When
something feels like a band-aid, check whether the layering is wrong.

**Single source of truth lives in rustdoc.** READMEs are the high-level map. If you find
yourself wanting to write more than a few sentences of conceptual prose in a README, that
content probably belongs in `//!` module docs instead — they show up on docs.rs and `cargo doc`,
and the README links to them.

**Workspace lints are strict.** `unsafe_code = deny`, `missing_docs = warn`, `unreachable_pub = warn`,
`clippy::pedantic = warn`. Public items need docs. Items that don't need to be public should be
`pub(crate)`. See `Cargo.toml` for the full set.

**Test, don't just check.** `cargo check` validates compilation in isolation; the smoke tests
in `polylog` and the doctests in `polybase` catch real regressions (missing pub, broken
re-exports, generic bound issues). Always run `cargo test --workspace` before declaring done.

## Working Together

- I'm primarily a Swift dev — I'm learning Rust on the job. Don't miss opportunities to teach
  while we work, especially for Rust-specific patterns (lifetimes, trait objects, async, etc.).
- Always ask if a design decision is genuinely ambiguous — don't just guess. Surface trade-offs
  clearly and let me decide.
- The Tauri Prism repo is the reference consumer. When proposing API changes, sketch how the
  Prism call sites would look first — if it's awkward there, it's wrong here.

## High Risk Areas

These are subtle and have produced regressions before. **Confirm with me before changing:**

- **Sync coordinator** (`crates/polybase/src/sync/coordinator.rs`) — single dispatcher for all
  writes. Every change touches the Echo / OfflineQueue / LocalStore / Pusher / EdgeClient at
  once. Easy to break invariants like "echo marked BEFORE network call" or "local mirror
  written FIRST so reads stay consistent on push failure".
- **Registry write paths** (`crates/polybase/src/registry/mod.rs`) — `WritePath` + `conflict_target`
  decide where mutations go. Wrong combination = `42P10` (PostgREST) or 4xx (Edge). The
  registration pattern is the worked KVS example in the registry rustdoc.
- **Encryption format** (`crates/polybase/src/encryption.rs`) — wire-compatible with
  PolyBase Swift. Changing key derivation or framing breaks cross-platform sync.
- **DbManager user switching** (`crates/polybase-sqlite/src/manager.rs`) — `switch_user`
  closes the previous user's pool before opening the new one. Bugs here corrupt local state
  on account switch.

## Build & Test

```bash
# Whole workspace, fast iteration:
cargo check --workspace

# Run all tests (the doctests in polybase catch a lot):
cargo test --workspace

# Workspace-wide lints:
cargo clippy --workspace --all-targets -- -D warnings
```

There's no separate "run the app" — that's Tauri Prism's job. Validate this workspace with
`cargo test`, then build / run Tauri Prism to validate end-to-end.

## When Touching PolyKit (the Swift Sibling)

Per my user rules: changes to `polykit-swift` need their own `swift test` validation. This
workspace does NOT depend on the Swift sibling, but the encryption format and KVS table shape
ARE shared contracts — when you change one, check whether the other needs the same change.
