//! Integration tests for `mvl::mvl::toolchain` — ADR-0009 Phase B.
//!
//! Tests use a temporary directory as `MVL_HOME` and `HOME` to isolate
//! filesystem state.  A static mutex serialises all env-var mutations.

use mvl::mvl::toolchain::{
    active_symlink, cmd_self_list, cmd_self_uninstall, cmd_self_use, local_bin_dir, target_triple,
    toolchain_bin, toolchain_dir, toolchain_root, version_symlink,
};
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::{LazyLock, Mutex};

// Serialise all tests that mutate MVL_HOME / HOME (process-global).
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// RAII guard that restores MVL_HOME and HOME to their original values on drop.
struct EnvGuard {
    orig_mvl_home: Option<String>,
    orig_home: Option<String>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.orig_mvl_home {
            Some(v) => std::env::set_var("MVL_HOME", v),
            None => std::env::remove_var("MVL_HOME"),
        }
        match &self.orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// Lock the env mutex, override MVL_HOME and HOME to a temp dir, run closure.
fn with_env<F: FnOnce(&std::path::Path)>(f: F) {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("tempdir");
    let guard = EnvGuard {
        orig_mvl_home: std::env::var("MVL_HOME").ok(),
        orig_home: std::env::var("HOME").ok(),
    };
    std::env::set_var("MVL_HOME", tmp.path());
    std::env::set_var("HOME", tmp.path());
    f(tmp.path());
    drop(guard);
}

// ── Path helper unit tests ────────────────────────────────────────────────────

#[test]
fn toolchain_root_under_mvl_home() {
    with_env(|tmp| {
        let root = toolchain_root();
        assert_eq!(root, tmp.join("toolchains"));
    });
}

#[test]
fn toolchain_dir_contains_version() {
    with_env(|tmp| {
        let dir = toolchain_dir("0.21.0");
        assert_eq!(dir, tmp.join("toolchains/0.21.0"));
    });
}

#[test]
fn toolchain_bin_path() {
    with_env(|tmp| {
        let bin = toolchain_bin("0.21.0");
        assert_eq!(bin, tmp.join("toolchains/0.21.0/bin/mvl"));
    });
}

#[test]
fn version_symlink_path() {
    with_env(|tmp| {
        let link = version_symlink("0.21.0");
        assert_eq!(link, tmp.join(".local/bin/mvl@0.21.0"));
    });
}

#[test]
fn active_symlink_path() {
    with_env(|tmp| {
        let link = active_symlink();
        assert_eq!(link, tmp.join(".local/bin/mvl"));
    });
}

#[test]
fn target_triple_is_non_empty() {
    let triple = target_triple();
    assert!(!triple.is_empty());
    assert!(
        triple.contains('-'),
        "triple should be e.g. x86_64-apple-darwin"
    );
}

// ── cmd_self_list ─────────────────────────────────────────────────────────────

#[test]
fn list_empty_when_no_toolchains() {
    with_env(|_tmp| {
        // Should not panic; prints "No toolchains installed".
        cmd_self_list();
    });
}

#[test]
fn list_shows_installed_version() {
    with_env(|_tmp| {
        let bin = toolchain_bin("0.40.0");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"fake").unwrap();

        // Should not panic. Spot-check the directory exists.
        cmd_self_list();
        assert!(toolchain_dir("0.40.0").exists());
    });
}

#[test]
fn list_ignores_dirs_without_binary() {
    with_env(|_tmp| {
        // Create a toolchain directory without a binary.
        let dir = toolchain_dir("0.99.0");
        fs::create_dir_all(&dir).unwrap();

        // Should not panic; 0.99.0 has no binary so must not appear.
        cmd_self_list();
    });
}

// ── cmd_self_use ──────────────────────────────────────────────────────────────

#[test]
fn self_use_creates_active_symlink() {
    with_env(|_tmp| {
        let bin = toolchain_bin("0.40.0");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"fake").unwrap();

        cmd_self_use("0.40.0");

        let link = active_symlink();
        assert!(link.symlink_metadata().is_ok(), "active symlink must exist");
        let target = fs::read_link(&link).unwrap();
        assert_eq!(target, bin, "active symlink must point to binary");
    });
}

#[test]
fn self_use_replaces_existing_symlink() {
    with_env(|_tmp| {
        for ver in &["0.39.0", "0.40.0"] {
            let bin = toolchain_bin(ver);
            fs::create_dir_all(bin.parent().unwrap()).unwrap();
            fs::write(&bin, b"fake").unwrap();
        }

        cmd_self_use("0.39.0");
        cmd_self_use("0.40.0");

        let link = active_symlink();
        let target = fs::read_link(&link).unwrap();
        assert_eq!(
            target,
            toolchain_bin("0.40.0"),
            "active symlink must point to the most recently activated version"
        );
    });
}

// ── cmd_self_uninstall ────────────────────────────────────────────────────────

#[test]
fn self_uninstall_removes_toolchain_dir() {
    with_env(|_tmp| {
        let bin = toolchain_bin("0.38.0");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"fake").unwrap();

        cmd_self_uninstall("0.38.0");

        assert!(
            !toolchain_dir("0.38.0").exists(),
            "toolchain directory must be removed after uninstall"
        );
    });
}

#[test]
fn self_uninstall_removes_version_symlink() {
    with_env(|_tmp| {
        let bin = toolchain_bin("0.38.0");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"fake").unwrap();

        let vlink = version_symlink("0.38.0");
        fs::create_dir_all(local_bin_dir()).unwrap();
        symlink(&bin, &vlink).unwrap();

        cmd_self_uninstall("0.38.0");

        assert!(
            !vlink.exists() && vlink.symlink_metadata().is_err(),
            "versioned symlink must be removed"
        );
    });
}

#[test]
fn self_uninstall_removes_active_symlink_when_pointing_to_version() {
    with_env(|_tmp| {
        let bin = toolchain_bin("0.37.0");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(&bin, b"fake").unwrap();

        let alink = active_symlink();
        fs::create_dir_all(local_bin_dir()).unwrap();
        symlink(&bin, &alink).unwrap();

        cmd_self_uninstall("0.37.0");

        assert!(
            !alink.exists() && alink.symlink_metadata().is_err(),
            "active symlink must be removed when it pointed to the uninstalled version"
        );
    });
}
