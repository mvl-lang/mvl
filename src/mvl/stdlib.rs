// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Stdlib path resolution and installation.
//!
//! The stdlib is a set of `.mvl` source files installed on disk at
//! `$XDG_DATA_HOME/mvl/toolchains/{compiler_version}/std/`. It ships as a
//! separate release artifact (`mvl-stdlib-{stdlib_version}.tar.gz`) — the
//! compiler binary no longer embeds stdlib content (ADR-0009, #1765).
//!
//! Resolution order for the stdlib directory:
//!   1. `$MVL_HOME/toolchains/{compiler_version}/std/` if `MVL_HOME` is set
//!   2. `$XDG_DATA_HOME/mvl/toolchains/{compiler_version}/std/`
//!   3. `~/.local/share/mvl/toolchains/{compiler_version}/std/`
//!   4. Dev fallback: `<CARGO_MANIFEST_DIR>/std/` when running from a
//!      cargo build tree (allows `cargo run` without prior `make install`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// The stdlib version this compiler requires — baked in at compile time
/// via `build.rs` (independent from the compiler version).
pub const STDLIB_VERSION: &str = env!("MVL_STDLIB_VERSION");

/// The compiler version — used to locate the toolchain directory.
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// The XDG-resolved stdlib path (may not exist yet).
///
/// Path: `<mvl_data_home>/toolchains/{compiler_version}/std/` (ADR-0009).
pub fn stdlib_path() -> PathBuf {
    mvl_data_home()
        .join("toolchains")
        .join(COMPILER_VERSION)
        .join("std")
}

/// Dev fallback: `<CARGO_MANIFEST_DIR>/std/` when we're inside the source tree.
///
/// `CARGO_MANIFEST_DIR` is baked in at compile time. In production the
/// path won't exist and this returns `None`; during `cargo run` it points
/// at the checked-out repo's `std/` directory.
fn dev_stdlib_path() -> Option<PathBuf> {
    let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("std");
    if candidate.join("core.mvl").exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Resolve the active stdlib directory: XDG install if present, else dev
/// fallback. Returns `None` if neither is available.
pub fn resolved_stdlib_path() -> Option<PathBuf> {
    let xdg = stdlib_path();
    if xdg.join("core.mvl").exists() {
        return Some(xdg);
    }
    dev_stdlib_path()
}

/// Return the stdlib directory, exiting with a helpful error if not installed.
#[must_use]
pub fn ensure_stdlib() -> PathBuf {
    resolved_stdlib_path().unwrap_or_else(|| {
        eprintln!(
            "error: MVL stdlib v{STDLIB_VERSION} is not installed at {}",
            stdlib_path().display()
        );
        eprintln!("  Run `make install` (dev) or `mvl self install` (end-user).");
        eprintln!(
            "  Or set MVL_HOME to a directory containing toolchains/{COMPILER_VERSION}/std/."
        );
        process::exit(1);
    })
}

// ── Content accessors ───────────────────────────────────────────────────────

/// Read the source of a single stdlib file (relative path, e.g. `core.mvl` or
/// `kv/file.mvl`). Returns `None` if the file does not exist.
pub fn stdlib_content(filename: &str) -> Option<String> {
    let dir = resolved_stdlib_path()?;
    fs::read_to_string(dir.join(filename)).ok()
}

/// Enumerate every `.mvl` file under the resolved stdlib directory.
///
/// Returns `(relative_name, content)` pairs, e.g. `("kv/file.mvl", "...")`.
/// Returns an empty vec if the stdlib is not installed.
pub fn stdlib_files() -> Vec<(String, String)> {
    let Some(root) = resolved_stdlib_path() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_mvl_files(&root, "", &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_mvl_files(dir: &Path, rel_prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let new_prefix = if rel_prefix.is_empty() {
                name.to_string()
            } else {
                format!("{rel_prefix}/{name}")
            };
            collect_mvl_files(&path, &new_prefix, out);
        } else if path.extension().is_some_and(|x| x == "mvl") {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let rel_name = if rel_prefix.is_empty() {
                name.to_string()
            } else {
                format!("{rel_prefix}/{name}")
            };
            if let Ok(content) = fs::read_to_string(&path) {
                out.push((rel_name, content));
            }
        }
    }
}

// ── Installation ─────────────────────────────────────────────────────────────

/// Download and extract the stdlib tarball into
/// `<mvl_data_home>/toolchains/{compiler_version}/std/`.
///
/// Tarball layout (relative paths inside the archive):
///   `std/…`  — every `.mvl` file, mirrored from the repo's `std/` tree
///
/// The asset name is keyed on the compiler version rather than the stdlib
/// version so that a single-step install works when the caller does not
/// (and cannot) know the old release's stdlib version — matching how
/// `install_runtime` is keyed. Skipped if the target directory already
/// contains `core.mvl` (idempotent).
pub fn install_stdlib(compiler_version: &str) {
    let dest = mvl_data_home()
        .join("toolchains")
        .join(compiler_version)
        .join("std");

    if dest.join("core.mvl").exists() {
        println!(
            "MVL stdlib for v{compiler_version} is already installed at {}",
            dest.display()
        );
        return;
    }

    let url = format!(
        "https://github.com/LAB271/mvl_language/releases/download/v{compiler_version}/mvl-stdlib-{compiler_version}.tar.gz"
    );

    println!("Downloading MVL stdlib for v{compiler_version}...");
    println!("  from: {url}");

    let response = ureq::get(&url).call().unwrap_or_else(|e| {
        eprintln!("error: stdlib download failed: {e}");
        eprintln!("  URL: {url}");
        process::exit(1);
    });

    if response.status() != 200 {
        eprintln!(
            "error: stdlib download returned HTTP {} for {url}",
            response.status()
        );
        process::exit(1);
    }

    // Extract into the parent so the `std/` prefix inside the tarball lands
    // at the expected `toolchains/{ver}/std/` path.
    let extract_to = dest.parent().expect("stdlib dest must have parent");
    std::fs::create_dir_all(extract_to).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {e}", extract_to.display());
        process::exit(1);
    });

    let gz = flate2::read::GzDecoder::new(response.into_reader());
    let mut archive = tar::Archive::new(gz);
    archive.unpack(extract_to).unwrap_or_else(|e| {
        eprintln!("error: failed to extract stdlib tarball: {e}");
        process::exit(1);
    });

    println!(
        "Installed MVL stdlib for v{compiler_version} to {}",
        dest.display()
    );
}
