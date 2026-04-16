//! Phase-1 and Phase-2 lint rules.
//!
//! Two families:
//!
//! * **Source rules** — operate on the raw source string line-by-line.
//!   They have no access to the AST.
//!
//! * **AST rules** — traverse the parsed [`Program`] to find naming,
//!   structural, and semantic violations.

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, Literal, MatchArm, MatchBody, Pattern, Program,
    SecurityLabel, Stmt, TypeBody, TypeExpr, VariantFields,
};
use std::collections::{HashMap, HashSet};

// ── Source rules ───────────────────────────────────────────────────────────

/// Check trailing whitespace on every line.
///
/// Rule id: `trailing-whitespace`
pub fn trailing_whitespace(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.trailing_ws {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let trimmed = line.trim_end();
        if trimmed.len() < line.len() {
            let col = (trimmed.len() + 1) as u32;
            out.push(LintDiag::warning(
                "trailing-whitespace",
                "trailing whitespace",
                line_no,
                col,
            ));
        }
    }
}

/// Check that no line exceeds `cfg.line_length` characters.
///
/// Rule id: `line-length`
pub fn line_length(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        let len = line.chars().count();
        if len > cfg.line_length {
            out.push(LintDiag::warning(
                "line-length",
                format!("line is {len} characters (max {})", cfg.line_length),
                line_no,
                (cfg.line_length + 1).min(u32::MAX as usize) as u32,
            ));
        }
    }
}

/// Check indentation style and width.
///
/// Rule id: `indentation`
///
/// Flags:
/// * Mixed tabs/spaces on an indented line.
/// * Use of the wrong character (tabs when `indent_style = spaces`, or vice versa).
/// * Indent not a multiple of `indent_size` when using spaces.
pub fn indentation(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        if line.is_empty() {
            continue;
        }
        let leading: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if leading.is_empty() {
            continue;
        }
        let has_spaces = leading.contains(' ');
        let has_tabs = leading.contains('\t');
        if has_spaces && has_tabs {
            out.push(LintDiag::warning(
                "indentation",
                "mixed tabs and spaces in indentation",
                line_no,
                1,
            ));
            continue;
        }
        if cfg.indent_spaces && has_tabs {
            out.push(LintDiag::warning(
                "indentation",
                "tab used for indentation (expected spaces)",
                line_no,
                1,
            ));
        } else if !cfg.indent_spaces && has_spaces {
            out.push(LintDiag::warning(
                "indentation",
                "spaces used for indentation (expected tabs)",
                line_no,
                1,
            ));
        } else if cfg.indent_spaces && cfg.indent_size > 0 {
            let n = leading.len();
            if !n.is_multiple_of(cfg.indent_size) {
                out.push(LintDiag::warning(
                    "indentation",
                    format!(
                        "indent of {n} spaces is not a multiple of {}",
                        cfg.indent_size
                    ),
                    line_no,
                    1,
                ));
            }
        }
    }
}

/// Check that the file ends with exactly one newline (no trailing blank lines).
///
/// Rule id: `final-newline`
pub fn final_newline(src: &str, _cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if src.is_empty() {
        return;
    }
    if !src.ends_with('\n') {
        let line_no = src.lines().count() as u32;
        out.push(LintDiag::warning(
            "final-newline",
            "file must end with a newline",
            line_no,
            1,
        ));
    } else if src.ends_with("\n\n") {
        let line_no = src.lines().count() as u32 + 1;
        out.push(LintDiag::warning(
            "final-newline",
            "file has trailing blank lines",
            line_no,
            1,
        ));
    }
}

// ── AST rules ──────────────────────────────────────────────────────────────

