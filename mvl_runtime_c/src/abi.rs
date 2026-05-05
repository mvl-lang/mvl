//! C-ABI marshalling types for the MVL LLVM runtime boundary.
//!
//! Every value that crosses the C-ABI boundary uses one of these types.
//! LLVM-generated code sees them as opaque structs; the helpers below
//! construct and destructure them on the Rust side.
//!
//! # String ownership
//!
//! Functions that return a string allocate a NUL-terminated `*mut c_char`
//! via [`string_to_c`].  The LLVM-side caller is responsible for freeing
//! the pointer with `libc::free` after use.  Functions that accept a
//! string input receive a `*const c_char` which they **do not** free.
//!
//! # ABI layout
//!
//! ```text
//! MvlOption  { tag: u8 (0=None, 1=Some), payload: *mut c_void }
//! MvlResult  { tag: u8 (0=Ok,   1=Err),  payload: *mut c_void }
//! ```

use libc::{c_char, c_void};
use std::ffi::{CStr, CString};

// ── Option ─────────────────────────────────────────────────────────────────

/// C-ABI representation of `Option[T]`.
///
/// `tag = 0` → None  (`payload` is null)
/// `tag = 1` → Some  (`payload` is a heap-allocated value)
#[repr(C)]
pub struct MvlOption {
    pub tag: u8,
    pub payload: *mut c_void,
}

impl MvlOption {
    /// Construct a `None` value.
    #[inline]
    pub fn none() -> Self {
        MvlOption {
            tag: 0,
            payload: std::ptr::null_mut(),
        }
    }

    /// Construct a `Some` wrapping a heap-allocated `*mut c_char`.
    /// Ownership of `ptr` transfers to the caller of the C-ABI function.
    #[inline]
    pub fn some_str(ptr: *mut c_char) -> Self {
        MvlOption {
            tag: 1,
            payload: ptr as *mut c_void,
        }
    }
}

// ── Result ─────────────────────────────────────────────────────────────────

/// C-ABI representation of `Result[T, E]`.
///
/// `tag = 0` → Ok   (`payload` is the success value; `err` is null)
/// `tag = 1` → Err  (`payload` is null; `err` is a `*mut c_char` error string)
#[repr(C)]
pub struct MvlResult {
    pub tag: u8,
    pub payload: *mut c_void,
    pub err: *mut c_char,
}

impl MvlResult {
    /// Construct an `Ok(())` value (unit success, no payload).
    #[inline]
    pub fn ok_unit() -> Self {
        MvlResult {
            tag: 0,
            payload: std::ptr::null_mut(),
            err: std::ptr::null_mut(),
        }
    }

    /// Construct an `Ok` wrapping a heap-allocated `*mut c_char`.
    #[inline]
    pub fn ok_str(ptr: *mut c_char) -> Self {
        MvlResult {
            tag: 0,
            payload: ptr as *mut c_void,
            err: std::ptr::null_mut(),
        }
    }

    /// Construct an `Err` wrapping a heap-allocated `*mut c_char` error message.
    #[inline]
    pub fn err_str(msg: &str) -> Self {
        MvlResult {
            tag: 1,
            payload: std::ptr::null_mut(),
            err: string_to_c(msg),
        }
    }
}

// ── String conversion helpers ───────────────────────────────────────────────

/// Convert a Rust `&str` to a heap-allocated `*mut c_char`.
/// The caller is responsible for freeing this with `libc::free`.
#[allow(unsafe_code)]
pub fn string_to_c(s: &str) -> *mut c_char {
    // Replace any embedded NUL bytes so CString::new never panics.
    let safe = s.replace('\0', "\u{FFFD}");
    match CString::new(safe) {
        Ok(cs) => cs.into_raw(),
        // Fallback: empty string (should not happen after NUL replacement).
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

/// Convert a `*const c_char` input to a Rust `String`.
///
/// Returns an empty string if `ptr` is null.
/// Returns a replacement-char string if the bytes are not valid UTF-8.
///
/// # Safety
/// `ptr` must either be null or point to a NUL-terminated C string that
/// remains valid for the duration of the call.
#[allow(unsafe_code)]
pub unsafe fn c_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // Safety: caller guarantees ptr is a valid NUL-terminated string.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
