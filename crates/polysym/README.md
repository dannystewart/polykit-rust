# polysym

SF Symbol icons for Tauri apps on macOS — native menus and WebView UI elements.

`polysym` is two crates: **`polysym-build`** generates symbol assets at build time using [`sfsym`](https://github.com/yapstudios/sfsym), and **`polysym`** gives you a typed `SfIcons` struct at runtime to use them anywhere in your app.

Two formats are generated for every symbol in your manifest:

- **PNG** (light + dark variants) for native menu icons via `IconMenuItem`
- **SVG** (single `currentColor` variant) for WebView UI elements — color is controlled by CSS, so dark mode, tinting, and hover states all work automatically

## Prerequisites

`sfsym` must be installed on any machine that builds your app:

```sh
brew install yapstudios/tap/sfsym
```

`polysym-build` will detect it in `PATH` and common Homebrew and manual install locations, and fail the build with a clear error if it's missing.

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

Call `polysym_build::generate` before `tauri_build::build`. List every SF Symbol name you want to use — these are the same names you'd use in SwiftUI's `Image(systemName:)`.

Symbols are exported at 20pt by default (40×40 px at 2x), which displays at 20 logical points in macOS 26 menus. Override per-symbol with `generate_with_opts` if needed.

```rust
fn main() {
    polysym_build::generate(&[
        "trash",
        "folder.badge.plus",
        "bell.slash",
        "square.and.pencil",
        "pin",
        "sparkles",
        "archivebox",
    ]);
    tauri_build::build();
}
```

This runs `sfsym` and embeds the resulting PNGs and SVGs in your binary. Regeneration happens automatically whenever `build.rs` changes.

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

For symbols that need different options, use `generate_with_opts` and `SymbolSpec`:

```rust
use polysym_build::SymbolSpec;

fn main() {
    polysym_build::generate_with_opts(&[
        SymbolSpec::new("trash"),                        // defaults: 20pt, regular
        SymbolSpec::new("sidebar.left").size(16),        // 16pt → 32×32 px at 2x
        SymbolSpec::new("bold").weight("semibold"),      // semibold weight
    ]);
    tauri_build::build();
}
```

Available weights: `ultralight`, `thin`, `light`, `regular`, `medium`, `semibold`, `bold`, `heavy`, `black`.

PNG output is always at 2x density: `--size N` produces an `N×2` × `N×2` pixel file.

## How it works

1. `polysym_build::generate` calls `sfsym batch` three times — black PNGs (light mode), white PNGs (dark mode), and monochrome SVGs — piping all export commands through stdin for maximum throughput (~800 symbols/sec).
2. PNGs are post-processed: composited onto a padded transparent canvas with `@2x` Retina DPI metadata (`pHYs` chunk, 5669 px/m = 144 DPI) so `NSImage` renders them sharp without scaling. SVGs are post-processed to replace the hardcoded color with `currentColor`.
3. All assets land in Cargo's `$OUT_DIR`, never in your source tree.
4. A `polysym_generated.rs` file is written to `$OUT_DIR` containing a `SfIcons` struct with typed methods per symbol (`include_bytes!` for PNGs, `include_str!` for SVGs) and `get`/`get_svg` dynamic lookups.
5. `polysym::include_symbols!()` expands to `include!(concat!(env!("OUT_DIR"), "/polysym_generated.rs"))`, pulling `SfIcons` into the calling module.
6. At runtime, `SfImage::bytes()` calls `polysym::is_dark_mode()` (a one-shot `defaults read -g AppleInterfaceStyle`) and returns the appropriate byte slice.

## Notes

- The generated assets live entirely in `$OUT_DIR` — nothing to add to `.gitignore`.
- Icons are appearance-aware at menu-build time, not per-render. If a user switches appearance while your app is running, menus created before the switch will keep their original icon variant until recreated.
- `sfsym` uses a private AppKit API to read SF Symbol geometry. It has been stable from macOS 13 through macOS 26, but it is a build-time tool — this risk is the same as any other build dependency.
- SF Symbols are Apple's property. Their license permits use only in apps targeting Apple platforms. Don't use `polysym` to ship symbols in a non-Apple-platform app.
