//! Rust implementations of `std.env` stdlib functions.
//!
//! Provides real environment, working-directory, Unix-identity, and signal
//! backing for the stubs declared in `std/env.mvl`.
//! Re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::{Clean, Tainted};

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

/// Read an environment variable by name.
pub fn get(name: Clean<String>) -> Option<Tainted<String>> {
    std::env::var(&**name).ok().map(Tainted)
}

/// Set an environment variable for the current process.
///
/// Returns `Err` if `name` contains `=` or a NUL byte (both are invalid on POSIX).
#[allow(deprecated)]
pub fn set(name: Clean<String>, value: Tainted<String>) -> Result<(), String> {
    let n = &**name;
    if n.contains('=') || n.contains('\0') || n.is_empty() {
        return Err("invalid environment variable name".to_string());
    }
    // set_var is deprecated in Rust 1.85 due to unsafety in multi-threaded code.
    // MVL programs that call std.env.set are expected to be single-threaded at the
    // point of mutation, matching the same restriction libc imposes.
    std::env::set_var(n, &**value);
    Ok(())
}

/// Unset an environment variable. No-op if not set.
#[allow(deprecated)]
pub fn remove_var(name: Clean<String>) {
    std::env::remove_var(&**name);
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
pub fn chdir(path: Clean<String>) -> Result<(), String> {
    std::env::set_current_dir(&**path).map_err(|_| "no such directory".to_string())
}

// ── Unix identity ──────────────────────────────────────────────────────────

// `mvl_runtime` forbids unsafe_code (#![forbid(unsafe_code)]), so we cannot
// call libc::getuid / libc::getgid directly. For Phase 2 we parse the UID/GID
// from `/proc/self/status` on Linux, and return 0 on platforms where that file
// is absent (macOS, Windows). A safe libc wrapper can replace this in Phase 3
// once the crate-level lint is relaxed for this specific use.

/// Return the effective user ID of the current process.
///
/// Reads from `/proc/self/status` on Linux. Returns 0 on other platforms.
pub fn getuid() -> i64 {
    read_id_from_proc_status("Uid:")
}

/// Return the effective group ID of the current process.
///
/// Reads from `/proc/self/status` on Linux. Returns 0 on other platforms.
pub fn getgid() -> i64 {
    read_id_from_proc_status("Gid:")
}

/// Parse the effective (second) ID from a `/proc/self/status` line.
///
/// The format is: `Uid:\t<real>\t<effective>\t<saved>\t<filesystem>`
fn read_id_from_proc_status(key: &str) -> i64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix(key) {
                    // Fields are tab-separated; the effective ID is the second field.
                    let mut fields = rest.split_whitespace();
                    let _real = fields.next();
                    if let Some(effective) = fields.next() {
                        if let Ok(id) = effective.parse::<i64>() {
                            return id;
                        }
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = key;
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
        let name = Clean("MVL_ENV_TEST_NONEXISTENT_XYZ".to_string());
        assert!(get(name).is_none());
    }

    #[test]
    #[allow(deprecated)]
    fn set_and_get_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = Clean("MVL_ENV_TEST_SET_GET".to_string());
        let value = Tainted("hello_mvl".to_string());
        set(name.clone(), value).expect("set must succeed");
        let got = get(name.clone()).expect("get must return Some after set");
        assert_eq!(got.0, "hello_mvl");
        std::env::remove_var(&**&name);
    }

    #[test]
    fn set_rejects_name_with_equals() {
        let name = Clean("BAD=NAME".to_string());
        let value = Tainted("v".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    fn set_rejects_empty_name() {
        let name = Clean(String::new());
        let value = Tainted("v".to_string());
        assert!(set(name, value).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn remove_var_is_idempotent() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = Clean("MVL_ENV_TEST_REMOVE_IDEMPOTENT".to_string());
        remove_var(name.clone());
        remove_var(name); // second call must not panic
    }

    #[test]
    fn all_returns_non_empty_list() {
        let pairs = all();
        assert!(!pairs.is_empty(), "at least one env var must be set");
    }

    #[test]
    fn args_returns_vec_of_tainted() {
        let a = args();
        // Test runner has at least the binary name.
        assert!(!a.is_empty());
    }

    #[test]
    fn current_dir_returns_ok() {
        let dir = current_dir();
        assert!(dir.is_ok(), "current_dir must not fail in tests");
    }

    #[test]
    fn getuid_non_negative() {
        assert!(getuid() >= 0);
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
