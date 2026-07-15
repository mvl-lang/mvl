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
//! Group B — allocation, returns `*MvlString` whose fields the emitter
//! unpacks back into the `(ptr, len)` representation everything else uses:
//! - `MvlString` struct — `{ ptr, len, cap, rc }` all `i32`, matches the
//!   `runtime/llvm/` layout (i64→i32 fields for wasm32 addressing).
//! - `_mvl_string_new` — allocate from `(bytes, len)`
//! - `_mvl_string_clone` — refcount bump, returns the same pointer
//! - `_mvl_string_drop` — refcount decrement, free when zero
//! - `_mvl_string_concat` — new `MvlString` from two `(ptr, len)` inputs
//! - `_mvl_string_substring` — byte-slice window into a new `MvlString`
//! - `_mvl_string_to_upper` / `_mvl_string_to_lower` — ASCII case fold
//! - `_mvl_string_trim` — strip leading / trailing ASCII whitespace
//! - `_mvl_string_replace` — non-overlapping byte-level replace-all
//!
//! Drop emission on the emitter side is best-effort — at every function's
//! implicit-return point, the emitter drops each `__ms_*` temp local it
//! allocated. Explicit `return` statements are not yet drop-aware; those
//! paths leak (fine for phase-2 corpus tests which all end via
//! implicit return).
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
// Refcount (`rc`) supports shared ownership between clones; `cap` is
// round-tripped through `Vec::from_raw_parts` on drop so the whole
// allocation is reclaimed.

#[repr(C)]
pub struct MvlString {
    pub ptr: i32,
    pub len: i32,
    pub cap: i32,
    pub rc: i32,
}

/// Internal: allocate an owned buffer that copies `src`, wrap it in an
/// `MvlString` with `rc = 1`, return the struct's linear-memory address
/// as `i32`. Shared entrypoint for every heap-owned string this runtime
/// creates (`_mvl_string_new`, `_mvl_string_substring`, …).
///
/// The bytes `Vec` is `mem::forget`ed here and reclaimed by
/// `_mvl_string_drop` using `Vec::from_raw_parts` with the recorded
/// `cap`. `_mvl_string_concat` inlines this pattern rather than calling
/// through here because it fills the buffer with two separate copies.
fn alloc_mvl_string(src: &[u8]) -> i32 {
    let mut bytes = Vec::with_capacity(src.len());
    bytes.extend_from_slice(src);
    let bytes_ptr = bytes.as_ptr() as i32;
    let bytes_len = bytes.len() as i32;
    let bytes_cap = bytes.capacity() as i32;
    core::mem::forget(bytes);
    let ms = Box::new(MvlString {
        ptr: bytes_ptr,
        len: bytes_len,
        cap: bytes_cap,
        rc: 1,
    });
    Box::into_raw(ms) as i32
}

/// Allocate a fresh `MvlString` from a `(ptr, len)` byte range. The
/// bytes are copied — the resulting `MvlString` owns its buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_new(ptr: i32, len: i32) -> i32 {
    let src = unsafe { slice_or_empty(ptr, len) };
    alloc_mvl_string(src)
}

/// Increment the refcount and return the same pointer. Passing an
/// `MvlString` around by clone gives every holder a valid reference; the
/// last drop frees. Null-safe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_clone(ms_ptr: i32) -> i32 {
    if ms_ptr == 0 {
        return 0;
    }
    let ms = unsafe { &mut *(ms_ptr as usize as *mut MvlString) };
    ms.rc += 1;
    ms_ptr
}

/// Decrement the refcount; when it hits zero, free both the byte buffer
/// and the `MvlString` struct. Null-safe.
///
/// `cap` (recorded at allocation) is essential here — reclaiming the byte
/// `Vec` requires the exact capacity from `Vec::with_capacity`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_drop(ms_ptr: i32) {
    if ms_ptr == 0 {
        return;
    }
    let ms = unsafe { &mut *(ms_ptr as usize as *mut MvlString) };
    ms.rc -= 1;
    if ms.rc > 0 {
        return;
    }
    if ms.cap > 0 && ms.ptr != 0 {
        unsafe {
            let _ =
                Vec::from_raw_parts(ms.ptr as usize as *mut u8, ms.len as usize, ms.cap as usize);
        }
    }
    unsafe {
        let _ = Box::from_raw(ms_ptr as usize as *mut MvlString);
    }
}

