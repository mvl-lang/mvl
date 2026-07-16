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
    let wasm_dir = runtime_dir.join("wasm");

    let stdlib_ok = stdlib_path.join(".version").exists();
    let rust_ok = runtime_dir.join("rust").exists();
    let llvm_ok = llvm_dylib(&llvm_dir).is_some();
    let wasm_ok = wasm_dir.join("mvl_runtime_wasm.wasm").exists();

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
    print_artifact("wasm-rt  ", RUNTIME_VERSION, &wasm_dir, wasm_ok);

    println!();
    // wasm-rt is optional today (WASM backend still under active development,
    // #1817). Compiler/stdlib/rust/llvm together are the core release surface;
    // wasm-rt missing warns but doesn't fail. Once the WASM backend reaches
    // parity, promote it into the required set.
    let required_ok = stdlib_ok && rust_ok && llvm_ok;
    if required_ok && wasm_ok {
        println!("  All artifacts present.");
    } else if required_ok {
        println!("  Core artifacts present; wasm-rt missing (run: make install).");
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
