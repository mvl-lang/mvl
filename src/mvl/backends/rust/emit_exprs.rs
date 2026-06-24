// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust expressions from MVL [`TirExpr`] nodes.

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_stmts::{emit_mcdc_guard_block, scrutinee_needs_clone};
use crate::mvl::backends::rust::emit_types::{emit_ty, emit_type_expr};
use crate::mvl::backends::rust::mcdc_instr::DecisionKind;
use crate::mvl::ir::{
    BinaryOp, Literal, Pattern, TirExpr, TirExprKind, TirMatchArm, TirMatchBody, TirStmt, Ty,
    UnaryOp,
};
use crate::mvl::passes::coverage::BranchKind;
use crate::mvl::passes::mcdc::analysis::count_clauses_ref;

impl RustEmitter {
    /// Emit an expression into the code buffer (no trailing newline).
    pub fn emit_expr(&mut self, expr: &TirExpr) {
        let span = expr.span;
        match &expr.kind {
            TirExprKind::Literal(lit) => {
                // Mutation mode: inject env-var dispatch for Bool and Integer literals.
                if self.mutation.is_some() {
                    match lit {
                        Literal::Bool(b) => {
                            if let Some(mid) = self.alloc_bool_mutation(*b, span.line) {
                                let (alt, orig) = if *b {
                                    ("false", "true")
                                } else {
                                    ("true", "false")
                                };
                                self.push(&format!(
                                    r#"{{ match ::std::env::var("MVL_MUTANT").as_deref() {{ Ok("{mid}") => {alt}, _ => {orig} }} }}"#
                                ));
                                return;
                            }
                        }
                        Literal::Integer(n) => {
                            if let Some(int_variants) = self.alloc_int_mutations(*n, span.line) {
                                self.push("{ match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                                for (mid, alt) in &int_variants {
                                    self.push(&format!(" Ok(\"{mid}\") => {alt},"));
                                }
                                self.push(&format!(" _ => {n} }}"));
                                self.push(" }");
                                return;
                            }
                        }
                        _ => {}
                    }
                }
                self.emit_literal(lit);
            }
            TirExprKind::Var(name) => {
                // #928: in free-function extension method bodies, `self` → `self_`.
                if name == "self" && self.self_as_free_param {
                    self.push("self_");
                } else {
                    self.push(&map_ident(name));
                }
            }
            TirExprKind::FieldAccess { expr: inner, field } => {
                self.emit_expr(inner);
                self.push(".");
                self.push(field);
            }
            TirExprKind::MethodCall {
                receiver,
                method,
                args,
            } => {
                self.emit_method_call(receiver, method.as_str(), args);
            }
            TirExprKind::FnCall {
                name,
                type_args,
                args,
            } => {
                // panic! is a Rust macro: first arg must be a bare string literal.
                // format → mvl_format to avoid collision with Rust's format! macro (#901).
                if name.as_str() == "format" {
                    self.push("mvl_format(");
                    self.emit_args(args);
                    self.push(")");
                } else if name.as_str() == "panic" {
                    self.push(&format!("{name}!"));
                    self.push("(");
                    self.emit_args_for_macro(args);
                    self.push(")");
                } else if matches!(name.as_str(), "assert_eq" | "assert_ne") {
                    self.push(&format!("{}!", name));
                    self.push("(");
                    self.emit_args_no_into(args);
                    self.push(")");
                } else if name.as_str() == "List::filled" {
                    // List::filled(n, value) → vec![(value).clone(); (n) as usize]
                    debug_assert_eq!(args.len(), 2, "List::filled requires exactly 2 arguments");
                    self.push("vec![(");
                    if let Some(v) = args.get(1) {
                        self.emit_expr(v);
                    }
                    self.push(").clone(); (");
                    if let Some(n) = args.first() {
                        self.emit_expr(n);
                    }
                    self.push(") as usize]");
                } else if name.as_str() == "map_new" || name.as_str() == "Map::new" {
                    self.push("std::collections::HashMap::new()");
                } else if name.as_str() == "from_int" || name.as_str() == "wrapping_from_int" {
                    // from_int: safe (prover enforces 0–255); wrapping_from_int: intentional truncation.
                    // Both emit identical Rust: ((arg) as i64 as u8).
                    // Cast through i64 so negative literals work: (-1 as i64 as u8) is valid,
                    // but (-1 as u8) triggers E0600 (cannot negate u8).
                    debug_assert_eq!(args.len(), 1, "{} requires exactly one argument", name);
                    self.push("((");
                    if let Some(arg) = args.first() {
                        self.emit_expr(arg);
                    }
                    self.push(") as i64 as u8)");
                } else if name.as_str() == "float_checked_to_int" {
                    // Checked Float→Int: returns None for NaN, ±Inf, out-of-range.
                    debug_assert_eq!(
                        args.len(),
                        1,
                        "float_checked_to_int requires exactly one argument"
                    );
                    self.push("{ let __x = ");
                    if let Some(arg) = args.first() {
                        self.emit_expr(arg);
                    }
                    self.push("; if __x.is_finite() && __x >= (i64::MIN as f64) && __x <= (i64::MAX as f64) { Some(__x as i64) } else { None } }");
                } else if name == "String::from_chars" {
                    self.push("str_from_chars(");
                    self.emit_args(args);
                    self.push(")");
                } else if name == "String::from_bytes" {
                    self.push("str_from_bytes(");
                    self.emit_args(args);
                    self.push(")");
                } else if matches!(name.as_str(), "link" | "unlink") {
                    // `link`/`unlink` conflict with Rust's built-in `link` attribute.
                    // The runtime exports them as `mvl_link`/`mvl_unlink` (u64 args);
                    // MVL passes `Int` (i64), so cast each argument explicitly.
                    let runtime_fn = if name.as_str() == "link" {
                        "mvl_link"
                    } else {
                        "mvl_unlink"
                    };
                    self.push(&format!("{runtime_fn}("));
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.push("(");
                        self.emit_expr(arg);
                        self.push(") as u64");
                    }
                    self.push(")");
                } else {
                    let is_extern = self.has_extern_fn(name.as_str());
                    if is_extern {
                        self.push("unsafe { ");
                    }
                    if !is_extern && self.actor_methods.contains(name.as_str()) {
                        self.push("self.");
                    }
                    if let Some(qualified) = self.stdlib_fn_qualified.get(name.as_str()).cloned() {
                        self.push(&qualified);
                    } else {
                        self.push(&map_fn_name(name));
                    }
                    if !type_args.is_empty() {
                        self.push("::<");
                        let strs: Vec<String> = type_args.iter().map(emit_type_expr).collect();
                        self.push(&strs.join(", "));
                        self.push(">");
                    }
                    self.push("(");
                    let borrows: Vec<Option<bool>> = self
                        .capability_params_map
                        .get(name.as_str())
                        .cloned()
                        .unwrap_or_default();
                    let param_tys: Vec<Ty> = self
                        .fn_param_types
                        .get(name.as_str())
                        .cloned()
                        .unwrap_or_default();
                    self.emit_args_with_borrows_and_coerce(args, &borrows, &param_tys);
                    self.push(")");
                    if is_extern {
                        self.push(" }");
                    }
                }
            }
            TirExprKind::Borrow {
                mutable,
                expr: inner,
            } => {
                let needs_parens = !matches!(
                    &inner.kind,
                    TirExprKind::Var(_) | TirExprKind::FieldAccess { .. }
                );
                if *mutable {
                    self.push(if needs_parens { "&mut (" } else { "&mut " });
                } else {
                    self.push(if needs_parens { "&(" } else { "&" });
                }
                self.emit_expr(inner);
                if needs_parens {
                    self.push(")");
                }
            }
            TirExprKind::Unary { op, expr: inner } => match op {
                UnaryOp::Neg => {
                    self.push("-");
                    self.emit_expr(inner);
                }
                UnaryOp::Not => {
                    self.push("(!");
                    self.emit_expr(inner);
                    self.push(")");
                }
                UnaryOp::Deref => {
                    self.push("*(");
                    self.emit_expr(inner);
                    self.push(")");
                }
                UnaryOp::BitNot => {
                    self.push("!");
                    self.emit_expr(inner);
                }
            },
            TirExprKind::Binary { op, left, right } => {
                // Mutation mode: inject env-var dispatch for behavioral operator alternatives.
                // String concatenation (`+` on string-literal-rooted chains) is excluded from
                // mutation: the `&`/`*` hoisting pattern cannot satisfy Rust's `String + &str`
                // ownership requirement, and the arithmetic alternatives (-, *, /) don't
                // type-check on strings. Such expressions fall through to the regular path.
                let mut_variants_opt = if *op == BinaryOp::Add && is_string_add_chain(left) {
                    None
                } else {
                    self.alloc_binary_mutations(*op, span.line)
                };
                if let Some(mut_variants) = mut_variants_opt {
                    let first_id = mut_variants
                        .first()
                        .expect("alloc_binary_mutations guarantees a non-empty variant list")
                        .0
                        .clone();
                    let lvar = format!("__mvl_l_{first_id}");
                    let rvar = format!("__mvl_r_{first_id}");
                    self.push(&format!("{{ let {lvar} = &("));
                    self.emit_expr(left);
                    self.push(&format!("); let {rvar} = &("));
                    self.emit_expr(right);
                    self.push("); match ::std::env::var(\"MVL_MUTANT\").as_deref() {");
                    for (mid, alt_op) in &mut_variants {
                        self.push(&format!(" Ok(\"{mid}\") => (*{lvar} {alt_op} *{rvar}),"));
                    }
                    self.push(&format!(
                        " _ => (*{lvar} {} *{rvar}), }} }}",
                        emit_binary_op(*op)
                    ));
                } else {
                    // For Int arithmetic, emit checked methods to match LLVM backend
                    // overflow behaviour (trap on overflow rather than wrapping).
                    // Div/Rem: checked_div/checked_rem catch division-by-zero and
                    // i64::MIN / -1 overflow (#1266).
                    let is_int_arith = op.is_arithmetic() && matches!(expr.ty, Ty::Int);
                    if is_int_arith {
                        let (method, msg) = match op {
                            BinaryOp::Add => ("checked_add", "integer overflow"),
                            BinaryOp::Sub => ("checked_sub", "integer overflow"),
                            BinaryOp::Mul => ("checked_mul", "integer overflow"),
                            BinaryOp::Div => ("checked_div", "division by zero or overflow"),
                            BinaryOp::Rem => ("checked_rem", "remainder by zero or overflow"),
                            _ => unreachable!(),
                        };
                        self.push("(<i64>::clone(&(");
                        self.emit_expr(left);
                        self.push(&format!(")).{method}(<i64>::clone(&("));
                        self.emit_expr(right);
                        self.push(&format!("))).expect(\"{msg}\"))"));
                    } else {
                        self.push("(");
                        self.emit_expr(left);
                        self.push(" ");
                        self.push(emit_binary_op(*op));
                        self.push(" ");
                        if *op == BinaryOp::Add && is_string_add_chain(left) {
                            self.push("&(");
                            self.emit_expr(right);
                            self.push(")");
                        } else {
                            self.emit_expr(right);
                        }
                        self.push(")");
                    }
                }
            }
            TirExprKind::If { cond, then, else_ } => {
                self.push("if ");
                self.emit_expr(cond);
                self.push(" {");
                self.nl();
                self.push_indent();
                self.emit_block_as_value(&then.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
                if let Some(else_expr) = else_ {
                    self.push(" else ");
                    self.emit_expr(else_expr);
                }
            }
            TirExprKind::Match { scrutinee, arms } => {
                // Allocate branch coverage IDs for each arm up-front.
                let arm_ids: Vec<Option<usize>> = (0..arms.len())
                    .map(|i| self.alloc_branch(span.line, BranchKind::MatchArm(i)))
                    .collect();
                let has_str_pattern = arms_have_str_pattern(arms);
                // Emit scrutinee first so any compound conditions inside it allocate
                // MC/DC IDs before the match-level decisions (mirrors analysis order).
                self.push("match ");
                self.emit_expr(scrutinee);
                // Clone when the scrutinee is a self.field access (can't move out of &self)
                // or a capability param (val/ref → &T/&mut T in Rust). Without clone,
                // match ergonomics yield reference bindings that fail E0507/E0277.
                if scrutinee_needs_clone(scrutinee)
                    || matches!(&scrutinee.kind, TirExprKind::Var(name) if self.capability_param_names.contains(name))
                {
                    self.push(".clone()");
                }
                // Allocate MC/DC arm-coverage decision after scrutinee.
                let match_mcdc_id: Option<usize> = if arms.len() >= 2 {
                    self.alloc_mcdc_decision(span.line, arms.len(), DecisionKind::Match, vec![])
                } else {
                    None
                };
                // Pre-allocate MatchGuard decision IDs (all arms, before body emission).
                let guard_mcdc_ids: Vec<Option<usize>> = arms
                    .iter()
                    .map(|arm| {
                        arm.guard.as_ref().and_then(|g| {
                            let n = count_clauses_ref(g);
                            if n >= 2 {
                                self.alloc_mcdc_decision(
                                    arm.span.line,
                                    n,
                                    DecisionKind::MatchGuard,
                                    vec![],
                                )
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                if has_str_pattern {
                    self.push(".as_str()");
                }
                self.push(" {");
                self.nl();
                self.push_indent();
                for ((arm_idx, arm), (cov_id, guard_mcdc_id)) in arms
                    .iter()
                    .enumerate()
                    .zip(arm_ids.iter().zip(guard_mcdc_ids.iter()))
                {
                    self.emit_match_arm(arm, arm_idx, *cov_id, match_mcdc_id, *guard_mcdc_id);
                }
                self.pop_indent();
                self.indent();
                self.push("}");
            }
            TirExprKind::Block(block) => {
                self.push("{");
                self.nl();
                self.push_indent();
                self.emit_block_as_value(&block.stmts);
                self.pop_indent();
                self.indent();
                self.push("}");
            }
            TirExprKind::Propagate(inner) => {
                self.emit_expr(inner);
                self.push("?");
            }
            TirExprKind::Construct { name, fields } => {
                self.push(name);
                self.push(" { ");
                for (i, (fname, fexpr)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(&format!("{fname}: "));
                    self.emit_expr_as_arg(fexpr);
                }
                self.push(" }");
            }
            TirExprKind::List { elems } => {
                self.push("vec![");
                self.emit_args(elems);
                self.push("]");
            }
            TirExprKind::Map { pairs } => {
                self.push("std::collections::HashMap::from([");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push("(");
                    self.emit_expr(k);
                    self.push(", ");
                    self.emit_expr(v);
                    self.push(".clone().into()");
                    self.push(")");
                }
                self.push("])");
            }
            TirExprKind::Set { elems } => {
                self.push("std::collections::HashSet::from([");
                self.emit_args(elems);
                self.push("])");
            }
            TirExprKind::Consume(inner) => {
                // `consume` mirrors Pony's `consume` for iso; just emit the inner expr in Phase 1
                self.emit_expr(inner);
            }
            // `relabel name(expr, "tag")` — IFC label bridge (#894, #896).
            TirExprKind::Relabel {
                name,
                expr: inner,
                tag,
                audit,
            } => {
                // Emit runtime audit event if expression-level or declaration-level `audit` (#896).
                let needs_audit = *audit || self.audit_relabels.contains_key(name.as_str());
                if needs_audit {
                    let (from_lbl, to_lbl) = self.relabel_label_strings(name);
                    let loc = self.current_file.clone();
                    self.push("{ let _mvl_rv = ");
                    self.emit_expr(inner);
                    self.push("; ");
                    self.push(&format!(
                        "mvl_runtime::stdlib::audit::emit_relabel_event({name:?}.to_string(), {from_lbl:?}.to_string(), {to_lbl:?}.to_string(), {tag:?}.to_string(), {loc:?}.to_string());"
                    ));
                    match name.as_str() {
                        "trust" | "release" | "undb_url" | "unconfig_path" | "unapi_endpoint"
                        | "unaudit_target" => self.push("(_mvl_rv).0.clone() }"),
                        "classify" => self.push("Secret((_mvl_rv)) }"),
                        "taint" => self.push("Tainted((_mvl_rv)) }"),
                        "db_url" => self.push("DbUrl((_mvl_rv)) }"),
                        "config_path" => self.push("ConfigPath((_mvl_rv)) }"),
                        "api_endpoint" => self.push("ApiEndpoint((_mvl_rv)) }"),
                        "audit_target" => self.push("AuditTarget((_mvl_rv)) }"),
                        _ => unreachable!(
                            "relabel '{name}': unknown transition — blocked by checker (#990)"
                        ),
                    }
                } else {
                    match name.as_str() {
                        // Unwrap: strip the label newtype to get the inner value.
                        "trust" | "release" | "undb_url" | "unconfig_path" | "unapi_endpoint"
                        | "unaudit_target" => {
                            self.push("(");
                            self.emit_expr(inner);
                            self.push(").0.clone()");
                        }
                        // Wrap: construct the label newtype around the value.
                        "classify" => {
                            self.push("Secret((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        "taint" => {
                            self.push("Tainted((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        "db_url" => {
                            self.push("DbUrl((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        "config_path" => {
                            self.push("ConfigPath((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        "api_endpoint" => {
                            self.push("ApiEndpoint((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        "audit_target" => {
                            self.push("AuditTarget((");
                            self.emit_expr(inner);
                            self.push("))");
                        }
                        _ => {
                            unreachable!(
                                "relabel '{name}': unknown transition — blocked by checker (#990)"
                            );
                        }
                    }
                }
            }
            TirExprKind::Lambda { params, body } => {
                self.push("|");
                let param_strs: Vec<String> = params
                    .iter()
                    .map(|p| {
                        let ty_str = emit_fn_param_ty(&p.ty);
                        format!("{}: {ty_str}", p.name)
                    })
                    .collect();
                self.push(&param_strs.join(", "));
                self.push("|");
                self.push(" ");
                self.emit_expr(body);
            }
            TirExprKind::Spawn { actor_type, fields } => {
                // Phase 8: `actor Counter { count: 0 }` → `_start_counter(CounterState { count: 0, _self_ref: None })`
                let snake =
                    crate::mvl::backends::rust::emit_actors::actor_name_to_snake(actor_type);
                self.push(&format!("_start_{snake}({actor_type}State {{"));
                for (i, (field_name, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(&format!("{field_name}: "));
                    self.emit_expr_as_arg(val);
                }
                // `_self_ref`/`_self_id` are always zero/None at construction;
                // `_start_<name>` sets them after the channel is created (#1128).
                if !fields.is_empty() {
                    self.push(", ");
                }
                self.push("_self_ref: None, _self_id: 0");
                self.push("})");
            }
            TirExprKind::Quantifier(..) => unreachable!("Quantifier in codegen position"),
            TirExprKind::Select { arms } => {
                self.push("{");
                self.nl();
                self.push_indent();
                if let Some(first) = arms.first() {
                    self.emit_block_stmts(&first.body.stmts);
                }
                self.pop_indent();
                self.indent();
                self.push("}");
            }
        }
    }
}

// ── Lambda param type helper ──────────────────────────────────────────────

/// Emit a lambda parameter type.  Fn types stay as bare `fn(T) -> U` to
/// remain compatible with enum/struct fields that use `fn(T) -> U`.
fn emit_fn_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Fn(params, ret, _, _) => {
            let params_str: Vec<String> = params.iter().map(emit_ty).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_ty(ret))
        }
        _ => emit_ty(ty),
    }
}

// ── Literal ───────────────────────────────────────────────────────────────

/// Re-escape a decoded string value for insertion into a Rust string literal.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            other => out.push(other),
        }
    }
    out
}

/// Re-escape a decoded char value for insertion into a Rust char literal.
fn escape_char(c: char) -> String {
    match c {
        '\\' => "\\\\".to_string(),
        '\'' => "\\'".to_string(),
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\0' => "\\0".to_string(),
        other => other.to_string(),
    }
}

impl RustEmitter {
    fn emit_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Integer(n) => self.push(&n.to_string()),
            Literal::Float(f) => {
                // Ensure float literals have a decimal point in Rust
                let s = format!("{f}");
                if s.contains('.') || s.contains('e') {
                    self.push(&s);
                } else {
                    self.push(&format!("{s}.0"));
                }
            }
            Literal::Str(s) => self.push(&format!("\"{}\".to_string()", escape_str(s))),
            Literal::Char(c) => self.push(&format!("'{}'", escape_char(*c))),
            Literal::Bool(b) => self.push(if *b { "true" } else { "false" }),
            Literal::Unit => self.push("()"),
        }
    }
}

/// Returns true if any match arm uses a string literal pattern.
///
/// When true, the scrutinee must be coerced to `&str` via `.as_str()` so that
/// Rust's pattern matching works (both `String` and IFC-labeled strings expose
/// `.as_str()`).  Called from both `TirExprKind::Match` and `TirStmt::Match` codegen.
pub fn arms_have_str_pattern(arms: &[TirMatchArm]) -> bool {
    arms.iter()
        .any(|a| matches!(&a.pattern, Pattern::Literal(Literal::Str(_), _)))
}

impl RustEmitter {
    /// Emit a literal in pattern position.  String literals must be bare `"s"`
    /// (not `"s".to_string()`) because Rust patterns cannot contain method calls.
    fn emit_literal_in_pattern(&mut self, lit: &Literal) {
        match lit {
            Literal::Str(s) => self.push(&format!("\"{}\"", escape_str(s))),
            other => self.emit_literal(other),
        }
    }

    // ── Arguments ─────────────────────────────────────────────────────────────

    pub(super) fn emit_args(&mut self, args: &[TirExpr]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.emit_expr_as_fn_arg(arg);
        }
    }

    /// Emit arguments without `.into()` on string literals.
    pub(super) fn emit_args_no_into(&mut self, args: &[TirExpr]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            if let TirExprKind::Literal(Literal::Str(s)) = &arg.kind {
                self.push(&format!("\"{}\".to_string()", escape_str(s)));
            } else {
                self.emit_expr_as_arg(arg);
            }
        }
    }