/// Allocate a fresh `MvlString` whose backing bytes are the concatenation
/// of `(p1, l1)` and `(p2, l2)`. Fills the buffer with both inputs in
/// one pass rather than routing through `alloc_mvl_string`.
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
    core::mem::forget(bytes);
    let ms = Box::new(MvlString {
        ptr: bytes_ptr,
        len: bytes_len,
        cap: bytes_cap,
        rc: 1,
    });
    Box::into_raw(ms) as i32
}

/// Byte-slice substring. `start` / `end` are MVL `Int`s (i64) — clamped
/// to `0..=len` and normalised so `end >= start`. Bytes are copied into a
/// fresh `MvlString` (owns its buffer, `rc = 1`).
///
/// **Byte-based, not codepoint-based.** `runtime/llvm/`'s equivalent
/// uses `char_indices` for Unicode; we skip that here — corpus tests use
/// ASCII, and codepoint indexing without a Unicode table adds ~50 KB to
/// the runtime WASM. Revisit if a non-ASCII substring test appears.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_substring(ptr: i32, len: i32, start: i64, end: i64) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let n = s.len() as i64;
    let lo = start.max(0).min(n) as usize;
    let hi = end.max(0).min(n) as usize;
    let hi = hi.max(lo);
    alloc_mvl_string(&s[lo..hi])
}

/// `s.to_upper()` — ASCII-only case conversion. Non-ASCII bytes pass
/// through unchanged. Full Unicode `to_uppercase` would drag in Rust
/// std's case-folding table (~50 KB in the WASM). Byte-level suffices
/// for the current corpus; upgrade path is a `#[cfg(feature =
/// "unicode")]` flag when a real test needs it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_to_upper(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let mut out = Vec::with_capacity(s.len());
    for &b in s {
        out.push(b.to_ascii_uppercase());
    }
    alloc_mvl_string(&out)
}

/// `s.to_lower()` — ASCII-only case conversion, same rationale as
/// `to_upper` above.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_to_lower(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let mut out = Vec::with_capacity(s.len());
    for &b in s {
        out.push(b.to_ascii_lowercase());
    }
    alloc_mvl_string(&out)
}

/// `s.replace(from, to)` — replace every non-overlapping occurrence of
/// `from` in `s` with `to`. Byte-level match; `from == ""` returns `s`
/// unchanged (Rust's `str::replace` on empty needle inserts `to`
/// between every char, which is rarely what MVL callers want and
/// diverges from `runtime/llvm/`'s `str::replace` in practice — matched
/// for MVL, see comment in `find_bytes`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_replace(
    sp: i32,
    sl: i32,
    fp: i32,
    fl: i32,
    tp: i32,
    tl: i32,
) -> i32 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let from = unsafe { slice_or_empty(fp, fl) };
    let to = unsafe { slice_or_empty(tp, tl) };
    if from.is_empty() {
        return alloc_mvl_string(s);
    }
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i + from.len() <= s.len() {
        if &s[i..i + from.len()] == from {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&s[i..]);
    alloc_mvl_string(&out)
}

/// `s.trim()` — strip leading and trailing ASCII whitespace (space,
/// `\t`, `\n`, `\r`, `\x0c`). Matches Rust's `u8::is_ascii_whitespace`
/// (WhatWG Infra Standard). Note that vertical tab `\x0b` is *not*
/// whitespace under that definition. Unicode whitespace (U+00A0,
/// U+2028, etc.) would need a `char_indices` traversal — punted
/// alongside the other case-fold ops above.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_trim(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let start = s
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    let trimmed = if start >= end {
        &[][..]
    } else {
        &s[start..end]
    };
    alloc_mvl_string(trimmed)
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

