//! Session management with refresh loop.
//!
//! See module docs for the high-level pattern: TypeScript owns sign-in (Supabase PKCE), hands the
//! resulting session into Rust via [`SessionStore::set_session`], and Rust then drives auto-refresh
//! plus broadcasts [`crate::events::PolyEvent::SessionChanged`] so subscribers (sync coordinator,
//! realtime, frontend) react.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex as SyncMutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::client::Client;
use crate::errors::PolyError;
use crate::events::{EventBus, PolyEvent, SessionChangeKind};

/// Concrete callback type for [`SessionStore::on_user_changed`].
type UserChangeHook = Arc<dyn Fn(Option<&str>) + Send + Sync + 'static>;

/// Cooldown between refresh attempts so we never thrash on a server that keeps 401-ing.
pub const REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

/// Full session payload handed off from the auth UX (TypeScript / supabase-js) to polybase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPayload {
    /// Supabase user id (UUID string).
    pub user_id: String,
    /// JWT access token used as `Authorization: Bearer ...`.
    pub access_token: String,
    /// Long-lived refresh token used to mint new access tokens.
    pub refresh_token: String,
    /// Unix-seconds at which `access_token` expires.
    pub expires_at: i64,
    /// Set by polybase on every store; lets sync code detect stale sessions without parsing JWTs.
    #[serde(default)]
    pub updated_at: u64,
}

/// Result of an `ensure_fresh` call.
#[derive(Debug, Clone)]
pub struct EnsureFreshResult {
    /// The session known to polybase after the check (refreshed or unchanged).
    pub session: SessionPayload,
    /// True when polybase actually performed a refresh round-trip; false when the existing
    /// session was already fresh enough.
    pub refreshed: bool,
}

/// Session-changed mutation kind reported back from `set_session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMutationKind {
    /// Payload matched the existing session; nothing changed.
    Noop,
    /// Same user, fresh access/refresh tokens.
    CredentialsRefreshed,
    /// Different user signed in.
    UserChanged,
    /// Session removed (sign-out).
    Cleared,
}

/// What `set_session` / `clear_session` did, plus the assigned `updated_at` watermark.
///
/// Callers capture `updated_at` and later check [`SessionStore::is_session_valid`] to
/// detect that the session has been replaced or cleared while async work was in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionMutation {
    /// The kind of mutation that was applied.
    pub kind: SessionMutationKind,
    /// Monotonic `updated_at` watermark of the active session after the mutation.
    /// Zero when there is no active session.
    pub updated_at: u64,
}

impl SessionMutationKind {
    fn into_event_kind(self) -> Option<SessionChangeKind> {
        match self {
            Self::Noop => None,
            Self::CredentialsRefreshed => Some(SessionChangeKind::CredentialsRefreshed),
            Self::UserChanged => Some(SessionChangeKind::UserChanged),
            Self::Cleared => Some(SessionChangeKind::Cleared),
        }
    }
}

/// In-memory session store with refresh coordination. Cheap to clone via `Arc`.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<SessionStoreInner>,
}

struct SessionStoreInner {
    client: Client,
    events: EventBus,
    session: RwLock<Option<SessionPayload>>,
    mutation_lock: Mutex<()>,
    refresh_state: Mutex<RefreshState>,
    update_counter: AtomicU64,
    user_change_hook: SyncMutex<Option<UserChangeHook>>,
}

struct RefreshState {
    last_attempt_at: Option<Instant>,
}

impl SessionStore {
    /// Build a new session store wired to a [`Client`] and an external [`EventBus`].
    pub fn new(client: Client, events: EventBus) -> Self {
        Self {
            inner: Arc::new(SessionStoreInner {
                client,
                events,
                session: RwLock::new(None),
                mutation_lock: Mutex::new(()),
                refresh_state: Mutex::new(RefreshState { last_attempt_at: None }),
                update_counter: AtomicU64::new(1),
                user_change_hook: SyncMutex::new(None),
            }),
        }
    }

    /// Convenience: build a self-contained store without an external `EventBus` (events drop).
    pub fn standalone(client: Client) -> Self {
        Self::new(client, EventBus::new())
    }

