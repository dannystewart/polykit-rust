//! Branded logger built on `tracing`.
//!
//! # Quickstart
//! ```no_run
//! use polykit::log;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let _guard = log::init()
//!         .level(log::Level::Info)
//!         .format(log::FormatMode::Context)
//!         .install()?;
//!
//!     log::info!("hello from polykit");
//!     Ok(())
//! }
//! ```
//!
//! The returned guard must remain in scope for the program lifetime;
//! dropping it flushes any pending file output.
//!
//! # Levels
//! Four levels: [`Level::Debug`], [`Level::Info`], [`Level::Warn`],
//! [`Level::Error`]. The `tracing` TRACE level is dropped (not rendered).
//! There is no CRITICAL level.

mod builder;
mod catch;
mod console;
mod error;
mod file;
mod format;
pub(crate) mod init;
mod level;
mod level_override;

pub use builder::LogBuilder;
pub use catch::catch;
pub use error::{InitError, InitGuard};
pub use format::{ColorMode, FormatMode};
pub use level::Level;
pub use level_override::LogLevelOverride;

/// Build a [`LogBuilder`] with default values.
pub fn init() -> LogBuilder {
    init::init()
}

// Re-export tracing macros at polykit::log:: for convenience.
pub use tracing::{debug, error, event, info, instrument, span, trace, warn};

// Also re-export tracing itself for users who want full access.
pub use tracing;
