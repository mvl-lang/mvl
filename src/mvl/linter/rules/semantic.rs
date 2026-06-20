// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase-2 semantic rules — traverse the AST to detect code smells.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, Literal, MatchArm, MatchBody, Pattern, Program, Stmt, TypeBody,
    TypeExpr, VariantFields,
};
use std::collections::{HashMap, HashSet};
use crate::mvl::parser::visit::{walk_block, walk_expr, walk_stmt, Visit};

/// Flag statements that are unreachable because they follow a `return` in the
/// same block.
///
/// Rule id: `unreachable-code`
///
/// Only the **first** unreachable statement in each block is flagged to avoid
/// noise from cascading violations.
pub fn unreachable_code(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.unreachable_code {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_unreachable(&f.body, out);
        }
    }
}

fn check_block_unreachable(block: &Block, out: &mut Vec<LintDiag>) {
    let mut returned = false;
    for stmt in &block.stmts {
        if returned {
            let s = stmt.span();
            out.push(LintDiag::warning(
                "unreachable-code",
                "unreachable statement after `return`",
                s.line,
                s.col,
            ));
            // Only flag the first unreachable statement per block.
            break;
        }
        match stmt {
            Stmt::Return { .. } => {
                returned = true;
            }
            Stmt::If {
                then, else_: None, ..
            } => {
                check_block_unreachable(then, out);
            }
            Stmt::If {
                then,
                else_: Some(ElseBranch::Block(eb)),
                ..
            } => {
                check_block_unreachable(then, out);
                check_block_unreachable(eb, out);
            }
            Stmt::If {
                then,
                else_: Some(ElseBranch::If(inner)),
                ..
            } => {
                check_block_unreachable(then, out);
                check_block_unreachable(
                    // The inner `if` is a Stmt — wrap it in a synthetic block
                    // by recursing into its then-branch directly.
                    &Block {
                        stmts: vec![*inner.clone()],
                        span: inner.span(),
                    },
                    out,
                );
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_unreachable(b, out);
                    }
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_unreachable(body, out);
            }
            Stmt::Expr {
                expr: Expr::Block(b),
                ..
            } => {
                check_block_unreachable(b, out);
            }
            _ => {}
        }
    }
}

/// Flag `match` expressions/statements with exactly one irrefutable arm.
///
/// Rule id: `redundant-match`
///
/// A single-arm `match` where the pattern is a wildcard (`_`) or a plain
/// binding (`x`) will always match — the `match` is then equivalent to a
/// `let` binding or a bare expression.  This is a common code-smell when
/// developers use `match` for destructuring a non-enum type.
pub fn redundant_match(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_match {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_redundant_match(&f.body, out);
        }
    }
}

fn check_block_redundant_match(block: &Block, out: &mut Vec<LintDiag>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                emit_if_redundant_match(scrutinee, arms, span.line, span.col, out);
                // Recurse into arm bodies
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_redundant_match(b, out);
                    }
                }
            }
            Stmt::Expr {
                expr:
                    Expr::Match {
                        scrutinee,
                        arms,
                        span,
                    },
                ..
            } => {
                emit_if_redundant_match(scrutinee, arms, span.line, span.col, out);
            }
            Stmt::If { then, else_, .. } => {
                check_block_redundant_match(then, out);
                match else_ {
                    Some(ElseBranch::Block(eb)) => check_block_redundant_match(eb, out),
                    Some(ElseBranch::If(inner)) => check_block_redundant_match(
                        &Block {
                            stmts: vec![*inner.clone()],
                            span: inner.span(),
                        },
                        out,
                    ),
                    None => {}
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_redundant_match(body, out);
            }
            _ => {}
        }
    }
}

/// Emit a `redundant-match` diagnostic if `arms` has a single irrefutable arm.
fn emit_if_redundant_match(
    scrutinee: &Expr,
    arms: &[MatchArm],
    line: u32,
    col: u32,
    out: &mut Vec<LintDiag>,
) {
    if arms.len() == 1 && is_irrefutable(&arms[0].pattern) {
        out.push(LintDiag::warning(
            "redundant-match",
            format!(
                "single-arm `match` with irrefutable pattern — use `let` instead: \
                 `let {} = {}`",
                pattern_display(&arms[0].pattern),
                expr_display(scrutinee),
            ),
            line,
            col,
        ));
    }
}

