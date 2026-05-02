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

pub mod const_eval;
pub mod context;
pub mod data_race;
pub mod errors;
pub mod ifc;
pub mod mcdc;
pub mod passes;
pub mod refinements;
pub mod termination;
pub mod types;

use crate::mvl::checker::context::{
    field_infos, variant_infos, BorrowState, FnInfo, TypeBodyInfo, TypeEnv, TypeInfo, VarInfo,
};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{
    BinaryOp, Block, Capability, ConstDecl, Decl, Effect, ElseBranch, Expr, ExternDecl, FnDecl,
    ImplDecl, LValue, Literal, MatchArm, MatchBody, Pattern, Program, SecurityLabel, Stmt,
    Totality, TypeBody, TypeDecl, UnaryOp,
};
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
}

impl CheckResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Entry point: type-check a parsed [`Program`].
/// Check a program with additional prelude programs whose declarations are
/// registered (but not checked) before the user program is type-checked.
/// Use this when stdlib files have been parsed and should be visible to the
/// checker (e.g. `use std.io.{...}` imports in corpus / CLI check mode).
pub fn check_with_prelude(prelude: &[Program], prog: &Program) -> CheckResult {
    let mut checker = TypeChecker::new();
    for p in prelude {
        checker.collect_declarations(&p.declarations);
    }
    checker.check_program(prog);
    termination::check_structural_recursion(prog, &mut checker.errors);
    data_race::check_iso_aliasing(prog, &mut checker.errors);
    ifc::check_implicit_flows(prog, &mut checker.errors);
    refinements::check_refinements(prog, &mut checker.errors);
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
    }
}

pub fn check(prog: &Program) -> CheckResult {
    check_with_prelude(&[], prog)
}

// ── Effect subsetting (002-effect-system/Req 3) ───────────────────────────────

/// Returns `true` when `declared` covers `required` for effect propagation.
///
/// Rules:
/// - Different effect names never satisfy each other.
/// - `declared` with no param (wildcard) satisfies any `required` for that name.
/// - `declared` with a param satisfies `required` with the same or more-specific param,
///   using prefix matching for path-style params (e.g. `declared("/data")` covers
///   `required("/data/config.toml")`).
/// - `declared` with a param does NOT satisfy a `required` with no param (general
///   access required but only restricted access declared).
fn effect_satisfies(declared: &Effect, required: &Effect) -> bool {
    if declared.name != required.name {
        return false;
    }
    match (&declared.param, &required.param) {
        // Wildcard declared covers everything with that name.
        (None, _) => true,
        // Specific declared cannot cover a general requirement.
        (Some(_), None) => false,
        // Both parametrized: declared must be a prefix of required (path subsetting).
        (Some(d), Some(r)) => r.starts_with(d.as_str()),
    }
}

// ── Valid effect names (002-effect-system/Req 2) ──────────────────────────────

/// The canonical set of effect names permitted in `! Effect` declarations.
///
/// Per 002-effect-system/Req 2: "Effects MUST be fine-grained, not a single `IO` bucket.
/// The minimum set: Console, FileRead, FileWrite, FileDelete, Net, DB, ProcessSpawn,
/// Random, CryptoRandom, Clock, Env, Log, Async."
///
/// `Terminal` is an extended effect for raw terminal control (cursor, colors, raw key input)
/// distinct from `Console` (line-oriented stdin/stdout). See pkg.tui / std.tui (#174).
const VALID_EFFECT_NAMES: &[&str] = &[
    "Console",
    "FileRead",
    "FileWrite",
    "FileDelete",
    "Net",
    "DB",
    "ProcessSpawn",
    "Random",
    "CryptoRandom",
    "Clock",
    "Env",
    "Log",
    "Async",
    "Terminal",
];

// ── TypeChecker ──────────────────────────────────────────────────────────────

