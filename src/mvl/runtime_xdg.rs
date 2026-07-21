// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! XDG path resolution for the MVL runtime artifact.
//!
//! The runtime is a separate release artifact (`mvl-runtime-{version}.tar.gz`)
//! downloaded by `mvl self install` and extracted to:
//!   `$XDG_DATA_HOME/mvl/runtime/{version}/`
//!
//! Override: `MVL_HOME` replaces the XDG base (useful for CI and dev).

use std::path::{Path, PathBuf};
use std::process;

/// The runtime version this compiler requires — baked in at compile time.
pub const RUNTIME_VERSION: &str = env!("MVL_RUNTIME_VERSION");

// ── Path resolution ─────────────────────────────────────────────────────────

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

/// Root of the installed runtime: `<mvl_data_home>/runtime/{version}/`.
pub fn runtime_root() -> PathBuf {
    mvl_data_home().join("runtime").join(RUNTIME_VERSION)
}

/// Path to the default (non-tokio) runtime crate.
pub fn runtime_rust_path() -> PathBuf {
    runtime_root().join("rust")
}

/// Path to the tokio runtime crate.
pub fn runtime_tokio_path() -> PathBuf {
    runtime_root().join("rust-tokio")
}

// ── Ensure helpers ───────────────────────────────────────────────────────────

/// Return the `runtime/rust` path, exiting with a helpful error if not installed.
#[must_use]
pub fn ensure_runtime_rust() -> PathBuf {
    let path = runtime_rust_path();
    if !path.join("Cargo.toml").exists() {
        missing_runtime_error();
    }
    path
}

/// Return the `runtime/rust-tokio` path, exiting with a helpful error if not installed.
#[must_use]
pub fn ensure_runtime_tokio() -> PathBuf {
    let path = runtime_tokio_path();
    if !path.join("Cargo.toml").exists() {
        missing_runtime_error();
    }
    path
}

fn missing_runtime_error() -> ! {
    eprintln!(
        "error: MVL runtime v{RUNTIME_VERSION} is not installed at {}",
        runtime_root().display()
    );
    eprintln!("  Run `mvl self install` to download and install it.");
    eprintln!("  Or set MVL_HOME to a directory containing runtime/{RUNTIME_VERSION}/.");
    process::exit(1);
}

// ── Installation ─────────────────────────────────────────────────────────────

/// Download and extract the runtime tarball for `version` from GitHub releases.
///
/// Tarball layout (relative paths inside the archive):
///   `core/`        — shared pure-algorithm crate (dep of `rust/` and `rust-tokio/`)
///   `rust/`        — default runtime crate
///   `rust-tokio/`  — tokio runtime crate
///
/// Extracted to `<runtime_root>/` so the final layout is:
///   `<mvl_data_home>/runtime/{version}/core/`
///   `<mvl_data_home>/runtime/{version}/rust/`
///   `<mvl_data_home>/runtime/{version}/rust-tokio/`
pub fn install_runtime(version: &str) {
    let dest = mvl_data_home().join("runtime").join(version);

    if dest.join("rust").join("Cargo.toml").exists() {
        println!(
            "MVL runtime {version} is already installed at {}",
            dest.display()
        );
        return;
    }

    let url = format!(
        "https://github.com/LAB271/mvl_language/releases/download/v{version}/mvl-runtime-{version}.tar.gz"
    );

    println!("Downloading MVL runtime {version}...");
    println!("  from: {url}");

    let response = ureq::get(&url).call().unwrap_or_else(|e| {
        eprintln!("error: runtime download failed: {e}");
        eprintln!("  URL: {url}");
        process::exit(1);
    });

    if response.status() != 200 {
        eprintln!(
            "error: runtime download returned HTTP {} for {url}",
            response.status()
        );
        process::exit(1);
    }

    std::fs::create_dir_all(&dest).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {e}", dest.display());
        process::exit(1);
    });

    extract_tarball(response.into_reader(), &dest);

    println!("Installed MVL runtime {version} to {}", dest.display());
}

fn extract_tarball(reader: impl std::io::Read, dest: &Path) {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest).unwrap_or_else(|e| {
        eprintln!("error: failed to extract runtime tarball: {e}");
        process::exit(1);
    });
}
