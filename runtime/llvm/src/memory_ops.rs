// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Heap-collection operations for the MVL LLVM backend.
//!
//! This module provides the `extern "C"` operation functions for `MvlString`,
//! `MvlArray`, and `MvlMap`.
//!
//! # Architecture (ADR-0016, #490)
//!
//! `memory` is responsible for **type definitions + lifecycle** (new/clone/drop).
//! This module is responsible for **operations** (len, ptr, concat, get, push, insert, …).
//!
//! Both sets of symbols are exported from `libmvl_runtime_llvm.{dylib,so}`.

use std::ptr;

use crate::memory::{
    _mvl_alloc, _mvl_array_clone, _mvl_array_new, _mvl_free, _mvl_string_clone, _mvl_string_drop,
    _mvl_string_new, MvlArray, MvlMap, MvlMapSlot, MvlString,
};

// ── format (#901) ───────────────────────────────────────────────────────────

/// `mvl_format(template, values)` — positional `{}` interpolation.
///
/// `template` is an `MvlString*` containing `{}` placeholders.
/// `values` is an `MvlArray*` of `MvlString*` pointers (elem_size = 8).
/// Returns a new `MvlString*` with placeholders replaced by values in order.
///
/// # Safety
/// Both pointers must be valid. `values` must contain `*mut MvlString` elements.
#[no_mangle]
pub unsafe extern "C" fn _mvl_format(
    template: *const MvlString,
    values: *const MvlArray,
) -> *mut MvlString {
    let tmpl_len = if template.is_null() {
        0
    } else {
        (*template).len as usize
    };
    let tmpl_ptr = if template.is_null() || tmpl_len == 0 {
        b"".as_ptr()
    } else {
        (*template).ptr as *const u8
    };
    let tmpl = std::slice::from_raw_parts(tmpl_ptr, tmpl_len);

    let val_count = if values.is_null() {
        0
    } else {
        (*values).len as usize
    };

    let mut result = Vec::with_capacity(tmpl_len);
    let mut val_idx: usize = 0;
    let mut i = 0;
    while i < tmpl_len {
        if tmpl[i] == b'{' && i + 1 < tmpl_len && tmpl[i + 1] == b'}' {
            // Replace {} with next value
            if val_idx < val_count {
                let elem_ptr = _mvl_array_get(values, val_idx as i64) as *const *mut MvlString;
                let s = *elem_ptr;
                if !s.is_null() {
                    let s_len = (*s).len as usize;
                    let s_ptr = (*s).ptr as *const u8;
                    result.extend_from_slice(std::slice::from_raw_parts(s_ptr, s_len));
                }
                val_idx += 1;
            }
            i += 2;
        } else {
            result.push(tmpl[i]);
            i += 1;
        }
    }

    _mvl_string_new(result.as_ptr(), result.len())
}

// ── String helper (shared by string primitives) ────────────────────────────────

/// Borrow the bytes of a `MvlString` as a Rust `str`.  Returns `""` for null/empty.
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.  The returned `str` is only
/// valid while `s` is alive.
#[inline(always)]
unsafe fn as_str<'a>(s: *const MvlString) -> &'a str {
    if s.is_null() {
        return "";
    }
    let len = (*s).len as usize;
    if len == 0 || (*s).ptr.is_null() {
        return "";
    }
    let bytes = std::slice::from_raw_parts((*s).ptr, len);
    std::str::from_utf8(bytes).unwrap_or("")
}

/// Allocate a new `MvlString` from a Rust `&str`.
#[inline(always)]
unsafe fn str_to_mvl(s: &str) -> *mut MvlString {
    _mvl_string_new(s.as_ptr(), s.len())
}

// ── Internal helpers ───────────────────────────────────────────────────────────

#[inline(always)]
fn checked_mul_size(a: usize, b: usize) -> usize {
    a.checked_mul(b).unwrap_or_else(|| std::process::abort())
}

/// Growth cap used in `_mvl_array_push` to mirror `crate::memory::ARRAY_INITIAL_CAP`.
const ARRAY_INITIAL_CAP: usize = 4;

/// Minimum slot count for map growth to mirror `crate::memory::MAP_INITIAL_CAP`.
const MAP_INITIAL_CAP: usize = 8;

/// Byte size of a single `MvlMapSlot`.
const SLOT_SIZE: usize = std::mem::size_of::<MvlMapSlot>();

// ── FNV-1a hash (for MvlMap) ──────────────────────────────────────────────────

unsafe fn fnv1a(key: *const u8, len: usize) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for i in 0..len {
        hash ^= *key.add(i) as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Probe for the slot matching `key` (or the first empty slot if absent).
///
/// Slot states: 0 = empty, 1 = live, 2 = tombstone (deleted).
/// Tombstones are skipped during lookup so that collision chains remain intact
/// after removal. The first empty slot (occupied == 0) terminates the probe.
unsafe fn map_find_slot(slots: *mut MvlMapSlot, cap: u64, key: *const u8, key_len: usize) -> usize {
    let h = fnv1a(key, key_len);
    let mut idx = (h % cap) as usize;
    loop {
        let slot = &*slots.add(idx);
        if slot.occupied == 0 {
            return idx; // empty — insertion point / not-found sentinel
        }
        if slot.occupied == 2 {
            idx = (idx + 1) % cap as usize;
            continue; // tombstone — keep probing
        }
        // occupied == 1: live entry
        if slot.key_len == key_len as u64
            && libc::memcmp(slot.key_ptr.cast(), key.cast(), key_len) == 0
        {
            return idx; // found
        }
        idx = (idx + 1) % cap as usize;
    }
}

// ── MvlString operations ───────────────────────────────────────────────────────

/// Return the byte length of the string.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_len(s: *const MvlString) -> i64 {
    if s.is_null() {
        return 0;
    }
    (*s).len as i64
}

/// Return a null-terminated `char*` pointer into the string's data (for printf).
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
/// The returned pointer is only valid while `s` is alive.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_ptr(s: *const MvlString) -> *const u8 {
    if s.is_null() {
        return b"\0".as_ptr();
    }
    (*s).ptr
}

/// Concatenate two strings and return a new `MvlString` with `refcount = 1`.
/// Does not consume `a` or `b`; the caller still owns them.
///
/// # Safety
/// `a` and `b` must be valid non-null `MvlString` pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_concat(
    a: *const MvlString,
    b: *const MvlString,
) -> *mut MvlString {
    let a_bytes = if a.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts((*a).ptr, (*a).len as usize)
    };
    let b_bytes = if b.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts((*b).ptr, (*b).len as usize)
    };
    let mut merged = mvl_runtime_core::concat_bytes(a_bytes, b_bytes);
    merged.push(0); // null terminator
    let total = merged.len() - 1;
    let cap = merged.len();
    let data = _mvl_alloc(cap);
    ptr::copy_nonoverlapping(merged.as_ptr(), data, cap);
    let s = _mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
    s.write(MvlString {
        ptr: data,
        len: total as u64,
        cap: cap as u64,
        refcount: 1,
    });
    s
}

/// Return 1 if the two strings are byte-equal, 0 otherwise.
///
/// # Safety
/// `a` and `b` must be valid non-null `MvlString` pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_eq(a: *const MvlString, b: *const MvlString) -> i32 {
    if a == b {
        return 1;
    }
    if a.is_null() || b.is_null() {
        return 0;
    }
    if (*a).len != (*b).len {
        return 0;
    }
    let len = (*a).len as usize;
    if len == 0 {
        return 1;
    }
    let eq = libc::memcmp((*a).ptr.cast(), (*b).ptr.cast(), len) == 0;
    if eq {
        1
    } else {
        0
    }
}

/// `_mvl_string_cmp(a, b) -> i32` — lexicographic ordering: -1 if `a < b`,
/// 0 if equal, 1 if `a > b` (#2260, found via `List[String]::sort_by`).
///
/// `emit_binary_tir` had a `String` case for `==`/`!=` (via
/// `_mvl_string_eq` above) but none for `<`/`>`/`<=`/`>=` — those fell to
/// the generic pointer-comparison path and compiled `a < b` to a raw
/// `icmp slt ptr %a, %b`, comparing the two `*MvlString` *handle addresses*
/// rather than content. Not merely wrong: since handle addresses depend on
/// allocator/ASLR placement, the "sort order" it produced was
/// non-deterministic across runs of the same compiled program.
///
/// # Safety
/// `a`/`b` must each be a valid `MvlString` pointer or null (treated as
/// empty).
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_cmp(a: *const MvlString, b: *const MvlString) -> i32 {
    let sa = as_str(a);
    let sb = as_str(b);
    match sa.cmp(sb) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Return a new `MvlString` with all ASCII bytes converted to lowercase.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_to_lower(s: *const MvlString) -> *mut MvlString {
    let len = (*s).len as usize;
    let cap = len + 1;
    let data = _mvl_alloc(cap);
    for i in 0..len {
        *data.add(i) = (*(*s).ptr.add(i) as char).to_ascii_lowercase() as u8;
    }
    *data.add(len) = 0;
    let out = _mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
    out.write(MvlString {
        ptr: data,
        len: len as u64,
        cap: cap as u64,
        refcount: 1,
    });
    out
}

/// Return a new `MvlString` with all ASCII bytes converted to uppercase.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_to_upper(s: *const MvlString) -> *mut MvlString {
    let len = (*s).len as usize;
    let cap = len + 1;
    let data = _mvl_alloc(cap);
    for i in 0..len {
        *data.add(i) = (*(*s).ptr.add(i) as char).to_ascii_uppercase() as u8;
    }
    *data.add(len) = 0;
    let out = _mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
    out.write(MvlString {
        ptr: data,
        len: len as u64,
        cap: cap as u64,
        refcount: 1,
    });
    out
}

/// Return the Unicode scalar-value count of the string (char count, not byte count).
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_len(s: *const MvlString) -> i64 {
    as_str(s).chars().count() as i64
}

/// Return a new `MvlString` with leading and trailing ASCII whitespace removed.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_trim(s: *const MvlString) -> *mut MvlString {
    str_to_mvl(as_str(s).trim())
}

/// Return 1 if `s` starts with `prefix`, 0 otherwise.
///
/// # Safety
/// Both pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_starts_with(
    s: *const MvlString,
    prefix: *const MvlString,
) -> i64 {
    as_str(s).starts_with(as_str(prefix)) as i64
}

/// Return 1 if `s` ends with `suffix`, 0 otherwise.
///
/// # Safety
/// Both pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_ends_with(s: *const MvlString, suffix: *const MvlString) -> i64 {
    as_str(s).ends_with(as_str(suffix)) as i64
}

/// Return 1 if `s` contains `sub`, 0 otherwise.
///
/// # Safety
/// Both pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_contains(s: *const MvlString, sub: *const MvlString) -> i64 {
    as_str(s).contains(as_str(sub)) as i64
}

/// Return the char-index of the first occurrence of `sub` in `s`, or -1 if not found.
///
/// # Safety
/// Both pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_find(s: *const MvlString, sub: *const MvlString) -> i64 {
    let haystack = as_str(s);
    let needle = as_str(sub);
    if needle.is_empty() {
        return 0;
    }
    match haystack.find(needle) {
        Some(byte_idx) => haystack[..byte_idx].chars().count() as i64,
        None => -1,
    }
}

/// Replace all occurrences of `from` with `to` in `s`, returning a new `MvlString`.
///
/// # Safety
/// All pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_replace(
    s: *const MvlString,
    from: *const MvlString,
    to: *const MvlString,
) -> *mut MvlString {
    let result = as_str(s).replace(as_str(from), as_str(to));
    str_to_mvl(&result)
}

/// Split `s` on `sep`, returning a `MvlArray*` of `*mut MvlString` elements.
///
/// The returned array owns its element strings; use `mvl_string_ptr_array_drop`
/// to free.
///
/// # Safety
/// Both pointers must be valid `MvlString` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_split(
    s: *const MvlString,
    sep: *const MvlString,
) -> *mut MvlArray {
    let arr = _mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    let text = as_str(s);
    let delimiter = as_str(sep);
    for part in text.split(delimiter) {
        let part_s = str_to_mvl(part);
        _mvl_array_push(arr, (&part_s as *const *mut MvlString).cast());
    }
    arr
}

/// Return the char-indexed substring `s[start..end]` (safe clamping).
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_substring(
    s: *const MvlString,
    start: i64,
    end: i64,
) -> *mut MvlString {
    let text = as_str(s);
    let char_count = text.chars().count() as i64;
    let lo = start.max(0).min(char_count) as usize;
    let hi = end.max(0).min(char_count) as usize;
    let result: String = text.chars().skip(lo).take(hi.saturating_sub(lo)).collect();
    str_to_mvl(&result)
}

/// Return a one-character `MvlString` at char-index `i`, or None if out of range.
///
/// Returns tag=0 (Some) and writes `*out = MvlString*`, or tag=1 (None).
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
/// `out` must be a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_char_at(
    s: *const MvlString,
    i: i64,
    out: *mut *mut MvlString,
) -> i8 {
    let text = as_str(s);
    if i < 0 {
        return 1; // None
    }
    match text.chars().nth(i as usize) {
        Some(ch) => {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            *out = str_to_mvl(encoded);
            0 // Some
        }
        None => 1, // None
    }
}

/// Backwards-compatible sentinel version for internal use.
/// Returns `""` if out of range. Used by stdlib callers that check bounds first.
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_char_at_or(
    s: *const MvlString,
    i: i64,
    default: *mut MvlString,
) -> *mut MvlString {
    let text = as_str(s);
    if i < 0 {
        return default;
    }
    match text.chars().nth(i as usize) {
        Some(ch) => {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            str_to_mvl(encoded)
        }
        None => default,
    }
}

/// Reconstruct a `MvlString` from a `MvlArray*` of `*mut MvlString` char elements.
///
/// The input array is as produced by `mvl_string_chars`: each element is a
/// `*mut MvlString` pointer (one Unicode scalar value per element).
///
/// # Safety
/// `arr` must be a valid `MvlArray*` or null.  Each element must be a valid
/// `*mut MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_from_chars(arr: *const MvlArray) -> *mut MvlString {
    if arr.is_null() {
        return str_to_mvl("");
    }
    let len = (*arr).len as usize;
    let mut result = String::new();
    let es = (*arr).elem_size as usize;
    for i in 0..len {
        let elem_ptr = (*arr).ptr.add(i * es) as *const *const MvlString;
        let cs = *elem_ptr;
        if !cs.is_null() {
            result.push_str(as_str(cs));
        }
    }
    str_to_mvl(&result)
}

