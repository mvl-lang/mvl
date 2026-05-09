//! Information Flow Control: security lattice operations and implicit flow analysis.
//!
//! Implements Requirement 11 of the MVL spec (003-information-flow).
//!
//! Security lattice (highest to lowest sensitivity):
//!   Secret (3) > Tainted (2) > Clean (1) > Public (0)
//!
//! Upward flow (lower → higher sensitivity) is always allowed.
//! Downward flow requires `declassify()` (Secret→Public) or `sanitize()` (Tainted→Clean).
//!
//! # Implicit flow analysis (Phase 3)
//!
//! Beyond direct-flow enforcement (Req 1, 3, 4, 6, 7 — done in the type checker),
//! Phase 3 detects *implicit* flows: information leaked through control flow rather
//! than data flow.  The canonical example:
//!
//! ```mvl
//! if secret_flag { println("branch taken") }
//! ```
//!
//! Even though the `println` argument is a literal string (Public), whether the
//! print fires at all reveals whether `secret_flag` was truthy.  This is an
//! implicit flow from the secret condition to the public output sink.
//!
//! The analysis tracks the **Program Counter (PC) label**: the join of all
//! security labels on conditions that control the current execution point.
//! A public sink (`println`, `print`) inside a branch whose PC label is
//! Secret or Tainted is flagged as `ImplicitFlowViolation`.

use std::collections::HashMap;

use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, MatchBody, Program, SecurityLabel, Stmt, TypeExpr,
};

/// Numeric rank for the security lattice (higher = more sensitive).
pub fn lattice_rank(label: SecurityLabel) -> u8 {
    match label {
        SecurityLabel::Public => 0,
        SecurityLabel::Clean => 1,
        SecurityLabel::Tainted => 2,
        SecurityLabel::Secret => 3,
    }
}

/// True if data with label `from` may flow to a context requiring label `to`
/// without explicit declassification or sanitization.
///
/// Upward flow (from lower to higher sensitivity) is always allowed.
pub fn can_flow(from: SecurityLabel, to: SecurityLabel) -> bool {
    lattice_rank(from) <= lattice_rank(to)
}

/// Compute the join (least upper bound) of two labels — the higher-sensitivity one.
pub fn join(a: SecurityLabel, b: SecurityLabel) -> SecurityLabel {
    if lattice_rank(a) >= lattice_rank(b) {
        a
    } else {
        b
    }
}

/// Compute the join of two optional labels.
/// `None` represents an unlabeled type (treated as Public for join purposes).
///
/// Invariant: `join_opt(Some(L), None) == Some(L)` because `join(L, Public) == L`
/// for any `L >= Public`. This follows from the "unlabeled = Public" convention.
pub fn join_opt(a: Option<SecurityLabel>, b: Option<SecurityLabel>) -> Option<SecurityLabel> {
    match (a, b) {
        (None, None) => None,
        (Some(l), None) | (None, Some(l)) => Some(l),
        (Some(la), Some(lb)) => Some(join(la, lb)),
    }
}

/// Extract the outermost security label from a type, if any.
/// Looks through Refined wrappers to find the label.
///
/// NOTE: Nested `Labeled` types (e.g., `Labeled(A, Labeled(B, T))`) are not
/// valid IR — the parser and checker must never produce them. This function
/// only reads the outermost label, which is sufficient for valid IR.
pub fn label_of(ty: &Ty) -> Option<SecurityLabel> {
    match ty {
        Ty::Labeled(l, _) => Some(*l),
        Ty::Refined(inner, _) => label_of(inner),
        _ => None,
    }
}

/// Remove the outermost security label from a type, returning the inner type.
/// Used for argument type-checking in label-transparent functions (ADR-0024):
/// the function accepts any label on its arguments; the label is collected
/// separately and applied to the return type.
pub fn strip_label(ty: &Ty) -> &Ty {
    match ty {
        Ty::Labeled(_, inner) => inner,
        _ => ty,
    }
}