/// Check naming conventions across all declarations.
///
/// Rules:
/// * Functions → `snake_case`  (rule id: `naming-fn`)
/// * Types     → `PascalCase`  (rule id: `naming-type`)
/// * Fields    → `snake_case`  (rule id: `naming-field`)
/// * Constants → `SCREAMING_SNAKE_CASE` (rule id: `naming-const`)
pub fn naming(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.naming {
        return;
    }
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) => {
                if !is_snake_case(&f.name) {
                    out.push(LintDiag::warning(
                        "naming-fn",
                        format!("function `{}` should be snake_case", f.name),
                        f.span.line,
                        f.span.col,
                    ));
                }
            }
            Decl::Type(t) => {
                if !is_pascal_case(&t.name) {
                    out.push(LintDiag::warning(
                        "naming-type",
                        format!("type `{}` should be PascalCase", t.name),
                        t.span.line,
                        t.span.col,
                    ));
                }
                // Check field names inside structs and enum struct-variants
                match &t.body {
                    TypeBody::Struct(fields) => {
                        for field in fields {
                            if !is_snake_case(&field.name) {
                                out.push(LintDiag::warning(
                                    "naming-field",
                                    format!("field `{}` should be snake_case", field.name),
                                    field.span.line,
                                    field.span.col,
                                ));
                            }
                        }
                    }
                    TypeBody::Enum(variants) => {
                        for variant in variants {
                            if !is_pascal_case(&variant.name) {
                                out.push(LintDiag::warning(
                                    "naming-variant",
                                    format!("enum variant `{}` should be PascalCase", variant.name),
                                    variant.span.line,
                                    variant.span.col,
                                ));
                            }
                            // Struct-variant fields
                            if let crate::mvl::parser::ast::VariantFields::Struct(fields) =
                                &variant.fields
                            {
                                for field in fields {
                                    if !is_snake_case(&field.name) {
                                        out.push(LintDiag::warning(
                                            "naming-field",
                                            format!("field `{}` should be snake_case", field.name),
                                            field.span.line,
                                            field.span.col,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    TypeBody::Alias(_) => {}
                }
            }
            Decl::Const(c) => {
                if !is_screaming_snake_case(&c.name) {
                    out.push(LintDiag::warning(
                        "naming-const",
                        format!("constant `{}` should be SCREAMING_SNAKE_CASE", c.name),
                        c.span.line,
                        c.span.col,
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Check function body length.
///
/// Rule id: `fn-length`
///
/// A function body that spans more than `cfg.max_fn_length` source lines is
/// flagged as a warning — long functions are harder for humans and LLMs alike.
pub fn fn_length(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_fn_length == 0 {
        return;
    }
    let lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            let body = &f.body;
            // Body span covers the opening `{` to the closing `}`.
            let start_line = body.span.line as usize; // 1-based
                                                      // End line: walk from start, count braces.
            let end_line = body_end_line(src, &lines, start_line);
            let length = end_line.saturating_sub(start_line) + 1;
            if length > cfg.max_fn_length {
                out.push(LintDiag::warning(
                    "fn-length",
                    format!(
                        "function `{}` body is {length} lines (max {})",
                        f.name, cfg.max_fn_length
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Estimate the closing-brace line of a block starting at `start_line` (1-based).
fn body_end_line(src: &str, lines: &[&str], start_line: usize) -> usize {
    let _ = src;
    let mut depth: i32 = 0;
    for (i, line) in lines.iter().enumerate().skip(start_line - 1) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1; // 1-based
                    }
                }
                _ => {}
            }
        }
    }
    lines.len() // fallback
}

// ── Naming helpers ─────────────────────────────────────────────────────────

/// `foo`, `foo_bar`, `_foo`, `foo_bar_baz42` — all lowercase, underscores ok.
fn is_snake_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    // Allow leading underscore (unused-binding convention: `_foo`)
    let check = s.trim_start_matches('_');
    check
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `Foo`, `FooBar`, `Foo42` — starts with uppercase, no underscores.
fn is_pascal_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// `FOO`, `FOO_BAR`, `FOO_BAR_42` — all uppercase, underscores ok.
fn is_screaming_snake_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

// ── Phase 2: Semantic rules ────────────────────────────────────────────────

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

/// Flag `let` bindings that carry an explicit type annotation when the type
/// is obvious from the literal initialiser.
///
/// Rule id: `unnecessary-annotation`
///
/// Examples that are flagged:
/// ```text
/// let x: Int    = 42       -- inferred as Int
/// let s: String = "hello"  -- inferred as String
/// let b: Bool   = true     -- inferred as Bool
/// let f: Float  = 3.14     -- inferred as Float
/// ```
pub fn unnecessary_annotations(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.unnecessary_annotations {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_annotations(&f.body, out);
        }
    }
}

fn check_block_annotations(block: &Block, out: &mut Vec<LintDiag>) {
    for stmt in &block.stmts {
        if let Stmt::Let {
            ty: Some(ty_expr),
            init,
            span: _,
            ..
        } = stmt
        {
            if let Some(obvious_ty) = obvious_literal_type(init) {
                if type_expr_matches_name(ty_expr, obvious_ty) {
                    let ty_span = ty_expr.span();
                    out.push(LintDiag::warning(
                        "unnecessary-annotation",
                        format!(
                            "type annotation `{obvious_ty}` is redundant — \
                             the initialiser type is unambiguous"
                        ),
                        ty_span.line,
                        ty_span.col,
                    ));
                }
            }
        }
        // Recurse
        match stmt {
            Stmt::If { then, else_, .. } => {
                check_block_annotations(then, out);
                match else_ {
                    Some(ElseBranch::Block(eb)) => check_block_annotations(eb, out),
                    Some(ElseBranch::If(inner)) => check_block_annotations(
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
                check_block_annotations(body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        check_block_annotations(b, out);
                    }
                }
            }
            Stmt::Expr {
                expr: Expr::Block(b),
                ..
            } => {
                check_block_annotations(b, out);
            }
            _ => {}
        }
    }
}

/// If `expr` is a literal whose type is always unambiguous, return the
/// canonical MVL type name; otherwise `None`.
fn obvious_literal_type(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::Literal(lit, _) => match lit {
            Literal::Integer(_) => Some("Int"),
            Literal::Float(_) => Some("Float"),
            Literal::Str(_) => Some("String"),
            Literal::Bool(_) => Some("Bool"),
            Literal::Unit => Some("Unit"),
            Literal::Char(_) => None, // Char type exists but uncommon — skip
        },
        _ => None,
    }
}

/// Return `true` if `ty_expr` is a bare base type with the given name.
fn type_expr_matches_name(ty_expr: &TypeExpr, name: &str) -> bool {
    matches!(ty_expr, TypeExpr::Base { name: n, args, .. } if n == name && args.is_empty())
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
                        f.effects.join(", "),
                    ),
                    f.span.line,
                    f.span.col,
                ));
            }
        }
    }
}

/// Return `true` if `block` (or any nested block/expression) contains at
/// least one function or method call.
fn block_has_calls(block: &Block) -> bool {
    block.stmts.iter().any(stmt_has_calls)
}

fn stmt_has_calls(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Let { init, .. } => expr_has_calls(init),
        Stmt::Assign { value, .. } => expr_has_calls(value),
        Stmt::Return { value: Some(e), .. } => expr_has_calls(e),
        Stmt::Return { value: None, .. } => false,
        Stmt::Expr { expr, .. } => expr_has_calls(expr),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_has_calls(cond)
                || block_has_calls(then)
                || else_
                    .as_ref()
                    .map(|e| match e {
                        crate::mvl::parser::ast::ElseBranch::Block(b) => block_has_calls(b),
                        crate::mvl::parser::ast::ElseBranch::If(s) => stmt_has_calls(s),
                    })
                    .unwrap_or(false)
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_has_calls(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_calls(e),
                    MatchBody::Block(b) => block_has_calls(b),
                })
        }
        Stmt::For { iter, body, .. } => expr_has_calls(iter) || block_has_calls(body),
        Stmt::While { cond, body, .. } => expr_has_calls(cond) || block_has_calls(body),
    }
}

fn expr_has_calls(expr: &Expr) -> bool {
    match expr {
        Expr::FnCall { .. } | Expr::MethodCall { .. } => true,
        Expr::Literal(..) | Expr::Ident(..) => false,
        Expr::FieldAccess { expr: e, .. } => expr_has_calls(e),
        Expr::Unary { expr: e, .. } => expr_has_calls(e),
        Expr::Binary { left, right, .. } => expr_has_calls(left) || expr_has_calls(right),
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_has_calls(cond)
                || block_has_calls(then)
                || else_.as_ref().map(|e| expr_has_calls(e)).unwrap_or(false)
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_has_calls(scrutinee)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_has_calls(e),
                    MatchBody::Block(b) => block_has_calls(b),
                })
        }
        Expr::Block(b) => block_has_calls(b),
        Expr::Lambda { body, .. } => expr_has_calls(body),
        Expr::Propagate { expr: e, .. }
        | Expr::Move { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Declassify { expr: e, .. }
        | Expr::Sanitize { expr: e, .. } => expr_has_calls(e),
        Expr::Construct { fields, .. } => fields.iter().any(|(_, e)| expr_has_calls(e)),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems.iter().any(expr_has_calls),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_has_calls(k) || expr_has_calls(v)),
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
                TypeBody::Struct(fields) => {
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

fn check_type_expr_ifc(ty: &TypeExpr, out: &mut Vec<LintDiag>) {
    match ty {
        TypeExpr::Labeled {
            label: SecurityLabel::Public,
            inner,
            span,
        } => {
            out.push(LintDiag::warning(
                "redundant-ifc-label",
                format!(
                    "`Public<{}>` is redundant — unannotated types are implicitly public; \
                     use `{}` instead",
                    type_expr_name(inner),
                    type_expr_name(inner),
                ),
                span.line,
                span.col,
            ));
            check_type_expr_ifc(inner, out);
        }
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
        TypeExpr::Tuple { elems, .. } => {
            for e in elems {
                check_type_expr_ifc(e, out);
            }
        }
        TypeExpr::IntConst { .. } => {}
    }
}

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
            let l = match label {
                SecurityLabel::Public => "Public",
                SecurityLabel::Tainted => "Tainted",
                SecurityLabel::Secret => "Secret",
                SecurityLabel::Clean => "Clean",
            };
            format!("{}<{}>", l, type_expr_name(inner))
        }
        _ => "<type>".to_string(),
    }
}