    /// Register a callback that fires whenever the active user changes (sign-in / account
    /// switch) or the session is cleared (sign-out). The callback receives the new user id
    /// (`None` for clear) and runs synchronously inside the session mutation. It must NOT
    /// call back into `SessionStore` (no recursive `set_session` / `clear_session`).
    ///
    /// This is the recommended way to wire [`crate::persistence::LocalStore::switch_user`]:
    /// the host app provides a closure that calls `local_store.switch_user(new_user_id)` so
    /// polybase invokes it deterministically on every session boundary, ensuring the local
    /// mirror never lags behind the active user.
    ///
    /// Replaces any previously-registered hook.
    pub fn on_user_changed<F>(&self, callback: F)
    where
        F: Fn(Option<&str>) + Send + Sync + 'static,
    {
        *self.inner.user_change_hook.lock() = Some(Arc::new(callback));
    }

    /// Set / update the session payload. Returns the mutation kind plus the assigned
    /// `updated_at` so callers can later check [`SessionStore::is_session_valid`].
    pub async fn set_session(
        &self,
        mut payload: SessionPayload,
    ) -> Result<SessionMutation, PolyError> {
        let _guard = self.inner.mutation_lock.lock().await;

        let current = self.current().await;
        if let Some(current) = current.as_ref()
            && session_matches(current, &payload)
        {
            return Ok(SessionMutation {
                kind: SessionMutationKind::Noop,
                updated_at: current.updated_at,
            });
        }

        let kind = match current.as_ref() {
            Some(current) if current.user_id == payload.user_id => {
                SessionMutationKind::CredentialsRefreshed
            }
            _ => SessionMutationKind::UserChanged,
        };

        payload.updated_at = self.next_updated_at();
        let assigned_updated_at = payload.updated_at;
        let user_id = payload.user_id.clone();

        {
            let mut guard = self.inner.session.write().await;
            *guard = Some(payload);
        }

        if matches!(kind, SessionMutationKind::UserChanged) {
            self.fire_user_change_hook(Some(&user_id));
        }

        if let Some(event_kind) = kind.into_event_kind() {
            self.inner
                .events
                .publish(PolyEvent::SessionChanged { user_id: Some(user_id), change: event_kind });
        }

        Ok(SessionMutation { kind, updated_at: assigned_updated_at })
    }

    /// Clear the session (sign-out).
    pub async fn clear_session(&self) -> Result<SessionMutation, PolyError> {
        let _guard = self.inner.mutation_lock.lock().await;
        let had_session = self.current().await.is_some();

        {
            let mut guard = self.inner.session.write().await;
            *guard = None;
        }
        self.inner.refresh_state.lock().await.last_attempt_at = None;

        let kind =
            if had_session { SessionMutationKind::Cleared } else { SessionMutationKind::Noop };
        if matches!(kind, SessionMutationKind::Cleared) {
            self.fire_user_change_hook(None);
            self.inner.events.publish(PolyEvent::SessionChanged {
                user_id: None,
                change: SessionChangeKind::Cleared,
            });
        }
        Ok(SessionMutation { kind, updated_at: 0 })
    }

    /// Snapshot of the current session.
    pub async fn current(&self) -> Option<SessionPayload> {
        self.inner.session.read().await.clone()
    }

    /// Convenience: just the active user id.
    pub async fn current_user_id(&self) -> Option<String> {
        self.inner.session.read().await.as_ref().map(|s| s.user_id.clone())
    }

    /// Monotonic `updated_at` of the current session, or `0` if none is stored.
    ///
    /// Pair with [`SessionStore::is_session_valid`] to gate long-running async work on the
    /// session that was active when the work started.
    pub async fn current_updated_at(&self) -> u64 {
        self.inner.session.read().await.as_ref().map(|s| s.updated_at).unwrap_or(0)
    }

    /// True when a session is still present AND its `updated_at` is at least `min_updated_at`.
    ///
    /// The sync runtime, replay loop, and any background work captures the active
    /// `updated_at` at start time and re-checks via this function before publishing results,
    /// so a sign-out / account switch in the middle of a long task cleanly cancels the work
    /// instead of leaking writes onto the new user.
    pub async fn is_session_valid(&self, min_updated_at: u64) -> bool {
        self.inner
            .session
            .read()
            .await
            .as_ref()
            .map(|s| s.updated_at >= min_updated_at)
            .unwrap_or(false)
    }

