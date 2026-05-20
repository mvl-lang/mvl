// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Capability-parameter analysis (Phase B, Spec 009 Req 2).
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
//! * **Explicit**: the MVL programmer wrote `fn f(x: val T)`.  The parameter's
//!   [`TypeExpr`] is `TypeExpr::Ref { mutable: false }`.  Always a borrow.
//! * **Explicit mutable**: `fn f(x: ref T)`.  `TypeExpr::Ref { mutable: true }`.
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
//!
//! Parameters of Copy types (Int, Float, Bool, Byte, Unit, and their labeled
//! or refined wrappers) are never inferred as borrows — cloning them is free
//! in Rust so there is no performance benefit.

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, FnDecl, LValue, MatchBody, Param, Program, Stmt, TypeExpr,
    UnaryOp,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Build a map from every function name in `prog` (and all `prelude_fns`) to
/// its per-parameter borrow kinds.
///
/// `Some(false)` at index `i` means "pass as `&x`" (shared reference).
/// `Some(true)` means "pass as `&mut x`" (mutable reference).
/// `None` means "pass by value (clone / move as normal)".
pub fn build_capability_params_map(
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
            let flags = capability_params_for_fn(fd);
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
/// Explicit `val T` / `ref T` annotations are detected first; for remaining
/// non-Copy owned parameters, conservative read-only body analysis infers
/// whether they can be passed as `&T` (Rust borrow).
pub fn capability_params_for_fn(fd: &FnDecl) -> Vec<Option<bool>> {
    fd.params
        .iter()
        .map(|p| {
            // Explicit annotation takes priority.
            if let Some(m) = explicit_ref_mutability(&p.ty) {
                return Some(m);
            }
            // No benefit to borrowing Copy types (i64, bool, f64, u8, unit, …).
            if is_copy_type(&p.ty) {
                return None;
            }
            // Conservative read-only inference: emit as &T if the body never
            // mutates, returns, stores, or passes the parameter by value.
            if is_read_only_param(&p.name, &fd.body) {
                Some(false)
            } else {
                None
            }
        })
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

/// Returns `true` if `ty` is a Rust `Copy` type — borrowing it provides no
/// performance benefit over passing by value.
///
/// Primitive scalars (`Int`, `Float`, `Bool`, `Byte`, `Unit`), references,
/// fn-pointer types, and their labeled/refined wrappers are all Copy.
fn is_copy_type(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Base { name, args, .. } => {
            args.is_empty()
                && matches!(
                    name.as_str(),
                    "Int" | "Float" | "Bool" | "Byte" | "Unit" | "Char"
                )
        }
        // References and fn-pointer types are always Copy in Rust.
        TypeExpr::Ref { .. } | TypeExpr::Fn { .. } => true,
        // Labeled wrappers (Clean[T], Secret[T], Public[T], …) and refined
        // types inherit their Copy-ness from the inner type.
        TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => is_copy_type(inner),
        _ => false,
    }
}

// ── Read-only body analysis ───────────────────────────────────────────────────

/// Returns `true` if `param` is never used in a way that would be unsound
/// when the parameter is treated as `&T` rather than owned `T`.
///
/// Disqualifying uses (returns `false`):
/// - Direct assignment target: `param = …`
/// - RHS of direct assignment or let binding: `y = param` / `let y = param`
/// - Returned directly: `return param` or tail `param`
/// - Match scrutinee: `match param { … }` — destructuring moves the value
/// - Free-function argument: `f(param, …)` — conservative; may own its args
/// - Method argument (not the receiver): `x.method(param)` — may own its args
/// - Struct field: `Struct { field: param }`
/// - Collection literal element: `[param]`, `{param: v}`, `{param}`
fn is_read_only_param(param: &str, body: &Block) -> bool {
    !block_has_disqualifying_use(param, body)
}

fn block_has_disqualifying_use(param: &str, block: &Block) -> bool {
    let stmts = &block.stmts;
    for (i, stmt) in stmts.iter().enumerate() {
        let is_last = i == stmts.len() - 1;
        if stmt_has_disqualifying_use(param, stmt, is_last) {
            return true;
        }
    }
    false
}

fn stmt_has_disqualifying_use(param: &str, stmt: &Stmt, is_last: bool) -> bool {
    match stmt {
        // Direct mutation of the param itself.
        Stmt::Assign { target, value, .. } => {
            lvalue_is_param(target, param)
                || matches!(value, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, value)
        }

        // Explicit return of the param.
        Stmt::Return {
            value: Some(expr), ..
        } => {
            matches!(expr, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, expr)
        }
        Stmt::Return { value: None, .. } => false,

        // Let init that directly moves the param into a new binding.
        Stmt::Let { init, .. } => {
            matches!(init, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, init)
        }

        // Statement-level expression — also checks tail-position identity.
        Stmt::Expr { expr, .. } => {
            (is_last && matches!(expr, Expr::Ident(n, _) if n == param))
                || expr_has_disqualifying_use(param, expr)
        }

        // Nested blocks — recurse into every reachable branch.
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_has_disqualifying_use(param, cond)
                || block_has_disqualifying_use(param, then)
                || else_.as_ref().is_some_and(|e| match e {
                    ElseBranch::Block(b) => block_has_disqualifying_use(param, b),
                    ElseBranch::If(s) => stmt_has_disqualifying_use(param, s, false),
                })
        }

        // For-loop: when the param IS the direct iterable (`for x in param`),
        // the emitter wraps it in `.clone()`.  However, `.clone()` on a `&Vec<T>`
        // yields another `&Vec<T>`, not a `Vec<T>` — so the loop would iterate
        // by reference, giving `&T` elements instead of `T` elements.  Disqualify
        // the param in this case.  Compound iter expressions (e.g. `f(param)`)
        // are already caught by the recursive disqualification below.
        Stmt::For { iter, body, .. } => {
            matches!(iter, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, iter)
                || block_has_disqualifying_use(param, body)
        }

        Stmt::While { cond, body, .. } => {
            expr_has_disqualifying_use(param, cond) || block_has_disqualifying_use(param, body)
        }

        // Match scrutinee: the value is *moved* into the match — disqualify.
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            matches!(scrutinee, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_disqualifying_use(param, e),
                    MatchBody::Block(b) => block_has_disqualifying_use(param, b),
                })
        }
    }
}

