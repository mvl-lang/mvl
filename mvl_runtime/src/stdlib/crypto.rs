//! Rust implementations of `std.crypto` stdlib functions.
//!
//! Provides real hashing and CSPRNG backing for the stubs declared in
//! `std/crypto.mvl`. These are re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::Secret;
use sha2::{Digest, Sha256, Sha512};

/// Returns the SHA-256 digest of the input bytes as a lowercase hex string.
///
/// Pure — hashing is deterministic.
/// Implements the Rust backing for `std/crypto.mvl::sha256`.
pub fn sha256(data: String) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Returns the SHA-512 digest of the input bytes as a lowercase hex string.
///
/// Pure — hashing is deterministic.
/// Implements the Rust backing for `std/crypto.mvl::sha512`.
pub fn sha512(data: String) -> String {
    let mut hasher = Sha512::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Returns `n` cryptographically secure random bytes as a `Secret<Vec<i64>>`.
///
/// Reads from the OS CSPRNG (platform-native via `getrandom`).
/// Returns `Secret` — callers cannot log or print the raw bytes from MVL.
/// Panics if the OS CSPRNG is unavailable (unrecoverable environment failure).
/// Implements the Rust backing for `std/crypto.mvl::crypto_random_bytes`.
pub fn crypto_random_bytes(n: i64) -> Secret<Vec<i64>> {
    let count = n.max(0) as usize;
    let bytes = os_random_bytes(count);
    Secret(bytes.into_iter().map(|b| b as i64).collect())
}

/// Read `n` random bytes from the OS CSPRNG.
fn os_random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).unwrap_or_else(|_| std::process::abort());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIST test vector: SHA-256("") — 64 hex chars
    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    // SHA-256("abc") — verified with openssl dgst -sha256 and shasum -a 256
    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    // NIST test vector: SHA-512("") — 128 hex chars
    const SHA512_EMPTY: &str = concat!(
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce",
        "47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    // NIST test vector: SHA-512("abc")
    const SHA512_ABC: &str = concat!(
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );

    #[test]
    fn sha256_empty_matches_nist_vector() {
        assert_eq!(sha256(String::new()), SHA256_EMPTY);
    }

    #[test]
    fn sha256_abc_matches_nist_vector() {
        assert_eq!(sha256("abc".to_string()), SHA256_ABC);
    }

    #[test]
    fn sha256_output_is_64_hex_chars() {
        assert_eq!(sha256("hello world".to_string()).len(), 64);
    }

    #[test]
    fn sha256_output_is_lowercase_hex() {
        let out = sha256("hello world".to_string());
        assert!(out
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn sha256_is_deterministic() {
        let a = sha256("test".to_string());
        let b = sha256("test".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn sha256_different_inputs_differ() {
        assert_ne!(sha256("a".to_string()), sha256("b".to_string()));
    }

    #[test]
    fn sha512_empty_matches_nist_vector() {
        assert_eq!(sha512(String::new()), SHA512_EMPTY);
    }

    #[test]
    fn sha512_abc_matches_nist_vector() {
        assert_eq!(sha512("abc".to_string()), SHA512_ABC);
    }

    #[test]
    fn sha512_output_is_128_hex_chars() {
        assert_eq!(sha512("hello world".to_string()).len(), 128);
    }

    #[test]
    fn sha512_output_is_lowercase_hex() {
        let out = sha512("hello world".to_string());
        assert!(out
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn sha512_is_deterministic() {
        let a = sha512("test".to_string());
        let b = sha512("test".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn sha512_different_inputs_differ() {
        assert_ne!(sha512("a".to_string()), sha512("b".to_string()));
    }

    #[test]
    fn crypto_random_bytes_length() {
        let Secret(bytes) = crypto_random_bytes(16);
        assert_eq!(bytes.len(), 16);
    }

    #[test]
    fn crypto_random_bytes_zero() {
        let Secret(bytes) = crypto_random_bytes(0);
        assert!(bytes.is_empty());
    }

    #[test]
    fn crypto_random_bytes_negative_treated_as_zero() {
        let Secret(bytes) = crypto_random_bytes(-5);
        assert!(bytes.is_empty());
    }

    #[test]
    fn crypto_random_bytes_values_are_non_negative() {
        let Secret(bytes) = crypto_random_bytes(32);
        assert!(bytes.iter().all(|&b| b >= 0));
    }

    #[test]
    fn crypto_random_bytes_are_unique_across_calls() {
        let Secret(a) = crypto_random_bytes(32);
        let Secret(b) = crypto_random_bytes(32);
        assert_ne!(a, b, "two CSPRNG calls should not produce identical output");
    }
}
