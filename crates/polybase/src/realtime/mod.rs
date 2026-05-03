//! Realtime subscriber over Phoenix WebSocket.
//!
//! Hardened Supabase Realtime client that handles connection lifecycle, reconnect, heartbeat,
//! and stale-connection detection internally. Callers see a stable [`Subscription`] that keeps
//! delivering [`RealtimeChange`] events across internal reconnects — they never need to
//! re-subscribe when the underlying socket churns.
//!
//! # Architecture
//!
//! ```text
//!  ┌──────────────────────────┐
//!  │ SupabaseRealtimeTransport│  ← caller constructs once (url, anon_key, SessionStore)
//!  └────────────┬─────────────┘
//!               │ subscribe(tables)
//!               v
//!  ┌──────────────────────────┐
//!  │      Supervisor task     │  ← owns the connection lifecycle. Loops:
//!  │                          │      connect → run_session → backoff → reconnect.
//!  └────────────┬─────────────┘      Refreshes the access token each connect via SessionStore.
//!               │ spawns
//!               v
//!  ┌──────────────────────────────────────────┐
//!  │   Per-connection inner tasks             │
//!  │     • read:       inbound WS frames      │
//!  │     • write:      outbound queue drainer │
//!  │     • heartbeat:  20s ping cycle with    │
//!  │                   reply-ref tracking     │
//!  │     • liveness:   inbound-silence guard  │
//!  └──────────────────────────────────────────┘
//!               │
//!               v
//!  ┌──────────────────────────┐
//!  │       Subscription       │  ← caller's handle. Two broadcast receivers:
//!  │                          │      • change events (postgres_changes)
//!  └──────────────────────────┘      • lifecycle events (Connected/Disconnected/Reconnected)
//! ```
//!
//! # Recovery features
//!
//! - **Heartbeat reply tracking** — every outbound `heartbeat` is tracked by `ref`. If no
//!   `phx_reply` arrives within `heartbeat_reply_timeout`, the connection is treated as dead
//!   and the supervisor force-reconnects. Catches "zombie" connections that the OS hasn't
//!   yet noticed are broken.
//! - **Stale-connection detection** — if no inbound frame arrives within `liveness_timeout`,
//!   the supervisor sends a synthetic heartbeat probe. If the probe gets no reply within
//!   `liveness_probe_timeout`, force-reconnect.
//! - **Exponential backoff with jitter** — first reconnect attempt fires after
//!   `initial_reconnect_delay` (default 250ms); subsequent attempts walk a configurable
//!   ladder with ±20% jitter to avoid thundering-herd reconnect storms.
//! - **Token refresh on reconnect** — each reconnect calls
//!   [`SessionStore::ensure_fresh`][crate::auth::SessionStore::ensure_fresh] so a refresh
//!   that failed earlier (or a token that aged past the access window) is healed before the
//!   next join.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex as SyncMutex;
use polylog::{debug, info, warn};
use rand::{Rng, thread_rng};
use serde_json::{Map, Value};
use tokio::sync::{Mutex, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::auth::SessionStore;
use crate::events::RealtimeOp;

// MARK: - Public types

/// Decoded `postgres_changes` payload, schema-agnostic.
#[derive(Debug, Clone)]
pub struct RealtimeChange {
    /// Source table name.
    pub table: String,
    /// Insert / update / delete.
    pub op: RealtimeOp,
    /// New row contents (None for DELETE).
    pub record: Option<Map<String, Value>>,
    /// Previous row contents (Some for UPDATE/DELETE; None for INSERT).
    pub old_record: Option<Map<String, Value>>,
}

impl RealtimeChange {
    /// Best-effort entity id extraction from `record` then `old_record`.
    pub fn entity_id(&self) -> Option<String> {
        let data = self.record.as_ref().or(self.old_record.as_ref())?;
        data.get("id").and_then(Value::as_str).map(String::from)
    }
}

/// Lifecycle event for a [`Subscription`]. Hosts subscribe to these to trigger reconcile
/// passes on reconnect, surface a "live/stale" indicator in UI, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Initial connection established and `phx_join` succeeded.
    Connected,
    /// Underlying socket / channel went away. Supervisor will attempt to reconnect.
    Disconnected(DisconnectReason),
    /// Supervisor successfully reconnected after a prior `Disconnected`.
    Reconnected,
}

/// Why the realtime connection went away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Server told us the channel closed (`phx_close`, `phx_error`, or non-ok system message).
    ChannelClosed,
    /// WebSocket-level error (TLS broke, EOF, network gone).
    WebsocketError,
    /// Outbound heartbeat went unacknowledged within the configured timeout.
    HeartbeatTimeout,
    /// No inbound traffic for too long and the synthetic probe also failed.
    LivenessTimeout,
    /// Caller asked for a forced reconnect via the supervisor's mailbox.
    Manual,
}