/// Return the byte at char-index `i`, or None if out of range or codepoint > 255.
///
/// Returns tag=0 (Some) and writes `*out = byte_value`, or tag=1 (None).
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
/// `out` must be a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_byte_at(s: *const MvlString, i: i64, out: *mut i64) -> i8 {
    let text = as_str(s);
    if i < 0 {
        return 1; // None
    }
    match text.chars().nth(i as usize) {
        Some(c) => {
            let cp = c as u32;
            if cp <= 255 {
                *out = cp as i64;
                0 // Some
            } else {
                1 // None
            }
        }
        None => 1, // None
    }
}

/// Reconstruct a `MvlString` from a `MvlArray*` of `Byte` values (Latin-1).
///
/// Each element maps to the Unicode codepoint of the same numeric value,
/// giving a lossless round-trip with `_mvl_str_byte_at` for every byte
/// 0..=255. `Byte` is laid out as `i8` (`elem_size == 1`, see
/// `emit_helpers.rs::scalar_leaf`), so elements are read as raw bytes, not
/// cast to a wider integer — a `List[Byte]`'s backing storage is only
/// 1-byte-aligned per element, and reading it as `*const i64` previously
/// crashed with a misaligned pointer dereference (#2123) the moment `len >
/// 1` pushed an element off an 8-byte boundary.
///
/// # Safety
/// `arr` must be a valid `MvlArray*` or null, with `elem_size == 1`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_from_bytes(arr: *const MvlArray) -> *mut MvlString {
    if arr.is_null() {
        return str_to_mvl("");
    }
    let len = (*arr).len as usize;
    let es = (*arr).elem_size as usize;
    let mut s = String::with_capacity(len);
    for i in 0..len {
        let elem_ptr = (*arr).ptr.add(i * es);
        s.push(*elem_ptr as char);
    }
    str_to_mvl(&s)
}

// ── MvlArray operations ────────────────────────────────────────────────────────

/// Append one element of `elem_size` bytes to the array, growing 2× if needed.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
/// `elem` must point to at least `(*a).elem_size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_push(a: *mut MvlArray, elem: *const u8) {
    if a.is_null() || elem.is_null() {
        return;
    }
    let es = (*a).elem_size as usize;
    if (*a).len >= (*a).cap {
        // Grow 2×
        let old_cap = (*a).cap as usize;
        let new_cap = checked_mul_size(old_cap, 2).max(ARRAY_INITIAL_CAP);
        let new_data = _mvl_alloc(checked_mul_size(new_cap, es));
        if old_cap > 0 && !(*a).ptr.is_null() {
            let old_bytes = checked_mul_size(old_cap, es);
            ptr::copy_nonoverlapping((*a).ptr, new_data, old_bytes);
            _mvl_free((*a).ptr, old_bytes);
        }
        (*a).ptr = new_data;
        (*a).cap = new_cap as u64;
    }
    let dest = (*a).ptr.add((*a).len as usize * es);
    ptr::copy_nonoverlapping(elem, dest, es);
    (*a).len += 1;
}

/// `List[T].extend(other)` (#2264) for scalar element types (Int/Byte/Bool/
/// Float) — append every element of `other` onto `self` in place. A raw
/// byte-level append is a correct, fully independent copy for scalar
/// elements (no shared ownership to worry about), unlike pointer-typed
/// elements (String, nested List) which need [`_mvl_array_extend_str`] /
/// [`_mvl_array_extend_nested`] instead.
///
/// # Safety
/// `self_arr` and `other` must be valid `MvlArray*` (same elem_size) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_extend(self_arr: *mut MvlArray, other: *const MvlArray) {
    if self_arr.is_null() || other.is_null() {
        return;
    }
    let es = (*other).elem_size as usize;
    let len = (*other).len as usize;
    for i in 0..len {
        let elem_ptr = (*other).ptr.add(i * es);
        _mvl_array_push(self_arr, elem_ptr);
    }
}

/// `List[String].extend(other)` (#2264) — append every element of `other`
/// onto `self` in place, cloning each string (refcount bump) so `other`
/// remains an independent, valid owner afterward — confirmed against the
/// Rust backend: the checker allows reusing `other` after `.extend()`, and
/// it must still show correct content.
///
/// # Safety
/// `self_arr` and `other` must be valid `MvlArray*` (elem_size == 8, holding
/// `*mut MvlString` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_extend_str(self_arr: *mut MvlArray, other: *const MvlArray) {
    if self_arr.is_null() || other.is_null() {
        return;
    }
    let len = (*other).len as usize;
    for i in 0..len {
        let src = (*other).ptr.add(i * 8) as *const *mut MvlString;
        let cloned = _mvl_string_clone(*src);
        _mvl_array_push(self_arr, (&cloned as *const *mut MvlString).cast());
    }
}

/// `List[List[U]].extend(other)` (#2264), `U` scalar — append every element
/// of `other` onto `self` in place, deep-cloning each nested array so
/// `other` remains an independent, valid owner afterward. Byte-level
/// [`_mvl_array_deep_clone`] is a correct independent copy here because the
/// *nested* array's own elements are scalar (no further pointer chasing
/// needed) — this is NOT safe for `List[List[String]]` or `List[<struct/
/// enum with heap fields>]`, which need a real per-element clone (#2265,
/// not implemented).
///
/// # Safety
/// `self_arr` and `other` must be valid `MvlArray*` (elem_size == 8, holding
/// `*mut MvlArray` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_extend_nested(self_arr: *mut MvlArray, other: *const MvlArray) {
    if self_arr.is_null() || other.is_null() {
        return;
    }
    let len = (*other).len as usize;
    for i in 0..len {
        let src = (*other).ptr.add(i * 8) as *const *mut MvlArray;
        let cloned = crate::memory::_mvl_array_deep_clone(*src);
        _mvl_array_push(self_arr, (&cloned as *const *mut MvlArray).cast());
    }
}

/// `List[T].extend(other)` (#2265) for a payload-enum element type `T`
/// (e.g. `HuffmanTree`) — append every element of `other` onto `self` in
/// place, calling the emitter-generated per-type `clone_fn` (`T`'s own
/// `@_mvl_clone_enum_<T>` trampoline, see `emit_helpers.rs::
/// ensure_enum_clone_fn`) on each element's payload so `other` remains an
/// independent, valid owner afterward — same contract as
/// [`_mvl_array_extend_str`]/[`_mvl_array_extend_nested`], generalized to
/// an arbitrary recursive struct/enum shape the runtime itself has no
/// static knowledge of. Each element is 16 bytes: a 1-byte discriminant at
/// offset 0 and an 8-byte payload pointer at offset 8 (the `{ i8, ptr }`
/// tagged-union layout the LLVM emitter uses for every payload enum) —
/// same slot shape [`_mvl_array_drop_option`]/[`_mvl_array_drop_result`]
/// already read.
///
/// # Safety
/// `self_arr` and `other` must be valid `MvlArray*` (elem_size == 16,
/// holding `{ i8, ptr }` payload-enum elements) or null. `clone_fn` must be
/// a valid, non-null C-ABI function matching `T`'s own
/// `@_mvl_clone_enum_<T>` trampoline signature.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_extend_enum(
    self_arr: *mut MvlArray,
    other: *const MvlArray,
    clone_fn: unsafe extern "C" fn(u8, *mut u8) -> *mut u8,
) {
    if self_arr.is_null() || other.is_null() {
        return;
    }
    let len = (*other).len as usize;
    for i in 0..len {
        let slot = (*other).ptr.add(i * 16);
        let disc = *slot;
        let payload = *(slot.add(8) as *mut *mut u8);
        let new_payload = if payload.is_null() {
            payload
        } else {
            clone_fn(disc, payload)
        };
        let mut new_slot = [0u8; 16];
        new_slot[0] = disc;
        new_slot[8..16].copy_from_slice(&(new_payload as usize).to_ne_bytes());
        _mvl_array_push(self_arr, new_slot.as_ptr());
    }
}

/// Overwrite the element at index `idx` in place.  No-op if out of bounds.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
/// `elem` must point to at least `(*a).elem_size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_set(a: *mut MvlArray, idx: i64, elem: *const u8) {
    if a.is_null() || elem.is_null() || idx < 0 || idx as u64 >= (*a).len {
        return;
    }
    let es = (*a).elem_size as usize;
    let dest = (*a).ptr.add(idx as usize * es);
    ptr::copy_nonoverlapping(elem, dest, es);
}

/// Create a new array of `n` elements all initialised to the value pointed to by `elem`.
///
/// # Safety
/// `elem` must point to at least `elem_size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_filled(
    elem_size: i64,
    n: i64,
    elem: *const u8,
) -> *mut MvlArray {
    let es = elem_size as usize;
    let count = if n > 0 { n as usize } else { 0 };
    let arr = _mvl_array_new(es, count);
    if arr.is_null() || count == 0 || elem.is_null() {
        return arr;
    }
    for i in 0..count {
        let dest = (*arr).ptr.add(i * es);
        ptr::copy_nonoverlapping(elem, dest, es);
    }
    (*arr).len = count as u64;
    arr
}

/// Return a pointer to element at `idx`.  Returns null if out of bounds.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_get(a: *const MvlArray, idx: i64) -> *const u8 {
    if a.is_null() || idx < 0 || idx as u64 >= (*a).len {
        return ptr::null();
    }
    let i = idx as usize;
    (*a).ptr.add(i * (*a).elem_size as usize)
}

/// Return the number of elements in the array.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_len(a: *const MvlArray) -> i64 {
    if a.is_null() {
        return 0;
    }
    (*a).len as i64
}

/// Dedupe an `MvlArray` in place — retain only the first occurrence of each
/// element under byte-equal comparison. Backs `Set[T]` literal construction on
/// the LLVM backend (#1845). O(n²) linear scan; sets in MVL literal form are
/// expected to be small (constant-time authorial content).
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_dedup(a: *mut MvlArray) {
    if a.is_null() {
        return;
    }
    let len = (*a).len as usize;
    let es = (*a).elem_size as usize;
    if len < 2 || es == 0 {
        return;
    }
    let data = (*a).ptr;
    let mut write = 1usize;
    for read in 1..len {
        let candidate = data.add(read * es);
        let mut seen = false;
        for prev in 0..write {
            let existing = data.add(prev * es);
            if std::slice::from_raw_parts(candidate, es) == std::slice::from_raw_parts(existing, es)
            {
                seen = true;
                break;
            }
        }
        if !seen {
            if write != read {
                let dst = data.add(write * es);
                std::ptr::copy_nonoverlapping(candidate, dst, es);
            }
            write += 1;
        }
    }
    (*a).len = write as u64;
}

/// Linear scan for an element in an `MvlArray`. Backs `List[T].contains(x)` on
/// the LLVM backend (#1858). Element comparison uses raw byte-equal via
/// `element_size` from the array header; works for primitive-sized elements
/// (i64, i32, i8, i1) and for pointer-typed elements (String, Map, List)
/// where the caller has already interned into the same allocation.
///
/// The emitter calls this via `alloca` on the needle, so the second parameter
/// is a pointer whose pointee has the same width as the array's element type:
///
/// ```llvm
/// %slot = alloca i64
/// store i64 %needle, ptr %slot
/// %result = call i1 @_mvl_array_contains(ptr %arr, ptr %slot)
/// ```
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer. `needle_ptr` must point to
/// at least `(*a).element_size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_contains(a: *const MvlArray, needle_ptr: *const u8) -> bool {
    if a.is_null() || needle_ptr.is_null() {
        return false;
    }
    let len = (*a).len as usize;
    let es = (*a).elem_size as usize;
    let data = (*a).ptr;
    for i in 0..len {
        let slot = data.add(i * es);
        if std::slice::from_raw_parts(slot, es) == std::slice::from_raw_parts(needle_ptr, es) {
            return true;
        }
    }
    false
}

/// `List[String].contains(x)` (#2256) — linear scan comparing string
/// *content*, not element pointer identity.
///
/// `_mvl_array_contains` compares elements as raw bytes, which for a `*mut
/// MvlString` element is the heap address, not the string's content — two
/// equal-content strings from different allocations would never match. Same
/// bug shape already fixed for `sort` in [`_mvl_list_sort_str`] (#2173).
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer (elem_size == 8, holding
/// `*mut MvlString` elements) or null. `needle` must be a valid `MvlString`
/// pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_contains_str(
    a: *const MvlArray,
    needle: *const MvlString,
) -> bool {
    if a.is_null() {
        return false;
    }
    let len = (*a).len as usize;
    let data = (*a).ptr;
    for i in 0..len {
        let elem = *(data.add(i * 8) as *const *mut MvlString);
        if _mvl_string_eq(elem, needle) != 0 {
            return true;
        }
    }
    false
}

/// Remove the first element equal to `*needle_ptr` by shifting subsequent
/// elements left one slot. No-op if the element is absent. Used for
/// `Set[T].remove(val)` (#2124) — Set elements are unique by construction,
/// so "first" and "only" coincide. Same needle-encoding contract as
/// [`_mvl_array_contains`] (a pointer to `element_size` readable bytes).
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer. `needle_ptr` must point
/// to at least `(*a).elem_size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_remove_value(a: *mut MvlArray, needle_ptr: *const u8) {
    if a.is_null() || needle_ptr.is_null() {
        return;
    }
    let len = (*a).len as usize;
    let es = (*a).elem_size as usize;
    let data = (*a).ptr;
    let needle = std::slice::from_raw_parts(needle_ptr, es);
    for i in 0..len {
        let slot = data.add(i * es);
        if std::slice::from_raw_parts(slot, es) == needle {
            let rest_bytes = (len - i - 1) * es;
            if rest_bytes > 0 {
                ptr::copy(data.add((i + 1) * es), slot, rest_bytes);
            }
            (*a).len -= 1;
            return;
        }
    }
}

/// Return a new `MvlArray` containing elements `[start, end)` from `arr` (safe clamping).
///
/// Correct only for scalar/pointer-identity-safe elements — copies each
/// element's raw bytes without bumping any refcount, so a `List[String]`
/// slice would alias the source array's strings (see
/// [`_mvl_list_slice_str`] for the String-safe variant `emit_list_slice_call`
/// (`emit_helpers.rs`) routes to instead).
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_slice(
    arr: *const MvlArray,
    start: i64,
    end: i64,
) -> *mut MvlArray {
    if arr.is_null() {
        let dummy = _mvl_array_new(8, 0);
        return dummy;
    }
    let es = (*arr).elem_size as usize;
    let len = (*arr).len as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    let count = hi.saturating_sub(lo);
    let out = _mvl_array_new(es, count.max(1));
    for i in lo..hi {
        let src = (*arr).ptr.add(i * es);
        _mvl_array_push(out, src);
    }
    out
}

