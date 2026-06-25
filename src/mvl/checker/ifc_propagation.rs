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

use std::collections::{HashMap, HashSet};

use crate::mvl::checker::context::TypeEnv;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc;
use crate::mvl::parser::ast::{Block, Decl, ElseBranch, Expr, MatchBody, Program, Stmt, TypeExpr};
use crate::mvl::parser::lexer::Span;

/// Sentinel value stored in the inferred label table for functions that explicitly
/// strip labels via `relabel` (e.g. `relabel trust(x, "TAG")` → bare).
///
/// The leading `<` makes this impossible to match any valid MVL label name, which
/// must start with `[A-Za-z_]`.  A user-defined `label` whose name happened to be
/// `__stripped__` would have collided with the old string literal; this one cannot.
const STRIPPED_SENTINEL: &str = "<stripped>";

// ── External taint source registry (#833) ─────────────────────────────────────

/// Function names that always return `Tainted` data, independent of arguments.
///
/// This supplements TypeEnv's explicit labels (e.g., `stdin_read_line → Tainted[String]`
/// from std.io) for environments where the full stdlib is not loaded — for example,
/// standalone test programs or sandboxed compilation.
///
/// Free-function forms only. Method-call forms (e.g. `env.get()`, `db.query()`)
/// are dot-syntax (`Expr::MethodCall`) and cannot be registered here until
/// method-call resolution is available post-monomorphization (ADR-0034, #838).
const TAINT_SOURCES: &[&str] = &[
    "read_line",
    "args",
    "read_tainted",
    "env_var",
    "read_file",
    "recv",
    "recv_line",
];

// ── Inferred label table ───────────────────────────────────────────────────────

/// Inferred security labels for function return types.
///
/// Built by [`propagate`]; stored in [`CheckResult`] for use by downstream
/// passes and tools.
///
/// Two disjoint sub-tables:
/// - **`explicit`** — TypeEnv-seeded annotations and hardcoded taint sources.
///   These are programmer (or stdlib) guarantees and are authoritative: a call
///   to an explicitly-annotated function always returns that label regardless
///   of argument labels.
/// - **`inferred`** — Labels derived from body analysis during the fixed-point
///   propagation loop.  These are conservative approximations and should be
///   *joined with argument labels* at call sites during violation detection to
///   preserve context sensitivity for label-polymorphic wrappers.
#[derive(Debug, Default, Clone)]
pub struct InferredLabels {
    explicit: HashMap<String, String>,
    inferred: HashMap<String, String>,
}

