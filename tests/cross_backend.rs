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
//!   5. crypto_sha256.mvl — sha256/sha512 NIST vectors (#180 Rust path; #438 LLVM path)
//!
//! Phase 8 actor parity tests (#698):
//!   6. actor_spawn.mvl — minimal actor spawn + fire-and-forget behaviors
//!   7. actor_send.mvl  — multi-field actor with val-capability params

#![cfg(feature = "llvm")]

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_mvl is set at compile time and works correctly under
    // cargo test, cargo nextest, and cross-compiled builds.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mvl"))
}

fn corpus(name: &str) -> String {
    format!("{}/examples/programs/{name}", env!("CARGO_MANIFEST_DIR"))
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

fn corpus_basics(name: &str) -> String {
    format!(
        "{}/tests/corpus/01_basics/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_stdlib(name: &str) -> String {
    format!("{}/tests/stdlib/{name}", env!("CARGO_MANIFEST_DIR"))
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
    if mvl::mvl::backends::llvm::find_lli().is_none() {
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
    if mvl::mvl::backends::llvm::find_lli().is_none() {
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

/// Assert that both backends produce `expected` stdout for an arbitrary file path.
/// Use with `corpus_stdlib`, `corpus_effects`, etc.
fn assert_parity(file: &str, expected: &str) {
    let transpiler_out = run_transpiler(file);
    assert_eq!(
        transpiler_out.trim(),
        expected,
        "{file}: unexpected output from transpiler backend"
    );
    if let Some(llvm_out) = run_llvm(file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "{file}: LLVM and transpiler backends must produce identical output"
        );
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

// ── #418: Map/Set native MVL collections ──────────────────────────────────────

/// Both backends must produce identical deterministic output for Map.len,
/// Map.contains_key, Set.len, and Set.contains — all implemented as native
/// MVL bodies in std/collections.mvl dispatched by each backend's method
/// call machinery.
#[test]
fn cross_backend_collections_basic() {
    assert_backends_agree("collections_basic.mvl");
}

// ── #421: Higher-order functions (filter, map, fold, any + inline lambdas) ─────

/// Both backends must agree on HOF operations (filter, map, fold, any) using
/// both named-function arguments and inline lambda syntax.
#[test]
fn cross_backend_hof_lambdas() {
    assert_backends_agree("hof_lambdas.mvl");
}

/// Both backends must produce identical output when lambdas capture variables
/// from the enclosing scope (closure lowering, #588).
#[test]
fn cross_backend_closure_lambdas() {
    assert_backends_agree("closure_lambdas.mvl");
    // Pin the expected output so symmetric bugs (both backends produce the same
    // wrong value) are still caught.
    let file = corpus("closure_lambdas.mvl");
    let expected = "above_threshold=3\nmap_with_offset=36\nfold_with_base=65\n";
    assert_eq!(
        run_transpiler(&file),
        expected,
        "closure_lambdas.mvl: transpiler output mismatch"
    );
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
        "use std.random.{float}\nfn main() -> Unit ! Random {\n  let v: Float = float();\n  println(format(\"{}\", v));\n}\n",
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

// ── ADR-0034: user-defined generic function parity ────────────────────────────

/// User-defined generic functions must produce identical output from both backends.
///
/// `generic_fns.mvl` exercises:
///   - `identity[T]` instantiated with Int and String (two MonoFn copies)
///   - `Option[Point]` payload — struct stored in a generic container
///
/// LLVM backend uses the MonoProgram pre-emit pass (ADR-0034);
/// Rust backend emits native Rust generics. Both must agree on output.
#[test]
fn cross_backend_generic_fns() {
    assert_backends_agree("generic_fns.mvl");
}

// ── #583: generic builtin parity tests (choice, shuffle) ─────────────────────

/// `random.choice` on a single-element list is deterministic: always `Some(42)`.
/// Empty list always returns `None`.  Both backends must match.
#[test]
fn cross_backend_random_choice() {
    let file = corpus_effects("random_choice.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "random_choice.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 2, "expected two lines");
        assert_eq!(lines[0], "42", "choice([42]) must return Some(42)");
        assert_eq!(lines[1], "none", "choice([]) must return None");
    }
}

/// `random.shuffle` on a single-element list is a no-op.
/// Both backends must return a list of length 1, and empty stays empty.
#[test]
fn cross_backend_random_shuffle() {
    let file = corpus_effects("random_shuffle.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "random_shuffle.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 2, "expected two lines");
        assert_eq!(lines[0], "1", "shuffle([7]) must have length 1");
        assert_eq!(lines[1], "0", "shuffle([]) must have length 0");
    }
}

// ── #434: log C-ABI parity tests ─────────────────────────────────────────────

/// Both backends must emit identical log records to stderr.
///
/// The transpiler backend uses pure-MVL log formatters (ADR-0024).
/// The LLVM backend is skipped: pure-MVL log formatting depends on `str_replace`,
/// `str_len`, `for` loops, and `.sort()` — all of which are stubs/missing in the
/// LLVM backend.  Tracked as a pre-existing LLVM limitation.
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
    for level in &["DEBUG ", "INFO  ", "WARN  ", "ERROR "] {
        assert!(
            t_stderr.contains(level),
            "transpiler stderr missing {level}:\n{t_stderr}"
        );
    }

    // LLVM backend: skip — pure-MVL log formatters need str_replace, for-loops,
    // and list sort, which are not yet implemented in the LLVM backend.
    eprintln!(
        "SKIP cross_backend_log_stderr LLVM half: pure-MVL log needs LLVM string/loop support"
    );
}

// ── #779: std.net — both backends ────────────────────────────────────────────

/// Actor connects to listener, writes "net ok", main reads and prints it.
/// Verifies that both backends correctly wire tcp_listen, tcp_connect,
/// tcp_accept, tcp_read, tcp_write, tcp_close_* via `! Net` effect.
/// Error-path coverage lives in tests/stdlib/net_test.mvl (test-stdlib suite).
#[test]
fn cross_backend_net_basic() {
    assert_parity(&corpus_stdlib("net_basic.mvl"), "net ok");
}

// ── #417 + #435: io stdlib — both backends ────────────────────────────────────

/// Write+read roundtrip, append, create_dir, remove.
/// Both backends must produce identical output: the file round-trips correctly.
#[test]
fn cross_backend_io_write_read_roundtrip() {
    let file = corpus_effects("io_basic.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "read_ok\nappend_ok\ndir_ok\nok",
        "io_basic.mvl: unexpected output from transpiler backend"
    );
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "io_basic.mvl: LLVM and transpiler backends must produce identical output"
        );
    }
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

/// `time.format_datetime` with a fixed `DateTime` — deterministic on all backends.
#[test]
fn cross_backend_time_format_datetime() {
    let file = corpus_effects("time_format_datetime.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "time_format_datetime.mvl: LLVM and transpiler backends must produce identical output"
        );
        assert_eq!(
            llvm_out.trim(),
            "2024-03-15T12:30:45",
            "expected fixed datetime string"
        );
    }
}

/// `time.now()` + `time.format_instant()` — non-deterministic value but both
/// backends must return a 4-character year string.
#[test]
fn cross_backend_time_format_instant() {
    let file = corpus_effects("time_format_instant.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "time_format_instant.mvl: LLVM and transpiler backends must produce identical output"
        );
        assert_eq!(
            llvm_out.trim().len(),
            4,
            "format_instant(now(), '%Y') must produce a 4-character year"
        );
    }
}

// ── #180 + #438: crypto stdlib — both backends ────────────────────────────────

const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA512_EMPTY: &str = concat!(
    "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce",
    "47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
);

/// sha256/sha512 with NIST test vectors — Rust transpiler backend.
/// The Rust backend calls mvl_runtime::stdlib::crypto directly via the prelude.
#[test]
fn cross_backend_crypto_sha256_transpiler() {
    let file = corpus_effects("crypto_sha256.mvl");
    let out = run_transpiler(&file);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 output lines, got: {out:?}");
    assert_eq!(lines[0], SHA256_EMPTY, "sha256(\"\") mismatch");
    assert_eq!(lines[1], SHA256_ABC, "sha256(\"abc\") mismatch");
    assert_eq!(lines[2], SHA512_EMPTY, "sha512(\"\") mismatch");
}

/// sha256/sha512 cross-backend parity — LLVM backend vs Rust transpiler.
///
/// Verifies that the LLVM path (via _mvl_crypto_sha256 / _mvl_crypto_sha512 in
/// libmvl_runtime_c) produces the same NIST vectors as the Rust transpiler path.
/// Implemented by #438.
#[test]
fn cross_backend_crypto_sha256_llvm() {
    let file = corpus_effects("crypto_sha256.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "crypto_sha256.mvl: LLVM and transpiler backends must produce identical output"
        );
    }
}