    /// Has there been a session set, and is it not yet expired?
    pub async fn has_unexpired_session(&self) -> bool {
        self.current()
            .await
            .map(|session| !session_expires_within(&session, Duration::ZERO))
            .unwrap_or(false)
    }

    /// Refresh if `min_valid_for` from now would be past `expires_at`. No-op otherwise.
    pub async fn ensure_fresh(
        &self,
        min_valid_for: Duration,
    ) -> Result<Option<EnsureFreshResult>, PolyError> {
        self.refresh_if_needed(min_valid_for, false).await
    }

    /// Force-refresh even if the current token is still valid.
    pub async fn force_refresh(&self) -> Result<Option<EnsureFreshResult>, PolyError> {
        self.refresh_if_needed(Duration::ZERO, true).await
    }

    async fn refresh_if_needed(
        &self,
        min_valid_for: Duration,
        force: bool,
    ) -> Result<Option<EnsureFreshResult>, PolyError> {
        let Some(session) = self.current().await else {
            return Ok(None);
        };
        if !force && !session_expires_within(&session, min_valid_for) {
            return Ok(Some(EnsureFreshResult { session, refreshed: false }));
        }

        let mut refresh_state = self.inner.refresh_state.lock().await;
        let current = self.current().await.ok_or(PolyError::NoSession)?;
        if !force && !session_expires_within(&current, min_valid_for) {
            return Ok(Some(EnsureFreshResult { session: current, refreshed: false }));
        }

        if let Some(last_attempt_at) = refresh_state.last_attempt_at {
            let elapsed = last_attempt_at.elapsed();
            if elapsed < REFRESH_COOLDOWN {
                let remaining_ms = (REFRESH_COOLDOWN - elapsed).as_millis();
                return Err(PolyError::other(format!(
                    "session refresh is cooling down; retry in {remaining_ms}ms"
                )));
            }
        }

        refresh_state.last_attempt_at = Some(Instant::now());
        drop(refresh_state);

        let refreshed_payload = self.refresh_payload(&current).await?;
        let mutation = self.set_session(refreshed_payload.clone()).await?;
        self.inner.refresh_state.lock().await.last_attempt_at = None;

        Ok(Some(EnsureFreshResult {
            session: self.current().await.unwrap_or(refreshed_payload),
            refreshed: matches!(mutation.kind, SessionMutationKind::CredentialsRefreshed),
        }))
    }

    async fn refresh_payload(&self, session: &SessionPayload) -> Result<SessionPayload, PolyError> {
        let url = format!("{}?grant_type=refresh_token", self.inner.client.auth_url("token"));
        let resp = self
            .inner
            .client
            .http()
            .post(url)
            .header("apikey", &self.inner.client.config().supabase_anon_key)
            .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let message = if body.trim().is_empty() {
                format!("HTTP {}", status.as_u16())
            } else {
                format!("HTTP {}: {}", status.as_u16(), body.trim())
            };
            return Err(PolyError::other(format!("session refresh failed: {message}")));
        }

        let payload: RefreshSessionResponse = resp.json().await?;
        let expires_at = payload
            .expires_at
            .or_else(|| {
                payload.expires_in.map(|expires_in| unix_time_secs().saturating_add(expires_in))
            })
            .unwrap_or_else(|| unix_time_secs().saturating_add(3600));

        Ok(SessionPayload {
            user_id: payload
                .user
                .map(|user| user.id)
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| session.user_id.clone()),
            access_token: payload.access_token,
            refresh_token: payload.refresh_token.unwrap_or_else(|| session.refresh_token.clone()),
            expires_at,
            updated_at: 0,
        })
    }

    fn next_updated_at(&self) -> u64 {
        self.inner.update_counter.fetch_add(1, Ordering::SeqCst)
    }

    fn fire_user_change_hook(&self, new_user: Option<&str>) {
        let hook = self.inner.user_change_hook.lock().clone();
        if let Some(hook) = hook {
            hook(new_user);
        }
    }
}

#[derive(Debug, Deserialize)]
struct RefreshSessionResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    user: Option<RefreshResponseUser>,
}

#[derive(Debug, Deserialize)]
struct RefreshResponseUser {
    id: String,
}