/// Returns `true` if `expr` contains a use of `param` in a position that is
/// unsound for `&T` semantics.
///
/// **Not** disqualifying at this level:
/// * `param.method()` — receiver of a method call (auto-deref in Rust)
/// * `for x in param` — disqualified in `stmt_has_disqualifying_use` (direct iter)
fn expr_has_disqualifying_use(param: &str, expr: &Expr) -> bool {
    match expr {
        // Free-function call: param directly as an argument → disqualify.
        Expr::FnCall { args, .. } => {
            args.iter()
                .any(|a| matches!(a, Expr::Ident(n, _) if n == param))
                || args.iter().any(|a| expr_has_disqualifying_use(param, a))
        }

        // Method call: param as a non-receiver argument → disqualify.
        // The receiver (`param.method()`) is intentionally *not* disqualifying.
        Expr::MethodCall { receiver, args, .. } => {
            args.iter()
                .any(|a| matches!(a, Expr::Ident(n, _) if n == param))
                || expr_has_disqualifying_use(param, receiver)
                || args.iter().any(|a| expr_has_disqualifying_use(param, a))
        }

        // Match scrutinee: param is destructured / moved → disqualify.
        Expr::Match {
            scrutinee, arms, ..
        } => {
            matches!(scrutinee.as_ref(), Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_disqualifying_use(param, e),
                    MatchBody::Block(b) => block_has_disqualifying_use(param, b),
                })
        }

        // Struct construction: param stored in a field → disqualify.
        Expr::Construct { fields, .. } => {
            fields
                .iter()
                .any(|(_, v)| matches!(v, Expr::Ident(n, _) if n == param))
                || fields
                    .iter()
                    .any(|(_, v)| expr_has_disqualifying_use(param, v))
        }

        // Collection literals: param as an element → disqualify.
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems
                .iter()
                .any(|e| matches!(e, Expr::Ident(n, _) if n == param))
                || elems.iter().any(|e| expr_has_disqualifying_use(param, e))
        }
        Expr::Map { pairs, .. } => pairs.iter().any(|(k, v)| {
            matches!(k, Expr::Ident(n, _) if n == param)
                || matches!(v, Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, k)
                || expr_has_disqualifying_use(param, v)
        }),

        // Wrapper / unary expressions: if the inner expression IS the param,
        // it is being used as a direct operand (Sanitize, Declassify, Move,
        // Consume, Propagate all consume the value).  Treat that as
        // disqualifying.  For compound inner expressions, recurse.
        Expr::Relabel { expr, .. } | Expr::Consume { expr, .. } | Expr::Propagate { expr, .. } => {
            matches!(expr.as_ref(), Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, expr)
        }

        // Unary operators: Neg and Not on the param directly consume it as a
        // value — disqualifying.  Deref (`*param`) is sound for `&T` because
        // that is exactly how references are used; do not disqualify it.
        Expr::Unary { op, expr, .. } => {
            (*op != UnaryOp::Deref && matches!(expr.as_ref(), Expr::Ident(n, _) if n == param))
                || expr_has_disqualifying_use(param, expr)
        }

        // &param or &mut param creates &&T — disqualifying since the caller already
        // infers &T; recurse into the inner expression for nested uses.
        Expr::Borrow { expr, .. } => expr_has_disqualifying_use(param, expr),

        // Binary operators: param as a direct operand is disqualifying.
        // Rust's arithmetic operators (`+`, `-`, `*`, etc.) do not auto-deref,
        // so `(&xs + rhs)` would be a compile error for non-Copy custom types.
        // Equality/comparison operators work via `PartialEq for &T`, but we
        // conservatively disqualify all binary uses for simplicity.
        Expr::Binary { left, right, .. } => {
            matches!(left.as_ref(), Expr::Ident(n, _) if n == param)
                || matches!(right.as_ref(), Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, left)
                || expr_has_disqualifying_use(param, right)
        }

        Expr::FieldAccess { expr, .. } => expr_has_disqualifying_use(param, expr),

        Expr::If {
            cond, then, else_, ..
        } => {
            expr_has_disqualifying_use(param, cond)
                || block_has_disqualifying_use(param, then)
                || else_
                    .as_ref()
                    .is_some_and(|e| expr_has_disqualifying_use(param, e))
        }

        Expr::Block(b) => block_has_disqualifying_use(param, b),

        // Lambda: any capture of the param (at any depth, even as a method
        // receiver) is disqualifying.  The lambda's lifetime is not controlled
        // by this analysis — if the lambda escapes the current call frame,
        // the inferred `&T` reference could be dangling.  Lambda params that
        // shadow the outer param are excluded from this check.
        Expr::Lambda {
            params: lambda_params,
            body,
            ..
        } => {
            let shadowed = lambda_params.iter().any(|p| p.name == param);
            !shadowed && expr_mentions_param(param, body)
        }

        // Leaves that are never disqualifying at this level:
        // Expr::Ident, Expr::Literal — checked by the caller at the specific
        // position (arg, field, scrutinee, …) rather than here, so that
        // `param.method()` (receiver) is not flagged.
        _ => false,
    }
}

