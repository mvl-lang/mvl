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
    Block, Decl, Expr, Literal, Pattern, Program, SecurityLabel, Stmt, TypeBody, TypeExpr,
    VariantFields,
};

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
                (cfg.line_length + 1) as u32,
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
                else_: Some(crate::mvl::parser::ast::ElseBranch::Block(eb)),
                ..
            } => {
                check_block_unreachable(then, out);
                check_block_unreachable(eb, out);
            }
            Stmt::If {
                then,
                else_: Some(crate::mvl::parser::ast::ElseBranch::If(inner)),
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
                    if let crate::mvl::parser::ast::MatchBody::Block(b) = &arm.body {
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
                if arms.len() == 1 && is_irrefutable(&arms[0].pattern) {
                    out.push(LintDiag::warning(
                        "redundant-match",
                        format!(
                            "single-arm `match` with irrefutable pattern — use `let` instead: \
                             `let {} = {}`",
                            pattern_display(&arms[0].pattern),
                            expr_display(scrutinee),
                        ),
                        span.line,
                        span.col,
                    ));
                }
                // Recurse into arm bodies
                for arm in arms {
                    if let crate::mvl::parser::ast::MatchBody::Block(b) = &arm.body {
                        check_block_redundant_match(b, out);
                    }
                }
            }
            Stmt::If { then, else_, .. } => {
                check_block_redundant_match(then, out);
                if let Some(crate::mvl::parser::ast::ElseBranch::Block(eb)) = else_ {
                    check_block_redundant_match(eb, out);
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_redundant_match(body, out);
            }
            _ => {}
        }
        // Also check match expressions in statement position
        if let Stmt::Expr {
            expr:
                Expr::Match {
                    scrutinee,
                    arms,
                    span,
                },
            ..
        } = stmt
        {
            if arms.len() == 1 && is_irrefutable(&arms[0].pattern) {
                out.push(LintDiag::warning(
                    "redundant-match",
                    format!(
                        "single-arm `match` with irrefutable pattern — use `let` instead: \
                         `let {} = {}`",
                        pattern_display(&arms[0].pattern),
                        expr_display(scrutinee),
                    ),
                    span.line,
                    span.col,
                ));
            }
        }
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
            span,
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
                    let _ = span;
                }
            }
        }
        // Recurse
        match stmt {
            Stmt::If { then, else_, .. } => {
                check_block_annotations(then, out);
                if let Some(crate::mvl::parser::ast::ElseBranch::Block(eb)) = else_ {
                    check_block_annotations(eb, out);
                }
            }
            Stmt::For { body, .. } | Stmt::While { body, .. } => {
                check_block_annotations(body, out);
            }
            Stmt::Match { arms, .. } => {
                for arm in arms {
                    if let crate::mvl::parser::ast::MatchBody::Block(b) = &arm.body {
                        check_block_annotations(b, out);
                    }
                }
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
                    crate::mvl::parser::ast::MatchBody::Expr(e) => expr_has_calls(e),
                    crate::mvl::parser::ast::MatchBody::Block(b) => block_has_calls(b),
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
                    crate::mvl::parser::ast::MatchBody::Expr(e) => expr_has_calls(e),
                    crate::mvl::parser::ast::MatchBody::Block(b) => block_has_calls(b),
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
                        if let VariantFields::Struct(fields) = &variant.fields {
                            for field in fields {
                                check_type_expr_ifc(&field.ty, out);
                            }
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
}
