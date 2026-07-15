// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL runtime for the WASM backend (#1819, epic #1817 phase 2).
//!
//! Compiled to `wasm32-wasip1` as a `cdylib`. Loaded by `wasmtime` via
//! `--preload runtime=<path>` alongside emitted user code — the emitter's
//! `(import "runtime" "_mvl_string_*" ...)` declarations resolve to the
//! symbols exported here.
//!
//! ## Scope today
//!
//! Group A — no allocation, `(ptr, len)` in, primitive out:
//! - `_mvl_string_eq` — bytewise equality
//! - `_mvl_string_len` — length as i64
//! - `_mvl_string_is_empty` — `len == 0`
//! - `_mvl_string_contains` — byte substring search
//! - `_mvl_string_starts_with` / `_mvl_string_ends_with` — prefix / suffix
//! - `_mvl_string_find` — byte position or `-1`
//!
//! Group B (first drop — this commit) — allocation, returns `*MvlString`
//! whose fields the emitter unpacks back into the `(ptr, len)`
//! representation everything else uses:
//! - `MvlString` struct — `{ ptr, len, cap, rc }` all `i32`, matches the
//!   `runtime/llvm/` layout (i64→i32 fields for wasm32 addressing).
//! - `_mvl_string_concat` — allocates a fresh buffer + `MvlString`.
//!
//! No `_mvl_string_drop` yet — Group B commit 1 leaks concat allocations.
//! Refcount / drop lands with commit 2 alongside `_mvl_string_clone` and
//! emitter-side scope-exit drop emission. Fine for tests.
//!
//! ## Symbol convention
//!
//! `#[unsafe(no_mangle)] pub extern "C" fn _mvl_string_*` — same prefix
//! and ABI as `runtime/llvm/` (which uses both `_mvl_string_*` and
//! `_mvl_str_*` inconsistently; we settle on `_mvl_string_*` throughout).
//!
//! Safety: the emitter passes valid `(ptr, len)` ranges. String literals
//! live in the module's data section; `Int.to_string()` output lives in
//! the bump-allocated region past `heap_start`. The runtime treats the
//! ranges as `&[u8]` slices; UB on caller misuse is inherent to the FFI
//! boundary.

// ── Slice helpers ────────────────────────────────────────────────────────
//
// Every function takes `(ptr, len)` arguments. `slice_or_empty` handles
// the pathological "empty string with null pointer" case — string
// literals for `""` don't get a data-section address, so the caller may
// pass `ptr = 0`. Rust's `slice::from_raw_parts` rejects null under
// debug-assertion checks; short-circuit to `&[]` before it can.

#[inline]
unsafe fn slice_or_empty<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if len <= 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) }
}

// ── Query ops ────────────────────────────────────────────────────────────

/// `s.len()` — length in bytes as i64. The receiver `len` is already on
/// the stack; this fn exists so the emitter dispatches uniformly through
/// `runtime.wasm` rather than open-coding stack juggling to convert the
/// receiver's `(ptr, len)` pair back to just `len as i64`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_string_len(_ptr: i32, len: i32) -> i64 {
    len as i64
}

/// `s.is_empty()` — 1 when `len == 0`, else 0. Same rationale as `len`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_string_is_empty(_ptr: i32, len: i32) -> i32 {
    if len == 0 {
        1
    } else {
        0
    }
}

/// `s.contains(needle)` — 1 if `needle` occurs anywhere in `s`, else 0.
/// Empty `needle` matches at position 0 by convention.
///
/// Safety: both slices are re-created via `slice_or_empty` — sound for
/// any `(ptr, len)` the emitter can produce.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_contains(sp: i32, sl: i32, np: i32, nl: i32) -> i32 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let n = unsafe { slice_or_empty(np, nl) };
    if find_bytes(s, n).is_some() {
        1
    } else {
        0
    }
}

/// `s.starts_with(prefix)` — 1 iff `prefix` is a prefix of `s`. Empty
/// prefix always matches.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_starts_with(sp: i32, sl: i32, pp: i32, pl: i32) -> i32 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let p = unsafe { slice_or_empty(pp, pl) };
    if s.starts_with(p) {
        1
    } else {
        0
    }
}

/// `s.ends_with(suffix)` — 1 iff `suffix` is a suffix of `s`. Empty
/// suffix always matches.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_ends_with(sp: i32, sl: i32, pp: i32, pl: i32) -> i32 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let p = unsafe { slice_or_empty(pp, pl) };
    if s.ends_with(p) {
        1
    } else {
        0
    }
}

/// `s.find(needle)` — byte position of the first occurrence of `needle`
/// in `s`, or `-1` when not found. Matches MVL's `Int` return convention
/// for `find` — no `Option[Int]` ABI needed here. Empty needle returns 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_find(sp: i32, sl: i32, np: i32, nl: i32) -> i64 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let n = unsafe { slice_or_empty(np, nl) };
    match find_bytes(s, n) {
        Some(i) => i as i64,
        None => -1,
    }
}

