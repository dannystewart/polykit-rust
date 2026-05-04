//! Build-time helper for generating SF Symbol icon assets with cross-platform fallbacks.
//!
//! Add this to your app's `build.rs`:
//!
//! ```rust,ignore
//! use polysym_build::SymbolSpec;
//!
//! fn main() {
//!     polysym_build::generate_with_opts(&[
//!         SymbolSpec::new("trash").lucide("trash-2"),
//!         SymbolSpec::new("folder.badge.plus").lucide("folder-plus"),
//!         SymbolSpec::new("bell.slash").lucide("bell-off"),
//!     ]);
//!     tauri_build::build();
//! }
//! ```
//!
//! # Three-tier resolution
//!
//! For each symbol, `polysym-build` attempts to produce a light PNG, dark PNG,
//! and SVG via three tiers in order:
//!
//! 1. **Tier 1 (sfsym):** if the `sfsym` binary is installed (`brew install yapstudios/tap/sfsym`),
//!    invoke it for live SF Symbol generation. On macOS dev machines this is
//!    the happy path. When `POLYSYM_REFRESH=1` is set, generated assets are
//!    also mirrored into `<CARGO_MANIFEST_DIR>/polysym-assets/` so they can be
//!    committed to source control.
//! 2. **Tier 2 (committed cache):** if `sfsym` is unavailable or fails, look in
//!    `<CARGO_MANIFEST_DIR>/polysym-assets/` for previously generated PNG/SVG
//!    files. This makes builds work on Windows or Linux as long as the assets
//!    were committed by a macOS developer with `sfsym` installed.
//! 3. **Tier 3 (Lucide):** if the symbol has a `.lucide("name")` fallback set,
//!    fetch the corresponding SVG from the `lucide-svg-rs` embedded archive,
//!    rasterize light/dark PNG variants via `resvg`, and use those. This lets
//!    apps degrade gracefully when an SF asset has not yet been refreshed.
//!
//! Symbols without any successful tier are simply omitted from the generated
//! `SfIcons` API; `SfIcons::get(name)` and `SfIcons::get_svg(name)` return
//! `None` for them, so callers can fall back at the call site.

use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};
use std::process::Command;

use lucide_svg_rs::{ICONS_TAR, LucideClient};

/// Geometry returned by `sfsym info <name> --json`.
///
/// Coordinates are in the symbol's natural (unscaled) point space.
#[derive(Debug, Clone)]
struct SymbolInfo {
    /// Full bounding box of the symbol as rendered by `AppKit`.
    natural_width: f64,
    natural_height: f64,
    /// Alignment rect: the visually meaningful glyph bounds within the full box.
    align_x: f64,
    align_y: f64,
    align_w: f64,
    align_h: f64,
}

/// Configuration for a single SF Symbol export.
#[derive(Debug, Clone)]
pub struct SymbolSpec {
    /// SF Symbol name, e.g. `"folder.badge.plus"`.
    pub name: String,
    /// Point size of the final icon canvas. Defaults to `20` (40×40 px at 2x).
    pub size: u32,
    /// Symbol weight. Defaults to `"regular"`.
    pub weight: String,
    /// Transparent padding added to each side, as a fraction of `size`.
    ///
    /// Defaults to `0.12` (12% per side → symbol fills ~76% of the canvas),
    /// which matches the visual breathing room of native macOS SF Symbol
    /// menu icons. sfsym renders symbols full-bleed to their canvas edges, so
    /// padding is applied by compositing the symbol into a larger transparent
    /// canvas after export.
    pub padding: f32,
    /// Optional Lucide icon name used as a tier-3 fallback when neither
    /// `sfsym` nor a committed cache asset is available for this symbol.
    ///
    /// Lucide names must match an entry in the `lucide-svg-rs` archive (run
    /// `lucide-svg-rs list` to see all available names). They are validated at
    /// build time; an invalid name produces a `cargo::error`.
    pub lucide: Option<String>,
}

