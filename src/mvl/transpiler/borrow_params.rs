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
//!
//! Parameters of Copy types (Int, Float, Bool, Byte, Unit, and their labeled
//! or refined wrappers) are never inferred as borrows — cloning them is free
//! in Rust so there is no performance benefit.

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, FnDecl, LValue, MatchBody, Param, Program, Stmt, TypeExpr,
};

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
/// Explicit `&T` / `&mut T` annotations are detected first; for remaining
/// non-Copy owned parameters, conservative read-only body analysis infers
/// whether they can be passed as `&T`.
pub fn borrow_params_for_fn(fd: &FnDecl) -> Vec<Option<bool>> {
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
            args.is_empty() && matches!(name.as_str(), "Int" | "Float" | "Bool" | "Byte" | "Unit")
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

        // For-loop: the iterable is wrapped in `.clone()` at emit time, so
        // `for x in param` is safe — the param is not consumed.  We still
        // check the iter expression for nested disqualifying uses (e.g. if
        // `iter` is `f(param, …)`).
        Stmt::For { iter, body, .. } => {
            expr_has_disqualifying_use(param, iter) || block_has_disqualifying_use(param, body)
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
/// * `for x in param` — for-loop iter (cloned at emit time)
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
        Expr::Sanitize { expr, .. }
        | Expr::Declassify { expr, .. }
        | Expr::Move { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. } => {
            matches!(expr.as_ref(), Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, expr)
        }

        // Unary operators: treat the same way — any direct reference to the
        // param as the sole operand is a value-level use.
        Expr::Unary { expr, .. } => {
            matches!(expr.as_ref(), Expr::Ident(n, _) if n == param)
                || expr_has_disqualifying_use(param, expr)
        }

        Expr::Binary { left, right, .. } => {
            expr_has_disqualifying_use(param, left) || expr_has_disqualifying_use(param, right)
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

        Expr::Lambda { body, .. } => expr_has_disqualifying_use(param, body),

        // Leaves that are never disqualifying at this level:
        // Expr::Ident, Expr::Literal — checked by the caller at the specific
        // position (arg, field, scrutinee, …) rather than here, so that
        // `param.method()` (receiver) is not flagged.
        _ => false,
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
    fn owned_copy_param_is_not_borrow() {
        // Int is Copy — no benefit to borrowing, so always None.
        let fd = parse_fn("fn f(x: Int) -> Int { x }");
        let flags = borrow_params_for_fn(&fd);
        assert_eq!(flags, vec![None]);
    }

    #[test]
    fn owned_copy_read_only_param_stays_none() {
        // Even a read-only Int param stays owned — Int is Copy.
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

    // ── Borrow inference: inferred as &T ─────────────────────────────────

    #[test]
    fn list_param_unused_in_body_inferred_as_borrow() {
        // xs is never used → trivially read-only → &T.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { 0 }");
        assert_eq!(borrow_params_for_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn list_param_used_only_in_for_iter_inferred_as_borrow() {
        // for-loop iter is cloned at emit time, so using xs there is safe.
        let fd = parse_fn(
            "fn sum(xs: List[Int]) -> Int { let mut acc = 0; for x in xs { acc = acc } acc }",
        );
        assert_eq!(borrow_params_for_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn list_param_used_only_as_method_receiver_inferred_as_borrow() {
        // Method call receiver auto-derefs → not disqualifying.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { xs.len() }");
        assert_eq!(borrow_params_for_fn(&fd), vec![Some(false)]);
    }

    #[test]
    fn struct_param_used_only_via_field_access_inferred_as_borrow() {
        // Field access on a struct auto-derefs in Rust.
        let fd = parse_fn("fn show(p: Point) -> Int { p.x }");
        assert_eq!(borrow_params_for_fn(&fd), vec![Some(false)]);
    }

    // ── Borrow inference: disqualified → None ────────────────────────────

    #[test]
    fn param_returned_directly_is_not_borrow() {
        // Returning xs would cause a type mismatch if xs: &Vec<i64>.
        let fd = parse_fn("fn f(xs: List[Int]) -> List[Int] { xs }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_passed_to_fn_call_is_not_borrow() {
        // xs passed directly to helper() → cannot guarantee callee owns it.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { helper(xs) }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_used_as_match_scrutinee_is_not_borrow() {
        // match xs { … } moves xs — cannot borrow.
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { match xs { _ => 0 } }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_stored_in_struct_field_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Wrapper { Wrapper { data: xs } }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_in_list_literal_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> List[List[Int]] { [xs] }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_moved_into_let_binding_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { let ys = xs; 0 }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_assigned_to_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> Int { xs = []; 0 }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    #[test]
    fn param_returned_via_explicit_return_is_not_borrow() {
        let fd = parse_fn("fn f(xs: List[Int]) -> List[Int] { return xs; xs }");
        assert_eq!(borrow_params_for_fn(&fd), vec![None]);
    }

    // ── Map building ─────────────────────────────────────────────────────

    #[test]
    fn owned_copy_only_fn_absent_from_map() {
        let prog = parse_prog("fn f(x: Int) -> Int { x }");
        let map = build_borrow_params_map(&prog, &[]);
        assert!(
            !map.contains_key("f"),
            "copy-only fn must not appear in borrow_params_map"
        );
    }

    #[test]
    fn inferred_borrow_fn_present_in_map() {
        let prog = parse_prog("fn sum(xs: List[Int]) -> Int { 0 }");
        let map = build_borrow_params_map(&prog, &[]);
        assert_eq!(map.get("sum"), Some(&vec![Some(false)]));
    }

    #[test]
    fn prelude_fn_with_ref_param_appears_in_map() {
        let prelude_fn = parse_fn("fn print_ref(x: &Int) -> Unit { }");
        let prog = parse_prog("");
        let map = build_borrow_params_map(&prog, &[&prelude_fn]);
        assert_eq!(map.get("print_ref"), Some(&vec![Some(false)]));
    }
}