// ── Byte-search primitive ────────────────────────────────────────────────

/// Byte-level substring search. Returns the position of the first match
/// or `None`. Empty needle matches at 0. Used by `contains` and `find`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    for i in 0..=last {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

// ── Heap-owned strings (Group B) ─────────────────────────────────────────
//
// `MvlString` mirrors `runtime/llvm/`'s layout — same field order, same
// semantic roles — but every field is `i32` for wasm32 addressing. The
// emitter treats a `*MvlString` as an opaque `i32` on the WASM stack and
// unpacks the two fields it cares about (`ptr`, `len`) via `i32.load`
// at offsets 0 and 4.
//
// Refcount (`rc`) and capacity (`cap`) are present so the layout is
// stable across commits; they'll be exercised when `_mvl_string_clone`
// and `_mvl_string_drop` land in the follow-up commit.

#[repr(C)]
pub struct MvlString {
    pub ptr: i32,
    pub len: i32,
    pub cap: i32,
    pub rc: i32,
}

/// Allocate a fresh `MvlString` whose backing bytes are the concatenation
/// of `(p1, l1)` and `(p2, l2)`. Returns a pointer to the struct (as
/// `i32` — WASM linear-memory offset). The emitter reads `.ptr` / `.len`
/// via `i32.load` to feed the concatenated bytes into everything
/// downstream that still uses the `(ptr, len)` representation
/// (`_mvl_string_eq`, `_mvl_string_len`, `println`, etc.).
///
/// Group B commit 1 deliberately leaks: neither the `Box<MvlString>` nor
/// the byte `Vec` is freed. Drop emission + `_mvl_string_drop` come next.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_concat(p1: i32, l1: i32, p2: i32, l2: i32) -> i32 {
    let a = unsafe { slice_or_empty(p1, l1) };
    let b = unsafe { slice_or_empty(p2, l2) };
    let mut bytes = Vec::with_capacity(a.len() + b.len());
    bytes.extend_from_slice(a);
    bytes.extend_from_slice(b);
    let bytes_ptr = bytes.as_ptr() as i32;
    let bytes_len = bytes.len() as i32;
    let bytes_cap = bytes.capacity() as i32;
    core::mem::forget(bytes); // leaked — freed by `_mvl_string_drop` in a
                              // follow-up commit.
    let ms = Box::new(MvlString {
        ptr: bytes_ptr,
        len: bytes_len,
        cap: bytes_cap,
        rc: 1,
    });
    Box::into_raw(ms) as i32
}

// ── Equality ─────────────────────────────────────────────────────────────

/// Bytewise equality of two strings. Returns 1 when equal, 0 otherwise.
/// Wired by the emitter for `assert_eq[String]` / `assert_ne[String]`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_eq(ptr1: i32, len1: i32, ptr2: i32, len2: i32) -> i32 {
    if len1 != len2 {
        return 0;
    }
    let a = unsafe { slice_or_empty(ptr1, len1) };
    let b = unsafe { slice_or_empty(ptr2, len2) };
    if a == b {
        1
    } else {
        0
    }
}

