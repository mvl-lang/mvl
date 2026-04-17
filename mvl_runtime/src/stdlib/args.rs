//! Rust implementations of `std.args` stdlib functions.
//!
//! Provides real CLI argument and environment access for the stubs declared
//! in `std/args.mvl`. Re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::{Clean, Tainted};

/// Scan command-line arguments for `--<name> <value>` and return the value.
///
/// Searches `std::env::args()` for the flag `--<name>` and returns the
/// following argument as `Tainted<String>` (untrusted external input).
/// Returns `None` if the flag is absent or has no following argument.
///
/// Implements the Rust backing for `std/args.mvl::get_arg`.
pub fn get_arg(name: Clean<String>) -> Option<Tainted<String>> {
    let flag = format!("--{}", *name);
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return Some(Tainted(args[i + 1].clone()));
        }
    }
    None
}

/// Return all command-line arguments (excluding the program name) as Tainted values.
///
/// Implements the Rust backing for `std/args.mvl::get_args`.
pub fn get_args() -> Vec<Tainted<String>> {
    std::env::args().skip(1).map(Tainted).collect()
}

/// Read an environment variable by name.
///
/// Returns `None` if the variable is not set.
/// Returns `Tainted<String>` because environment variables are externally controlled.
///
/// Implements the Rust backing for `std/args.mvl::get_env`.
pub fn get_env(name: Clean<String>) -> Option<Tainted<String>> {
    std::env::var(&*name).ok().map(Tainted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_arg_returns_none_for_unknown_flag() {
        // In unit tests the binary name is the test runner — no --mvl-unknown flag.
        let name = Clean("mvl-unknown-flag-xyz".to_string());
        assert!(get_arg(name).is_none());
    }

    #[test]
    fn get_args_returns_vec() {
        // Just verify it doesn't panic and returns a Vec.
        let args = get_args();
        // In unit test context, args may include test runner flags.
        let _ = args;
    }

    #[test]
    fn get_env_returns_none_for_unknown_var() {
        let name = Clean("MVL_NONEXISTENT_VAR_XYZ_12345".to_string());
        assert!(get_env(name).is_none());
    }

    #[test]
    fn get_env_returns_value_when_set() {
        std::env::set_var("MVL_TEST_GET_ENV", "hello");
        let name = Clean("MVL_TEST_GET_ENV".to_string());
        let val = get_env(name);
        assert!(val.is_some());
        assert_eq!(val.unwrap().0, "hello");
        std::env::remove_var("MVL_TEST_GET_ENV");
    }
}
