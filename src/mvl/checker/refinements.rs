// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Refinement type checker — symbolic proof for `where` predicates.
//!
//! Implements Requirement 10 of the MVL spec (001-type-system/Req 5).
//!
//! # Approach
//!
//! Three outcomes per call-site argument that has a refined parameter:
//!
//! | Outcome      | Meaning                                                       |
//! |--------------|---------------------------------------------------------------|
//! | Proven       | The argument's value/type statically satisfies the refinement |
//! | RuntimeCheck | Cannot prove statically — runtime assertion needed            |
//! | Failed       | The argument statically violates the refinement               |
//!
//! ## Constraint evaluation strategy
//!
//! - **Literals** (`42`, `0.0`): evaluate the predicate with the literal as `self`.
//! - **Same-refinement variables**: if the argument identifier carries a structurally
//!   equivalent refinement predicate, subsumption is proven.
//! - **Everything else**: falls back to `RuntimeCheck`.
//!
//! This approach covers the acceptance criteria for Phase 3 without requiring
//! an external SMT solver.  Full Z3/CVC5 integration is deferred to a later phase.

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::mvl::checker::const_eval;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::solver::atom_norm::AtomNormalizer;
use crate::mvl::checker::solver::{
    binary_op_to_cmp, dummy_span, try_cooper, try_interval, try_symbolic, try_trivial, try_z3,
    RefResult, SolverMode,
};
use crate::mvl::parser::ast::{
    ArithOp, BinaryOp, CmpOp, Decl, ElseBranch, Expr, FnDecl, LValue, Literal, LogicOp, MatchArm,
    MatchBody, Param, Pattern, Program, RefExpr, Stmt, StringOp, TypeBody, TypeExpr,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::parser::visit::{walk_expr, walk_stmt, Visit};

// ── Counts ────────────────────────────────────────────────────────────────────

/// Per-program refinement check outcome counts.
/// A single refinement proof entry for detailed reporting.
#[derive(Debug, Clone)]
pub struct ProofEntry {
    /// Source file (set by the assurance aggregator, empty inside the checker).
    pub file: String,
    /// 1-based line number of the call site / contract.
    pub line: u32,
    /// Function containing the call site (caller).
    pub caller: String,
    /// Function or contract being checked (callee / "ensures" / "requires").
    pub callee: String,
    /// Human-readable predicate that was proven.
    pub predicate: String,
    /// Layer that proved it (1–5), or 0 for runtime-checked.
    pub layer: usize,
}

#[derive(Debug, Default, Clone)]
pub struct RefinementCounts {
    /// Solver mode used for this run.
    pub mode: SolverMode,
    /// Call-site arguments proven to satisfy their refinement statically.
    pub proven: usize,
    /// Call-site arguments that could not be proven; will need runtime checks.
    pub runtime_checked: usize,
    /// Call-site arguments definitively known to violate their refinement.
    pub failed: usize,
    /// Per-layer proof counts: `by_layer[n]` = number of proofs by Layer n.
    /// Index 0 is unused (layers are 1–5).
    pub by_layer: [usize; 6],
    /// Functions in this program that have at least one refined call site
    /// (including calls to imported refined functions from prelude modules).
    pub fn_total: usize,
    /// Subset of `fn_total` where ALL refined call sites are statically proven.
    pub fully_verified_fns: usize,
    /// Detailed proof log (populated only in verbose/assurance mode).
    /// Records ONLY successfully-proven sites — consumed by the assurance dashboard.
    pub proof_log: Vec<ProofEntry>,
    /// Per-call-site proof records covering ALL outcomes (proven / runtime / failed).
    /// Populated unconditionally; consumed by `mvl prove` (#836) for the breakdown report.
    pub sites: Vec<ProofSite>,
    /// Axis 2 (#1931): proven `ensures` clauses where a tighter bound is also provable.
    /// Populated during `check_ensures_for_return`; consumed by `mvl harden`.
    pub tightening_candidates: Vec<TighteningCandidate>,
    /// Name of the function currently being analyzed (set by `check_refinements` before
    /// each `analyze_block` call so `ProofSite::caller_fn` can be populated without
    /// threading the name through every recursive helper).
    pub current_fn: String,
}

// ── Axis 2: contract tightening candidates (#1931) ────────────────────────────

/// A proven `ensures` clause where Z3 can prove a strictly tighter bound.
///
/// Collected during `check_ensures_for_return` and surfaced by `mvl harden`.
/// One candidate is emitted per return point (branch); callers should
/// deduplicate by `(fn_name, declared_pred)` keeping the weakest tighter bound
/// (min for `>=`/`>`, max for `<=`/`<`) to produce a globally-sound suggestion.
#[derive(Debug, Clone)]
pub struct TighteningCandidate {
    /// Function whose postcondition could be tightened.
    pub fn_name: String,
    /// The declared predicate string (e.g. `"ensures result >= 0"`).
    pub declared_pred: String,
    /// The tighter predicate Z3 can prove (e.g. `"ensures result >= 5"`).
    pub tighter_pred: String,
    /// Raw tighter bound value for deduplication arithmetic.
    pub tighter_bound: i64,
    /// Whether to find the min (Ge/Gt) or max (Le/Lt) across branches.
    /// `true` = take the minimum tighter bound (lower-bound predicates).
    pub take_min: bool,
    /// Source location of the return expression that was proven.
    pub span: Span,
    // ── Axis 3: boundary witness synthesis (#1931) ─────────────────────────
    /// Function parameters — used by `mvl harden --emit-tests` to synthesize
    /// call arguments.  Cloned from `FnDecl.params` at contract-check time.
    pub params: Vec<Param>,
    /// Branch conditions active at this return point — used as Z3 constraints
    /// when searching for a witness input that reaches this return path.
    pub branch_hyps: Vec<Expr>,
}

// ── Axis 3: boundary witness synthesis (#1931) ────────────────────────────────

/// A concrete value found by Z3 as a witness for a boundary test input.
#[derive(Debug, Clone)]
pub enum WitnessValue {
    /// A concrete integer (covers Int, Bool-as-int, refined Int, etc.).
    Int(i64),
    /// A struct constructed from field witnesses.
    Struct {
        type_name: String,
        fields: Vec<(String, WitnessValue)>,
    },
    /// Z3 returned unknown or the param type is unsupported.
    Unknown,
}

/// A single function parameter bound to a witness value.
#[derive(Debug, Clone)]
pub struct WitnessArg {
    pub param_name: String,
    pub value: WitnessValue,
}

// ── Per-call-site proof records (#836) ────────────────────────────────────────

/// Outcome of a single call-site refinement check.
#[derive(Debug, Clone)]
pub enum ProofOutcome {
    /// Proven statically at the given solver layer (1–5).
    /// `is_bv = true` when Layer 5 used Z3 QF-BV (bit-vector theory) (#1928).
    Proven { layer: usize, is_bv: bool },
    /// Could not prove statically; a runtime assertion will be emitted.
    RuntimeCheck,
    /// Could not prove statically, but Z3 found a concrete counter-example
    /// showing how the predicate can fail (#1896).
    RuntimeCheckWithWitness { counterexample: String },
    /// Statically violated — the argument provably breaks the predicate.
    Failed,
}

/// Per-call-site record of a refinement proof attempt.
#[derive(Debug, Clone)]
pub struct ProofSite {
    /// Name of the function containing the call (caller).
    pub caller_fn: String,
    /// Name of the function being called (callee).
    pub fn_name: String,
    /// Name of the refined parameter whose predicate was checked.
    pub param_name: String,
    /// Human-readable predicate string (e.g. `"self > 0"`).
    pub predicate: String,
    /// Source location of the call expression.
    pub span: Span,
    /// What the solver determined for this site.
    pub outcome: ProofOutcome,
}

/// Axis 3 witness synthesis — public thin wrapper over the `pub(crate)` layer5 entry point.
///
/// Exposed here so the CLI crate (`src/cli/harden.rs`) can call it without
/// requiring access to the private `checker::solver` module.
pub fn synthesize_witness(
    params: &[crate::mvl::parser::ast::Param],
    branch_hyps: &[crate::mvl::parser::ast::Expr],
    struct_fields: &std::collections::HashMap<String, Vec<(String, String)>>,
) -> Option<Vec<WitnessArg>> {
    crate::mvl::checker::solver::layer5::try_z3_witness(params, branch_hyps, struct_fields)
}

// ── Entry points ──────────────────────────────────────────────────────────────

/// Emit [`CheckError::RefinementViolated`] for every definite predicate violation.
///
/// Called from `checker::check()` after the main type-checking pass.
/// Accepts prelude slices so that calls to imported functions with refined
/// parameters are checked — `fn_total` / `fully_verified_fns` then count
/// cross-module call sites, not just same-file ones.
/// Returns aggregated counts of proven / runtime-checked / failed checks.
pub fn check_refinements(
    prelude_a: &[Program],
    prelude_b: &[&Program],
    prog: &Program,
    errors: &mut Vec<CheckError>,
    mode: SolverMode,
) -> RefinementCounts {
    let mut counts = RefinementCounts {
        mode,
        ..Default::default()
    };
    // Build fn_params from ALL programs so cross-module refined call sites
    // (e.g. calling a prelude function with `where` params) are checked.
    let all_progs: Vec<&Program> = prelude_a
        .iter()
        .chain(prelude_b.iter().copied())
        .chain(std::iter::once(prog))
        .collect();
    let fn_params = build_fn_param_refinements_combined(&all_progs);
    let fn_ensures = build_fn_ensures_combined(&all_progs);
    let type_refs = build_type_alias_refinements(prog);
    let struct_fields = build_struct_field_refinements_combined(&all_progs);
    let fn_decls = build_pure_fn_decls(prog);
    // #1805 follow-up: hoist top-level `const` decls into `self == value`
    // hypotheses so bare Ident uses reach L1 as concrete integers.
    let const_map = build_const_map(&all_progs);
    let const_refs = const_map_to_var_refs(&const_map);
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                counts.current_fn = fd.name.clone();
                let var_refs = param_refinements_full(fd, &type_refs, &struct_fields, &const_refs);
                RefinementAnalyzer::new(
                    var_refs,
                    &fn_params,
                    &fn_ensures,
                    &type_refs,
                    &fn_decls,
                    errors,
                    &mut counts,
                )
                .visit_block(&fd.body);
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    counts.current_fn = method.name.clone();
                    let var_refs =
                        param_refinements_full(method, &type_refs, &struct_fields, &const_refs);
                    RefinementAnalyzer::new(
                        var_refs,
                        &fn_params,
                        &fn_ensures,
                        &type_refs,
                        &fn_decls,
                        errors,
                        &mut counts,
                    )
                    .visit_block(&method.body);
                }
            }
            // D2 (Phase 8, #37): Check refinements inside actor behavior bodies.
            // Behaviors may call functions with `where` refinements on their
            // parameters; the same 5-layer solver applies.
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    counts.current_fn = format!("{}::{}", ad.name, method.name);
                    let var_refs = params_to_var_refs_full(
                        &method.params,
                        &type_refs,
                        &struct_fields,
                        &const_refs,
                    );
                    RefinementAnalyzer::new(
                        var_refs,
                        &fn_params,
                        &fn_ensures,
                        &type_refs,
                        &fn_decls,
                        errors,
                        &mut counts,
                    )
                    .visit_block(&method.body);
                }
            }
            _ => {}
        }
    }

    // Compute fn_total / fully_verified_fns: per-function counts using the
    // same combined fn_params so cross-module refined call sites are counted.
    for decl in &prog.declarations {
        let fns: Vec<&FnDecl> = match decl {
            Decl::Fn(fd) => vec![fd],
            Decl::Impl(id) => id.methods.iter().collect(),
            _ => vec![],
        };
        for fd in fns {
            let var_refs = param_refinements_full(fd, &type_refs, &struct_fields, &const_refs);
            let mut per_fn_errors = Vec::new();
            let mut per_fn_counts = RefinementCounts::default();
            RefinementAnalyzer::new(
                var_refs,
                &fn_params,
                &fn_ensures,
                &type_refs,
                &fn_decls,
                &mut per_fn_errors,
                &mut per_fn_counts,
            )
            .visit_block(&fd.body);
            let total = per_fn_counts.proven + per_fn_counts.runtime_checked + per_fn_counts.failed;
            if total > 0 {
                counts.fn_total += 1;
                if per_fn_counts.runtime_checked == 0 && per_fn_counts.failed == 0 {
                    counts.fully_verified_fns += 1;
                }
            }
        }
        if let Decl::Actor(ad) = decl {
            for method in &ad.methods {
                let var_refs = params_to_var_refs_full(
                    &method.params,
                    &type_refs,
                    &struct_fields,
                    &const_refs,
                );
                let mut per_fn_errors = Vec::new();
                let mut per_fn_counts = RefinementCounts::default();
                RefinementAnalyzer::new(
                    var_refs,
                    &fn_params,
                    &fn_ensures,
                    &type_refs,
                    &fn_decls,
                    &mut per_fn_errors,
                    &mut per_fn_counts,
                )
                .visit_block(&method.body);
                let total =
                    per_fn_counts.proven + per_fn_counts.runtime_checked + per_fn_counts.failed;
                if total > 0 {
                    counts.fn_total += 1;
                    if per_fn_counts.runtime_checked == 0 && per_fn_counts.failed == 0 {
                        counts.fully_verified_fns += 1;
                    }
                }
            }
        }
    }

    counts
}

