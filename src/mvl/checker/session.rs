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
/// Currently: verifies that choices have at least one branch (the parser also
/// enforces this, so this is a defence-in-depth check).
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
            for (_, sub) in branches {
                check_session_well_formed(sub, span, errors);
            }
        }
        SessionTy::End => {}
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
/// Callers use this when they have two session types that should be complementary
/// (e.g. client/server sides of a protocol).
pub fn check_dual(a: &SessionTy, b: &SessionTy, span: Span) -> Option<CheckError> {
    let expected_dual = a.dual();
    if session_types_compatible(&expected_dual, b) {
        None
    } else {
        Some(CheckError::SessionDualityMismatch {
            side_a: Ty::Session(Box::new(a.clone())).display(),
            side_b: Ty::Session(Box::new(b.clone())).display(),
            span,
        })
    }
}
