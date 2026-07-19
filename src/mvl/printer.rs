// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL source formatter — produces canonical MVL output from a parsed AST.
//!
//! # Comment Preservation
//!
//! Comments are not stored in the AST (the lexer discards them). To preserve
//! them, the printer scans the raw source text for comment lines, maps them to
//! 1-indexed line numbers, and re-emits them before the declaration that follows.
//!
//! Inline trailing comments (same line as code) are not preserved — that would
//! require lexer changes to attach comments to tokens.

use std::collections::BTreeMap;

use crate::mvl::parser::ast::{
    ActorDecl, ActorMethod, ArithOp, BinaryOp, Block, Capability, CmpOp, ConstDecl, Decl,
    EffectDecl, ElseBranch, Expr, ExternDecl, ExternFnDecl, FieldDecl, FnDecl, GenericParam,
    ImplDecl, LValue, LabelDecl, LetKind, Literal, LogicOp, MatchArm, MatchBody, Pattern, Program,
    RefExpr, RelabelDecl, SessionOp, Stmt, Totality, TypeBody, TypeDecl, TypeExpr, UnaryOp,
    UseDecl, Variant, VariantFields,
};

const LINE_WIDTH: usize = 100;

// ── Comment extraction ─────────────────────────────────────────────────────

/// Return a map from 1-indexed line number → comment text for every line in
/// `source` whose non-whitespace content starts with `//`.
/// Blank lines are NOT included — the formatter inserts its own structural
/// blank lines between declarations.
///
/// MVL-port note (#1581): `BTreeMap` is used here for [`std::collections::BTreeMap::range`]
/// queries in [`Printer::flush_comments`].  MVL `Map[K, V]` is unordered AND has
/// no range query — the port should store comments as a sorted `List[(Int, String)]`
/// (insertion order is already sorted because we iterate `source.lines()`) and
/// scan with a cursor.
fn extract_comments(source: &str) -> BTreeMap<u32, String> {
    let mut map = BTreeMap::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            map.insert((i + 1) as u32, trimmed.to_string());
        }
    }
    map
}

// ── Printer ────────────────────────────────────────────────────────────────

pub struct Printer<'src> {
    out: String,
    indent: usize,
    /// MVL-port note (#1581): `BTreeMap` used for range queries — see
    /// [`extract_comments`] for the porting strategy.
    comments: BTreeMap<u32, String>,
    last_line: u32,
    source: &'src str,
}

