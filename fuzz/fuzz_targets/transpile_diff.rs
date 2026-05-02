//! Phase 3 fuzz target: differential fuzzing — Rust transpiler vs LLVM backend.
//!
//! Each libFuzzer iteration:
//!   1. Generate a terminating MVL program with a main that prints Int results.
//!   2. Write it to a temp file.
//!   3. Run `mvl run <file>` (Rust transpiler) → capture stdout.
//!   4. Run `mvl run <file> --backend=llvm` → capture stdout.
//!   5. If both succeed: assert stdout is identical.
//!
//! Throughput is ~100-500 iter/sec (subprocess per iteration) — much lower than
//! Phases 1/2, but this is the only technique that catches silent miscompilation
//! where neither backend panics but they produce different output.
//!
//! Prerequisites: `make build` must be run before `make fuzz-diff` so the `mvl`
//! binary exists in `target/debug/mvl` (or set MVL_BIN env var).

#![no_main]

use libfuzzer_sys::fuzz_target;
use mvl::mvl::codegen::find_lli;
use mvl_fuzz::generator::Generator;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::NamedTempFile;

fn find_mvl() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MVL_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // CARGO_MANIFEST_DIR is the fuzz/ crate; workspace root is one level up.
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    for profile in &["debug", "release"] {
        let p = workspace.join("target").join(profile).join("mvl");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn run_backend(mvl: &Path, src: &Path, extra_args: &[&str]) -> Option<String> {
    let out = Command::new(mvl)
        .arg("run")
        .arg(src)
        .args(extra_args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // Strip mvl progress lines ("Transpiled to: ...", "Running: ...").
    let raw = String::from_utf8_lossy(&out.stdout);
    Some(
        raw.lines()
            .filter(|l| !l.starts_with("Transpiled to:") && !l.starts_with("Running:"))
            .map(|l| format!("{l}\n"))
            .collect(),
    )
}

fuzz_target!(|data: &[u8]| {
    // Require both `mvl` binary and `lli` — skip silently if either is absent.
    let Some(mvl) = find_mvl() else { return };
    if find_lli().is_none() {
        return;
    }

    let mut gen = Generator::new(data);
    let Ok(src) = gen.gen_diff_program() else {
        return;
    };

    // Write generated source to a temp file.
    let Ok(mut tmp) = NamedTempFile::with_suffix(".mvl") else {
        return;
    };
    if tmp.write_all(src.as_bytes()).is_err() {
        return;
    }
    let path = tmp.path().to_path_buf();

    let rust_out = run_backend(&mvl, &path, &[]);
    let llvm_out = run_backend(&mvl, &path, &["--backend=llvm"]);

    // Both backends compiled and ran — outputs must be identical.
    if let (Some(r), Some(l)) = (rust_out, llvm_out) {
        assert_eq!(
            r,
            l,
            "backend divergence!\n\
             === source ===\n{src}\n\
             === rust  ===\n{r}\
             === llvm  ===\n{l}"
        );
    }
    // If either backend failed (type error, unsupported construct, etc.) that's
    // not a differential bug — Phase 1/2 will catch panics.
});