impl SymbolSpec {
    /// Create a spec with default settings (20pt, regular weight, 12% padding).
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), size: 20, weight: "regular".into(), padding: 0.12, lucide: None }
    }

    /// Override the point size of the final canvas. The symbol is rendered at
    /// `size * (1 - 2 * padding)` points and then composited into a `size * 2`
    /// pixel canvas to achieve the requested padding.
    pub fn size(mut self, size: u32) -> Self {
        self.size = size;
        self
    }

    /// Override the symbol weight (ultralight, thin, light, regular, medium,
    /// semibold, bold, heavy, black).
    pub fn weight(mut self, weight: impl Into<String>) -> Self {
        self.weight = weight.into();
        self
    }

    /// Override the per-side padding fraction (0.0 = full bleed, 0.2 = generous margin).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding.clamp(0.0, 0.45);
        self
    }

    /// Set the Lucide icon name to use as a tier-3 fallback when neither
    /// `sfsym` nor a committed cache asset is available for this symbol.
    ///
    /// Lucide names use kebab-case (e.g. `"trash-2"`, `"folder-plus"`).
    pub fn lucide(mut self, name: impl Into<String>) -> Self {
        self.lucide = Some(name.into());
        self
    }

    /// The point size passed to sfsym — smaller than `size` to leave room for padding.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    fn inner_size(&self) -> u32 {
        let fill = 1.0 - 2.0 * self.padding;
        ((self.size as f32) * fill).round().max(1.0) as u32
    }

    /// Final canvas side length in pixels (2x density).
    fn canvas_px(&self) -> u32 {
        self.size * 2
    }
}

impl<S: Into<String>> From<S> for SymbolSpec {
    fn from(name: S) -> Self {
        Self::new(name)
    }
}

/// Which tier resolved a given symbol's assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolResolution {
    /// `sfsym` produced live PNG + SVG output for this symbol.
    Sf,
    /// PNG + SVG were copied from the committed `polysym-assets/` cache.
    Cached,
    /// PNG + SVG were derived from a Lucide tier-3 fallback.
    Lucide,
    /// No tier produced output; the symbol is omitted from the generated API.
    Missing,
}

/// Generate SF Symbol assets for a list of symbol names using default settings.
///
/// Symbols are exported at 20pt (40×40 px at 2x) with 12% padding per side,
/// in both light (black) and dark (white) variants. No Lucide fallback is
/// configured — pass [`SymbolSpec::lucide`] explicitly via [`generate_with_opts`]
/// if you want cross-platform tier-3 graceful degradation.
pub fn generate(symbols: &[&str]) {
    let specs: Vec<SymbolSpec> = symbols.iter().map(|&s| SymbolSpec::new(s)).collect();
    generate_with_opts(&specs);
}