/// Wrap a type in a security label, or return it unchanged if label is None.
pub fn apply_label(label: Option<SecurityLabel>, ty: Ty) -> Ty {
    match label {
        Some(l) => Ty::Labeled(l, Box::new(ty)),
        None => ty,
    }
}

/// Human-readable name for a security label.
pub fn label_name(label: SecurityLabel) -> &'static str {
    match label {
        SecurityLabel::Public => "Public",
        SecurityLabel::Tainted => "Tainted",
        SecurityLabel::Secret => "Secret",
        SecurityLabel::Clean => "Clean",
    }
}

// ── Implicit flow analysis (Phase 3) ─────────────────────────────────────────

/// Walk every function in `prog` and emit [`CheckError::ImplicitFlowViolation`]
/// for any public sink (`println` / `print`) that appears inside a branch
/// controlled by a `Secret` or `Tainted` condition.
///
/// **Precondition:** `TypeChecker::check_program` MUST have run first so that
/// direct-flow violations (Req 11 Phase 1) are already captured.
///
/// **Phase 3 scope:** This pass handles the main implicit-flow pattern —
/// a branching condition that carries a high security label, with a public
/// output sink inside the body.  Indirect implicit flows through deeply nested
/// data structures or cross-function call chains are deferred to a future phase.
pub fn check_implicit_flows(prog: &Program, errors: &mut Vec<CheckError>) {
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            // Build initial label env from parameter type annotations.
            let mut env: HashMap<String, SecurityLabel> = HashMap::new();
            for param in &fd.params {
                if let Some(label) = label_of_type_expr(&param.ty) {
                    env.insert(param.name.clone(), label);
                }
            }
            // Walk the function body with pc_label = None (Public).
            check_block_flows(&fd.body, None, &mut env, errors);
        }
    }
}

/// Count all `declassify()` and `sanitize()` call sites in the program.
/// Used by the IFC pass to include the audit trail in the `Proven` evidence.
///
/// Returns `(declassify_count, sanitize_count)`.
pub fn count_declassifications(prog: &Program) -> (usize, usize) {
    let mut dc = 0usize;
    let mut sc = 0usize;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            count_in_block(&fd.body, &mut dc, &mut sc);
        }
    }
    (dc, sc)
}

/// Extract the outermost security label from a `TypeExpr`, if any.
fn label_of_type_expr(te: &TypeExpr) -> Option<SecurityLabel> {
    match te {
        TypeExpr::Labeled { label, .. } => Some(*label),
        TypeExpr::Refined { inner, .. } => label_of_type_expr(inner),
        TypeExpr::Tuple { elems, .. } => elems.iter().find_map(label_of_type_expr),
        TypeExpr::Base { .. }
        | TypeExpr::Option { .. }
        | TypeExpr::Result { .. }
        | TypeExpr::Ref { .. }
        | TypeExpr::Fn { .. }
        | TypeExpr::IntConst { .. } => None,
    }
}

/// Infer the security label of an expression from the current label env.
/// Conservative: returns `None` if the label cannot be determined.
fn infer_label(expr: &Expr, env: &HashMap<String, SecurityLabel>) -> Option<SecurityLabel> {
    match expr {
        Expr::Ident(name, _) => env.get(name.as_str()).copied(),
        Expr::Binary { left, right, .. } => {
            join_opt(infer_label(left, env), infer_label(right, env))
        }
        Expr::Unary { expr, .. } | Expr::Borrow { expr, .. } => infer_label(expr, env),
        Expr::FieldAccess { expr, .. } => infer_label(expr, env),
        // `declassify()` always produces Public; `sanitize()` produces Clean.
        Expr::Declassify { .. } => Some(SecurityLabel::Public),
        Expr::Sanitize { .. } => Some(SecurityLabel::Clean),
        // Function calls: join labels of arguments (conservative over-approximation).
        Expr::FnCall { args, .. } => args
            .iter()
            .map(|a| infer_label(a, env))
            .fold(None, join_opt),
        // Method calls: join receiver label with argument labels.
        Expr::MethodCall { receiver, args, .. } => {
            let recv_label = infer_label(receiver, env);
            let arg_label = args
                .iter()
                .map(|a| infer_label(a, env))
                .fold(None, join_opt);
            join_opt(recv_label, arg_label)
        }
        _ => None,
    }
}

