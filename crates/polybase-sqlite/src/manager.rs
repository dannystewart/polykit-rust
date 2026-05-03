//! Per-user database file management.
//!
//! Mirrors the Tauri Prism convention: each authenticated user owns exactly one SQLite file at
//! `{root}/{user_id}.db`. Switching users wipes the previous user's database file before opening
//! the new one — that wipe is the v1 contract requirement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::RwLock;

/// Errors from the manager.
#[derive(Debug, thiserror::Error)]
pub enum DbManagerError {
    /// Underlying `sqlx` failure (open, query, migration, etc.).
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Filesystem failure (creating the root directory, removing prior user file).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// User id contained characters outside the allowed alphabet.
    #[error("invalid user id: {0}")]
    InvalidUserId(String),
}

/// Per-user pool manager.
#[derive(Clone)]
pub struct DbManager {
    inner: Arc<DbManagerInner>,
}

struct DbManagerInner {
    root: PathBuf,
    state: RwLock<DbManagerState>,
}

#[derive(Default)]
struct DbManagerState {
    user_id: Option<String>,
    pool: Option<SqlitePool>,
}

impl DbManager {
    /// Create a new manager rooted at `root`. Per-user files live as `{root}/{user_id}.db`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(DbManagerInner {
                root: root.into(),
                state: RwLock::new(DbManagerState::default()),
            }),
        }
    }

    /// Root directory under which per-user database files live.
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Active user's pool, if a user is signed in.
    pub async fn pool(&self) -> Option<SqlitePool> {
        self.inner.state.read().await.pool.clone()
    }

    /// Switch to a new user. Empty `user_id` means sign-out — wipes any per-user state.
    ///
    /// Per the v1 contract, the previous user's database file is deleted before activating the
    /// next one. Apps that want to preserve old user data should snapshot it before calling.
    pub async fn switch_user(&self, user_id: &str) -> Result<(), DbManagerError> {
        let mut guard = self.inner.state.write().await;

        if guard.user_id.as_deref() == Some(user_id) {
            return Ok(());
        }

        if let Some(prior) = guard.user_id.as_deref() {
            let prior_path = self.user_db_path(prior);
            if let Some(pool) = guard.pool.take() {
                pool.close().await;
            }
            if prior_path.exists() {
                tokio::fs::remove_file(&prior_path).await?;
            }
        } else if let Some(pool) = guard.pool.take() {
            pool.close().await;
        }

        if user_id.is_empty() {
            guard.user_id = None;
            guard.pool = None;
            return Ok(());
        }

        if !user_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(DbManagerError::InvalidUserId(user_id.into()));
        }

        tokio::fs::create_dir_all(&self.inner.root).await?;
        let path = self.user_db_path(user_id);
        let opts = SqliteConnectOptions::new().filename(&path).create_if_missing(true);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        guard.user_id = Some(user_id.into());
        guard.pool = Some(pool);
        Ok(())
    }

    fn user_db_path(&self, user_id: &str) -> PathBuf {
        self.inner.root.join(format!("{user_id}.db"))
    }
}
