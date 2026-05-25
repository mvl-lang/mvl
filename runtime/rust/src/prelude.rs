// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Prelude — language fundamentals needed in every generated MVL file.
//!
//! Every file emitted by the MVL transpiler starts with:
//! ```rust
//! use mvl_runtime::prelude::*;
//! ```
//!
//! OS-specific modules (`std.io`, `std.env`, `std.process`, etc.) are NOT
//! re-exported here. The transpiler emits explicit `use mvl_runtime::stdlib::X::*`
//! imports for each `use std.X.*` declaration in the MVL source (#488 / #489).

pub use crate::actors::{
    mvl_channel, mvl_join_actors, mvl_register_actor, mvl_send, mvl_spawn, MvlJoinHandle,
    MvlReceiver, MvlSender,
};
pub use crate::capability::{ApiEndpoint, AuditTarget, ConfigPath, DbUrl};
pub use crate::ifc::{declassify, sanitize, Clean, Public, Secret, Tainted};
pub use crate::mvl_refine;

// ── Extern kernel primitives ───────────────────────────────────────────────
//
// These are the `extern "rust"` trust-boundary functions declared in
// `std/primitives.mvl`. Pure MVL stdlib wrappers (`std/strings.mvl`,
// `std/lists.mvl`) call these; they are available in every generated file via
// `use mvl_runtime::prelude::*`. ADR-0012: explicit extern bridge pattern.

/// String kernel (17 primitives — trust boundary for string stdlib methods).
pub use crate::stdlib::primitives::{
    str_byte_at, str_char_at, str_chars, str_concat, str_contains, str_ends_with, str_find,
    str_from_bytes, str_from_chars, str_len, str_parse_float, str_parse_int, str_replace,
    str_split, str_starts_with, str_substring, str_to_lower, str_to_upper, str_trim,
};

/// List kernel (6 primitives — trust boundary for list stdlib methods).
pub use crate::stdlib::primitives::{
    list_concat, list_contains, list_get, list_len, list_push, list_slice,
};

// ── Crypto tier-1 builtins ─────────────────────────────────────────────────
//
// sha256/sha512 are pure hash functions available without an explicit
// `use std.crypto.*` import (tier-1, same as format/range).
// crypto_random_bytes requires `! CryptoRandom` in the caller's signature but
// is available in scope without an explicit import, consistent with how the
// checker registers these as tier-1 builtins.

pub use crate::stdlib::crypto::{crypto_random_bytes, sha256, sha512};

// ── Core builtins ────────────────────────────────────────────────────────

/// `format(template, values)` — positional `{}` interpolation (#901).
///
/// Named `mvl_format` to avoid collision with Rust's `format!` macro.
/// The MVL transpiler emits calls to this function for `format(...)` in MVL source.
pub fn mvl_format(template: String, values: Vec<String>) -> String {
    let mut result = String::new();
    let mut val_iter = values.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'}') {
            chars.next(); // consume '}'
            if let Some(v) = val_iter.next() {
                result.push_str(v);
            }
        } else {
            result.push(c);
        }
    }
    result
}
