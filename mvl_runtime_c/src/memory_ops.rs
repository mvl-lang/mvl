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

use mvl_memory::{mvl_alloc, mvl_free, MvlArray, MvlMap, MvlMapSlot, MvlString};

// ── Internal helpers ───────────────────────────────────────────────────────────

#[inline(always)]
fn checked_mul_size(a: usize, b: usize) -> usize {
    a.checked_mul(b).unwrap_or_else(|| std::process::abort())
}

#[inline(always)]
fn checked_add_size(a: usize, b: usize) -> usize {
    a.checked_add(b).unwrap_or_else(|| std::process::abort())
}

/// Growth cap used in `mvl_array_push` to mirror `mvl_memory::ARRAY_INITIAL_CAP`.
const ARRAY_INITIAL_CAP: usize = 4;

/// Minimum slot count for map growth to mirror `mvl_memory::MAP_INITIAL_CAP`.
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
unsafe fn map_find_slot(slots: *mut MvlMapSlot, cap: u64, key: *const u8, key_len: usize) -> usize {
    let h = fnv1a(key, key_len);
    let mut idx = (h % cap) as usize;
    loop {
        let slot = &*slots.add(idx);
        if slot.occupied == 0 {
            return idx; // empty — insertion point
        }
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
pub unsafe extern "C" fn mvl_string_ptr(s: *const MvlString) -> *const u8 {
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

// ── MvlMap operations ──────────────────────────────────────────────────────────

/// Insert `(key[0..key_len], val[0..val_len])` into the map.
/// Replaces the existing value if the key already exists.
/// Grows 2× if load factor exceeds 50%.
///
/// # Safety
/// `m`, `key`, and `val` must be valid non-null pointers.
#[no_mangle]
pub unsafe extern "C" fn mvl_map_insert(
    m: *mut MvlMap,
    key: *const u8,
    key_len: usize,
    val: *const u8,
    val_len: usize,
) {
    if m.is_null() || key.is_null() {
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
            if old.occupied != 0 {
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
        mvl_free(slot.val_ptr, slot.val_len as usize);
        let new_val = mvl_alloc(val_len);
        ptr::copy_nonoverlapping(val, new_val, val_len);
        slot.val_ptr = new_val;
        slot.val_len = val_len as u64;
    } else {
        // New entry.
        let kp = mvl_alloc(key_len);
        ptr::copy_nonoverlapping(key, kp, key_len);
        let vp = mvl_alloc(val_len);
        ptr::copy_nonoverlapping(val, vp, val_len);
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
#[no_mangle]
pub unsafe extern "C" fn mvl_map_get(
    m: *const MvlMap,
    key: *const u8,
    key_len: usize,
) -> *const u8 {
    if m.is_null() || key.is_null() {
        return ptr::null();
    }
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
#[no_mangle]
pub unsafe extern "C" fn mvl_map_len(m: *const MvlMap) -> u64 {
    if m.is_null() {
        return 0;
    }
    (*m).len
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mvl_memory::{mvl_array_clone, mvl_array_drop, mvl_array_new};
    use mvl_memory::{mvl_map_clone, mvl_map_drop, mvl_map_new};
    use mvl_memory::{mvl_string_clone, mvl_string_drop, mvl_string_new};

    // ── string operations ──────────────────────────────────────────────────────

    #[test]
    fn string_len_and_ptr() {
        unsafe {
            let s = mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!(mvl_string_len(s), 5);
            assert_eq!(*mvl_string_ptr(s).add(5), 0); // null-terminated
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_empty_len() {
        unsafe {
            let s = mvl_string_new(b"".as_ptr(), 0);
            assert_eq!(mvl_string_len(s), 0);
            assert_eq!(*mvl_string_ptr(s), 0);
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
            let slice = std::slice::from_raw_parts(mvl_string_ptr(c), 6);
            assert_eq!(slice, b"foobar");
            assert_eq!(*mvl_string_ptr(c).add(6), 0);
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
            let _ = mvl_string_clone(a); // bump refcount to test equality of same pointer
            assert_eq!(mvl_string_eq(a, a), 1); // same pointer → eq
            mvl_string_drop(a); // drop the clone
            mvl_string_drop(a);
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
}
