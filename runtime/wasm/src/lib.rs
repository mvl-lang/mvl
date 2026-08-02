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
//! - `_mvl_string_split` — split on a separator into a `List[String]`
//!   (`*MvlArray` of `*MvlString`, so Group C's `elem_size == 4`)
//!
//! Drop emission on the emitter side is best-effort — at every function's
//! implicit-return point, the emitter drops each `__ms_*` temp local it
//! allocated. Explicit `return` statements are drop-aware too: `emit_stmt`'s
//! `Return` arm sweeps the same heap locals, excluding the one being returned
//! so it survives for the caller. Function *parameters* are deliberately
//! excluded from that sweep — they are borrowed, not owned, and dropping one
//! freed the caller's array (#2014).
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
    if ptr == 0 || len <= 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) }
}

// ── Query ops ────────────────────────────────────────────────────────────

/// `s.len()` — number of Unicode scalar values (chars), not bytes.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_len`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_len(ptr: i32, len: i32) -> i64 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    text.chars().count() as i64
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

/// `s.find(needle)` — character index of the first occurrence of `needle`
/// in `s`, or `-1` when not found. Returns character index, not byte index,
/// matching `runtime/rust/src/stdlib/primitives.rs::str_find`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_find(sp: i32, sl: i32, np: i32, nl: i32) -> i64 {
    let s = unsafe { slice_or_empty(sp, sl) };
    let n = unsafe { slice_or_empty(np, nl) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let needle = core::str::from_utf8(n).unwrap_or("");
    match text.find(needle) {
        Some(byte_idx) => {
            // Convert byte index to char index
            text[..byte_idx].chars().count() as i64
        }
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

/// `Float.to_string()` (#2039) — shortest round-trip decimal via `f64`'s
/// `Display` impl (`25.0` → `"25"`, `25.5` → `"25.5"`), matching
/// `runtime/llvm/src/stdlib/random.rs::_mvl_float_to_string`. Returns a
/// `*MvlString`; the emitter unpacks `.ptr`/`.len` immediately after the
/// call, same as `_mvl_string_new`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_float_to_string(v: f64) -> i32 {
    alloc_mvl_string(format!("{v}").as_bytes())
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
/// of `(p1, l1)` and `(p2, l2)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_concat(p1: i32, l1: i32, p2: i32, l2: i32) -> i32 {
    let a = unsafe { slice_or_empty(p1, l1) };
    let b = unsafe { slice_or_empty(p2, l2) };
    let bytes = mvl_runtime_core::concat_bytes(a, b);
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

/// Unicode-aware substring. `start` / `end` are character indices (not byte
/// indices), clamped to `0..=char_count`. Matches
/// `runtime/rust/src/stdlib/primitives.rs::str_substring`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_substring(ptr: i32, len: i32, start: i64, end: i64) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as i64;
    let lo = start.max(0).min(n) as usize;
    let hi = end.max(0).min(n) as usize;
    let hi = hi.max(lo);
    let result: String = chars[lo..hi].iter().collect();
    alloc_mvl_string(result.as_bytes())
}

/// `s.to_upper()` — full Unicode case conversion.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_to_upper`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_to_upper(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let upper = text.to_uppercase();
    alloc_mvl_string(upper.as_bytes())
}

/// `s.to_lower()` — full Unicode case conversion.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_to_lower`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_to_lower(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let lower = text.to_lowercase();
    alloc_mvl_string(lower.as_bytes())
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

/// `s.split(sep)` — split on every occurrence of `sep`, returning a
/// `List[String]`: an `MvlArray` with `elem_size == 4` holding one
/// `*MvlString` per part. Drop it with `_mvl_string_ptr_array_drop`, never
/// `_mvl_array_drop` — the latter leaks every element string.
///
/// Deliberately routed through `str::split` on a UTF-8 view rather than the
/// byte-level scan `_mvl_string_replace` above uses. `runtime/llvm/`'s
/// `_mvl_str_split` is `as_str(s).split(as_str(sep))`, and `as_str` is
/// `from_utf8(..).unwrap_or("")` — identical here, so both backends agree
/// bit-for-bit on the cases a byte scan would have to special-case by hand:
/// an empty separator (Rust yields a leading and trailing `""` plus one part
/// per char), an empty subject (one `""` part), and a separator longer than
/// the subject (one part, the whole subject). Corpus parity between
/// `test-rust-wasm` and `test-rust-rust` depends on that agreement, so
/// resist "simplifying" this into a hand-rolled loop.
///
/// # Safety
/// Both `(ptr, len)` pairs must describe valid ranges or be `(0, 0)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_split(sp: i32, sl: i32, sepp: i32, sepl: i32) -> i32 {
    let text = core::str::from_utf8(unsafe { slice_or_empty(sp, sl) }).unwrap_or("");
    let sep = core::str::from_utf8(unsafe { slice_or_empty(sepp, sepl) }).unwrap_or("");
    // elem_size 4 — `*MvlString` is an i32 address on wasm32.
    let arr = _mvl_array_new(4, 0);
    for part in text.split(sep) {
        unsafe { _mvl_array_push_i32(arr, alloc_mvl_string(part.as_bytes())) };
    }
    arr
}

/// `_mvl_env_args()` — process argv as a `List[String]`.
///
/// Backs `std.env`'s `pub builtin fn args() -> List[Tainted[String]] ! Env`
/// (the `Tainted` wrapper is erased before codegen). The Rust backend uses
/// `std::env::args()` and LLVM has `_mvl_env_args`; WASM had neither a runtime
/// function nor an import, so `args()` emitted a bare `call $args` and the
/// module could not load — the same gap #2076 closed for `read_file`.
///
/// `runtime/wasm` targets `wasm32-wasip1` with `std`, so this is the same
/// `std::env::args()` the Rust backend uses; wasmtime populates it from the
/// host command line. Returns an `*MvlArray` of `*MvlString` with `elem_size`
/// 4, matching `_mvl_string_split` — so `local_drop_fn` already maps the
/// result to `_mvl_string_ptr_array_drop`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_args() -> i32 {
    let arr = _mvl_array_new(4, 0);
    for a in std::env::args() {
        unsafe { _mvl_array_push_i32(arr, alloc_mvl_string(a.as_bytes())) };
    }
    arr
}

/// `_mvl_env_get(ptr, len)` — environment variable as `Option[String]`.
///
/// Backs `std.env`'s `pub builtin fn get(name: String) -> Option[Tainted[String]] ! Env`.
/// A missing variable, or one whose value is not valid UTF-8, is `None` —
/// matching `std::env::var`, which the Rust backend uses.
///
/// # Safety
/// `(ptr, len)` must describe a valid range or be `(0, 0)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_env_get(ptr: i32, len: i32) -> i32 {
    let name = match core::str::from_utf8(unsafe { slice_or_empty(ptr, len) }) {
        Ok(s) if !s.is_empty() => s,
        _ => return _mvl_option_none(),
    };
    match std::env::var(name) {
        Ok(v) => _mvl_option_some_i32(alloc_mvl_string(v.as_bytes())),
        Err(_) => _mvl_option_none(),
    }
}

/// `s.trim()` — strip leading and trailing Unicode whitespace.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_trim`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_trim(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let trimmed = text.trim();
    alloc_mvl_string(trimmed.as_bytes())
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

/// `_mvl_box_new(size) -> ptr` — heap slot of `size` bytes for a `Box[T]`.
///
/// Backs `Box::new(x)`, which MVL needs to make a recursive enum payload
/// finite-sized (`HuffmanTree::Node(w, Box::new(l), Box::new(r))`). The
/// emitter stores the value into the returned slot itself, so this only has to
/// hand back writable, correctly-sized, zeroed memory.
///
/// Port of `runtime/llvm`'s `_mvl_box_new`, which mallocs. Here it reuses
/// `alloc_byte_buffer`, the same allocator `MvlArray` element storage uses.
///
/// Returns 0 for a non-positive size rather than aborting: the emitter only
/// ever passes a fixed 4 or 8, so a 0 here means a caller bug, and returning a
/// null the caller will trap on beats killing the module.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_box_new(size: i32) -> i32 {
    if size <= 0 {
        return 0;
    }
    let (ptr, _cap) = alloc_byte_buffer(size as usize);
    ptr
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
    if arr.len == 0 {
        1
    } else {
        0
    }
}

/// Grow `arr`'s backing buffer to fit one more element. Doubles capacity
/// on overflow, copies live prefix, reclaims the old buffer.
///
/// # Safety
/// `arr` must be a valid `MvlArray` reference.
unsafe fn ensure_capacity(arr: &mut MvlArray) {
    if arr.len < arr.cap {
        return;
    }
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

/// `_mvl_array_push(a, elem_ptr)` — copy `elem_size` bytes from `elem_ptr`
/// into the next slot. The emitter typically prefers the typed variants
/// (`_mvl_array_push_i32` / `_i64` / `_f64`) which pass the value directly
/// on the WASM stack — WASM has no `alloca`, so materialising a byte-ptr
/// argument for each push means a scratch allocation. This byte-ptr entry
/// point stays for tests + hypothetical callers with an existing address.
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
    unsafe { ensure_capacity(arr) };
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

/// `_mvl_array_push_i32(a, val)` — push a 4-byte value (Bool, Byte, enum
/// disc, or nested pointer) directly. Element size must be 4.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_push_i32(a: i32, val: i32) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    unsafe { ensure_capacity(arr) };
    let slot = (arr.ptr as usize) + (arr.len as usize) * (arr.elem_size as usize);
    unsafe { core::ptr::write(slot as *mut i32, val) };
    arr.len += 1;
}

/// `_mvl_array_push_i64(a, val)` — push an 8-byte integer value (Int / UInt).
/// Element size must be 8.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_push_i64(a: i32, val: i64) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    unsafe { ensure_capacity(arr) };
    let slot = (arr.ptr as usize) + (arr.len as usize) * (arr.elem_size as usize);
    unsafe { core::ptr::write(slot as *mut i64, val) };
    arr.len += 1;
}

/// `_mvl_array_push_f64(a, val)` — push an 8-byte float value. Element
/// size must be 8.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_push_f64(a: i32, val: f64) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    unsafe { ensure_capacity(arr) };
    let slot = (arr.ptr as usize) + (arr.len as usize) * (arr.elem_size as usize);
    unsafe { core::ptr::write(slot as *mut f64, val) };
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
/// handled here — use `_mvl_string_ptr_array_drop` for `List[String]`.
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

