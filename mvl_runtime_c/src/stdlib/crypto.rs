// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.crypto` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::crypto`. Every public function in that module
//! has a corresponding `_mvl_crypto_*` symbol here, callable from LLVM IR via
//! `lli --load=libmvl_runtime_c.{dylib,so}`.
//!
//! # String ownership
//!
//! - Input strings (`*const MvlString`): borrowed from the LLVM heap — not freed.
//! - Output strings (`*mut MvlString`): heap-allocated via `mvl_string_new`.
//!   LLVM heap-drop tracking frees them at scope exit.
//!
//! # Array return for `_mvl_crypto_random_bytes`
//!
//! Returns a heap-allocated `*mut MvlArray` (element size 8, one i64 per byte).
//! Each element is in `[0, 255]` (byte range stored as i64).
//! The array is owned by the LLVM heap-drop system and freed via `mvl_array_drop`.
//! This is compatible with all list stdlib operations (`list_len`, `list_get`, …).
//!
//! # LLVM dispatch coverage
//!
//! `sha256` and `sha512` are wired as tier-1 builtins (`PtrIdentArg`) in codegen.
//! `crypto_random_bytes` is wired as a tier-1 builtin (`I64ReturnsPtrArg`) — see
//! `collect_stdlib_imports` in `src/mvl/codegen/mod.rs` (#507).
//!
//! # IFC at the C-ABI boundary
//!
//! The `Secret` label from `crypto_random_bytes` is a Rust compile-time wrapper;
//! it is stripped at the C-ABI boundary. IFC enforcement on the LLVM path relies on
//! the MVL static checker (which runs before codegen) and codegen-level debug_asserts.
//! The codegen registers the return type as `Secret[List[Int]]` in `fn_return_types`
//! and tracks it through let-binding types to catch routing bugs. Tracked in #508.

use std::slice;

use mvl_memory::{mvl_array_new, mvl_string_new, MvlArray, MvlString};
use mvl_runtime::ifc::Secret;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read a `MvlString*` as a Rust `String`. Null / empty are handled gracefully.
#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    // Bound len against cap to guard against corrupted MvlString fields.
    let len = ((*s).len as usize).min((*s).cap as usize);
    if len == 0 || (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = slice::from_raw_parts((*s).ptr as *const u8, len);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Allocate a new heap `MvlString` from a Rust `&str`.
#[allow(unsafe_code)]
fn new_mvl_str(s: &str) -> *mut MvlString {
    let bytes = s.as_bytes();
    unsafe { mvl_string_new(bytes.as_ptr(), bytes.len()) }
}

// ── Hash functions (pure, deterministic) ─────────────────────────────────────

/// Return the SHA-256 digest of `data` as a lowercase hex `MvlString*` (64 chars).
///
/// Pure — hashing is deterministic. Input is a borrowed `MvlString*`; output is
/// a new heap-allocated `MvlString*` owned by the LLVM heap-drop system.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_sha256(data: *const MvlString) -> *mut MvlString {
    let s = unsafe { read_mvl_string(data) };
    new_mvl_str(&mvl_runtime::stdlib::crypto::sha256(s))
}

/// Return the SHA-512 digest of `data` as a lowercase hex `MvlString*` (128 chars).
///
/// Pure — hashing is deterministic. Input is a borrowed `MvlString*`; output is
/// a new heap-allocated `MvlString*` owned by the LLVM heap-drop system.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_sha512(data: *const MvlString) -> *mut MvlString {
    let s = unsafe { read_mvl_string(data) };
    new_mvl_str(&mvl_runtime::stdlib::crypto::sha512(s))
}

// ── CSPRNG (non-deterministic, ! CryptoRandom) ───────────────────────────────

