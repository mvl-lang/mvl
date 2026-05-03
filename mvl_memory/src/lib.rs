//! MVL memory runtime — heap allocation and reference counting for collection types.
//!
//! Compiled as a `cdylib` and loaded by `lli` at runtime via `--load=libmvl_memory.{dylib,so}`.
//! All exported functions use C ABI (`extern "C"` + `#[no_mangle]`) so the LLVM IR
//! can call them with `declare` + `Linkage::External`.
//!
//! # Memory layout (per ADR-0016)
//!
//! | Type       | LLVM IR type                     |
//! |------------|----------------------------------|
//! | MvlString  | `{ ptr, i64 len, i64 cap, i64 rc }` |
//! | MvlArray   | `{ ptr, i64 len, i64 cap, i64 elem_size, i64 rc }` |
//! | MvlMap     | `{ ptr, i64 len, i64 cap, i64 rc }` |
//!
//! # Safety
//!
//! This crate is the single `unsafe` boundary in the MVL compiler pipeline.
//! All functions that cross the C ABI must use `unsafe`.  Internal helpers
//! use raw pointers directly.  Run with Miri (`cargo +nightly miri test`) to
//! catch UB in the test suite.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr;

// ── Internal size arithmetic helpers ─────────────────────────────────────────
//
// All size calculations before `mvl_alloc` calls use these helpers to abort
// on integer overflow instead of producing a truncated allocation that could
// lead to heap buffer overruns.

/// Checked size multiply — aborts on overflow.
#[inline(always)]
fn checked_mul_size(a: usize, b: usize) -> usize {
    a.checked_mul(b).unwrap_or_else(|| std::process::abort())
}

/// Checked size addition — aborts on overflow.
#[inline(always)]
fn checked_add_size(a: usize, b: usize) -> usize {
    a.checked_add(b).unwrap_or_else(|| std::process::abort())
}

// ── Allocation primitives ─────────────────────────────────────────────────────

/// Allocate `size` bytes, aligned to 8 bytes.  Aborts on allocation failure.
///
/// # Safety
/// Caller must free with `mvl_free` using the same `size`.
#[no_mangle]
pub unsafe extern "C" fn mvl_alloc(size: usize) -> *mut u8 {
    if size == 0 {
        return ptr::NonNull::dangling().as_ptr();
    }
    let Ok(layout) = Layout::from_size_align(size, 8) else {
        std::process::abort();
    };
    let ptr = alloc(layout);
    if ptr.is_null() {
        mvl_panic(b"mvl_alloc: out of memory\0".as_ptr().cast());
    }
    ptr
}

/// Free memory allocated by `mvl_alloc`.
///
/// # Safety
/// `ptr` must have been returned by `mvl_alloc(size)` and must not have been
/// freed already.  Passing `size = 0` with a dangling pointer is a no-op.
#[no_mangle]
pub unsafe extern "C" fn mvl_free(ptr: *mut u8, size: usize) {
    if size == 0 || ptr.is_null() {
        return;
    }
    let Ok(layout) = Layout::from_size_align(size, 8) else {
        std::process::abort();
    };
    dealloc(ptr, layout);
}

/// Print `msg` (null-terminated C string) to stderr and abort.
#[no_mangle]
pub unsafe extern "C" fn mvl_panic(msg: *const u8) {
    use std::io::Write;
    let msg_str = if msg.is_null() {
        "mvl_panic: (null message)"
    } else {
        let len = libc::strlen(msg.cast());
        std::str::from_utf8(std::slice::from_raw_parts(msg, len)).unwrap_or("(invalid utf8)")
    };
    let _ = std::io::stderr().write_fmt(format_args!("mvl panic: {msg_str}\n"));
    std::process::abort();
}

// ── MvlString ────────────────────────────────────────────────────────────────
//
// Layout (matches LLVM IR `%MvlString = type { ptr, i64, i64, i64 }`):
//   offset  0: *mut u8  ptr       — heap bytes (null-terminated for printf)
//   offset  8: u64      len       — byte length (excluding null terminator)
//   offset 16: u64      cap       — allocated capacity (>= len + 1)
//   offset 24: u64      refcount  — reference count; 1 on construction

#[repr(C)]
pub struct MvlString {
    pub ptr: *mut u8,
    pub len: u64,
    pub cap: u64,
    pub refcount: u64,
}

impl MvlString {
    unsafe fn alloc_bytes(cap: usize) -> *mut u8 {
        mvl_alloc(cap)
    }

    unsafe fn free_bytes(ptr: *mut u8, cap: usize) {
        mvl_free(ptr, cap);
    }
}

