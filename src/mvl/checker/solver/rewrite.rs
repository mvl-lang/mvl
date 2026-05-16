// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Builtin rewrite rules for Layer 3 symbolic execution.
//!
//! Rewrites `Expr::MethodCall` nodes on concrete receivers into simpler
//! expressions that Layers 1 and 2 can evaluate directly.
//!
//! Rules are **sound** — they only fire when the result is definitively known
//! from the structure of the argument alone.  No rule can produce an incorrect
//! `Proven`; at worst an inapplicable rule leaves the expression unchanged.
//!
//! ## Rules
//!
//! | Pattern                        | Rewrite                     |
//! |--------------------------------|-----------------------------|
//! | `"hello".len()`                | `Integer(5)`                |
//! | `"".is_empty()`                | `Bool(true)`                |
//! | `[a, b, c].len()`              | `Integer(3)`                |
//! | `Some(x).is_some()`            | `Bool(true)`                |
//! | `None.is_some()`               | `Bool(false)`               |
//! | `Some(x).is_none()`            | `Bool(false)`               |
//! | `None.is_none()`               | `Bool(true)`                |
//! | `Ok(x).is_ok()`                | `Bool(true)`                |
//! | `Err(e).is_ok()`               | `Bool(false)`               |
//! | `Ok(x).is_err()`               | `Bool(false)`               |
//! | `Err(e).is_err()`              | `Bool(true)`                |
//!
//! **Issue:** #596

use crate::mvl::parser::ast::{Expr, Literal};

