//! Build-time helper for generating SF Symbol icon assets.
//!
//! Add this to your app's `build.rs`:
//!
//! ```rust,ignore
//! fn main() {
//!     polysym_build::generate(&[
//!         "trash",
//!         "folder.badge.plus",
//!         "bell.slash",
//!     ]);
//!     tauri_build::build();
//! }
//! ```
//!
//! This requires `sfsym` to be installed: `brew install yapstudios/tap/sfsym`

use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};
use std::process::Command;

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
}

impl SymbolSpec {
    /// Create a spec with default settings (20pt, regular weight, 12% padding).
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), size: 20, weight: "regular".into(), padding: 0.12 }
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

/// Generate SF Symbol PNG assets for a list of symbol names using default settings.
///
/// Symbols are exported at 20pt (40×40 px at 2x) with 12% padding per side,
/// in both light (black) and dark (white) variants. Call this from your app's
/// `build.rs` before `tauri_build::build()`.
pub fn generate(symbols: &[&str]) {
    let specs: Vec<SymbolSpec> = symbols.iter().map(|&s| SymbolSpec::new(s)).collect();
    generate_with_opts(&specs);
}

/// Generate SF Symbol PNG and SVG assets with full control over each symbol's options.
pub fn generate_with_opts(specs: &[SymbolSpec]) {
    let sfsym = find_sfsym();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let symbol_dir = out_dir.join("polysym");
    std::fs::create_dir_all(&symbol_dir).expect("failed to create polysym output dir");

    println!("cargo::rerun-if-changed=build.rs");

    // PNG — light: monochrome black; dark: hierarchical white.
    // (monochrome mode in sfsym ignores --color and always outputs black;
    // hierarchical respects it, so we use it for the white dark variant.)
    let light_batch = build_png_batch_input(specs, &symbol_dir, "light", "monochrome", "#000000");
    let dark_batch = build_png_batch_input(specs, &symbol_dir, "dark", "hierarchical", "#ffffff");

    run_batch(&sfsym, &light_batch, "light PNG");
    run_batch(&sfsym, &dark_batch, "dark PNG");

    // Post-process PNGs: composite each symbol into a padded canvas with @2x DPI metadata.
    for spec in specs {
        for variant in ["light", "dark"] {
            let path = symbol_dir.join(format!("{}.{}.png", spec.name, variant));
            apply_padding(&path, spec);
        }
    }

    // SVG — monochrome black, then post-process for currentColor + normalized viewBox.
    // A single variant is sufficient: callers style it via the CSS `color` property.
    let svg_batch = build_svg_batch_input(specs, &symbol_dir);
    run_batch(&sfsym, &svg_batch, "SVG");

    for spec in specs {
        let path = symbol_dir.join(format!("{}.svg", spec.name));
        let info = query_symbol_info(&sfsym, &spec.name);
        apply_svg_postprocess(&path, spec, info.as_ref());
    }

    let generated_path = out_dir.join("polysym_generated.rs");
    let generated_source = build_generated_source(specs, &symbol_dir);
    std::fs::write(&generated_path, generated_source)
        .expect("failed to write polysym_generated.rs");
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
    let inner_px = spec.inner_size() * 2; // sfsym exports at 2x

    // No compositing needed — sfsym's output already has correct @2x pHYs.
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

    // Write with the raw png encoder so we can inject the pHYs chunk.
    // image::save() would produce a plain 72 DPI PNG, stripping the @2x metadata.
    let file = std::fs::File::create(path)
        .unwrap_or_else(|e| panic!("failed to open {} for writing: {e}", path.display()));

    let mut enc = png::Encoder::new(BufWriter::new(file), canvas_px, canvas_px);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);

    let mut writer =
        enc.write_header().unwrap_or_else(|e| panic!("PNG header for {}: {e}", path.display()));

    // pHYs chunk: 5669 pixels/meter ≈ 144 DPI = @2x Retina.
    // This makes NSImage interpret canvas_px pixels as canvas_px/2 logical points.
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

