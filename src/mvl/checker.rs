// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL type checker — verifies requirements 1, 3, 4, 5, 6, 10.
//!
//! The checker runs after parsing and before transpilation.  It reports
//! [`CheckError`] values for every violation found; unlike the parser, it
//! does not short-circuit on the first error.
//!
//! # Architecture
//!
//! ```text
//! Program
//!   └─ pass 1: collect_declarations  (populate type/function tables)
//!   └─ pass 2: check_declarations    (verify each decl)
//!              └─ check_fn_decl      (type-check function body)
//!                 └─ check_block / check_stmt / infer_expr
//! ```

pub mod call_graph;
mod calls;
mod capabilities;
pub mod const_eval;
pub mod context;
pub mod contracts;
pub mod data_race;
mod decls;
pub mod effects;
pub mod errors;
pub mod ifc;
pub mod ifc_propagation;
mod infer;
mod method_types;
pub mod passes;
mod patterns;
pub mod refinements;
pub mod session;
pub(crate) mod solver;
mod stmts;
pub mod termination;
pub mod types;
pub(crate) mod walk;

pub use crate::mvl::checker::solver::SolverMode;

use crate::mvl::checker::call_graph::CallGraph;
use crate::mvl::checker::context::{FnInfo, TypeEnv};
use crate::mvl::checker::effects::EffectHierarchy;
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::ifc_propagation::InferredLabels;
use crate::mvl::checker::refinements::RefinementCounts;
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{Decl, Effect, EffectDecl, Program, Totality, UseDecl};
use crate::mvl::parser::lexer::Span;
use std::collections::{HashMap, HashSet};

// ── Public API ───────────────────────────────────────────────────────────────

/// Result of running the type checker over a [`Program`].
#[derive(Debug, Default)]
pub struct CheckResult {
    pub errors: Vec<CheckError>,
    /// Number of `extern` blocks found — each is a trust boundary.
    /// Reported in the assurance summary: "N extern declarations".
    pub extern_count: usize,
    /// Error counts indexed by requirement number (1–11). Index 0 is unused.
    pub req_errors: [usize; 12],
    /// Inferred type for every expression in the program, keyed by span.
    /// Used by the transpiler to emit type-specific Rust code (#554).
    pub expr_types: HashMap<Span, Ty>,
    /// Per-layer refinement proof counts from the most recent `check_refinements` run.
    /// Populated only when checked via `check_with_two_preludes_mode`.
    pub refinement_counts: RefinementCounts,
    /// True when at least one prelude function called from this program carries
    /// IFC-labeled parameters — indicates the security lattice is exercised via
    /// cross-module function calls (e.g. `execute(db, sql: Clean[String])`).
    pub has_prelude_ifc_boundary: bool,
    /// Full type environment produced by the checker — function signatures, type
    /// declarations, and impl registrations.  Exposed for downstream consumers
    /// such as the call graph and interprocedural analysis passes (#829, #825).
    pub type_env: TypeEnv,
    /// Whole-program call graph — function call topology for interprocedural
    /// analysis (IFC #830, refinement contract propagation #830, data race #9).
    /// Built from the AST post type-checking; edges are `FnCall` expressions only
    /// (MethodCall resolution deferred to post-monomorphization pass #838).
    pub call_graph: CallGraph,
    /// Inferred security labels for function return types — built by the
    /// forward label propagation pass (#830/#833).  Supplements TypeEnv's
    /// explicit annotations for unannotated wrapper functions.
    pub inferred_labels: InferredLabels,
}

impl CheckResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Collect all `EffectDecl` nodes from a flat iterator of programs.
fn collect_effect_decls<'a>(programs: impl Iterator<Item = &'a Program>) -> Vec<&'a EffectDecl> {
    programs
        .flat_map(|p| p.declarations.iter())
        .filter_map(|d| {
            if let Decl::EffectDecl(ed) = d {
                Some(ed)
            } else {
                None
            }
        })
        .collect()
}

/// Check a program with two prelude slices chained together.
///
/// Prelude declarations are registered but not individually type-checked.
/// Use this when stdlib files have been parsed and should be visible to the
/// checker (e.g. `use std.io.{...}` imports in corpus / CLI check mode).
/// `prelude_b` holds references so callers can pass flanking slices of an
/// existing vec without cloning individual `Program`s.
pub fn check_with_two_preludes(
    prelude_a: &[Program],
    prelude_b: &[&Program],
    prog: &Program,
) -> CheckResult {
    check_with_two_preludes_mode(prelude_a, prelude_b, prog, SolverMode::Layered)
}

