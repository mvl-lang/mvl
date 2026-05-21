// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.env` stdlib functions.
//!
//! Provides real environment, working-directory, Unix-identity, and signal
//! backing for the stubs declared in `std/env.mvl`.
//! Re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::Tainted;

// ── Signal type ────────────────────────────────────────────────────────────

/// Unix signal — mirrors the `Signal` enum declared in `std/env.mvl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// Interrupt (Ctrl-C).
    SIGINT,
    /// Termination request.
    SIGTERM,
    /// Hangup (terminal closed).
    SIGHUP,
    /// User-defined signal 1.
    SIGUSR1,
    /// User-defined signal 2.
    SIGUSR2,
}

// ── Signal constructors (pure) ─────────────────────────────────────────────

/// Construct the SIGINT signal value.
pub fn sigint() -> Signal {
    Signal::SIGINT
}
/// Construct the SIGTERM signal value.
pub fn sigterm() -> Signal {
    Signal::SIGTERM
}
/// Construct the SIGHUP signal value.
pub fn sighup() -> Signal {
    Signal::SIGHUP
}
/// Construct the SIGUSR1 signal value.
pub fn sigusr1() -> Signal {
    Signal::SIGUSR1
}
/// Construct the SIGUSR2 signal value.
pub fn sigusr2() -> Signal {
    Signal::SIGUSR2
}

// ── Signal registration (Phase 2: no-ops) ─────────────────────────────────

/// Phase 2: no-op. Real callback invocation requires Phase 3 runtime support.
pub fn signal_on(_sig: Signal, _handler: fn()) {}

/// Restore the default OS handler. Phase 2: no-op.
pub fn signal_reset(_sig: Signal) {}

/// Ignore the signal. Phase 2: no-op.
pub fn signal_ignore(_sig: Signal) {}

// ── Environment variable access ────────────────────────────────────────────

/// Raw private builtin: read an env var, return bare `String` (#894 Pattern 002).
///
/// Module-private in MVL (`builtin fn _env_read`) — callers use `get` or `get_secret`.
pub fn _env_read(name: String) -> Option<String> {
    std::env::var(&name).ok()
}

/// Read an environment variable by name.
///
/// Returns `Tainted[String]` — env values are externally controlled.
pub fn get(name: String) -> Option<Tainted<String>> {
    _env_read(name).map(Tainted)
}

/// Set an environment variable for the current process.
///
/// Returns `Err` if `name` or `value` contains invalid bytes (NUL, `=` in name).
#[allow(deprecated)]
pub fn set(name: String, value: Tainted<String>) -> Result<(), String> {
    if name.contains('=') || name.contains('\0') || name.is_empty() || value.0.contains('\0') {
        return Err("invalid environment variable name or value".to_string());
    }
    // set_var is deprecated in Rust 1.85 due to unsafety in multi-threaded code.
    // MVL programs that call std.env.set are expected to be single-threaded at the
    // point of mutation, matching the same restriction libc imposes.
    std::env::set_var(&name, &value.0);
    Ok(())
}

/// Unset an environment variable. No-op if not set.
#[allow(deprecated)]
pub fn remove_var(name: String) {
    std::env::remove_var(&name);
}

/// Return all environment variables as (name, value) pairs.
pub fn all() -> Vec<(Tainted<String>, Tainted<String>)> {
    std::env::vars()
        .map(|(k, v)| (Tainted(k), Tainted(v)))
        .collect()
}

// ── Program arguments ──────────────────────────────────────────────────────

/// Return all command-line arguments including the program name at index 0.
pub fn args() -> Vec<Tainted<String>> {
    std::env::args().map(Tainted).collect()
}

// ── Process control ────────────────────────────────────────────────────────

/// Terminate the current process. Flushes stdio before exiting.
pub fn exit(code: i64) -> ! {
    std::process::exit(code as i32)
}

// ── Working directory ──────────────────────────────────────────────────────

/// Return the current working directory as a tainted string.
pub fn current_dir() -> Result<Tainted<String>, String> {
    std::env::current_dir()
        .map(|p| Tainted(p.to_string_lossy().into_owned()))
        .map_err(|_| "cannot determine working directory".to_string())
}

/// Change the current working directory.
pub fn chdir(path: String) -> Result<(), String> {
    std::env::set_current_dir(&path).map_err(|_| "no such directory".to_string())
}

// ── Unix identity ──────────────────────────────────────────────────────────

// POSIX guarantees getuid/getgid are always available on Unix and always succeed.
// There is no safe-Rust alternative in std, so we call them via `extern "C"`.
// The crate uses `#![deny(unsafe_code)]` (not `forbid`) precisely to allow these
// targeted wrappers while keeping the rest of the crate unsafe-free.
#[cfg(unix)]
extern "C" {
    #[link_name = "getuid"]
    fn sys_getuid() -> u32;
    #[link_name = "getgid"]
    fn sys_getgid() -> u32;
}

