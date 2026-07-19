// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Layer 3 — symbolic path analysis for refinement predicates.
//!
//! Handles ~15% of refinement proofs by symbolically executing pure function
//! bodies and proving the return value satisfies the predicate on every path.
//!
//! | Pattern              | Example                                     |
//! |----------------------|---------------------------------------------|
//! | Clamp-style          | `clamp(x, 1, 10)` where pred is `>= 1`      |
//! | Min/max-style        | `min(a, b)` where pred is `<= a`            |
//! | Multi-branch returns | any pure fn with if/else returning a value  |
//!
//! ## Algorithm
//!
//! 1. Detect `Expr::FnCall { name, args }` with a body in `fn_decls`.
//! 2. Collect all execution paths: `(path_conditions, return_expr)` pairs.
//! 3. For each path, substitute parameter names with actual argument expressions.
//! 4. Inject path conditions as narrowing hypotheses into a cloned `var_refs`.
//! 5. Check the substituted return expression against `pred` using Layer 1 + Layer 2.
//! 6. Return `Some(Proven)` iff all paths pass; `Some(Failed)` if any path fails;
//!    `None` if any path is undecided.
//!
//! ## Limitations (deferred to Layer 4 / Z3)
//!
//! - Variable-vs-variable comparisons in path conditions (e.g. `x < y` where
//!   both are unresolved variables — `inject_condition` silently skips these)
//! - Non-linear arithmetic in return expressions
//! - Loop bodies (skipped conservatively)
//! - [`MAX_PATHS`] or more execution paths (returns `None`)

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, ElseBranch, Expr, FnDecl, Literal, LogicOp, Pattern, RefExpr, Stmt,
};

use super::{binary_op_to_cmp, dummy_span, layer1, layer2, rewrite, RefResult};

/// Functions with this many or more execution paths fall back to `None`.
const MAX_PATHS: usize = 32;

/// Blocks nested more deeply than this are treated conservatively (bail out).
const MAX_DEPTH: usize = 64;

// ── Data structures ───────────────────────────────────────────────────────────