/// Generate SF Symbol PNG and SVG assets with full control over each symbol's options.
///
/// See the crate-level documentation for the three-tier resolution algorithm
/// and the `POLYSYM_REFRESH` environment variable.
pub fn generate_with_opts(specs: &[SymbolSpec]) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let symbol_dir = out_dir.join("polysym");
    std::fs::create_dir_all(&symbol_dir).expect("failed to create polysym output dir");

    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let assets_dir = manifest_dir.join("polysym-assets");

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=POLYSYM_REFRESH");
    println!("cargo::rerun-if-env-changed=POLYSYM_NO_SFSYM");
    println!("cargo::rerun-if-changed={}", assets_dir.display());

    let refresh = std::env::var("POLYSYM_REFRESH").is_ok_and(|v| !v.is_empty() && v != "0");

    // Validate Lucide names early so a typo fails the build with a clear error
    // before any subprocess work happens.
    let lucide_client = LucideClient::new(ICONS_TAR)
        .expect("failed to open bundled Lucide icon archive (lucide-svg-rs ICONS_TAR)");
    let valid_lucide: HashSet<String> = lucide_client
        .list_icons()
        .expect("failed to list bundled Lucide icons")
        .into_iter()
        .collect();

    let mut bad_lucide = false;
    for spec in specs {
        if let Some(lname) = &spec.lucide
            && !valid_lucide.contains(lname)
        {
            println!(
                "cargo::error=polysym: '{}' is not a valid Lucide icon name \
                 (configured as fallback for SF symbol '{}'). \
                 Run `lucide-svg-rs list` to see available names.",
                lname, spec.name,
            );
            bad_lucide = true;
        }
    }
    // Cargo will fail the build when it sees the cargo::error lines above;
    // bailing here avoids spending more time on a doomed build.
    assert!(
        !bad_lucide,
        "polysym: one or more .lucide() fallback names are invalid; see cargo errors above",
    );

    let sfsym = find_sfsym();

    // Tier 1: try sfsym for everything. The set returned contains symbols for
    // which sfsym successfully produced light PNG, dark PNG, and SVG output.
    let sfsym_succeeded: HashSet<String> = if let Some(ref sfsym_path) = sfsym {
        run_sfsym_for_all(sfsym_path, specs, &symbol_dir)
    } else {
        HashSet::new()
    };

    // Resolve each symbol through the three tiers and post-process / write
    // the final files into OUT_DIR.
    let mut resolutions: Vec<(SymbolSpec, SymbolResolution)> = Vec::with_capacity(specs.len());
    for spec in specs {
        let resolution = if sfsym_succeeded.contains(&spec.name) {
            finalize_sfsym_assets(spec, &symbol_dir, sfsym.as_deref());
            if refresh {
                copy_to_assets_dir(spec, &symbol_dir, &assets_dir);
            }
            check_stale_asset(spec, &symbol_dir, &assets_dir);
            SymbolResolution::Sf
        } else if try_copy_from_assets(spec, &symbol_dir, &assets_dir) {
            println!(
                "cargo::warning=polysym: '{}' using committed cache (sfsym unavailable or failed)",
                spec.name,
            );
            SymbolResolution::Cached
        } else if let Some(lname) = &spec.lucide {
            let svg = lucide_client.get_icon_content(lname).unwrap_or_else(|e| {
                panic!(
                    "polysym: failed to load Lucide fallback '{}' for symbol '{}': {e}",
                    lname, spec.name,
                )
            });
            write_lucide_assets(&svg, spec, &symbol_dir);
            println!("cargo::warning=polysym: '{}' using Lucide fallback '{}'", spec.name, lname);
            SymbolResolution::Lucide
        } else {
            println!(
                "cargo::warning=polysym: '{}' has no asset and no .lucide() fallback - omitted",
                spec.name,
            );
            SymbolResolution::Missing
        };
        resolutions.push((spec.clone(), resolution));
    }

    let generated_path = out_dir.join("polysym_generated.rs");
    let generated_source = build_generated_source(&resolutions, &symbol_dir);
    std::fs::write(&generated_path, generated_source)
        .expect("failed to write polysym_generated.rs");
}

/// Post-process raw sfsym output (PNG padding + SVG normalization) for one symbol.
fn finalize_sfsym_assets(spec: &SymbolSpec, symbol_dir: &Path, sfsym: Option<&Path>) {
    for variant in ["light", "dark"] {
        let path = symbol_dir.join(format!("{}.{}.png", spec.name, variant));
        apply_padding(&path, spec);
    }
    let svg_path = symbol_dir.join(format!("{}.svg", spec.name));
    let info = sfsym.and_then(|s| query_symbol_info(s, &spec.name));
    apply_svg_postprocess(&svg_path, spec, info.as_ref());
}

/// Run all three sfsym batches and report which symbols fully succeeded.
///
/// Returns an empty set if any batch fails entirely; otherwise verifies on a
/// per-symbol basis that all three output files exist (some symbols may be
/// invalid SF Symbol names and silently skipped by sfsym).
fn run_sfsym_for_all(sfsym: &Path, specs: &[SymbolSpec], symbol_dir: &Path) -> HashSet<String> {
    let light_batch = build_png_batch_input(specs, symbol_dir, "light", "monochrome", "#000000");
    let dark_batch = build_png_batch_input(specs, symbol_dir, "dark", "hierarchical", "#ffffff");
    let svg_batch = build_svg_batch_input(specs, symbol_dir);

    let light_ok = run_batch(sfsym, &light_batch, "light PNG");
    let dark_ok = run_batch(sfsym, &dark_batch, "dark PNG");
    let svg_ok = run_batch(sfsym, &svg_batch, "SVG");

    if !(light_ok && dark_ok && svg_ok) {
        return HashSet::new();
    }

    let mut succeeded = HashSet::new();
    for spec in specs {
        let light = symbol_dir.join(format!("{}.light.png", spec.name));
        let dark = symbol_dir.join(format!("{}.dark.png", spec.name));
        let svg = symbol_dir.join(format!("{}.svg", spec.name));
        if light.exists() && dark.exists() && svg.exists() {
            succeeded.insert(spec.name.clone());
        }
    }
    succeeded
}

