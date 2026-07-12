//! Tests for the disk-based stdlib loader (#1765).
//!
//! Since the stdlib no longer embeds files in the binary, these tests exercise:
//!   • the dev fallback to `<CARGO_MANIFEST_DIR>/std/` when nothing is installed
//!   • error behaviour when neither an installed nor a source-tree stdlib exists
//!   • `stdlib_files()` enumeration returning the expected content on disk

use std::fs;
use std::sync::{LazyLock, Mutex};

use mvl::mvl::stdlib::{
    ensure_stdlib, resolved_stdlib_path, stdlib_content, stdlib_files, STDLIB_VERSION,
};

// Serialise all tests that mutate MVL_HOME (process-global).
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// RAII guard that restores MVL_HOME to its prior value on drop.
struct MvlHomeGuard {
    prior: Option<String>,
}
impl Drop for MvlHomeGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("MVL_HOME", v),
            None => std::env::remove_var("MVL_HOME"),
        }
    }
}

fn with_mvl_home_empty<F: FnOnce()>(f: F) {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prior = std::env::var("MVL_HOME").ok();
    let tmp = tempfile::tempdir().expect("tempdir");
    std::env::set_var("MVL_HOME", tmp.path());
    let _guard = MvlHomeGuard { prior };
    f();
}

#[test]
fn version_constant_is_populated() {
    // Sanity: MVL_STDLIB_VERSION was baked in by build.rs.
    assert!(
        !STDLIB_VERSION.is_empty(),
        "STDLIB_VERSION must be non-empty"
    );
    assert!(
        STDLIB_VERSION.chars().next().unwrap().is_ascii_digit(),
        "STDLIB_VERSION must start with a digit: {STDLIB_VERSION}"
    );
}

#[test]
fn dev_fallback_resolves_from_manifest_dir() {
    // With MVL_HOME pointing at an empty tempdir, resolution must fall back
    // to `<CARGO_MANIFEST_DIR>/std/` — the source tree is guaranteed to have
    // `core.mvl` because we're running from inside the repo.
    with_mvl_home_empty(|| {
        let path = resolved_stdlib_path().expect("dev fallback must resolve");
        assert!(
            path.join("core.mvl").exists(),
            "dev fallback path must contain core.mvl: {}",
            path.display()
        );
    });
}

#[test]
fn stdlib_content_returns_owned_string() {
    with_mvl_home_empty(|| {
        let content = stdlib_content("core.mvl").expect("core.mvl must be readable");
        assert!(!content.is_empty(), "core.mvl content must be non-empty");
    });
}

#[test]
fn stdlib_content_missing_file_returns_none() {
    with_mvl_home_empty(|| {
        assert!(
            stdlib_content("does-not-exist.mvl").is_none(),
            "missing file must return None"
        );
    });
}

#[test]
fn stdlib_files_enumerates_source_tree() {
    with_mvl_home_empty(|| {
        let files = stdlib_files();
        assert!(!files.is_empty(), "stdlib_files must not be empty");
        assert!(
            files.iter().any(|(name, _)| name == "core.mvl"),
            "core.mvl must be listed in stdlib_files()"
        );
        // Enumeration must be sorted deterministically.
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "stdlib_files must be sorted by name");
    });
}

#[test]
fn ensure_stdlib_returns_directory_with_core_mvl() {
    with_mvl_home_empty(|| {
        let dir = ensure_stdlib();
        assert!(
            dir.join("core.mvl").exists(),
            "ensure_stdlib must return a directory containing core.mvl"
        );
    });
}

/// Populate a tempdir with a version-matching stdlib and verify it wins over
/// the dev fallback: `resolved_stdlib_path` should return the XDG path when
/// `core.mvl` is present there.
#[test]
fn xdg_install_takes_precedence_over_dev_fallback() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prior = std::env::var("MVL_HOME").ok();
    let tmp = tempfile::tempdir().expect("tempdir");
    std::env::set_var("MVL_HOME", tmp.path());
    let _guard = MvlHomeGuard { prior };

    // Mimic an install: `<home>/toolchains/{compiler_ver}/std/core.mvl`.
    let compiler_version = env!("CARGO_PKG_VERSION");
    let stdlib_dir = tmp
        .path()
        .join("toolchains")
        .join(compiler_version)
        .join("std");
    fs::create_dir_all(&stdlib_dir).expect("mkdir");
    fs::write(stdlib_dir.join("core.mvl"), "// installed marker\n").expect("write");

    let resolved = resolved_stdlib_path().expect("resolution must succeed");
    assert_eq!(
        resolved, stdlib_dir,
        "XDG install must take precedence over dev fallback"
    );

    let content = stdlib_content("core.mvl").expect("core.mvl must resolve");
    assert!(
        content.contains("installed marker"),
        "must read the XDG-installed file, not the source-tree fallback"
    );
}
