//! End-to-end integration tests: .mvl file → parse → check → transpile → cargo build → run → verify output.
//!
//! These tests exercise the complete compilation chain and are the primary
//! validation for Phase 1 correctness.

use std::process::Command;

/// Path to the compiled `mvl` binary (built by cargo before running tests).
fn mvl_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    // Strip the test binary name: target/debug/deps/compile_and_run-<hash>
    p.pop(); // file
    p.pop(); // deps/
    p.push("mvl");
    p
}

fn corpus(name: &str) -> String {
    format!(
        "{}/tests/corpus/09_full_programs/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

// ── check ─────────────────────────────────────────────────────────────────

fn run_check(file: &str) -> std::process::Output {
    Command::new(mvl_bin())
        .args(["check", file])
        .output()
        .expect("failed to run mvl check")
}

// ── run ───────────────────────────────────────────────────────────────────

fn run_mvl_run(file: &str) -> std::process::Output {
    Command::new(mvl_bin())
        .args(["run", file])
        .output()
        .expect("failed to run mvl run")
}

// ── simple_math.mvl ───────────────────────────────────────────────────────

/// simple_math.mvl has no fn main — `mvl check` must pass.
#[test]
fn simple_math_check_passes() {
    let out = run_check(&corpus("simple_math.mvl"));
    assert!(
        out.status.success(),
        "mvl check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK"), "expected OK, got: {stdout}");
}

// ── hello_mvl.mvl ─────────────────────────────────────────────────────────

/// hello_mvl.mvl has fn main — `mvl check` must pass.
#[test]
fn hello_mvl_check_passes() {
    let out = run_check(&corpus("hello_mvl.mvl"));
    assert!(
        out.status.success(),
        "mvl check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// hello_mvl.mvl runs and produces the expected output.
///
/// This is the primary end-to-end test:
///   .mvl → mvl check → mvl run → binary → verify stdout
#[test]
fn hello_mvl_runs_and_produces_expected_output() {
    let out = run_mvl_run(&corpus("hello_mvl.mvl"));
    assert!(
        out.status.success(),
        "mvl run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // The combined output of `mvl run` includes cargo's output followed by the
    // program's stdout (cargo run merges them on the same stream).
    let combined = String::from_utf8_lossy(&out.stdout);
    assert!(
        combined.contains("double(21) = 42"),
        "expected 'double(21) = 42' in output:\n{combined}"
    );
    assert!(
        combined.contains("safe_add(10, 32) = 42"),
        "expected 'safe_add(10, 32) = 42' in output:\n{combined}"
    );
    assert!(
        combined.contains("MathError: DivisionByZero"),
        "expected 'MathError: DivisionByZero' in output:\n{combined}"
    );
}