/// A pattern is irrefutable if it always matches: wildcard (`_`) or plain binding.
fn is_irrefutable(pat: &Pattern) -> bool {
    matches!(pat, Pattern::Wildcard(_) | Pattern::Ident(..))
}

fn pattern_display(pat: &Pattern) -> &str {
    match pat {
        Pattern::Wildcard(_) => "_",
        Pattern::Ident(name, _) => name.as_str(),
        _ => "_",
    }
}

fn expr_display(expr: &Expr) -> &str {
    match expr {
        Expr::Ident(name, _) => name.as_str(),
        _ => "<expr>",
    }
}

/// Flag functions that declare effects but whose body contains no function
/// or method calls at all.
///
/// Rule id: `redundant-effects`
///
/// If a function body is entirely free of calls, no effect can ever be
/// triggered — any declared effects are therefore dead annotations.
///
/// Note: this is a conservative check.  A function that only calls local
/// pure helpers is not flagged even if those helpers are call-free.
pub fn redundant_effects(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_effects {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if f.effects.is_empty() {
                continue;
            }
            if !block_has_calls(&f.body) {
                out.push(LintDiag::warning(
                    "redundant-effects",
                    format!(
                        "function `{}` declares effect(s) [{}] but contains no calls — \
                         remove the effect declaration",
                        f.name,
                        f.effects
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Warn when a function has calls in its body but no declared effects.
///
/// Rule id: `missing-annotation`
///
/// This is the inverse of `redundant-effects`: where that rule flags declared
/// effects with no calls, this rule flags calls with no declared effects.
/// The rule is **opt-in** (disabled by default) because the linter cannot
/// distinguish calls to pure MVL helpers from calls to effectful stdlib
/// functions without a full symbol table.  Enable with
/// `missing_annotations = true` in `.mvllintrc` for code that should
/// be explicit-everywhere.
///
/// `test fn` declarations are excluded — test bodies call the system under
/// test and do not need to declare effects themselves.
///
/// See: Spec 011 Req 4, ADR-0017 amendment.
pub fn missing_annotations(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.missing_annotations {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if f.is_test || !f.effects.is_empty() {
                continue;
            }
            if block_has_calls(&f.body) {
                out.push(LintDiag::warning(
                    "missing-annotation",
                    format!(
                        "function `{}` has calls but no effect declaration \
                         — add `! Effects` or verify all calls are pure",
                        f.name,
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Warn on unannotated `pub fn` — every public function must explicitly declare
/// `total` or `partial` to make the termination contract visible.
///
/// Rule id: `missing-totality`
///
/// On by default (`require_explicit_totality = true`).  The rule checks only
/// `pub fn` declarations; private helpers, test functions, and built-in
/// functions are excluded.  The diagnostic includes a suggestion based on
/// body analysis: `partial` when the body contains a `while` loop without a
/// `decreases` variant or a direct recursive self-call; `total` otherwise.
pub fn missing_totality(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.require_explicit_totality {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            if !f.visible || f.is_test || f.is_builtin || f.totality.is_some() {
                continue;
            }
            let suggestion =
                if block_has_while_no_decreases(&f.body) || block_calls_fn(&f.body, &f.name) {
                    "partial"
                } else {
                    "total"
                };
            out.push(LintDiag::warning(
                "missing-totality",
                format!(
                    "pub fn `{}` has no explicit `total` or `partial` keyword \
                     — add `{}` to document the termination contract",
                    f.name, suggestion,
                ),
                f.span.line,
                f.span.col,
            ));
        }
    }
}

/// Return `true` if `block` (or any nested block/expression) contains a
/// `while` loop that has no `decreases` variant.
fn block_has_while_no_decreases(block: &Block) -> bool {
    let mut v = WhileNoDecreases { found: false };
    walk_block(&mut v, block);
    v.found
}

struct WhileNoDecreases {
    found: bool,
}

impl<'ast> Visit<'ast> for WhileNoDecreases {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if self.found {
            return;
        }
        match s {
            Stmt::While { decreases, .. } if decreases.is_none() => self.found = true,
            _ => walk_stmt(self, s),
        }
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        if self.found {
            return;
        }
        walk_expr(self, e);
    }
}

/// Return `true` if `block` (or any nested block/expression) contains a
/// direct call to the function named `name` (used for recursion detection).
fn block_calls_fn(block: &Block, name: &str) -> bool {
    let mut v = CallsFn { name, found: false };
    walk_block(&mut v, block);
    v.found
}

struct CallsFn<'n> {
    name: &'n str,
    found: bool,
}

impl<'ast> Visit<'ast> for CallsFn<'_> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if !self.found {
            walk_stmt(self, s);
        }
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        if self.found {
            return;
        }
        if let Expr::FnCall { name, .. } = e {
            if *name == self.name {
                self.found = true;
                return;
            }
        }
        walk_expr(self, e);
    }
}

/// Return `true` if `block` (or any nested block/expression) contains at
/// least one function or method call.
fn block_has_calls(block: &Block) -> bool {
    let mut v = HasCalls { found: false };
    walk_block(&mut v, block);
    v.found
}

struct HasCalls {
    found: bool,
}

impl<'ast> Visit<'ast> for HasCalls {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        if !self.found {
            walk_stmt(self, s);
        }
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        if self.found {
            return;
        }
        match e {
            Expr::FnCall { .. } | Expr::MethodCall { .. } => self.found = true,
            _ => walk_expr(self, e),
        }
    }
}

/// Flag `Public<T>` type annotations.
///
/// Rule id: `redundant-ifc-label`
///
/// `Public` is the base level of the MVL IFC lattice — all un-annotated types
/// are implicitly public.  Writing `Public<T>` is therefore always redundant
/// and should be simplified to `T`.
pub fn redundant_ifc_labels(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.redundant_ifc_labels {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                for param in &f.params {
                    check_type_expr_ifc(&param.ty, out);
                }
                check_type_expr_ifc(&f.return_type, out);
            }
            Decl::Const(c) => check_type_expr_ifc(&c.ty, out),
            Decl::Type(t) => match &t.body {
                TypeBody::Struct { fields, .. } => {
                    for field in fields {
                        check_type_expr_ifc(&field.ty, out);
                    }
                }
                TypeBody::Enum(variants) => {
                    for variant in variants {
                        match &variant.fields {
                            VariantFields::Struct(fields) => {
                                for field in fields {
                                    check_type_expr_ifc(&field.ty, out);
                                }
                            }
                            VariantFields::Tuple(types) => {
                                for ty in types {
                                    check_type_expr_ifc(ty, out);
                                }
                            }
                            VariantFields::Unit => {}
                        }
                    }
                }
                TypeBody::Alias(ty) => check_type_expr_ifc(ty, out),
            },
            _ => {}
        }
    }
}

#[allow(clippy::only_used_in_recursion)]
fn check_type_expr_ifc(ty: &TypeExpr, out: &mut Vec<LintDiag>) {
    match ty {
        TypeExpr::Labeled { inner, .. } => check_type_expr_ifc(inner, out),
        TypeExpr::Option { inner, .. }
        | TypeExpr::Ref { inner, .. }
        | TypeExpr::Refined { inner, .. } => check_type_expr_ifc(inner, out),
        TypeExpr::Result { ok, err, .. } => {
            check_type_expr_ifc(ok, out);
            check_type_expr_ifc(err, out);
        }
        TypeExpr::Base { args, .. } => {
            for arg in args {
                check_type_expr_ifc(arg, out);
            }
        }
        TypeExpr::Fn { params, ret, .. } => {
            for p in params {
                check_type_expr_ifc(p, out);
            }
            check_type_expr_ifc(ret, out);
        }
        TypeExpr::IntConst { .. } => {}
        // Session types have no IFC label — no check needed.
        TypeExpr::Session { .. } => {}
    }
}

#[allow(dead_code, clippy::only_used_in_recursion)]
fn type_expr_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Base { name, args, .. } if args.is_empty() => name.clone(),
        TypeExpr::Base { name, args, .. } => {
            format!(
                "{}<{}>",
                name,
                args.iter()
                    .map(type_expr_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        TypeExpr::Option { inner, .. } => format!("Option<{}>", type_expr_name(inner)),
        TypeExpr::Result { ok, err, .. } => {
            format!("Result<{}, {}>", type_expr_name(ok), type_expr_name(err))
        }
        TypeExpr::Labeled { label, inner, .. } => {
            format!("{}<{}>", label, type_expr_name(inner))
        }
        _ => "<type>".to_string(),
    }
}

/// Warn on `extern "rust"` blocks — deprecated in favour of `extern "C"` (#561).
///
/// Rule id: `deprecated-extern-rust`
///
/// `extern "rust"` only works with the Rust transpiler backend. Use `extern "C"` with
/// `#[no_mangle] pub extern "C"` on the Rust side for backend-symmetric FFI.
pub fn deprecated_extern_rust(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.deprecated_extern_rust {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Extern(ed) = decl {
            if ed.abi == "rust" {
                let s = ed.span;
                out.push(LintDiag::warning(
                    "deprecated-extern-rust",
                    "extern \"rust\" is deprecated; use extern \"C\" with #[no_mangle] pub extern \"C\" on the Rust implementation",
                    s.line,
                    s.col,
                ));
            }
        }
    }
}

/// Flag non-`pub`, non-`main` functions that are never called within the program.
///
/// Rule id: `unused-function`
///
/// A function that is never invoked is dead code. This rule performs a whole-file
/// call-set diff: declared names minus called names. `pub fn`, `fn main`, test
/// functions, builtins, and extension methods are excluded.
pub fn unused_functions(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.unused_functions {
        return;
    }

    // Collect candidate declared functions (non-pub, non-main, non-test, non-builtin,
    // non-extension-method).
    let candidates: Vec<(&str, u32, u32)> = prog
        .declarations
        .iter()
        .filter_map(|decl| {
            if let Decl::Fn(f) = decl {
                if !f.visible
                    && !f.is_test
                    && !f.is_builtin
                    && f.receiver_type.is_none()
                    && f.name != "main"
                {
                    return Some((f.name.as_str(), f.span.line, f.span.col));
                }
            }
            None
        })
        .collect();

    if candidates.is_empty() {
        return;
    }

    // Collect all called function and method names across all declarations.
    let mut called: HashSet<String> = HashSet::new();
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            let mut v = CollectCalls { called: &mut called };
            walk_block(&mut v, &f.body);
        }
    }

    // Flag any candidate not in the call set.
    for (name, line, col) in candidates {
        if !called.contains(name) {
            out.push(LintDiag::warning(
                "unused-function",
                format!("function `{name}` is never called — remove it or make it `pub`"),
                line,
                col,
            ));
        }
    }
}

struct CollectCalls<'a> {
    called: &'a mut HashSet<String>,
}

impl<'ast> Visit<'ast> for CollectCalls<'_> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::FnCall { name, .. } => {
                self.called.insert(name.clone());
            }
            Expr::MethodCall { method, .. } => {
                self.called.insert(method.clone());
            }
            _ => {}
        }
        walk_expr(self, e);
    }
}

