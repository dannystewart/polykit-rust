//! Runtime support for SF Symbol icons in Tauri apps.
//!
//! After adding `polysym-build` to your `build.rs`, use this crate to pull the
//! generated symbols into your source and convert them to Tauri images.
//!
//! # Setup
//!
//! In any source file (e.g. `src/icons.rs`):
//!
//! ```rust,ignore
//! polysym::include_symbols!();
//! ```
//!
//! This brings `SfIcons` into scope in that module.
//!
//! # Native menu icons (PNG)
//!
//! ```rust,ignore
//! use crate::icons::SfIcons;
//!
//! let icon = SfIcons::trash().to_tauri_image()?;
//! let item = IconMenuItem::with_id(app, "delete", "Delete", true, Some(icon), None::<&str>)?;
//! ```
//!
//! For menus built from the JS side, expose a Tauri command:
//!
//! ```rust,ignore
//! #[tauri::command]
//! fn sf_icon(name: String, dark: bool) -> Result<Vec<u8>, String> {
//!     icons::SfIcons::get(&name)
//!         .map(|img| img.bytes_for(dark).to_vec())
//!         .ok_or_else(|| format!("sf symbol not registered: {name}"))
//! }
//! ```
//!
//! # WebView UI elements (SVG)
//!
//! SVGs use `currentColor` fill, so the icon color follows the CSS `color`
//! property of its container — dark mode, tinting, and states all work with
//! pure CSS, no light/dark variants needed.
//!
//! Expose a Tauri command and call it from your Svelte components:
//!
//! ```rust,ignore
//! #[tauri::command]
//! fn sf_svg(name: String) -> Result<String, String> {
//!     icons::SfIcons::get_svg(&name)
//!         .map(|s| s.to_string())
//!         .ok_or_else(|| format!("sf symbol not registered: {name}"))
//! }
//! ```
//!
//! In Svelte:
//!
//! ```svelte,ignore
//! <script lang="ts">
//!   import { invoke } from "@tauri-apps/api/core"
//!   let trashSvg = $state("")
//!   invoke<string>("sf_svg", { name: "trash" }).then(svg => trashSvg = svg)
//! </script>
//!
//! <!-- inline SVG inherits color from CSS -->
//! <span class="icon" style="color: red">{@html trashSvg}</span>
//!
//! <!-- or as an image via data URI -->
//! <img src="data:image/svg+xml,{encodeURIComponent(trashSvg)}" width="20" height="20" />
//! ```

use tauri::image::Image;

/// A light/dark pair of raw PNG bytes for an SF Symbol.
///
/// Created by the generated `SfIcons` methods. Call [`to_tauri_image`](SfImage::to_tauri_image)
/// to produce a `tauri::image::Image` for the current system appearance.
pub struct SfImage {
    light: &'static [u8],
    dark: &'static [u8],
}

impl SfImage {
    /// Create an `SfImage` from raw PNG bytes for each appearance.
    ///
    /// This is called by the generated `SfIcons` code and is not typically
    /// invoked directly.
    pub fn new(light: &'static [u8], dark: &'static [u8]) -> Self {
        Self { light, dark }
    }

    /// Return the PNG bytes for an explicit appearance choice.
    ///
    /// Prefer this over [`bytes`](Self::bytes) when the caller already knows the
    /// current appearance (e.g. passed in from the JS side via Tauri's window
    /// theme API), since it avoids the subprocess launched by [`is_dark_mode`].
    pub fn bytes_for(&self, dark_mode: bool) -> &'static [u8] {
        if dark_mode { self.dark } else { self.light }
    }

    /// Return the appropriate PNG bytes by auto-detecting the system appearance.
    ///
    /// For Rust-side menu construction. When bridging to JS, prefer passing the
    /// appearance as a parameter and calling [`bytes_for`](Self::bytes_for) instead.
    pub fn bytes(&self) -> &'static [u8] {
        self.bytes_for(is_dark_mode())
    }

    /// Convert to a `tauri::image::Image` for an explicit appearance choice.
    ///
    /// # Errors
    ///
    /// Returns an error if the PNG bytes cannot be decoded by Tauri.
    pub fn to_tauri_image_for(&self, dark_mode: bool) -> tauri::Result<Image<'static>> {
        Image::from_bytes(self.bytes_for(dark_mode))
    }

    /// Convert to a `tauri::image::Image` by auto-detecting the system appearance.
    ///
    /// # Errors
    ///
    /// Returns an error if the PNG bytes cannot be decoded by Tauri.
    pub fn to_tauri_image(&self) -> tauri::Result<Image<'static>> {
        Image::from_bytes(self.bytes())
    }
}

/// Returns `true` when the system is currently in dark mode.
///
/// Uses `/usr/bin/defaults read -g AppleInterfaceStyle` with the absolute path
/// to avoid PATH lookup issues inside GUI app processes. Call this at menu-build
/// time from Rust-side menu construction.
///
/// When bridging to JS, prefer getting the theme from Tauri's window API and
/// calling [`SfImage::bytes_for`] directly — it's more reliable than spawning a
/// subprocess from within the app process.
///
/// Returns `false` on any failure (including non-macOS platforms).
#[must_use]
pub fn is_dark_mode() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Include the `SfIcons` struct generated by `polysym-build` into the current module.
///
/// Place this in any source file (e.g. `src/icons.rs`). After this call,
/// `SfIcons` is available in that module and can be imported elsewhere with
/// `use crate::icons::SfIcons;`.
///
/// Requires that `polysym-build::generate(...)` was called in your `build.rs`.
#[macro_export]
macro_rules! include_symbols {
    () => {
        include!(concat!(env!("OUT_DIR"), "/polysym_generated.rs"));
    };
}
