// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Refactoring-suggestion rules — detect anti-patterns and suggest better alternatives.

use std::collections::HashMap;

use crate::mvl::linter::{config::LintConfig, errors::LintDiag};
use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, Expr, LValue, Literal, MatchArm, MatchBody, Pattern, Program, Stmt,
};
use crate::mvl::parser::visit::Visit;

/// Error on the `while / .get(i) / match / None => ()` anti-pattern (#705).
///
/// Rule id: `for-iter-antipattern`
///
/// The pattern:
///
/// ```mvl
/// let i: ref Int = 0;
/// while i < xs.len() {
///     match xs.get(i) {
///         None    => (),
///         Some(x) => { ... }
///     }
///     i = i + 1
/// }
/// ```
///
/// is never correct when iterating a `List[T]`.  `for x in xs { ... }` is
/// always equivalent, safer (no off-by-one risk), and more readable.
/// The `None => ()` arm is a false branch that only satisfies exhaustiveness.
///
/// **Detection:** a `while` whose direct body contains a `match` on
/// `<expr>.get(<args>)` where any arm is `None => ()`.
///
/// **Escape hatch:** if the `None` arm contains real logic (not just `()`),
/// the rule is silent — the user is deliberately handling a missing element.
pub fn for_iter_antipattern(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.for_iter_antipattern {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_for_iter_antipattern(&f.body, out);
        }
    }
}

fn check_block_for_iter_antipattern(block: &Block, out: &mut Vec<LintDiag>) {
    ForIterAntipattern { out }.visit_block(block);
}

struct ForIterAntipattern<'a> {
    out: &'a mut Vec<LintDiag>,
}

impl<'ast> Visit<'ast> for ForIterAntipattern<'_> {
    fn visit_block(&mut self, b: &'ast Block) {
        // Intentional: walks stmts directly to match only direct children of while bodies.
        // Default walk_block would lose the "direct child" constraint needed for the pattern.
        for stmt in &b.stmts {
            match stmt {
                Stmt::While { body, span, .. } => {
                    // Check direct children of the while body for the anti-pattern.
                    for inner in &body.stmts {
                        if let Stmt::Match {
                            scrutinee, arms, ..
                        } = inner
                        {
                            if is_get_call(scrutinee) && has_none_unit_arm(arms) {
                                self.out.push(LintDiag::error(
                                    "for-iter-antipattern",
                                    "use `for x in list { }` for List[T] iteration; \
                                     `while/.get(i)/match/None=>()` is not allowed",
                                    span.line,
                                    span.col,
                                ));
                                break;
                            }
                        }
                    }
                    self.visit_block(body);
                }
                Stmt::For { body, .. } | Stmt::If { then: body, .. } => {
                    self.visit_block(body);
                }
                _ => {}
            }
        }
    }
}

/// Returns `true` if `expr` is a method call to `.get(...)`.
fn is_get_call(expr: &Expr) -> bool {
    matches!(expr, Expr::MethodCall { method, .. } if method == "get")
}

/// Returns `true` if any arm of the match has pattern `None` and body `()`.
fn has_none_unit_arm(arms: &[MatchArm]) -> bool {
    arms.iter().any(|arm| {
        matches!(arm.pattern, Pattern::None(_))
            && matches!(&arm.body, MatchBody::Expr(Expr::Literal(Literal::Unit, _)))
    })
}

/// Warn on counter-increment while loops that can be rewritten as `for i in range()`.
///
/// Rule id: `while-to-for-range`
///
/// Detects the pattern:
/// ```mvl
/// let i: ref Int = 0;
/// while i < n {
///     // ...
///     i = i + 1
/// }
/// ```
/// and suggests: `for i in range(0, n)` which uses `range`'s `decreases` clause
/// and is therefore provably total.
///
/// **Escape hatch:** loops with an explicit `decreases` clause are already
/// annotated and are silently skipped.
pub fn while_to_for_range(prog: &Program, cfg: &LintConfig, out: &mut Vec<LintDiag>) {
    if !cfg.while_to_for_range {
        return;
    }
    for decl in &prog.declarations {
        if let Decl::Fn(f) = decl {
            check_block_for_while_range(&f.body, out);
        }
    }
}

fn check_block_for_while_range(block: &Block, out: &mut Vec<LintDiag>) {
    WhileToForRange { out }.visit_block(block);
}

struct WhileToForRange<'a> {
    out: &'a mut Vec<LintDiag>,
}