/// One complete execution path collected from a function body.
struct ExecutionPath {
    /// Raw (un-substituted) branch conditions accumulated along this path.
    conditions: Vec<Expr>,
    /// Raw (un-substituted) expression returned on this path.
    return_expr: Expr,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Try to prove or refute `pred` for `arg` using symbolic path analysis (Layer 3).
///
/// Only applicable when `arg` is a call to a pure (no-effect) function whose
/// body is available in `fn_decls`.
///
/// # Purity invariant
///
/// `fn_decls` must contain only pure (effect-free) functions. The caller
/// (`check_arg_against_pred` in `refinements.rs`) enforces this via
/// `build_pure_fn_decls` before passing the map here.
///
/// Returns `None` when Layer 3 cannot make a determination; the caller should
/// fall back to `RuntimeCheck`.
pub(super) fn try_symbolic(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
) -> Option<RefResult> {
    // Apply builtin rewrite rules to method calls on known receivers (#596).
    // If the rewrite reduces a MethodCall to a simpler expression, try the
    // fast layers on the result before falling through to symbolic execution.
    if matches!(arg, Expr::MethodCall { .. }) {
        let rewritten = rewrite::rewrite_expr(arg);
        if !matches!(rewritten, Expr::MethodCall { .. }) {
            return layer1::try_trivial(pred, &rewritten, var_refs, fn_decls)
                .or_else(|| layer2::try_interval(pred, &rewritten, var_refs));
        }
    }

    // NEW: If the argument is itself an if-expression or a block (typical
    // when checking an `ensures` postcondition against a function body that
    // returns via a multi-branch clamp-style expression), collect the
    // execution paths directly and check the predicate on each branch's
    // return value, propagating the branch condition as a hypothesis.
    //
    // Concrete case: `clamp_col(x, field)` with body
    //     if x < 0 { 0 } else if x >= field.width { field.width - 1 } else { x }
    // and ensures `result >= 0`, `result < field.width` — each branch
    // discharges via L1 + injected path conditions.
    if matches!(arg, Expr::If { .. } | Expr::Block(_)) {
        let mut paths: Vec<ExecutionPath> = Vec::new();
        collect_expr_paths(arg, vec![], &mut paths, 0);
        return check_direct_paths(&paths, pred, var_refs, fn_decls);
    }

    let (fn_name, actual_args) = match arg {
        Expr::FnCall { name, args, .. } => (name.as_str(), args),
        _ => return None,
    };
    let fd = fn_decls.get(fn_name)?;

    // Skip builtins — they have no body to analyse.
    if fd.is_builtin {
        return None;
    }
    // Arity must match (otherwise the type checker will have flagged this already).
    if fd.params.len() != actual_args.len() {
        return None;
    }

    // Build parameter bindings: param name → actual argument expression.
    let bindings: HashMap<&str, &Expr> = fd
        .params
        .iter()
        .zip(actual_args.iter())
        .map(|(p, a)| (p.name.as_str(), a))
        .collect();

    // Collect all execution paths from the function body.
    let mut paths: Vec<ExecutionPath> = Vec::new();
    collect_block_paths(&fd.body, vec![], &mut paths, 0);

    // Bail out on empty collection (e.g. body has no return) or path explosion.
    if paths.is_empty() || paths.len() >= MAX_PATHS {
        return None;
    }

    let mut any_undecided = false;

    for path in &paths {
        // Build a narrowed var_refs: start from caller's context, then inject
        // the path conditions (after substituting parameter names).
        let mut path_var_refs = var_refs.clone();
        for cond in &path.conditions {
            let cond_subst = substitute_expr(cond, &bindings);
            inject_condition(&cond_subst, &mut path_var_refs, 0);
        }

        // Substitute parameters in the return expression, apply rewrite rules,
        // then check against pred.
        let return_expr = rewrite::rewrite_expr(&substitute_expr(&path.return_expr, &bindings));
        let result = layer1::try_trivial(pred, &return_expr, &path_var_refs, fn_decls)
            .or_else(|| layer2::try_interval(pred, &return_expr, &path_var_refs));

        match result {
            Some(RefResult::Proven | RefResult::ProvenBv) => {}
            Some(RefResult::Failed { counterexample }) => {
                return Some(RefResult::Failed { counterexample })
            }
            Some(RefResult::RuntimeCheck) | None => any_undecided = true,
        }
    }

    if any_undecided {
        None
    } else {
        Some(RefResult::Proven)
    }
}

/// Check `pred` against every branch of a pre-collected set of paths, using
/// each path's conditions as narrowing hypotheses in `var_refs`.
///
/// Unlike the FnCall path in `try_symbolic`, this variant does no parameter
/// substitution — it's used when the argument expression itself is an
/// if/else chain and the branch conditions reference caller-visible
/// variables directly.
fn check_direct_paths(
    paths: &[ExecutionPath],
    pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
) -> Option<RefResult> {
    if paths.is_empty() || paths.len() >= MAX_PATHS {
        return None;
    }
    let mut any_undecided = false;
    for path in paths {
        let mut path_var_refs = var_refs.clone();
        for cond in &path.conditions {
            inject_condition(cond, &mut path_var_refs, 0);
        }
        let return_expr = rewrite::rewrite_expr(&path.return_expr);
        let result = layer1::try_trivial(pred, &return_expr, &path_var_refs, fn_decls)
            .or_else(|| layer2::try_interval(pred, &return_expr, &path_var_refs));
        match result {
            Some(RefResult::Proven | RefResult::ProvenBv) => {}
            Some(RefResult::Failed { counterexample }) => {
                return Some(RefResult::Failed { counterexample })
            }
            Some(RefResult::RuntimeCheck) | None => any_undecided = true,
        }
    }
    if any_undecided {
        None
    } else {
        Some(RefResult::Proven)
    }
}

// ── Path collection ───────────────────────────────────────────────────────────

/// Walk a [`Block`] and collect all execution paths as `(conditions, return_expr)`.
///
/// `prefix` holds the branch conditions accumulated on the way to this block.
/// `depth` limits recursion; at [`MAX_DEPTH`] the block is treated conservatively.
///
/// ## Handled patterns
///
/// - `Stmt::Return { value }` — explicit early return
/// - `Stmt::If { ... }` — branch and recurse into both sides
/// - Tail `Stmt::Expr { expr }` — implicit return (last statement in block)
///
/// All other statements (let bindings, assignments, loops) are skipped.
/// This is conservative: skipped bindings may cause path conditions to be
/// weaker than they could be, but the check is always sound.
fn collect_block_paths(
    block: &Block,
    prefix: Vec<Expr>,
    paths: &mut Vec<ExecutionPath>,
    depth: usize,
) {
    if depth > MAX_DEPTH || paths.len() >= MAX_PATHS {
        return;
    }
    let stmts = &block.stmts;
    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            // Explicit return: emit one path and stop.
            Stmt::Return { value: Some(e), .. } => {
                paths.push(ExecutionPath {
                    conditions: prefix,
                    return_expr: e.clone(),
                });
                return;
            }
            Stmt::Return { value: None, .. } => {
                return; // returns Unit — no value to check
            }

            // If-statement: fork into then/else branches.
            Stmt::If {
                cond, then, else_, ..
            } => {
                // Then-branch: prefix + [cond].
                let mut then_prefix = prefix.clone();
                then_prefix.push(cond.clone());
                collect_block_paths(then, then_prefix.clone(), paths, depth + 1);

                // Else / fall-through handling.
                let else_prefix = with_negated(prefix.clone(), cond);

                match else_ {
                    Some(ElseBranch::Block(b)) => {
                        collect_block_paths(b, else_prefix, paths, depth + 1);
                    }
                    Some(ElseBranch::If(s)) => {
                        // `else if …` — wrap in a synthetic block and recurse.
                        let synthetic = Block {
                            stmts: vec![*s.clone()],
                            span: dummy_span(),
                        };
                        collect_block_paths(&synthetic, else_prefix, paths, depth + 1);
                    }
                    None => {
                        // No else: both the cond-true and cond-false paths can
                        // fall through to the remaining statements.
                        if i + 1 < stmts.len() {
                            let rest_stmts = stmts[i + 1..].to_vec();

                            // cond-false fall-through (then-body skipped entirely).
                            let else_rest = Block {
                                stmts: rest_stmts.clone(),
                                span: dummy_span(),
                            };
                            collect_block_paths(&else_rest, else_prefix, paths, depth + 1);

                            // cond-true fall-through (then-body executed, did not return).
                            if !block_always_returns(then) {
                                let then_rest = Block {
                                    stmts: rest_stmts,
                                    span: dummy_span(),
                                };
                                collect_block_paths(&then_rest, then_prefix, paths, depth + 1);
                            }
                        }
                    }
                }
                // All continuations from this if-statement have been handled.
                return;
            }

            // Tail expression (last statement): implicit return value.
            Stmt::Expr { expr, .. } if i == stmts.len() - 1 => {
                collect_expr_paths(expr, prefix, paths, depth + 1);
                return;
            }

            // Let-binding: unfold into per-path substitution over the remainder
            // of the block (#1805).  For each path through `init`, replace the
            // bound name in every subsequent statement's return expression with
            // that path's tail expression.  Enables patterns like:
            //
            //     let s = if cond { a + 1 } else { a };
            //     Game { .. right_score: s }
            //
            // where the ensures references `s` transitively through the struct
            // literal.  When the init has a single path (no branching), this is
            // equivalent to plain substitution.
            Stmt::Let {
                pattern: Pattern::Ident(name, _),
                init,
                ..
            } => {
                // Collect init paths under an empty prefix — their conditions
                // are additive on top of the outer prefix.
                let mut init_paths: Vec<ExecutionPath> = Vec::new();
                collect_expr_paths(init, vec![], &mut init_paths, depth + 1);

                // Path explosion guard: if the init alone already saturates the
                // budget, fall back to the conservative skip.
                if init_paths.is_empty() || init_paths.len() >= MAX_PATHS {
                    // Skip this let (conservative — subsequent tail may still
                    // discharge via a hypothesis that doesn't reference the
                    // bound name).
                    continue;
                }

                let rest = if i + 1 < stmts.len() {
                    stmts[i + 1..].to_vec()
                } else {
                    // Trailing let with no continuation — its bound value is
                    // effectively the return.  Emit one path per init path.
                    for ip in init_paths {
                        let mut merged = prefix.clone();
                        merged.extend(ip.conditions);
                        paths.push(ExecutionPath {
                            conditions: merged,
                            return_expr: ip.return_expr,
                        });
                    }
                    return;
                };

                for ip in init_paths {
                    let mut branch_prefix = prefix.clone();
                    branch_prefix.extend(ip.conditions);
                    let mut bindings: HashMap<&str, &Expr> = HashMap::new();
                    bindings.insert(name.as_str(), &ip.return_expr);

                    let subst_rest: Vec<Stmt> =
                        rest.iter().map(|s| substitute_stmt(s, &bindings)).collect();
                    let synthetic = Block {
                        stmts: subst_rest,
                        span: dummy_span(),
                    };
                    collect_block_paths(&synthetic, branch_prefix, paths, depth + 1);
                }
                return;
            }

            // All other statements: skip (conservative).
            _ => {}
        }
    }
}