fn session_matches(left: &SessionPayload, right: &SessionPayload) -> bool {
    left.user_id == right.user_id
        && left.access_token == right.access_token
        && left.refresh_token == right.refresh_token
        && left.expires_at == right.expires_at
}

fn session_expires_within(session: &SessionPayload, min_valid_for: Duration) -> bool {
    let now = unix_time_secs();
    let threshold = now.saturating_add(min_valid_for.as_secs() as i64);
    session.expires_at <= threshold
}

fn unix_time_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use super::*;
    use crate::client::ClientConfig;

    fn make_store() -> SessionStore {
        let client = Client::new(ClientConfig {
            supabase_url: "https://example.supabase.co".into(),
            supabase_anon_key: "anon".into(),
            encryption_secret: None,
            storage_bucket: None,
        })
        .unwrap();
        SessionStore::standalone(client)
    }

    fn payload(user: &str, refresh: &str) -> SessionPayload {
        SessionPayload {
            user_id: user.into(),
            access_token: "a".into(),
            refresh_token: refresh.into(),
            expires_at: unix_time_secs() + 3600,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn set_session_records_user_changed_then_noop() {
        let store = make_store();
        let payload = payload("u1", "r");

        let mutation = store.set_session(payload.clone()).await.unwrap();
        assert_eq!(mutation.kind, SessionMutationKind::UserChanged);
        assert!(mutation.updated_at > 0);
        assert_eq!(store.current_updated_at().await, mutation.updated_at);

        let again = store.set_session(payload).await.unwrap();
        assert_eq!(again.kind, SessionMutationKind::Noop);
        assert_eq!(again.updated_at, mutation.updated_at);
    }

    #[tokio::test]
    async fn updated_at_advances_on_credentials_refresh() {
        let store = make_store();
        let first = store.set_session(payload("u1", "r")).await.unwrap();
        let second = store.set_session(payload("u1", "r2")).await.unwrap();

        assert_eq!(second.kind, SessionMutationKind::CredentialsRefreshed);
        assert!(second.updated_at > first.updated_at);
    }

    #[tokio::test]
    async fn is_session_valid_tracks_replacement_and_clear() {
        let store = make_store();
        let first = store.set_session(payload("u1", "r")).await.unwrap();
        assert!(store.is_session_valid(first.updated_at).await);

        let second = store.set_session(payload("u2", "r")).await.unwrap();
        assert!(store.is_session_valid(second.updated_at).await);
        // The first watermark is still <= second.updated_at, so it should still be valid.
        assert!(store.is_session_valid(first.updated_at).await);

        store.clear_session().await.unwrap();
        assert!(!store.is_session_valid(first.updated_at).await);
        assert!(!store.is_session_valid(second.updated_at).await);
        assert_eq!(store.current_updated_at().await, 0);
    }

    #[tokio::test]
    async fn clear_session_after_set_emits_cleared() {
        let store = make_store();
        store.set_session(payload("u1", "r")).await.unwrap();

        let mutation = store.clear_session().await.unwrap();
        assert_eq!(mutation.kind, SessionMutationKind::Cleared);
        assert!(store.current().await.is_none());
    }

    #[tokio::test]
    async fn clear_session_when_empty_is_noop() {
        let store = make_store();
        let mutation = store.clear_session().await.unwrap();
        assert_eq!(mutation.kind, SessionMutationKind::Noop);
        assert_eq!(mutation.updated_at, 0);
    }

    #[tokio::test]
    async fn user_change_hook_fires_on_user_change_and_clear() {
        let store = make_store();
        let counter = Arc::new(AtomicU64::new(0));
        let last = Arc::new(parking_lot::Mutex::new(Option::<String>::None));

        let counter_clone = counter.clone();
        let last_clone = last.clone();
        store.on_user_changed(move |user| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            *last_clone.lock() = user.map(str::to_owned);
        });

        store.set_session(payload("u1", "r")).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(last.lock().as_deref(), Some("u1"));

        store.set_session(payload("u1", "r2")).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1, "credentials refresh must not fire hook");

        store.set_session(payload("u2", "r")).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert_eq!(last.lock().as_deref(), Some("u2"));

        store.clear_session().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        assert!(last.lock().is_none());

        store.clear_session().await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 3, "no-op clear must not fire hook");
    }
}