/// `_mvl_array_slice(a, start, end)` — new array holding elements
/// `[start, end)`, with both bounds clamped into `[0, len]` and a reversed
/// range yielding an empty array (#2014).
///
/// Port of `runtime/llvm/`'s `_mvl_list_slice`. Backs `List[T]::take`
/// (`self.slice(0, n)`) and `::skip` (`self.slice(n, self.len())`), which are
/// pure-MVL wrappers over the `slice` builtin.
///
/// Elements are copied byte-wise at `elem_size` granularity, so this is correct
/// for scalar arrays but does *not* refcount-bump `*MvlString` elements: a
/// slice of a `List[String]` aliases the parent's strings, and dropping both
/// with `_mvl_string_ptr_array_drop` would double-free. The corpus slices only
/// scalar lists; `List[String]::take` would need an element-aware copy first.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_slice(a: i32, start: i64, end: i64) -> i32 {
    if a == 0 {
        // Null in, null out. The earlier `_mvl_array_new(8, 0)` fabricated an
        // 8-byte-stride array regardless of what the caller's elements actually
        // are, so any later push/get/drop against the result would have used the
        // wrong stride. Every other null guard in this file returns 0.
        return 0;
    }
    let arr = unsafe { &*(a as usize as *const MvlArray) };
    let es = arr.elem_size;
    let len = arr.len as i64;
    let lo = start.clamp(0, len);
    let hi = end.clamp(0, len);
    let count = (hi - lo).max(0);
    let out = _mvl_array_new(es, count as i32);
    if count == 0 {
        return out;
    }
    let dst = unsafe { &mut *(out as usize as *mut MvlArray) };
    let bytes = (count as usize) * (es as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(
            (arr.ptr as usize + (lo as usize) * (es as usize)) as *const u8,
            dst.ptr as *mut u8,
            bytes,
        );
    }
    dst.len = count as i32;
    out
}

/// `_mvl_array_concat(a, b)` — new array holding `a`'s elements followed
/// by `b`'s (#2114). Port of `runtime/llvm/`'s `_mvl_list_concat`.
///
/// Elements are copied byte-wise at `elem_size` granularity, same caveat as
/// `_mvl_array_slice`: correct for scalar/pointer arrays, not refcount-aware
/// for `*MvlString` elements — the emitter's `concat_is_supported` gate
/// keeps `List[String]::concat` off this path.
///
/// `a` and `b` must share the same `elem_size`, which the type system
/// guarantees (`List[T]::concat` requires both operands to be `List[T]`).
///
/// # Safety
/// `a` and `b` must each be a valid `MvlArray` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_concat(a: i32, b: i32) -> i32 {
    let (es, la, lb) = match (a, b) {
        (0, 0) => return _mvl_array_new(8, 0),
        (0, _) => {
            let arr_b = unsafe { &*(b as usize as *const MvlArray) };
            (arr_b.elem_size, 0, arr_b.len)
        }
        (_, 0) => {
            let arr_a = unsafe { &*(a as usize as *const MvlArray) };
            (arr_a.elem_size, arr_a.len, 0)
        }
        (_, _) => {
            let arr_a = unsafe { &*(a as usize as *const MvlArray) };
            let arr_b = unsafe { &*(b as usize as *const MvlArray) };
            (arr_a.elem_size, arr_a.len, arr_b.len)
        }
    };
    let total = la + lb;
    let out = _mvl_array_new(es, total);
    if total == 0 {
        return out;
    }
    let dst = unsafe { &mut *(out as usize as *mut MvlArray) };
    if la > 0 {
        let arr_a = unsafe { &*(a as usize as *const MvlArray) };
        unsafe {
            core::ptr::copy_nonoverlapping(
                arr_a.ptr as *const u8,
                dst.ptr as *mut u8,
                (la as usize) * (es as usize),
            );
        }
    }
    if lb > 0 {
        let arr_b = unsafe { &*(b as usize as *const MvlArray) };
        let dst_off = (dst.ptr as usize) + (la as usize) * (es as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(
                arr_b.ptr as *const u8,
                dst_off as *mut u8,
                (lb as usize) * (es as usize),
            );
        }
    }
    dst.len = total;
    out
}

/// `_mvl_string_ptr_array_drop(a)` — refcount decrement for a `List[String]`
/// array. When the refcount hits zero each element `*MvlString` is dropped
/// via `_mvl_string_drop`, then the backing buffer and struct are freed.
///
/// Use instead of `_mvl_array_drop` whenever the array's elements are
/// `*MvlString` pointers (`elem_size == 4`).
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer with `elem_size == 4`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_ptr_array_drop(a: i32) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    arr.rc -= 1;
    if arr.rc > 0 {
        return;
    }
    let len = arr.len as usize;
    let es = arr.elem_size as usize;
    let base = arr.ptr as usize;
    for i in 0..len {
        let s = unsafe { core::ptr::read((base + i * es) as *const i32) };
        if s != 0 {
            unsafe { _mvl_string_drop(s) };
        }
    }
    let nbytes = (arr.cap as usize) * es;
    unsafe { reclaim_byte_buffer(arr.ptr, nbytes, nbytes) };
    unsafe {
        let _ = Box::from_raw(a as usize as *mut MvlArray);
    }
}

/// `_mvl_string_ptr_array_dedup(a)` — remove duplicate `*MvlString` elements
/// by content equality.  Duplicates are freed via `_mvl_string_drop`; the
/// array is compacted in-place and `arr.len` is updated.  O(n²) — acceptable
/// for small `Set[String]` literals.
///
/// Use instead of `_mvl_array_dedup_i32` whenever elements are `*MvlString`
/// pointers; pointer-address dedup does not detect equal strings from distinct
/// allocations.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer with `elem_size == 4`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_ptr_array_dedup(a: i32) {
    if a == 0 {
        return;
    }
    let arr = unsafe { &mut *(a as usize as *mut MvlArray) };
    let es = arr.elem_size as usize;
    let base = arr.ptr as usize;
    let mut write = 0usize;
    'outer: for read in 0..arr.len as usize {
        let s = unsafe { core::ptr::read((base + read * es) as *const i32) };
        for kept in 0..write {
            let k = unsafe { core::ptr::read((base + kept * es) as *const i32) };
            // Read raw ptr/len from MvlString (offsets 0 and 4).
            let s_ptr = unsafe { core::ptr::read(s as usize as *const i32) };
            let s_len = unsafe { core::ptr::read((s as usize + 4) as *const i32) };
            let k_ptr = unsafe { core::ptr::read(k as usize as *const i32) };
            let k_len = unsafe { core::ptr::read((k as usize + 4) as *const i32) };
            if unsafe { _mvl_string_eq(s_ptr, s_len, k_ptr, k_len) } != 0 {
                unsafe { _mvl_string_drop(s) };
                continue 'outer;
            }
        }
        if write != read {
            unsafe { core::ptr::write((base + write * es) as *mut i32, s) };
        }
        write += 1;
    }
    arr.len = write as i32;
}

/// `_mvl_array_get_option_i64(a, idx)` → `*MvlOption` — Some(value) when
/// `idx` is in `[0, len)`, otherwise None. Returned Option owns its box
/// (`rc = 1`); caller drops via `_mvl_option_drop`.
///
/// Wraps `_mvl_array_get` + typed load — spares the emitter from doing
/// the null-check + Option construction inline for every `.get(i)` call.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer with `elem_size == 8`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_get_option_i64(a: i32, idx: i64) -> i32 {
    let elem_ptr = unsafe { _mvl_array_get(a, idx) };
    if elem_ptr == 0 {
        return _mvl_option_none();
    }
    let val: i64 = unsafe { core::ptr::read(elem_ptr as usize as *const i64) };
    _mvl_option_some_i64(val)
}

/// `_mvl_array_get_option_i32(a, idx)` → `*MvlOption` — i32 variant for
/// `List[Bool]` / `List[Byte]` / enum discriminant elements. Same
/// null-check + Some/None wrapping as the i64 variant.
///
/// # Safety
/// `a` must be a valid `MvlArray` pointer with `elem_size == 4`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_get_option_i32(a: i32, idx: i64) -> i32 {
    let elem_ptr = unsafe { _mvl_array_get(a, idx) };
    if elem_ptr == 0 {
        return _mvl_option_none();
    }
    let val: i32 = unsafe { core::ptr::read(elem_ptr as usize as *const i32) };
    _mvl_option_some_i32(val)
}

// ── MvlOption (#1821 partial — Phase 4 prelude, unblocks Phase 3 corpus) ─
//
// Heap-allocated `Option[T]` for the WASM ABI. Mirrors `runtime/llvm/src/
// abi.rs::MvlOption` in spirit, but this backend uses i32 pointers +
// i64/i32 payload rather than the LLVM `{ i8, ptr }` aggregate — WASM
// doesn't have struct returns in the C-ABI sense, so the emitter treats
// the pointer as opaque i32 and unpacks via accessor calls.
//
// Layout:
//   offset 0: i32 tag         — 0 = Some, 1 = None
//   offset 4: i32 rc          — refcount
//   offset 8: i64 value       — payload (only meaningful when tag == 0)
//
// Tag convention (Some = 0, None = 1) matches the LLVM emitter's
// `wrap_result_pair("0", …)` / `wrap_result_pair("1", …)` usage
// (emit_helpers.rs::emit_none_constructor, emit_option_constructor_tir).
// The abi.rs doc comment is the opposite convention — the LLVM code has
// diverged from that comment and we follow the code, not the comment.
//
// `_mvl_option_some_i32` upcasts the i32 into the i64 slot; `_i32`
// accessor downcasts via `i32.wrap_i64` on read. Single layout for all
// payload widths keeps the emitter dispatch simple.

#[repr(C)]
pub struct MvlOption {
    pub tag: i32,
    pub rc: i32,
    pub value: i64,
}

/// Construct `Some(v)` wrapping an i64-typed payload. Returns the
/// `MvlOption` pointer as i32 with `rc = 1`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_option_some_i64(v: i64) -> i32 {
    let opt = Box::new(MvlOption {
        tag: 0,
        rc: 1,
        value: v,
    });
    Box::into_raw(opt) as i32
}

/// Construct `Some(v)` wrapping an i32-typed payload. Upcast to i64 for
/// the shared `value` slot.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_option_some_i32(v: i32) -> i32 {
    _mvl_option_some_i64(v as i64)
}

/// Construct `None`. `value` is 0 by convention but shouldn't be read
/// when `tag != 0`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_option_none() -> i32 {
    let opt = Box::new(MvlOption {
        tag: 1,
        rc: 1,
        value: 0,
    });
    Box::into_raw(opt) as i32
}

/// `_mvl_option_tag(opt)` — 0 for Some, 1 for None.
///
/// # Safety
/// `opt` must be a valid `MvlOption` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_option_tag(opt: i32) -> i32 {
    if opt == 0 {
        return 1;
    }
    let o = unsafe { &*(opt as usize as *const MvlOption) };
    o.tag
}

