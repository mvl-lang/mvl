//! Cross-backend regression tests: verify that the LLVM backend produces the
//! same stdout as the Rust transpiler backend for the same MVL programs.
//!
//! Tests are skipped automatically when `lli` is not installed.
//!
//! Corpus programs chosen for Phase A+B compatibility (no borrow/extern/impl):
//!   1. hello_world.mvl  — minimal fn main + println
//!   2. calculator.mvl   — total fns, if/else, arithmetic
//!   3. shapes.mvl       — enums, match dispatch, function composition
//!
//! ADR-0019 (C-ABI stdlib) parity tests:
//!   4. env_basic.mvl    — getuid + getgid via libmvl_runtime_c
//!                         also serves as the cdylib load smoke test (#431 AC):
//!                         proves libmvl_runtime_c loads and symbols resolve via lli

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

fn corpus_effects(name: &str) -> String {
    format!(
        "{}/tests/corpus/05_effects/{name}",
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

// ── ADR-0019: C-ABI stdlib parity tests ──────────────────────────────────────

/// Both backends call `getuid()` and `getgid()` and must produce identical output.
/// Both ultimately call the same POSIX syscalls, so UID and GID are the same.
#[test]
fn cross_backend_env_basic() {
    let file = corpus_effects("env_basic.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "env_basic.mvl: LLVM and transpiler backends must produce identical output"
        );
        // Sanity: output is two non-empty lines (uid and gid as integers).
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 2, "expected two lines (uid, gid)");
        assert!(
            lines[0].parse::<i64>().is_ok(),
            "first line must be an integer (uid): {:?}",
            lines[0]
        );
        assert!(
            lines[1].parse::<i64>().is_ok(),
            "second line must be an integer (gid): {:?}",
            lines[1]
        );
    }
}

/// Both backends call `getuid()` — result must be non-negative.
#[test]
fn cross_backend_env_getuid_nonnegative() {
    let file = corpus_effects("env_basic.mvl");
    if let Some(out) = run_llvm(&file) {
        let uid: i64 = out.lines().next().unwrap_or("0").parse().unwrap_or(-1);
        assert!(
            uid >= 0,
            "LLVM backend: getuid() must be non-negative, got {uid}"
        );
    }
}

/// Both backends call `getgid()` — result must be non-negative.
#[test]
fn cross_backend_env_getgid_nonnegative() {
    let file = corpus_effects("env_basic.mvl");
    if let Some(out) = run_llvm(&file) {
        let lines: Vec<&str> = out.lines().collect();
        let gid: i64 = lines.get(1).unwrap_or(&"0").parse().unwrap_or(-1);
        assert!(
            gid >= 0,
            "LLVM backend: getgid() must be non-negative, got {gid}"
        );
    }
}

// ── #433: time + random C-ABI parity tests ───────────────────────────────────

/// `random.int(n, n)` with a single-element range is deterministic.
/// Both backends must produce identical output: "42\n0\n".
#[test]
fn cross_backend_random_int() {
    let file = corpus_effects("random_int.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "random_int.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 2, "expected two lines");
        assert_eq!(lines[0], "42", "int(42,42) must return 42");
        assert_eq!(lines[1], "0", "int(0,0) must return 0");
    }
}

/// `random.float()` returns a value in [0.0, 1.0). Non-deterministic; only
/// validates output shape, not exact equality across backends.
#[test]
fn cross_backend_random_float_shape() {
    // Build a small inline program rather than a corpus file since the value
    // is non-deterministic.  We only check that the LLVM backend produces a
    // valid float in range.
    let tmp = tempfile::NamedTempFile::with_suffix(".mvl").expect("tempfile");
    std::fs::write(
        tmp.path(),
        "use std.random.{float}\nfn main() -> Unit ! Random {\n  let v: Float = float();\n  println(\"{}\", v);\n}\n",
    )
    .expect("write");
    if let Some(out) = run_llvm(&tmp.path().to_string_lossy()) {
        let v: f64 = out
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("LLVM random.float output must be a float, got: {out:?}"));
        assert!((0.0..1.0).contains(&v), "random.float out of range: {v}");
    }
}

// ── #434: log C-ABI parity tests ─────────────────────────────────────────────