    /// Emit arguments for a function call, using per-parameter borrow kinds (Phase B)
    /// and refined alias wrapping/unwrapping (#1326).
    fn emit_args_with_borrows_and_coerce(
        &mut self,
        args: &[TirExpr],
        borrows: &[Option<bool>],
        param_tys: &[Ty],
    ) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            let param_ty = param_tys.get(i);
            // Check if we need refined alias wrapping/unwrapping
            let wrap_alias = param_ty.and_then(|pt| {
                if let Ty::Named(name, _) = pt {
                    if self.refined_aliases.contains_key(name.as_str())
                        && self.refined_alias_base(&arg.ty).is_none()
                    {
                        return Some(name.clone());
                    }
                }
                None
            });
            let unwrap_alias = param_ty.is_some_and(|pt| {
                self.refined_alias_base(pt).is_none() && self.refined_alias_base(&arg.ty).is_some()
            });

            if let Some(ref alias_name) = wrap_alias {
                self.push(&format!("{}::new(", alias_name));
            }
            if wrap_alias.is_some() || unwrap_alias {
                // Refined alias coercion: emit raw expression without .into()
                // since wrapping/unwrapping handles the type conversion.
                self.emit_expr(arg);
            } else {
                match borrows.get(i).copied().flatten() {
                    Some(mutable) => self.emit_expr_as_borrow_arg(arg, mutable),
                    None => self.emit_expr_as_fn_arg(arg),
                }
            }
            if unwrap_alias {
                self.push(".0");
            }
            if wrap_alias.is_some() {
                self.push(")");
            }
        }
    }

    /// Emit an expression as a reference argument (`&x` or `&mut x`).
    fn emit_expr_as_borrow_arg(&mut self, expr: &TirExpr, mutable: bool) {
        match &expr.kind {
            // Fix 3: already a borrow expression — emit as-is, no extra & needed
            TirExprKind::Borrow { .. } => self.emit_expr(expr),
            TirExprKind::Var(_) | TirExprKind::FieldAccess { .. } => {
                self.push(if mutable { "&mut " } else { "&" });
                self.emit_expr(expr);
            }
            _ => {
                self.push(if mutable { "&mut (" } else { "&(" });
                self.emit_expr(expr);
                self.push(")");
            }
        }
    }

    /// Emit an expression in argument position.
    ///
    /// MVL has value semantics: passing a value to a function is a copy, not a move.
    /// We insert `.clone()` for identifiers and field accesses so the caller retains
    /// ownership, matching MVL semantics.
    ///
    /// `coerce` — when `true`, appends `.into()` so unlabeled (`Public`) values coerce
    /// into labeled parameters (e.g. `String` → `Clean<String>`) via the `From<T> for
    /// Label<T>` impls in `mvl_runtime::ifc`.  When `false`, value semantics only (no
    /// label coercion).
    ///
    /// # Phase A: last-use move elision (Spec 009 Req 2)
    ///
    /// When a `TirExprKind::Var`'s span appears in [`RustEmitter::last_uses`] the
    /// variable is used for the last time.  Emitting a Rust move (no `.clone()`) is
    /// sound: the caller's binding is consumed but never read again.
    fn emit_expr_as_value_arg(&mut self, expr: &TirExpr, coerce: bool) {
        match &expr.kind {
            TirExprKind::Literal(Literal::Str(s)) => {
                self.push(&format!("\"{}\".to_string().into()", escape_str(s)));
            }
            // Phase 8: `self` used as a tag argument inside an actor behavior.
            TirExprKind::Var(name) if name == "self" && !self.actor_self_type.is_empty() => {
                let ty = self.actor_self_type.clone();
                self.push(&format!(
                    "{ty} {{ _sender: self._self_ref.as_ref().unwrap().upgrade().unwrap(), _id: self._self_id }}"
                ));
            }
            // coerce only: `self` receiver in a type-attached method cannot be moved.
            TirExprKind::Var(name)
                if coerce
                    && name == "self"
                    && self.actor_self_type.is_empty()
                    && !self.self_as_free_param =>
            {
                self.push("self.clone().into()");
            }
            // coerce only: Rust function items do not implement `Into<_>` generically.
            TirExprKind::Var(_) if coerce && matches!(expr.ty, Ty::Fn(..)) => {
                self.emit_expr(expr);
                if !self.last_uses.contains(&expr.span) {
                    self.push(".clone()");
                }
            }
            // coerce only: `Option`/`Result` — no blanket `Into` impl available.
            TirExprKind::Var(_)
                if coerce && matches!(expr.ty, Ty::Option(_) | Ty::Result(_, _)) =>
            {
                self.emit_expr(expr);
                if !self.last_uses.contains(&expr.span) {
                    self.push(".clone()");
                }
            }
            TirExprKind::Var(name) => {
                self.emit_expr(expr);
                if coerce {
                    if !self.last_uses.contains(&expr.span)
                        || self.capability_param_names.contains(name.as_str())
                    {
                        self.push(".clone().into()");
                    } else {
                        self.push(".into()");
                    }
                } else if !self.last_uses.contains(&expr.span) {
                    self.push(".clone()");
                }
            }
            // Field accesses: conservatively clone (partial moves are complex in Rust).
            TirExprKind::FieldAccess { .. } => {
                self.emit_expr(expr);
                self.push(if coerce {
                    ".clone().into()"
                } else {
                    ".clone()"
                });
            }
            _ => {
                self.emit_expr(expr);
            }
        }
    }

    /// Value semantics, no IFC label coercion. See [`Self::emit_expr_as_value_arg`].
    pub(super) fn emit_expr_as_arg(&mut self, expr: &TirExpr) {
        self.emit_expr_as_value_arg(expr, false);
    }

    /// Value semantics with IFC label coercion (`.into()`). See [`Self::emit_expr_as_value_arg`].
    pub(super) fn emit_expr_as_fn_arg(&mut self, expr: &TirExpr) {
        self.emit_expr_as_value_arg(expr, true);
    }

    /// Emit arguments for Rust macros like `println!` where the first argument
    /// must be a bare string literal (not a `.to_string()` expression).
    fn emit_args_for_macro(&mut self, args: &[TirExpr]) {
        if args.is_empty() {
            return;
        }
        match &args[0].kind {
            TirExprKind::Literal(Literal::Str(s)) => {
                self.push(&format!("\"{}\"", escape_str(s)));
                for arg in &args[1..] {
                    self.push(", ");
                    self.emit_expr_as_arg(arg);
                }
            }
            _ => {
                let placeholders = vec!["{}"; args.len()].join(" ");
                self.push(&format!("\"{placeholders}\""));
                for arg in args {
                    self.push(", ");
                    self.emit_expr_as_arg(arg);
                }
            }
        }
    }
}