/// Count proven / runtime-checked / failed refinement call sites.
///
/// Does not emit errors; used by [`crate::mvl::checker::passes::RefinementsPass`]
/// to build the assurance verdict.
pub fn count_refinements(prog: &Program) -> RefinementCounts {
    let mut errors = Vec::new();
    let mut counts = RefinementCounts {
        mode: SolverMode::Layered,
        ..Default::default()
    };
    let fn_params = build_fn_param_refinements(prog);
    let fn_ensures = build_fn_ensures_combined(&[prog]);
    let type_refs = build_type_alias_refinements(prog);
    let struct_fields = build_struct_field_refinements(prog);
    let fn_decls = build_pure_fn_decls(prog);
    let const_map = build_const_map(&[prog]);
    let const_refs = const_map_to_var_refs(&const_map);
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) => {
                let var_refs = param_refinements_full(fd, &type_refs, &struct_fields, &const_refs);
                RefinementAnalyzer::new(
                    var_refs,
                    &fn_params,
                    &fn_ensures,
                    &type_refs,
                    &fn_decls,
                    &mut errors,
                    &mut counts,
                )
                .visit_block(&fd.body);
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    let var_refs =
                        param_refinements_full(method, &type_refs, &struct_fields, &const_refs);
                    RefinementAnalyzer::new(
                        var_refs,
                        &fn_params,
                        &fn_ensures,
                        &type_refs,
                        &fn_decls,
                        &mut errors,
                        &mut counts,
                    )
                    .visit_block(&method.body);
                }
            }
            Decl::Actor(ad) => {
                for method in &ad.methods {
                    let var_refs = params_to_var_refs_full(
                        &method.params,
                        &type_refs,
                        &struct_fields,
                        &const_refs,
                    );
                    RefinementAnalyzer::new(
                        var_refs,
                        &fn_params,
                        &fn_ensures,
                        &type_refs,
                        &fn_decls,
                        &mut errors,
                        &mut counts,
                    )
                    .visit_block(&method.body);
                }
            }
            _ => {}
        }
    }
    counts
}

/// Count functions where every refined call site is statically proven.
///
/// Returns `(fully_verified, fn_total)` where:
/// - `fn_total`: functions that have at least one refined call site
/// - `fully_verified`: subset where all refined call sites are `Proven` (none runtime-checked)
///
/// Used by [`crate::mvl::checker::passes::RefinementsPass`] to build the coverage report.
pub fn count_fully_verified_fns(prog: &Program) -> (usize, usize) {
    let fn_params = build_fn_param_refinements(prog);
    let fn_ensures = build_fn_ensures_combined(&[prog]);
    let type_refs = build_type_alias_refinements(prog);
    let struct_fields = build_struct_field_refinements(prog);
    let fn_decls = build_pure_fn_decls(prog);
    let const_map = build_const_map(&[prog]);
    let const_refs = const_map_to_var_refs(&const_map);
    let mut fn_total = 0usize;
    let mut fully_verified = 0usize;

    for decl in &prog.declarations {
        let fns: Vec<&FnDecl> = match decl {
            Decl::Fn(fd) => vec![fd],
            Decl::Impl(id) => id.methods.iter().collect(),
            _ => vec![],
        };
        for fd in fns {
            let var_refs = param_refinements_full(fd, &type_refs, &struct_fields, &const_refs);
            let mut errors = Vec::new();
            let mut counts = RefinementCounts::default();
            RefinementAnalyzer::new(
                var_refs,
                &fn_params,
                &fn_ensures,
                &type_refs,
                &fn_decls,
                &mut errors,
                &mut counts,
            )
            .visit_block(&fd.body);
            let total = counts.proven + counts.runtime_checked + counts.failed;
            if total > 0 {
                fn_total += 1;
                if counts.runtime_checked == 0 && counts.failed == 0 {
                    fully_verified += 1;
                }
            }
        }
        // Actor behavior methods must also be counted.
        if let Decl::Actor(ad) = decl {
            for method in &ad.methods {
                let var_refs = params_to_var_refs_full(
                    &method.params,
                    &type_refs,
                    &struct_fields,
                    &const_refs,
                );
                let mut errors = Vec::new();
                let mut counts = RefinementCounts::default();
                RefinementAnalyzer::new(
                    var_refs,
                    &fn_params,
                    &fn_ensures,
                    &type_refs,
                    &fn_decls,
                    &mut errors,
                    &mut counts,
                )
                .visit_block(&method.body);
                let total = counts.proven + counts.runtime_checked + counts.failed;
                if total > 0 {
                    fn_total += 1;
                    if counts.runtime_checked == 0 && counts.failed == 0 {
                        fully_verified += 1;
                    }
                }
            }
        }
    }
    (fully_verified, fn_total)
}

// ── Lookup table builders ─────────────────────────────────────────────────────

/// Maps pure function name → `FnDecl` for compile-time constant folding.
///
/// Only pure functions (empty effects list) are included; effectful functions
/// cannot be safely evaluated at compile time.
///
/// Both top-level `fn` declarations and pure methods inside `impl` blocks are
/// collected.  Methods are registered under their bare name; if two methods on
/// different types share the same name the last one wins (acceptable — folding
/// is conservative and `None` is always a safe fallback).
fn build_pure_fn_decls(prog: &Program) -> HashMap<String, FnDecl> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Fn(fd) if fd.effects.is_empty() => {
                map.insert(fd.name.clone(), fd.clone());
            }
            Decl::Impl(impl_decl) => {
                for method in &impl_decl.methods {
                    if method.effects.is_empty() {
                        map.insert(method.name.clone(), method.clone());
                    }
                }
            }
            _ => {}
        }
    }
    map
}

/// Walk every `Decl::Const` across the passed programs and evaluate its
/// initializer to a `ConstValue` (#1805 follow-up).  Only trivially-foldable
/// initializers land in the map; anything the const evaluator can't reduce
/// is silently omitted (the same conservatism `try_fold_call` uses).
///
/// The resulting map is threaded into `var_refs` as `self == value`
/// hypotheses so bare `Expr::Ident(NAME)` uses of a top-level `const`
/// reach L1's existing Ident handler with a usable equality — matching the
/// behaviour of the older `pub total fn NAME() -> T { LITERAL }` idiom.
pub(crate) fn build_const_map(
    progs: &[&Program],
) -> HashMap<String, crate::mvl::checker::const_eval::ConstValue> {
    let mut map = HashMap::new();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Const(cd) = decl {
                if let Some(v) = crate::mvl::checker::const_eval::expr_as_const(&cd.value) {
                    map.insert(cd.name.clone(), v);
                }
            }
        }
    }
    map
}