// ── Tests ────────────────────────────────────────────────────────────────
//
// Compiled + run under wasm32-wasip1 so the i32-pointer ABI works as it
// does in production. `.cargo/config.toml` sets `runner = wasmtime run`
// for this target.

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    fn addr(s: &'static [u8]) -> i32 {
        s.as_ptr() as usize as i32
    }

    // ── eq ────
    #[test]
    fn eq_equal_strings() {
        let a = b"hello";
        let b = b"hello";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            1
        );
    }

    #[test]
    fn eq_different_content() {
        let a = b"hello";
        let b = b"world";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            0
        );
    }

    #[test]
    fn eq_different_lengths() {
        let a = b"hello";
        let b = b"hell";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            0
        );
    }

    #[test]
    fn eq_both_empty() {
        assert_eq!(unsafe { _mvl_string_eq(0, 0, 0, 0) }, 1);
    }

    #[test]
    fn eq_one_empty() {
        let a = b"x";
        assert_eq!(unsafe { _mvl_string_eq(addr(a), 1, 0, 0) }, 0);
    }

    // ── len ────
    #[test]
    fn len_regular() {
        let a = b"hello";
        assert_eq!(_mvl_string_len(addr(a), a.len() as i32), 5);
    }

    #[test]
    fn len_empty() {
        assert_eq!(_mvl_string_len(0, 0), 0);
    }

    // ── is_empty ────
    #[test]
    fn is_empty_true() {
        assert_eq!(_mvl_string_is_empty(0, 0), 1);
    }

    #[test]
    fn is_empty_false() {
        let a = b"x";
        assert_eq!(_mvl_string_is_empty(addr(a), 1), 0);
    }

    // ── contains ────
    #[test]
    fn contains_middle() {
        let s = b"hello world";
        let n = b"lo wo";
        assert_eq!(
            unsafe { _mvl_string_contains(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            1
        );
    }

    #[test]
    fn contains_empty_needle() {
        let s = b"hello";
        assert_eq!(
            unsafe { _mvl_string_contains(addr(s), s.len() as i32, 0, 0) },
            1
        );
    }

    #[test]
    fn contains_missing() {
        let s = b"hello";
        let n = b"xyz";
        assert_eq!(
            unsafe { _mvl_string_contains(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            0
        );
    }

    #[test]
    fn contains_needle_larger_than_haystack() {
        let s = b"hi";
        let n = b"hello";
        assert_eq!(
            unsafe { _mvl_string_contains(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            0
        );
    }

    // ── starts_with ────
    #[test]
    fn starts_with_true() {
        let s = b"hello";
        let p = b"hel";
        assert_eq!(
            unsafe { _mvl_string_starts_with(addr(s), s.len() as i32, addr(p), p.len() as i32) },
            1
        );
    }

    #[test]
    fn starts_with_full_match() {
        let s = b"hello";
        assert_eq!(
            unsafe { _mvl_string_starts_with(addr(s), s.len() as i32, addr(s), s.len() as i32) },
            1
        );
    }

    #[test]
    fn starts_with_false() {
        let s = b"hello";
        let p = b"world";
        assert_eq!(
            unsafe { _mvl_string_starts_with(addr(s), s.len() as i32, addr(p), p.len() as i32) },
            0
        );
    }

    #[test]
    fn starts_with_empty_prefix() {
        let s = b"hello";
        assert_eq!(
            unsafe { _mvl_string_starts_with(addr(s), s.len() as i32, 0, 0) },
            1
        );
    }

    // ── ends_with ────
    #[test]
    fn ends_with_true() {
        let s = b"hello";
        let p = b"llo";
        assert_eq!(
            unsafe { _mvl_string_ends_with(addr(s), s.len() as i32, addr(p), p.len() as i32) },
            1
        );
    }

    #[test]
    fn ends_with_false() {
        let s = b"hello";
        let p = b"hel";
        assert_eq!(
            unsafe { _mvl_string_ends_with(addr(s), s.len() as i32, addr(p), p.len() as i32) },
            0
        );
    }

    #[test]
    fn ends_with_empty_suffix() {
        let s = b"hello";
        assert_eq!(
            unsafe { _mvl_string_ends_with(addr(s), s.len() as i32, 0, 0) },
            1
        );
    }

    // ── concat ────
    //
    // `concat` returns a `*MvlString` — read `.ptr` / `.len` fields back
    // via unsafe deref to reconstruct the resulting `&[u8]`. Mirrors what
    // the emitter does via `i32.load` at offsets 0 / 4 of the returned
    // pointer.
    unsafe fn concat_result(ms_ptr: i32) -> &'static [u8] {
        let ms = unsafe { &*(ms_ptr as usize as *const MvlString) };
        unsafe { core::slice::from_raw_parts(ms.ptr as usize as *const u8, ms.len as usize) }
    }

    #[test]
    fn concat_two_strings() {
        let a = b"hello";
        let b = b" world";
        let ptr = unsafe { _mvl_string_concat(addr(a), a.len() as i32, addr(b), b.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello world");
    }

    #[test]
    fn concat_with_empty_left() {
        let b = b"world";
        let ptr = unsafe { _mvl_string_concat(0, 0, addr(b), b.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"world");
    }

    #[test]
    fn concat_with_empty_right() {
        let a = b"hello";
        let ptr = unsafe { _mvl_string_concat(addr(a), a.len() as i32, 0, 0) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
    }

    #[test]
    fn concat_both_empty() {
        let ptr = unsafe { _mvl_string_concat(0, 0, 0, 0) };
        assert_eq!(unsafe { concat_result(ptr) }, b"");
    }

    #[test]
    fn concat_result_has_rc_1() {
        let a = b"x";
        let ptr = unsafe { _mvl_string_concat(addr(a), 1, addr(a), 1) };
        let ms = unsafe { &*(ptr as usize as *const MvlString) };
        assert_eq!(ms.rc, 1);
        assert_eq!(ms.len, 2);
    }

    // ── find ────
    #[test]
    fn find_at_start() {
        let s = b"hello";
        let n = b"hel";
        assert_eq!(
            unsafe { _mvl_string_find(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            0
        );
    }

    #[test]
    fn find_in_middle() {
        let s = b"hello world";
        let n = b"world";
        assert_eq!(
            unsafe { _mvl_string_find(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            6
        );
    }

    #[test]
    fn find_missing_returns_neg_one() {
        let s = b"hello";
        let n = b"xyz";
        assert_eq!(
            unsafe { _mvl_string_find(addr(s), s.len() as i32, addr(n), n.len() as i32) },
            -1
        );
    }

    #[test]
    fn find_empty_needle_returns_zero() {
        let s = b"hello";
        assert_eq!(
            unsafe { _mvl_string_find(addr(s), s.len() as i32, 0, 0) },
            0
        );
    }
}