/// Mirror finalized `OUT_DIR` assets for a symbol into the committed cache.
fn copy_to_assets_dir(spec: &SymbolSpec, symbol_dir: &Path, assets_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(assets_dir) {
        println!(
            "cargo::warning=polysym: could not create assets dir {}: {e}",
            assets_dir.display(),
        );
        return;
    }
    for ext in ["light.png", "dark.png", "svg"] {
        let src = symbol_dir.join(format!("{}.{}", spec.name, ext));
        let dst = assets_dir.join(format!("{}.{}", spec.name, ext));
        if let Err(e) = std::fs::copy(&src, &dst) {
            println!(
                "cargo::warning=polysym: failed to mirror {} to {}: {e}",
                src.display(),
                dst.display(),
            );
        }
    }
}

/// Try to copy committed cache assets into `OUT_DIR`. Returns `true` if all three
/// files (light PNG, dark PNG, SVG) were present and copied successfully.
fn try_copy_from_assets(spec: &SymbolSpec, symbol_dir: &Path, assets_dir: &Path) -> bool {
    let parts = ["light.png", "dark.png", "svg"];
    if !parts.iter().all(|ext| assets_dir.join(format!("{}.{}", spec.name, ext)).is_file()) {
        return false;
    }
    for ext in parts {
        let src = assets_dir.join(format!("{}.{}", spec.name, ext));
        let dst = symbol_dir.join(format!("{}.{}", spec.name, ext));
        if let Err(e) = std::fs::copy(&src, &dst) {
            println!(
                "cargo::warning=polysym: failed to copy cached {} to {}: {e}",
                src.display(),
                dst.display(),
            );
            return false;
        }
    }
    true
}

/// Compare freshly generated tier-1 assets against the committed cache and
/// warn if they would differ. Only meaningful on macOS dev machines where
/// `sfsym` actually ran; silently no-ops elsewhere.
fn check_stale_asset(spec: &SymbolSpec, symbol_dir: &Path, assets_dir: &Path) {
    let mut stale = Vec::new();
    for ext in ["light.png", "dark.png", "svg"] {
        let fresh = symbol_dir.join(format!("{}.{}", spec.name, ext));
        let cached = assets_dir.join(format!("{}.{}", spec.name, ext));
        let differs = match (std::fs::read(&fresh), std::fs::read(&cached)) {
            (Ok(f), Ok(c)) => f != c,
            (Ok(_), Err(_)) => true,
            // If we can't read the fresh output something else has gone wrong;
            // skip the stale check rather than emit confusing warnings.
            _ => false,
        };
        if differs {
            stale.push(ext);
        }
    }
    if !stale.is_empty() {
        println!(
            "cargo::warning=polysym: '{}' has stale or missing committed assets [{}] - \
             run scripts/polysym-refresh.sh before commit",
            spec.name,
            stale.join(", "),
        );
    }
}

/// Write a Lucide-derived SVG and rasterize PNG variants for one symbol.
///
/// The SVG is normalized for inline use (drop hardcoded width/height so CSS
/// can size it). The PNGs are produced by replacing `currentColor` with black
/// or white and rendering through resvg, then run through the same padding +
/// pHYs metadata pipeline as tier-1/tier-2 PNGs so they behave identically in
/// `IconMenuItem`.
fn write_lucide_assets(svg: &str, spec: &SymbolSpec, symbol_dir: &Path) {
    let svg_path = symbol_dir.join(format!("{}.svg", spec.name));
    let normalized_svg = rewrite_svg_tag(svg, spec, None);
    std::fs::write(&svg_path, &normalized_svg).unwrap_or_else(|e| {
        panic!("polysym: failed to write Lucide SVG {}: {e}", svg_path.display())
    });

    for (variant, color) in [("light", "#000000"), ("dark", "#ffffff")] {
        let png_path = symbol_dir.join(format!("{}.{}.png", spec.name, variant));
        let colored = svg.replace("currentColor", color);
        let png_bytes = rasterize_svg_to_png(&colored, spec);
        std::fs::write(&png_path, &png_bytes).unwrap_or_else(|e| {
            panic!("polysym: failed to write Lucide PNG {}: {e}", png_path.display())
        });
        apply_padding(&png_path, spec);
    }
}