/// Apply `bindings` to any `Expr` positions inside `stmt`.  Used by the
/// let-unfolding step in [`collect_block_paths`] and by the contract checker
/// to propagate a bound name's value into subsequent statements' return
/// expressions.
///
/// Only the return-carrying positions are substituted: `Stmt::Expr`,
/// `Stmt::Return`, `Stmt::If.cond` / `then` / `else_`, and `Stmt::Let.init`.
/// Loops, assignments, and match arms are left untouched (conservative —
/// path collection already skips those bodies).
pub(crate) fn substitute_stmt(stmt: &Stmt, bindings: &HashMap<&str, &Expr>) -> Stmt {
    match stmt {
        Stmt::Expr { expr, span } => Stmt::Expr {
            expr: substitute_expr(expr, bindings),
            span: *span,
        },
        Stmt::Return { value, span } => Stmt::Return {
            value: value.as_ref().map(|e| substitute_expr(e, bindings)),
            span: *span,
        },
        Stmt::If {
            cond,
            then,
            else_,
            span,
        } => Stmt::If {
            cond: substitute_expr(cond, bindings),
            then: substitute_block(then, bindings),
            else_: else_.as_ref().map(|eb| match eb {
                ElseBranch::Block(b) => ElseBranch::Block(substitute_block(b, bindings)),
                ElseBranch::If(inner) => ElseBranch::If(Box::new(substitute_stmt(inner, bindings))),
            }),
            span: *span,
        },
        Stmt::Let {
            kind,
            pattern,
            ty,
            init,
            span,
        } => Stmt::Let {
            kind: kind.clone(),
            pattern: pattern.clone(),
            ty: ty.clone(),
            init: substitute_expr(init, bindings),
            span: *span,
        },
        other => other.clone(),
    }
}

