# polylog

Branded logger built on `tracing`. Sibling of [polykit](https://github.com/dannystewart/polykit)
(Python) and [polykit-swift](https://github.com/dannystewart/polykit-swift) (Swift). Lives in
the [polykit-rust](https://github.com/dannystewart/polykit-rust) workspace alongside
[`polybase`](../polybase/).

This is the simplest crate in the workspace — a thin opinionated wrapper around `tracing` so
all my Rust apps and libraries log with consistent timestamps, colors, level prefixes, and
file output.

## Quickstart

```rust,no_run
use polylog::{FormatMode, Level};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = polylog::init()
        .level(Level::Info)
        .format(FormatMode::Context)
        .install()?;

    polylog::info!("hello from polylog");
    polylog::warn!("something to look at");
    polylog::error!("everything's on fire");
    Ok(())
}
```

The returned `InitGuard` must remain in scope for the program lifetime — dropping it flushes
any pending file output.

## Levels

Four levels: `Debug`, `Info`, `Warn`, `Error`.

- The `tracing` `TRACE` level is dropped (not rendered) — I've never wanted finer than `DEBUG`
  in practice.
- There is no `CRITICAL` level — `Error` covers it.

## Format modes

| Mode | Output |
|------|--------|
| `Bare` | `[info] message` |
| `Context` | `3:14:15 PM [info] my_crate src/file.rs:42 message` |

## Re-exports

The crate re-exports `tracing` itself plus the common macros so you can use `polylog::info!`,
`polylog::span!`, `polylog::instrument`, etc. without a separate `tracing` import:

```rust,ignore
use polylog::{info, instrument};

#[instrument]
fn my_function(x: u32) {
    info!(x, "called");
}
```

## License

MIT — see workspace [`LICENSE`](../../LICENSE).
