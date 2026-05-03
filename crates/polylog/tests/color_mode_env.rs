//! Verify ColorMode::Auto honors NO_COLOR / FORCE_COLOR env vars.
#![allow(unsafe_code)] // env::set_var is unsafe in Rust 2024; tests deliberately mutate env.

use polylog::ColorMode;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvSnapshot {
    no_color: Option<String>,
    force_color: Option<String>,
}

impl EnvSnapshot {
    fn capture() -> Self {
        Self {
            no_color: std::env::var("NO_COLOR").ok(),
            force_color: std::env::var("FORCE_COLOR").ok(),
        }
    }

    fn restore(self) {
        match self.no_color {
            Some(value) => unsafe { std::env::set_var("NO_COLOR", value) },
            None => unsafe { std::env::remove_var("NO_COLOR") },
        }
        match self.force_color {
            Some(value) => unsafe { std::env::set_var("FORCE_COLOR", value) },
            None => unsafe { std::env::remove_var("FORCE_COLOR") },
        }
    }
}

#[test]
fn auto_respects_no_color_env_var() {
    let _guard = env_lock().lock().unwrap();
    let snapshot = EnvSnapshot::capture();
    unsafe { std::env::set_var("NO_COLOR", "1") };
    unsafe { std::env::remove_var("FORCE_COLOR") };
    assert!(!ColorMode::Auto.should_emit_ansi(true));
    snapshot.restore();
}

#[test]
fn auto_respects_force_color_env_var() {
    let _guard = env_lock().lock().unwrap();
    let snapshot = EnvSnapshot::capture();
    unsafe { std::env::set_var("FORCE_COLOR", "1") };
    unsafe { std::env::remove_var("NO_COLOR") };
    assert!(ColorMode::Auto.should_emit_ansi(false));
    snapshot.restore();
}

#[test]
fn auto_falls_back_to_tty_detection() {
    let _guard = env_lock().lock().unwrap();
    let snapshot = EnvSnapshot::capture();
    unsafe { std::env::remove_var("NO_COLOR") };
    unsafe { std::env::remove_var("FORCE_COLOR") };
    assert!(ColorMode::Auto.should_emit_ansi(true));
    assert!(!ColorMode::Auto.should_emit_ansi(false));
    snapshot.restore();
}
