//! Prelude — everything a generated MVL file needs in one `use` line.
//!
//! Every file emitted by the MVL transpiler starts with:
//! ```rust
//! use mvl_runtime::prelude::*;
//! ```

pub use crate::effects::{
    Alloc, Clock, Concurrent, Console, Db, Env, FileDelete, FileRead, FileWrite, Log, Net, Panic,
    ProcessSpawn, Random, Terminal,
};
pub use crate::ifc::{declassify, sanitize, Clean, Public, Secret, Tainted};
pub use crate::mvl_refine;

// ── Standard library implementations ──────────────────────────────────────
//
// These re-exports provide the Rust backing for stdlib functions declared as
// stubs in `std/*.mvl`. Programs that import `use std.io.*` or `use std.args.*`
// call these directly — no per-program `bridge.rs` is needed for generic I/O.

/// `std.io` — file I/O operations.
pub use crate::stdlib::io::{join, path, read_file, read_to_string, to_string, Path};

/// `std.args` — CLI argument and environment access.
pub use crate::stdlib::args::{get_arg, get_args, get_env, parse, ParseFromArgs};

/// `std.crypto` — hashing and CSPRNG (Phase 3: real Rust backing).
pub use crate::stdlib::crypto::{crypto_random_bytes, sha256, sha512};

/// `std.log` — structured logging (Phase 2: no-op stubs).
pub use crate::stdlib::log::{log_debug, log_error, log_info, log_warn};

/// `std.env` — environment variables, working directory, Unix identity, signals.
pub use crate::stdlib::env::{
    all, args, chdir, current_dir, exit, get, getgid, getuid, remove_var, set, sighup, sigint,
    signal_ignore, signal_on, signal_reset, sigterm, sigusr1, sigusr2, Signal,
};

/// `std.process` — child process spawning and lifecycle.
pub use crate::stdlib::process::{
    exit_code, is_success, kill, spawn, stderr_read, stdin_write, stdout_read, wait, Child,
    ChildStderr, ChildStdin, ChildStdout, ExitStatus, ProcessOutput, Stdio,
};

// ── Extern kernel primitives ───────────────────────────────────────────────
//
// These are the `extern "rust"` trust-boundary functions declared in
// `std/primitives.mvl`. Pure MVL stdlib wrappers (`std/strings.mvl`,
// `std/lists.mvl`) call these; they are available in every generated file via
// `use mvl_runtime::prelude::*`. ADR-0012: explicit extern bridge pattern.

/// String kernel (17 primitives — trust boundary for string stdlib methods).
pub use crate::stdlib::primitives::{
    str_byte_at, str_char_at, str_chars, str_concat, str_contains, str_ends_with, str_find,
    str_from_bytes, str_from_chars, str_len, str_replace, str_split, str_starts_with,
    str_substring, str_to_lower, str_to_upper, str_trim,
};

/// List kernel (6 primitives — trust boundary for list stdlib methods).
pub use crate::stdlib::primitives::{
    list_concat, list_contains, list_get, list_len, list_push, list_slice,
};

// ── Higher-order method traits ─────────────────────────────────────────────
//
// These traits allow the transpiler to emit a single method name for `map`
// and `pow` across multiple types (List/Option/Result and Int/Float) without
// needing receiver-type information at emit time.

/// Uniform `map` for `Vec<T>`, `Option<T>`, and `Result<T, E>`.
///
/// The transpiler emits `receiver.mvl_map(|__x| f(__x.clone()))` for all MVL
/// `.map(f)` calls.  Rust resolves the correct impl via type inference.
pub trait MvlMap {
    /// The element type being mapped over.
    type Inner;
    /// The container type after mapping to element type `U`.
    type Mapped<U>;
    /// Apply `f` to each element, returning a new container of the same shape.
    fn mvl_map<U, F: FnMut(Self::Inner) -> U>(self, f: F) -> Self::Mapped<U>;
}

impl<T> MvlMap for Vec<T> {
    type Inner = T;
    type Mapped<U> = Vec<U>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, f: F) -> Vec<U> {
        self.into_iter().map(f).collect()
    }
}

impl<T> MvlMap for Option<T> {
    type Inner = T;
    type Mapped<U> = Option<U>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, f: F) -> Option<U> {
        self.map(f)
    }
}

impl<T, E> MvlMap for Result<T, E> {
    type Inner = T;
    type Mapped<U> = Result<U, E>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, f: F) -> Result<U, E> {
        self.map(f)
    }
}

/// Uniform `contains` check across `Vec<T>`, `String`, and `HashSet<T>`.
///
/// The transpiler emits `receiver.mvl_contains(&(...))` for all MVL
/// `.contains(x)` calls (see `emit_exprs.rs`).  This lets the same method
/// syntax work on all three container types without requiring type information
/// at codegen time.  Corresponds to the `list_contains` / `str_contains`
/// extern primitives declared in `std/primitives.mvl`.
///
/// Note: constraint requirements differ per impl — `Vec<T>` needs `T: PartialEq`,
/// `HashSet<T>` needs `T: Eq + Hash` (required by the underlying collection).
/// MVL's `Set<T>` therefore requires `T` to be both `Eq` and `Hash`.
///
/// Keep in sync with the inline strings in `src/mvl/transpiler/emit_types.rs`.
pub trait MvlContains<T: ?Sized> {
    /// Return `true` if `self` contains `x`.
    fn mvl_contains(&self, x: &T) -> bool;
}

impl<T: PartialEq> MvlContains<T> for Vec<T> {
    fn mvl_contains(&self, x: &T) -> bool {
        self.contains(x)
    }
}

impl MvlContains<String> for String {
    fn mvl_contains(&self, x: &String) -> bool {
        self.contains(x.as_str())
    }
}

impl MvlContains<str> for String {
    fn mvl_contains(&self, x: &str) -> bool {
        self.contains(x)
    }
}

impl<T: Eq + std::hash::Hash> MvlContains<T> for std::collections::HashSet<T> {
    fn mvl_contains(&self, x: &T) -> bool {
        self.contains(x)
    }
}

/// Uniform `pow` for `i64` (uses `u32` exponent cast) and `f64` (uses `powf`).
///
/// The transpiler emits `receiver.mvl_pow(arg.clone())` for all MVL `.pow(e)`
/// calls.  This fixes `i64::pow`'s `u32`-exponent requirement while also
/// supporting `f64::powf`.
pub trait MvlPow {
    /// Raise `self` to the power `exp`.
    fn mvl_pow(self, exp: Self) -> Self;
}

impl MvlPow for i64 {
    fn mvl_pow(self, exp: i64) -> i64 {
        self.pow(exp as u32)
    }
}

impl MvlPow for f64 {
    fn mvl_pow(self, exp: f64) -> f64 {
        self.powf(exp)
    }
}
