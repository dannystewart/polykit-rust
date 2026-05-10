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
//! Use [`SfImage::to_tauri_menu_image`] (or [`SfImage::to_tauri_menu_image_for`]) for
//! `IconMenuItem` so Windows omits icons by default while macOS and Linux keep them. Use
//! [`SfImage::to_tauri_image`] for tray buttons, WebView chrome, and anywhere else you still want
//! a PNG on Windows.
//!
//! ```rust,ignore
//! use crate::icons::SfIcons;
//!
//! let item = IconMenuItem::with_id(
//!     app,
//!     "delete",
//!     "Delete",
//!     true,
//!     SfIcons::trash().to_tauri_menu_image()?,
//!     None::<&str>,
//! )?;
//! ```
//!
//! For menus built from the JS side, expose a command that uses [`SfImage::menu_png_bytes_for`]
//! (returns `None` on Windows unless the `windows_menu_icons` crate feature is enabled):
//!
//! ```rust,ignore
//! #[tauri::command]
//! fn sf_menu_icon(name: String, dark: bool) -> Result<Option<Vec<u8>>, String> {
//!     Ok(icons::SfIcons::get(&name)
//!         .and_then(|img| img.menu_png_bytes_for(dark).map(|bytes| bytes.to_vec())))
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
/// for toolbars, tray images, and other UI. For native context menus and menu bar items, prefer
/// [`to_tauri_menu_image`](SfImage::to_tauri_menu_image) so Windows can omit icons by default.
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

    /// PNG bytes for a native menu icon, or `None` when menu icons are suppressed on this
    /// platform.
    ///
    /// On Windows, returns `None` by default so frontends can pass `undefined` for
    /// `IconMenuItemOptions.icon` without duplicating menu definitions. Enable the
    /// `polysym/windows_menu_icons` crate feature to return [`Some`] with the same bytes as
    /// [`bytes_for`](Self::bytes_for) on Windows.
    ///
    /// On macOS and Linux, always returns [`Some`].
    #[must_use]
    pub fn menu_png_bytes_for(&self, dark_mode: bool) -> Option<&'static [u8]> {
        #[cfg(all(target_os = "windows", not(feature = "windows_menu_icons")))]
        {
            let _ = (self, dark_mode);
            None
        }
        #[cfg(not(all(target_os = "windows", not(feature = "windows_menu_icons"))))]
        {
            Some(self.bytes_for(dark_mode))
        }
    }

    /// Convert to a `tauri::image::Image` for a native menu item and explicit appearance.
    ///
    /// On Windows, returns `Ok(None)` by default (no decode). With the `windows_menu_icons`
    /// feature, behaves like [`to_tauri_image_for`](Self::to_tauri_image_for).
    ///
    /// # Errors
    ///
    /// Returns an error if the PNG bytes cannot be decoded by Tauri (non-Windows, or Windows
    /// with `windows_menu_icons` enabled).
    pub fn to_tauri_menu_image_for(
        &self,
        dark_mode: bool,
    ) -> tauri::Result<Option<Image<'static>>> {
        #[cfg(all(target_os = "windows", not(feature = "windows_menu_icons")))]
        {
            let _ = (self, dark_mode);
            Ok(None)
        }
        #[cfg(not(all(target_os = "windows", not(feature = "windows_menu_icons"))))]
        {
            self.to_tauri_image_for(dark_mode).map(Some)
        }
    }

    /// Convert to a `tauri::image::Image` for a native menu item using [`is_dark_mode`].
    ///
    /// See [`to_tauri_menu_image_for`](Self::to_tauri_menu_image_for).
    pub fn to_tauri_menu_image(&self) -> tauri::Result<Option<Image<'static>>> {
        self.to_tauri_menu_image_for(is_dark_mode())
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
