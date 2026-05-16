// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Interprocedural IFC analysis: label propagation and violation detection.
//!
//! Implements three sub-tickets of the IFC epic (#825):
//!
//! - **#833 Return-label inference**: Infer security labels for unannotated
//!   function return types from body analysis and an external-source registry.
//!
//! - **#830 Forward label propagation**: Fixed-point algorithm — propagate
//!   inferred labels through function bodies until stable.
//!
//! - **#831 Interprocedural violation detection**: After propagation, detect
//!   call sites where an inferred arg label cannot flow to the required
//!   parameter label; emit [`CheckError::InterprocFlowViolation`] with a
//!   call-chain path.
//!
//! # Algorithm
//!
//! ```text
//! 1. Seed: TypeEnv explicit return labels (stdlib stdin_read_line → Tainted, etc.)
//! 2. Seed: external taint source registry (#833)
//! 3. Fixed-point (#830 + #833):
//!    for each unannotated fn F in user programs:
//!      infer return label from body using current table
//!      if label changed → mark changed
//!    repeat until stable
//! 4. Violation detection (#831):
//!    for each fn body, at each FnCall callee(args):
//!      if callee has labeled params in TypeEnv:
//!        infer arg label using enhanced infer_label (checks table)
//!        if inferred > required → emit InterprocFlowViolation
//! ```
//!
//! # References
//! - #830 — forward label propagation
//! - #831 — interprocedural violation detection
//! - #833 — return-label inference
//! - #825 — interprocedural IFC epic

use std::collections::HashMap;

use crate::mvl::checker::context::TypeEnv;
use crate::mvl::checker::ifc;
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, MatchBody, Program, SecurityLabel, Stmt, TypeExpr,
};

// ── External taint source registry (#833) ─────────────────────────────────────

/// Function names that always return `Tainted` data, independent of arguments.
///
/// This supplements TypeEnv's explicit labels (e.g., `stdin_read_line → Tainted[String]`
/// from std.io) for environments where the full stdlib is not loaded — for example,
/// standalone test programs or sandboxed compilation.
const TAINT_SOURCES: &[&str] = &["read_line", "args", "read_tainted"];

// ── Inferred label table ───────────────────────────────────────────────────────

/// Inferred security labels for function return types.
///
/// Built by [`propagate`]; stored in [`CheckResult`] for use by downstream
/// passes and tools.  Explicit TypeEnv annotations (seeded first) cannot be
/// downgraded by inference.
#[derive(Debug, Default, Clone)]
pub struct InferredLabels(HashMap<String, SecurityLabel>);

impl InferredLabels {
    /// Return the inferred return label for `fn_name`, if any.
    pub fn get(&self, fn_name: &str) -> Option<SecurityLabel> {
        self.0.get(fn_name).copied()
    }
}

// ── Propagation (#830 + #833) ─────────────────────────────────────────────────