impl InferredLabels {
    /// Return the return label for `fn_name`, if any.
    /// Explicit TypeEnv-seeded labels take precedence over propagation-derived ones.
    pub fn get(&self, fn_name: &str) -> Option<String> {
        let label = self
            .explicit
            .get(fn_name)
            .or_else(|| self.inferred.get(fn_name))
            .cloned();
        // Filter out the internal STRIPPED_SENTINEL — callers see None (bare).
        label.filter(|l| l != STRIPPED_SENTINEL)
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
    let mut table: HashMap<String, String> = HashMap::new();
    // Track which names were seeded explicitly (TypeEnv or TAINT_SOURCES) so we can
    // split the table into explicit vs inferred at the end (#849).
    let mut explicit_names: HashSet<String> = HashSet::new();

    // Seed 1: explicit return labels from TypeEnv (covers stdlib taint sources).
    for (name, fn_info) in type_env.fns() {
        if let Some(label) = ifc::label_of(&fn_info.ret) {
            table.insert(name.clone(), label.to_string());
            explicit_names.insert(name.clone());
        }
    }

    // Seed 2: external taint source registry (#833) — safety net for unloaded stdlib.
    for &src in TAINT_SOURCES {
        table
            .entry(src.to_string())
            .or_insert("Tainted".to_string());
        explicit_names.insert(src.to_string());
    }

    // Fixed-point body-analysis loop (#830 + #833).
    loop {
        let mut changed = false;
        for prog in programs {
            for decl in &prog.declarations {
                // Collect all function-like items: top-level fns, impl methods, actor methods.
                let fns: Vec<(&str, &[crate::mvl::parser::ast::Param], &TypeExpr, &Block)> =
                    match decl {
                        Decl::Fn(fd) => vec![(&fd.name, &fd.params, &fd.return_type, &fd.body)],
                        Decl::Impl(id) => id
                            .methods
                            .iter()
                            .map(|m| {
                                (
                                    m.name.as_str(),
                                    m.params.as_slice(),
                                    &*m.return_type,
                                    &m.body,
                                )
                            })
                            .collect(),
                        Decl::Actor(ad) => ad
                            .methods
                            .iter()
                            .map(|m| {
                                (
                                    m.name.as_str(),
                                    m.params.as_slice(),
                                    &*m.return_type,
                                    &m.body,
                                )
                            })
                            .collect(),
                        _ => continue,
                    };
                for (name, params, return_type, body) in fns {
                    // Skip functions with an explicit return label — annotation wins.
                    if label_of_type_expr(return_type).is_some() {
                        continue;
                    }
                    // Build param label env from declared annotations.
                    let mut param_env: HashMap<String, String> = HashMap::new();
                    for param in params {
                        if let Some(l) = label_of_type_expr(&param.ty) {
                            param_env.insert(param.name.clone(), l);
                        }
                    }
                    // Infer return label from body return expressions.
                    let has_labeled_params = !param_env.is_empty();
                    if let Some(label) = infer_return_label(body, &param_env, &table) {
                        let current = table.get(name).cloned();
                        // In the new model, any label is non-Public; keep the first label found.
                        let new_label = current.clone().unwrap_or_else(|| label.clone());
                        if current.as_deref() != Some(new_label.as_str()) {
                            table.insert(name.to_string(), new_label);
                            changed = true;
                        }
                    } else if has_labeled_params && !table.contains_key(name) {
                        // Function has labeled params but body explicitly returns bare (None).
                        // Mark as "explicitly bare" so call sites don't apply label-polymorphic
                        // propagation (which would incorrectly infer Tainted from arg labels).
                        // STRIPPED_SENTINEL = function explicitly strips labels via relabel.
                        table.insert(name.to_string(), STRIPPED_SENTINEL.to_string());
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Split combined table into explicit (TypeEnv-seeded) and inferred (body-analysis).
    let mut explicit: HashMap<String, String> = HashMap::new();
    let mut inferred: HashMap<String, String> = HashMap::new();
    for (name, label) in table {
        if explicit_names.contains(&name) {
            explicit.insert(name, label);
        } else {
            inferred.insert(name, label);
        }
    }
    InferredLabels { explicit, inferred }
}

/// Infer the return label for a function body from its return points.
///
/// Returns the join of labels inferred from:
/// - Explicit `return expr` statements anywhere in the body.
/// - The tail expression of the block (implicit return value).
fn infer_return_label(
    block: &Block,
    param_env: &HashMap<String, String>,
    table: &HashMap<String, String>,
) -> Option<String> {
    // During propagation, table is the full combined table; pass it as both
    // explicit and inferred so all known labels are treated as authoritative.
    let returns = collect_explicit_returns(block, param_env, table);
    let tail = block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, param_env, table, table)
        } else {
            None
        }
    });
    ifc::join_opt(returns, tail)
}

/// Walk a block collecting labels from explicit `return expr` statements.
fn collect_explicit_returns(
    block: &Block,
    env: &HashMap<String, String>,
    table: &HashMap<String, String>,
) -> Option<String> {
    block
        .stmts
        .iter()
        .map(|s| collect_returns_in_stmt(s, env, table))
        .fold(None, ifc::join_opt)
}

fn collect_returns_in_stmt(
    stmt: &Stmt,
    env: &HashMap<String, String>,
    table: &HashMap<String, String>,
) -> Option<String> {
    // Propagation helpers: pass table as both explicit and inferred (all labels authoritative).
    match stmt {
        Stmt::Return { value: Some(e), .. } => infer_label_extended(e, env, table, table),
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
                MatchBody::Expr(e) => infer_label_extended(e, env, table, table),
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
/// the provided tables:
///
/// - `explicit` — TypeEnv-seeded annotations and TAINT_SOURCES (authoritative).
///   A call to an explicitly-annotated callee short-circuits: the callee's
///   declared return label is returned without consulting argument labels.
///   This is correct because explicit annotations are programmer guarantees
///   (e.g. `sanitize → Clean` means the function always returns Clean regardless
///   of input taint).
///
/// - `inferred` — Labels derived from body analysis.  For these callees the
///   return label is *joined with the argument labels* to preserve context
///   sensitivity: an unannotated wrapper that passes its argument through may
///   return a higher label when called with a tainted argument than when the
///   fixed-point analysis saw it in isolation.
///
/// During propagation, callers pass the same combined table for both `explicit`
/// and `inferred` (so all known labels are treated as authoritative), preserving
/// the behaviour of the original single-table implementation.  During violation
/// detection, callers pass the split tables for context-sensitive precision.
pub fn infer_label_extended(
    expr: &Expr,
    env: &HashMap<String, String>,
    explicit: &HashMap<String, String>,
    inferred: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => env.get(name.as_str()).cloned(),
        Expr::FnCall { name, args, .. } => {
            // Explicit annotation → authoritative; short-circuit without examining args.
            if let Some(label) = explicit.get(name.as_str()) {
                return Some(label.clone());
            }
            // STRIPPED_SENTINEL: function explicitly strips labels via relabel.
            // Do not propagate arg labels — return None (bare).
            if inferred
                .get(name.as_str())
                .is_some_and(|l| l == STRIPPED_SENTINEL)
            {
                return None;
            }
            // Fix #858: local lambda variable — env holds the lambda's return label.
            // Enables `let f = || -> Tainted[T] { ... }; f()` to propagate taint.
            // Guard: only apply when the name is not a known function in `inferred`,
            // to avoid a variable shadowing an unannotated function of the same name.
            if !inferred.contains_key(name.as_str()) {
                if let Some(label) = env.get(name.as_str()) {
                    return Some(label.clone());
                }
            }
            // Inferred label → join with arg labels for context sensitivity.
            let base = inferred.get(name.as_str()).cloned();
            let arg_join = args
                .iter()
                .map(|a| infer_label_extended(a, env, explicit, inferred))
                .fold(None, ifc::join_opt);
            ifc::join_opt(base, arg_join)
        }
        // relabel() produces the `to` side: bare (None) or labeled.
        // Conservative: return None — the caller should check the declared transition.
        Expr::Relabel { .. } => None,
        Expr::Binary { left, right, .. } => ifc::join_opt(
            infer_label_extended(left, env, explicit, inferred),
            infer_label_extended(right, env, explicit, inferred),
        ),
        Expr::Unary { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::FieldAccess { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::As { expr, .. } => infer_label_extended(expr, env, explicit, inferred),
        Expr::If { then, else_, .. } => {
            // The value label of an if-expression is the join of its branch
            // result labels only. The condition label tracks implicit flow and
            // is handled separately by check_implicit_flows in ifc.rs.
            let then_label = tail_label_of_block(then, env, explicit, inferred);
            let else_label = else_
                .as_ref()
                .and_then(|e| infer_label_extended(e, env, explicit, inferred));
            ifc::join_opt(then_label, else_label)
        }
        Expr::Block(b) => tail_label_of_block(b, env, explicit, inferred),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scr = infer_label_extended(scrutinee, env, explicit, inferred);
            let arms_label = arms
                .iter()
                .map(|arm| match &arm.body {
                    MatchBody::Expr(e) => infer_label_extended(e, env, explicit, inferred),
                    MatchBody::Block(b) => tail_label_of_block(b, env, explicit, inferred),
                })
                .fold(None, ifc::join_opt);
            ifc::join_opt(scr, arms_label)
        }
        Expr::MethodCall { receiver, args, .. } => {
            let recv = infer_label_extended(receiver, env, explicit, inferred);
            let arg_label = args
                .iter()
                .map(|a| infer_label_extended(a, env, explicit, inferred))
                .fold(None, ifc::join_opt);
            ifc::join_opt(recv, arg_label)
        }
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => fields
            .iter()
            .map(|(_, e)| infer_label_extended(e, env, explicit, inferred))
            .fold(None, ifc::join_opt),
        Expr::List { elems, .. } | Expr::Set { elems, .. } => elems
            .iter()
            .map(|e| infer_label_extended(e, env, explicit, inferred))
            .fold(None, ifc::join_opt),
        Expr::Map { pairs, .. } => pairs
            .iter()
            .map(|(k, v)| {
                ifc::join_opt(
                    infer_label_extended(k, env, explicit, inferred),
                    infer_label_extended(v, env, explicit, inferred),
                )
            })
            .fold(None, ifc::join_opt),
        // Fix #851: build lambda-local env with param labels before recursing.
        // Fix #858: also incorporate the declared return type label so that
        // `|x| -> Tainted[String] { ... }` is visible as tainted at the call site.
        Expr::Lambda {
            params,
            ret_type,
            body,
            ..
        } => {
            let mut lambda_env = env.clone();
            for param in params {
                if let Some(l) = label_of_type_expr(&param.ty) {
                    lambda_env.insert(param.name.clone(), l);
                }
            }
            let body_label = infer_label_extended(body, &lambda_env, explicit, inferred);
            let ret_label = ret_type.as_deref().and_then(label_of_type_expr);
            ifc::join_opt(body_label, ret_label)
        }
        Expr::Select { arms, .. } => arms
            .iter()
            .map(|a| infer_label_extended(&a.expr, env, explicit, inferred))
            .fold(None, ifc::join_opt),
        Expr::Literal(..) | Expr::Quantifier(..) => None,
    }
}

/// Return the label of the tail expression of a block, if it ends in an expression.
fn tail_label_of_block(
    block: &Block,
    env: &HashMap<String, String>,
    explicit: &HashMap<String, String>,
    inferred: &HashMap<String, String>,
) -> Option<String> {
    block.stmts.last().and_then(|s| {
        if let Stmt::Expr { expr, .. } = s {
            infer_label_extended(expr, env, explicit, inferred)
        } else {
            None
        }
    })
}

// ── Violation detection (#831) ────────────────────────────────────────────────

/// Detect interprocedural IFC violations in `prog`.
///
/// For each `FnCall callee(args)` where `callee` has labeled parameters in
/// `type_env`, computes the inferred label of each arg via [`infer_label_extended`]
/// and checks whether it can flow to the required parameter label.
///
/// Reports violations that the direct type checker missed because the arg's
/// *declared* type is unlabeled (treated as Public) but its *inferred* label
/// is higher — e.g., an unannotated wrapper returning data from `stdin_read_line`.
///
/// Returns [`CheckError::InterprocFlowViolation`] values (Req 11) ready to
/// append to `CheckResult.errors`.
pub fn detect_violations(
    prog: &Program,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
) -> Vec<CheckError> {
    let mut collector = ViolationCollector {
        caller: "",
        env: HashMap::new(),
        type_env,
        inferred,
        errors: Vec::new(),
    };
    for decl in &prog.declarations {
        // Check top-level fns, impl methods, and actor methods.
        let fns: Vec<(&str, &[crate::mvl::parser::ast::Param], &Block)> = match decl {
            Decl::Fn(fd) => vec![(&fd.name, &fd.params, &fd.body)],
            Decl::Impl(id) => id
                .methods
                .iter()
                .map(|m| (m.name.as_str(), m.params.as_slice(), &m.body))
                .collect(),
            Decl::Actor(ad) => ad
                .methods
                .iter()
                .map(|m| (m.name.as_str(), m.params.as_slice(), &m.body))
                .collect(),
            _ => continue,
        };
        for (name, params, body) in fns {
            collector.caller = name;
            collector.env.clear();
            for param in params {
                if let Some(l) = label_of_type_expr(&param.ty) {
                    collector.env.insert(param.name.clone(), l);
                }
            }
            crate::mvl::parser::visit::Visit::visit_block(&mut collector, body);
        }
    }
    collector.errors
}

/// Per-function violation collector — owns the label-binding environment and
/// the accumulating error vec.  Implements [`Visit`] so AST traversal is shared
/// with the rest of the checker (#1522, #1526).
///
/// **Scope handling.**  The pre-#1526 walkers cloned the environment at every
/// scope boundary (block entry, match arm, lambda body, for-loop body, else-if
/// branch).  This visitor preserves that semantics via explicit save/restore in
/// the overrides below.  `visit_block` is the centralised entry point: it
/// snapshots `env` at block entry and restores it on exit, so any `let` binding
/// is automatically scoped to its enclosing block.
struct ViolationCollector<'a> {
    caller: &'a str,
    env: HashMap<String, String>,
    type_env: &'a TypeEnv,
    inferred: &'a InferredLabels,
    errors: Vec<CheckError>,
}

impl<'a, 'ast> crate::mvl::parser::visit::Visit<'ast> for ViolationCollector<'a> {
    fn visit_block(&mut self, b: &'ast Block) {
        // Block-local scope: `let` bindings inside this block die at block exit.
        let saved = self.env.clone();
        crate::mvl::parser::visit::walk_block(self, b);
        self.env = saved;
    }

    fn visit_stmt(&mut self, s: &'ast Stmt) {
        match s {
            Stmt::Let {
                pattern, init, ty, ..
            } => {
                self.visit_expr(init);
                // Prefer declared label over inferred init label so validated
                // bindings (`let clean: Clean[T] = validate(tainted)?`) keep
                // their declared label.  See #849, #850, #858.
                let init_label = infer_label_extended(
                    init,
                    &self.env,
                    &self.inferred.explicit,
                    &self.inferred.inferred,
                );
                let effective_label = label_of_type_expr(ty).or(init_label);
                if let Some(l) = effective_label {
                    ifc::bind_pattern_labels(pattern, &l, &mut self.env);
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                // Per-arm scope: every arm starts from the env at match entry.
                let saved = self.env.clone();
                for arm in arms {
                    self.env = saved.clone();
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => self.visit_block(b),
                    }
                }
                self.env = saved;
            }
            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => {
                self.visit_expr(iter);
                // Bind the loop variable to the iterator's label inside the
                // body only (#858).
                let iter_label = infer_label_extended(
                    iter,
                    &self.env,
                    &self.inferred.explicit,
                    &self.inferred.inferred,
                );
                let saved = self.env.clone();
                if let Some(l) = iter_label {
                    ifc::bind_pattern_labels(pattern, &l, &mut self.env);
                }
                self.visit_block(body);
                self.env = saved;
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond);
                self.visit_block(then);
                match else_ {
                    Some(ElseBranch::Block(b)) => self.visit_block(b),
                    Some(ElseBranch::If(s)) => {
                        // Else-if branch sees env at if entry, not after-then.
                        let saved = self.env.clone();
                        self.visit_stmt(s);
                        self.env = saved;
                    }
                    None => {}
                }
            }
            _ => crate::mvl::parser::visit::walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::FnCall {
                name, args, span, ..
            } => {
                check_call_violations(
                    name,
                    args,
                    *span,
                    self.caller,
                    &self.env,
                    self.type_env,
                    self.inferred,
                    &mut self.errors,
                );
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.visit_expr(scrutinee);
                let saved = self.env.clone();
                for arm in arms {
                    self.env = saved.clone();
                    match &arm.body {
                        MatchBody::Expr(e) => self.visit_expr(e),
                        MatchBody::Block(b) => self.visit_block(b),
                    }
                }
                self.env = saved;
            }
            // Fix #851: build lambda-local env with param labels before recursing.
            Expr::Lambda { params, body, .. } => {
                let saved = self.env.clone();
                for param in params {
                    if let Some(l) = label_of_type_expr(&param.ty) {
                        self.env.insert(param.name.clone(), l);
                    }
                }
                self.visit_expr(body);
                self.env = saved;
            }
            _ => crate::mvl::parser::visit::walk_expr(self, e),
        }
    }
}

/// Check a single call site for interprocedural IFC violations.
#[allow(clippy::too_many_arguments)]
fn check_call_violations(
    callee_name: &str,
    args: &[Expr],
    call_span: Span,
    caller: &str,
    env: &HashMap<String, String>,
    type_env: &TypeEnv,
    inferred: &InferredLabels,
    errors: &mut Vec<CheckError>,
) {
    let Some(fn_info) = type_env.lookup_fn(callee_name) else {
        return;
    };
    // Skip variadic builtins (empty params is the sentinel).
    if fn_info.params.is_empty() {
        return;
    }
    // Skip generic functions (monomorphization deferred to #838).
    if !fn_info.type_params.is_empty() {
        return;
    }

    for (param_idx, (arg, param_ty)) in args.iter().zip(fn_info.params.iter()).enumerate() {
        // In the new model, both labeled and unlabeled params can be violated:
        // - Arg is Tainted, param is bare → violation (labeled ≠ bare)
        // - Arg is Tainted, param is Secret → violation (Tainted ≠ Secret)
        // - Arg is bare, param is Tainted → violation (bare ≠ labeled, but caught by type checker)
        let required = ifc::label_of(param_ty); // None = bare, Some(name) = labeled
                                                // Use split tables: explicit labels are authoritative; inferred labels join with
                                                // arg labels to preserve context sensitivity for label-polymorphic wrappers (#849).
        let arg_label = infer_label_extended(arg, env, &inferred.explicit, &inferred.inferred);
        // Violation: arg label ≠ required label (either side may be None = bare)
        if arg_label.as_deref() == required {
            continue; // Labels match exactly.
        }
        // Only report violation when arg is labeled and required is bare or a different label.
        // (bare arg to labeled param is caught by the direct type checker, not propagation)
        let Some(ref al) = arg_label else {
            continue; // Cannot determine arg label — skip.
        };
        // Build a simplified call chain for the error message.
        let chain = extract_chain(arg, env, &inferred.explicit, &inferred.inferred);
        errors.push(CheckError::InterprocFlowViolation {
            callee: callee_name.to_string(),
            param_idx,
            required_label: required.unwrap_or("bare").to_string(),
            inferred_label: al.clone(),
            chain,
            caller: caller.to_string(),
            span: call_span,
        });
    }
}

/// Extract a simplified call chain from an arg expression for error messages.
///
/// Returns function/variable names from outermost to innermost, tracing the
/// path through which labeled data flows into the violation.
fn extract_chain(
    expr: &Expr,
    env: &HashMap<String, String>,
    explicit: &HashMap<String, String>,
    inferred: &HashMap<String, String>,
) -> Vec<String> {
    match expr {
        Expr::FnCall { name, args, .. } => {
            let mut chain = vec![name.clone()];
            // Descend into the first arg that contributes a label.
            for arg in args {
                if infer_label_extended(arg, env, explicit, inferred).is_some() {
                    let sub = extract_chain(arg, env, explicit, inferred);
                    if !sub.is_empty() {
                        chain.extend(sub);
                    }
                    break;
                }
            }
            chain
        }
        Expr::Ident(name, _) => vec![name.clone()],
        _ => vec![],
    }
}

use super::ifc::label_of_type_expr;

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
        env.define_fn(
            "source".into(),
            FnInfo {
                ret: Ty::Labeled("Tainted".to_string(), Box::new(Ty::String)),
                ..Default::default()
            },
        );
        // Simulate: fn log_it(q: Clean[String]) -> Unit
        env.define_fn(
            "log_it".into(),
            FnInfo {
                params: vec![Ty::String],
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
        assert_eq!(labels.get("read_line"), Some("Tainted".to_string()));
        assert_eq!(labels.get("args"), Some("Tainted".to_string()));
    }

    #[test]
    fn typeenv_explicit_label_seeded() {
        let prog = parse("");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("source"), Some("Tainted".to_string()));
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
            Some("Tainted".to_string()),
            "wrapper calling Tainted source should be inferred Tainted"
        );
    }

