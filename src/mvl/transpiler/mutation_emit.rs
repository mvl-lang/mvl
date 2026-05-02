//! Rust-specific mutation emission helpers.
//!
//! The mutation pass (`passes/mutation/transform.rs`) is already
//! target-neutral: it tracks mutation points and formats score reports,
//! but emits no Rust syntax strings (see ADR-0014).
//!
//! The Rust-specific wrapping — `match std::env::var("MVL_MUTANT") { … }`
//! dispatch blocks — is emitted inline by `transpiler/emit_exprs.rs` and
//! `transpiler/emit_stmts.rs` using data from `MutationMap`.
//!
//! # LLVM backend integration (future)
//!
//! An LLVM-side mutation pass would lower the `MutationMap` markers to
//! conditional-branch IR selecting between the original and mutant
//! computations, keyed by an env-var or compile-time flag.  No source-level
//! dispatch strings are involved.
