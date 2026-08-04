//! CLI coverage for `--target=<name>` under `--backend=wasm` (#2093 Phase 2).
//! `mvl build --backend=wasm --target=wasm-browser` emits the same WAT as
//! every other target — the difference is entirely in which runtime module
//! gets linked in at instantiation time (see `runtime/wasm-browser/`,
//! ADR-0063) — so `build` accepts it like any other target. `mvl test
//! --backend=wasm --target=wasm-browser` still rejects: `cmd_test_wasm`'s
//! harness is wasmtime-based and has no browser/JS host to run against.

use std::io::Write;
use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop(); // test binary
    p.pop(); // deps/
    p.push("mvl");
    p
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let mut p = std::env::temp_dir();
        p.push(format!("mvl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create tempdir");
        Self(p)
    }

    fn write(&self, name: &str, contents: &str) -> std::path::PathBuf {
        let p = self.0.join(name);
        let mut f = std::fs::File::create(&p).expect("create file in tempdir");
        f.write_all(contents.as_bytes()).expect("write file");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const HELLO: &str = "fn main() -> Unit { }\n";

#[test]
fn wasm_build_accepts_default_and_wasi_targets() {
    let tmp = TempDir::new("wasm-target-ok");
    let file = tmp.write("hello.mvl", HELLO);
    for target_flag in [None, Some("--target=wasi"), Some("--target=default")] {
        let mut args = vec![
            "build".to_string(),
            "hello.mvl".to_string(),
            "--backend=wasm".to_string(),
        ];
        if let Some(flag) = target_flag {
            args.push(flag.to_string());
        }
        let out = Command::new(mvl_bin())
            .args(&args)
            .current_dir(&tmp.0)
            .output()
            .expect("run mvl build");
        assert!(
            out.status.success(),
            "mvl build --backend=wasm {target_flag:?} failed:\n  stdout: {}\n  stderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            file.with_extension("wat").exists(),
            "expected hello.wat to be written"
        );
    }
}

#[test]
fn wasm_build_accepts_wasm_browser_target() {
    let tmp = TempDir::new("wasm-target-browser");
    let file = tmp.write("hello.mvl", HELLO);
    let out = Command::new(mvl_bin())
        .args([
            "build",
            "hello.mvl",
            "--backend=wasm",
            "--target=wasm-browser",
        ])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl build");
    assert!(
        out.status.success(),
        "mvl build --backend=wasm --target=wasm-browser failed:\n  stdout: {}\n  stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        file.with_extension("wat").exists(),
        "expected hello.wat to be written"
    );
}

#[test]
fn wasm_test_rejects_wasm_browser_target_until_implemented() {
    let tmp = TempDir::new("wasm-target-browser-test");
    tmp.write("hello.mvl", "// expect: ok\nfn main() -> Unit { }\n");
    let out = Command::new(mvl_bin())
        .args([
            "test",
            "hello.mvl",
            "--backend=wasm",
            "--target=wasm-browser",
        ])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl test");
    assert!(
        !out.status.success(),
        "expected --target=wasm-browser to be rejected until implemented"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wasm-browser") && stderr.contains("not yet implemented"),
        "expected a clear not-yet-implemented error, got:\n{stderr}"
    );
}

#[test]
fn wasm_build_rejects_unknown_target() {
    let tmp = TempDir::new("wasm-target-unknown");
    tmp.write("hello.mvl", HELLO);
    let out = Command::new(mvl_bin())
        .args(["build", "hello.mvl", "--backend=wasm", "--target=bogus"])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl build");
    assert!(
        !out.status.success(),
        "expected unknown target to be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown target 'bogus'") && stderr.contains("wasm-browser"),
        "expected unknown-target error to list wasm-browser as a supported value, got:\n{stderr}"
    );
}

#[test]
fn run_backend_wasm_is_rejected_with_a_clear_message() {
    let tmp = TempDir::new("wasm-run-unsupported");
    tmp.write("hello.mvl", HELLO);
    let out = Command::new(mvl_bin())
        .args(["run", "hello.mvl", "--backend=wasm"])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl run");
    assert!(
        !out.status.success(),
        "expected `mvl run --backend=wasm` to be rejected"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not supported") && stderr.contains("mvl build --backend=wasm"),
        "expected a clear not-supported error pointing at `mvl build`, got:\n{stderr}"
    );
}

#[test]
fn non_wasm_backend_target_validation_is_unaffected() {
    let tmp = TempDir::new("non-wasm-target");
    tmp.write("hello.mvl", HELLO);
    // `tokio` is a valid target for the default (non-wasm) backend and must
    // still be accepted now that target validation branches on backend.
    let out = Command::new(mvl_bin())
        .args(["build", "hello.mvl", "--target=tokio", "--emit-only"])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl build");
    assert!(
        out.status.success(),
        "mvl build --target=tokio failed:\n  stdout: {}\n  stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // `wasm-browser` is only meaningful for --backend=wasm; the default
    // backend must still reject it as an unknown target.
    let out = Command::new(mvl_bin())
        .args(["build", "hello.mvl", "--target=wasm-browser", "--emit-only"])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl build");
    assert!(
        !out.status.success(),
        "expected --target=wasm-browser to be rejected on the default backend"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown target 'wasm-browser'"),
        "expected unknown-target error, got:\n{stderr}"
    );
}
