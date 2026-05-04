//! Default [`polybase::persistence::LocalStore`] implementation backed by `sqlx` + SQLite.
//!
//! # Path layout
//!
//! Each authenticated user owns one SQLite file at `{root}/{user_id}/sync.db`. This matches
//! Tauri Prism's existing layout so the cutover can drop in without re-pathing user data.
//!
//! # Built-in schema
//!
//! The crate ships an embedded migrator ([`MIGRATOR`]) that creates the polybase-owned
//! tables:
//!
//! - `kvs` — required by [`polybase::Kvs`].
//! - `polybase_queue` — required by [`SqliteOfflineQueue`] (the SQLite-backed
//!   [`polybase::offline_queue::OfflineQueue`] implementation).
//!
//! Apps with their own pre-existing migrations can either:
//!
//! - layer the polybase migrator alongside their own via [`DbManager::with_polybase_migrator`]
//!   plus [`DbManager::with_app_migrator`], or
//! - leave the polybase migrator off and ship the `kvs` and `polybase_queue` tables inside
//!   their own migrations.
//!
//! Apps with completely different table shapes can ship their own [`polybase::LocalStore`]
//! and/or [`polybase::offline_queue::OfflineQueue`] instead of using this crate at all.

mod manager;
mod queue;
mod store;

pub use manager::{DbManager, DbManagerError};
pub use queue::SqliteOfflineQueue;
pub use store::SqliteLocalStore;

/// Embedded migrator that ships the polybase-owned schema (`kvs` + `polybase_queue`).
/// Wire it in via [`DbManager::with_polybase_migrator`].
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