/// Return `n` cryptographically secure random bytes as a heap-allocated `*mut MvlArray`.
///
/// Each element is an i64 in `[0, 255]` (element size 8 bytes).
/// Reads from the OS CSPRNG via `getrandom`. The array is owned by the LLVM
/// heap-drop system; callers must not free it with `libc::free`.
/// The `Secret` wrapper is a Rust compile-time label; the C-ABI returns a raw ptr.
/// Returns null if `n > 131_072` (1 MiB cap) or if allocation fails.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_crypto_random_bytes(n: i64) -> *mut MvlArray {
    // Cap to 1 MiB (131 072 bytes) to prevent unbounded allocation on adversarial input.
    const MAX_RANDOM_BYTES: i64 = 131_072;
    if n > MAX_RANDOM_BYTES {
        return std::ptr::null_mut();
    }
    let Secret(vals) = mvl_runtime::stdlib::crypto::crypto_random_bytes(n);
    unsafe {
        let arr = mvl_array_new(std::mem::size_of::<i64>(), vals.len());
        if arr.is_null() {
            return std::ptr::null_mut();
        }
        for v in &vals {
            crate::memory_ops::mvl_array_push(arr, (v as *const i64).cast());
        }
        arr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const SHA512_EMPTY: &str = concat!(
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce",
        "47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    const SHA512_ABC: &str = concat!(
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );

    fn mvl_str(s: &str) -> *mut MvlString {
        new_mvl_str(s)
    }

    #[allow(unsafe_code)]
    fn from_mvl_str(ptr: *mut MvlString) -> String {
        let s = unsafe { read_mvl_string(ptr) };
        unsafe { mvl_memory::mvl_string_drop(ptr) };
        s
    }

    #[test]
    fn sha256_empty_nist_vector() {
        let input = mvl_str("");
        let out = _mvl_crypto_sha256(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA256_EMPTY);
    }

    #[test]
    fn sha256_abc_nist_vector() {
        let input = mvl_str("abc");
        let out = _mvl_crypto_sha256(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA256_ABC);
    }

    #[test]
    fn sha512_empty_nist_vector() {
        let input = mvl_str("");
        let out = _mvl_crypto_sha512(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA512_EMPTY);
    }

    #[test]
    fn sha256_output_is_lowercase_hex() {
        let input = mvl_str("hello world");
        let out = _mvl_crypto_sha256(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        let result = from_mvl_str(out);
        assert_eq!(result.len(), 64);
        assert!(result
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn random_bytes_correct_length() {
        let arr = _mvl_crypto_random_bytes(16);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::mvl_array_len(arr) };
        assert_eq!(len, 16);
        unsafe { mvl_memory::mvl_array_drop(arr) };
    }

    #[test]
    fn random_bytes_zero_length() {
        let arr = _mvl_crypto_random_bytes(0);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::mvl_array_len(arr) };
        assert_eq!(len, 0);
        unsafe { mvl_memory::mvl_array_drop(arr) };
    }

    #[test]
    fn random_bytes_values_in_byte_range() {
        let arr = _mvl_crypto_random_bytes(32);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::mvl_array_len(arr) } as usize;
        for i in 0..len {
            let elem_ptr = unsafe { crate::memory_ops::mvl_array_get(arr, i) as *const i64 };
            let v = unsafe { elem_ptr.read() };
            assert!((0..=255).contains(&v), "byte value {v} out of range");
        }
        unsafe { mvl_memory::mvl_array_drop(arr) };
    }

    #[test]
    fn sha512_abc_nist_vector() {
        let input = mvl_str("abc");
        let out = _mvl_crypto_sha512(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA512_ABC);
    }

    #[test]
    fn sha512_output_is_128_hex_chars() {
        let input = mvl_str("hello world");
        let out = _mvl_crypto_sha512(input);
        unsafe { mvl_memory::mvl_string_drop(input) };
        assert!(!out.is_null());
        let result = from_mvl_str(out);
        assert_eq!(result.len(), 128);
        assert!(result
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn random_bytes_negative_n_returns_empty() {
        let arr = _mvl_crypto_random_bytes(-1);
        assert!(!arr.is_null());
        let len = unsafe { crate::memory_ops::mvl_array_len(arr) };
        assert_eq!(len, 0, "negative n must produce 0 bytes");
        unsafe { mvl_memory::mvl_array_drop(arr) };
    }

    #[test]
    fn sha256_null_ptr_returns_hash_of_empty() {
        let out = _mvl_crypto_sha256(std::ptr::null());
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA256_EMPTY);
    }

    #[test]
    fn sha512_null_ptr_returns_hash_of_empty() {
        let out = _mvl_crypto_sha512(std::ptr::null());
        assert!(!out.is_null());
        assert_eq!(from_mvl_str(out), SHA512_EMPTY);
    }
}