/// `List[String]::slice(start, end)` (backing `take`/`skip` too) (#2260,
/// found alongside the `sort_by` String-ordering fix in the same
/// investigation). [`_mvl_list_slice`] byte-copies each element's raw
/// bytes, correct for scalars but aliasing for a `*MvlString` handle — the
/// source array and the slice would then both independently drop (and
/// free) the same strings. Refcount-clones each element instead, same
/// relationship [`_mvl_list_reverse_str`] has to [`_mvl_array_reverse`].
///
/// # Safety
/// `arr` must be a valid `MvlArray` pointer (elem_size == 8, holding
/// `*mut MvlString` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_slice_str(
    arr: *const MvlArray,
    start: i64,
    end: i64,
) -> *mut MvlArray {
    if arr.is_null() {
        return _mvl_array_new(8, 0);
    }
    let len = (*arr).len as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    let count = hi.saturating_sub(lo);
    let out = _mvl_array_new(8, count.max(1));
    for i in lo..hi {
        let src = (*arr).ptr.add(i * 8) as *const *mut MvlString;
        let cloned = _mvl_string_clone(*src);
        _mvl_array_push(out, (&cloned as *const *mut MvlString).cast());
    }
    out
}

/// `List[T]::slice(start, end)` for a *pointer-typed* element `T` that isn't
/// `String` — a nested `List`/`Set`/`Array`/`Map` (#2265). Same relationship
/// [`_mvl_list_slice_str`] has to [`_mvl_list_slice`], generalized via a
/// caller-supplied clone callback so one helper covers every pointer-shaped
/// element kind (`_mvl_array_clone` for a nested array, `_mvl_map_clone` for
/// a nested map) instead of needing one runtime symbol per element type.
///
/// [`_mvl_list_slice`] byte-copies each element's raw bytes — correct for
/// scalars, but for a pointer element it hands the slice the *same* heap
/// object the source array still owns. Both then independently drop it:
/// `examples/bzip/huffman.mvl::remove_at_ll`'s `list.slice(..)` on a
/// `List[List[Int]]` aliased the inner `List[Int]`s, so dropping the source
/// and the slice freed each inner array twice.
///
/// # Safety
/// `arr` must be a valid `MvlArray*` (elem_size == 8, holding pointer
/// elements) or null. `clone_fn` must be a valid C-ABI function matching the
/// elements' actual type.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_slice_ptr(
    arr: *const MvlArray,
    start: i64,
    end: i64,
    clone_fn: unsafe extern "C" fn(*mut u8) -> *mut u8,
) -> *mut MvlArray {
    if arr.is_null() {
        return _mvl_array_new(8, 0);
    }
    let len = (*arr).len as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    let count = hi.saturating_sub(lo);
    let out = _mvl_array_new(8, count.max(1));
    for i in lo..hi {
        let src = (*arr).ptr.add(i * 8) as *const *mut u8;
        let cloned = if (*src).is_null() {
            *src
        } else {
            clone_fn(*src)
        };
        _mvl_array_push(out, (&cloned as *const *mut u8).cast());
    }
    out
}

/// `List[T]::slice(start, end)` for a payload-enum element `T` (#2265) — the
/// slice counterpart of [`_mvl_array_extend_enum`]. Each 16-byte `{ i8, ptr }`
/// element's payload is cloned through the emitter-generated per-type
/// `clone_fn` trampoline, so the slice owns its payloads independently of the
/// source array (which byte-copying via [`_mvl_list_slice`] would not).
///
/// # Safety
/// `arr` must be a valid `MvlArray*` (elem_size == 16, holding `{ i8, ptr }`
/// payload-enum elements) or null. `clone_fn` must be a valid C-ABI function
/// matching `T`'s own `@_mvl_clone_enum_<T>` trampoline signature.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_slice_enum(
    arr: *const MvlArray,
    start: i64,
    end: i64,
    clone_fn: unsafe extern "C" fn(u8, *mut u8) -> *mut u8,
) -> *mut MvlArray {
    if arr.is_null() {
        return _mvl_array_new(16, 0);
    }
    let len = (*arr).len as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    let count = hi.saturating_sub(lo);
    let out = _mvl_array_new(16, count.max(1));
    for i in lo..hi {
        let slot = (*arr).ptr.add(i * 16);
        let disc = *slot;
        let payload = *(slot.add(8) as *mut *mut u8);
        let new_payload = if payload.is_null() {
            payload
        } else {
            clone_fn(disc, payload)
        };
        let mut new_slot = [0u8; 16];
        new_slot[0] = disc;
        new_slot[8..16].copy_from_slice(&(new_payload as usize).to_ne_bytes());
        _mvl_array_push(out, new_slot.as_ptr());
    }
    out
}

/// Concatenate `a` and `b`, returning a new `MvlArray` with all elements of `a`
/// followed by all elements of `b`.  `a` and `b` must have the same `elem_size`.
///
/// # Safety
/// `a` and `b` must be valid non-null `MvlArray` pointers or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_concat(a: *const MvlArray, b: *const MvlArray) -> *mut MvlArray {
    let (es, la, lb) = match (a.is_null(), b.is_null()) {
        (true, true) => return _mvl_array_new(8, 0),
        (false, true) => ((*a).elem_size as usize, (*a).len as usize, 0usize),
        (true, false) => ((*b).elem_size as usize, 0usize, (*b).len as usize),
        (false, false) => (
            (*a).elem_size as usize,
            (*a).len as usize,
            (*b).len as usize,
        ),
    };
    let a_bytes = if a.is_null() || la == 0 {
        &[] as &[u8]
    } else {
        std::slice::from_raw_parts((*a).ptr, la * es)
    };
    let b_bytes = if b.is_null() || lb == 0 {
        &[] as &[u8]
    } else {
        std::slice::from_raw_parts((*b).ptr, lb * es)
    };
    let merged = mvl_runtime_core::concat_bytes(a_bytes, b_bytes);
    let total = la + lb;
    let out = _mvl_array_new(es, total.max(1));
    if !merged.is_empty() {
        ptr::copy_nonoverlapping(merged.as_ptr(), (*out).ptr, merged.len());
        (*out).len = total as u64;
    }
    out
}

/// `List[T]::concat(other)` for a *pointer-typed* element `T` that isn't a
/// scalar — `String`, or a nested `List`/`Set`/`Array`/`Map` (#2285).
///
/// [`_mvl_list_concat`] byte-copies both inputs' element bytes, which for a
/// pointer element hands the result the *same* heap objects its inputs still
/// own. Whichever side is dropped first frees them out from under the
/// concatenated list. `std/csv.mvl::parse_rows_with` accumulates its rows as
/// `rows = rows.concat([row])` on a `List[List[String]]`, so every parsed row
/// was freed by the temporary one-element literal's own scope-exit drop while
/// the accumulated list still pointed at it — `.len()` on the result stayed
/// correct (the outer array is fine) but `.get(0)` returned a dangling inner
/// pointer.
///
/// Same shape as [`_mvl_list_slice_ptr`]: one helper for every pointer-shaped
/// element kind, with the per-kind `_mvl_*_clone` supplied as a callback.
///
/// # Safety
/// `a`/`b` must be valid `MvlArray*` (elem_size == 8, holding pointer
/// elements) or null. `clone_fn` must be a valid C-ABI function matching the
/// elements' actual type.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_concat_ptr(
    a: *const MvlArray,
    b: *const MvlArray,
    clone_fn: unsafe extern "C" fn(*mut u8) -> *mut u8,
) -> *mut MvlArray {
    let la = if a.is_null() { 0 } else { (*a).len as usize };
    let lb = if b.is_null() { 0 } else { (*b).len as usize };
    let out = _mvl_array_new(8, (la + lb).max(1));
    for (src_arr, len) in [(a, la), (b, lb)] {
        if src_arr.is_null() {
            continue;
        }
        for i in 0..len {
            let src = (*src_arr).ptr.add(i * 8) as *const *mut u8;
            let cloned = if (*src).is_null() {
                *src
            } else {
                clone_fn(*src)
            };
            _mvl_array_push(out, (&cloned as *const *mut u8).cast());
        }
    }
    out
}

/// `List[T]::concat(other)` for a payload-enum element `T` (#2285) — the
/// concat counterpart of [`_mvl_list_slice_enum`]/[`_mvl_array_extend_enum`].
/// Each 16-byte `{ i8, ptr }` element's payload is cloned through the
/// emitter-generated per-type `clone_fn` trampoline so the result owns its
/// payloads independently of both inputs.
///
/// # Safety
/// `a`/`b` must be valid `MvlArray*` (elem_size == 16, holding `{ i8, ptr }`
/// payload-enum elements) or null. `clone_fn` must match `T`'s own
/// `@_mvl_clone_enum_<T>` trampoline signature.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_concat_enum(
    a: *const MvlArray,
    b: *const MvlArray,
    clone_fn: unsafe extern "C" fn(u8, *mut u8) -> *mut u8,
) -> *mut MvlArray {
    let la = if a.is_null() { 0 } else { (*a).len as usize };
    let lb = if b.is_null() { 0 } else { (*b).len as usize };
    let out = _mvl_array_new(16, (la + lb).max(1));
    for (src_arr, len) in [(a, la), (b, lb)] {
        if src_arr.is_null() {
            continue;
        }
        for i in 0..len {
            let slot = (*src_arr).ptr.add(i * 16);
            let disc = *slot;
            let payload = *(slot.add(8) as *mut *mut u8);
            let new_payload = if payload.is_null() {
                payload
            } else {
                clone_fn(disc, payload)
            };
            let mut new_slot = [0u8; 16];
            new_slot[0] = disc;
            new_slot[8..16].copy_from_slice(&(new_payload as usize).to_ne_bytes());
            _mvl_array_push(out, new_slot.as_ptr());
        }
    }
    out
}

// ── MvlMap operations ──────────────────────────────────────────────────────────

/// Insert `(key[0..key_len], val[0..val_len])` into the map.
/// Replaces the existing value if the key already exists.
/// Grows 2× if load factor exceeds 50%.
///
/// # Safety
/// `m`, `key`, and `val` must be valid non-null pointers.
pub(crate) unsafe fn mvl_map_insert(
    m: *mut MvlMap,
    key: *const u8,
    key_len: usize,
    val: *const u8,
    val_len: usize,
) {
    if m.is_null() || key.is_null() || key_len == 0 {
        return;
    }
    // Grow if load factor > 50%.
    if (*m).len + 1 > (*m).cap / 2 {
        let old_cap = (*m).cap as usize;
        let new_cap = checked_mul_size(old_cap, 2).max(MAP_INITIAL_CAP);
        let new_slot_bytes = checked_mul_size(new_cap, SLOT_SIZE);
        let new_slots = _mvl_alloc(new_slot_bytes) as *mut MvlMapSlot;
        ptr::write_bytes(new_slots as *mut u8, 0, new_slot_bytes);
        for i in 0..old_cap {
            let old = &*(*m).slots.add(i);
            if old.occupied == 1 {
                let idx =
                    map_find_slot(new_slots, new_cap as u64, old.key_ptr, old.key_len as usize);
                ptr::copy_nonoverlapping(old, new_slots.add(idx), 1);
            }
        }
        _mvl_free((*m).slots as *mut u8, checked_mul_size(old_cap, SLOT_SIZE));
        (*m).slots = new_slots;
        (*m).cap = new_cap as u64;
    }

    let idx = map_find_slot((*m).slots, (*m).cap, key, key_len);
    let slot = &mut *(*m).slots.add(idx);
    if slot.occupied != 0 {
        // Replace existing value.
        if slot.val_len > 0 {
            _mvl_free(slot.val_ptr, slot.val_len as usize);
        }
        if val_len > 0 {
            let new_val = _mvl_alloc(val_len);
            ptr::copy_nonoverlapping(val, new_val, val_len);
            slot.val_ptr = new_val;
        } else {
            slot.val_ptr = ptr::null_mut();
        }
        slot.val_len = val_len as u64;
    } else {
        // New entry.
        let kp = _mvl_alloc(key_len);
        ptr::copy_nonoverlapping(key, kp, key_len);
        let vp = if val_len > 0 {
            let p = _mvl_alloc(val_len);
            ptr::copy_nonoverlapping(val, p, val_len);
            p
        } else {
            ptr::null_mut()
        };
        slot.occupied = 1;
        slot.key_ptr = kp;
        slot.key_len = key_len as u64;
        slot.val_ptr = vp;
        slot.val_len = val_len as u64;
        (*m).len += 1;
    }
}

/// Return a pointer to the value bytes for `key`, or null if not found.
///
/// # Safety
/// `m` and `key` must be valid non-null pointers.
/// The returned pointer is valid only while `m` is alive and not mutated.
pub(crate) unsafe fn mvl_map_get(m: *const MvlMap, key: *const u8, key_len: usize) -> *const u8 {
    if m.is_null() || key.is_null() {
        return ptr::null();
    }
    // Growth invariant: len < cap is maintained by mvl_map_insert (grows at >50% load).
    // map_find_slot loops until it finds an empty slot; a 100% full map with an absent
    // key would loop forever.
    debug_assert!((*m).len < (*m).cap, "map invariant violated: len >= cap");
    let idx = map_find_slot((*m).slots, (*m).cap, key, key_len);
    let slot = &*(*m).slots.add(idx);
    if slot.occupied == 0 {
        ptr::null()
    } else {
        slot.val_ptr
    }
}

/// Return the number of entries in the map.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
pub(crate) unsafe fn mvl_map_len(m: *const MvlMap) -> u64 {
    if m.is_null() {
        return 0;
    }
    (*m).len
}

/// Decompose a UTF-8 string into a `MvlArray` of `*mut MvlString` pointers (one per char).
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_chars(s: *const MvlString) -> *mut MvlArray {
    let arr = _mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    if s.is_null() {
        return arr;
    }
    let len = (*s).len as usize;
    if len == 0 {
        return arr;
    }
    let bytes = std::slice::from_raw_parts((*s).ptr, len);
    let text =
        std::str::from_utf8(bytes).expect("mvl_string_chars: MvlString contains invalid UTF-8");
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buf);
        let char_s = _mvl_string_new(encoded.as_ptr(), encoded.len());
        _mvl_array_push(arr, (&char_s as *const *mut MvlString).cast());
    }
    arr
}