fn substitute_block(block: &Block, bindings: &HashMap<&str, &Expr>) -> Block {
    Block {
        stmts: block
            .stmts
            .iter()
            .map(|s| substitute_stmt(s, bindings))
            .collect(),
        span: block.span,
    }
}

/// Walk an [`Expr`] and collect execution paths (for if-expressions and tail positions).
fn collect_expr_paths(
    expr: &Expr,
    prefix: Vec<Expr>,
    paths: &mut Vec<ExecutionPath>,
    depth: usize,
) {
    if depth > MAX_DEPTH || paths.len() >= MAX_PATHS {
        return;
    }
    match expr {
        Expr::If {
            cond, then, else_, ..
        } => {
            // Then-branch.
            let mut then_prefix = prefix.clone();
            then_prefix.push(*cond.clone());
            collect_block_paths(then, then_prefix, paths, depth + 1);

            // Else-branch.
            let else_prefix = with_negated(prefix, cond);
            if let Some(e) = else_ {
                collect_expr_paths(e, else_prefix, paths, depth + 1);
            }
        }
        Expr::Block(b) => collect_block_paths(b, prefix, paths, depth + 1),
        other => paths.push(ExecutionPath {
            conditions: prefix,
            return_expr: other.clone(),
        }),
    }
}

/// Returns `true` if every path through `block` ends with an explicit `return`.
///
/// Conservative: returns `false` when the block's last statement is not a return
/// or a fully-covered if/else. Used to determine whether a then-branch without
/// an else clause can fall through to subsequent statements.
fn block_always_returns(block: &Block) -> bool {
    // Only the last statement determines whether the block always returns.
    match block.stmts.last() {
        Some(Stmt::Return { .. }) => true,
        Some(Stmt::If {
            then,
            else_: Some(ElseBranch::Block(else_block)),
            ..
        }) => block_always_returns(then) && block_always_returns(else_block),
        Some(Stmt::If {
            then,
            else_: Some(ElseBranch::If(s)),
            ..
        }) => {
            let synthetic = Block {
                stmts: vec![*s.clone()],
                span: dummy_span(),
            };
            block_always_returns(then) && block_always_returns(&synthetic)
        }
        _ => false,
    }
}

