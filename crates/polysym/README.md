# polysym

SF Symbol icons for Tauri apps — native menus and WebView UI elements, with cross-platform fallback so the same code runs on Windows and Linux.

`polysym` is two crates: **`polysym-build`** resolves and embeds symbol assets at build time, and **`polysym`** gives you a typed `SfIcons` struct at runtime to use them anywhere in your app.

Two formats are generated for every symbol in your manifest:

- **PNG** (light + dark variants) for native menu icons via `IconMenuItem`
- **SVG** (single `currentColor` variant) for WebView UI elements — color is controlled by CSS, so dark mode, tinting, and hover states all work automatically

## Three-tier resolution

For each symbol, `polysym-build` tries three strategies in order:

1. **Tier 1: `sfsym`.** On macOS dev machines with [`sfsym`](https://github.com/yapstudios/sfsym) installed (`brew install yapstudios/tap/sfsym`), live SF Symbol assets are produced from the system. This is the happy path and yields the highest-fidelity icons.
2. **Tier 2: committed cache.** When `sfsym` is unavailable (e.g. CI on Windows or Linux), `polysym-build` reads pre-generated assets from `<CARGO_MANIFEST_DIR>/polysym-assets/`. These are the exact tier-1 outputs, mirrored there by running `POLYSYM_REFRESH=1 cargo check` on a Mac and committed to the repo.
3. **Tier 3: Lucide.** When neither tier 1 nor tier 2 has a given symbol (e.g. you added an icon and forgot to refresh the cache before pushing), `polysym-build` falls back to a [Lucide](https://lucide.dev) icon you nominate per-symbol via `SymbolSpec::lucide("name")`. The Lucide SVG is rasterized into matching light/dark PNGs at build time, so menus and WebView icons both keep working with no consumer-side awareness.

Symbols that have no successful tier are quietly omitted from the generated `SfIcons` API. `SfIcons::get(name)` and `SfIcons::get_svg(name)` return `None` for them, so callers can fall back at the call site.

## Setup

### 1. Cargo.toml

```toml
[build-dependencies]
polysym-build = { git = "https://github.com/dannystewart/polykit-rust" }

[dependencies]
polysym = { git = "https://github.com/dannystewart/polykit-rust" }
tauri = { version = "...", features = ["image-png"] }  # image-png is required
```

### 2. build.rs

List every SF Symbol name you want to use along with its Lucide fallback. SF Symbol names match what you'd use in SwiftUI's `Image(systemName:)`; Lucide names use kebab-case and must exist in the [Lucide library](https://lucide.dev/icons/) (run `lucide-svg-rs list` to enumerate available names).

```rust
use polysym_build::SymbolSpec;

fn main() {
    polysym_build::generate_with_opts(&[
        SymbolSpec::new("trash").lucide("trash-2"),
        SymbolSpec::new("folder.badge.plus").lucide("folder-plus"),
        SymbolSpec::new("bell.slash").lucide("bell-off"),
        SymbolSpec::new("square.and.pencil").lucide("square-pen"),
        SymbolSpec::new("pin").lucide("pin"),
        SymbolSpec::new("sparkles").lucide("sparkles"),
        SymbolSpec::new("archivebox").lucide("archive"),
    ]);
    tauri_build::build();
}
```

Symbols are exported at 20pt by default (40×40 px at 2x), which displays at 20 logical points in macOS 26 menus. Override per-symbol options with the builder methods on `SymbolSpec` (`.size(...)`, `.weight(...)`, `.padding(...)`, `.lucide(...)`).

If you don't need a Lucide fallback for a given symbol, omit `.lucide(...)`. If the simpler form is enough — bare names with no Lucide support — use `polysym_build::generate(&["trash", "pin", ...])`.

This embeds the resulting PNGs and SVGs in your binary. Regeneration happens automatically whenever `build.rs` changes.

### Refreshing the committed cache

After adding or changing a symbol in `build.rs`, run:

```sh
POLYSYM_REFRESH=1 cargo check
```

…anywhere in your project on a macOS machine with `sfsym` installed. This mirrors the live tier-1 outputs into `<CARGO_MANIFEST_DIR>/polysym-assets/`, which you then commit. Cross-platform CI uses those committed assets via tier 2; if a symbol's cache is missing or stale on Mac, you'll see a `cargo::warning` reminding you to refresh.

A typical wrapper script in your app's repo looks like:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
POLYSYM_REFRESH=1 cargo check --manifest-path src-tauri/Cargo.toml
```

Wire that into your pre-commit hook to keep the cache in sync automatically.

### 3. src/icons.rs

Create a module file with a single line:

```rust
polysym::include_symbols!();
```

This pulls the generated `SfIcons` struct into that module.

### 4. lib.rs / main.rs

```rust
mod icons;
```

## Usage

### From Rust (Rust-side menu construction)

```rust
use crate::icons::SfIcons;
use tauri::menu::IconMenuItem;

let item = IconMenuItem::with_id(
    app,
    "delete",
    "Delete Conversation",
    true,
    Some(SfIcons::trash().to_tauri_image()?),
    None::<&str>,
)?;
```

Symbol names map to methods by replacing dots and hyphens with underscores:

| SF Symbol name         | Method                         |
|------------------------|--------------------------------|
| `trash`                | `SfIcons::trash()`             |
| `folder.badge.plus`    | `SfIcons::folder_badge_plus()` |
| `bell.slash`           | `SfIcons::bell_slash()`        |
| `square.and.pencil`    | `SfIcons::square_and_pencil()` |

### From JavaScript / Svelte (JS-side menu construction)

Expose a Tauri command that bridges the icon bytes to your frontend:

```rust
// src/lib.rs
mod icons;
use icons::SfIcons;

#[tauri::command]
fn sf_icon(name: String) -> Result<Vec<u8>, String> {
    SfIcons::get(&name)
        .map(|img| img.bytes().to_vec())
        .ok_or_else(|| format!("sf symbol not registered: {name}"))
}
```

Then in Svelte (the return type `number[]` is accepted directly by `IconMenuItemOptions.icon`):

```ts
import { invoke } from "@tauri-apps/api/core"
import { IconMenuItem, Menu } from "@tauri-apps/api/menu"

const dark = window.matchMedia("(prefers-color-scheme: dark)").matches
const trashIcon = await invoke<number[]>("sf_icon", { name: "trash", dark }).catch(() => undefined)

const deleteItem = await IconMenuItem.new({
    id: "delete-conversation",
    text: "Delete Conversation",
    icon: trashIcon,
    action: () => { /* ... */ },
})
```

Passing `dark` explicitly from the JS side is more reliable than letting the Rust command detect appearance via a subprocess. `window.matchMedia("(prefers-color-scheme: dark)").matches` is evaluated by Tauri's WKWebView directly against the macOS system appearance, making it the most accurate and zero-cost source of truth. The `.catch(() => undefined)` is a safe fallback: if `sf_icon` fails (e.g. in a web dev build without the Tauri runtime), the menu item renders without an icon rather than throwing.

The corresponding Tauri command signature should accept the `dark` flag and call `bytes_for`:

```rust
#[tauri::command]
fn sf_icon(name: String, dark: bool) -> Result<Vec<u8>, String> {
    SfIcons::get(&name)
        .map(|img| img.bytes_for(dark).to_vec())
        .ok_or_else(|| format!("sf symbol not registered: {name}"))
}
```

### From JavaScript / Svelte (WebView UI elements via SVG)

SVGs use `currentColor` fill, so the icon color follows the CSS `color` property of its container — no `dark` parameter needed.

Add a Tauri command:

```rust
#[tauri::command]
fn sf_svg(name: String) -> Result<String, String> {
    icons::SfIcons::get_svg(&name)
        .map(|s| s.to_string())
        .ok_or_else(|| format!("sf symbol not registered: {name}"))
}
```

Register it alongside `sf_icon` in `invoke_handler!`. Then in Svelte:

```svelte
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core"

  let trashSvg = $state("")
  invoke<string>("sf_svg", { name: "trash" }).then(svg => (trashSvg = svg))
</script>

<!-- Inline: inherits color from CSS, scales with font size -->
<span class="icon" style="color: currentColor">{@html trashSvg}</span>

<!-- Or as an image with explicit size -->
<img src="data:image/svg+xml,{encodeURIComponent(trashSvg)}" width="20" height="20" alt="" />
```

To tint or switch appearance, just set `color` on the container:

```css
.icon { color: #333; }
@media (prefers-color-scheme: dark) { .icon { color: #eee; } }
```

Or in a Svelte component:

```svelte
<span style="color: {isDark ? 'white' : 'black'}">{@html trashSvg}</span>
```

For convenience when you know you'll always use a symbol inline, you can also call the typed method directly from Rust before passing to the frontend:

```rust
// Returns the SVG string directly (no dynamic lookup needed)
let svg = icons::SfIcons::trash_svg();
```

## Custom size and weight

`SymbolSpec` is a builder; chain methods for finer control:

```rust
use polysym_build::SymbolSpec;

fn main() {
    polysym_build::generate_with_opts(&[
        SymbolSpec::new("trash").lucide("trash-2"),                              // defaults: 20pt, regular
        SymbolSpec::new("sidebar.left").size(16).lucide("panel-left"),          // 16pt → 32×32 px at 2x
        SymbolSpec::new("bold").weight("semibold").lucide("bold"),              // semibold weight
        SymbolSpec::new("rare.sf.symbol"),                                       // no Lucide fallback
    ]);
    tauri_build::build();
}
```

Available weights: `ultralight`, `thin`, `light`, `regular`, `medium`, `semibold`, `bold`, `heavy`, `black`.

PNG output is always at 2x density: `--size N` produces an `N×2` × `N×2` pixel file.

## How it works

1. **Validation.** Every `.lucide("name")` is checked against the bundled Lucide archive (via [`lucide-svg-rs`](https://crates.io/crates/lucide-svg-rs)). A typo fails the build with a clear `cargo::error`.
2. **Tier 1.** `polysym_build` calls `sfsym batch` three times — black PNGs (light mode), white PNGs (dark mode), and monochrome SVGs — piping all export commands through stdin for maximum throughput (~800 symbols/sec). PNGs are post-processed: composited onto a padded transparent canvas with `@2x` Retina DPI metadata (`pHYs` chunk, 5669 px/m = 144 DPI) so `NSImage` renders them sharp without scaling. SVGs are post-processed to replace the hardcoded color with `currentColor` and rewrite the viewBox to a tight crop around the glyph's alignment rect.
3. **Tier 2.** When `sfsym` is unavailable, `polysym_build` looks for `<name>.light.png`, `<name>.dark.png`, and `<name>.svg` under `<CARGO_MANIFEST_DIR>/polysym-assets/`. If all three are present, they're copied byte-for-byte into `$OUT_DIR`.
4. **Tier 3.** When no cached asset exists, `polysym_build` reads the configured Lucide SVG, normalizes it for inline use, and rasterizes it twice through [`resvg`](https://crates.io/crates/resvg) — once with `currentColor` resolved to `#000000` (light) and once to `#ffffff` (dark) — applying the same padding + `pHYs` pipeline as tier 1 so the resulting PNGs work transparently in `IconMenuItem`.
5. All resolved assets land in Cargo's `$OUT_DIR`. The committed cache directory `polysym-assets/` is the only thing that lives in your source tree.
6. A `polysym_generated.rs` file is written to `$OUT_DIR` containing a `SfIcons` struct with typed methods per symbol (`include_bytes!` for PNGs, `include_str!` for SVGs) and `get` / `get_svg` dynamic lookups. Symbols whose all three tiers fail are simply absent from this file — typed methods don't exist for them and dynamic lookups return `None`.
7. `polysym::include_symbols!()` expands to `include!(concat!(env!("OUT_DIR"), "/polysym_generated.rs"))`, pulling `SfIcons` into the calling module.
8. At runtime, `SfImage::bytes()` calls `polysym::is_dark_mode()` (a one-shot `defaults read -g AppleInterfaceStyle` on macOS, `false` elsewhere) and returns the appropriate byte slice.

## Notes

- The generated assets live in `$OUT_DIR` (per build) plus `polysym-assets/` (committed cache). Nothing else needs to change in your `.gitignore`.
- Icons are appearance-aware at menu-build time, not per-render. If a user switches appearance while your app is running, menus created before the switch will keep their original icon variant until recreated.
- `sfsym` uses a private AppKit API to read SF Symbol geometry. It has been stable from macOS 13 through macOS 26, but it is a build-time tool — this risk is the same as any other build dependency.
- SF Symbols are Apple's property. Their license permits use only in apps targeting Apple platforms. The Lucide tier-3 fallback exists so that **the same Tauri app** can be built on Windows / Linux without shipping SF Symbol assets there — those builds receive Lucide-rendered PNGs/SVGs instead. Do not commit SF Symbol PNGs / SVGs into a non-Apple-only repo if your distribution requires you to avoid shipping Apple-licensed assets to other platforms; in that case, omit the `polysym-assets/` directory and let tier 3 handle every symbol on non-Apple builds.
