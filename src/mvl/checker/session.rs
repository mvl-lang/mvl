// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Session type checker (Honda 1993, Phase 8 — Issue #260).
//!
//! Verifies structural correctness of session type declarations and, when two
//! type aliases are annotated as a dual pair, checks that one is the dual of
//! the other.
//!
//! # What is checked
//!
//! 1. **Structural well-formedness**: every `SessionOp` tree resolves cleanly
//!    to a `SessionTy` (handled by `resolve_session_op` in `types.rs`).
//!
//! 2. **Duality annotation** (opt-in): if two type aliases carry a
//!    `// dual: OtherProtocol` doc-comment convention, the checker verifies
//!    `dual(A) == B`.  This is wired into the `check_decl` pass by
//!    `check_session_duality_annotations`.
//!
//! 3. **Choice branch non-emptiness**: a `+{}` or `&{}` with no branches is
//!    a parse error (caught in the parser), so no additional check is needed.
//!
//! # Architecture note
//!
//! Session type *protocol compliance* (verifying that channel operations at
//! call sites match the declared protocol step-by-step) requires linear/typestate
//! tracking that is planned for a follow-on issue.  This module provides the
//! foundation: duality and structural checks on type declarations.

use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::{resolve_session_op, session_types_compatible, SessionTy, Ty};
use crate::mvl::parser::ast::{Decl, Program, TypeBody, TypeExpr};
use crate::mvl::parser::lexer::Span;

/// Check all session type declarations in a program for duality consistency.
///
/// A duality annotation is expressed by declaring two type aliases where one
/// is the exact dual of the other. We detect this by checking every pair of
/// session-type aliases and reporting when `dual(A) == B` fails for explicitly
/// paired declarations.
///
/// Currently we check: if a type alias resolves to a session type, verify
/// that the session type is structurally non-empty (has at least one step or
/// ends with `end`).  Explicit duality pairing (via attribute or naming
/// convention) is left for a future pass once the attribute syntax is settled.
pub fn check_session_types(prog: &Program, errors: &mut Vec<CheckError>) {
    for decl in &prog.declarations {
        if let Decl::Type(td) = decl {
            if let TypeBody::Alias(ty_expr) = &td.body {
                if let Some(s) = extract_session_ty(ty_expr) {
                    check_session_well_formed(&s, ty_expr.span(), errors);
                }
            }
        }
    }
}

/// Check that a `SessionTy` is well-formed.
///
/// Checks:
/// 1. Choices have at least one branch (defence-in-depth; parser also enforces this).
/// 2. No duplicate branch labels within a single choice block.
fn check_session_well_formed(s: &SessionTy, span: Span, errors: &mut Vec<CheckError>) {
    match s {
        SessionTy::Send(_, cont) | SessionTy::Receive(_, cont) => {
            check_session_well_formed(cont, span, errors);
        }
        SessionTy::InternalChoice(branches) | SessionTy::ExternalChoice(branches) => {
            if branches.is_empty() {
                errors.push(CheckError::SessionProtocolMismatch {
                    expected: "at least one choice branch".to_string(),
                    found: "empty choice {}".to_string(),
                    span,
                });
            }
            check_duplicate_labels(branches, span, errors);
            for (_, sub) in branches {
                check_session_well_formed(sub, span, errors);
            }
        }
        SessionTy::End => {}
    }
}

/// Detect duplicate branch labels within a choice block.
///
/// Duplicate labels produce unreachable states: only the first matching label
/// is ever selected, making subsequent identical labels dead code.
fn check_duplicate_labels(
    branches: &[(String, SessionTy)],
    span: Span,
    errors: &mut Vec<CheckError>,
) {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    for (label, _) in branches {
        if !seen.insert(label.as_str()) {
            errors.push(CheckError::SessionDuplicateLabel {
                label: label.clone(),
                span,
            });
        }
    }
}

/// Check that a declared dual pair `(a, b)` has no mutual-blocking deadlock.
///
/// Walks the product state `(a, b)` and reports [`CheckError::SessionDeadlock`]
/// if there is any reachable state where both sides are simultaneously in
/// `Receive` (neither will send first → infinite wait).
///
/// Returns `Some(error)` on the first deadlock found, `None` if the pair is
/// deadlock-free (or if the types are structurally incompatible — duality
/// mismatches are handled separately by [`check_dual`]).
// Note: `SessionTy` is acyclic (there is no mu-binder / recursive type alias in the
// current type system), so this recursion always terminates.  If recursive session
// types are ever introduced, add a visited-state set or depth counter here.
pub fn check_no_mutual_blocking(a: &SessionTy, b: &SessionTy, span: Span) -> Option<CheckError> {
    match (a, b) {
        // Both sides waiting to receive — nobody sends first: deadlock.
        (SessionTy::Receive(..), SessionTy::Receive(..)) => {
            Some(CheckError::SessionDeadlock { span })
        }
        // Normal progress: a sends while b receives, or vice versa.
        (SessionTy::Send(_, a_cont), SessionTy::Receive(_, b_cont)) => {
            check_no_mutual_blocking(a_cont, b_cont, span)
        }
        (SessionTy::Receive(_, a_cont), SessionTy::Send(_, b_cont)) => {
            check_no_mutual_blocking(a_cont, b_cont, span)
        }
        // One side picks a branch (internal), the other handles it (external).
        // Walk only labels present in both sides; labels present in one side but
        // absent from the other are a structural mismatch caught by check_dual,
        // not a deadlock.
        (SessionTy::InternalChoice(a_branches), SessionTy::ExternalChoice(b_branches))
        | (SessionTy::ExternalChoice(a_branches), SessionTy::InternalChoice(b_branches)) => {
            for (label, a_sub) in a_branches {
                if let Some((_, b_sub)) = b_branches.iter().find(|(l, _)| l == label) {
                    if let Some(e) = check_no_mutual_blocking(a_sub, b_sub, span) {
                        return Some(e);
                    }
                }
            }
            None
        }
        // Both sides complete — no deadlock.
        (SessionTy::End, SessionTy::End) => None,
        // Structural mismatch: caught by check_dual, not a deadlock.
        _ => None,
    }
}

/// If `te` is a `TypeExpr::Session`, return its resolved `SessionTy`.
fn extract_session_ty(te: &TypeExpr) -> Option<SessionTy> {
    match te {
        TypeExpr::Session { op, .. } => Some(resolve_session_op(op)),
        _ => None,
    }
}

/// Verify that `a` and `b` are duals of each other.
///
/// Returns `Some(error)` if the duality check fails, `None` if they are duals.
/// When the pair is not dual, a more specific [`CheckError::SessionDeadlock`]
/// is returned if both sides mutually block (both in `Receive`); otherwise
/// [`CheckError::SessionDualityMismatch`] is returned.
pub fn check_dual(a: &SessionTy, b: &SessionTy, span: Span) -> Option<CheckError> {
    let expected_dual = a.dual();
    if session_types_compatible(&expected_dual, b) {
        None
    } else {
        // Prefer the more specific deadlock error when the failure is mutual blocking.
        check_no_mutual_blocking(a, b, span).or_else(|| {
            Some(CheckError::SessionDualityMismatch {
                side_a: Ty::Session(Box::new(a.clone())).display(),
                side_b: Ty::Session(Box::new(b.clone())).display(),
                span,
            })
        })
    }
}
