//! C-ABI exports for `std.regex` stdlib functions — LLVM backend path (#420/#439).
//!
//! Mirrors `mvl_runtime::stdlib::regex`. Every public function has a
//! corresponding `_mvl_regex_*` symbol callable from LLVM-generated code
//! via `lli --load=libmvl_runtime_c`.
//!
//! # String convention
//!
//! String arguments are `*const MvlString` — LLVM heap-allocated string objects.
//! String return values are `*mut c_void` cast from `*mut MvlString` (LLVM heap strings).
//! The `read_mvl_string` / `new_mvl_str` helpers do the Rust↔MvlString conversion.
//!
//! # Regex handle ownership
//!
//! `_mvl_regex_compile` returns a `LlvmResult` whose Ok payload is a
//! `Box<rt::Regex>` cast to `*mut c_void`. The LLVM caller owns the
//! allocation and must free it via `_mvl_regex_drop` when done.
//! All other functions borrow the handle — they do NOT free it.

use std::slice;

use libc::c_void;
use mvl_memory::{mvl_string_new, MvlString};
use mvl_runtime::stdlib::regex as rt;

use crate::abi::LlvmResult;

// ── MvlString helpers ─────────────────────────────────────────────────────────

/// Read a `MvlString*` as a Rust `String`.
#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (*s).len as usize;
    if len == 0 || (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = slice::from_raw_parts((*s).ptr as *const u8, len);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Allocate a new heap `MvlString` from a Rust `&str`, returning `*mut c_void`.
#[allow(unsafe_code)]
fn new_mvl_str(s: &str) -> *mut c_void {
    let bytes = s.as_bytes();
    unsafe { mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void }
}

// ── Regex lifecycle ────────────────────────────────────────────────────────────

/// Compile a regex pattern.
///
/// Returns `LlvmResult { tag=0, payload=*mut c_void }` on success where
/// `payload` is a heap-allocated `Box<rt::Regex>` cast to `*mut c_void`.
/// Returns `LlvmResult { tag=1, payload=*mut MvlString }` on failure.
///
/// The caller must free a successful result with `_mvl_regex_drop`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_regex_compile(pattern: *const MvlString) -> LlvmResult {
    let pat = read_mvl_string(pattern);
    match rt::compile(pat) {
        Ok(re) => {
            let ptr = Box::into_raw(Box::new(re)) as *mut c_void;
            LlvmResult::ok_ptr(ptr)
        }
        Err(e) => LlvmResult::err_mvl(new_mvl_str(&e)),
    }
}

/// Free a compiled regex handle previously returned by `_mvl_regex_compile`.
///
/// # Safety
/// `handle` must be a non-null pointer from a successful `_mvl_regex_compile`
/// call and must not be used after this call.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_regex_drop(handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(handle as *mut rt::Regex)) };
}

// ── Match operations ───────────────────────────────────────────────────────────

/// Replace all matches of `handle` in `input` with `replacement`.
///
/// Returns a heap-allocated `*mut MvlString` cast to `*mut c_void` (caller owns).
/// Returns null if `handle` is null.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_regex_replace(
    handle: *mut c_void,
    input: *const MvlString,
    replacement: *const MvlString,
) -> *mut c_void {
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    let s = read_mvl_string(input);
    let repl = read_mvl_string(replacement);
    let re_ref: &rt::Regex = &*(handle as *const rt::Regex);
    let result = re_ref.replace_all_borrowed(&s, &repl);
    new_mvl_str(&result)
}

// ── Deferred (pending Option[Struct] / List marshalling) ──────────────────────
//
// `_mvl_regex_find`, `_mvl_regex_find_all`, `_mvl_regex_captures` have
// complex return types (Option[Match], List[Match], Option[Captures]) that
// require MvlArray*/MvlStruct* marshalling not yet in place.
// Their C-ABI symbols will be added in the follow-up ticket.

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a MvlString* from a Rust str for testing.
    unsafe fn mvl_str(s: &str) -> *mut MvlString {
        let bytes = s.as_bytes();
        mvl_string_new(bytes.as_ptr(), bytes.len())
    }

    #[test]
    fn compile_valid_returns_ok() {
        let r = unsafe { _mvl_regex_compile(mvl_str(r"\d+")) };
        assert_eq!(r.tag, 0, "valid pattern must return Ok");
        assert!(!r.payload.is_null());
        _mvl_regex_drop(r.payload);
    }

    #[test]
    fn compile_invalid_returns_err() {
        let r = unsafe { _mvl_regex_compile(mvl_str(r"[unclosed")) };
        assert_eq!(r.tag, 1, "invalid pattern must return Err");
        assert!(!r.payload.is_null());
        // Don't free the MvlString* — it's owned by the GC/arena in tests
    }

    #[test]
    fn replace_substitutes_all_matches() {
        let r = unsafe { _mvl_regex_compile(mvl_str(r"\d+")) };
        assert_eq!(r.tag, 0);

        let out_ptr = unsafe { _mvl_regex_replace(r.payload, mvl_str("a1b22c333"), mvl_str("N")) };
        assert!(!out_ptr.is_null());

        let out_str = unsafe { read_mvl_string(out_ptr as *const MvlString) };
        assert_eq!(out_str, "aNbNcN");

        _mvl_regex_drop(r.payload);
    }

    #[test]
    fn replace_null_handle_returns_null() {
        let out =
            unsafe { _mvl_regex_replace(std::ptr::null_mut(), mvl_str("hello"), mvl_str("x")) };
        assert!(out.is_null());
    }
}