// ── MvlArray (Group C, #1820) ────────────────────────────────────────────
//
// Backing storage for `List[T]`, `Array[T, N]`, and (once dedup'd) `Set[T]`.
// Mirrors `runtime/llvm/src/memory.rs::MvlArray` with i32 fields for wasm32
// addressing:
//
//   offset  0: i32  ptr        — heap-allocated element buffer, or 0
//   offset  4: i32  len        — number of live elements
//   offset  8: i32  cap        — capacity in elements
//   offset 12: i32  elem_size  — bytes per element (matches Vec<u8> stride)
//   offset 16: i32  rc         — refcount
//
// The emitter treats `*MvlArray` as an opaque i32. `_mvl_array_new` returns
// a fresh one with `rc = 1`; `_mvl_array_get` returns a pointer into the
// backing buffer (element accessed with `i64.load` for Int, etc.).
//
// Backing buffer allocation goes through `Vec<u8>` — same trick as
// `MvlString`: `Vec::with_capacity` allocates, `Vec::from_raw_parts` on
// drop reclaims. The `elem_size * cap` product is the buffer size.

#[repr(C)]
pub struct MvlArray {
    pub ptr: i32,
    pub len: i32,
    pub cap: i32,
    pub elem_size: i32,
    pub rc: i32,
}

const ARRAY_INITIAL_CAP: i32 = 4;

/// Allocate a raw byte buffer of `nbytes` and forget it — caller owns the
/// returned pointer. Used by `MvlArray` for its element storage; freed via
/// `reclaim_byte_buffer` on drop.
fn alloc_byte_buffer(nbytes: usize) -> (i32, i32) {
    if nbytes == 0 {
        return (0, 0);
    }
    let mut bytes = Vec::<u8>::with_capacity(nbytes);
    // Zero-init to give the emitter predictable slot contents before push.
    bytes.resize(nbytes, 0);
    let ptr = bytes.as_ptr() as i32;
    let cap = bytes.capacity() as i32;
    core::mem::forget(bytes);
    (ptr, cap)
}

/// Reclaim a buffer allocated via `alloc_byte_buffer`. `cap_bytes` must be
/// the exact `Vec` capacity in bytes recorded at allocation time.
///
/// # Safety
/// `ptr` must be a valid allocation from `alloc_byte_buffer` with the
/// recorded `cap_bytes`.
unsafe fn reclaim_byte_buffer(ptr: i32, len_bytes: usize, cap_bytes: usize) {
    if ptr == 0 || cap_bytes == 0 {
        return;
    }
    unsafe {
        let _ = Vec::from_raw_parts(ptr as usize as *mut u8, len_bytes, cap_bytes);
    }
}

/// Create a new `MvlArray` with the given element size and initial capacity.
/// Returns a heap pointer with `rc = 1`. `initial_cap` is clamped up to
/// `ARRAY_INITIAL_CAP` (4).
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_array_new(elem_size: i32, initial_cap: i32) -> i32 {
    let cap = initial_cap.max(ARRAY_INITIAL_CAP).max(0);
    let elem_size = elem_size.max(1);
    let nbytes = (cap as usize).saturating_mul(elem_size as usize);
    let (ptr, _actual_cap) = alloc_byte_buffer(nbytes);
    let a = Box::new(MvlArray {
        ptr,
        len: 0,
        cap,
        elem_size,
        rc: 1,
    });
    Box::into_raw(a) as i32
}

/// `_mvl_array_len(a) -> i64` — number of live elements. i64 matches MVL
/// `Int`, so the emitter can pass the result straight to `assert_eq[Int]`.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_len(a: i32) -> i64 {
    if a == 0 {
        return 0;
    }
    let arr = unsafe { &*(a as usize as *const MvlArray) };
    arr.len as i64
}

/// `_mvl_array_is_empty(a) -> i32` — 1 when `len == 0`, else 0.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_is_empty(a: i32) -> i32 {
    if a == 0 {
        return 1;
    }
    let arr = unsafe { &*(a as usize as *const MvlArray) };
    if arr.len == 0 { 1 } else { 0 }
}

