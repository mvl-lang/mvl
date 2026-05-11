// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Ghost erasure — strips `ghost let` bindings before transpilation/codegen (Phase 4, #627).
//!
//! Ghost bindings (`ghost let x: T = expr`) are specification-only:
//! they are type-checked normally but must never appear in emitted code.
//!
//! # Erasure strategy
//!
//! Ghost erasure is implemented at the backend level: each backend simply skips
//! `Stmt::Let { kind: LetKind::Ghost, .. }` nodes when emitting statements.  This is
//! simpler than a full AST transformation pass and produces the same result
//! because ghost lets have no runtime effect.
//!
//! # `old(e)` expressions
//!
//! `RefExpr::Old { inner, .. }` in `ensures` predicates is handled similarly:
//! the backends treat `old(x)` as `x` (the current parameter value).  Full
//! entry-time capture is deferred to a future enhancement.
//!
//! # This module
//!
//! This module serves as the canonical documentation point for the ghost erasure
//! strategy.  Backends import from `crate::mvl::parser::ast::{LetKind, Stmt}` directly
//! and pattern-match on `LetKind::Ghost`.

use crate::mvl::parser::ast::LetKind;

/// Returns `true` when a `Stmt::Let` should be erased (i.e. not emitted by backends).
///
/// Ghost lets are always `LetKind::Ghost` by construction — the invalid state
/// `ghost + mutable` is unrepresentable in [`LetKind`] (#651).
///
/// Usage in backends:
/// ```ignore
/// if is_ghost_let(kind) { return; }
/// ```
#[allow(dead_code)]
pub fn is_ghost_let(kind: &LetKind) -> bool {
    matches!(kind, LetKind::Ghost)
}
