//! C-ABI exports for `std.env` — wraps `mvl_runtime::stdlib::env` (#432).
//!
//! # String ownership
//!
//! Functions that return strings (`_mvl_env_get`, `_mvl_env_current_dir`) return
//! a heap-allocated null-terminated C string. The caller is responsible for
//! freeing it with `_mvl_env_free_cstr`. `None` / error is indicated by a null
//! return.
//!
//! # LLVM codegen integration status
//!
//! | Symbol                  | Codegen wired? |
//! |-------------------------|----------------|
//! | `_mvl_env_getuid`       | yes (#432)     |
//! | `_mvl_env_getgid`       | yes (#432)     |
//! | `_mvl_env_exit`         | yes (#432)     |
//! | `_mvl_env_get`          | pending        |
//! | `_mvl_env_set_var`      | pending        |
//! | `_mvl_env_remove_var`   | pending        |
//! | `_mvl_env_current_dir`  | pending        |
//! | `_mvl_env_args_count`   | pending        |
//! | `_mvl_env_args_get`     | pending        |
//! | `_mvl_env_free_cstr`    | pending        |

use libc::c_char;
use mvl_runtime::{ifc::Clean, stdlib::env};
use std::ffi::{CStr, CString};

// ── Helpers ───────────────────────────────────────────────────────────────────

unsafe fn cstr_to_string(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    CStr::from_ptr(s).to_string_lossy().into_owned()
}

fn string_to_cstr(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

// ── Identity (no FFI conversion needed) ──────────────────────────────────────

/// Returns the effective user ID of the current process.
#[no_mangle]
pub extern "C" fn _mvl_env_getuid() -> i64 {
    env::getuid()
}

/// Returns the effective group ID of the current process.
#[no_mangle]
pub extern "C" fn _mvl_env_getgid() -> i64 {
    env::getgid()
}

// ── Process control ───────────────────────────────────────────────────────────

/// Terminate the process with the given exit code.
#[no_mangle]
pub extern "C" fn _mvl_env_exit(code: i64) -> ! {
    env::exit(code)
}

// ── Environment variables ─────────────────────────────────────────────────────

/// Read an environment variable. Returns a heap-allocated C string (caller must
/// free with `_mvl_env_free_cstr`), or null if the variable is not set.
#[no_mangle]
pub unsafe extern "C" fn _mvl_env_get(key: *const c_char) -> *mut c_char {
    let name = cstr_to_string(key);
    match env::get(Clean(name)) {
        None => std::ptr::null_mut(),
        Some(val) => string_to_cstr(val.0),
    }
}

/// Set an environment variable. Returns 0 on success, 1 on error.
#[no_mangle]
pub unsafe extern "C" fn _mvl_env_set_var(key: *const c_char, val: *const c_char) -> i32 {
    let name = cstr_to_string(key);
    let value = cstr_to_string(val);
    match env::set(Clean(name), mvl_runtime::ifc::Tainted(value)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// Remove an environment variable. No-op if not set.
#[no_mangle]
pub unsafe extern "C" fn _mvl_env_remove_var(key: *const c_char) {
    let name = cstr_to_string(key);
    env::remove_var(Clean(name));
}

/// Return the current working directory as a heap-allocated C string
/// (caller must free with `_mvl_env_free_cstr`), or null on error.
#[no_mangle]
pub extern "C" fn _mvl_env_current_dir() -> *mut c_char {
    match env::current_dir() {
        Ok(path) => string_to_cstr(path.0),
        Err(_) => std::ptr::null_mut(),
    }
}

// ── Arguments (split into count + indexed access to avoid MvlArray) ──────────

/// Return the number of command-line arguments (including program name).
#[no_mangle]
pub extern "C" fn _mvl_env_args_count() -> i64 {
    env::args().len() as i64
}

/// Return the i-th command-line argument as a heap-allocated C string
/// (caller must free with `_mvl_env_free_cstr`), or null if out of bounds.
#[no_mangle]
pub extern "C" fn _mvl_env_args_get(i: i64) -> *mut c_char {
    let args = env::args();
    if i < 0 || i as usize >= args.len() {
        return std::ptr::null_mut();
    }
    string_to_cstr(args[i as usize].0.clone())
}

// ── Memory ────────────────────────────────────────────────────────────────────

/// Free a C string returned by any `_mvl_env_*` function.
///
/// # Safety
/// `s` must be a pointer previously returned by an `_mvl_env_*` function and
/// must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn _mvl_env_free_cstr(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
