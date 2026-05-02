//! C-ABI marshalling types for values that cross the LLVM ↔ Rust boundary (ADR-0018).
//!
//! These types are opaque structs from the LLVM backend's perspective.  The LLVM IR
//! treats them as `ptr` and never inspects their fields directly.
//!
//! # Layout
//!
//! | MVL type  | C struct                                          |
//! |-----------|---------------------------------------------------|
//! | Option<T> | `{ u8 tag, *mut c_void payload }`                 |
//! | Result<T> | `{ u8 tag, *mut c_void ok_payload, *mut c_void err_payload }` |
//!
//! `MvlString*`, `MvlArray*`, and `MvlMap*` are reused from `mvl_memory`.

use libc::c_void;

/// C-ABI representation of an MVL `Option<T>`.
///
/// `tag = 0` → None.  `tag = 1` → Some; `payload` points to the value.
#[repr(C)]
pub struct MvlOption {
    pub tag: u8,
    pub payload: *mut c_void,
}

/// C-ABI representation of an MVL `Result<T, E>`.
///
/// `tag = 0` → Ok; `ok_payload` is the value.
/// `tag = 1` → Err; `err_payload` is the error.
#[repr(C)]
pub struct MvlResult {
    pub tag: u8,
    pub ok_payload: *mut c_void,
    pub err_payload: *mut c_void,
}

impl MvlOption {
    pub fn none() -> Self {
        MvlOption {
            tag: 0,
            payload: std::ptr::null_mut(),
        }
    }

    pub fn some(payload: *mut c_void) -> Self {
        MvlOption { tag: 1, payload }
    }
}

impl MvlResult {
    pub fn ok(ok_payload: *mut c_void) -> Self {
        MvlResult {
            tag: 0,
            ok_payload,
            err_payload: std::ptr::null_mut(),
        }
    }

    pub fn err(err_payload: *mut c_void) -> Self {
        MvlResult {
            tag: 1,
            ok_payload: std::ptr::null_mut(),
            err_payload,
        }
    }
}