// ── Binary operators ──────────────────────────────────────────────────────

/// Return true when `expr` is the left side of a string concatenation chain.
fn is_string_add_chain(expr: &TirExpr) -> bool {
    match &expr.kind {
        TirExprKind::Literal(Literal::Str(_)) => true,
        TirExprKind::Binary {
            op: BinaryOp::Add,
            left,
            ..
        } => is_string_add_chain(left),
        _ => false,
    }
}

fn emit_binary_op(op: BinaryOp) -> &'static str {
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

// ── Match arms ────────────────────────────────────────────────────────────

impl RustEmitter {
    fn emit_match_arm(
        &mut self,
        arm: &TirMatchArm,
        arm_idx: usize,
        cov_id: Option<usize>,
        match_mcdc_id: Option<usize>,
        guard_mcdc_id: Option<usize>,
    ) {
        self.indent();
        self.emit_pattern(&arm.pattern);
        if let Some(guard) = &arm.guard {
            self.push(" if ");
            if let Some(gid) = guard_mcdc_id {
                let n = count_clauses_ref(guard);
                self.push(&emit_mcdc_guard_block(guard, gid, n));
            } else {
                use crate::mvl::backends::rust::emit_types::emit_ref_expr_for_assert;
                self.push(&emit_ref_expr_for_assert(guard, "_"));
            }
        }
        self.push(" => ");
        match &arm.body {
            TirMatchBody::Expr(e) => {
                self.push("{ ");
                if let Some(id) = cov_id {
                    self.push(&format!("#[cfg(test)] crate::__mvl_cov::hit({id}); "));
                }
                if let Some(mid) = match_mcdc_id {
                    self.push(&format!(
                        "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32); "
                    ));
                }
                self.emit_expr(e);
                self.push(" }");
                self.push(",");
                self.nl();
            }
            TirMatchBody::Block(block) => {
                self.push("{");
                self.nl();
                self.push_indent();
                if let Some(id) = cov_id {
                    self.emit_cov_hit(id);
                }
                if let Some(mid) = match_mcdc_id {
                    self.line(&format!(
                        "#[cfg(test)] crate::__mvl_mcdc::record({mid}usize, {arm_idx}u32);"
                    ));
                }
                self.emit_block_as_value(&block.stmts);
                self.pop_indent();
                self.indent();
                self.push("},");
                self.nl();
            }
        }
    }