/// True if a label is "high" (Secret or Tainted), meaning it should not
/// control whether a public sink fires.
fn is_high(label: SecurityLabel) -> bool {
    matches!(label, SecurityLabel::Secret | SecurityLabel::Tainted)
}

/// True if a label is "high" (Secret or Tainted) — Option variant.
fn is_high_opt(label: Option<SecurityLabel>) -> bool {
    label.map(is_high).unwrap_or(false)
}

/// Walk a block, tracking the current PC label and the label env.
/// Let bindings extend the env sequentially within the block.
fn check_block_flows(
    block: &Block,
    pc: Option<SecurityLabel>,
    env: &mut HashMap<String, SecurityLabel>,
    errors: &mut Vec<CheckError>,
) {
    for stmt in &block.stmts {
        check_stmt_flows(stmt, pc, env, errors);
    }
}

fn check_stmt_flows(
    stmt: &Stmt,
    pc: Option<SecurityLabel>,
    env: &mut HashMap<String, SecurityLabel>,
    errors: &mut Vec<CheckError>,
) {
    match stmt {
        Stmt::Let {
            pattern, ty, init, ..
        } => {
            // Walk the RHS under the current PC label.
            check_expr_flows(init, pc, env, errors);
            // Extend the label env for simple identifier patterns.
            // Complex patterns (tuples, structs) are treated conservatively.
            if let crate::mvl::parser::ast::Pattern::Ident(name, _) = pattern {
                let label = label_of_type_expr(ty).or_else(|| infer_label(init, env));
                if let Some(l) = label {
                    env.insert(name.clone(), l);
                }
            }
        }
        Stmt::Assign { value, .. } => {
            check_expr_flows(value, pc, env, errors);
        }
        Stmt::Return { value: Some(e), .. } => {
            check_expr_flows(e, pc, env, errors);
        }
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => {
            check_expr_flows(expr, pc, env, errors);
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc, cond_label);
            check_expr_flows(cond, pc, env, errors);
            let mut then_env = env.clone();
            check_block_flows(then, body_pc, &mut then_env, errors);
            match else_ {
                Some(ElseBranch::Block(blk)) => {
                    let mut else_env = env.clone();
                    check_block_flows(blk, body_pc, &mut else_env, errors);
                }
                Some(ElseBranch::If(nested)) => {
                    let mut else_env = env.clone();
                    check_stmt_flows(nested, body_pc, &mut else_env, errors);
                }
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let scr_label = infer_label(scrutinee, env);
            let body_pc = join_opt(pc, scr_label);
            check_expr_flows(scrutinee, pc, env, errors);
            for arm in arms {
                let mut arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(expr) => check_expr_flows(expr, body_pc, &mut arm_env, errors),
                    MatchBody::Block(blk) => check_block_flows(blk, body_pc, &mut arm_env, errors),
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc, cond_label);
            check_expr_flows(cond, pc, env, errors);
            let mut body_env = env.clone();
            check_block_flows(body, body_pc, &mut body_env, errors);
        }
        Stmt::For { iter, body, .. } => {
            let iter_label = infer_label(iter, env);
            let body_pc = join_opt(pc, iter_label);
            check_expr_flows(iter, pc, env, errors);
            let mut body_env = env.clone();
            check_block_flows(body, body_pc, &mut body_env, errors);
        }
    }
}

