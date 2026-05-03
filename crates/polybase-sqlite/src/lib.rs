//! Default [`polybase::persistence::LocalStore`] implementation backed by `sqlx` + SQLite.
//!
//! # Path layout
//!
//! Each authenticated user owns one SQLite file at `{root}/{user_id}/sync.db`. This matches
//! Tauri Prism's existing layout so the cutover can drop in without re-pathing user data.
//!
//! # Built-in schema
//!
//! The crate ships a single migration ([`MIGRATOR`]) that creates the `kvs` table required
//! by [`polybase::Kvs`]. Apps with their own pre-existing migrations can either:
//!
//! - layer the polybase migrator alongside their own via [`DbManager::with_polybase_migrator`]
//!   plus [`DbManager::with_app_migrator`], or
//! - leave the polybase migrator off and ship the `kvs` table inside their own migrations
//!   (recommended for the Tauri Prism cutover so there's a single source of truth).
//!
//! Apps with completely different table shapes can ship their own [`polybase::LocalStore`]
//! instead of using this crate at all.

mod manager;
mod store;

pub use manager::{DbManager, DbManagerError};
pub use store::SqliteLocalStore;

/// Embedded migrator that ships the polybase-owned schema (currently: the `kvs` table).
/// Wire it in via [`DbManager::with_polybase_migrator`].
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