/// Render the given SVG into a square `inner_size * 2` pixel PNG byte buffer.
///
/// The output has no `pHYs` chunk; [`apply_padding`] will composite it into the
/// final padded canvas and add the @2x metadata.
#[allow(clippy::cast_precision_loss)]
fn rasterize_svg_to_png(svg: &str, spec: &SymbolSpec) -> Vec<u8> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opt)
        .unwrap_or_else(|e| panic!("polysym: failed to parse Lucide SVG for '{}': {e}", spec.name));

    let inner_px = spec.inner_size() * 2;
    let mut pixmap = tiny_skia::Pixmap::new(inner_px, inner_px)
        .unwrap_or_else(|| panic!("polysym: failed to allocate pixmap for '{}'", spec.name));

    let svg_size = tree.size();
    let target = inner_px as f32;
    let scale = target / svg_size.width().max(svg_size.height());
    let transform = tiny_skia::Transform::from_scale(scale, scale);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .unwrap_or_else(|e| panic!("polysym: failed to encode Lucide PNG for '{}': {e}", spec.name))
}

/// Composite the symbol PNG (rendered at `inner_size`) into a larger transparent
/// canvas (`canvas_px`), centering it, and write the result with the @2x Retina
/// `pHYs` DPI metadata chunk preserved.
///
/// Without the `pHYs` chunk, `NSImage` treats the canvas as 1x (e.g. 40×40px
/// becomes 40×40pt), and the menu system scales it down to the icon slot,
/// producing blurry output. With `pHYs` set to 144 DPI (5669 pixels/meter),
/// `NSImage` correctly interprets the canvas as @2x (40px = 20pt) and no
/// scaling occurs.
fn apply_padding(path: &Path, spec: &SymbolSpec) {
    use image::RgbaImage;
    use std::io::BufWriter;

    let canvas_px = spec.canvas_px();
    let inner_px = spec.inner_size() * 2;

    if canvas_px <= inner_px {
        return;
    }

    let src = image::open(path)
        .unwrap_or_else(|e| panic!("failed to open {} for padding: {e}", path.display()))
        .to_rgba8();

    let mut canvas = RgbaImage::new(canvas_px, canvas_px);
    let x_off = (canvas_px - src.width()) / 2;
    let y_off = (canvas_px - src.height()) / 2;
    image::imageops::overlay(&mut canvas, &src, i64::from(x_off), i64::from(y_off));

    let file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("failed to open {} for writing: {e}", path.display()));

    let mut enc = png::Encoder::new(BufWriter::new(file), canvas_px, canvas_px);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);

    let mut writer =
        enc.write_header().unwrap_or_else(|e| panic!("PNG header for {}: {e}", path.display()));

    // pHYs chunk: 5669 pixels/meter ≈ 144 DPI = @2x Retina.
    #[allow(clippy::items_after_statements)]
    const PPM_2X: u32 = 5669;
    let mut phys = [0u8; 9];
    phys[0..4].copy_from_slice(&PPM_2X.to_be_bytes());
    phys[4..8].copy_from_slice(&PPM_2X.to_be_bytes());
    phys[8] = 1; // unit: meter
    writer
        .write_chunk(png::chunk::ChunkType(*b"pHYs"), &phys)
        .unwrap_or_else(|e| panic!("pHYs chunk for {}: {e}", path.display()));

    writer
        .write_image_data(canvas.as_raw())
        .unwrap_or_else(|e| panic!("PNG data for {}: {e}", path.display()));
}

