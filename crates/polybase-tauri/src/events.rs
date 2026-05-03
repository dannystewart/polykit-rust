//! Forward [`polybase::events::PolyEvent`] values to the JS frontend as Tauri events.
//!
//! Apps can attach an `EventForwarder` to subscribe to the polybase `EventBus` and translate
//! every event into `app_handle.emit("polybase:<kind>", payload)`. The Svelte/React frontend
//! then listens via `@tauri-apps/api/event::listen("polybase:session_changed", ...)` etc.

use polybase::events::{EventBus, PolyEvent};
use tauri::{AppHandle, Emitter, Runtime};

/// Spawn a task that forwards every [`PolyEvent`] to the Tauri frontend.
pub struct EventForwarder;

impl EventForwarder {
    /// Subscribe to `bus` and forward events under `polybase:<kind>` event names. The returned
    /// task handle keeps the forwarder alive; drop it to stop forwarding.
    ///
    /// Spawned via [`tauri::async_runtime::spawn`] so the caller does not need to be inside a
    /// Tokio runtime context — this is important because Tauri's `setup` closure runs on the
    /// main thread before the runtime is `current`, so a bare `tokio::spawn` would panic with
    /// "there is no reactor running".
    pub fn spawn<R: Runtime>(
        handle: AppHandle<R>,
        bus: &EventBus,
    ) -> tauri::async_runtime::JoinHandle<()> {
        let mut rx = bus.subscribe();
        tauri::async_runtime::spawn(async move {
            while let Ok(event) = rx.recv().await {
                let event_name = topic_for(&event);
                let _ = handle.emit(event_name, &event);
            }
        })
    }
}

fn topic_for(event: &PolyEvent) -> &'static str {
    match event {
        PolyEvent::SessionChanged { .. } => "polybase:session_changed",
        PolyEvent::RealtimeChanged { .. } => "polybase:realtime_changed",
        PolyEvent::OfflineQueueChanged { .. } => "polybase:offline_queue_changed",
        PolyEvent::ReconcileProgress { .. } => "polybase:reconcile_progress",
        PolyEvent::PullProgress { .. } => "polybase:pull_progress",
        PolyEvent::KvsChanged { .. } => "polybase:kvs_changed",
    }
}
