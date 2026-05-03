//! Tauri 2 plugin wiring [`polybase`] into a Tauri app.
//!
//! The plugin exposes a unified command surface (`polybase_set_session`, `polybase_edge_call`,
//! `polybase_kvs_*`, `polybase_storage_*`, `polybase_encrypt`/`polybase_decrypt`, etc.) and emits
//! events (`polybase:session-changed`, `polybase:realtime-changed`, etc.) so the JS frontend can
//! react without holding its own Supabase HTTP stack for non-auth surfaces.
//!
//! Build the plugin in `tauri::Builder::default()`:
//!
//! ```ignore
//! use polybase_tauri::Builder as PolyBaseBuilder;
//!
//! tauri::Builder::default()
//!     .plugin(PolyBaseBuilder::new().build())
//!     // ... other plugins
//!     .run(tauri::generate_context!())?;
//! ```

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
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        PluginBuilder::new(self.name)
            .invoke_handler(tauri::generate_handler![
                commands::polybase_configure,
                commands::polybase_set_session,
                commands::polybase_clear_session,
                commands::polybase_edge_call,
                commands::polybase_encrypt,
                commands::polybase_decrypt,
                commands::polybase_kvs_set,
                commands::polybase_kvs_delete,
                commands::polybase_storage_upload,
                commands::polybase_storage_download,
                commands::polybase_storage_delete,
            ])
            .build()
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