/// Locate the `sfsym` binary, searching common install locations.
fn find_sfsym() -> PathBuf {
    let candidates = ["sfsym", "/usr/local/bin/sfsym", "/opt/homebrew/bin/sfsym", "/usr/bin/sfsym"];

    for candidate in candidates {
        if let Ok(output) = Command::new(candidate).arg("--version").output() {
            if output.status.success() {
                return PathBuf::from(candidate);
            }
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let local_bin = PathBuf::from(home).join(".local/bin/sfsym");
        if local_bin.exists() {
            return local_bin;
        }
    }

    println!(
        "cargo::error=`sfsym` was not found. \
        Install it with: brew install yapstudios/tap/sfsym\n\
        Or from source: https://github.com/yapstudios/sfsym"
    );
    panic!("`sfsym` not found — see cargo output for install instructions");
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
///
/// Uses monochrome black at the full canvas size (no padding reduction needed —
/// SVGs are vector and callers control size via CSS). Color is replaced with
/// `currentColor` in a post-processing step.
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

    // Minimal JSON parse — avoid pulling in serde just for build-time use.
    // The output is compact and predictable; we extract four numeric fields.
    let json = std::str::from_utf8(&output.stdout).ok()?;

    let natural_width = extract_json_f64(json, "width", 0)?;
    let natural_height = extract_json_f64(json, "height", 0)?;
    // alignmentRect fields — find the second occurrence of each key since
    // "width" and "height" also appear in the top-level "size" object.
    let align_w = extract_json_f64(json, "width", 1)?;
    let align_h = extract_json_f64(json, "height", 1)?;
    let align_x = extract_json_f64(json, "x", 0)?;
    let align_y = extract_json_f64(json, "y", 0)?;

    Some(SymbolInfo { natural_width, natural_height, align_x, align_y, align_w, align_h })
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
            // Skip whitespace, then parse the number.
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
///
/// Step 3 is the key one: sfsym always produces a square canvas, but symbols
/// like `book` are naturally wide and short, so their glyph only fills a
/// fraction of the canvas height. The tight viewBox makes every symbol appear
/// at consistent visual weight when displayed at the same CSS size.
fn apply_svg_postprocess(path: &Path, spec: &SymbolSpec, info: Option<&SymbolInfo>) {
    let svg = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read SVG {}: {e}", path.display()));

    // Step 1: currentColor.
    let updated = svg
        .replace("fill=\"#000000\"", "fill=\"currentColor\"")
        .replace("fill=\"#000\"", "fill=\"currentColor\"")
        .replace("fill='#000000'", "fill='currentColor'")
        .replace("fill='#000'", "fill='currentColor'")
        .replace("stroke=\"#000000\"", "stroke=\"currentColor\"")
        .replace("stroke=\"#000\"", "stroke=\"currentColor\"")
        .replace("stroke='#000000'", "stroke='currentColor'")
        .replace("stroke='#000'", "stroke='currentColor'");

    // Steps 2 + 3: rewrite the <svg> opening tag.
    let updated = rewrite_svg_tag(&updated, spec, info);

    std::fs::write(path, updated)
        .unwrap_or_else(|e| panic!("failed to write SVG {}: {e}", path.display()));
}

/// Rewrite the opening `<svg ...>` tag: strip `width`/`height` and set a
/// normalized `viewBox` derived from the glyph's alignment rect.
///
/// If `info` is `None` (sfsym info failed), falls back to just stripping the
/// dimension attributes and leaving the original viewBox intact.
fn rewrite_svg_tag(svg: &str, spec: &SymbolSpec, info: Option<&SymbolInfo>) -> String {
    // Skip past any XML declaration (<?xml...?>) to find the <svg opening tag,
    // then find its closing >. Using svg.find('>') alone would stop at the ?>
    // in the XML declaration, causing the entire rewrite to silently no-op.
    let svg_tag_start = svg.find("<svg").unwrap_or(0);
    let tag_end = svg[svg_tag_start..].find('>').map(|p| svg_tag_start + p).unwrap_or(svg.len());
    let rest = &svg[tag_end..];

    // Build the new viewBox value from alignment rect geometry.
    let viewbox = info.map(|info| {
        let canvas = f64::from(spec.size);

        // sfsym scales the symbol to fit the square canvas, preserving aspect
        // ratio and centering it. Reproduce that transform to get the
        // alignmentRect in canvas coordinate space.
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

        // SF Symbols use the alignment rect HEIGHT as the optical size axis
        // (analogous to cap-height in typography). Size the square viewBox by
        // ar_h + padding so that all symbols normalize by height, giving
        // visually consistent sizes in horizontal layouts.
        //
        // For unusually wide symbols (ar_w/2 > ar_h/2 + pad), fall back to
        // width-driven sizing so the glyph is never clipped at the sides.
        let half = f64::max(ar_h / 2.0 + pad, ar_w / 2.0);

        let vx = cx - half;
        let vy = cy - half;
        let vsize = half * 2.0;

        format!("{vx:.4} {vy:.4} {vsize:.4} {vsize:.4}")
    });

    // Reconstruct the tag: copy everything except width/height/viewBox attrs.
    let tag = &svg[..tag_end];
    let mut out = String::with_capacity(svg.len());
    let mut remaining = tag;

    while !remaining.is_empty() {
        // Find the next attribute we want to drop or replace.
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
                // For viewBox: capture the original value before advancing,
                // so the fallback can re-emit it if sfsym info wasn't available.
                let original_viewbox = if prefix == " viewBox=\"" {
                    remaining.split('"').next().map(str::to_string)
                } else {
                    None
                };
                // Skip past the closing quote of this attribute's value.
                if let Some(close) = remaining.find('"') {
                    remaining = &remaining[close + 1..];
                }
                // Re-inject viewBox with our computed (or original) value.
                // width and height are silently dropped.
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

/// Run `sfsym batch` with the given stdin input, panicking on failure.
fn run_batch(sfsym: &Path, batch_input: &str, variant: &str) {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(sfsym)
        .arg("batch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn sfsym for {variant} variant: {e}"));

    child
        .stdin
        .take()
        .expect("sfsym stdin not captured")
        .write_all(batch_input.as_bytes())
        .expect("failed to write to sfsym stdin");

    let output = child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("sfsym batch ({variant}) did not complete: {e}"));

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("sfsym batch failed for {variant} variant:\n{stderr}");
    }
}

/// Convert an SF Symbol name to a valid Rust method name.
///
/// `folder.badge.plus` → `folder_badge_plus`
fn symbol_to_method_name(name: &str) -> String {
    name.replace(['.', '-'], "_")
}

/// Generate the Rust source file that consumers `include!` into their codebase.
fn build_generated_source(specs: &[SymbolSpec], symbol_dir: &Path) -> String {
    let mut src = String::new();

    src.push_str("// Auto-generated by polysym-build. Do not edit.\n");
    src.push_str("#[allow(dead_code)]\n");
    src.push_str("pub struct SfIcons;\n\n");
    src.push_str("#[allow(dead_code)]\n");
    src.push_str("impl SfIcons {\n");

    for spec in specs {
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
         /// Returns `None` if the name was not included in the build manifest.\n\
         pub fn get(name: &str) -> Option<::polysym::SfImage> {\n\
             match name {\n",
    );
    for spec in specs {
        let method = symbol_to_method_name(&spec.name);
        writeln!(src, "            {:?} => Some(Self::{}()),", spec.name, method).unwrap();
    }
    src.push_str("            _ => None,\n        }\n    }\n\n");

    // SVG dynamic lookup
    src.push_str(
        "    /// Look up a symbol's SVG string by SF Symbol name (e.g. `\"trash\"`).\n\
         /// Returns `None` if the name was not included in the build manifest.\n\
         /// The SVG uses `currentColor` fill — set the CSS `color` property to tint it.\n\
         pub fn get_svg(name: &str) -> Option<&'static str> {\n\
             match name {\n",
    );
    for spec in specs {
        let method = symbol_to_method_name(&spec.name);
        writeln!(src, "            {:?} => Some(Self::{}_svg()),", spec.name, method).unwrap();
    }
    src.push_str("            _ => None,\n        }\n    }\n");

    src.push_str("}\n");
    src
}
