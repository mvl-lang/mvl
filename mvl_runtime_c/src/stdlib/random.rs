// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.random` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::random`. The xorshift64 PRNG state lives in
//! an `AtomicU64` (global, CAS-based) in the `mvl_runtime` crate; see that
//! module for the thread-safety contract.
//!
//! # LLVM dispatch coverage
//!
//! The following functions are wired into the LLVM codegen dispatch table:
//!   - `_mvl_random_int`   — (i64, i64) → i64  (`I64TwoI64Args`)
//!   - `_mvl_random_float` — () → f64           (`F64NoArg`)
//!   - `_mvl_random_bytes` — (i64) → ptr        (`I64ReturnsPtrArg`)
//!
//! `_mvl_random_choice_index` and `_mvl_random_shuffle_i64` use the old
//! length-prefixed i64 layout and are deferred pending LLVM wiring.

use libc::c_void;

use mvl_runtime::stdlib::random;

// ── Primitive dispatch ────────────────────────────────────────────────────────

/// Return a random integer in `[min, max]` (inclusive). Both args and return are i64.
#[no_mangle]
pub extern "C" fn _mvl_random_int(min: i64, max: i64) -> i64 {
    random::int(min, max)
}

/// Return a random float in `[0.0, 1.0)`.
#[no_mangle]
pub extern "C" fn _mvl_random_float() -> f64 {
    random::float()
}

// ── MvlArray* dispatch ────────────────────────────────────────────────────────

/// Return `n` pseudo-random bytes as a `*mut MvlArray` of i64 values in [0, 255].
///
/// Each element is an i64 byte value. The LLVM caller is responsible for
/// dropping the array via `mvl_array_drop`. Wired via `I64ReturnsPtrArg`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_bytes(n: i64) -> *mut mvl_memory::MvlArray {
    use mvl_memory::mvl_array_new;
    let vals = random::bytes(n);
    let arr = unsafe { mvl_array_new(std::mem::size_of::<i64>(), vals.len().max(1)) };
    for v in vals {
        unsafe {
            crate::memory_ops::mvl_array_push(arr, (&v as *const i64).cast());
        }
    }
    arr
}

/// Return the index of a random element in a length-prefixed i64 array, or -1 if empty.
///
/// Input: `*mut c_void` with layout `[len: i64][elem0: i64]...`
/// LLVM codegen wiring is deferred pending `MvlArray*` marshalling support.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_choice_index(arr: *mut c_void) -> i64 {
    if arr.is_null() {
        return -1;
    }
    let len = unsafe { (arr as *const i64).read() } as usize;
    if len == 0 {
        return -1;
    }
    random::int(0, (len - 1) as i64)
}

/// Shuffle an i64 array in place (length-prefix layout). Returns void.
///
/// Input: `*mut c_void` with layout `[len: i64][elem0: i64]...`
/// LLVM codegen wiring is deferred pending `MvlArray*` marshalling support.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_shuffle_i64(arr: *mut c_void) {
    if arr.is_null() {
        return;
    }
    let ptr = arr as *mut i64;
    let len = unsafe { ptr.read() } as usize;
    let mut items: Vec<i64> = (0..len).map(|i| unsafe { ptr.add(1 + i).read() }).collect();
    items = random::shuffle(items);
    for (i, v) in items.iter().enumerate() {
        unsafe { ptr.add(1 + i).write(*v) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_int_in_range() {
        for _ in 0..100 {
            let v = _mvl_random_int(1, 10);
            assert!((1..=10).contains(&v));
        }
    }

    #[test]
    fn test_random_int_deterministic_range() {
        for _ in 0..50 {
            assert_eq!(_mvl_random_int(7, 7), 7);
        }
    }

    #[test]
    fn test_random_float_in_range() {
        for _ in 0..100 {
            let v = _mvl_random_float();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_random_bytes_length() {
        use mvl_memory::{mvl_array_drop, MvlArray};
        let arr = _mvl_random_bytes(16);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::mvl_array_len(arr as *const MvlArray) };
        assert_eq!(len, 16);
        unsafe { mvl_array_drop(arr) };
    }
}
