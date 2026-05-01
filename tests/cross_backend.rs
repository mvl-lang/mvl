//! Cross-backend regression tests: verify that the LLVM backend produces the
//! same stdout as the Rust transpiler backend for the same MVL programs.
//!
//! Tests are skipped automatically when `lli` is not installed.
//!
//! Corpus programs chosen for Phase A+B compatibility (no borrow/extern/impl):
//!   1. hello_world.mvl  — minimal fn main + println
//!   2. calculator.mvl   — total fns, if/else, arithmetic
//!   3. shapes.mvl       — enums, match dispatch, function composition

#![cfg(feature = "llvm")]

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_mvl is set at compile time and works correctly under
    // cargo test, cargo nextest, and cross-compiled builds.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mvl"))
}

fn corpus(name: &str) -> String {
    format!(
        "{}/tests/corpus/11_programs/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_types(name: &str) -> String {
    format!(
        "{}/tests/corpus/02_types/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Run a program via the Rust transpiler backend; return stdout.
fn run_transpiler(file: &str) -> String {
    let out = Command::new(mvl_bin())
        .args(["run", file])
        .output()
        .expect("failed to run mvl run");
    assert!(
        out.status.success(),
        "transpiler backend failed for {file}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // Strip transpiler progress lines ("Transpiled to: ...", "Running: ...").
    let raw = String::from_utf8_lossy(&out.stdout);
    raw.lines()
        .filter(|l| !l.starts_with("Transpiled to:") && !l.starts_with("Running:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Run a program via the LLVM backend; return stdout.
/// Returns `None` if `lli` is not available.
fn run_llvm(file: &str) -> Option<String> {
    // Skip silently if lli is not installed.
    if mvl::mvl::codegen::find_lli().is_none() {
        return None;
    }
    let out = Command::new(mvl_bin())
        .args(["run", file, "--backend=llvm"])
        .output()
        .expect("failed to run mvl run --backend=llvm");
    assert!(
        out.status.success(),
        "LLVM backend failed for {file}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    // Strip any backend progress lines that may appear on stdout, same as run_transpiler.
    let raw = String::from_utf8_lossy(&out.stdout);
    Some(
        raw.lines()
            .filter(|l| !l.starts_with("Transpiled to:") && !l.starts_with("Running:"))
            .map(|l| format!("{l}\n"))
            .collect(),
    )
}

/// Run a program via the LLVM backend and assert expected output.
/// Skips silently if `lli` is not available.
fn assert_llvm_output(file: &str, expected: &str) {
    if mvl::mvl::codegen::find_lli().is_none() {
        eprintln!("SKIP {file}: lli not found — install LLVM (brew install llvm)");
        return;
    }
    let out = Command::new(mvl_bin())
        .args(["run", file, "--backend=llvm"])
        .output()
        .expect("failed to run mvl run --backend=llvm");
    assert!(
        out.status.success(),
        "LLVM backend failed for {file}:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let actual = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "{file}: LLVM output mismatch.\nexpected: {expected:?}\nactual:   {actual:?}"
    );
}

/// Assert that both backends produce identical stdout for the given corpus program.
fn assert_backends_agree(name: &str) {
    let file = corpus(name);
    let transpiler_out = run_transpiler(&file);
    match run_llvm(&file) {
        None => {
            eprintln!("SKIP {name}: lli not found — install LLVM (brew install llvm)");
        }
        Some(llvm_out) => {
            assert_eq!(
                transpiler_out, llvm_out,
                "{name}: LLVM and transpiler backends produced different output.\n\
                 transpiler: {transpiler_out:?}\n\
                 llvm:       {llvm_out:?}"
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn cross_backend_hello_world() {
    assert_backends_agree("hello_world.mvl");
}

#[test]
fn cross_backend_calculator() {
    assert_backends_agree("calculator.mvl");
}

#[test]
fn cross_backend_shapes() {
    assert_backends_agree("shapes.mvl");
}

// ── Phase C: heap allocation tests (LLVM-only) ────────────────────────────────

#[test]
fn llvm_string_heap() {
    let file = corpus_types("string_heap_llvm.mvl");
    assert_llvm_output(&file, "5\nhello world\n11");
}

// ── L5-15: ownership-based drop (LLVM-only) ───────────────────────────────────

#[test]
fn llvm_move_string() {
    let file = corpus_types("move_string_llvm.mvl");
    assert_llvm_output(&file, "hello\nworld");
}

#[test]
fn llvm_fn_takes_string() {
    let file = corpus_types("fn_takes_string_llvm.mvl");
    assert_llvm_output(&file, "hello world");
}
