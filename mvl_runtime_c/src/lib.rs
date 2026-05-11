// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

// All public `extern "C"` functions accept raw pointer parameters from C/LLVM
// callers.  The unsafety is documented per-function via the Safety contract in
// their doc comments.  Clippy's `not_unsafe_ptr_arg_deref` is suppressed
// crate-wide because marking every `#[no_mangle] extern "C"` as `unsafe` would
// require unsafe blocks in all Rust unit tests without adding safety at the C
// call site (C has no notion of `unsafe`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! `mvl_runtime_c` — C-ABI stdlib for the MVL LLVM backend.
//!
//! This crate is a `cdylib` loaded by `lli` at runtime alongside `mvl_memory`:
//!
//! ```text
//! lli --load=libmvl_memory.{dylib,so} \
//!     --load=libmvl_runtime_c.{dylib,so} \
//!     program.ll
//! ```
//!
//! It wraps `mvl_runtime` Rust APIs with `#[no_mangle] extern "C"` symbols
//! so LLVM-generated code can call them.  The Rust transpiler path is
//! unaffected and continues to use `mvl_runtime` natively via the prelude.
//!
//! Collection operations (mvl_string_len, mvl_array_push, mvl_map_get, …)
//! live in [`memory_ops`]; `mvl_memory` retains only types + lifecycle (#490).
//!
//! # Architecture
//!
//! See ADR-0019 for the two-path design rationale.
//!
//! ```text
//! Path 1 (Rust transpiler):  MVL → Rust source → cargo/rustc
//!                             stdlib via `use mvl_runtime::prelude::*`
//!
//! Path 2 (LLVM backend):     MVL → LLVM IR → lli
//!                             stdlib via libmvl_runtime_c (this crate)
//! ```

#[macro_use]
pub mod macros;
pub mod abi;
pub mod memory_ops;
pub mod stdlib;
pub mod version;

// Re-export the pilot symbol at crate root for visibility in tests.
pub use version::_mvl_runtime_version;
