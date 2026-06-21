// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust kernel primitives for the MVL standard library.
//!
//! These are the `extern "rust"` trust-boundary functions declared in
//! `std/primitives.mvl`. They provide the irreducible Rust operations that
//! cannot be expressed in pure MVL, while all higher-level stdlib methods are
//! implemented as pure MVL wrappers on top of these primitives.
//!
//! # Design
//!
//! - Each function is safe, pure (no side effects), and panic-free.
//! - Bounds are clamped rather than panicked: out-of-range indices return
//!   `None` or an empty slice.
//! - All types use MVL's Rust representations: `String`, `Vec<T>`, `i64`, `u8`.
//!
//! ADR-0012: explicit extern bridge pattern.

// ── String kernel ────────────────────────────────────────────────────────────

/// Number of Unicode scalar values (chars) in `s`.
///
/// MVL `String.len()` is character-count, not byte-count.
pub fn str_len(s: String) -> i64 {
    s.chars().count() as i64
}

/// Decompose `s` into a list of single-character strings.
pub fn str_chars(s: String) -> Vec<String> {
    s.chars().map(|c| c.to_string()).collect()
}

/// Return the character at index `i` (0-based). Returns `None` if out of range.
pub fn str_char_at(s: String, i: i64) -> Option<String> {
    if i < 0 {
        return None;
    }
    s.chars().nth(i as usize).map(|c| c.to_string())
}

/// Reconstruct a `String` from a list of single-character strings.
pub fn str_from_chars(chars: Vec<String>) -> String {
    chars.into_iter().collect()
}

/// Return the byte value at character position `i` (0-based).
///
/// Returns `None` if out of range or if the character's codepoint > 255
/// (cannot be represented as a single Byte).
///
/// `str_byte_at(str_from_bytes(bs), i)` is a lossless round-trip for every
/// byte in 0..=255 because `str_from_bytes` maps each byte to the codepoint
/// of the same numeric value (Latin-1 / ISO-8859-1).
pub fn str_byte_at(s: String, i: i64) -> Option<u8> {
    if i < 0 {
        return None;
    }
    s.chars().nth(i as usize).and_then(|c| {
        let cp = c as u32;
        if cp <= 255 {
            Some(cp as u8)
        } else {
            None
        }
    })
}

/// Reconstruct a `String` from a raw byte sequence (Latin-1 / ISO-8859-1).
///
/// Each input byte 0..=255 maps to the Unicode codepoint of the same numeric
/// value, producing one MVL character per byte. This guarantees a lossless
/// round-trip with `str_byte_at` for every byte value, making `String` usable
/// as a transparent carrier for binary data (network protocols, hashes, etc.).
///
/// Note: this is NOT a UTF-8 decode. To interpret the bytes as UTF-8 text,
/// decode externally before constructing the `String`.
pub fn str_from_bytes(bytes: Vec<u8>) -> String {
    bytes.into_iter().map(|b| b as char).collect()
}

/// Concatenate two strings. `str_concat(a, b)` == `a + b`.
pub fn str_concat(s: String, other: String) -> String {
    s + &other
}

/// Remove leading and trailing ASCII whitespace.
pub fn str_trim(s: String) -> String {
    s.trim().to_string()
}

/// Convert all characters to uppercase.
pub fn str_to_upper(s: String) -> String {
    s.to_uppercase()
}

/// Convert all characters to lowercase.
pub fn str_to_lower(s: String) -> String {
    s.to_lowercase()
}

/// Return true if `s` begins with `prefix`.
pub fn str_starts_with(s: String, prefix: String) -> bool {
    s.starts_with(prefix.as_str())
}

/// Return true if `s` ends with `suffix`.
pub fn str_ends_with(s: String, suffix: String) -> bool {
    s.ends_with(suffix.as_str())
}

/// Return the character index of the first occurrence of `sub` in `s`, or
/// `None` if not found. Returns a character index, not a byte index.
pub fn str_find(s: String, sub: String) -> Option<i64> {
    // find() returns a byte offset; convert to char index.
    let byte_idx = s.find(sub.as_str())?;
    let char_idx = s[..byte_idx].chars().count() as i64;
    Some(char_idx)
}

/// Replace all occurrences of `from` with `to` in `s`.
pub fn str_replace(s: String, from: String, to: String) -> String {
    s.replace(from.as_str(), to.as_str())
}

/// Split `s` on `sep`, returning a list of substrings.
pub fn str_split(s: String, sep: String) -> Vec<String> {
    s.split(sep.as_str()).map(|part| part.to_string()).collect()
}

/// Return the substring of `s` from char index `start` (inclusive) to `end`
/// (exclusive). Negative indices are clamped to 0; indices beyond the length
/// are clamped to `len`. Inverted range returns `""`.
pub fn str_substring(s: String, start: i64, end: i64) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    if lo >= hi {
        return String::new();
    }
    chars[lo..hi].iter().collect()
}

/// Return true if `sub` is contained in `s`.
pub fn str_contains(s: String, sub: String) -> bool {
    s.contains(sub.as_str())
}

// ── List kernel ───────────────────────────────────────────────────────────────

/// Number of elements in `xs`.
pub fn list_len<T>(xs: Vec<T>) -> i64 {
    xs.len() as i64
}

/// Return `Some(xs[i])` or `None` if `i` is out of range.
pub fn list_get<T: Clone>(xs: Vec<T>, i: i64) -> Option<T> {
    if i < 0 {
        return None;
    }
    xs.get(i as usize).cloned()
}

/// Append `x` to `xs`, returning the new list (pure / non-mutating).
pub fn list_push<T>(mut xs: Vec<T>, x: T) -> Vec<T> {
    xs.push(x);
    xs
}

/// Return `xs[start..end]` with safe clamping. Negative indices → 0; indices
/// beyond length → length; inverted range → empty list. Never panics.
pub fn list_slice<T: Clone>(xs: Vec<T>, start: i64, end: i64) -> Vec<T> {
    let len = xs.len() as i64;
    let lo = start.max(0).min(len) as usize;
    let hi = end.max(0).min(len) as usize;
    if lo >= hi {
        return Vec::new();
    }
    xs[lo..hi].to_vec()
}

/// Concatenate two lists.
pub fn list_concat<T>(mut xs: Vec<T>, ys: Vec<T>) -> Vec<T> {
    xs.extend(ys);
    xs
}

/// Return true if `xs` contains `x` (requires `PartialEq`).
pub fn list_contains<T: PartialEq>(xs: Vec<T>, x: T) -> bool {
    xs.contains(&x)
}

// ── String parsing ────────────────────────────────────────────────────────────

/// Parse `s` as a signed 64-bit integer.
///
/// Returns `Ok(n)` on success, `Err(msg)` with a human-readable message on failure.
/// MVL method syntax: `s.parse_int()`.
pub fn str_parse_int(s: String) -> Result<i64, String> {
    s.trim().parse::<i64>().map_err(|e| e.to_string())
}

/// Parse `s` as a 64-bit float.
///
/// Returns `Ok(x)` on success, `Err(msg)` with a human-readable message on failure.
/// MVL method syntax: `s.parse_float()`.
pub fn str_parse_float(s: String) -> Result<f64, String> {
    s.trim().parse::<f64>().map_err(|e| e.to_string())
}
