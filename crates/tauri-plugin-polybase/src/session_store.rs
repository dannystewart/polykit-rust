//! Persist the polybase session payload to the Tauri store plugin (`session.dat`).
//!
//! Mirrors the existing Tauri Prism convention so existing app data continues to load. The
//! frontend's `supabase-js` `TauriStoreStorage` writes the session under a primary key derived
//! from the project ref; this module reads it back on startup.
//!
//! These helpers are intentional placeholders consumed once the Tauri plugin's startup hook is
//! wired up to load the persisted session before commands fire.
#![allow(dead_code)]

use polybase::auth::SessionPayload;

/// Default file name used by Tauri Prism today.
pub(crate) const DEFAULT_STORE_FILE: &str = "session.dat";

/// Stable storage key for the polybase session payload.
pub(crate) const POLYBASE_SESSION_KEY: &str = "polybase.session";

/// Helper to load a session payload from a JSON value held in the Tauri store.
pub(crate) fn parse_session(value: &serde_json::Value) -> Option<SessionPayload> {
    serde_json::from_value(value.clone()).ok()
}

/// Helper to render a session payload as a JSON value for the Tauri store.
pub(crate) fn render_session(
    payload: &SessionPayload,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(payload)
}

/// Marker type so callers have something to depend on; concrete plumbing lives in the host app
/// because tauri-plugin-store v2 takes an `&AppHandle` for `Store::load(...)`.
pub(crate) struct TauriSessionPersister;