/// Like [`check_with_two_preludes`] but uses the given [`SolverMode`] for refinement checking.
///
/// Populates [`CheckResult::refinement_counts`] with per-layer proof statistics.
/// Used by the CLI `check` command when `--refinement-solver` or `--refinement-stats` is set.
pub fn check_with_two_preludes_mode(
    prelude_a: &[Program],
    prelude_b: &[&Program],
    prog: &Program,
    solver_mode: SolverMode,
) -> CheckResult {
    // Dual-pass: collect all EffectDecl nodes from every parsed program, build
    // the hierarchy (validates parents + detects cycles), then type-check.
    let all_effect_decls = collect_effect_decls(
        prelude_a
            .iter()
            .chain(prelude_b.iter().copied())
            .chain(std::iter::once(prog)),
    );
    let (hierarchy, hierarchy_errors) = EffectHierarchy::from_decls(&all_effect_decls);

    let mut checker = TypeChecker::new_with_hierarchy(hierarchy);
    checker.errors.extend(hierarchy_errors);
    for p in prelude_a.iter().chain(prelude_b.iter().copied()) {
        checker.collect_declarations(&p.declarations);
    }
    checker.check_program(prog);
    // Build the whole-program program slice — used by IFC cross-function analysis,
    // the call graph, and the interprocedural label propagation pass.
    let all_prog_refs: Vec<&Program> = prelude_a
        .iter()
        .chain(prelude_b.iter().copied())
        .chain(std::iter::once(prog))
        .collect();
    // Build call graph early so that both termination and IFC passes can use it.
    let call_graph = call_graph::build(&all_prog_refs, &checker.env);
    termination::check_structural_recursion(prog, &mut checker.errors);
    termination::check_mutual_recursion(prog, &call_graph, &mut checker.errors);
    data_race::check_iso_aliasing(prog, &mut checker.errors);
    data_race::check_ref_escape_to_spawn(prog, &mut checker.errors);

    ifc::check_implicit_flows(
        prog,
        &all_prog_refs,
        Some(&checker.env.fns),
        &mut checker.errors,
    );
    let mut refinement_counts = refinements::check_refinements(
        prelude_a,
        prelude_b,
        prog,
        &mut checker.errors,
        solver_mode,
    );
    let contract_counts = contracts::check_contracts(prog, &mut checker.errors, solver_mode);
    // Merge contract proof-layer counts into the refinement totals.
    // Contract proofs only populate by_layer (proven/runtime_checked are not
    // incremented by the leaf solver); derive proven from the layer sum.
    let contract_proven: usize = contract_counts.by_layer.iter().sum();
    refinement_counts.proven += contract_proven;
    for i in 0..6 {
        refinement_counts.by_layer[i] += contract_counts.by_layer[i];
    }
    refinement_counts
        .proof_log
        .extend(contract_counts.proof_log);
    session::check_session_types(prog, &mut checker.errors);
    contracts::check_actor_field_refinements(prog, &mut checker.errors, solver_mode);
    contracts::check_struct_field_refinements(prog, &mut checker.errors, solver_mode);
    contracts::check_return_refinements(prog, &mut checker.errors, solver_mode);

    // Determine whether any function imported from the prelude and called by
    // `prog` carries IFC-labeled parameters — exercises the security lattice
    // even when `prog` itself defines no labeled functions.
    let has_prelude_ifc_boundary = ifc::prelude_has_ifc_boundary(prelude_a, prelude_b, prog);

    // Interprocedural IFC: forward label propagation (#830/#833) then violation
    // detection (#831).  Runs after the call graph is built so that both the
    // full TypeEnv and the call topology are available.
    let inferred_labels = ifc_propagation::propagate(&[prog], &checker.env);
    let interproc_violations =
        ifc_propagation::detect_violations(prog, &checker.env, &inferred_labels);
    checker.errors.extend(interproc_violations);

    let mut req_errors = [0usize; 12];
    for e in &checker.errors {
        let req = e.requirement_number() as usize;
        debug_assert!(
            (1..=11).contains(&req),
            "requirement_number() returned {req}, must be 1–11"
        );
        req_errors[req] += 1;
    }
    debug_assert_eq!(
        req_errors[1..].iter().sum::<usize>(),
        checker.errors.len(),
        "req_errors sum must equal total error count"
    );
    CheckResult {
        errors: checker.errors,
        extern_count: checker.extern_count,
        req_errors,
        expr_types: checker.expr_types,
        refinement_counts,
        has_prelude_ifc_boundary,
        type_env: checker.env,
        call_graph,
        inferred_labels,
    }
}

pub fn check_with_prelude(prelude: &[Program], prog: &Program) -> CheckResult {
    check_with_two_preludes(prelude, &[], prog)
}

/// Collect inferred expression types for a set of programs without surfacing errors.
///
/// Used by the transpiler to get type information for stdlib prelude programs
/// (e.g. json.mvl, collections.mvl) so method-call sites in those files can
/// emit direct Rust rather than trait-dispatch (#554).
pub fn check(prog: &Program) -> CheckResult {
    check_with_prelude(&[], prog)
}

