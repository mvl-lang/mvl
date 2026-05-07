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

pub use crate::effects::{
    Alloc, Clock, Concurrent, Console, CryptoRandom, Db, Env, FileDelete, FileRead, FileWrite, Log,
    Net, Panic, ProcessSpawn, Random, Terminal,
};
pub use crate::ifc::{declassify, sanitize, Clean, Public, Secret, Tainted};
pub use crate::mvl_refine;

// ── Struct parsing infrastructure ─────────────────────────────────────────
//
// ParseFromArgs is a transpiler-generated trait: the emitter synthesises
// `impl ParseFromArgs for T` for every concrete struct with parseable fields,
// and the generated `parse_from_args()` body calls `get_arg` and `parse`.
// These are language infrastructure (not OS-specific) so they live in the
// prelude rather than being gated behind `use std.args.*`. ADR-0012.
//
// The remaining `args` functions (get_args, get_env) are OS-level and are
// only available after an explicit `use std.args.*` declaration (#488/#489).

pub use crate::stdlib::args::{get_arg, parse, ParseFromArgs};

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

// ── Secret<T>.mvl_len() ────────────────────────────────────────────────────
//
// The method dispatch traits (MvlLen, MvlPow, etc.) are no longer exported
// from the prelude — the transpiler emits them inline in each generated file
// (#554).  However, `Secret<T>` is defined in this crate and can only receive
// inherent methods here (E0116 prevents adding them in generated files).
//
// A private `MvlLen` bridge trait provides the bound without leaking it into
// generated code; the inherent `mvl_len` method is callable without importing
// the trait.

trait MvlLen {
    fn mvl_len(&self) -> i64;
}
impl<T> MvlLen for Vec<T> {
    fn mvl_len(&self) -> i64 {
        self.len() as i64
    }
}
impl<K, V> MvlLen for std::collections::HashMap<K, V> {
    fn mvl_len(&self) -> i64 {
        self.len() as i64
    }
}
impl<T> MvlLen for std::collections::HashSet<T> {
    fn mvl_len(&self) -> i64 {
        self.len() as i64
    }
}
impl MvlLen for String {
    fn mvl_len(&self) -> i64 {
        self.chars().count() as i64
    }
}

impl<T: MvlLen> crate::ifc::Secret<T> {
    /// Return the length of the inner collection as a `Secret<i64>`,
    /// propagating the IFC label so callers must `declassify` before logging.
    pub fn mvl_len(&self) -> crate::ifc::Secret<i64> {
        crate::ifc::Secret(self.0.mvl_len())
    }
}