// ── #420/#439: regex C-ABI parity tests ──────────────────────────────────────

/// Both backends must produce identical output for `regex.compile` + `regex.replace`.
#[test]
fn cross_backend_regex_replace() {
    let file = corpus_stdlib("regex_replace.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "regex_replace.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 3, "expected three output lines");
        assert_eq!(lines[0], "abc N def N", "digits must be redacted");
        assert_eq!(lines[1], "no digits here", "no-digit input unchanged");
        assert_eq!(lines[2], "N N N", "all three digit groups redacted");
    }
}

/// Both backends must produce identical output for `regex.find` returning `Option[Match]`.
/// Verifies that text extraction and None handling are consistent.
#[test]
fn cross_backend_regex_find() {
    let file = corpus_stdlib("regex_find.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "regex_find.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 3, "expected three output lines");
        assert_eq!(lines[0], "123", "first digit run extracted");
        assert_eq!(lines[1], "(none)", "no-digits input returns None");
        assert_eq!(lines[2], "42", "leading digit run extracted");
    }
}

/// Both backends must produce identical output for `regex.find_all` returning `List[Match]`.
/// Verifies that the match count is correct for a multi-match and a zero-match input.
#[test]
fn cross_backend_regex_find_all() {
    let file = corpus_stdlib("regex_find_all.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        let transpiler_out = run_transpiler(&file);
        assert_eq!(
            llvm_out, transpiler_out,
            "regex_find_all.mvl: LLVM and transpiler backends must produce identical output"
        );
        let lines: Vec<&str> = llvm_out.lines().collect();
        assert_eq!(lines.len(), 2, "expected two output lines");
        assert_eq!(lines[0], "3", "digit pattern matches 3 times in '1 22 333'");
        assert_eq!(lines[1], "0", "digit pattern matches 0 times in 'abc'");
    }
}