/// Convert const values to `var_refs` entries as `self == n` hypotheses.
/// Non-numeric consts (strings, unit) are skipped — the solver reasons over
/// integers and booleans only.  Booleans encode as `self == 0` / `self == 1`
/// so downstream integer-domain layers can pick them up (L1's bool handler
/// also picks up the RefExpr::Bool comparison shape via a separate path).
pub(crate) fn const_map_to_var_refs(
    consts: &HashMap<String, crate::mvl::checker::const_eval::ConstValue>,
) -> HashMap<String, Option<RefExpr>> {
    use crate::mvl::checker::const_eval::ConstValue;
    let s = dummy_span();
    let mut map = HashMap::new();
    for (name, cv) in consts {
        let n = match cv {
            ConstValue::Integer(n) => *n,
            ConstValue::Bool(b) => {
                if *b {
                    1
                } else {
                    0
                }
            }
            _ => continue,
        };
        let pred = RefExpr::Compare {
            op: CmpOp::Eq,
            left: Box::new(RefExpr::Ident {
                name: "self".to_string(),
                span: s,
            }),
            right: Box::new(RefExpr::Integer { value: n, span: s }),
            span: s,
        };
        map.insert(name.clone(), Some(pred));
    }
    map
}

/// Maps function/method name → its `ensures` postconditions expressed as
/// `RefExpr`s in `self`-form (#1805 MethodCall axiom projection).
///
/// Postconditions that can be lowered via [`expr_to_ref_expr_ext`] are kept;
/// the rest are dropped (they still get checked by `contracts::check_contracts`
/// but cannot be used as static hypotheses).  `result` is normalized to `self`
/// so downstream code can index the RefExpr against an atom whose `self` is
/// the method's return value.
///
/// Builtin methods like `List[T]::len` — declared with `ensures result >= 0`
/// — surface here so that a call site `xs.len()` can inject `self >= 0` as
/// a hypothesis under the canonical key `"xs.len()"`.
fn build_fn_ensures_combined(progs: &[&Program]) -> HashMap<String, Vec<RefExpr>> {
    let mut map: HashMap<String, Vec<RefExpr>> = HashMap::new();
    for prog in progs {
        for decl in &prog.declarations {
            let fns: Vec<&FnDecl> = match decl {
                Decl::Fn(fd) => vec![fd],
                Decl::Impl(id) => id.methods.iter().collect(),
                _ => vec![],
            };
            for fd in fns {
                if fd.ensures.is_empty() {
                    continue;
                }
                let mut lowered = Vec::new();
                for e in &fd.ensures {
                    let span = crate::mvl::checker::solver::dummy_span();
                    if let Some(r) = crate::mvl::parser::ast::expr_to_ref_expr_ext(e, span) {
                        lowered.push(normalize_pred(&r, "result"));
                    }
                }
                if !lowered.is_empty() {
                    // If multiple methods share a name, later definitions win.
                    // Acceptable — the axiom is conservative and the analysis
                    // is aware of this loss at hypothesis-lookup time.
                    map.insert(fd.name.clone(), lowered);
                }
            }
        }
    }
    map
}

/// Maps function name → `Vec<(param_name, Option<RefExpr>)>` for top-level functions.
fn build_fn_param_refinements(prog: &Program) -> HashMap<String, Vec<(String, Option<RefExpr>)>> {
    build_fn_param_refinements_combined(&[prog])
}

/// Build the refinement parameter map from multiple programs (e.g. prelude + prog).
/// Used by [`check_refinements`] to enable cross-module refined call-site checking.
fn build_fn_param_refinements_combined(
    progs: &[&Program],
) -> HashMap<String, Vec<(String, Option<RefExpr>)>> {
    let mut map = HashMap::new();
    for prog in progs {
        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) => {
                    map.insert(fd.name.clone(), param_ref_vec(fd));
                }
                Decl::Impl(impl_decl) => {
                    for method in &impl_decl.methods {
                        // Methods are registered under their bare name for simplicity;
                        // collision between methods on different types is acceptable
                        // at this phase — the analysis is conservative.
                        map.insert(method.name.clone(), param_ref_vec(method));
                    }
                }
                _ => {}
            }
        }
    }
    map
}

fn param_ref_vec(fd: &FnDecl) -> Vec<(String, Option<RefExpr>)> {
    fd.params
        .iter()
        .map(|p| {
            // Normalise the parameter name to "self" so that predicates written
            // as `b != 0` (where `b` is the param name) compare equal to
            // `self != 0` and to caller-side predicates like `y != 0`.
            let pred = p.refinement.as_ref().map(|r| normalize_pred(r, &p.name));
            (p.name.clone(), pred)
        })
        .collect()
}

/// Maps struct name → its per-field refinements (#1805 hypothesis threading).
///
/// E.g. `type Box = struct { size: Int where self > 5 }`
/// → `"Box" → { "size" → RefExpr(self > 5) }`.
///
/// Used by [`params_to_var_refs`] to project field-level invariants of a
/// struct-typed parameter into synthetic `"param.field"` hypothesis keys, so
/// that the solver's atom normalizer (`solver::atom_norm`) can bridge them
/// onto atom names when a `FieldAccess` argument is seen at a call site.
fn build_struct_field_refinements(prog: &Program) -> HashMap<String, HashMap<String, RefExpr>> {
    build_struct_field_refinements_combined(&[prog])
}

/// Multi-program variant of [`build_struct_field_refinements`] — needed for
/// projects that split struct declarations across multiple `.mvl` files
/// (e.g. pong keeps `Field`, `Ball` in `models.mvl` but consumes them in
/// `game.mvl`).  Prior to this the single-file variant was called on the
/// current file only, so cross-module refined fields silently lost their
/// hypothesis.
pub(crate) fn build_struct_field_refinements_combined(
    progs: &[&Program],
) -> HashMap<String, HashMap<String, RefExpr>> {
    let mut map: HashMap<String, HashMap<String, RefExpr>> = HashMap::new();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                if let TypeBody::Struct { fields, .. } = &td.body {
                    let mut fmap = HashMap::new();
                    for f in fields {
                        if let Some(pred) = &f.refinement {
                            fmap.insert(f.name.clone(), pred.clone());
                        }
                    }
                    if !fmap.is_empty() {
                        // Later definitions win — MVL's parser rejects
                        // duplicate `type` names across a program so this
                        // path only fires when a type is defined once and
                        // referenced from another file.
                        map.insert(td.name.clone(), fmap);
                    }
                }
            }
        }
    }
    map
}

/// Maps type alias name → the refinement attached to that alias (if any).
///
/// E.g. `type PositiveInt = Int where self > 0` → `"PositiveInt" → Some(self > 0)`.
pub(crate) fn build_type_alias_refinements(prog: &Program) -> HashMap<String, Option<RefExpr>> {
    let mut map = HashMap::new();
    for decl in &prog.declarations {
        if let Decl::Type(td) = decl {
            // Only simple `type A = B where pred` aliases carry a refinement.
            // Struct / enum bodies do not resolve to a single predicate here.
            let pred = match &td.body {
                TypeBody::Alias(inner) => extract_type_refinement(inner),
                _ => None,
            };
            map.insert(td.name.clone(), pred);
        }
    }
    map
}

/// Multi-file variant: merge type-alias refinements from all loaded programs.
///
/// Used by the contracts checker so that `ensures` / `requires` clauses over
/// parameters typed with a refined alias defined in an imported module (e.g.
/// `s: SafeSqlParam` where `SafeSqlParam` is in `model.mvl`) see the correct
/// type predicate in `var_refs`.  Without this, cross-module refined type
/// aliases resolve to `None` and the solver cannot discharge the obligation.
pub(crate) fn build_type_alias_refinements_combined(
    progs: &[&Program],
) -> HashMap<String, Option<RefExpr>> {
    let mut map = HashMap::new();
    for prog in progs {
        map.extend(build_type_alias_refinements(prog));
    }
    map
}

/// Extract the outermost refinement from a `TypeExpr`, if present.
fn extract_type_refinement(ty: &TypeExpr) -> Option<RefExpr> {
    match ty {
        TypeExpr::Refined { pred, .. } => Some(pred.clone()),
        _ => None,
    }
}

/// Build the variable-refinement map for a function's own parameters,
/// seeding top-level const hypotheses (#1805 follow-up).
///
/// Inline refinements are normalised so the parameter name becomes `"self"`,
/// matching the canonical form used in type aliases and in the callee table.
fn param_refinements_full(
    fd: &FnDecl,
    type_refs: &HashMap<String, Option<RefExpr>>,
    struct_fields: &HashMap<String, HashMap<String, RefExpr>>,
    const_refs: &HashMap<String, Option<RefExpr>>,
) -> HashMap<String, Option<RefExpr>> {
    params_to_var_refs_full(&fd.params, type_refs, struct_fields, const_refs)
}

