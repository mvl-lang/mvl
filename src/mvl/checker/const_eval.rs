//! Compile-time evaluation of pure function calls (constant folding).
//!
//! When a function with no declared effects is called with all-literal arguments,
//! this module evaluates the function body at compile time and returns the result.
//! The result feeds into the refinement solver, allowing predicates to be proved
//! statically without runtime checks.
//!
//! # Bounded computation
//!
//! To prevent non-termination, evaluation is limited by a step budget
//! (`EVAL_BUDGET`). When the budget is exhausted `None` is returned
//! conservatively, falling back to a runtime check.
//!
//! # Spec reference
//!
//! - Issue #239: compile-time evaluation of pure functions (constant folding)

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, ElseBranch, Expr, FnDecl, Literal, MatchArm, MatchBody, Pattern, Stmt, UnaryOp,
};

/// Maximum evaluation steps. Prevents non-termination in recursive functions.
const EVAL_BUDGET: usize = 10_000;

// ── Public types ──────────────────────────────────────────────────────────────

/// A compile-time constant value produced by the evaluator.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Integer(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Unit,
}

impl ConstValue {
    /// Convert back to an AST `Literal` for injection into refinement checking.
    pub fn to_literal(&self) -> Literal {
        match self {
            ConstValue::Integer(n) => Literal::Integer(*n),
            ConstValue::Float(f) => Literal::Float(*f),
            ConstValue::Str(s) => Literal::Str(s.clone()),
            ConstValue::Bool(b) => Literal::Bool(*b),
            ConstValue::Unit => Literal::Unit,
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Try to extract a `ConstValue` from an expression that is already a literal
/// (or a negated literal). Returns `None` for anything else.
pub fn expr_as_const(expr: &Expr) -> Option<ConstValue> {
    match expr {
        Expr::Literal(Literal::Integer(n), _) => Some(ConstValue::Integer(*n)),
        Expr::Literal(Literal::Float(f), _) => Some(ConstValue::Float(*f)),
        Expr::Literal(Literal::Str(s), _) => Some(ConstValue::Str(s.clone())),
        Expr::Literal(Literal::Bool(b), _) => Some(ConstValue::Bool(*b)),
        Expr::Literal(Literal::Unit, _) => Some(ConstValue::Unit),
        Expr::Literal(Literal::Char(_), _) => None,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            ..
        } => match inner.as_ref() {
            Expr::Literal(Literal::Integer(n), _) => n.checked_neg().map(ConstValue::Integer),
            Expr::Literal(Literal::Float(f), _) => Some(ConstValue::Float(-f)),
            _ => None,
        },
        _ => None,
    }
}

/// Return `true` if every expression in `args` is a literal (or negated literal).
pub fn all_literal_args(args: &[Expr]) -> bool {
    args.iter().all(|a| expr_as_const(a).is_some())
}

/// Try to constant-fold a call to `fn_decl` with the given argument expressions.
///
/// Succeeds only when:
/// - `fn_decl.effects` is empty (the function is pure),
/// - all `args` reduce to literal values,
/// - evaluation completes within [`EVAL_BUDGET`] steps.
///
/// Returns `None` on any failure, conservatively deferring to runtime.
pub fn try_fold_call(
    fn_decl: &FnDecl,
    args: &[Expr],
    fn_decls: &HashMap<String, FnDecl>,
) -> Option<ConstValue> {
    if !fn_decl.effects.is_empty() {
        return None;
    }
    let arg_vals: Vec<ConstValue> = args.iter().map(expr_as_const).collect::<Option<Vec<_>>>()?;
    if fn_decl.params.len() != arg_vals.len() {
        return None;
    }
    let mut env: HashMap<String, ConstValue> = fn_decl
        .params
        .iter()
        .zip(arg_vals.iter())
        .map(|(p, v)| (p.name.clone(), v.clone()))
        .collect();
    let mut budget = EVAL_BUDGET;
    eval_block(&fn_decl.body, &mut env, fn_decls, &mut budget)
}

// ── Internal evaluators ───────────────────────────────────────────────────────

/// Early-return signal threaded through statement evaluation.
enum Signal {
    Value(ConstValue),
    Return(ConstValue),
}

/// Evaluate a block, threading the environment and returning the last value.
/// An early `return` statement short-circuits the block.
fn eval_block(
    block: &Block,
    env: &mut HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<ConstValue> {
    let mut last = ConstValue::Unit;
    for stmt in &block.stmts {
        match eval_stmt(stmt, env, fn_decls, budget)? {
            Signal::Return(v) => return Some(v),
            Signal::Value(v) => last = v,
        }
    }
    Some(last)
}

/// Evaluate a block, preserving the `Return` signal so the caller can propagate it.
fn eval_block_signal(
    block: &Block,
    env: &mut HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<Signal> {
    let mut last = ConstValue::Unit;
    for stmt in &block.stmts {
        match eval_stmt(stmt, env, fn_decls, budget)? {
            Signal::Return(v) => return Some(Signal::Return(v)),
            Signal::Value(v) => last = v,
        }
    }
    Some(Signal::Value(last))
}

fn eval_stmt(
    stmt: &Stmt,
    env: &mut HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<Signal> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;

    match stmt {
        Stmt::Let { pattern, init, .. } => {
            let val = eval_expr(init, env, fn_decls, budget)?;
            if let Pattern::Ident(name, _) = pattern {
                env.insert(name.clone(), val);
            }
            Some(Signal::Value(ConstValue::Unit))
        }
        Stmt::Return { value, .. } => {
            let val = match value {
                Some(e) => eval_expr(e, env, fn_decls, budget)?,
                None => ConstValue::Unit,
            };
            Some(Signal::Return(val))
        }
        Stmt::Expr { expr, .. } => {
            let val = eval_expr(expr, env, fn_decls, budget)?;
            Some(Signal::Value(val))
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            let cond_val = eval_expr(cond, env, fn_decls, budget)?;
            match cond_val {
                ConstValue::Bool(true) => eval_block_signal(then, env, fn_decls, budget),
                ConstValue::Bool(false) => match else_ {
                    None => Some(Signal::Value(ConstValue::Unit)),
                    Some(ElseBranch::Block(b)) => eval_block_signal(b, env, fn_decls, budget),
                    Some(ElseBranch::If(s)) => eval_stmt(s, env, fn_decls, budget),
                },
                _ => None,
            }
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let val = eval_expr(scrutinee, env, fn_decls, budget)?;
            for arm in arms {
                if let Some(mut arm_env) = match_pattern(&arm.pattern, &val, env) {
                    // Guards reference RefExpr, not Expr — cannot evaluate statically.
                    // Return None immediately rather than continuing to the next arm:
                    // skipping a matching guarded arm and executing a later arm would
                    // produce the wrong result.
                    if arm.guard.is_some() {
                        return None;
                    }
                    let result = match &arm.body {
                        MatchBody::Expr(e) => eval_expr(e, &arm_env, fn_decls, budget)?,
                        MatchBody::Block(b) => eval_block(b, &mut arm_env, fn_decls, budget)?,
                    };
                    return Some(Signal::Value(result));
                }
            }
            None
        }
        // Loops and mutations are not folded (conservative).
        Stmt::Assign { .. } | Stmt::While { .. } | Stmt::For { .. } => None,
    }
}

fn eval_expr(
    expr: &Expr,
    env: &HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<ConstValue> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;

    match expr {
        Expr::Literal(lit, _) => match lit {
            Literal::Integer(n) => Some(ConstValue::Integer(*n)),
            Literal::Float(f) => Some(ConstValue::Float(*f)),
            Literal::Str(s) => Some(ConstValue::Str(s.clone())),
            Literal::Bool(b) => Some(ConstValue::Bool(*b)),
            Literal::Unit => Some(ConstValue::Unit),
            Literal::Char(_) => None,
        },

        Expr::Ident(name, _) => env.get(name).cloned(),

        Expr::Unary {
            op, expr: inner, ..
        } => {
            let val = eval_expr(inner, env, fn_decls, budget)?;
            match op {
                UnaryOp::Neg => match val {
                    ConstValue::Integer(n) => n.checked_neg().map(ConstValue::Integer),
                    ConstValue::Float(f) => Some(ConstValue::Float(-f)),
                    _ => None,
                },
                UnaryOp::Not => match val {
                    ConstValue::Bool(b) => Some(ConstValue::Bool(!b)),
                    _ => None,
                },
                UnaryOp::Deref => None,
            }
        }

        Expr::Binary {
            op, left, right, ..
        } => {
            let lv = eval_expr(left, env, fn_decls, budget)?;
            let rv = eval_expr(right, env, fn_decls, budget)?;
            eval_binary(*op, lv, rv)
        }

        Expr::If {
            cond, then, else_, ..
        } => {
            let cond_val = eval_expr(cond, env, fn_decls, budget)?;
            match cond_val {
                ConstValue::Bool(true) => eval_block_expr(then, env, fn_decls, budget),
                ConstValue::Bool(false) => match else_ {
                    None => Some(ConstValue::Unit),
                    Some(e) => eval_expr(e, env, fn_decls, budget),
                },
                _ => None,
            }
        }

        Expr::Block(block) => eval_block_expr(block, env, fn_decls, budget),

        Expr::FnCall { name, args, .. } => {
            let fn_decl = fn_decls.get(name)?;
            if !fn_decl.effects.is_empty() {
                return None;
            }
            // Evaluate arguments in the *caller's* environment first.
            let arg_vals: Vec<ConstValue> = args
                .iter()
                .map(|a| eval_expr(a, env, fn_decls, budget))
                .collect::<Option<Vec<_>>>()?;
            if fn_decl.params.len() != arg_vals.len() {
                return None;
            }
            let mut callee_env: HashMap<String, ConstValue> = fn_decl
                .params
                .iter()
                .zip(arg_vals.iter())
                .map(|(p, v)| (p.name.clone(), v.clone()))
                .collect();
            eval_block(&fn_decl.body, &mut callee_env, fn_decls, budget)
        }

        Expr::Match {
            scrutinee, arms, ..
        } => {
            let val = eval_expr(scrutinee, env, fn_decls, budget)?;
            eval_match_arms(&val, arms, env, fn_decls, budget)
        }

        // Everything else is not foldable at compile time.
        _ => None,
    }
}

/// Evaluate a `Block` as an expression (returns the last evaluated value or Unit).
fn eval_block_expr(
    block: &Block,
    env: &HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<ConstValue> {
    // Clone env so inner `let` bindings don't leak to the outer scope.
    let mut inner_env = env.clone();
    let mut last = ConstValue::Unit;
    for stmt in &block.stmts {
        match eval_stmt(stmt, &mut inner_env, fn_decls, budget)? {
            Signal::Return(v) => return Some(v),
            Signal::Value(v) => last = v,
        }
    }
    Some(last)
}

fn eval_match_arms(
    val: &ConstValue,
    arms: &[MatchArm],
    env: &HashMap<String, ConstValue>,
    fn_decls: &HashMap<String, FnDecl>,
    budget: &mut usize,
) -> Option<ConstValue> {
    for arm in arms {
        if let Some(mut arm_env) = match_pattern(&arm.pattern, val, env) {
            // Guards reference RefExpr — cannot evaluate statically.
            // Return None immediately rather than continuing to the next arm:
            // skipping a matching guarded arm and executing a later arm would
            // produce the wrong result.
            if arm.guard.is_some() {
                return None;
            }
            return match &arm.body {
                MatchBody::Expr(e) => eval_expr(e, &arm_env, fn_decls, budget),
                MatchBody::Block(b) => eval_block(b, &mut arm_env, fn_decls, budget),
            };
        }
    }
    None
}

/// Attempt to match `val` against `pattern`, returning an updated environment
/// with any new bindings, or `None` if the pattern does not match.
fn match_pattern(
    pattern: &Pattern,
    val: &ConstValue,
    env: &HashMap<String, ConstValue>,
) -> Option<HashMap<String, ConstValue>> {
    let mut new_env = env.clone();
    match (pattern, val) {
        (Pattern::Wildcard(_), _) => Some(new_env),
        (Pattern::Ident(name, _), v) => {
            new_env.insert(name.clone(), v.clone());
            Some(new_env)
        }
        (Pattern::Literal(Literal::Integer(n), _), ConstValue::Integer(v)) => {
            if n == v {
                Some(new_env)
            } else {
                None
            }
        }
        (Pattern::Literal(Literal::Bool(b), _), ConstValue::Bool(v)) => {
            if b == v {
                Some(new_env)
            } else {
                None
            }
        }
        (Pattern::Literal(Literal::Str(s), _), ConstValue::Str(v)) => {
            if s == v {
                Some(new_env)
            } else {
                None
            }
        }
        (Pattern::Literal(Literal::Unit, _), ConstValue::Unit) => Some(new_env),
        _ => None,
    }
}

fn eval_binary(op: BinaryOp, lv: ConstValue, rv: ConstValue) -> Option<ConstValue> {
    use BinaryOp::*;
    match (op, lv, rv) {
        // Integer arithmetic (checked to avoid panics)
        (Add, ConstValue::Integer(l), ConstValue::Integer(r)) => {
            l.checked_add(r).map(ConstValue::Integer)
        }
        (Sub, ConstValue::Integer(l), ConstValue::Integer(r)) => {
            l.checked_sub(r).map(ConstValue::Integer)
        }
        (Mul, ConstValue::Integer(l), ConstValue::Integer(r)) => {
            l.checked_mul(r).map(ConstValue::Integer)
        }
        (Div, ConstValue::Integer(l), ConstValue::Integer(r)) => {
            if r == 0 {
                None
            } else {
                l.checked_div(r).map(ConstValue::Integer)
            }
        }
        (Rem, ConstValue::Integer(l), ConstValue::Integer(r)) => {
            if r == 0 {
                None
            } else {
                l.checked_rem(r).map(ConstValue::Integer)
            }
        }
        // Integer comparisons
        (Eq, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l == r)),
        (Ne, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l != r)),
        (Lt, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l < r)),
        (Gt, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l > r)),
        (Le, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l <= r)),
        (Ge, ConstValue::Integer(l), ConstValue::Integer(r)) => Some(ConstValue::Bool(l >= r)),
        // Float arithmetic — guard NaN inputs and non-finite results.
        (Add, ConstValue::Float(l), ConstValue::Float(r)) => {
            let v = l + r;
            if v.is_finite() {
                Some(ConstValue::Float(v))
            } else {
                None
            }
        }
        (Sub, ConstValue::Float(l), ConstValue::Float(r)) => {
            let v = l - r;
            if v.is_finite() {
                Some(ConstValue::Float(v))
            } else {
                None
            }
        }
        (Mul, ConstValue::Float(l), ConstValue::Float(r)) => {
            let v = l * r;
            if v.is_finite() {
                Some(ConstValue::Float(v))
            } else {
                None
            }
        }
        (Div, ConstValue::Float(l), ConstValue::Float(r)) => {
            if r == 0.0 || r.is_nan() || l.is_nan() {
                None
            } else {
                let v = l / r;
                if v.is_finite() {
                    Some(ConstValue::Float(v))
                } else {
                    None
                }
            }
        }
        // Float comparisons — NaN operands produce unreliable results; return None.
        (Eq, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l == r))
        }
        (Ne, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l != r))
        }
        (Lt, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l < r))
        }
        (Gt, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l > r))
        }
        (Le, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l <= r))
        }
        (Ge, ConstValue::Float(l), ConstValue::Float(r)) if !l.is_nan() && !r.is_nan() => {
            Some(ConstValue::Bool(l >= r))
        }
        // Boolean logic
        (And, ConstValue::Bool(l), ConstValue::Bool(r)) => Some(ConstValue::Bool(l && r)),
        (Or, ConstValue::Bool(l), ConstValue::Bool(r)) => Some(ConstValue::Bool(l || r)),
        // String equality
        (Eq, ConstValue::Str(l), ConstValue::Str(r)) => Some(ConstValue::Bool(l == r)),
        (Ne, ConstValue::Str(l), ConstValue::Str(r)) => Some(ConstValue::Bool(l != r)),
        _ => None,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::Decl;
    use crate::mvl::parser::Parser;

    fn parse_fn(src: &str) -> FnDecl {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        prog.declarations
            .into_iter()
            .find_map(|d| if let Decl::Fn(f) = d { Some(f) } else { None })
            .expect("expected a fn decl")
    }

    fn make_fn_map(fns: Vec<FnDecl>) -> HashMap<String, FnDecl> {
        fns.into_iter().map(|f| (f.name.clone(), f)).collect()
    }

    #[test]
    fn fold_simple_addition() {
        let fd = parse_fn("fn add(a: Int, b: Int) -> Int { a + b }");
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        let args = vec![
            Expr::Literal(Literal::Integer(3), dummy),
            Expr::Literal(Literal::Integer(4), dummy),
        ];
        let result = try_fold_call(&fd, &args, &fns);
        assert_eq!(result, Some(ConstValue::Integer(7)));
    }

    #[test]
    fn fold_factorial_5() {
        let src = "total fn factorial(n: Int) -> Int {
            match n {
                0 => 1,
                _ => n * factorial(n - 1),
            }
        }";
        let fd = parse_fn(src);
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        let args = vec![Expr::Literal(Literal::Integer(5), dummy)];
        let result = try_fold_call(&fd, &args, &fns);
        assert_eq!(result, Some(ConstValue::Integer(120)));
    }

    #[test]
    fn effectful_fn_not_folded() {
        let fd = parse_fn("fn noisy(x: Int) -> Int ! Console { println(x); x }");
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        let args = vec![Expr::Literal(Literal::Integer(1), dummy)];
        assert_eq!(try_fold_call(&fd, &args, &fns), None);
    }

    #[test]
    fn non_literal_arg_not_folded() {
        let fd = parse_fn("fn double(x: Int) -> Int { x * 2 }");
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        // Pass a variable reference, not a literal.
        let args = vec![Expr::Ident("n".to_string(), dummy)];
        assert_eq!(try_fold_call(&fd, &args, &fns), None);
    }

    #[test]
    fn division_by_zero_returns_none() {
        let fd = parse_fn("fn div(a: Int, b: Int) -> Int { a / b }");
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        let args = vec![
            Expr::Literal(Literal::Integer(10), dummy),
            Expr::Literal(Literal::Integer(0), dummy),
        ];
        assert_eq!(try_fold_call(&fd, &args, &fns), None);
    }

    #[test]
    fn fold_if_expr() {
        let fd = parse_fn("fn max_val(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
        let fns = make_fn_map(vec![fd.clone()]);
        let dummy = crate::mvl::parser::lexer::Span::default();
        let args = vec![
            Expr::Literal(Literal::Integer(7), dummy),
            Expr::Literal(Literal::Integer(3), dummy),
        ];
        let result = try_fold_call(&fd, &args, &fns);
        assert_eq!(result, Some(ConstValue::Integer(7)));
    }
}
