//! Prelude — everything a generated MVL file needs in one `use` line.
//!
//! Every file emitted by the MVL transpiler starts with:
//! ```rust
//! use mvl_runtime::prelude::*;
//! ```

pub use crate::effects::{
    Alloc, Concurrent, Console, Db, FileDelete, FileRead, FileWrite, Log, Net, Panic, Terminal,
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

/// `std.log` — structured logging (Phase 2: no-op stubs).
pub use crate::stdlib::log::{log_debug, log_error, log_info, log_warn};

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
    type Inner;
    type Mapped<U>;
    fn mvl_map<U, F: FnMut(Self::Inner) -> U>(self, f: F) -> Self::Mapped<U>;
}

impl<T> MvlMap for Vec<T> {
    type Inner = T;
    type Mapped<U> = Vec<U>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Vec<U> {
        self.into_iter().map(|x| f(x)).collect()
    }
}

impl<T> MvlMap for Option<T> {
    type Inner = T;
    type Mapped<U> = Option<U>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Option<U> {
        self.map(|x| f(x))
    }
}

impl<T, E> MvlMap for Result<T, E> {
    type Inner = T;
    type Mapped<U> = Result<U, E>;
    fn mvl_map<U, F: FnMut(T) -> U>(self, mut f: F) -> Result<U, E> {
        self.map(|x| f(x))
    }
}

/// Uniform `pow` for `i64` (uses `u32` exponent cast) and `f64` (uses `powf`).
///
/// The transpiler emits `receiver.mvl_pow(arg.clone())` for all MVL `.pow(e)`
/// calls.  This fixes `i64::pow`'s `u32`-exponent requirement while also
/// supporting `f64::powf`.
pub trait MvlPow {
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