/// Return all keys in the map as a `MvlArray` of `*mut MvlString` pointers.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
pub(crate) unsafe fn mvl_map_keys(m: *const MvlMap) -> *mut MvlArray {
    let arr = _mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    if m.is_null() || (*m).cap == 0 {
        return arr;
    }
    let cap = (*m).cap as usize;
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 {
            let key_s = _mvl_string_new(slot.key_ptr, slot.key_len as usize);
            _mvl_array_push(arr, (&key_s as *const *mut MvlString).cast());
        }
    }
    arr
}

/// Return an `MvlArray*` of the map's values, in the same raw representation
/// `V` already has in each slot (a scalar's bits, or a boxed value's
/// pointer — always exactly `val_len` bytes, since every MVL value that fits
/// a map slot is a single GPR-class register — see `mvl_map_insert`).
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
///
/// Previously wrapped each value in a freshly-allocated `MvlString` via
/// `_mvl_string_new(slot.val_ptr, slot.val_len)`, mirroring `mvl_map_keys` —
/// correct there because a key's stored bytes really are its UTF-8 content,
/// but wrong here: a value's stored bytes are a fixed-size register value
/// (e.g. an `Int`'s raw bits, or a `String`'s pointer), not variable-length
/// text. Treating those bytes as string content built a garbage `MvlString`
/// and returned *its* pointer in place of the real value, so every element
/// silently became an unrelated heap address (#2251 — surfaced via
/// `Map[K,V]::fold`/`any`/`all`/`filter`/`map_values`, which all forward
/// through `self.values()` in `std/collections.mvl`).
///
/// Boxed value types (`String`, nested collections, structs holding boxed
/// fields) are copied by raw pointer without bumping their refcount here —
/// the returned array aliases the map's own owned reference rather than
/// cloning it (contrast `Map::get`'s scalar-vs-String clone split at the
/// call site). Fine for the scalar-`V` corpus this fixes; a boxed-`V`
/// `.values()`/`.fold()` call that outlives the source map, or drops both
/// independently, is a latent double-free/use-after-free this doesn't
/// address — not attempted here.
pub(crate) unsafe fn mvl_map_values(m: *const MvlMap) -> *mut MvlArray {
    if m.is_null() || (*m).cap == 0 {
        return _mvl_array_new(std::mem::size_of::<i64>(), 0);
    }
    let cap = (*m).cap as usize;
    let mut elem_size = std::mem::size_of::<i64>();
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 {
            elem_size = slot.val_len as usize;
            break;
        }
    }
    let arr = _mvl_array_new(elem_size, 0);
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 && !slot.val_ptr.is_null() {
            _mvl_array_push(arr, slot.val_ptr);
        }
    }
    arr
}

/// Drop an array whose elements are owned `*mut MvlString` pointers.
///
/// Decrements the array's refcount.  When refcount reaches zero, each element
/// string is freed via `_mvl_string_drop` before the array itself is freed.
/// Use this instead of `_mvl_array_drop` for arrays returned by `mvl_string_chars`
/// or `mvl_map_keys`, which own their element strings.
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer whose elements are
/// `*mut MvlString` pointers produced by `_mvl_string_new`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_string_ptr_array_drop(arr: *mut MvlArray) {
    if arr.is_null() {
        return;
    }
    (*arr).refcount = (*arr)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*arr).refcount == 0 {
        let len = (*arr).len as usize;
        let es = (*arr).elem_size as usize;
        for i in 0..len {
            let elem_ptr = (*arr).ptr.add(i * es) as *mut *mut MvlString;
            let s = *elem_ptr;
            if !s.is_null() {
                _mvl_string_drop(s);
            }
        }
        // Free the array buffer and struct (same logic as _mvl_array_drop).
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            _mvl_free((*arr).ptr, data_size);
        }
        _mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
    }
}

/// Drop an array whose elements are owned `*mut MvlArray` pointers (e.g.
/// `List[List[T]]`, `List[Set[T]]`).
///
/// Decrements the array's refcount.  When refcount reaches zero, each element
/// array is dropped via `inner_drop` before the outer array itself is freed.
/// `inner_drop` must be the correct typed drop for the inner array's own
/// element type (`_mvl_array_drop` for scalar inner elements,
/// `_mvl_string_ptr_array_drop` for `String` inner elements) — the emitter
/// selects it based on the declared type (#1991).
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer whose elements are
/// `*mut MvlArray` pointers. `inner_drop` must be a valid, non-null C-ABI
/// function matching the inner arrays' actual element type.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_drop_mvlarray(
    arr: *mut MvlArray,
    inner_drop: unsafe extern "C" fn(*mut u8),
) {
    if arr.is_null() {
        return;
    }
    (*arr).refcount = (*arr)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*arr).refcount == 0 {
        let len = (*arr).len as usize;
        let es = (*arr).elem_size as usize;
        for i in 0..len {
            let elem_ptr = (*arr).ptr.add(i * es) as *mut *mut u8;
            let inner = *elem_ptr;
            if !inner.is_null() {
                inner_drop(inner);
            }
        }
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            _mvl_free((*arr).ptr, data_size);
        }
        _mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
    }
}

/// Drop an array of `Option[T]` elements (`{ i8, ptr }` inline slots, disc
/// 0 = Some / 1 = None per the LLVM emitter's convention).
///
/// For each `Some` slot, frees the heap-allocated payload slot (size
/// `payload_size`, matching what the emitter allocated via `_mvl_alloc`),
/// calling `payload_drop` first when `T` is itself heap-owning (e.g.
/// `String`) — pass `None` for scalar `T`, which needs no destructor before
/// the payload slot is freed (#1991).
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer with `elem_size == 16`
/// (the `{ i8, ptr }` Option representation). `payload_size` must match the
/// size used to allocate each `Some` payload slot. `payload_drop`, if
/// present, must be a valid C-ABI function matching the payload's actual type.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_drop_option(
    arr: *mut MvlArray,
    payload_size: u64,
    payload_drop: Option<unsafe extern "C" fn(*mut u8)>,
) {
    if arr.is_null() {
        return;
    }
    (*arr).refcount = (*arr)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*arr).refcount == 0 {
        let len = (*arr).len as usize;
        let es = (*arr).elem_size as usize;
        for i in 0..len {
            let slot = (*arr).ptr.add(i * es);
            let disc = *slot;
            if disc == 0 {
                let payload_ptr = *(slot.add(8) as *mut *mut u8);
                if !payload_ptr.is_null() {
                    if let Some(drop_fn) = payload_drop {
                        drop_fn(payload_ptr);
                    }
                    _mvl_free(payload_ptr, payload_size as usize);
                }
            }
        }
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            _mvl_free((*arr).ptr, data_size);
        }
        _mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
    }
}

/// Drop an array of `Result[T, E]` elements (`{ i8, ptr }` inline slots,
/// disc 0 = Ok / 1 = Err, each pointing at a heap-allocated payload of its
/// respective type).
///
/// Mirrors [`_mvl_array_drop_option`] but with separate size/drop-fn pairs
/// for the `Ok` and `Err` payloads, since `T` and `E` may differ (#1991).
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer with `elem_size == 16`.
/// `ok_size`/`err_size` must match the sizes used to allocate each payload
/// slot. `ok_drop`/`err_drop`, if present, must be valid C-ABI functions
/// matching the respective payload types.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_drop_result(
    arr: *mut MvlArray,
    ok_size: u64,
    ok_drop: Option<unsafe extern "C" fn(*mut u8)>,
    err_size: u64,
    err_drop: Option<unsafe extern "C" fn(*mut u8)>,
) {
    if arr.is_null() {
        return;
    }
    (*arr).refcount = (*arr)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*arr).refcount == 0 {
        let len = (*arr).len as usize;
        let es = (*arr).elem_size as usize;
        for i in 0..len {
            let slot = (*arr).ptr.add(i * es);
            let disc = *slot;
            let payload_ptr = *(slot.add(8) as *mut *mut u8);
            if !payload_ptr.is_null() {
                if disc == 0 {
                    if let Some(drop_fn) = ok_drop {
                        drop_fn(payload_ptr);
                    }
                    _mvl_free(payload_ptr, ok_size as usize);
                } else {
                    if let Some(drop_fn) = err_drop {
                        drop_fn(payload_ptr);
                    }
                    _mvl_free(payload_ptr, err_size as usize);
                }
            }
        }
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            _mvl_free((*arr).ptr, data_size);
        }
        _mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
    }
}

/// Drop a map whose values are pointer-typed (`String`, `List[..]`,
/// `Set[..]`, `Map[..]`) — the map stores only the value's 8-byte address, so
/// each occupied slot's value must be followed and dropped via `value_drop`
/// before the slot itself is freed. Keys never need this treatment: they are
/// always deep-copied into the map's own buffer at insert time (#1991).
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer whose values are all
/// pointers of the same type that `value_drop` expects.
#[no_mangle]
pub unsafe extern "C" fn _mvl_map_drop_ptr_values(
    m: *mut MvlMap,
    value_drop: unsafe extern "C" fn(*mut u8),
) {
    if m.is_null() {
        return;
    }
    (*m).refcount = (*m)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*m).refcount == 0 {
        let cap = (*m).cap as usize;
        for i in 0..cap {
            let slot = &*(*m).slots.add(i);
            if slot.occupied != 0 {
                let value_ptr = *(slot.val_ptr as *mut *mut u8);
                if !value_ptr.is_null() {
                    value_drop(value_ptr);
                }
                _mvl_free(slot.key_ptr, slot.key_len as usize);
                _mvl_free(slot.val_ptr, slot.val_len as usize);
            }
        }
        let slot_bytes = cap
            .checked_mul(std::mem::size_of::<MvlMapSlot>())
            .unwrap_or_else(|| std::process::abort());
        _mvl_free((*m).slots as *mut u8, slot_bytes);
        _mvl_free(m as *mut u8, std::mem::size_of::<MvlMap>());
    }
}

/// Remove the entry with the given key from the map (no-op if absent).
///
/// # Safety
/// `m` and `key` must be valid non-null pointers.
pub(crate) unsafe fn mvl_map_remove(m: *mut MvlMap, key: *const u8, key_len: usize) {
    if m.is_null() || key.is_null() || key_len == 0 || (*m).cap == 0 {
        return;
    }
    if (*m).len == 0 {
        return;
    }
    debug_assert!(
        (*m).len < (*m).cap,
        "mvl_map_remove: map invariant violated (len >= cap)"
    );
    let idx = map_find_slot((*m).slots, (*m).cap, key, key_len);
    let slot = &mut *(*m).slots.add(idx);
    if slot.occupied != 1 {
        return; // empty (0) or tombstone (2) — key not present
    }
    if slot.key_len > 0 && !slot.key_ptr.is_null() {
        _mvl_free(slot.key_ptr, slot.key_len as usize);
    }
    if slot.val_len > 0 && !slot.val_ptr.is_null() {
        _mvl_free(slot.val_ptr, slot.val_len as usize);
    }
    // Mark as tombstone (2) so collision chains remain intact for subsequent lookups.
    slot.occupied = 2;
    slot.key_ptr = ptr::null_mut();
    slot.key_len = 0;
    slot.val_ptr = ptr::null_mut();
    slot.val_len = 0;
    (*m).len = (*m)
        .len
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
}

// ── String parsing ─────────────────────────────────────────────────────────────
//
// Both functions use out-pointer parameters to avoid returning large structs
// (> 16 bytes), which triggers the sret calling convention on ARM64 and is
// not reliably handled by `lli`'s JIT when calling into external dylibs.
//
// Signature pattern:
//   tag = fn(s, ok_out, err_out)
//   0 → Ok: *ok_out  is written; *err_out is untouched
//   1 → Err: *err_out is written (heap MvlString, caller must drop); *ok_out is untouched

/// Parse a `MvlString` as a signed 64-bit integer.
///
/// Returns 0 (Ok) and writes the value to `*ok_out`,
/// or returns 1 (Err) and writes a heap `MvlString` error message to `*err_out`.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
/// `ok_out` and `err_out` must be valid non-null writable pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_parse_int(
    s: *const MvlString,
    ok_out: *mut i64,
    err_out: *mut *mut MvlString,
) -> i8 {
    let len = (*s).len as usize;
    let bytes = std::slice::from_raw_parts((*s).ptr, len);
    let text = std::str::from_utf8(bytes).unwrap_or("").trim();
    match text.parse::<i64>() {
        Ok(n) => {
            *ok_out = n;
            0
        }
        Err(e) => {
            let msg = e.to_string();
            *err_out = _mvl_string_new(msg.as_ptr(), msg.len());
            1
        }
    }
}

/// Parse a `MvlString` as a 64-bit float.
///
/// Returns 0 (Ok) and writes the value to `*ok_out`,
/// or returns 1 (Err) and writes a heap `MvlString` error message to `*err_out`.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
/// `ok_out` and `err_out` must be valid non-null writable pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_parse_float(
    s: *const MvlString,
    ok_out: *mut f64,
    err_out: *mut *mut MvlString,
) -> i8 {
    let len = (*s).len as usize;
    let bytes = std::slice::from_raw_parts((*s).ptr, len);
    let text = std::str::from_utf8(bytes).unwrap_or("").trim();
    match text.parse::<f64>() {
        Ok(x) => {
            *ok_out = x;
            0
        }
        Err(e) => {
            let msg = e.to_string();
            *err_out = _mvl_string_new(msg.as_ptr(), msg.len());
            1
        }
    }
}

// ── Higher-order list functions (#1163) ─────────────────────────────────────
//
// Closure struct layout matches `%__closure_type = type { ptr, ptr }` emitted
// by `llvm_text`.  Field 0 is the function pointer, field 1 is the captured-
// environment pointer (null for non-capturing lambdas / named-fn wrappers).
//
// All closure fn_ptrs use the convention: `fn(env: ptr, params…) -> ret`.

/// Closure struct matching `%__closure_type = type { ptr, ptr }`.
#[repr(C)]
pub struct MvlClosure {
    fn_ptr: *const (),
    env_ptr: *const (),
}

/// `_mvl_list_filter(list, closure)` — keep elements where `closure(elem)` is true.
///
/// Supports any element size.  The closure receives a *pointer* to each element
/// (not the element by value), so it works for both scalars and aggregates like
/// `Option[Int]` (`{ i8, ptr }`).
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_filter(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let out = _mvl_array_new(es, len.max(1));
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if pred(env, elem_ptr) {
            _mvl_array_push(out, elem_ptr);
        }
    }
    out
}

