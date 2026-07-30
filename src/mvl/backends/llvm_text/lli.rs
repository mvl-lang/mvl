// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Pure-Rust utilities for finding `lli`, the MVL C runtime library, and
//! parsing `// expect:` test annotations from `.mvl` source files.
//!
//! No inkwell / C FFI dependency — usable from any module regardless of the
//! `llvm` feature flag.

use std::path::{Path, PathBuf};

const RUNTIME_VERSION: &str = env!("MVL_RUNTIME_VERSION");

// ── lli discovery ─────────────────────────────────────────────────────────────

/// Locate the `lli` interpreter on this machine.
///
/// Search order:
/// 1. `which lli` (PATH)
/// 2. Homebrew keg-only ARM path: `/opt/homebrew/opt/llvm/bin/lli`
/// 3. Homebrew keg-only Intel path: `/usr/local/opt/llvm/bin/lli`
pub fn find_lli() -> Option<PathBuf> {
    if let Some(p) = which_lli() {
        return Some(p);
    }
    for prefix in &[
        "/opt/homebrew/opt/llvm/bin/lli",
        "/usr/local/opt/llvm/bin/lli",
    ] {
        let p = PathBuf::from(prefix);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn which_lli() -> Option<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("lli")
        .output()
        .ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

// ── mvl_runtime_llvm library discovery ───────────────────────────────────────

/// Locate `libmvl_runtime_llvm` for `lli --load=<lib>` (ADR-0018).
///
/// Search order:
/// 1. `MVL_RUNTIME_LLVM_LIB` env var (must end in `.dylib` or `.so`)
/// 2. XDG runtime dir: `<mvl_data_home>/runtime/{RUNTIME_VERSION}/llvm/`
/// 3. Sibling of the current executable (resolved, not the symlink path):
///    `target/{profile}/libmvl_runtime_llvm.{dylib,so}`
/// 4. Cargo cdylib output: `target/{profile}/deps/libmvl_runtime_llvm.{dylib,so}`
pub fn find_mvl_runtime_llvm_lib() -> Option<PathBuf> {
    find_cdylib("MVL_RUNTIME_LLVM_LIB", "libmvl_runtime_llvm")
}

// ── mvl_runtime_wasm module discovery ────────────────────────────────────────

/// Locate `mvl_runtime_wasm.wasm` for `wasmtime run --preload runtime=<path>` (#1819).
///
/// Search order:
/// 1. `MVL_RUNTIME_WASM` env var (must end in `.wasm` and exist)
/// 2. XDG runtime dir: `<mvl_data_home>/runtime/{RUNTIME_VERSION}/wasm/mvl_runtime_wasm.wasm`
/// 3. Sibling of the current executable's `target/{profile}/` dir, resolved to
///    `target/wasm32-wasip1/{debug,release}/mvl_runtime_wasm.wasm`
pub fn find_mvl_runtime_wasm_lib() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MVL_RUNTIME_WASM") {
        let p = PathBuf::from(&path);
        if p.extension().and_then(|e| e.to_str()) == Some("wasm") && p.exists() {
            return Some(p);
        }
        if !path.is_empty() {
            eprintln!("warning: MVL_RUNTIME_WASM ignored — must end in .wasm and exist: {path}");
        }
    }
    let xdg_wasm = mvl_data_home()
        .join("runtime")
        .join(RUNTIME_VERSION)
        .join("wasm")
        .join("mvl_runtime_wasm.wasm");
    if xdg_wasm.exists() {
        return Some(xdg_wasm);
    }
    let exe = std::env::current_exe().ok()?;
    find_wasm_sibling(&exe.canonicalize().unwrap_or(exe))
}

/// Given a resolved `mvl` executable path (`.../target/{profile}/mvl`), look
/// for the wasm runtime built by the sibling `wasm32-wasip1` cargo target.
/// Split out from `find_mvl_runtime_wasm_lib` so the search can be unit-tested
/// against a fabricated directory tree instead of the test binary's own
/// `current_exe()` (which lives under `target/{profile}/deps/`, not
/// `target/{profile}/`, and would give a false negative here).
fn find_wasm_sibling(resolved_exe: &Path) -> Option<PathBuf> {
    let target_dir = resolved_exe.parent()?.parent()?;
    for profile in &["debug", "release"] {
        let lib = target_dir
            .join("wasm32-wasip1")
            .join(profile)
            .join("mvl_runtime_wasm.wasm");
        if lib.exists() {
            return Some(lib);
        }
    }
    None
}

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

