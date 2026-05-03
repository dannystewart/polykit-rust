//! Tauri 2 plugin wiring [`polybase`] into a Tauri app.
//!
//! The plugin exposes a unified command surface and emits events on the JS side under the
//! `polybase:` topic prefix, so the frontend can react to session, sync, queue, and KVS
//! changes without holding its own Supabase HTTP stack for non-auth surfaces.
//!
//! # Wiring
//!
//! ```ignore
//! use std::sync::Arc;
//! use polybase::{LocalStore, NullLocalStore, OfflineQueue, MemoryQueue};
//! use polybase_tauri::{Builder as PolyBaseBuilder, EventForwarder, FileBackedQueue, RuntimeHandle};
//!
//! tauri::Builder::default()
//!     .plugin(PolyBaseBuilder::new().build())
//!     .setup(|app| {
//!         let runtime: tauri::State<RuntimeHandle> = app.state();
//!         let local: Arc<dyn LocalStore> = Arc::new(NullLocalStore); // swap with polybase-sqlite
//!         let queue: Arc<dyn OfflineQueue> = Arc::new(FileBackedQueue::new(
//!             app.path().app_data_dir().unwrap().join("offline_queue.json"),
//!         ));
//!         tauri::async_runtime::block_on(runtime.attach(local, queue));
//!         let bus = tauri::async_runtime::block_on(runtime.events());
//!         EventForwarder::spawn(app.handle().clone(), &bus);
//!         Ok(())
//!     })
//!     .invoke_handler(tauri::generate_handler![/* your app commands */])
//!     .run(tauri::generate_context!())?;
//! ```
//!
//! After setup completes, the JS layer must call `polybase_configure` once with the
//! Supabase URL / anon key / optional encryption secret / storage bucket, then call
//! `polybase_set_session` whenever supabase-js issues a fresh session.

mod commands;
mod events;
mod queue_store;
mod session_store;

pub use commands::*;
pub use events::EventForwarder;
pub use queue_store::FileBackedQueue;

use tauri::Runtime;
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};

/// Plugin builder. Use [`Builder::new`] then [`Builder::build`].
pub struct Builder {
    name: &'static str,
}

impl Builder {
    /// Build a new plugin builder using the default plugin name (`polybase`).
    pub fn new() -> Self {
        Self { name: "polybase" }
    }

    /// Construct the [`TauriPlugin`] ready to register with `tauri::Builder::default().plugin(...)`.
    /// The plugin manages its own [`RuntimeHandle`] state — call `app.state::<RuntimeHandle>()`
    /// inside `.setup(|app| { ... })` to wire up the LocalStore + OfflineQueue.
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        PluginBuilder::new(self.name)
            .invoke_handler(tauri::generate_handler![
                commands::polybase_configure,
                commands::polybase_set_session,
                commands::polybase_clear_session,
                commands::polybase_current_session,
                commands::polybase_edge_call,
                commands::polybase_encrypt,
                commands::polybase_decrypt,
                commands::polybase_kvs_get,
                commands::polybase_kvs_set,
                commands::polybase_kvs_delete,
                commands::polybase_storage_upload,
                commands::polybase_storage_download,
                commands::polybase_storage_delete,
                commands::polybase_storage_list,
                commands::polybase_storage_signed_url,
            ])
            .setup(|app, _api| {
                use tauri::Manager;
                app.manage(RuntimeHandle::new());
                Ok(())
            })
            .build()
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