/// `_mvl_list_map(list, closure)` — transform each element via `closure(elem)`.
///
/// The closure receives a pointer to each element and returns an i64-sized
/// result (output `elem_size` == 8).  Input elements can be any size.
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> i64`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_map(
    list: *mut MvlArray,
    closure: *const MvlClosure,
    out_elem_size: i64,
) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(out_elem_size as usize, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    // #2264: the OUTPUT array's element size is the closure's *return*
    // type's size, not the input's — `(*list).elem_size` was used for both
    // before this fix, silently wrong whenever `.map()` changes element
    // size (e.g. `List[Int]::map(|v: Int| -> Byte {...})`, exactly
    // `examples/bzip/bwt.mvl`'s pattern for every `List[Byte]` it builds).
    // Every pushed result byte-copied a full closure-return-width value
    // into a slot sized for the *input* element instead — silently correct
    // for same-width in/out types (String→String, Int→Int), corrupting/
    // truncating anything else.
    let out = _mvl_array_new(out_elem_size as usize, len.max(1));
    let map_fn: unsafe extern "C" fn(*const u8, *const u8) -> i64 =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let result = map_fn(env, elem_ptr);
        _mvl_array_push(out, (&result as *const i64) as *const u8);
    }
    out
}

/// `_mvl_list_fold(list, acc_ptr, closure)` — reduce list with accumulator.
///
/// `acc_ptr` points to the initial accumulator value (stack-allocated by the
/// caller).  The closure has signature `fn(env, acc_val, elem_ptr) -> acc_val`.
/// The final accumulator is written back to `acc_ptr`, which is also returned.
///
/// Accumulator is i64 (8 bytes).  Elements can be any size (passed by pointer).
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `acc_ptr` must be a writable pointer
/// to at least 8 bytes.  `closure` must point to a valid `MvlClosure` whose
/// `fn_ptr` has signature `fn(env: ptr, acc: i64, elem: ptr) -> i64`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_fold(
    list: *mut MvlArray,
    acc_ptr: *mut u8,
    closure: *const MvlClosure,
) -> *mut u8 {
    if list.is_null() || acc_ptr.is_null() {
        std::process::abort();
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let fold_fn: unsafe extern "C" fn(*const u8, i64, *const u8) -> i64 =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    let mut acc = *(acc_ptr as *const i64);
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        acc = fold_fn(env, acc, elem_ptr);
    }
    *(acc_ptr as *mut i64) = acc;
    acc_ptr
}

/// `_mvl_list_any(list, closure)` — return true if any element satisfies predicate.
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_any(list: *mut MvlArray, closure: *const MvlClosure) -> bool {
    if list.is_null() {
        return false;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if pred(env, elem_ptr) {
            return true;
        }
    }
    false
}

/// `_mvl_list_all(list, closure)` — return true if all elements satisfy predicate.
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_all(list: *mut MvlArray, closure: *const MvlClosure) -> bool {
    if list.is_null() {
        return true;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if !pred(env, elem_ptr) {
            return false;
        }
    }
    true
}

/// `_mvl_list_take_while(list, closure)` — return a new list of leading elements
/// that satisfy `closure`; stop at the first element that does not.
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> bool`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_take_while(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let out = _mvl_array_new(es, len.max(1));
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if !pred(env, elem_ptr) {
            break;
        }
        _mvl_array_push(out, elem_ptr);
    }
    out
}

/// `_mvl_list_skip_while(list, closure)` — return a new list with the leading
/// elements that satisfy `closure` removed; keep everything from the first
/// element that does not satisfy it onwards.
///
/// # Safety
/// `list` must be a valid `MvlArray*`.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(env: ptr, elem: ptr) -> bool`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_skip_while(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let out = _mvl_array_new(es, len.max(1));
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    let mut skipping = true;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if skipping && pred(env, elem_ptr) {
            continue;
        }
        skipping = false;
        _mvl_array_push(out, elem_ptr);
    }
    out
}

/// `List[T].reverse()` (#2256) — return a new list with elements in
/// reverse order. Raw byte-level element copy, safe for any non-owning
/// (scalar or pointer-free struct) element type of any `elem_size`.
/// Pointer-owning elements (String, nested List/Map/Set, or structs
/// containing them) need [`_mvl_list_reverse_str`]'s clone-based approach
/// instead — same reason `sort`/`contains`/`join` needed dedicated
/// String-aware C-ABI symbols. Also backs `rev()`, whose MVL fallback body
/// just delegates to `reverse()`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_reverse(list: *const MvlArray) -> *mut MvlArray {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    let es = if list.is_null() {
        8
    } else {
        (*list).elem_size as usize
    };
    let out = _mvl_array_new(es, len.max(1));
    for i in (0..len).rev() {
        let elem_ptr = (*list).ptr.add(i * es);
        _mvl_array_push(out, elem_ptr);
    }
    out
}

/// `List[String].reverse()` / `.rev()` (#2256) — return a new
/// `List[String]` with elements in reverse order. Elements are cloned
/// (refcount bumped) into the output array rather than moved, matching
/// `_mvl_list_sort_str`'s ownership contract: `list` and the returned array
/// are independent owners once `reverse()` returns.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut
/// MvlString` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_reverse_str(list: *const MvlArray) -> *mut MvlArray {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    let out = _mvl_array_new(8, len.max(1));
    for i in (0..len).rev() {
        let src = (*list).ptr.add(i * 8) as *const *mut MvlString;
        let cloned = _mvl_string_clone(*src);
        _mvl_array_push(out, (&cloned as *const *mut MvlString).cast());
    }
    out
}

// ── Category-D builtins: sort / partition / group_by / windows / chunks ────────

/// `_mvl_list_sort(list)` — return a new list with elements sorted ascending.
///
/// Elements are compared as i64 (8-byte) values.  Correct for Int, Bool, and
/// Byte lists only.  Float lists will sort by bit pattern (wrong for negatives
/// and NaN).  TODO: add type-aware comparator for Float (#1290 Phase 2).
///
/// # Safety
/// `list` must be a valid `MvlArray*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_sort(list: *mut MvlArray) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 0);
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let out = _mvl_array_new(es, len.max(1));
    for i in 0..len {
        _mvl_array_push(out, (*list).ptr.add(i * es));
    }
    if len <= 1 {
        return out;
    }
    // All MVL scalar types (Int, Bool, Byte, Float) are stored as 8 bytes.
    // Sort by reading each element as i64 and comparing numerically.
    // NOTE: Float sort is incorrect for negative values / NaN (bit-pattern
    // comparison). A type-aware comparator is needed for Phase 2.
    debug_assert!(
        es <= 8,
        "_mvl_list_sort: elem_size {} > 8 not supported",
        es
    );
    let mut vals: Vec<i64> = (0..len)
        .map(|i| {
            let mut buf = [0u8; 8];
            let src = (*out).ptr.add(i * es);
            std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), es);
            i64::from_ne_bytes(buf)
        })
        .collect();
    vals.sort_unstable();
    for (i, v) in vals.iter().enumerate() {
        let dst = (*out).ptr.add(i * es);
        std::ptr::copy_nonoverlapping(v.to_ne_bytes().as_ptr(), dst, es);
    }
    out
}

/// `List[T].min()` / `.max()` (#2256) — index of the numerically smallest
/// (resp. largest) element, or `-1` for an empty list. Compares elements as
/// signed i64 bit patterns — correct for `Int`/`Byte`; same bit-pattern
/// caveat as `_mvl_list_sort` for `Float` (negative values / NaN order
/// incorrectly).
///
/// Returns an index rather than a value or pointer so the emitter can reuse
/// `_mvl_array_get` + its existing clone-on-extract handling (see the `get`/
/// `first`/`last` dispatch arms) unchanged.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size <= 8) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_min_index_i64(list: *const MvlArray) -> i64 {
    list_extreme_index_i64(list, |a, b| a < b)
}

#[no_mangle]
pub unsafe extern "C" fn _mvl_list_max_index_i64(list: *const MvlArray) -> i64 {
    list_extreme_index_i64(list, |a, b| a > b)
}

unsafe fn list_extreme_index_i64(list: *const MvlArray, better: impl Fn(i64, i64) -> bool) -> i64 {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    if len == 0 {
        return -1;
    }
    let es = (*list).elem_size as usize;
    debug_assert!(es <= 8, "_mvl_list_min/max_index_i64: elem_size {es} > 8");
    let read = |i: usize| -> i64 {
        let mut buf = [0u8; 8];
        let src = (*list).ptr.add(i * es);
        std::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), es);
        i64::from_ne_bytes(buf)
    };
    let mut best_idx = 0usize;
    let mut best_val = read(0);
    for i in 1..len {
        let v = read(i);
        if better(v, best_val) {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx as i64
}

/// `List[T] == List[T]` / `Set[T] == Set[T]` (#2264), `T` scalar — content
/// equality: same length and identical raw bytes. Two arrays with different
/// `elem_size` are never equal (ill-typed comparison, shouldn't happen for a
/// well-typed caller, but avoids reading past a shorter buffer).
///
/// Not safe for pointer-containing element types (String, nested List/Set/
/// Map, struct/enum with heap fields) — those need per-element equality
/// (e.g. `_mvl_string_eq` per element for `List[String]`), not a raw byte
/// compare of the pointers themselves. Not implemented here — see #2265 for
/// the general per-element dispatch gap this and `extend`/`sort` share.
///
/// # Safety
/// `a` and `b` must be valid `MvlArray*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_array_eq(a: *const MvlArray, b: *const MvlArray) -> bool {
    if a == b {
        return true;
    }
    if a.is_null() || b.is_null() {
        return false;
    }
    if (*a).len != (*b).len || (*a).elem_size != (*b).elem_size {
        return false;
    }
    let len = (*a).len as usize;
    if len == 0 {
        return true;
    }
    let bytes = len * (*a).elem_size as usize;
    libc::memcmp((*a).ptr.cast(), (*b).ptr.cast(), bytes) == 0
}

/// `List[String].min()` / `.max()` (#2256) — index of the lexicographically
/// smallest (resp. largest) string, or `-1` for an empty list. Same
/// index-based contract as [`_mvl_list_min_index_i64`], comparing string
/// *content* rather than element pointer identity — same reason `sort`/
/// `contains`/`join` needed dedicated String-aware C-ABI symbols.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut
/// MvlString` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_min_index_str(list: *const MvlArray) -> i64 {
    list_extreme_index_str(list, |a, b| a < b)
}

#[no_mangle]
pub unsafe extern "C" fn _mvl_list_max_index_str(list: *const MvlArray) -> i64 {
    list_extreme_index_str(list, |a, b| a > b)
}

unsafe fn list_extreme_index_str(
    list: *const MvlArray,
    better: impl Fn(&str, &str) -> bool,
) -> i64 {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    if len == 0 {
        return -1;
    }
    let read = |i: usize| -> &str {
        let s = *((*list).ptr.add(i * 8) as *const *mut MvlString);
        as_str(s)
    };
    let mut best_idx = 0usize;
    let mut best_val = read(0);
    for i in 1..len {
        let v = read(i);
        if better(v, best_val) {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx as i64
}

/// `List[List[Byte]].sort()` (#2264) — return a new list of byte-lists
/// sorted ascending by lexicographic content.
///
/// `_mvl_list_sort` compares elements as raw i64 bit patterns, which for a
/// `*mut MvlArray` element is the heap address, not the nested list's
/// content — same shape as the #2173 `List[String]::sort` fix, scoped here
/// to `List[Byte]`-element lists (`bwt.mvl`'s cyclic-rotation sort). Nested
/// arrays are cloned via [`crate::memory::_mvl_array_deep_clone`] into the
/// output — safe because the *inner* list's own elements are scalar bytes,
/// same reasoning as `_mvl_array_extend_nested`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut
/// MvlArray` elements, each itself holding `Byte`/i8 elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_sort_bytelist(list: *mut MvlArray) -> *mut MvlArray {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    let out = _mvl_array_new(8, len.max(1));
    let mut ptrs: Vec<*mut MvlArray> = (0..len)
        .map(|i| {
            let src = (*list).ptr.add(i * 8) as *const *mut MvlArray;
            crate::memory::_mvl_array_deep_clone(*src)
        })
        .collect();
    ptrs.sort_unstable_by(|&a, &b| {
        let sa = std::slice::from_raw_parts((*a).ptr, (*a).len as usize);
        let sb = std::slice::from_raw_parts((*b).ptr, (*b).len as usize);
        sa.cmp(sb)
    });
    for p in ptrs {
        _mvl_array_push(out, (&p as *const *mut MvlArray).cast());
    }
    out
}

/// `_mvl_list_sort_str(list)` — return a new `List[String]` sorted ascending
/// by byte content (#2173).
///
/// `_mvl_list_sort` compares elements as raw i64 bit patterns, which for a
/// `*mut MvlString` element is the heap *address*, not the string's
/// content — order would depend on allocation order rather than being
/// lexicographic. Elements are cloned (refcount bumped) into the output
/// array rather than moved, since `list` and the returned array are
/// independent owners once `sort()` returns — matching what a hand-written
/// `sort_by` body achieves via `.clone()`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut
/// MvlString` elements) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_sort_str(list: *mut MvlArray) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 0);
    }
    let len = (*list).len as usize;
    let out = _mvl_array_new(8, len.max(1));
    let mut ptrs: Vec<*mut MvlString> = (0..len)
        .map(|i| {
            let src = (*list).ptr.add(i * 8) as *const *mut MvlString;
            _mvl_string_clone(*src)
        })
        .collect();
    ptrs.sort_unstable_by(|&a, &b| as_str(a).cmp(as_str(b)));
    for p in ptrs {
        _mvl_array_push(out, &p as *const *mut MvlString as *const u8);
    }
    out
}

