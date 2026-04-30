//! End-to-end integration tests: .mvl file → parse → check → transpile → cargo build → run → verify output.
//!
//! These tests exercise the complete compilation chain and are the primary
//! validation for Phase 1 correctness.
//!
//! Corpus programs (in order of complexity):
//!   1. hello_world.mvl    — minimal: fn main + println
//!   2. hello_mvl.mvl      — ADTs, total fns, enum match
//!   3. calculator.mvl     — total fns, if/else expressions, arithmetic
//!   4. shapes.mvl         — two enums, multiple match functions, composition
//!   5. struct_value_semantics.mvl — struct value semantics, Clone-on-pass
//!   6. safe_division.mvl  — Result<T,E>, match on Result, IFC labels (Req 5)
//!   7. linked_list.mvl    — recursive enum (Box<T>), deref, total fn recursion
//!   8. examples/log_analyzer — multi-file, bridge.rs, IFC labels end-to-end

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
        "{}/tests/corpus/11_programs/{name}",
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

/// Build a corpus program and assert the build succeeds.
fn assert_build_ok(name: &str) {
    let out = run_mvl_build(&corpus(name));
    assert!(
        out.status.success(),
        "{name}: mvl build must succeed;\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
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

// ── 5. struct_value_semantics.mvl ─────────────────────────────────────────

#[test]
fn struct_value_semantics_check_passes() {
    assert_check_ok("struct_value_semantics.mvl");
}

/// Struct passed to multiple functions — value semantics via Clone.
///
/// Expected stdout:
///   1, 2
///   4, 6
#[test]
fn struct_value_semantics_runs_and_produces_expected_output() {
    assert_run_output("struct_value_semantics.mvl", &["1, 2", "4, 6"]);
}

// ── 6. safe_division.mvl ──────────────────────────────────────────────────

#[test]
fn safe_division_check_passes() {
    let stdout = assert_check_ok("safe_division.mvl");
    assert!(
        stdout.contains("OK"),
        "expected 'OK' in check output:\n{stdout}"
    );
}

/// Result<T,E>, match on Result, division-by-zero handling (Req 5 end-to-end).
///
/// Note: `to_nonzero` is a stub that always returns Ok, so the Err branches in main
/// are unreachable at runtime. This test covers the Ok path and match exhaustiveness.
/// The output is "25" (not "25.0") because the transpiler renders trailing-zero floats
/// without the decimal point.
///
/// Expected stdout:
///   100 / 4 = 25
#[test]
fn safe_division_runs_and_produces_expected_output() {
    assert_run_output("safe_division.mvl", &["100 / 4 = 25"]);
}

// ── 7. linked_list.mvl ────────────────────────────────────────────────────

#[test]
fn linked_list_check_passes() {
    assert_check_ok("linked_list.mvl");
}

/// Recursive enum (linked list) — Box<T> payload, pattern-match deref, total fn.
///
/// Expected stdout:
///   length: 3
#[test]
fn linked_list_runs_and_produces_expected_output() {
    assert_run_output("linked_list.mvl", &["length: 3"]);
}

// ── simple_math.mvl (library — no fn main) ────────────────────────────────

/// simple_math.mvl has no fn main — `mvl check` must pass.
#[test]
fn simple_math_check_passes() {
    let stdout = assert_check_ok("simple_math.mvl");
    assert!(stdout.contains("OK"));
}

// ── auth_handler.mvl (library — no fn main) ───────────────────────────────

/// auth_handler.mvl demonstrates all 11 requirements (ADTs, IFC labels,
/// effect annotations, refinement types, Result/Option, ownership).
/// It has no fn main — `mvl check` validates the full type-checking pipeline
/// including Tainted/Secret/Public label flow and the `! Console` effect annotation.
/// Part of the incremental path to #175.
#[test]
fn auth_handler_check_passes() {
    let stdout = assert_check_ok("auth_handler.mvl");
    assert!(
        stdout.contains("OK"),
        "expected 'OK' in check output:\n{stdout}"
    );
}

// ── 8. examples/log_analyzer (multi-file, bridge.rs) ─────────────────────

/// Multi-file example: log_analyzer uses main.mvl + parser.mvl + utils.mvl
/// with a Rust bridge (bridge.rs). `mvl build` must succeed end-to-end.
///
/// We pass the *directory* (not main.mvl) so the crate name is "log_analyzer",
/// avoiding the `/tmp/mvl_build_main` collision with bridge tests.
///
/// Issue #195: multi-file builds need CI coverage.
#[test]
fn log_analyzer_build_succeeds() {
    let path = format!("{}/examples/log_analyzer", env!("CARGO_MANIFEST_DIR"));
    let out = run_mvl_build(&path);
    assert!(
        out.status.success(),
        "mvl build must succeed for examples/log_analyzer;\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Multi-file example: log_analyzer run produces a JSON summary.
///
/// logs.jsonl is gitignored (*.jsonl), so we generate a small inline fixture.
/// 4 entries: 1 error, 1 warn, 2 info → expected: {"count":4,"errors":1,"warnings":1,"infos":2}
#[test]
fn log_analyzer_run_produces_json_summary() {
    use std::io::Write;
    // Pass the directory so crate_name = "log_analyzer" (not "main").
    let log_analyzer_dir = format!("{}/examples/log_analyzer", env!("CARGO_MANIFEST_DIR"));
    // Write a small JSONL fixture to a temp file.
    let mut tmp = tempfile::NamedTempFile::new().expect("create temp file");
    writeln!(
        tmp,
        r#"{{"level":"error","message":"disk full","timestamp":1000}}"#
    )
    .unwrap();
    writeln!(
        tmp,
        r#"{{"level":"warn","message":"retrying","timestamp":1001}}"#
    )
    .unwrap();
    writeln!(
        tmp,
        r#"{{"level":"info","message":"started","timestamp":1002}}"#
    )
    .unwrap();
    writeln!(
        tmp,
        r#"{{"level":"info","message":"ready","timestamp":1003}}"#
    )
    .unwrap();
    let logs_path = tmp.path().to_string_lossy().to_string();
    let out = Command::new(mvl_bin())
        .args(["run", &log_analyzer_dir, "--", "--file", &logs_path])
        .output()
        .expect("failed to run mvl run for log_analyzer");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"count\":4"),
        "expected JSON summary with \"count\":4, got:\n{stdout}"
    );
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

/// bridge.rs that is a symlink pointing outside the source directory MUST be
/// rejected with a non-zero exit and an actionable error message.
///
/// Spec 006 path-hardening: symlink-escape guard (canonicalize + starts_with).
#[cfg(unix)]
#[test]
fn bridge_symlink_outside_source_dir_rejected() {
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    let outer = tempdir().expect("outer tempdir");
    let project = tempdir().expect("project tempdir");

    // Real bridge.rs lives outside the project directory.
    let real_bridge = outer.path().join("real_bridge.rs");
    std::fs::write(
        &real_bridge,
        "#[no_mangle] pub extern \"Rust\" fn foo() -> i64 { 1 }\n",
    )
    .expect("write real_bridge.rs");

    // Symlink bridge.rs inside the project pointing outside.
    let bridge_link = project.path().join("bridge.rs");
    symlink(&real_bridge, &bridge_link).expect("create symlink");

    // Minimal MVL program with extern "rust".
    let mvl_src = project.path().join("main.mvl");
    std::fs::write(
        &mvl_src,
        "extern \"rust\" { fn foo() -> Int; }\nfn main() -> Unit ! Console { println(\"x\"); }\n",
    )
    .expect("write main.mvl");

    let out = run_mvl_build(&mvl_src.display().to_string());
    assert!(
        !out.status.success(),
        "mvl build must exit non-zero when bridge.rs is a symlink outside the source dir; \
         stderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("outside source directory"),
        "error must mention 'outside source directory', got:\n{stderr}"
    );
}

// ── random_dice (! Random effect + extern bridge) ─────────────────────────

/// random_dice/ declares `extern "rust" { fn roll_dice() -> Int; }` and
/// annotates `main` with `! Random, Console`. Validates the ! Random effect
/// annotation + extern bridge end-to-end: build, link, and run all pass.
///
/// The bridge uses std::time for seeding — no external crates.
/// Output value is non-deterministic; only the prefix "rolled: " is asserted.
///
/// Acceptance: `mvl run` exits 0 and stdout contains "rolled: ".
#[test]
fn random_dice_runs_and_prints_dice_roll() {
    assert_run_output("random_dice", &["rolled: "]);
}

// ── else_if_chain.mvl (regression #197) ───────────────────────────────────

/// Regression for #197: `else if` must emit `} else if cond {` on one line.
#[test]
fn else_if_chain_check_passes() {
    let stdout = assert_check_ok("else_if_chain.mvl");
    assert!(stdout.contains("OK"));
}

/// else-if chain: classify positive, negative, zero.
///
/// Expected stdout:
///   classify(5) = positive
///   classify(-3) = negative
///   classify(0) = zero
#[test]
fn else_if_chain_runs_and_produces_expected_output() {
    assert_run_output(
        "else_if_chain.mvl",
        &[
            "classify(5) = positive",
            "classify(-3) = negative",
            "classify(0) = zero",
        ],
    );
}

// ── Phase 4 gate tests (issue #229) ───────────────────────────────────────

fn corpus_stdlib(name: &str) -> String {
    format!(
        "{}/tests/corpus/03_stdlib/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Phase 4 gate: stdlib range() is transpiled from MVL source, not hardcoded.
///
/// Expected stdout:
///   5
#[test]
fn range_pipeline_runs_and_produces_expected_output() {
    let out = Command::new(mvl_bin())
        .args(["run", &corpus_stdlib("range_pipeline.mvl")])
        .output()
        .expect("failed to run mvl run");
    assert!(
        out.status.success(),
        "range_pipeline: mvl run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l.trim() == "5"),
        "range_pipeline: expected a line containing '5', got:\n{stdout}"
    );
}

/// Phase 4 gate: all 9 core types compile and run.
#[test]
fn core_types_demo_check_passes() {
    let out = Command::new(mvl_bin())
        .args(["check", &corpus("core_types_demo.mvl")])
        .output()
        .expect("failed to run mvl check");
    assert!(
        out.status.success(),
        "core_types_demo: mvl check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn core_types_demo_runs_and_produces_expected_output() {
    assert_run_output(
        "core_types_demo.mvl",
        &[
            "Int: abs=5 min=3 max=7",
            "Float: ceil=4 floor=3 sqrt=2",
            "String: len=5",
            "List: len=5 first=1",
            "Map: len=2",
            "Set: has_two=true len=3",
            "Option: got 42",
            "Result: value=42",
        ],
    );
}

// ── unix.mvl — Unix process lifecycle and environment (#45) ───────────────

fn corpus_basics(name: &str) -> String {
    format!(
        "{}/tests/corpus/01_basics/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Issue #45: process.spawn/wait/kill, env.get/set/all, signal_on.
/// Validates that the process and env stdlib modules are accepted by the
/// type-checker with correct effect annotations and Tainted labels.
#[test]
fn unix_process_lifecycle_check_passes() {
    let out = Command::new(mvl_bin())
        .args(["check", &corpus_basics("unix.mvl")])
        .output()
        .expect("failed to run mvl check");
    assert!(
        out.status.success(),
        "unix: mvl check failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── println_non_string_first_arg.mvl (regression #198) ────────────────────

/// Regression for #198: println with a non-string first arg must generate
/// valid Rust with one `{}` placeholder per argument.
///
/// Covers: two-arg (String+Int), single Int, two Ints, three args (Int+Int+Str).
#[test]
fn println_non_string_first_arg_check_passes() {
    assert_check_ok("println_non_string_first_arg.mvl");
}

#[test]
fn println_non_string_first_arg_runs() {
    assert_run_output(
        "println_non_string_first_arg.mvl",
        &[
            "hello 42",     // println(msg, x)        — String var + Int
            "42",           // println(x)             — single Int arg
            "42 100",       // println(x, y)          — two Int args
            "42 100 hello", // println(x, y, msg)   — three args
        ],
    );
}
