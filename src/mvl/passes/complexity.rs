// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Static complexity analysis for MVL programs (issue #208).
//!
//! Measures code regenerability and composition complexity as an AST pass.
//! Unlike the linter, this is not threshold-gated — it always emits metrics
//! and lets the caller decide what is noteworthy.
//!
//! # Metrics
//!
//! **Per-function (classical)**
//! - Cyclomatic complexity (CC): independent execution paths through the body.
//! - Function length: number of top-level statements in the body.
//! - Nested match depth: maximum depth of nested `match` expressions.
//! - Effect signature width: number of declared effects (`! Eff1 + Eff2`).
//!
//! **Per-module (MVL-specific)**
//! - Fan-out: number of distinct imported modules (`use` declarations).
//! - Trait impl count per type: `impl Trait for Type` blocks per type name.
//! - Trait fan-out per trait: number of types implementing each trait.
//! - Extern function count / total function count → extern ratio.

use crate::mvl::parser::ast::{Block, Decl, ElseBranch, Expr, FnDecl, MatchBody, Program, Stmt};

// ── Per-function metrics ──────────────────────────────────────────────────

/// Complexity metrics for a single function or method.
#[derive(Debug, Clone)]
pub struct FunctionMetrics {
    /// Fully-qualified name: `"fn_name"` or `"TraitName::MethodName for TypeName"`.
    pub name: String,
    /// Source line where the function/method is declared.
    pub line: u32,
    /// Cyclomatic complexity (minimum 1).
    pub cyclomatic_complexity: usize,
    /// Number of top-level statements in the function body.
    pub lines: usize,
    /// Maximum nesting depth of `match` expressions.
    pub match_depth: usize,
    /// Number of effects declared on the function signature.
    pub effect_width: usize,
}

/// Complexity metrics for an entire module/file.
#[derive(Debug, Clone)]
pub struct ModuleMetrics {
    /// Number of distinct imported modules (`use std.X.*` or `use module::*`).
    pub fan_out: usize,
    /// Number of `impl Trait for T` blocks per type name.
    pub trait_impl_count: std::collections::HashMap<String, usize>,
    /// Number of types implementing each trait name.
    pub trait_fan_out: std::collections::HashMap<String, usize>,
    /// Number of extern function declarations.
    pub extern_fn_count: usize,
    /// Total non-extern function declarations (top-level + method).
    pub total_fn_count: usize,
}

impl ModuleMetrics {
    /// `extern_fn_count / (extern_fn_count + total_fn_count)`, or 0.0 if no functions.
    pub fn extern_ratio(&self) -> f64 {
        let denom = self.extern_fn_count + self.total_fn_count;
        if denom == 0 {
            0.0
        } else {
            self.extern_fn_count as f64 / denom as f64
        }
    }
}

/// Full complexity report for a single file.
#[derive(Debug, Clone)]
pub struct ComplexityReport {
    pub file: String,
    pub functions: Vec<FunctionMetrics>,
    pub module: ModuleMetrics,
}

// ── Public entry point ────────────────────────────────────────────────────

/// Compute the full complexity report for one program file.
pub fn analyze(file: &str, prog: &Program) -> ComplexityReport {
    let mut functions = Vec::new();
    let mut fan_out_modules = std::collections::HashSet::new();
    let mut trait_impl_count: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut trait_fan_out: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut extern_fn_count = 0usize;
    let mut total_fn_count = 0usize;

    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                functions.push(fn_metrics(f, &f.name));
                total_fn_count += 1;
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let qual = format!(
                        "{}::{} for {}",
                        impl_decl.trait_name, method.name, impl_decl.type_name
                    );
                    functions.push(fn_metrics(method, &qual));
                    total_fn_count += 1;
                }
                *trait_impl_count
                    .entry(impl_decl.type_name.clone())
                    .or_insert(0) += 1;
                *trait_fan_out
                    .entry(impl_decl.trait_name.clone())
                    .or_insert(0) += 1;
            }
            Decl::Extern(ext) => {
                extern_fn_count += ext.fns.len();
            }
            Decl::Use(u) => {
                // Fan-out: count unique module prefixes (first two path segments).
                let module_key = u
                    .path
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("::");
                fan_out_modules.insert(module_key);
            }
            Decl::Type(_) | Decl::Const(_) => {}
        }
    }

    ComplexityReport {
        file: file.to_string(),
        functions,
        module: ModuleMetrics {
            fan_out: fan_out_modules.len(),
            trait_impl_count,
            trait_fan_out,
            extern_fn_count,
            total_fn_count,
        },
    }
}

// ── Per-function helpers ──────────────────────────────────────────────────

fn fn_metrics(f: &FnDecl, qualified_name: &str) -> FunctionMetrics {
    FunctionMetrics {
        name: qualified_name.to_string(),
        line: f.span.line,
        cyclomatic_complexity: cyclomatic_complexity_block(&f.body),
        lines: f.body.stmts.len(),
        match_depth: match_depth_block(&f.body),
        effect_width: f.effects.len(),
    }
}

