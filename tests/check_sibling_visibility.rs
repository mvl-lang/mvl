// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Integration tests for cross-file visibility in `mvl check <dir>` (#2204).
//!
//! Spec 005-modules Req 2/3: every cross-file reference needs an explicit
//! `use`, with no directory-scoped exception. Directory mode used to hand each
//! file every other file in the directory as its prelude, so a missing `use`
//! was silently accepted — real spec violations went undetected in the
//! invocation nearly everyone actually runs.
//!
//! These tests exercise the **CLI**, not the checker API, because the defect
//! lived in how `check.rs` *wires up* the prelude rather than in the checker
//! itself. A unit test against `check_with_two_preludes_and_methods_mode`
//! cannot observe it — the enforcement either happens for a real directory
//! invocation or it doesn't.
//!
//! The narrow exception the enforcement must preserve is Go-model method
//! dispatch (#1706): extension methods on a shared receiver type may be split
//! across siblings that call each other in a cycle, which Req 2/3's ban on
//! circular imports means cannot be expressed as an import graph at all.

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
        p.push(format!("mvl-sibvis-{name}-{}", std::process::id()));
        // Wipe any stale dir from a previous run.
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create tempdir");
        Self(p)
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.0.join(name), contents).expect("write fixture");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `mvl check <dir>` and return `(succeeded, combined output)`.
///
/// `MVL_NO_REEXEC=1` is mandatory: without it the CLI re-execs to the
/// project-pinned installed toolchain and the test would silently assert
/// against a different binary than the one just built.
fn check_dir(tmp: &TempDir) -> (bool, String) {
    let out = Command::new(mvl_bin())
        .args(["check", tmp.0.to_str().expect("utf-8 tempdir")])
        .env("MVL_NO_REEXEC", "1")
        .output()
        .expect("failed to run mvl check");
    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), combined)
}

// ── Req 2/3: cross-file references require an explicit `use` ─────────────────

const TYPES_MVL: &str = r#"
pub type Item = struct {
    visible: Bool,
    name:    String,
}

pub type Wrapper = struct {
    items: List[Item],
}
"#;

/// The issue's own repro. This is the regression guard for #2204 itself: if
/// directory mode ever goes back to handing out a blanket prelude, this is the
/// test that notices.
#[test]
fn unimported_cross_file_type_fails_directory_check() {
    let tmp = TempDir::new("type-no-use");
    tmp.write("types.mvl", TYPES_MVL);
    tmp.write(
        "logic.mvl",
        r#"
pub fn find_or_default(w: Wrapper) -> Item {
    w.items.get(0).unwrap_or(Item { visible: false, name: "" })
}
"#,
    );

    let (ok, output) = check_dir(&tmp);
    assert!(
        !ok,
        "cross-file type reference with no `use` must fail directory check, got:\n{output}"
    );
    assert!(
        output.contains("undefined type `Item`"),
        "expected an undefined-type diagnostic naming `Item`, got:\n{output}"
    );
}

/// The other half: adding the `use` must make the identical program pass.
/// Without this, the test above could be satisfied by rejecting everything.
#[test]
fn explicit_use_makes_directory_check_pass() {
    let tmp = TempDir::new("type-with-use");
    tmp.write("types.mvl", TYPES_MVL);
    tmp.write(
        "logic.mvl",
        r#"
use types::Item;
use types::Wrapper;

pub fn find_or_default(w: Wrapper) -> Item {
    w.items.get(0).unwrap_or(Item { visible: false, name: "" })
}
"#,
    );

    let (ok, output) = check_dir(&tmp);
    assert!(
        ok,
        "explicit `use` of a sibling's types must pass directory check, got:\n{output}"
    );
}

/// Free functions were already correctly gated before #2204 — pin it so the
/// methods-only prelude channel can't widen into them.
#[test]
fn unimported_free_function_fails_directory_check() {
    let tmp = TempDir::new("free-fn-no-use");
    tmp.write(
        "helpers.mvl",
        "pub fn free_helper(n: Int) -> Int { n + 1 }\n",
    );
    tmp.write(
        "caller.mvl",
        "pub fn use_free() -> Int { free_helper(1) }\n",
    );

    let (ok, output) = check_dir(&tmp);
    assert!(
        !ok,
        "cross-file free function with no `use` must fail directory check, got:\n{output}"
    );
    assert!(
        output.contains("undefined function `free_helper`"),
        "expected an undefined-function diagnostic, got:\n{output}"
    );
}

// ── #1706: Go-model method dispatch survives the enforcement ────────────────

/// A sibling's extension method on a **user-defined** receiver type resolves
/// without `use`ing the file that declares the method.
#[test]
fn sibling_extension_method_on_user_type_resolves() {
    let tmp = TempDir::new("method-user-ty");
    tmp.write("ctx.mvl", "pub type Ctx = struct { v: Int }\n");
    tmp.write(
        "m1.mvl",
        "use ctx::Ctx;\n\npub fn Ctx::bump(self) -> Int { self.v + 1 }\n",
    );
    tmp.write(
        "m2.mvl",
        "use ctx::Ctx;\n\npub fn call(c: Ctx) -> Int { c.bump() }\n",
    );

    let (ok, output) = check_dir(&tmp);
    assert!(
        ok,
        "same-directory extension method must dispatch without `use` (#1706), got:\n{output}"
    );
}

/// Same, for a **builtin** receiver — `String::`/`List::`/`Map::` is the most
/// common extension-method shape in MVL, and builtins never appear in the
/// checker's type table. A receiver-visibility check written as a bare
/// `lookup_type(..).is_none()` silently drops all of them; `compiler/` happens
/// to contain none, so the self-hosted migration cannot catch this.
#[test]
fn sibling_extension_method_on_builtin_receiver_resolves() {
    let tmp = TempDir::new("method-builtin-ty");
    tmp.write("helpers.mvl", "pub fn String::shout(self) -> Int { 42 }\n");
    tmp.write(
        "caller.mvl",
        "pub fn greet(s: String) -> Int { s.shout() }\n",
    );

    let (ok, output) = check_dir(&tmp);
    assert!(
        ok,
        "extension method on a builtin receiver must dispatch without `use`, got:\n{output}"
    );
}

/// The methods-only channel must expose *methods* and nothing else: a
/// builtin-receiver sibling's free functions stay gated on an explicit `use`.
#[test]
fn builtin_receiver_sibling_does_not_leak_free_functions() {
    let tmp = TempDir::new("builtin-no-leak");
    tmp.write(
        "helpers.mvl",
        "pub fn String::shout(self) -> Int { 42 }\npub fn hidden_helper(n: Int) -> Int { n + 1 }\n",
    );
    tmp.write("caller.mvl", "pub fn f() -> Int { hidden_helper(1) }\n");

    let (ok, output) = check_dir(&tmp);
    assert!(
        !ok,
        "a free function beside a builtin-receiver method must stay gated on `use`, got:\n{output}"
    );
    assert!(
        output.contains("undefined function `hidden_helper`"),
        "expected an undefined-function diagnostic, got:\n{output}"
    );
}
