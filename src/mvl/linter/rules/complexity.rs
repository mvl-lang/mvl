// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase 4 complexity rules — regenerability metrics.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{BinaryOp, Block, Decl, ElseBranch, Expr, MatchBody, Program, Stmt};
use std::collections::{HashMap, HashSet};

// ── Phase 4: Complexity rules ───────────────────────────────────────────────

/// Flag functions whose cyclomatic complexity exceeds `cfg.max_cyclomatic_complexity`.
///
/// Rule id: `complexity-cyclomatic`
///
/// Cyclomatic complexity counts the independent paths through a function:
/// start at 1, add 1 for each `if`, `else if`, `while`, `for`, `match` arm,
/// and each short-circuit logical operator (`&&`, `||`) in conditions.
pub fn complexity_cyclomatic(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_cyclomatic_complexity == 0 {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                let cc = cyclomatic_complexity_block(&f.body);
                if cc > cfg.max_cyclomatic_complexity {
                    out.push(LintDiag::warning(
                        "complexity-cyclomatic",
                        format!(
                            "function `{}` has cyclomatic complexity {cc} (max {})",
                            f.name, cfg.max_cyclomatic_complexity
                        ),
                        f.span.line,
                        f.span.col,
                    ));
                }
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let cc = cyclomatic_complexity_block(&method.body);
                    if cc > cfg.max_cyclomatic_complexity {
                        out.push(LintDiag::warning(
                            "complexity-cyclomatic",
                            format!(
                                "method `{}` (impl {} for {}) has cyclomatic complexity {cc} (max {})",
                                method.name,
                                impl_decl.trait_name,
                                impl_decl.type_name,
                                cfg.max_cyclomatic_complexity
                            ),
                            method.span.line,
                            method.span.col,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn cyclomatic_complexity_block(block: &Block) -> usize {
    let mut cc = 1usize;
    for stmt in &block.stmts {
        cc += cyclomatic_complexity_stmt(stmt);
    }
    cc
}

fn cyclomatic_complexity_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::If {
            cond, then, else_, ..
        } => {
            let mut cc = 1; // the if itself
            cc += cyclomatic_complexity_expr(cond);
            cc += cyclomatic_complexity_block_inner(then);
            match else_ {
                Some(ElseBranch::Block(b)) => cc += cyclomatic_complexity_block_inner(b),
                Some(ElseBranch::If(inner)) => {
                    cc += cyclomatic_complexity_stmt(inner);
                }
                None => {}
            }
            cc
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let mut cc = arms.len().saturating_sub(1); // each arm beyond first
            cc += cyclomatic_complexity_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => cc += cyclomatic_complexity_block_inner(b),
                    MatchBody::Expr(e) => cc += cyclomatic_complexity_expr(e),
                }
            }
            cc
        }
        Stmt::While { cond, body, .. } => {
            let mut cc = 1;
            cc += cyclomatic_complexity_expr(cond);
            cc += cyclomatic_complexity_block_inner(body);
            cc
        }
        Stmt::For { iter, body, .. } => {
            let mut cc = 1;
            cc += cyclomatic_complexity_expr(iter);
            cc += cyclomatic_complexity_block_inner(body);
            cc
        }
        Stmt::Let { init, .. } | Stmt::Assign { value: init, .. } => {
            cyclomatic_complexity_expr(init)
        }
        Stmt::Return { value: Some(e), .. } | Stmt::Expr { expr: e, .. } => {
            cyclomatic_complexity_expr(e)
        }
        Stmt::Return { value: None, .. } => 0,
    }
}

/// Count decision-point contributions from expressions (without the base +1).
fn cyclomatic_complexity_expr(expr: &Expr) -> usize {
    match expr {
        Expr::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            left,
            right,
            ..
        } => 1 + cyclomatic_complexity_expr(left) + cyclomatic_complexity_expr(right),
        Expr::Binary { left, right, .. } => {
            cyclomatic_complexity_expr(left) + cyclomatic_complexity_expr(right)
        }
        Expr::Unary { expr: e, .. } => cyclomatic_complexity_expr(e),
        Expr::If {
            cond, then, else_, ..
        } => {
            let mut cc = 1;
            cc += cyclomatic_complexity_expr(cond);
            cc += cyclomatic_complexity_block_inner(then);
            if let Some(e) = else_ {
                cc += cyclomatic_complexity_expr(e);
            }
            cc
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let mut cc = arms.len().saturating_sub(1);
            cc += cyclomatic_complexity_expr(scrutinee);
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => cc += cyclomatic_complexity_block_inner(b),
                    MatchBody::Expr(e) => cc += cyclomatic_complexity_expr(e),
                }
            }
            cc
        }
        Expr::Block(b) => cyclomatic_complexity_block_inner(b),
        Expr::FnCall { args, .. } => args.iter().map(cyclomatic_complexity_expr).sum(),
        Expr::MethodCall { receiver, args, .. } => {
            cyclomatic_complexity_expr(receiver)
                + args.iter().map(cyclomatic_complexity_expr).sum::<usize>()
        }
        Expr::FieldAccess { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => cyclomatic_complexity_expr(e),
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => fields
            .iter()
            .map(|(_, e)| cyclomatic_complexity_expr(e))
            .sum(),
        Expr::Select { arms, .. } => arms
            .iter()
            .map(|a| {
                1 + cyclomatic_complexity_expr(&a.expr) + cyclomatic_complexity_block_inner(&a.body)
            })
            .sum(),
        Expr::Concurrently { body, .. } => cyclomatic_complexity_block_inner(body),
        Expr::Quantifier(..) => 0,
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().map(cyclomatic_complexity_expr).sum()
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .map(|(k, v)| cyclomatic_complexity_expr(k) + cyclomatic_complexity_expr(v))
            .sum(),
        Expr::Lambda { body, .. } => cyclomatic_complexity_expr(body),
        Expr::Literal(..) | Expr::Ident(..) => 0,
    }
}