impl<'src> Printer<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            out: String::with_capacity(source.len()),
            indent: 0,
            comments: extract_comments(source),
            last_line: 0,
            source,
        }
    }

    /// Format `prog` and return canonical MVL source.
    pub fn print(mut self, prog: &Program) -> String {
        for (i, decl) in prog.declarations.iter().enumerate() {
            let line = decl.span().line;
            // Blank separator between top-level declarations comes BEFORE the
            // comments for the next declaration, not after.
            if i > 0 {
                self.newline();
            }
            self.flush_comments(line);
            self.last_line = line;
            self.print_decl(decl);
        }
        // Trailing comments at end of file.
        self.flush_comments(u32::MAX);
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    // ── Low-level output ───────────────────────────────────────────────────

    fn ind(&self) -> String {
        "    ".repeat(self.indent)
    }

    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn line(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn newline(&mut self) {
        self.out.push('\n');
    }

    /// Emit comments whose line numbers fall in `(last_line, before_line)`.
    fn flush_comments(&mut self, before_line: u32) {
        let lines: Vec<String> = self
            .comments
            .range(self.last_line + 1..before_line)
            .map(|(_, v)| v.clone())
            .collect();
        let ind = self.ind();
        for text in lines {
            self.push(&ind);
            self.line(&text);
        }
    }

    // ── Top-level declarations ──────────────────────────────────────────────

    fn print_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Use(d) => self.print_use(d),
            Decl::Fn(d) => self.print_fn(d),
            Decl::Type(d) => self.print_type_decl(d),
            Decl::Const(d) => self.print_const(d),
            Decl::Extern(d) => self.print_extern(d),
            Decl::Impl(d) => self.print_impl(d),
            Decl::Actor(d) => self.print_actor(d),
            Decl::EffectDecl(d) => self.print_effect_decl(d),
            Decl::Label(d) => self.print_label(d),
            Decl::Relabel(d) => self.print_relabel(d),
        }
    }

    fn print_use(&mut self, d: &UseDecl) {
        let ind = self.ind();
        // The brace-group item list is discarded by the parser, so we emit the
        // raw source text to preserve it.
        let start = d.span.offset as usize;
        let end = (d.span.offset + d.span.len) as usize;
        let raw = self
            .source
            .get(start..end.min(self.source.len()))
            .unwrap_or("")
            .trim();
        self.line(&format!("{}{}", ind, raw));
    }

    fn print_fn(&mut self, d: &FnDecl) {
        let ind = self.ind();
        let sig_prefix = self.fn_sig_prefix(d);
        let params_str = self.fmt_params(&d.params);
        let single_params = format!("({})", params_str);
        let after_ret = self.fn_after_params(d);

        let contracts: Vec<String> = d
            .requires
            .iter()
            .map(|e| format!("{}    requires {}", ind, self.fmt_expr(e, self.indent)))
            .chain(
                d.ensures
                    .iter()
                    .map(|e| format!("{}    ensures {}", ind, self.fmt_expr(e, self.indent))),
            )
            .collect();

        if d.is_builtin {
            let sig = format!("{}{}{}{}", ind, sig_prefix, single_params, after_ret);
            self.line(&sig);
            return;
        }

        let body_inline = self.try_block_inline(&d.body);
        let has_contracts = !contracts.is_empty();

        // Attempt single-line format.
        if !has_contracts {
            if let Some(ref inline_body) = body_inline {
                let candidate = format!(
                    "{}{}{}{} {}",
                    ind, sig_prefix, single_params, after_ret, inline_body
                );
                if candidate.len() <= LINE_WIDTH {
                    self.line(&candidate);
                    return;
                }
            }
        }

        // Multi-line: wrap params if needed.
        let multi_params = if d.params.len() > 1
            || format!("{}{}{}{}", ind, sig_prefix, single_params, after_ret).len() > LINE_WIDTH
        {
            let plines: Vec<String> = d
                .params
                .iter()
                .map(|p| format!("{}    {},", ind, self.fmt_param(p)))
                .collect();
            format!("(\n{}\n{})", plines.join("\n"), ind)
        } else {
            single_params
        };

        let sig_line = format!("{}{}{}{}", ind, sig_prefix, multi_params, after_ret);

        if has_contracts {
            self.line(&sig_line);
            for c in &contracts {
                self.line(c);
            }
            let body = self.fmt_block_multi(&d.body, self.indent);
            self.push(&ind);
            self.line(body.trim_start());
        } else {
            let body = self.fmt_block_multi(&d.body, self.indent);
            self.push(&sig_line);
            self.push(" ");
            self.line(body.trim_start());
        }
    }

    fn fn_sig_prefix(&self, d: &FnDecl) -> String {
        let mut s = String::new();
        if d.visible {
            s.push_str("pub ");
        }
        // Order must match the parser grammar: `[test] [total|partial|builtin] fn …`
        // (see src/mvl/parser/functions.rs::parse_fn_decl). Emitting totality
        // before `test` produces an unparseable signature — the parser drops
        // the totality marker on recovery, silently turning `test partial fn`
        // into a default-total test fn and creating spurious PartialCallInTotal
        // errors on round-trip.
        if d.is_test {
            s.push_str("test ");
        }
        if let Some(tot) = &d.totality {
            match tot {
                Totality::Total => s.push_str("total "),
                Totality::Partial => s.push_str("partial "),
            }
        }
        if d.is_builtin {
            s.push_str("builtin ");
        }
        s.push_str("fn ");
        if let Some(recv) = &d.receiver_type {
            s.push_str(recv);
            s.push_str("::");
        }
        s.push_str(&d.name);
        if !d.type_params.is_empty() {
            s.push('[');
            s.push_str(&self.fmt_type_params(&d.type_params));
            s.push(']');
        }
        s
    }

    fn fn_after_params(&self, d: &FnDecl) -> String {
        let mut s = String::new();
        s.push_str(" -> ");
        s.push_str(&self.fmt_type_expr(&d.return_type));
        if let Some(ref_pred) = &d.return_refinement {
            s.push_str(" where ");
            s.push_str(&self.fmt_ref_expr(ref_pred));
        }
        if !d.effects.is_empty() {
            let eff = d
                .effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(" + ");
            s.push_str(" ! ");
            s.push_str(&eff);
        }
        if !d.constraints.is_empty() {
            let cs = d
                .constraints
                .iter()
                .map(|c| format!("{}: {}", c.name, c.bound))
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(" where ");
            s.push_str(&cs);
        }
        s
    }

    fn print_type_decl(&mut self, d: &TypeDecl) {
        let ind = self.ind();
        let vis = if d.visible { "pub " } else { "" };
        let params = if d.params.is_empty() {
            String::new()
        } else {
            format!("[{}]", self.fmt_type_params(&d.params))
        };
        match &d.body {
            TypeBody::Alias(ty) => {
                self.line(&format!(
                    "{}{}type {}{} = {}",
                    ind,
                    vis,
                    d.name,
                    params,
                    self.fmt_type_expr(ty)
                ));
            }
            TypeBody::Struct { fields, invariant } => {
                self.line(&format!(
                    "{}{}type {}{} = struct {{",
                    ind, vis, d.name, params
                ));
                for field in fields {
                    self.push(&format!("{}    {}", ind, self.fmt_field_decl(field)));
                    self.line(",");
                }
                if let Some(inv) = invariant {
                    self.line(&format!(
                        "{}}} with invariant {}",
                        ind,
                        self.fmt_ref_expr(inv)
                    ));
                } else {
                    self.line(&format!("{}}}", ind));
                }
            }
            TypeBody::Enum(variants) => {
                self.line(&format!(
                    "{}{}type {}{} = enum {{",
                    ind, vis, d.name, params
                ));
                for variant in variants {
                    self.push(&format!("{}    {}", ind, self.fmt_variant(variant)));
                    self.line(",");
                }
                self.line(&format!("{}}}", ind));
            }
        }
    }

    fn print_const(&mut self, d: &ConstDecl) {
        let ind = self.ind();
        let vis = if d.visible { "pub " } else { "" };
        let val = self.fmt_expr(&d.value, self.indent);
        self.line(&format!(
            "{}{}const {}: {} = {};",
            ind,
            vis,
            d.name,
            self.fmt_type_expr(&d.ty),
            val
        ));
    }

    fn print_extern(&mut self, d: &ExternDecl) {
        let ind = self.ind();
        self.line(&format!("{}extern \"{}\" {{", ind, d.abi));
        for f in &d.fns {
            self.push(&format!("{}    ", ind));
            self.line(&self.fmt_extern_fn(f, self.indent + 1));
        }
        self.line(&format!("{}}}", ind));
    }

    fn fmt_extern_fn(&self, f: &ExternFnDecl, _indent: usize) -> String {
        let mut s = String::new();
        if let Some(tot) = &f.totality {
            match tot {
                Totality::Total => s.push_str("total "),
                Totality::Partial => s.push_str("partial "),
            }
        }
        s.push_str("fn ");
        s.push_str(&f.name);
        s.push('(');
        s.push_str(&self.fmt_params(&f.params));
        s.push(')');
        s.push_str(" -> ");
        s.push_str(&self.fmt_type_expr(&f.return_type));
        if !f.effects.is_empty() {
            let eff = f
                .effects
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(" + ");
            s.push_str(" ! ");
            s.push_str(&eff);
        }
        s
    }

    fn print_impl(&mut self, d: &ImplDecl) {
        let ind = self.ind();
        let trait_args = if d.trait_type_args.is_empty() {
            String::new()
        } else {
            format!(
                "[{}]",
                d.trait_type_args
                    .iter()
                    .map(|t| self.fmt_type_expr(t))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.line(&format!(
            "{}impl {}{} for {} {{",
            ind, d.trait_name, trait_args, d.type_name
        ));
        self.indent += 1;
        for (i, method) in d.methods.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.print_fn(method);
        }
        self.indent -= 1;
        self.line(&format!("{}}}", ind));
    }

    fn print_actor(&mut self, d: &ActorDecl) {
        let ind = self.ind();
        let vis = if d.visible { "pub " } else { "" };
        let params = if d.type_params.is_empty() {
            String::new()
        } else {
            format!("[{}]", self.fmt_type_params(&d.type_params))
        };
        self.line(&format!("{}{}actor {}{} {{", ind, vis, d.name, params));
        for field in &d.fields {
            self.push(&format!("{}    {}", ind, self.fmt_field_decl(field)));
            self.newline();
        }
        if !d.fields.is_empty() && !d.methods.is_empty() {
            self.newline();
        }
        for (i, method) in d.methods.iter().enumerate() {
            if i > 0 {
                self.newline();
            }
            self.push(&format!("{}    ", ind));
            self.line(&self.fmt_actor_method(method, self.indent + 1));
        }
        self.line(&format!("{}}}", ind));
    }

    fn fmt_actor_method(&self, m: &ActorMethod, indent: usize) -> String {
        let vis = if m.is_public { "pub " } else { "" };
        let params = self.fmt_params(&m.params);
        let ret = self.fmt_type_expr(&m.return_type);
        let eff = if m.effects.is_empty() {
            String::new()
        } else {
            format!(
                " ! {}",
                m.effects
                    .iter()
                    .map(|e| e.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" + ")
            )
        };
        let body = self.fmt_block_multi(&m.body, indent);
        format!(
            "{}fn {}({}) -> {}{} {}",
            vis,
            m.name,
            params,
            ret,
            eff,
            body.trim_start()
        )
    }

    fn print_effect_decl(&mut self, d: &EffectDecl) {
        let ind = self.ind();
        if d.subsumes.is_empty() {
            self.line(&format!("{}effect {}", ind, d.name));
        } else {
            self.line(&format!(
                "{}effect {} > {}",
                ind,
                d.name,
                d.subsumes.join(" + ")
            ));
        }
    }

    fn print_label(&mut self, d: &LabelDecl) {
        let ind = self.ind();
        let vis = if d.visible { "pub " } else { "" };
        self.line(&format!("{}{}label {}", ind, vis, d.name));
    }

    fn print_relabel(&mut self, d: &RelabelDecl) {
        let ind = self.ind();
        let vis = if d.visible { "pub " } else { "" };
        let from = d.from.as_deref().unwrap_or("_");
        let to = d.to.as_deref().unwrap_or("_");
        let audit_kw = if d.audit { " audit" } else { "" };
        self.line(&format!(
            "{}{}relabel {}: {} -> {}{}",
            ind, vis, d.name, from, to, audit_kw
        ));
    }

    // ── Block formatting ────────────────────────────────────────────────────

    /// Try to render the block on a single line: `{ expr }`.
    /// Only inlines empty blocks or single-expression (no leading statements) blocks.
    fn try_block_inline(&self, block: &Block) -> Option<String> {
        if block.stmts.is_empty() {
            return Some("{ }".to_string());
        }
        // Only inline blocks with a single statement.
        if block.stmts.len() != 1 {
            return None;
        }
        let s = self.fmt_stmt_inline(&block.stmts[0], true)?;
        if s.contains('\n') {
            return None;
        }
        let result = format!("{{ {} }}", s);
        if result.len() <= 60 {
            Some(result)
        } else {
            None
        }
    }

    /// Render a single statement for inline (single-line) use.
    /// Returns `None` if the statement can't be represented inline.
    fn fmt_stmt_inline(&self, stmt: &Stmt, is_last: bool) -> Option<String> {
        match stmt {
            Stmt::Expr { expr, .. } => {
                let s = self.fmt_expr(expr, 0);
                if s.contains('\n') {
                    return None;
                }
                if is_last {
                    Some(s)
                } else {
                    Some(format!("{};", s))
                }
            }
            Stmt::Return { value: None, .. } => Some("return;".to_string()),
            Stmt::Return { value: Some(v), .. } => {
                let s = self.fmt_expr(v, 0);
                if s.contains('\n') {
                    return None;
                }
                Some(format!("return {};", s))
            }
            Stmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                let ghost = if matches!(kind, LetKind::Ghost) {
                    "ghost "
                } else {
                    ""
                };
                let p = self.fmt_pattern(pattern);
                let t = self.fmt_type_expr(ty);
                let v = self.fmt_expr(init, 0);
                if v.contains('\n') {
                    return None;
                }
                Some(format!("{}let {}: {} = {};", ghost, p, t, v))
            }
            _ => None,
        }
    }

    /// Render a block in multi-line form, indented at `outer_indent`.
    fn fmt_block_multi(&self, block: &Block, outer_indent: usize) -> String {
        if block.stmts.is_empty() {
            return "{ }".to_string();
        }
        let ind = "    ".repeat(outer_indent);
        let inner_ind = "    ".repeat(outer_indent + 1);
        let n = block.stmts.len();
        let mut lines = vec!["{\n".to_string()];
        for (i, stmt) in block.stmts.iter().enumerate() {
            let is_last = i == n - 1;
            let s = self.fmt_stmt(stmt, is_last, outer_indent + 1);
            // Indent each line of the stmt output.
            for (j, sline) in s.lines().enumerate() {
                if j == 0 {
                    lines.push(format!("{}{}\n", inner_ind, sline));
                } else {
                    lines.push(format!("{}\n", sline));
                }
            }
        }
        lines.push(format!("{}}}", ind));
        lines.join("")
    }

    // ── Statement formatting ────────────────────────────────────────────────

    fn fmt_stmt(&self, stmt: &Stmt, is_last: bool, indent: usize) -> String {
        match stmt {
            Stmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                let ghost = if matches!(kind, LetKind::Ghost) {
                    "ghost "
                } else {
                    ""
                };
                let p = self.fmt_pattern(pattern);
                let t = self.fmt_type_expr(ty);
                let v = self.fmt_expr(init, indent);
                format!("{}let {}: {} = {};", ghost, p, t, v)
            }
            Stmt::Assign { target, value, .. } => {
                format!(
                    "{} = {};",
                    self.fmt_lvalue(target),
                    self.fmt_expr(value, indent)
                )
            }
            Stmt::Return { value: None, .. } => "return;".to_string(),
            Stmt::Return { value: Some(v), .. } => {
                format!("return {};", self.fmt_expr(v, indent))
            }
            Stmt::If {
                cond, then, else_, ..
            } => self.fmt_if_stmt(cond, then, else_.as_ref(), indent),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.fmt_match_stmt(scrutinee, arms, indent),
            Stmt::For {
                pattern,
                iter,
                invariants,
                body,
                ..
            } => self.fmt_for_stmt(pattern, iter, invariants, body, indent),
            Stmt::While {
                cond,
                invariants,
                decreases,
                body,
                ..
            } => self.fmt_while_stmt(cond, invariants, decreases.as_deref(), body, indent),
            Stmt::Expr { expr, .. } => {
                let s = self.fmt_expr(expr, indent);
                if is_last {
                    s // no semicolon — return expression
                } else {
                    format!("{};", s)
                }
            }
        }
    }

    fn fmt_if_stmt(
        &self,
        cond: &Expr,
        then: &Block,
        else_: Option<&ElseBranch>,
        indent: usize,
    ) -> String {
        let cond_s = self.fmt_expr(cond, indent);
        let then_s = self.fmt_block_multi(then, indent);
        let mut s = format!("if {} {}", cond_s, then_s.trim_start());
        if let Some(branch) = else_ {
            match branch {
                ElseBranch::Block(b) => {
                    s.push_str(" else ");
                    s.push_str(self.fmt_block_multi(b, indent).trim_start());
                }
                ElseBranch::If(inner_if) => {
                    s.push_str(" else ");
                    s.push_str(&self.fmt_stmt(inner_if, false, indent));
                }
            }
        }
        s
    }

    fn fmt_match_stmt(&self, scrutinee: &Expr, arms: &[MatchArm], indent: usize) -> String {
        let ind = "    ".repeat(indent);
        let inner_ind = "    ".repeat(indent + 1);
        let mut s = format!("match {} {{\n", self.fmt_expr(scrutinee, indent));
        for arm in arms {
            let guard = arm
                .guard
                .as_ref()
                .map(|g| format!(" if {}", self.fmt_ref_expr(g)))
                .unwrap_or_default();
            let body = match &arm.body {
                MatchBody::Expr(e) => self.fmt_expr(e, indent + 1),
                MatchBody::Block(b) => self.fmt_block_multi(b, indent + 1),
            };
            s.push_str(&format!(
                "{}{}{} => {},\n",
                inner_ind,
                self.fmt_pattern(&arm.pattern),
                guard,
                body
            ));
        }
        s.push_str(&format!("{}}}", ind));
        s
    }

    fn fmt_for_stmt(
        &self,
        pattern: &Pattern,
        iter: &Expr,
        invariants: &[Expr],
        body: &Block,
        indent: usize,
    ) -> String {
        let ind = "    ".repeat(indent);
        let mut s = format!(
            "for {} in {} ",
            self.fmt_pattern(pattern),
            self.fmt_expr(iter, indent)
        );
        for inv in invariants {
            s = format!(
                "{}\n{}    invariant {} ",
                s.trim_end(),
                ind,
                self.fmt_expr(inv, indent)
            );
        }
        s.push_str(self.fmt_block_multi(body, indent).trim_start());
        s
    }

    fn fmt_while_stmt(
        &self,
        cond: &Expr,
        invariants: &[Expr],
        decreases: Option<&Expr>,
        body: &Block,
        indent: usize,
    ) -> String {
        let ind = "    ".repeat(indent);
        let mut s = format!("while {} ", self.fmt_expr(cond, indent));
        for inv in invariants {
            s = format!(
                "{}\n{}    invariant {} ",
                s.trim_end(),
                ind,
                self.fmt_expr(inv, indent)
            );
        }
        if let Some(dec) = decreases {
            s = format!(
                "{}\n{}    decreases {} ",
                s.trim_end(),
                ind,
                self.fmt_expr(dec, indent)
            );
        }
        s.push_str(self.fmt_block_multi(body, indent).trim_start());
        s
    }

    // ── Expression formatting ───────────────────────────────────────────────

    fn fmt_expr(&self, expr: &Expr, indent: usize) -> String {
        match expr {
            Expr::Literal(lit, _) => self.fmt_literal(lit),
            Expr::Ident(name, _) => name.clone(),
            Expr::FieldAccess { expr, field, .. } => {
                format!("{}.{}", self.fmt_expr(expr, indent), field)
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let recv = self.fmt_expr(receiver, indent);
                let args_s = args
                    .iter()
                    .map(|a| self.fmt_expr(a, indent))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}.{}({})", recv, method, args_s)
            }
            Expr::FnCall {
                name,
                type_args,
                args,
                ..
            } => {
                let targs = if type_args.is_empty() {
                    String::new()
                } else {
                    format!(
                        "[{}]",
                        type_args
                            .iter()
                            .map(|t| self.fmt_type_expr(t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let args_s = args
                    .iter()
                    .map(|a| self.fmt_expr(a, indent))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}{}({})", name, targs, args_s)
            }
            Expr::Unary { op, expr, .. } => {
                let op_s = self.fmt_unary_op(op);
                let e = self.fmt_expr(expr, indent);
                // Add parens around complex sub-expressions for unary.
                if self.expr_needs_parens_unary(expr) {
                    format!("{}({})", op_s, e)
                } else {
                    format!("{}{}", op_s, e)
                }
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let lhs = self.fmt_expr_parens(left, op, true, indent);
                let rhs = self.fmt_expr_parens(right, op, false, indent);
                format!("{} {} {}", lhs, self.fmt_binary_op(op), rhs)
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                let cond_s = self.fmt_expr(cond, indent);
                let then_s = self.fmt_block_multi(then, indent);
                let mut s = format!("if {} {}", cond_s, then_s.trim_start());
                if let Some(e) = else_ {
                    let else_s = self.fmt_expr(e, indent);
                    s.push_str(" else ");
                    s.push_str(&else_s);
                }
                s
            }
            Expr::Match {
                scrutinee, arms, ..
            } => self.fmt_match_expr(scrutinee, arms, indent),
            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => {
                let params_s = params
                    .iter()
                    .map(|p| self.fmt_param(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                let ret_s = ret_type
                    .as_ref()
                    .map(|t| format!(" -> {}", self.fmt_type_expr(t)))
                    .unwrap_or_default();
                let body_s = self.fmt_expr(body, indent);
                format!("|{}|{} {}", params_s, ret_s, body_s)
            }
            Expr::Block(b) => {
                // Inline blocks in expression position.
                if let Some(inline) = self.try_block_inline(b) {
                    inline
                } else {
                    self.fmt_block_multi(b, indent)
                }
            }
            Expr::Propagate { expr, .. } => {
                format!("{}?", self.fmt_expr(expr, indent))
            }
            Expr::Construct { name, fields, .. } => {
                let fields_s = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.fmt_expr(v, indent)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} {{ {} }}", name, fields_s)
            }
            Expr::List { elems, .. } => {
                let s = elems
                    .iter()
                    .map(|e| self.fmt_expr(e, indent))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{}]", s)
            }
            Expr::Map { pairs, .. } => {
                if pairs.is_empty() {
                    "Map::new()".to_string()
                } else {
                    let s = pairs
                        .iter()
                        .map(|(k, v)| {
                            format!("{}: {}", self.fmt_expr(k, indent), self.fmt_expr(v, indent))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{{{}}}", s)
                }
            }
            Expr::Set { elems, .. } => {
                let s = elems
                    .iter()
                    .map(|e| self.fmt_expr(e, indent))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{}}}", s)
            }
            Expr::Consume { expr, .. } => {
                format!("consume({})", self.fmt_expr(expr, indent))
            }
            Expr::Relabel {
                name,
                expr,
                tag,
                audit,
                ..
            } => {
                let audit_kw = if *audit { " audit" } else { "" };
                format!(
                    "relabel {}({}, {:?}){}",
                    name,
                    self.fmt_expr(expr, indent),
                    tag,
                    audit_kw
                )
            }
            Expr::Borrow { mutable, expr, .. } => {
                let kw = if *mutable { "ref" } else { "val" };
                format!("{} {}", kw, self.fmt_expr(expr, indent))
            }
            Expr::Spawn {
                actor_type, fields, ..
            } => {
                let fields_s = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.fmt_expr(v, indent)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("actor {} {{ {} }}", actor_type, fields_s)
            }
            Expr::Select { arms, .. } => self.fmt_select(arms, indent),

            Expr::As { expr, target, .. } => {
                format!(
                    "{} as {}",
                    self.fmt_expr(expr, indent),
                    self.fmt_type_expr(target)
                )
            }
            Expr::Quantifier(ref_expr, _) => self.fmt_ref_expr(ref_expr),
        }
    }

    fn fmt_match_expr(&self, scrutinee: &Expr, arms: &[MatchArm], indent: usize) -> String {
        let ind = "    ".repeat(indent);
        let inner_ind = "    ".repeat(indent + 1);
        let mut s = format!("match {} {{\n", self.fmt_expr(scrutinee, indent));
        for arm in arms {
            let guard = arm
                .guard
                .as_ref()
                .map(|g| format!(" if {}", self.fmt_ref_expr(g)))
                .unwrap_or_default();
            let body = match &arm.body {
                MatchBody::Expr(e) => self.fmt_expr(e, indent + 1),
                MatchBody::Block(b) => self.fmt_block_multi(b, indent + 1),
            };
            s.push_str(&format!(
                "{}{}{} => {},\n",
                inner_ind,
                self.fmt_pattern(&arm.pattern),
                guard,
                body
            ));
        }
        s.push_str(&format!("{}}}", ind));
        s
    }

    fn fmt_select(&self, arms: &[crate::mvl::parser::ast::SelectArm], indent: usize) -> String {
        let ind = "    ".repeat(indent);
        let inner_ind = "    ".repeat(indent + 1);
        let mut s = "select {\n".to_string();
        for arm in arms {
            let binding = arm
                .binding
                .as_ref()
                .map(|b| format!("{} = ", b))
                .unwrap_or_default();
            let expr_s = self.fmt_expr(&arm.expr, indent + 1);
            let body_s = self.fmt_block_multi(&arm.body, indent + 1);
            if arm.is_timeout {
                s.push_str(&format!(
                    "{}timeout({}) => {}\n",
                    inner_ind,
                    expr_s,
                    body_s.trim_start()
                ));
            } else {
                s.push_str(&format!(
                    "{}{}{} => {}\n",
                    inner_ind,
                    binding,
                    expr_s,
                    body_s.trim_start()
                ));
            }
        }
        s.push_str(&format!("{}}}", ind));
        s
    }

    /// Add parentheses around `expr` when it has lower precedence than `op`.
    fn fmt_expr_parens(&self, expr: &Expr, op: &BinaryOp, is_left: bool, indent: usize) -> String {
        let s = self.fmt_expr(expr, indent);
        if self.needs_parens(expr, op, is_left) {
            format!("({})", s)
        } else {
            s
        }
    }

    fn needs_parens(&self, expr: &Expr, parent_op: &BinaryOp, _is_left: bool) -> bool {
        if let Expr::Binary { op, .. } = expr {
            op_precedence(op) < op_precedence(parent_op)
        } else {
            false
        }
    }

    fn expr_needs_parens_unary(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::Binary { .. })
    }

    // ── Type expression formatting ──────────────────────────────────────────

    fn fmt_type_expr(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, args, .. } => {
                if args.is_empty() {
                    name.clone()
                } else {
                    let a = args
                        .iter()
                        .map(|t| self.fmt_type_expr(t))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}[{}]", name, a)
                }
            }
            TypeExpr::Option { inner, .. } => {
                format!("Option[{}]", self.fmt_type_expr(inner))
            }
            TypeExpr::Result { ok, err, .. } => {
                format!(
                    "Result[{}, {}]",
                    self.fmt_type_expr(ok),
                    self.fmt_type_expr(err)
                )
            }
            TypeExpr::Ref { mutable, inner, .. } => {
                let kw = if *mutable { "ref" } else { "val" };
                format!("{} {}", kw, self.fmt_type_expr(inner))
            }
            TypeExpr::Labeled { label, inner, .. } => {
                format!("{}[{}]", label, self.fmt_type_expr(inner))
            }
            TypeExpr::Refined { inner, pred, .. } => {
                format!(
                    "{} where {}",
                    self.fmt_type_expr(inner),
                    self.fmt_ref_expr(pred)
                )
            }
            TypeExpr::Fn {
                params,
                ret,
                effects,
                ..
            } => {
                let ps = params
                    .iter()
                    .map(|t| self.fmt_type_expr(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                let eff = if effects.is_empty() {
                    String::new()
                } else {
                    format!(
                        " ! {}",
                        effects
                            .iter()
                            .map(|e| e.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" + ")
                    )
                };
                format!("fn({}) -> {}{}", ps, self.fmt_type_expr(ret), eff)
            }
            TypeExpr::IntConst { value, .. } => value.to_string(),
            TypeExpr::Session { op, .. } => self.fmt_session_op(op),
        }
    }

    fn fmt_session_op(&self, op: &SessionOp) -> String {
        match op {
            SessionOp::Send { msg, cont, .. } => {
                format!(
                    "!{}. {}",
                    self.fmt_type_expr(msg),
                    self.fmt_session_op(cont)
                )
            }
            SessionOp::Receive { msg, cont, .. } => {
                format!(
                    "?{}. {}",
                    self.fmt_type_expr(msg),
                    self.fmt_session_op(cont)
                )
            }
            SessionOp::InternalChoice { branches, .. } => {
                let bs = branches
                    .iter()
                    .map(|(l, s)| format!("{}: {}", l, self.fmt_session_op(s)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("+{{ {} }}", bs)
            }
            SessionOp::ExternalChoice { branches, .. } => {
                let bs = branches
                    .iter()
                    .map(|(l, s)| format!("{}: {}", l, self.fmt_session_op(s)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("&{{ {} }}", bs)
            }
            SessionOp::End { .. } => "end".to_string(),
        }
    }

    // ── Refinement expression formatting ───────────────────────────────────

    fn fmt_ref_expr(&self, re: &RefExpr) -> String {
        match re {
            RefExpr::LogicOp {
                op, left, right, ..
            } => {
                let op_s = match op {
                    LogicOp::And => "&&",
                    LogicOp::Or => "||",
                };
                format!(
                    "{} {} {}",
                    self.fmt_ref_expr(left),
                    op_s,
                    self.fmt_ref_expr(right)
                )
            }
            RefExpr::Compare {
                op, left, right, ..
            } => {
                let op_s = match op {
                    CmpOp::Eq => "==",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Gt => ">",
                    CmpOp::Le => "<=",
                    CmpOp::Ge => ">=",
                };
                format!(
                    "{} {} {}",
                    self.fmt_ref_expr(left),
                    op_s,
                    self.fmt_ref_expr(right)
                )
            }
            RefExpr::ArithOp {
                op, left, right, ..
            } => {
                let op_s = match op {
                    ArithOp::Add => "+",
                    ArithOp::Sub => "-",
                    ArithOp::Mul => "*",
                    ArithOp::Div => "/",
                    ArithOp::Rem => "%",
                };
                format!(
                    "{} {} {}",
                    self.fmt_ref_expr(left),
                    op_s,
                    self.fmt_ref_expr(right)
                )
            }
            RefExpr::Not { inner, .. } => format!("!{}", self.fmt_ref_expr(inner)),
            RefExpr::Ident { name, .. } => name.clone(),
            RefExpr::FieldAccess { object, field, .. } => {
                format!("{}.{}", self.fmt_ref_expr(object), field)
            }
            RefExpr::Integer { value, .. } => value.to_string(),
            RefExpr::Float { value, .. } => {
                let s = format!("{}", value);
                if s.contains('.') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            RefExpr::Bool { value, .. } => value.to_string(),
            RefExpr::Len { ident, .. } => format!("len({})", ident),
            RefExpr::Grouped { inner, .. } => format!("({})", self.fmt_ref_expr(inner)),
            RefExpr::Old { inner, .. } => format!("old({})", self.fmt_ref_expr(inner)),
            RefExpr::Forall { var, ty, body, .. } => {
                format!(
                    "forall {}: {}, {}",
                    var,
                    self.fmt_type_expr(ty),
                    self.fmt_ref_expr(body)
                )
            }
            RefExpr::Exists { var, ty, body, .. } => {
                format!(
                    "exists {}: {}, {}",
                    var,
                    self.fmt_type_expr(ty),
                    self.fmt_ref_expr(body)
                )
            }
            RefExpr::BitwiseOp {
                op, left, right, ..
            } => {
                use crate::mvl::parser::ast::BitwiseOp;
                let op_s = match op {
                    BitwiseOp::And => "&",
                    BitwiseOp::Or => "|",
                    BitwiseOp::Xor => "^",
                    BitwiseOp::Shl => "<<",
                    BitwiseOp::Shr => ">>",
                };
                format!(
                    "{} {} {}",
                    self.fmt_ref_expr(left),
                    op_s,
                    self.fmt_ref_expr(right)
                )
            }
            RefExpr::BitwiseNot { inner, .. } => format!("~{}", self.fmt_ref_expr(inner)),
            RefExpr::BoundedForall {
                var, lo, hi, body, ..
            } => {
                format!(
                    "forall {} in [{}..{}]. {}",
                    var,
                    lo,
                    hi,
                    self.fmt_ref_expr(body)
                )
            }
            RefExpr::BoundedExists {
                var, lo, hi, body, ..
            } => {
                format!(
                    "exists {} in [{}..{}]. {}",
                    var,
                    lo,
                    hi,
                    self.fmt_ref_expr(body)
                )
            }
            RefExpr::StringOp { op, receiver, literal, .. } => {
                use crate::mvl::parser::ast::StringOp;
                let method = match op {
                    StringOp::Contains => "contains",
                    StringOp::StartsWith => "starts_with",
                    StringOp::EndsWith => "ends_with",
                };
                format!("{}.{}({:?})", self.fmt_ref_expr(receiver), method, literal)
            }
        }
    }

    // ── Pattern formatting ──────────────────────────────────────────────────

    fn fmt_pattern(&self, pat: &Pattern) -> String {
        match pat {
            Pattern::Wildcard(_) => "_".to_string(),
            Pattern::Ident(name, _) => name.clone(),
            Pattern::Literal(lit, _) => self.fmt_literal(lit),
            Pattern::TupleStruct { name, fields, .. } => {
                let fs = fields
                    .iter()
                    .map(|p| self.fmt_pattern(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", name, fs)
            }
            Pattern::Struct {
                name, fields, rest, ..
            } => {
                let mut parts: Vec<String> = fields
                    .iter()
                    .map(|(k, p)| format!("{}: {}", k, self.fmt_pattern(p)))
                    .collect();
                if *rest {
                    parts.push("..".to_string());
                }
                format!("{} {{ {} }}", name, parts.join(", "))
            }
            Pattern::Some { inner, .. } => format!("Some({})", self.fmt_pattern(inner)),
            Pattern::None(_) => "None".to_string(),
            Pattern::Ok { inner, .. } => format!("Ok({})", self.fmt_pattern(inner)),
            Pattern::Err { inner, .. } => format!("Err({})", self.fmt_pattern(inner)),
            Pattern::Or { patterns, .. } => patterns
                .iter()
                .map(|p| self.fmt_pattern(p))
                .collect::<Vec<_>>()
                .join(" | "),
        }
    }

    // ── Helper formatters ───────────────────────────────────────────────────

    fn fmt_literal(&self, lit: &Literal) -> String {
        match lit {
            Literal::Integer(n) => n.to_string(),
            Literal::Float(f) => {
                let s = format!("{}", f);
                if s.contains('.') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            Literal::Str(s) => format!("{:?}", s),
            Literal::Char(c) => format!("'{}'", c),
            Literal::Bool(b) => b.to_string(),
            Literal::Unit => "()".to_string(),
        }
    }

    fn fmt_binary_op(&self, op: &BinaryOp) -> &'static str {
        match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Rem => "%",
            BinaryOp::Eq => "==",
            BinaryOp::Ne => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Gt => ">",
            BinaryOp::Le => "<=",
            BinaryOp::Ge => ">=",
            BinaryOp::And => "&&",
            BinaryOp::Or => "||",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
        }
    }

    fn fmt_unary_op(&self, op: &UnaryOp) -> &'static str {
        match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::Deref => "*",
            UnaryOp::BitNot => "~",
        }
    }

    fn fmt_type_params(&self, params: &[GenericParam]) -> String {
        params
            .iter()
            .map(|p| match p {
                GenericParam::Type(name) => name.clone(),
                GenericParam::Const(name, ty) => format!("const {}: {}", name, ty),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn fmt_param(&self, p: &crate::mvl::parser::ast::Param) -> String {
        let cap = p
            .capability
            .as_ref()
            .map(|c| match c {
                Capability::Iso => "iso ",
                Capability::Val => "val ",
                Capability::Ref => "ref ",
                Capability::Tag => "tag ",
            })
            .unwrap_or("");
        let refinement = p
            .refinement
            .as_ref()
            .map(|r| format!(" where {}", self.fmt_ref_expr(r)))
            .unwrap_or_default();
        format!(
            "{}{}: {}{}",
            cap,
            p.name,
            self.fmt_type_expr(&p.ty),
            refinement
        )
    }

    fn fmt_params(&self, params: &[crate::mvl::parser::ast::Param]) -> String {
        params
            .iter()
            .map(|p| self.fmt_param(p))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn fmt_field_decl(&self, f: &FieldDecl) -> String {
        let refinement = f
            .refinement
            .as_ref()
            .map(|r| format!(" where {}", self.fmt_ref_expr(r)))
            .unwrap_or_default();
        format!("{}: {}{}", f.name, self.fmt_type_expr(&f.ty), refinement)
    }

    fn fmt_variant(&self, v: &Variant) -> String {
        match &v.fields {
            VariantFields::Unit => v.name.clone(),
            VariantFields::Tuple(tys) => {
                let ts = tys
                    .iter()
                    .map(|t| self.fmt_type_expr(t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({})", v.name, ts)
            }
            VariantFields::Struct(fields) => {
                let fs = fields
                    .iter()
                    .map(|f| self.fmt_field_decl(f))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} {{ {} }}", v.name, fs)
            }
        }
    }

    fn fmt_lvalue(&self, lval: &LValue) -> String {
        match lval {
            LValue::Ident(name, _) => name.clone(),
            LValue::Field { base, field, .. } => format!("{}.{}", self.fmt_lvalue(base), field),
        }
    }
}

// ── Operator precedence ────────────────────────────────────────────────────

fn op_precedence(op: &BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::BitOr => 3,
        BinaryOp::BitXor => 4,
        BinaryOp::BitAnd => 5,
        BinaryOp::Eq | BinaryOp::Ne => 6,
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => 7,
        BinaryOp::Shl | BinaryOp::Shr => 8,
        BinaryOp::Add | BinaryOp::Sub => 9,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => 10,
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Format `source` as canonical MVL. Returns the formatted string, or an
/// error message if parsing fails.
pub fn format_source(source: &str) -> Result<String, String> {
    use crate::mvl::parser::Parser;

    let (mut parser, lex_errs) = Parser::new(source);
    if !lex_errs.is_empty() {
        return Err(lex_errs[0].message.clone());
    }

    let prog = parser.parse_program();

    if !parser.errors().is_empty() {
        return Err(parser.errors()[0].message.clone());
    }

    Ok(Printer::new(source).print(&prog))
}
