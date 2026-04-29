# polykit

Personal utility library for Rust, sibling to [polykit](https://github.com/dannystewart/polykit) (Python) and [polykit-swift](https://github.com/dannystewart/polykit-swift) (Swift).

## Status

Early development. v0.1 ships the `log` module only.

## Modules

- `polykit::log` — Branded logger built on [`tracing`](https://docs.rs/tracing). Console + optional rolling file output, three format modes, color modes, and ergonomic helpers.

Future modules (not yet shipped): `polykit::time`, `polykit::env`, etc.

## Quickstart

    use polykit::log;

    fn main() -> anyhow::Result<()> {
        let _guard = log::init()
            .level(log::Level::Info)
            .format(log::FormatMode::Context)
            .color(log::ColorMode::Auto)
            .install()?;

        log::info!("hello from polykit");
        log::warn!(user_id = 42, "elevated event");

        Ok(())
    }

The `_guard` must remain in scope for the lifetime of the program (it flushes the file writer on drop).

## Requirements

- Rust 1.85+ (edition 2024)

## License

TBD.
