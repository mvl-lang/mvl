// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for Map[K, V] operations.
//!
//! Wraps the internal map helpers from `memory_ops` under the `_mvl_map_*`
//! naming convention, consistent with all other stdlib modules.
//!
//! # LLVM dispatch coverage
//!
//! The following functions are wired into the LLVM codegen dispatch table
//! (`emit_method_call.rs`):
//!   - `_mvl_map_get`    — (ptr, ptr, i64) → ptr  (`Map.get` / `Map.contains_key`)
//!   - `_mvl_map_insert` — (ptr, ptr, i64, ptr, i64) → void  (`Map.insert`)
//!   - `_mvl_map_remove` — (ptr, ptr, i64) → void  (`Map.remove` / `Map.without`)
//!   - `_mvl_map_len`    — (ptr) → i64  (`Map.len`)
//!   - `_mvl_map_keys`   — (ptr) → ptr  (`Map.keys`)
//!   - `_mvl_map_values` — (ptr) → ptr  (`Map.values`)

use crate::memory::{MvlArray, MvlMap};
use crate::memory_ops;

/// Look up `key` in `map`; returns a pointer to the value bytes or null if absent.
///
/// `_mvl_map_get(map: *const MvlMap, key: *const u8, key_len: i64) -> *const u8`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_get(
    map: *const MvlMap,
    key: *const u8,
    key_len: i64,
) -> *const u8 {
    memory_ops::mvl_map_get(map, key, key_len as usize)
}

/// Insert `key → value` into `map` (overwrites any existing entry).
///
/// `_mvl_map_insert(map: *mut MvlMap, key: *const u8, key_len: i64, val: *const u8, val_len: i64)`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_insert(
    map: *mut MvlMap,
    key: *const u8,
    key_len: i64,
    val: *const u8,
    val_len: i64,
) {
    memory_ops::mvl_map_insert(map, key, key_len as usize, val, val_len as usize);
}

/// Remove `key` from `map` (no-op if absent).
///
/// `_mvl_map_remove(map: *mut MvlMap, key: *const u8, key_len: i64)`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_remove(map: *mut MvlMap, key: *const u8, key_len: i64) {
    memory_ops::mvl_map_remove(map, key, key_len as usize);
}

/// Return the number of entries in `map`.
///
/// `_mvl_map_len(map: *const MvlMap) -> i64`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_len(map: *const MvlMap) -> i64 {
    memory_ops::mvl_map_len(map) as i64
}

/// Return all keys as a `MvlArray*` of `*mut MvlString` pointers (order unspecified).
///
/// `_mvl_map_keys(map: *const MvlMap) -> *mut MvlArray`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_keys(map: *const MvlMap) -> *mut MvlArray {
    memory_ops::mvl_map_keys(map)
}

/// Return all values as a `MvlArray*` of `*mut MvlString` pointers (order unspecified).
///
/// `_mvl_map_values(map: *const MvlMap) -> *mut MvlArray`
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_map_values(map: *const MvlMap) -> *mut MvlArray {
    memory_ops::mvl_map_values(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::_mvl_map_new;

    #[allow(unsafe_code)]
    unsafe fn make_map() -> *mut MvlMap {
        _mvl_map_new(0)
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_insert_and_get() {
        unsafe {
            let m = make_map();
            let key = b"hello";
            let val = b"world";
            _mvl_map_insert(
                m,
                key.as_ptr(),
                key.len() as i64,
                val.as_ptr(),
                val.len() as i64,
            );
            let got = _mvl_map_get(m as *const MvlMap, key.as_ptr(), key.len() as i64);
            assert!(!got.is_null());
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_len() {
        unsafe {
            let m = make_map();
            assert_eq!(_mvl_map_len(m as *const MvlMap), 0);
            _mvl_map_insert(m, b"a".as_ptr(), 1, b"1".as_ptr(), 1);
            assert_eq!(_mvl_map_len(m as *const MvlMap), 1);
            _mvl_map_insert(m, b"b".as_ptr(), 1, b"2".as_ptr(), 1);
            assert_eq!(_mvl_map_len(m as *const MvlMap), 2);
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_remove() {
        unsafe {
            let m = make_map();
            _mvl_map_insert(m, b"x".as_ptr(), 1, b"v".as_ptr(), 1);
            assert_eq!(_mvl_map_len(m as *const MvlMap), 1);
            _mvl_map_remove(m, b"x".as_ptr(), 1);
            assert_eq!(_mvl_map_len(m as *const MvlMap), 0);
            let got = _mvl_map_get(m as *const MvlMap, b"x".as_ptr(), 1);
            assert!(got.is_null());
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_keys_and_values_len() {
        unsafe {
            let m = make_map();
            _mvl_map_insert(m, b"a".as_ptr(), 1, b"1".as_ptr(), 1);
            _mvl_map_insert(m, b"b".as_ptr(), 1, b"2".as_ptr(), 1);
            let keys = _mvl_map_keys(m as *const MvlMap);
            let vals = _mvl_map_values(m as *const MvlMap);
            assert_eq!(
                crate::memory_ops::_mvl_array_len(keys as *const MvlArray),
                2
            );
            assert_eq!(
                crate::memory_ops::_mvl_array_len(vals as *const MvlArray),
                2
            );
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_get_absent_key_returns_null() {
        unsafe {
            let m = make_map();
            let got = _mvl_map_get(m as *const MvlMap, b"missing".as_ptr(), 7);
            assert!(got.is_null());
        }
    }
}