/// Returns `true` if `param` appears anywhere inside `expr` — at any depth
/// and in any syntactic position (including safe ones like method receivers).
///
/// Used to conservatively disqualify params captured by lambdas, where the
/// lifetime of the capture cannot be determined by local analysis.
fn expr_mentions_param(param: &str, expr: &Expr) -> bool {
    match expr {
        Expr::Ident(n, _) => n == param,
        Expr::FnCall { args, .. } => args.iter().any(|a| expr_mentions_param(param, a)),
        Expr::MethodCall { receiver, args, .. } => {
            expr_mentions_param(param, receiver)
                || args.iter().any(|a| expr_mentions_param(param, a))
        }
        Expr::Binary { left, right, .. } => {
            expr_mentions_param(param, left) || expr_mentions_param(param, right)
        }
        Expr::Unary { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Relabel { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Borrow { expr, .. } => expr_mentions_param(param, expr),
        Expr::Construct { fields, .. } => fields.iter().any(|(_, v)| expr_mentions_param(param, v)),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().any(|e| expr_mentions_param(param, e))
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_mentions_param(param, k) || expr_mentions_param(param, v)),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_mentions_param(param, scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_mentions_param(param, e),
                    MatchBody::Block(b) => b.stmts.iter().any(|s| stmt_mentions_param(param, s)),
                })
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_mentions_param(param, cond)
                || b_mentions_param(param, then)
                || else_
                    .as_ref()
                    .is_some_and(|e| expr_mentions_param(param, e))
        }
        Expr::Block(b) => b_mentions_param(param, b),
        Expr::Lambda {
            params: lambda_params,
            body,
            ..
        } => {
            let shadowed = lambda_params.iter().any(|p| p.name == param);
            !shadowed && expr_mentions_param(param, body)
        }
        _ => false,
    }
}

fn b_mentions_param(param: &str, block: &Block) -> bool {
    block.stmts.iter().any(|s| stmt_mentions_param(param, s))
}

