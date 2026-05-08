//! C-ABI exports for `std.args` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::args`. Provides typed argument and
//! environment access at the C-ABI boundary for the LLVM backend.
//!
//! # LLVM dispatch coverage
//!
//! - `_mvl_args_get_args` — () → *mut MvlArray (List[Tainted[String]])
//!
//! `_mvl_args_get_arg` and `_mvl_args_get_env` return `Option[Tainted[String]]`
//! with a ptr argument — not yet in the dispatch table (needs OptionStrOnePtrArg).

use libc::c_char;
use mvl_runtime::ifc::{Clean, Tainted};

use crate::abi::{c_to_string, string_to_c, MvlOption};

/// Return all command-line arguments (excluding the program name) as a
/// `*mut MvlArray` of `*mut MvlString`.
///
/// The LLVM caller is responsible for dropping the array via `mvl_array_drop`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_args_get_args() -> *mut mvl_memory::MvlArray {
    use mvl_memory::{mvl_array_new, mvl_string_new, MvlString};
    let elem_size = std::mem::size_of::<*mut MvlString>();
    let arr = unsafe { mvl_array_new(elem_size, 0) };
    for Tainted(s) in mvl_runtime::stdlib::args::get_args() {
        let s_ptr = unsafe { mvl_string_new(s.as_ptr(), s.len()) };
        unsafe {
            crate::memory_ops::mvl_array_push(arr, (&s_ptr as *const *mut MvlString).cast());
        }
    }
    arr
}

/// Look up `--<name> <value>` in the command-line arguments.
///
/// Returns `MvlOption { tag=1, payload=*mut c_char }` on success (caller frees),
/// or `MvlOption { tag=0, payload=null }` when the flag is absent.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_args_get_arg(name: *const c_char) -> MvlOption {
    let key = unsafe { c_to_string(name) };
    match mvl_runtime::stdlib::args::get_arg(Clean(key)) {
        Some(Tainted(s)) => MvlOption::some_str(string_to_c(&s)),
        None => MvlOption::none(),
    }
}

/// Read an environment variable by name.
///
/// Returns `MvlOption { tag=1, payload=*mut c_char }` on success (caller frees),
/// or `MvlOption { tag=0, payload=null }` when the variable is not set.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_args_get_env(name: *const c_char) -> MvlOption {
    let key = unsafe { c_to_string(name) };
    match mvl_runtime::stdlib::args::get_env(Clean(key)) {
        Some(Tainted(s)) => MvlOption::some_str(string_to_c(&s)),
        None => MvlOption::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_args_returns_array() {
        use mvl_memory::{mvl_array_drop, MvlArray};
        let arr = _mvl_args_get_args();
        assert!(!arr.is_null());
        // At least zero elements (no CLI args in test binary)
        let len = unsafe { crate::memory_ops::mvl_array_len(arr as *const MvlArray) };
        assert!(len < 1000, "sanity check: length is sane");
        unsafe { mvl_array_drop(arr) };
    }

    #[test]
    fn get_arg_missing_returns_none() {
        use std::ffi::CString;
        let name = CString::new("MVL_TEST_ARG_MISSING_XYZ").unwrap();
        let opt = _mvl_args_get_arg(name.as_ptr());
        assert_eq!(opt.tag, 0);
        assert!(opt.payload.is_null());
    }

    #[test]
    fn get_env_missing_returns_none() {
        use std::ffi::CString;
        let name = CString::new("MVL_TEST_ENV_MISSING_XYZ").unwrap();
        let opt = _mvl_args_get_env(name.as_ptr());
        assert_eq!(opt.tag, 0);
        assert!(opt.payload.is_null());
    }
}
