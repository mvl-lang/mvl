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

// Operations on MvlString (len, ptr, concat, eq) have moved to
// `mvl_runtime_c::memory_ops` (#490). Only lifecycle functions remain here.

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

// Operations on MvlArray (push, get, len) have moved to
// `mvl_runtime_c::memory_ops` (#490). Only lifecycle functions remain here.

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

// Operations on MvlMap (insert, get, len) have moved to
// `mvl_runtime_c::memory_ops` (#490). Only lifecycle functions remain here.

// ── Lifecycle tests (Miri-friendly: no operations, raw struct field access) ───
//
// Operation tests (concat, eq, push/get/len, insert/get/len) have moved to
// `mvl_runtime_c::memory_ops` where they can import lifecycle functions here.
// These tests cover only new/clone/drop and verify refcounting + field layout.

#[cfg(test)]
mod tests {
    use super::*;

    // ── MvlString lifecycle ────────────────────────────────────────────────────

    #[test]
    fn string_new_fields() {
        unsafe {
            let s = mvl_string_new(b"hello".as_ptr(), 5);
            assert_eq!((*s).len, 5);
            assert_eq!((*s).refcount, 1);
            assert!(!(*s).ptr.is_null());
            assert_eq!(*(*s).ptr.add(5), 0); // null-terminated
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
        }
    }

    #[test]
    fn string_new_empty() {
        unsafe {
            let s = mvl_string_new(b"".as_ptr(), 0);
            assert_eq!((*s).len, 0);
            assert_eq!((*s).refcount, 1);
            assert_eq!(*(*s).ptr, 0); // null terminator
            mvl_string_drop(s);
        }
    }

    // ── MvlArray lifecycle ─────────────────────────────────────────────────────

    #[test]
    fn array_new_fields() {
        unsafe {
            let a = mvl_array_new(8, 0); // i64 elements, default cap
            assert_eq!((*a).len, 0);
            assert_eq!((*a).elem_size, 8);
            assert_eq!((*a).refcount, 1);
            mvl_array_drop(a);
        }
    }

    #[test]
    fn array_clone_and_drop() {
        unsafe {
            let a = mvl_array_new(8, 4);
            let a2 = mvl_array_clone(a);
            assert_eq!((*a).refcount, 2);
            mvl_array_drop(a2);
            assert_eq!((*a).refcount, 1);
            mvl_array_drop(a);
        }
    }

    // ── MvlMap lifecycle ───────────────────────────────────────────────────────

    #[test]
    fn map_new_fields() {
        unsafe {
            let m = mvl_map_new(0);
            assert_eq!((*m).len, 0);
            assert!((*m).cap >= 8); // >= MAP_INITIAL_CAP
            assert_eq!((*m).refcount, 1);
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
