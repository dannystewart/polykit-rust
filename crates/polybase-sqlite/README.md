# polybase-sqlite

Default `sqlx` + SQLite backends for the polybase persistence traits:

- [`SqliteLocalStore`](src/store.rs) — implements [`polybase::LocalStore`](../polybase/src/persistence.rs).
- [`SqliteOfflineQueue`](src/queue.rs) — implements [`polybase::offline_queue::OfflineQueue`](../polybase/src/offline_queue/mod.rs).

Per-user mirror DB with built-in migration support. Both impls share a single
[`DbManager`](src/manager.rs), so the local mirror and the offline queue live in the same
SQLite file (`{root}/{user_id}/sync.db`) for a coherent per-user state surface.

## When to use it

- ✅ You're building a Tauri app and want polybase to "just work" with a local mirror.
- ✅ You want the same per-user file layout that Tauri Prism uses.
- ✅ You want the offline queue persisted to SQLite (atomic finalize transactions, schema-level
  dedupe via `(table_name, entity_id)` PK) instead of the file-backed default.
- ❌ You're targeting a platform without filesystem access (use a custom `LocalStore`).
- ❌ You have an entirely different local schema (write your own `LocalStore` and/or
  `OfflineQueue` instead — they're just two traits).

## File layout

Each authenticated user gets one SQLite file:

```text
{root}/
  ├─ {user_id_1}/
  │    └─ sync.db
  ├─ {user_id_2}/
  │    └─ sync.db
  └─ ...
```

The root path is whatever the app passes in (typically `app_data_dir().join("polybase")`
inside a Tauri setup block). The per-user subdirectory means [`DbManager::switch_user`](src/manager.rs)
just opens a different file — the previous user's pool is closed cleanly before the new one
opens, and nothing leaks across users.

This matches Tauri Prism's existing layout so the cutover drops in without re-pathing user
data.

## Migrations

The crate ships its own embedded migrator at [`MIGRATOR`](src/lib.rs) which now contains
two migrations:

- `0001_kvs.sql` — the `kvs` table required by [`polybase::Kvs`](../polybase/src/kvs.rs).
- `0002_offline_queue.sql` — the `polybase_queue` table required by
  [`SqliteOfflineQueue`](src/queue.rs).

### Two ways to layer migrations

**Option 1 — let polybase-sqlite ship the polybase-owned tables for you:**

```rust,ignore
use polybase_sqlite::DbManager;

let db = DbManager::new(root_path)
    .with_polybase_migrator(); // creates kvs + polybase_queue
```

**Option 2 — ship the polybase tables inside your app's own migrations:**

```rust,ignore
use polybase_sqlite::DbManager;

let db = DbManager::new(root_path)
    .with_app_migrator(&MY_APP_MIGRATOR); // includes kvs + polybase_queue schema
```

Recommended only when your app already has a migration pipeline that owns those table
names — most apps should prefer Option 1.

## API surface

| Type | Purpose |
|------|---------|
| [`SqliteLocalStore`](src/store.rs) | Implements `polybase::LocalStore`. Hand this to `Coordinator::new`. |
| [`SqliteOfflineQueue`](src/queue.rs) | Implements `polybase::offline_queue::OfflineQueue`. Schema-enforced dedupe, transactional finalize. |
| [`DbManager`](src/manager.rs) | Per-user pool lifecycle (`switch_user`, `current_user`, etc.). |
| [`MIGRATOR`](src/lib.rs) | Embedded `sqlx::migrate::Migrator` for the polybase-owned schema. |
| [`DbManagerError`](src/manager.rs) | Typed errors for the manager. |

## Handling user switching

`SqliteLocalStore::switch_user` is wired up automatically by
[`tauri-plugin-polybase`](../tauri-plugin-polybase/) — when the JS side calls
`set_session`, the `SessionStore::on_user_changed` hook fires and switches the local pool
to the new user's mirror.

If you're using `polybase` directly (without the Tauri plugin), you're responsible for
calling `local_store.switch_user(&new_user_id)` yourself when the session changes. The
[`SessionStore::on_user_changed`](../polybase/src/auth.rs) hook is the right place.

## License

MIT — see workspace [`LICENSE`](../../LICENSE).