/// Apply builtin rewrite rules to `expr`, reducing method calls on concrete
/// receivers to literal values where possible.
///
/// Recurses into sub-expressions so that nested rewrites compose correctly —
/// e.g. `Some("hello".len() > 0).is_some()` rewrites step by step.
///
/// Returns a structurally different `Expr` when a rule fires; otherwise returns
/// a clone of the input with recursively-rewritten sub-expressions.
pub(super) fn rewrite_expr(expr: &Expr) -> Expr {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => {
            // Rewrite the receiver first so nested rules compose (e.g.
            // `"".concat("x").len()` first rewrites the concat result, then len).
            let receiver = Box::new(rewrite_expr(receiver));

            match (receiver.as_ref(), method.as_str(), args.as_slice()) {
                // ── String methods ────────────────────────────────────────────

                // `"hello".len()` → `Integer(5)`
                (Expr::Literal(Literal::Str(s), _), "len", []) => {
                    Expr::Literal(Literal::Integer(s.len() as i64), *span)
                }

                // `"hello".is_empty()` → `Bool(false)`, `"".is_empty()` → `Bool(true)`
                (Expr::Literal(Literal::Str(s), _), "is_empty", []) => {
                    Expr::Literal(Literal::Bool(s.is_empty()), *span)
                }

                // ── List methods ──────────────────────────────────────────────

                // `[1, 2, 3].len()` → `Integer(3)`
                (Expr::List { elems, .. }, "len", []) => {
                    Expr::Literal(Literal::Integer(elems.len() as i64), *span)
                }

                // ── Option constructor methods ─────────────────────────────────

                // `Some(x).is_some()` → `Bool(true)`
                (Expr::FnCall { name, .. }, "is_some", []) if name == "Some" => {
                    Expr::Literal(Literal::Bool(true), *span)
                }

                // `None.is_some()` → `Bool(false)`
                (Expr::Ident(name, _), "is_some", []) if name == "None" => {
                    Expr::Literal(Literal::Bool(false), *span)
                }

                // `Some(x).is_none()` → `Bool(false)`
                (Expr::FnCall { name, .. }, "is_none", []) if name == "Some" => {
                    Expr::Literal(Literal::Bool(false), *span)
                }

                // `None.is_none()` → `Bool(true)`
                (Expr::Ident(name, _), "is_none", []) if name == "None" => {
                    Expr::Literal(Literal::Bool(true), *span)
                }

                // ── Result constructor methods ─────────────────────────────────

                // `Ok(x).is_ok()` → `Bool(true)`
                (Expr::FnCall { name, .. }, "is_ok", []) if name == "Ok" => {
                    Expr::Literal(Literal::Bool(true), *span)
                }

                // `Err(e).is_ok()` → `Bool(false)`
                (Expr::FnCall { name, .. }, "is_ok", []) if name == "Err" => {
                    Expr::Literal(Literal::Bool(false), *span)
                }

                // `Ok(x).is_err()` → `Bool(false)`
                (Expr::FnCall { name, .. }, "is_err", []) if name == "Ok" => {
                    Expr::Literal(Literal::Bool(false), *span)
                }

                // `Err(e).is_err()` → `Bool(true)`
                (Expr::FnCall { name, .. }, "is_err", []) if name == "Err" => {
                    Expr::Literal(Literal::Bool(true), *span)
                }

                // ── No applicable rule ────────────────────────────────────────

                // Reconstruct with the (possibly rewritten) receiver and
                // recursively-rewritten arguments.
                _ => Expr::MethodCall {
                    receiver,
                    method: method.clone(),
                    args: args.iter().map(rewrite_expr).collect(),
                    span: *span,
                },
            }
        }

        // ── Recursive descent into compound expressions ───────────────────────
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(rewrite_expr(left)),
            right: Box::new(rewrite_expr(right)),
            span: *span,
        },

        Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(rewrite_expr(expr)),
            span: *span,
        },

        Expr::FnCall {
            name,
            type_args,
            args,
            span,
        } => Expr::FnCall {
            name: name.clone(),
            type_args: type_args.clone(),
            args: args.iter().map(rewrite_expr).collect(),
            span: *span,
        },

        // Leaf nodes and unsupported compound nodes: return unchanged.
        other => other.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::lexer::Span;

    fn sp() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn str_lit(s: &str) -> Expr {
        Expr::Literal(Literal::Str(s.to_string()), sp())
    }

    fn int_lit(n: i64) -> Expr {
        Expr::Literal(Literal::Integer(n), sp())
    }

    fn bool_lit(b: bool) -> Expr {
        Expr::Literal(Literal::Bool(b), sp())
    }

    fn method_call(receiver: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr::MethodCall {
            receiver: Box::new(receiver),
            method: method.to_string(),
            args,
            span: sp(),
        }
    }

    fn fn_call(name: &str, args: Vec<Expr>) -> Expr {
        Expr::FnCall {
            name: name.to_string(),
            type_args: vec![],
            args,
            span: sp(),
        }
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.to_string(), sp())
    }

    fn list(elems: Vec<Expr>) -> Expr {
        Expr::List { elems, span: sp() }
    }

    // ── String length ─────────────────────────────────────────────────────────

    #[test]
    fn string_len_hello() {
        let expr = method_call(str_lit("hello"), "len", vec![]);
        assert_eq!(rewrite_expr(&expr), int_lit(5));
    }

    #[test]
    fn string_len_empty() {
        let expr = method_call(str_lit(""), "len", vec![]);
        assert_eq!(rewrite_expr(&expr), int_lit(0));
    }

    #[test]
    fn string_is_empty_true() {
        let expr = method_call(str_lit(""), "is_empty", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }

    #[test]
    fn string_is_empty_false() {
        let expr = method_call(str_lit("hi"), "is_empty", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(false));
    }

    // ── List length ───────────────────────────────────────────────────────────

    #[test]
    fn list_len_three() {
        let expr = method_call(
            list(vec![int_lit(1), int_lit(2), int_lit(3)]),
            "len",
            vec![],
        );
        assert_eq!(rewrite_expr(&expr), int_lit(3));
    }

    #[test]
    fn list_len_empty() {
        let expr = method_call(list(vec![]), "len", vec![]);
        assert_eq!(rewrite_expr(&expr), int_lit(0));
    }

    // ── Option ────────────────────────────────────────────────────────────────

    #[test]
    fn some_is_some() {
        let expr = method_call(fn_call("Some", vec![int_lit(42)]), "is_some", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }

    #[test]
    fn none_is_some() {
        let expr = method_call(ident("None"), "is_some", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(false));
    }

    #[test]
    fn some_is_none() {
        let expr = method_call(fn_call("Some", vec![int_lit(1)]), "is_none", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(false));
    }

    #[test]
    fn none_is_none() {
        let expr = method_call(ident("None"), "is_none", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }

    // ── Result ────────────────────────────────────────────────────────────────

    #[test]
    fn ok_is_ok() {
        let expr = method_call(fn_call("Ok", vec![int_lit(0)]), "is_ok", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }

    #[test]
    fn err_is_ok() {
        let expr = method_call(fn_call("Err", vec![str_lit("fail")]), "is_ok", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(false));
    }

    #[test]
    fn ok_is_err() {
        let expr = method_call(fn_call("Ok", vec![int_lit(0)]), "is_err", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(false));
    }

    #[test]
    fn err_is_err() {
        let expr = method_call(fn_call("Err", vec![str_lit("oops")]), "is_err", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }

    // ── No-op cases ───────────────────────────────────────────────────────────

    #[test]
    fn unknown_method_unchanged() {
        let expr = method_call(str_lit("x"), "unknown_method", vec![]);
        // Should not crash; returns a reconstructed MethodCall.
        assert!(matches!(rewrite_expr(&expr), Expr::MethodCall { .. }));
    }

    #[test]
    fn ident_unchanged() {
        let expr = ident("x");
        assert_eq!(rewrite_expr(&expr), expr);
    }

    // ── Confluence: nested rewrites compose ───────────────────────────────────

    #[test]
    fn nested_rewrite_composes() {
        // `Some("hello".len() > 0).is_some()` should ultimately rewrite
        // the outer is_some to Bool(true) regardless of the inner expression.
        use crate::mvl::parser::ast::BinaryOp;
        let inner = Expr::Binary {
            op: BinaryOp::Gt,
            left: Box::new(method_call(str_lit("hello"), "len", vec![])),
            right: Box::new(int_lit(0)),
            span: sp(),
        };
        let expr = method_call(fn_call("Some", vec![inner]), "is_some", vec![]);
        assert_eq!(rewrite_expr(&expr), bool_lit(true));
    }
}
