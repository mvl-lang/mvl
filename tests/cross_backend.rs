//! Cross-backend regression tests: verify that the LLVM backend produces the
//! same stdout as the Rust transpiler backend for the same MVL programs.
//!
//! Post-ADR-0040 (inkwell removed), `--backend=llvm` resolves to the
//! `llvm_text` emitter. The helpers below are named accordingly (#1154).
//!
//! Skip policy:
//!   * `lli` not installed → environment skip (helper returns `None`).
//!   * llvm_text compile/JIT failure → test failure (strict helpers panic;
//!     legacy soft helpers return `None` and the caller decides).
//!
//! Parity policy:
//!   * `assert_backends_agree` / `assert_parity` are STRICT — divergence
//!     between Rust transpiler and llvm_text is a test failure (#1154).
//!   * Tests known to diverge are marked `#[ignore = "llvm_text: <reason>"]`
//!     with an upstream issue link, NOT silently masked.
//!
//! Seed corpus (kept for historical reference):
//!   1. hello_world.mvl  — minimal fn main + println
//!   2. calculator.mvl   — total fns, if/else, arithmetic
//!   3. shapes.mvl       — enums, match dispatch, function composition
//!   4. env_basic.mvl    — getuid + getgid via libmvl_runtime_c
//!   5. crypto_sha256.mvl — sha256/sha512 NIST vectors

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    // CARGO_BIN_EXE_mvl is set at compile time and works correctly under
    // cargo test, cargo nextest, and cross-compiled builds.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mvl"))
}