/// Locate the `sfsym` binary, returning `None` if it is not installed.
///
/// Searches `PATH` plus common Homebrew and manual install locations so it
/// works inside Cargo build environments that may have a stripped `PATH`.
fn find_sfsym() -> Option<PathBuf> {
    if std::env::var_os("POLYSYM_NO_SFSYM").is_some() {
        return None;
    }

    let candidates = ["sfsym", "/usr/local/bin/sfsym", "/opt/homebrew/bin/sfsym", "/usr/bin/sfsym"];

    for candidate in candidates {
        if let Ok(output) = Command::new(candidate).arg("--version").output()
            && output.status.success()
        {
            return Some(PathBuf::from(candidate));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let local_bin = PathBuf::from(home).join(".local/bin/sfsym");
        if local_bin.exists() {
            return Some(local_bin);
        }
    }

    None
}

/// Build the stdin batch input for `sfsym batch` for one PNG appearance variant.
///
/// The sfsym `--size` is `spec.inner_size()` — smaller than the final canvas
/// to leave room for padding composited in afterwards.
fn build_png_batch_input(
    specs: &[SymbolSpec],
    symbol_dir: &Path,
    variant: &str,
    mode: &str,
    color: &str,
) -> String {
    let mut batch = String::new();
    for spec in specs {
        let out_path = symbol_dir.join(format!("{}.{}.png", spec.name, variant));
        writeln!(
            batch,
            "{} -f png --mode {} --size {} --weight {} --color '{}' -o {}",
            spec.name,
            mode,
            spec.inner_size(),
            spec.weight,
            color,
            out_path.display(),
        )
        .unwrap();
    }
    batch
}

/// Build the stdin batch input for `sfsym batch` for SVG export.
fn build_svg_batch_input(specs: &[SymbolSpec], symbol_dir: &Path) -> String {
    let mut batch = String::new();
    for spec in specs {
        let out_path = symbol_dir.join(format!("{}.svg", spec.name));
        writeln!(
            batch,
            "{} -f svg --mode monochrome --size {} --weight {} --color '#000000' -o {}",
            spec.name,
            spec.size,
            spec.weight,
            out_path.display(),
        )
        .unwrap();
    }
    batch
}

/// Query `sfsym info <name> --json` and parse the geometry we need.
///
/// Returns `None` on any failure so callers can fall back to the raw SVG
/// rather than panicking — the icon will still render, just without viewBox
/// normalization.
fn query_symbol_info(sfsym: &Path, name: &str) -> Option<SymbolInfo> {
    let output = Command::new(sfsym).args(["info", name, "--json"]).output().ok()?;

    if !output.status.success() {
        return None;
    }

    let json = std::str::from_utf8(&output.stdout).ok()?;

    let ar_section = section_after(json, "\"alignmentRect\"")?;
    let size_section = section_after(json, "\"size\"")?;

    let align_w = extract_json_f64(ar_section, "width", 0)?;
    let align_h = extract_json_f64(ar_section, "height", 0)?;
    let align_x = extract_json_f64(ar_section, "x", 0)?;
    let align_y = extract_json_f64(ar_section, "y", 0)?;
    let natural_width = extract_json_f64(size_section, "width", 0)?;
    let natural_height = extract_json_f64(size_section, "height", 0)?;

    Some(SymbolInfo { natural_width, natural_height, align_x, align_y, align_w, align_h })
}

/// Return the substring of `json` that starts immediately after the first
/// occurrence of `key`. Used to scope field extraction to a specific JSON
/// section, avoiding false matches in other sections.
fn section_after<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pos = json.find(key)?;
    Some(&json[pos + key.len()..])
}

/// Extract the Nth (0-based) occurrence of `"key" : <number>` from a JSON string.
fn extract_json_f64(json: &str, key: &str, occurrence: usize) -> Option<f64> {
    let needle = format!("\"{key}\" :");
    let mut remaining = json;
    let mut count = 0;
    loop {
        let pos = remaining.find(needle.as_str())?;
        remaining = &remaining[pos + needle.len()..];
        if count == occurrence {
            let trimmed = remaining.trim_start();
            let end = trimmed.find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')?;
            return trimmed[..end].parse().ok();
        }
        count += 1;
    }
}

