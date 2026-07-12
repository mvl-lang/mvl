//! Regression test for #1788: CLI subcommands `mvl tir` and `mvl mutate` must
//! load pure-MVL stdlib extras (via `loader::load_mvl_native_stdlib_extras`)
//! in addition to the implicit prelude.
//!
//! Symptom before the fix: `mvl tir` on a program that `use`s a pure-MVL stdlib
//! module (e.g. `std.log`) type-checked with unresolved names — the emitted TIR
//! JSON silently had stdlib expression types serialized as `{"tag": "Unknown"}`.

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

/// #1788: TIR emission for a program using `std.log` must not silently drop
/// stdlib expression types to `Ty::Unknown`. Before the fix, `parse_log_level`
/// was unresolved and its call expression serialized as `{"tag": "Unknown"}`.
#[test]
fn mvl_tir_resolves_stdlib_extras() {
    let src = r#"use std.log.{parse_log_level, LogLevel}

fn f() -> LogLevel {
    parse_log_level("info")
}
"#;
    let tmp = TempDir::new("tir-stdlib");
    let file = tmp.write("main.mvl", src);
    // Run from the tempdir so the workspace's mvl.toml `requires-mvl` pin does
    // not trigger a re-exec to an installed toolchain binary (which would mask
    // the bug fix under test).
    let out = Command::new(mvl_bin())
        .args(["tir", file.to_str().unwrap()])
        .current_dir(&tmp.0)
        .output()
        .expect("run mvl tir");
    assert!(
        out.status.success(),
        "mvl tir failed:\n  stdout: {}\n  stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Without the fix, the call `parse_log_level("info")` would have expression
    // type `Ty::Unknown`, serialized as `{"tag": "Unknown"}` in the TIR JSON.
    assert!(
        !stdout.contains(r#""tag": "Unknown""#),
        "TIR output contains unresolved stdlib types (missing load_mvl_native_stdlib_extras):\n{stdout}",
    );
}
