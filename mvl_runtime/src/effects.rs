//! Effect marker types for MVL's capability/effect system.
//!
//! MVL Requirement 5: all side-effects are declared in function signatures.
//! These zero-sized types are compile-time markers — they carry no runtime cost.
//!
//! # Usage in generated code
//!
//! ```rust
//! use mvl_runtime::prelude::*;
//!
//! // MVL: fn log(msg: String) ! Console
//! // Generated Rust:
//! fn log(_effect: &Console, msg: String) { println!("{msg}"); }
//! ```
//!
//! Note: Phase 1 uses these as doc/comment markers. Phase 2 will enforce
//! capability passing in the type system.

/// Marks a function that may write to standard output (print/println).
#[derive(Debug, Clone, Copy, Default)]
pub struct Console;

/// Marks a function that may read from the filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileRead;

/// Marks a function that may write to the filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileWrite;

/// Marks a function that may delete files or directories from the filesystem.
#[derive(Debug, Clone, Copy, Default)]
pub struct FileDelete;

/// Marks a function that may perform network I/O.
#[derive(Debug, Clone, Copy, Default)]
pub struct Net;

/// Marks a function that may access a database.
#[derive(Debug, Clone, Copy, Default)]
pub struct Db;

/// Marks a function that may spawn threads or use async runtimes.
///
/// Named `Concurrent` rather than `Async` to avoid confusion with Rust's
/// `async`/`await` keyword — this is a capability marker, not a syntax concept.
#[derive(Debug, Clone, Copy, Default)]
pub struct Concurrent;

/// Marks a function that may allocate heap memory (for bounded-heap analysis).
#[derive(Debug, Clone, Copy, Default)]
pub struct Alloc;

/// Marks a function that may panic (useful for totality tracking).
#[derive(Debug, Clone, Copy, Default)]
pub struct Panic;

/// Marks a function that performs raw terminal control (cursor positioning, colors,
/// single-keypress input, screen clear).
///
/// Distinct from `Console` (line-oriented stdin/stdout) — a function with `! Terminal`
/// controls the screen; a function with `! Console` prints lines.
/// Used by `std.tui` / future `pkg.tui` (#174).
#[derive(Debug, Clone, Copy, Default)]
pub struct Terminal;

/// Marks a function that may emit structured log records.
///
/// A function without `! Log` provably never logs — the compiler guarantees silence.
/// IFC invariant: `Secret<T>` arguments are rejected at compile time (OWASP A07).
/// See `std.log` and issue #54.
#[derive(Debug, Clone, Copy, Default)]
pub struct Log;

/// Marks a function that reads or modifies process environment variables,
/// the working directory, or Unix signal disposition.
/// See `std.env` and issue #414.
#[derive(Debug, Clone, Copy, Default)]
pub struct Env;

/// Marks a function that spawns child processes or communicates with them.
/// See `std.process` and issue #414.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessSpawn;

/// Marks a function that reads the wall clock or sleeps the current thread.
/// See `std.time` and issue #415.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock;

/// Marks a function that produces non-deterministic random values.
/// See `std.random` and issue #415.
#[derive(Debug, Clone, Copy, Default)]
pub struct Random;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_markers_are_zero_sized() {
        assert_eq!(std::mem::size_of::<Console>(), 0);
        assert_eq!(std::mem::size_of::<FileRead>(), 0);
        assert_eq!(std::mem::size_of::<FileWrite>(), 0);
        assert_eq!(std::mem::size_of::<Net>(), 0);
        assert_eq!(std::mem::size_of::<Db>(), 0);
        assert_eq!(std::mem::size_of::<Concurrent>(), 0);
        assert_eq!(std::mem::size_of::<Alloc>(), 0);
        assert_eq!(std::mem::size_of::<Panic>(), 0);
        assert_eq!(std::mem::size_of::<Terminal>(), 0);
        assert_eq!(std::mem::size_of::<Log>(), 0);
        assert_eq!(std::mem::size_of::<Env>(), 0);
        assert_eq!(std::mem::size_of::<ProcessSpawn>(), 0);
        assert_eq!(std::mem::size_of::<Clock>(), 0);
        assert_eq!(std::mem::size_of::<Random>(), 0);
    }
}