/// Flag `Result` values that are silently discarded without inspecting the `Err` variant.
///
/// Rule id: `silent-result-discard`
///
/// Detects five patterns:
/// 1. `let _: Result[_, _] = expr;` — wildcard binding with explicit Result type annotation.
/// 2. `fn_returning_result();` — statement-position call where the declared return type is
///    `Result` (limited to functions declared in the same program).
/// 3. `.unwrap_or(...)`, `.unwrap_or_else(...)`, `.unwrap_or_default()` on a call expression
///    whose declared return type is `Result`.
/// 4. `if let Ok(v) = res { ... }` with no else — the `Err` branch silently no-ops.
/// 5. `.ok()` on a call expression whose declared return type is `Result`.
///
/// Per-site suppression: `// allow: silent-result-discard <reason>` on the preceding line.
pub fn silent_result_discard(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.silent_result_discard {
        return;
    }

    // Build a map from function name → return TypeExpr.
    let ret_types: HashMap<&str, &TypeExpr> = prog
        .declarations
        .iter()
        .filter_map(|d| {
            if let Decl::Fn(f) = d {
                Some((f.name.as_str(), f.return_type.as_ref()))
            } else {
                None
            }
        })
        .collect();

    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_result_discard(&f.body, &ret_types, out);
        }
    }
}