/// Read the i64 payload. Undefined when `tag != 0` — the emitter must
/// only call this on a proven-Some branch.
///
/// # Safety
/// `opt` must be a valid `MvlOption` pointer whose Some payload is
/// i64-typed. Reading None's payload returns 0 (harmless).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_option_value_i64(opt: i32) -> i64 {
    if opt == 0 {
        return 0;
    }
    let o = unsafe { &*(opt as usize as *const MvlOption) };
    o.value
}

/// Read the i32 payload. Downcasts the i64 slot via `as i32`. Undefined
/// when `tag != 0`.
///
/// # Safety
/// `opt` must be a valid `MvlOption` pointer whose Some payload is
/// i32-typed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_option_value_i32(opt: i32) -> i32 {
    if opt == 0 {
        return 0;
    }
    let o = unsafe { &*(opt as usize as *const MvlOption) };
    o.value as i32
}

/// Refcount decrement; free the box when it reaches zero. Null-safe.
///
/// Payload is not itself dropped — `Option[Int]` payload is a value type,
/// nothing to reclaim. `Option[String]` / `Option[List[T]]` would need a
/// typed drop; those lower in later phases.
///
/// # Safety
/// `opt` must be a valid `MvlOption` pointer, not used after drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_option_drop(opt: i32) {
    if opt == 0 {
        return;
    }
    let o = unsafe { &mut *(opt as usize as *mut MvlOption) };
    o.rc -= 1;
    if o.rc > 0 {
        return;
    }
    unsafe {
        let _ = Box::from_raw(opt as usize as *mut MvlOption);
    }
}

// ── Result ops (#1821 extension) ─────────────────────────────────────────
//
// `Result[T, E]` — tagged union: Ok(v: T) or Err(e: E). Heap-allocated
// `MvlResult` pointer returned as i32, matching the Option pattern.
//
// Layout:
//   offset 0: i32 tag      — 0 = Ok, 1 = Err
//   offset 4: i32 rc       — refcount
//   offset 8: i64 ok_value — Ok payload (i32 types stored upcast to i64)
//   offset 16: i32 err_ptr  — *MvlString for Err payload (0 when tag == 0)
//
// Tag convention: Ok = 0, Err = 1 (matches Option Some=0, None=1).

#[repr(C)]
pub struct MvlResult {
    pub tag: i32,
    pub rc: i32,
    pub ok_value: i64,
    pub err_ptr: i32,
}

/// Construct `Ok(v)` with an i64-typed payload. Returns the `MvlResult`
/// pointer as i32 with `rc = 1`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_result_ok_i64(v: i64) -> i32 {
    let r = Box::new(MvlResult {
        tag: 0,
        rc: 1,
        ok_value: v,
        err_ptr: 0,
    });
    Box::into_raw(r) as i32
}

/// Construct `Ok(v)` with an i32-typed payload. Upcasts to i64.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_result_ok_i32(v: i32) -> i32 {
    _mvl_result_ok_i64(v as i64)
}

/// Construct `Err(s)` from a raw string `(ptr, len)` byte slice.
/// Copies bytes into a heap-allocated `MvlString`.
///
/// Stores the resulting pointer in *both* `err_ptr` (so `_mvl_result_drop`
/// still knows to free the string) and `ok_value` (so `_mvl_result_value_i32`/
/// `_mvl_result_value_i64` — the same generic getters `Ok` payloads use —
/// read it back correctly; previously only `err_ptr` was set, so those
/// getters returned 0 for any bound `Err(e)` string payload) (#2066).
///
/// # Safety
/// `ptr..ptr+len` must be valid readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_result_err_str(ptr: i32, len: i32) -> i32 {
    let err_ptr = unsafe { _mvl_string_new(ptr, len) };
    let r = Box::new(MvlResult {
        tag: 1,
        rc: 1,
        ok_value: err_ptr as i64,
        err_ptr,
    });
    Box::into_raw(r) as i32
}

/// Construct `Err(v)` with an i64-typed non-String payload (#2066). Stores
/// `v` in `ok_value` — the same slot `Ok` payloads use — since `err_ptr` is
/// reserved for the `*MvlString` case `_mvl_result_err_str` handles above
/// (it doubles as `_mvl_result_drop`'s ownership marker for that case; 0
/// here means "nothing to free").
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_result_err_i64(v: i64) -> i32 {
    let r = Box::new(MvlResult {
        tag: 1,
        rc: 1,
        ok_value: v,
        err_ptr: 0,
    });
    Box::into_raw(r) as i32
}

/// Construct `Err(v)` with an i32-typed non-String payload. Upcasts to i64.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_result_err_i32(v: i32) -> i32 {
    _mvl_result_err_i64(v as i64)
}

/// `_mvl_result_tag(r)` — 0 for Ok, 1 for Err.
///
/// # Safety
/// `r` must be a valid `MvlResult` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_result_tag(r: i32) -> i32 {
    if r == 0 {
        return 1;
    }
    let res = unsafe { &*(r as usize as *const MvlResult) };
    res.tag
}

/// Read the i64 Ok payload. Only valid when `tag == 0`.
///
/// # Safety
/// `r` must be a valid `MvlResult` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_result_value_i64(r: i32) -> i64 {
    if r == 0 {
        return 0;
    }
    let res = unsafe { &*(r as usize as *const MvlResult) };
    res.ok_value
}

/// Read the i32 Ok payload (downcast from i64 slot).
///
/// # Safety
/// `r` must be a valid `MvlResult` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_result_value_i32(r: i32) -> i32 {
    if r == 0 {
        return 0;
    }
    let res = unsafe { &*(r as usize as *const MvlResult) };
    res.ok_value as i32
}

/// Refcount decrement; free when it reaches zero. Drops the Err string
/// when present. Null-safe.
///
/// # Safety
/// `r` must be a valid `MvlResult` pointer, not used after drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_result_drop(r: i32) {
    if r == 0 {
        return;
    }
    let res = unsafe { &mut *(r as usize as *mut MvlResult) };
    res.rc -= 1;
    if res.rc > 0 {
        return;
    }
    if res.tag == 1 && res.err_ptr != 0 {
        unsafe { _mvl_string_drop(res.err_ptr) };
    }
    unsafe {
        let _ = Box::from_raw(r as usize as *mut MvlResult);
    }
}

// ── String parse ops ─────────────────────────────────────────────────────

/// `s.parse_int()` — parse a `(ptr, len)` byte slice as a decimal integer.
/// Returns a heap-allocated `MvlResult` (tag=0 → Ok(i64), tag=1 → Err(*MvlString)).
///
/// # Safety
/// `ptr..ptr+len` must be valid readable memory (or `len == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_parse_int(ptr: i32, len: i32) -> i32 {
    let bytes = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(bytes).unwrap_or("").trim();
    match text.parse::<i64>() {
        Ok(n) => _mvl_result_ok_i64(n),
        Err(e) => {
            let msg = e.to_string();
            unsafe { _mvl_result_err_str(msg.as_ptr() as i32, msg.len() as i32) }
        }
    }
}

/// `s.parse_float()` — parse a `(ptr, len)` byte slice as a 64-bit float.
/// Returns a heap-allocated `MvlResult` (tag=0 → Ok(f64), tag=1 → Err(*MvlString)).
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_parse_float`.
///
/// # Safety
/// `ptr..ptr+len` must be valid readable memory (or `len == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_parse_float(ptr: i32, len: i32) -> i32 {
    let bytes = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(bytes).unwrap_or("").trim();
    match text.parse::<f64>() {
        Ok(v) => {
            // Store f64 as i64 bits in the result, caller reinterprets
            _mvl_result_ok_i64(v.to_bits() as i64)
        }
        Err(e) => {
            let msg = e.to_string();
            unsafe { _mvl_result_err_str(msg.as_ptr() as i32, msg.len() as i32) }
        }
    }
}

// ── Additional string primitives (Unicode-aware) ─────────────────────────

/// `s.chars()` — decompose string into a list of single-character strings.
/// Returns a `*MvlArray` of `*MvlString` with `elem_size == 4`.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_chars`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_chars(ptr: i32, len: i32) -> i32 {
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    let arr = _mvl_array_new(4, 0);
    for c in text.chars() {
        let mut buf = [0u8; 4];
        let char_str = c.encode_utf8(&mut buf);
        unsafe { _mvl_array_push_i32(arr, alloc_mvl_string(char_str.as_bytes())) };
    }
    arr
}

/// `s.char_at(i)` — return the character at index `i` (0-based).
/// Returns a `*MvlOption` wrapping a `*MvlString`, or None if out of range.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_char_at`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_char_at(ptr: i32, len: i32, idx: i64) -> i32 {
    if idx < 0 {
        return _mvl_option_none();
    }
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    match text.chars().nth(idx as usize) {
        Some(c) => {
            let mut buf = [0u8; 4];
            let char_str = c.encode_utf8(&mut buf);
            let ms = alloc_mvl_string(char_str.as_bytes());
            _mvl_option_some_i32(ms)
        }
        None => _mvl_option_none(),
    }
}

/// `s.byte_at(i)` — return the byte value at character position `i` (0-based).
/// Returns a `*MvlOption` wrapping a `u8` (as i32), or None if out of range
/// or if the character's codepoint > 255.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_byte_at`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_byte_at(ptr: i32, len: i32, idx: i64) -> i32 {
    if idx < 0 {
        return _mvl_option_none();
    }
    let s = unsafe { slice_or_empty(ptr, len) };
    let text = core::str::from_utf8(s).unwrap_or("");
    match text.chars().nth(idx as usize) {
        Some(c) => {
            let cp = c as u32;
            if cp <= 255 {
                _mvl_option_some_i32(cp as i32)
            } else {
                _mvl_option_none()
            }
        }
        None => _mvl_option_none(),
    }
}

/// Reconstruct a `String` from a raw byte sequence (Latin-1 / ISO-8859-1).
/// Each byte 0..=255 maps to the Unicode codepoint of the same numeric value.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_from_bytes`.
///
/// `bytes` is a `*MvlArray` of `u8` values (elem_size == 1 or stored as i32).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_from_bytes(arr: i32) -> i32 {
    if arr == 0 {
        return alloc_mvl_string(&[]);
    }
    let array = unsafe { &*(arr as usize as *const MvlArray) };
    let mut result = String::with_capacity(array.len as usize);
    // Bytes are stored as i32 values in the array (elem_size typically 4 for Byte)
    for i in 0..array.len {
        let slot = (array.ptr as usize) + (i as usize) * (array.elem_size as usize);
        let byte_val = if array.elem_size == 1 {
            unsafe { *(slot as *const u8) }
        } else {
            // Byte stored as i32
            unsafe { *(slot as *const i32) as u8 }
        };
        result.push(byte_val as char);
    }
    alloc_mvl_string(result.as_bytes())
}