struct TypeChecker {
    errors: Vec<CheckError>,
    env: TypeEnv,
    /// Return type of the function currently being checked (for `?` and `return`).
    current_return_ty: Option<Ty>,
    /// Name of the function currently being checked (for effect error messages).
    current_fn_name: String,
    /// Effects declared by the current function (Req 7, 8).
    current_fn_effects: Vec<Effect>,
    /// Totality of the current function (Req 8); None = implicitly total.
    current_fn_totality: Option<Totality>,
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
    /// Type parameter names in scope for the current function.
    current_type_params: HashSet<String>,
    /// Trait bounds for type params in the current function (from `where` clauses).
    current_type_constraints: HashMap<String, Vec<String>>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            errors: Vec::new(),
            env: TypeEnv::new(),
            current_return_ty: None,
            current_fn_name: String::new(),
            current_fn_effects: Vec::new(),
            current_fn_totality: None,
            extern_count: 0,
            lambda_scope_starts: Vec::new(),
            iterator_impls: HashMap::new(),
            current_type_params: HashSet::new(),
            current_type_constraints: HashMap::new(),
        }
    }

    fn emit(&mut self, err: CheckError) {
        self.errors.push(err);
    }

    // ── Program ──────────────────────────────────────────────────────────

    fn check_program(&mut self, prog: &Program) {
        self.collect_declarations(&prog.declarations);
        for decl in &prog.declarations {
            self.check_decl(decl);
        }
    }

    /// Pass 1: register all type and function signatures so forward references work.
    fn collect_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match decl {
                Decl::Type(td) => self.register_type(td),
                Decl::Fn(fd) => self.register_fn(fd),
                Decl::Const(_) => {}
                Decl::Extern(ed) => self.register_extern(ed),
                Decl::Use(_) => {} // resolved by the module resolver, not the type checker
                Decl::Impl(id) => self.register_impl(id),
            }
        }
    }

    fn register_type(&mut self, td: &TypeDecl) {
        let body_info = match &td.body {
            TypeBody::Struct(fields) => TypeBodyInfo::Struct(field_infos(fields)),
            TypeBody::Enum(variants) => TypeBodyInfo::Enum(variant_infos(variants)),
            TypeBody::Alias(ty_expr) => TypeBodyInfo::Alias(resolve(ty_expr)),
        };
        self.env.define_type(
            td.name.clone(),
            TypeInfo {
                params: td.params.clone(),
                body: body_info,
            },
        );
    }

    fn register_fn(&mut self, fd: &FnDecl) {
        let params: Vec<Ty> = fd.params.iter().map(|p| resolve(&p.ty)).collect();
        let ret = resolve(&fd.return_type);
        let type_params = fd
            .type_params
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        self.env.define_fn(
            fd.name.clone(),
            FnInfo {
                params,
                ret,
                effects: fd.effects.clone(),
                totality: fd.totality.clone(),
                type_params,
            },
        );
    }

    /// Register all functions declared inside an `extern` block so that MVL
    /// callers can resolve them as regular function calls.
    fn register_extern(&mut self, ed: &ExternDecl) {
        // Note: extern_count is incremented in check_extern_decl (pass 2) after
        // ABI validation, not here, so rejected blocks don't inflate the count.
        for f in &ed.fns {
            let params: Vec<Ty> = f.params.iter().map(|p| resolve(&p.ty)).collect();
            let ret = resolve(&f.return_type);
            self.env.define_fn(
                f.name.clone(),
                FnInfo {
                    params,
                    ret,
                    effects: f.effects.clone(),
                    totality: None,
                    type_params: HashSet::new(), // extern fns may or may not terminate
                },
            );
        }
    }

    /// Register trait implementations for use during type checking.
    /// - `impl From<A> for B` → enables `?` propagation
    /// - `impl Iterator<T> for X` → enables `X` in `for...in` loops
    fn register_impl(&mut self, id: &ImplDecl) {
        if id.trait_name == "From" {
            if let Some(source_ty) = id.trait_type_args.first() {
                let source = resolve(source_ty).display();
                self.env.register_from_impl(id.type_name.clone(), source);
            }
        } else if id.trait_name == "Iterator" {
            let elem_ty = id
                .trait_type_args
                .first()
                .map(resolve)
                .unwrap_or(Ty::Unknown);
            self.iterator_impls.insert(id.type_name.clone(), elem_ty);
        }
    }

    /// Return the iterator element type for `ty`, or emit `NotIterator` and return `Unknown`.
    ///
    /// Accepted iterator types (001-type-system Req 11):
    /// - `List<T>` — treated as `Iterator<T>` (existing behavior)
    /// - `Array<T, N>` — built-in `Iterator<T>` implementation
    /// - Any named type registered via `impl Iterator<T> for X`
    fn check_iterator_type(&mut self, ty: &Ty, span: Span) -> Ty {
        let unlabeled = ty.unlabeled();
        // Built-in iterable types.
        match unlabeled {
            Ty::List(inner) | Ty::Array(inner, _) => return *inner.clone(),
            Ty::Unknown => return Ty::Unknown, // propagate without double-reporting
            _ => {}
        }
        // User-declared iterator implementations.
        if let Ty::Named(name, _) = unlabeled {
            if let Some(elem) = self.iterator_impls.get(name).cloned() {
                return elem;
            }
        }
        self.emit(CheckError::NotIterator {
            ty: ty.display(),
            span,
        });
        Ty::Unknown
    }

    // ── Declarations ─────────────────────────────────────────────────────

    fn check_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Type(_) => {} // type declarations are structurally valid if parsed
            Decl::Fn(fd) => self.check_fn_decl(fd),
            Decl::Const(cd) => self.check_const_decl(cd),
            Decl::Extern(ed) => self.check_extern_decl(ed),
            Decl::Use(_) => {} // resolved by the module resolver, not the type checker
            Decl::Impl(_) => {} // bodies not yet type-checked; registration done in collect_declarations
        }
    }

    fn check_extern_decl(&mut self, ed: &ExternDecl) {
        // Validate ABI string: only "rust" and "c" are supported.
        // Unsupported ABIs are rejected and do NOT count toward the assurance surface.
        if ed.abi != "rust" && ed.abi != "c" {
            self.emit(CheckError::UnsupportedExternAbi {
                abi: ed.abi.clone(),
                span: ed.span,
            });
            return;
        }
        // Count only validated extern blocks in the assurance metric.
        self.extern_count += 1;
        // Each extern fn must have a valid return type (basic check).
        // Future: verify no MVL-specific types (security labels) cross the boundary
        // without explicit wrapping — for now we accept all types.
    }

    fn check_fn_decl(&mut self, fd: &FnDecl) {
        let ret_ty = resolve(&fd.return_type);
        let prev_ret = self.current_return_ty.replace(ret_ty.clone());

        // Phase C (Spec 009 Req 2): scope-based lifetime check.
        // If the return type is &T (immutable or mutable reference) and the function has
        // no &T parameters, the reference can only point to a local variable — which would
        // be deallocated when the function returns.  Reject this statically.
        // Additionally verify that the tail expression actually flows from one of those
        // &T parameters (not from a local variable or literal).
        if matches!(ret_ty, Ty::Ref(_, _)) {
            let ref_param_names: HashSet<&str> = fd
                .params
                .iter()
                .filter(|p| matches!(resolve(&p.ty), Ty::Ref(_, _)))
                .map(|p| p.name.as_str())
                .collect();
            if ref_param_names.is_empty() {
                self.emit(CheckError::ReferenceEscapesScope {
                    name: fd.name.clone(),
                    span: fd.span,
                });
            } else if let Some(bad_span) =
                block_return_flows_from_ref_param(&fd.body, &ref_param_names)
            {
                self.emit(CheckError::ReferenceEscapesScope {
                    name: fd.name.clone(),
                    span: bad_span,
                });
            }
        }

        // Validate effect names against the canonical set (002-effect-system/Req 2).
        for effect in &fd.effects {
            if !VALID_EFFECT_NAMES.contains(&effect.name.as_str()) {
                self.emit(CheckError::InvalidEffectName {
                    name: effect.name.clone(),
                    span: effect.span,
                });
            }
        }

        // Save and set effect/totality context (Req 7, 8, 9).
        let prev_fn_name = std::mem::replace(&mut self.current_fn_name, fd.name.clone());
        let prev_effects = std::mem::replace(&mut self.current_fn_effects, fd.effects.clone());
        let prev_totality = std::mem::replace(&mut self.current_fn_totality, fd.totality.clone());

        // Build type-param constraint context (001-type-system/Req 9).
        let type_params: HashSet<String> = fd
            .type_params
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        let mut type_constraints: HashMap<String, Vec<String>> = HashMap::new();
        for c in &fd.constraints {
            type_constraints
                .entry(c.name.clone())
                .or_default()
                .push(c.bound.clone());
        }
        let prev_type_params = std::mem::replace(&mut self.current_type_params, type_params);
        let prev_type_constraints =
            std::mem::replace(&mut self.current_type_constraints, type_constraints);

        // Phase D (Spec 009 Req 2): mutable-borrow alias check.
        // Two `&mut T` parameters of the same inner type, or a `&T` + `&mut T` pair,
        // could be aliased at a call site.  Reject both statically.
        // Two-pass: collect all `&T` inner types first so the check is order-independent.
        {
            let mut seen_shared_ref_types: HashSet<String> = HashSet::new();
            let mut seen_mut_ref_types: HashSet<String> = HashSet::new();
            for param in &fd.params {
                if let Ty::Ref(false, inner) = resolve(&param.ty) {
                    seen_shared_ref_types.insert(inner.display());
                }
            }
            for param in &fd.params {
                if let Ty::Ref(true, inner) = resolve(&param.ty) {
                    let key = inner.display();
                    if seen_shared_ref_types.contains(&key) {
                        self.emit(CheckError::AliasingMutableBorrow {
                            name: param.name.clone(),
                            span: param.span,
                        });
                    } else if !seen_mut_ref_types.insert(key) {
                        self.emit(CheckError::DoubleMutableBorrow {
                            name: param.name.clone(),
                            span: param.span,
                        });
                    }
                }
            }
        }

        self.env.push_scope();
        for param in &fd.params {
            let ty = resolve(&param.ty);
            self.env.define(
                param.name.clone(),
                VarInfo::new(ty, param.mutable).with_capability(param.capability.clone()),
            );
        }

        // Use infer_block_type so that the last expression in the body is
        // treated as the implicit return value rather than a discarded statement.
        // This prevents false ResultIgnored errors for `Ok(...)` / `Err(...)`
        // at the end of Result-returning functions.
        self.infer_block_type(&fd.body, Some(&ret_ty));
        self.env.pop_scope();

        self.current_return_ty = prev_ret;
        self.current_fn_name = prev_fn_name;
        self.current_fn_effects = prev_effects;
        self.current_fn_totality = prev_totality;
        self.current_type_params = prev_type_params;
        self.current_type_constraints = prev_type_constraints;
    }

    fn check_const_decl(&mut self, cd: &ConstDecl) {
        let expected = resolve(&cd.ty);
        let found = self.infer_expr(&cd.value);
        if !types_compatible(&expected, &found) {
            self.emit(CheckError::TypeMismatch {
                expected: expected.display(),
                found: found.display(),
                span: cd.value.span(),
            });
        }
    }

    // ── Blocks and statements ─────────────────────────────────────────────

    /// Check whether `branch_ty` (the implicit return of one branch of an if-statement)
    /// needs to be promoted due to the condition's security label, and emit a TypeMismatch
    /// if the promoted type is incompatible with `return_ty`.
    ///
    /// Only fires when:
    /// - the condition carries a security label (`cond_label` is `Some`),
    /// - the function declares a concrete return type (`return_ty` is `Some`),
    /// - and the branch yields a non-Unit, non-Unknown result.
    fn check_branch_label_promotion(
        &mut self,
        cond_label: Option<SecurityLabel>,
        branch_ty: &Ty,
        return_ty: Option<&Ty>,
        span: Span,
    ) {
        if let (Some(lbl), Some(ret)) = (cond_label, return_ty) {
            if !matches!(branch_ty.unlabeled(), Ty::Unit | Ty::Unknown) {
                let promoted = ifc::apply_label(Some(lbl), branch_ty.unlabeled().clone());
                if !matches!(promoted, Ty::Unknown) && !types_compatible(ret, &promoted) {
                    self.emit(CheckError::TypeMismatch {
                        expected: ret.display(),
                        found: promoted.display(),
                        span,
                    });
                }
            }
        }
    }

    fn check_block(&mut self, block: &Block, expected_ty: Option<&Ty>) {
        self.env.push_scope();
        for stmt in &block.stmts {
            self.check_stmt(stmt, expected_ty);
        }
        self.env.pop_scope();
    }

    /// Check a block and return the type of its final expression (or Unit).
    ///
    /// Used for if-expression then-branches where the block's value matters.
    /// The last `Stmt::Expr` provides the block's type; earlier statements
    /// are checked normally. Unlike `check_block`, the final expression is
    /// NOT flagged as `ResultIgnored` because its value is consumed.
    fn infer_block_type(&mut self, block: &Block, return_ty: Option<&Ty>) -> Ty {
        self.env.push_scope();
        let stmts = &block.stmts;
        let n = stmts.len();
        let mut last_ty = Ty::Unit;
        for (i, stmt) in stmts.iter().enumerate() {
            if i + 1 == n {
                // Tail-position statement: infer its type so the block propagates the
                // correct return value.  `match` and `if/else` in tail position produce
                // their arm/branch values, not Unit.
                match stmt {
                    Stmt::Expr { expr, .. } => {
                        last_ty = self.infer_expr(expr);

                        // Check implicit return type against declared return type.
                        // Resolve named alias types (e.g. PositiveInt = Int) before comparing
                        // so that `Int` is accepted where a refined alias is declared.
                        // For Result types, ResultIgnored below is the more specific error.
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !types_compatible(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: expr.span(),
                                });
                            }
                        }

                        // Suppress ResultIgnored only when the block's expected return
                        // type is itself compatible with Result (the value is used).
                        // If the expected return type is Unit or incompatible, the
                        // caller is discarding the Result — emit ResultIgnored as usual.
                        if last_ty.is_result() {
                            let consumed_by_caller = return_ty
                                .map(|rt| types_compatible(rt, &last_ty))
                                .unwrap_or(false);
                            if !consumed_by_caller {
                                self.emit(CheckError::ResultIgnored { span: expr.span() });
                            }
                        }
                        break;
                    }

                    Stmt::Match {
                        scrutinee,
                        arms,
                        span,
                    } => {
                        // `match` in tail position: check arms and infer the block's type.
                        let scrutinee_ty = self.infer_expr(scrutinee);
                        last_ty = self.check_match_arms(arms, &scrutinee_ty, *span, return_ty);
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !types_compatible(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: *span,
                                });
                            }
                        }
                        // Mirror the ResultIgnored check from Stmt::Expr: a tail match
                        // that produces an unhandled Result must still be flagged.
                        if last_ty.is_result() {
                            let consumed_by_caller = return_ty
                                .map(|rt| types_compatible(rt, &last_ty))
                                .unwrap_or(false);
                            if !consumed_by_caller {
                                self.emit(CheckError::ResultIgnored { span: *span });
                            }
                        }
                        break;
                    }

                    Stmt::If {
                        cond,
                        then,
                        else_,
                        span,
                    } => {
                        // `if/else` in tail position: delegate to helper so that
                        // `else if` chains are also inferred recursively.
                        last_ty = self.infer_tail_if(cond, then, else_, *span, return_ty);
                        // Check the overall result against the declared return type.
                        if let Some(ret) = return_ty {
                            let resolved_ret = self.resolve_alias(ret.clone());
                            if !matches!(last_ty, Ty::Unknown)
                                && !matches!(resolved_ret, Ty::Unknown)
                                && !last_ty.is_result()
                                && !types_compatible(&resolved_ret, &last_ty)
                            {
                                self.emit(CheckError::TypeMismatch {
                                    expected: ret.display(),
                                    found: last_ty.display(),
                                    span: *span,
                                });
                            }
                        }
                        break;
                    }

                    _ => {
                        // A tail `return` statement means the block always diverges
                        // and never falls through.  Use Unknown (the "skip" sentinel)
                        // so callers don't see a spurious `Unit` type — the return
                        // value's compatibility with `return_ty` is already checked
                        // inside `check_stmt` for `Stmt::Return`.
                        if matches!(stmt, Stmt::Return { .. }) {
                            last_ty = Ty::Unknown;
                        }
                        self.check_stmt(stmt, return_ty);
                        break;
                    }
                }
            }
            self.check_stmt(stmt, return_ty);
        }
        self.env.pop_scope();
        last_ty
    }

    /// Infer the type of an `if/else` in tail position, handling `else if` chains
    /// recursively so that every branch contributes to the block's return type.
    /// Returns the inferred type (the then-branch type when branches are compatible).
    fn infer_tail_if(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: &Option<ElseBranch>,
        span: Span,
        return_ty: Option<&Ty>,
    ) -> Ty {
        let cond_ty = self.infer_expr(cond);
        if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
            self.emit(CheckError::TypeMismatch {
                expected: "Bool".to_string(),
                found: cond_ty.display(),
                span: cond.span(),
            });
        }
        let cond_label = ifc::label_of(&cond_ty);
        let then_ty = self.infer_block_type(then, return_ty);
        self.check_branch_label_promotion(cond_label, &then_ty, return_ty, span);
        let result_ty = then_ty;
        if let Some(else_branch) = else_ {
            match else_branch {
                ElseBranch::Block(b) => {
                    let else_ty = self.infer_block_type(b, return_ty);
                    self.check_branch_label_promotion(cond_label, &else_ty, return_ty, span);
                    if !matches!(result_ty, Ty::Unknown)
                        && !matches!(else_ty, Ty::Unknown)
                        && !types_compatible(&result_ty, &else_ty)
                    {
                        self.emit(CheckError::TypeMismatch {
                            expected: result_ty.display(),
                            found: else_ty.display(),
                            span,
                        });
                    }
                }
                ElseBranch::If(nested_if) => {
                    // `else if` chain: recurse so the nested if's type is also
                    // inferred and checked for compatibility with the then-branch.
                    if let Stmt::If {
                        cond: c,
                        then: t,
                        else_: e,
                        span: s,
                    } = nested_if.as_ref()
                    {
                        let nested_ty = self.infer_tail_if(c, t, e, *s, return_ty);
                        self.check_branch_label_promotion(cond_label, &nested_ty, return_ty, span);
                        if !matches!(result_ty, Ty::Unknown)
                            && !matches!(nested_ty, Ty::Unknown)
                            && !types_compatible(&result_ty, &nested_ty)
                        {
                            self.emit(CheckError::TypeMismatch {
                                expected: result_ty.display(),
                                found: nested_ty.display(),
                                span,
                            });
                        }
                    } else {
                        // Shouldn't happen by construction (ElseBranch::If always wraps
                        // Stmt::If), but fall back gracefully.
                        self.check_stmt(nested_if, return_ty);
                    }
                }
            }
        }
        result_ty
    }

    fn check_stmt(&mut self, stmt: &Stmt, return_ty: Option<&Ty>) {
        match stmt {
            Stmt::Let {
                mutable,
                pattern,
                ty,
                init,
                span,
            } => {
                let init_ty = self.infer_expr(init);
                if let Some(ann) = ty {
                    let ann_ty = resolve(ann);
                    // Phase C (#305, #363): scope-depth check for any reference assignment.
                    // Covers both implicit borrow (`let r: &T = x` where x: T) and explicit
                    // borrow / ref-copy (`let r: &T = &x` or `let r: &T = existing_ref`).
                    let is_ref_assignment = if let Ty::Ref(_, inner_ty) = &ann_ty {
                        types_compatible(inner_ty, &init_ty) || types_compatible(&ann_ty, &init_ty)
                    } else {
                        false
                    };
                    if is_ref_assignment {
                        self.check_borrow_lifetime(pattern, init);
                    } else if !types_compatible(&ann_ty, &init_ty) {
                        self.emit(CheckError::TypeMismatch {
                            expected: ann_ty.display(),
                            found: init_ty.display(),
                            span: init.span(),
                        });
                    }
                    self.bind_pattern(pattern, &ann_ty, *mutable);
                } else {
                    self.emit(CheckError::MissingTypeAnnotation { span: *span });
                    self.bind_pattern(pattern, &init_ty, *mutable);
                }
                // Phase D (#362): record which variable the new binding borrows so that
                // `pop_scope()` can release the borrow when the binding goes out of scope.
                // Also update the referent's borrow_state here (not in Expr::Borrow) so
                // that state is only set when borrows_var is simultaneously recorded.
                if let (Pattern::Ident(bound_name, _), Expr::Borrow { expr, mutable, .. }) =
                    (pattern, init)
                {
                    if let Expr::Ident(borrowed_name, _) = expr.as_ref() {
                        if let Some(bound_info) = self.env.lookup_mut_var(bound_name) {
                            bound_info.borrows_var = Some(borrowed_name.clone());
                        }
                        if let Some(referent) = self.env.lookup_mut_var(borrowed_name) {
                            referent.borrow_state = if *mutable {
                                BorrowState::MutablyBorrowed
                            } else {
                                match referent.borrow_state.clone() {
                                    BorrowState::SharedBorrowed(n) => {
                                        BorrowState::SharedBorrowed(n + 1)
                                    }
                                    _ => BorrowState::SharedBorrowed(1),
                                }
                            };
                        }
                    }
                }
                // #14: ResultIgnored — if the init expression is a Result and
                // it's not being used at all, that would be caught at Stmt::Expr.
                // Here the Result is being bound, which is acceptable.
                let _ = span;
            }

            // #17: immutability enforcement
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let val_ty = self.infer_expr(value);
                self.check_assignment(target, &val_ty, *span);
            }

            Stmt::Return { value, span } => {
                if let Some(expr) = value {
                    let found = self.infer_expr(expr);
                    if let Some(ret) = return_ty {
                        if !types_compatible(ret, &found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: ret.display(),
                                found: found.display(),
                                span: *span,
                            });
                        }
                    }
                }
            }

            Stmt::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                // Extract the condition's security label for implicit return-type promotion.
                // Branching on Secret<Bool> or Tainted<Bool> means the choice of branch
                // reveals the condition's value; non-Unit results must be promoted.
                let cond_label = ifc::label_of(&cond_ty);

                let then_ty = self.infer_block_type(then, return_ty);
                // After the normal branch check, apply label promotion for non-Unit results:
                // the implicit return of an if-statement branch inherits at least the
                // condition's label. Skip Unit (carries no information).
                self.check_branch_label_promotion(cond_label, &then_ty, return_ty, *span);

                if let Some(else_branch) = else_ {
                    match else_branch {
                        ElseBranch::Block(b) => {
                            let else_ty = self.infer_block_type(b, return_ty);
                            self.check_branch_label_promotion(
                                cond_label, &else_ty, return_ty, *span,
                            );
                        }
                        // `else if` chains: recurse so each nested if also gets
                        // promotion applied to its own branches.
                        ElseBranch::If(s) => self.check_stmt(s, return_ty),
                    }
                }
            }

            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                self.check_match_arms(arms, &scrutinee_ty, *span, return_ty);
            }

            Stmt::For {
                pattern,
                iter,
                body,
                span,
            } => {
                // Req 8: `for` loops are bounded (total) — reject in `partial` functions.
                if matches!(self.current_fn_totality, Some(Totality::Partial)) {
                    self.emit(CheckError::ForLoopInPartialFn { span: *span });
                }
                let iter_ty = self.infer_expr(iter);
                let iter_span = iter.span();
                let elem_ty = self.check_iterator_type(&iter_ty, iter_span);
                self.env.push_scope();
                self.bind_pattern(pattern, &elem_ty, false);
                self.check_block(body, return_ty);
                self.env.pop_scope();
            }

            Stmt::While { cond, body, span } => {
                let cond_ty = self.infer_expr(cond);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                // Req 8: reject `while` in total functions (only `for` is bounded).
                // Unannotated `fn` is implicitly total and also rejects while loops.
                if !matches!(self.current_fn_totality, Some(Totality::Partial)) {
                    self.emit(CheckError::UnboundedLoopInTotal { span: *span });
                }
                self.check_block(body, return_ty);
            }

            // #14: Reject bare Result expressions (ResultIgnored)
            Stmt::Expr { expr, .. } => {
                let ty = self.infer_expr(expr);
                if ty.is_result() {
                    self.emit(CheckError::ResultIgnored { span: expr.span() });
                }
            }
        }
    }

    // ── Assignment target (#17 immutability) ─────────────────────────────

    fn check_assignment(&mut self, target: &LValue, val_ty: &Ty, span: Span) {
        match target {
            LValue::Ident(name, _) => {
                if let Some(info) = self.env.lookup(name).cloned() {
                    if !info.mutable {
                        self.emit(CheckError::AssignToImmutable {
                            name: name.clone(),
                            span,
                        });
                    }
                    // #17: also verify the assigned value is type-compatible
                    if !types_compatible(&info.ty, val_ty) {
                        self.emit(CheckError::TypeMismatch {
                            expected: info.ty.display(),
                            found: val_ty.display(),
                            span,
                        });
                    }
                } else {
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span,
                    });
                }
            }
            LValue::Field {
                base,
                field,
                span: field_span,
            } => {
                let base_ty = self.infer_lvalue(base);
                // Check that the specific field is mutable.
                self.check_field_mutation(&base_ty, field, *field_span);
                // Check the assigned value against the FIELD type (not the base struct type).
                // Recursing with val_ty into check_assignment on the base would incorrectly
                // compare the base struct type against the field value type.
                let field_ty = self.field_type(&base_ty, field).unwrap_or(Ty::Unknown);
                if !matches!(field_ty, Ty::Unknown) && !types_compatible(&field_ty, val_ty) {
                    self.emit(CheckError::TypeMismatch {
                        expected: field_ty.display(),
                        found: val_ty.display(),
                        span,
                    });
                }
            }
        }
    }

    /// Resolve a named type through the type environment if it is a type alias.
    /// Returns the alias base type (with Refined stripped), or the original type if not an alias.
    /// Used for return-type and arithmetic checks where named aliases should be transparent.
    fn resolve_alias(&self, ty: Ty) -> Ty {
        use crate::mvl::checker::context::TypeBodyInfo;
        if let Ty::Named(ref name, _) = ty {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Alias(inner) = &type_info.body {
                    return inner.base().clone();
                }
            }
        }
        ty
    }

    /// Resolve named aliases inside a Labeled wrapper.
    /// E.g. `Public<Amount>` where `Amount = Float where ...` → `Public<Float>`.
    fn resolve_alias_in_labeled(&self, ty: Ty) -> Ty {
        if let Ty::Labeled(label, inner) = ty {
            Ty::Labeled(label, Box::new(self.resolve_alias(*inner)))
        } else {
            self.resolve_alias(ty)
        }
    }

    fn infer_lvalue(&self, target: &LValue) -> Ty {
        match target {
            LValue::Ident(name, _) => self
                .env
                .lookup(name)
                .map(|i| i.ty.clone())
                .unwrap_or(Ty::Unknown),
            LValue::Field { base, field, .. } => {
                let base_ty = self.infer_lvalue(base);
                self.field_type(&base_ty, field).unwrap_or(Ty::Unknown)
            }
        }
    }

    fn check_field_mutation(&mut self, ty: &Ty, field: &str, span: Span) {
        let base = ty.unlabeled();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name).cloned() {
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
                    if let Some(fi) = fields.iter().find(|f| f.name == field) {
                        if !fi.mutable {
                            self.emit(CheckError::MutateImmutableField {
                                ty: name.clone(),
                                field: field.to_string(),
                                span,
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Expression type inference ─────────────────────────────────────────

    fn infer_expr(&mut self, expr: &Expr) -> Ty {
        match expr {
            // #11: Literals
            Expr::Literal(lit, _) => self.infer_literal(lit),

            // #11/#15: Variable reference
            Expr::Ident(name, span) => {
                if let Some((scope_idx, info)) = self.env.lookup_with_scope_index(name) {
                    // Clone early to release the borrow on `self.env` before calling self.emit.
                    let is_mutable = info.mutable;
                    let is_moved = info.moved;
                    let ty = info.ty.clone();

                    // ADR-0002: Lambdas may only capture immutable bindings.
                    // If we are inside a lambda and the variable was found in a scope
                    // that predates the lambda's own scope, it is a captured binding.
                    if let Some(&boundary) = self.lambda_scope_starts.last() {
                        if scope_idx < boundary && is_mutable {
                            self.emit(CheckError::CaptureMutabilityViolation {
                                name: name.clone(),
                                span: *span,
                            });
                        }
                    }
                    // #15: ownership — reject use after move
                    if is_moved {
                        self.emit(CheckError::UseAfterMove {
                            name: name.clone(),
                            span: *span,
                        });
                        return Ty::Unknown;
                    }
                    ty
                } else {
                    // Before emitting UndefinedVariable, check whether the ident
                    // is a known enum unit-variant or the built-in `None`.
                    if name == "None" {
                        return Ty::Option(Box::new(Ty::Unknown));
                    }
                    // Bare variant: `DivisionByZero` or path: `MathError::DivisionByZero`
                    let variant_name = if let Some((_, v)) = name.split_once("::") {
                        v
                    } else {
                        name.as_str()
                    };
                    if let Some(enum_ty) = self.lookup_enum_for_variant(variant_name) {
                        return enum_ty;
                    }
                    // Function reference: `xs.map(double)` — ident is a known function name.
                    // Return Ty::Fn so callers like map/filter can infer the output type.
                    if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
                        return Ty::Fn(fn_info.params.clone(), Box::new(fn_info.ret.clone()));
                    }
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span: *span,
                    });
                    Ty::Unknown
                }
            }

            // #11: Binary operations
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.infer_binary(*op, left, right, *span),

            Expr::Unary { op, expr, span } => self.infer_unary(*op, expr, *span),

            Expr::Borrow {
                mutable,
                expr,
                span,
            } => {
                let inner = self.infer_expr(expr);
                // Fix 1: reject `&mut x` on an immutable binding (#366)
                if *mutable {
                    if let Expr::Ident(name, _) = expr.as_ref() {
                        if let Some(info) = self.env.lookup(name).cloned() {
                            if !info.mutable {
                                self.emit(CheckError::AssignToImmutable {
                                    name: name.clone(),
                                    span: *span,
                                });
                            }
                        }
                    }
                }
                // Fix 4: reject `&&x` — borrowing an already-borrowed value (#366)
                if let Ty::Ref(_, _) = &inner {
                    self.emit(CheckError::TypeMismatch {
                        expected: inner.display(),
                        found: format!("&{}", inner.display()),
                        span: *span,
                    });
                    return Ty::Unknown;
                }
                // Phase D (#362): check BorrowState on the referent (error only).
                // State updates are deferred to Stmt::Let where borrows_var is also set,
                // so that borrow_state is always released on scope exit. Updating state
                // here (expression position) would leak when the borrow is not `let`-bound.
                if let Expr::Ident(name, _) = expr.as_ref() {
                    let current = self
                        .env
                        .lookup(name)
                        .map(|i| i.borrow_state.clone())
                        .unwrap_or(BorrowState::Owned);

                    if *mutable {
                        if current != BorrowState::Owned {
                            self.emit(CheckError::AliasingMutableBorrow {
                                name: name.clone(),
                                span: *span,
                            });
                        }
                    } else if matches!(current, BorrowState::MutablyBorrowed) {
                        self.emit(CheckError::AliasingMutableBorrow {
                            name: name.clone(),
                            span: *span,
                        });
                    }
                }
                Ty::Ref(*mutable, Box::new(inner))
            }

            // #12: Field access — reject direct field access on enum or Option
            Expr::FieldAccess { expr, field, span } => {
                let ty = self.infer_expr(expr);
                // #14: Option direct access
                if ty.is_option() {
                    self.emit(CheckError::OptionDirectAccess { span: *span });
                    return Ty::Unknown;
                }
                self.field_type_checked(&ty, field, *span)
            }

            // #11: Function call
            Expr::FnCall {
                name, args, span, ..
            } => self.infer_fn_call(name, args, *span),

            Expr::MethodCall {
                receiver,
                method,
                args,
                span,
            } => {
                let recv_ty = self.infer_expr(receiver);
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();
                // Req 9: capability check for actor-boundary crossings.
                // `channel.send(val)` — first argument must be `iso` or `val`.
                if method == "send" {
                    if let Some(first_arg) = args.first() {
                        self.check_send_capability(first_arg, *span);
                    }
                }
                // Stdlib method resolution (#43): dispatch on receiver type.
                // IFC labels propagate through method results via the receiver label.
                self.infer_method_call(&recv_ty, method, &arg_tys, *span)
            }

            // #13: Match expressions
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                self.infer_match_expr(arms, &scrutinee_ty, *span)
            }

            Expr::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                // IFC: extract the condition's security label for implicit flow promotion.
                // Branching on Secret<Bool> must promote the result to at least Secret<T>;
                // otherwise the choice of branch would leak the guard's value.
                let cond_label = ifc::label_of(&cond_ty);
                if !cond_ty.is_bool() && !matches!(cond_ty, Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                let then_ty = self.infer_block_type(then, None);
                // Promote branch type by joining with the condition's label (#26 implicit flow).
                let promoted_then = {
                    let label = ifc::join_opt(cond_label, ifc::label_of(&then_ty));
                    ifc::apply_label(label, then_ty.unlabeled().clone())
                };
                if let Some(else_expr) = else_ {
                    let else_ty = self.infer_expr(else_expr);
                    let promoted_else = {
                        let label = ifc::join_opt(cond_label, ifc::label_of(&else_ty));
                        ifc::apply_label(label, else_ty.unlabeled().clone())
                    };
                    if !matches!(promoted_then, Ty::Unknown)
                        && !matches!(promoted_else, Ty::Unknown)
                        && !types_compatible(&promoted_then, &promoted_else)
                    {
                        self.emit(CheckError::TypeMismatch {
                            expected: promoted_then.display(),
                            found: promoted_else.display(),
                            span: *span,
                        });
                    }
                    if matches!(promoted_then, Ty::Unknown) {
                        promoted_else
                    } else {
                        promoted_then
                    }
                } else {
                    promoted_then
                }
            }

            Expr::Block(block) => {
                // Infer the type of the last expression so that block-expressions
                // (e.g. the else-branch of an if-expression) return the correct type.
                self.infer_block_type(block, None)
            }

            // #12: Struct construction
            Expr::Construct { name, fields, span } => self.check_construction(name, fields, *span),

            Expr::List { elems, .. } => {
                let elem_ty = elems
                    .first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) {
                    self.infer_expr(e);
                }
                Ty::List(Box::new(elem_ty))
            }

            Expr::Map { pairs, .. } => {
                // Join the labels of all value expressions so the resulting Map
                // type reflects any sensitivity present in the values (#54, Req 6).
                // This ensures `{"k": secret_val}` is typed as
                // `Secret<Map<String,String>>` rather than `Map<String,Secret<String>>`,
                // making the standard `label_of` check work for log-sink enforcement.
                let mut joined_label: Option<crate::mvl::parser::ast::SecurityLabel> = None;
                let (key_ty, val_ty) = pairs
                    .first()
                    .map(|(k, v)| {
                        let kt = self.infer_expr(k);
                        let vt = self.infer_expr(v);
                        joined_label = ifc::join_opt(joined_label, ifc::label_of(&vt));
                        (kt, vt.unlabeled().clone())
                    })
                    .unwrap_or((Ty::Unknown, Ty::Unknown));
                for (k, v) in pairs.iter().skip(1) {
                    self.infer_expr(k);
                    let vt = self.infer_expr(v);
                    joined_label = ifc::join_opt(joined_label, ifc::label_of(&vt));
                }
                let map_ty = Ty::Named("Map".into(), vec![key_ty, val_ty]);
                ifc::apply_label(joined_label, map_ty)
            }

            Expr::Set { elems, .. } => {
                let elem_ty = elems
                    .first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) {
                    self.infer_expr(e);
                }
                Ty::Named("Set".into(), vec![elem_ty])
            }

            // #14: `?` propagation
            Expr::Propagate { expr, span } => {
                let ty = self.infer_expr(expr);
                if !ty.is_propagatable() && !matches!(ty, Ty::Unknown) {
                    self.emit(CheckError::PropagateNotResult {
                        ty: ty.display(),
                        span: *span,
                    });
                    return Ty::Unknown;
                }
                // If both the expression and enclosing function return Result types,
                // verify error types are compatible — either identical or convertible via From.
                if let (Ty::Result(_, expr_err), Some(Ty::Result(_, ret_err))) = (
                    ty.unlabeled(),
                    self.current_return_ty.as_ref().map(|t| t.unlabeled()),
                ) {
                    let from_ty = expr_err.display();
                    let into_ty = ret_err.display();
                    if from_ty != into_ty
                        && !matches!(**expr_err, Ty::Unknown)
                        && !matches!(**ret_err, Ty::Unknown)
                        && !self.env.has_from_impl(&into_ty, &from_ty)
                    {
                        self.emit(CheckError::PropagateIncompatibleError {
                            from_ty,
                            into_ty,
                            span: *span,
                        });
                    }
                }
                ty.propagate_inner()
            }

            // #15: explicit move — infer first, then mark as moved so
            // subsequent references to the same binding are caught.
            Expr::Move { expr, .. } => {
                let ty = self.infer_expr(expr);
                if let Expr::Ident(name, _) = expr.as_ref() {
                    self.env.mark_moved(name);
                }
                ty
            }

            Expr::Consume { expr, .. } => self.infer_expr(expr),

            // #27: declassify() — converts Secret<T> to Public<T>
            Expr::Declassify { expr, span } => {
                let inner_ty = self.infer_expr(expr);
                match inner_ty.base() {
                    Ty::Labeled(SecurityLabel::Secret, inner) => {
                        Ty::Labeled(SecurityLabel::Public, inner.clone())
                    }
                    Ty::Unknown => Ty::Labeled(SecurityLabel::Public, Box::new(Ty::Unknown)),
                    _ => {
                        self.emit(CheckError::InvalidDeclassify {
                            found: inner_ty.display(),
                            span: *span,
                        });
                        Ty::Unknown
                    }
                }
            }

            // #27: sanitize() — converts Tainted<T> to Clean<T>
            Expr::Sanitize { expr, span } => {
                let inner_ty = self.infer_expr(expr);
                match inner_ty.base() {
                    Ty::Labeled(SecurityLabel::Tainted, inner) => {
                        Ty::Labeled(SecurityLabel::Clean, inner.clone())
                    }
                    Ty::Unknown => Ty::Labeled(SecurityLabel::Clean, Box::new(Ty::Unknown)),
                    _ => {
                        self.emit(CheckError::InvalidSanitize {
                            found: inner_ty.display(),
                            span: *span,
                        });
                        Ty::Unknown
                    }
                }
            }

            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => {
                // Record the current scope depth as the lambda boundary so that
                // Expr::Ident can detect mutable captures (ADR-0002).
                let boundary = self.env.scope_depth();
                self.lambda_scope_starts.push(boundary);

                self.env.push_scope();
                let param_tys: Vec<Ty> = params
                    .iter()
                    .map(|p| {
                        let ty = resolve(&p.ty);
                        self.env
                            .define(p.name.clone(), VarInfo::new(ty.clone(), p.mutable));
                        ty
                    })
                    .collect();
                let ret_ty = ret_type.as_ref().map(|t| resolve(t)).unwrap_or(Ty::Unknown);
                let body_ty = self.infer_expr(body);
                // Verify body type matches declared return annotation
                if !matches!(ret_ty, Ty::Unknown)
                    && !matches!(body_ty, Ty::Unknown)
                    && !types_compatible(&ret_ty, &body_ty)
                {
                    self.emit(CheckError::TypeMismatch {
                        expected: ret_ty.display(),
                        found: body_ty.display(),
                        span: body.span(),
                    });
                }
                self.env.pop_scope();
                self.lambda_scope_starts.pop();
                Ty::Fn(param_tys, Box::new(ret_ty))
            }
        }
    }

    // ── Literal types (#11) ───────────────────────────────────────────────

    fn infer_literal(&self, lit: &Literal) -> Ty {
        match lit {
            Literal::Integer(_) => Ty::Int,
            Literal::Float(_) => Ty::Float,
            Literal::Str(_) => Ty::String,
            Literal::Char(_) => Ty::Char,
            Literal::Bool(_) => Ty::Bool,
            Literal::Unit => Ty::Unit,
        }
    }

    // ── Binary operations (#11) ───────────────────────────────────────────

    fn infer_binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr, span: Span) -> Ty {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);

        match op {
            // Arithmetic: both operands must be numeric and the same type.
            // Labels propagate via join: Secret<Int> + Public<Int> → Secret<Int>.
            // Named alias types (e.g. Amount = Float where ...) are resolved before checking.
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                // Resolve named aliases so that Labeled<NamedAlias> is treated as Labeled<baseType>.
                let lt = self.resolve_alias_in_labeled(lt);
                let rt = self.resolve_alias_in_labeled(rt);
                if !matches!(lt, Ty::Unknown) && !lt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: lt.display(),
                        span: left.span(),
                    });
                    return Ty::Unknown;
                }
                if !matches!(rt, Ty::Unknown) && !rt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: rt.display(),
                        span: right.span(),
                    });
                    return Ty::Unknown;
                }
                // Compare unlabeled base types to allow mixed-label arithmetic
                let lt_inner = lt.unlabeled().clone();
                let rt_inner = rt.unlabeled().clone();
                if !matches!(lt_inner, Ty::Unknown)
                    && !matches!(rt_inner, Ty::Unknown)
                    && lt_inner != rt_inner
                {
                    self.emit(CheckError::ArithmeticTypeMismatch {
                        op: format!("{op:?}").to_lowercase(),
                        left: lt.display(),
                        right: rt.display(),
                        span,
                    });
                    return Ty::Unknown;
                }
                // Propagate the join of labels to the result (#26)
                let label = ifc::join_opt(ifc::label_of(&lt), ifc::label_of(&rt));
                let base = if matches!(lt_inner, Ty::Unknown) {
                    rt_inner
                } else {
                    lt_inner
                };
                ifc::apply_label(label, base)
            }

            // Comparison: both sides same type → Bool
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => {
                // Constraint enforcement: unconstrained type params may not be compared.
                // `<`, `>`, `<=`, `>=` require `Ord`; `==`, `!=` require `Eq`.
                // `Ord` is a supertype of `Eq`, so `where T: Ord` satisfies an `Eq` check.
                let required_bound = match op {
                    BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => "Ord",
                    BinaryOp::Eq | BinaryOp::Ne => "Eq",
                    _ => unreachable!(),
                };
                for operand_ty in [&lt, &rt] {
                    if let Ty::Named(name, args) = operand_ty.unlabeled() {
                        if args.is_empty() && self.current_type_params.contains(name) {
                            let has_bound =
                                self.current_type_constraints
                                    .get(name)
                                    .is_some_and(|bounds| {
                                        bounds.iter().any(|b| {
                                            b == required_bound
                                                || (required_bound == "Eq" && b == "Ord")
                                        })
                                    });
                            if !has_bound {
                                self.emit(CheckError::MissingConstraint {
                                    type_param: name.clone(),
                                    required_bound: required_bound.to_string(),
                                    span,
                                });
                            }
                        }
                    }
                }
                if !matches!(lt, Ty::Unknown)
                    && !matches!(rt, Ty::Unknown)
                    && !types_compatible(&lt, &rt)
                {
                    self.emit(CheckError::TypeMismatch {
                        expected: lt.display(),
                        found: rt.display(),
                        span,
                    });
                }
                Ty::Bool
            }

            // Logic: both must be Bool (labels stripped — Bool logic yields Bool)
            BinaryOp::And | BinaryOp::Or => {
                let op_str = format!("{op:?}").to_lowercase();
                if !matches!(lt.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str.clone(),
                        ty: lt.display(),
                        span: left.span(),
                    });
                }
                if !matches!(rt.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str,
                        ty: rt.display(),
                        span: right.span(),
                    });
                }
                Ty::Bool
            }
        }
    }

    fn infer_unary(&mut self, op: UnaryOp, expr: &Expr, span: Span) -> Ty {
        let ty = self.infer_expr(expr);
        match op {
            UnaryOp::Neg => {
                if !matches!(ty, Ty::Unknown) && !ty.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: ty.display(),
                        span,
                    });
                    Ty::Unknown
                } else {
                    ty
                }
            }
            UnaryOp::Not => {
                if !matches!(ty.unlabeled(), Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: ty.display(),
                        span,
                    });
                }
                Ty::Bool
            }
            UnaryOp::Deref => {
                // Deref `*expr`: if expr has type Box<T>, return T.
                match ty {
                    Ty::Named(ref name, ref args) if name == "Box" && args.len() == 1 => {
                        args[0].clone()
                    }
                    Ty::Unknown => Ty::Unknown,
                    _ => {
                        self.emit(CheckError::TypeMismatch {
                            expected: "Box<T>".to_string(),
                            found: ty.display(),
                            span,
                        });
                        Ty::Unknown
                    }
                }
            }
        }
    }

    // ── Enum constructor resolution (#12) ────────────────────────────────
    //
    // `Some(v)`, `Ok(v)`, `Err(e)` and user-defined tuple-variant constructors
    // are parsed as `Expr::FnCall` because they syntactically look like calls.
    // `None` and unit variants are `Expr::Ident`.  We must recognise them
    // before falling through to UndefinedFunction / UndefinedVariable.

    /// Return the enum type that contains a variant named `variant`, or `None`.
    fn lookup_enum_for_variant(&self, variant: &str) -> Option<Ty> {
        for (type_name, type_info) in &self.env.types {
            if let TypeBodyInfo::Enum(variants) = &type_info.body {
                if variants.iter().any(|v| v.name == variant) {
                    return Some(Ty::Named(type_name.clone(), vec![]));
                }
            }
        }
        None
    }

    // ── Function calls (#11) ──────────────────────────────────────────────

    fn infer_fn_call(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
        // Infer all argument types (for side-effect error collection)
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();

        // 003-information-flow/Req 6: logging functions MUST accept only Public<T>.
        // Reject any argument labeled Secret, Tainted, or Clean (Clean is sanitized
        // but not declassified — an explicit declassify() is required before logging).
        // Covers println/print (Console) and log_debug/info/warn/error (! Log, #54).
        if matches!(
            name,
            "println" | "print" | "log_debug" | "log_info" | "log_warn" | "log_error"
        ) {
            for (arg, arg_ty) in args.iter().zip(arg_tys.iter()) {
                if let Some(label) = ifc::label_of(arg_ty) {
                    if matches!(
                        label,
                        crate::mvl::parser::ast::SecurityLabel::Secret
                            | crate::mvl::parser::ast::SecurityLabel::Tainted
                            | crate::mvl::parser::ast::SecurityLabel::Clean
                    ) {
                        self.emit(CheckError::LoggingLabelViolation {
                            label: ifc::label_name(label).to_string(),
                            span: arg.span(),
                        });
                    }
                }
            }
        }

        if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
            // Variadic built-ins (println, print, assert_eq) have an empty params
            // vec as a sentinel — skip arity and type checking for them.
            // log_* (#54) are also treated as variadic so that the IFC label check
            // (above) validates each argument individually without a fixed-arity guard.
            let is_variadic_builtin = matches!(
                name,
                "println"
                    | "print"
                    | "assert_eq"
                    | "parse_int"
                    | "format"
                    | "choice"
                    | "shuffle"
                    | "float"
                    | "log_debug"
                    | "log_info"
                    | "log_warn"
                    | "log_error"
            );
            // L5-08: Generic functions are monomorphized at the LLVM level.
            // Skip arity and type checking at call sites; the LLVM backend handles
            // concrete type dispatch.
            let is_generic = !fn_info.type_params.is_empty();
            if !is_variadic_builtin && !is_generic && fn_info.params.len() != arg_tys.len() {
                self.emit(CheckError::WrongArgCount {
                    name: name.to_string(),
                    expected: fn_info.params.len(),
                    found: arg_tys.len(),
                    span,
                });
                return fn_info.ret.clone();
            }
            if !is_variadic_builtin && !is_generic {
                for (i, (expected, found)) in fn_info.params.iter().zip(arg_tys.iter()).enumerate()
                {
                    if !types_compatible(expected, found) {
                        self.emit(CheckError::TypeMismatch {
                            expected: expected.display(),
                            found: found.display(),
                            span: args[i].span(),
                        });
                    }
                }
            }

            // Req 7/8: Effect propagation — caller must declare all effects of callee.
            // Req 3: Parametrized effects — declared `/data` covers required `/data/file.txt`
            // (prefix subsetting via `effect_satisfies`).
            for required in &fn_info.effects {
                let covered = self
                    .current_fn_effects
                    .iter()
                    .any(|declared| effect_satisfies(declared, required));
                if !covered {
                    if self.current_fn_effects.is_empty() {
                        // Pure function calling effectful one (#19)
                        self.emit(CheckError::UndeclaredEffect {
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    } else {
                        // Caller has some effects but not this one (#20)
                        self.emit(CheckError::MissingEffect {
                            caller: self.current_fn_name.clone(),
                            callee: name.to_string(),
                            effect: required.to_string(),
                            span,
                        });
                    }
                }
            }

            // Req 8: Total function must not call partial functions.
            if matches!(fn_info.totality, Some(Totality::Partial))
                && !matches!(self.current_fn_totality, Some(Totality::Partial))
            {
                self.emit(CheckError::PartialCallInTotal {
                    callee: name.to_string(),
                    span,
                });
            }

            // Req 7: for `format()`, join argument labels into the result so that
            // `format("x={}", secret_val)` correctly returns `Secret<String>`.
            if name == "format" {
                let arg_label = arg_tys
                    .iter()
                    .fold(None, |acc, ty| ifc::join_opt(acc, ifc::label_of(ty)));
                return ifc::apply_label(arg_label, fn_info.ret.clone());
            }
            // L5-08: for generic functions the declared return type is a type-parameter
            // name (e.g. `T`), not a concrete type.  Return Unknown so the call site
            // unifies freely with any annotation or context type.
            if is_generic {
                return Ty::Unknown;
            }
            fn_info.ret.clone()
        } else {
            // ── Built-in enum constructors ────────────────────────────────
            // These are not in the function table but are valid expressions.
            let arg_count = arg_tys.len();
            let first_arg = arg_tys.into_iter().next().unwrap_or(Ty::Unknown);
            match name {
                "Some" => return Ty::Option(Box::new(first_arg)),
                "Ok" => return Ty::Result(Box::new(first_arg), Box::new(Ty::Unknown)),
                "Err" => return Ty::Result(Box::new(Ty::Unknown), Box::new(first_arg)),
                // Byte constructor: from_int(n: Int) -> Byte  (wrapping cast)
                "from_int" => {
                    if arg_count != 1 {
                        self.emit(CheckError::WrongArgCount {
                            name: "from_int".to_string(),
                            expected: 1,
                            found: arg_count,
                            span,
                        });
                    } else if !matches!(first_arg, Ty::Int) {
                        self.emit(CheckError::TypeMismatch {
                            expected: "Int".to_string(),
                            found: first_arg.display(),
                            span,
                        });
                    }
                    return Ty::Byte;
                }
                // Box::new(x) wraps x in a heap-allocated Box<T> (for recursive ADTs)
                "Box::new" => {
                    if arg_count != 1 {
                        self.emit(CheckError::WrongArgCount {
                            name: "Box::new".to_string(),
                            expected: 1,
                            found: arg_count,
                            span,
                        });
                    }
                    return Ty::Named("Box".to_string(), vec![first_arg]);
                }
                _ => {}
            }
            // User-defined enum tuple-variant constructor (bare or path form)
            let variant_name = if let Some((_, v)) = name.split_once("::") {
                v
            } else {
                name
            };
            if let Some(enum_ty) = self.lookup_enum_for_variant(variant_name) {
                return enum_ty;
            }
            // Not in function table — could be builtin or foreign; emit Unknown
            self.emit(CheckError::UndefinedFunction {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }

    // ── Method resolution (#43: string + collection ops) ─────────────────

    /// Resolve the return type of a method call based on the receiver type.
    ///
    /// All collection methods return `Option<T>` where there is any possibility
    /// of absence (e.g. `.get`, `.first`) — never panic on valid input.
    /// IFC labels on the receiver propagate to the result via `apply_label`.
    fn infer_method_call(
        &mut self,
        recv_ty: &Ty,
        method: &str,
        arg_tys: &[Ty],
        span: crate::mvl::parser::lexer::Span,
    ) -> Ty {
        // Validate concat(other: String) — exactly one String argument.
        // Other String methods have flexible or zero args and don't need pre-validation here.
        if matches!(recv_ty.unlabeled(), Ty::String) && method == "concat" {
            if arg_tys.len() != 1 {
                self.emit(CheckError::WrongArgCount {
                    name: "String.concat".to_string(),
                    expected: 1,
                    found: arg_tys.len(),
                    span,
                });
                return Ty::Unknown;
            }
            if !matches!(arg_tys[0].unlabeled(), Ty::String) {
                self.emit(CheckError::TypeMismatch {
                    expected: "String".to_string(),
                    found: arg_tys[0].display(),
                    span,
                });
                return Ty::Unknown;
            }
        }
        // Join receiver label with all argument labels (Req 7: result sensitivity is
        // the join of all inputs, e.g. `public_str.replace("x", secret_arg)` → Secret<String>).
        let recv_label = ifc::label_of(recv_ty);
        let arg_label = arg_tys
            .iter()
            .fold(None, |acc, ty| ifc::join_opt(acc, ifc::label_of(ty)));
        let label = ifc::join_opt(recv_label, arg_label);
        let base = recv_ty.unlabeled();
        let result = match base {
            Ty::Int => Self::int_method_ty(method),
            Ty::Byte => Self::byte_method_ty(method),
            Ty::Float => Self::float_method_ty(method),
            Ty::String => Self::string_method_ty(method, arg_tys),
            Ty::List(elem_ty) => Self::list_method_ty(elem_ty.as_ref(), method, arg_tys),
            Ty::Option(inner) => Self::option_method_ty(inner.as_ref(), method, arg_tys),
            Ty::Result(ok_ty, _) => Self::result_method_ty(ok_ty.as_ref(), method, arg_tys),
            Ty::Named(name, type_args) => match name.as_str() {
                "Map" => Self::map_method_ty(type_args, method),
                "Set" => Self::set_method_ty(type_args, method),
                _ => Ty::Unknown,
            },
            _ => Ty::Unknown,
        };
        // Only apply label when we resolved a concrete type.
        // Leaving Ty::Unknown unwrapped preserves the "Unknown = unresolved" sentinel;
        // wrapping it (e.g. Tainted<Unknown>) confuses downstream operators like `?`.
        if matches!(result, Ty::Unknown) {
            result
        } else {
            ifc::apply_label(label, result)
        }
    }

    /// Return type for methods on `Int`.
    fn int_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_float" => Ty::Float,
            "to_string" => Ty::String,
            // Arithmetic
            "abs" | "pow" | "min" | "max" | "clamp" => Ty::Int,
            // Bitwise
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => Ty::Int,
            // Predicates
            "is_positive" | "is_negative" | "is_zero" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Byte`.
    fn byte_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_int" => Ty::Int,
            "to_string" => Ty::String,
            // Bitwise (same set as Int)
            "bit_and" | "bit_or" | "bit_xor" | "bit_not" | "shift_left" | "shift_right" => Ty::Byte,
            // Arithmetic — Rust's u8 exposes these natively; the transpiler's
            // generic method-call fallthrough emits `receiver.wrapping_add(arg)`
            // which is valid Rust.  No dedicated emit arm is required.
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" => Ty::Byte,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Float`.
    fn float_method_ty(method: &str) -> Ty {
        match method {
            // Conversion
            "to_int" => Ty::Int,
            "to_string" => Ty::String,
            // Arithmetic
            "abs" | "ceil" | "floor" | "round" | "sqrt" | "min" | "max" | "clamp" | "pow" => {
                Ty::Float
            }
            // Predicates
            "is_nan" | "is_infinite" | "is_finite" | "is_positive" | "is_negative" => Ty::Bool,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Option<T>`.
    fn option_method_ty(inner: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            "is_some" | "is_none" => Ty::Bool,
            "unwrap_or" => inner.clone(),
            // map(f: fn(T) -> U) -> Option<U>
            "map" => {
                let u = if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Option(Box::new(u))
            }
            // and_then(f: fn(T) -> Option<U>) -> Option<U>
            "and_then" => {
                if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                }
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Result<T, E>`.
    fn result_method_ty(ok_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            "is_ok" | "is_err" => Ty::Bool,
            "unwrap_or" => ok_ty.clone(),
            // map(f: fn(T) -> U) -> Result<U, E>  — infer U from lambda return type
            "map" => {
                let u = if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                // We don't track E in the return type here; use Unknown for E
                Ty::Result(Box::new(u), Box::new(Ty::Unknown))
            }
            // and_then(f: fn(T) -> Result<U,E>) -> Result<U,E>
            "and_then" => {
                if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                }
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `String`.
    fn string_method_ty(method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            // Splitting: String → List<String> (never panics, always valid)
            "split" | "chars" | "lines" => Ty::List(Box::new(Ty::String)),
            // Transformations returning String
            "trim" | "trim_start" | "trim_end" | "to_upper" | "to_lower" | "replace"
            | "replace_all" | "format" => Ty::String,
            // concat(other: String) -> String — exactly one String argument required
            "concat" if arg_tys.len() == 1 && matches!(arg_tys[0], Ty::String) => Ty::String,
            "concat" => Ty::Unknown,
            // Searching: Option<Int> — returns None when not found
            "find" | "rfind" => Ty::Option(Box::new(Ty::Int)),
            // Predicates
            "contains" | "starts_with" | "ends_with" | "is_empty" => Ty::Bool,
            // Numeric
            "len" => Ty::Int,
            // Parsing
            "parse_int" => Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
            "parse_float" => Ty::Result(Box::new(Ty::Float), Box::new(Ty::String)),
            // Slicing: substring(start, end) — exclusive range → String; requires 2 Int args
            "substring"
                if arg_tys.len() == 2
                    && matches!((&arg_tys[0], &arg_tys[1]), (Ty::Int, Ty::Int)) =>
            {
                Ty::String
            }
            "substring" => Ty::Unknown,
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `List<T>`.
    fn list_method_ty(elem_ty: &Ty, method: &str, arg_tys: &[Ty]) -> Ty {
        match method {
            // map(f: fn(T) -> U) -> List<U>  — infer U from lambda return type
            "map" => {
                let u_ty = if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(u_ty))
            }
            // filter(f: fn(T) -> Bool) -> List<T>
            "filter" | "sort" | "sort_by" | "collect" | "rev" | "dedup" => {
                Ty::List(Box::new(elem_ty.clone()))
            }
            // fold(init: U, f: fn(U, T) -> U) -> U  — U inferred from init type
            "fold" => {
                if let Some(init_ty) = arg_tys.first() {
                    init_ty.clone()
                } else {
                    Ty::Unknown
                }
            }
            // reduce(f: fn(T, T) -> T) -> Option<T>  — returns None for empty list
            "reduce" => Ty::Option(Box::new(elem_ty.clone())),
            // enumerate() -> List<(Int, T)>
            "enumerate" => Ty::List(Box::new(Ty::Tuple(vec![Ty::Int, elem_ty.clone()]))),
            // zip(other: List<U>) -> List<(T, U)>
            "zip" => {
                let u_ty = if let Some(Ty::List(u)) = arg_tys.first() {
                    *u.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(Ty::Tuple(vec![elem_ty.clone(), u_ty])))
            }
            // join(sep: String) -> String  — only meaningful for List<String>
            "join" => Ty::String,
            // Numeric
            "len" => Ty::Int,
            // Predicates
            "contains" | "is_empty" | "any" | "all" => Ty::Bool,
            // Safe indexed access — Option, never panic
            "first" | "last" => Ty::Option(Box::new(elem_ty.clone())),
            "get" => Ty::Option(Box::new(elem_ty.clone())),
            // Mutations
            "push" | "extend" | "append" => Ty::Unit,
            // Flat-map
            "flat_map" => {
                let u_ty = if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    if let Ty::List(inner) = ret.as_ref() {
                        *inner.clone()
                    } else {
                        *ret.clone()
                    }
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(u_ty))
            }
            // find returns the element wrapped in Option
            "find" => Ty::Option(Box::new(elem_ty.clone())),
            // min/max — Option<T>
            "min" | "max" => Ty::Option(Box::new(elem_ty.clone())),
            // slice(start, end) — exclusive range → List<T>; requires exactly 2 Int args
            "slice"
                if arg_tys.len() == 2
                    && matches!((&arg_tys[0], &arg_tys[1]), (Ty::Int, Ty::Int)) =>
            {
                Ty::List(Box::new(elem_ty.clone()))
            }
            "slice" => Ty::Unknown,
            // take(n)/skip(n) — first/last N elements → List<T>
            "take" | "skip" => Ty::List(Box::new(elem_ty.clone())),
            // take_while(f)/skip_while(f) — List<T>
            "take_while" | "skip_while" => Ty::List(Box::new(elem_ty.clone())),
            // windows(n)/chunks(n) — List<List<T>>
            "windows" | "chunks" => Ty::List(Box::new(Ty::List(Box::new(elem_ty.clone())))),
            // flatten() — List<List<U>> → List<U>; infer U from elem_ty
            "flatten" => {
                let inner = if let Ty::List(u) = elem_ty {
                    *u.clone()
                } else {
                    Ty::Unknown
                };
                Ty::List(Box::new(inner))
            }
            // partition(f) — (List<T>, List<T>)
            "partition" => Ty::Tuple(vec![
                Ty::List(Box::new(elem_ty.clone())),
                Ty::List(Box::new(elem_ty.clone())),
            ]),
            // group_by(f: fn(T) -> K) — Map<K, List<T>>
            "group_by" => {
                let k_ty = if let Some(Ty::Fn(_, ret)) = arg_tys.first() {
                    *ret.clone()
                } else {
                    Ty::Unknown
                };
                Ty::Named(
                    "Map".into(),
                    vec![k_ty, Ty::List(Box::new(elem_ty.clone()))],
                )
            }
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Map<K, V>`.
    fn map_method_ty(type_args: &[Ty], method: &str) -> Ty {
        let (k_ty, v_ty) = match type_args {
            [k, v] => (k.clone(), v.clone()),
            [k] => (k.clone(), Ty::Unknown),
            _ => (Ty::Unknown, Ty::Unknown),
        };
        match method {
            // Safe access — Option<V>, never panic
            "get" => Ty::Option(Box::new(v_ty)),
            // Predicates
            "contains_key" | "is_empty" => Ty::Bool,
            // Numeric
            "len" => Ty::Int,
            // Mutation
            "insert" | "remove_entry" => Ty::Unit,
            // remove returns old value if present
            "remove" => Ty::Option(Box::new(v_ty)),
            // Iteration views
            "keys" => Ty::List(Box::new(k_ty)),
            "values" => Ty::List(Box::new(v_ty.clone())),
            "entries" => Ty::List(Box::new(Ty::Tuple(vec![k_ty, v_ty]))),
            _ => Ty::Unknown,
        }
    }

    /// Return type for methods on `Set<T>`.
    fn set_method_ty(type_args: &[Ty], method: &str) -> Ty {
        let t_ty = type_args.first().cloned().unwrap_or(Ty::Unknown);
        match method {
            "contains" | "is_empty" | "is_subset" | "is_superset" => Ty::Bool,
            "len" => Ty::Int,
            "insert" | "remove" => Ty::Unit,
            "iter" | "to_list" => Ty::List(Box::new(t_ty.clone())),
            "union" | "intersection" | "difference" => Ty::Named("Set".to_string(), vec![t_ty]),
            _ => Ty::Unknown,
        }
    }

    // ── Field access (#12) ────────────────────────────────────────────────

    /// Look up a field type without emitting errors.
    fn field_type(&self, ty: &Ty, field: &str) -> Option<Ty> {
        let base = ty.unlabeled();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
                    return fields
                        .iter()
                        .find(|f| f.name == field)
                        .map(|f| f.ty.clone());
                }
            }
        }
        None
    }

    /// Look up a field type, emitting errors for violations.
    fn field_type_checked(&mut self, ty: &Ty, field: &str, span: Span) -> Ty {
        let base = ty.unlabeled().clone();
        match &base {
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    match &type_info.body {
                        TypeBodyInfo::Struct(fields) => {
                            if let Some(fi) = fields.iter().find(|f| f.name == field) {
                                fi.ty.clone()
                            } else {
                                self.emit(CheckError::FieldNotFound {
                                    ty: name.clone(),
                                    field: field.to_string(),
                                    span,
                                });
                                Ty::Unknown
                            }
                        }
                        TypeBodyInfo::Enum(_) => {
                            self.emit(CheckError::FieldAccessOnEnum {
                                ty: name.clone(),
                                span,
                            });
                            Ty::Unknown
                        }
                        TypeBodyInfo::Alias(inner) => {
                            self.field_type_checked(&inner.clone(), field, span)
                        }
                    }
                } else {
                    // Unknown named type — already reported elsewhere
                    Ty::Unknown
                }
            }
            Ty::Unknown => Ty::Unknown,
            other => {
                self.emit(CheckError::FieldNotFound {
                    ty: other.display(),
                    field: field.to_string(),
                    span,
                });
                Ty::Unknown
            }
        }
    }

    // ── Struct construction (#12) ─────────────────────────────────────────

    fn check_construction(&mut self, name: &str, fields: &[(String, Expr)], span: Span) -> Ty {
        // Infer all provided field values
        let provided: Vec<(String, Ty)> = fields
            .iter()
            .map(|(fname, fexpr)| (fname.clone(), self.infer_expr(fexpr)))
            .collect();

        if let Some(type_info) = self.env.lookup_type(name).cloned() {
            match &type_info.body {
                TypeBodyInfo::Struct(declared_fields) => {
                    // Check that all declared fields are provided
                    for df in declared_fields.iter() {
                        if !provided.iter().any(|(pname, _)| pname == &df.name) {
                            self.emit(CheckError::MissingField {
                                ty: name.to_string(),
                                field: df.name.clone(),
                                span,
                            });
                        }
                    }
                    // Check no extra fields are provided
                    for (pname, pty) in &provided {
                        if let Some(df) = declared_fields.iter().find(|f| &f.name == pname) {
                            if !types_compatible(&df.ty, pty) {
                                self.emit(CheckError::TypeMismatch {
                                    expected: df.ty.display(),
                                    found: pty.display(),
                                    span,
                                });
                            }
                        } else {
                            self.emit(CheckError::UnknownField {
                                ty: name.to_string(),
                                field: pname.clone(),
                                span,
                            });
                        }
                    }
                    Ty::Named(name.to_string(), vec![])
                }
                TypeBodyInfo::Enum(_) => {
                    // Enum variant construction — name might be "EnumType::Variant"
                    // For now just return the type
                    Ty::Named(name.to_string(), vec![])
                }
                TypeBodyInfo::Alias(inner) => inner.clone(),
            }
        } else {
            // Unknown type
            self.emit(CheckError::UndefinedType {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }

    // ── Match exhaustiveness (#13) ────────────────────────────────────────

    fn infer_match_expr(&mut self, arms: &[MatchArm], scrutinee_ty: &Ty, span: Span) -> Ty {
        self.check_match_arms(arms, scrutinee_ty, span, None)
    }

    /// Check match arms for exhaustiveness and return the result type.
    fn check_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
        return_ty: Option<&Ty>,
    ) -> Ty {
        // Check each arm body
        let mut arm_tys: Vec<Ty> = Vec::new();
        for arm in arms {
            self.env.push_scope();
            self.bind_match_pattern(&arm.pattern, scrutinee_ty);
            let body_ty = match &arm.body {
                MatchBody::Expr(e) => self.infer_expr(e),
                // Use infer_block_type so the last Stmt::Expr is treated as
                // the arm's return value rather than a discarded statement.
                // This prevents false ResultIgnored errors on Ok(...)/Err(...)
                // that appear at the end of match arm blocks.
                MatchBody::Block(b) => self.infer_block_type(b, return_ty),
            };
            self.env.pop_scope();
            arm_tys.push(body_ty);
        }

        // Exhaustiveness check
        self.check_exhaustiveness(arms, scrutinee_ty, span);

        arm_tys
            .into_iter()
            .find(|t| !matches!(t, Ty::Unknown))
            .unwrap_or(Ty::Unknown)
    }

    fn check_exhaustiveness(&mut self, arms: &[MatchArm], scrutinee_ty: &Ty, span: Span) {
        let base = scrutinee_ty.unlabeled().clone();

        match &base {
            // Option<T>: must cover Some(_) and None
            Ty::Option(_) => {
                // A bare `_` or non-Option-variant ident is a wildcard → exhaustive
                if arms.iter().any(|a| is_wildcard_pattern(&a.pattern, &[])) {
                    return;
                }
                let has_some = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Some { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Some")
                });
                let has_none = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::None(_))
                        || matches!(&a.pattern, Pattern::Ident(n, _) if n == "None")
                });
                let mut missing = Vec::new();
                if !has_some {
                    missing.push("Some(_)".to_string());
                }
                if !has_none {
                    missing.push("None".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Result<T,E>: must cover Ok(_) and Err(_)
            Ty::Result(_, _) => {
                if arms.iter().any(|a| is_wildcard_pattern(&a.pattern, &[])) {
                    return;
                }
                let has_ok = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Ok { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Ok")
                });
                let has_err = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Err { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Err")
                });
                let mut missing = Vec::new();
                if !has_ok {
                    missing.push("Ok(_)".to_string());
                }
                if !has_err {
                    missing.push("Err(_)".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Named enum: collect which variants are covered
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    if let TypeBodyInfo::Enum(variants) = &type_info.body {
                        let variant_names: Vec<String> =
                            variants.iter().map(|v| v.name.clone()).collect();

                        // A wildcard is any Pattern::Wildcard OR a bare ident not in the enum's variants
                        if arms
                            .iter()
                            .any(|a| is_wildcard_pattern(&a.pattern, &variant_names))
                        {
                            return;
                        }

                        // Collect which variant names are explicitly covered
                        let covered: Vec<String> = arms
                            .iter()
                            .filter_map(|arm| covered_variant_name(&arm.pattern, &variant_names))
                            .collect();

                        let missing: Vec<String> = variant_names
                            .iter()
                            .filter(|v| !covered.contains(v))
                            .cloned()
                            .collect();
                        if !missing.is_empty() {
                            self.emit(CheckError::NonExhaustiveMatch { missing, span });
                        }
                    }
                }
                // Unknown type or non-enum → no exhaustiveness check
            }

            _ => {} // literals, bools, tuples — skip exhaustiveness
        }
    }

    // ── Pattern binding ───────────────────────────────────────────────────

    fn bind_pattern(&mut self, pattern: &Pattern, ty: &Ty, mutable: bool) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(ty.clone(), mutable));
            }
            Pattern::Wildcard(_) => {}
            Pattern::Tuple { elems, .. } => {
                if let Ty::Tuple(elem_tys) = ty.unlabeled() {
                    for (p, t) in elems.iter().zip(elem_tys.iter()) {
                        self.bind_pattern(p, t, mutable);
                    }
                } else {
                    for p in elems {
                        self.bind_pattern(p, &Ty::Unknown, mutable);
                    }
                }
            }
            Pattern::Literal(_, _) => {}
            _ => {
                // For struct/tuple-struct patterns, just bind sub-patterns as Unknown
                self.bind_sub_patterns(pattern, mutable);
            }
        }
    }

    fn bind_match_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Ty) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(scrutinee_ty.clone(), false));
            }
            Pattern::Wildcard(_) | Pattern::Literal(_, _) | Pattern::None(_) => {}
            Pattern::Some { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Option(t) => *t.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Ok { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Result(ok, _) => *ok.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Err { inner, .. } => {
                let inner_ty = match scrutinee_ty.unlabeled() {
                    Ty::Result(_, err) => *err.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::TupleStruct { fields, .. } => {
                for p in fields {
                    self.bind_match_pattern(p, &Ty::Unknown);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_match_pattern(p, &Ty::Unknown);
                }
            }
            Pattern::Tuple { elems, .. } => {
                let elem_tys = match scrutinee_ty.unlabeled() {
                    Ty::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                for (i, p) in elems.iter().enumerate() {
                    let ty = elem_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
        }
    }

    fn bind_sub_patterns(&mut self, pattern: &Pattern, mutable: bool) {
        match pattern {
            Pattern::TupleStruct { fields, .. } => {
                for p in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Some { inner, .. }
            | Pattern::Ok { inner, .. }
            | Pattern::Err { inner, .. } => {
                self.bind_pattern(inner, &Ty::Unknown, mutable);
            }
            _ => {}
        }
    }

    // ── Lambda capture immutability (ADR-0002) ────────────────────────────

    /// Verify that the lambda body does not capture mutable outer bindings.
    ///
    /// ADR-0002 prescribes "Lambdas with immutable captures only".  This helper
    /// walks `expr` collecting every `Expr::Ident` that is NOT one of the lambda
    /// parameter names; if any such captured name is bound as `mutable` in the
    /// current environment, we emit [`CheckError::CaptureMutabilityViolation`].
    ///
    /// Note: superseded by the scope-index check in `Expr::Ident` (via `lambda_scope_starts`).
    /// Retained for reference; callers should prefer the scope-based approach.
    #[allow(dead_code)]
    fn check_lambda_captures(&mut self, expr: &Expr, param_names: &[&str]) {
        let captures = collect_free_var_refs(expr, param_names);
        for (name, span) in captures {
            if let Some(info) = self.env.lookup(&name) {
                if info.mutable {
                    self.emit(CheckError::CaptureMutabilityViolation { name, span });
                }
            }
        }
    }

    // ── Reference capability checking (#22) ───────────────────────────────

    /// Verify that an argument to `channel.send()` has a sendable capability.
    ///
    /// Only `iso` and `val` may cross actor boundaries; `ref` and `tag` may not.
    /// `consume` wrapping is detected by looking for `Expr::Consume` (or equivalent).
    ///
    /// # Scope limitation
    /// Currently only checks simple identifier arguments (e.g. `channel.send(x)`).
    /// Complex expressions like `channel.send(get_payload())` or `channel.send(obj.field)`
    /// are not checked. See #73 for tracking.
    fn check_send_capability(&mut self, arg: &Expr, span: Span) {
        if let Expr::Ident(name, _) = arg {
            if let Some(info) = self.env.lookup(name).cloned() {
                match &info.capability {
                    Some(Capability::Ref) => {
                        self.emit(CheckError::CapabilityViolation {
                            param: name.clone(),
                            capability: "ref".to_string(),
                            span,
                        });
                    }
                    Some(Capability::Tag) => {
                        self.emit(CheckError::CapabilityViolation {
                            param: name.clone(),
                            capability: "tag".to_string(),
                            span,
                        });
                    }
                    // iso and val are sendable; None (default) is treated as val
                    _ => {}
                }
            }
        }
    }

    /// Phase C (#305, #363): scope-depth check for reference assignments.
    ///
    /// Emits `ReferenceOutlivesOwner` when the referent variable is defined at a
    /// deeper (shorter-lived) scope than the reference binding, or when the referent
    /// is block-local and leaves scope before the binding is made.
    ///
    /// Handles both implicit borrow (`let r: &T = x`) and explicit borrow
    /// (`let r: &T = &x`) via `referent_ident`'s `Expr::Borrow` unwrapping.
    fn check_borrow_lifetime(&mut self, pattern: &Pattern, init: &Expr) {
        let Pattern::Ident(ref_name, _) = pattern else {
            return;
        };
        let Some(owner_name) = referent_ident(init) else {
            return;
        };
        // scope_depth() returns scopes.len() (raw count); VarInfo.scope_depth is 0-based (scopes.len()-1).
        let r_depth = self.env.scope_depth().saturating_sub(1);
        let owner_too_deep = match self.env.lookup(owner_name) {
            Some(info) => info.scope_depth > r_depth,
            // Not in scope: defined inside the init block → always dangling.
            None => true,
        };
        if owner_too_deep {
            self.emit(CheckError::ReferenceOutlivesOwner {
                ref_name: ref_name.clone(),
                owner_name: owner_name.to_owned(),
                span: init.span(),
            });
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Walk `expr` and return all `(name, span)` pairs for `Expr::Ident` references
/// whose name is NOT in `param_names` (i.e. free variables / potential captures).
///
/// Nested lambdas are NOT recursed into — their params shadow outer names and
/// their own captures will be checked when that lambda is visited by the checker.
///
/// Used by `check_lambda_captures` (superseded approach — see `lambda_scope_starts`).
#[allow(dead_code)]
fn collect_free_var_refs(expr: &Expr, param_names: &[&str]) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    collect_refs_expr(expr, param_names, &mut out);
    out
}

#[allow(dead_code)]
fn collect_refs_expr(expr: &Expr, params: &[&str], out: &mut Vec<(String, Span)>) {
    match expr {
        Expr::Ident(name, span) => {
            if !params.contains(&name.as_str()) {
                out.push((name.clone(), *span));
            }
        }
        Expr::Lambda { .. } => {
            // Do NOT recurse: the nested lambda is checked independently.
        }
        Expr::Literal(..) => {}
        Expr::FieldAccess { expr: e, .. } => collect_refs_expr(e, params, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_refs_expr(receiver, params, out);
            for a in args {
                collect_refs_expr(a, params, out);
            }
        }
        Expr::FnCall { args, .. } => {
            for a in args {
                collect_refs_expr(a, params, out);
            }
        }
        Expr::Unary { expr: e, .. }
        | Expr::Propagate { expr: e, .. }
        | Expr::Move { expr: e, .. }
        | Expr::Consume { expr: e, .. }
        | Expr::Declassify { expr: e, .. }
        | Expr::Sanitize { expr: e, .. }
        | Expr::Borrow { expr: e, .. } => collect_refs_expr(e, params, out),
        Expr::Binary { left, right, .. } => {
            collect_refs_expr(left, params, out);
            collect_refs_expr(right, params, out);
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            collect_refs_expr(cond, params, out);
            collect_refs_block(then, params, out);
            if let Some(e) = else_ {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_refs_expr(scrutinee, params, out);
            for arm in arms {
                match &arm.body {
                    MatchBody::Expr(e) => collect_refs_expr(e, params, out),
                    MatchBody::Block(b) => collect_refs_block(b, params, out),
                }
            }
        }
        Expr::Block(b) => collect_refs_block(b, params, out),
        Expr::Construct { fields, .. } => {
            for (_, e) in fields {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            for e in elems {
                collect_refs_expr(e, params, out);
            }
        }
        Expr::Map { pairs, .. } => {
            for (k, v) in pairs {
                collect_refs_expr(k, params, out);
                collect_refs_expr(v, params, out);
            }
        }
    }
}

#[allow(dead_code)]
fn collect_refs_block(block: &Block, params: &[&str], out: &mut Vec<(String, Span)>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { init, .. } => collect_refs_expr(init, params, out),
            Stmt::Assign { value, .. } => collect_refs_expr(value, params, out),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    collect_refs_expr(e, params, out);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                collect_refs_expr(cond, params, out);
                collect_refs_block(then, params, out);
                match else_ {
                    Some(ElseBranch::Block(b)) => collect_refs_block(b, params, out),
                    Some(ElseBranch::If(s)) => {
                        collect_refs_block(
                            &Block {
                                stmts: vec![*s.clone()],
                                span: s.span(),
                            },
                            params,
                            out,
                        );
                    }
                    None => {}
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                collect_refs_expr(scrutinee, params, out);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => collect_refs_expr(e, params, out),
                        MatchBody::Block(b) => collect_refs_block(b, params, out),
                    }
                }
            }
            Stmt::For { iter, body, .. } => {
                collect_refs_expr(iter, params, out);
                collect_refs_block(body, params, out);
            }
            Stmt::While { cond, body, .. } => {
                collect_refs_expr(cond, params, out);
                collect_refs_block(body, params, out);
            }
            Stmt::Expr { expr, .. } => collect_refs_expr(expr, params, out),
        }
    }
}

/// Extract the "root identifier" from an expression used as a `&T` init.
///
/// For `let r: &T = x`, returns `Some("x")`.
/// For `let r: &T = { ...; x }`, returns `Some("x")` (the block's tail ident).
/// Returns `None` for complex expressions where the referent cannot be named.
///
/// Used by Phase C scope-depth checking (#305, #363).
fn referent_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        // `&x` and `&mut x` — unwrap to the inner expression.
        Expr::Borrow { expr, .. } => referent_ident(expr),
        Expr::Block(block) => block.stmts.last().and_then(|s| match s {
            Stmt::Expr { expr, .. } => referent_ident(expr),
            _ => None,
        }),
        _ => None,
    }
}

/// Check whether the tail return expression of a block flows from one of the
/// given reference-parameter names.
///
/// Returns `None` if every path through the block tail returns a value whose
/// origin is one of `ref_params`.  Returns `Some(span)` pointing at the first
/// sub-expression that is NOT traceable to a reference parameter.
///
/// Also scans all statements for early `return` expressions that don't flow
/// from a reference parameter (catches returns before the tail position).
///
/// Used by Phase C return-flow checking (#364).
fn block_return_flows_from_ref_param(block: &Block, ref_params: &HashSet<&str>) -> Option<Span> {
    // First, scan every statement for embedded early returns that don't flow
    // from a reference parameter.
    if let Some(bad) = block_early_return_violation(block, ref_params) {
        return Some(bad);
    }
    // Then check the tail expression (the implicit return value of the block).
    match block.stmts.last() {
        None => Some(block.span),
        Some(stmt) => stmt_return_flows_from_ref_param(stmt, block.span, ref_params),
    }
}

/// Check whether `stmt`, when in tail position, produces a value that flows
/// from one of the reference parameters in `ref_params`.
///
/// Returns `None` if the value flows from a reference parameter, or
/// `Some(span)` pointing at the first sub-expression that does not.
fn stmt_return_flows_from_ref_param(
    stmt: &Stmt,
    fallback_span: Span,
    ref_params: &HashSet<&str>,
) -> Option<Span> {
    match stmt {
        Stmt::Expr { expr, .. } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return {
            value: Some(expr), ..
        } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return { value: None, span } => Some(*span),
        Stmt::If {
            then, else_, span, ..
        } => {
            if let Some(bad) = block_return_flows_from_ref_param(then, ref_params) {
                return Some(bad);
            }
            match else_ {
                None => Some(*span),
                Some(ElseBranch::Block(b)) => block_return_flows_from_ref_param(b, ref_params),
                Some(ElseBranch::If(inner)) => {
                    stmt_return_flows_from_ref_param(inner, *span, ref_params)
                }
            }
        }
        Stmt::Match { arms, span, .. } => {
            if arms.is_empty() {
                return Some(*span);
            }
            check_match_arms_flow(arms, ref_params)
        }
        _ => Some(fallback_span),
    }
}

/// Check whether `expr` produces a value that flows from one of the reference
/// parameters in `ref_params`.
///
/// Returns `None` if the value flows from a reference parameter, or
/// `Some(span)` pointing at the first sub-expression that does not.
fn expr_return_flows_from_ref_param(expr: &Expr, ref_params: &HashSet<&str>) -> Option<Span> {
    match expr {
        Expr::Ident(name, _) => {
            if ref_params.contains(name.as_str()) {
                None
            } else {
                Some(expr.span())
            }
        }
        // A borrow expression `&inner` is transparent: the underlying value
        // still needs to flow from a reference parameter.
        Expr::Borrow { expr: inner, .. } => expr_return_flows_from_ref_param(inner, ref_params),
        Expr::If {
            then, else_, span, ..
        } => {
            if let Some(bad) = block_return_flows_from_ref_param(then, ref_params) {
                return Some(bad);
            }
            match else_ {
                None => Some(*span),
                Some(else_expr) => expr_return_flows_from_ref_param(else_expr, ref_params),
            }
        }
        Expr::Match { arms, span, .. } => {
            if arms.is_empty() {
                return Some(*span);
            }
            check_match_arms_flow(arms, ref_params)
        }
        Expr::Block(block) => block_return_flows_from_ref_param(block, ref_params),
        _ => Some(expr.span()),
    }
}

/// Check each arm of a match expression; return the span of the first arm
/// whose body does not flow from a reference parameter, or `None` if all
/// arms are valid.
fn check_match_arms_flow(arms: &[MatchArm], ref_params: &HashSet<&str>) -> Option<Span> {
    for arm in arms {
        let bad = match &arm.body {
            MatchBody::Expr(e) => expr_return_flows_from_ref_param(e, ref_params),
            MatchBody::Block(b) => block_return_flows_from_ref_param(b, ref_params),
        };
        if let Some(bad_span) = bad {
            return Some(bad_span);
        }
    }
    None
}

/// Walk every statement in `block` (at any depth) and return the span of the
/// first `Stmt::Return` whose value does not flow from `ref_params`, or `None`
/// if every explicit return is valid.
///
/// This catches early `return` statements that appear before the tail position.
fn block_early_return_violation(block: &Block, ref_params: &HashSet<&str>) -> Option<Span> {
    for stmt in &block.stmts {
        if let Some(bad) = stmt_early_return_violation(stmt, ref_params) {
            return Some(bad);
        }
    }
    None
}

fn stmt_early_return_violation(stmt: &Stmt, ref_params: &HashSet<&str>) -> Option<Span> {
    match stmt {
        Stmt::Return {
            value: Some(expr), ..
        } => expr_return_flows_from_ref_param(expr, ref_params),
        Stmt::Return { value: None, span } => Some(*span),
        Stmt::If { then, else_, .. } => {
            block_early_return_violation(then, ref_params).or_else(|| match else_ {
                None => None,
                Some(ElseBranch::Block(b)) => block_early_return_violation(b, ref_params),
                Some(ElseBranch::If(inner)) => stmt_early_return_violation(inner, ref_params),
            })
        }
        Stmt::Match { arms, .. } => {
            for arm in arms {
                if let MatchBody::Block(b) = &arm.body {
                    if let Some(bad) = block_early_return_violation(b, ref_params) {
                        return Some(bad);
                    }
                }
            }
            None
        }
        Stmt::Expr { expr, .. } => expr_early_return_violation(expr, ref_params),
        _ => None,
    }
}

fn expr_early_return_violation(expr: &Expr, ref_params: &HashSet<&str>) -> Option<Span> {
    match expr {
        Expr::If { then, else_, .. } => {
            block_early_return_violation(then, ref_params).or_else(|| match else_ {
                None => None,
                Some(e) => expr_early_return_violation(e, ref_params),
            })
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                if let MatchBody::Block(b) = &arm.body {
                    if let Some(bad) = block_early_return_violation(b, ref_params) {
                        return Some(bad);
                    }
                }
            }
            None
        }
        Expr::Block(b) => block_early_return_violation(b, ref_params),
        _ => None,
    }
}

/// True if `pattern` acts as a catch-all / wildcard in the context of an enum
/// whose variants are listed in `variant_names`.
///
/// - `Pattern::Wildcard` is always a wildcard.
/// - `Pattern::Ident(name)` is a wildcard when `name` is NOT a known variant
///   (it's a variable binding, not a variant tag).
fn is_wildcard_pattern(pattern: &Pattern, variant_names: &[String]) -> bool {
    match pattern {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => !variant_names.contains(name),
        _ => false,
    }
}

/// Extract the variant name that a pattern explicitly covers, given the set of
/// known variant names.  Returns `None` for non-variant or wildcard patterns.
fn covered_variant_name(pattern: &Pattern, variant_names: &[String]) -> Option<String> {
    match pattern {
        Pattern::TupleStruct { name, .. } | Pattern::Struct { name, .. } => Some(name.clone()),
        // A bare ident that IS a known variant name counts as that variant
        Pattern::Ident(name, _) if variant_names.contains(name) => Some(name.clone()),
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

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

    // ── Requirement 2 / Scenario: Ownership / use-after-move (#15) ────────

    #[test]
    fn use_after_move_rejected() {
        // move(x) is the MVL syntax for explicit move
        let src = "fn f() -> Int { let x: Int = 1; let y: Int = move(x); x }";
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
        let src = "fn f() -> Unit { let mut x: Int = 1; x = true; }";
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
        let src = r#"fn f() -> Unit ! Foo { println("hi"); }"#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Foo")),
            "expected InvalidEffectName for unknown effect 'Foo', got: {errors:?}"
        );
    }

    #[test]
    fn valid_effect_names_accepted() {
        let src = r#"fn f() -> Unit ! Console, Net, DB, Terminal { println("hi"); }"#;
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected no InvalidEffectName for valid effects, got: {errors:?}"
        );
    }

    #[test]
    fn terminal_effect_name_accepted() {
        // Terminal is a distinct effect from Console — raw terminal control (cursor,
        // colors, single keypress) vs line-oriented I/O. See std.tui / #174.
        let src = r#"fn f() -> Unit ! Terminal { }"#;
        let errors = errors_for(src);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, CheckError::InvalidEffectName { .. })),
            "expected Terminal to be a valid effect name, got: {errors:?}"
        );
    }

    // ── Parametrized effects (002-effect-system Req 3 / issue #290) ──────

    #[test]
    fn parametrized_effect_parses_and_is_accepted() {
        let src = r#"fn f() -> Unit ! FileRead("/etc/app/") { }"#;
        let errors = errors_for(src);
        assert!(
            errors.is_empty(),
            "expected no errors for parametrized effect, got: {errors:?}"
        );
    }

    #[test]
    fn parametrized_effect_invalid_name_rejected() {
        let src = r#"fn f() -> Unit ! Bogus("/path") { }"#;
        let errors = errors_for(src);
        assert!(
            errors.iter().any(
                |e| matches!(e, CheckError::InvalidEffectName { name, .. } if name == "Bogus")
            ),
            "expected InvalidEffectName for parametrized unknown effect, got: {errors:?}"
        );
    }

    #[test]
    fn general_caller_covers_parametrized_callee() {
        // Unparametrized FileRead (wildcard) covers FileRead("/etc/").
        let src = r#"
            extern "kernel" {
                fn read_config() -> Unit ! FileRead("/etc/")
            }
            fn caller() -> Unit ! FileRead {
                read_config()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(|e| matches!(
                e,
                CheckError::UndeclaredEffect { .. } | CheckError::MissingEffect { .. }
            )),
            "general FileRead should cover FileRead(\"/etc/\"), got: {errors:?}"
        );
    }

    #[test]
    fn prefix_caller_covers_more_specific_parametrized_callee() {
        // FileRead("/etc/") covers FileRead("/etc/app/config.toml") via prefix match.
        let src = r#"
            extern "kernel" {
                fn read_file() -> Unit ! FileRead("/etc/app/config.toml")
            }
            fn caller() -> Unit ! FileRead("/etc/") {
                read_file()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            !errors.iter().any(|e| matches!(
                e,
                CheckError::MissingEffect { .. } | CheckError::UndeclaredEffect { .. }
            )),
            "FileRead(\"/etc/\") should cover FileRead(\"/etc/app/config.toml\"), got: {errors:?}"
        );
    }

    #[test]
    fn specific_caller_does_not_cover_different_path_callee() {
        // FileRead("/data/") must NOT satisfy FileRead("/etc/").
        let src = r#"
            extern "kernel" {
                fn read_etc() -> Unit ! FileRead("/etc/")
            }
            fn caller() -> Unit ! FileRead("/data/") {
                read_etc()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingEffect { .. })),
            "FileRead(\"/data/\") should NOT cover FileRead(\"/etc/\"), got: {errors:?}"
        );
    }

    #[test]
    fn specific_caller_does_not_cover_general_callee() {
        // FileRead("/etc/") must NOT satisfy unparametrized FileRead.
        let src = r#"
            extern "kernel" {
                fn read_any() -> Unit ! FileRead
            }
            fn caller() -> Unit ! FileRead("/etc/") {
                read_any()
            }
        "#;
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingEffect { .. })),
            "FileRead(\"/etc/\") should NOT cover general FileRead, got: {errors:?}"
        );
    }

    // ── Lambda capture immutability (ADR-0002) ────────────────────────────
    //
    // Lambda surface syntax is not yet parsed (parser does not handle `|p| expr`).
    // The capture-check logic is exercised via direct AST construction below.

    #[test]
    fn lambda_mutable_capture_rejected_via_ast() {
        use crate::mvl::parser::ast::{Block, Decl, Expr, FnDecl, Param, Program, TypeExpr};
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
        //    let mut x = 1;
        //    let g = |y: Int| -> Int { x };
        // }
        let lambda = Expr::Lambda {
            params: vec![Param {
                name: "y".into(),
                ty: int_ty.clone(),
                mutable: false,
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty.clone())),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                totality: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            mutable: true,
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: None,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            mutable: false,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: None,
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
        use crate::mvl::parser::ast::{Block, Decl, Expr, FnDecl, Param, Program, TypeExpr};
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
                mutable: false,
                capability: None,
                refinement: None,
                span: dummy_span,
            }],
            ret_type: Some(Box::new(int_ty)),
            body: Box::new(Expr::Ident("x".into(), dummy_span)),
            span: dummy_span,
        };

        let prog = Program {
            span: dummy_span,
            declarations: vec![Decl::Fn(FnDecl {
                visible: false,
                is_test: false,
                totality: None,
                name: "f".into(),
                type_params: vec![],
                params: vec![],
                return_type: Box::new(unit_ty),
                return_refinement: None,
                effects: vec![],
                constraints: vec![],
                body: Block {
                    stmts: vec![
                        Stmt::Let {
                            mutable: false, // immutable
                            pattern: Pattern::Ident("x".into(), dummy_span),
                            ty: None,
                            init: Expr::Literal(Literal::Integer(1), dummy_span),
                            span: dummy_span,
                        },
                        Stmt::Let {
                            mutable: false,
                            pattern: Pattern::Ident("g".into(), dummy_span),
                            ty: None,
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
}