// ── #587: set algebra (intersection, difference, union) ───────────────────────

/// Both backends must produce identical element counts for set_intersection,
/// set_difference, and set_union on integer sets.
#[test]
fn cross_backend_set_algebra() {
    let file = corpus_stdlib("set_algebra.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "2\n2\n6",
        "Rust transpiler: expected intersection=2, difference=2, union=6, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out.trim(),
            transpiler_out.trim(),
            "LLVM output must match Rust transpiler for set algebra"
        );
    }
}

// ── #586: signal handling (ignore, reset, on) ─────────────────────────────────

/// Both backends must produce identical output for `signal_ignore` and `signal_reset`.
/// Both are no-op stubs; the test verifies they compile and run without crashing.
#[test]
fn cross_backend_env_signal_ignore_reset() {
    let file = corpus_effects("env_signal_ignore.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "ok",
        "Rust transpiler: expected 'ok', got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out.trim(),
            "ok",
            "LLVM backend: expected 'ok', got: {llvm_out:?}"
        );
    }
}

/// LLVM-only: `signal_on` with a named non-capturing handler must not crash.
#[test]
fn cross_backend_env_signal_on_llvm() {
    let file = corpus_effects("env_signal_on.mvl");
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out.trim(),
            "ok",
            "LLVM backend: expected 'ok', got: {llvm_out:?}"
        );
    }
}