fn corpus(name: &str) -> String {
    format!("{}/examples/programs/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn corpus_primitives(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/04_primitives/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_13_stdlib(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/13_stdlib/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_ownership(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/06_ownership/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_collections(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/05_collections/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_stdlib_tests(name: &str) -> String {
    format!("{}/tests/stdlib/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn corpus_types(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/03_types/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_functions(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/02_functions/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn corpus_actors(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/12_actors/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Strip transpiler/backend progress lines ("Transpiled to: ...", "Running: ...").
fn strip_progress_lines(raw: &str) -> String {
    raw.lines()
        .filter(|l| !l.starts_with("Transpiled to:") && !l.starts_with("Running:"))
        .map(|l| format!("{l}\n"))
        .collect()
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
    strip_progress_lines(&String::from_utf8_lossy(&out.stdout))
}

/// Failure mode for the llvm_text runner.
enum LlvmFailure {
    /// Backend compile/JIT failure is a test failure — panic with stderr.
    Panic,
    /// Backend compile/JIT failure is a soft skip — log stderr, return None.
    SoftSkip,
}

/// Shared implementation for both [`run_llvm_text`] and [`run_llvm_text_or_skip`].
/// Environment skip (no `lli`) always returns `None`. Backend failure is
/// dispatched by `on_failure`.
fn run_llvm_text_inner(file: &str, on_failure: LlvmFailure) -> Option<String> {
    mvl::mvl::backends::llvm_text::lli::find_lli()?;
    let out = Command::new(mvl_bin())
        .args(["run", file, "--backend=llvm"])
        .output()
        .expect("failed to run mvl run --backend=llvm");
    if !out.status.success() {
        match on_failure {
            LlvmFailure::Panic => panic!(
                "llvm_text backend failed for {file}:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr),
            ),
            LlvmFailure::SoftSkip => {
                eprintln!(
                    "SKIP {file}: llvm_text backend failed:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                return None;
            }
        }
    }
    Some(strip_progress_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Run a program via the llvm_text backend (`--backend=llvm`, ADR-0040).
///
/// Returns `Some(stdout)` on success. Returns `None` for **environment skip**
/// (no `lli`) only. **Compile/JIT failure is a test failure**: the helper
/// panics with the backend stderr so #1154 can surface real divergences
/// instead of silently masking them.
///
/// If you intentionally want a soft skip on backend failure (e.g. a test
/// that pre-dates a known-broken feature), use [`run_llvm_text_or_skip`].
fn run_llvm_text(file: &str) -> Option<String> {
    run_llvm_text_inner(file, LlvmFailure::Panic)
}

/// Soft variant of [`run_llvm_text`]: returns `None` on either environment
/// skip (no `lli`) or llvm_text compile/JIT failure, with the failure logged
/// to stderr.
///
/// Use this only for tests that pre-date a known-broken feature. New tests
/// should call [`run_llvm_text`] so backend regressions surface immediately.
///
/// Retained as a future escape hatch per #1548; currently unused.
#[allow(dead_code)]
fn run_llvm_text_or_skip(file: &str) -> Option<String> {
    run_llvm_text_inner(file, LlvmFailure::SoftSkip)
}

/// Run a program via the llvm_text backend and assert expected output.
/// Environment-skips if `lli` is not available; backend failure is a test
/// failure (see [`run_llvm_text`]).
fn assert_llvm_output(file: &str, expected: &str) {
    if let Some(actual) = run_llvm_text(file) {
        assert_eq!(
            actual.trim(),
            expected.trim(),
            "{file}: llvm_text output mismatch.\nexpected: {expected:?}\nactual:   {actual:?}"
        );
    }
}

/// Assert that both backends produce identical stdout for the given corpus
/// program. STRICT (#1154): divergence is a test failure, not a logged warning.
/// Tests known to diverge are marked `#[ignore]` with a reason.
fn assert_backends_agree(name: &str) {
    let file = corpus(name);
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "{name}: llvm_text and transpiler backends produced different output"
        );
    }
}

/// Assert that both backends produce `expected` stdout for an arbitrary file
/// path. Use with `corpus_stdlib`, `corpus_effects`, etc.
///
/// STRICT (#1154): transpiler output must match `expected`, AND llvm_text
/// output must match transpiler output. Divergence is a test failure.
fn assert_parity(file: &str, expected: &str) {
    let transpiler_out = run_transpiler(file);
    assert_eq!(
        transpiler_out.trim(),
        expected,
        "{file}: unexpected output from transpiler backend"
    );
    if let Some(llvm_out) = run_llvm_text(file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "{file}: llvm_text output differs from transpiler"
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
    // Pin expected output so the LLVM backend is actually verified (#1163 AC).
    let file = corpus("hof_lambdas.mvl");
    let expected = "filter_len=3\nmap_sum=12\nfold_sum=15\nany_even=true\nany_odd=false\n";
    assert_eq!(
        run_transpiler(&file),
        expected,
        "hof_lambdas.mvl: transpiler output mismatch"
    );
    assert_llvm_output(&file, expected);
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
    assert_llvm_output(&file, expected);
}

/// Category-D list builtins: sort/partition/group_by/windows/chunks must agree
/// across backends (#1290).
#[test]
fn cross_backend_list_stubs() {
    assert_backends_agree("list_stubs.mvl");
    let file = corpus("list_stubs.mvl");
    let expected =
        "sort=[1, 1, 3, 4, 5]\npartition_yes=2\npartition_no=3\ngroup_by_key1=2\nwindows=3\nchunks=3\n";
    assert_eq!(
        run_transpiler(&file),
        expected,
        "list_stubs.mvl: transpiler output mismatch"
    );
    assert_llvm_output(&file, expected);
}

// ── Phase C: heap allocation tests (LLVM-only) ────────────────────────────────

#[test]
fn llvm_string_heap() {
    let file = corpus_primitives("string_heap_llvm.mvl");
    assert_llvm_output(&file, "5\nhello world\n11");
}

// ── L5-15: ownership-based drop (LLVM-only) ───────────────────────────────────

#[test]
fn llvm_move_string() {
    let file = corpus_primitives("move_string_llvm.mvl");
    assert_llvm_output(&file, "hello\nworld");
}

#[test]
fn llvm_fn_takes_string() {
    let file = corpus_primitives("fn_takes_string_llvm.mvl");
    assert_llvm_output(&file, "hello world");
}

// ── ADR-0019: C-ABI stdlib parity tests ──────────────────────────────────────

/// Both backends call `getuid()` and `getgid()` and must produce identical output.
/// Both ultimately call the same POSIX syscalls, so UID and GID are the same.
#[test]
fn cross_backend_env_basic() {
    let file = corpus_13_stdlib("env_basic.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("env_basic.mvl");
    if let Some(out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("env_basic.mvl");
    if let Some(out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("random_int.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
        "use std.random.{float}\nfn main() -> Unit ! Random {\n  let v: Float = float();\n  println(format(\"{}\", [v.to_string()]));\n}\n",
    )
    .expect("write");
    if let Some(out) = run_llvm_text(&tmp.path().to_string_lossy()) {
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
    let file = corpus_13_stdlib("random_choice.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("random_shuffle.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
/// The LLVM backend is skipped: the `json_escape` redefinition (#1551) is fixed,
/// but `_log_timestamp` still hits a transitive-builtin dispatch bug — `now()`
/// is declared in `std/time.mvl` but `collect_llvm_text_builtins` only scans
/// top-level user imports, so transitive `use std.time` from `std/log.mvl`
/// is not registered.  Plus pure-MVL log formatting depends on `str_replace`,
/// `str_len`, `for` loops, and `.sort()` — all stubs/missing in the LLVM backend.
#[test]
fn cross_backend_log_stderr() {
    let file = corpus_13_stdlib("log_output.mvl");

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

    // LLVM backend: still skipped — blocked on #1551 (duplicate @json_escape).
    eprintln!("SKIP cross_backend_log_stderr LLVM half: blocked on #1551");
}

/// Regression for #1551: importing both `std.json` and `std.log` must not
/// produce an `invalid redefinition of function 'json_escape'` error from lli.
///
/// Narrow assertion: we don't require the program to run successfully (other
/// pre-existing LLVM backend bugs may surface for `std.log` / `std.json`).
/// We only assert that the specific redefinition error does *not* appear on
/// stderr, so a future regression that re-adds a duplicate `json_escape`
/// will fail this test loudly.
#[test]
fn cross_backend_json_log_no_redefinition() {
    if mvl::mvl::backends::llvm_text::lli::find_lli().is_none() {
        eprintln!("SKIP cross_backend_json_log_no_redefinition: lli not available");
        return;
    }
    let file = corpus_13_stdlib("json_log_imports.mvl");
    let out = Command::new(mvl_bin())
        .args(["run", &file, "--backend=llvm"])
        .output()
        .expect("failed to run mvl run --backend=llvm");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("redefinition of function 'json_escape'"),
        "regression of #1551: lli reported json_escape redefinition:\n{stderr}"
    );
}

// ── #779: std.net — both backends ────────────────────────────────────────────

/// Actor connects to listener, writes "net ok", main reads and prints it.
/// Verifies that both backends correctly wire tcp_listen, tcp_connect,
/// tcp_accept, tcp_read, tcp_write, tcp_close_* via `! Net` effect.
/// Error-path coverage lives in tests/stdlib/net_test.mvl (test-stdlib suite).
#[test]
fn cross_backend_net_basic() {
    assert_parity(&corpus_stdlib_tests("net_basic.mvl"), "net ok");
}

// ── #417 + #435: io stdlib — both backends ────────────────────────────────────

/// Write+read roundtrip, append, create_dir, remove.
/// Both backends must produce identical output: the file round-trips correctly.
#[test]
fn cross_backend_io_write_read_roundtrip() {
    let file = corpus_13_stdlib("io_basic.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "read_ok\nappend_ok\ndir_ok\nok",
        "io_basic.mvl: unexpected output from transpiler backend"
    );
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("time_sleep.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("time_format_datetime.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("time_format_instant.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("crypto_sha256.mvl");
    let out = run_transpiler(&file);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 output lines, got: {out:?}");
    assert_eq!(lines[0], SHA256_EMPTY, "sha256(\"\") mismatch");
    assert_eq!(lines[1], SHA256_ABC, "sha256(\"abc\") mismatch");
    assert_eq!(lines[2], SHA512_EMPTY, "sha512(\"\") mismatch");
}

/// sha256/sha512 cross-backend parity — LLVM backend vs NIST test vectors.
///
/// Verifies that the LLVM path (via _mvl_crypto_sha256 / _mvl_crypto_sha512 in
/// libmvl_runtime_c) produces the same NIST vectors as the Rust transpiler path.
/// Uses hardcoded constants rather than calling run_transpiler to avoid a parallel
/// cargo build race when both sha256 tests run concurrently on the same temp dir.
#[test]
fn cross_backend_crypto_sha256_llvm() {
    let file = corpus_13_stdlib("crypto_sha256.mvl");
    let expected = format!("{SHA256_EMPTY}\n{SHA256_ABC}\n{SHA512_EMPTY}\n");
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, expected,
            "crypto_sha256.mvl: LLVM and transpiler backends must produce identical output"
        );
    }
}

// ── #420/#439: regex C-ABI parity tests ──────────────────────────────────────

/// Both backends must produce identical output for `regex.compile` + `regex.replace`.
#[test]
fn cross_backend_regex_replace() {
    let file = corpus_stdlib_tests("regex_replace.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_stdlib_tests("regex_find.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_stdlib_tests("regex_find_all.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_stdlib_tests("set_algebra.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "2\n2\n6",
        "Rust transpiler: expected intersection=2, difference=2, union=6, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out.trim(),
            transpiler_out.trim(),
            "set_algebra.mvl: llvm_text output differs from transpiler"
        );
    }
}

// ── #586: signal handling (ignore, reset, on) ─────────────────────────────────

/// Both backends must produce identical output for `signal_ignore` and `signal_reset`.
/// Both are no-op stubs; the test verifies they compile and run without crashing.
#[test]
fn cross_backend_env_signal_ignore_reset() {
    let file = corpus_13_stdlib("env_signal_ignore.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "ok",
        "Rust transpiler: expected 'ok', got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_13_stdlib("env_signal_on.mvl");
    if let Some(llvm_out) = run_llvm_text(&file) {
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
#[test]
fn cross_backend_crypto_random_bytes_llvm_shape() {
    let file = corpus_13_stdlib("crypto_random_bytes_shape.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "16",
        "Rust transpiler: expected length 16, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out.trim(),
            "16",
            "LLVM backend: expected length 16, got: {llvm_out:?}"
        );
    }
}

/// crypto_random_bytes(0) — both backends must return an empty list.
#[test]
fn cross_backend_crypto_random_bytes_zero_llvm() {
    let file = corpus_13_stdlib("crypto_random_bytes_zero.mvl");
    let transpiler_out = run_transpiler(&file);
    assert_eq!(
        transpiler_out.trim(),
        "0",
        "Rust transpiler: expected length 0, got: {transpiler_out:?}"
    );
    if let Some(llvm_out) = run_llvm_text(&file) {
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
    let file = corpus_primitives("parse_int_float_llvm.mvl");
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
    let file = corpus_13_stdlib("eprint_stderr.mvl");

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

    if mvl::mvl::backends::llvm_text::lli::find_lli().is_none() {
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
    format!("{}/tests/corpus_old/00_intrinsics/{name}", env!("CARGO_MANIFEST_DIR"))
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
#[test]
fn cross_backend_actor_spawn() {
    assert_backends_agree("actor_spawn.mvl");
    let file = corpus("actor_spawn.mvl");
    assert_eq!(run_transpiler(&file).trim(), "ok");
    if let Some(out) = run_llvm_text(&file) {
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
    if let Some(out) = run_llvm_text(&file) {
        assert_eq!(out.trim(), "sent");
    }
}

/// #904: `.clone()` on a List creates an independent copy in the LLVM backend.
///
/// Clones a list, pushes an element onto the clone, and verifies the original
/// length is unchanged (both backends must print "3").
#[test]
fn clone_list_independent_of_original() {
    assert_parity(&corpus_ownership("clone_heap_independence.mvl"), "3");
}

/// #906: UFCS String method parity — LLVM backend now has a dispatch table
/// mirroring the Rust backend's STDLIB_UFCS_METHODS.
///
/// Tests: trim, to_lower, to_upper, starts_with, ends_with, contains (String),
///        replace, substring, concat, split.
#[test]
fn cross_backend_string_ufcs_methods() {
    assert_parity(
        &corpus_primitives("string_ufcs.mvl"),
        "Hello, World!\nmvl\nMVL\nstarts_ok\nends_ok\ncontains_ok\nHello, MVL!\nHello\nfoobar\n3",
    );
}

/// #906: UFCS List method parity — LLVM backend dispatches slice/take/skip
/// directly via _mvl_list_slice C runtime (Group E/F dispatch table).
///
/// Tests: slice (ptr×i64×i64→ptr), take (slice from 0..n), skip (slice from n..len).
#[test]
fn cross_backend_list_ufcs_methods() {
    assert_parity(&corpus_collections("list_ufcs.mvl"), "3\n3\n3");
}

// ── #1234: Expanded cross-backend parity — examples/programs ─────────────────

#[test]
fn cross_backend_hello_mvl() {
    assert_backends_agree("hello_mvl.mvl");
}

#[test]
fn cross_backend_else_if_chain() {
    assert_parity(
        &corpus("else_if_chain.mvl"),
        "classify(5) = positive\nclassify(-3) = negative\nclassify(0) = zero",
    );
}

#[test]
fn cross_backend_safe_division() {
    assert_backends_agree("safe_division.mvl");
}

#[test]
fn cross_backend_struct_value_semantics() {
    assert_parity(&corpus("struct_value_semantics.mvl"), "1, 2\n4, 6");
}

#[test]
fn cross_backend_core_types_demo() {
    assert_backends_agree("core_types_demo.mvl");
}

// ── #1234: Expanded cross-backend parity — corpus/02_types ───────────────────

#[test]
fn cross_backend_enum_match() {
    assert_parity(&corpus_types("enum_match_llvm.mvl"), "0\n4\n3");
}

#[test]
fn cross_backend_enum_string_match() {
    assert_parity(
        &corpus_types("enum_string_match_llvm.mvl"),
        "DivisionByZero\nOverflow\nMathError: DivisionByZero",
    );
}

#[test]
fn cross_backend_for_loop() {
    assert_parity(&corpus_primitives("for_loop_llvm.mvl"), "0\n1\n2\n3\n4");
}

#[test]
fn cross_backend_while_loop() {
    assert_parity(&corpus_primitives("while_loop_llvm.mvl"), "0\n1\n2\n3\n4");
}

#[test]
fn cross_backend_struct_fields() {
    assert_parity(&corpus_types("struct_fields_llvm.mvl"), "10\n20\n30");
}

#[test]
fn cross_backend_result_propagate() {
    assert_parity(&corpus_types("result_propagate_llvm.mvl"), "10\nbad");
}

// ── #1234: Expanded parity — corpus/01_basics ────────────────────────────────

/// env_identity_llvm: getuid/getgid output — non-deterministic but both
/// backends must agree on the values.
#[test]
fn cross_backend_env_identity() {
    let file = corpus_13_stdlib("env_identity_llvm.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "env_identity_llvm.mvl: backends must agree"
        );
    }
}

// ── #1547 / ADR-0049: IFC label round-trip parity ────────────────────────────

/// `Tainted[T]` and `Secret[T]` wrappers, `relabel classify(...)`, and
/// `.into_inner()` must produce identical output on both backends.
///
/// Per ADR-0049: IFC enforcement happens in the checker; both backends pass
/// the inner value through unchanged at runtime. This pins that invariant.
#[test]
fn cross_backend_ifc_label_round_trip() {
    let file = format!(
        "{}/tests/corpus_old/08_ifc/label_into_inner.mvl",
        env!("CARGO_MANIFEST_DIR")
    );
    let transpiler_out = run_transpiler(&file);
    assert_eq!(transpiler_out.trim(), "43", "transpiler must print 43");
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "label_into_inner.mvl: backends must agree on IFC round-trip"
        );
    }
}

// ── #1546: for-in-List support on LLVM ───────────────────────────────────────

/// `for x in <list-expr> { … }` must compile to a working loop on LLVM
/// (previously silently emitted no body). Exercises:
/// - plain `List[Int]` iteration accumulating into `ref Int`,
/// - sort-then-iterate, where the iterable is a fresh `.sort()` result,
/// - `m.keys().sort()` iteration — the pattern std.log uses internally.
#[test]
fn cross_backend_for_in_list() {
    let file = corpus_collections("for_in_list.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "for_in_list.mvl: backends must agree"
        );
    }
}

// ── #1234: Expanded parity — stdlib tests ────────────────────────────────────

#[test]
fn cross_backend_range_pipeline() {
    assert_parity(&corpus_stdlib_tests("range_pipeline.mvl"), "5");
}

// ── #1234: Expanded parity — contracts ───────────────────────────────────────

fn corpus_contracts(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/11_contracts/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// basic_contracts.mvl: both backends must compile and run without crashing.
#[test]
fn cross_backend_basic_contracts() {
    let file = corpus_contracts("basic_contracts.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "basic_contracts.mvl: backends must agree"
        );
    }
}

/// ghost_old_contracts.mvl: both backends must compile and run.
#[test]
fn cross_backend_ghost_old_contracts() {
    let file = corpus_contracts("ghost_old_contracts.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "ghost_old_contracts.mvl: backends must agree"
        );
    }
}

// ── #1234: Expanded parity — concurrency ─────────────────────────────────────

fn corpus_concurrency(name: &str) -> String {
    format!(
        "{}/tests/corpus_old/12_actors/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// structured_concurrency.mvl: both backends must compile and run.
#[test]
fn cross_backend_structured_concurrency() {
    let file = corpus_concurrency("structured_concurrency.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "structured_concurrency.mvl: backends must agree"
        );
    }
}

// ── #1234: Expanded parity — check-only corpus tests ─────────────────────────
// These verify that both backends can at least check the corpus file without
// errors, even for programs without fn main / stdout output.

fn corpus_ifc(name: &str) -> String {
    format!("{}/tests/corpus_old/08_ifc/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn assert_check_passes(file: &str) {
    let out = std::process::Command::new(mvl_bin())
        .args(["check", file])
        .output()
        .expect("failed to run mvl check");
    assert!(
        out.status.success(),
        "check failed for {file}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cross_backend_check_ownership_consume() {
    assert_check_passes(&corpus_ownership("consume_transfer.mvl"));
}

#[test]
fn cross_backend_check_ownership_multi_use() {
    assert_check_passes(&corpus_ownership("multi_use_clone.mvl"));
}

#[test]
fn cross_backend_check_ownership_last_use() {
    assert_check_passes(&corpus_ownership("last_use_move.mvl"));
}

#[test]
fn cross_backend_check_ownership_field_access() {
    assert_check_passes(&corpus_ownership("field_access.mvl"));
}

#[test]
fn cross_backend_check_ownership_nested_calls() {
    assert_check_passes(&corpus_ownership("nested_calls.mvl"));
}

#[test]
fn cross_backend_check_ownership_value_semantics() {
    assert_check_passes(&corpus_ownership("value_semantics.mvl"));
}

#[test]
fn cross_backend_check_ownership_ref_mutation() {
    assert_check_passes(&corpus_ownership("ref_mutation.mvl"));
}

#[test]
fn cross_backend_check_ownership_lambda_capture() {
    assert_check_passes(&corpus_ownership("lambda_capture.mvl"));
}

#[test]
fn cross_backend_check_ownership_loop_clone() {
    assert_check_passes(&corpus_ownership("loop_clone.mvl"));
}

#[test]
fn cross_backend_check_ownership_collection_post_use() {
    assert_check_passes(&corpus_ownership("collection_post_use.mvl"));
}

#[test]
fn cross_backend_check_ifc_labels() {
    assert_check_passes(&corpus_ifc("labels.mvl"));
}

#[test]
fn cross_backend_check_ifc_propagation() {
    assert_check_passes(&corpus_ifc("propagation.mvl"));
}

#[test]
fn cross_backend_check_ifc_declassification() {
    assert_check_passes(&corpus_ifc("declassification.mvl"));
}

#[test]
fn cross_backend_check_ifc_interprocedural_clean() {
    assert_check_passes(&corpus_ifc("interprocedural_clean.mvl"));
}

#[test]
fn cross_backend_check_ifc_lattice() {
    assert_check_passes(&corpus_ifc("lattice.mvl"));
}

// ── #1250: LLVM closure capture cross-backend tests ──────────────────────────

/// Value capture: closures capturing Int values via filter/map/fold HOFs.
#[test]
fn cross_backend_closure_value_capture() {
    assert_parity(
        &corpus_functions("closure_value_capture.mvl"),
        "above_threshold=3\nshifted_sum=36\nfold_with_base=65",
    );
}

/// Closure composition: multiple closures capturing from same scope via fold.
#[test]
fn cross_backend_closure_nested() {
    assert_parity(
        &corpus_functions("closure_nested.mvl"),
        "doubled_sum=12\ncombined=27",
    );
}

/// String and Bool capture: closures capturing non-Int types (#1271).
#[test]
fn cross_backend_closure_string_bool_capture() {
    assert_parity(
        &corpus_functions("closure_string_bool_capture.mvl"),
        "matches=2\nkept=3\ntotal_len=13",
    );
}

/// Closure as return value: function returning a closure that captures params (#1271).
/// Closures capturing variables can now be returned (#1271 fixed).
#[test]
fn cross_backend_closure_return_value() {
    assert_parity(
        &corpus_functions("closure_return_value.mvl"),
        "add5_result=15\nmul3_result=21",
    );
}

/// Nested closure capture: a closure that captures another closure (#1271).
#[test]
fn cross_backend_closure_nested_capture() {
    assert_parity(
        &corpus_functions("closure_nested_capture.mvl"),
        "composed=26\npipeline=14",
    );
}

// ── #1251: LLVM monomorphization cross-backend tests ─────────────────────────

/// Generic function instantiation — check-only (no fn main).
/// Not a parity test; renamed from cross_backend_check_generic_instantiation (#1272).
#[test]
fn check_generic_instantiation() {
    assert_check_passes(&corpus_functions("generic_instantiation.mvl"));
}

/// Multiple generic instantiations in one program — run and compare output.
#[test]
fn cross_backend_generic_multi_instantiation() {
    assert_parity(
        &corpus_functions("generic_multi_instantiation.mvl"),
        "id_int=42\nid_str=hello\npick_a=10\npick_b=world",
    );
}

/// HOF methods on generic containers: map/fold on List (#1272).
#[test]
fn cross_backend_generic_container_ops() {
    assert_parity(
        &corpus_functions("generic_container_ops.mvl"),
        "opt_count=4\npair_sum=60",
    );
}

/// Nested Option[Int] unwrapping (#1272).
/// LLVM emits some_sum=0 and nested_first=0; both match-arm payload extractions
/// are broken when the payload is Option[Int] or List[Int].
#[test]
fn cross_backend_generic_nested_option() {
    assert_parity(
        &corpus_functions("generic_nested_option.mvl"),
        "some_sum=6\nnested_first=10",
    );
}

// ── #1253: Actor system cross-backend tests ──────────────────────────────────

/// Actor model declarations — check-only.
#[test]
fn cross_backend_actor_corpus_actors() {
    assert_check_passes(&corpus_actors("actors.mvl"));
}

/// Actor capability enforcement — check-only.
#[test]
fn cross_backend_actor_corpus_capabilities() {
    assert_check_passes(&corpus_actors("capabilities.mvl"));
}

/// Actor session types — check-only.
#[test]
fn cross_backend_actor_corpus_session_types() {
    assert_check_passes(&corpus_actors("session_types.mvl"));
}

/// Actor supervisor — check-only.
#[test]
fn cross_backend_actor_corpus_supervisor() {
    assert_check_passes(&corpus_actors("supervisor.mvl"));
}

/// Actor dead letter handling — check-only.
#[test]
fn cross_backend_actor_corpus_dead_letter() {
    assert_check_passes(&corpus_actors("dead_letter.mvl"));
}

/// Actor process links — check-only.
#[test]
fn cross_backend_actor_corpus_process_links() {
    assert_check_passes(&corpus_actors("process_links.mvl"));
}

/// Actor select — check-only.
#[test]
fn cross_backend_actor_corpus_select() {
    assert_check_passes(&corpus_actors("select.mvl"));
}

// ── #1273: Actor runtime parity tests ─────────────────────────────────────────

/// Actor println: spawn actor, call behaviors, verify FIFO output (#1273).
#[test]
fn cross_backend_actor_println_parity() {
    assert_parity(&corpus_actors("actor_println_llvm.mvl"), "ping\npong\nping");
}

/// Actor val arguments: unpacked and printed in order (#1273).
/// (Corpus file name `actor_state_mutation_llvm.mvl` is historical — no mutable state is tested.)
#[test]
fn cross_backend_actor_val_argument_parity() {
    assert_parity(
        &corpus_actors("actor_state_mutation_llvm.mvl"),
        "n=1\nn=2\nn=3\nmsg=done",
    );
}

/// Multiple actors: fan-out from main to two actors (#1273).
#[test]
fn cross_backend_actor_multi_comm_parity() {
    assert_parity(
        &corpus_actors("actor_multi_comm_llvm.mvl"),
        "a_done\nb_done",
    );
}

/// Actor lifecycle: spawn, send behaviors, verify output ordering (#1273).
#[test]
fn cross_backend_actor_lifecycle_parity() {
    assert_parity(
        &corpus_actors("actor_lifecycle_llvm.mvl"),
        "spawned\nsent_3\ndone",
    );
}

/// Bounded mailbox: flooding an actor beyond capacity must not crash (#1273).
#[test]
fn cross_backend_actor_bounded_mailbox_parity() {
    assert_parity(&corpus_actors("actor_bounded_mailbox_llvm.mvl"), "ok");
}

// ── #1254: C runtime cross-backend tests ─────────────────────────────────────

/// SHA256 determinism: same input produces same hash across backends.
#[test]
fn cross_backend_crypto_sha256_corpus_parity() {
    let file = corpus_13_stdlib("crypto_sha256.mvl");
    let transpiler_out = run_transpiler(&file);
    if let Some(llvm_out) = run_llvm_text(&file) {
        assert_eq!(
            llvm_out, transpiler_out,
            "crypto_sha256.mvl: crypto hash must be deterministic across backends"
        );
    }
}

/// Process spawn + wait lifecycle: both backends complete spawn→wait without error (#1274).
#[test]
fn cross_backend_process_echo_parity() {
    assert_parity(
        &corpus_13_stdlib("process_echo.mvl"),
        "spawn_ok=true\nwait_ok=true",
    );
}

// ── #1554: IFC relabel audit trail parity ────────────────────────────────────

/// Strip the timestamp field from a single JSONL audit line so two backends
/// emitting events at different wall-clock seconds compare equal. The line
/// shape is the verbatim format from `mvl_runtime::stdlib::audit`.
fn strip_audit_timestamp(line: &str) -> String {
    let prefix = r#"{"timestamp":""#;
    let Some(rest) = line.strip_prefix(prefix) else {
        return line.to_string();
    };
    match rest.find("\",\"") {
        Some(idx) => format!("{{\"timestamp\":\"<TS>\",{}", &rest[idx + 3..]),
        None => line.to_string(),
    }
}

/// Both backends must produce identical `MVL_AUDIT_SINK` JSONL output (modulo
/// the timestamp) for `relabel ... audit` (#1554, ADR-0049). The corpus uses
/// inline expression composition so the previously transparent LLVM `Relabel`
/// arm has nowhere to hide.
#[test]
fn cross_backend_audit_relabel_sink_parity() {
    if mvl::mvl::backends::llvm_text::lli::find_lli().is_none() {
        return; // environment skip
    }
    let file = corpus_ifc("audit_relabel_runnable.mvl");

    let rust_sink = tempfile::NamedTempFile::new().expect("tempfile rust");
    let llvm_sink = tempfile::NamedTempFile::new().expect("tempfile llvm");
    let _ = std::fs::remove_file(rust_sink.path());
    let _ = std::fs::remove_file(llvm_sink.path());

    let rust_out = Command::new(mvl_bin())
        .args(["run", &file])
        .env("MVL_AUDIT_SINK", rust_sink.path())
        .output()
        .expect("failed to run mvl run");
    assert!(
        rust_out.status.success(),
        "rust backend failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&rust_out.stdout),
        String::from_utf8_lossy(&rust_out.stderr),
    );

    let llvm_out = Command::new(mvl_bin())
        .args(["run", &file, "--backend=llvm"])
        .env("MVL_AUDIT_SINK", llvm_sink.path())
        .output()
        .expect("failed to run mvl run --backend=llvm");
    assert!(
        llvm_out.status.success(),
        "llvm backend failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&llvm_out.stdout),
        String::from_utf8_lossy(&llvm_out.stderr),
    );

    let rust_jsonl =
        std::fs::read_to_string(rust_sink.path()).expect("rust MVL_AUDIT_SINK file missing");
    let llvm_jsonl =
        std::fs::read_to_string(llvm_sink.path()).expect("llvm MVL_AUDIT_SINK file missing");

    let rust_lines: Vec<String> = rust_jsonl.lines().map(strip_audit_timestamp).collect();
    let llvm_lines: Vec<String> = llvm_jsonl.lines().map(strip_audit_timestamp).collect();

    // The corpus emits three `audit`-marked relabels in main; the silent
    // relabel must not appear in either backend's sink.
    assert_eq!(rust_lines.len(), 3, "rust audit line count");
    assert_eq!(
        rust_lines, llvm_lines,
        "llvm_text audit JSONL differs from rust transpiler"
    );
}

// ── #1610: `use std.actors` must not duplicate actor definitions in IR ────────

/// Importing `std.actors` previously caused Supervisor and DeadLetterHandler
/// to be emitted once per `emit_program` call (~5× each), producing IR with
/// invalid redefinitions. The fix tracks already-emitted actor names in
/// `module.actor_emitted`. This test asserts the IR contains exactly one
/// definition of each std.actors dispatch function.
#[test]
fn llvm_std_actors_import_no_duplicate_decls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("import_std_actors.mvl");
    std::fs::write(&src, "use std.actors.{link}\nfn main() -> Unit { }\n").expect("write source");

    let out = Command::new(mvl_bin())
        .args(["build", src.to_str().unwrap(), "--backend=llvm"])
        .current_dir(dir.path())
        .output()
        .expect("mvl build");
    assert!(
        out.status.success(),
        "mvl build --backend=llvm failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let ll = dir.path().join("import_std_actors.ll");
    let ir = std::fs::read_to_string(&ll).expect("read emitted IR");

    for sym in &[
        "@supervisor_dispatch",
        "@supervisor_init",
        "@supervisor_add_child",
        "@dead_letter_handler_dispatch",
        "@dead_letter_handler_undeliverable",
    ] {
        let needle = format!("define void {sym}(");
        let count = ir.matches(needle.as_str()).count();
        assert_eq!(
            count, 1,
            "expected exactly one definition of {sym}, found {count}",
        );
    }
}

// ── #1615: byte_* helpers must emit valid IR ────────────────────────────────

/// std.math.byte_* helpers previously emitted invalid/undef IR on the LLVM
/// backend — `byte_to_string` returned `ret ptr %b` where %b was i8 (an LLVM
/// IR verification error), and `byte_to_int` / bitwise / shift / wrapping
/// helpers returned `ret undef`. This test compiles a program that pulls in
/// `std.math` (transitively, via `use std.actors.{link}`) and checks the
/// emitted IR has well-formed bodies for every byte_* stub.
#[test]
fn llvm_byte_methods_emit_valid_ir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("byte_helpers.mvl");
    std::fs::write(&src, "use std.actors.{link}\nfn main() -> Unit { }\n").expect("write source");

    let out = Command::new(mvl_bin())
        .args(["build", src.to_str().unwrap(), "--backend=llvm"])
        .current_dir(dir.path())
        .output()
        .expect("mvl build");
    assert!(
        out.status.success(),
        "mvl build --backend=llvm failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let ll = dir.path().join("byte_helpers.ll");
    let ir = std::fs::read_to_string(&ll).expect("read emitted IR");

    // Slice out each byte_* function body (from `define ... @byte_X` to the
    // matching closing `}`) and assert no `ret undef` and that the byte_to_string
    // body returns a real ptr — not `ret ptr %b` where %b is i8.
    let expected: &[(&str, &str)] = &[
        ("@byte_to_string", "_mvl_string_new"),
        ("@byte_to_int", "zext i8"),
        ("@byte_bit_and", "and i8"),
        ("@byte_bit_or", "or i8"),
        ("@byte_bit_xor", "xor i8"),
        ("@byte_bit_not", "xor i8"),
        ("@byte_shift_left", "shl i8"),
        ("@byte_shift_right", "lshr i8"),
        ("@byte_wrapping_add", "add i8"),
        ("@byte_wrapping_sub", "sub i8"),
        ("@byte_wrapping_mul", "mul i8"),
    ];

    for (sym, needle) in expected {
        let header = format!(
            "define {} ",
            if sym.contains("to_string") {
                "ptr"
            } else if sym.contains("to_int") {
                "i64"
            } else {
                "i8"
            }
        );
        let start_pat = format!("{header}{sym}(");
        let start = ir
            .find(&start_pat)
            .unwrap_or_else(|| panic!("missing definition for {sym} (looked for `{start_pat}`)"));
        // Find the matching `}` of the function body.
        let after_open = ir[start..]
            .find("\n{\n")
            .map(|i| start + i + 3)
            .unwrap_or_else(|| panic!("malformed body for {sym}"));
        let end = ir[after_open..]
            .find("\n}")
            .map(|i| after_open + i)
            .unwrap_or_else(|| panic!("unterminated body for {sym}"));
        let body = &ir[after_open..end];

        assert!(
            body.contains(needle),
            "{sym} body missing `{needle}`:\n{body}",
        );
        assert!(
            !body.contains("ret i8 undef")
                && !body.contains("ret i64 undef")
                && !body.contains("ret ptr undef"),
            "{sym} still returns undef:\n{body}",
        );
    }
}

// ── #1617: per-iteration / per-branch heap_locals must be dropped before the
// branch joins, not at function-end ──────────────────────────────────────────

/// Before #1617 the LLVM emitter pushed every `let s: String = ...` SSA into a
/// flat function-wide `heap_locals` list and emitted drop calls for the whole
/// list at function-end, even for SSAs defined only inside a branch or loop
/// body. When the branch wasn't taken (or the loop body never entered) those
/// SSAs were undefined where the drop tried to use them — SSA dominance
/// violation, lli rejection.
///
/// Repro: `use std.actors.{link}` + empty main pulls in std.log and std.json,
/// both of which have for-loops with String lets and if-statements with
/// branch-only lets. With the fix the resulting IR runs to completion on lli.
#[test]
fn llvm_branch_and_loop_local_drops_dominate() {
    if mvl::mvl::backends::llvm_text::lli::find_lli().is_none() {
        return; // env skip — no lli available
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("std_actors_runs.mvl");
    std::fs::write(&src, "use std.actors.{link}\nfn main() -> Unit { }\n").expect("write source");

    let out = Command::new(mvl_bin())
        .args(["run", src.to_str().unwrap(), "--backend=llvm"])
        .output()
        .expect("mvl run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "mvl run --backend=llvm failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        stderr,
    );
    assert!(
        !stderr.contains("Instruction does not dominate all uses"),
        "lli reported SSA dominance violation:\n{stderr}",
    );
}
