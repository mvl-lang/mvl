// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl_runtime_tokio` — tokio-backed actor runtime for MVL programs.
//!
//! This crate provides the same interface as `mvl_runtime` (the default runtime)
//! but uses tokio channels for actor mailboxes. Selected via `--target=tokio`.
//!
//! The non-actor modules (ifc, prelude, stdlib, capability, refine) are
//! re-exported from the default `mvl_runtime` crate to avoid duplication.
//!
//! ADR-0027 §"--target selects the runtime".

// Re-export all non-actor modules from the default runtime.
// Generated code uses `use mvl_runtime::prelude::*` etc., and since this crate
// is aliased as `mvl_runtime` in the generated Cargo.toml, these re-exports
// make those imports resolve correctly.
pub use mvl_runtime::capability;
pub use mvl_runtime::ifc;
pub use mvl_runtime::prelude;
pub use mvl_runtime::refine;
pub use mvl_runtime::stdlib;

// Also re-export the macro at crate root so `use mvl_runtime::mvl_refine` works.
pub use mvl_runtime::mvl_refine;

pub mod actors;
