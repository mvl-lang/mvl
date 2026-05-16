// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.config` stdlib functions — LLVM backend path (#804).
//!
//! Single export: `_mvl_config_load` — resolves path (XDG convention), parses
//! TOML or JSON, applies env overlay, returns the root ConfigValue.
//!
//! # Return layout
//!
//! Uses `LlvmResult { tag: u8, payload: *mut c_void }` (same as io.rs / net.rs):
//! - tag=0 Ok:  payload = `*mut ConfigValue` (heap-allocated, free with `_mvl_config_value_drop`)
//! - tag=1 Err: payload = `*mut MvlString`   (error message)

use crate::memory::{mvl_string_new, MvlString};
use libc::c_void;
use mvl_runtime::stdlib::config::ConfigError;

use super::io::LlvmResult;

// ── Helpers ───────────────────────────────────────────────────────────────────

#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = unsafe { (*s).len as usize };
    if len == 0 || unsafe { (*s).ptr.is_null() } {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts((*s).ptr as *const u8, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn new_mvl_str(s: &str) -> *mut c_void {
    let bytes = s.as_bytes();
    #[allow(unsafe_code)]
    unsafe {
        mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void
    }
}

// ── C-ABI export ──────────────────────────────────────────────────────────────

/// Load config following the standard layered convention (C-ABI wrapper).
///
/// `path`   — null → auto (XDG + local); non-null → explicit path string.
/// `prefix` — env var prefix string (e.g. "MYAPP"); null or empty → no overlay.
///
/// Returns `LlvmResult { tag=0, payload=*mut ConfigValue }` on success.
/// The ConfigValue is heap-allocated; free with `_mvl_config_value_drop`.
/// Returns `LlvmResult { tag=1, payload=*mut MvlString }` on error (caller frees).
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_config_load(
    path: *const MvlString,
    prefix: *const MvlString,
) -> LlvmResult {
    let path_opt: Option<String> = if path.is_null() {
        None
    } else {
        Some(unsafe { read_mvl_string(path) })
    };
    let prefix_str = if prefix.is_null() {
        String::new()
    } else {
        unsafe { read_mvl_string(prefix) }
    };

    match mvl_runtime::stdlib::config::load_config(path_opt, prefix_str) {
        Ok(val) => LlvmResult {
            tag: 0,
            payload: Box::into_raw(Box::new(val)) as *mut c_void,
        },
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(&e.to_string()),
        },
    }
}

/// Drop a heap-allocated `ConfigValue` produced by `_mvl_config_load`.
///
/// # Safety
/// `ptr` must be a non-null pointer returned as an Ok payload by `_mvl_config_load`.
/// Calling twice is undefined behaviour.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_config_value_drop(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(Box::from_raw(
            ptr as *mut mvl_runtime::stdlib::config::ConfigValue,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::mvl_string_new;

    fn make_mvl_str(s: &str) -> *const MvlString {
        let bytes = s.as_bytes();
        unsafe { mvl_string_new(bytes.as_ptr(), bytes.len()) }
    }

    #[test]
    fn load_nonexistent_absolute_returns_err() {
        let path = make_mvl_str("/nonexistent/mvl_test_config.toml");
        let result = unsafe { _mvl_config_load(path, std::ptr::null()) };
        assert_eq!(result.tag, 1);
    }

    #[test]
    fn load_null_path_does_not_crash() {
        // No XDG dirs set in test env — returns Ok or Err, never panics.
        let result = unsafe { _mvl_config_load(std::ptr::null(), std::ptr::null()) };
        assert!(result.tag == 0 || result.tag == 1);
    }
}