/// Create a new `MvlString` from `len` bytes at `bytes` (does not need null terminator).
/// Returns a heap pointer with `refcount = 1`.
///
/// # Safety
/// `bytes` must be a valid pointer to at least `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn mvl_string_new(bytes: *const u8, len: usize) -> *mut MvlString {
    let cap = checked_add_size(len, 1); // +1 for null terminator
    let data = MvlString::alloc_bytes(cap);
    if len > 0 {
        ptr::copy_nonoverlapping(bytes, data, len);
    }
    *data.add(len) = 0; // null terminator
    let s = mvl_alloc(std::mem::size_of::<MvlString>()) as *mut MvlString;
    s.write(MvlString {
        ptr: data,
        len: len as u64,
        cap: cap as u64,
        refcount: 1,
    });
    s
}

/// Increment refcount and return the same pointer.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.
#[no_mangle]
pub unsafe extern "C" fn mvl_string_clone(s: *mut MvlString) -> *mut MvlString {
    if s.is_null() {
        return s;
    }
    (*s).refcount = (*s)
        .refcount
        .checked_add(1)
        .unwrap_or_else(|| std::process::abort());
    s
}

/// Decrement refcount; free the string when it reaches zero.
///
/// # Safety
/// `s` must be a valid non-null `MvlString` pointer.  Must not be used after drop.
#[no_mangle]
pub unsafe extern "C" fn mvl_string_drop(s: *mut MvlString) {
    if s.is_null() {
        return;
    }
    (*s).refcount = (*s)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*s).refcount == 0 {
        MvlString::free_bytes((*s).ptr, (*s).cap as usize);
        mvl_free(s as *mut u8, std::mem::size_of::<MvlString>());
    }
}

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
    let data = MvlString::alloc_bytes(cap);
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

// ── MvlArray ─────────────────────────────────────────────────────────────────
//
// Layout (matches LLVM IR `%MvlArray = type { ptr, i64, i64, i64, i64 }`):
//   offset  0: *mut u8  ptr        — heap element data
//   offset  8: u64      len        — number of live elements
//   offset 16: u64      cap        — capacity in elements
//   offset 24: u64      elem_size  — bytes per element
//   offset 32: u64      refcount

#[repr(C)]
pub struct MvlArray {
    pub ptr: *mut u8,
    pub len: u64,
    pub cap: u64,
    pub elem_size: u64,
    pub refcount: u64,
}

const ARRAY_INITIAL_CAP: u64 = 4;

/// Create a new `MvlArray` with the given element size and initial capacity.
/// Returns a heap pointer with `refcount = 1`.
///
/// # Safety
/// Always safe to call; `elem_size` must be > 0.
#[no_mangle]
pub unsafe extern "C" fn mvl_array_new(elem_size: usize, initial_cap: usize) -> *mut MvlArray {
    let cap = initial_cap.max(ARRAY_INITIAL_CAP as usize);
    let data = if cap > 0 && elem_size > 0 {
        mvl_alloc(checked_mul_size(cap, elem_size))
    } else {
        ptr::null_mut()
    };
    let a = mvl_alloc(std::mem::size_of::<MvlArray>()) as *mut MvlArray;
    a.write(MvlArray {
        ptr: data,
        len: 0,
        cap: cap as u64,
        elem_size: elem_size as u64,
        refcount: 1,
    });
    a
}

/// Increment refcount and return the same pointer.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.
#[no_mangle]
pub unsafe extern "C" fn mvl_array_clone(a: *mut MvlArray) -> *mut MvlArray {
    if a.is_null() {
        return a;
    }
    (*a).refcount = (*a)
        .refcount
        .checked_add(1)
        .unwrap_or_else(|| std::process::abort());
    a
}

/// Decrement refcount; free when it reaches zero.
///
/// # Safety
/// `a` must be a valid non-null `MvlArray` pointer.  Must not be used after drop.
#[no_mangle]
pub unsafe extern "C" fn mvl_array_drop(a: *mut MvlArray) {
    if a.is_null() {
        return;
    }
    (*a).refcount = (*a)
        .refcount
        .checked_sub(1)
        .unwrap_or_else(|| std::process::abort());
    if (*a).refcount == 0 {
        let data_size = checked_mul_size((*a).cap as usize, (*a).elem_size as usize);
        if data_size > 0 && !(*a).ptr.is_null() {
            mvl_free((*a).ptr, data_size);
        }
        mvl_free(a as *mut u8, std::mem::size_of::<MvlArray>());
    }
}

/// Append a copy of the `elem_size` bytes at `elem` to the array.
/// Grows capacity by 2x if needed.
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
        // Grow 2x
        let old_cap = (*a).cap as usize;
        let new_cap = checked_mul_size(old_cap, 2).max(ARRAY_INITIAL_CAP as usize);
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

// ── MvlMap ───────────────────────────────────────────────────────────────────
//
// Open-addressing hash map with byte-key lookup.
// Layout (matches LLVM IR `%MvlMap = type { ptr, i64, i64, i64 }`):
//   offset  0: *mut u8  slots     — pointer to slot array
//   offset  8: u64      len       — number of live entries
//   offset 16: u64      cap       — slot count (power of two)
//   offset 24: u64      refcount
//
// Each slot is `MvlMapSlot { occupied: u8, key_ptr: *mut u8, key_len: u64, val_ptr: *mut u8 }`.
// Keys and values are stored as heap copies.