/// Both backends must emit identical log records to stderr.
///
/// Checks that `_mvl_log_*` wrappers produce the same `[LEVEL TIMESTAMP] msg field=value`
/// format as the Rust-path implementation, including deterministic field sort order.
#[test]
fn cross_backend_log_stderr() {
    let file = corpus_effects("log_output.mvl");

    // Always assert the transpiler path regardless of LLVM availability.
    let transpiler = Command::new(mvl_bin())
        .args(["run", &file])
        .output()
        .expect("failed to run mvl run (transpiler)");
    assert!(
        transpiler.status.success(),
        "transpiler failed:\n{}",
        String::from_utf8_lossy(&transpiler.stderr)
    );
    let t_stderr = String::from_utf8_lossy(&transpiler.stderr);
    for level in &["[DEBUG ", "[INFO ", "[WARN ", "[ERROR "] {
        assert!(
            t_stderr.contains(level),
            "transpiler stderr missing {level}:\n{t_stderr}"
        );
    }

    if mvl::mvl::codegen::find_lli().is_none() {
        eprintln!("SKIP cross_backend_log_stderr LLVM half: lli not found");
        return;
    }

    let llvm = Command::new(mvl_bin())
        .args(["run", &file, "--backend=llvm"])
        .output()
        .expect("failed to run mvl run --backend=llvm");
    assert!(
        llvm.status.success(),
        "LLVM backend failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&llvm.stdout),
        String::from_utf8_lossy(&llvm.stderr)
    );

    let l_stderr = String::from_utf8_lossy(&llvm.stderr);

    // Both backends must emit all four level tags.
    for level in &["[DEBUG ", "[INFO ", "[WARN ", "[ERROR "] {
        assert!(
            l_stderr.contains(level),
            "LLVM stderr missing {level}:\n{l_stderr}"
        );
    }

    // ISO 8601 shape: T separator and Z UTC suffix.
    assert!(l_stderr.contains('T'), "LLVM stderr: missing ISO 8601 T");
    assert!(l_stderr.contains('Z'), "LLVM stderr: missing ISO 8601 Z");

    // Field key=value pairs.
    assert!(l_stderr.contains("v=1"), "LLVM stderr: missing v=1");
    assert!(
        l_stderr.contains("port=8080"),
        "LLVM stderr: missing port=8080"
    );

    // Sorted field order on the ordering line.
    let ordering = l_stderr
        .lines()
        .find(|l| l.contains("ordering"))
        .expect("LLVM stderr: no line containing 'ordering'");
    let a = ordering.find("a=first").expect("a=first not found");
    let m = ordering.find("m=mid").expect("m=mid not found");
    let z = ordering.find("z=last").expect("z=last not found");
    assert!(a < m && m < z, "LLVM fields not sorted: {ordering}");

    // Both backends must emit the same number of log lines.
    let log_lines = |s: &str| -> usize {
        s.lines()
            .filter(|l| {
                l.contains("[DEBUG ")
                    || l.contains("[INFO ")
                    || l.contains("[WARN ")
                    || l.contains("[ERROR ")
            })
            .count()
    };
    assert_eq!(
        log_lines(&t_stderr),
        log_lines(&l_stderr),
        "transpiler emitted {} log lines, LLVM emitted {}",
        log_lines(&t_stderr),
        log_lines(&l_stderr),
    );
}

// ── #417: io stdlib (Rust transpiler path) ────────────────────────────────────

/// Write+read roundtrip, path queries, append, create_dir, remove.
/// Transpiler-only until #435 (LLVM C-ABI exports for io).
/// LLVM-pending: #435
#[test]
fn transpiler_io_write_read_roundtrip() {
    let file = corpus_effects("io_basic.mvl");
    let out = run_transpiler(&file);
    assert_eq!(
        out.trim(),
        "hello io\nhello io appended\ndir_ok\nok",
        "io_basic.mvl: unexpected output from transpiler backend"
    );
}

/// `time.sleep(seconds(0))` — zero-duration sleep — must complete without
/// error and both backends must print "ok".
#[test]
fn cross_backend_time_sleep() {
    let file = corpus_effects("time_sleep.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "time_sleep.mvl: LLVM and transpiler backends must produce identical output"
        );
        assert_eq!(
            llvm_out.trim(),
            "ok",
            "expected 'ok' after zero-duration sleep"
        );
    }
}
