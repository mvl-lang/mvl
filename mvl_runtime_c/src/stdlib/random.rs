//! C-ABI exports for `std.random` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::random`. The xorshift64 PRNG state lives in
//! an `AtomicU64` (global, CAS-based) in the `mvl_runtime` crate; see that
//! module for the thread-safety contract.
//!
//! # LLVM dispatch coverage
//!
//! The following functions are wired into the LLVM codegen dispatch table
//! (no-arg or primitive-arg, primitive return):
//!   - `_mvl_random_int`   — (i64, i64) → i64
//!   - `_mvl_random_float` — () → f64
//!
//! The remaining functions (`_mvl_random_bytes`, `_mvl_random_choice`,
//! `_mvl_random_shuffle`) require `MvlArray*` marshalling and are exported
//! here for future codegen integration. They are intentionally excluded from
//! the current dispatch table.

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

// ── Complex dispatch (MvlArray* — deferred LLVM wiring) ──────────────────────

/// Return `n` pseudo-random bytes as a heap-allocated `*mut c_void`.
///
/// Layout: `[len: i64][val0: i64][val1: i64]...` where each value is in [0, 255].
/// Matches `random::bytes() -> Vec<i64>` — each element is a byte value stored as i64.
/// Caller frees the allocation with `libc::free`.
///
/// LLVM codegen wiring is deferred pending `MvlArray*` marshalling support.
#[no_mangle]
pub extern "C" fn _mvl_random_bytes(n: i64) -> *mut c_void {
    let vals = random::bytes(n);
    let len = vals.len();
    let total = (1 + len) * std::mem::size_of::<i64>();
    let ptr = unsafe { libc::malloc(total) } as *mut i64;
    if ptr.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        ptr.write(len as i64);
        for (i, v) in vals.iter().enumerate() {
            ptr.add(1 + i).write(*v);
        }
    }
    ptr as *mut c_void
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
        let ptr = _mvl_random_bytes(16);
        assert!(!ptr.is_null());
        let len = unsafe { (ptr as *const i64).read() };
        assert_eq!(len, 16);
        unsafe { libc::free(ptr) };
    }
}
