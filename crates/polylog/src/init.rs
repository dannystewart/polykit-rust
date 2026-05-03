use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::builder::{LogBuilder, LogConfig};
use crate::console::ConsoleLayer;
use crate::error::{InitError, InitGuard};
use crate::file::build_file_layer;
use crate::level::Level;

/// Process-global flag: has the logger been initialized?
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Dynamic minimum log level (u8 encoding: Debug=0, Info=1, Warn=2, Error=3).
static MIN_LEVEL: AtomicU8 = AtomicU8::new(1); // default Info

/// Obtain a [`LogBuilder`] to configure and install the polylog logger.
pub(crate) fn init() -> LogBuilder {
    LogBuilder::new()
}

pub(crate) fn install_with_config(builder: LogBuilder) -> Result<InitGuard, InitError> {
    // Idempotency: only one init() per process.
    let exchanged = INITIALIZED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
    if exchanged.is_err() {
        return Err(InitError::AlreadyInitialized);
    }

    let config = LogConfig::from(builder);

    // Set dynamic min level before building layers.
    MIN_LEVEL.store(config.level.as_u8(), Ordering::Release);

    // Resolve timezone once and pass clones to both layers.
    let tz = resolve_tz();

    let console_layer = ConsoleLayer::new_with_tz(&config, tz.clone());

    let (file_layer_opt, worker_guard) = if config.log_file.is_some() {
        match build_file_layer(&config, tz) {
            Ok((layer, guard)) => (Some(layer), Some(guard)),
            Err(e) => {
                INITIALIZED.store(false, Ordering::Release);
                return Err(e);
            }
        }
    } else {
        (None, None)
    };

    // Compose and try to install the global subscriber.
    let subscriber = tracing_subscriber::registry().with(console_layer).with(file_layer_opt);

    if let Err(e) = subscriber.try_init() {
        INITIALIZED.store(false, Ordering::Release);
        return Err(InitError::SetGlobalDefaultFailed(e));
    }

    // Bridge the `log` crate → tracing. Non-fatal if it fails.
    if let Err(e) = tracing_log::LogTracer::init() {
        eprintln!("polylog: warning: failed to bridge log crate: {e}");
    }

    Ok(InitGuard::with_worker_opt(worker_guard))
}

fn resolve_tz() -> jiff::tz::TimeZone {
    match jiff::tz::TimeZone::try_system() {
        Ok(tz) => tz,
        Err(_) => {
            eprintln!(
                "polylog: could not detect system timezone; falling back to America/New_York"
            );
            jiff::tz::TimeZone::get("America/New_York").unwrap_or_else(|_| {
                eprintln!("polylog: failed to load America/New_York timezone; falling back to UTC");
                jiff::tz::TimeZone::UTC
            })
        }
    }
}

pub(crate) fn current_min_level() -> Level {
    Level::from_u8(MIN_LEVEL.load(Ordering::Acquire)).unwrap_or(Level::Info)
}

pub(crate) fn set_min_level(level: Level) {
    MIN_LEVEL.store(level.as_u8(), Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_atomics_default_to_info() {
        assert_eq!(MIN_LEVEL.load(Ordering::Relaxed), Level::Info.as_u8());
        assert!(!INITIALIZED.load(Ordering::Relaxed));
    }

    #[test]
    fn set_and_get_min_level_round_trips() {
        for level in [Level::Debug, Level::Info, Level::Warn, Level::Error] {
            MIN_LEVEL.store(level.as_u8(), Ordering::Relaxed);
            let retrieved = current_min_level();
            assert_eq!(retrieved, level);
        }
        // Reset to default so other tests see the expected value.
        MIN_LEVEL.store(Level::Info.as_u8(), Ordering::Relaxed);
    }
}
