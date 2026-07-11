// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl doctor` — report installed versions and flag path mismatches.

use mvl::mvl::stdlib;
use std::path::{Path, PathBuf};

const RUNTIME_VERSION: &str = env!("MVL_RUNTIME_VERSION");
const STDLIB_VERSION: &str = env!("MVL_STDLIB_VERSION");

pub fn run() {
    let compiler_ver = env!("CARGO_PKG_VERSION");
    let stdlib_path = stdlib::stdlib_path();
    let runtime_dir = mvl_runtime_dir();
    let llvm_dir = runtime_dir.join("llvm");

    let stdlib_ok = stdlib_path.join(".version").exists();
    let rust_ok = runtime_dir.join("rust").exists();
    let llvm_ok = llvm_dylib(&llvm_dir).is_some();

    println!("mvl doctor");
    println!();
    println!("  compiler  v{compiler_ver}  (this binary)");
    print_artifact("stdlib   ", STDLIB_VERSION, &stdlib_path, stdlib_ok);
    print_artifact(
        "runtime  ",
        RUNTIME_VERSION,
        &runtime_dir.join("rust"),
        rust_ok,
    );
    print_artifact("llvm-rt  ", RUNTIME_VERSION, &llvm_dir, llvm_ok);

    println!();
    if stdlib_ok && rust_ok && llvm_ok {
        println!("  All artifacts present.");
    } else {
        println!("  Some artifacts are missing — run `make install` (dev) or `mvl self install`.");
        std::process::exit(1);
    }
}

fn print_artifact(label: &str, version: &str, path: &Path, ok: bool) {
    let status = if ok { "✓" } else { "✗" };
    println!("  {status} {label} v{version}  {}", path.display());
}

fn mvl_runtime_dir() -> PathBuf {
    let base = if let Ok(home) = std::env::var("MVL_HOME") {
        PathBuf::from(home)
    } else {
        let xdg = std::env::var("XDG_DATA_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .unwrap_or_else(|| PathBuf::from("."));
        xdg.join("mvl")
    };
    base.join("runtime").join(RUNTIME_VERSION)
}

fn llvm_dylib(dir: &Path) -> Option<PathBuf> {
    for ext in &["dylib", "so"] {
        let p = dir.join(format!("libmvl_runtime_llvm.{ext}"));
        if p.exists() {
            return Some(p);
        }
    }
    None
}
