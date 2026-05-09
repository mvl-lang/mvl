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
//! - More than [`MAX_PATHS`] paths (returns `None`)

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, CmpOp, ElseBranch, Expr, FnDecl, Literal, LogicOp, RefExpr, Stmt,
};
use crate::mvl::parser::lexer::Span;

use super::{layer1, layer2, RefResult};

/// Maximum number of execution paths before falling back to `None`.
const MAX_PATHS: usize = 32;

// ── Data structures ───────────────────────────────────────────────────────────

/// Accumulated branch conditions along a single execution path (a conjunction).
struct PathConstraint {
    conditions: Vec<Expr>,
}

/// Symbolic execution state for a call site: parameter bindings + path constraint.
struct SymbolicEnv<'a> {
    /// Maps parameter name → actual argument expression from the call site.
    bindings: HashMap<&'a str, &'a Expr>,
    path: PathConstraint,
}

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
/// Returns `None` when Layer 3 cannot make a determination; the caller should
/// fall back to `RuntimeCheck`.
pub(super) fn try_symbolic(
    pred: &RefExpr,
    arg: &Expr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
) -> Option<RefResult> {
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

    // Build symbolic env: param name → actual argument expression.
    let bindings: HashMap<&str, &Expr> = fd
        .params
        .iter()
        .zip(actual_args.iter())
        .map(|(p, a)| (p.name.as_str(), a))
        .collect();
    let env = SymbolicEnv {
        bindings,
        path: PathConstraint { conditions: vec![] },
    };

    // Collect all execution paths from the function body.
    let mut paths: Vec<ExecutionPath> = Vec::new();
    collect_block_paths(&fd.body, env.path.conditions.clone(), &mut paths);

    // Bail out on empty collection (e.g. body has no return) or path explosion.
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return None;
    }

    let mut any_undecided = false;

    for path in &paths {
        // Build a narrowed var_refs: start from caller's context, then inject
        // the path conditions (after substituting parameter names).
        let mut path_var_refs = var_refs.clone();
        for cond in &path.conditions {
            let cond_subst = substitute_expr(cond, &env.bindings);
            inject_condition(&cond_subst, &mut path_var_refs);
        }

        // Substitute parameters in the return expression, then check against pred.
        let return_expr = substitute_expr(&path.return_expr, &env.bindings);
        let result = layer1::try_trivial(pred, &return_expr, &path_var_refs, fn_decls)
            .or_else(|| layer2::try_interval(pred, &return_expr, &path_var_refs));

        match result {
            Some(RefResult::Proven) => {}
            Some(RefResult::Failed) => return Some(RefResult::Failed),
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
fn collect_block_paths(block: &Block, prefix: Vec<Expr>, paths: &mut Vec<ExecutionPath>) {
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
                collect_block_paths(then, then_prefix, paths);

                // Else-branch: prefix + [!cond] (if negation is possible).
                let else_prefix = with_negated(prefix.clone(), cond);

                match else_ {
                    Some(ElseBranch::Block(b)) => {
                        collect_block_paths(b, else_prefix, paths);
                    }
                    Some(ElseBranch::If(s)) => {
                        // `else if …` — wrap in a synthetic block and recurse.
                        let synthetic = Block {
                            stmts: vec![*s.clone()],
                            span: dummy_span(),
                        };
                        collect_block_paths(&synthetic, else_prefix, paths);
                    }
                    None => {
                        // No else — fall through to the remaining statements.
                        if i + 1 < stmts.len() {
                            let rest = Block {
                                stmts: stmts[i + 1..].to_vec(),
                                span: dummy_span(),
                            };
                            collect_block_paths(&rest, else_prefix, paths);
                        }
                    }
                }
                // Both branches handled — remaining stmts (if any) are unreachable
                // on paths where the then-branch returned.
                return;
            }

            // Tail expression (last statement): implicit return value.
            Stmt::Expr { expr, .. } if i == stmts.len() - 1 => {
                collect_expr_paths(expr, prefix, paths);
                return;
            }

            // All other statements: skip (conservative).
            _ => {}
        }
    }
}

/// Walk an [`Expr`] and collect execution paths (for if-expressions and tail positions).
fn collect_expr_paths(expr: &Expr, prefix: Vec<Expr>, paths: &mut Vec<ExecutionPath>) {
    match expr {
        Expr::If {
            cond, then, else_, ..
        } => {
            // Then-branch.
            let mut then_prefix = prefix.clone();
            then_prefix.push(*cond.clone());
            collect_block_paths(then, then_prefix, paths);

            // Else-branch.
            let else_prefix = with_negated(prefix, cond);
            if let Some(e) = else_ {
                collect_expr_paths(e, else_prefix, paths);
            }
        }
        Expr::Block(b) => collect_block_paths(b, prefix, paths),
        other => paths.push(ExecutionPath {
            conditions: prefix,
            return_expr: other.clone(),
        }),
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
fn substitute_expr(expr: &Expr, bindings: &HashMap<&str, &Expr>) -> Expr {
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
        other => other.clone(),
    }
}

// ── Condition injection ───────────────────────────────────────────────────────

/// Inject a (substituted) path condition into `var_refs` as a narrowing hypothesis.
///
/// Mirrors `inject_if_hypothesis` in `refinements.rs`.  Handles `x op n` and
/// `n op x` patterns (integer literal on one side) and `&&`-conjunctions.
/// All other patterns are silently ignored — conservative and always sound.
fn inject_condition(cond: &Expr, var_refs: &mut HashMap<String, Option<RefExpr>>) {
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
                (name.clone(), flip_cmp(cmp), *n)
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
        inject_condition(left, var_refs);
        inject_condition(right, var_refs);
    }
}

fn binary_op_to_cmp(op: BinaryOp) -> Option<CmpOp> {
    match op {
        BinaryOp::Gt => Some(CmpOp::Gt),
        BinaryOp::Ge => Some(CmpOp::Ge),
        BinaryOp::Lt => Some(CmpOp::Lt),
        BinaryOp::Le => Some(CmpOp::Le),
        BinaryOp::Eq => Some(CmpOp::Eq),
        BinaryOp::Ne => Some(CmpOp::Ne),
        _ => None,
    }
}

fn flip_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::Le => CmpOp::Ge,
        CmpOp::Ge => CmpOp::Le,
        CmpOp::Eq => CmpOp::Eq,
        CmpOp::Ne => CmpOp::Ne,
    }
}

fn dummy_span() -> Span {
    Span::new(0, 0, 0, 0)
}