impl DisconnectReason {
    /// Stable label for logs and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChannelClosed => "channel_closed",
            Self::WebsocketError => "websocket_error",
            Self::HeartbeatTimeout => "heartbeat_timeout",
            Self::LivenessTimeout => "liveness_timeout",
            Self::Manual => "manual",
        }
    }
}

/// Subscription handle. Drop or call [`RealtimeTransport::unsubscribe`] to stop delivery.
///
/// The receivers stay valid across internal reconnects: the supervisor never replaces the
/// underlying broadcast channels, so callers can `select!` on `next()` indefinitely.
pub struct Subscription {
    rx: broadcast::Receiver<RealtimeChange>,
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
}

impl Subscription {
    /// Construct a subscription from raw channel halves. Public so alternative
    /// [`RealtimeTransport`] implementations (e.g. test fixtures, polling backends, custom
    /// WebSocket clients) can build a `Subscription` without going through
    /// [`SupabaseRealtimeTransport`]. Production callers should never need this — they get
    /// a `Subscription` from `transport.subscribe(...)`.
    pub fn new(
        rx: broadcast::Receiver<RealtimeChange>,
        lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    ) -> Self {
        Self { rx, lifecycle_tx }
    }

    /// Wait for the next change. Returns `None` if the underlying channel closes (transport
    /// was unsubscribed). Lagged subscribers transparently skip dropped events.
    pub async fn next(&mut self) -> Option<RealtimeChange> {
        loop {
            match self.rx.recv().await {
                Ok(change) => return Some(change),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(target: "realtime", skipped, "subscription lagged; dropping events");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Get a fresh receiver for lifecycle events. Multiple consumers can call this to react
    /// to connect/disconnect/reconnect independently.
    pub fn lifecycle(&self) -> broadcast::Receiver<LifecycleEvent> {
        self.lifecycle_tx.subscribe()
    }
}

/// Tuning knobs for [`SupabaseRealtimeTransport`]. Defaults are conservative and match
/// production Tauri Prism behavior with the recovery improvements applied.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// How often to send Phoenix heartbeats. Default `20s` (Supabase recommendation).
    pub heartbeat_interval: Duration,
    /// Max wait for the `phx_reply` to a heartbeat before declaring the connection dead.
    /// Default `10s`.
    pub heartbeat_reply_timeout: Duration,
    /// If no inbound message in this window, send a probe. Default `30s`.
    pub liveness_timeout: Duration,
    /// Max wait for the probe response before forcing reconnect. Default `5s`.
    pub liveness_probe_timeout: Duration,
    /// Delay before the very first reconnect attempt after a disconnect. Kept short to
    /// recover fast from transient network blips. Default `250ms`.
    pub initial_reconnect_delay: Duration,
    /// Backoff ladder for sustained reconnect failures. Each entry is the base delay before
    /// the corresponding consecutive failure (clamped to the last entry). Default
    /// `[500ms, 1s, 2s, 5s, 10s, 30s]`.
    pub backoff_ladder: Vec<Duration>,
    /// Jitter applied to each reconnect delay as a fraction (e.g. `0.20` = ±20%). Set to
    /// `0.0` to disable jitter entirely. Default `0.20`.
    pub backoff_jitter: f64,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(20),
            heartbeat_reply_timeout: Duration::from_secs(10),
            liveness_timeout: Duration::from_secs(30),
            liveness_probe_timeout: Duration::from_secs(5),
            initial_reconnect_delay: Duration::from_millis(250),
            backoff_ladder: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(30),
            ],
            backoff_jitter: 0.20,
        }
    }
}

/// Pluggable transport for realtime delivery. The default implementation is
/// [`SupabaseRealtimeTransport`].
#[async_trait]
pub trait RealtimeTransport: Send + Sync {
    /// Subscribe to `postgres_changes` on the given tables. The returned [`Subscription`]
    /// stays valid across internal reconnects.
    async fn subscribe(&self, tables: &[&str]) -> Result<Subscription, RealtimeError>;

    /// Stop and disconnect. Idempotent — safe to call when not subscribed.
    async fn unsubscribe(&self) -> Result<(), RealtimeError>;

    /// Whether a session is currently active (i.e. supervisor running).
    async fn is_subscribed(&self) -> bool;
}