fn check_block_result_discard<'a>(
    block: &'a Block,
    ret_types: &HashMap<&str, &'a TypeExpr>,
    out: &mut Vec<LintDiag>,
) {
    for stmt in &block.stmts {
        match stmt {
            // Pattern 1: let _: Result[_, _] = expr;
            Stmt::Let {
                pattern: Pattern::Wildcard(_),
                ty,
                span,
                ..
            } if is_result_type(ty) => {
                out.push(LintDiag::warning(
                    "silent-result-discard",
                    "Result value bound to `_` — handle the `Err` variant or add an allow comment",
                    span.line,
                    span.col,
                ));
            }
            // Pattern 2: statement-position call that returns Result (trailing semicolon discards)
            Stmt::Expr { expr, span } => {
                if let Some(msg) = detect_result_discard_expr(expr, ret_types) {
                    out.push(LintDiag::warning("silent-result-discard", msg, span.line, span.col));
                }
                // Recurse into nested blocks (e.g. block expressions)
                if let Expr::Block(b) = expr {
                    check_block_result_discard(b, ret_types, out);
                }
            }
            // Pattern 4: if let Ok(v) = res { ... } with no else
            // Desugared by the parser into a 2-arm match where the second arm has a unit body.
            Stmt::Match { scrutinee: _, arms, span } => {
                if is_if_let_ok_no_else(arms) {
                    out.push(LintDiag::warning(
                        "silent-result-discard",
                        "`if let Ok(…)` with no `else` silently ignores the `Err` variant",
                        span.line,
                        span.col,
                    ));
                }
                // Recurse into arm bodies
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_result_discard(b, ret_types, out);
                    }
                }
            }
            Stmt::If { then, else_, .. } => {
                check_block_result_discard(then, ret_types, out);
                match else_ {
                    Some(ElseBranch::Block(eb)) => {
                        check_block_result_discard(eb, ret_types, out);
                    }
                    Some(ElseBranch::If(inner)) => {
                        check_block_result_discard(
                            &Block {
                                stmts: vec![*inner.clone()],
                                span: inner.span(),
                            },
                            ret_types,
                            out,
                        );
                    }
                    None => {}
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_result_discard(body, ret_types, out);
            }
            _ => {}
        }
    }
}