/// `_mvl_list_sort_nested_bytelist(list)` — return a new
/// `List[List[Byte]]`/`List[List[Bool]]` sorted ascending by the *content* of
/// each inner array (#2264).
///
/// Same defect and same shape as `_mvl_list_sort_str` above, one nesting level
/// out: `_mvl_list_sort` compares elements as raw i64 bit patterns, which for
/// a `*mut MvlArray` element is the heap *address*, so the result depends on
/// allocation order rather than being lexicographic.
/// `examples/bzip/bwt.mvl::bwt_encode` sorts `List[List[Byte]]` rotations and
/// needs genuine lexicographic order to find the correct primary index;
/// pointer order gave a silently wrong result rather than a crash. The WASM
/// backend got the equivalent `_mvl_array_sort_nested_bytelist` in #2267 —
/// this is the LLVM half.
///
/// Comparing raw bytes lexicographically is only equivalent to element-wise
/// ordering for `Byte`/`Bool` inner elements, whose LLVM element type is `i8`
/// (`elem_size == 1`), so each byte *is* one non-negative element. It does not
/// hold for `Int`/`Float` inner lists — those are 8-byte little-endian lanes,
/// where the low-order byte sorts first (e.g. `256` vs `2`: `256`'s first byte
/// is `0x00`, placing it before `2`) and sign bits break the order outright.
/// `emit_exprs_tir.rs` only dispatches here for `Byte`/`Bool`; `Int`/`Float`
/// inner lists stay on the pre-existing (wrong-by-pointer, not fixed here)
/// `_mvl_list_sort` fallback.
///
/// Inner arrays are cloned (refcount bumped) into the output rather than
/// moved, because `list` and the returned array become independent owners that
/// each deep-drop their elements (`HeapKind::ArrayOfArray` →
/// `_mvl_array_drop_mvlarray`) — sharing them un-bumped would double-free.
/// Same reasoning as `_mvl_list_sort_str`'s `_mvl_string_clone`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut MvlArray`
/// elements, each itself a `Byte`/`Bool`-element array) or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_sort_nested_bytelist(list: *mut MvlArray) -> *mut MvlArray {
    if list.is_null() {
        return _mvl_array_new(8, 0);
    }
    let len = (*list).len as usize;
    let out = _mvl_array_new(8, len.max(1));
    let mut ptrs: Vec<*mut MvlArray> = (0..len)
        .map(|i| {
            let src = (*list).ptr.add(i * 8) as *const *mut MvlArray;
            _mvl_array_clone(*src)
        })
        .collect();
    ptrs.sort_unstable_by(|&a, &b| mvl_array_bytes(a).cmp(mvl_array_bytes(b)));
    for p in ptrs {
        _mvl_array_push(out, &p as *const *mut MvlArray as *const u8);
    }
    out
}

/// Byte view of a scalar-element `MvlArray`'s live elements, or `&[]` for a
/// null pointer. Helper for [`_mvl_list_sort_nested_bytelist`].
///
/// # Safety
/// `a` must be a valid `MvlArray*` or null.
unsafe fn mvl_array_bytes<'a>(a: *const MvlArray) -> &'a [u8] {
    if a.is_null() || (*a).ptr.is_null() {
        return &[];
    }
    let nbytes = ((*a).len as usize).saturating_mul((*a).elem_size as usize);
    std::slice::from_raw_parts((*a).ptr as *const u8, nbytes)
}

/// `List[String].join(sep)` (#2256) — concatenate every element into a
/// single new `String`, inserting `sep`'s bytes between adjacent elements.
/// Returns an empty string for an empty list.
///
/// Method calls to `List[String]::join` never reached this far: dispatch in
/// `emit_method_call_tir` had no arm for a non-generic extension method
/// named `join` (the generic-fallback path only covers `List[T]` methods
/// monomorphized per element type), so the call silently evaluated to
/// nothing. This gives it a dedicated C-ABI symbol, same as `sort`/`contains`
/// got for the same "String needs content-aware handling" reason.
///
/// Does not take ownership of `list` or `sep` — callers retain and drop them
/// separately, matching how `_mvl_string_concat` borrows its arguments.
///
/// # Safety
/// `list` must be a valid `MvlArray*` (elem_size == 8, holding `*mut
/// MvlString` elements) or null. `sep` must be a valid `MvlString*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_join_str(
    list: *const MvlArray,
    sep: *const MvlString,
) -> *mut MvlString {
    let len = if list.is_null() {
        0
    } else {
        (*list).len as usize
    };
    let sep_bytes: &[u8] = if sep.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts((*sep).ptr, (*sep).len as usize)
    };
    let elems: Vec<&[u8]> = (0..len)
        .map(|i| {
            let s = *((*list).ptr.add(i * 8) as *const *mut MvlString);
            if s.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts((*s).ptr, (*s).len as usize)
            }
        })
        .collect();
    let total: usize = elems.iter().map(|e| e.len()).sum::<usize>()
        + sep_bytes.len().saturating_mul(len.saturating_sub(1));
    let mut merged = Vec::with_capacity(total + 1);
    for (i, e) in elems.iter().enumerate() {
        if i > 0 {
            merged.extend_from_slice(sep_bytes);
        }
        merged.extend_from_slice(e);
    }
    merged.push(0); // null terminator
    let out_len = merged.len() - 1;
    let cap = merged.len();
    let data = _mvl_alloc(cap);
    ptr::copy_nonoverlapping(merged.as_ptr(), data, cap);
    let s = _mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
    s.write(MvlString {
        ptr: data,
        len: out_len as u64,
        cap: cap as u64,
        refcount: 1,
    });
    s
}

/// `_mvl_list_partition(list, closure)` — split into matching and non-matching.
///
/// Returns a heap-allocated `[ptr; 2]`: index 0 is elements where predicate
/// is true, index 1 is elements where predicate is false.  The LLVM emitter
/// destructures this into two named bindings via `getelementptr` + `load`.
///
/// **Ownership:** The caller owns the returned 16-byte pair buffer and both
/// inner `MvlArray*` pointers.  The emitter must free the pair buffer after
/// extracting the two arrays.
///
/// Predicate signature: `fn(env: ptr, elem: ptr) -> i1`.
///
/// Note: category-D HOFs (partition, group_by) pass elements by pointer,
/// unlike category-A/B HOFs (filter, map, fold) which pass by i64 value.
/// The LLVM emitter uses `ptr_param_indices` in `emit_as_hof_closure` to
/// generate the correct closure wrapper.
///
/// # Safety
/// `list` and `closure` must be valid non-null pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_partition(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut u8 {
    let pair = _mvl_alloc(16) as *mut *mut MvlArray;
    if list.is_null() {
        *pair = _mvl_array_new(8, 0);
        *pair.add(1) = _mvl_array_new(8, 0);
        return pair as *mut u8;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let yes = _mvl_array_new(es, len.max(1));
    let no = _mvl_array_new(es, len.max(1));
    let pred: unsafe extern "C" fn(*const u8, *const u8) -> bool =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        if pred(env, elem_ptr) {
            _mvl_array_push(yes, elem_ptr);
        } else {
            _mvl_array_push(no, elem_ptr);
        }
    }
    *pair = yes;
    *pair.add(1) = no;
    pair as *mut u8
}

/// `_mvl_list_group_by(list, closure)` — group elements by key.
///
/// Calls `closure(env, elem_ptr) -> i64` for each element.  Returns a
/// `MvlMap*` mapping each i64 key to its `MvlArray*` group.  Map values
/// are 8-byte pointer slots storing `MvlArray*` pointers.
///
/// Key closure signature: `fn(env: ptr, elem: ptr) -> i64`.
///
/// Note: like `_mvl_list_partition`, elements are passed by pointer (not
/// by i64 value).  See partition doc for calling convention rationale.
///
/// # Safety
/// `list` and `closure` must be valid non-null pointers.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_group_by(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlMap {
    let map = crate::memory::_mvl_map_new(0);
    if list.is_null() {
        return map;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    let key_fn: unsafe extern "C" fn(*const u8, *const u8) -> i64 =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let key: i64 = key_fn(env, elem_ptr);
        let key_bytes = key.to_ne_bytes();
        let existing = crate::memory_ops::mvl_map_get(map as *const MvlMap, key_bytes.as_ptr(), 8);
        let group: *mut MvlArray = if existing.is_null() {
            let new_group = _mvl_array_new(es, 1);
            let ptr_val = new_group as usize;
            let ptr_bytes = ptr_val.to_ne_bytes();
            crate::memory_ops::mvl_map_insert(map, key_bytes.as_ptr(), 8, ptr_bytes.as_ptr(), 8);
            new_group
        } else {
            // The map stores the 8-byte pointer value; dereference it.
            *(existing as *const *mut MvlArray)
        };
        _mvl_array_push(group, elem_ptr);
    }
    map
}

/// `_mvl_list_windows(list, n)` — all contiguous windows of length `n`.
///
/// Returns a `MvlArray*` of `MvlArray*` pointers (List[List[T]]).
/// Each inner array is a fresh slice of `n` elements.
///
/// # Safety
/// `list` must be a valid `MvlArray*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_windows(list: *mut MvlArray, n: i64) -> *mut MvlArray {
    // Result is a list of ptrs (elem_size = 8).
    let out = _mvl_array_new(8, 1);
    if list.is_null() || n <= 0 {
        return out;
    }
    let len = (*list).len as i64;
    if n > len {
        return out;
    }
    let count = (len - n + 1) as usize;
    for i in 0..count {
        let window = _mvl_list_slice(list, i as i64, i as i64 + n);
        let ptr_val = window as usize;
        let ptr_bytes = ptr_val.to_ne_bytes();
        _mvl_array_push(out, ptr_bytes.as_ptr());
    }
    out
}

/// `_mvl_list_chunks(list, n)` — non-overlapping chunks of at most `n` elements.
///
/// Returns a `MvlArray*` of `MvlArray*` pointers (List[List[T]]).
/// The last chunk may be shorter than `n`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_chunks(list: *mut MvlArray, n: i64) -> *mut MvlArray {
    let out = _mvl_array_new(8, 1);
    if list.is_null() || n <= 0 {
        return out;
    }
    let len = (*list).len as i64;
    let mut i: i64 = 0;
    while i < len {
        let end = (i + n).min(len);
        let chunk = _mvl_list_slice(list, i, end);
        let ptr_val = chunk as usize;
        let ptr_bytes = ptr_val.to_ne_bytes();
        _mvl_array_push(out, ptr_bytes.as_ptr());
        i += n;
    }
    out
}

// ── Struct-returning list/map builtins (#1383) ────────────────────────────────

/// `_mvl_list_enumerate(list)` — produce `List[Indexed[T]]`.
///
/// Each output element is a 16-byte `{ i64 index, 8-byte value }` struct,
/// matching the LLVM layout of `%Indexed { i64, ptr/i64 }`.
/// The value slot copies the raw 8 bytes from the input element
/// (either an i64 scalar or an 8-byte pointer for heap types).
///
/// # Safety
/// `list` must be a valid non-null `MvlArray` pointer with `elem_size <= 8`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_enumerate(list: *mut MvlArray) -> *mut MvlArray {
    let out = _mvl_array_new(16, 0);
    if list.is_null() {
        return out;
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    for i in 0..len {
        let src = (*list).ptr.add(i * es);
        let mut buf = [0u8; 16];
        buf[..8].copy_from_slice(&(i as i64).to_ne_bytes());
        let copy_len = es.min(8);
        buf[8..8 + copy_len].copy_from_slice(std::slice::from_raw_parts(src, copy_len));
        _mvl_array_push(out, buf.as_ptr());
    }
    out
}

