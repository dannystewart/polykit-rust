//! Per-user database file management.
//!
//! Mirrors the Tauri Prism convention so an existing app can drop in this implementation
//! without re-pathing user data:
//!
//! - Each authenticated user owns one SQLite file at `{root}/{user_id}/sync.db`.
//! - Switching users wipes the previous user's directory before opening the new one.
//! - WAL journal, normal sync, foreign keys on, 10 s busy timeout.
//!
//! Migrations are opt-in via [`DbManager::with_polybase_migrator`] (ships the built-in
//! `kvs` table) and [`DbManager::with_app_migrator`] (host app's own schema). Both run on
//! every `switch_user` after the pool opens; sqlx tracks applied migrations so calling
//! the same migrator multiple times is idempotent.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tokio::sync::RwLock;

/// Errors from the manager.
#[derive(Debug, thiserror::Error)]
pub enum DbManagerError {
    /// Underlying `sqlx` failure (open, query, migration, etc.).
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Migration failure during `switch_user`.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// Filesystem failure (creating the root directory, removing prior user dir).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// User id contained characters outside the allowed alphabet.
    #[error("invalid user id: {0}")]
    InvalidUserId(String),
}

/// Per-user pool manager.
#[derive(Clone)]
pub struct DbManager {
    config: DbManagerConfig,
    state: Arc<RwLock<DbManagerState>>,
}

#[derive(Clone)]
struct DbManagerConfig {
    root: PathBuf,
    polybase_migrator: Option<&'static Migrator>,
    app_migrator: Option<&'static Migrator>,
    max_connections: u32,
}

#[derive(Default)]
struct DbManagerState {
    user_id: Option<String>,
    pool: Option<SqlitePool>,
}

impl DbManager {
    /// Create a new manager rooted at `root`. Per-user files live under `{root}/{user_id}/sync.db`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            config: DbManagerConfig {
                root: root.into(),
                polybase_migrator: None,
                app_migrator: None,
                max_connections: 4,
            },
            state: Arc::new(RwLock::new(DbManagerState::default())),
        }
    }

    /// Run the built-in polybase migrator (currently: the `kvs` table) on every `switch_user`.
    /// New apps that don't ship their own KVS table should turn this on. Apps that already
    /// declare `kvs` in their own migrator can leave it off.
    pub fn with_polybase_migrator(mut self) -> Self {
        self.config.polybase_migrator = Some(&crate::MIGRATOR);
        self
    }

    /// Register an app-supplied [`Migrator`] to run on every `switch_user`. The polybase
    /// migrator (if enabled) runs first, then the app migrator. Both are idempotent.
    pub fn with_app_migrator(mut self, migrator: &'static Migrator) -> Self {
        self.config.app_migrator = Some(migrator);
        self
    }

    /// Customize the connection pool's max connection count (default: 4, matching
    /// Tauri Prism's setting).
    pub fn with_max_connections(mut self, max: u32) -> Self {
        self.config.max_connections = max;
        self
    }

    /// Root directory under which per-user database files live.
    pub fn root(&self) -> &Path {
        &self.config.root
    }

    /// Active user's pool, if a user is signed in.
    pub async fn pool(&self) -> Option<SqlitePool> {
        self.state.read().await.pool.clone()
    }

    /// Currently active user id, if any.
    pub async fn current_user(&self) -> Option<String> {
        self.state.read().await.user_id.clone()
    }

    /// Switch to a new user. Empty `user_id` means sign-out — wipes any per-user state.
    ///
    /// Per the v1 contract, the previous user's database directory is deleted before
    /// activating the next one. Apps that want to preserve old user data should snapshot
    /// it before calling.
    pub async fn switch_user(&self, user_id: &str) -> Result<(), DbManagerError> {
        let mut guard = self.state.write().await;

        if guard.user_id.as_deref() == Some(user_id) {
            return Ok(());
        }

        if let Some(pool) = guard.pool.take() {
            pool.close().await;
        }
        if let Some(prior) = guard.user_id.take() {
            let prior_dir = self.user_db_dir(&prior);
            if prior_dir.exists() {
                tokio::fs::remove_dir_all(&prior_dir).await?;
            }
        }

        if user_id.is_empty() {
            guard.user_id = None;
            guard.pool = None;
            return Ok(());
        }

        if !user_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(DbManagerError::InvalidUserId(user_id.into()));
        }

        let user_dir = self.user_db_dir(user_id);
        tokio::fs::create_dir_all(&user_dir).await?;
        let path = user_dir.join("sync.db");
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(self.config.max_connections)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(opts)
            .await?;

        if let Some(migrator) = self.config.polybase_migrator {
            migrator.run(&pool).await?;
        }
        if let Some(migrator) = self.config.app_migrator {
            migrator.run(&pool).await?;
        }

        guard.user_id = Some(user_id.into());
        guard.pool = Some(pool);
        Ok(())
    }

    fn user_db_dir(&self, user_id: &str) -> PathBuf {
        self.config.root.join(user_id)
    }
}
