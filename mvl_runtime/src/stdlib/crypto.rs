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
/// Reads from the OS CSPRNG (`/dev/urandom` on Unix).
/// Returns `Secret` — callers cannot log or print the raw bytes from MVL.
/// Implements the Rust backing for `std/crypto.mvl::crypto_random_bytes`.
pub fn crypto_random_bytes(n: i64) -> Secret<Vec<i64>> {
    let count = n.max(0) as usize;
    let bytes = os_random_bytes(count);
    Secret(bytes.into_iter().map(|b| b as i64).collect())
}

/// Read `n` random bytes from the OS CSPRNG.
fn os_random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIST test vector: SHA-256("") — 64 hex chars
    const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    // NIST test vector: SHA-512("") — 128 hex chars
    const SHA512_EMPTY: &str = concat!(
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce",
        "47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );

    #[test]
    fn sha256_empty_matches_nist_vector() {
        assert_eq!(sha256(String::new()), SHA256_EMPTY);
    }

    #[test]
    fn sha256_output_is_64_hex_chars() {
        assert_eq!(sha256("hello world".to_string()).len(), 64);
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
    fn sha512_output_is_128_hex_chars() {
        assert_eq!(sha512("hello world".to_string()).len(), 128);
    }

    #[test]
    fn sha512_is_deterministic() {
        let a = sha512("test".to_string());
        let b = sha512("test".to_string());
        assert_eq!(a, b);
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
    fn crypto_random_bytes_values_in_byte_range() {
        let Secret(bytes) = crypto_random_bytes(32);
        assert!(bytes.iter().all(|&b| (0..=255).contains(&b)));
    }
}
