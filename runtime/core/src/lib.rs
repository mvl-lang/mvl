// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Pure algorithms shared across MVL backends (Rust, LLVM, WASM).
//!
//! Each backend owns its ABI plumbing (pointer extraction and result wrapping).
//! This crate owns the algorithms so each new stdlib primitive is implemented
//! once and wrapped per backend rather than duplicated three times.

/// Concatenate two byte slices into a new `Vec<u8>`.
///
/// Used by `str_concat` and `list_concat` across all backends.
pub fn concat_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    out.extend_from_slice(a);
    out.extend_from_slice(b);
    out
}

/// Concatenate two typed lists. For the Rust backend's generic `Vec<T>` layer.
pub fn list_concat<T>(mut a: Vec<T>, b: Vec<T>) -> Vec<T> {
    a.extend(b);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concat_bytes_empty() {
        assert_eq!(concat_bytes(b"", b""), b"");
        assert_eq!(concat_bytes(b"hello", b""), b"hello");
        assert_eq!(concat_bytes(b"", b"world"), b"world");
    }

    #[test]
    fn concat_bytes_nonempty() {
        assert_eq!(concat_bytes(b"hello", b" world"), b"hello world");
    }

    #[test]
    fn list_concat_typed() {
        assert_eq!(list_concat(vec![1i64, 2], vec![3, 4]), vec![1, 2, 3, 4]);
        assert_eq!(list_concat::<i64>(vec![], vec![]), vec![]);
    }
}
