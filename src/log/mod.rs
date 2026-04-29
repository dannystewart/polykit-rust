//! Branded logging built on `tracing`.
//!
//! See [`init`] and the module-level documentation for usage.

mod builder;
mod catch;
mod console;
mod error;
mod file;
mod format;
mod init;
mod level;
mod level_override;

pub use level::Level;
pub use format::{ColorMode, FormatMode};
pub use error::{InitError, InitGuard};