/// Error class for the realtime layer.
#[derive(Debug, thiserror::Error, Clone)]
pub enum RealtimeError {
    /// WebSocket connect or TLS handshake failed.
    #[error("realtime connect failed: {0}")]
    ConnectFailed(String),
    /// Phoenix `phx_join` was rejected.
    #[error("realtime subscribe failed: {0}")]
    SubscribeFailed(String),
    /// Caller asked to subscribe twice without an intervening unsubscribe.
    #[error("realtime already subscribed")]
    AlreadySubscribed,
    /// Underlying broadcast or watch channel was closed unexpectedly.
    #[error("realtime channel closed")]
    Closed,
    /// Failed to obtain a fresh access token from `SessionStore`.
    #[error("realtime auth refresh failed: {0}")]
    AuthRefreshFailed(String),
}

// MARK: - SupabaseRealtimeTransport

/// Default Supabase Realtime transport. Speaks Phoenix v2 over `wss://.../realtime/v1/websocket`.
///
/// Owns its own reconnect / heartbeat / liveness machinery. Construct once per session and
/// reuse — `subscribe`/`unsubscribe` are inexpensive lifecycle calls.
pub struct SupabaseRealtimeTransport {
    supabase_url: String,
    anon_key: String,
    sessions: SessionStore,
    config: TransportConfig,
    state: Arc<Mutex<SupervisorState>>,
}

struct SupervisorState {
    handle: Option<SupervisorHandle>,
}

struct SupervisorHandle {
    shutdown_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
    /// Lifecycle sender owned by the supervisor; we keep a clone here so callers can re-derive
    /// receivers via [`Subscription::lifecycle`] even after the original Subscription is dropped.
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    /// Same idea for the change channel — kept alive so the receivers in dropped Subscriptions
    /// stay valid for any background pumps still consuming them.
    change_tx: broadcast::Sender<RealtimeChange>,
}

/// Capacity of the change broadcast channel. Sized for ~half a second of bursty traffic.
const CHANGE_CHANNEL_CAPACITY: usize = 512;
/// Capacity of the lifecycle channel. A handful of events per session is the norm.
const LIFECYCLE_CHANNEL_CAPACITY: usize = 32;

impl SupabaseRealtimeTransport {
    /// Construct with default [`TransportConfig`].
    pub fn new(supabase_url: String, anon_key: String, sessions: SessionStore) -> Self {
        Self::with_config(supabase_url, anon_key, sessions, TransportConfig::default())
    }

    /// Construct with custom [`TransportConfig`] (e.g. faster heartbeats in tests).
    pub fn with_config(
        supabase_url: String,
        anon_key: String,
        sessions: SessionStore,
        config: TransportConfig,
    ) -> Self {
        Self {
            supabase_url,
            anon_key,
            sessions,
            config,
            state: Arc::new(Mutex::new(SupervisorState { handle: None })),
        }
    }

    fn websocket_url(&self) -> String {
        let base = self.supabase_url.trim_end_matches('/');
        let ws_base = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("wss://{base}")
        };
        format!("{ws_base}/realtime/v1/websocket?apikey={}&vsn=2.0.0", self.anon_key)
    }
}

#[async_trait]
impl RealtimeTransport for SupabaseRealtimeTransport {
    async fn subscribe(&self, tables: &[&str]) -> Result<Subscription, RealtimeError> {
        let mut guard = self.state.lock().await;
        if guard.handle.is_some() {
            return Err(RealtimeError::AlreadySubscribed);
        }

        let (change_tx, change_rx) = broadcast::channel::<RealtimeChange>(CHANGE_CHANNEL_CAPACITY);
        let (lifecycle_tx, _lifecycle_rx) =
            broadcast::channel::<LifecycleEvent>(LIFECYCLE_CHANNEL_CAPACITY);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let tables_owned: Vec<String> = tables.iter().map(|s| (*s).to_string()).collect();

        let supervisor = Supervisor {
            url: self.websocket_url(),
            sessions: self.sessions.clone(),
            config: self.config.clone(),
            tables: tables_owned,
            change_tx: change_tx.clone(),
            lifecycle_tx: lifecycle_tx.clone(),
            shutdown_rx,
        };

        let task = tokio::spawn(supervisor.run());

        guard.handle = Some(SupervisorHandle {
            shutdown_tx,
            task,
            lifecycle_tx: lifecycle_tx.clone(),
            change_tx: change_tx.clone(),
        });

        Ok(Subscription::new(change_rx, lifecycle_tx))
    }

    async fn unsubscribe(&self) -> Result<(), RealtimeError> {
        let mut guard = self.state.lock().await;
        let Some(handle) = guard.handle.take() else {
            return Ok(());
        };
        let _ = handle.shutdown_tx.send(true);
        // Drop senders so the receivers close cleanly when the supervisor exits.
        drop(handle.lifecycle_tx);
        drop(handle.change_tx);
        // Best-effort wait for the supervisor to drain. Don't block forever if it's wedged.
        let _ = tokio::time::timeout(Duration::from_secs(3), handle.task).await;
        Ok(())
    }

