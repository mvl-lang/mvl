// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.env` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::env`. Every public function in that module
//! has a corresponding `_mvl_env_*` symbol here, callable from LLVM IR via
//! `lli --load=libmvl_runtime_llvm.{dylib,so}`.
//!
//! # String ownership
//!
//! - Input strings (`*const c_char`): owned by the caller, not freed here.
//! - Output strings inside `MvlOption`/`MvlResult`: heap-allocated by this
//!   crate. The LLVM caller is responsible for freeing the inner `*mut c_char`
//!   with `libc::free` after use.

use libc::c_char;
use mvl_runtime::ifc::Tainted;

use crate::abi::{c_to_string, string_to_c, MvlOption, MvlResult};

// ── Primitive returns (no marshalling) ─────────────────────────────────────

/// Return the effective user ID of the current process.
/// `Int` return — no marshalling required.
#[no_mangle]
pub extern "C" fn _mvl_env_getuid() -> i64 {
    mvl_runtime::stdlib::env::getuid()
}

/// Return the effective group ID of the current process.
/// `Int` return — no marshalling required.
#[no_mangle]
pub extern "C" fn _mvl_env_getgid() -> i64 {
    mvl_runtime::stdlib::env::getgid()
}

// ── Environment variable access ─────────────────────────────────────────────

/// Read an environment variable by name.
///
/// Returns `MvlOption { tag=1, payload=*mut c_char }` on success (caller frees),
/// or `MvlOption { tag=0, payload=null }` when the variable is not set.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_env_get(name: *const c_char) -> MvlOption {
    let key = unsafe { c_to_string(name) };
    match mvl_runtime::stdlib::env::get(key) {
        Some(Tainted(s)) => MvlOption::some_str(string_to_c(&s)),
        None => MvlOption::none(),
    }
}

/// Set an environment variable.
///
/// Returns `MvlResult { tag=0 }` on success, `MvlResult { tag=1, err=msg }`
/// on failure (e.g. name contains `=` or is empty).
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_env_set(name: *const c_char, value: *const c_char) -> MvlResult {
    let n = unsafe { c_to_string(name) };
    let v = unsafe { c_to_string(value) };
    match mvl_runtime::stdlib::env::set(n, Tainted(v)) {
        Ok(()) => MvlResult::ok_unit(),
        Err(e) => MvlResult::err_str(&e),
    }
}

/// Unset an environment variable. No-op if not set.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_env_remove_var(name: *const c_char) {
    let n = unsafe { c_to_string(name) };
    mvl_runtime::stdlib::env::remove_var(n);
}

// ── Working directory ───────────────────────────────────────────────────────

/// Return the current working directory.
///
/// Returns `MvlResult { tag=0, payload=*mut c_char }` on success (caller frees),
/// or `MvlResult { tag=1, err=msg }` on failure.
#[no_mangle]
pub extern "C" fn _mvl_env_current_dir() -> MvlResult {
    match mvl_runtime::stdlib::env::current_dir() {
        Ok(Tainted(s)) => MvlResult::ok_str(string_to_c(&s)),
        Err(e) => MvlResult::err_str(&e),
    }
}

/// Change the current working directory.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_env_chdir(path: *const c_char) -> MvlResult {
    let p = unsafe { c_to_string(path) };
    match mvl_runtime::stdlib::env::chdir(p) {
        Ok(()) => MvlResult::ok_unit(),
        Err(e) => MvlResult::err_str(&e),
    }
}

// ── Process control ─────────────────────────────────────────────────────────

/// Terminate the current process with the given exit code.
/// This function does not return.
#[no_mangle]
pub extern "C" fn _mvl_env_exit(code: i64) -> ! {
    mvl_runtime::stdlib::env::exit(code)
}

// ── Program arguments ────────────────────────────────────────────────────────

/// Return the number of command-line arguments (including the program name).
#[no_mangle]
pub extern "C" fn _mvl_env_args_len() -> i64 {
    mvl_runtime::stdlib::env::args().len() as i64
}

/// Return the command-line argument at index `i` as a heap-allocated C string.
/// Returns null if `i` is out of range. Caller frees with `libc::free`.
#[no_mangle]
pub extern "C" fn _mvl_env_args_get(i: i64) -> *mut c_char {
    if i < 0 {
        return std::ptr::null_mut();
    }
    let args = mvl_runtime::stdlib::env::args();
    match args.into_iter().nth(i as usize) {
        Some(Tainted(s)) => string_to_c(&s),
        None => std::ptr::null_mut(),
    }
}

/// Return all command-line arguments as a `*mut MvlArray` of `*mut MvlString`.
///
/// Includes the program name at index 0. Each element is a `Tainted[String]`.
/// The LLVM caller is responsible for dropping the array via `_mvl_array_drop`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_env_args() -> *mut crate::memory::MvlArray {
    use crate::memory::{_mvl_array_new, _mvl_string_new, MvlString};
    let elem_size = std::mem::size_of::<*mut MvlString>();
    let arr = unsafe { _mvl_array_new(elem_size, 0) };
    for mvl_runtime::ifc::Tainted(s) in mvl_runtime::stdlib::env::args() {
        let s_ptr = unsafe { _mvl_string_new(s.as_ptr(), s.len()) };
        unsafe {
            crate::memory_ops::_mvl_array_push(arr, (&s_ptr as *const *mut MvlString).cast());
        }
    }
    arr
}