/// Build the inferred label table by fixed-point body-analysis propagation.
///
/// Seeds from TypeEnv explicit return labels (covers all loaded stdlib functions),
/// then augments with the hardcoded external-source registry (#833), and finally
/// runs a fixed-point loop over user program function bodies (#830).
///
/// Called from `check_with_two_preludes_mode` with the user-supplied programs
/// (excluding prelude slices — their annotations are already in TypeEnv).
pub fn propagate(programs: &[&Program], type_env: &TypeEnv) -> InferredLabels {
    let mut table: HashMap<String, SecurityLabel> = HashMap::new();

    // Seed 1: explicit return labels from TypeEnv (covers stdlib taint sources).
    for (name, fn_info) in &type_env.fns {
        if let Some(label) = ifc::label_of(&fn_info.ret) {
            table.insert(name.clone(), label);
        }
    }

    // Seed 2: external taint source registry (#833) — safety net for unloaded stdlib.
    for &src in TAINT_SOURCES {
        table
            .entry(src.to_string())
            .or_insert(SecurityLabel::Tainted);
    }

    // Fixed-point body-analysis loop (#830 + #833).
    loop {
        let mut changed = false;
        for prog in programs {
            for decl in &prog.declarations {
                if let Decl::Fn(fd) = decl {
                    // Skip functions with an explicit return label — annotation wins.
                    if label_of_type_expr(&fd.return_type).is_some() {
                        continue;
                    }
                    // Build param label env from declared annotations.
                    let mut param_env: HashMap<String, SecurityLabel> = HashMap::new();
                    for param in &fd.params {
                        if let Some(l) = label_of_type_expr(&param.ty) {
                            param_env.insert(param.name.clone(), l);
                        }
                    }
                    // Infer return label from body return expressions.
                    if let Some(label) = infer_return_label(&fd.body, &param_env, &table) {
                        let current = table.get(&fd.name).copied();
                        let new_label = match current {
                            Some(c) => ifc::join(c, label),
                            None => label,
                        };
                        if current != Some(new_label) {
                            table.insert(fd.name.clone(), new_label);
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    InferredLabels(table)
}

/// Infer the return label for a function body from its return points.
///
/// Returns the join of labels inferred from:
/// - Explicit `return expr` statements anywhere in the body.
/// - The tail expression of the block (implicit return value).
fn infer_return_label(
    block: &Block,
    param_env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    let explicit = collect_explicit_returns(block, param_env, table);
    let tail = block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, param_env, table)
        } else {
            None
        }
    });
    ifc::join_opt(explicit, tail)
}

/// Walk a block collecting labels from explicit `return expr` statements.
fn collect_explicit_returns(
    block: &Block,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    block
        .stmts
        .iter()
        .map(|s| collect_returns_in_stmt(s, env, table))
        .fold(None, ifc::join_opt)
}

fn collect_returns_in_stmt(
    stmt: &Stmt,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    match stmt {
        Stmt::Return { value: Some(e), .. } => infer_label_extended(e, env, table),
        Stmt::Return { value: None, .. } => None,
        Stmt::If { then, else_, .. } => {
            let then_ret = collect_explicit_returns(then, env, table);
            let else_ret = match else_ {
                Some(ElseBranch::Block(b)) => collect_explicit_returns(b, env, table),
                Some(ElseBranch::If(s)) => collect_returns_in_stmt(s, env, table),
                None => None,
            };
            ifc::join_opt(then_ret, else_ret)
        }
        Stmt::Match { arms, .. } => arms
            .iter()
            .map(|arm| match &arm.body {
                MatchBody::Block(b) => collect_explicit_returns(b, env, table),
                MatchBody::Expr(_) => None,
            })
            .fold(None, ifc::join_opt),
        Stmt::While { body, .. } | Stmt::For { body, .. } => {
            collect_explicit_returns(body, env, table)
        }
        _ => None,
    }
}

// ── Extended label inference ──────────────────────────────────────────────────

/// Infer the security label of an expression using the inferred label table.
///
/// Extends `ifc::infer_label` by looking up FnCall callee return labels in
/// `table` (both TypeEnv explicit and propagation-inferred labels), giving
/// correct results for chains through unannotated wrapper functions.
pub fn infer_label_extended(
    expr: &Expr,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    match expr {
        Expr::Ident(name, _) => env.get(name.as_str()).copied(),
        Expr::FnCall { name, args, .. } => {
            // If the callee has a known return label (explicit TypeEnv annotation or
            // inferred via body analysis), use it exclusively.
            // Explicit annotations (e.g. clean_input → Clean, sanitize → Clean) guarantee
            // the output label regardless of input labels — joining with arg labels would
            // produce false positives (e.g. treating clean_input(tainted) as Tainted).
            if let Some(label) = table.get(name.as_str()) {
                return Some(*label);
            }
            // Callee has no known return label — conservatively propagate from args.
            args.iter()
                .map(|a| infer_label_extended(a, env, table))
                .fold(None, ifc::join_opt)
        }
        // declassify/sanitize always produce specific labels.
        Expr::Declassify { .. } => Some(SecurityLabel::Public),
        Expr::Sanitize { .. } => Some(SecurityLabel::Clean),
        Expr::Binary { left, right, .. } => ifc::join_opt(
            infer_label_extended(left, env, table),
            infer_label_extended(right, env, table),
        ),
        Expr::Unary { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. } => infer_label_extended(expr, env, table),
        Expr::If {
            cond, then, else_, ..
        } => {
            let cond_label = infer_label_extended(cond, env, table);
            let then_label = tail_label_of_block(then, env, table);
            let else_label = else_
                .as_ref()
                .and_then(|e| infer_label_extended(e, env, table));
            ifc::join_opt(cond_label, ifc::join_opt(then_label, else_label))
        }
        Expr::Block(b) => tail_label_of_block(b, env, table),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scr = infer_label_extended(scrutinee, env, table);
            let arms_label = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Expr(e) => infer_label_extended(e, env, table),
                    MatchBody::Block(b) => tail_label_of_block(b, env, table),
                })
                .fold(None, ifc::join_opt);
            ifc::join_opt(scr, arms_label)
        }
        Expr::MethodCall { receiver, args, .. } => {
            let recv = infer_label_extended(receiver, env, table);
            let arg_label = args
                .iter()
                .map(|a| infer_label_extended(a, env, table))
                .fold(None, ifc::join_opt);
            ifc::join_opt(recv, arg_label)
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => fields
            .iter()
            .map(|(_, e)| infer_label_extended(e, env, table))
            .fold(None, ifc::join_opt),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems
            .iter()
            .map(|e| infer_label_extended(e, env, table))
            .fold(None, ifc::join_opt),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .map(|(k, v)| {
                ifc::join_opt(
                    infer_label_extended(k, env, table),
                    infer_label_extended(v, env, table),
                )
            })
            .fold(None, ifc::join_opt),
        Expr::Lambda { body, .. } => infer_label_extended(body, env, table),
        Expr::Select { arms, .. } => arms
            .iter()
            .map(|a| infer_label_extended(&a.expr, env, table))
            .fold(None, ifc::join_opt),
        Expr::Concurrently { body, .. } => tail_label_of_block(body, env, table),
        Expr::Literal(..) => None,
    }
}