/// Collect inferred expression types for a set of programs without surfacing errors.
///
/// Used by the transpiler to get type information for stdlib prelude programs
/// (e.g. json.mvl, collections.mvl) so method-call sites in those files can
/// emit direct Rust rather than trait-dispatch (#554).
///
/// All declarations from all programs are registered first so cross-file
/// references resolve correctly, then each declaration is checked for type
/// inference (errors are discarded).
pub fn collect_prelude_expr_types(programs: &[Program]) -> HashMap<Span, Ty> {
    let mut checker = TypeChecker::new();
    for p in programs {
        checker.collect_declarations(&p.declarations);
    }
    for p in programs {
        for decl in &p.declarations {
            checker.check_decl(decl);
        }
    }
    checker.expr_types
}

// ── FnContext ────────────────────────────────────────────────────────────────

/// Per-function context saved/restored on function entry/exit (#1258).
///
/// Replaces the former `current_*` fields on `TypeChecker` with an explicit
/// stack, consistent with how `TypeEnv` manages lexical scopes.
#[derive(Default)]
struct FnContext {
    /// Name of the function currently being checked (for effect error messages).
    fn_name: String,
    /// Return type of the function currently being checked (for `?` and `return`).
    return_ty: Option<Ty>,
    /// Effects declared by the current function (Req 7, 8).
    effects: Vec<Effect>,
    /// Totality of the current function (Req 8); None = implicitly total.
    totality: Option<Totality>,
    /// Type parameter names in scope for the current function.
    type_params: HashSet<String>,
    /// Trait bounds for type params in the current function (from `where` clauses).
    type_constraints: HashMap<String, Vec<String>>,
}

// ── TypeChecker ──────────────────────────────────────────────────────────────