    #[test]
    fn annotated_fn_not_overridden() {
        // fn labeled() -> Tainted[String] { source() }  — explicit annotation wins
        let prog = parse("fn labeled() -> Tainted[String] { source() }");
        let env = env_with_taint_source();
        let labels = propagate(&[&prog], &env);
        // labeled() has explicit annotation → propagation skips it (annotation wins)
        assert_eq!(
            labels.get("labeled"),
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
        assert_eq!(labels.get("step1"), Some("Tainted".to_string()));
        assert_eq!(
            labels.get("step2"),
            Some("Tainted".to_string()),
            "step2 calling step1 (Tainted) should be inferred Tainted"
        );
    }

    #[test]
    fn external_registry_read_line_propagates() {
        // read_line is in TAINT_SOURCES — even without TypeEnv registration
        let prog = parse("fn wrapper() -> String { read_line() }");
        let env = TypeEnv::default();
        let labels = propagate(&[&prog], &env);
        assert_eq!(labels.get("wrapper"), Some("Tainted".to_string()));
    }

    // ── Violation detection tests ─────────────────────────────────────────

    #[test]
    fn no_violations_on_clean_flow() {
        // fn wrapper() -> String { source() }
        // fn caller() -> Unit { log_it(wrapper()) }  — Tainted → Clean[String]: violation!
        // But here we test the CLEAN case: calling with a non-tainted value
        let prog = parse("fn caller() -> Unit { log_it(\"safe\") }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            violations.is_empty(),
            "literal arg to Clean sink should not violate"
        );
    }

    #[test]
    fn violation_detected_tainted_to_clean_log_it() {
        // fn wrapper() -> String { source() }   ← inferred Tainted
        // fn caller() -> Unit { log_it(wrapper()) }  ← Tainted → Clean param
        let prog =
            parse("fn wrapper() -> String { source() } fn caller() -> Unit { log_it(wrapper()) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "Tainted arg to Clean[String] param should produce a violation"
        );
        // Verify the error is tagged as Req 11
        for v in &violations {
            assert_eq!(v.requirement_number(), 11);
        }
    }

    #[test]
    fn violation_chain_extracted() {
        let prog =
            parse("fn wrapper() -> String { source() } fn caller() -> Unit { log_it(wrapper()) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        if let Some(CheckError::InterprocFlowViolation { chain, .. }) = violations.first() {
            assert!(
                chain.contains(&"wrapper".to_string()),
                "chain should include wrapper, got {chain:?}"
            );
        }
    }

    // ── New tests added by review fixes ───────────────────────────────────

    #[test]
    fn three_hop_chain_produces_violation() {
        // Canonical SQL-injection scenario from #831:
        // fn get_input() -> String { read_line() }      ← Tainted via registry
        // fn build_query() -> String { get_input() }    ← inferred Tainted
        // fn caller() -> Unit { log_it(build_query()) }   ← Tainted → Clean: violation
        let prog = parse(
            "fn get_input() -> String { read_line() } \
             fn build_query() -> String { get_input() } \
             fn caller() -> Unit { log_it(build_query()) }",
        );
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        assert_eq!(inferred.get("get_input"), Some("Tainted".to_string()));
        assert_eq!(inferred.get("build_query"), Some("Tainted".to_string()));
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "three-hop taint chain must produce a violation"
        );
        if let Some(CheckError::InterprocFlowViolation { callee, chain, .. }) = violations.first() {
            assert_eq!(callee, "log_it");
            assert!(
                chain.contains(&"build_query".to_string()),
                "chain should trace through build_query, got {chain:?}"
            );
        }
    }