/// Sum contributions of all statements in a block (without adding the base +1).
fn cyclomatic_complexity_block_inner(block: &Block) -> usize {
    block.stmts.iter().map(cyclomatic_complexity_stmt).sum()
}

/// Flag functions where `match` expressions are nested deeper than
/// `cfg.max_nested_match_depth`.
///
/// Rule id: `complexity-match-depth`
pub fn complexity_match_depth(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_nested_match_depth == 0 {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                let depth = max_match_depth_block(&f.body, 0);
                if depth > cfg.max_nested_match_depth {
                    out.push(LintDiag::warning(
                        "complexity-match-depth",
                        format!(
                            "function `{}` has nested match depth {depth} (max {})",
                            f.name, cfg.max_nested_match_depth
                        ),
                        f.span.line,
                        f.span.col,
                    ));
                }
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let depth = max_match_depth_block(&method.body, 0);
                    if depth > cfg.max_nested_match_depth {
                        out.push(LintDiag::warning(
                            "complexity-match-depth",
                            format!(
                                "method `{}` (impl {} for {}) has nested match depth {depth} (max {})",
                                method.name,
                                impl_decl.trait_name,
                                impl_decl.type_name,
                                cfg.max_nested_match_depth
                            ),
                            method.span.line,
                            method.span.col,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn max_match_depth_block(block: &Block, current_depth: usize) -> usize {
    block
        .stmts
        .iter()
        .map(|s| max_match_depth_stmt(s, current_depth))
        .max()
        .unwrap_or(current_depth)
}

fn max_match_depth_stmt(stmt: &Stmt, depth: usize) -> usize {
    match stmt {
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            let inner_depth = depth + 1;
            let from_scrutinee = max_match_depth_expr(scrutinee, inner_depth);
            let from_arms = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Block(b) => max_match_depth_block(b, inner_depth),
                    MatchBody::Expr(e) => max_match_depth_expr(e, inner_depth),
                })
                .max()
                .unwrap_or(inner_depth);
            inner_depth.max(from_scrutinee).max(from_arms)
        }
        Stmt::If {
            cond, then, else_, ..
        } => {
            let from_cond = max_match_depth_expr(cond, depth);
            let from_then = max_match_depth_block(then, depth);
            let from_else = match else_ {
                Some(ElseBranch::Block(b)) => max_match_depth_block(b, depth),
                Some(ElseBranch::If(s)) => max_match_depth_stmt(s, depth),
                None => depth,
            };
            from_cond.max(from_then).max(from_else)
        }
        Stmt::While { cond, body, .. }
        | Stmt::For {
            iter: cond, body, ..
        } => max_match_depth_expr(cond, depth).max(max_match_depth_block(body, depth)),
        Stmt::Let { init, .. } | Stmt::Assign { value: init, .. } => {
            max_match_depth_expr(init, depth)
        }
        Stmt::Return { value: Some(e), .. } | Stmt::Expr { expr: e, .. } => {
            max_match_depth_expr(e, depth)
        }
        Stmt::Return { value: None, .. } => depth,
    }
}

fn max_match_depth_expr(expr: &Expr, depth: usize) -> usize {
    match expr {
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let inner_depth = depth + 1;
            let from_scrutinee = max_match_depth_expr(scrutinee, inner_depth);
            let from_arms = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Block(b) => max_match_depth_block(b, inner_depth),
                    MatchBody::Expr(e) => max_match_depth_expr(e, inner_depth),
                })
                .max()
                .unwrap_or(inner_depth);
            inner_depth.max(from_scrutinee).max(from_arms)
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            let from_cond = max_match_depth_expr(cond, depth);
            let from_then = max_match_depth_block(then, depth);
            let from_else = else_
                .as_ref()
                .map(|e| max_match_depth_expr(e, depth))
                .unwrap_or(depth);
            from_cond.max(from_then).max(from_else)
        }
        Expr::Block(b) => max_match_depth_block(b, depth),
        Expr::Binary { left, right, .. } => {
            max_match_depth_expr(left, depth).max(max_match_depth_expr(right, depth))
        }
        Expr::Unary { expr: e, .. }
        | Expr::FieldAccess { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Relabel { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => max_match_depth_expr(e, depth),
        Expr::FnCall { args, .. } => args
            .iter()
            .map(|e| max_match_depth_expr(e, depth))
            .max()
            .unwrap_or(depth),
        Expr::MethodCall { receiver, args, .. } => {
            let r = max_match_depth_expr(receiver, depth);
            args.iter()
                .map(|e| max_match_depth_expr(e, depth))
                .max()
                .unwrap_or(depth)
                .max(r)
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => fields
            .iter()
            .map(|(_, e)| max_match_depth_expr(e, depth))
            .max()
            .unwrap_or(depth),
        Expr::Select { arms, .. } => arms
            .iter()
            .map(|a| max_match_depth_expr(&a.expr, depth))
            .max()
            .unwrap_or(depth),
        Expr::Concurrently { body, .. } => body
            .stmts
            .iter()
            .map(|s| max_match_depth_stmt(s, depth))
            .max()
            .unwrap_or(depth),
        Expr::Quantifier(..) => depth,
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems
            .iter()
            .map(|e| max_match_depth_expr(e, depth))
            .max()
            .unwrap_or(depth),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .map(|(k, v)| max_match_depth_expr(k, depth).max(max_match_depth_expr(v, depth)))
            .max()
            .unwrap_or(depth),
        Expr::Lambda { body, .. } => max_match_depth_expr(body, depth),
        Expr::Literal(..) | Expr::Ident(..) => depth,
    }
}

/// Flag functions that declare more effects than `cfg.max_effect_signature_width`.
///
/// Rule id: `complexity-effect-width`
///
/// A wide effect signature is harder for an LLM to regenerate faithfully and
/// indicates a function with broad side-effect footprint.
///
/// Both free functions (`fn`) and trait impl methods are checked, since
/// `FnDecl` carries an `effects` field in both positions.
pub fn complexity_effect_width(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_effect_signature_width == 0 {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                check_effect_width(&f.name, None, &f.effects, f.span, cfg, out);
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    check_effect_width(
                        &method.name,
                        Some((impl_decl.trait_name.as_str(), impl_decl.type_name.as_str())),
                        &method.effects,
                        method.span,
                        cfg,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
}

fn check_effect_width(
    name: &str,
    impl_ctx: Option<(&str, &str)>,
    effects: &[crate::mvl::parser::ast::Effect],
    span: crate::mvl::parser::lexer::Span,
    cfg: &LintConfig,
    out: &mut Vec<LintDiag>,
) {
    if effects.len() > cfg.max_effect_signature_width {
        let label = match impl_ctx {
            Some((trait_name, type_name)) => {
                format!("method `{name}` (impl {trait_name} for {type_name})")
            }
            None => format!("function `{name}`"),
        };
        out.push(LintDiag::warning(
            "complexity-effect-width",
            format!(
                "{label} declares {} effects [{}] (max {})",
                effects.len(),
                effects
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                cfg.max_effect_signature_width
            ),
            span.line,
            span.col,
        ));
    }
}

/// Flag types that have more trait `impl` blocks than `cfg.max_trait_impl_count`.
///
/// Rule id: `complexity-trait-impl-count`
///
/// Many trait implementations per type indicate high composition complexity —
/// the type participates in many abstraction boundaries.  In MVL this replaces
/// the classical inheritance depth metric.
pub fn complexity_trait_impl_count(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_trait_impl_count == 0 {
        return;
    }
    // Count impl blocks per type name; record the span of the first impl for diagnostics.
    let mut counts: HashMap<&str, (usize, crate::mvl::parser::lexer::Span)> = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Impl(id) = decl {
            let entry = counts.entry(id.type_name.as_str()).or_insert((0, id.span));
            entry.0 += 1;
        }
    }
    for (type_name, (count, span)) in &counts {
        if *count > cfg.max_trait_impl_count {
            out.push(LintDiag::warning(
                "complexity-trait-impl-count",
                format!(
                    "type `{type_name}` has {count} trait impl blocks (max {})",
                    cfg.max_trait_impl_count
                ),
                span.line,
                span.col,
            ));
        }
    }
}

/// Flag files that import from more than `cfg.max_module_fanout` distinct modules.
///
/// Rule id: `complexity-module-fanout`
///
/// Module fan-out measures how many external dependencies a file has.  A high
/// fan-out makes the file fragile: changes in any of those modules can break
/// it, and an LLM must hold all those interfaces in context simultaneously.
pub fn complexity_module_fanout(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_module_fanout == 0 {
        return;
    }
    let mut modules: HashSet<&str> = HashSet::new();
    let mut first_span = None;
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if let Some(root) = ud.path.first() {
                modules.insert(root.as_str());
            }
            if first_span.is_none() {
                first_span = Some(ud.span);
            }
        }
    }
    if modules.len() > cfg.max_module_fanout {
        let span = first_span.unwrap_or(prog.span);
        out.push(LintDiag::warning(
            "complexity-module-fanout",
            format!(
                "file imports from {} distinct modules (max {})",
                modules.len(),
                cfg.max_module_fanout
            ),
            span.line,
            span.col,
        ));
    }
}

/// Flag files where the ratio of `extern` function declarations to total
/// function declarations exceeds `cfg.max_extern_ratio`.
///
/// Rule id: `complexity-extern-ratio`
///
/// A high extern ratio means most of the program's logic is unverifiable —
/// it widens the trust boundary and reduces the portion the compiler can
/// formally check (Req 11).
pub fn complexity_extern_ratio(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_extern_ratio <= 0.0 {
        return;
    }
    let mut total_fns: usize = 0;
    let mut extern_fns: usize = 0;
    let mut first_extern_span = None;

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(_) => total_fns += 1,
            Decl::Impl(id) => total_fns += id.methods.len(),
            Decl::Extern(ed) => {
                extern_fns += ed.fns.len();
                total_fns += ed.fns.len();
                if first_extern_span.is_none() {
                    first_extern_span = Some(ed.span);
                }
            }
            _ => {}
        }
    }
    if total_fns == 0 {
        return;
    }
    if cfg.min_fns_for_extern_ratio > 0 && total_fns < cfg.min_fns_for_extern_ratio {
        return;
    }
    let ratio = extern_fns as f64 / total_fns as f64;
    if ratio > cfg.max_extern_ratio {
        let span = first_extern_span.unwrap_or(prog.span);
        out.push(LintDiag::warning(
            "complexity-extern-ratio",
            format!(
                "extern fns are {:.0}% of all fn declarations ({extern_fns}/{total_fns}, max {:.0}%)",
                ratio * 100.0,
                cfg.max_extern_ratio * 100.0
            ),
            span.line,
            span.col,
        ));
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────