// ── Phase 3: LLM corpus quality rules ──────────────────────────────────────

/// Flag block-comment syntax (`/*`).
///
/// Rule id: `consistent-comment-style`
///
/// MVL allows only `//` line comments (and `///` doc comments). Block comments
/// from other languages are not part of the grammar; this rule catches them in
/// raw source so the lexer does not need to be extended.
pub fn consistent_comment_style(src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.consistent_comment_style {
        return;
    }
    for (i, line) in src.lines().enumerate() {
        let line_no = (i + 1) as u32;
        // Scan for `/*` not inside a line comment. If `//` appears before `/*`
        // on the same line, the `/*` is inside a comment and must be ignored.
        // Known limitation: `/*` inside string literals is still flagged; the
        // parser rejects such source anyway, so false positives are rare.
        if let Some(pos) = line.find("/*") {
            let in_line_comment = line.find("//").is_some_and(|cc| cc < pos);
            if !in_line_comment {
                out.push(LintDiag::warning(
                    "consistent-comment-style",
                    "block comment `/*` not allowed; use `//` line comments",
                    line_no,
                    (pos + 1) as u32,
                ));
            }
        }
    }
}

/// Require `///` doc comments on every public function and type declaration.
///
/// Rule id: `missing-doc-comment`
///
/// Because the lexer discards comments, this rule correlates AST span line
/// numbers with raw source text: a declaration is considered documented if one
/// or more `///` lines appear immediately above it (blank lines between the
/// comment and the declaration are allowed; a non-comment, non-blank line
/// breaks the block).
pub fn doc_comments_required(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.require_doc_comments {
        return;
    }
    let src_lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(f) if f.visible => {
                if !has_doc_comment_before(f.span.line as usize, &src_lines) {
                    out.push(LintDiag::warning(
                        "missing-doc-comment",
                        format!(
                            "public function `{}` is missing a doc comment (`///`)",
                            f.name
                        ),
                        f.span.line,
                        f.span.col,
                    ));
                }
            }
            Decl::Type(t) if t.visible => {
                if !has_doc_comment_before(t.span.line as usize, &src_lines) {
                    out.push(LintDiag::warning(
                        "missing-doc-comment",
                        format!("public type `{}` is missing a doc comment (`///`)", t.name),
                        t.span.line,
                        t.span.col,
                    ));
                }
            }
            Decl::Const(c) if c.visible => {
                if !has_doc_comment_before(c.span.line as usize, &src_lines) {
                    out.push(LintDiag::warning(
                        "missing-doc-comment",
                        format!("public const `{}` is missing a doc comment (`///`)", c.name),
                        c.span.line,
                        c.span.col,
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Recommend an `Example:` section inside doc-comment blocks on public items.
///
/// Rule id: `doc-comment-example`
///
/// This rule is opt-in (`doc_comment_examples = false` by default). When
/// enabled it emits a warning for every public function or type whose doc
/// comment block does not contain an `Example:` or `# Example` line.
pub fn doc_comment_examples(prog: &Program, src: &str, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.doc_comment_examples {
        return;
    }
    let src_lines: Vec<&str> = src.lines().collect();
    for decl in &prog.declarations {
        // Only pub fn and pub type are checked; pub const is intentionally
        // excluded (example sections are less meaningful for constants).
        let Some((span, kind, name)) = (match decl {
            Decl::Fn(f) if f.visible => Some((f.span, "function", f.name.as_str())),
            Decl::Type(t) if t.visible => Some((t.span, "type", t.name.as_str())),
            _ => None,
        }) else {
            continue;
        };
        let doc_lines = collect_doc_lines_before(span.line as usize, &src_lines);
        if doc_lines.is_empty() {
            // missing-doc-comment will fire; skip duplicate noise here
            continue;
        }
        let has_example = doc_lines.iter().any(|l| {
            let lower = l.trim_start_matches('/').trim().to_ascii_lowercase();
            lower.starts_with("example") || lower.starts_with("# example")
        });
        if !has_example {
            out.push(LintDiag::warning(
                "doc-comment-example",
                format!(
                    "public {kind} `{name}` doc comment has no `Example:` section (recommended)"
                ),
                span.line,
                span.col,
            ));
        }
    }
}

// ── Phase 3 helpers ─────────────────────────────────────────────────────────

/// Returns `true` if the source line immediately preceding `decl_line`
/// (1-based) belongs to a `///` doc-comment block.
///
/// Blank lines between the comment block and the declaration are skipped.
/// A regular `//` comment (not `///`) does **not** count as documentation.
fn has_doc_comment_before(decl_line: usize, src_lines: &[&str]) -> bool {
    !collect_doc_lines_before(decl_line, src_lines).is_empty()
}

/// Collect all `///` lines from the comment block immediately above
/// `decl_line` (1-based). Returns an empty vec if none are found.
fn collect_doc_lines_before<'a>(decl_line: usize, src_lines: &[&'a str]) -> Vec<&'a str> {
    if decl_line == 0 || decl_line > src_lines.len() {
        return vec![];
    }
    // Walk backwards from the line immediately above the declaration.
    // decl_line is 1-based, so the line above is at 0-based index decl_line - 2,
    // meaning we iterate over 0..decl_line-1 in reverse.
    let mut result: Vec<&'a str> = vec![];
    for i in (0..decl_line.saturating_sub(1)).rev() {
        let line = src_lines[i].trim();
        if line.starts_with("///") {
            result.push(src_lines[i]);
        } else if line.is_empty() {
            // blank lines between doc block and declaration are allowed
            continue;
        } else {
            break;
        }
    }
    result
}

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
        | Expr::Move { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Declassify { expr: e, .. }
        | Expr::Sanitize { expr: e, .. } => cyclomatic_complexity_expr(e),
        Expr::Construct { fields, .. } => fields
            .iter()
            .map(|(_, e)| cyclomatic_complexity_expr(e))
            .sum(),
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
        | Expr::Move { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Declassify { expr: e, .. }
        | Expr::Sanitize { expr: e, .. } => max_match_depth_expr(e, depth),
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
        Expr::Construct { fields, .. } => fields
            .iter()
            .map(|(_, e)| max_match_depth_expr(e, depth))
            .max()
            .unwrap_or(depth),
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
pub fn complexity_effect_width(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if cfg.max_effect_signature_width == 0 {
        return;
    }
    for decl in &prog.declarations {
        let (name, effects, span) = match decl {
            Decl::Fn(f) => (f.name.as_str(), &f.effects, f.span),
            _ => continue,
        };
        if effects.len() > cfg.max_effect_signature_width {
            out.push(LintDiag::warning(
                "complexity-effect-width",
                format!(
                    "function `{name}` declares {} effects [{}] (max {})",
                    effects.len(),
                    effects.join(", "),
                    cfg.max_effect_signature_width
                ),
                span.line,
                span.col,
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::linter::config::LintConfig;
    use crate::mvl::parser::Parser;

    fn cfg() -> LintConfig {
        LintConfig::default()
    }

    fn parse(src: &str) -> crate::mvl::parser::ast::Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // -- trailing_whitespace --

    #[test]
    fn trailing_ws_detected() {
        let src = "fn foo() -> Int { 1 }   \nfn bar() -> Int { 2 }\n";
        let mut diags = vec![];
        trailing_whitespace(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "trailing-whitespace");
        assert_eq!(diags[0].span.line, 1);
    }

    #[test]
    fn trailing_ws_clean() {
        let src = "fn foo() -> Int { 1 }\n";
        let mut diags = vec![];
        trailing_whitespace(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn trailing_ws_disabled() {
        let src = "fn foo() -> Int { 1 }   \n";
        let mut diags = vec![];
        let mut c = cfg();
        c.trailing_ws = false;
        trailing_whitespace(src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- line_length --

    #[test]
    fn line_length_over_limit() {
        let long = "x".repeat(121);
        let src = format!("fn foo() -> Int {{\n    {long}\n}}\n");
        let mut diags = vec![];
        line_length(&src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "line-length");
        assert_eq!(diags[0].span.line, 2);
    }

    #[test]
    fn line_length_at_limit_is_ok() {
        let exactly = "x".repeat(120);
        let src = format!("{exactly}\n");
        let mut diags = vec![];
        line_length(&src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- indentation --

    #[test]
    fn mixed_indent_detected() {
        let src = "fn foo() {\n\t    x\n}\n"; // tab + spaces
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert!(diags.iter().any(|d| d.rule == "indentation"));
    }

    #[test]
    fn tab_indent_when_spaces_expected() {
        let src = "fn foo() {\n\tx\n}\n";
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("tab"));
    }

    #[test]
    fn non_multiple_indent() {
        let src = "fn foo() {\n   x\n}\n"; // 3 spaces, not multiple of 4
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("multiple"));
    }

    #[test]
    fn correct_indent_clean() {
        let src = "fn foo() {\n    x\n}\n"; // 4 spaces
        let mut diags = vec![];
        indentation(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // -- naming --

    #[test]
    fn naming_snake_case_fn_ok() {
        assert!(is_snake_case("foo_bar"));
        assert!(is_snake_case("_unused"));
        assert!(is_snake_case("foo42"));
    }

    #[test]
    fn naming_snake_case_fn_bad() {
        assert!(!is_snake_case("FooBar"));
        assert!(!is_snake_case("fooBar"));
    }

    #[test]
    fn naming_pascal_case_ok() {
        assert!(is_pascal_case("FooBar"));
        assert!(is_pascal_case("Foo42"));
    }

    #[test]
    fn naming_pascal_case_bad() {
        assert!(!is_pascal_case("foo_bar"));
        assert!(!is_pascal_case("fooBar"));
        assert!(!is_pascal_case("Foo_Bar"));
    }

    #[test]
    fn naming_screaming_snake_ok() {
        assert!(is_screaming_snake_case("FOO_BAR"));
        assert!(is_screaming_snake_case("MAX_LEN"));
    }

    #[test]
    fn naming_screaming_snake_bad() {
        assert!(!is_screaming_snake_case("foo_bar"));
        assert!(!is_screaming_snake_case("FooBar"));
    }

    // ── Phase 2 tests ─────────────────────────────────────────────────────

    // -- unreachable_code --

    #[test]
    fn unreachable_after_return_detected() {
        let src = "fn f() -> Int { return 1; let x = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unreachable_code(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1, "expected 1 unreachable diagnostic");
        assert_eq!(diags[0].rule, "unreachable-code");
    }

    #[test]
    fn unreachable_code_disabled() {
        let src = "fn f() -> Int { return 1; let x = 2; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.unreachable_code = false;
        unreachable_code(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn reachable_code_clean() {
        let src = "fn f() -> Int { let x = 1; return x }\n";
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

    // -- unnecessary_annotations --

    #[test]
    fn int_annotation_on_int_literal_detected() {
        let src = "fn f() -> Unit { let x: Int = 42; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-annotation");
        assert!(diags[0].message.contains("Int"));
    }

    #[test]
    fn bool_annotation_on_bool_literal_detected() {
        let src = "fn f() -> Unit { let b: Bool = true; b }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-annotation");
    }

    #[test]
    fn annotation_on_non_literal_clean() {
        let src = "fn f(x: Int) -> Unit { let y: Int = x; y }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
        assert!(
            diags.is_empty(),
            "binding to variable, not literal — should be clean"
        );
    }

    #[test]
    fn no_annotation_clean() {
        let src = "fn f() -> Unit { let x = 42; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
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
    fn public_label_on_param_detected() {
        let src = "fn f(x: Public<Int>) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-ifc-label");
        assert!(diags[0].message.contains("Public"));
    }

    #[test]
    fn secret_label_on_param_clean() {
        let src = "fn f(x: Secret<Int>) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_return_type_detected() {
        let src = "fn f() -> Public<String> { \"hi\" }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-ifc-label");
    }

    #[test]
    fn redundant_ifc_disabled() {
        let src = "fn f(x: Public<Int>) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.redundant_ifc_labels = false;
        redundant_ifc_labels(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn public_label_on_struct_field_detected() {
        let src = "type Wrapper = struct { data: Public<Int> }\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-ifc-label");
    }

    #[test]
    fn public_label_in_type_alias_detected() {
        let src = "type MyInt = Public<Int>\n";
        let prog = parse(src);
        let mut diags = vec![];
        redundant_ifc_labels(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "redundant-ifc-label");
    }

    // -- redundant_match: missing config-disable test --

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

    // -- unnecessary_annotations: missing literal types and config-disable --

    #[test]
    fn string_annotation_on_str_literal_detected() {
        let src = "fn f() -> Unit { let s: String = \"hello\"; s }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-annotation");
        assert!(diags[0].message.contains("String"));
    }

    #[test]
    fn float_annotation_on_float_literal_detected() {
        let src = "fn f() -> Unit { let x: Float = 3.14; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        unnecessary_annotations(&prog, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "unnecessary-annotation");
        assert!(diags[0].message.contains("Float"));
    }

    #[test]
    fn unnecessary_annotations_disabled() {
        let src = "fn f() -> Unit { let x: Int = 42; x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.unnecessary_annotations = false;
        unnecessary_annotations(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- redundant_effects: missing config-disable test --

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

    // ── Phase 3: consistent_comment_style ──────────────────────────────

    #[test]
    fn block_comment_detected() {
        let src = "/* this is illegal */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "consistent-comment-style");
        assert_eq!(diags[0].span.line, 1);
    }

    #[test]
    fn block_comment_mid_line_detected() {
        let src = "fn f() -> Int { 42 } /* whoops */\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "consistent-comment-style");
        assert_eq!(diags[0].span.col, 22);
    }

    #[test]
    fn line_comment_clean() {
        let src = "// ok\n/// doc\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn consistent_comment_style_disabled() {
        let src = "/* block */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        let mut c = cfg();
        c.consistent_comment_style = false;
        consistent_comment_style(src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn block_comment_after_line_comment_not_flagged() {
        // `/*` appearing after `//` on the same line is inside a line comment.
        let src = "// this is fine /* not a block comment */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn block_comment_multiple_on_same_line_single_diag() {
        // find() stops at the first match; only one diag per line is emitted.
        let src = "/* a */ /* b */\nfn f() -> Int { 42 }\n";
        let mut diags = vec![];
        consistent_comment_style(src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.col, 1); // only first occurrence reported
    }

    // ── Phase 3: doc_comments_required ─────────────────────────────────

    #[test]
    fn pub_fn_missing_doc_comment_detected() {
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("foo"));
    }

    #[test]
    fn pub_fn_with_doc_comment_ok() {
        let src = "/// Does something.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn private_fn_no_doc_comment_ok() {
        let src = "fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_type_missing_doc_comment_detected() {
        let src = "pub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("Foo"));
    }

    #[test]
    fn pub_type_with_doc_comment_ok() {
        let src = "/// A wrapper type.\npub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_blank_line_between_ok() {
        // A blank line between doc comment and declaration is allowed.
        let src = "/// Docs here.\n\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn regular_comment_not_doc_comment() {
        // `//` is not `///`; should still flag.
        let src = "// not a doc comment\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
    }

    #[test]
    fn require_doc_comments_disabled() {
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.require_doc_comments = false;
        doc_comments_required(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_const_missing_doc_comment_detected() {
        let src = "pub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "missing-doc-comment");
        assert!(diags[0].message.contains("MAX"));
    }

    #[test]
    fn pub_const_with_doc_comment_ok() {
        let src = "/// The maximum value.\npub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn private_const_no_doc_comment_ok() {
        let src = "const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comments_required(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    // ── Phase 3: doc_comment_examples ──────────────────────────────────

    #[test]
    fn pub_fn_doc_without_example_flagged() {
        let src = "/// Does something.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "doc-comment-example");
        assert!(diags[0].message.contains("foo"));
    }

    #[test]
    fn pub_fn_doc_with_example_ok() {
        let src = "/// Does something.\n/// Example: foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_examples_disabled_by_default() {
        // default config has doc_comment_examples = false
        let src = "/// No example.\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        doc_comment_examples(&prog, src, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_fn_no_doc_skipped_for_examples() {
        // If there's no doc comment at all, missing-doc-comment fires but
        // doc-comment-example should stay silent to avoid duplicate noise.
        let src = "pub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_type_doc_without_example_flagged() {
        let src = "/// A wrapper type.\npub type Foo = struct { x: Int }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].rule, "doc-comment-example");
        assert!(diags[0].message.contains("Foo"));
        assert!(diags[0].message.contains("type"));
    }

    #[test]
    fn doc_comment_example_case_insensitive_ok() {
        // "# Example" (capital E) and "Examples:" (plural) both accepted.
        let src =
            "/// Does something.\n/// # Example\n/// foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn doc_comment_examples_plural_ok() {
        let src = "/// Does something.\n/// Examples: foo() == 42\npub fn foo() -> Int { 42 }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn pub_const_no_example_not_flagged() {
        // doc_comment_examples intentionally excludes pub const; pin this design decision.
        let src = "/// The maximum value.\npub const MAX: Int = 100;\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.doc_comment_examples = true;
        doc_comment_examples(&prog, src, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // ── Phase 4: complexity rules ──────────────────────────────────────

    // -- complexity_cyclomatic --

    #[test]
    fn cyclomatic_simple_fn_clean() {
        // CC = 1 (no branches)
        let src = "fn f(x: Int) -> Int { x }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_if_increments() {
        // CC = 1 + 1 (if) = 2, well within default 10
        let src = "fn f(x: Int) -> Int { if x > 0 { x } else { 0 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_exceeds_threshold() {
        // Build a function with CC > 10 via nested ifs
        let src = r#"fn complex(x: Int) -> Int {
    if x > 1 {
        if x > 2 {
            if x > 3 {
                if x > 4 {
                    if x > 5 {
                        if x > 6 {
                            if x > 7 {
                                if x > 8 {
                                    if x > 9 {
                                        if x > 10 {
                                            x
                                        } else { 0 }
                                    } else { 0 }
                                } else { 0 }
                            } else { 0 }
                        } else { 0 }
                    } else { 0 }
                } else { 0 }
            } else { 0 }
        } else { 0 }
    } else { 0 }
}
"#;
        let prog = parse(src);
        let mut diags = vec![];
        complexity_cyclomatic(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-cyclomatic"),
            "expected cyclomatic-complexity warning, got: {diags:?}"
        );
    }

    #[test]
    fn cyclomatic_disabled() {
        let src = r#"fn f(x: Int) -> Int {
    if x > 1 { if x > 2 { if x > 3 { if x > 4 { if x > 5 {
    if x > 6 { if x > 7 { if x > 8 { if x > 9 { if x > 10 {
        x } else { 0 } } else { 0 } } else { 0 } } else { 0 } } else { 0 }
    } else { 0 } } else { 0 } } else { 0 } } else { 0 } } else { 0 }
}
"#;
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_cyclomatic_complexity = 0;
        complexity_cyclomatic(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn cyclomatic_match_arms_contribute() {
        // 5-arm match → CC = 1 + 4 = 5
        let src = "type D = enum { A, B, C, D, E }\nfn f(d: D) -> Int { match d { D::A => 1 D::B => 2 D::C => 3 D::D => 4 D::E => 5 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_cyclomatic_complexity = 3; // lower threshold to trigger
        complexity_cyclomatic(&prog, &c, &mut diags);
        assert!(diags.iter().any(|d| d.rule == "complexity-cyclomatic"));
    }

    // -- complexity_match_depth --

    #[test]
    fn match_depth_single_match_clean() {
        // depth = 1, default max = 3
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int { match c { C::X => 1 C::Y => 2 } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_match_depth(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn match_depth_exceeds_threshold() {
        // depth = 4 > max 3
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int {\n    match c {\n        C::X => match c { C::X => match c { C::X => match c { C::X => 1 C::Y => 2 } C::Y => 3 } C::Y => 4 }\n        C::Y => 0\n    }\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_match_depth(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-match-depth"),
            "expected match-depth warning, got: {diags:?}"
        );
    }

    #[test]
    fn match_depth_disabled() {
        let src = "type C = enum { X, Y }\nfn f(c: C) -> Int {\n    match c { C::X => match c { C::X => match c { C::X => match c { C::X => 1 C::Y => 2 } C::Y => 3 } C::Y => 4 } C::Y => 0 }\n}\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_nested_match_depth = 0;
        complexity_match_depth(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_effect_width --

    #[test]
    fn effect_width_within_limit_clean() {
        let src = "fn f() -> Unit ! Console { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_effect_width(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn effect_width_exceeds_threshold() {
        // 4 effects > default max 3
        let src = "fn f() -> Unit ! Console, DB, Network, FileSystem { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_effect_width(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-effect-width"),
            "expected effect-width warning, got: {diags:?}"
        );
    }

    #[test]
    fn effect_width_disabled() {
        let src = "fn f() -> Unit ! Console, DB, Network, FileSystem { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_effect_signature_width = 0;
        complexity_effect_width(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_trait_impl_count --

    #[test]
    fn trait_impl_count_within_limit_clean() {
        let src = "type Foo = struct { x: Int }\nimpl Display for Foo { fn fmt(t: Foo) -> String { \"foo\" } }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_trait_impl_count(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn trait_impl_count_exceeds_threshold() {
        let src = concat!(
            "type T = struct { x: Int }\n",
            "impl A for T { fn a(t: T) -> Int { 1 } }\n",
            "impl B for T { fn b(t: T) -> Int { 2 } }\n",
            "impl C for T { fn c(t: T) -> Int { 3 } }\n",
            "impl D for T { fn d(t: T) -> Int { 4 } }\n",
            "impl E for T { fn e(t: T) -> Int { 5 } }\n",
            "impl F for T { fn f(t: T) -> Int { 6 } }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_trait_impl_count(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "complexity-trait-impl-count"),
            "expected trait-impl-count warning, got: {diags:?}"
        );
    }

    #[test]
    fn trait_impl_count_disabled() {
        let src = concat!(
            "type T = struct { x: Int }\n",
            "impl A for T { fn a(t: T) -> Int { 1 } }\n",
            "impl B for T { fn b(t: T) -> Int { 2 } }\n",
            "impl C for T { fn c(t: T) -> Int { 3 } }\n",
            "impl D for T { fn d(t: T) -> Int { 4 } }\n",
            "impl E for T { fn e(t: T) -> Int { 5 } }\n",
            "impl F for T { fn f(t: T) -> Int { 6 } }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_trait_impl_count = 0;
        complexity_trait_impl_count(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_module_fanout --

    #[test]
    fn module_fanout_within_limit_clean() {
        // Both imports from "std" → fanout = 1, well within default 15
        let src = "use std.io.{File, Read}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        complexity_module_fanout(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn module_fanout_exceeds_threshold() {
        // 3 distinct root modules (a, b, c), threshold 2
        let src = "use a.{Foo}\nuse b.{Bar}\nuse c.{Baz}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_module_fanout = 2;
        complexity_module_fanout(&prog, &c, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-module-fanout"),
            "expected module-fanout warning, got: {diags:?}"
        );
    }

    #[test]
    fn module_fanout_disabled() {
        let src = "use a.{Foo}\nuse b.{Bar}\nuse c.{Baz}\nfn f() -> Unit { f() }\n";
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_module_fanout = 0;
        complexity_module_fanout(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }

    // -- complexity_extern_ratio --

    #[test]
    fn extern_ratio_clean() {
        // 1 extern fn, 4 total → 25% ≤ 20%? No, so let's use 1 extern / 10 total
        let src = concat!(
            "extern \"rust\" { fn ext() -> Int }\n",
            "fn a() -> Int { 1 }\nfn b() -> Int { 2 }\nfn c() -> Int { 3 }\n",
            "fn d() -> Int { 4 }\nfn e() -> Int { 5 }\nfn g() -> Int { 6 }\n",
            "fn h() -> Int { 7 }\nfn i() -> Int { 8 }\nfn j() -> Int { 9 }\n",
        );
        // 1 extern / 10 total = 10% <= 20%
        let prog = parse(src);
        let mut diags = vec![];
        complexity_extern_ratio(&prog, &cfg(), &mut diags);
        assert!(diags.is_empty());
    }

    #[test]
    fn extern_ratio_exceeds_threshold() {
        // 3 extern fns / 4 total = 75% > 20%
        let src = concat!(
            "extern \"rust\" { fn e1() -> Int fn e2() -> Int fn e3() -> Int }\n",
            "fn native() -> Int { 1 }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        complexity_extern_ratio(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "complexity-extern-ratio"),
            "expected extern-ratio warning, got: {diags:?}"
        );
    }

    #[test]
    fn extern_ratio_disabled() {
        let src = concat!(
            "extern \"rust\" { fn e1() -> Int fn e2() -> Int }\n",
            "fn native() -> Int { 1 }\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        let mut c = cfg();
        c.max_extern_ratio = 0.0;
        complexity_extern_ratio(&prog, &c, &mut diags);
        assert!(diags.is_empty());
    }
}