// ── Signal constructors (pure) ───────────────────────────────────────────────

// Signal values are encoded as i8 integers at the C boundary:
//   0 = SIGINT, 1 = SIGTERM, 2 = SIGHUP, 3 = SIGUSR1, 4 = SIGUSR2

/// Construct the SIGINT signal value (0).
#[no_mangle]
pub extern "C" fn _mvl_env_sigint() -> i8 {
    0
}

/// Construct the SIGTERM signal value (1).
#[no_mangle]
pub extern "C" fn _mvl_env_sigterm() -> i8 {
    1
}

/// Construct the SIGHUP signal value (2).
#[no_mangle]
pub extern "C" fn _mvl_env_sighup() -> i8 {
    2
}

/// Construct the SIGUSR1 signal value (3).
#[no_mangle]
pub extern "C" fn _mvl_env_sigusr1() -> i8 {
    3
}

/// Construct the SIGUSR2 signal value (4).
#[no_mangle]
pub extern "C" fn _mvl_env_sigusr2() -> i8 {
    4
}

/// Register a signal handler (no-op stub; real libc integration tracked separately).
///
/// `handler` is a function pointer (`fn() -> Unit`) cast to `*mut c_void`.
/// The callback model requires no-captures, so the trampoline pattern is safe
/// for named functions.  Real dispatch deferred to Phase 3 (#45).
#[no_mangle]
pub extern "C" fn _mvl_env_signal_on(_sig: i8, _handler: *mut libc::c_void) {}

/// No-op signal registration (Phase 2: callbacks not yet implemented).
#[no_mangle]
pub extern "C" fn _mvl_env_signal_reset(_sig: i8) {}

/// No-op signal ignore (Phase 2: callbacks not yet implemented).
#[no_mangle]
pub extern "C" fn _mvl_env_signal_ignore(_sig: i8) {}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString};
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[test]
    fn getuid_non_negative() {
        assert!(_mvl_env_getuid() >= 0);
    }

    #[test]
    fn getgid_non_negative() {
        assert!(_mvl_env_getgid() >= 0);
    }

    #[test]
    fn env_get_missing_returns_none() {
        let key = CString::new("MVL_RC_TEST_MISSING_XYZ").unwrap();
        let opt = _mvl_env_get(key.as_ptr());
        assert_eq!(opt.tag, 0);
        assert!(opt.payload.is_null());
    }

    #[test]
    #[allow(deprecated)]
    fn env_get_set_roundtrip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let name = CString::new("MVL_RC_TEST_GET_SET").unwrap();
        let val = CString::new("mvl_runtime_c_test").unwrap();

        let set_result = _mvl_env_set(name.as_ptr(), val.as_ptr());
        assert_eq!(set_result.tag, 0, "set must succeed");

        let opt = _mvl_env_get(name.as_ptr());
        assert_eq!(opt.tag, 1, "get must return Some after set");

        #[allow(unsafe_code)]
        let got = unsafe { CStr::from_ptr(opt.payload as *const libc::c_char) }
            .to_str()
            .unwrap();
        assert_eq!(got, "mvl_runtime_c_test");

        // Free the returned string.
        #[allow(unsafe_code)]
        unsafe {
            libc::free(opt.payload)
        };

        std::env::remove_var("MVL_RC_TEST_GET_SET");
    }

    #[test]
    fn env_set_invalid_name_returns_err() {
        let name = CString::new("INVALID=NAME").unwrap();
        let val = CString::new("v").unwrap();
        let r = _mvl_env_set(name.as_ptr(), val.as_ptr());
        assert_eq!(r.tag, 1, "set with = in name must return Err");
        // Free the error string.
        #[allow(unsafe_code)]
        unsafe {
            libc::free(r.err as *mut libc::c_void)
        };
    }

    #[test]
    fn args_len_positive() {
        assert!(_mvl_env_args_len() > 0);
    }

    #[test]
    fn args_get_zero_returns_binary_name() {
        let ptr = _mvl_env_args_get(0);
        assert!(!ptr.is_null());
        #[allow(unsafe_code)]
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert!(!s.is_empty());
        #[allow(unsafe_code)]
        unsafe {
            libc::free(ptr as *mut libc::c_void)
        };
    }

    #[test]
    fn args_get_out_of_range_returns_null() {
        let ptr = _mvl_env_args_get(i64::MAX);
        assert!(ptr.is_null());
    }

    #[test]
    fn args_get_negative_returns_null() {
        let ptr = _mvl_env_args_get(-1);
        assert!(ptr.is_null(), "negative index must return null");
    }

    #[test]
    fn current_dir_returns_ok() {
        let r = _mvl_env_current_dir();
        assert_eq!(r.tag, 0, "current_dir must succeed");
        assert!(!r.payload.is_null());
        #[allow(unsafe_code)]
        unsafe {
            libc::free(r.payload)
        };
    }

    #[test]
    fn signal_constructors_return_correct_values() {
        assert_eq!(_mvl_env_sigint(), 0);
        assert_eq!(_mvl_env_sigterm(), 1);
        assert_eq!(_mvl_env_sighup(), 2);
        assert_eq!(_mvl_env_sigusr1(), 3);
        assert_eq!(_mvl_env_sigusr2(), 4);
    }
}