/// Return the label of the tail expression of a block, if it ends in an expression.
fn tail_label_of_block(
    block: &Block,
    env: &HashMap<String, SecurityLabel>,
    table: &HashMap<String, SecurityLabel>,
) -> Option<SecurityLabel> {
    block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, env, table)
        } else {
            None
        }
    })
}

// ── Label extraction from TypeExpr ───────────────────────────────────────────

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
        | TypeExpr::IntConst { .. }
        | TypeExpr::Session { .. } => None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::context::TypeEnv;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        p.parse_program()
    }

    fn env_with_taint_source() -> TypeEnv {
        use crate::mvl::checker::context::FnInfo;
        use crate::mvl::checker::types::Ty;
        let mut env = TypeEnv::default();
        // Simulate: fn source() -> Tainted[String]
        env.fns.insert(
            "source".into(),
            FnInfo {
                ret: Ty::Labeled(SecurityLabel::Tainted, Box::new(Ty::String)),
                ..Default::default()
            },
        );
        // Simulate: fn sink(q: Clean[String]) -> Unit
        env.fns.insert(
            "sink".into(),
            FnInfo {
                params: vec![Ty::Labeled(SecurityLabel::Clean, Box::new(Ty::String))],
                ret: Ty::Unit,
                ..Default::default()
            },
        );
        env
    }

    // ── Propagation tests ─────────────────────────────────────────────────

    #[test]
    fn external_source_registry_seeds_tainted() {
        let prog = parse("");
        let env = TypeEnv::default();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("read_line"), Some(SecurityLabel::Tainted));
        assert_eq!(labels.get("args"), Some(SecurityLabel::Tainted));
    }

    #[test]
    fn typeenv_explicit_label_seeded() {
        let prog = parse("");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("source"), Some(SecurityLabel::Tainted));
    }

    #[test]
    fn unannotated_fn_calling_taint_source_inferred_tainted() {
        // fn wrapper() -> String { source() }
        // source() is in TypeEnv as Tainted[String]
        let prog = parse("fn wrapper() -> String { source() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(
            labels.get("wrapper"),
            Some(SecurityLabel::Tainted),
            "wrapper calling Tainted source should be inferred Tainted"
        );
    }

    #[test]
    fn annotated_fn_not_overridden() {
        // fn clean() -> Clean[String] { source() }  — annotation wins
        let prog = parse("fn clean() -> Clean[String] { source() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        // clean() has explicit label Clean — propagation must not upgrade it
        // (it's skipped by the "annotation wins" guard)
        assert_eq!(
            labels.get("clean"),
            None,
            "explicitly annotated fn should not be inferred"
        );
    }

    #[test]
    fn chain_propagation_two_hops() {
        // fn step1() -> String { source() }
        // fn step2() -> String { step1() }
        let prog = parse("fn step1() -> String { source() } fn step2() -> String { step1() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("step1"), Some(SecurityLabel::Tainted));
        assert_eq!(
            labels.get("step2"),
            Some(SecurityLabel::Tainted),
            "step2 calling step1 (Tainted) should be inferred Tainted"
        );
    }

    #[test]
    fn external_registry_read_line_propagates() {
        // read_line is in TAINT_SOURCES — even without TypeEnv registration
        let prog = parse("fn wrapper() -> String { read_line() }");
        let env = TypeEnv::default();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("wrapper"), Some(SecurityLabel::Tainted));
    }
}