/// Reconstruct a `String` from a list of single-character strings.
/// Matches `runtime/rust/src/stdlib/primitives.rs::str_from_chars`.
///
/// `chars` is a `*MvlArray` of `*MvlString` (elem_size == 4).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_from_chars(arr: i32) -> i32 {
    if arr == 0 {
        return alloc_mvl_string(&[]);
    }
    let array = unsafe { &*(arr as usize as *const MvlArray) };
    let mut result = String::new();
    for i in 0..array.len {
        let slot = (array.ptr as usize) + (i as usize) * 4; // elem_size == 4 for *MvlString
        let ms_ptr = unsafe { *(slot as *const i32) };
        if ms_ptr != 0 {
            let ms = unsafe { &*(ms_ptr as usize as *const MvlString) };
            let bytes = unsafe { slice_or_empty(ms.ptr, ms.len) };
            if let Ok(s) = core::str::from_utf8(bytes) {
                result.push_str(s);
            }
        }
    }
    alloc_mvl_string(result.as_bytes())
}

// ── IFC audit events (#2013) ─────────────────────────────────────────────
//
// `relabel name(expr, "tag") audit` emits a JSONL audit line via this call.
// Mirrors `runtime/rust/src/stdlib/audit.rs::emit_relabel_event` /
// `runtime/llvm/src/stdlib/audit.rs::_mvl_audit_emit_relabel` — same JSONL
// shape and same `MVL_AUDIT_SINK`-env-var-or-stderr fallback. Kept as a
// self-contained copy rather than a dependency on `mvl_runtime_rust` to
// avoid pulling a native-oriented crate onto the wasm32-wasip1 target.

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Convert Unix epoch seconds to (year, month, day, hour, min, sec) UTC.
fn epoch_to_ymd_hms(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    secs /= 60;
    let mi = secs % 60;
    secs /= 60;
    let h = secs % 24;
    let mut days = secs / 24;
    let mut y = 1970u64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }
    let month_days = [
        31u64,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        mo += 1;
    }
    (y, mo, days + 1, h, mi, s)
}

/// Emit a structured JSONL audit event for an IFC relabel transition.
///
/// Five `(ptr, len)` byte-slice pairs: transition name, from-label,
/// to-label, tag, and location. Writes to the path in `MVL_AUDIT_SINK`
/// (env var) if set and reachable, otherwise to stderr.
///
/// # Safety
/// Each `(ptr, len)` pair must describe a live, readable byte range (or
/// `len == 0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_audit_emit_relabel(
    t_ptr: i32,
    t_len: i32,
    from_ptr: i32,
    from_len: i32,
    to_ptr: i32,
    to_len: i32,
    tag_ptr: i32,
    tag_len: i32,
    loc_ptr: i32,
    loc_len: i32,
) {
    let read = |p: i32, l: i32| -> String {
        String::from_utf8_lossy(unsafe { slice_or_empty(p, l) }).into_owned()
    };
    let transition = read(t_ptr, t_len);
    let from_label = read(from_ptr, from_len);
    let to_label = read(to_ptr, to_len);
    let tag = read(tag_ptr, tag_len);
    let location = read(loc_ptr, loc_len);

    let ts = {
        let dur = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(dur.as_secs());
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    };
    let line = format!(
        "{{\"timestamp\":\"{ts}\",\"kind\":\"relabel\",\"transition\":\"{}\",\"from\":\"{}\",\"to\":\"{}\",\"tag\":\"{}\",\"location\":\"{}\"}}",
        json_escape(&transition),
        json_escape(&from_label),
        json_escape(&to_label),
        json_escape(&tag),
        json_escape(&location),
    );
    if let Ok(sink) = std::env::var("MVL_AUDIT_SINK") {
        use std::io::Write as _;
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sink);
        if let Ok(mut f) = opened {
            // A write failure here (disk full, EIO, broken pipe) still needs
            // the event surfaced somewhere — fall through to stderr rather
            // than silently dropping it, same as when `open` itself fails.
            if writeln!(f, "{line}").is_ok() {
                return;
            }
        }
    }
    eprintln!("[mvl-audit] {line}");
}

// ── Set ops (#1820) ──────────────────────────────────────────────────────
//
// Set[T] is backed by MvlArray (same as List[T]) but enforces uniqueness.
// Construction: emit elements with `_mvl_array_push_*`, then call
// `_mvl_array_dedup_{i64|i32}` once to sort and remove duplicates in-place.
// Subsequent `insert` uses the element-presence check before pushing.

/// `_mvl_array_dedup_i64(a)` — sort and deduplicate i64 elements in-place.
/// Used after constructing a `Set[Int]` literal. Element order after dedup
/// is sorted (ascending), which is deterministic for corpus tests.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_dedup_i64(a: i32) {
    if a == 0 {
        return;
    }
    let arr = &mut *(a as usize as *mut MvlArray);
    let len = arr.len as usize;
    if len <= 1 {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(arr.ptr as *mut i64, len);
    slice.sort_unstable();
    let mut write = 1usize;
    for read in 1..len {
        if slice[read] != slice[write - 1] {
            slice[write] = slice[read];
            write += 1;
        }
    }
    arr.len = write as i32;
}

/// `_mvl_array_dedup_i32(a)` — sort and deduplicate i32 elements in-place.
/// Used after constructing a `Set[Bool]` or `Set[Byte]` literal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_dedup_i32(a: i32) {
    if a == 0 {
        return;
    }
    let arr = &mut *(a as usize as *mut MvlArray);
    let len = arr.len as usize;
    if len <= 1 {
        return;
    }
    let slice = std::slice::from_raw_parts_mut(arr.ptr as *mut i32, len);
    slice.sort_unstable();
    let mut write = 1usize;
    for read in 1..len {
        if slice[read] != slice[write - 1] {
            slice[write] = slice[read];
            write += 1;
        }
    }
    arr.len = write as i32;
}

/// `_mvl_array_contains_i64(a, val) -> i32` — 1 if `val` is in the array, 0 otherwise.
/// Used for `Set[Int].contains(val)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_contains_i64(a: i32, val: i64) -> i32 {
    if a == 0 {
        return 0;
    }
    let arr = &*(a as usize as *const MvlArray);
    let slice = std::slice::from_raw_parts(arr.ptr as *const i64, arr.len as usize);
    slice.iter().any(|&e| e == val) as i32
}

/// `_mvl_array_contains_i32(a, val) -> i32` — 1 if `val` is in the array, 0 otherwise.
/// Used for `Set[Bool].contains(val)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_contains_i32(a: i32, val: i32) -> i32 {
    if a == 0 {
        return 0;
    }
    let arr = &*(a as usize as *const MvlArray);
    let slice = std::slice::from_raw_parts(arr.ptr as *const i32, arr.len as usize);
    slice.iter().any(|&e| e == val) as i32
}

/// `_mvl_array_insert_i64(a, val)` — push `val` only if not already present.
/// Used for `Set[Int].insert(val)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_insert_i64(a: i32, val: i64) {
    if _mvl_array_contains_i64(a, val) == 0 {
        _mvl_array_push_i64(a, val);
    }
}

/// `_mvl_array_insert_i32(a, val)` — push `val` only if not already present.
/// Used for `Set[Bool].insert(val)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_array_insert_i32(a: i32, val: i32) {
    if _mvl_array_contains_i32(a, val) == 0 {
        _mvl_array_push_i32(a, val);
    }
}

// ── Map[String, Int] ops (#1820, #1993) ──────────────────────────────────
//
// `MvlMap` is a simple linear-scan map from `String` keys to `i64` values.
// Backed by `Vec<MvlMapEntry>` allocated on the Rust heap.
//
// Keys are stored as `*MvlString` handles (i32) so that drop correctly
// releases the key allocation via `_mvl_string_drop`. This matches the
// List[String] / Set[String] pattern (PR #1992) and is consistent with the
// LLVM backend's treatment of pointer-typed map keys.
//
// Naming convention: `si64` suffix = String key, i64 (Int) value.

struct MvlMapEntry {
    key: i32, // *MvlString handle — owned; freed by _mvl_map_drop_si64
    val: i64,
}

struct MvlMap {
    entries: Vec<MvlMapEntry>,
    rc: u32,
}

/// `_mvl_map_new_si64() -> i32` — allocate an empty `Map[String, Int]`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_map_new_si64() -> i32 {
    let m = Box::new(MvlMap {
        entries: Vec::new(),
        rc: 1,
    });
    Box::into_raw(m) as usize as i32
}

/// `_mvl_map_len(m) -> i64` — number of entries in the map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_len(m: i32) -> i64 {
    if m == 0 {
        return 0;
    }
    let map = &*(m as usize as *const MvlMap);
    map.entries.len() as i64
}

/// Compare a `*MvlString` handle's content against raw bytes `(ptr, len)`.
unsafe fn ms_handle_eq_bytes(handle: i32, k_ptr: i32, k_len: i32) -> bool {
    if handle == 0 {
        return k_len == 0;
    }
    let ms = &*(handle as usize as *const MvlString);
    if ms.len != k_len {
        return false;
    }
    slice_or_empty(ms.ptr, ms.len) == slice_or_empty(k_ptr, k_len)
}

/// `_mvl_map_insert_si64(m, k_ptr, k_len, val)` — insert or overwrite the
/// entry for the given string key. A new `*MvlString` handle is allocated
/// for each distinct key; the map owns it until drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_insert_si64(m: i32, k_ptr: i32, k_len: i32, val: i64) {
    if m == 0 {
        return;
    }
    let map = &mut *(m as usize as *mut MvlMap);
    for entry in &mut map.entries {
        if ms_handle_eq_bytes(entry.key, k_ptr, k_len) {
            entry.val = val;
            return;
        }
    }
    let key = _mvl_string_new(k_ptr, k_len);
    map.entries.push(MvlMapEntry { key, val });
}

/// `_mvl_map_get_si64(m, k_ptr, k_len) -> *MvlOption` — look up a key and
/// return `Some(val)` or `None` as a heap-allocated `MvlOption` (same ABI
/// as `_mvl_array_get_option_i64`). Caller must drop the returned pointer.
///
/// Only for `Map[String, V]` where `V` is a plain scalar (Int, Bool, enum
/// discriminant, …) — `val` is handed back verbatim, not cloned. For
/// `Map[String, String]`, use [`_mvl_map_get_str`] instead: `val` there is a
/// `*MvlString` handle still owned by this map's entry, and the emitter's
/// `unwrap_or`/match unpacking unconditionally drops whatever it extracts
/// from the Option — returning the raw (un-cloned) handle from a String map
/// is a use-after-free the moment the caller drops the "unwrapped" string
/// and then this map (#2047).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_get_si64(m: i32, k_ptr: i32, k_len: i32) -> i32 {
    if m == 0 {
        return _mvl_option_none();
    }
    let map = &*(m as usize as *const MvlMap);
    for entry in &map.entries {
        if ms_handle_eq_bytes(entry.key, k_ptr, k_len) {
            return _mvl_option_some_i64(entry.val);
        }
    }
    _mvl_option_none()
}