    // ── Patterns ─────────────────────────────────────────────────────────────

    pub fn emit_pattern(&mut self, pat: &Pattern) {
        match pat {
            Pattern::Wildcard(_) => self.push("_"),
            Pattern::Ident(name, _) => self.push(&map_ident(name)),
            Pattern::Literal(lit, _) => self.emit_literal_in_pattern(lit),
            Pattern::TupleStruct { name, fields, .. } => {
                self.push(name);
                self.push("(");
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.emit_pattern(f);
                }
                self.push(")");
            }
            Pattern::Struct {
                name, fields, rest, ..
            } => {
                self.push(name);
                self.push(" { ");
                for (i, (fname, fpat)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.push(fname);
                    self.push(": ");
                    self.emit_pattern(fpat);
                }
                if *rest {
                    if !fields.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                }
                self.push(" }");
            }
            Pattern::Some { inner, .. } => {
                self.push("Some(");
                self.emit_pattern(inner);
                self.push(")");
            }
            Pattern::None(_) => self.push("None"),
            Pattern::Ok { inner, .. } => {
                self.push("Ok(");
                self.emit_pattern(inner);
                self.push(")");
            }
            Pattern::Err { inner, .. } => {
                self.push("Err(");
                self.emit_pattern(inner);
                self.push(")");
            }
            Pattern::Or { patterns, .. } => {
                for (i, p) in patterns.iter().enumerate() {
                    if i > 0 {
                        self.push(" | ");
                    }
                    self.emit_pattern(p);
                }
            }
        }
    }

    // ── Block statements (used in if/match body/function body) ────────────────

    pub fn emit_block_stmts(&mut self, stmts: &[TirStmt]) {
        for stmt in stmts {
            self.emit_stmt(stmt);
        }
    }

    /// Emit block statements where the final `TirStmt::Expr` is a tail expression
    /// (no semicolon), so it becomes the implicit return value of the block.
    pub fn emit_block_as_value(&mut self, stmts: &[TirStmt]) {
        if stmts.is_empty() {
            return;
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for stmt in head {
            self.emit_stmt(stmt);
        }
        match &tail[0] {
            TirStmt::Expr { expr, .. } => {
                self.indent();
                self.emit_expr(expr);
                self.nl();
            }
            other => self.emit_stmt(other),
        }
    }
}

// ── Name mappings ─────────────────────────────────────────────────────────

fn map_ident(name: &str) -> String {
    name.to_string()
}

fn map_fn_name(name: &str) -> String {
    match name {
        "panic" => "panic!".to_string(),
        "assert" => "assert!".to_string(),
        "assert_eq" => "assert_eq!".to_string(),
        "assert_ne" => "assert_ne!".to_string(),
        _ => name.to_string(),
    }
}
