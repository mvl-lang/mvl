//! C-ABI exports for `std.crypto` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::crypto`. Every public function in that module
//! has a corresponding `_mvl_crypto_*` symbol here, callable from LLVM IR via
//! `lli --load=libmvl_runtime_c.{dylib,so}`.
//!
//! # String ownership
//!
//! - Input strings (`*const c_char`): owned by the caller, not freed here.
//! - Output strings (`*mut c_char`): heap-allocated by this crate via `libc::malloc`.
//!   The LLVM caller is responsible for freeing with `libc::free` after use.
//!
//! # Array layout for `_mvl_crypto_random_bytes`
//!
//! Returns a length-prefixed i64 array on the heap:
//!   `[len: i64][val0: i64][val1: i64]...`
//! Each value is in `[0, 255]` (byte range stored as i64).
//! Caller frees with `libc::free`. This matches `_mvl_random_bytes` in random.rs.

use libc::{c_char, c_void};
use mvl_runtime::ifc::Secret;

use crate::abi::{c_to_string, string_to_c};

// ── Hash functions (pure, deterministic) ─────────────────────────────────────

/// Return the SHA-256 digest of `data` as a lowercase hex string (64 chars).
///
/// Pure — hashing is deterministic. Input is caller-owned; output is a
/// heap-allocated NUL-terminated string that the caller must free.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_sha256(data: *const c_char) -> *mut c_char {
    let s = unsafe { c_to_string(data) };
    string_to_c(&mvl_runtime::stdlib::crypto::sha256(s))
}

/// Return the SHA-512 digest of `data` as a lowercase hex string (128 chars).
///
/// Pure — hashing is deterministic. Input is caller-owned; output is a
/// heap-allocated NUL-terminated string that the caller must free.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_sha512(data: *const c_char) -> *mut c_char {
    let s = unsafe { c_to_string(data) };
    string_to_c(&mvl_runtime::stdlib::crypto::sha512(s))
}

// ── CSPRNG (non-deterministic, ! CryptoRandom) ───────────────────────────────

/// Return `n` cryptographically secure random bytes as a heap-allocated
/// length-prefixed i64 array.
///
/// Layout: `[len: i64][val0: i64]...` where each value is in `[0, 255]`.
/// Reads from the OS CSPRNG via `getrandom`. Caller frees with `libc::free`.
/// The `Secret` wrapper is a Rust compile-time label; the C-ABI returns raw bytes.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_random_bytes(n: i64) -> *mut c_void {
    let Secret(vals) = mvl_runtime::stdlib::crypto::crypto_random_bytes(n);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn to_c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const SHA512_EMPTY: &str = concat!(
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce",
        "47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );

    #[test]
    #[allow(unsafe_code)]
    fn sha256_empty_nist_vector() {
        let input = to_c("");
        let out = _mvl_crypto_sha256(input.as_ptr());
        assert!(!out.is_null());
        let result = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { libc::free(out as *mut c_void) };
        assert_eq!(result, SHA256_EMPTY);
    }

    #[test]
    #[allow(unsafe_code)]
    fn sha256_abc_nist_vector() {
        let input = to_c("abc");
        let out = _mvl_crypto_sha256(input.as_ptr());
        assert!(!out.is_null());
        let result = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { libc::free(out as *mut c_void) };
        assert_eq!(result, SHA256_ABC);
    }

    #[test]
    #[allow(unsafe_code)]
    fn sha512_empty_nist_vector() {
        let input = to_c("");
        let out = _mvl_crypto_sha512(input.as_ptr());
        assert!(!out.is_null());
        let result = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { libc::free(out as *mut c_void) };
        assert_eq!(result, SHA512_EMPTY);
    }

    #[test]
    #[allow(unsafe_code)]
    fn sha256_output_is_lowercase_hex() {
        let input = to_c("hello world");
        let out = _mvl_crypto_sha256(input.as_ptr());
        assert!(!out.is_null());
        let result = unsafe { std::ffi::CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { libc::free(out as *mut c_void) };
        assert_eq!(result.len(), 64);
        assert!(result
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    #[allow(unsafe_code)]
    fn random_bytes_correct_length() {
        let ptr = _mvl_crypto_random_bytes(16);
        assert!(!ptr.is_null());
        let len = unsafe { (ptr as *const i64).read() };
        assert_eq!(len, 16);
        unsafe { libc::free(ptr) };
    }

    #[test]
    #[allow(unsafe_code)]
    fn random_bytes_zero_length() {
        let ptr = _mvl_crypto_random_bytes(0);
        assert!(!ptr.is_null());
        let len = unsafe { (ptr as *const i64).read() };
        assert_eq!(len, 0);
        unsafe { libc::free(ptr) };
    }

    #[test]
    #[allow(unsafe_code)]
    fn random_bytes_values_in_byte_range() {
        let ptr = _mvl_crypto_random_bytes(32);
        assert!(!ptr.is_null());
        let base = ptr as *const i64;
        let len = unsafe { base.read() } as usize;
        for i in 0..len {
            let v = unsafe { base.add(1 + i).read() };
            assert!((0..=255).contains(&v), "byte value {v} out of range");
        }
        unsafe { libc::free(ptr) };
    }
}