fn find_cdylib(env_var: &str, lib_name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(env_var) {
        let p = PathBuf::from(&path);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "dylib" | "so") && p.exists() {
            return Some(p);
        }
        // An empty value means "no override" — the Makefile sets this from a
        // wildcard that expands to nothing before the runtime is built. Only
        // warn about values that look like a real, wrong path.
        if !path.is_empty() {
            eprintln!("warning: {env_var} ignored — must end in .dylib or .so and exist: {path}");
        }
    }
    // XDG runtime dir — the canonical installed location (ADR-0009, #1765).
    let xdg_llvm = mvl_data_home()
        .join("runtime")
        .join(RUNTIME_VERSION)
        .join("llvm");
    for ext in &["dylib", "so"] {
        let lib = xdg_llvm.join(format!("{lib_name}.{ext}"));
        if lib.exists() {
            return Some(lib);
        }
    }
    // Sibling of the resolved executable — covers dev builds where the binary
    // lives in target/{profile}/ and the dylib is next to it.  Canonicalize
    // so that a ~/.local/bin/mvl symlink resolves to the actual toolchain dir.
    if let Ok(exe) = std::env::current_exe() {
        let resolved = exe.canonicalize().unwrap_or(exe);
        if let Some(dir) = resolved.parent() {
            for ext in &["dylib", "so"] {
                for suffix in &["", "deps/"] {
                    let lib = dir.join(format!("{suffix}{lib_name}.{ext}"));
                    if lib.exists() {
                        return Some(lib);
                    }
                }
            }
        }
    }
    None
}

// ── expect annotation parsing ─────────────────────────────────────────────────

/// Parse `// expect: <line>` or `// Expected stdout:` block from MVL source.
/// Returns expected stdout joined with newlines, or `None` if absent.
pub fn parse_expect_annotation(source: &str) -> Option<String> {
    let single: Vec<String> = source
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("// expect:")
                .map(|s| s.trim().to_string())
        })
        .collect();
    if !single.is_empty() {
        return Some(single.join("\n"));
    }
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "// Expected stdout:" {
            let mut collected: Vec<String> = Vec::new();
            for following in lines.by_ref() {
                let t = following.trim();
                if let Some(rest) = t.strip_prefix("//") {
                    collected.push(rest.trim_start_matches(' ').to_string());
                } else {
                    break;
                }
            }
            if !collected.is_empty() {
                return Some(collected.join("\n"));
            }
        }
    }
    None
}

/// Parse `// expect-pattern: <glob>` annotation (for non-deterministic output).
pub fn parse_expect_pattern_annotation(source: &str) -> Option<String> {
    source.lines().find_map(|l| {
        l.trim()
            .strip_prefix("// expect-pattern:")
            .map(|s| s.trim().to_string())
    })
}

/// Simple glob match: `?` = any single char, `*` = any sequence.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn inner(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (None, _) => false,
            (Some('*'), _) => inner(&p[1..], t) || (!t.is_empty() && inner(p, &t[1..])),
            (_, None) => false,
            (Some('?'), _) => inner(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) => pc == tc && inner(&p[1..], &t[1..]),
        }
    }
    inner(&p, &t)
}

#[cfg(test)]
mod tests {
    use super::find_wasm_sibling;
    use std::fs;

    #[test]
    fn find_wasm_sibling_locates_debug_profile_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        let target_dir = tmp.path().join("target");
        let wasm_dir = target_dir.join("wasm32-wasip1").join("debug");
        fs::create_dir_all(&wasm_dir).unwrap();
        let runtime = wasm_dir.join("mvl_runtime_wasm.wasm");
        fs::write(&runtime, b"").unwrap();

        let exe = target_dir.join("debug").join("mvl");
        assert_eq!(find_wasm_sibling(&exe), Some(runtime));
    }

    #[test]
    fn find_wasm_sibling_none_when_runtime_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("target").join("debug").join("mvl");
        assert_eq!(find_wasm_sibling(&exe), None);
    }
}
