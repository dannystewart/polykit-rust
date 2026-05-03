# polykit-rust

Personal Rust workspace centered on **PolyBase v2 (Rust)** — a hybrid Supabase library for
my Tauri apps. Sibling to [polykit](https://github.com/dannystewart/polykit) (Python) and
[polykit-swift](https://github.com/dannystewart/polykit-swift) (Swift).

The Rust port exists because I'm building Tauri apps now (starting with [Prism Tauri](https://github.com/dannystewart/prism-tauri))
and want the same battle-tested data patterns I have in Swift, with the same wire-compatible
encryption and the same Supabase contracts.

## Crates

| Crate | What | Status |
|-------|------|--------|
| [`polylog`](crates/polylog/) | Branded `tracing`-based logger. | Stable |
| [`polybase`](crates/polybase/) | Supabase client + registry + sync coordinator + KVS + encryption + edge calls + storage. | In active development |
| [`polybase-sqlite`](crates/polybase-sqlite/) | Default `LocalStore` implementation (`sqlx` + SQLite). | In active development |
| [`tauri-plugin-polybase`](crates/tauri-plugin-polybase/) | Tauri 2 plugin wrapping polybase. | In active development |

## Design

PolyBase v2's headline change vs. the Swift sibling is the **hybrid write path**:

- **Synced chat data** (Conversations, Messages, Personas) flows through Supabase **Edge
  Functions** so server-side validation, idempotency, and tool-loop logic all live in one
  place.
- **Lightweight per-user data** (KVS preferences, device tokens) goes direct via **PostgREST**
  with the user JWT — no Edge Function in front, since there's no real business logic to
  protect.

Reads, realtime, reconcile, storage, encryption, and offline replay all live in Rust regardless
of the write path. The decision is per-entity, declared in the [`Registry`](crates/polybase/src/registry/mod.rs).

```text
host app
   │
   ▼
Coordinator ──► Registry → WritePath ──► PostgREST
   │                                ──► Edge Fn
   ├─ LocalStore (write FIRST)
   ├─ OfflineQueue (transient failure replay)
   └─ EchoTracker (suppress own realtime echoes)
```

The headline architecture diagram lives in [`polybase`'s crate-level rustdoc](crates/polybase/src/lib.rs)
so it shows up on `cargo doc` and docs.rs.

## Quickstart

Most apps wire polybase through the Tauri plugin:

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
            let local: Arc<dyn LocalStore> = Arc::new(SqliteLocalStore::new(/* ... */));
            let queue: Arc<dyn OfflineQueue> = Arc::new(FileBackedQueue::new(
                app_data.join("offline_queue.json"),
            ));
            tauri::async_runtime::block_on(runtime.attach(local, queue));
            let bus = tauri::async_runtime::block_on(runtime.events());
            EventForwarder::spawn(app.handle().clone(), &bus);
            Ok(())
        })
        .run(tauri::generate_context!())
        .unwrap();
}
```

Then from JS:

```ts
import { invoke } from "@tauri-apps/api/core"

await invoke("plugin:polybase|configure", {
    config: {
        supabase_url: "https://xxxx.supabase.co",
        supabase_anon_key: "...",
        encryption_secret: "...",
        storage_bucket: "attachments",
    },
})
await invoke("plugin:polybase|set_session", { payload: /* session payload */ })
await invoke("plugin:polybase|kvs_set", {
    args: { namespace: "myapp.settings", key: "theme", value: "dark" },
})
```

See each crate's README for details.

## Reference Consumer

[Tauri Prism](https://github.com/dannystewart/prism-tauri) is the canonical reference
consumer. When the documentation is unclear, the answer is "look at how Prism does it."

## License

MIT — see [`LICENSE`](LICENSE).
