// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for set algebra operations on `MvlArray*`.
//!
//! All set elements are stored as i64 (8-byte slots). The functions operate on
//! `MvlArray*` pointers and return newly allocated `MvlArray*` results.
//!
//! # LLVM dispatch coverage
//!
//! The following functions are wired into the LLVM codegen dispatch table:
//!   - `_mvl_set_intersection` — (ptr, ptr) → ptr  (`PtrTwoPtrArgs`)
//!   - `_mvl_set_difference`   — (ptr, ptr) → ptr  (`PtrTwoPtrArgs`)
//!   - `_mvl_set_union`        — (ptr, ptr) → ptr  (`PtrTwoPtrArgs`)
//!   - `_mvl_set_contains_i64` — (ptr, i64) → i1  (Set[Int].contains)

use crate::memory::MvlArray;

/// Read the i64 element at index `i` from a `MvlArray*`.
#[allow(unsafe_code)]
unsafe fn array_get_i64(arr: *const MvlArray, i: i64) -> i64 {
    let ptr = crate::memory_ops::_mvl_array_get(arr, i);
    if ptr.is_null() {
        0
    } else {
        (ptr as *const i64).read()
    }
}

/// Push an i64 value into a `MvlArray*`.
#[allow(unsafe_code)]
unsafe fn array_push_i64(arr: *mut MvlArray, val: i64) {
    crate::memory_ops::_mvl_array_push(arr, (&val as *const i64).cast());
}

/// Check if `needle` exists in `arr` (linear scan).
#[allow(unsafe_code)]
unsafe fn array_contains_i64(arr: *const MvlArray, needle: i64) -> bool {
    let len = crate::memory_ops::_mvl_array_len(arr);
    for i in 0..len {
        if array_get_i64(arr, i) == needle {
            return true;
        }
    }
    false
}

/// Allocate a new empty `MvlArray*` with 8-byte element slots.
#[allow(unsafe_code)]
unsafe fn new_i64_array(cap: usize) -> *mut MvlArray {
    crate::memory::_mvl_array_new(std::mem::size_of::<i64>(), cap.max(1))
}

/// Return elements of `a` that are also in `b`.
///
/// `_mvl_set_intersection(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray`
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_set_intersection(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray {
    unsafe {
        let result = new_i64_array(4);
        let len_a = crate::memory_ops::_mvl_array_len(a as *const MvlArray);
        for i in 0..len_a {
            let elem = array_get_i64(a as *const MvlArray, i);
            if array_contains_i64(b as *const MvlArray, elem) {
                array_push_i64(result, elem);
            }
        }
        result
    }
}

/// Return elements of `a` that are NOT in `b`.
///
/// `_mvl_set_difference(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray`
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_set_difference(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray {
    unsafe {
        let result = new_i64_array(4);
        let len_a = crate::memory_ops::_mvl_array_len(a as *const MvlArray);
        for i in 0..len_a {
            let elem = array_get_i64(a as *const MvlArray, i);
            if !array_contains_i64(b as *const MvlArray, elem) {
                array_push_i64(result, elem);
            }
        }
        result
    }
}

/// True iff `needle` is present in the i64-element set `arr` (linear scan).
///
/// `_mvl_set_contains_i64(arr: *const MvlArray, needle: i64) -> bool`
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_set_contains_i64(arr: *const MvlArray, needle: i64) -> bool {
    unsafe { array_contains_i64(arr, needle) }
}

/// Return all elements of `a` plus elements of `b` not already in `a`.
///
/// `_mvl_set_union(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray`
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_set_union(a: *mut MvlArray, b: *mut MvlArray) -> *mut MvlArray {
    unsafe {
        let result = new_i64_array(4);
        let len_a = crate::memory_ops::_mvl_array_len(a as *const MvlArray);
        let len_b = crate::memory_ops::_mvl_array_len(b as *const MvlArray);
        // Copy all of a
        for i in 0..len_a {
            let elem = array_get_i64(a as *const MvlArray, i);
            array_push_i64(result, elem);
        }
        // Add elements from b not in a
        for i in 0..len_b {
            let elem = array_get_i64(b as *const MvlArray, i);
            if !array_contains_i64(a as *const MvlArray, elem) {
                array_push_i64(result, elem);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(unsafe_code)]
    unsafe fn make_set(vals: &[i64]) -> *mut MvlArray {
        let arr = new_i64_array(vals.len().max(1));
        for &v in vals {
            array_push_i64(arr, v);
        }
        arr
    }

    #[allow(unsafe_code)]
    unsafe fn read_set(arr: *const MvlArray) -> Vec<i64> {
        let len = crate::memory_ops::_mvl_array_len(arr);
        (0..len).map(|i| array_get_i64(arr, i)).collect()
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_intersection() {
        unsafe {
            let a = make_set(&[1, 2, 3, 4]);
            let b = make_set(&[3, 4, 5, 6]);
            let r = _mvl_set_intersection(a, b);
            assert_eq!(read_set(r as *const MvlArray), vec![3, 4]);
            crate::memory::_mvl_array_drop(a);
            crate::memory::_mvl_array_drop(b);
            crate::memory::_mvl_array_drop(r);
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_difference() {
        unsafe {
            let a = make_set(&[1, 2, 3, 4]);
            let b = make_set(&[3, 4, 5, 6]);
            let r = _mvl_set_difference(a, b);
            assert_eq!(read_set(r as *const MvlArray), vec![1, 2]);
            crate::memory::_mvl_array_drop(a);
            crate::memory::_mvl_array_drop(b);
            crate::memory::_mvl_array_drop(r);
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_union() {
        unsafe {
            let a = make_set(&[1, 2, 3]);
            let b = make_set(&[3, 4, 5]);
            let r = _mvl_set_union(a, b);
            assert_eq!(read_set(r as *const MvlArray), vec![1, 2, 3, 4, 5]);
            crate::memory::_mvl_array_drop(a);
            crate::memory::_mvl_array_drop(b);
            crate::memory::_mvl_array_drop(r);
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_empty_sets() {
        unsafe {
            let empty = make_set(&[]);
            let full = make_set(&[1, 2, 3]);
            let ri = _mvl_set_intersection(empty, full);
            assert_eq!(read_set(ri as *const MvlArray), Vec::<i64>::new());
            let rd = _mvl_set_difference(empty, full);
            assert_eq!(read_set(rd as *const MvlArray), Vec::<i64>::new());
            let ru = _mvl_set_union(empty, full);
            assert_eq!(read_set(ru as *const MvlArray), vec![1, 2, 3]);
            crate::memory::_mvl_array_drop(empty);
            crate::memory::_mvl_array_drop(full);
            crate::memory::_mvl_array_drop(ri);
            crate::memory::_mvl_array_drop(rd);
            crate::memory::_mvl_array_drop(ru);
        }
    }
}
