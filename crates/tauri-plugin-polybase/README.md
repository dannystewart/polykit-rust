# tauri-plugin-polybase

Tauri 2 plugin that exposes the [`polybase`](../polybase/) library to the JS frontend through
the standard `plugin:polybase|*` invoke namespace and forwards backend events under the
`polybase:*` topic prefix.

> **The reference consumer is [Tauri Prism](https://github.com/dannystewart/prism-tauri).**
> When the docs leave a question open, look at how Prism does it.

## What it gives you

- A `RuntimeHandle` Tauri-managed state object holding the configured `Client`, `SessionStore`,
  `Coordinator`, `Kvs`, and `EventBus`.
- A unified command surface (15 commands at the time of writing — see below).
- An `EventForwarder` that relays `polybase::events::PolyEvent`s to the JS frontend.
- A `FileBackedQueue` — production-ready persistent `OfflineQueue` for non-test apps.
- Auto-generated Tauri permissions via `build.rs` so app capabilities just need
  `"polybase:default"`.

## Quickstart

```rust,ignore
use std::sync::Arc;
use polybase::{LocalStore, OfflineQueue};
use polybase_sqlite::SqliteLocalStore;
use tauri::Manager;
use tauri_plugin_polybase::{Builder, EventForwarder, FileBackedQueue, RuntimeHandle};

fn main() {
    tauri::Builder::default()
        .plugin(Builder::new().build())
        .setup(|app| {
            let runtime: tauri::State<RuntimeHandle> = app.state();
            let app_data = app.path().app_data_dir().unwrap();

            // Plug in the LocalStore (per-user SQLite mirror) and OfflineQueue.
            let local: Arc<dyn LocalStore> = Arc::new(SqliteLocalStore::new(/* see polybase-sqlite */));
            let queue: Arc<dyn OfflineQueue> = Arc::new(FileBackedQueue::new(
                app_data.join("offline_queue.json"),
            ));
            tauri::async_runtime::block_on(runtime.attach(local, queue));

            // Forward polybase events to the JS frontend (kvs_changed, session_changed, etc.).
            let bus = tauri::async_runtime::block_on(runtime.events());
            EventForwarder::spawn(app.handle().clone(), &bus);
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap();
}
```

In `src-tauri/capabilities/default.json` (or wherever your capabilities live):

```json
{
    "permissions": ["polybase:default"]
}
```

## JS surface

All commands live under `plugin:polybase|<name>` and all events under `polybase:<name>`.

### Commands

| Command | Purpose |
|---------|---------|
| `configure` | Initialize the client + session store + encryption from a JSON config. Builds the `Coordinator` IF `attach` has already supplied LocalStore + OfflineQueue. **Must be called once on app start.** |
| `set_session` | Hand a fresh Supabase JWT payload to Rust. Triggers `LocalStore::switch_user` via the on-user-changed hook. **Must be called whenever supabase-js issues a fresh session** (sign-in, refresh, account switch). |
| `clear_session` | Sign-out from Rust's perspective. |
| `current_session` | Read the active session payload back. Useful for JS bootstrap. |
| `edge_call` | Generic Edge Function call. Used for any `*-write` function. |
| `encrypt` / `decrypt` | Encrypt / decrypt a string with the configured secret for the active user. |
| `kvs_get` / `kvs_set` / `kvs_delete` | Typed key-value operations on the `kvs` table. |
| `storage_upload` / `storage_download` / `storage_delete` / `storage_list` / `storage_signed_url` | Supabase Storage adapter. |

### Events

| Event | Payload | Fires when |
|-------|---------|------------|
| `polybase:session_changed` | `{ change, user_id }` | Session is set, refreshed, or cleared. |
| `polybase:kvs_changed` | `{ namespace, key, deleted }` | A KVS row is set or tombstoned (locally OR via realtime). |
| `polybase:offline_queue_changed` | `{ depth, in_flight }` | Queue depth changed. |
| `polybase:sync_status_changed` | `{ ... }` | Sync status transitioned. |

## Configure / set_session ordering

The plugin's commands have a specific dependency order at app boot:

1. `configure` — builds `Coordinator` + `Kvs`. Without this, `set_session`, `kvs_*`, `edge_call`,
   `encrypt`, `decrypt`, and all `storage_*` commands return "polybase not configured" or
   "polybase coordinator not attached".
2. `set_session` — populates the session and triggers `LocalStore::switch_user`. Without a
   user, the per-user SQLite pool isn't open and KVS reads error out.
3. Everything else.

**Best practice:** wrap `configure` in a single-flight idempotent helper on the JS side and
have every other consumer await it before invoking. See [Tauri Prism's `src/lib/polybase.ts`](https://github.com/dannystewart/prism-tauri/blob/main/src/lib/polybase.ts)
for the canonical pattern.

## Permissions

`build.rs` auto-generates `polybase:allow-<command>` and `polybase:deny-<command>` permissions
for every command in the [`COMMANDS` list](build.rs). The default permission set
([`permissions/default.toml`](permissions/default.toml)) allows everything — either include
`"polybase:default"` in your capabilities, or pick individual `polybase:allow-*` entries.

## Plugin name

The crate name is `tauri-plugin-polybase` because Tauri's plugin system derives the namespace
from the package name with `tauri-plugin-` stripped — that's how `polybase:default` ends up
being the right permission identifier rather than `tauri-plugin-polybase:default`.

## License

MIT — see workspace [`LICENSE`](../../LICENSE).