// ── Condition negation ────────────────────────────────────────────────────────

/// Return `prefix` with the logical negation of `cond` appended, if possible.
///
/// When negation is not possible (compound or non-comparison condition), `cond`
/// is not appended — the caller proceeds conservatively with fewer constraints.
fn with_negated(mut prefix: Vec<Expr>, cond: &Expr) -> Vec<Expr> {
    if let Some(neg) = negate_simple(cond) {
        prefix.push(neg);
    }
    prefix
}

/// Negate a simple `x op n` (or `n op x`) comparison expression.
///
/// Returns `None` for compound conditions (`&&`, `||`, non-comparisons).
fn negate_simple(cond: &Expr) -> Option<Expr> {
    let Expr::Binary {
        op,
        left,
        right,
        span,
    } = cond
    else {
        return None;
    };
    let flipped = flip_binary_op(*op)?;
    Some(Expr::Binary {
        op: flipped,
        left: left.clone(),
        right: right.clone(),
        span: *span,
    })
}

fn flip_binary_op(op: BinaryOp) -> Option<BinaryOp> {
    match op {
        BinaryOp::Lt => Some(BinaryOp::Ge),
        BinaryOp::Le => Some(BinaryOp::Gt),
        BinaryOp::Gt => Some(BinaryOp::Le),
        BinaryOp::Ge => Some(BinaryOp::Lt),
        BinaryOp::Eq => Some(BinaryOp::Ne),
        BinaryOp::Ne => Some(BinaryOp::Eq),
        _ => None,
    }
}

// ── Expression substitution ───────────────────────────────────────────────────