    async fn is_subscribed(&self) -> bool {
        self.state.lock().await.handle.is_some()
    }
}

// MARK: - Supervisor

/// Long-running task that owns one realtime "session" (across many actual WebSocket connections).
///
/// Loops: `connect` → `run_session` → publish disconnect → backoff sleep → reconnect.
/// Stops only when the shutdown signal flips to `true`.
struct Supervisor {
    url: String,
    sessions: SessionStore,
    config: TransportConfig,
    tables: Vec<String>,
    change_tx: broadcast::Sender<RealtimeChange>,
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    shutdown_rx: watch::Receiver<bool>,
}

impl Supervisor {
    async fn run(mut self) {
        let mut consecutive_failures: u32 = 0;
        let mut had_prior_connection = false;

        loop {
            if *self.shutdown_rx.borrow() {
                debug!(target: "realtime", "supervisor shutting down (pre-connect)");
                break;
            }

            let access_token = match self.sessions.ensure_fresh(Duration::from_secs(60)).await {
                Ok(Some(result)) => result.session.access_token,
                Ok(None) => {
                    warn!(target: "realtime", "no active session; skipping connect");
                    if self.sleep_with_backoff(consecutive_failures).await {
                        break;
                    }
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    continue;
                }
                Err(error) => {
                    warn!(
                        target: "realtime",
                        error = %error,
                        "session refresh failed before realtime connect"
                    );
                    if self.sleep_with_backoff(consecutive_failures).await {
                        break;
                    }
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    continue;
                }
            };

            match Connection::connect(
                &self.url,
                &self.tables,
                &access_token,
                &self.config,
                self.change_tx.clone(),
            )
            .await
            {
                Ok(connection) => {
                    consecutive_failures = 0;
                    let event = if had_prior_connection {
                        LifecycleEvent::Reconnected
                    } else {
                        LifecycleEvent::Connected
                    };
                    info!(
                        target: "realtime",
                        event = if had_prior_connection { "reconnected" } else { "connected" },
                        table_count = self.tables.len(),
                        "realtime connection established"
                    );
                    let _ = self.lifecycle_tx.send(event);
                    had_prior_connection = true;

                    let reason = connection.run(self.shutdown_rx.clone()).await;

                    if matches!(reason, DisconnectReason::Manual) {
                        debug!(target: "realtime", "supervisor shutting down (post-session)");
                        let _ = self.lifecycle_tx.send(LifecycleEvent::Disconnected(reason));
                        break;
                    }

                    warn!(
                        target: "realtime",
                        reason = reason.as_str(),
                        "realtime session ended; will reconnect"
                    );
                    let _ = self.lifecycle_tx.send(LifecycleEvent::Disconnected(reason));
                }
                Err(error) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    warn!(
                        target: "realtime",
                        attempt = consecutive_failures,
                        error = %error,
                        "realtime connect failed"
                    );
                }
            }

            if self.sleep_with_backoff(consecutive_failures).await {
                break;
            }
        }

        debug!(target: "realtime", "supervisor exited");
    }

    /// Sleep with backoff. Returns true if shutdown was signaled during the sleep.
    async fn sleep_with_backoff(&mut self, consecutive_failures: u32) -> bool {
        let delay = compute_backoff_delay(consecutive_failures, &self.config);
        debug!(
            target: "realtime",
            delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            attempt = consecutive_failures,
            "sleeping before reconnect"
        );

        tokio::select! {
            () = tokio::time::sleep(delay) => false,
            changed = self.shutdown_rx.changed() => {
                changed.is_ok() && *self.shutdown_rx.borrow()
            }
        }
    }
}

/// Compute the backoff delay for the Nth consecutive failure (zero-indexed via
/// `consecutive_failures = 0` for the very first reconnect attempt). Applies ±jitter.
fn compute_backoff_delay(consecutive_failures: u32, config: &TransportConfig) -> Duration {
    let base = if consecutive_failures == 0 {
        config.initial_reconnect_delay
    } else {
        let idx = (consecutive_failures - 1) as usize;
        let last = config.backoff_ladder.len().saturating_sub(1);
        let clamped = idx.min(last);
        config.backoff_ladder.get(clamped).copied().unwrap_or(Duration::from_secs(30))
    };
    apply_jitter(base, config.backoff_jitter)
}

fn apply_jitter(base: Duration, jitter: f64) -> Duration {
    if jitter <= 0.0 {
        return base;
    }
    let jitter = jitter.clamp(0.0, 1.0);
    let multiplier = 1.0 + thread_rng().gen_range(-jitter..jitter);
    let nanos = (base.as_secs_f64() * multiplier).max(0.0) * 1_000_000_000.0;
    // Bounded by base * 2 in the worst case, so within u64 nanosecond range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Duration::from_nanos(nanos as u64)
}

