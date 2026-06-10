// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

// All public `extern "C"` functions accept raw pointer parameters from C/LLVM
// callers.  The unsafety is documented per-function via the Safety contract in
// their doc comments.  Clippy's `not_unsafe_ptr_arg_deref` is suppressed
// crate-wide because marking every `#[no_mangle] extern "C"` as `unsafe` would
// require unsafe blocks in all Rust unit tests without adding safety at the C
// call site (C has no notion of `unsafe`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

//! `mvl_runtime_llvm` — C-ABI stdlib for the MVL LLVM backend.
//!
//! `mvl_runtime_llvm` — C-ABI stdlib for the MVL LLVM backend (merged with `mvl_memory`).
//!
//! This crate is a `cdylib` loaded by `lli` at runtime:
//!
//! ```text
//! lli --load=libmvl_runtime_llvm.{dylib,so} program.ll
//! ```
//!
//! It wraps `mvl_runtime` Rust APIs with `#[no_mangle] extern "C"` symbols
//! so LLVM-generated code can call them.  The Rust transpiler path is
//! unaffected and continues to use `mvl_runtime` natively via the prelude.
//!
//! Heap types (MvlString, MvlArray, MvlMap) and their lifecycle functions live
//! in [`memory`]; collection operations live in [`memory_ops`].
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
//!                             stdlib via libmvl_runtime_llvm (this crate)
//! ```

#[macro_use]
pub mod macros;
pub mod abi;
pub mod actors;
pub mod memory;
pub mod memory_ops;
pub mod stdlib;
pub mod version;

// Re-export the pilot symbol at crate root for visibility in tests.
pub use version::_mvl_runtime_version;