/// Substitute parameter names in `expr` with their actual argument expressions.
///
/// Handles `Ident`, `Binary`, `Unary`, and `FnCall` recursively.
/// All other variants are cloned unchanged — conservative but always sound
/// (worst case: the check falls through to `RuntimeCheck`).
/// Note: `Expr::If` and `Expr::Block` are not substituted into; path
/// collection extracts return expressions before substitution is applied.
pub(crate) fn substitute_expr(expr: &Expr, bindings: &HashMap<&str, &Expr>) -> Expr {
    match expr {
        Expr::Ident(name, _) => {
            if let Some(&replacement) = bindings.get(name.as_str()) {
                replacement.clone()
            } else {
                expr.clone()
            }
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(substitute_expr(left, bindings)),
            right: Box::new(substitute_expr(right, bindings)),
            span: *span,
        },
        Expr::Unary {
            op,
            expr: inner,
            span,
        } => Expr::Unary {
            op: *op,
            expr: Box::new(substitute_expr(inner, bindings)),
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
            args: args.iter().map(|a| substitute_expr(a, bindings)).collect(),
            span: *span,
        },
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } => Expr::MethodCall {
            receiver: Box::new(substitute_expr(receiver, bindings)),
            method: method.clone(),
            args: args.iter().map(|a| substitute_expr(a, bindings)).collect(),
            span: *span,
        },
        // FieldAccess: substitute inside the base expression (#1805 let-unfold).
        // Enables `Game { right_score: new_score, .. }.right_score` after a let
        // rewrite to normalize through the field projection.
        Expr::FieldAccess {
            expr: inner,
            field,
            span,
        } => Expr::FieldAccess {
            expr: Box::new(substitute_expr(inner, bindings)),
            field: field.clone(),
            span: *span,
        },
        // Construct: substitute inside each field value (#1805 let-unfold).
        // Needed for the ticket's `Game { .. right_score: new_score }` pattern
        // where the let-bound name appears in a struct-literal tail return.
        Expr::Construct { name, fields, span } => Expr::Construct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.clone(), substitute_expr(v, bindings)))
                .collect(),
            span: *span,
        },
        other => other.clone(),
    }
}

// ── Condition injection ───────────────────────────────────────────────────────