/// `_mvl_array_push(a, elem_ptr)` — copy `elem_size` bytes from `elem_ptr`
/// into the next slot. Grows the buffer by doubling when full.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer; `elem_ptr` must point to at
/// least `elem_size` bytes of readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_push(a: i32, elem_ptr: i32) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    if arr.len >= arr.cap {
        let new_cap = (arr.cap.max(1) * 2).max(ARRAY_INITIAL_CAP);
        let elem_size = arr.elem_size as usize;
        let new_nbytes = (new_cap as usize).saturating_mul(elem_size);
        let (new_ptr, _) = alloc_byte_buffer(new_nbytes);
        if arr.len > 0 && arr.ptr != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    arr.ptr as *const u8,
                    new_ptr as *mut u8,
                    (arr.len as usize) * elem_size,
                );
            }
        }
        let old_nbytes = (arr.cap as usize).saturating_mul(elem_size);
        unsafe { reclaim_byte_buffer(arr.ptr, old_nbytes, old_nbytes) };
        arr.ptr = new_ptr;
        arr.cap = new_cap;
    }
    let slot = (arr.ptr as usize) + (arr.len as usize) * (arr.elem_size as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(
            elem_ptr as *const u8,
            slot as *mut u8,
            arr.elem_size as usize,
        );
    }
    arr.len += 1;
}

/// `_mvl_array_get(a, idx) -> i32` — pointer to the `idx`-th element in
/// the backing buffer, or 0 when out of bounds. Caller reads through the
/// pointer with the appropriate `i32.load` / `i64.load` per element type.
///
/// `idx` is i64 to match MVL's `Int` type on the WASM stack.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_get(a: i32, idx: i64) -> i32 {
    if a == 0 {
        return 0;
    }
    let arr = unsafe { &*(a as usize as *const MvlArray) };
    if idx < 0 || idx >= arr.len as i64 {
        return 0;
    }
    (arr.ptr as usize + (idx as usize) * (arr.elem_size as usize)) as i32
}

/// `_mvl_array_clone(a)` — refcount bump, returns the same pointer.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_clone(a: i32) -> i32 {
    if a == 0 {
        return 0;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    arr.rc += 1;
    a
}