// ── Cyclomatic complexity ─────────────────────────────────────────────────

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
            let mut cc = 1;
            cc += cyclomatic_complexity_expr(cond);
            cc += cyclomatic_complexity_block_inner(then);
            match else_ {
                Some(ElseBranch::Block(b)) => cc += cyclomatic_complexity_block_inner(b),
                Some(ElseBranch::If(inner)) => cc += cyclomatic_complexity_stmt(inner),
                None => {}
            }
            cc
        }
        Stmt::Match {
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
        Stmt::While { cond, body, .. } => {
            1 + cyclomatic_complexity_expr(cond) + cyclomatic_complexity_block_inner(body)
        }
        Stmt::For { iter, body, .. } => {
            1 + cyclomatic_complexity_expr(iter) + cyclomatic_complexity_block_inner(body)
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

fn cyclomatic_complexity_block_inner(block: &Block) -> usize {
    block.stmts.iter().map(cyclomatic_complexity_stmt).sum()
}

fn cyclomatic_complexity_expr(expr: &Expr) -> usize {
    use crate::mvl::parser::ast::BinaryOp;
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
        Expr::Unary { expr, .. } => cyclomatic_complexity_expr(expr),
        Expr::FnCall { args, .. } => args.iter().map(cyclomatic_complexity_expr).sum(),
        Expr::MethodCall { receiver, args, .. } => {
            cyclomatic_complexity_expr(receiver)
                + args.iter().map(cyclomatic_complexity_expr).sum::<usize>()
        }
        Expr::Block(b) => cyclomatic_complexity_block_inner(b),
        Expr::If {
            cond, then, else_, ..
        } => {
            1 + cyclomatic_complexity_expr(cond)
                + cyclomatic_complexity_block_inner(then)
                + else_
                    .as_ref()
                    .map(|e| cyclomatic_complexity_expr(e))
                    .unwrap_or(0)
        }
        Expr::Lambda { body, .. } => cyclomatic_complexity_expr(body),
        _ => 0,
    }
}

// ── Match nesting depth ───────────────────────────────────────────────────

fn match_depth_block(block: &Block) -> usize {
    block.stmts.iter().map(match_depth_stmt).max().unwrap_or(0)
}

fn match_depth_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Match { arms, .. } => {
            let inner = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Block(b) => match_depth_block(b),
                    MatchBody::Expr(_) => 0,
                })
                .max()
                .unwrap_or(0);
            1 + inner
        }
        Stmt::If { then, else_, .. } => {
            let then_depth = match_depth_block(then);
            let else_depth = match else_ {
                Some(ElseBranch::Block(b)) => match_depth_block(b),
                Some(ElseBranch::If(s)) => match_depth_stmt(s),
                None => 0,
            };
            then_depth.max(else_depth)
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => match_depth_block(body),
        _ => 0,
    }
}

// ── Human-readable output ─────────────────────────────────────────────────

/// Print a human-readable complexity report to stdout.
pub fn print_human(report: &ComplexityReport) {
    println!("{}", report.file);
    println!(
        "  module: fan-out={} extern-ratio={:.0}% trait-impls={} extern-fns={}",
        report.module.fan_out,
        report.module.extern_ratio() * 100.0,
        report.module.trait_impl_count.len(),
        report.module.extern_fn_count,
    );
    if !report.module.trait_fan_out.is_empty() {
        let mut traits: Vec<_> = report.module.trait_fan_out.iter().collect();
        traits.sort_by_key(|(t, _)| t.as_str());
        let summary: Vec<String> = traits.iter().map(|(t, n)| format!("{t}×{n}")).collect();
        println!("  traits: {}", summary.join(", "));
    }
    for f in &report.functions {
        println!(
            "  {}:{} cc={} lines={} match-depth={} effects={}",
            f.name, f.line, f.cyclomatic_complexity, f.lines, f.match_depth, f.effect_width
        );
    }
}

/// Emit a JSON complexity report (array of file objects) to stdout.
pub fn print_json(reports: &[ComplexityReport]) {
    println!("[");
    for (ri, report) in reports.iter().enumerate() {
        println!("  {{");
        println!("    \"file\": \"{}\",", json_escape(&report.file));
        println!("    \"functions\": [");
        for (fi, f) in report.functions.iter().enumerate() {
            let comma = if fi + 1 < report.functions.len() {
                ","
            } else {
                ""
            };
            println!("      {{");
            println!("        \"name\": \"{}\",", json_escape(&f.name));
            println!("        \"line\": {},", f.line);
            println!(
                "        \"cyclomatic_complexity\": {},",
                f.cyclomatic_complexity
            );
            println!("        \"lines\": {},", f.lines);
            println!("        \"match_depth\": {},", f.match_depth);
            println!("        \"effect_width\": {}", f.effect_width);
            println!("      }}{comma}");
        }
        println!("    ],");
        println!("    \"module\": {{");
        println!("      \"fan_out\": {},", report.module.fan_out);
        println!(
            "      \"extern_ratio\": {:.4},",
            report.module.extern_ratio()
        );
        println!(
            "      \"extern_fn_count\": {},",
            report.module.extern_fn_count
        );
        println!("      \"total_fn_count\": {}", report.module.total_fn_count);
        println!("    }}");
        let comma = if ri + 1 < reports.len() { "," } else { "" };
        println!("  }}{comma}");
    }
    println!("]");
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