/// `_mvl_map_get_str(m, k_ptr, k_len) -> *MvlOption` — look up a key in a
/// `Map[String, String]` and return `Some(val)` or `None`. Unlike
/// [`_mvl_map_get_si64`], `val` (a `*MvlString` handle) is refcount-cloned
/// before being wrapped, so it is a fresh, independently-owned reference —
/// safe for the caller to drop without affecting this map's own copy
/// (#2047).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_get_str(m: i32, k_ptr: i32, k_len: i32) -> i32 {
    if m == 0 {
        return _mvl_option_none();
    }
    let map = &*(m as usize as *const MvlMap);
    for entry in &map.entries {
        if ms_handle_eq_bytes(entry.key, k_ptr, k_len) {
            let cloned = _mvl_string_clone(entry.val as i32);
            return _mvl_option_some_i32(cloned);
        }
    }
    _mvl_option_none()
}

/// `_mvl_map_contains_key_si64(m, k_ptr, k_len) -> i32` — 1 if the key is
/// present, 0 otherwise. Used for `Map[String, Int].contains_key(key)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_contains_key_si64(m: i32, k_ptr: i32, k_len: i32) -> i32 {
    if m == 0 {
        return 0;
    }
    let map = &*(m as usize as *const MvlMap);
    for entry in &map.entries {
        if ms_handle_eq_bytes(entry.key, k_ptr, k_len) {
            return 1;
        }
    }
    0
}

/// `_mvl_map_drop_si64(m)` — decrement refcount; free when it reaches zero.
/// Each key `*MvlString` handle is explicitly dropped before the map is freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_map_drop_si64(m: i32) {
    if m == 0 {
        return;
    }
    let ptr = m as usize as *mut MvlMap;
    (*ptr).rc = (*ptr).rc.saturating_sub(1);
    if (*ptr).rc == 0 {
        for entry in &(*ptr).entries {
            _mvl_string_drop(entry.key);
        }
        let _ = Box::from_raw(ptr);
    }
}

// ── Struct / payload allocation (#1821) ──────────────────────────────────

/// Allocate `size` bytes on the Rust heap and return the pointer as i32.
/// Used by the WASM emitter for struct construction and payload-enum
/// header + payload allocation (#1821). The returned region is zeroed.
/// Callers are responsible for freeing via `Box::from_raw` when done;
/// for corpus tests the allocations are short-lived and leaking is fine.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_struct_alloc(size: i32) -> i32 {
    if size <= 0 {
        return 0;
    }
    let mut v: Vec<u8> = vec![0u8; size as usize];
    let ptr = v.as_mut_ptr() as i32;
    std::mem::forget(v);
    ptr
}

// ── format() builtin (#2039) ──────────────────────────────────────────────

/// `format(template, values)` — positional `{}` interpolation, mirroring
/// `runtime/rust/src/prelude.rs::mvl_format`. `template` is a raw
/// `(ptr, len)` byte range; `values` is a `List[String]` — a `*MvlArray`
/// whose elements are `*MvlString` pointers (`elem_size == 4`). Returns a
/// `*MvlString`; the emitter unpacks `.ptr`/`.len` immediately after the
/// call, same as `_mvl_string_new`.
///
/// # Safety
/// `values` must be `0` or a valid `*MvlArray` of `*MvlString` elements.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_format(tmpl_ptr: i32, tmpl_len: i32, values: i32) -> i32 {
    let template = unsafe { slice_or_empty(tmpl_ptr, tmpl_len) };
    let elems: &[i32] = if values == 0 {
        &[]
    } else {
        let arr = unsafe { &*(values as usize as *const MvlArray) };
        unsafe { core::slice::from_raw_parts(arr.ptr as usize as *const i32, arr.len as usize) }
    };
    let mut result = Vec::<u8>::with_capacity(template.len());
    let mut val_idx = 0usize;
    let mut i = 0usize;
    while i < template.len() {
        if template[i] == b'{' && template.get(i + 1) == Some(&b'}') {
            if let Some(&ms_ptr) = elems.get(val_idx) {
                if ms_ptr != 0 {
                    let ms = unsafe { &*(ms_ptr as usize as *const MvlString) };
                    result.extend_from_slice(unsafe { slice_or_empty(ms.ptr, ms.len) });
                }
            }
            val_idx += 1;
            i += 2;
        } else {
            result.push(template[i]);
            i += 1;
        }
    }
    alloc_mvl_string(&result)
}

// ── std.io::read_file (#2076) ────────────────────────────────────────────
//
// WASI file read, backed directly by wasm32-wasip1's `std::fs` — no
// hand-declared `path_open`/`fd_read` WASI imports needed the way #2056's
// `write(fd, msg)` hand-declares `fd_write` in `wasm_text.rs`'s inline WAT
// prelude; `std::fs::read_to_string` already lowers to the right WASI
// syscalls for this target. Backs both `std.io::read_file` and
// `std.io::_read_file` (std/io.mvl) identically — `Tainted[String]`/`Path`
// erase to the same `(ptr, len)` representation as `String` at this layer.
//
// `IoError` variant discriminants below (0=NotFound, 1=PermissionDenied,
// 2=AlreadyExists, 3=Other(String)) match std/io.mvl's declaration order
// — the same order `collect_payload_enums` (wasm_text.rs) assigns via
// `enumerate()` — and the existing `IO_ERR_*` constants in
// `runtime/llvm/src/stdlib/io.rs`.

const IO_ERR_NOT_FOUND: i32 = 0;
const IO_ERR_PERMISSION_DENIED: i32 = 1;
const IO_ERR_ALREADY_EXISTS: i32 = 2;
const IO_ERR_OTHER: i32 = 3;

/// Build an `IoError` payload-enum header: the same 8-byte
/// `{disc: i32, payload_ptr: i32}` shape `emit_enum_variant_construct`
/// (wasm_text.rs) builds for every other payload enum, so a caller that
/// binds a named `Err(e)` and matches on it (`IoError::NotFound => ...`)
/// decodes it exactly like compiler-emitted construction would. `msg` is
/// `Some` only for `Other`; its bytes become a fresh `MvlString`, stored
/// as an i64-widened pointer at payload offset 0 — matching
/// `emit_payload_store`'s String-field convention.
fn alloc_io_error(disc: i32, msg: Option<&[u8]>) -> i32 {
    let header = _mvl_struct_alloc(8);
    if header == 0 {
        return 0;
    }
    unsafe {
        *(header as usize as *mut i32) = disc;
    }
    let payload_ptr = match msg {
        Some(bytes) => {
            let str_ptr = alloc_mvl_string(bytes);
            let payload = _mvl_struct_alloc(8);
            if payload != 0 {
                unsafe {
                    *(payload as usize as *mut i64) = str_ptr as i64;
                }
            }
            payload
        }
        None => 0,
    };
    unsafe {
        *((header as usize + 4) as *mut i32) = payload_ptr;
    }
    header
}

/// Map a `std::io::Error` to an allocated `IoError` header, mirroring
/// `runtime/rust/src/stdlib/io.rs::sanitize_io_error`'s NotFound /
/// PermissionDenied / AlreadyExists / Other(kind-string) cases.
fn io_error_from_std(e: &std::io::Error) -> i32 {
    match e.kind() {
        std::io::ErrorKind::NotFound => alloc_io_error(IO_ERR_NOT_FOUND, None),
        std::io::ErrorKind::PermissionDenied => alloc_io_error(IO_ERR_PERMISSION_DENIED, None),
        std::io::ErrorKind::AlreadyExists => alloc_io_error(IO_ERR_ALREADY_EXISTS, None),
        _ => alloc_io_error(IO_ERR_OTHER, Some(e.to_string().as_bytes())),
    }
}

/// `read_file(path)` / `_read_file(path)` — read a file's entire contents.
/// `path` is a raw `(ptr, len)` byte range. Returns a heap-allocated
/// `MvlResult`:
///   Ok  -> `_mvl_result_ok_i32(<*MvlString>)`
///   Err -> `_mvl_result_err_i32(<IoError header ptr>)` — #2066's
///   non-String Err-payload convention (`ok_value` holds the header
///   pointer, `err_ptr` stays 0). The header/payload are leaked on drop,
///   same tradeoff `runtime/llvm`'s `LlvmEnumError::with_str` already
///   documents as "acceptable for MVP error paths."
///
/// # Safety
/// `path_ptr..path_ptr+path_len` must be valid readable memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_read_file(path_ptr: i32, path_len: i32) -> i32 {
    let path_bytes = unsafe { slice_or_empty(path_ptr, path_len) };
    let path = match core::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ));
        }
    };
    match std::fs::read_to_string(path) {
        Ok(contents) => _mvl_result_ok_i32(alloc_mvl_string(contents.as_bytes())),
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

// ── std.env — environment and process control ───────────────────────────
//
// WASI provides environment variables, command-line arguments, and process
// exit via the wasm32-wasip1 target's `std::env` facade.

/// `env.set(name, value)` — set an environment variable.
/// Returns a `*MvlResult`: Ok(()) on success, Err(String) on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_env_set(
    name_ptr: i32,
    name_len: i32,
    val_ptr: i32,
    val_len: i32,
) -> i32 {
    let name = match core::str::from_utf8(unsafe { slice_or_empty(name_ptr, name_len) }) {
        Ok(s) => s,
        Err(_) => return _mvl_result_err_i32(alloc_mvl_string(b"name is not valid UTF-8")),
    };
    let value = match core::str::from_utf8(unsafe { slice_or_empty(val_ptr, val_len) }) {
        Ok(s) => s,
        Err(_) => return _mvl_result_err_i32(alloc_mvl_string(b"value is not valid UTF-8")),
    };
    // WASI supports setting env vars
    std::env::set_var(name, value);
    _mvl_result_ok_i64(0) // Ok(Unit)
}

/// `env.remove_var(name)` — unset an environment variable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_env_remove_var(name_ptr: i32, name_len: i32) {
    if let Ok(name) = core::str::from_utf8(unsafe { slice_or_empty(name_ptr, name_len) }) {
        std::env::remove_var(name);
    }
}

/// `env.current_dir()` — get the current working directory.
/// Returns a `*MvlResult`: Ok(*MvlString) on success, Err(*IoError) on failure.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_current_dir() -> i32 {
    match std::env::current_dir() {
        Ok(path) => {
            let s = path.to_string_lossy();
            _mvl_result_ok_i32(alloc_mvl_string(s.as_bytes()))
        }
        Err(e) => _mvl_result_err_i32(alloc_io_error(IO_ERR_OTHER, Some(e.to_string().as_bytes()))),
    }
}