    #[test]
    fn mutual_recursion_propagation_terminates_no_taint() {
        // fn a() -> String { b() }
        // fn b() -> String { a() }
        // Neither touches a taint source → neither inferred Tainted; must not hang.
        let prog = parse("fn a() -> String { b() } fn b() -> String { a() }");
        let env = TypeEnv::default();
        let inferred = propagate(&[&prog], &env);
        assert_eq!(
            inferred.get("a"),
            None,
            "no taint source → a should not be Tainted"
        );
        assert_eq!(
            inferred.get("b"),
            None,
            "no taint source → b should not be Tainted"
        );
    }

    #[test]
    fn mutual_recursion_with_taint_source_propagates() {
        // fn a() -> String { b() }
        // fn b() -> String { read_line() }  ← TAINT_SOURCES entry
        let prog = parse("fn a() -> String { b() } fn b() -> String { read_line() }");
        let env = TypeEnv::default();
        let inferred = propagate(&[&prog], &env);
        assert_eq!(inferred.get("b"), Some("Tainted".to_string()));
        assert_eq!(inferred.get("a"), Some("Tainted".to_string()));
    }

    #[test]
    fn violation_error_fields_are_correct() {
        let prog =
            parse("fn wrapper() -> String { source() } fn caller() -> Unit { log_it(wrapper()) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert_eq!(violations.len(), 1);
        match &violations[0] {
            CheckError::InterprocFlowViolation {
                callee,
                param_idx,
                required_label,
                inferred_label,
                caller,
                ..
            } => {
                assert_eq!(callee, "log_it");
                assert_eq!(*param_idx, 0);
                assert_eq!(required_label, "bare"); // sink takes bare String (#894: no Clean label)
                assert_eq!(inferred_label, "Tainted");
                assert_eq!(caller, "caller");
            }
            other => panic!("expected InterprocFlowViolation, got {other:?}"),
        }
    }

    #[test]
    fn tainted_arg_to_public_sink_is_violation() {
        use crate::mvl::checker::context::FnInfo;
        use crate::mvl::checker::types::Ty;
        let prog = parse(
            "fn wrapper() -> String { source() } fn caller() -> Unit { public_sink(wrapper()) }",
        );
        let mut env = env_with_taint_source();
        env.define_fn(
            "public_sink".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::Unit,
                ..Default::default()
            },
        );
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "Tainted → Public param must be a violation"
        );
    }

    #[test]
    fn let_binding_taint_tracked_to_violation() {
        // let x: String = read_line(); log_it(x)  — taint flows through a let-binding
        // MVL requires explicit type annotations on let; the inferred table is consulted
        // for read_line's label and the binding is tracked in env for subsequent stmts.
        let prog = parse("fn caller() -> Unit { let x: String = read_line(); log_it(x) }");
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "taint through let binding should produce a violation (let x: String = read_line(); log_it(x))"
        );
    }

    // ── Tests for #849, #850, #851 ────────────────────────────────────────

    // #849: label-polymorphic wrapper — relabel trust() strips the Tainted label;
    // the result is bare and satisfies a bare-requiring sink → no violation.
    #[test]
    fn relabel_trust_strips_label_no_violation_at_bare_log_it() {
        // GIVEN: caller passes source() through relabel trust() before sink.
        // relabel trust(source(), "VALIDATED-01") → bare String (Expr::Relabel → None).
        // sink requires bare String → arg_label (None) == required (None) → no violation.
        let prog =
            parse(r#"fn caller() -> Unit { log_it(relabel trust(source(), "VALIDATED-01")) }"#);
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            violations.is_empty(),
            "relabel trust() strips the Tainted label — no violation expected at bare sink, got: {violations:?}"
        );
    }

    #[test]
    fn inferred_wrapper_taint_propagates_through_tainted_arg() {
        // fn wrapper(x: String) -> String { x }  ← unannotated
        // The wrapper has no explicit annotation; its inferred label is None (body is
        // just x which is unlabeled). When called with a Tainted arg, the arg-join in
        // infer_label_extended must pick up the taint → violation at sink.
        let prog = parse(
            "fn wrapper(x: String) -> String { x } \
             fn caller() -> Unit { log_it(wrapper(source())) }",
        );
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "tainted arg through unannotated wrapper must reach sink — got no violations"
        );
    }

    // #851: lambda parameter label is propagated into the lambda body.
    #[test]
    fn lambda_param_label_visible_in_body() {
        // let f: String = |x: Tainted[String]| log_it(x);
        // The lambda param x gets label Tainted in the lambda-local env.
        // collect_violations_in_expr recurses into the lambda body with that env
        // and detects the Tainted→Clean violation on log_it(x).
        // Without fix #851, x is absent from env inside the lambda body → no violation.
        let prog = parse(
            "fn caller() -> Unit { \
                 let f: String = |x: Tainted[String]| log_it(x); \
             }",
        );
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "lambda param x: Tainted[String] passed to Clean sink must produce a violation"
        );
    }

    #[test]
    fn lambda_captures_outer_tainted_variable() {
        // fn caller() -> Unit {
        //   let t: String = source();
        //   let f: String = || log_it(t);
        //   f()
        // }
        // t is tainted in the outer env; the lambda body is analysed with a clone of
        // that env (fix #851), so t is visible as Tainted and log_it(t) produces a violation.
        let prog = parse(
            "fn caller() -> Unit { \
                 let t: String = source(); \
                 let f: String = || log_it(t); \
                 f() \
             }",
        );
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "lambda capturing outer tainted variable t must produce violation on log_it(t)"
        );
    }

    // #858 gap 1: for-loop iterator taint propagates to loop variable.
    #[test]
    fn for_loop_tainted_iterator_propagates_to_body() {
        // for x in source_iter() { log_it(x) }
        // source_iter() is Tainted; x must receive that label so log_it(x) is a violation.
        use crate::mvl::checker::context::FnInfo;
        use crate::mvl::checker::types::Ty;
        let prog = parse(
            "fn caller() -> Unit { \
                 for x in source_iter() { log_it(x) } \
             }",
        );
        let mut env = env_with_taint_source();
        env.define_fn(
            "source_iter".into(),
            FnInfo {
                ret: Ty::Labeled("Tainted".to_string(), Box::new(Ty::String)),
                ..Default::default()
            },
        );
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            !violations.is_empty(),
            "tainted iterator must propagate label to loop variable x → log_it(x) is a violation"
        );
    }

    // #858 gap 3: lambda return type annotation makes the lambda's result tainted.
    #[test]
    fn lambda_return_label_visible_at_call_site() {
        // let f: Tainted[String] = || -> Tainted[String] { "safe" };
        // f's ret_type declares Tainted; calling f() and passing the result to sink
        // must be a violation. The explicit let annotation also labels `f` as Tainted
        // so the intent is clear: f() must be treated as tainted at the call site.
        let prog = parse(
            "fn caller() -> Unit { \
                 let f: Tainted[String] = || -> Tainted[String] { \"safe\" }; \
                 log_it(f()) \
             }",
        );
        let env = env_with_taint_source();
        let inferred = propagate(&[&prog], &env);
        let violations = detect_violations(&prog, &env, &inferred);
        assert!(
            violations
                .iter()
                .any(|e| matches!(e, CheckError::InterprocFlowViolation { callee, .. }
                    if callee == "log_it")),
            "lambda with ret_type Tainted[String] must make f() tainted → log_it(f()) is a violation, got: {violations:?}"
        );
    }
}
