# polybase-sqlite

Default [`polybase::LocalStore`](../polybase/src/persistence.rs) implementation backed by
`sqlx` + SQLite. Per-user mirror DB with built-in migration support.

## When to use it

- ✅ You're building a Tauri app and want polybase to "just work" with a local mirror.
- ✅ You want the same per-user file layout that Tauri Prism uses.
- ❌ You're targeting a platform without filesystem access (use a custom `LocalStore`).
- ❌ You have an entirely different local schema (write your own `LocalStore` instead — it's
  just one trait).

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

The crate ships its own embedded migrator at [`MIGRATOR`](src/lib.rs) which currently
contains a single migration: the `kvs` table required by [`polybase::Kvs`](../polybase/src/kvs.rs).

### Two ways to layer migrations

**Option 1 — let polybase-sqlite ship `kvs` for you:**

```rust,ignore
use polybase_sqlite::{DbManager, MIGRATOR};

let db = DbManager::new(root_path)
    .with_polybase_migrator(&MIGRATOR);
```

**Option 2 — ship `kvs` inside your app's own migrations:**

```rust,ignore
use polybase_sqlite::DbManager;

let db = DbManager::new(root_path)
    .with_app_migrator(&MY_APP_MIGRATOR); // includes the kvs schema
```

Recommended for the Tauri Prism cutover so there's a single source of truth — the project
already has its own migration pipeline and the `kvs` migration ships there.

## API surface

| Type | Purpose |
|------|---------|
| [`SqliteLocalStore`](src/store.rs) | Implements `polybase::LocalStore`. Hand this to `Coordinator::new`. |
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