/// Detect patterns 2, 3, 5 in an expression used in statement position or as a method receiver.
fn detect_result_discard_expr<'a>(
    expr: &'a Expr,
    ret_types: &HashMap<&str, &'a TypeExpr>,
) -> Option<String> {
    match expr {
        // Pattern 2: bare fn call at statement level that returns Result
        Expr::FnCall { name, .. } if call_returns_result(name, ret_types) => Some(format!(
            "return value of `{name}()` (which returns `Result`) is silently discarded"
        )),
        // Pattern 3 & 5: method call on a result-returning expression
        Expr::MethodCall { receiver, method, .. } => {
            match method.as_str() {
                "unwrap_or" | "unwrap_or_else" | "unwrap_or_default" => {
                    if receiver_is_result(receiver, ret_types) {
                        return Some(format!(
                            "`.{method}()` silently replaces the `Err` variant — handle the error explicitly"
                        ));
                    }
                }
                "ok" => {
                    if receiver_is_result(receiver, ret_types) {
                        return Some(
                            "`.ok()` discards the `Err` variant by converting `Result` to `Option`"
                                .into(),
                        );
                    }
                }
                _ => {}
            }
            None
        }
        _ => None,
    }
}

/// True if the expression is a call to a function declared with `Result` return type.
fn receiver_is_result<'a>(expr: &'a Expr, ret_types: &HashMap<&str, &'a TypeExpr>) -> bool {
    match expr {
        Expr::FnCall { name, .. } => call_returns_result(name, ret_types),
        _ => false,
    }
}

