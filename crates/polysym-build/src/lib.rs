//! Build-time helper for generating SF Symbol icon assets.
//!
//! Add this to your app's `build.rs`:
//!
//! ```rust,no_run
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

/// Configuration for a single SF Symbol export.
#[derive(Debug, Clone)]
pub struct SymbolSpec {
    /// SF Symbol name, e.g. `"folder.badge.plus"`.
    pub name: String,
    /// Point size for the icon canvas. Defaults to `16` (produces 32×32 px at 2x).
    pub size: u32,
    /// Symbol weight. Defaults to `"regular"`.
    pub weight: String,
}

impl SymbolSpec {
    /// Create a spec with default settings (16pt, regular weight).
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), size: 16, weight: "regular".into() }
    }

    /// Override the point size. PNG output will be `size * 2` pixels square (2x density).
    pub fn size(mut self, size: u32) -> Self {
        self.size = size;
        self
    }

    /// Override the symbol weight (ultralight, thin, light, regular, medium, semibold, bold, heavy, black).
    pub fn weight(mut self, weight: impl Into<String>) -> Self {
        self.weight = weight.into();
        self
    }
}

impl<S: Into<String>> From<S> for SymbolSpec {
    fn from(name: S) -> Self {
        Self::new(name)
    }
}

/// Generate SF Symbol PNG assets for a list of symbol names using default settings.
///
/// Symbols are exported at 16pt (32×32 px at 2x), regular weight, in both
/// light (black) and dark (white) variants. Call this from your app's `build.rs`
/// before `tauri_build::build()`.
pub fn generate(symbols: &[&str]) {
    let specs: Vec<SymbolSpec> = symbols.iter().map(|&s| SymbolSpec::new(s)).collect();
    generate_with_opts(&specs);
}

/// Generate SF Symbol PNG assets with full control over each symbol's options.
pub fn generate_with_opts(specs: &[SymbolSpec]) {
    let sfsym = find_sfsym();

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let symbol_dir = out_dir.join("polysym");
    std::fs::create_dir_all(&symbol_dir).expect("failed to create polysym output dir");

    // Emit rerun directives so cargo reruns this script when the build.rs changes.
    println!("cargo::rerun-if-changed=build.rs");

    let light_batch = build_batch_input(specs, &symbol_dir, "light", "#000000");
    let dark_batch = build_batch_input(specs, &symbol_dir, "dark", "#ffffff");

    run_batch(&sfsym, &light_batch, "light");
    run_batch(&sfsym, &dark_batch, "dark");

    let generated_path = out_dir.join("polysym_generated.rs");
    let generated_source = build_generated_source(specs, &symbol_dir);
    std::fs::write(&generated_path, generated_source)
        .expect("failed to write polysym_generated.rs");
}

/// Locate the `sfsym` binary, searching common install locations.
fn find_sfsym() -> PathBuf {
    // Try PATH first, then common Homebrew and manual install locations.
    let candidates = ["sfsym", "/usr/local/bin/sfsym", "/opt/homebrew/bin/sfsym", "/usr/bin/sfsym"];

    for candidate in candidates {
        if let Ok(output) = Command::new(candidate).arg("--version").output() {
            if output.status.success() {
                return PathBuf::from(candidate);
            }
        }
    }

    // Also check ~/.local/bin (install-from-source default)
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

/// Build the stdin batch input for `sfsym batch` for one appearance variant.
fn build_batch_input(
    specs: &[SymbolSpec],
    symbol_dir: &Path,
    variant: &str,
    color: &str,
) -> String {
    let mut batch = String::new();
    for spec in specs {
        let out_path = symbol_dir.join(format!("{}.{}.png", spec.name, variant));
        writeln!(
            batch,
            "{} -f png --size {} --weight {} --color '{}' -o {}",
            spec.name,
            spec.size,
            spec.weight,
            color,
            out_path.display(),
        )
        .unwrap();
    }
    batch
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

    // One typed method per symbol.
    for spec in specs {
        let method = symbol_to_method_name(&spec.name);
        let light_path = symbol_dir.join(format!("{}.light.png", spec.name));
        let dark_path = symbol_dir.join(format!("{}.dark.png", spec.name));

        writeln!(
            src,
            "    pub fn {method}() -> ::polysym::SfImage {{\n\
                     ::polysym::SfImage::new(\n\
                         include_bytes!({light_path:?}),\n\
                         include_bytes!({dark_path:?}),\n\
                     )\n\
                 }}\n"
        )
        .unwrap();
    }

    // Dynamic lookup by SF Symbol name string — useful for Tauri commands.
    src.push_str(
        "    /// Look up a symbol by its SF Symbol name (e.g. `\"trash\"`).\n\
         /// Returns `None` if the name was not included in the build manifest.\n\
         pub fn get(name: &str) -> Option<::polysym::SfImage> {\n\
             match name {\n",
    );
    for spec in specs {
        let method = symbol_to_method_name(&spec.name);
        writeln!(src, "            {:?} => Some(Self::{}()),", spec.name, method).unwrap();
    }
    src.push_str("            _ => None,\n        }\n    }\n");

    src.push_str("}\n");
    src
}