/// crypto_random_bytes shape test — both backends must print the correct list length.
///
/// Non-deterministic values are not compared; only the length (always == n) is checked.
/// This exercises the I64ReturnsPtrArg dispatch (#507) and Secret[List[Int]] label
/// handling in the LLVM codegen (#508).
#[test]
fn cross_backend_crypto_random_bytes_llvm_shape() {
    let file = corpus_effects("crypto_random_bytes_shape.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "16",
        "Rust transpiler: expected length 16, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out.trim(),
            "16",
            "LLVM backend: expected length 16, got: {llvm_out:?}"
        );
    }
}

/// crypto_random_bytes(0) — both backends must return an empty list.
///
/// Edge-case for the I64ReturnsPtrArg dispatch and MvlArray zero-length allocation (#507).
#[test]
fn cross_backend_crypto_random_bytes_zero_llvm() {
    let file = corpus_effects("crypto_random_bytes_zero.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "0",
        "Rust transpiler: expected length 0, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm(&file) {
        assert_eq!(
            llvm_out.trim(),
            "0",
            "LLVM backend: expected length 0, got: {llvm_out:?}"
        );
    }
}

/// parse_int / parse_float — verify both succeed and fail correctly in the LLVM backend.
#[test]
fn cross_backend_parse_int_float_llvm() {
    let file = corpus_types("parse_int_float_llvm.mvl");
    assert_llvm_output(&file, "42\n1\nok\n1");
}

#[test]
fn cross_backend_println_multi_arg() {
    let file = corpus("println_non_string_first_arg.mvl");
    assert_llvm_output(&file, "hello 42\n42\n42 100\n42 100 hello");
}