/// `env.chdir(path)` — change the current working directory.
/// Returns a `*MvlResult`: Ok(()) on success, Err(*IoError) on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_env_chdir(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    match std::env::set_current_dir(path) {
        Ok(()) => _mvl_result_ok_i64(0),
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `env.exit(code)` — terminate the process with the given exit code.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_exit(code: i64) -> ! {
    std::process::exit(code as i32)
}

/// `env.getuid()` — effective user ID. Returns 0 on WASI (no Unix UIDs).
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_getuid() -> i64 {
    0 // WASI doesn't expose Unix UIDs
}

/// `env.getgid()` — effective group ID. Returns 0 on WASI (no Unix GIDs).
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_getgid() -> i64 {
    0 // WASI doesn't expose Unix GIDs
}

/// `env.all()` — return all environment variables as a List of (name, value) pairs.
/// Returns a `*MvlArray` of struct pointers (each struct has two *MvlString fields).
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_env_all() -> i32 {
    // Each entry is a struct { name: *MvlString, value: *MvlString } = 8 bytes
    let arr = _mvl_array_new(8, 0);
    for (key, value) in std::env::vars() {
        let name_ptr = alloc_mvl_string(key.as_bytes());
        let value_ptr = alloc_mvl_string(value.as_bytes());
        // Allocate struct with two i32 pointers
        let entry = _mvl_struct_alloc(8);
        unsafe {
            *(entry as usize as *mut i32) = name_ptr;
            *((entry as usize + 4) as *mut i32) = value_ptr;
            _mvl_array_push_i32(arr, entry);
        }
    }
    arr
}

// ── std.io — file system operations ──────────────────────────────────────
//
// WASI provides filesystem access via the wasm32-wasip1 target.

/// `io.write_file(path, content)` — write content to a file, creating or truncating.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_write_file(
    path_ptr: i32,
    path_len: i32,
    content_ptr: i32,
    content_len: i32,
) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    let content = unsafe { slice_or_empty(content_ptr, content_len) };
    match std::fs::write(path, content) {
        Ok(()) => _mvl_result_ok_i64(0),
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `io.append(path, content)` — append content to a file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_append(
    path_ptr: i32,
    path_len: i32,
    content_ptr: i32,
    content_len: i32,
) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    let content = unsafe { slice_or_empty(content_ptr, content_len) };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    match file {
        Ok(mut f) => {
            use std::io::Write;
            match f.write_all(content) {
                Ok(()) => _mvl_result_ok_i64(0),
                Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
            }
        }
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `io.path_exists(path)` — check if a path exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_exists(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    std::path::Path::new(path).exists() as i32
}

/// `io.is_file(path)` — check if path is a file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_is_file(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    std::path::Path::new(path).is_file() as i32
}

/// `io.is_dir(path)` — check if path is a directory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_is_dir(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    std::path::Path::new(path).is_dir() as i32
}

/// `io.create_dir_all(path)` — create a directory and all parent directories.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_create_dir_all(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    match std::fs::create_dir_all(path) {
        Ok(()) => _mvl_result_ok_i64(0),
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `io.remove(path)` — remove a file or empty directory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_remove(path_ptr: i32, path_len: i32) -> i32 {
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    let p = std::path::Path::new(path);
    let result = if p.is_dir() {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => _mvl_result_ok_i64(0),
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `io.open(path)` — open a file for reading/writing, creating it if it
/// does not exist. Mirrors `runtime/rust/src/stdlib/io.rs::open`: the
/// returned `Fd.inner` is a raw WASI file descriptor obtained via
/// `IntoRawFd`, heap-allocated into an `Fd { inner: Int }` struct the same
/// way `stdout()`/`stderr()` do in the emitter (`wasm_text.rs`) — 8 bytes,
/// the fd stored as i64 at offset 0. Returns a `*MvlResult`: Ok(*Fd) on
/// success, Err(*IoError) on failure (#2110).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_open(path_ptr: i32, path_len: i32) -> i32 {
    use std::os::wasi::io::IntoRawFd as _;
    let path = match core::str::from_utf8(unsafe { slice_or_empty(path_ptr, path_len) }) {
        Ok(s) => s,
        Err(_) => {
            return _mvl_result_err_i32(alloc_io_error(
                IO_ERR_OTHER,
                Some(b"path is not valid UTF-8"),
            ))
        }
    };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path);
    match file {
        Ok(f) => {
            let raw_fd = f.into_raw_fd() as i64;
            let fd_struct = _mvl_struct_alloc(8);
            unsafe {
                *(fd_struct as usize as *mut i64) = raw_fd;
            }
            _mvl_result_ok_i32(fd_struct)
        }
        Err(e) => _mvl_result_err_i32(io_error_from_std(&e)),
    }
}

/// `io.close(fd)` — close a file descriptor and release the OS resource.
/// Reconstructs the `File` via `FromRawFd` and drops it, mirroring
/// `runtime/rust/src/stdlib/io.rs::close` (#2110).
///
/// # Safety
/// `fd` must be a raw WASI file descriptor previously returned by
/// `_mvl_io_open` and not already closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_io_close(fd: i32) {
    use std::os::wasi::io::FromRawFd as _;
    drop(unsafe { std::fs::File::from_raw_fd(fd) });
}

// ── std.time — time and duration ─────────────────────────────────────────
//
// WASI provides wall-clock time via `std::time::SystemTime`.

/// `time.now()` — current time as epoch seconds (i64).
/// Returns a boxed i64 handle that can be passed to `_instant_epoch_seconds`.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_time_now() -> i32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let boxed = Box::new(secs);
    Box::into_raw(boxed) as usize as i32
}

/// `_instant_epoch_seconds(handle)` — read epoch seconds from an Instant handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_time_instant_epoch_seconds(handle: i32) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { *(handle as usize as *const i64) }
}

/// `time.sleep(secs, nanos)` — sleep for the specified duration.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_time_thread_sleep(secs: i64, nanos: i64) {
    let duration = std::time::Duration::new(secs.max(0) as u64, nanos.max(0) as u32);
    std::thread::sleep(duration);
}

// ── std.random — pseudo-random number generation ─────────────────────────
//
// Uses a simple xorshift64 PRNG seeded from WASI's random_get or fallback.

static RANDOM_STATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn ensure_seeded() {
    use std::sync::atomic::Ordering;
    if RANDOM_STATE.load(Ordering::Relaxed) == 0 {
        // Seed from system time + address entropy
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let seed = seed ^ ((&RANDOM_STATE as *const _ as u64).wrapping_mul(0x517cc1b727220a95));
        RANDOM_STATE.store(seed | 1, Ordering::Relaxed); // Ensure non-zero
    }
}

fn xorshift64() -> u64 {
    use std::sync::atomic::Ordering;
    ensure_seeded();
    let mut state = RANDOM_STATE.load(Ordering::Relaxed);
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    RANDOM_STATE.store(state, Ordering::Relaxed);
    state
}

/// `random.int(min, max)` — random integer in [min, max] inclusive.
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_random_int(min: i64, max: i64) -> i64 {
    if min >= max {
        return min;
    }
    let range = (max - min + 1) as u64;
    let r = xorshift64() % range;
    min + r as i64
}

/// `random.float()` — random float in [0.0, 1.0).
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_random_float() -> f64 {
    let r = xorshift64();
    // Convert to [0, 1) by dividing by 2^64
    (r as f64) / (u64::MAX as f64 + 1.0)
}

/// `random.bytes(n)` — return n random bytes as a `*MvlArray` of i64 values [0, 255].
#[unsafe(no_mangle)]
pub extern "C" fn _mvl_random_bytes(n: i64) -> i32 {
    let arr = _mvl_array_new(8, n.max(0) as i32); // elem_size 8 for i64
    for _ in 0..n.max(0) {
        let byte = (xorshift64() & 0xFF) as i64;
        unsafe { _mvl_array_push_i64(arr, byte) };
    }
    arr
}

/// `random.choice_index(arr)` — random index from array, or -1 if empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_random_choice_index(arr: i32) -> i64 {
    if arr == 0 {
        return -1;
    }
    let len = unsafe { _mvl_array_len(arr) };
    if len == 0 {
        return -1;
    }
    _mvl_random_int(0, len - 1)
}