/// `_mvl_list_zip(a, b)` — produce `List[Pair[T, U]]`.
///
/// Each output element is a 16-byte `{ 8-byte first, 8-byte second }` struct,
/// matching the LLVM layout of `%Pair { ptr/i64, ptr/i64 }`.
/// Stops at the shorter of the two lists.
///
/// # Safety
/// `a` and `b` must be valid non-null `MvlArray` pointers with `elem_size <= 8`.
#[no_mangle]
pub unsafe extern "C" fn _mvl_list_zip(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray {
    let out = _mvl_array_new(16, 0);
    if a.is_null() || b.is_null() {
        return out;
    }
    let len = ((*a).len as usize).min((*b).len as usize);
    let es_a = (*a).elem_size as usize;
    let es_b = (*b).elem_size as usize;
    for i in 0..len {
        let src_a = (*a).ptr.add(i * es_a);
        let src_b = (*b).ptr.add(i * es_b);
        let mut buf = [0u8; 16];
        let copy_a = es_a.min(8);
        let copy_b = es_b.min(8);
        buf[..copy_a].copy_from_slice(std::slice::from_raw_parts(src_a, copy_a));
        buf[8..8 + copy_b].copy_from_slice(std::slice::from_raw_parts(src_b, copy_b));
        _mvl_array_push(out, buf.as_ptr());
    }
    out
}

/// `_mvl_map_entries(map)` — produce `List[Entry[K, V]]`.
///
/// Each output element is a 16-byte `{ ptr key_string, 8-byte value }` struct,
/// matching the LLVM layout of `%Entry { ptr, ptr/i64 }`.
/// Keys become freshly-allocated `MvlString*` objects (caller owns them).
/// Values are copied raw (up to 8 bytes) from the map slot.
///
/// # Safety
/// `map` must be a valid non-null `MvlMap` pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_map_entries(map: *mut MvlMap) -> *mut MvlArray {
    let out = _mvl_array_new(16, 0);
    if map.is_null() || (*map).cap == 0 {
        return out;
    }
    let cap = (*map).cap as usize;
    for i in 0..cap {
        let slot = &*(*map).slots.add(i);
        if slot.occupied == 1 {
            let key_s = _mvl_string_new(slot.key_ptr, slot.key_len as usize);
            let val_len = (slot.val_len as usize).min(8);
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&(key_s as usize).to_ne_bytes());
            if val_len > 0 && !slot.val_ptr.is_null() {
                buf[8..8 + val_len]
                    .copy_from_slice(std::slice::from_raw_parts(slot.val_ptr, val_len));
            }
            _mvl_array_push(out, buf.as_ptr());
        }
    }
    out
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{_mvl_array_clone, _mvl_array_drop, _mvl_array_new};
    use crate::memory::{_mvl_map_clone, _mvl_map_drop, _mvl_map_new};
    use crate::memory::{_mvl_string_clone, _mvl_string_drop, _mvl_string_new};

    // ── string operations ──────────────────────────────────────────────────────

    #[test]
    fn string_len_and_ptr() {
        unsafe {
            let s = _mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!(_mvl_string_len(s), 5);
            assert_eq!(*_mvl_string_ptr(s).add(5), 0); // null-terminated
            _mvl_string_drop(s);
        }
    }

    #[test]
    fn string_empty_len() {
        unsafe {
            let s = _mvl_string_new(b"".as_ptr(), 0);
            assert_eq!(_mvl_string_len(s), 0);
            assert_eq!(*_mvl_string_ptr(s), 0);
            _mvl_string_drop(s);
        }
    }

    #[test]
    fn string_concat() {
        unsafe {
            let a = _mvl_string_new(b"foo".as_ptr(), 3);
            let b = _mvl_string_new(b"bar".as_ptr(), 3);
            let c = _mvl_string_concat(a, b);
            assert_eq!(_mvl_string_len(c), 6);
            let slice = std::slice::from_raw_parts(_mvl_string_ptr(c), 6);
            assert_eq!(slice, b"foobar");
            assert_eq!(*_mvl_string_ptr(c).add(6), 0);
            _mvl_string_drop(a);
            _mvl_string_drop(b);
            _mvl_string_drop(c);
        }
    }

    #[test]
    fn string_eq() {
        unsafe {
            let a = _mvl_string_new(b"abc".as_ptr(), 3);
            let b = _mvl_string_new(b"abc".as_ptr(), 3);
            let c = _mvl_string_new(b"xyz".as_ptr(), 3);
            assert_eq!(_mvl_string_eq(a, b), 1);
            assert_eq!(_mvl_string_eq(a, c), 0);
            let _ = _mvl_string_clone(a); // refcount → 2 (same ptr; raw ptr, no Rust Drop)
            assert_eq!(_mvl_string_eq(a, a), 1); // pointer-equality short-circuit
            _mvl_string_drop(a); // refcount → 1
            _mvl_string_drop(a); // refcount → 0, freed
            _mvl_string_drop(b);
            _mvl_string_drop(c);
        }
    }

    #[test]
    fn list_join_str_inserts_separator() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let bb = _mvl_string_new(b"bb".as_ptr(), 2);
            let s = _mvl_string_new(b"a".as_ptr(), 1);
            let ccc = _mvl_string_new(b"ccc".as_ptr(), 3);
            _mvl_array_push(a, (&bb as *const *mut MvlString).cast());
            _mvl_array_push(a, (&s as *const *mut MvlString).cast());
            _mvl_array_push(a, (&ccc as *const *mut MvlString).cast());

            let sep = _mvl_string_new(b"-".as_ptr(), 1);
            let joined = _mvl_list_join_str(a, sep);
            assert_eq!(as_str(joined), "bb-a-ccc");

            _mvl_string_drop(joined);
            _mvl_string_drop(sep);
            _mvl_string_ptr_array_drop(a);
        }
    }

    #[test]
    fn list_join_str_empty_list_is_empty_string() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let sep = _mvl_string_new(b"-".as_ptr(), 1);
            let joined = _mvl_list_join_str(a, sep);
            assert_eq!(_mvl_string_len(joined), 0);
            _mvl_string_drop(joined);
            _mvl_string_drop(sep);
            _mvl_string_ptr_array_drop(a);
        }
    }

    #[test]
    fn list_min_max_index_i64() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            for v in [3i64, 1, 2] {
                _mvl_array_push(a, (&v as *const i64).cast());
            }
            assert_eq!(_mvl_list_min_index_i64(a), 1);
            assert_eq!(_mvl_list_max_index_i64(a), 0);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_min_max_index_i64_empty() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            assert_eq!(_mvl_list_min_index_i64(a), -1);
            assert_eq!(_mvl_list_max_index_i64(a), -1);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_min_max_index_str() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let bb = _mvl_string_new(b"bb".as_ptr(), 2);
            let s = _mvl_string_new(b"a".as_ptr(), 1);
            let ccc = _mvl_string_new(b"ccc".as_ptr(), 3);
            _mvl_array_push(a, (&bb as *const *mut MvlString).cast());
            _mvl_array_push(a, (&s as *const *mut MvlString).cast());
            _mvl_array_push(a, (&ccc as *const *mut MvlString).cast());
            assert_eq!(_mvl_list_min_index_str(a), 1); // "a"
            assert_eq!(_mvl_list_max_index_str(a), 2); // "ccc"
            _mvl_string_ptr_array_drop(a);
        }
    }

    #[test]
    fn array_eq_compares_content_not_pointer() {
        unsafe {
            let a = _mvl_array_new(1, 0);
            let b = _mvl_array_new(1, 0);
            for arr in [a, b] {
                for byte in [1u8, 2, 3] {
                    _mvl_array_push(arr, &byte as *const u8);
                }
            }
            assert_ne!(a, b); // different allocations
            assert!(_mvl_array_eq(a, b));

            let c = _mvl_array_new(1, 0);
            for byte in [1u8, 2, 4] {
                _mvl_array_push(c, &byte as *const u8);
            }
            assert!(!_mvl_array_eq(a, c));

            let d = _mvl_array_new(1, 0);
            _mvl_array_push(d, &1u8 as *const u8);
            assert!(!_mvl_array_eq(a, d)); // different length

            _mvl_array_drop(a);
            _mvl_array_drop(b);
            _mvl_array_drop(c);
            _mvl_array_drop(d);
        }
    }

    #[test]
    fn list_sort_bytelist_by_content_not_pointer() {
        unsafe {
            let list = _mvl_array_new(8, 0);
            for bytes in [&b"ccc"[..], &b"a"[..], &b"bb"[..]] {
                let inner = _mvl_array_new(1, 0);
                for &b in bytes {
                    _mvl_array_push(inner, &b as *const u8);
                }
                _mvl_array_push(list, (&inner as *const *mut MvlArray).cast());
            }
            let sorted = _mvl_list_sort_bytelist(list);
            assert_eq!(_mvl_array_len(sorted), 3);
            let expect = |i: i64, want: &[u8]| {
                let inner = *(_mvl_array_get(sorted, i) as *const *mut MvlArray);
                let got = std::slice::from_raw_parts((*inner).ptr, (*inner).len as usize);
                assert_eq!(got, want);
            };
            expect(0, b"a");
            expect(1, b"bb");
            expect(2, b"ccc");

            // Drop the (now-unused) original list's shell + its nested
            // arrays, and the sorted output's shell + its (cloned) nested
            // arrays — independent allocations, no double-free.
            for i in 0..3 {
                let inner = *(_mvl_array_get(list, i) as *const *mut MvlArray);
                _mvl_array_drop(inner);
            }
            _mvl_array_drop(list);
            for i in 0..3 {
                let inner = *(_mvl_array_get(sorted, i) as *const *mut MvlArray);
                _mvl_array_drop(inner);
            }
            _mvl_array_drop(sorted);
        }
    }

    #[test]
    fn array_extend_scalar() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            for v in [1i64, 2, 3] {
                _mvl_array_push(a, (&v as *const i64).cast());
            }
            let b = _mvl_array_new(8, 0);
            for v in [4i64, 5] {
                _mvl_array_push(b, (&v as *const i64).cast());
            }
            _mvl_array_extend(a, b);
            assert_eq!(_mvl_array_len(a), 5);
            for (i, expected) in [1i64, 2, 3, 4, 5].iter().enumerate() {
                let p = _mvl_array_get(a, i as i64) as *const i64;
                assert_eq!(*p, *expected);
            }
            // `other` (b) must remain valid and untouched.
            assert_eq!(_mvl_array_len(b), 2);
            _mvl_array_drop(a);
            _mvl_array_drop(b);
        }
    }

    #[test]
    fn array_extend_str_clones_and_leaves_other_valid() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let x = _mvl_string_new(b"x".as_ptr(), 1);
            _mvl_array_push(a, (&x as *const *mut MvlString).cast());

            let b = _mvl_array_new(8, 0);
            let y = _mvl_string_new(b"y".as_ptr(), 1);
            let z = _mvl_string_new(b"z".as_ptr(), 1);
            _mvl_array_push(b, (&y as *const *mut MvlString).cast());
            _mvl_array_push(b, (&z as *const *mut MvlString).cast());

            _mvl_array_extend_str(a, b);
            assert_eq!(_mvl_array_len(a), 3);
            assert_eq!(
                as_str(*(_mvl_array_get(a, 1) as *const *mut MvlString)),
                "y"
            );
            assert_eq!(
                as_str(*(_mvl_array_get(a, 2) as *const *mut MvlString)),
                "z"
            );

            // `other` (b) must remain independently valid — its elements
            // weren't moved, they were cloned.
            assert_eq!(_mvl_array_len(b), 2);
            assert_eq!(
                as_str(*(_mvl_array_get(b, 0) as *const *mut MvlString)),
                "y"
            );

            _mvl_string_ptr_array_drop(a);
            _mvl_string_ptr_array_drop(b);
        }
    }

    #[test]
    fn array_extend_nested_clones_and_leaves_other_valid() {
        unsafe {
            let a = _mvl_array_new(8, 0);

            let b = _mvl_array_new(8, 0);
            let inner = _mvl_array_new(8, 0);
            let v: i64 = 42;
            _mvl_array_push(inner, (&v as *const i64).cast());
            _mvl_array_push(b, (&inner as *const *mut MvlArray).cast());

            _mvl_array_extend_nested(a, b);
            assert_eq!(_mvl_array_len(a), 1);
            let cloned_inner = *(_mvl_array_get(a, 0) as *const *mut MvlArray);
            assert_ne!(cloned_inner, inner); // independent allocation
            assert_eq!(_mvl_array_len(cloned_inner), 1);

            // `other` (b) and its nested array remain independently valid.
            assert_eq!(_mvl_array_len(b), 1);
            assert_eq!(_mvl_array_len(inner), 1);

            _mvl_array_drop(cloned_inner);
            _mvl_array_drop(a);
            _mvl_array_drop(inner);
            _mvl_array_drop(b);
        }
    }

    // ── array operations ───────────────────────────────────────────────────────

    #[test]
    fn array_push_get_len() {
        unsafe {
            let a = _mvl_array_new(8, 0); // i64 elements
            assert_eq!(_mvl_array_len(a), 0);
            let v1: i64 = 42;
            let v2: i64 = 99;
            _mvl_array_push(a, (&v1 as *const i64).cast());
            _mvl_array_push(a, (&v2 as *const i64).cast());
            assert_eq!(_mvl_array_len(a), 2);
            let p1 = _mvl_array_get(a, 0) as *const i64;
            let p2 = _mvl_array_get(a, 1) as *const i64;
            assert_eq!(*p1, 42);
            assert_eq!(*p2, 99);
            assert!(_mvl_array_get(a, 2).is_null());
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn array_grows_past_initial_cap() {
        unsafe {
            let a = _mvl_array_new(8, 2);
            for i in 0i64..16 {
                _mvl_array_push(a, (&i as *const i64).cast());
            }
            assert_eq!(_mvl_array_len(a), 16);
            for i in 0i64..16 {
                let p = _mvl_array_get(a, i) as *const i64;
                assert_eq!(*p, i);
            }
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn array_clone_refcount() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let v: i64 = 7;
            _mvl_array_push(a, (&v as *const i64).cast());
            let a2 = _mvl_array_clone(a);
            assert_eq!((*a).refcount, 2);
            _mvl_array_drop(a2);
            assert_eq!((*a).refcount, 1);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn array_contains_str_compares_content_not_identity() {
        unsafe {
            let a = _mvl_array_new(8, 0); // *mut MvlString elements
            let bb = _mvl_string_new(b"bb".as_ptr(), 2);
            let s = _mvl_string_new(b"a".as_ptr(), 1);
            let ccc = _mvl_string_new(b"ccc".as_ptr(), 3);
            _mvl_array_push(a, (&bb as *const *mut MvlString).cast());
            _mvl_array_push(a, (&s as *const *mut MvlString).cast());
            _mvl_array_push(a, (&ccc as *const *mut MvlString).cast());

            // Different allocation, same content — must match by content, not pointer.
            let needle = _mvl_string_new(b"a".as_ptr(), 1);
            assert_ne!(needle, s);
            assert!(_mvl_array_contains_str(a, needle));

            let missing = _mvl_string_new(b"zzz".as_ptr(), 3);
            assert!(!_mvl_array_contains_str(a, missing));

            _mvl_string_drop(needle);
            _mvl_string_drop(missing);
            _mvl_string_ptr_array_drop(a);
        }
    }

    // ── map operations ─────────────────────────────────────────────────────────

    #[test]
    fn map_insert_get_len() {
        unsafe {
            let m = _mvl_map_new(0);
            assert_eq!(mvl_map_len(m), 0);
            let k = b"key1";
            let v: i64 = 123;
            mvl_map_insert(m, k.as_ptr(), 4, (&v as *const i64).cast(), 8);
            assert_eq!(mvl_map_len(m), 1);
            let got = mvl_map_get(m, k.as_ptr(), 4) as *const i64;
            assert!(!got.is_null());
            assert_eq!(*got, 123);
            assert!(mvl_map_get(m, b"nope".as_ptr(), 4).is_null());
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_replace_value() {
        unsafe {
            let m = _mvl_map_new(0);
            let k = b"x";
            let v1: i64 = 1;
            let v2: i64 = 2;
            mvl_map_insert(m, k.as_ptr(), 1, (&v1 as *const i64).cast(), 8);
            mvl_map_insert(m, k.as_ptr(), 1, (&v2 as *const i64).cast(), 8);
            assert_eq!(mvl_map_len(m), 1);
            let got = *(mvl_map_get(m, k.as_ptr(), 1) as *const i64);
            assert_eq!(got, 2);
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_grows_past_initial_cap() {
        unsafe {
            let m = _mvl_map_new(0);
            for i in 0i64..32 {
                let key = i.to_le_bytes();
                mvl_map_insert(m, key.as_ptr(), 8, (&i as *const i64).cast(), 8);
            }
            assert_eq!(mvl_map_len(m), 32);
            for i in 0i64..32 {
                let key = i.to_le_bytes();
                let got = *(mvl_map_get(m, key.as_ptr(), 8) as *const i64);
                assert_eq!(got, i);
            }
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_clone_refcount() {
        unsafe {
            let m = _mvl_map_new(0);
            let m2 = _mvl_map_clone(m);
            assert_eq!((*m).refcount, 2);
            _mvl_map_drop(m2);
            assert_eq!((*m).refcount, 1);
            _mvl_map_drop(m);
        }
    }

    // ── map_remove + tombstone ─────────────────────────────────────────────────

    #[test]
    fn map_remove_simple() {
        unsafe {
            let m = _mvl_map_new(0);
            let k = b"foo";
            let v: i64 = 42;
            mvl_map_insert(m, k.as_ptr(), 3, (&v as *const i64).cast(), 8);
            assert_eq!(mvl_map_len(m), 1);
            mvl_map_remove(m, k.as_ptr(), 3);
            assert_eq!(mvl_map_len(m), 0);
            assert!(
                mvl_map_get(m, k.as_ptr(), 3).is_null(),
                "removed key should be absent"
            );
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_remove_absent_noop() {
        unsafe {
            let m = _mvl_map_new(0);
            let k = b"x";
            let v: i64 = 1;
            mvl_map_insert(m, k.as_ptr(), 1, (&v as *const i64).cast(), 8);
            mvl_map_remove(m, b"y".as_ptr(), 1); // absent key — no-op
            assert_eq!(mvl_map_len(m), 1);
            let got = *(mvl_map_get(m, k.as_ptr(), 1) as *const i64);
            assert_eq!(got, 1);
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_remove_tombstone_collision_chain() {
        // Verify that removing a key does not break lookup for keys that probed
        // past the removed slot (the classic tombstone correctness test).
        unsafe {
            let m = _mvl_map_new(0);
            // Insert enough entries that at least some will collide on a cap=8 table.
            // Use single-byte numeric keys to maximise collision probability.
            let mut inserted: Vec<(Vec<u8>, i64)> = Vec::new();
            for i in 0i64..6 {
                let key = i.to_le_bytes().to_vec();
                mvl_map_insert(m, key.as_ptr(), 8, (&i as *const i64).cast(), 8);
                inserted.push((key, i));
            }
            assert_eq!(mvl_map_len(m), 6);

            // Remove the first three; they become tombstones.
            for (key, _) in &inserted[..3] {
                mvl_map_remove(m, key.as_ptr(), 8);
            }
            assert_eq!(mvl_map_len(m), 3);

            // The remaining three must still be reachable through tombstone chains.
            for (key, val) in &inserted[3..] {
                let got = mvl_map_get(m, key.as_ptr(), 8) as *const i64;
                assert!(!got.is_null(), "key {val} should survive tombstone removal");
                assert_eq!(*got, *val);
            }

            // Re-insert the removed keys — must land correctly.
            for (key, val) in &inserted[..3] {
                mvl_map_insert(m, key.as_ptr(), 8, (val as *const i64).cast(), 8);
            }
            assert_eq!(mvl_map_len(m), 6);
            for (key, val) in &inserted {
                let got = *(mvl_map_get(m, key.as_ptr(), 8) as *const i64);
                assert_eq!(got, *val);
            }
            _mvl_map_drop(m);
        }
    }

    // ── mvl_string_chars ──────────────────────────────────────────────────────

    #[test]
    fn string_chars_ascii() {
        unsafe {
            let s = _mvl_string_new(b"abc".as_ptr(), 3);
            let arr = _mvl_string_chars(s);
            assert_eq!(_mvl_array_len(arr), 3);
            let expected = [b"a" as &[u8], b"b", b"c"];
            for (i, exp) in expected.iter().enumerate() {
                let elem_ptr = _mvl_array_get(arr, i as i64) as *const *mut MvlString;
                let cs = *elem_ptr;
                assert_eq!(_mvl_string_len(cs), 1);
                let slice = std::slice::from_raw_parts(_mvl_string_ptr(cs), 1);
                assert_eq!(slice, *exp);
            }
            _mvl_string_ptr_array_drop(arr);
            _mvl_string_drop(s);
        }
    }

    #[test]
    fn string_chars_empty() {
        unsafe {
            let s = _mvl_string_new(b"".as_ptr(), 0);
            let arr = _mvl_string_chars(s);
            assert_eq!(_mvl_array_len(arr), 0);
            _mvl_string_ptr_array_drop(arr);
            _mvl_string_drop(s);
        }
    }

    #[test]
    fn string_chars_utf8_multibyte() {
        // "é" is 2 bytes in UTF-8 (0xC3 0xA9); should produce one char element.
        unsafe {
            let text = "aé"; // 3 bytes: 'a' + 0xC3 + 0xA9
            let s = _mvl_string_new(text.as_ptr(), text.len());
            let arr = _mvl_string_chars(s);
            assert_eq!(_mvl_array_len(arr), 2, "expected 2 chars: 'a' and 'é'");
            // First char: 'a' (1 byte)
            let p0 = *(_mvl_array_get(arr, 0) as *const *mut MvlString);
            assert_eq!(_mvl_string_len(p0), 1);
            let s0 = std::slice::from_raw_parts(_mvl_string_ptr(p0), 1);
            assert_eq!(s0, b"a");
            // Second char: 'é' (2 bytes)
            let p1 = *(_mvl_array_get(arr, 1) as *const *mut MvlString);
            assert_eq!(_mvl_string_len(p1), 2);
            let s1 = std::slice::from_raw_parts(_mvl_string_ptr(p1), 2);
            assert_eq!(s1, "é".as_bytes());
            _mvl_string_ptr_array_drop(arr);
            _mvl_string_drop(s);
        }
    }

    // ── mvl_str_from_bytes ─────────────────────────────────────────────────────

    #[test]
    fn str_from_bytes_ascii() {
        unsafe {
            let arr = _mvl_array_new(1, 0); // `Byte` elem_size == 1 (i8)
            for b in [72u8, 105u8] {
                _mvl_array_push(arr, &b as *const u8);
            }
            let s = _mvl_str_from_bytes(arr);
            assert_eq!(_mvl_string_len(s), 2);
            let slice = std::slice::from_raw_parts(_mvl_string_ptr(s), 2);
            assert_eq!(slice, b"Hi");
            _mvl_string_drop(s);
            _mvl_array_drop(arr);
        }
    }

    #[test]
    fn str_from_bytes_empty() {
        unsafe {
            let arr = _mvl_array_new(1, 0);
            let s = _mvl_str_from_bytes(arr);
            assert_eq!(_mvl_string_len(s), 0);
            _mvl_string_drop(s);
            _mvl_array_drop(arr);
        }
    }

    #[test]
    fn str_from_bytes_null_array() {
        unsafe {
            let s = _mvl_str_from_bytes(ptr::null());
            assert_eq!(_mvl_string_len(s), 0);
            _mvl_string_drop(s);
        }
    }

    // Regression for #2123: with `elem_size == 1`, the third element sits at
    // byte offset 2 within the array's backing buffer — not 8-byte aligned.
    // The original implementation cast that offset to `*const i64` and
    // dereferenced it directly, which crashed with "misaligned pointer
    // dereference" (SIGBUS-class trap) as soon as a `List[Byte]` had more
    // than one element. Every byte 0..=255 must roundtrip losslessly (#1487).
    #[test]
    fn str_from_bytes_misaligned_offsets_and_full_byte_range() {
        unsafe {
            let arr = _mvl_array_new(1, 0);
            for b in [0u8, 1, 127, 128, 200, 255] {
                _mvl_array_push(arr, &b as *const u8);
            }
            let s = _mvl_str_from_bytes(arr);
            // `.len` is the UTF-8 *byte* length, not the input element count —
            // codepoints 128..=255 encode as 2 UTF-8 bytes each (0,1,127 → 1
            // byte; 128,200,255 → 2 bytes: 3 + 6 = 9), so byte-index into the
            // reconstructed string via `_mvl_str_byte_at` (char-indexed) is
            // the correct way to verify the roundtrip, not `.len()`.
            assert_eq!(_mvl_string_len(s), 9);
            for (i, expected) in [0u8, 1, 127, 128, 200, 255].iter().enumerate() {
                let mut out: i64 = -1;
                let tag = _mvl_str_byte_at(s, i as i64, &mut out);
                assert_eq!(tag, 0, "byte {i} should be Some");
                assert_eq!(out, *expected as i64);
            }
            _mvl_string_drop(s);
            _mvl_array_drop(arr);
        }
    }

    // ── mvl_map_keys ──────────────────────────────────────────────────────────

    #[test]
    fn map_keys_basic() {
        unsafe {
            let m = _mvl_map_new(0);
            let v: i64 = 0;
            mvl_map_insert(m, b"alpha".as_ptr(), 5, (&v as *const i64).cast(), 8);
            mvl_map_insert(m, b"beta".as_ptr(), 4, (&v as *const i64).cast(), 8);
            let arr = mvl_map_keys(m);
            assert_eq!(_mvl_array_len(arr), 2);
            // Collect returned key strings into a set for order-independent check.
            let mut found = std::collections::HashSet::new();
            for i in 0..2i64 {
                let elem_ptr = _mvl_array_get(arr, i) as *const *mut MvlString;
                let ks = *elem_ptr;
                let len = _mvl_string_len(ks) as usize;
                let slice = std::slice::from_raw_parts(_mvl_string_ptr(ks), len);
                found.insert(std::str::from_utf8(slice).unwrap().to_string());
            }
            assert!(found.contains("alpha"));
            assert!(found.contains("beta"));
            _mvl_string_ptr_array_drop(arr);
            _mvl_map_drop(m);
        }
    }

    #[test]
    fn map_keys_excludes_tombstones() {
        unsafe {
            let m = _mvl_map_new(0);
            let v: i64 = 0;
            mvl_map_insert(m, b"a".as_ptr(), 1, (&v as *const i64).cast(), 8);
            mvl_map_insert(m, b"b".as_ptr(), 1, (&v as *const i64).cast(), 8);
            mvl_map_remove(m, b"a".as_ptr(), 1);
            let arr = mvl_map_keys(m);
            assert_eq!(
                _mvl_array_len(arr),
                1,
                "tombstone key must not appear in keys()"
            );
            let ks = *(_mvl_array_get(arr, 0) as *const *mut MvlString);
            let slice =
                std::slice::from_raw_parts(_mvl_string_ptr(ks), _mvl_string_len(ks) as usize);
            assert_eq!(slice, b"b");
            _mvl_string_ptr_array_drop(arr);
            _mvl_map_drop(m);
        }
    }

    // ── HOF list functions (#1163) ────────────────────────────────────────────

    /// Helper: build an i64 array from a slice.
    unsafe fn make_i64_array(vals: &[i64]) -> *mut MvlArray {
        let a = _mvl_array_new(8, vals.len().max(1));
        for v in vals {
            _mvl_array_push(a, (v as *const i64).cast());
        }
        a
    }

    /// Helper: read all i64 elements from an array.
    unsafe fn read_i64_array(a: *mut MvlArray) -> Vec<i64> {
        let len = _mvl_array_len(a);
        (0..len)
            .map(|i| *(_mvl_array_get(a, i) as *const i64))
            .collect()
    }

    /// Simple predicate: is x even?  (receives pointer to i64 element)
    unsafe extern "C" fn pred_is_even(_env: *const u8, elem: *const u8) -> bool {
        let x = *(elem as *const i64);
        x % 2 == 0
    }

    /// Simple map fn: double x.  (receives pointer to i64 element)
    unsafe extern "C" fn map_double(_env: *const u8, elem: *const u8) -> i64 {
        let x = *(elem as *const i64);
        x * 2
    }

    /// Simple fold fn: add acc + x.  (receives pointer to i64 element)
    unsafe extern "C" fn fold_add(_env: *const u8, acc: i64, elem: *const u8) -> i64 {
        let x = *(elem as *const i64);
        acc + x
    }

    fn make_closure(fn_ptr: *const (), env_ptr: *const ()) -> MvlClosure {
        MvlClosure { fn_ptr, env_ptr }
    }

    #[test]
    fn list_filter_basic() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3, 4, 5, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = _mvl_list_filter(a, &c);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = _mvl_list_filter(a, &c);
            assert_eq!(_mvl_array_len(out), 0);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_none_match() {
        unsafe {
            let a = make_i64_array(&[1, 3, 5]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = _mvl_list_filter(a, &c);
            assert_eq!(_mvl_array_len(out), 0);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_all_match() {
        unsafe {
            let a = make_i64_array(&[2, 4, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = _mvl_list_filter(a, &c);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_map_basic() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3]);
            let c = make_closure(map_double as *const (), std::ptr::null());
            let out = _mvl_list_map(a, &c, 8);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_map_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(map_double as *const (), std::ptr::null());
            let out = _mvl_list_map(a, &c, 8);
            assert_eq!(_mvl_array_len(out), 0);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    /// Map fn narrowing i64 -> byte-sized value (element still passed back
    /// as i64 per the closure ABI, but the caller declares `out_elem_size`
    /// as 1). Mirrors `List[Int]::map(|v: Int| -> Byte {...})`.
    unsafe extern "C" fn map_to_byte(_env: *const u8, elem: *const u8) -> i64 {
        let x = *(elem as *const i64);
        (x % 256) as i64
    }

    #[test]
    fn list_map_narrowing_uses_out_elem_size_not_input_elem_size() {
        unsafe {
            // #2264: input elements are 8-byte i64; output should be
            // 1-byte, driven by `out_elem_size`, not `(*list).elem_size`.
            let a = make_i64_array(&[104, 101, 108, 108, 111]);
            let c = make_closure(map_to_byte as *const (), std::ptr::null());
            let out = _mvl_list_map(a, &c, 1);
            assert_eq!((*out).elem_size, 1);
            let len = _mvl_array_len(out);
            let bytes: Vec<u8> = (0..len).map(|i| *(_mvl_array_get(out, i))).collect();
            assert_eq!(bytes, vec![104u8, 101, 108, 108, 111]);
            // A same-size List[Byte] built independently (elem_size == 1)
            // must byte-for-byte match this map result — the exact check
            // that caught this bug via `_mvl_array_eq`.
            let independent = _mvl_array_new(1, 5);
            for b in &bytes {
                _mvl_array_push(independent, b as *const u8);
            }
            assert!(_mvl_array_eq(out, independent));
            _mvl_array_drop(independent);
            _mvl_array_drop(out);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_sum() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3, 4, 5]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 0;
            let result = _mvl_list_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(*(result as *const i64), 15);
            assert_eq!(acc, 15);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 42;
            let result = _mvl_list_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(*(result as *const i64), 42);
            assert_eq!(acc, 42);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_nonzero_init() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 100;
            _mvl_list_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(acc, 106);
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_found() {
        unsafe {
            let a = make_i64_array(&[1, 3, 4, 7]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(_mvl_list_any(a, &c));
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_not_found() {
        unsafe {
            let a = make_i64_array(&[1, 3, 5, 7]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!_mvl_list_any(a, &c));
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!_mvl_list_any(a, &c));
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_basic() {
        unsafe {
            let a = make_i64_array(&[2, 4, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(_mvl_list_all(a, &c));
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_fails() {
        unsafe {
            let a = make_i64_array(&[2, 3, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!_mvl_list_all(a, &c));
            _mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(_mvl_list_all(a, &c)); // vacuously true
            _mvl_array_drop(a);
        }
    }
}