/// Inject a (substituted) path condition into `var_refs` as a narrowing hypothesis.
///
/// Handles `x op n` and `n op x` patterns (integer literal on one side) and
/// `&&`-conjunctions. All other patterns are silently ignored — conservative and
/// always sound. `depth` limits `&&`-recursion to prevent stack overflow on
/// pathological inputs.
pub(crate) fn inject_condition(
    cond: &Expr,
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let Expr::Binary {
        op, left, right, ..
    } = cond
    else {
        return;
    };

    if let Some(cmp) = binary_op_to_cmp(*op) {
        let (var_name, cmp_op, int_val) =
            if let (Expr::Ident(name, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), cmp, *n)
            } else if let (Expr::Literal(Literal::Integer(n), _), Expr::Ident(name, _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), cmp.flip(), *n)
            } else {
                return; // var-vs-var or non-integer: skip conservatively
            };

        let s = dummy_span();
        let new_ref = RefExpr::Compare {
            op: cmp_op,
            left: Box::new(RefExpr::Ident {
                name: "self".to_string(),
                span: s,
            }),
            right: Box::new(RefExpr::Integer {
                value: int_val,
                span: s,
            }),
            span: s,
        };
        let hypothesis = match var_refs.get(&var_name).and_then(|v| v.clone()) {
            Some(existing) => RefExpr::LogicOp {
                op: LogicOp::And,
                left: Box::new(existing),
                right: Box::new(new_ref),
                span: s,
            },
            None => new_ref,
        };
        var_refs.insert(var_name, Some(hypothesis));
    } else if *op == BinaryOp::And {
        // Recurse into `&&` conjunctions.
        inject_condition(left, var_refs, depth + 1);
        inject_condition(right, var_refs, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{flip_binary_op, inject_condition, negate_simple};
    use crate::mvl::parser::ast::{BinaryOp, CmpOp, Expr, Literal, LogicOp, RefExpr};
    use crate::mvl::parser::lexer::Span;

    fn sp() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn int_expr(n: i64) -> Expr {
        Expr::Literal(Literal::Integer(n), sp())
    }

    fn ident_expr(name: &str) -> Expr {
        Expr::Ident(name.to_string(), sp())
    }

    fn bin(left: Expr, op: BinaryOp, right: Expr) -> Expr {
        Expr::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
            span: sp(),
        }
    }

    // ── flip_binary_op ────────────────────────────────────────────────────────

    #[test]
    fn test_flip_lt() {
        assert_eq!(flip_binary_op(BinaryOp::Lt), Some(BinaryOp::Ge));
    }
    #[test]
    fn test_flip_le() {
        assert_eq!(flip_binary_op(BinaryOp::Le), Some(BinaryOp::Gt));
    }
    #[test]
    fn test_flip_gt() {
        assert_eq!(flip_binary_op(BinaryOp::Gt), Some(BinaryOp::Le));
    }
    #[test]
    fn test_flip_ge() {
        assert_eq!(flip_binary_op(BinaryOp::Ge), Some(BinaryOp::Lt));
    }
    #[test]
    fn test_flip_eq() {
        assert_eq!(flip_binary_op(BinaryOp::Eq), Some(BinaryOp::Ne));
    }
    #[test]
    fn test_flip_ne() {
        assert_eq!(flip_binary_op(BinaryOp::Ne), Some(BinaryOp::Eq));
    }
    #[test]
    fn test_flip_non_comparison_returns_none() {
        assert_eq!(flip_binary_op(BinaryOp::Add), None);
    }

    // ── negate_simple ─────────────────────────────────────────────────────────

    #[test]
    fn test_negate_lt_becomes_ge() {
        let cond = bin(ident_expr("x"), BinaryOp::Lt, int_expr(5));
        let neg = negate_simple(&cond).unwrap();
        assert!(matches!(
            neg,
            Expr::Binary {
                op: BinaryOp::Ge,
                ..
            }
        ));
    }

    #[test]
    fn test_negate_non_binary_returns_none() {
        assert!(negate_simple(&ident_expr("x")).is_none());
    }

    #[test]
    fn test_negate_arithmetic_returns_none() {
        let cond = bin(ident_expr("x"), BinaryOp::Add, int_expr(1));
        assert!(negate_simple(&cond).is_none());
    }

    // ── inject_condition ──────────────────────────────────────────────────────

    #[test]
    fn test_inject_x_op_n() {
        let mut vr: HashMap<String, Option<RefExpr>> = HashMap::new();
        inject_condition(&bin(ident_expr("x"), BinaryOp::Ge, int_expr(0)), &mut vr, 0);
        assert!(matches!(
            &vr["x"],
            Some(RefExpr::Compare { op: CmpOp::Ge, .. })
        ));
    }

    #[test]
    fn test_inject_n_op_x_flips_cmp() {
        // 5 > x  ↔  x < 5
        let mut vr: HashMap<String, Option<RefExpr>> = HashMap::new();
        inject_condition(&bin(int_expr(5), BinaryOp::Gt, ident_expr("x")), &mut vr, 0);
        assert!(matches!(
            &vr["x"],
            Some(RefExpr::Compare { op: CmpOp::Lt, .. })
        ));
    }

    #[test]
    fn test_inject_var_vs_var_skipped() {
        let mut vr: HashMap<String, Option<RefExpr>> = HashMap::new();
        inject_condition(
            &bin(ident_expr("x"), BinaryOp::Lt, ident_expr("y")),
            &mut vr,
            0,
        );
        assert!(vr.is_empty());
    }

    #[test]
    fn test_inject_and_conjunction() {
        let mut vr: HashMap<String, Option<RefExpr>> = HashMap::new();
        let cond = bin(
            bin(ident_expr("x"), BinaryOp::Ge, int_expr(0)),
            BinaryOp::And,
            bin(ident_expr("x"), BinaryOp::Le, int_expr(10)),
        );
        inject_condition(&cond, &mut vr, 0);
        // Both constraints conjoined → LogicOp::And
        assert!(matches!(
            &vr["x"],
            Some(RefExpr::LogicOp {
                op: LogicOp::And,
                ..
            })
        ));
    }

    #[test]
    fn test_inject_depth_limit_bails() {
        let mut vr: HashMap<String, Option<RefExpr>> = HashMap::new();
        // depth 33 exceeds the limit of 32 — nothing injected
        inject_condition(
            &bin(ident_expr("x"), BinaryOp::Ge, int_expr(0)),
            &mut vr,
            33,
        );
        assert!(vr.is_empty());
    }
}
