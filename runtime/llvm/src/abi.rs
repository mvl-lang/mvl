// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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

// ── LLVM-path Result ───────────────────────────────────────────────────────

/// `{i8, ptr}` — the 2-field Result layout used by the LLVM backend.
///
/// Distinct from [`MvlResult`] (3 fields, used by the C-ABI env/time/random
/// path). The LLVM codegen emits `{i8, ptr}` struct types; functions called
/// via `emit_stdlib_call_result_*` must return this layout.
///
/// `tag = 0` → Ok  — `payload` is null (Unit) or `*mut MvlString` (String or opaque).
/// `tag = 1` → Err — `payload` is `*mut MvlString` (error message).
#[repr(C)]
pub struct LlvmResult {
    pub tag: u8,
    pub payload: *mut c_void,
}

impl LlvmResult {
    /// Construct a `None` / missing-value result — tag=1, null payload.
    #[inline]
    pub fn none() -> Self {
        LlvmResult {
            tag: 1,
            payload: std::ptr::null_mut(),
        }
    }

    /// Construct an `Ok(ptr)` — opaque heap pointer (e.g. boxed Regex handle or MvlString*).
    #[inline]
    pub fn ok_ptr(ptr: *mut c_void) -> Self {
        LlvmResult {
            tag: 0,
            payload: ptr,
        }
    }

    /// Construct an `Err` with a `*mut MvlString` error message.
    #[inline]
    pub fn err_mvl(msg: *mut c_void) -> Self {
        LlvmResult {
            tag: 1,
            payload: msg,
        }
    }
}

// ── Enum error ABI ─────────────────────────────────────────────────────────

/// Heap-allocated enum error value for the LLVM stdlib error path.
///
/// Layout matches the LLVM IR type `{ i8, [8 x i8] }` that the codegen
/// generates for payload enums where the largest variant payload is one
/// pointer (8 bytes).  Both fields have alignment 1, so there is no padding.
///
/// - `disc`    — variant discriminant (0-based, matches MVL enum declaration order)
/// - `payload` — pointer-sized payload bytes; zeroed for unit variants,
///               contains a `*mut MvlString` (little-endian) for `Other(String)`.
///
/// The LLVM codegen receives this via `LlvmResult { tag=1, payload=*mut LlvmEnumError }`.
/// It follows the pointer, reads the discriminant at field 0, and optionally
/// reads the string pointer from field 1 for the `Other` variant.
#[repr(C)]
pub struct LlvmEnumError {
    pub disc: u8,
    pub payload: [u8; 8],
}

impl LlvmEnumError {
    /// Allocate a unit variant (no payload).  `disc` is the 0-based variant index.
    #[inline]
    #[allow(unsafe_code)]
    pub fn unit(disc: u8) -> *mut c_void {
        Box::into_raw(Box::new(LlvmEnumError {
            disc,
            payload: [0u8; 8],
        })) as *mut c_void
    }

    /// Allocate a variant whose sole payload is a `String` (stored as `*mut MvlString`).
    /// `disc` is the 0-based variant index.  `msg` is copied into a new `MvlString`.
    ///
    /// # Safety
    /// The caller is responsible for the lifetime of the returned pointer.
    /// The `MvlString` embedded in the payload is heap-allocated and currently
    /// leaked (acceptable for MVP error paths).
    #[inline]
    #[allow(unsafe_code)]
    pub fn with_str(disc: u8, msg: &str) -> *mut c_void {
        use crate::memory::mvl_string_new;
        let bytes = msg.as_bytes();
        let str_ptr = unsafe { mvl_string_new(bytes.as_ptr(), bytes.len()) as usize };
        let mut payload = [0u8; 8];
        payload.copy_from_slice(&str_ptr.to_ne_bytes());
        Box::into_raw(Box::new(LlvmEnumError { disc, payload })) as *mut c_void
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