#[repr(C)]
pub struct MvlMapSlot {
    pub occupied: u8,
    pub key_ptr: *mut u8,
    pub key_len: u64,
    pub val_ptr: *mut u8,
    pub val_len: u64,
}

#[repr(C)]
pub struct MvlMap {
    pub slots: *mut MvlMapSlot,
    pub len: u64,
    pub cap: u64,
    pub refcount: u64,
}

const MAP_INITIAL_CAP: u64 = 8;
const SLOT_SIZE: usize = std::mem::size_of::<MvlMapSlot>();

unsafe fn fnv1a(key: *const u8, len: usize) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for i in 0..len {
        hash ^= *key.add(i) as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

unsafe fn map_find_slot(slots: *mut MvlMapSlot, cap: u64, key: *const u8, key_len: usize) -> usize {
    let h = fnv1a(key, key_len);
    let mut idx = (h % cap) as usize;
    loop {
        let slot = &*slots.add(idx);
        if slot.occupied == 0 {
            return idx; // empty slot — insertion point
        }
        if slot.key_len == key_len as u64
            && libc::memcmp(slot.key_ptr.cast(), key.cast(), key_len) == 0
        {
            return idx; // found
        }
        idx = (idx + 1) % cap as usize;
    }
}

/// Create a new empty `MvlMap` with the given initial slot capacity.
/// Returns a heap pointer with `refcount = 1`.
///
/// # Safety
/// Always safe to call.
#[no_mangle]
pub unsafe extern "C" fn mvl_map_new(initial_cap: usize) -> *mut MvlMap {
    let cap = initial_cap
        .next_power_of_two()
        .max(MAP_INITIAL_CAP as usize);
    let slot_bytes = checked_mul_size(cap, SLOT_SIZE);
    let slots = mvl_alloc(slot_bytes) as *mut MvlMapSlot;
    // Zero-initialize all slots (occupied = 0).
    ptr::write_bytes(slots as *mut u8, 0, slot_bytes);
    let m = mvl_alloc(std::mem::size_of::<MvlMap>()) as *mut MvlMap;
    m.write(MvlMap {
        slots,
        len: 0,
        cap: cap as u64,
        refcount: 1,
    });
    m
}

/// Increment refcount and return the same pointer.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.
#[no_mangle]
pub unsafe extern "C" fn mvl_map_clone(m: *mut MvlMap) -> *mut MvlMap {
    if m.is_null() {
        return m;
    }
    (*m).refcount = (*m)
        .refcount
        .checked_add(1)
        .unwrap_or_else(|| std::process::abort());
    m
}

/// Decrement refcount; free all memory when it reaches zero.
///
/// # Safety
/// `m` must be a valid non-null `MvlMap` pointer.  Must not be used after drop.
#[no_mangle]
pub unsafe extern "C" fn mvl_map_drop(m: *mut MvlMap) {
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
                mvl_free(slot.key_ptr, slot.key_len as usize);
                mvl_free(slot.val_ptr, slot.val_len as usize);
            }
        }
        mvl_free((*m).slots as *mut u8, checked_mul_size(cap, SLOT_SIZE));
        mvl_free(m as *mut u8, std::mem::size_of::<MvlMap>());
    }
}

/// Insert `(key[0..key_len], val[0..val_len])` into the map.
/// Replaces the existing value if the key already exists.
/// Grows by 2x if load factor exceeds 50%.
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
        let new_cap = checked_mul_size(old_cap, 2).max(MAP_INITIAL_CAP as usize);
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

/// Return a pointer to the value bytes for the given key, or null if not found.
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── mvl_string ────────────────────────────────────────────────────────────

    #[test]
    fn string_new_and_len() {
        unsafe {
            let s = mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!(mvl_string_len(s), 5);
            assert_eq!(*mvl_string_ptr(s).add(5), 0); // null-terminated
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_empty() {
        unsafe {
            let s = mvl_string_new(b"".as_ptr(), 0);
            assert_eq!(mvl_string_len(s), 0);
            assert_eq!(*mvl_string_ptr(s), 0);
            mvl_string_drop(s);
        }
    }

    #[test]
    fn string_clone_and_drop() {
        unsafe {
            let s = mvl_string_new(b"world".as_ptr(), 5);
            let s2 = mvl_string_clone(s);
            assert_eq!((*s).refcount, 2);
            mvl_string_drop(s2);
            assert_eq!((*s).refcount, 1);
            mvl_string_drop(s);
            // s is freed — no further access
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
            mvl_string_drop(a);
            mvl_string_drop(b);
            mvl_string_drop(c);
        }
    }

    // ── mvl_array ─────────────────────────────────────────────────────────────

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
    fn array_clone_and_drop() {
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

    // ── mvl_map ───────────────────────────────────────────────────────────────

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
            // Missing key returns null.
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
    fn map_clone_and_drop() {
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