// MARK: - Connection (single WebSocket session)

/// Outstanding heartbeat refs keyed by ref string, with the time we sent them. Shared between
/// the heartbeat / liveness probe tasks (both insert) and the read task (removes on `phx_reply`).
type HeartbeatTracker = Arc<SyncMutex<HashMap<String, Instant>>>;
/// Wall-clock of the last inbound frame (any kind). The read task updates it; the liveness task
/// reads it to detect inbound silence.
type LastInboundAt = Arc<SyncMutex<Instant>>;

/// One live WebSocket connection. Owns the read/write/heartbeat/liveness tasks while open.
///
/// All connection-scoped state (heartbeat tracker, liveness clock, outbound queue) lives inside
/// the inner tasks. `Connection` itself holds only the disconnect channel and the task handles
/// it needs to abort on teardown.
struct Connection {
    disconnect_rx: mpsc::UnboundedReceiver<DisconnectReason>,
    inner_tasks: Vec<JoinHandle<()>>,
}

impl Connection {
    /// Open a WebSocket, send `phx_join`, spawn inner tasks. Returns a [`Connection`] ready to
    /// `run()` until it dies. Changes parsed off the read side are broadcast directly to
    /// `change_tx` so they flow to subscribers without re-routing.
    async fn connect(
        url: &str,
        tables: &[String],
        access_token: &str,
        config: &TransportConfig,
        change_tx: broadcast::Sender<RealtimeChange>,
    ) -> Result<Self, RealtimeError> {
        let (ws_stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| RealtimeError::ConnectFailed(format!("websocket connect failed: {e}")))?;
        let (mut write, mut read) = ws_stream.split();

        let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<Message>();
        let (disconnect_tx, disconnect_rx) = mpsc::unbounded_channel::<DisconnectReason>();

        let join_ref = "1".to_string();
        let topic = "realtime:polybase-sync".to_string();

        let heartbeat_tracker: HeartbeatTracker = Arc::new(SyncMutex::new(HashMap::new()));
        let last_inbound_at: LastInboundAt = Arc::new(SyncMutex::new(Instant::now()));

        // -- Write task: drains outbound queue.
        let write_task = tokio::spawn(async move {
            while let Some(msg) = outgoing_rx.recv().await {
                if write.send(msg).await.is_err() {
                    break;
                }
            }
            let _ = write.close().await;
        });

        // -- Read task: parses inbound frames, broadcasts changes, watches for disconnect /
        // heartbeat replies / liveness.
        let read_disconnect = disconnect_tx.clone();
        let read_heartbeat = heartbeat_tracker.clone();
        let read_last_inbound = last_inbound_at.clone();
        let read_change_tx = change_tx.clone();
        let read_task = tokio::spawn(async move {
            while let Some(result) = read.next().await {
                match result {
                    Ok(Message::Text(text)) => {
                        *read_last_inbound.lock() = Instant::now();
                        let Some((_topic, event, payload, frame_ref)) =
                            parse_phoenix_message(&text)
                        else {
                            continue;
                        };
                        match event.as_str() {
                            "postgres_changes" => {
                                if let Some(change) = parse_postgres_changes(&payload) {
                                    let _ = read_change_tx.send(change);
                                }
                            }
                            "phx_reply" => {
                                if let Some(ref_id) = frame_ref {
                                    read_heartbeat.lock().remove(&ref_id);
                                }
                                let status = payload.get("status").and_then(Value::as_str);
                                if status == Some("error") {
                                    warn!(
                                        target: "realtime",
                                        payload = %payload,
                                        "phx_reply error"
                                    );
                                }
                            }
                            "system" => {
                                let status = payload
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown");
                                if status != "ok" {
                                    warn!(
                                        target: "realtime",
                                        status,
                                        payload = %payload,
                                        "system message reported non-ok status"
                                    );
                                    let _ = read_disconnect.send(DisconnectReason::ChannelClosed);
                                    break;
                                }
                            }
                            "phx_error" => {
                                warn!(target: "realtime", payload = %payload, "channel error");
                                let _ = read_disconnect.send(DisconnectReason::ChannelClosed);
                                break;
                            }
                            "phx_close" => {
                                warn!(target: "realtime", payload = %payload, "channel closed");
                                let _ = read_disconnect.send(DisconnectReason::ChannelClosed);
                                break;
                            }
                            _ => {}
                        }
                    }
                    Ok(Message::Ping(payload)) => {
                        *read_last_inbound.lock() = Instant::now();
                        // Tungstenite responds to pings automatically via the underlying stream;
                        // we just refresh the liveness clock.
                        let _ = payload;
                    }
                    Ok(Message::Pong(_)) => {
                        *read_last_inbound.lock() = Instant::now();
                    }
                    Ok(Message::Close(_)) => {
                        let _ = read_disconnect.send(DisconnectReason::ChannelClosed);
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        warn!(target: "realtime", error = %e, "websocket error");
                        let _ = read_disconnect.send(DisconnectReason::WebsocketError);
                        break;
                    }
                }
            }
        });

        // -- Heartbeat task: 20s pings with reply tracking.
        let heartbeat_tx = outgoing_tx.clone();
        let heartbeat_disconnect = disconnect_tx.clone();
        let heartbeat_tracker_for_task = heartbeat_tracker.clone();
        let heartbeat_interval = config.heartbeat_interval;
        let heartbeat_reply_timeout = config.heartbeat_reply_timeout;
        let heartbeat_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate first tick; the freshly-opened socket counts as live.
            interval.tick().await;
            let mut next_ref: u64 = 100;
            loop {
                interval.tick().await;
                let ref_id = format!("hb-{next_ref}");
                next_ref = next_ref.wrapping_add(1);
                heartbeat_tracker_for_task.lock().insert(ref_id.clone(), Instant::now());
                let frame = build_phoenix_array(
                    None,
                    &ref_id,
                    "phoenix",
                    "heartbeat",
                    Value::Object(Map::new()),
                );
                if heartbeat_tx.send(Message::Text(frame.to_string().into())).is_err() {
                    break;
                }

                tokio::time::sleep(heartbeat_reply_timeout).await;
                let still_pending = heartbeat_tracker_for_task.lock().contains_key(&ref_id);
                if still_pending {
                    warn!(
                        target: "realtime",
                        ref_id = %ref_id,
                        "heartbeat reply timeout; treating connection as dead"
                    );
                    let _ = heartbeat_disconnect.send(DisconnectReason::HeartbeatTimeout);
                    break;
                }
            }
        });

        // -- Liveness task: detect inbound silence, send synthetic probe.
        let liveness_tx = outgoing_tx.clone();
        let liveness_disconnect = disconnect_tx.clone();
        let liveness_tracker = heartbeat_tracker.clone();
        let liveness_last_inbound = last_inbound_at.clone();
        let liveness_timeout = config.liveness_timeout;
        let liveness_probe_timeout = config.liveness_probe_timeout;
        let liveness_task = tokio::spawn(async move {
            // Check every 1/4 of the liveness window for snappy detection.
            let tick = liveness_timeout / 4;
            let mut interval = tokio::time::interval(tick.max(Duration::from_millis(500)));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            let mut next_ref: u64 = 5000;
            loop {
                interval.tick().await;
                let elapsed = liveness_last_inbound.lock().elapsed();
                if elapsed < liveness_timeout {
                    continue;
                }
                debug!(
                    target: "realtime",
                    elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    "no inbound activity; sending liveness probe"
                );
                let ref_id = format!("live-{next_ref}");
                next_ref = next_ref.wrapping_add(1);
                liveness_tracker.lock().insert(ref_id.clone(), Instant::now());
                let frame = build_phoenix_array(
                    None,
                    &ref_id,
                    "phoenix",
                    "heartbeat",
                    Value::Object(Map::new()),
                );
                if liveness_tx.send(Message::Text(frame.to_string().into())).is_err() {
                    break;
                }
                tokio::time::sleep(liveness_probe_timeout).await;
                let still_pending = liveness_tracker.lock().contains_key(&ref_id);
                if still_pending {
                    warn!(
                        target: "realtime",
                        ref_id = %ref_id,
                        "liveness probe timeout; treating connection as dead"
                    );
                    let _ = liveness_disconnect.send(DisconnectReason::LivenessTimeout);
                    break;
                }
            }
        });

        // -- Send phx_join to subscribe to the requested tables.
        let postgres_changes: Vec<Value> = tables
            .iter()
            .map(|table| {
                serde_json::json!({
                    "event": "*",
                    "schema": "public",
                    "table": table
                })
            })
            .collect();
        let join_payload = serde_json::json!({
            "config": {
                "broadcast": { "ack": false, "self": false },
                "presence": { "enabled": false },
                "postgres_changes": postgres_changes,
                "private": false
            },
            "access_token": access_token
        });
        let join_msg =
            build_phoenix_array(Some(&join_ref), "join-1", &topic, "phx_join", join_payload);
        outgoing_tx
            .send(Message::Text(join_msg.to_string().into()))
            .map_err(|e| RealtimeError::SubscribeFailed(format!("phx_join send failed: {e}")))?;

        // Hand off ownership of outgoing_tx to the supervisor's lifetime by dropping the local
        // copy here. The clones already in the inner tasks keep the mpsc alive while the
        // connection is up.
        drop(outgoing_tx);

        Ok(Self {
            disconnect_rx,
            inner_tasks: vec![read_task, write_task, heartbeat_task, liveness_task],
        })
    }

    /// Run the connection until it dies. Returns the disconnect reason.
    ///
    /// The connection terminates when:
    /// - An inner task surfaces a disconnect reason via `disconnect_rx`.
    /// - The supervisor's `shutdown_rx` flips to `true` (returns `Manual`).
    ///
    /// Inner tasks (read / write / heartbeat / liveness) are aborted on exit.
    async fn run(mut self, mut shutdown_rx: watch::Receiver<bool>) -> DisconnectReason {
        let reason = tokio::select! {
            r = self.disconnect_rx.recv() => r.unwrap_or(DisconnectReason::ChannelClosed),
            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    DisconnectReason::Manual
                } else {
                    DisconnectReason::ChannelClosed
                }
            }
        };

        for task in self.inner_tasks.drain(..) {
            task.abort();
        }

        reason
    }
}