/// Extend a `var_refs` map with the const hypotheses in `const_refs` (#1805
/// follow-up).  Existing entries win — a shadowing param name keeps its
/// parameter-derived hypothesis instead of the const value.
pub(crate) fn merge_consts_into_var_refs(
    var_refs: &mut HashMap<String, Option<RefExpr>>,
    const_refs: &HashMap<String, Option<RefExpr>>,
) {
    for (k, v) in const_refs {
        var_refs.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Build var_refs from a slice of parameters (used for both `FnDecl` and `ActorMethod`).
///
/// Also projects struct-field invariants into synthetic hypothesis keys of
/// the form `"param.field"` (#1805).  These keys line up with the canonical
/// form produced by [`solver::atom_norm::AtomNormalizer`], enabling
/// arithmetic layers to see a hypothesis for a `FieldAccess` argument.
#[allow(dead_code)] // kept for external callers; internal uses go through
                    // `params_to_var_refs_full` so const hypotheses are always
                    // in scope.
pub(crate) fn params_to_var_refs(
    params: &[Param],
    type_refs: &HashMap<String, Option<RefExpr>>,
    struct_fields: &HashMap<String, HashMap<String, RefExpr>>,
) -> HashMap<String, Option<RefExpr>> {
    params_to_var_refs_full(params, type_refs, struct_fields, &HashMap::new())
}

/// Full variant that also seeds `self == value` hypotheses for top-level
/// `const` declarations (#1805 follow-up).  Const hypotheses are added
/// only for names not already bound by a param or struct-field projection,
/// so a param shadowing a const keeps the parameter-derived binding.
pub(crate) fn params_to_var_refs_full(
    params: &[Param],
    type_refs: &HashMap<String, Option<RefExpr>>,
    struct_fields: &HashMap<String, HashMap<String, RefExpr>>,
    const_refs: &HashMap<String, Option<RefExpr>>,
) -> HashMap<String, Option<RefExpr>> {
    let mut map = HashMap::new();
    for p in params {
        // Inline refinement takes priority; normalise param name → "self".
        let pred = p
            .refinement
            .as_ref()
            .map(|r| normalize_pred(r, &p.name))
            .or_else(|| resolve_type_alias_pred(&p.ty, type_refs));
        map.insert(p.name.clone(), pred);

        // Struct-field projection: if this param is a struct with refined
        // fields, synthesize a hypothesis for each such field under the key
        // `"param.field"`.  Field refinements already use `self` to refer to
        // the field value, so no rewriting is needed.
        if let Some(struct_name) = struct_type_name(&p.ty) {
            if let Some(fmap) = struct_fields.get(struct_name) {
                for (fname, fpred) in fmap {
                    let key = format!("{}.{}", p.name, fname);
                    map.insert(key, Some(fpred.clone()));
                }
            }
        }
    }
    // Merge const hypotheses last so params/fields shadow.
    merge_consts_into_var_refs(&mut map, const_refs);
    map
}

/// Unwrap `Refined` / `Ref` / `Labeled` wrappers and return the base struct
/// name, if the type ultimately resolves to a named struct.  Returns `None`
/// for `Option`, `Result`, generics, and non-Base types — those cases are
/// out of scope for this projection.
fn struct_type_name(ty: &TypeExpr) -> Option<&str> {
    match ty {
        TypeExpr::Base { name, .. } => Some(name.as_str()),
        TypeExpr::Refined { inner, .. }
        | TypeExpr::Ref { inner, .. }
        | TypeExpr::Labeled { inner, .. } => struct_type_name(inner),
        _ => None,
    }
}

// ── Synthetic predicate helpers ──────────────────────────────────────────────

/// `self == n` (integer literal equality).
fn self_eq_int(n: i64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Eq,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Integer { value: n, span: s }),
        span: s,
    }
}

/// `self != n` (integer literal inequality).
fn self_ne_int(n: i64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Ne,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Integer { value: n, span: s }),
        span: s,
    }
}

/// `self == f` (float literal equality).
fn self_eq_float(f: f64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Eq,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Float { value: f, span: s }),
        span: s,
    }
}

/// `self != f` (float literal inequality).
fn self_ne_float(f: f64) -> RefExpr {
    let s = dummy_span();
    RefExpr::Compare {
        op: CmpOp::Ne,
        left: Box::new(RefExpr::Ident {
            name: "self".to_string(),
            span: s,
        }),
        right: Box::new(RefExpr::Float { value: f, span: s }),
        span: s,
    }
}

/// Conjoin a non-empty list of predicates with `&&`.  Returns `None` when empty.
fn conj_preds(preds: Vec<RefExpr>) -> Option<RefExpr> {
    let s = dummy_span();
    let mut iter = preds.into_iter();
    let first = iter.next()?;
    Some(iter.fold(first, |acc, p| RefExpr::LogicOp {
        op: LogicOp::And,
        left: Box::new(acc),
        right: Box::new(p),
        span: s,
    }))
}

/// Build a `self == value` refinement predicate from a numeric `ConstValue`.
///
/// Used when a `let` binding is initialised with a constant-folded pure-function
/// call — we inject `self == <folded_value>` into `var_refs` so that the
/// refinement solver can statically prove predicates on that variable.
///
/// Returns `None` for non-numeric values (`Bool`, `Str`, `Unit`) because the
/// refinement language has no literal form for those types. Callers must skip
/// insertion into `var_refs` when `None` is returned.
fn lit_eq_pred(cv: &const_eval::ConstValue) -> Option<RefExpr> {
    let dummy = Span::default();
    let self_ref = Box::new(RefExpr::Ident {
        name: "self".to_string(),
        span: dummy,
    });
    let rhs = match cv {
        const_eval::ConstValue::Integer(n) => Box::new(RefExpr::Integer {
            value: *n,
            span: dummy,
        }),
        const_eval::ConstValue::Float(f) => Box::new(RefExpr::Float {
            value: *f,
            span: dummy,
        }),
        // Non-numeric folded values have no useful refinement hypothesis.
        _ => return None,
    };
    Some(RefExpr::Compare {
        op: CmpOp::Eq,
        left: self_ref,
        right: rhs,
        span: dummy,
    })
}

/// Extract the identifier name from a simple `Expr::Ident`, if present.
fn ident_name_from_expr(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        _ => None,
    }
}