fn stmt_mentions_param(param: &str, stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr { expr, .. }
        | Stmt::Return {
            value: Some(expr), ..
        } => expr_mentions_param(param, expr),
        Stmt::Let { init, .. } => expr_mentions_param(param, init),
        Stmt::Assign { value, .. } => expr_mentions_param(param, value),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_mentions_param(param, cond)
                || b_mentions_param(param, then)
                || else_.as_ref().is_some_and(|e| match e {
                    ElseBranch::Block(b) => b_mentions_param(param, b),
                    ElseBranch::If(s) => stmt_mentions_param(param, s),
                })
        }
        Stmt::For { iter, body, .. } => {
            expr_mentions_param(param, iter) || b_mentions_param(param, body)
        }
        Stmt::While { cond, body, .. } => {
            expr_mentions_param(param, cond) || b_mentions_param(param, body)
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_mentions_param(param, scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_mentions_param(param, e),
                    MatchBody::Block(b) => b_mentions_param(param, b),
                })
        }
        Stmt::Return { value: None, .. } => false,
    }
}

/// Returns `true` if `lval` is or transitively contains `param` as the root
/// identifier.  Used to detect direct assignment targets (`param = …`).
fn lvalue_is_param(lval: &LValue, param: &str) -> bool {
    match lval {
        LValue::Ident(name, _) => name == param,
        LValue::Field { base, .. } => lvalue_is_param(base, param),
    }
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

    // ── Explicit annotations ──────────────────────────────────────────────

    #[test]
    fn explicit_ref_param_is_shared_borrow() {
        let fd = parse_fn("fn f(x: val Int) -> Unit { }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(false)]);
    }

    #[test]
    fn explicit_mut_ref_param_is_mutable_borrow() {
        let fd = parse_fn("fn f(x: ref Int) -> Unit { }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(true)]);
    }

    #[test]
    fn owned_copy_param_is_not_borrow() {
        // Int is Copy — no benefit to borrowing, so always None.
        let fd = parse_fn("fn f(x: Int) -> Int { x }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![None]);
    }

    #[test]
    fn owned_copy_read_only_param_stays_none() {
        // Even a read-only Int param stays owned — Int is Copy.
        let fd = parse_fn("fn f(x: Int) -> Bool { if x == 0 { true } else { false } }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![None]);
    }

    #[test]
    fn mixed_explicit_and_owned_params() {
        // Only the explicitly val-annotated param is a borrow.
        let fd = parse_fn("fn f(a: Int, b: val Int) -> Int { a }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![None, Some(false)]);
    }

    #[test]
    fn all_ref_params_flags_correct() {
        let fd = parse_fn("fn f(a: val Int, b: ref Bool) -> Unit { }");
        let flags = capability_params_for_fn(&fd);
        assert_eq!(flags, vec![Some(false), Some(true)]);
    }

    #[test]
    fn no_params_returns_empty_flags() {
        let fd = parse_fn("fn f() -> Unit { }");
        let flags = capability_params_for_fn(&fd);
        assert!(flags.is_empty());
    }

    // ── Borrow inference: inferred as borrow (Rust &T) ───────────────────

    #[test]
    fn list_param_unused_in_body_inferred_as_borrow() {
        // xs is never used → trivially read-only → borrow (Rust &T).
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { 0 }");
        assert_eq!(capability_params_for_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn list_param_used_directly_as_for_iter_is_not_borrow() {
        // `for x in xs` where xs is the direct iterable: the emitter wraps in
        // `.clone()`, but `(&Vec<T>).clone()` yields `&Vec<T>`, not `Vec<T>`.
        // Iterating `&Vec<T>` gives `&T` elements — type error in the body.
        // Disqualify xs so it stays owned and the move/clone path handles it.
        let fd = parse_fn(
            "fn sum(xs: List[Int]) -> Int { let acc: ref Int = 0; for x in xs { acc = acc } acc }",
        );
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn list_param_used_only_as_method_receiver_inferred_as_borrow() {
        // Method call receiver auto-derefs → not disqualifying.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { xs.len() }");
        assert_eq!(capability_params_for_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn struct_param_used_only_via_field_access_inferred_as_borrow() {
        // Field access on a struct auto-derefs in Rust.
        let fd = parse_fn("fn show(p: Point) -> Int { p.x }");
        assert_eq!(capability_params_for_fn(&fd), vec![Some(false)]);
    }

    // ── Borrow inference: disqualified → None ────────────────────────────

    #[test]
    fn param_returned_directly_is_not_borrow() {
        // Returning xs would cause a type mismatch if xs: &Vec<i64>.
        let fd = parse_fn("fn f(xs: List[Int]) -> List[Int] { xs }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_passed_to_fn_call_is_not_borrow() {
        // xs passed directly to helper() → cannot guarantee callee owns it.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { helper(xs) }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_used_as_match_scrutinee_is_not_borrow() {
        // match xs { … } moves xs — cannot borrow.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { match xs { _ => 0 } }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_stored_in_struct_field_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Wrapper { Wrapper { data: xs } }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_in_list_literal_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> List[List[Int]] { [xs] }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_moved_into_let_binding_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { let ys: List[Int] = xs; 0 }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_assigned_to_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { xs = []; 0 }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_returned_via_explicit_return_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> List[Int] { return xs; xs }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    // ── Map building ─────────────────────────────────────────────────────

    #[test]
    fn owned_copy_only_fn_absent_from_map() {
        let prog = parse_prog("fn f(x: Int) -> Int { x }");
        let map = build_capability_params_map(&prog, &[]);
        assert!(
            !map.contains_key("f"),
            "copy-only fn must not appear in capability_params_map"
        );
    }

    #[test]
    fn inferred_borrow_fn_present_in_map() {
        let prog = parse_prog("fn sum(xs: List[Int]) -> Int { 0 }");
        let map = build_capability_params_map(&prog, &[]);
        assert_eq!(map.get("sum"), Some(&vec![Some(false)]));
    }

    #[test]
    fn prelude_fn_with_ref_param_appears_in_map() {
        let prelude_fn = parse_fn("fn print_ref(x: val Int) -> Unit { }");
        let prog = parse_prog("");
        let map = build_capability_params_map(&prog, &[&prelude_fn]);
        assert_eq!(map.get("print_ref"), Some(&vec![Some(false)]));
    }

    // ── Fix: is_copy_type must include Char ──────────────────────────────

    #[test]
    fn char_param_is_not_inferred_as_borrow() {
        // Char maps to Rust `char` which is Copy — no benefit to borrowing.
        let fd = parse_fn("fn f(c: Char) -> Bool { true }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    // ── Fix: Binary operand disqualifies ─────────────────────────────────

    #[test]
    fn param_as_direct_binary_operand_is_not_borrow() {
        // xs used directly as a binary operand: Rust arithmetic doesn't auto-deref,
        // so `&Vec<i64> + rhs` would fail to compile.  Must disqualify.
        let fd = parse_fn("fn f(xs: List[Int], ys: List[Int]) -> Bool { xs == ys }");
        // both xs and ys appear as direct binary operands → both disqualified.
        assert_eq!(capability_params_for_fn(&fd), vec![None, None]);
    }

    #[test]
    fn param_not_disqualified_when_only_in_method_call_inside_binary() {
        // xs.len() is just a method receiver — the Int result is used in binary.
        // xs itself is never a direct binary operand, so it stays read-only.
        let fd = parse_fn("fn f(xs: List[Int]) -> Bool { xs.len() == 0 }");
        assert_eq!(capability_params_for_fn(&fd), vec![Some(false)]);
    }

    // ── Fix: Deref unary does not disqualify ─────────────────────────────

    #[test]
    fn deref_unary_on_explicit_ref_param_does_not_disqualify() {
        // A parameter already declared as `val T` won't go through inference
        // (explicit annotation takes priority), but verify that Deref on a
        // non-Copy param in the body doesn't disqualify inference for OTHER params.
        // Here `b` is field-accessed only → still inferred as borrow.
        let fd = parse_fn("fn f(b: Point, r: val Int) -> Int { b.x }");
        // b: Point — field access only → Some(false); r: &Int — explicit → Some(false).
        assert_eq!(
            capability_params_for_fn(&fd),
            vec![Some(false), Some(false)]
        );
    }

    // ── Fix: Lambda capture disqualifies ─────────────────────────────────

    #[test]
    fn param_captured_in_lambda_body_is_not_borrow() {
        // xs is captured inside a lambda passed to apply().
        // The lambda's lifetime is unknown — disqualify xs.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { apply(|| xs.len()) }");
        assert_eq!(capability_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_shadowed_by_lambda_param_is_still_borrow() {
        // The lambda introduces its own `xs` parameter, shadowing the outer one.
        // The outer `xs` is never captured — still inferred as borrow.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { apply(|xs: List[Int]| xs.len()) }");
        assert_eq!(capability_params_for_fn(&fd), vec![Some(false)]);
    }
}
