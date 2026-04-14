//! End-to-end integration tests: .mvl file → parse → check → transpile → cargo build → run → verify output.
//!
//! These tests exercise the complete compilation chain and are the primary
//! validation for Phase 1 correctness.
//!
//! Corpus programs (in order of complexity):
//!   1. hello_world.mvl  — minimal: fn main + println
//!   2. hello_mvl.mvl    — ADTs, total fns, enum match
//!   3. calculator.mvl   — total fns, if/else expressions, arithmetic
//!   4. shapes.mvl       — two enums, multiple match functions, composition

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

// ── helpers ───────────────────────────────────────────────────────────────

fn run_check(file: &str) -> std::process::Output {
    Command::new(mvl_bin())
        .args(["check", file])
        .output()
        .expect("failed to run mvl check")
}

fn run_mvl_run(file: &str) -> std::process::Output {
    Command::new(mvl_bin())
        .args(["run", file])
        .output()
        .expect("failed to run mvl run")
}

fn run_mvl_build(path: &str) -> std::process::Output {
    Command::new(mvl_bin())
        .args(["build", path])
        .output()
        .expect("failed to run mvl build")
}

/// Assert check passes and return combined stdout.
fn assert_check_ok(name: &str) -> String {
    let out = run_check(&corpus(name));
    assert!(
        out.status.success(),
        "{name}: mvl check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run a corpus program and assert each expected line appears in stdout.
fn assert_run_output(name: &str, expected_lines: &[&str]) {
    let out = run_mvl_run(&corpus(name));
    assert!(
        out.status.success(),
        "{name}: mvl run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let combined = String::from_utf8_lossy(&out.stdout);
    for line in expected_lines {
        assert!(
            combined.contains(line),
            "{name}: expected '{line}' in output:\n{combined}"
        );
    }
}

// ── 1. hello_world.mvl ────────────────────────────────────────────────────

#[test]
fn hello_world_check_passes() {
    let stdout = assert_check_ok("hello_world.mvl");
    assert!(stdout.contains("OK"));
}

/// Simplest possible MVL program: fn main + println.
///
/// Expected stdout:
///   Hello, world!
#[test]
fn hello_world_runs_and_produces_expected_output() {
    assert_run_output("hello_world.mvl", &["Hello, world!"]);
}

// ── 2. hello_mvl.mvl ──────────────────────────────────────────────────────

#[test]
fn hello_mvl_check_passes() {
    assert_check_ok("hello_mvl.mvl");
}

/// ADTs, total fns, enum match.
///
/// Expected stdout:
///   double(21) = 42
///   safe_add(10, 32) = 42
///   MathError: DivisionByZero
#[test]
fn hello_mvl_runs_and_produces_expected_output() {
    assert_run_output(
        "hello_mvl.mvl",
        &[
            "double(21) = 42",
            "safe_add(10, 32) = 42",
            "MathError: DivisionByZero",
        ],
    );
}

// ── 3. calculator.mvl ─────────────────────────────────────────────────────

#[test]
fn calculator_check_passes() {
    assert_check_ok("calculator.mvl");
}

/// Total functions and if/else expressions.
///
/// Expected stdout:
///   10 + 5 = 15
///   10 - 5 = 5
///   4 * 7 = 28
///   max(17, 42) = 42
#[test]
fn calculator_runs_and_produces_expected_output() {
    assert_run_output(
        "calculator.mvl",
        &[
            "10 + 5 = 15",
            "10 - 5 = 5",
            "4 * 7 = 28",
            "max(17, 42) = 42",
        ],
    );
}

// ── 4. shapes.mvl ─────────────────────────────────────────────────────────

#[test]
fn shapes_check_passes() {
    assert_check_ok("shapes.mvl");
}

/// Two enums, multiple match functions, function composition.
///
/// Expected stdout:
///   circle has 0 sides and is curved
///   rectangle has 4 sides and is flat
///   triangle has 3 sides and is flat
#[test]
fn shapes_runs_and_produces_expected_output() {
    assert_run_output(
        "shapes.mvl",
        &[
            "circle has 0 sides and is curved",
            "rectangle has 4 sides and is flat",
            "triangle has 3 sides and is flat",
        ],
    );
}

// ── simple_math.mvl (library — no fn main) ────────────────────────────────

/// simple_math.mvl has no fn main — `mvl check` must pass.
#[test]
fn simple_math_check_passes() {
    let stdout = assert_check_ok("simple_math.mvl");
    assert!(stdout.contains("OK"));
}

// ── bridge.rs convention (Spec 006) ───────────────────────────────────────

/// password_checker.mvl declares `extern "rust"` but ships with no bridge.rs.
/// It is an intentional negative fixture: `mvl build` MUST exit non-zero and
/// emit a clear error containing "bridge.rs not found" and `extern "rust"`.
///
/// Spec 006 Requirement 3: Missing Bridge Error [MUST].
#[test]
fn missing_bridge_exits_nonzero_with_actionable_error() {
    let out = run_mvl_build(&corpus("password_checker.mvl"));
    assert!(
        !out.status.success(),
        "mvl build must exit non-zero when bridge.rs is absent; \
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bridge.rs not found"),
        "error must mention 'bridge.rs not found', got:\n{stderr}"
    );
    assert!(
        stderr.contains("extern \"rust\""),
        "error must mention 'extern \"rust\"', got:\n{stderr}"
    );
}

/// bridge_ok/ contains a minimal `extern "rust"` program with a valid bridge.rs.
/// `mvl build` MUST succeed (exit 0).
///
/// Spec 006 Requirement 1 (Bridge Discovery) and Requirement 2 (Bridge Injection).
#[test]
fn build_succeeds_with_valid_bridge() {
    let out = run_mvl_build(&corpus("bridge_ok"));
    assert!(
        out.status.success(),
        "mvl build must succeed with a valid bridge.rs; \
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
