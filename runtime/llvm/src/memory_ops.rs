// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Heap-collection operations for the MVL LLVM backend.
//!
//! This module provides the `extern "C"` operation functions for `MvlString`,
//! `MvlArray`, and `MvlMap` that were previously in `mvl_memory`.
//!
//! # Architecture (ADR-0016, #490)
//!
//! `mvl_memory` is responsible for **type definitions + lifecycle** (new/clone/drop).
//! This module is responsible for **operations** (len, ptr, concat, get, push, insert, …).
//!
//! Both sets of symbols are exported from `libmvl_runtime_c.{dylib,so}`, which
//! the LLVM backend loads alongside `libmvl_memory.{dylib,so}`.

use std::ptr;

use crate::memory::{
    mvl_alloc, mvl_array_new, mvl_free, mvl_string_drop, mvl_string_new, MvlArray, MvlMap,
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
pub unsafe extern "C" fn mvl_format(
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
                let elem_ptr = mvl_array_get(values, val_idx) as *const *mut MvlString;
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

    mvl_string_new(result.as_ptr(), result.len())
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
    mvl_string_new(s.as_ptr(), s.len())
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

/// Growth cap used in `mvl_array_push` to mirror `crate::memory::ARRAY_INITIAL_CAP`.
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
pub unsafe extern "C" fn mvl_string_len(s: *const MvlString) -> u64 {
    if s.is_null() {
        return 0;
    }
    (*s).len
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
pub unsafe extern "C" fn mvl_string_concat(
    a: *const MvlString,
    b: *const MvlString,
) -> *mut MvlString {
    let la = if a.is_null() { 0 } else { (*a).len as usize };
    let lb = if b.is_null() { 0 } else { (*b).len as usize };
    let total = checked_add_size(la, lb);
    let cap = checked_add_size(total, 1);
    let data = mvl_alloc(cap);
    if la > 0 {
        ptr::copy_nonoverlapping((*a).ptr, data, la);
    }
    if lb > 0 {
        ptr::copy_nonoverlapping((*b).ptr, data.add(la), lb);
    }
    *data.add(total) = 0;
    let s = mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
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
pub unsafe extern "C" fn mvl_string_eq(a: *const MvlString, b: *const MvlString) -> i32 {
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
    let data = mvl_alloc(cap);
    for i in 0..len {
        *data.add(i) = (*(*s).ptr.add(i) as char).to_ascii_lowercase() as u8;
    }
    *data.add(len) = 0;
    let out = mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
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
    let data = mvl_alloc(cap);
    for i in 0..len {
        *data.add(i) = (*(*s).ptr.add(i) as char).to_ascii_uppercase() as u8;
    }
    *data.add(len) = 0;
    let out = mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
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
    let arr = mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    let text = as_str(s);
    let delimiter = as_str(sep);
    for part in text.split(delimiter) {
        let part_s = str_to_mvl(part);
        mvl_array_push(arr, (&part_s as *const *mut MvlString).cast());
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

/// Return a one-character `MvlString` at char-index `i`, or `""` if out of range.
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_char_at(s: *const MvlString, i: i64) -> *mut MvlString {
    let text = as_str(s);
    if i < 0 {
        return str_to_mvl("");
    }
    match text.chars().nth(i as usize) {
        Some(ch) => {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            str_to_mvl(encoded)
        }
        None => str_to_mvl(""),
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

/// Return the raw byte at byte-index `i`, or 0 if out of range.
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
#[no_mangle]
pub unsafe extern "C" fn _mvl_str_byte_at(s: *const MvlString, i: i64) -> i64 {
    if s.is_null() || i < 0 {
        return 0;
    }
    let idx = i as usize;
    let len = (*s).len as usize;
    if idx >= len || (*s).ptr.is_null() {
        return 0;
    }
    *(*s).ptr.add(idx) as i64
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
pub unsafe extern "C" fn mvl_array_push(a: *mut MvlArray, elem: *const u8) {
    if a.is_null() || elem.is_null() {
        return;
    }
    let es = (*a).elem_size as usize;
    if (*a).len >= (*a).cap {
        // Grow 2×
        let old_cap = (*a).cap as usize;
        let new_cap = checked_mul_size(old_cap, 2).max(ARRAY_INITIAL_CAP);
        let new_data = mvl_alloc(checked_mul_size(new_cap, es));
        if old_cap > 0 && !(*a).ptr.is_null() {
            let old_bytes = checked_mul_size(old_cap, es);
            ptr::copy_nonoverlapping((*a).ptr, new_data, old_bytes);
            mvl_free((*a).ptr, old_bytes);
        }
        (*a).ptr = new_data;
        (*a).cap = new_cap as u64;
    }
    let dest = (*a).ptr.add((*a).len as usize * es);
    ptr::copy_nonoverlapping(elem, dest, es);
    (*a).len += 1;
}

/// Return a pointer to element at `idx`.  Returns null if out of bounds.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn mvl_array_get(a: *const MvlArray, idx: usize) -> *const u8 {
    if a.is_null() || idx >= (*a).len as usize {
        return ptr::null();
    }
    (*a).ptr.add(idx * (*a).elem_size as usize)
}

/// Return the number of elements in the array.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn mvl_array_len(a: *const MvlArray) -> u64 {
    if a.is_null() {
        return 0;
    }
    (*a).len
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
        let dummy = mvl_array_new(8, 0);
        return dummy;
    }
    let es = (*arr).elem_size as usize;
    let len = (*arr).len as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    let count = hi.saturating_sub(lo);
    let out = mvl_array_new(es, count.max(1));
    for i in lo..hi {
        let src = (*arr).ptr.add(i * es);
        mvl_array_push(out, src);
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
        (true, true) => return mvl_array_new(8, 0),
        (false, true) => ((*a).elem_size as usize, (*a).len as usize, 0usize),
        (true, false) => ((*b).elem_size as usize, 0usize, (*b).len as usize),
        (false, false) => (
            (*a).elem_size as usize,
            (*a).len as usize,
            (*b).len as usize,
        ),
    };
    let out = mvl_array_new(es, (la + lb).max(1));
    for i in 0..la {
        let src = (*a).ptr.add(i * es);
        mvl_array_push(out, src);
    }
    for i in 0..lb {
        let src = (*b).ptr.add(i * es);
        mvl_array_push(out, src);
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
        let new_slots = mvl_alloc(new_slot_bytes) as *mut MvlMapSlot;
        ptr::write_bytes(new_slots as *mut u8, 0, new_slot_bytes);
        for i in 0..old_cap {
            let old = &*(*m).slots.add(i);
            if old.occupied == 1 {
                let idx =
                    map_find_slot(new_slots, new_cap as u64, old.key_ptr, old.key_len as usize);
                ptr::copy_nonoverlapping(old, new_slots.add(idx), 1);
            }
        }
        mvl_free((*m).slots as *mut u8, checked_mul_size(old_cap, SLOT_SIZE));
        (*m).slots = new_slots;
        (*m).cap = new_cap as u64;
    }

    let idx = map_find_slot((*m).slots, (*m).cap, key, key_len);
    let slot = &mut *(*m).slots.add(idx);
    if slot.occupied != 0 {
        // Replace existing value.
        if slot.val_len > 0 {
            mvl_free(slot.val_ptr, slot.val_len as usize);
        }
        if val_len > 0 {
            let new_val = mvl_alloc(val_len);
            ptr::copy_nonoverlapping(val, new_val, val_len);
            slot.val_ptr = new_val;
        } else {
            slot.val_ptr = ptr::null_mut();
        }
        slot.val_len = val_len as u64;
    } else {
        // New entry.
        let kp = mvl_alloc(key_len);
        ptr::copy_nonoverlapping(key, kp, key_len);
        let vp = if val_len > 0 {
            let p = mvl_alloc(val_len);
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
pub unsafe extern "C" fn mvl_string_chars(s: *const MvlString) -> *mut MvlArray {
    let arr = mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
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
        let char_s = mvl_string_new(encoded.as_ptr(), encoded.len());
        mvl_array_push(arr, (&char_s as *const *mut MvlString).cast());
    }
    arr
}

/// Return all keys in the map as a `MvlArray` of `*mut MvlString` pointers.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
pub(crate) unsafe fn mvl_map_keys(m: *const MvlMap) -> *mut MvlArray {
    let arr = mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    if m.is_null() || (*m).cap == 0 {
        return arr;
    }
    let cap = (*m).cap as usize;
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 {
            let key_s = mvl_string_new(slot.key_ptr, slot.key_len as usize);
            mvl_array_push(arr, (&key_s as *const *mut MvlString).cast());
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
    let arr = mvl_array_new(std::mem::size_of::<*mut MvlString>(), 0);
    if m.is_null() || (*m).cap == 0 {
        return arr;
    }
    let cap = (*m).cap as usize;
    for i in 0..cap {
        let slot = &*(*m).slots.add(i);
        if slot.occupied == 1 {
            let val_s = mvl_string_new(slot.val_ptr, slot.val_len as usize);
            mvl_array_push(arr, (&val_s as *const *mut MvlString).cast());
        }
    }
    arr
}

/// Drop an array whose elements are owned `*mut MvlString` pointers.
///
/// Decrements the array's refcount.  When refcount reaches zero, each element
/// string is freed via `mvl_string_drop` before the array itself is freed.
/// Use this instead of `mvl_array_drop` for arrays returned by `mvl_string_chars`
/// or `mvl_map_keys`, which own their element strings.
///
/// # Safety
/// `arr` must be a valid non-null `MvlArray` pointer whose elements are
/// `*mut MvlString` pointers produced by `mvl_string_new`.
#[no_mangle]
pub unsafe extern "C" fn mvl_string_ptr_array_drop(arr: *mut MvlArray) {
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
                mvl_string_drop(s);
            }
        }
        // Free the array buffer and struct (same logic as mvl_array_drop).
        let data_size = ((*arr).cap as usize)
            .checked_mul(es)
            .unwrap_or_else(|| std::process::abort());
        if data_size > 0 && !(*arr).ptr.is_null() {
            mvl_free((*arr).ptr, data_size);
        }
        mvl_free(arr as *mut u8, std::mem::size_of::<MvlArray>());
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
        mvl_free(slot.key_ptr, slot.key_len as usize);
    }
    if slot.val_len > 0 && !slot.val_ptr.is_null() {
        mvl_free(slot.val_ptr, slot.val_len as usize);
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
            *err_out = mvl_string_new(msg.as_ptr(), msg.len());
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
            *err_out = mvl_string_new(msg.as_ptr(), msg.len());
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
/// Only supports 8-byte (i64) elements — all MVL scalar types use `elem_size=8`.
///
/// # Safety
/// `list` must be a valid `MvlArray*` with `elem_size == 8`.  `closure` must
/// point to a valid `MvlClosure` whose `fn_ptr` has signature `fn(ptr, i64) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn List_filter(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlArray {
    if list.is_null() {
        return mvl_array_new(8, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    debug_assert_eq!(es, 8, "List_filter: only 8-byte (i64) elements supported");
    let out = mvl_array_new(es, len.max(1));
    let pred: unsafe extern "C" fn(*const u8, i64) -> bool = std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let elem_val = *(elem_ptr as *const i64);
        if pred(env, elem_val) {
            mvl_array_push(out, elem_ptr);
        }
    }
    out
}

/// `List_map(list, closure)` — transform each element via `closure(elem)`.
///
/// Only supports i64→i64 mappings (output `elem_size` == input `elem_size`).
///
/// # Safety
/// `list` must be a valid `MvlArray*` with `elem_size == 8`.  `closure` must
/// point to a valid `MvlClosure` whose `fn_ptr` has signature `fn(ptr, i64) -> i64`.
#[no_mangle]
pub unsafe extern "C" fn List_map(
    list: *mut MvlArray,
    closure: *const MvlClosure,
) -> *mut MvlArray {
    if list.is_null() {
        return mvl_array_new(8, 1);
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    debug_assert_eq!(es, 8, "List_map: only 8-byte (i64) elements supported");
    let out = mvl_array_new(es, len.max(1));
    let map_fn: unsafe extern "C" fn(*const u8, i64) -> i64 =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let elem_val = *(elem_ptr as *const i64);
        let result = map_fn(env, elem_val);
        mvl_array_push(out, (&result as *const i64) as *const u8);
    }
    out
}

/// `List_fold(list, acc_ptr, closure)` — reduce list with accumulator.
///
/// `acc_ptr` points to the initial accumulator value (stack-allocated by the
/// caller).  The closure has signature `fn(env, acc, elem) -> acc`.  The final
/// accumulator is written back to `acc_ptr`, which is also returned.
///
/// Only supports 8-byte (i64) elements and accumulator.
///
/// # Safety
/// `list` must be a valid `MvlArray*` with `elem_size == 8`.  `acc_ptr` must
/// be a writable pointer to at least 8 bytes.  `closure` must point to a valid
/// `MvlClosure` whose `fn_ptr` has signature `fn(ptr, i64, i64) -> i64`.
#[no_mangle]
pub unsafe extern "C" fn List_fold(
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
    debug_assert_eq!(es, 8, "List_fold: only 8-byte (i64) elements supported");
    let fold_fn: unsafe extern "C" fn(*const u8, i64, i64) -> i64 =
        std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    let mut acc = *(acc_ptr as *const i64);
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let elem_val = *(elem_ptr as *const i64);
        acc = fold_fn(env, acc, elem_val);
    }
    *(acc_ptr as *mut i64) = acc;
    acc_ptr
}

/// `List_any(list, closure)` — return true if any element satisfies predicate.
///
/// Only supports 8-byte (i64) elements.
///
/// # Safety
/// `list` must be a valid `MvlArray*` with `elem_size == 8`.  `closure` must
/// point to a valid `MvlClosure` whose `fn_ptr` has signature `fn(ptr, i64) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn List_any(list: *mut MvlArray, closure: *const MvlClosure) -> bool {
    if list.is_null() {
        return false;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    debug_assert_eq!(es, 8, "List_any: only 8-byte (i64) elements supported");
    let pred: unsafe extern "C" fn(*const u8, i64) -> bool = std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let elem_val = *(elem_ptr as *const i64);
        if pred(env, elem_val) {
            return true;
        }
    }
    false
}

/// `List_all(list, closure)` — return true if all elements satisfy predicate.
///
/// Only supports 8-byte (i64) elements.
///
/// # Safety
/// `list` must be a valid `MvlArray*` with `elem_size == 8`.  `closure` must
/// point to a valid `MvlClosure` whose `fn_ptr` has signature `fn(ptr, i64) -> i1`.
#[no_mangle]
pub unsafe extern "C" fn List_all(list: *mut MvlArray, closure: *const MvlClosure) -> bool {
    if list.is_null() {
        return true;
    }
    if closure.is_null() || (*closure).fn_ptr.is_null() {
        std::process::abort();
    }
    let len = (*list).len as usize;
    let es = (*list).elem_size as usize;
    debug_assert_eq!(es, 8, "List_all: only 8-byte (i64) elements supported");
    let pred: unsafe extern "C" fn(*const u8, i64) -> bool = std::mem::transmute((*closure).fn_ptr);
    let env = (*closure).env_ptr as *const u8;
    for i in 0..len {
        let elem_ptr = (*list).ptr.add(i * es);
        let elem_val = *(elem_ptr as *const i64);
        if !pred(env, elem_val) {
            return false;
        }
    }
    true
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{mvl_array_clone, mvl_array_drop, mvl_array_new};
    use crate::memory::{mvl_map_clone, mvl_map_drop, mvl_map_new};
    use crate::memory::{mvl_string_clone, mvl_string_drop, mvl_string_new};

    // ── string operations ──────────────────────────────────────────────────────

    #[test]
    fn string_len_and_ptr() {
        unsafe {
            let s = mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!(mvl_string_len(s), 5);
            assert_eq!(*_mvl_string_ptr(s).add(5), 0); // null-terminated
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_empty_len() {
        unsafe {
            let s = mvl_string_new(b"".as_ptr(), 0);
            assert_eq!(mvl_string_len(s), 0);
            assert_eq!(*_mvl_string_ptr(s), 0);
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_concat() {
        unsafe {
            let a = mvl_string_new(b"foo".as_ptr(), 3);
            let b = mvl_string_new(b"bar".as_ptr(), 3);
            let c = mvl_string_concat(a, b);
            assert_eq!(mvl_string_len(c), 6);
            let slice = std::slice::from_raw_parts(_mvl_string_ptr(c), 6);
            assert_eq!(slice, b"foobar");
            assert_eq!(*_mvl_string_ptr(c).add(6), 0);
            mvl_string_drop(a);
            mvl_string_drop(b);
            mvl_string_drop(c);
        }
    }

    #[test]
    fn string_eq() {
        unsafe {
            let a = mvl_string_new(b"abc".as_ptr(), 3);
            let b = mvl_string_new(b"abc".as_ptr(), 3);
            let c = mvl_string_new(b"xyz".as_ptr(), 3);
            assert_eq!(mvl_string_eq(a, b), 1);
            assert_eq!(mvl_string_eq(a, c), 0);
            let _ = mvl_string_clone(a); // refcount → 2 (same ptr; raw ptr, no Rust Drop)
            assert_eq!(mvl_string_eq(a, a), 1); // pointer-equality short-circuit
            mvl_string_drop(a); // refcount → 1
            mvl_string_drop(a); // refcount → 0, freed
            mvl_string_drop(b);
            mvl_string_drop(c);
        }
    }

    // ── array operations ───────────────────────────────────────────────────────

    #[test]
    fn array_push_get_len() {
        unsafe {
            let a = mvl_array_new(8, 0); // i64 elements
            assert_eq!(mvl_array_len(a), 0);
            let v1: i64 = 42;
            let v2: i64 = 99;
            mvl_array_push(a, (&v1 as *const i64).cast());
            mvl_array_push(a, (&v2 as *const i64).cast());
            assert_eq!(mvl_array_len(a), 2);
            let p1 = mvl_array_get(a, 0) as *const i64;
            let p2 = mvl_array_get(a, 1) as *const i64;
            assert_eq!(*p1, 42);
            assert_eq!(*p2, 99);
            assert!(mvl_array_get(a, 2).is_null());
            mvl_array_drop(a);
        }
    }

    #[test]
    fn array_grows_past_initial_cap() {
        unsafe {
            let a = mvl_array_new(8, 2);
            for i in 0i64..16 {
                mvl_array_push(a, (&i as *const i64).cast());
            }
            assert_eq!(mvl_array_len(a), 16);
            for i in 0i64..16 {
                let p = mvl_array_get(a, i as usize) as *const i64;
                assert_eq!(*p, i);
            }
            mvl_array_drop(a);
        }
    }

    #[test]
    fn array_clone_refcount() {
        unsafe {
            let a = mvl_array_new(8, 0);
            let v: i64 = 7;
            mvl_array_push(a, (&v as *const i64).cast());
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let m = mvl_map_new(0);
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
            let s = mvl_string_new(b"abc".as_ptr(), 3);
            let arr = mvl_string_chars(s);
            assert_eq!(mvl_array_len(arr), 3);
            let expected = [b"a" as &[u8], b"b", b"c"];
            for (i, exp) in expected.iter().enumerate() {
                let elem_ptr = mvl_array_get(arr, i) as *const *mut MvlString;
                let cs = *elem_ptr;
                assert_eq!(mvl_string_len(cs), 1);
                let slice = std::slice::from_raw_parts(_mvl_string_ptr(cs), 1);
                assert_eq!(slice, *exp);
            }
            mvl_string_ptr_array_drop(arr);
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_chars_empty() {
        unsafe {
            let s = mvl_string_new(b"".as_ptr(), 0);
            let arr = mvl_string_chars(s);
            assert_eq!(mvl_array_len(arr), 0);
            mvl_string_ptr_array_drop(arr);
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_chars_utf8_multibyte() {
        // "é" is 2 bytes in UTF-8 (0xC3 0xA9); should produce one char element.
        unsafe {
            let text = "aé"; // 3 bytes: 'a' + 0xC3 + 0xA9
            let s = mvl_string_new(text.as_ptr(), text.len());
            let arr = mvl_string_chars(s);
            assert_eq!(mvl_array_len(arr), 2, "expected 2 chars: 'a' and 'é'");
            // First char: 'a' (1 byte)
            let p0 = *(mvl_array_get(arr, 0) as *const *mut MvlString);
            assert_eq!(mvl_string_len(p0), 1);
            let s0 = std::slice::from_raw_parts(_mvl_string_ptr(p0), 1);
            assert_eq!(s0, b"a");
            // Second char: 'é' (2 bytes)
            let p1 = *(mvl_array_get(arr, 1) as *const *mut MvlString);
            assert_eq!(mvl_string_len(p1), 2);
            let s1 = std::slice::from_raw_parts(_mvl_string_ptr(p1), 2);
            assert_eq!(s1, "é".as_bytes());
            mvl_string_ptr_array_drop(arr);
            mvl_string_drop(s);
        }
    }

    // ── mvl_map_keys ──────────────────────────────────────────────────────────

    #[test]
    fn map_keys_basic() {
        unsafe {
            let m = mvl_map_new(0);
            let v: i64 = 0;
            mvl_map_insert(m, b"alpha".as_ptr(), 5, (&v as *const i64).cast(), 8);
            mvl_map_insert(m, b"beta".as_ptr(), 4, (&v as *const i64).cast(), 8);
            let arr = mvl_map_keys(m);
            assert_eq!(mvl_array_len(arr), 2);
            // Collect returned key strings into a set for order-independent check.
            let mut found = std::collections::HashSet::new();
            for i in 0..2usize {
                let elem_ptr = mvl_array_get(arr, i) as *const *mut MvlString;
                let ks = *elem_ptr;
                let len = mvl_string_len(ks) as usize;
                let slice = std::slice::from_raw_parts(_mvl_string_ptr(ks), len);
                found.insert(std::str::from_utf8(slice).unwrap().to_string());
            }
            assert!(found.contains("alpha"));
            assert!(found.contains("beta"));
            mvl_string_ptr_array_drop(arr);
            mvl_map_drop(m);
        }
    }

    #[test]
    fn map_keys_excludes_tombstones() {
        unsafe {
            let m = mvl_map_new(0);
            let v: i64 = 0;
            mvl_map_insert(m, b"a".as_ptr(), 1, (&v as *const i64).cast(), 8);
            mvl_map_insert(m, b"b".as_ptr(), 1, (&v as *const i64).cast(), 8);
            mvl_map_remove(m, b"a".as_ptr(), 1);
            let arr = mvl_map_keys(m);
            assert_eq!(
                mvl_array_len(arr),
                1,
                "tombstone key must not appear in keys()"
            );
            let ks = *(mvl_array_get(arr, 0) as *const *mut MvlString);
            let slice =
                std::slice::from_raw_parts(_mvl_string_ptr(ks), mvl_string_len(ks) as usize);
            assert_eq!(slice, b"b");
            mvl_string_ptr_array_drop(arr);
            mvl_map_drop(m);
        }
    }

    // ── HOF list functions (#1163) ────────────────────────────────────────────

    /// Helper: build an i64 array from a slice.
    unsafe fn make_i64_array(vals: &[i64]) -> *mut MvlArray {
        let a = mvl_array_new(8, vals.len().max(1));
        for v in vals {
            mvl_array_push(a, (v as *const i64).cast());
        }
        a
    }

    /// Helper: read all i64 elements from an array.
    unsafe fn read_i64_array(a: *mut MvlArray) -> Vec<i64> {
        let len = mvl_array_len(a) as usize;
        (0..len)
            .map(|i| *(mvl_array_get(a, i) as *const i64))
            .collect()
    }

    /// Simple predicate: is x even?
    unsafe extern "C" fn pred_is_even(_env: *const u8, x: i64) -> bool {
        x % 2 == 0
    }

    /// Simple map fn: double x.
    unsafe extern "C" fn map_double(_env: *const u8, x: i64) -> i64 {
        x * 2
    }

    /// Simple fold fn: add acc + x.
    unsafe extern "C" fn fold_add(_env: *const u8, acc: i64, x: i64) -> i64 {
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
            assert_eq!(mvl_array_len(out), 0);
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
            assert_eq!(mvl_array_len(out), 0);
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
            assert_eq!(mvl_array_len(out), 0);
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