/// Post-process a sfsym SVG:
/// 1. Replace hardcoded black fills with `currentColor` so the icon inherits
///    its color from the CSS `color` property.
/// 2. Strip the baked-in `width`/`height` attributes so CSS controls size.
/// 3. Rewrite `viewBox` to a tight square crop around the glyph's alignment
///    rect, normalizing optical size across symbols with different aspect ratios.
fn apply_svg_postprocess(path: &Path, spec: &SymbolSpec, info: Option<&SymbolInfo>) {
    let svg = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read SVG {}: {e}", path.display()));

    let updated = svg
        .replace("fill=\"#000000\"", "fill=\"currentColor\"")
        .replace("fill=\"#000\"", "fill=\"currentColor\"")
        .replace("fill='#000000'", "fill='currentColor'")
        .replace("fill='#000'", "fill='currentColor'")
        .replace("stroke=\"#000000\"", "stroke=\"currentColor\"")
        .replace("stroke=\"#000\"", "stroke=\"currentColor\"")
        .replace("stroke='#000000'", "stroke='currentColor'")
        .replace("stroke='#000'", "stroke='currentColor'");

    let updated = rewrite_svg_tag(&updated, spec, info);

    std::fs::write(path, updated)
        .unwrap_or_else(|e| panic!("failed to write SVG {}: {e}", path.display()));
}

/// Rewrite the opening `<svg ...>` tag: strip `width`/`height` and (when
/// `info` is provided) set a normalized `viewBox` derived from the glyph's
/// alignment rect.
///
/// If `info` is `None`, the original viewBox is preserved.
fn rewrite_svg_tag(svg: &str, spec: &SymbolSpec, info: Option<&SymbolInfo>) -> String {
    let svg_tag_start = svg.find("<svg").unwrap_or(0);
    let tag_end = svg[svg_tag_start..].find('>').map_or(svg.len(), |p| svg_tag_start + p);
    let rest = &svg[tag_end..];

    let viewbox = info.map(|info| {
        let canvas = f64::from(spec.size);

        let scale = (canvas / info.natural_width).min(canvas / info.natural_height);
        let rendered_w = info.natural_width * scale;
        let rendered_h = info.natural_height * scale;
        let off_x = (canvas - rendered_w) / 2.0;
        let off_y = (canvas - rendered_h) / 2.0;

        let ar_x = off_x + info.align_x * scale;
        let ar_y = off_y + info.align_y * scale;
        let ar_w = info.align_w * scale;
        let ar_h = info.align_h * scale;

        let cx = ar_x + ar_w / 2.0;
        let cy = ar_y + ar_h / 2.0;
        let pad = f64::from(spec.padding) * canvas;

        let half = f64::max(ar_h / 2.0 + pad, ar_w / 2.0);

        let vx = cx - half;
        let vy = cy - half;
        let vsize = half * 2.0;

        format!("{vx:.4} {vy:.4} {vsize:.4} {vsize:.4}")
    });

    let tag = &svg[..tag_end];
    let mut out = String::with_capacity(svg.len());
    let mut remaining = tag;

    while !remaining.is_empty() {
        let next = [" width=\"", " height=\"", " viewBox=\""]
            .iter()
            .filter_map(|prefix| remaining.find(prefix).map(|pos| (pos, *prefix)))
            .min_by_key(|(pos, _)| *pos);

        match next {
            None => {
                out.push_str(remaining);
                break;
            }
            Some((pos, prefix)) => {
                out.push_str(&remaining[..pos]);
                remaining = &remaining[pos + prefix.len()..];
                let original_viewbox = if prefix == " viewBox=\"" {
                    remaining.split('"').next().map(str::to_string)
                } else {
                    None
                };
                if let Some(close) = remaining.find('"') {
                    remaining = &remaining[close + 1..];
                }
                if prefix == " viewBox=\"" {
                    let vb = viewbox.as_deref().or(original_viewbox.as_deref()).unwrap_or("");
                    write!(out, " viewBox=\"{vb}\"").unwrap();
                }
            }
        }
    }

    out.push_str(rest);
    out
}

/// Run `sfsym batch` with the given stdin input. Returns `true` on success.
///
/// Emits a `cargo::warning` (not a panic) on failure so callers can decide
/// whether to fall through to tier-2 / tier-3 resolution.
fn run_batch(sfsym: &Path, batch_input: &str, variant: &str) -> bool {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = match Command::new(sfsym)
        .arg("batch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            println!("cargo::warning=polysym: failed to spawn sfsym for {variant} variant: {e}");
            return false;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(batch_input.as_bytes()) {
            println!("cargo::warning=polysym: failed to write to sfsym stdin ({variant}): {e}");
            return false;
        }
    } else {
        println!("cargo::warning=polysym: could not capture sfsym stdin for {variant}");
        return false;
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            println!("cargo::warning=polysym: sfsym batch ({variant}) did not complete: {e}");
            return false;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("cargo::warning=polysym: sfsym batch failed for {variant} variant: {stderr}");
        return false;
    }
    true
}