/// eprint/eprintln both backends write to stderr with identical output (#556).
#[test]
fn cross_backend_eprint_stderr() {
    let file = corpus_basics("eprint_stderr.mvl");

    // Transpiler path: capture stderr.
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
    assert!(
        t_stderr.contains("error: something went wrong"),
        "transpiler stderr missing expected output:\n{t_stderr}"
    );
    assert!(
        t_stderr.contains("count=42"),
        "transpiler stderr missing count=42:\n{t_stderr}"
    );

    if mvl::mvl::backends::llvm::find_lli().is_none() {
        eprintln!("SKIP cross_backend_eprint_stderr LLVM half: lli not found");
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
    assert!(
        l_stderr.contains("error: something went wrong"),
        "LLVM stderr missing expected output:\n{l_stderr}"
    );
    assert!(
        l_stderr.contains("count=42"),
        "LLVM stderr missing count=42:\n{l_stderr}"
    );
}

// ── ADR-0022: Category 1 operator intrinsics (LLVM backend) ──────────────────

fn intrinsic(name: &str) -> String {
    format!("{}/tests/intrinsics/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn intrinsic_arithmetic() {
    assert_llvm_output(&intrinsic("01_arithmetic.mvl"), "ok");
}

#[test]
fn intrinsic_comparison() {
    assert_llvm_output(&intrinsic("02_comparison.mvl"), "ok");
}

#[test]
fn intrinsic_logical() {
    assert_llvm_output(&intrinsic("03_logical.mvl"), "ok");
}

#[test]
fn intrinsic_bitwise() {
    assert_llvm_output(&intrinsic("04_bitwise.mvl"), "ok");
}

// ── #557: parity quick wins ───────────────────────────────────────────────────

#[test]
fn intrinsic_random_bytes() {
    assert_llvm_output(&intrinsic("05_random_bytes.mvl"), "ok");
}

#[test]
fn intrinsic_env_args() {
    assert_llvm_output(&intrinsic("06_env_args.mvl"), "ok");
}

// ── #571: recursive ADT with Box[T] ──────────────────────────────────────────

#[test]
fn cross_backend_linked_list() {
    assert_backends_agree("linked_list.mvl");
    assert_llvm_output(&corpus("linked_list.mvl"), "length: 3");
}

// ── #606: Box[T] deref via struct field access ────────────────────────────────

#[test]
fn cross_backend_box_field_deref() {
    assert_backends_agree("box_field_deref.mvl");
    assert_llvm_output(&corpus("box_field_deref.mvl"), "value: 42");
}

// ── #541: cross-profile behavioral parity (trusted vs proven) ────────────────
//
// Verifies that --stdlib=trusted and --stdlib=proven (which currently falls
// back to trusted pending #538) produce identical output.  This test acts as
// a regression guard: once proven mode has MVL implementations (#538), adding
// an explicit proven-mode runner here will catch any behavioral divergence.

fn run_transpiler_with_profile(file: &str, profile: &str) -> String {
    let out = Command::new(mvl_bin())
        .args(["run", file, &format!("--stdlib={profile}")])
        .output()
        .expect("failed to run mvl run --stdlib=...");
    assert!(
        out.status.success(),
        "transpiler backend failed for {file} (--stdlib={profile}):\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let raw = String::from_utf8_lossy(&out.stdout);
    raw.lines()
        .filter(|l| !l.starts_with("Transpiled to:") && !l.starts_with("Running:"))
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn stdlib_trusted_profile_produces_expected_output() {
    // Explicit --stdlib=trusted is identical to the default.
    let default_out = run_transpiler(&corpus("hello_world.mvl"));
    let trusted_out = run_transpiler_with_profile(&corpus("hello_world.mvl"), "trusted");
    assert_eq!(
        default_out, trusted_out,
        "explicit --stdlib=trusted must match default (no flag)"
    );
}

#[test]
fn stdlib_proven_profile_falls_back_to_trusted() {
    // --stdlib=proven currently falls back to trusted (#538 pending).
    // Output must be identical; the only difference is a diagnostic note on stderr.
    let trusted_out = run_transpiler_with_profile(&corpus("calculator.mvl"), "trusted");
    let proven_out = run_transpiler_with_profile(&corpus("calculator.mvl"), "proven");
    assert_eq!(
        trusted_out, proven_out,
        "--stdlib=proven fallback output must match --stdlib=trusted until #538 is implemented"
    );
}

// ── #698: Phase 8 actor parity (spawn + fire-and-forget send) ────────────────

/// Minimal actor spawn: `actor Counter { count: 0 }` + two behaviors + reset.
/// Both backends must compile the actor infrastructure and produce "ok" from main.
/// Behavior bodies are empty stubs — main output is deterministic regardless of
/// message delivery timing.
#[test]
fn cross_backend_actor_spawn() {
    assert_backends_agree("actor_spawn.mvl");
    // Also assert exact output so the test stays pinned even if run_transpiler
    // filtering changes.
    let file = corpus("actor_spawn.mvl");
    assert_eq!(run_transpiler(&file).trim(), "ok");
    if let Some(out) = run_llvm(&file) {
        assert_eq!(out.trim(), "ok");
    }
}

/// Multi-field actor with `val`-capability behavior params.
/// Exercises the iso/val sendability path on both backends.
#[test]
fn cross_backend_actor_send() {
    assert_backends_agree("actor_send.mvl");
    let file = corpus("actor_send.mvl");
    assert_eq!(run_transpiler(&file).trim(), "sent");
    if let Some(out) = run_llvm(&file) {
        assert_eq!(out.trim(), "sent");
    }
}

/// Req 10 / Phase 4 (#627): LLVM backend emits `llvm.trap` for `requires` predicates.
///
/// Verifies that a function with a `requires` clause generates a conditional
/// branch to `call void @llvm.trap()` in the LLVM IR (runtime guard parity
/// with the Rust backend's `assert!(pred, "requires: ...")`).
#[test]
#[cfg(feature = "llvm")]
fn llvm_requires_clause_emits_trap() {
    let src = r#"
fn safe_divide(a: Int, b: Int) -> Int requires b != 0 { a / b }
fn main() -> Unit { println("ok") }
"#;
    let (mut parser, lex_errors) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    let ir =
        mvl::mvl::backends::llvm::compile_to_ir(&prog, "test_req").expect("IR generation failed");
    assert!(
        ir.contains("llvm.trap"),
        "LLVM IR for fn with `requires` must contain llvm.trap.\nIR:\n{ir}"
    );
}

/// #670: LLVM backend emits `llvm.trap` for structs with `with invariant` (#670).
///
/// Verifies backend parity: the LLVM IR for a struct constructor with an invariant
/// contains a conditional branch to `call void @llvm.trap()`.
#[test]
fn llvm_struct_invariant_emits_trap() {
    let src = r#"
type Range = struct { lo: Int, hi: Int, } with invariant self.lo <= self.hi
fn main() -> Unit {
    let r: Range = Range { lo: 1, hi: 10 };
    println("ok")
}
"#;
    let (mut parser, lex_errors) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    let ir =
        mvl::mvl::backends::llvm::compile_to_ir(&prog, "test_inv").expect("IR generation failed");
    assert!(
        ir.contains("llvm.trap"),
        "LLVM IR for struct with `with invariant` must contain llvm.trap.\nIR:\n{ir}"
    );
}

/// Req 10 / Phase 4 (#627): `AssertMode::Assume` emits `llvm.assume` instead of `llvm.trap`.
///
/// Verifies that when the compiler is configured with `AssertMode::Assume`, a `requires`
/// predicate is lowered to `llvm.assume` (hint to the optimizer) rather than a trap guard.
#[test]
#[cfg(feature = "llvm")]
fn llvm_requires_clause_assume_mode_emits_assume() {
    let src = r#"
fn safe_divide(a: Int, b: Int) -> Int requires b != 0 { a / b }
fn main() -> Unit { println("ok") }
"#;
    let (mut parser, lex_errors) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    let mut compiler = mvl::mvl::backends::llvm::LlvmCompiler::new();
    compiler.assert_mode = mvl::mvl::backends::AssertMode::Assume;
    let ir = compiler
        .compile_to_ir(&prog, "test_assume")
        .expect("IR generation failed");
    assert!(
        ir.contains("llvm.assume"),
        "LLVM IR with AssertMode::Assume must contain llvm.assume.\nIR:\n{ir}"
    );
    assert!(
        !ir.contains("llvm.trap"),
        "LLVM IR with AssertMode::Assume must NOT contain llvm.trap.\nIR:\n{ir}"
    );
}

/// Req 10 / Phase 4 (#627): single-parameter `requires` predicate with `self` alias binding.
///
/// When a function has exactly one Int parameter, the LLVM backend also binds it under
/// the `"self"` alias so that normalised predicates using `self` can be evaluated.
/// Verifies the guard is emitted for the single-parameter case.
#[test]
#[cfg(feature = "llvm")]
fn llvm_requires_single_param_self_normalised_emits_trap() {
    let src = r#"
fn check_positive(x: Int) -> Int requires x > 0 { x }
fn main() -> Unit { println("ok") }
"#;
    let (mut parser, lex_errors) = mvl::mvl::parser::Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    let ir = mvl::mvl::backends::llvm::compile_to_ir(&prog, "test_self_req")
        .expect("IR generation failed");
    assert!(
        ir.contains("llvm.trap"),
        "LLVM IR for single-param `requires x > 0` must contain llvm.trap.\nIR:\n{ir}"
    );
}
