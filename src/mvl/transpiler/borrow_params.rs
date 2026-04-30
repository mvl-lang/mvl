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
/// its per-parameter borrow kinds.
///
/// `Some(false)` at index `i` means "pass as `&x`" (shared reference).
/// `Some(true)` means "pass as `&mut x`" (mutable reference).
/// `None` means "pass by value (clone / move as normal)".
pub fn build_borrow_params_map(
    prog: &Program,
    prelude_fns: &[&FnDecl],
) -> HashMap<String, Vec<Option<bool>>> {
    let mut map = HashMap::new();

    // Prelude functions (stdlib) — explicit &T only, no body to analyse.
    for fd in prelude_fns {
        let flags = explicit_borrow_flags(&fd.params);
        if flags.iter().any(|b| b.is_some()) {
            map.insert(fd.name.clone(), flags);
        }
    }

    // User functions — explicit + inferred.
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            let flags = borrow_params_for_fn(fd);
            if flags.iter().any(|b| b.is_some()) {
                map.insert(fd.name.clone(), flags);
            }
        }
    }

    map
}

/// Borrow kinds for a single function declaration.
///
/// The returned `Vec` has the same length as `fd.params`.
/// Currently only explicit `&T` / `&mut T` annotations are detected.
/// Read-only inference (`TODO #304`) is not yet implemented — see below.
pub fn borrow_params_for_fn(fd: &FnDecl) -> Vec<Option<bool>> {
    // TODO(#304): add read-only inference once the transpiler emits `*x` derefs
    // for inferred-borrow params inside function bodies.
    fd.params
        .iter()
        .map(|p| explicit_ref_mutability(&p.ty))
        .collect()
}

// ── Per-parameter analysis ────────────────────────────────────────────────────

/// Returns the borrow kind for an explicitly annotated reference type.
///
/// * `Some(false)` — `&T`  (shared reference)
/// * `Some(true)`  — `&mut T` (mutable reference)
/// * `None`        — `T`  (owned; not a reference)
fn explicit_ref_mutability(ty: &TypeExpr) -> Option<bool> {
    match ty {
        TypeExpr::Ref { mutable, .. } => Some(*mutable),
        _ => None,
    }
}

/// Returns the explicit borrow kinds for a list of parameters (no body analysis).
fn explicit_borrow_flags(params: &[Param]) -> Vec<Option<bool>> {
    params
        .iter()
        .map(|p| explicit_ref_mutability(&p.ty))
        .collect()
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
    fn explicit_ref_param_is_shared_borrow() {
        let fd = parse_fn("fn f(x: &Int) -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(false)]);
    }

    #[test]
    fn explicit_mut_ref_param_is_mutable_borrow() {
        let fd = parse_fn("fn f(x: &mut Int) -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(true)]);
    }

    #[test]
    fn owned_param_is_not_borrow() {
        // Inference is disabled — owned params are never auto-inferred as borrows.
        let fd = parse_fn("fn f(x: Int) -> Int { x }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![None]);
    }

    #[test]
    fn owned_read_only_param_is_not_borrow_without_explicit_annotation() {
        // Even a clearly read-only param stays owned until inference is enabled (TODO #304).
        let fd = parse_fn("fn f(x: Int) -> Bool { if x == 0 { true } else { false } }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![None]);
    }

    #[test]
    fn mixed_explicit_and_owned_params() {
        // Only the explicitly &-annotated param is a borrow.
        let fd = parse_fn("fn f(a: Int, b: &Int) -> Int { a }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![None, Some(false)]);
    }

    #[test]
    fn all_ref_params_flags_correct() {
        let fd = parse_fn("fn f(a: &Int, b: &mut Bool) -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(false), Some(true)]);
    }

    #[test]
    fn no_params_returns_empty_flags() {
        let fd = parse_fn("fn f() -> Unit { }");
        let flags = borrow_params_for_fn(&fd);
        assert!(flags.is_empty());
    }

    #[test]
    fn owned_only_fn_absent_from_map() {
        let prog = parse_prog("fn f(x: Int) -> Int { x }");
        let map = build_borrow_params_map(&prog, &[]);
        assert!(
            !map.contains_key("f"),
            "owned-only fn must not appear in borrow_params_map"
        );
    }

    #[test]
    fn prelude_fn_with_ref_param_appears_in_map() {
        let prelude_fn = parse_fn("fn print_ref(x: &Int) -> Unit { }");
        let prog = parse_prog("");
        let map = build_borrow_params_map(&prog, &[&prelude_fn]);
        assert_eq!(map.get("print_ref"), Some(&vec![Some(false)]));
    }
}
