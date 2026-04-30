//! Borrow-parameter analysis (Phase B, Spec 009 Req 2).
//!
//! For each function declaration, determines which parameters are passed by
//! reference (`&T`) rather than by value.  The result drives two
//! transformations:
//!
//! 1. **Signature**: inferred-borrow parameters are emitted as `&T` in the
//!    Rust function signature even if the MVL source declares them as `T`.
//!
//! 2. **Call sites**: arguments destined for a `&T` parameter are emitted
//!    as `&x` instead of `x.clone()`, eliminating the clone entirely.
//!
//! # Explicit vs inferred borrows
//!
//! * **Explicit**: the MVL programmer wrote `fn f(x: &T)`.  The parameter's
//!   [`TypeExpr`] is `TypeExpr::Ref { mutable: false }`.  Always a borrow.
//! * **Explicit mutable**: `fn f(x: &mut T)`.  `TypeExpr::Ref { mutable: true }`.
//!   Also a borrow (call site emits `&mut x`).
//! * **Inferred immutable borrow**: the parameter is declared as owned (`T`)
//!   but analysis proves the function body never mutates it, never stores it
//!   in a struct, and never returns it.  Safe to pass as `&T`.
//!
//! # Conservative cases
//!
//! Parameters that are passed to *other* MVL functions are excluded from
//! inference: without a fixed-point analysis we cannot guarantee that the
//! callee also expects a reference.  Such parameters keep value semantics.

use std::collections::HashMap;

use crate::mvl::parser::ast::{Decl, FnDecl, Param, Program, TypeExpr};

// ── Public API ────────────────────────────────────────────────────────────────

/// Build a map from every function name in `prog` (and all `prelude_fns`) to
/// its per-parameter borrow flags.
///
/// `true` at index `i` means "argument `i` should be passed as `&x`".
/// `false` means "pass by value (clone / move as normal)".
pub fn build_borrow_params_map(
    prog: &Program,
    prelude_fns: &[&FnDecl],
) -> HashMap<String, Vec<bool>> {
    let mut map = HashMap::new();

    // Prelude functions (stdlib) — explicit &T only, no body to analyse.
    for fd in prelude_fns {
        let flags = explicit_borrow_flags(&fd.params);
        if flags.iter().any(|&b| b) {
            map.insert(fd.name.clone(), flags);
        }
    }

    // User functions — explicit + inferred.
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            let flags = borrow_params_for_fn(fd);
            if flags.iter().any(|&b| b) {
                map.insert(fd.name.clone(), flags);
            }
        }
    }

    map
}

/// Borrow flags for a single function declaration.
///
/// The returned `Vec` has the same length as `fd.params`.
pub fn borrow_params_for_fn(fd: &FnDecl) -> Vec<bool> {
    fd.params.iter().map(|p| param_is_borrow(p, fd)).collect()
}

// ── Per-parameter analysis ────────────────────────────────────────────────────

/// A parameter is a borrow if it is explicitly typed `&T` / `&mut T`, or if
/// analysis proves it is read-only in the function body.
fn param_is_borrow(p: &Param, fd: &FnDecl) -> bool {
    if is_explicit_ref(&p.ty) {
        return true;
    }
    is_read_only_param(&p.name, fd)
}

fn is_explicit_ref(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Ref { .. })
}

/// Returns the explicit borrow flags for a list of parameters (no body analysis).
fn explicit_borrow_flags(params: &[Param]) -> Vec<bool> {
    params.iter().map(|p| is_explicit_ref(&p.ty)).collect()
}

// ── Read-only inference ───────────────────────────────────────────────────────

/// Infer whether a parameter is read-only (future work).
///
/// # Why this is currently disabled
///
/// Emitting an inferred `&T` parameter also requires the transpiler to insert
/// `*x` dereferences every time the parameter is used in a position that
/// expects `T` (arithmetic, comparisons, struct construction, etc.).
/// Without that companion change the generated Rust does not type-check.
///
/// Until the transpiler handles automatic dereferencing of inferred borrows,
/// we conservatively return `false` here, meaning only EXPLICIT `&T`
/// annotations drive borrow inference.
fn is_read_only_param(_name: &str, _fd: &FnDecl) -> bool {
    false
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse_prog(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    fn parse_fn(src: &str) -> FnDecl {
        let prog = parse_prog(src);
        match prog.declarations.into_iter().next().unwrap() {
            Decl::Fn(fd) => fd,
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn explicit_ref_param_is_borrow() {
        let fd = parse_fn("fn f(x: &Int) -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![true]);
    }

    #[test]
    fn explicit_mut_ref_param_is_borrow() {
        let fd = parse_fn("fn f(x: &mut Int) -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![true]);
    }

    #[test]
    fn owned_param_without_explicit_ref_is_not_borrow() {
        // Inference is disabled — owned params are never auto-inferred as borrows.
        let fd = parse_fn("fn f(x: Int) -> Int { x }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![false]);
    }

    #[test]
    fn owned_param_only_read_is_not_borrow_without_explicit_annotation() {
        // Even a clearly read-only param stays owned until inference is enabled.
        let fd = parse_fn("fn f(x: Int) -> Bool { if x == 0 { true } else { false } }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![false]);
    }

    #[test]
    fn mixed_explicit_and_owned_params() {
        // Only the explicitly &-annotated param is a borrow.
        let fd = parse_fn("fn f(a: Int, b: &Int) -> Int { a }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![false, true]);
    }
}