/// Convert an SF Symbol name to a valid Rust method name.
///
/// `folder.badge.plus` → `folder_badge_plus`
fn symbol_to_method_name(name: &str) -> String {
    name.replace(['.', '-'], "_")
}

/// Generate the Rust source file that consumers `include!` into their codebase.
///
/// `Missing` symbols are skipped entirely from the generated API (no typed
/// methods, no match arms). Consumers using `SfIcons::get(name)` /
/// `SfIcons::get_svg(name)` will receive `None` for them.
fn build_generated_source(
    resolutions: &[(SymbolSpec, SymbolResolution)],
    symbol_dir: &Path,
) -> String {
    let mut src = String::new();

    src.push_str("// Auto-generated by polysym-build. Do not edit.\n");
    src.push_str("#[allow(dead_code)]\n");
    src.push_str("pub struct SfIcons;\n\n");
    src.push_str("#[allow(dead_code)]\n");
    src.push_str("impl SfIcons {\n");

    let resolved: Vec<&SymbolSpec> = resolutions
        .iter()
        .filter_map(|(spec, res)| if *res == SymbolResolution::Missing { None } else { Some(spec) })
        .collect();

    for spec in &resolved {
        let method = symbol_to_method_name(&spec.name);
        let light_path = symbol_dir.join(format!("{}.light.png", spec.name));
        let dark_path = symbol_dir.join(format!("{}.dark.png", spec.name));
        let svg_path = symbol_dir.join(format!("{}.svg", spec.name));

        writeln!(src, "    pub fn {method}() -> ::polysym::SfImage {{").unwrap();
        writeln!(src, "        ::polysym::SfImage::new(").unwrap();
        writeln!(src, "            include_bytes!({:?}),", light_path.display()).unwrap();
        writeln!(src, "            include_bytes!({:?}),", dark_path.display()).unwrap();
        writeln!(src, "        )").unwrap();
        writeln!(src, "    }}\n").unwrap();

        writeln!(
            src,
            "    /// SVG for `{}` with `currentColor` fill — style via CSS `color`.",
            spec.name
        )
        .unwrap();
        writeln!(src, "    pub fn {method}_svg() -> &'static str {{").unwrap();
        writeln!(src, "        include_str!({:?})", svg_path.display()).unwrap();
        writeln!(src, "    }}\n").unwrap();
    }

    // PNG dynamic lookup
    src.push_str(
        "    /// Look up a symbol's PNG pair by SF Symbol name (e.g. `\"trash\"`).\n\
         /// Returns `None` if the name was not included in the build manifest\n\
         /// or had no available asset (sfsym, cache, or Lucide).\n\
         pub fn get(name: &str) -> Option<::polysym::SfImage> {\n\
             match name {\n",
    );
    for spec in &resolved {
        let method = symbol_to_method_name(&spec.name);
        writeln!(src, "            {:?} => Some(Self::{}()),", spec.name, method).unwrap();
    }
    src.push_str("            _ => None,\n        }\n    }\n\n");

    // SVG dynamic lookup
    src.push_str(
        "    /// Look up a symbol's SVG string by SF Symbol name (e.g. `\"trash\"`).\n\
         /// Returns `None` if the name was not included in the build manifest\n\
         /// or had no available asset (sfsym, cache, or Lucide).\n\
         /// The SVG uses `currentColor` fill — set the CSS `color` property to tint it.\n\
         pub fn get_svg(name: &str) -> Option<&'static str> {\n\
             match name {\n",
    );
    for spec in &resolved {
        let method = symbol_to_method_name(&spec.name);
        writeln!(src, "            {:?} => Some(Self::{}_svg()),", spec.name, method).unwrap();
    }
    src.push_str("            _ => None,\n        }\n    }\n");

    src.push_str("}\n");
    src
}