// MARK: - Phoenix framing helpers

fn build_phoenix_array(
    join_ref: Option<&str>,
    ref_field: &str,
    topic: &str,
    event: &str,
    payload: Value,
) -> Value {
    Value::Array(vec![
        join_ref.map_or(Value::Null, |s| Value::String(s.to_string())),
        Value::String(ref_field.to_string()),
        Value::String(topic.to_string()),
        Value::String(event.to_string()),
        payload,
    ])
}

/// Returns `(topic, event, payload, ref)` if the message is a valid Phoenix v2 frame.
fn parse_phoenix_message(msg: &str) -> Option<(String, String, Value, Option<String>)> {
    let arr: Vec<Value> = serde_json::from_str(msg).ok()?;
    if arr.len() != 5 {
        return None;
    }
    let ref_field = arr[1].as_str().map(String::from);
    let topic = arr[2].as_str()?.to_string();
    let event = arr[3].as_str()?.to_string();
    let payload = arr[4].clone();
    Some((topic, event, payload, ref_field))
}

fn parse_postgres_changes(payload: &Value) -> Option<RealtimeChange> {
    let data = payload.get("data")?;
    let table = data.get("table")?.as_str()?.to_string();
    let type_str = data.get("type")?.as_str()?;
    let op = match type_str {
        "INSERT" => RealtimeOp::Insert,
        "UPDATE" => RealtimeOp::Update,
        "DELETE" => RealtimeOp::Delete,
        _ => return None,
    };
    let record = data.get("record").and_then(Value::as_object).cloned();
    let old_record = data.get("old_record").and_then(Value::as_object).cloned();
    Some(RealtimeChange { table, op, record, old_record })
}