impl<'ast> Visit<'ast> for WhileToForRange<'_> {
    fn visit_block(&mut self, b: &'ast Block) {
        // Intentional: walks stmts directly to maintain a per-scope let_inits map.
        // Default walk_block discards block boundaries, losing the scoping needed here.
        // Fresh let-init map per block scope — mirrors the original per-call HashMap.
        let mut let_inits: HashMap<String, String> = HashMap::new();
        for stmt in &b.stmts {
            match stmt {
                Stmt::Let {
                    pattern: Pattern::Ident(name, _),
                    init,
                    ..
                } => {
                    let_inits.insert(name.clone(), simple_expr_str(init));
                }
                Stmt::While {
                    cond,
                    decreases,
                    body,
                    span,
                    ..
                } => {
                    if decreases.is_none() {
                        if let Some((var, end)) = counter_lt_cond(cond) {
                            if is_counter_increment(body, &var) {
                                let start = let_inits.get(&var).map(String::as_str).unwrap_or("0");
                                self.out.push(LintDiag::warning(
                                    "while-to-for-range",
                                    format!(
                                        "`while {var} < {end}` counter loop — use \
                                         `for {var} in range({start}, {end})` for a \
                                         provably-terminating loop",
                                    ),
                                    span.line,
                                    span.col,
                                ));
                            }
                        }
                    }
                    self.visit_block(body);
                }
                Stmt::For { body, .. } | Stmt::If { then: body, .. } => {
                    self.visit_block(body);
                }
                _ => {}
            }
        }
    }
}

/// If `expr` is `VAR < END`, return `(var_name, end_repr)`.
fn counter_lt_cond(expr: &Expr) -> Option<(String, String)> {
    if let Expr::Binary {
        op: BinaryOp::Lt,
        left,
        right,
        ..
    } = expr
    {
        if let Expr::Ident(name, _) = left.as_ref() {
            return Some((name.clone(), simple_expr_str(right)));
        }
    }
    None
}

/// Return `true` if the last statement in `block` is `var = var + N`.
fn is_counter_increment(block: &Block, var: &str) -> bool {
    match block.stmts.last() {
        Some(Stmt::Assign { target, value, .. }) => {
            if let LValue::Ident(name, _) = target {
                if name == var {
                    if let Expr::Binary {
                        op: BinaryOp::Add,
                        left,
                        ..
                    } = value
                    {
                        if let Expr::Ident(n, _) = left.as_ref() {
                            return n == var;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Format simple expressions for diagnostic messages.
fn simple_expr_str(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::Literal(Literal::Integer(n), _) => n.to_string(),
        _ => "_".to_string(),
    }
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

    // -- while_to_for_range --

    #[test]
    fn while_to_for_range_fires_on_counter_loop() {
        // classic counter pattern must warn with default config
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    let s: ref Int = 0;\n",
            "    while i < n {\n",
            "        s = s + i;\n",
            "        i = i + 1\n",
            "    }\n",
            "    s\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "while-to-for-range" && d.message.contains("range(0, n)")),
            "expected while-to-for-range for counter loop; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_silent_with_decreases() {
        // while with decreases is already total — must not warn
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n decreases n - i {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "while with decreases must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_silent_without_increment() {
        // while with no VAR=VAR+N increment in last position — not the pattern
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1;\n",
            "        let x: Int = 0;\n",
            "        x\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "increment not in last position must not warn; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_suggestion_shows_start() {
        // start value from let binding must appear in suggestion
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 3;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg(), &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "while-to-for-range" && d.message.contains("range(3, n)")),
            "suggestion must include start value; got: {diags:?}"
        );
    }

    #[test]
    fn while_to_for_range_off_when_disabled() {
        // rule can be opted out via config
        let cfg_off = LintConfig {
            while_to_for_range: false,
            ..LintConfig::default()
        };
        let src = concat!(
            "fn f(n: Int) -> Int {\n",
            "    let i: ref Int = 0;\n",
            "    while i < n {\n",
            "        i = i + 1\n",
            "    }\n",
            "    i\n",
            "}\n",
        );
        let prog = parse(src);
        let mut diags = vec![];
        while_to_for_range(&prog, &cfg_off, &mut diags);
        assert!(
            diags.iter().all(|d| d.rule != "while-to-for-range"),
            "rule must be silent when disabled; got: {diags:?}"
        );
    }
}