fn call_returns_result<'a>(name: &str, ret_types: &HashMap<&str, &'a TypeExpr>) -> bool {
    ret_types.get(name).map_or(false, |ty| is_result_type(ty))
}

fn is_result_type(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Result { .. })
}

/// True if `arms` represents an `if let Ok(…) = expr { … }` with no else clause.
///
/// The parser desugars this into a 2-arm match: first arm carries the `Ok(…)` pattern,
/// second arm is a wildcard that evaluates to `()`.
fn is_if_let_ok_no_else(arms: &[MatchArm]) -> bool {
    if arms.len() != 2 {
        return false;
    }
    let first_is_ok = matches!(&arms[0].pattern, Pattern::Ok { .. });
    let second_is_wildcard_unit = matches!(&arms[1].pattern, Pattern::Wildcard(_))
        && matches!(&arms[1].body, MatchBody::Expr(Expr::Literal(Literal::Unit, _)));
    first_is_ok && second_is_wildcard_unit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn cfg() -> LintConfig {
        let mut c = LintConfig::default();
        c.line_length = 120;
        c.trailing_ws = true;
        c.indentation = true;
        c.final_newline = true;
        c.consistent_comment_style = true;
        c
    }

    fn parse(src: &str) -> crate::mvl::parser::ast::Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // -- unreachable_code --

    #[test]
    fn unreachable_after_return_detected() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1, "expected 1 unreachable diagnostic");
        assert_eq!(diags[0].rule, "unreachable-code");
    }

    #[test]
    fn unreachable_code_disabled() {
        let src = "fn f() -> Int { return 1; let x: Int = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.unreachable_code = false;
        unreachable_code(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn reachable_code_clean() {
        let src = "fn f() -> Int { let x: Int = 1; return x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match --

    #[test]
    fn single_arm_wildcard_detected() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn single_arm_binding_detected() {
        let src = "fn f(x: Int) -> Int { match x { v => v } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-match");
    }

    #[test]
    fn multi_arm_match_clean() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { match c { Color::Red => 1 Color::Green => 2 Color::Blue => 3 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_match(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects --

    #[test]
    fn effects_on_call_free_fn_detected() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-effects");
        assert!(diags[0].message.contains("Console"));
    }

    #[test]
    fn effects_on_fn_with_call_clean() {
        let src = "fn f() -> Unit ! Console {\n    println(\"hi\")\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn no_effects_declared_clean() {
        let src = "fn f() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_effects(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_ifc_labels --

    #[test]
    fn public_label_on_param_no_longer_detected(/* #894: Public is not a label anymore */) {
        // Post-#894: `Public` is a plain identifier, not a label keyword.
        // `Public[Int]` parses as a generic base type, not TypeExpr::Labeled.
        // The redundant-ifc-label rule no longer fires for it.
        let src = "fn f(x: Public[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty(), "Public is no longer a label (#894)");
    }

    #[test]
    fn secret_label_on_param_clean() {
        let src = "fn f(x: Secret[Int]) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_return_type_no_longer_detected() {
        // Post-#894: `Public[String]` is just a generic type — no lint fires.
        let src = "fn f() -> Public[String] { \"hi\" }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn redundant_ifc_disabled() {
        let src = "fn f(x: Tainted[Int]) -> Int { relabel trust(x, \"V-01\") }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_ifc_labels = false;
        redundant_ifc_labels(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_struct_field_no_longer_detected() {
        // Post-#894: `Public[Int]` in a struct is a generic type, not a labeled type.
        let src = "type Wrapper = struct { data: Public[Int] }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_in_type_alias_no_longer_detected() {
        // Post-#894: `Public[Int]` as type alias — not a labeled type.
        let src = "type MyInt = Public[Int]\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_match: config-disable test --

    #[test]
    fn redundant_match_disabled() {
        let src = "fn f(x: Int) -> Int { match x { _ => x } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_match = false;
        redundant_match(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects: config-disable test --

    #[test]
    fn redundant_effects_disabled() {
        let src = "fn f() -> Int ! Console { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_effects = false;
        redundant_effects(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- missing_annotations --

    fn cfg_missing_annotations_on() -> LintConfig {
        LintConfig {
            missing_annotations: true,
            ..LintConfig::default()
        }
    }

    #[test]
    fn missing_annotations_fires_on_call_without_effects() {
        // fn has a call but no declared effects — must warn when rule is enabled
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-annotation" && d.message.contains("foo")),
            "expected missing-annotation for `foo`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_off_by_default() {
        // default config has missing_annotations = false — rule must be silent
        let src = "fn foo() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "missing-annotation must not fire with default config; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_effects_declared() {
        // fn has calls AND declared effects — must not warn
        let src = "fn foo() -> Unit ! Console {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn when effects are declared; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_callless_fn() {
        // pure arithmetic fn — no calls, no effects — must not warn
        let src = "fn add(x: Int, y: Int) -> Int {\n    x + y\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "must not warn on call-free function; got: {diags:?}"
        );
    }

    #[test]
    fn missing_annotations_no_fire_on_test_fn() {
        // test fn is excluded from the rule
        let src = "test fn check_add() -> Unit {\n    bar()\n}\nfn bar() -> Unit { 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_annotations(&prog, &cfg_missing_annotations_on(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-annotation"),
            "test fn must be excluded; got: {diags:?}"
        );
    }

    // -- missing_totality --

    #[test]
    fn missing_totality_fires_on_unannotated_pub_fn() {
        // unannotated pub fn must warn with default config (rule is ON by default)
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("add")),
            "expected missing-totality for `add`; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_on_private_fn() {
        // private fn must not warn even when unannotated
        let src = "fn helper(x: Int) -> Int { x + 1 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "private fn must not trigger missing-totality; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_silent_when_annotated() {
        // explicit `total` suppresses the warning
        let src = "pub total fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "annotated pub fn must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_total_for_simple_fn() {
        // no while, no recursion → suggest `total`
        let src = "pub fn double(x: Int) -> Int { x * 2 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`total`")),
            "should suggest `total` for simple fn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_while_no_decreases() {
        // while without decreases → suggest `partial`
        let src = concat!(
            "pub fn count(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for while without decreases; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_suggests_partial_for_recursive_fn() {
        // direct recursion → suggest `partial`
        let src = concat!(
            "pub fn factorial(n: Int) -> Int {\n",
            "    if n <= 1 { 1 } else { n * factorial(n - 1) }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "missing-totality" && d.message.contains("`partial`")),
            "should suggest `partial` for recursive fn; got: {diags:?}"
        );
    }

    #[test]
    fn missing_totality_off_when_disabled() {
        // rule can be opted out via config
        let cfg_off = LintConfig {
            require_explicit_totality: false,
            ..LintConfig::default()
        };
        let src = "pub fn add(x: Int, y: Int) -> Int { x + y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        missing_totality(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "missing-totality"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    // -- deprecated_extern_rust --

    #[test]
    fn deprecated_extern_rust_fires_on_extern_rust() {
        let src = "extern \"rust\" {\n    fn hash(data: String) -> String;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "deprecated-extern-rust"),
            "extern \"rust\" should trigger deprecated-extern-rust; got: {diags:?}"
        );
    }

    #[test]
    fn deprecated_extern_rust_silent_for_extern_c() {
        let src = "extern \"c\" {\n    fn sqrt(x: Float) -> Float;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "deprecated-extern-rust"),
            "extern \"c\" must not trigger the rule; got: {diags:?}"
        );
    }

    #[test]
    fn deprecated_extern_rust_off_when_disabled() {
        let cfg_off = LintConfig {
            deprecated_extern_rust: false,
            ..LintConfig::default()
        };
        let src = "extern \"rust\" {\n    fn hash(data: String) -> String;\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        deprecated_extern_rust(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "deprecated-extern-rust"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }

    // -- unused_functions --

    #[test]
    fn unused_fn_detected() {
        let src = "fn dead() -> Int { 0 }\nfn main() -> Unit { }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unused_functions(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "unused-function" && d.message.contains("dead")),
            "expected unused-function for `dead`; got: {diags:?}"
        );
    }

    #[test]
    fn called_fn_not_flagged() {
        let src = "fn helper() -> Int { 0 }\nfn main() -> Unit { let x: Int = helper(); }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unused_functions(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "unused-function"),
            "called fn must not be flagged; got: {diags:?}"
        );
    }

    #[test]
    fn pub_fn_not_flagged_even_if_uncalled() {
        let src = "pub fn api() -> Int { 0 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unused_functions(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "unused-function"),
            "pub fn must never be flagged; got: {diags:?}"
        );
    }

    #[test]
    fn main_fn_not_flagged() {
        let src = "fn main() -> Unit { }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unused_functions(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "unused-function"),
            "fn main must not be flagged; got: {diags:?}"
        );
    }

    #[test]
    fn unused_functions_disabled() {
        let src = "fn dead() -> Int { 0 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let cfg_off = LintConfig {
            unused_functions: false,
            ..LintConfig::default()
        };
        unused_functions(&prog, &cfg_off, &mut diags);
        assert!(diags.is_empty(), "rule must be silent when disabled");
    }

    // -- silent_result_discard --

    #[test]
    fn pattern1_let_wildcard_result_detected() {
        let src = concat!(
            "fn try_parse(s: String) -> Result[Int, String] { Ok(0) }\n",
            "fn main() -> Unit {\n",
            "    let _: Result[Int, String] = try_parse(\"x\");\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        silent_result_discard(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "silent-result-discard"),
            "expected silent-result-discard for let _ pattern; got: {diags:?}"
        );
    }

    #[test]
    fn pattern2_statement_position_result_call_detected() {
        let src = concat!(
            "fn do_thing() -> Result[Unit, String] { Ok(()) }\n",
            "fn main() -> Unit {\n",
            "    do_thing();\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        silent_result_discard(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "silent-result-discard"),
            "expected silent-result-discard for statement-position Result call; got: {diags:?}"
        );
    }

    #[test]
    fn pattern4_if_let_ok_no_else_detected() {
        let src = concat!(
            "fn try_parse(s: String) -> Result[Int, String] { Ok(0) }\n",
            "fn main() -> Unit {\n",
            "    if let Ok(v) = try_parse(\"x\") {\n",
            "        let _: Int = v;\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        silent_result_discard(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "silent-result-discard"),
            "expected silent-result-discard for if let Ok with no else; got: {diags:?}"
        );
    }

    #[test]
    fn if_let_ok_with_else_clean() {
        let src = concat!(
            "fn try_parse(s: String) -> Result[Int, String] { Ok(0) }\n",
            "fn main() -> Unit {\n",
            "    if let Ok(v) = try_parse(\"x\") {\n",
            "        let _: Int = v;\n",
            "    } else {\n",
            "        let _: Unit = ();\n",
            "    }\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        silent_result_discard(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "silent-result-discard"),
            "if let Ok with else must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn option_unwrap_or_clean() {
        // Option::unwrap_or is explicitly excluded — must not fire
        let src = concat!(
            "fn find(xs: List[Int]) -> Option[Int] { None }\n",
            "fn main() -> Unit {\n",
            "    let x: Int = find([1, 2, 3]).unwrap_or(0);\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        silent_result_discard(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "silent-result-discard"),
            "Option::unwrap_or must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn silent_result_discard_disabled() {
        let src = concat!(
            "fn do_thing() -> Result[Unit, String] { Ok(()) }\n",
            "fn main() -> Unit {\n",
            "    do_thing();\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let cfg_off = LintConfig {
            silent_result_discard: false,
            ..LintConfig::default()
        };
        silent_result_discard(&prog, &cfg_off, &mut diags);
        assert!(diags.is_empty(), "rule must be silent when disabled");
    }
}
