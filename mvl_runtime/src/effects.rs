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

/// Marks a function that may perform network I/O.
#[derive(Debug, Clone, Copy, Default)]
pub struct Net;

/// Marks a function that may access a database.
#[derive(Debug, Clone, Copy, Default)]
pub struct Db;

/// Marks a function that may spawn threads or use async runtimes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Async;

/// Marks a function that may allocate heap memory (for bounded-heap analysis).
#[derive(Debug, Clone, Copy, Default)]
pub struct Alloc;

/// Marks a function that may panic (useful for totality tracking).
#[derive(Debug, Clone, Copy, Default)]
pub struct Panic;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_markers_are_zero_sized() {
        assert_eq!(std::mem::size_of::<Console>(), 0);
        assert_eq!(std::mem::size_of::<FileRead>(), 0);
        assert_eq!(std::mem::size_of::<Net>(), 0);
        assert_eq!(std::mem::size_of::<Db>(), 0);
    }
}