/// `random.shuffle(arr)` — return a shuffled copy of the array (Fisher-Yates).
/// Returns a new `*MvlArray`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_random_shuffle(arr: i32) -> i32 {
    if arr == 0 {
        return _mvl_array_new(8, 0);
    }
    let src = unsafe { &*(arr as usize as *const MvlArray) };
    let len = src.len as usize;
    let elem_size = src.elem_size as usize;

    // Clone the array
    let clone = _mvl_array_new(src.elem_size, src.len);
    let dst = unsafe { &mut *(clone as usize as *mut MvlArray) };
    if len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                src.ptr as *const u8,
                dst.ptr as *mut u8,
                len * elem_size,
            );
        }
        dst.len = src.len;
    }

    // Fisher-Yates shuffle
    for i in (1..len).rev() {
        let j = _mvl_random_int(0, i as i64) as usize;
        if i != j {
            // Swap elements at i and j
            let ptr_i = (dst.ptr as usize + i * elem_size) as *mut u8;
            let ptr_j = (dst.ptr as usize + j * elem_size) as *mut u8;
            for k in 0..elem_size {
                unsafe {
                    let tmp = *ptr_i.add(k);
                    *ptr_i.add(k) = *ptr_j.add(k);
                    *ptr_j.add(k) = tmp;
                }
            }
        }
    }

    clone
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
        assert_eq!(unsafe { _mvl_string_len(addr(a), a.len() as i32) }, 5);
    }

    #[test]
    fn len_empty() {
        assert_eq!(unsafe { _mvl_string_len(0, 0) }, 0);
    }

    #[test]
    fn len_unicode() {
        // "héllo" — 5 chars but 6 bytes (é is 2 bytes in UTF-8)
        let a = "héllo".as_bytes();
        assert_eq!(
            unsafe { _mvl_string_len(a.as_ptr() as i32, a.len() as i32) },
            5
        );
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
    fn to_upper_unicode() {
        // `é` (U+00E9) uppercases to `É` (U+00C9) — full Unicode case conversion.
        // "café" → "CAFÉ"
        let s = "café".as_bytes();
        let ptr = unsafe { _mvl_string_to_upper(s.as_ptr() as i32, s.len() as i32) };
        assert_eq!(unsafe { concat_result(ptr) }, "CAFÉ".as_bytes());
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

    // ── split (#2014) ────
    //
    // Collect a split result into owned Strings so each assertion reads as a
    // plain slice comparison. Consumes the array via
    // `_mvl_string_ptr_array_drop`, which is also the drop path the emitter
    // picks for a `List[String]` local — so every one of these tests
    // exercises that pairing, not just the split itself.
    unsafe fn split_parts(s: &'static [u8], sep: &'static [u8]) -> Vec<Vec<u8>> {
        let sep_ptr = if sep.is_empty() { 0 } else { addr(sep) };
        let arr = unsafe { _mvl_string_split(addr(s), s.len() as i32, sep_ptr, sep.len() as i32) };
        let n = unsafe { _mvl_array_len(arr) };
        let mut out = Vec::new();
        for i in 0..n {
            let slot = unsafe { _mvl_array_get(arr, i) };
            let sp = unsafe { core::ptr::read(slot as *const i32) };
            out.push(unsafe { concat_result(sp) }.to_vec());
        }
        unsafe { _mvl_string_ptr_array_drop(arr) };
        out
    }

    #[test]
    fn split_basic_comma_separated() {
        assert_eq!(
            unsafe { split_parts(b"a,b,c", b",") },
            vec![b"a", b"b", b"c"]
        );
    }

    #[test]
    fn split_separator_absent_yields_whole_subject() {
        assert_eq!(
            unsafe { split_parts(b"hello", b",") },
            vec![b"hello".to_vec()]
        );
    }

    #[test]
    fn split_multichar_separator() {
        assert_eq!(
            unsafe { split_parts(b"one::two::three", b"::") },
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn split_adjacent_separators_yield_empty_parts() {
        assert_eq!(
            unsafe { split_parts(b"a,,b", b",") },
            vec![b"a".to_vec(), b"".to_vec(), b"b".to_vec()]
        );
    }

    #[test]
    fn split_leading_and_trailing_separator_yield_empty_edges() {
        assert_eq!(
            unsafe { split_parts(b",a,", b",") },
            vec![b"".to_vec(), b"a".to_vec(), b"".to_vec()]
        );
    }

    // The three cases the doc comment promises match `runtime/llvm/`'s
    // `str::split` rather than a hand-rolled byte scan. Pinned here because a
    // future "simplify this loop" would silently break rust/wasm ↔ rust/rust
    // corpus parity, not any test in this file.
    #[test]
    fn split_empty_subject_yields_one_empty_part() {
        assert_eq!(unsafe { split_parts(b"", b",") }, vec![b"".to_vec()]);
    }

    #[test]
    fn split_empty_separator_matches_rust_str_split() {
        // Rust yields a leading and trailing "" around one part per char.
        assert_eq!(
            unsafe { split_parts(b"ab", b"") },
            vec![b"".to_vec(), b"a".to_vec(), b"b".to_vec(), b"".to_vec()]
        );
    }

    #[test]
    fn split_separator_longer_than_subject_yields_whole_subject() {
        assert_eq!(unsafe { split_parts(b"ab", b"abcd") }, vec![b"ab".to_vec()]);
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

    #[test]
    fn array_typed_push_i64() {
        let a = _mvl_array_new(8, 4);
        unsafe {
            _mvl_array_push_i64(a, 100);
            _mvl_array_push_i64(a, 200);
            _mvl_array_push_i64(a, 300);
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 3);
        assert_eq!(unsafe { get_i64(a, 0) }, 100);
        assert_eq!(unsafe { get_i64(a, 2) }, 300);
        unsafe { _mvl_array_drop(a) };
    }

    // ── slice (#2014) ────
    //
    // Backs `List[T]::take` (`slice(0, n)`) and `::skip`
    // (`slice(n, self.len())`), so the clamping cases below are the ones those
    // wrappers actually hit at the ends of a list.
    unsafe fn i64_array(vals: &[i64]) -> i32 {
        let a = _mvl_array_new(8, vals.len() as i32);
        for v in vals {
            unsafe { _mvl_array_push_i64(a, *v) };
        }
        a
    }

    unsafe fn slice_vals(src: &[i64], start: i64, end: i64) -> Vec<i64> {
        let a = unsafe { i64_array(src) };
        let s = unsafe { _mvl_array_slice(a, start, end) };
        let n = unsafe { _mvl_array_len(s) };
        let out = (0..n).map(|i| unsafe { get_i64(s, i) }).collect();
        unsafe { _mvl_array_drop(s) };
        unsafe { _mvl_array_drop(a) };
        out
    }

    #[test]
    fn slice_middle_range() {
        assert_eq!(unsafe { slice_vals(&[1, 2, 3, 4], 1, 3) }, vec![2, 3]);
    }

    #[test]
    fn slice_take_prefix() {
        // `take(3)`
        assert_eq!(
            unsafe { slice_vals(&[10, 20, 30, 40], 0, 3) },
            vec![10, 20, 30]
        );
    }

    #[test]
    fn slice_skip_suffix() {
        // `skip(2)` — end is the full length.
        assert_eq!(unsafe { slice_vals(&[10, 20, 30, 40], 2, 4) }, vec![30, 40]);
    }

    #[test]
    fn slice_clamps_end_past_len() {
        // `take(99)` on a 2-element list yields the whole list, not garbage.
        assert_eq!(unsafe { slice_vals(&[7, 8], 0, 99) }, vec![7, 8]);
    }

    #[test]
    fn slice_clamps_negative_start() {
        assert_eq!(unsafe { slice_vals(&[7, 8], -5, 1) }, vec![7]);
    }

    #[test]
    fn slice_reversed_range_is_empty() {
        assert_eq!(unsafe { slice_vals(&[1, 2, 3], 2, 1) }, Vec::<i64>::new());
    }

    #[test]
    fn slice_start_past_len_is_empty() {
        // `skip(n)` where n >= len — the empty tail.
        assert_eq!(unsafe { slice_vals(&[1, 2], 5, 9) }, Vec::<i64>::new());
    }

    #[test]
    fn slice_of_empty_is_empty() {
        assert_eq!(unsafe { slice_vals(&[], 0, 3) }, Vec::<i64>::new());
    }

    #[test]
    fn slice_null_array_yields_empty() {
        let s = unsafe { _mvl_array_slice(0, 0, 3) };
        // Null in, null out — not a fabricated array with a guessed elem_size.
        assert_eq!(s, 0, "null receiver must not allocate");
        assert_eq!(unsafe { _mvl_array_len(s) }, 0);
        unsafe { _mvl_array_drop(s) };
    }

    /// Cross-backend parity guard, matching the three `_mvl_string_split` ones.
    ///
    /// `_mvl_array_slice` is a port of `runtime/llvm`'s `_mvl_list_slice`, whose
    /// clamping is `start.max(0).min(len)` / `end.max(0).min(len)` with
    /// `hi.saturating_sub(lo)`. A corpus file that slices under both backends
    /// compares *results*, so a divergence here surfaces as a rust/wasm ↔
    /// rust/rust parity failure rather than a unit-test failure — pin the
    /// agreement directly instead.
    #[test]
    fn slice_clamping_matches_llvm_list_slice() {
        // (start, end, expected) over [10, 20, 30, 40].
        let cases: &[(i64, i64, &[i64])] = &[
            (0, 4, &[10, 20, 30, 40]),
            (1, 3, &[20, 30]),
            (0, 99, &[10, 20, 30, 40]), // end past len clamps to len
            (10, 20, &[]),              // start past len is empty
            (3, 1, &[]),                // reversed range is empty
            (-5, 2, &[10, 20]),         // negative start clamps to 0
            (2, 2, &[]),                // zero-length window
        ];
        for (start, end, expect) in cases {
            let a = unsafe { i64_array(&[10, 20, 30, 40]) };
            let s = unsafe { _mvl_array_slice(a, *start, *end) };
            let got: Vec<i64> = (0..unsafe { _mvl_array_len(s) })
                .map(|i| unsafe { get_i64(s, i) })
                .collect();
            assert_eq!(
                got, *expect,
                "slice({start}, {end}) diverges from _mvl_list_slice"
            );
            unsafe { _mvl_array_drop(s) };
            unsafe { _mvl_array_drop(a) };
        }
    }

    /// The slice must be an independent buffer — mutating the source afterwards
    /// must not change it.
    #[test]
    fn slice_does_not_alias_source_buffer() {
        let a = unsafe { i64_array(&[1, 2, 3]) };
        let s = unsafe { _mvl_array_slice(a, 0, 2) };
        unsafe { _mvl_array_push_i64(a, 99) };
        assert_eq!(unsafe { _mvl_array_len(s) }, 2);
        assert_eq!(unsafe { get_i64(s, 0) }, 1);
        assert_eq!(unsafe { get_i64(s, 1) }, 2);
        unsafe { _mvl_array_drop(s) };
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn slice_preserves_i32_elem_size() {
        let a = _mvl_array_new(4, 4);
        unsafe {
            _mvl_array_push_i32(a, 11);
            _mvl_array_push_i32(a, 22);
            _mvl_array_push_i32(a, 33);
        }
        let s = unsafe { _mvl_array_slice(a, 1, 3) };
        assert_eq!(unsafe { _mvl_array_len(s) }, 2);
        let p = unsafe { _mvl_array_get(s, 0) };
        assert_eq!(unsafe { *(p as usize as *const i32) }, 22);
        unsafe { _mvl_array_drop(s) };
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_typed_push_i32() {
        let a = _mvl_array_new(4, 4);
        unsafe {
            _mvl_array_push_i32(a, 1);
            _mvl_array_push_i32(a, 0);
            _mvl_array_push_i32(a, 1);
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 3);
        let p = unsafe { _mvl_array_get(a, 1) };
        let v: i32 = unsafe { *(p as usize as *const i32) };
        assert_eq!(v, 0);
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_typed_push_f64() {
        let a = _mvl_array_new(8, 4);
        unsafe {
            _mvl_array_push_f64(a, 3.14);
            _mvl_array_push_f64(a, -0.5);
        }
        assert_eq!(unsafe { _mvl_array_len(a) }, 2);
        let p = unsafe { _mvl_array_get(a, 0) };
        let v: f64 = unsafe { *(p as usize as *const f64) };
        assert!((v - 3.14).abs() < 1e-9);
        unsafe { _mvl_array_drop(a) };
    }

    // ── MvlOption ────

    #[test]
    fn option_none_tag() {
        let n = _mvl_option_none();
        assert_eq!(unsafe { _mvl_option_tag(n) }, 1);
        unsafe { _mvl_option_drop(n) };
    }

    #[test]
    fn option_some_i64_roundtrip() {
        let s = _mvl_option_some_i64(42);
        assert_eq!(unsafe { _mvl_option_tag(s) }, 0);
        assert_eq!(unsafe { _mvl_option_value_i64(s) }, 42);
        unsafe { _mvl_option_drop(s) };
    }

    #[test]
    fn option_some_i32_roundtrip() {
        let s = _mvl_option_some_i32(1);
        assert_eq!(unsafe { _mvl_option_tag(s) }, 0);
        assert_eq!(unsafe { _mvl_option_value_i32(s) }, 1);
        unsafe { _mvl_option_drop(s) };
    }

    #[test]
    fn option_some_negative_i64() {
        let s = _mvl_option_some_i64(-123);
        assert_eq!(unsafe { _mvl_option_value_i64(s) }, -123);
        unsafe { _mvl_option_drop(s) };
    }

    #[test]
    fn option_null_ptr_treated_as_none() {
        // `_mvl_option_tag(0)` returns 1 (None) — mirrors MvlString's
        // null-safe accessors so an accidental null read doesn't UB.
        assert_eq!(unsafe { _mvl_option_tag(0) }, 1);
        assert_eq!(unsafe { _mvl_option_value_i64(0) }, 0);
    }

    // ── _mvl_array_get_option ────

    #[test]
    fn array_get_option_i64_in_bounds() {
        let a = _mvl_array_new(8, 4);
        unsafe {
            _mvl_array_push_i64(a, 100);
            _mvl_array_push_i64(a, 200);
        }
        let opt = unsafe { _mvl_array_get_option_i64(a, 1) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 0);
        assert_eq!(unsafe { _mvl_option_value_i64(opt) }, 200);
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_get_option_i64_out_of_bounds() {
        let a = _mvl_array_new(8, 4);
        unsafe { _mvl_array_push_i64(a, 42) };
        let opt = unsafe { _mvl_array_get_option_i64(a, 99) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 1, "OOB should be None");
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_array_drop(a) };
    }

    #[test]
    fn array_get_option_i32_in_bounds() {
        let a = _mvl_array_new(4, 4);
        unsafe {
            _mvl_array_push_i32(a, 1);
            _mvl_array_push_i32(a, 0);
        }
        let opt = unsafe { _mvl_array_get_option_i32(a, 0) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 0);
        assert_eq!(unsafe { _mvl_option_value_i32(opt) }, 1);
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_array_drop(a) };
    }

    // ── MvlMap ────

    #[test]
    fn map_get_si64_missing_key_is_none() {
        let m = _mvl_map_new_si64();
        let k = b"missing";
        let opt = unsafe { _mvl_map_get_si64(m, addr(k), k.len() as i32) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 1);
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_map_drop_si64(m) };
    }

    #[test]
    fn map_get_si64_roundtrips_int_value() {
        let m = _mvl_map_new_si64();
        let k = b"count";
        unsafe { _mvl_map_insert_si64(m, addr(k), k.len() as i32, 7) };
        let opt = unsafe { _mvl_map_get_si64(m, addr(k), k.len() as i32) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 0);
        assert_eq!(unsafe { _mvl_option_value_i64(opt) }, 7);
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_map_drop_si64(m) };
    }

    // `_mvl_map_get_str` must bump the stored `*MvlString` handle's refcount
    // before handing it back (#2047) — otherwise dropping the "unwrapped"
    // string (as `unwrap_or`'s codegen always does) frees memory the map's
    // entry still points at, and the map's own later drop double-frees it.
    // Reproduce the exact sequence the WASM emitter generates: get -> drop
    // the unwrapped string -> drop the map (which drops its own reference).
    // `MvlString.rc` starts at 1 from `_mvl_string_new`; a bare (unfixed)
    // `_mvl_map_get_si64`-style getter would hand back that same rc=1
    // handle, and the first drop below would free it out from under the map.
    #[test]
    fn map_get_str_clones_so_caller_can_drop_independently() {
        let m = _mvl_map_new_si64();
        let k = b"status";
        let v = b"ready";
        let boxed = unsafe { _mvl_string_new(addr(v), v.len() as i32) };
        unsafe { _mvl_map_insert_si64(m, addr(k), k.len() as i32, boxed as i64) };

        let opt = unsafe { _mvl_map_get_str(m, addr(k), k.len() as i32) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 0);
        let handle = unsafe { _mvl_option_value_i32(opt) };
        assert_eq!(
            handle, boxed,
            "same MvlString handle — cloning bumps rc, not the pointer"
        );
        unsafe { _mvl_option_drop(opt) };

        // Mirrors `unwrap_or`'s cleanup: drop the value extracted from the option.
        // If `_mvl_map_get_str` hadn't bumped rc, this would free `boxed`
        // while the map's entry still references it.
        unsafe { _mvl_string_drop(handle) };

        // The map's own entry must still be valid — dropping the whole map
        // must not double-free.
        unsafe { _mvl_map_drop_si64(m) };
    }

    #[test]
    fn map_get_str_missing_key_is_none() {
        let m = _mvl_map_new_si64();
        let k = b"missing";
        let opt = unsafe { _mvl_map_get_str(m, addr(k), k.len() as i32) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 1);
        unsafe { _mvl_option_drop(opt) };
        unsafe { _mvl_map_drop_si64(m) };
    }

    #[test]
    fn map_contains_key_and_len() {
        let m = _mvl_map_new_si64();
        let k = b"a";
        assert_eq!(
            unsafe { _mvl_map_contains_key_si64(m, addr(k), k.len() as i32) },
            0
        );
        unsafe { _mvl_map_insert_si64(m, addr(k), k.len() as i32, 1) };
        assert_eq!(
            unsafe { _mvl_map_contains_key_si64(m, addr(k), k.len() as i32) },
            1
        );
        assert_eq!(unsafe { _mvl_map_len(m) }, 1);
        unsafe { _mvl_map_drop_si64(m) };
    }

    // ── std.env tests ────────────────────────────────────────────────────────

    #[test]
    fn env_getuid_returns_zero_on_wasi() {
        // WASI doesn't expose Unix UIDs, so we return 0
        assert_eq!(_mvl_env_getuid(), 0);
    }

    #[test]
    fn env_getgid_returns_zero_on_wasi() {
        // WASI doesn't expose Unix GIDs, so we return 0
        assert_eq!(_mvl_env_getgid(), 0);
    }

    #[test]
    fn env_get_missing_var_is_none() {
        let name = b"__MVL_TEST_NONEXISTENT_VAR_12345__";
        let opt = unsafe { _mvl_env_get(addr(name), name.len() as i32) };
        assert_eq!(unsafe { _mvl_option_tag(opt) }, 1); // None
        unsafe { _mvl_option_drop(opt) };
    }

    #[test]
    fn env_current_dir_returns_ok() {
        let result = _mvl_env_current_dir();
        // Should be Ok (tag 0), not Err
        assert_eq!(unsafe { _mvl_result_tag(result) }, 0);
        unsafe { _mvl_result_drop(result) };
    }

    // ── std.time tests ───────────────────────────────────────────────────────

    #[test]
    fn time_now_returns_valid_handle() {
        let handle = _mvl_time_now();
        assert!(handle != 0);
        let secs = unsafe { _mvl_time_instant_epoch_seconds(handle) };
        assert!(secs > 1_700_000_000);
    }

    // ── std.random tests ─────────────────────────────────────────────────────

    #[test]
    fn random_int_in_range() {
        for _ in 0..100 {
            let v = _mvl_random_int(10, 20);
            assert!(v >= 10 && v <= 20);
        }
    }

    #[test]
    fn random_int_min_equals_max() {
        let v = _mvl_random_int(42, 42);
        assert_eq!(v, 42);
    }

    #[test]
    fn random_int_min_greater_than_max_returns_min() {
        let v = _mvl_random_int(100, 50);
        assert_eq!(v, 100);
    }

    #[test]
    fn random_float_in_zero_one() {
        for _ in 0..100 {
            let v = _mvl_random_float();
            assert!(v >= 0.0 && v < 1.0);
        }
    }

    #[test]
    fn random_bytes_returns_correct_length() {
        let arr = _mvl_random_bytes(10);
        assert_eq!(unsafe { _mvl_array_len(arr) }, 10);
        // Check values are in byte range [0, 255]
        for i in 0..10 {
            let ptr = unsafe { _mvl_array_get(arr, i) };
            let val = unsafe { *(ptr as usize as *const i64) };
            assert!(val >= 0 && val <= 255);
        }
        unsafe { _mvl_array_drop(arr) };
    }

    #[test]
    fn random_bytes_zero_returns_empty() {
        let arr = _mvl_random_bytes(0);
        assert_eq!(unsafe { _mvl_array_len(arr) }, 0);
        unsafe { _mvl_array_drop(arr) };
    }

    #[test]
    fn random_choice_index_empty_returns_negative() {
        let arr = _mvl_array_new(8, 0);
        let idx = unsafe { _mvl_random_choice_index(arr) };
        assert_eq!(idx, -1);
        unsafe { _mvl_array_drop(arr) };
    }

    #[test]
    fn random_choice_index_single_element() {
        let arr = _mvl_array_new(8, 1);
        unsafe { _mvl_array_push_i64(arr, 999) };
        let idx = unsafe { _mvl_random_choice_index(arr) };
        assert_eq!(idx, 0);
        unsafe { _mvl_array_drop(arr) };
    }

    #[test]
    fn random_shuffle_preserves_length() {
        let arr = _mvl_array_new(8, 5);
        for i in 0..5 {
            unsafe { _mvl_array_push_i64(arr, i) };
        }
        let shuffled = unsafe { _mvl_random_shuffle(arr) };
        assert_eq!(unsafe { _mvl_array_len(shuffled) }, 5);
        unsafe { _mvl_array_drop(arr) };
        unsafe { _mvl_array_drop(shuffled) };
    }

    #[test]
    fn random_shuffle_returns_new_array() {
        let arr = _mvl_array_new(8, 3);
        for i in 0..3 {
            unsafe { _mvl_array_push_i64(arr, i) };
        }
        let shuffled = unsafe { _mvl_random_shuffle(arr) };
        // Should be a different pointer (new array)
        assert!(arr != shuffled);
        unsafe { _mvl_array_drop(arr) };
        unsafe { _mvl_array_drop(shuffled) };
    }

    // ── std.io tests ─────────────────────────────────────────────────────────

    #[test]
    fn io_exists_nonexistent_returns_false() {
        let path = b"/nonexistent/path/that/should/not/exist";
        let exists = unsafe { _mvl_io_exists(addr(path), path.len() as i32) };
        assert_eq!(exists, 0);
    }

    #[test]
    fn io_is_file_nonexistent_returns_false() {
        let path = b"/nonexistent/path/that/should/not/exist";
        let is_file = unsafe { _mvl_io_is_file(addr(path), path.len() as i32) };
        assert_eq!(is_file, 0);
    }

    #[test]
    fn io_is_dir_nonexistent_returns_false() {
        let path = b"/nonexistent/path/that/should/not/exist";
        let is_dir = unsafe { _mvl_io_is_dir(addr(path), path.len() as i32) };
        assert_eq!(is_dir, 0);
    }
}