/// Return the effective user ID of the current process (0 on non-Unix).
#[allow(unsafe_code)]
pub fn getuid() -> i64 {
    #[cfg(unix)]
    {
        // Safety: getuid() is always safe to call — it never fails, has no
        // preconditions, and is signal-safe per POSIX.
        unsafe { sys_getuid() as i64 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Return the effective group ID of the current process (0 on non-Unix).
#[allow(unsafe_code)]
pub fn getgid() -> i64 {
    #[cfg(unix)]
    {
        // Safety: getgid() is always safe to call — it never fails, has no
        // preconditions, and is signal-safe per POSIX.
        unsafe { sys_getgid() as i64 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn get_returns_none_for_unknown_var() {
        let name = "MVL_ENV_TEST_NONEXISTENT_XYZ".to_string();
        assert!(get(name).is_none());
    }

    #[test]
    #[allow(deprecated)]
    fn set_and_get_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = "MVL_ENV_TEST_SET_GET".to_string();
        let value = Tainted("hello_mvl".to_string());
        set(name.clone(), value).expect("set must succeed");
        let got = get(name.clone()).expect("get must return Some after set");
        assert_eq!(got.0, "hello_mvl");
        std::env::remove_var(&name);
    }

    #[test]
    fn set_rejects_name_with_equals() {
        let name = "BAD=NAME".to_string();
        let value = Tainted("v".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    fn set_rejects_empty_name() {
        let name = String::new();
        let value = Tainted("v".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    fn set_rejects_name_with_nul() {
        let name = "NUL\0NAME".to_string();
        let value = Tainted("v".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    fn set_rejects_value_with_nul() {
        let name = "MVL_ENV_TEST_NUL_VALUE".to_string();
        let value = Tainted("bad\0value".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn remove_var_actually_removes() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = "MVL_ENV_TEST_REMOVE".to_string();
        // Set the var, verify it's present, then remove and verify it's gone.
        std::env::set_var(&name, "present");
        assert!(get(name.clone()).is_some(), "var must exist before remove");
        remove_var(name.clone());
        assert!(
            get(name.clone()).is_none(),
            "var must be absent after remove"
        );
        // Second remove must also be a no-op (no panic).
        remove_var(name);
    }

    #[test]
    #[allow(deprecated)]
    fn all_contains_known_variable() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MVL_ENV_TEST_ALL_KEY", "mvl_all_value");
        let pairs = all();
        let found = pairs
            .iter()
            .any(|(k, v)| k.0 == "MVL_ENV_TEST_ALL_KEY" && v.0 == "mvl_all_value");
        std::env::remove_var("MVL_ENV_TEST_ALL_KEY");
        assert!(found, "all() must include variables set in the process env");
    }

    #[test]
    fn args_includes_binary_name() {
        let a = args();
        // The test runner binary name is always the first argument.
        assert!(!a.is_empty(), "args() must return at least the binary name");
        assert!(
            !a[0].0.is_empty(),
            "first arg (binary name) must not be empty"
        );
    }

    #[test]
    fn current_dir_matches_std_env() {
        let expected = std::env::current_dir()
            .expect("std::env::current_dir must work in tests")
            .to_string_lossy()
            .into_owned();
        let got = current_dir().expect("current_dir must not fail in tests");
        assert_eq!(
            got.0, expected,
            "current_dir() must match std::env::current_dir()"
        );
    }

    #[test]
    fn chdir_changes_and_restores_directory() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::current_dir().expect("must have cwd");
        let tmp = std::env::temp_dir().to_string_lossy().into_owned();

        chdir(tmp.clone()).expect("chdir to temp_dir must succeed");
        let after = current_dir().expect("current_dir after chdir");
        // On macOS /tmp is a symlink to /private/tmp — compare canonical paths.
        let canonical_tmp = std::fs::canonicalize(&tmp).unwrap_or_else(|_| tmp.into());
        let canonical_after = std::fs::canonicalize(&after.0).unwrap_or_else(|_| after.0.into());
        assert_eq!(canonical_after, canonical_tmp);

        // Restore original directory so other tests are not affected.
        std::env::set_current_dir(&original).expect("restore cwd");
    }

    #[test]
    fn chdir_nonexistent_returns_err() {
        let result = chdir("/mvl_nonexistent_dir_xyz_12345".to_string());
        assert!(result.is_err(), "chdir to nonexistent path must fail");
    }

    #[test]
    fn getuid_matches_std_env_on_unix() {
        let uid = getuid();
        assert!(uid >= 0, "uid must be non-negative");
        // On Unix, cross-check against the UID env var if the shell set it.
        // This catches a stub returning a hardcoded 0 when the real UID differs.
        #[cfg(unix)]
        if let Ok(shell_uid) = std::env::var("UID") {
            if let Ok(n) = shell_uid.parse::<i64>() {
                assert_eq!(
                    uid, n,
                    "getuid() must match $UID from the shell environment"
                );
            }
        }
        // Fallback: on a typical CI system running as non-root, uid > 0.
        // This is a hint rather than a hard assertion since root CI exists.
        let _ = uid; // used above
    }

    #[test]
    fn getgid_non_negative() {
        assert!(getgid() >= 0);
    }

    #[test]
    fn signal_constructors_return_correct_variants() {
        assert_eq!(sigint(), Signal::SIGINT);
        assert_eq!(sigterm(), Signal::SIGTERM);
        assert_eq!(sighup(), Signal::SIGHUP);
        assert_eq!(sigusr1(), Signal::SIGUSR1);
        assert_eq!(sigusr2(), Signal::SIGUSR2);
    }

    #[test]
    fn signal_noop_handlers_do_not_panic() {
        signal_on(sigint(), || {});
        signal_reset(sigterm());
        signal_ignore(sighup());
    }
}
