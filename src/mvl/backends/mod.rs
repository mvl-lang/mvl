// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Backend dispatch for MVL — defines the [`Backend`] trait and sub-modules
//! for each concrete backend.
//!
//! # Extension points
//!
//! To add a new backend:
//! 1. Add a sub-module (e.g. `pub mod wasm;`).
//! 2. Implement the `Backend` trait for your emitter type.
//! 3. Wire it up in `src/main.rs` via `parse_backend`.

#[cfg(feature = "llvm")]
pub mod llvm;
pub mod rust;

use crate::mvl::parser::ast::Program;

/// Common interface shared by all MVL code-generation backends.
///
/// Each backend receives a checked program (plus any prelude programs) and
/// produces output that can be compiled or executed.  The trait is intentionally
/// minimal; specialised functionality (coverage, MC/DC, mutation) lives on the
/// concrete backend types and is called directly from `src/main.rs`.
pub trait Backend {
    /// Human-readable backend identifier (matches the `--backend=` flag value).
    fn name(&self) -> &'static str;

    /// File extension for generated source files (without leading dot).
    fn file_extension(&self) -> &'static str;

    /// Emit a single-file program to a source string.
    ///
    /// `crate_name` is used as the Rust crate/module name for the Rust backend
    /// and ignored by the LLVM backend.
    fn emit_program(&self, prog: &Program, crate_name: &str) -> String;
}
