//! Test harness: drives polylog init based on argv.
#![allow(unsafe_code)] // env::set_var is unsafe in Rust 2024; tests deliberately mutate env.

use polylog::{ColorMode, InitError, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let scenario = args.get(1).cloned().unwrap_or_default();
    let log_path = args.get(2).cloned().unwrap_or("-".to_string());

    match scenario.as_str() {
        "first_init" => {
            let _g = polylog::init().level(Level::Info).install()?;
            polylog::info!("first init ok");
        }
        "second_init_returns_error" => {
            let _g = polylog::init().install()?;
            match polylog::init().install() {
                Err(InitError::AlreadyInitialized) => println!("ALREADY_INITIALIZED"),
                Ok(_) => {
                    eprintln!("BUG: second init succeeded");
                    std::process::exit(2)
                }
                Err(e) => {
                    eprintln!("BUG: wrong error: {e}");
                    std::process::exit(2)
                }
            }
        }
        "pre_init_logging_no_panic" => {
            polylog::info!("before init");
            polylog::warn!("still before init");
            println!("PRE_INIT_OK");
        }
        "file_init_creates_dir" => {
            if log_path == "-" {
                eprintln!("need log path");
                std::process::exit(2);
            }
            let _g = polylog::init().log_file(&log_path).install()?;
            polylog::info!("file write");
        }
        "file_init_unwritable_returns_error" => {
            let bad = "/dev/null/sub/app.log";
            match polylog::init().log_file(bad).install() {
                Err(InitError::FileSetupFailed { .. }) => println!("FILE_SETUP_FAILED"),
                Ok(_) => {
                    eprintln!("BUG: expected FileSetupFailed, got Ok");
                    std::process::exit(2)
                }
                Err(e) => {
                    eprintln!("BUG: expected FileSetupFailed, got {e}");
                    std::process::exit(2)
                }
            }
        }
        "no_color_env_disables_ansi" => {
            unsafe { std::env::set_var("NO_COLOR", "1") };
            let _g = polylog::init().color(ColorMode::Auto).install()?;
            polylog::info!("no_color test");
        }
        "force_color_env_enables_ansi_when_piped" => {
            unsafe { std::env::set_var("FORCE_COLOR", "1") };
            let _g = polylog::init().color(ColorMode::Auto).install()?;
            polylog::info!("force_color test");
        }
        "log_crate_bridge" => {
            let _g = polylog::init().level(Level::Info).install()?;
            ::log::info!("from log crate");
        }
        "level_override_in_scope" => {
            let _g = polylog::init().level(Level::Info).install()?;
            polylog::info!("info visible");
            polylog::debug!("debug filtered");
            {
                let _o = polylog::LogLevelOverride::new(Level::Debug);
                polylog::debug!("now visible");
            }
            polylog::debug!("filtered again");
        }
        "catch_logs_error_chain" => {
            let _g = polylog::init().level(Level::Info).install()?;
            #[derive(Debug)]
            struct Outer(String);
            impl std::fmt::Display for Outer {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", self.0)
                }
            }
            impl std::error::Error for Outer {
                fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                    Some(&Inner)
                }
            }
            #[derive(Debug)]
            struct Inner;
            impl std::fmt::Display for Inner {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "inner cause")
                }
            }
            impl std::error::Error for Inner {}
            let _ = polylog::catch("ctx", || -> Result<(), Outer> {
                Err(Outer("outer error".to_string()))
            });
        }
        "concurrent_logging_no_corruption" => {
            let _g = polylog::init().level(Level::Info).install()?;
            use std::thread;
            let handles: Vec<_> = (0..8)
                .map(|t| {
                    thread::spawn(move || {
                        for i in 0..100 {
                            polylog::info!(thread = t, idx = i, "CONCURRENT_MARKER");
                        }
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        }
        other => {
            eprintln!("unknown scenario: {other}");
            std::process::exit(2)
        }
    }

    Ok(())
}
