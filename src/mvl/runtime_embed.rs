// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Runtime embedding, extraction, and path resolution.
//!
//! All `runtime/rust` and `runtime/rust-tokio` source files are embedded in
//! the binary at compile time via `include_str!`. On first run (or when the
//! version stamp mismatches), they are extracted to:
//!   `$XDG_DATA_HOME/mvl/toolchains/{version}/runtime/{rust,rust-tokio}/`
//!
//! This makes the installed `mvl` binary independent of the source-tree path
//! it was built from (fixes the `CARGO_MANIFEST_DIR` bake-in problem).
//!
//! Override: `MVL_HOME` replaces all XDG paths (useful for CI and testing).

use std::fs;
use std::path::{Path, PathBuf};

pub const RUNTIME_RUST_FILES: &[(&str, &str)] =
    include!(concat!(env!("OUT_DIR"), "/runtime_rust_files.rs"));

pub const RUNTIME_TOKIO_FILES: &[(&str, &str)] =
    include!(concat!(env!("OUT_DIR"), "/runtime_tokio_files.rs"));

/// The runtime version — tied to the compiler version so they stay in sync.
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

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

fn runtime_base() -> PathBuf {
    mvl_data_home()
        .join("toolchains")
        .join(RUNTIME_VERSION)
        .join("runtime")
}

pub fn runtime_rust_path() -> PathBuf {
    runtime_base().join("rust")
}

pub fn runtime_tokio_path() -> PathBuf {
    runtime_base().join("rust-tokio")
}

// ── Extraction ───────────────────────────────────────────────────────────────

/// Ensure `runtime/rust` is extracted to XDG and return its path.
#[must_use]
pub fn ensure_runtime_rust() -> PathBuf {
    let target = runtime_rust_path();
    if needs_extraction(&target, RUNTIME_RUST_FILES) {
        extract(&target, RUNTIME_RUST_FILES);
    }
    target
}

/// Ensure `runtime/rust-tokio` is extracted to XDG and return its path.
#[must_use]
pub fn ensure_runtime_tokio() -> PathBuf {
    let target = runtime_tokio_path();
    if needs_extraction(&target, RUNTIME_TOKIO_FILES) {
        extract(&target, RUNTIME_TOKIO_FILES);
    }
    target
}

fn needs_extraction(target: &Path, files: &[(&str, &str)]) -> bool {
    let stamp = target.join(".version");
    match fs::read_to_string(&stamp) {
        Ok(v) => {
            if v.trim() != RUNTIME_VERSION {
                return true;
            }
            files.iter().any(|(name, content)| {
                match fs::read_to_string(target.join(name)) {
                    Ok(disk) => disk != *content,
                    Err(_) => true,
                }
            })
        }
        Err(_) => true,
    }
}

fn extract(target: &Path, files: &[(&str, &str)]) {
    if let Err(e) = fs::create_dir_all(target) {
        eprintln!(
            "mvl: warning: could not create runtime dir {}: {e}",
            target.display()
        );
        return;
    }
    for (name, content) in files {
        let dest = target.join(name);
        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!(
                    "mvl: warning: could not create runtime subdir {}: {e}",
                    parent.display()
                );
                return;
            }
        }
        if let Err(e) = fs::write(&dest, content) {
            eprintln!("mvl: warning: could not write runtime file {name}: {e}");
            return;
        }
    }
    if let Err(e) = fs::write(target.join(".version"), RUNTIME_VERSION) {
        eprintln!("mvl: warning: could not write runtime version stamp: {e}");
        return;
    }
    eprintln!(
        "mvl: installed runtime v{RUNTIME_VERSION} to {}",
        target.display()
    );
}
