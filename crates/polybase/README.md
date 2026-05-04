# polybase

Hybrid Supabase library for Rust. Sibling to [PolyBase (Swift)](https://github.com/dannystewart/polykit-swift),
rebuilt around a per-entity hybrid write path.

> **Layered library, not a framework.** Each layer has one job and talks to its neighbours
> through a small typed surface. Read the [crate-level architecture diagram in rustdoc](src/lib.rs)
> for the data flow story.

## Why this exists

I have a Swift PolyBase library that's been carrying my chat data through Supabase for years.
Now I'm building Tauri apps and need the same patterns in Rust — same wire-compatible
encryption, same conflict resolution semantics, same offline-replay behavior. PolyBase v2
takes the chance to fix the one thing the Swift version got wrong: trying to push everything
through PostgREST. Some things genuinely belong on the server.

## What's in the box

| Module | Purpose | Read first if... |
|--------|---------|------------------|
| [`registry`](src/registry/mod.rs) | Entity registration, field maps, write-path policy, schema introspection. | Adding a new synced entity. |
| [`sync::coordinator`](src/sync/coordinator.rs) | Single dispatcher for persist / delete. | Anything to do with how a row gets written. |
| [`sync::reducer`](src/sync/reducer.rs) | Pure state machine for sync scheduling decisions (port of Swift `Sync/Core.swift`). | Building a custom sync runtime. |
| [`sync::remote`](src/sync/remote.rs) | `RemoteWriter` / `RemoteReader` traits + Supabase live impls + memory test impls. | Wiring custom backends or tests. |
| [`sync::push`](src/sync/push.rs) | PostgREST upsert/tombstone + `is_permanent_push_error_message` (Swift's 10-pattern classifier). | Deciding "retry or drop" on push errors. |
| [`sync::echo`](src/sync/echo.rs) | `EchoTracker` for echo-then-push self-write suppression. | Sharing echo state between writers and realtime. |
| [`kvs`](src/kvs.rs) | Typed key-value rows on the `kvs` table. | Cross-device preferences. |
| [`auth`](src/auth.rs) | JWT session management, refresh loop, user-changed hook. | Sign-in / sign-out / account switch. |
| [`encryption`](src/encryption.rs) | AES-256-GCM with HKDF-SHA256, wire-compatible with PolyBase Swift. | Field-level encryption. |
| [`edge`](src/edge.rs) | Typed Edge Function client (idempotency keys, structured errors). | Calling `*-write` Edge Functions. |
| [`storage`](src/storage.rs) | Supabase Storage REST adapter (upload, download, list, signed URLs). | File attachments. |
| [`realtime`](src/realtime/mod.rs) | Hardened Phoenix WebSocket client with internal reconnect, heartbeat reply tracking, and stale-connection detection. | Live `postgres_changes` for sync. |
| [`events`](src/events.rs) | Broadcast channels for sync / auth / queue / KVS events. | Reacting to backend changes. |
| [`offline_queue`](src/offline_queue/mod.rs) | Persistent retry queue trait + reducer. | Customizing offline replay. |
| [`persistence`](src/persistence.rs) | `LocalStore` trait — local mirror abstraction. | Plugging in a non-SQLite store. |
| [`client`](src/client.rs) | Configured Supabase HTTP client. | Tuning HTTP / headers. |
| [`contract`](src/contract.rs) | Frozen invariants (version steps, backoff ladder, echo window). | You probably shouldn't. |

## The hybrid write path

This is the central design decision. Each registered entity declares a [`WritePath`](src/registry/mod.rs):

| `WritePath` | When | Examples |
|-------------|------|----------|
| `PostgREST` | User JWT can write the table directly. RLS is enough. | `kvs`, `device_tokens` |
| `Edge { function, default_op }` | Server-side validation, idempotency, or fan-out logic is required. | `messages`, `conversations`, `personas` |

The [`Coordinator`](src/sync/coordinator.rs) reads the registered `WritePath` for the table
and dispatches accordingly. Both paths share the same [`EchoTracker`](src/sync/echo.rs) so
realtime echoes of the host's own writes are suppressed identically.

PostgREST entities additionally specify `conflict_target` — the column list for `on_conflict=`
on the upsert. Most chat-style entities have a single-column `id` PK and use the default
`"id"`; entities with composite PKs override (e.g. KVS uses `"id,user_id"` because its
Supabase PK is `(user_id, namespace, key)`). Wrong target = Postgres error `42P10`.

## Worked example: KVS

KVS is the simplest synced entity, registered for you idempotently by [`Kvs::register`](src/kvs.rs).
The pattern looks like this — see the [registration rustdoc](src/registry/mod.rs) for a
compiling example with a richer chat-style entity alongside.

```rust,ignore
use polybase::{ColumnDef, EntityConfig, Registry};

let registry = Registry::new();
registry.register(
    EntityConfig::synced("kvs", "Kvs")
        .columns([
            ColumnDef::synced("id", "id", "id", "TEXT", "string", false),
            ColumnDef::synced("namespace", "namespace", "namespace", "TEXT", "string", false),
            ColumnDef::synced("key", "key", "key", "TEXT", "string", false),
            ColumnDef::synced("value", "value", "value", "TEXT", "jsonb", false),
            ColumnDef::synced("version", "version", "version", "INTEGER", "integer", false),
            ColumnDef::synced("deleted", "deleted", "deleted", "INTEGER", "boolean", false),
            ColumnDef::synced("updated_at", "updated_at", "updated_at", "TEXT", "string", false),
        ])
        .conflict_target("id,user_id"),  // composite PK on the Supabase side.
);
```

The `kvs` table itself is shipped as `migrations/0001_kvs.sql` (Supabase) and
`../polybase-sqlite/migrations/0001_kvs.sql` (local mirror). Run the Supabase one through
your project's normal migration pipeline.

## Wiring it up

Polybase is a library — it doesn't own a runtime. Most apps wire it through the
[`tauri-plugin-polybase`](../tauri-plugin-polybase/) crate. For non-Tauri or test code, the
quickstart is in the [crate-level rustdoc](src/lib.rs).

The five things every host has to provide:

1. A [`Client`](src/client.rs) (Supabase URL + anon key + optional encryption secret + storage bucket).
2. A [`SessionStore`](src/auth.rs) — built from the client.
3. A [`Registry`](src/registry/mod.rs) with every entity registered.
4. A [`LocalStore`](src/persistence.rs) implementation — `polybase-sqlite::SqliteLocalStore`
   in production, `NullLocalStore` for tests.
5. An [`OfflineQueue`](src/offline_queue/mod.rs) implementation — `MemoryQueue` for tests,
   `tauri_plugin_polybase::FileBackedQueue` for Tauri apps.

The [`Coordinator`](src/sync/coordinator.rs) ties them together.

## Conventions

- **`async/await` throughout.** Built on `tokio` (multi-thread runtime).
- **Errors are typed.** [`PolyError`](src/errors.rs) at the top, per-subsystem variants below.
  No `anyhow` in the public API.
- **Local-first writes.** [`Coordinator`](src/sync/coordinator.rs) writes the local mirror
  BEFORE the network call. On transient failure, the network op enqueues for replay; reads
  stay consistent either way.
- **Echo-then-push.** The [`EchoTracker`](src/sync/echo.rs) is marked BEFORE the network call
  so realtime can never re-deliver our own write into the merge pipeline. Critical ordering
  invariant.
- **No backward compat.** This is v2 specifically because v1 was Swift. The Rust API is
  designed to be clean, not a port.

## Features

- `realtime` (default on) — [`SupabaseRealtimeTransport`](src/realtime/mod.rs): hardened
  Phoenix WebSocket subscriber with internal reconnect (250 ms first attempt, exponential
  backoff with ±20% jitter), heartbeat reply tracking, and stale-connection detection via
  liveness probes. The transport survives socket churn transparently — the `Subscription`
  receivers stay valid across reconnects. Disable with `default-features = false` if you only
  need pull/push.

## License

MIT — see workspace [`LICENSE`](../../LICENSE).