/// Inject pattern-induced narrowing hypotheses into `arm_refs` for one match arm.
///
/// Four kinds of hypotheses are generated:
///
/// 1. **Literal arm** — `0 => ...` tells the solver the scrutinee equals `0`.
///    A guard on a literal arm (unusual but valid) is conjoined with the equality.
///    NaN float literals are skipped — no hypothesis is injected.
/// 2. **Catch-all ident arm** — `n => ...` after literal arms `0`, `1` tells the
///    solver that `n != 0 && n != 1` (complement of all prior literal values).
///    The complement is also written under the scrutinee name so that passing
///    either `n` or `x` to a callee proves the same refinement.
/// 3. **Wildcard arm** — `_ => ...` after literal arms gets the same complement
///    hypothesis injected under the scrutinee name.
/// 4. **Guard** — `n if n > 0 => ...` adds `n > 0` as a hypothesis for `n`.
fn inject_arm_hypotheses(
    scrutinee_name: Option<&str>,
    pattern: &Pattern,
    guard: Option<&RefExpr>,
    prior_int_lits: &[i64],
    prior_float_lits: &[f64],
    arm_refs: &mut HashMap<String, Option<RefExpr>>,
) {
    match pattern {
        // ── Literal arms: scrutinee is known to equal the matched literal ──────
        Pattern::Literal(Literal::Integer(n), _) => {
            if let Some(name) = scrutinee_name {
                let eq_hyp = self_eq_int(*n);
                let hyp = if let Some(g) = guard {
                    let normalized = normalize_pred(g, name);
                    RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(eq_hyp),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    }
                } else {
                    eq_hyp
                };
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // NaN cannot be a concrete equality hypothesis (NaN != NaN in IEEE 754).
        Pattern::Literal(Literal::Float(f), _) if !f.is_nan() => {
            if let Some(name) = scrutinee_name {
                let eq_hyp = self_eq_float(*f);
                let hyp = if let Some(g) = guard {
                    let normalized = normalize_pred(g, name);
                    RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(eq_hyp),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    }
                } else {
                    eq_hyp
                };
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // ── Catch-all ident: bound variable differs from all prior literals ────
        Pattern::Ident(var_name, _) => {
            let mut ne_preds: Vec<RefExpr> =
                prior_int_lits.iter().map(|&n| self_ne_int(n)).collect();
            ne_preds.extend(
                prior_float_lits
                    .iter()
                    .filter(|f| !f.is_nan())
                    .map(|&f| self_ne_float(f)),
            );
            let base_hyp = conj_preds(ne_preds);
            // Merge with guard predicate (if any).
            let hyp = match (base_hyp, guard) {
                (Some(base), Some(g)) => {
                    let normalized = normalize_pred(g, var_name);
                    Some(RefExpr::LogicOp {
                        op: LogicOp::And,
                        left: Box::new(base),
                        right: Box::new(normalized),
                        span: dummy_span(),
                    })
                }
                (Some(base), None) => Some(base),
                (None, Some(g)) => Some(normalize_pred(g, var_name)),
                (None, None) => None,
            };
            if let Some(h) = &hyp {
                arm_refs.insert(var_name.clone(), Some(h.clone()));
                // The scrutinee and the bound variable carry the same value;
                // narrow both so callers can use either name.
                if let Some(sname) = scrutinee_name {
                    if sname != var_name.as_str() {
                        arm_refs.insert(sname.to_string(), Some(h.clone()));
                    }
                }
            }
        }
        // ── Wildcard: complement of all prior literals on the scrutinee ───────
        Pattern::Wildcard(_) => {
            let mut ne_preds: Vec<RefExpr> =
                prior_int_lits.iter().map(|&n| self_ne_int(n)).collect();
            ne_preds.extend(
                prior_float_lits
                    .iter()
                    .filter(|f| !f.is_nan())
                    .map(|&f| self_ne_float(f)),
            );
            if let (Some(name), Some(hyp)) = (scrutinee_name, conj_preds(ne_preds)) {
                arm_refs.insert(name.to_string(), Some(hyp));
            }
        }
        // ── Other patterns: no scalar refinement hypothesis ───────────────────
        _ => {}
    }
}

// ── Predicate normalisation ───────────────────────────────────────────────────

/// Replace every occurrence of `param_name` with `"self"` in a predicate.
///
/// This canonicalises predicates written as `b != 0` (where `b` is the param
/// name) into `self != 0`, so that structural comparison works regardless of
/// what the parameter is called in different functions.
fn normalize_pred(pred: &RefExpr, param_name: &str) -> RefExpr {
    match pred {
        RefExpr::Ident { name, span } => RefExpr::Ident {
            name: if name == param_name {
                "self".to_string()
            } else {
                name.clone()
            },
            span: *span,
        },
        RefExpr::Compare {
            op,
            left,
            right,
            span,
        } => RefExpr::Compare {
            op: *op,
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::LogicOp {
            op,
            left,
            right,
            span,
        } => RefExpr::LogicOp {
            op: *op,
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::ArithOp {
            op,
            left,
            right,
            span,
        } => RefExpr::ArithOp {
            op: *op,
            left: Box::new(normalize_pred(left, param_name)),
            right: Box::new(normalize_pred(right, param_name)),
            span: *span,
        },
        RefExpr::Not { inner, span } => RefExpr::Not {
            inner: Box::new(normalize_pred(inner, param_name)),
            span: *span,
        },
        RefExpr::Grouped { inner, span } => RefExpr::Grouped {
            inner: Box::new(normalize_pred(inner, param_name)),
            span: *span,
        },
        // Field access: recurse into object, keep field unchanged.
        RefExpr::FieldAccess {
            object,
            field,
            span,
        } => RefExpr::FieldAccess {
            object: Box::new(normalize_pred(object, param_name)),
            field: field.clone(),
            span: *span,
        },
        // StringOp: recurse into receiver (which may reference the param as `self`).
        RefExpr::StringOp {
            op,
            receiver,
            literal,
            span,
        } => RefExpr::StringOp {
            op: *op,
            receiver: Box::new(normalize_pred(receiver, param_name)),
            literal: literal.clone(),
            span: *span,
        },
        // RegexMatch: same shape — recurse into receiver, pattern is a compile-time const.
        RefExpr::RegexMatch {
            receiver,
            pattern,
            span,
        } => RefExpr::RegexMatch {
            receiver: Box::new(normalize_pred(receiver, param_name)),
            pattern: pattern.clone(),
            span: *span,
        },
        // Literals and Len don't contain the param name.
        other => other.clone(),
    }
}

/// If `ty` names a type alias that itself has a refinement, return that
/// refinement (so that `fn f(x: PositiveInt)` is equivalent to
/// `fn f(x: Int where self > 0)` for call-site checking).
fn resolve_type_alias_pred(
    ty: &TypeExpr,
    type_refs: &HashMap<String, Option<RefExpr>>,
) -> Option<RefExpr> {
    if let TypeExpr::Base { name, .. } = ty {
        return type_refs.get(name).and_then(|v| v.clone());
    }
    None
}

// ── If-condition narrowing ────────────────────────────────────────────────────

/// Inject narrowing hypotheses derived from an if-condition into `var_refs`.
///
/// Handles simple integer comparisons (`x op n`, `n op x`) and `&&`
/// conjunctions.  Everything else is silently ignored — conservative and
/// always sound.  The caller is responsible for working on a *clone* of
/// `var_refs` so that the narrowing does not escape the if-branch.
fn inject_if_hypothesis(cond: &Expr, var_refs: &mut HashMap<String, Option<RefExpr>>) {
    let Expr::Binary {
        op, left, right, ..
    } = cond
    else {
        return;
    };
    if let Some(cmp) = binary_op_to_cmp(*op) {
        // Recognise `x op n` and `n op x` (integer literal only).
        let (var_name, cmp_op, int_val) =
            if let (Expr::Ident(name, _), Expr::Literal(Literal::Integer(n), _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), cmp, *n)
            } else if let (Expr::Literal(Literal::Integer(n), _), Expr::Ident(name, _)) =
                (left.as_ref(), right.as_ref())
            {
                (name.clone(), cmp.flip(), *n)
            } else {
                return;
            };

        let s = dummy_span();
        let ref_expr = RefExpr::Compare {
            op: cmp_op,
            left: Box::new(RefExpr::Ident {
                name: "self".to_string(),
                span: s,
            }),
            right: Box::new(RefExpr::Integer {
                value: int_val,
                span: s,
            }),
            span: s,
        };
        // Conjoin with any existing hypothesis for this variable.
        let new_hyp = match var_refs.get(&var_name).and_then(|v| v.clone()) {
            Some(existing) => RefExpr::LogicOp {
                op: LogicOp::And,
                left: Box::new(existing),
                right: Box::new(ref_expr),
                span: s,
            },
            None => ref_expr,
        };
        var_refs.insert(var_name, Some(new_hyp));
    } else if *op == BinaryOp::And {
        // Recurse into both arms of a `&&` conjunction.
        inject_if_hypothesis(left, var_refs);
        inject_if_hypothesis(right, var_refs);
    }
}

/// Canonical string form of an `Expr` subtree, aligned with
/// [`crate::mvl::checker::solver::atom_norm`]'s canonical form so that the
/// hypothesis inserted here under a canonical key is picked up by the
/// atom normalizer's bridge step.
///
/// Kept private and intentionally minimal — the atom normalizer owns the
/// authoritative canonicalization; this mirror is invoked only for the
/// subtrees an `enrich_var_refs` call actually visits (MethodCall / FnCall).
fn canon_call_key(e: &Expr) -> String {
    match e {
        Expr::Ident(n, _) => n.clone(),
        Expr::Literal(l, _) => match l {
            Literal::Integer(n) => n.to_string(),
            Literal::Float(f) => format!("f{f}"),
            Literal::Str(s) => format!("\"{s}\""),
            Literal::Char(c) => format!("'{c}'"),
            Literal::Bool(b) => b.to_string(),
            Literal::Unit => "()".to_string(),
        },
        Expr::FieldAccess { expr, field, .. } => {
            format!("{}.{field}", canon_call_key(expr))
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let args_s = args
                .iter()
                .map(canon_call_key)
                .collect::<Vec<_>>()
                .join(",");
            format!("{}.{method}({args_s})", canon_call_key(receiver))
        }
        Expr::FnCall { name, args, .. } => {
            let args_s = args
                .iter()
                .map(canon_call_key)
                .collect::<Vec<_>>()
                .join(",");
            format!("{name}({args_s})")
        }
        other => format!("#{other:?}"),
    }
}

/// Walk `expr` and inject a `MethodCall` / `FnCall` postcondition hypothesis
/// into `out` for every callee whose `fn_ensures` entry is present (#1805).
///
/// The canonical key form matches `canon_expr` in `solver::atom_norm`, so the
/// atom normalizer's `rewrite_var_refs` step bridges the entry onto the
/// synthesized atom name.
fn collect_call_hypotheses(
    expr: &Expr,
    fn_ensures: &HashMap<String, Vec<RefExpr>>,
    out: &mut HashMap<String, Option<RefExpr>>,
) {
    match expr {
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if let Some(hyps) = fn_ensures.get(method) {
                let key = canon_call_key(expr);
                if let Some(conjunction) = conjoin(hyps) {
                    out.insert(key, Some(conjunction));
                }
            }
            collect_call_hypotheses(receiver, fn_ensures, out);
            for a in args {
                collect_call_hypotheses(a, fn_ensures, out);
            }
        }
        Expr::FnCall { name, args, .. } => {
            if let Some(hyps) = fn_ensures.get(name) {
                let key = canon_call_key(expr);
                if let Some(conjunction) = conjoin(hyps) {
                    out.insert(key, Some(conjunction));
                }
            }
            for a in args {
                collect_call_hypotheses(a, fn_ensures, out);
            }
        }
        Expr::Unary { expr: inner, .. } => collect_call_hypotheses(inner, fn_ensures, out),
        Expr::Binary { left, right, .. } => {
            collect_call_hypotheses(left, fn_ensures, out);
            collect_call_hypotheses(right, fn_ensures, out);
        }
        Expr::FieldAccess { expr: inner, .. } => collect_call_hypotheses(inner, fn_ensures, out),
        _ => {}
    }
}

/// Fold a list of `RefExpr` hypotheses into a single `And`-conjunction.
/// Returns `None` for an empty input.
fn conjoin(hyps: &[RefExpr]) -> Option<RefExpr> {
    let mut it = hyps.iter().cloned();
    let first = it.next()?;
    Some(it.fold(first, |acc, r| RefExpr::LogicOp {
        op: LogicOp::And,
        left: Box::new(acc),
        right: Box::new(r),
        span: dummy_span(),
    }))
}

// ── AST walker (Visit-trait based) ────────────────────────────────────────────

/// Per-function refinement analyzer.  Walks the AST and reports refinement
/// violations into a shared error vector while updating `RefinementCounts`.
///
/// Replaces four previously hand-rolled walkers (`analyze_block`,
/// `analyze_stmt`, `analyze_expr`, `analyze_match_arms`) so that new AST
/// variants force a deliberate include/exclude decision at the visitor level
/// rather than a silent skip (see [`crate::mvl::parser::visit`]).
struct RefinementAnalyzer<'a, 'ast> {
    var_refs: HashMap<String, Option<RefExpr>>,
    fn_params: &'a HashMap<String, Vec<(String, Option<RefExpr>)>>,
    fn_ensures: &'a HashMap<String, Vec<RefExpr>>,
    type_refs: &'a HashMap<String, Option<RefExpr>>,
    fn_decls: &'a HashMap<String, FnDecl>,
    errors: &'a mut Vec<CheckError>,
    counts: &'a mut RefinementCounts,
    _marker: PhantomData<&'ast ()>,
}

impl<'a, 'ast> RefinementAnalyzer<'a, 'ast> {
    fn new(
        var_refs: HashMap<String, Option<RefExpr>>,
        fn_params: &'a HashMap<String, Vec<(String, Option<RefExpr>)>>,
        fn_ensures: &'a HashMap<String, Vec<RefExpr>>,
        type_refs: &'a HashMap<String, Option<RefExpr>>,
        fn_decls: &'a HashMap<String, FnDecl>,
        errors: &'a mut Vec<CheckError>,
        counts: &'a mut RefinementCounts,
    ) -> Self {
        Self {
            var_refs,
            fn_params,
            fn_ensures,
            type_refs,
            fn_decls,
            errors,
            counts,
            _marker: PhantomData,
        }
    }

    /// Enrich `var_refs` with hypotheses derived from method / function
    /// postconditions of any `MethodCall` or `FnCall` subtree in `arg`
    /// (#1805).  For each such subtree, look up the callee's ensures in
    /// `fn_ensures`; if present, insert them under the canonical string form
    /// of the subtree (as produced by `atom_norm::canon_expr`) so that the
    /// atom normalizer can bridge the hypothesis onto the synthesized atom
    /// name.
    ///
    /// Returns a fresh `HashMap` — the analyzer's own `var_refs` is not
    /// mutated (each call site sees its own enrichment).
    #[allow(dead_code)]
    fn enrich_var_refs(&self, arg: &Expr) -> HashMap<String, Option<RefExpr>> {
        let mut out = self.var_refs.clone();
        collect_call_hypotheses(arg, self.fn_ensures, &mut out);
        out
    }

    /// Like [`enrich_var_refs`] but for a slice of arguments (call sites
    /// pass all arguments in one shot).
    fn enrich_var_refs_from_args(&self, args: &[Expr]) -> HashMap<String, Option<RefExpr>> {
        let mut out = self.var_refs.clone();
        for a in args {
            collect_call_hypotheses(a, self.fn_ensures, &mut out);
        }
        out
    }

    /// Swap in a narrowed `var_refs` for the duration of `f`, then restore.
    /// Used for `if` then-branch and lambda body, where the inner scope sees
    /// additional hypotheses but those narrowings must not leak back out.
    fn with_narrowed<F: FnOnce(&mut Self)>(
        &mut self,
        narrowed: HashMap<String, Option<RefExpr>>,
        f: F,
    ) {
        let saved = std::mem::replace(&mut self.var_refs, narrowed);
        f(self);
        self.var_refs = saved;
    }

    /// Walk match arms, injecting per-arm hypotheses.
    ///
    /// Shared by `Stmt::Match` and `Expr::Match` — the loop body is identical
    /// in both cases.
    fn analyze_match_arms(&mut self, scrutinee: &'ast Expr, arms: &'ast [MatchArm]) {
        self.visit_expr(scrutinee);
        let scrutinee_name = ident_name_from_expr(scrutinee);
        let mut prior_int_lits: Vec<i64> = Vec::new();
        let mut prior_float_lits: Vec<f64> = Vec::new();
        for arm in arms {
            let mut arm_refs = self.var_refs.clone();
            inject_arm_hypotheses(
                scrutinee_name,
                &arm.pattern,
                arm.guard.as_ref(),
                &prior_int_lits,
                &prior_float_lits,
                &mut arm_refs,
            );
            match &arm.pattern {
                Pattern::Literal(Literal::Integer(n), _) => prior_int_lits.push(*n),
                Pattern::Literal(Literal::Float(f), _) if !f.is_nan() => prior_float_lits.push(*f),
                _ => {}
            }
            self.with_narrowed(arm_refs, |a| match &arm.body {
                MatchBody::Expr(e) => a.visit_expr(e),
                MatchBody::Block(b) => a.visit_block(b),
            });
        }
    }
}

impl<'a, 'ast> Visit<'ast> for RefinementAnalyzer<'a, 'ast> {
    fn visit_stmt(&mut self, s: &'ast Stmt) {
        match s {
            Stmt::Let {
                pattern,
                ty,
                init,
                span,
                ..
            } => {
                self.visit_expr(init);
                // Record refinement for the new variable, from its declared type or alias.
                let mut pred = extract_type_refinement(ty)
                    .or_else(|| resolve_type_alias_pred(ty, self.type_refs));
                // If no explicit refinement, try to constant-fold the initialiser.
                // When successful, inject a `self == folded_value` hypothesis so
                // that the refinement solver can prove predicates on the bound
                // name statically.
                if pred.is_none() {
                    if let Expr::FnCall { name, args, .. } = init {
                        if let Some(fd) = self.fn_decls.get(name) {
                            if let Some(cv) = const_eval::try_fold_call(fd, args, self.fn_decls) {
                                pred = lit_eq_pred(&cv);
                            }
                        }
                    }
                }
                // Check that the initialiser satisfies the declared type's
                // refinement predicate.
                if let Some(ref p) = pred {
                    let enriched = self.enrich_var_refs(init);
                    let outcome = check_arg_against_pred_counted(
                        init,
                        p,
                        &enriched,
                        self.fn_decls,
                        self.counts,
                    );
                    match outcome {
                        RefResult::Proven | RefResult::ProvenBv => self.counts.proven += 1,
                        RefResult::RuntimeCheck | RefResult::RuntimeCheckWithWitness { .. } => {
                            self.counts.runtime_checked += 1
                        }
                        RefResult::Failed { counterexample } => {
                            self.counts.failed += 1;
                            self.errors.push(CheckError::RefinementViolated {
                                pred: format!(
                                    "let binding initialiser violates refinement `{}`",
                                    display_pred(p)
                                ),
                                span: *span,
                                counterexample,
                            });
                        }
                    }
                }
                if let Pattern::Ident(name, _) = pattern {
                    self.var_refs.insert(name.clone(), pred);
                }
            }
            Stmt::Assign { target, value, .. } => {
                self.visit_expr(value);
                // Reassignment invalidates any refinement the variable carried
                // from its binding.  Field assignments don't affect the
                // variable's top-level refinement.
                if let LValue::Ident(name, _) = target {
                    self.var_refs.insert(name.clone(), None);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond);
                // Narrow the then-branch: clone var_refs and inject the
                // condition as an integer hypothesis.  Narrowings do not
                // propagate out of the branch — the original var_refs is left
                // unchanged.
                let mut then_refs = self.var_refs.clone();
                inject_if_hypothesis(cond, &mut then_refs);
                self.with_narrowed(then_refs, |a| a.visit_block(then));
                if let Some(eb) = else_ {
                    match eb {
                        ElseBranch::Block(b) => self.visit_block(b),
                        ElseBranch::If(s) => self.visit_stmt(s),
                    }
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.analyze_match_arms(scrutinee, arms);
            }
            // While / For / Return / Stmt::Expr — no scoped narrowing; the
            // default walker recurses with the current var_refs.  Note: loop
            // invariants and `decreases` clauses are intentionally not walked
            // here (the prover handles them separately).
            Stmt::While { cond, body, .. } => {
                self.visit_expr(cond);
                self.visit_block(body);
            }
            Stmt::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.visit_block(body);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        match e {
            Expr::FnCall {
                name, args, span, ..
            } => {
                // Check each argument against the callee's parameter refinements.
                if let Some(param_refs) = self.fn_params.get(name) {
                    let enriched = self.enrich_var_refs_from_args(args);
                    check_call_site(
                        name,
                        args,
                        *span,
                        param_refs,
                        &enriched,
                        self.fn_decls,
                        self.errors,
                        self.counts,
                    );
                }
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
                span,
            } => {
                // Check args against the method's parameter refinements (same
                // as FnCall).  Methods are registered under their bare name in
                // fn_params.  The first parameter of an extension/impl method
                // is the implicit `self` receiver, which is NOT included in
                // `args` — skip it when present.
                if let Some(param_refs) = self.fn_params.get(method) {
                    let arg_params = if param_refs.first().is_some_and(|(name, _)| name == "self") {
                        &param_refs[1..]
                    } else {
                        param_refs.as_slice()
                    };
                    let enriched = self.enrich_var_refs_from_args(args);
                    check_call_site(
                        method,
                        args,
                        *span,
                        arg_params,
                        &enriched,
                        self.fn_decls,
                        self.errors,
                        self.counts,
                    );
                }
                self.visit_expr(receiver);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.visit_expr(cond);
                let mut then_refs = self.var_refs.clone();
                inject_if_hypothesis(cond, &mut then_refs);
                self.with_narrowed(then_refs, |a| a.visit_block(then));
                if let Some(e) = else_ {
                    self.visit_expr(e);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.analyze_match_arms(scrutinee, arms);
            }
            Expr::Lambda { params, body, .. } => {
                // Lambda params may have refinements; normalise them (param
                // name → "self") before inserting so that preds_equivalent
                // works correctly.
                let mut child_refs = self.var_refs.clone();
                for p in params {
                    let pred = p.refinement.as_ref().map(|r| normalize_pred(r, &p.name));
                    child_refs.insert(p.name.clone(), pred);
                }
                self.with_narrowed(child_refs, |a| a.visit_expr(body));
            }
            // Block, structural recursion (Binary, Unary, Field/As/Borrow/
            // Relabel, Propagate, Consume, Construct, List, Set, Map, Spawn,
            // Select), and leaves (Literal, Ident, Quantifier) — the default
            // walker recurses under the current var_refs.
            _ => walk_expr(self, e),
        }
    }
}

// ── Call-site checker ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn check_call_site(
    fn_name: &str,
    args: &[Expr],
    call_span: Span,
    param_refs: &[(String, Option<RefExpr>)],
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    errors: &mut Vec<CheckError>,
    counts: &mut RefinementCounts,
) {
    for (arg, (param_name, param_pred)) in args.iter().zip(param_refs.iter()) {
        let Some(pred) = param_pred else { continue };
        let layer_before: [usize; 6] = counts.by_layer;
        let outcome = check_arg_against_pred_counted(arg, pred, var_refs, fn_decls, counts);
        // Determine layer (only meaningful for Proven outcomes).
        let layer = (1..6)
            .find(|&i| counts.by_layer[i] > layer_before[i])
            .unwrap_or(0);
        // #1863 (part 2): counters are updated inside
        // `check_arg_against_pred_counted` via `record()` — do not
        // increment here or they will double-count.
        let proof_outcome = match &outcome {
            RefResult::Proven | RefResult::ProvenBv => {
                counts.proof_log.push(ProofEntry {
                    file: String::new(), // filled by assurance aggregator
                    line: call_span.line,
                    caller: String::new(), // filled by assurance aggregator
                    callee: fn_name.to_string(),
                    predicate: format!("{}: {}", param_name, display_pred(pred)),
                    layer,
                });
                ProofOutcome::Proven {
                    layer,
                    is_bv: matches!(outcome, RefResult::ProvenBv),
                }
            }
            RefResult::RuntimeCheck => ProofOutcome::RuntimeCheck,
            RefResult::RuntimeCheckWithWitness { counterexample } => {
                ProofOutcome::RuntimeCheckWithWitness {
                    counterexample: counterexample.clone(),
                }
            }
            RefResult::Failed { counterexample } => {
                errors.push(CheckError::RefinementViolated {
                    pred: format!(
                        "argument to `{fn_name}` violates refinement `{}`",
                        display_pred(pred)
                    ),
                    span: call_span,
                    counterexample: counterexample.clone(),
                });
                ProofOutcome::Failed
            }
        };
        counts.sites.push(ProofSite {
            caller_fn: counts.current_fn.clone(),
            fn_name: fn_name.to_string(),
            param_name: param_name.clone(),
            predicate: display_pred(pred),
            span: call_span,
            outcome: proof_outcome,
        });
    }
}

// ── Argument checking ─────────────────────────────────────────────────────────

fn record(n: usize, r: RefResult, counts: &mut RefinementCounts) -> RefResult {
    // #1863 (part 2): single source of truth for outcome counting.
    // Every path through `check_arg_against_pred_counted` returns via `record`
    // (Some(r) branches) or the trailing `RuntimeCheck` fallthrough (which
    // this function's caller must also count — see the tail of
    // `check_arg_against_pred_counted`). Prior to this fix, only `by_layer`
    // was updated here and each of the ~11 higher-level call sites (call-site
    // checks, requires, ensures, invariants, spawn/construct field checks)
    // had to manually bump `proven` / `runtime_checked` / `failed` — nine of
    // them didn't, so top-level Req 10 counters read 0 while `by_layer`
    // summed to the real proof total. Centralising the update here makes it
    // impossible to add a new caller that silently drops counts.
    match &r {
        RefResult::Proven | RefResult::ProvenBv => {
            counts.by_layer[n] += 1;
            counts.proven += 1;
        }
        RefResult::RuntimeCheck | RefResult::RuntimeCheckWithWitness { .. } => {
            counts.runtime_checked += 1;
        }
        RefResult::Failed { .. } => {
            counts.failed += 1;
        }
    }
    r
}

/// Mode-aware dispatch used by `check_call_site` and `check_arg_against_pred`.
///
/// Records which layer resolved the check in `counts.by_layer[n]`.
pub(crate) fn check_arg_against_pred_counted(
    arg: &Expr,
    pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    counts: &mut RefinementCounts,
) -> RefResult {
    // L3 bounded-quantifier expansion (#1915): before the layered cascade,
    // unroll `forall i in [lo..hi]. p(i)` into a conjunction of substituted
    // instances (and existentials into a disjunction). Each instance is then
    // dispatched through the full L1..L5 cascade. See ADR-0057.
    if matches!(
        pred,
        RefExpr::BoundedForall { .. } | RefExpr::BoundedExists { .. }
    ) {
        return expand_bounded_quantifier(arg, pred, var_refs, fn_decls, counts);
    }

    // Atom normalization (#1805): L2/L4/L5 gate on `Expr::Ident`, which excludes
    // real-world compound atoms like `field.height` or `xs.len()`.  We rewrite
    // those non-arithmetic subtrees to fresh `Ident("__atom_N")` in a single
    // pass, then hand the normalized triple to the arithmetic layers.  Layers 1
    // and 3 receive the *original* inputs — L1 relies on structural shape
    // equality, and L3 dispatches on `Expr::FnCall` which normalization does
    // not rewrite anyway (but keeping originals avoids any accidental drift).
    //
    // The normalizer is preserved (not wrapped in a closure) so that try_z3
    // can use it to project __atom_N names back to source-level names in
    // counter-example witnesses (#1896).
    let mut norm = AtomNormalizer::new();
    let make_normalized = |norm: &mut AtomNormalizer| {
        let n_arg = norm.rewrite_expr(arg);
        let n_pred = norm.rewrite_refexpr(pred);
        let n_var_refs = norm.rewrite_var_refs(var_refs);
        (n_arg, n_pred, n_var_refs)
    };

    match counts.mode {
        SolverMode::Z3Only => {
            let (n_arg, n_pred, n_var_refs) = make_normalized(&mut norm);
            if let Some(r) = try_z3(&n_pred, &n_arg, &n_var_refs, Some(&norm)) {
                return record(5, r, counts);
            }
        }
        SolverMode::FastOnly => {
            if let Some(r) = try_trivial(pred, arg, var_refs, fn_decls) {
                return record(1, r, counts);
            }
            let (n_arg, n_pred, n_var_refs) = make_normalized(&mut norm);
            if let Some(r) = try_interval(&n_pred, &n_arg, &n_var_refs) {
                return record(2, r, counts);
            }
        }
        SolverMode::Layered => {
            if let Some(r) = try_trivial(pred, arg, var_refs, fn_decls) {
                return record(1, r, counts);
            }
            let (n_arg, n_pred, n_var_refs) = make_normalized(&mut norm);
            if let Some(r) = try_interval(&n_pred, &n_arg, &n_var_refs) {
                return record(2, r, counts);
            }
            // L3 needs the original argument (dispatch on `Expr::FnCall` /
            // `Expr::If` / `Expr::Block`).  Normalization would not rewrite
            // these but we pass the originals explicitly to make the intent
            // obvious.
            if let Some(r) = try_symbolic(pred, arg, var_refs, fn_decls) {
                return record(3, r, counts);
            }
            if let Some(r) = try_cooper(&n_pred, &n_arg, &n_var_refs) {
                return record(4, r, counts);
            }
            if let Some(r) = try_z3(&n_pred, &n_arg, &n_var_refs, Some(&norm)) {
                return record(5, r, counts);
            }
        }
    }
    // No solver returned a verdict — the site is deferred to runtime.
    // Count it before returning so callers see it in `runtime_checked`.
    counts.runtime_checked += 1;
    RefResult::RuntimeCheck
}

// ── L3 bounded-quantifier expansion (#1915) ─────────────────────────────────

/// Maximum number of expanded obligations produced by a single bounded
/// quantifier before the checker falls back to `RuntimeCheck`. Prevents
/// pathological blow-up on wide ranges. Same pattern as `MAX_PATHS` in L3.
const MAX_BOUNDED_EXPANSION: usize = 1000;

/// Expand a bounded quantifier by substituting the bound variable with each
/// integer in `[lo..hi]` and dispatching each instance through the layered
/// solver. Aggregates results:
/// - `forall`: any failure ⇒ failure; all proven ⇒ proven; otherwise runtime.
/// - `exists`: any proven ⇒ proven; all failed ⇒ failure; otherwise runtime.
///
/// Each expanded instance is credited to `by_layer[3]` regardless of which
/// inner layer discharges it (per #1915 AC): the expansion is the L3 activity.
fn expand_bounded_quantifier(
    arg: &Expr,
    pred: &RefExpr,
    var_refs: &HashMap<String, Option<RefExpr>>,
    fn_decls: &HashMap<String, FnDecl>,
    counts: &mut RefinementCounts,
) -> RefResult {
    let (var, lo, hi, body, is_forall) = match pred {
        RefExpr::BoundedForall {
            var, lo, hi, body, ..
        } => (var.clone(), *lo, *hi, body.as_ref(), true),
        RefExpr::BoundedExists {
            var, lo, hi, body, ..
        } => (var.clone(), *lo, *hi, body.as_ref(), false),
        _ => unreachable!("expand_bounded_quantifier requires a bounded quantifier"),
    };

    // Range width. Parser has already rejected `lo > hi`; guard defensively.
    if hi < lo {
        counts.runtime_checked += 1;
        return RefResult::RuntimeCheck;
    }
    let width = (hi - lo + 1) as usize;
    if width > MAX_BOUNDED_EXPANSION {
        // Cap exceeded — record one runtime-check obligation, do not expand.
        // A follow-up ticket can add an explicit diagnostic here.
        counts.runtime_checked += 1;
        return RefResult::RuntimeCheck;
    }

    let mut all_proven = true;
    let mut all_failed = true;
    let mut first_failure: Option<String> = None;

    for k in lo..=hi {
        let instance_span = dummy_span();
        let value = RefExpr::Integer {
            value: k,
            span: instance_span,
        };
        let instance = crate::mvl::checker::contracts::subst_pred_ident(body, &var, &value);

        // Dispatch this instance through the full cascade, but suppress the
        // inner counting — we credit each instance to L3 below regardless of
        // which inner layer discharged it (AC #5).
        let mut inner_counts = RefinementCounts {
            mode: counts.mode,
            ..Default::default()
        };
        let r =
            check_arg_against_pred_counted(arg, &instance, var_refs, fn_decls, &mut inner_counts);
        counts.by_layer[3] += 1;
        match r {
            RefResult::Proven | RefResult::ProvenBv => {
                counts.proven += 1;
                all_failed = false;
                // `exists` short-circuits on first proof.
                if !is_forall {
                    return RefResult::Proven;
                }
            }
            RefResult::Failed { counterexample } => {
                counts.failed += 1;
                all_proven = false;
                if is_forall {
                    // `forall` short-circuits on first refutation.
                    return RefResult::Failed {
                        counterexample: counterexample.or_else(|| Some(format!("{var} = {k}"))),
                    };
                }
                if first_failure.is_none() {
                    first_failure = counterexample;
                }
            }
            RefResult::RuntimeCheck | RefResult::RuntimeCheckWithWitness { .. } => {
                counts.runtime_checked += 1;
                all_proven = false;
                all_failed = false;
            }
        }
    }

    if is_forall {
        if all_proven {
            RefResult::Proven
        } else {
            RefResult::RuntimeCheck
        }
    } else if all_failed {
        RefResult::Failed {
            counterexample: first_failure
                .or_else(|| Some(format!("no witness found in [{lo}..{hi}]"))),
        }
    } else {
        RefResult::RuntimeCheck
    }
}

// ── Predicate display ─────────────────────────────────────────────────────────

fn display_pred(pred: &RefExpr) -> String {
    match pred {
        RefExpr::Ident { name, .. } => name.clone(),
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => value.to_string(),
        RefExpr::Bool { value, .. } => value.to_string(),
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let op_str = match op {
                CmpOp::Eq => "==",
                CmpOp::Ne => "!=",
                CmpOp::Lt => "<",
                CmpOp::Gt => ">",
                CmpOp::Le => "<=",
                CmpOp::Ge => ">=",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                LogicOp::And => "&&",
                LogicOp::Or => "||",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                ArithOp::Add => "+",
                ArithOp::Sub => "-",
                ArithOp::Mul => "*",
                ArithOp::Div => "/",
                ArithOp::Rem => "%",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::Not { inner, .. } => format!("!{}", display_pred(inner)),
        RefExpr::Grouped { inner, .. } => format!("({})", display_pred(inner)),
        RefExpr::Old { inner, .. } => format!("old({})", display_pred(inner)),
        RefExpr::Len { ident, .. } => format!("len({ident})"),
        RefExpr::Forall { var, body, .. } => format!("forall {var}, {}", display_pred(body)),
        RefExpr::Exists { var, body, .. } => format!("exists {var}, {}", display_pred(body)),
        RefExpr::BoundedForall {
            var, lo, hi, body, ..
        } => format!("forall {var} in [{lo}..{hi}]. {}", display_pred(body)),
        RefExpr::BoundedExists {
            var, lo, hi, body, ..
        } => format!("exists {var} in [{lo}..{hi}]. {}", display_pred(body)),
        RefExpr::FieldAccess { object, field, .. } => {
            format!("{}.{}", display_pred(object), field)
        }
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => {
            use crate::mvl::parser::ast::BitwiseOp;
            let op_str = match op {
                BitwiseOp::And => "&",
                BitwiseOp::Or => "|",
                BitwiseOp::Xor => "^",
                BitwiseOp::Shl => "<<",
                BitwiseOp::Shr => ">>",
            };
            format!("{} {op_str} {}", display_pred(left), display_pred(right))
        }
        RefExpr::BitwiseNot { inner, .. } => format!("~{}", display_pred(inner)),
        RefExpr::StringOp {
            op,
            receiver,
            literal,
            ..
        } => {
            let method = match op {
                StringOp::Contains => "contains",
                StringOp::StartsWith => "starts_with",
                StringOp::EndsWith => "ends_with",
            };
            format!("{}.{}({:?})", display_pred(receiver), method, literal)
        }
        RefExpr::ArrayGet { list, index, .. } => {
            format!("{}.get({})", display_pred(list), display_pred(index))
        }
        RefExpr::RegexMatch {
            receiver, pattern, ..
        } => {
            format!("{}.matches({:?})", display_pred(receiver), pattern)
        }
        RefExpr::Abs { inner, .. } => format!("abs({})", display_pred(inner)),
        RefExpr::Min { left, right, .. } => {
            format!("min({}, {})", display_pred(left), display_pred(right))
        }
        RefExpr::Max { left, right, .. } => {
            format!("max({}, {})", display_pred(left), display_pred(right))
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::solver::{dummy_span, RefResult};
    use crate::mvl::parser::ast::{CmpOp, Expr, Literal, RefExpr};

    fn self_gt(n: i64) -> RefExpr {
        RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: n,
                span: dummy_span(),
            }),
            span: dummy_span(),
        }
    }

    fn int_lit(v: i64) -> Expr {
        Expr::Literal(Literal::Integer(v), dummy_span())
    }

    fn make_counts(mode: SolverMode) -> RefinementCounts {
        RefinementCounts {
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn layered_mode_records_layer1_for_literal() {
        // pred: self > 0, arg: 5 — Layer 1 (trivial) proves this.
        let pred = self_gt(0);
        let arg = int_lit(5);
        let var_refs = HashMap::new();
        let fn_decls = HashMap::new();
        let mut counts = make_counts(SolverMode::Layered);
        let result = check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
        assert_eq!(result, RefResult::Proven);
        assert_eq!(counts.by_layer[1], 1, "Layer 1 should record the proof");
        assert_eq!(counts.by_layer[2..].iter().sum::<usize>(), 0);
    }

    #[test]
    fn fast_only_mode_skips_layers_3_to_5() {
        // pred: self > 0, arg: 5 — Layer 1 still proves it in FastOnly mode.
        let pred = self_gt(0);
        let arg = int_lit(5);
        let var_refs = HashMap::new();
        let fn_decls = HashMap::new();
        let mut counts = make_counts(SolverMode::FastOnly);
        let result = check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
        assert_eq!(result, RefResult::Proven);
        assert_eq!(counts.by_layer[1], 1);
    }

    #[test]
    fn fast_only_mode_falls_to_runtime_when_layers_12_cannot_decide() {
        // A variable with no hypothesis — no layer can prove self > 0.
        let pred = self_gt(0);
        let arg = Expr::Ident("x".into(), dummy_span());
        let mut var_refs = HashMap::new();
        var_refs.insert("x".into(), None::<RefExpr>);
        let fn_decls = HashMap::new();
        let mut counts = make_counts(SolverMode::FastOnly);
        let result = check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
        // FastOnly skips Layer 3+ — must fall through to RuntimeCheck.
        assert_eq!(result, RefResult::RuntimeCheck);
        assert_eq!(counts.by_layer.iter().sum::<usize>(), 0);
    }

    #[test]
    fn z3_only_mode_bypasses_layers_1_to_4() {
        // pred: self > 0, arg: 5 — Z3 should prove it directly.
        // (If Z3 feature disabled, returns RuntimeCheck — both acceptable.)
        let pred = self_gt(0);
        let arg = int_lit(5);
        let var_refs = HashMap::new();
        let fn_decls = HashMap::new();
        let mut counts = make_counts(SolverMode::Z3Only);
        let result = check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
        // With Z3 feature: Proven (by_layer[5] = 1). Without: RuntimeCheck.
        match result {
            RefResult::Proven | RefResult::ProvenBv => assert_eq!(counts.by_layer[5], 1),
            RefResult::RuntimeCheck | RefResult::RuntimeCheckWithWitness { .. } => {} // z3 feature not enabled
            RefResult::Failed { .. } => panic!("unexpected Failed"),
        }
        // Layers 1–4 must NOT have been credited.
        assert_eq!(counts.by_layer[1..5].iter().sum::<usize>(), 0);
    }

    #[test]
    fn failed_outcome_does_not_credit_by_layer() {
        // pred: self > 0, arg: 0 — Layer 1 definitively refutes this.
        // by_layer must stay all-zero: we only count proofs, not failures.
        let pred = self_gt(0);
        let arg = int_lit(0);
        let var_refs = HashMap::new();
        let fn_decls = HashMap::new();
        for mode in [SolverMode::Layered, SolverMode::FastOnly] {
            let mut counts = make_counts(mode);
            let result =
                check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
            assert!(
                matches!(result, RefResult::Failed { .. }),
                "mode {mode:?}: expected Failed for 0 > 0"
            );
            assert_eq!(
                counts.by_layer.iter().sum::<usize>(),
                0,
                "mode {mode:?}: by_layer should be zero for Failed results"
            );
        }
    }

    #[test]
    fn layered_mode_credits_layer2_for_interval_proof() {
        // Variable `x` carries hypothesis `self >= 1`, callee requires `self > 0`.
        // Layer 1 (trivial) cannot prove this from hypothesis alone; Layer 2 (interval) can.
        use crate::mvl::parser::ast::CmpOp;
        let pred = self_gt(0); // requires self > 0
        let arg = Expr::Ident("x".into(), dummy_span());
        // Hypothesis: x >= 1  (i.e. self >= 1)
        let hypothesis = RefExpr::Compare {
            op: CmpOp::Ge,
            left: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: dummy_span(),
            }),
            right: Box::new(RefExpr::Integer {
                value: 1,
                span: dummy_span(),
            }),
            span: dummy_span(),
        };
        let mut var_refs = HashMap::new();
        var_refs.insert("x".into(), Some(hypothesis));
        let fn_decls = HashMap::new();
        let mut counts = make_counts(SolverMode::Layered);
        let result = check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts);
        assert_eq!(
            result,
            RefResult::Proven,
            "Layer 2 should prove x>=1 satisfies self>0"
        );
        assert_eq!(counts.by_layer[2], 1, "Layer 2 should be credited");
        assert_eq!(counts.by_layer[1], 0, "Layer 1 should not have resolved it");
        // FastOnly also includes Layer 2
        let mut counts2 = make_counts(SolverMode::FastOnly);
        var_refs.insert(
            "x".into(),
            Some(RefExpr::Compare {
                op: CmpOp::Ge,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy_span(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 1,
                    span: dummy_span(),
                }),
                span: dummy_span(),
            }),
        );
        let result2 =
            check_arg_against_pred_counted(&arg, &pred, &var_refs, &fn_decls, &mut counts2);
        assert_eq!(
            result2,
            RefResult::Proven,
            "FastOnly Layer 2 should also prove it"
        );
        assert_eq!(counts2.by_layer[2], 1);
    }

    #[test]
    fn check_refinements_returns_counts_with_mode() {
        // Minimal program with one refinement call site.
        let src = "fn pos(n: Int where self > 0) -> Int { n } fn caller() -> Int { pos(1) }";
        let (mut parser, _) = crate::mvl::parser::Parser::new(src);
        let prog = parser.parse_program();
        let mut errors = Vec::new();
        let counts = check_refinements(&[], &[], &prog, &mut errors, SolverMode::FastOnly);
        assert_eq!(counts.mode, SolverMode::FastOnly);
        // pos(1) — literal 1 satisfies self > 0, proven by Layer 1.
        assert_eq!(counts.proven, 1);
        assert_eq!(counts.failed, 0);
        assert_eq!(counts.by_layer[1], 1);
    }
}
