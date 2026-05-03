//! Default [`polybase::persistence::LocalStore`] implementation backed by `sqlx` + SQLite.
//!
//! The schema mirrors Tauri Prism's `src-tauri/migrations/0001_initial_schema.sql`. Apps with
//! different table shapes can ship their own `LocalStore` instead.

mod manager;
mod store;

pub use manager::DbManager;
pub use store::SqliteLocalStore;
