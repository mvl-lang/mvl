// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! AST transformation passes — backend-agnostic instrumentation.
//!
//! Passes operate on the typed AST produced by the checker and produce an
//! instrumented AST (with markers / metadata) consumed by backends.
//!
//! # Pipeline position
//!
//! ```text
//! parser → resolver → checker → passes → backends (transpiler / codegen)
//! ```
//!
//! # Passes
//!
//! | Pass       | Analysis module          | Transform module           |
//! |------------|--------------------------|----------------------------|
//! | mono       | `mono::monomorphize`     | —                          |
//! | coverage   | —                        | `coverage::transform`      |
//! | mcdc       | `mcdc::analysis`         | `mcdc::transform`          |
//! | mutation   | —                        | `mutation::transform`      |

pub mod complexity;
pub mod coverage;
pub mod ghost_erasure;
pub mod mcdc;
pub mod mono;
pub mod mutation;