struct TypeChecker {
    errors: Vec<CheckError>,
    env: TypeEnv,
    /// Per-function context stack (#1258). The top entry holds the context
    /// for the function currently being checked; push on fn entry, pop on exit.
    fn_context_stack: Vec<FnContext>,
    /// Count of extern declarations for assurance reporting.
    extern_count: usize,
    /// Stack of scope depths at each lambda entry point.
    ///
    /// Used by capture-immutability checking (ADR-0002): when a variable is looked
    /// up and its scope index is strictly less than the boundary recorded here, it
    /// was captured from an outer scope and must be immutable.
    lambda_scope_starts: Vec<usize>,
    /// Types that implement `Iterator<T>`, mapped to their element type.
    /// Populated by `register_impl` for `impl Iterator<T> for X` declarations.
    iterator_impls: HashMap<String, Ty>,
    /// Method table for type-attached methods (`fn TypeName::method(self, …)`).
    /// Maps receiver type name → method name → signature.
    method_table: HashMap<String, HashMap<String, FnInfo>>,
    /// Names of all declared actor types — used to enforce Spawn/Send effects (#1126).
    actor_type_names: HashSet<String>,
    /// Inferred type for every expression, keyed by span. Populated during
    /// `infer_expr` and surfaced in [`CheckResult::expr_types`] for the
    /// transpiler to use when emitting type-specific Rust code (#554).
    expr_types: HashMap<Span, Ty>,
    /// Resolved effect subsumption hierarchy, built from all parsed EffectDecl nodes (#853).
    effect_hierarchy: EffectHierarchy,
    /// Module-level import aliases: `use std.json` → `"json" → ["std", "json"]`.
    /// Populated from the user program's use declarations in `check_program`.
    /// Enables qualified calls: `json.decode(s)` resolves as a function call (#820).
    module_aliases: HashMap<String, Vec<String>>,
    /// Stack of effect accumulators for nested lambda bodies (#1068 Gap 3).
    /// When non-empty, the innermost entry collects effects used in the current
    /// lambda body so that `Ty::Fn` carries the correct effect list.
    lambda_body_effects: Vec<Vec<Effect>>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker::new_with_hierarchy(EffectHierarchy::default())
    }

    fn new_with_hierarchy(hierarchy: EffectHierarchy) -> Self {
        TypeChecker {
            errors: Vec::new(),
            env: TypeEnv::new(),
            fn_context_stack: vec![FnContext::default()],
            extern_count: 0,
            lambda_scope_starts: Vec::new(),
            iterator_impls: HashMap::new(),
            method_table: HashMap::new(),
            actor_type_names: HashSet::new(),
            expr_types: HashMap::new(),
            effect_hierarchy: hierarchy,
            module_aliases: HashMap::new(),
            lambda_body_effects: Vec::new(),
        }
    }

    // ── FnContext stack management (#1258) ────────────────────────────────

    /// Returns a reference to the current function context (top of stack).
    fn fn_context(&self) -> &FnContext {
        self.fn_context_stack
            .last()
            .expect("fn_context_stack must never be empty")
    }

    /// Pushes a new function context onto the stack.
    fn push_fn_context(&mut self, ctx: FnContext) {
        self.fn_context_stack.push(ctx);
    }

    /// Pops the current function context, restoring the previous one.
    fn pop_fn_context(&mut self) {
        self.fn_context_stack.pop();
        debug_assert!(
            !self.fn_context_stack.is_empty(),
            "fn_context_stack underflow"
        );
    }

    fn emit(&mut self, err: CheckError) {
        self.errors.push(err);
    }

    // ── Effect subsetting (002-effect-system/Req 3, ADR-0035) ────────────

    /// Returns `true` when `declared` covers `required` for effect propagation.
    ///
    /// Uses the hierarchy to check transitive subsumption: `IO > Log > Clock`
    /// means `declared = IO` satisfies `required = Clock`.
    pub(crate) fn effect_satisfies(&self, declared: &Effect, required: &Effect) -> bool {
        self.effect_hierarchy
            .subsumes_transitive(&declared.name, &required.name)
    }

    // ── Program ──────────────────────────────────────────────────────────

    fn check_program(&mut self, prog: &Program) {
        // Collect module-level aliases before checking so qualified calls like
        // `json.decode(s)` (from `use std.json`) can be resolved (#820).
        for decl in &prog.declarations {
            if let Decl::Use(UseDecl {
                module_only: true,
                path,
                ..
            }) = decl
            {
                if let Some(qualifier) = path.last() {
                    self.module_aliases.insert(qualifier.clone(), path.clone());
                }
            }
        }
        self.collect_declarations(&prog.declarations);
        for decl in &prog.declarations {
            self.check_decl(decl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn check_src(src: &str) -> CheckResult {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        check(&prog)
    }

    fn errors_for(src: &str) -> Vec<CheckError> {
        check_src(src).errors
    }

    /// Check `src` with std/effects.mvl loaded so the effect hierarchy is populated.
    fn check_with_effects(src: &str) -> CheckResult {
        let effects_src = include_str!("../../std/effects.mvl");
        let (mut ep, _) = Parser::new(effects_src);
        let effects_prog = ep.parse_program();
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        check_with_prelude(&[effects_prog], &prog)
    }

    // ── Requirement 1 / Scenario: Basic type inference (#11) ─────────────

    #[test]
    fn literal_int_inferred() {
        let result = check_src("fn f() -> Int { 42 }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn literal_bool_inferred() {
        let result = check_src("fn f() -> Bool { true }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn arithmetic_requires_numeric() {
        let errors = errors_for("fn f() -> Int { true + 1 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::NonNumericArithmetic { .. })),
            "expected NonNumericArithmetic, got: {errors:?}"
        );
    }

    #[test]
    fn arithmetic_mixed_types_rejected() {
        let errors = errors_for("fn f() -> Float { 1 + 2.0 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ArithmeticTypeMismatch { .. })),
            "expected ArithmeticTypeMismatch, got: {errors:?}"
        );
    }

    #[test]
    fn logic_requires_bool() {
        let errors = errors_for("fn f() -> Bool { 1 && true }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::LogicTypeMismatch { .. })),
            "expected LogicTypeMismatch, got: {errors:?}"
        );
    }

    // ── Requirement 1 / Scenario: ADT checking (#12) ─────────────────────

    #[test]
    fn struct_construction_valid() {
        let src =
            "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1, y: 2 } }";
        let result = check_src(src);
        // UndefinedFunction for `Point { x: 1, y: 2 }` should not appear;
        // struct construction goes through Construct not FnCall
        let serious: Vec<_> = result
            .errors
            .iter()
            .filter(|e| !matches!(e, CheckError::TypeMismatch { .. }))
            .collect();
        assert!(
            serious.iter().all(|e| !matches!(
                e,
                CheckError::MissingField { .. } | CheckError::UnknownField { .. }
            )),
            "unexpected errors: {serious:?}"
        );
    }

    #[test]
    fn struct_missing_field_rejected() {
        let src = "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1 } }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingField { field, .. } if field == "y")),
            "expected MissingField(y), got: {errors:?}"
        );
    }

    #[test]
    fn field_access_on_enum_rejected() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { c.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::FieldAccessOnEnum { .. })),
            "expected FieldAccessOnEnum, got: {errors:?}"
        );
    }

    // ── Requirement 3 / Scenario: Exhaustive match (#13) ─────────────────

    #[test]
    fn option_match_exhaustive() {
        let src = "fn f(x: Option[Int]) -> Int { match x { Some(v) => v, None => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn option_match_missing_none_rejected() {
        let src = "fn f(x: Option[Int]) -> Int { match x { Some(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"None".to_string())
            )),
            "expected NonExhaustiveMatch(None), got: {errors:?}"
        );
    }

    #[test]
    fn result_match_exhaustive() {
        let src = "fn f(x: Result[Int, String]) -> Int { match x { Ok(v) => v, Err(_) => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn result_match_missing_err_rejected() {
        let src = "fn f(x: Result[Int, String]) -> Int { match x { Ok(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"Err(_)".to_string())
            )),
            "expected NonExhaustiveMatch(Err(_)), got: {errors:?}"
        );
    }

    // ── Requirement 4/5 / Scenario: Option/Result enforcement (#14) ───────

    #[test]
    fn option_direct_access_rejected() {
        let src = "fn f(x: Option[Int]) -> Int { x.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::OptionDirectAccess { .. })),
            "expected OptionDirectAccess, got: {errors:?}"
        );
    }

    #[test]
    fn result_ignored_rejected() {
        let src = "fn produce() -> Result[Int, String] { Ok(1) }\nfn f() -> Unit { produce() }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
            "expected ResultIgnored, got: {errors:?}"
        );
    }

    // ── Requirement 6 / Scenario: Immutability enforcement (#17) ──────────

    #[test]
    fn assign_to_immutable_rejected() {
        let src = "fn f() -> Unit { let x: Int = 1; x = 2; }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::AssignToImmutable { name, .. } if name == "x")),
            "expected AssignToImmutable(x), got: {errors:?}"
        );
    }

    #[test]
    fn assign_to_mutable_allowed() {
        let src = "fn f() -> Unit { let mut x: Int = 1; x = 2; }";
        let errors = errors_for(src);
        let assign_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, CheckError::AssignToImmutable { .. }))
            .collect();
        assert!(
            assign_errors.is_empty(),
            "should allow mut assignment, got: {assign_errors:?}"
        );
    }

    // ── Requirement 2 / Scenario: Ownership / use-after-consume (#15) ────────

    #[test]
    fn use_after_move_rejected() {
        // consume(x) is the MVL syntax for explicit ownership transfer (iso semantics)
        let src = "fn f() -> Int { let x: Int = 1; let y: Int = consume(x); x }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "x")),
            "expected UseAfterMove(x), got: {errors:?}"
        );
    }

    // ── Fix: enum constructors as expressions ─────────────────────────────

    #[test]
    fn some_constructor_no_undefined_function() {
        let src = "fn f(x: Int) -> Option[Int] { Some(x) }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Some")),
            "Some() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn ok_constructor_no_undefined_function() {
        let src = "fn produce() -> Result[Int, String] { Ok(1) }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Ok")),
            "Ok() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn err_constructor_no_undefined_function() {
        let src = "fn f() -> Result[Int, String] { Err(\"oops\") }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { name, .. } if name == "Err")),
            "Err() should not emit UndefinedFunction, got: {errors:?}"
        );
    }

    #[test]
    fn none_ident_no_undefined_variable() {
        let src = "fn f() -> Option[Int] { None }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "None")),
            "None should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn user_enum_unit_variant_no_undefined_variable() {
        let src = "type Dir = enum { North, South }\nfn f() -> Dir { North }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "North")
            ),
            "enum unit variant should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    // ── Fix: assignment type-check ────────────────────────────────────────

    #[test]
    fn assign_type_mismatch_rejected() {
        let src = "fn f() -> Unit { let x: ref Int = 1; x = true; }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch on type-incompatible assignment, got: {errors:?}"
        );
    }

    // ── Fix: lambda return type check ─────────────────────────────────────
    //
    // Lambda parsing is not yet implemented in the parser; the lambda return-type
    // check in infer_expr is exercised via direct AST construction in the
    // integration test suite when lambda parsing lands.  The check itself is
    // verified by ensuring the guard condition compiles and the path exists.

    // ── Effect name validation (002-effect-system Req 2) ──────────────────

    #[test]
    fn invalid_effect_name_rejected() {
        let result = check_with_effects(r#"fn f() -> Unit ! Foo { }"#);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Foo")),
            "expected InvalidEffectName for unknown effect 'Foo', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn valid_effect_names_accepted() {
        let result = check_with_effects(r#"fn f() -> Unit ! Console + Net + DB { }"#);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected no InvalidEffectName for valid effects, got: {:?}",
            result.errors
        );
    }

    // ── Parametrized effects (002-effect-system Req 3 / issue #290) ──────

    #[test]
    fn terminal_effect_name_accepted() {
        let result = check_with_effects(r#"fn f() -> Unit ! Terminal { }"#);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected Terminal to be a valid effect name, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn no_hierarchy_does_not_emit_invalid_effect_name() {
        // check_src uses check() which creates an empty EffectHierarchy.
        // has_any() returns false, so unknown effect names are silently accepted.
        let result = check_src(r#"fn f() -> Unit ! CompletelyMadeUp { }"#);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "empty hierarchy must not emit InvalidEffectName, got: {:?}",
            result.errors
        );
    }

    // ── Lambda capture immutability (ADR-0002) ────────────────────────────
    //
    // Lambda surface syntax is not yet parsed (parser does not handle `|p| expr`).
    // The capture-check logic is exercised via direct AST construction below.

    #[test]
    fn lambda_mutable_capture_rejected_via_ast() {
        use crate::mvl::parser::ast::{
            Block, Decl, Expr, FnDecl, LetKind, Literal, Param, Pattern, Program, Stmt, TypeExpr,
        };
        use crate::mvl::parser::lexer::Span;

        let dummy_span = Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 0,
        };
        let unit_ty = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: dummy_span,
        };
        let int_ty = TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: dummy_span,
        };
        // `ref Int` makes the binding mutable (ADR-0002: lambdas may not capture mutable bindings)
        let ref_int_ty = TypeExpr::Ref {
            mutable: true,
            inner: Box::new(int_ty.clone()),
            span: dummy_span,
        };

        // Build: fn f() -> Unit {
        //    let x: ref Int = 1;   // mutable binding
        //    let g = |y: Int| -> Int { x };  // captures mutable x — should fail
        // }
        let lambda = Expr::Lambda {
            params: vec![Param {
                name: "y".into(),
                ty: int_ty.clone(),
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty.clone())),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let fn_int_int_ty = TypeExpr::Fn {
            params: vec![int_ty.clone()],
            ret: Box::new(int_ty.clone()),
            effects: vec![],
            span: dummy_span,
        };
        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                is_builtin: false,
                totality: None,
                receiver_type: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                requires: vec![],
                ensures: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            kind: LetKind::Regular,
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: ref_int_ty,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            kind: LetKind::Regular,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: fn_int_int_ty,
                            init: lambda,
                            span: dummy_span,
                        },
                    ],
                    span: dummy_span,
                },
                span: dummy_span,
            })],
        };

        let result = check(&prog);
        assert!(
            result.errors.iter().any(|e| matches!(
                e, CheckError::CaptureMutabilityViolation { name, .. } if name == "x"
            )),
            "expected CaptureMutabilityViolation for mutable capture of 'x', got: {:?}",
            result.errors
        );
    }

    #[test]
    fn lambda_immutable_capture_accepted_via_ast() {
        use crate::mvl::parser::ast::{
            Block, Decl, Expr, FnDecl, LetKind, Literal, Param, Pattern, Program, Stmt, TypeExpr,
        };
        use crate::mvl::parser::lexer::Span;

        let dummy_span = Span {
            line: 1,
            col: 1,
            offset: 0,
            len: 0,
        };
        let unit_ty = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: dummy_span,
        };
        let int_ty = TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: dummy_span,
        };

        // Build: fn f() -> Unit {
        //    let x = 1;          ← immutable
        //    let g = |y: Int| -> Int { x };
        // }
        let lambda = Expr::Lambda {
            params: vec![Param {
                name: "y".into(),
                ty: int_ty.clone(),
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty.clone())),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let fn_int_int_ty = TypeExpr::Fn {
            params: vec![int_ty.clone()],
            ret: Box::new(int_ty.clone()),
            effects: vec![],
            span: dummy_span,
        };
        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                is_builtin: false,
                totality: None,
                receiver_type: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                requires: vec![],
                ensures: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            kind: LetKind::Regular,
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: int_ty,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            kind: LetKind::Regular,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: fn_int_int_ty,
                            init: lambda,
                            span: dummy_span,
                        },
                    ],
                    span: dummy_span,
                },
                span: dummy_span,
            })],
        };

        let result = check(&prog);
        assert!(
            !result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::CaptureMutabilityViolation { .. })),
            "expected no CaptureMutabilityViolation for immutable capture, got: {:?}",
            result.errors
        );
    }

    // ── Fix: if-expression branch type check ─────────────────────────────

    #[test]
    fn if_expr_branch_type_mismatch_rejected() {
        // The `if` must be in expression position (init of `let`) to hit Expr::If.
        let src = "fn f(b: Bool) -> Int { let x: Int = if b { 1 } else { true }; x }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for mismatched if-expression branches, got: {errors:?}"
        );
    }

    // ── Fix #189: match/if in tail position returns arm values, not Unit ──

    #[test]
    fn match_in_tail_position_accepted() {
        // GIVEN: a function whose only statement is a match in tail position
        // THEN: no type errors — the match's arm type satisfies the return type
        let src = r#"
            fn classify(x: Int) -> Int {
                match x {
                    0 => 1,
                    _ => 2,
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for match in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn match_in_tail_position_wrong_type_rejected() {
        // GIVEN: a function whose tail match arms return Bool but the fn declares Int
        // THEN: TypeMismatch error is emitted (was silently accepted before fix)
        let src = r#"
            fn wrong(x: Int) -> Int {
                match x {
                    0 => true,
                    _ => false,
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for tail match returning Bool in Int function, got: {errors:?}"
        );
    }

    #[test]
    fn if_else_in_tail_position_accepted() {
        // GIVEN: a function whose only statement is an if/else in tail position
        // THEN: no type errors
        let src = r#"
            fn abs(x: Int) -> Int {
                if x >= 0 {
                    x
                } else {
                    0 - x
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for if/else in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn if_else_in_tail_position_branch_mismatch_rejected() {
        // GIVEN: if/else in tail position where branches return different types
        // THEN: TypeMismatch error
        let src = r#"
            fn bad(b: Bool) -> Int {
                if b {
                    1
                } else {
                    true
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for mismatched if/else tail branches, got: {errors:?}"
        );
    }

    #[test]
    fn else_if_chain_in_tail_position_accepted() {
        // GIVEN: if/else-if/else chain in tail position, all branches return Int
        // THEN: no type errors — the nested else-if types are inferred recursively
        let src = r#"
            fn classify(x: Int) -> Int {
                if x > 0 {
                    1
                } else if x < 0 {
                    0 - 1
                } else {
                    0
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for else-if chain in tail position, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn else_if_branch_type_mismatch_rejected() {
        // GIVEN: else-if branch returns a different type than the then branch
        // THEN: TypeMismatch should be emitted (was silently accepted before fix)
        let src = r#"
            fn bad(x: Int) -> Int {
                if x > 0 {
                    1
                } else if x < 0 {
                    true
                } else {
                    0
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::TypeMismatch { .. })),
            "expected TypeMismatch for else-if branch type mismatch, got: {errors:?}"
        );
    }

    #[test]
    fn match_in_tail_position_result_ignored_rejected() {
        // GIVEN: a Unit function whose tail match returns a Result
        // THEN: ResultIgnored is emitted — same behaviour as a bare tail expression
        let src = r#"
            fn produce() -> Result[Int, String] { Ok(1) }
            fn f(x: Int) -> Unit {
                match x {
                    0 => produce(),
                    _ => produce(),
                }
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
            "expected ResultIgnored for match-in-tail-position discarding Result, got: {errors:?}"
        );
    }

    #[test]
    fn match_inside_if_else_tail_position_accepted() {
        // GIVEN: if/else in tail position where the else branch contains a match
        // THEN: no type errors — nested combinations resolve correctly
        let src = r#"
            fn f(flag: Bool, x: Int) -> Int {
                if flag {
                    0
                } else {
                    match x {
                        0 => 1,
                        _ => 2,
                    }
                }
            }
        "#;
        let result = check_src(src);
        assert!(
            result.is_ok(),
            "expected no errors for match inside if-else tail position, got: {:?}",
            result.errors
        );
    }
    // ── Fix #332: enum variant qualified paths in == / != expressions ─────

    #[test]
    fn enum_variant_qualified_path_in_eq_no_undefined_variable() {
        let src = "type Status = enum { Absent, Present }
fn f(s: Status) -> Bool { s == Status::Absent }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_ne_no_undefined_variable() {
        let src = "type Status = enum { Absent, Present }
fn f(s: Status) -> Bool { s != Status::Absent }";
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in != should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_compound_boolean_expr() {
        // Two distinct enums with different variant names — both paths must resolve.
        let src = "type A = enum { X, Y }
type B = enum { P, Q }
fn f(a: A, b: B) -> Bool { a == A::X && b == B::P }";
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedVariable { .. })),
            "compound enum == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_struct_field_eq() {
        // Matches the medical_triage scenario: field access on the left, qualified variant on the right.
        let src = r#"
            type BreathingStatus = enum { Absent, Labored, Normal }
            type Patient = struct { breathing: BreathingStatus, age: Int }
            fn is_apnoeic(v: Patient) -> Bool {
                v.breathing == BreathingStatus::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "BreathingStatus::Absent")
            ),
            "qualified enum variant in struct field == should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_let_binding() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn f() -> Bool {
                let x = Status::Absent
                x == Status::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in let binding should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_in_return_value() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn default_status() -> Status {
                Status::Absent
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant in return should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    #[test]
    fn enum_variant_qualified_path_as_fn_arg() {
        let src = r#"
            type Status = enum { Absent, Present }
            fn check(s: Status) -> Bool { s == Status::Absent }
            fn outer() -> Bool { check(Status::Absent) }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(
                |e| matches!(e, CheckError::UndefinedVariable { name, .. } if name == "Status::Absent")
            ),
            "qualified enum variant as fn arg should not emit UndefinedVariable, got: {errors:?}"
        );
    }

    // ── #798: --refinement-solver flag affects contract checking ─────────────

    #[test]
    fn solver_mode_fast_only_applies_to_requires_contract() {
        // Verify that SolverMode flows through to contract checking.
        // Layer 1 (trivial) proves `pos(1)` in both Layered and FastOnly.
        // The contract path must use the supplied mode, not a hardcoded Layered.
        let src = "fn pos(n: Int) -> Int requires n > 0 { n } fn caller() -> Int { pos(1) }";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result_fast = check_with_two_preludes_mode(&[], &[], &prog, SolverMode::FastOnly);
        let result_layered = check_with_two_preludes_mode(&[], &[], &prog, SolverMode::Layered);
        assert!(
            result_fast.is_ok(),
            "FastOnly should not produce errors for simple requires: {:?}",
            result_fast.errors
        );
        assert!(
            result_layered.is_ok(),
            "Layered should not produce errors for simple requires: {:?}",
            result_layered.errors
        );
    }

    #[test]
    fn solver_mode_fast_only_applies_to_ensures_contract() {
        // Layer 1 proves `ensures result > 0` for `return 1` in both modes.
        let src = "fn positive() -> Int ensures result > 0 { 1 }";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check_with_two_preludes_mode(&[], &[], &prog, SolverMode::FastOnly);
        assert!(
            result.is_ok(),
            "FastOnly should not error on Layer-1-provable ensures: {:?}",
            result.errors
        );
    }

    // ── Type-attached methods (#868) ──────────────────────────────────────────

    #[test]
    fn type_attached_method_resolves_on_dot_call() {
        // GIVEN: a struct type and a type-attached method
        // WHEN:  x.level() is called where x: Counter
        // THEN:  no type errors (method resolved via method_table)
        let src = r#"
type Counter = struct { value: Int }
fn Counter::get(self) -> Int { 0 }
fn main() -> Int {
    let c: Counter = Counter { value: 42 }
    c.get()
}
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result.is_ok(),
            "expected no errors for type-attached method call, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn type_attached_method_self_binds_receiver_type() {
        // GIVEN: method body references self — must resolve as the receiver type
        // THEN:  no undefined variable error
        let src = r#"
type Point = struct { x: Int, y: Int }
fn Point::x_coord(self) -> Int { self.x }
fn main() -> Int {
    let p: Point = Point { x: 3, y: 4 }
    p.x_coord()
}
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result.is_ok(),
            "expected no errors for self field access in method, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn type_attached_method_on_undefined_receiver_type_is_an_error() {
        // GIVEN: fn Ghost::method — type `Ghost` is not declared
        // THEN:  UndefinedType error
        let src = r#"
fn Ghost::haunt(self) -> Int { 0 }
fn main() -> Int { 0 }
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedType { .. })),
            "expected UndefinedType error for unknown receiver, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn type_attached_method_without_self_is_static() {
        // GIVEN: fn Counter::reset(n: Int) — no `self` first param
        // THEN:  accepted as a static/associated function (#928)
        let src = r#"
type Counter = struct { value: Int }
fn Counter::reset(n: Int) -> Unit { }
fn main() -> Unit { }
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result.is_ok(),
            "static methods (no self) should be accepted on type-attached functions"
        );
    }

    #[test]
    fn type_attached_method_duplicate_is_an_error() {
        // GIVEN: two declarations of fn Counter::reset
        // THEN:  error (duplicate method)
        let src = r#"
type Counter = struct { value: Int }
fn Counter::reset(self) -> Int { 0 }
fn Counter::reset(self) -> Int { 1 }
fn main() -> Int { 0 }
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            !result.is_ok(),
            "expected error for duplicate type-attached method, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn type_attached_method_wrong_arg_count_is_an_error() {
        // GIVEN: method declared with one param, called with two
        // THEN:  WrongArgCount error
        let src = r#"
type Counter = struct { value: Int }
fn Counter::add(self, n: Int) -> Int { 0 }
fn main() -> Int {
    let c: Counter = Counter { value: 0 };
    c.add(1, 2)
}
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::WrongArgCount { .. })),
            "expected WrongArgCount for extra arg, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn type_attached_method_undefined_call_is_an_error() {
        // GIVEN: type with one declared method, different method call attempted
        // THEN:  UndefinedFunction error (type is in method_table but method is absent)
        let src = r#"
type Foo = struct { x: Int }
fn Foo::get(self) -> Int { 0 }
fn main() -> Int {
    let f: Foo = Foo { x: 1 };
    f.nonexistent()
}
"#;
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, CheckError::UndefinedFunction { .. })),
            "expected UndefinedFunction for missing method, got: {:?}",
            result.errors
        );
    }
}
