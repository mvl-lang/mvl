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
    _mvl_alloc, _mvl_array_new, _mvl_free, _mvl_string_drop, _mvl_string_new, MvlArray, MvlMap,
    MvlMapSlot, MvlString,
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

#[inline(always)]
fn checked_add_size(a: usize, b: usize) -> usize {
    a.checked_add(b).unwrap_or_else(|| std::process::abort())
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
    let la = if a.is_null() { 0 } else { (*a).len as usize };
    let lb = if b.is_null() { 0 } else { (*b).len as usize };
    let total = checked_add_size(la, lb);
    let cap = checked_add_size(total, 1);
    let data = _mvl_alloc(cap);
    if la > 0 {
        ptr::copy_nonoverlapping((*a).ptr, data, la);
    }
    if lb > 0 {
        ptr::copy_nonoverlapping((*b).ptr, data.add(la), lb);
    }
    *data.add(total) = 0;
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

/// Reconstruct a `MvlString` from a `MvlArray*` of i64 byte values (UTF-8, lossy).
///
/// Each element in the array is an i64 representing one byte (0–255).
///
/// # Safety
/// `arr` must be a valid `MvlArray*` or null.  Each element is an i64.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_from_bytes(arr: *const MvlArray) -> *mut MvlString {
    if arr.is_null() {
        return str_to_mvl("");
    }
    let len = (*arr).len as usize;
    let es = (*arr).elem_size as usize;
    let mut bytes: Vec<u8> = Vec::with_capacity(len);
    for i in 0..len {
        let elem_ptr = (*arr).ptr.add(i * es) as *const i64;
        bytes.push((*elem_ptr & 0xFF) as u8);
    }
    let s = String::from_utf8_lossy(&bytes).into_owned();
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

/// Return a new `MvlArray` containing elements `[start, end)` from `arr` (safe clamping).
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
    let out = _mvl_array_new(es, (la + lb).max(1));
    for i in 0..la {
        let src = (*a).ptr.add(i * es);
        _mvl_array_push(out, src);
    }
    for i in 0..lb {
        let src = (*b).ptr.add(i * es);
        _mvl_array_push(out, src);
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

/// Return an `MvlArray*` of raw-byte values stored in the map.
///
/// Each value is wrapped in a freshly-allocated `MvlString` (reusing the
/// string container as a typed byte-buffer), mirroring the layout returned
/// by `mvl_map_keys`.  Callers should drop the result with
/// `mvl_string_ptr_array_drop` when done.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
pub(crate) unsafe fn mvl_map_values(m: *const MvlMap) -> *mut MvlArray {
    let arr = _mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    if m.is_null() || (*m).cap == 0 {
        return arr;
    }
    let cap = (*m).cap as usize;
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 {
            let val_s = _mvl_string_new(slot.val_ptr, slot.val_len as usize);
            _mvl_array_push(arr, (&val_s as *const *mut MvlString).cast());
        }
    }
    arr
}

/// Drop an array whose elements are owned `*mut MvlString` pointers.
///
/// Decrements the array's refcount.  When refcount reaches zero, each element
/// string is freed via `_mvl_string_drop` before the array itself is freed.
/// Use this instead of `mvl_array_drop` for arrays returned by `mvl_string_chars`
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
        // Free the array buffer and struct (same logic as mvl_array_drop).
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            _mvl_free((*arr).ptr, data_size);
        }
        _mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
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

/// `List_filter(list, closure)` — keep elements where `closure(elem)` is true.
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

/// `List_map(list, closure)` — transform each element via `closure(elem)`.
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

/// `List_fold(list, acc_ptr, closure)` — reduce list with accumulator.
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

/// `List_any(list, closure)` — return true if any element satisfies predicate.
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

/// `List_all(list, closure)` — return true if all elements satisfy predicate.
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
    use crate::memory::{_mvl_array_new, mvl_array_clone, mvl_array_drop};
    use crate::memory::{_mvl_map_new, mvl_map_clone, mvl_map_drop};
    use crate::memory::{_mvl_string_drop, _mvl_string_new, mvl_string_clone};

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
            let _ = mvl_string_clone(a); // refcount → 2 (same ptr; raw ptr, no Rust Drop)
            assert_eq!(_mvl_string_eq(a, a), 1); // pointer-equality short-circuit
            _mvl_string_drop(a); // refcount → 1
            _mvl_string_drop(a); // refcount → 0, freed
            _mvl_string_drop(b);
            _mvl_string_drop(c);
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
            mvl_array_drop(a);
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
            mvl_array_drop(a);
        }
    }

    #[test]
    fn array_clone_refcount() {
        unsafe {
            let a = _mvl_array_new(8, 0);
            let v: i64 = 7;
            _mvl_array_push(a, (&v as *const i64).cast());
            let a2 = mvl_array_clone(a);
            assert_eq!((*a).refcount, 2);
            mvl_array_drop(a2);
            assert_eq!((*a).refcount, 1);
            mvl_array_drop(a);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
        }
    }

    #[test]
    fn map_clone_refcount() {
        unsafe {
            let m = _mvl_map_new(0);
            let m2 = mvl_map_clone(m);
            assert_eq!((*m).refcount, 2);
            mvl_map_drop(m2);
            assert_eq!((*m).refcount, 1);
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            mvl_map_drop(m);
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
            let out = List_filter(a, &c);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = List_filter(a, &c);
            assert_eq!(_mvl_array_len(out), 0);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_none_match() {
        unsafe {
            let a = make_i64_array(&[1, 3, 5]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = List_filter(a, &c);
            assert_eq!(_mvl_array_len(out), 0);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_filter_all_match() {
        unsafe {
            let a = make_i64_array(&[2, 4, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            let out = List_filter(a, &c);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_map_basic() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3]);
            let c = make_closure(map_double as *const (), std::ptr::null());
            let out = List_map(a, &c);
            assert_eq!(read_i64_array(out), vec![2, 4, 6]);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_map_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(map_double as *const (), std::ptr::null());
            let out = List_map(a, &c);
            assert_eq!(_mvl_array_len(out), 0);
            mvl_array_drop(out);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_sum() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3, 4, 5]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 0;
            let result = List_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(*(result as *const i64), 15);
            assert_eq!(acc, 15);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 42;
            let result = List_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(*(result as *const i64), 42);
            assert_eq!(acc, 42);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_fold_nonzero_init() {
        unsafe {
            let a = make_i64_array(&[1, 2, 3]);
            let c = make_closure(fold_add as *const (), std::ptr::null());
            let mut acc: i64 = 100;
            List_fold(a, (&mut acc as *mut i64).cast(), &c);
            assert_eq!(acc, 106);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_found() {
        unsafe {
            let a = make_i64_array(&[1, 3, 4, 7]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(List_any(a, &c));
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_not_found() {
        unsafe {
            let a = make_i64_array(&[1, 3, 5, 7]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!List_any(a, &c));
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_any_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!List_any(a, &c));
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_basic() {
        unsafe {
            let a = make_i64_array(&[2, 4, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(List_all(a, &c));
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_fails() {
        unsafe {
            let a = make_i64_array(&[2, 3, 6]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(!List_all(a, &c));
            mvl_array_drop(a);
        }
    }

    #[test]
    fn list_all_empty() {
        unsafe {
            let a = make_i64_array(&[]);
            let c = make_closure(pred_is_even as *const (), std::ptr::null());
            assert!(List_all(a, &c)); // vacuously true
            mvl_array_drop(a);
        }
    }
}
