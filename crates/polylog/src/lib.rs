//! Branded logger built on `tracing`.
//!
//! Sibling to [`polykit`](https://github.com/dannystewart/polykit) (Python) and
//! [`polykit-swift`](https://github.com/dannystewart/polykit-swift) (Swift). Lives in the
//! [`polykit-rust`](https://github.com/dannystewart/polykit-rust) workspace alongside
//! [`polybase`](../polybase/index.html).
//!
//! # Quickstart
//! ```no_run
//! use polylog::{FormatMode, Level};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let _guard = polylog::init()
//!         .level(Level::Info)
//!         .format(FormatMode::Context)
//!         .install()?;
//!
//!     polylog::info!("hello from polylog");
//!     Ok(())
//! }
//! ```
//!
//! The returned guard must remain in scope for the program lifetime;
//! dropping it flushes any pending file output.
//!
//! # Levels
//! Four levels: [`Level::Debug`], [`Level::Info`], [`Level::Warn`], [`Level::Error`].
//! The `tracing` TRACE level is dropped (not rendered). There is no CRITICAL level.

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

// Re-export tracing macros for convenience: polylog::info!, polylog::warn!, etc.
pub use tracing::{debug, error, event, info, instrument, span, trace, warn};

// Re-export tracing itself for full access (spans, fields, custom subscribers, etc.).
pub use tracing;