/// Check the public-sink names that must not appear inside high-PC contexts.
/// Includes std.log functions (#54): a log call inside a Secret branch leaks
/// whether the branch was taken (implicit flow via the log record's presence).
const PUBLIC_SINKS: &[&str] = &[
    "println",
    "print",
    "print_styled",
    "log_debug",
    "log_info",
    "log_warn",
    "log_error",
];

fn check_expr_flows(
    expr: &Expr,
    pc: Option<SecurityLabel>,
    env: &mut HashMap<String, SecurityLabel>,
    errors: &mut Vec<CheckError>,
) {
    match expr {
        Expr::FnCall {
            name, args, span, ..
        } => {
            // Detect public sink under high PC label.
            if PUBLIC_SINKS.contains(&name.as_str()) && is_high_opt(pc) {
                errors.push(CheckError::ImplicitFlowViolation {
                    pc_label: label_name(pc.unwrap()).to_string(),
                    sink: name.clone(),
                    span: *span,
                });
            }
            for arg in args {
                check_expr_flows(arg, pc, env, errors);
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label(cond, env);
            let body_pc = join_opt(pc, cond_label);
            check_expr_flows(cond, pc, env, errors);
            let mut then_env = env.clone();
            check_block_flows(then, body_pc, &mut then_env, errors);
            if let Some(e) = else_ {
                let mut else_env = env.clone();
                check_expr_flows(e, body_pc, &mut else_env, errors);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scr_label = infer_label(scrutinee, env);
            let body_pc = join_opt(pc, scr_label);
            check_expr_flows(scrutinee, pc, env, errors);
            for arm in arms {
                let mut arm_env = env.clone();
                match &arm.body {
                    MatchBody::Expr(e) => check_expr_flows(e, body_pc, &mut arm_env, errors),
                    MatchBody::Block(blk) => check_block_flows(blk, body_pc, &mut arm_env, errors),
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            check_expr_flows(left, pc, env, errors);
            check_expr_flows(right, pc, env, errors);
        }
        Expr::Unary { expr, .. }
        | Expr::Declassify { expr, .. }
        | Expr::Sanitize { expr, .. }
        | Expr::Move { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Borrow { expr, .. } => {
            check_expr_flows(expr, pc, env, errors);
        }
        Expr::MethodCall { receiver, args, .. } => {
            check_expr_flows(receiver, pc, env, errors);
            for arg in args {
                check_expr_flows(arg, pc, env, errors);
            }
        }
        Expr::Block(blk) => {
            let mut blk_env = env.clone();
            check_block_flows(blk, pc, &mut blk_env, errors);
        }
        Expr::Lambda { body, .. } => {
            // Lambdas capture the outer env but reset pc (they are called later).
            check_expr_flows(body, None, env, errors);
        }
        Expr::Construct { fields, .. } => {
            for (_, v) in fields {
                check_expr_flows(v, pc, env, errors);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                check_expr_flows(e, pc, env, errors);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                check_expr_flows(k, pc, env, errors);
                check_expr_flows(v, pc, env, errors);
            }
        }
        // Leaves — no sub-expressions to walk.
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

/// Recursively count `Expr::Declassify` and `Expr::Sanitize` nodes in a block.
fn count_in_block(block: &Block, dc: &mut usize, sc: &mut usize) {
    for stmt in &block.stmts {
        count_in_stmt(stmt, dc, sc);
    }
}

fn count_in_stmt(stmt: &Stmt, dc: &mut usize, sc: &mut usize) {
    match stmt {
        Stmt::Let { init, .. } => count_in_expr(init, dc, sc),
        Stmt::Assign { value, .. } => count_in_expr(value, dc, sc),
        Stmt::Return { value: Some(e), .. } => count_in_expr(e, dc, sc),
        Stmt::Return { value: None, .. } => {}
        Stmt::Expr { expr, .. } => count_in_expr(expr, dc, sc),
        Stmt::If {
            cond, then, else_, ..
        } => {
            count_in_expr(cond, dc, sc);
            count_in_block(then, dc, sc);
            match else_ {
                Some(ElseBranch::Block(blk)) => count_in_block(blk, dc, sc),
                Some(ElseBranch::If(s)) => count_in_stmt(s, dc, sc),
                None => {}
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            count_in_expr(scrutinee, dc, sc);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => count_in_expr(e, dc, sc),
                    MatchBody::Block(blk) => count_in_block(blk, dc, sc),
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            count_in_expr(cond, dc, sc);
            count_in_block(body, dc, sc);
        }
        Stmt::For { iter, body, .. } => {
            count_in_expr(iter, dc, sc);
            count_in_block(body, dc, sc);
        }
    }
}

fn count_in_expr(expr: &Expr, dc: &mut usize, sc: &mut usize) {
    match expr {
        Expr::Declassify { expr, .. } => {
            *dc += 1;
            count_in_expr(expr, dc, sc);
        }
        Expr::Sanitize { expr, .. } => {
            *sc += 1;
            count_in_expr(expr, dc, sc);
        }
        Expr::FnCall { args, .. } => {
            for a in args {
                count_in_expr(a, dc, sc);
            }
        }
        Expr::Binary { left, right, .. } => {
            count_in_expr(left, dc, sc);
            count_in_expr(right, dc, sc);
        }
        Expr::Unary { expr, .. }
        | Expr::Move { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Borrow { expr, .. } => count_in_expr(expr, dc, sc),
        Expr::MethodCall { receiver, args, .. } => {
            count_in_expr(receiver, dc, sc);
            for a in args {
                count_in_expr(a, dc, sc);
            }
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            count_in_expr(cond, dc, sc);
            count_in_block(then, dc, sc);
            if let Some(e) = else_ {
                count_in_expr(e, dc, sc);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            count_in_expr(scrutinee, dc, sc);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => count_in_expr(e, dc, sc),
                    MatchBody::Block(blk) => count_in_block(blk, dc, sc),
                }
            }
        }
        Expr::Block(blk) => count_in_block(blk, dc, sc),
        Expr::Lambda { body, .. } => count_in_expr(body, dc, sc),
        Expr::Construct { fields, .. } => {
            for (_, v) in fields {
                count_in_expr(v, dc, sc);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                count_in_expr(e, dc, sc);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                count_in_expr(k, dc, sc);
                count_in_expr(v, dc, sc);
            }
        }
        Expr::Literal(..) | Expr::Ident(..) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_opt_both_none_is_none() {
        assert_eq!(join_opt(None, None), None);
    }

    #[test]
    fn join_opt_with_one_none_preserves_label() {
        // Invariant: None (= unlabeled = Public) does not lower the result
        assert_eq!(
            join_opt(Some(SecurityLabel::Secret), None),
            Some(SecurityLabel::Secret)
        );
        assert_eq!(
            join_opt(None, Some(SecurityLabel::Tainted)),
            Some(SecurityLabel::Tainted)
        );
    }

    #[test]
    fn join_opt_takes_higher_label() {
        assert_eq!(
            join_opt(Some(SecurityLabel::Public), Some(SecurityLabel::Secret)),
            Some(SecurityLabel::Secret)
        );
        assert_eq!(
            join_opt(Some(SecurityLabel::Clean), Some(SecurityLabel::Tainted)),
            Some(SecurityLabel::Tainted)
        );
    }

    #[test]
    fn can_flow_upward_allowed() {
        assert!(can_flow(SecurityLabel::Public, SecurityLabel::Secret));
        assert!(can_flow(SecurityLabel::Clean, SecurityLabel::Tainted));
        assert!(can_flow(SecurityLabel::Public, SecurityLabel::Public));
    }

    #[test]
    fn can_flow_downward_rejected() {
        assert!(!can_flow(SecurityLabel::Secret, SecurityLabel::Public));
        assert!(!can_flow(SecurityLabel::Tainted, SecurityLabel::Clean));
    }
}