// MARK: - Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_swaps_https_to_wss() {
        let transport = SupabaseRealtimeTransport::new(
            "https://example.supabase.co".to_string(),
            "anon-key".to_string(),
            crate::auth::SessionStore::standalone(
                crate::Client::new(crate::ClientConfig {
                    supabase_url: "https://example.supabase.co".into(),
                    supabase_anon_key: "anon-key".into(),
                    encryption_secret: None,
                    storage_bucket: None,
                })
                .unwrap(),
            ),
        );
        let url = transport.websocket_url();
        assert!(url.starts_with("wss://example.supabase.co/realtime/v1/websocket"));
        assert!(url.contains("apikey=anon-key"));
        assert!(url.contains("vsn=2.0.0"));
    }

    #[test]
    fn websocket_url_swaps_http_to_ws() {
        let transport = SupabaseRealtimeTransport::new(
            "http://localhost:54321".to_string(),
            "k".to_string(),
            crate::auth::SessionStore::standalone(
                crate::Client::new(crate::ClientConfig {
                    supabase_url: "http://localhost:54321".into(),
                    supabase_anon_key: "k".into(),
                    encryption_secret: None,
                    storage_bucket: None,
                })
                .unwrap(),
            ),
        );
        assert!(
            transport.websocket_url().starts_with("ws://localhost:54321/realtime/v1/websocket")
        );
    }

    #[test]
    fn parse_postgres_changes_decodes_insert() {
        let payload = serde_json::json!({
            "data": {
                "table": "messages",
                "type": "INSERT",
                "record": { "id": "abc", "content": "hi" }
            }
        });
        let change = parse_postgres_changes(&payload).expect("parse");
        assert_eq!(change.table, "messages");
        assert_eq!(change.op, RealtimeOp::Insert);
        assert_eq!(change.record.as_ref().unwrap().get("id").unwrap().as_str(), Some("abc"));
        assert!(change.old_record.is_none());
    }

    #[test]
    fn parse_postgres_changes_decodes_update_with_old() {
        let payload = serde_json::json!({
            "data": {
                "table": "personas",
                "type": "UPDATE",
                "record": { "id": "p1", "name": "new" },
                "old_record": { "id": "p1", "name": "old" }
            }
        });
        let change = parse_postgres_changes(&payload).expect("parse");
        assert_eq!(change.op, RealtimeOp::Update);
        assert_eq!(change.old_record.as_ref().unwrap().get("name").unwrap().as_str(), Some("old"));
    }

    #[test]
    fn parse_postgres_changes_decodes_delete() {
        let payload = serde_json::json!({
            "data": {
                "table": "conversations",
                "type": "DELETE",
                "old_record": { "id": "c1" }
            }
        });
        let change = parse_postgres_changes(&payload).expect("parse");
        assert_eq!(change.op, RealtimeOp::Delete);
        assert!(change.record.is_none());
        assert_eq!(change.entity_id().as_deref(), Some("c1"));
    }

    #[test]
    fn parse_postgres_changes_rejects_unknown_op() {
        let payload = serde_json::json!({
            "data": { "table": "t", "type": "TRUNCATE" }
        });
        assert!(parse_postgres_changes(&payload).is_none());
    }

    #[test]
    fn parse_phoenix_message_extracts_ref() {
        let frame = build_phoenix_array(
            Some("1"),
            "abc",
            "realtime:foo",
            "phx_reply",
            serde_json::json!({ "status": "ok" }),
        );
        let (topic, event, payload, ref_id) =
            parse_phoenix_message(&frame.to_string()).expect("parse");
        assert_eq!(topic, "realtime:foo");
        assert_eq!(event, "phx_reply");
        assert_eq!(payload.get("status").unwrap().as_str(), Some("ok"));
        assert_eq!(ref_id.as_deref(), Some("abc"));
    }

    #[test]
    fn parse_phoenix_message_rejects_wrong_arity() {
        assert!(parse_phoenix_message(r#"["only", "two"]"#).is_none());
    }

    #[test]
    fn backoff_first_attempt_uses_initial_delay() {
        let config = TransportConfig {
            backoff_jitter: 0.0,
            initial_reconnect_delay: Duration::from_millis(123),
            ..Default::default()
        };
        assert_eq!(compute_backoff_delay(0, &config), Duration::from_millis(123));
    }

    #[test]
    fn backoff_walks_ladder_then_clamps_to_last() {
        let config = TransportConfig {
            backoff_jitter: 0.0,
            backoff_ladder: vec![
                Duration::from_millis(100),
                Duration::from_millis(200),
                Duration::from_millis(400),
            ],
            ..Default::default()
        };
        assert_eq!(compute_backoff_delay(1, &config), Duration::from_millis(100));
        assert_eq!(compute_backoff_delay(2, &config), Duration::from_millis(200));
        assert_eq!(compute_backoff_delay(3, &config), Duration::from_millis(400));
        assert_eq!(compute_backoff_delay(99, &config), Duration::from_millis(400));
    }

    #[test]
    fn jitter_zero_is_pass_through() {
        let base = Duration::from_secs(1);
        assert_eq!(apply_jitter(base, 0.0), base);
    }

    #[test]
    fn jitter_stays_within_band() {
        let base = Duration::from_secs(1);
        for _ in 0..1000 {
            let result = apply_jitter(base, 0.20);
            let ms = u64::try_from(result.as_millis()).expect("ms fits in u64");
            assert!((800..=1200).contains(&ms), "jitter result {ms}ms outside ±20% of 1000ms");
        }
    }

    #[test]
    fn jitter_clamps_excess_jitter() {
        // 5.0 should be clamped to 1.0; result should be in [0, 2*base].
        let base = Duration::from_secs(1);
        for _ in 0..100 {
            let result = apply_jitter(base, 5.0);
            assert!(result.as_secs_f64() >= 0.0 && result.as_secs_f64() <= 2.0);
        }
    }

    #[test]
    fn disconnect_reason_labels_are_stable() {
        assert_eq!(DisconnectReason::ChannelClosed.as_str(), "channel_closed");
        assert_eq!(DisconnectReason::WebsocketError.as_str(), "websocket_error");
        assert_eq!(DisconnectReason::HeartbeatTimeout.as_str(), "heartbeat_timeout");
        assert_eq!(DisconnectReason::LivenessTimeout.as_str(), "liveness_timeout");
        assert_eq!(DisconnectReason::Manual.as_str(), "manual");
    }
}
