//! Stdlib embedding, extraction, and path resolution.
//!
//! All `.mvl` stdlib source files are embedded in the binary at compile time
//! via `include_str!`. On first run (or when the version stamp mismatches),
//! they are extracted to `$XDG_DATA_HOME/mvl/std/`.
//!
//! Override: `MVL_HOME` replaces all XDG paths (useful for CI and testing).

use std::fs;
use std::path::{Path, PathBuf};

// ── Embedded files ──────────────────────────────────────────────────────────

/// All stdlib `.mvl` source files embedded at compile time.
pub const STDLIB_FILES: &[(&str, &str)] = &[("core.mvl", include_str!("../../../std/core.mvl"))];

/// The stdlib version — tied to the compiler version so they stay in sync.
pub const STDLIB_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Path resolution ─────────────────────────────────────────────────────────

/// Returns `$MVL_HOME`, `$XDG_DATA_HOME/mvl`, or `~/.local/share/mvl`.
fn mvl_data_home() -> PathBuf {
    if let Ok(home) = std::env::var("MVL_HOME") {
        return PathBuf::from(home);
    }
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("mvl")
}

/// Returns the resolved stdlib directory path (may not exist yet).
pub fn stdlib_path() -> PathBuf {
    mvl_data_home().join("std")
}

// ── Extraction ───────────────────────────────────────────────────────────────

/// Ensure the stdlib is extracted to the XDG path and return its location.
///
/// Extracts if the directory is missing or the `.version` stamp doesn't match
/// the current compiler version. Prints a one-line notice to stderr on
/// (re-)installation.
#[must_use]
pub fn ensure_stdlib() -> PathBuf {
    let target = stdlib_path();
    if needs_extraction(&target) {
        extract(&target);
    }
    target
}

fn needs_extraction(target: &Path) -> bool {
    let stamp = target.join(".version");
    match fs::read_to_string(&stamp) {
        Ok(v) => v.trim() != STDLIB_VERSION,
        Err(_) => true,
    }
}

fn extract(target: &Path) {
    if let Err(e) = fs::create_dir_all(target) {
        eprintln!(
            "mvl: warning: could not create stdlib dir {}: {e}",
            target.display()
        );
        return;
    }
    for (name, content) in STDLIB_FILES {
        if let Err(e) = fs::write(target.join(name), content) {
            eprintln!("mvl: warning: could not write stdlib file {name}: {e}");
            return;
        }
    }
    if let Err(e) = fs::write(target.join(".version"), STDLIB_VERSION) {
        eprintln!("mvl: warning: could not write stdlib version stamp: {e}");
        return;
    }
    eprintln!(
        "mvl: installed stdlib v{STDLIB_VERSION} to {}",
        target.display()
    );
}
