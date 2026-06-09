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

use crate::memory::_mvl_string_new;
use mvl_runtime::stdlib::random;

// ── Float → string conversion (#1202) ────────────────────────────────────────

/// Convert a `Float` (f64) to a heap-allocated `MvlString*`.
///
/// Used by the LLVM backend for `Float::to_string()`. Returns the shortest
/// round-trip decimal representation via Rust's default `f64` Display impl.
/// The returned `*mut c_void` is a `*mut MvlString`; the caller must drop it.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_float_to_string(v: f64) -> *mut c_void {
    let s = format!("{v}");
    let bytes = s.as_bytes();
    unsafe { _mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void }
}

// ── Float → Int checked conversion (#1262) ───────────────────────────────────

/// Checked Float→Int: writes `*out = truncated` and returns 0 (Some) when `v`
/// is finite and within i64 range; returns 1 (None) for NaN, ±Inf, or
/// out-of-range values.
///
/// # Safety
/// `out` must be a valid non-null writable pointer.
#[no_mangle]
pub unsafe extern "C" fn _mvl_float_checked_to_int(v: f64, out: *mut i64) -> i8 {
    if v.is_finite() && v >= (i64::MIN as f64) && v <= (i64::MAX as f64) {
        *out = v as i64;
        0 // Some
    } else {
        1 // None
    }
}

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
/// Each element is stored as i64 in the MvlArray (LLVM backend uniform layout).
/// The LLVM caller is responsible for dropping the array via `_mvl_array_drop`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_bytes(n: i64) -> *mut crate::memory::MvlArray {
    use crate::memory::_mvl_array_new;
    let vals = random::bytes(n);
    let arr = unsafe { _mvl_array_new(std::mem::size_of::<i64>(), vals.len().max(1)) };
    for v in vals {
        let wide = v as i64;
        unsafe {
            crate::memory_ops::_mvl_array_push(arr, (&wide as *const i64).cast());
        }
    }
    arr
}

/// Return a random index from a `MvlArray*`, or -1 if empty.
///
/// Used by the LLVM backend for `choice[T](list)`: the backend calls this to
/// get the index, then does the type-dependent element load itself.
/// Calling convention: `(ptr) → i64`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_choice_index(arr: *mut crate::memory::MvlArray) -> i64 {
    if arr.is_null() {
        return -1;
    }
    let len = unsafe { crate::memory_ops::_mvl_array_len(arr as *const crate::memory::MvlArray) };
    if len == 0 {
        return -1;
    }
    random::int(0, len - 1)
}

/// Deep-clone the array, then Fisher-Yates shuffle the clone. Returns a new `MvlArray*`.
///
/// Returns a new array so that the source and result are independent — MVL has
/// value semantics and `let ys = shuffle(xs)` must not alias `xs`.
/// All MVL element slots are 8 bytes (i64/f64/ptr all fit), so swapping as raw
/// i64 words is type-safe regardless of the actual element type.
/// Calling convention: `(ptr) → ptr`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_random_shuffle(
    arr: *mut crate::memory::MvlArray,
) -> *mut crate::memory::MvlArray {
    if arr.is_null() {
        return arr;
    }
    unsafe {
        // Deep-clone so source and result are independent (value semantics).
        let clone = crate::memory::_mvl_array_deep_clone(arr);
        let len = crate::memory_ops::_mvl_array_len(clone as *const crate::memory::MvlArray);
        if len <= 1 {
            return clone;
        }
        // Fisher-Yates: iterate i from len-1 down to 1, swap clone[i] with clone[rand(0..=i)].
        for i in (1..len).rev() {
            let j = random::int(0, i);
            if i != j {
                let ptr_i =
                    crate::memory_ops::_mvl_array_get(clone as *const crate::memory::MvlArray, i)
                        as *mut i64;
                let ptr_j =
                    crate::memory_ops::_mvl_array_get(clone as *const crate::memory::MvlArray, j)
                        as *mut i64;
                let tmp = ptr_i.read();
                ptr_i.write(ptr_j.read());
                ptr_j.write(tmp);
            }
        }
        clone
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
        use crate::memory::{_mvl_array_drop, MvlArray};
        let arr = _mvl_random_bytes(16);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::_mvl_array_len(arr as *const MvlArray) };
        assert_eq!(len, 16);
        unsafe { _mvl_array_drop(arr) };
    }
}