/// `_mvl_array_drop(a)` — refcount decrement; free the backing buffer and
/// the `MvlArray` header when refcount hits zero.
///
/// Element-level drops (e.g., strings inside a `List[String]`) are *not*
/// emitted here — the LLVM backend does per-element drops in the emitter
/// for now. Follow-up if it becomes a real leak.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer, not used after drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_drop(a: i32) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    arr.rc -= 1;
    if arr.rc > 0 {
        return;
    }
    let nbytes = (arr.cap as usize) * (arr.elem_size as usize);
    unsafe { reclaim_byte_buffer(arr.ptr, nbytes, nbytes) };
    unsafe {
        let _ = Box::from_raw(a as usize as *mut MvlArray);
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

    // ── new / clone / drop ────
    #[test]
    fn new_copies_bytes() {
        let src = b"world";
        let ptr = unsafe { _mvl_string_new(addr(src), src.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"world");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn new_empty() {
        let ptr = unsafe { _mvl_string_new(0, 0) };
        let ms = unsafe { &*(ptr as usize as *const MvlString) };
        assert_eq!(ms.len, 0);
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn clone_bumps_refcount() {
        let src = b"x";
        let ptr = unsafe { _mvl_string_new(addr(src), 1) };
        let ptr2 = unsafe { _mvl_string_clone(ptr) };
        assert_eq!(ptr, ptr2, "clone returns the same pointer");
        let ms = unsafe { &*(ptr as usize as *const MvlString) };
        assert_eq!(ms.rc, 2);
        // Drop twice — first is a no-op (rc→1), second frees.
        unsafe { _mvl_string_drop(ptr) };
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn clone_null_is_null() {
        assert_eq!(unsafe { _mvl_string_clone(0) }, 0);
    }

    #[test]
    fn drop_null_is_noop() {
        unsafe { _mvl_string_drop(0) }; // must not crash
    }

    #[test]
    fn drop_frees_shared_alloc() {
        // Alloc a MvlString, clone twice → rc=3, drop three times, last
        // one frees. A leak-detector on the host would catch a missed
        // free here; the best we can do under wasmtime is exercise the
        // path and rely on `Vec::from_raw_parts` to complain if the
        // capacity is wrong.
        let src = b"probe";
        let ptr = unsafe { _mvl_string_new(addr(src), 5) };
        unsafe { _mvl_string_clone(ptr) };
        unsafe { _mvl_string_clone(ptr) };
        unsafe { _mvl_string_drop(ptr) };
        unsafe { _mvl_string_drop(ptr) };
        unsafe { _mvl_string_drop(ptr) }; // final: frees
    }

    // ── substring ────
    #[test]
    fn substring_middle() {
        let s = b"hello world";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, 6, 11) };
        assert_eq!(unsafe { concat_result(ptr) }, b"world");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn substring_start_zero() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, 0, 3) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hel");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn substring_empty_range() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, 2, 2) };
        assert_eq!(unsafe { concat_result(ptr) }, b"");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn substring_clamps_end() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, 3, 999) };
        assert_eq!(unsafe { concat_result(ptr) }, b"lo");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn substring_clamps_negative_start() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, -1, 3) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hel");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn substring_reversed_range_clamps_to_empty() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_substring(addr(s), s.len() as i32, 4, 1) };
        assert_eq!(unsafe { concat_result(ptr) }, b"");
        unsafe { _mvl_string_drop(ptr) };
    }

    // ── to_upper / to_lower ────
    #[test]
    fn to_upper_ascii() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_to_upper(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"HELLO");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn to_upper_mixed_case() {
        let s = b"Mixed Case";
        let ptr = unsafe { _mvl_string_to_upper(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"MIXED CASE");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn to_upper_already_upper() {
        let s = b"HELLO";
        let ptr = unsafe { _mvl_string_to_upper(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"HELLO");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn to_upper_non_ascii_passthrough() {
        // `é` in UTF-8 is 0xc3 0xa9 — both above 0x7f, so `to_ascii_uppercase`
        // leaves them unchanged. Sanity check the byte-level guarantee.
        let s = b"caf\xc3\xa9";
        let ptr = unsafe { _mvl_string_to_upper(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"CAF\xc3\xa9");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn to_lower_ascii() {
        let s = b"HELLO";
        let ptr = unsafe { _mvl_string_to_lower(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn to_lower_mixed_case() {
        let s = b"Mixed Case";
        let ptr = unsafe { _mvl_string_to_lower(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"mixed case");
        unsafe { _mvl_string_drop(ptr) };
    }

    // ── trim ────
    #[test]
    fn trim_both_sides() {
        let s = b"  hello  ";
        let ptr = unsafe { _mvl_string_trim(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn trim_no_whitespace() {
        let s = b"hello";
        let ptr = unsafe { _mvl_string_trim(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn trim_all_whitespace() {
        let s = b"   \t\n ";
        let ptr = unsafe { _mvl_string_trim(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn trim_empty() {
        let ptr = unsafe { _mvl_string_trim(0, 0) };
        assert_eq!(unsafe { concat_result(ptr) }, b"");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn trim_mixed_whitespace_chars() {
        // \t, \n, \r, space, form feed — all ASCII whitespace under Rust's
        // WhatWG-Infra definition. Vertical tab (\x0b) is deliberately
        // *not* included; adding it here would fail.
        let s = b"\t\n\r hello\x0c ";
        let ptr = unsafe { _mvl_string_trim(addr(s), s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    // ── replace ────
    #[test]
    fn replace_single_occurrence() {
        let s = b"hello world";
        let f = b"world";
        let t = b"there";
        let ptr = unsafe {
            _mvl_string_replace(
                addr(s),
                s.len() as i32,
                addr(f),
                f.len() as i32,
                addr(t),
                t.len() as i32,
            )
        };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello there");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn replace_multiple_occurrences() {
        let s = b"aXbXc";
        let f = b"X";
        let t = b"YY";
        let ptr = unsafe {
            _mvl_string_replace(
                addr(s),
                s.len() as i32,
                addr(f),
                f.len() as i32,
                addr(t),
                t.len() as i32,
            )
        };
        assert_eq!(unsafe { concat_result(ptr) }, b"aYYbYYc");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn replace_no_match() {
        let s = b"hello";
        let f = b"xyz";
        let t = b"???";
        let ptr = unsafe {
            _mvl_string_replace(
                addr(s),
                s.len() as i32,
                addr(f),
                f.len() as i32,
                addr(t),
                t.len() as i32,
            )
        };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn replace_with_empty() {
        // Removing substring by replacing with "".
        let s = b"hello world";
        let f = b" world";
        let ptr =
            unsafe { _mvl_string_replace(addr(s), s.len() as i32, addr(f), f.len() as i32, 0, 0) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
    }

    #[test]
    fn replace_empty_needle_returns_unchanged() {
        let s = b"hello";
        let t = b"XYZ";
        let ptr =
            unsafe { _mvl_string_replace(addr(s), s.len() as i32, 0, 0, addr(t), t.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, b"hello");
        unsafe { _mvl_string_drop(ptr) };
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

    // ── MvlArray ────

    /// Push i64 values (8-byte elements) into an array, read them back via
    /// `_mvl_array_get`. This exercises the raw byte-copy path that the
    /// emitter will drive.
    unsafe fn push_i64(arr: i32, v: i64) {
        let slot = v;
        unsafe { _mvl_array_push(arr, &slot as *const i64 as i32) };
    }

    unsafe fn get_i64(arr: i32, idx: i64) -> i64 {
        let p = unsafe { _mvl_array_get(arr, idx) };
        unsafe { *(p as usize as *const i64) }
    }

    #[test]
    fn array_new_empty() {
        let a = _mvl_array_new(8, 4);
        assert_eq!(unsafe { _mvl_array_len(a) }, 0);
        assert_eq!(unsafe { _mvl_array_is_empty(a) }, 1);
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_push_and_len() {
        let a = _mvl_array_new(8, 4);
        unsafe {
            push_i64(a, 10);
            push_i64(a, 20);
            push_i64(a, 30);
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 3);
        assert_eq!(unsafe { _mvl_array_is_empty(a) }, 0);
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_get_by_index() {
        let a = _mvl_array_new(8, 4);
        unsafe {
            push_i64(a, 100);
            push_i64(a, 200);
            push_i64(a, 300);
        }
        assert_eq!(unsafe { get_i64(a, 0) }, 100);
        assert_eq!(unsafe { get_i64(a, 1) }, 200);
        assert_eq!(unsafe { get_i64(a, 2) }, 300);
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_get_out_of_bounds_returns_null() {
        let a = _mvl_array_new(8, 4);
        unsafe { push_i64(a, 1) };
        assert_eq!(unsafe { _mvl_array_get(a, 5) }, 0);
        assert_eq!(unsafe { _mvl_array_get(a, -1) }, 0);
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_push_grows_past_initial_cap() {
        // Initial cap is 4; push 10 to force at least one growth.
        let a = _mvl_array_new(8, 4);
        unsafe {
            for i in 0..10 {
                push_i64(a, i);
            }
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 10);
        for i in 0..10i64 {
            assert_eq!(unsafe { get_i64(a, i) }, i);
        }
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_clone_bumps_rc() {
        let a = _mvl_array_new(8, 4);
        unsafe { push_i64(a, 42) };
        let b = unsafe { _mvl_array_clone(a) };
        assert_eq!(a, b, "clone returns same pointer");
        // Drop twice: rc goes 2→1→0.
        unsafe { _mvl_array_drop(a) };
        assert_eq!(unsafe { _mvl_array_len(b) }, 1, "still live after one drop");
        unsafe { _mvl_array_drop(b) };
    }

    #[test]
    fn array_i32_elements() {
        // Bool / Byte lower to i32 in the WASM stack — verify the
        // 4-byte element_size path works.
        let a = _mvl_array_new(4, 4);
        for i in 0..5i32 {
            unsafe {
                _mvl_array_push(a, &i as *const i32 as i32);
            }
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 5);
        for i in 0..5i64 {
            let p = unsafe { _mvl_array_get(a, i) };
            let v: i32 = unsafe { *(p as usize as *const i32) };
            assert_eq!(v, i as i32);
        }
        unsafe { _mvl_array_drop(a) };
    }
}
