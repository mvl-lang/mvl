// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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

use crate::abi::{LlvmEnumError, LlvmResult};
use crate::memory::{_mvl_string_drop, _mvl_string_new, MvlString};
use libc::c_void;
use mvl_runtime::stdlib::regex as rt;

// ── RegexError discriminants (must match variant order in std/regex.mvl) ──────
const REGEX_ERR_INVALID_PATTERN: u8 = 0;

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
    unsafe { _mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void }
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
        Err(rt::RegexError::InvalidPattern(msg)) => {
            LlvmResult::err_mvl(LlvmEnumError::with_str(REGEX_ERR_INVALID_PATTERN, &msg))
        }
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

// ── Find (Option[Match]) ──────────────────────────────────────────────────────

/// C repr of `Match` — the payload returned by `_mvl_regex_find`.
///
/// Layout mirrors `type Match = struct { text: String, start: Int, end: Int }` in
/// `std/regex.mvl`.  Fields are ordered to match the MVL struct definition so that
/// the LLVM codegen can GEP into this layout using field indices.
///
/// # Safety
/// Heap-allocated by `_mvl_regex_find`; caller must free with `_mvl_match_drop`.
#[repr(C)]
pub struct MvlMatch {
    /// The matched text (heap-allocated MvlString).
    pub text: *mut MvlString,
    /// Byte offset of the start of the match.
    pub start: i64,
    /// Byte offset one past the end of the match.
    pub end: i64,
}

/// Free a `MvlMatch` previously allocated by `_mvl_regex_find`.
///
/// # Safety
/// `m` must be a non-null pointer from `_mvl_regex_find` and must not be used after this call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_match_drop(m: *mut MvlMatch) {
    if !m.is_null() {
        // Free the inner MvlString before dropping the struct itself.
        if !(*m).text.is_null() {
            _mvl_string_drop((*m).text);
        }
        drop(Box::from_raw(m));
    }
}

/// Return the first match of `handle` in `input`, or `None`.
///
/// Returns `LlvmResult { tag=0, payload=*mut MvlMatch }` on a match (caller owns the
/// allocation; free with `_mvl_match_drop`).
/// Returns `LlvmResult { tag=1, payload=null }` when there is no match.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_regex_find(
    handle: *mut c_void,
    input: *const MvlString,
) -> LlvmResult {
    if handle.is_null() {
        return LlvmResult::none();
    }
    let s = read_mvl_string(input);
    let re: &rt::Regex = &*(handle as *const rt::Regex);
    match re.find_borrowed(&s) {
        Some(m) => {
            let heap = Box::new(MvlMatch {
                text: _mvl_string_new(m.text.as_ptr(), m.text.len()),
                start: m.start,
                end: m.end,
            });
            LlvmResult::ok_ptr(Box::into_raw(heap) as *mut c_void)
        }
        None => LlvmResult::none(),
    }
}

// ── find_all (List[Match]) ────────────────────────────────────────────────────

/// Return all non-overlapping matches of `handle` in `input` as a heap `MvlArray*`
/// of `*mut MvlMatch` pointers (elem_size = 8).
///
/// Each element of the returned array is a heap-allocated `*mut MvlMatch`;
/// the caller is responsible for freeing every element with `_mvl_match_drop`
/// and the array itself with `mvl_array_drop`.
///
/// Returns an empty (zero-length) array if `handle` is null or there are no matches.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_regex_find_all(
    handle: *mut c_void,
    input: *const MvlString,
) -> *mut crate::memory::MvlArray {
    use crate::memory::{_mvl_array_new, MvlArray};
    use crate::memory_ops::_mvl_array_push;
    let arr: *mut MvlArray = _mvl_array_new(std::mem::size_of::<*mut MvlMatch>(), 0);
    if handle.is_null() {
        return arr;
    }
    let s = read_mvl_string(input);
    let re: &rt::Regex = &*(handle as *const rt::Regex);
    for m in re.find_all_borrowed(&s) {
        let heap = Box::new(MvlMatch {
            text: _mvl_string_new(m.text.as_ptr(), m.text.len()),
            start: m.start,
            end: m.end,
        });
        let ptr: *mut MvlMatch = Box::into_raw(heap);
        _mvl_array_push(arr, (&ptr as *const *mut MvlMatch).cast());
    }
    arr
}

// ── Deferred: captures (Option[Captures]) ─────────────────────────────────────
//
// `_mvl_regex_captures` has complex return types (Option[Captures] requires
// nested List[Option[String]] and Map[String, Option[String]]).
// LLVM codegen dispatch for this is deferred until that infrastructure is in place.

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a MvlString* from a Rust str for testing.
    unsafe fn mvl_str(s: &str) -> *mut MvlString {
        let bytes = s.as_bytes();
        _mvl_string_new(bytes.as_ptr(), bytes.len())
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
        // Free the LlvmEnumError box (inner MvlString string leaks — acceptable for MVP).
        unsafe { drop(Box::from_raw(r.payload as *mut LlvmEnumError)) };
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

    #[test]
    fn find_some_match_returns_ok_with_correct_fields() {
        let r = unsafe { _mvl_regex_compile(mvl_str(r"\d+")) };
        assert_eq!(r.tag, 0);

        let result = unsafe { _mvl_regex_find(r.payload, mvl_str("abc 123 def")) };
        assert_eq!(result.tag, 0, "expected Some (tag=0)");
        assert!(!result.payload.is_null());

        let m = result.payload as *mut MvlMatch;
        let text = unsafe { read_mvl_string((*m).text as *const MvlString) };
        assert_eq!(text, "123");
        assert_eq!(unsafe { (*m).start }, 4);
        assert_eq!(unsafe { (*m).end }, 7);

        unsafe { _mvl_match_drop(m) };
        _mvl_regex_drop(r.payload);
    }

    #[test]
    fn find_no_match_returns_none() {
        let r = unsafe { _mvl_regex_compile(mvl_str(r"\d+")) };
        assert_eq!(r.tag, 0);

        let result = unsafe { _mvl_regex_find(r.payload, mvl_str("no digits here")) };
        assert_eq!(result.tag, 1, "expected None (tag=1)");
        assert!(result.payload.is_null(), "None payload must be null");

        _mvl_regex_drop(r.payload);
    }

    #[test]
    fn match_drop_null_is_safe() {
        // _mvl_match_drop(null) must not panic or segfault.
        unsafe { _mvl_match_drop(std::ptr::null_mut()) };
    }
}
