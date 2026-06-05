// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! SHA-256 hashing primitives — shared by package fetch and SBOM generation.
//!
//! Pure-Rust FIPS 180-4 SHA-256.  No external crates — keeps the compiler
//! binary dependency-free for this operation (consistent with the approach
//! established in `fetch.rs`).
//!
//! # Public API
//!
//! | Function | Returns |
//! |---|---|
//! | `sha256_hex(data)` | lowercase hex string of SHA-256 digest |
//! | `sha256_file(path)` | `"sha256:<hex>"` of file raw bytes |
//! | `sha256_source_tree(files)` | deterministic tree digest over `(rel_path, sha256_hex)` pairs |

use std::io;
use std::path::Path;

// ── Public API ────────────────────────────────────────────────────────────────

/// Hash `data` and return the lowercase hex digest (no prefix).
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256State::new();
    h.update(data);
    hex_encode(&h.finalize())
}

/// Hash the raw bytes of `path` and return `"sha256:<hex>"`.
///
/// The file is read into memory once; no streaming is needed for typical
/// `.mvl` source files.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let data = std::fs::read(path)?;
    Ok(format!("sha256:{}", sha256_hex(&data)))
}

/// Compute a deterministic digest over a set of source files.
///
/// Input: `(canonical_relative_path, sha256_hex_of_file)` pairs.
/// The pairs are sorted lexicographically by path before hashing, so the
/// result is independent of filesystem enumeration order.
///
/// Hash input for each sorted entry: `"<rel_path>:<file_sha256>\n"`.
///
/// Returns `"sha256:<hex>"`.
pub fn sha256_source_tree(files: &[(&str, &str)]) -> String {
    let mut sorted: Vec<_> = files.to_vec();
    sorted.sort_by_key(|(path, _)| *path);

    let mut h = Sha256State::new();
    for (path, file_hash) in &sorted {
        h.update(path.as_bytes());
        h.update(b":");
        h.update(file_hash.as_bytes());
        h.update(b"\n");
    }
    format!("sha256:{}", hex_encode(&h.finalize()))
}

// ── Internal helpers (crate-visible for fetch.rs) ─────────────────────────────

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

// ── SHA-256 state machine (FIPS 180-4) ────────────────────────────────────────

pub(crate) struct Sha256State {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256State {
    pub(crate) fn new() -> Self {
        Sha256State {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total: 0,
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        let mut pos = 0;
        while pos < data.len() {
            let space = 64 - self.buf_len;
            let take = space.min(data.len() - pos);
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[pos..pos + take]);
            self.buf_len += take;
            pos += take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
    }

    pub(crate) fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total * 8;
        self.update(&[0x80]);
        while self.buf_len % 64 != 56 {
            self.update(&[0x00]);
        }
        let be = bit_len.to_be_bytes();
        self.update(&be);
        debug_assert_eq!(self.buf_len, 0);
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_empty() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_hello() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_file_round_trip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"hello").unwrap();
        let digest = sha256_file(tmp.path()).unwrap();
        assert_eq!(
            digest,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_source_tree_sorted_deterministic() {
        let files_a = [("b.mvl", "bbb"), ("a.mvl", "aaa")];
        let files_b = [("a.mvl", "aaa"), ("b.mvl", "bbb")];
        assert_eq!(
            sha256_source_tree(&files_a),
            sha256_source_tree(&files_b),
            "order of input must not affect result"
        );
    }

    #[test]
    fn sha256_source_tree_differs_on_content_change() {
        let v1 = sha256_source_tree(&[("main.mvl", "version1")]);
        let v2 = sha256_source_tree(&[("main.mvl", "version2")]);
        assert_ne!(v1, v2);
    }

    #[test]
    fn sha256_source_tree_differs_on_path_change() {
        let v1 = sha256_source_tree(&[("main.mvl", "same")]);
        let v2 = sha256_source_tree(&[("other.mvl", "same")]);
        assert_ne!(v1, v2);
    }

    #[test]
    fn sha256_source_tree_empty_returns_sha256_prefix() {
        let d = sha256_source_tree(&[]);
        assert!(d.starts_with("sha256:"), "must start with sha256: prefix");
        assert_eq!(d.len(), "sha256:".len() + 64);
    }
}
