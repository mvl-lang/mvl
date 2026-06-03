// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Monomorphization pass (ADR-0034).
//!
//! Produces a [`MonoProgram`] containing a concrete [`MonoFn`] for every
//! reachable generic function instantiation.  Runs after type checking and
//! before analysis passes.
//!
//! # Pipeline position
//!
//! ```text
//! parser → resolver → checker → monomorphize → [IFC, refinements, …] → backends
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! let all_fns = collect_fns([&prelude_a, &prelude_b, &prog]);
//! let mono = monomorphize(&prog, &all_fns, &check_result.expr_types);
//! ```
//!
//! # Scope
//!
//! - Entry points: `fn main` and top-level `pub fn` without type parameters.
//! - Call following: top-level `FnCall` expressions; method calls are not
//!   currently traced into generic dispatch (future work).
//! - Builtin functions are excluded — the runtime handles them directly.
//! - Impl-block methods are not seeded as entry points but are reachable when
//!   called from a seeded entry point.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{
    Block, Decl, ElseBranch, Expr, FnDecl, GenericParam, MatchBody, Param, Program, Stmt, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

// ── Public types ──────────────────────────────────────────────────────────────

/// Type-parameter substitution map: `"T" → Int`, `"U" → String`.
pub type TypeSubst = HashMap<String, TypeExpr>;

/// A concrete (non-generic) function copy produced by monomorphization.
#[derive(Debug, Clone)]
pub struct MonoFn {
    /// Mangled symbol, e.g. `map_Int_String` for `map[T=Int, U=String]`.
    pub mangled_name: String,
    /// Original generic function name (`"map"`).
    pub original_name: String,
    /// The substitution applied: `{ "T" → Int, "U" → String }`.
    pub type_subs: TypeSubst,
    /// Concrete `FnDecl` — `type_params` is empty; parameter and return types
    /// are fully resolved.
    pub decl: FnDecl,
}

/// Output of the monomorphization pass.
#[derive(Debug, Clone, Default)]
pub struct MonoProgram {
    /// Concrete function copies for every reachable instantiation.
    pub fns: Vec<MonoFn>,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the monomorphization pass over `prog`.
///
/// `all_fns` must contain every function declaration reachable from `prog`
/// (user program + preludes).  Build it with [`collect_fns`].
///
/// `expr_types` is `CheckResult::expr_types` — the checker's inferred type
/// map used to determine concrete type arguments at generic call sites.
pub fn monomorphize(
    prog: &Program,
    all_fns: &HashMap<String, FnDecl>,
    expr_types: &HashMap<Span, Ty>,
) -> MonoProgram {
    let mut m = Monomorphizer::new(all_fns, expr_types);
    m.seed(prog);
    m.drain();
    MonoProgram { fns: m.result }
}

/// Collect all top-level function declarations from a list of programs.
///
/// When multiple programs define the same name, the last one wins (user
/// program should come last to shadow prelude definitions).
pub fn collect_fns<'a>(programs: impl IntoIterator<Item = &'a Program>) -> HashMap<String, FnDecl> {
    let mut map: HashMap<String, FnDecl> = HashMap::new();
    for prog in programs {
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                map.insert(fd.name.clone(), fd.clone());
            }
        }
    }
    map
}

// ── Mangling ──────────────────────────────────────────────────────────────────

/// Produce a mangled name for a function given its type-parameter substitution.
///
/// Examples:
/// - `identity` with `{ T → Int }` → `"identity_Int"`
/// - `map` with `{ T → Int, U → String }` → `"map_Int_String"`
/// - Non-generic `add` with `{}` → `"add"`
pub fn mangle(name: &str, type_params: &[GenericParam], subs: &TypeSubst) -> String {
    let parts: Vec<String> = type_params
        .iter()
        .filter_map(|tp| subs.get(tp.name()).map(type_name))
        .collect();
    if parts.is_empty() {
        name.to_string()
    } else {
        format!("{}_{}", name, parts.join("_"))
    }
}

/// Human-readable name for a `TypeExpr`, used in mangled symbols.
pub fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Base { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!(
                    "{}_{}",
                    name,
                    args.iter().map(type_name).collect::<Vec<_>>().join("_")
                )
            }
        }
        TypeExpr::Option { inner, .. } => format!("Option_{}", type_name(inner)),
        TypeExpr::Result { ok, err, .. } => {
            format!("Result_{}_{}", type_name(ok), type_name(err))
        }
        TypeExpr::Ref { inner, .. } => format!("Ref_{}", type_name(inner)),
        TypeExpr::Labeled { inner, .. } => type_name(inner),
        TypeExpr::Refined { inner, .. } => type_name(inner),
        TypeExpr::Fn { .. } => "Fn".into(),
        TypeExpr::Tuple { elems, .. } => {
            format!(
                "Tuple_{}",
                elems.iter().map(type_name).collect::<Vec<_>>().join("_")
            )
        }
        TypeExpr::IntConst { value, .. } => value.to_string(),
        TypeExpr::Session { .. } => "Session".into(),
    }
}

// ── Type substitution ─────────────────────────────────────────────────────────

/// Recursively substitute type parameters in `ty` using `subs`.
pub fn substitute_type(ty: &TypeExpr, subs: &TypeSubst) -> TypeExpr {
    match ty {
        TypeExpr::Base { name, args, span } => {
            if args.is_empty() {
                if let Some(concrete) = subs.get(name.as_str()) {
                    return concrete.clone();
                }
            }
            TypeExpr::Base {
                name: name.clone(),
                args: args.iter().map(|a| substitute_type(a, subs)).collect(),
                span: *span,
            }
        }
        TypeExpr::Option { inner, span } => TypeExpr::Option {
            inner: Box::new(substitute_type(inner, subs)),
            span: *span,
        },
        TypeExpr::Result { ok, err, span } => TypeExpr::Result {
            ok: Box::new(substitute_type(ok, subs)),
            err: Box::new(substitute_type(err, subs)),
            span: *span,
        },
        TypeExpr::Ref {
            mutable,
            inner,
            span,
        } => TypeExpr::Ref {
            mutable: *mutable,
            inner: Box::new(substitute_type(inner, subs)),
            span: *span,
        },
        TypeExpr::Labeled { label, inner, span } => TypeExpr::Labeled {
            label: label.clone(),
            inner: Box::new(substitute_type(inner, subs)),
            span: *span,
        },
        TypeExpr::Refined { inner, pred, span } => TypeExpr::Refined {
            inner: Box::new(substitute_type(inner, subs)),
            pred: pred.clone(),
            span: *span,
        },
        TypeExpr::Fn {
            params,
            ret,
            effects,
            span,
        } => TypeExpr::Fn {
            params: params.iter().map(|p| substitute_type(p, subs)).collect(),
            ret: Box::new(substitute_type(ret, subs)),
            effects: effects.clone(),
            span: *span,
        },
        TypeExpr::Tuple { elems, span } => TypeExpr::Tuple {
            elems: elems.iter().map(|e| substitute_type(e, subs)).collect(),
            span: *span,
        },
        TypeExpr::IntConst { .. } | TypeExpr::Session { .. } => ty.clone(),
    }
}

/// Produce a concrete `FnDecl` by applying `subs` to all parameter and return types.
///
/// The resulting decl has `type_params` cleared and `name` set to `new_name`.
/// The body is preserved as-is — body-level type annotations are not substituted
/// in this pass (future work when body analysis consumes `MonoProgram`).
fn apply_subs(fd: &FnDecl, subs: &TypeSubst, new_name: &str) -> FnDecl {
    FnDecl {
        name: new_name.to_string(),
        type_params: vec![],
        params: fd
            .params
            .iter()
            .map(|p| Param {
                ty: substitute_type(&p.ty, subs),
                ..p.clone()
            })
            .collect(),
        return_type: Box::new(substitute_type(&fd.return_type, subs)),
        ..fd.clone()
    }
}

// ── Type unification ──────────────────────────────────────────────────────────

/// Infer type-parameter substitutions by matching `pattern` (a parameter's
/// declared `TypeExpr`) against `concrete` (the checker's resolved `Ty` for
/// the call-site argument).
///
/// Writes to `subs`; existing entries are not overwritten (first match wins).
pub fn unify(
    pattern: &TypeExpr,
    concrete: &Ty,
    type_params: &[GenericParam],
    subs: &mut TypeSubst,
) {
    match pattern {
        TypeExpr::Base { name, args, .. } if args.is_empty() => {
            if type_params.iter().any(|tp| tp.name() == name.as_str()) {
                subs.entry(name.clone())
                    .or_insert_with(|| ty_to_type_expr(concrete));
            }
        }
        TypeExpr::Base { args, .. } => {
            for (pat, ty) in args.iter().zip(ty_inner_args(concrete).iter()) {
                unify(pat, ty, type_params, subs);
            }
        }
        TypeExpr::Option { inner, .. } => {
            if let Ty::Option(ty) = concrete {
                unify(inner, ty, type_params, subs);
            }
        }
        TypeExpr::Result { ok, err, .. } => {
            if let Ty::Result(ok_ty, err_ty) = concrete {
                unify(ok, ok_ty, type_params, subs);
                unify(err, err_ty, type_params, subs);
            }
        }
        TypeExpr::Ref { inner, .. } => {
            if let Ty::Ref(_, ty) = concrete {
                unify(inner, ty, type_params, subs);
            }
        }
        TypeExpr::Labeled { inner, .. } => {
            if let Ty::Labeled(_, ty) = concrete {
                unify(inner, ty, type_params, subs);
            }
        }
        TypeExpr::Refined { inner, .. } => unify(inner, concrete, type_params, subs),
        TypeExpr::Fn { params, ret, .. } => {
            if let Ty::Fn(param_tys, ret_ty, _, _) = concrete {
                for (p, t) in params.iter().zip(param_tys.iter()) {
                    unify(p, t, type_params, subs);
                }
                unify(ret, ret_ty, type_params, subs);
            }
        }
        TypeExpr::Tuple { elems, .. } => {
            if let Ty::Tuple(tys) = concrete {
                for (e, t) in elems.iter().zip(tys.iter()) {
                    unify(e, t, type_params, subs);
                }
            }
        }
        TypeExpr::IntConst { .. } | TypeExpr::Session { .. } => {}
    }
}

/// Extract the inner type arguments of a `Ty` for generic pattern matching.
fn ty_inner_args(ty: &Ty) -> Vec<Ty> {
    match ty {
        Ty::Named(_, args) => args.clone(),
        Ty::List(inner) => vec![(**inner).clone()],
        Ty::Map(k, v) => vec![(**k).clone(), (**v).clone()],
        Ty::Set(inner) => vec![(**inner).clone()],
        Ty::Array(inner, _) => vec![(**inner).clone()],
        _ => vec![],
    }
}

/// Convert a resolved `Ty` back to a `TypeExpr` for use in substitution maps.
///
/// The resulting `TypeExpr` uses `Span::default()` for all spans — it is a
/// synthetic node, not from source.
pub fn ty_to_type_expr(ty: &Ty) -> TypeExpr {
    let sp = Span::default();
    match ty {
        Ty::Int => TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: sp,
        },
        Ty::Float => TypeExpr::Base {
            name: "Float".into(),
            args: vec![],
            span: sp,
        },
        Ty::String => TypeExpr::Base {
            name: "String".into(),
            args: vec![],
            span: sp,
        },
        Ty::Bool => TypeExpr::Base {
            name: "Bool".into(),
            args: vec![],
            span: sp,
        },
        Ty::Char => TypeExpr::Base {
            name: "Char".into(),
            args: vec![],
            span: sp,
        },
        Ty::Byte => TypeExpr::Base {
            name: "Byte".into(),
            args: vec![],
            span: sp,
        },
        Ty::UByte => TypeExpr::Base {
            name: "UByte".into(),
            args: vec![],
            span: sp,
        },
        Ty::UInt => TypeExpr::Base {
            name: "UInt".into(),
            args: vec![],
            span: sp,
        },
        Ty::Unit => TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: sp,
        },
        Ty::Never => TypeExpr::Base {
            name: "Never".into(),
            args: vec![],
            span: sp,
        },
        Ty::Unknown => TypeExpr::Base {
            name: "Unknown".into(),
            args: vec![],
            span: sp,
        },
        Ty::Named(name, args) => TypeExpr::Base {
            name: name.clone(),
            args: args.iter().map(ty_to_type_expr).collect(),
            span: sp,
        },
        Ty::Option(inner) => TypeExpr::Option {
            inner: Box::new(ty_to_type_expr(inner)),
            span: sp,
        },
        Ty::Result(ok, err) => TypeExpr::Result {
            ok: Box::new(ty_to_type_expr(ok)),
            err: Box::new(ty_to_type_expr(err)),
            span: sp,
        },
        Ty::Ref(mutable, inner) => TypeExpr::Ref {
            mutable: *mutable,
            inner: Box::new(ty_to_type_expr(inner)),
            span: sp,
        },
        Ty::Fn(params, ret, effects, _) => TypeExpr::Fn {
            params: params.iter().map(ty_to_type_expr).collect(),
            ret: Box::new(ty_to_type_expr(ret)),
            effects: effects.clone(),
            span: sp,
        },
        Ty::Tuple(elems) => TypeExpr::Tuple {
            elems: elems.iter().map(ty_to_type_expr).collect(),
            span: sp,
        },
        Ty::List(inner) => TypeExpr::Base {
            name: "List".into(),
            args: vec![ty_to_type_expr(inner)],
            span: sp,
        },
        Ty::Array(inner, size) => TypeExpr::Base {
            name: "Array".into(),
            args: vec![
                ty_to_type_expr(inner),
                TypeExpr::IntConst {
                    value: *size as i64,
                    span: sp,
                },
            ],
            span: sp,
        },
        Ty::Map(k, v) => TypeExpr::Base {
            name: "Map".into(),
            args: vec![ty_to_type_expr(k), ty_to_type_expr(v)],
            span: sp,
        },
        Ty::Set(inner) => TypeExpr::Base {
            name: "Set".into(),
            args: vec![ty_to_type_expr(inner)],
            span: sp,
        },
        Ty::Refined(inner, _) => ty_to_type_expr(inner),
        Ty::Labeled(label, inner) => TypeExpr::Labeled {
            label: label.clone(),
            inner: Box::new(ty_to_type_expr(inner)),
            span: sp,
        },
        Ty::Session(_) => TypeExpr::Base {
            name: "Session".into(),
            args: vec![],
            span: sp,
        },
    }
}

// ── Monomorphizer ─────────────────────────────────────────────────────────────

struct Monomorphizer<'a> {
    all_fns: &'a HashMap<String, FnDecl>,
    expr_types: &'a HashMap<Span, Ty>,
    emitted: HashSet<String>,
    queue: VecDeque<(String, FnDecl, TypeSubst)>,
    result: Vec<MonoFn>,
}

impl<'a> Monomorphizer<'a> {
    fn new(all_fns: &'a HashMap<String, FnDecl>, expr_types: &'a HashMap<Span, Ty>) -> Self {
        Self {
            all_fns,
            expr_types,
            emitted: HashSet::new(),
            queue: VecDeque::new(),
            result: Vec::new(),
        }
    }

    fn seed(&mut self, prog: &Program) {
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if fd.type_params.is_empty() {
                    self.enqueue(fd.name.clone(), fd.clone(), TypeSubst::new());
                }
            }
        }
    }

    fn enqueue(&mut self, mangled: String, fd: FnDecl, subs: TypeSubst) {
        if self.emitted.insert(mangled.clone()) {
            self.queue.push_back((mangled, fd, subs));
        }
    }

    fn drain(&mut self) {
        while let Some((mangled, fd, subs)) = self.queue.pop_front() {
            self.process(mangled, fd, subs);
        }
    }

    fn process(&mut self, mangled_name: String, fd: FnDecl, subs: TypeSubst) {
        let concrete_decl = apply_subs(&fd, &subs, &mangled_name);

        for site in &collect_calls(&fd.body) {
            self.visit_call(site);
        }

        self.result.push(MonoFn {
            mangled_name,
            original_name: fd.name.clone(),
            type_subs: subs,
            decl: concrete_decl,
        });
    }

    fn visit_call(&mut self, site: &CallSite) {
        let Some(callee) = self.all_fns.get(&site.callee_name) else {
            return; // extern or unknown — skip
        };
        if callee.is_builtin {
            return; // runtime-provided; no body to monomorphize
        }

        let call_subs = self.make_subs(callee, site);
        let mangled = mangle(&callee.name, &callee.type_params, &call_subs);
        let fd = callee.clone();
        self.enqueue(mangled, fd, call_subs);
    }

    fn make_subs(&self, callee: &FnDecl, site: &CallSite) -> TypeSubst {
        // Prefer explicit type args if fully specified: `f[Int, String](…)`
        if !site.explicit_type_args.is_empty()
            && site.explicit_type_args.len() == callee.type_params.len()
        {
            return callee
                .type_params
                .iter()
                .map(|tp| tp.name().to_string())
                .zip(site.explicit_type_args.iter().cloned())
                .collect();
        }

        // Infer from argument types recorded by the checker
        let mut subs = TypeSubst::new();
        for (param, span) in callee.params.iter().zip(site.arg_spans.iter()) {
            if let Some(ty) = self.expr_types.get(span) {
                unify(&param.ty, ty, &callee.type_params, &mut subs);
            }
        }
        subs
    }
}

// ── Call collection ───────────────────────────────────────────────────────────

struct CallSite {
    callee_name: String,
    arg_spans: Vec<Span>,
    explicit_type_args: Vec<TypeExpr>,
}

fn collect_calls(block: &Block) -> Vec<CallSite> {
    let mut c = CallCollector { sites: Vec::new() };
    c.block(block);
    c.sites
}

struct CallCollector {
    sites: Vec<CallSite>,
}

impl CallCollector {
    fn block(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { init, .. } => self.expr(init),
            Stmt::Assign { value, .. } => self.expr(value),
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Stmt::If {
                cond, then, else_, ..
            } => {
                self.expr(cond);
                self.block(then);
                if let Some(branch) = else_ {
                    match branch {
                        ElseBranch::Block(b) => self.block(b),
                        ElseBranch::If(s) => self.stmt(s),
                    }
                }
            }
            Stmt::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                }
            }
            Stmt::For { iter, body, .. } => {
                self.expr(iter);
                self.block(body);
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Expr { expr, .. } => self.expr(expr),
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::FnCall {
                name,
                type_args,
                args,
                ..
            } => {
                self.sites.push(CallSite {
                    callee_name: name.clone(),
                    arg_spans: args.iter().map(|a| a.span()).collect(),
                    explicit_type_args: type_args.clone(),
                });
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Literal(..) | Expr::Ident(..) => {}
            Expr::FieldAccess { expr, .. } => self.expr(expr),
            Expr::MethodCall { receiver, args, .. } => {
                self.expr(receiver);
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Unary { expr, .. } => self.expr(expr),
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.expr(cond);
                self.block(then);
                if let Some(e) = else_ {
                    self.expr(e);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.expr(e),
                        MatchBody::Block(b) => self.block(b),
                    }
                }
            }
            Expr::Lambda { body, .. } => self.expr(body),
            Expr::Block(b) => self.block(b),
            Expr::Propagate { expr, .. }
            | Expr::Consume { expr, .. }
            | Expr::Relabel { expr, .. }
            | Expr::Borrow { expr, .. } => self.expr(expr),
            Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
                for (_, e) in fields {
                    self.expr(e);
                }
            }
            Expr::List { elems, .. } | Expr::Set { elems, .. } => {
                for e in elems {
                    self.expr(e);
                }
            }
            Expr::Map { pairs, .. } => {
                for (k, v) in pairs {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::Select { arms, .. } => {
                for arm in arms {
                    self.expr(&arm.expr);
                    self.block(&arm.body);
                }
            }
            Expr::Quantifier(..) => {}
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::{CmpOp, RefExpr};

    fn sp() -> Span {
        Span::default()
    }

    fn base(name: &str) -> TypeExpr {
        TypeExpr::Base {
            name: name.into(),
            args: vec![],
            span: sp(),
        }
    }

    fn list_of(inner: TypeExpr) -> TypeExpr {
        TypeExpr::Base {
            name: "List".into(),
            args: vec![inner],
            span: sp(),
        }
    }

    fn type_param(name: &str) -> GenericParam {
        GenericParam::Type(name.into())
    }

    // ── substitute_type ──────────────────────────────────────────────────────

    #[test]
    fn substitute_direct_type_param() {
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Int"));
        assert_eq!(substitute_type(&base("T"), &subs), base("Int"));
    }

    #[test]
    fn substitute_non_param_unchanged() {
        let subs = TypeSubst::new();
        assert_eq!(substitute_type(&base("Int"), &subs), base("Int"));
    }

    #[test]
    fn substitute_generic_arg() {
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Bool"));
        assert_eq!(
            substitute_type(&list_of(base("T")), &subs),
            list_of(base("Bool"))
        );
    }

    #[test]
    fn substitute_option_inner() {
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Float"));
        let option_t = TypeExpr::Option {
            inner: Box::new(base("T")),
            span: sp(),
        };
        let option_float = TypeExpr::Option {
            inner: Box::new(base("Float")),
            span: sp(),
        };
        assert_eq!(substitute_type(&option_t, &subs), option_float);
    }

    #[test]
    fn substitute_fn_type() {
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Int"));
        subs.insert("U".into(), base("String"));
        let fn_ty = TypeExpr::Fn {
            params: vec![base("T")],
            ret: Box::new(base("U")),
            effects: vec![],
            span: sp(),
        };
        let result = substitute_type(&fn_ty, &subs);
        if let TypeExpr::Fn { params, ret, .. } = result {
            assert_eq!(params[0], base("Int"));
            assert_eq!(*ret, base("String"));
        } else {
            panic!("expected Fn");
        }
    }

    // ── mangle ───────────────────────────────────────────────────────────────

    #[test]
    fn mangle_non_generic() {
        let subs = TypeSubst::new();
        assert_eq!(mangle("add", &[], &subs), "add");
    }

    #[test]
    fn mangle_single_type_param() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Int"));
        assert_eq!(mangle("identity", &params, &subs), "identity_Int");
    }

    #[test]
    fn mangle_two_type_params() {
        let params = vec![type_param("T"), type_param("U")];
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), base("Int"));
        subs.insert("U".into(), base("String"));
        assert_eq!(mangle("map", &params, &subs), "map_Int_String");
    }

    #[test]
    fn mangle_list_param() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        subs.insert("T".into(), list_of(base("Bool")));
        assert_eq!(mangle("wrap", &params, &subs), "wrap_List_Bool");
    }

    // ── unify ────────────────────────────────────────────────────────────────

    #[test]
    fn unify_direct_param() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        unify(&base("T"), &Ty::Int, &params, &mut subs);
        assert_eq!(subs.get("T"), Some(&base("Int")));
    }

    #[test]
    fn unify_list_of_param() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        unify(
            &list_of(base("T")),
            &Ty::List(Box::new(Ty::Bool)),
            &params,
            &mut subs,
        );
        assert_eq!(subs.get("T"), Some(&base("Bool")));
    }

    #[test]
    fn unify_two_params_from_fn_type() {
        let params = vec![type_param("T"), type_param("U")];
        let mut subs = TypeSubst::new();
        let pattern = TypeExpr::Fn {
            params: vec![base("T")],
            ret: Box::new(base("U")),
            effects: vec![],
            span: sp(),
        };
        let concrete = Ty::Fn(vec![Ty::Int], Box::new(Ty::String), vec![], None);
        unify(&pattern, &concrete, &params, &mut subs);
        assert_eq!(subs.get("T"), Some(&base("Int")));
        assert_eq!(subs.get("U"), Some(&base("String")));
    }

    #[test]
    fn unify_non_param_no_op() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        unify(&base("Int"), &Ty::Int, &params, &mut subs);
        assert!(subs.is_empty());
    }

    #[test]
    fn unify_first_match_wins() {
        let params = vec![type_param("T")];
        let mut subs = TypeSubst::new();
        // First call binds T → Int
        unify(&base("T"), &Ty::Int, &params, &mut subs);
        // Second call with different type should not overwrite
        unify(&base("T"), &Ty::Bool, &params, &mut subs);
        assert_eq!(subs.get("T"), Some(&base("Int")));
    }

    // ── ty_to_type_expr ──────────────────────────────────────────────────────

    #[test]
    fn ty_to_type_expr_primitives() {
        assert_eq!(ty_to_type_expr(&Ty::Int), base("Int"));
        assert_eq!(ty_to_type_expr(&Ty::Bool), base("Bool"));
        assert_eq!(ty_to_type_expr(&Ty::String), base("String"));
        assert_eq!(ty_to_type_expr(&Ty::Unit), base("Unit"));
    }

    #[test]
    fn ty_to_type_expr_list() {
        let result = ty_to_type_expr(&Ty::List(Box::new(Ty::Int)));
        assert_eq!(result, list_of(base("Int")));
    }

    #[test]
    fn ty_to_type_expr_option() {
        let result = ty_to_type_expr(&Ty::Option(Box::new(Ty::Bool)));
        assert_eq!(
            result,
            TypeExpr::Option {
                inner: Box::new(base("Bool")),
                span: sp()
            }
        );
    }

    #[test]
    fn ty_to_type_expr_refined_strips_predicate() {
        let pred = RefExpr::Compare {
            op: CmpOp::Gt,
            left: Box::new(RefExpr::Ident {
                name: "self".into(),
                span: sp(),
            }),
            right: Box::new(RefExpr::Integer {
                value: 0,
                span: sp(),
            }),
            span: sp(),
        };
        let result = ty_to_type_expr(&Ty::Refined(Box::new(Ty::Int), Box::new(pred)));
        assert_eq!(result, base("Int"));
    }

    // ── collect_fns ──────────────────────────────────────────────────────────

    #[test]
    fn collect_fns_empty() {
        let prog = Program {
            declarations: vec![],
            span: sp(),
        };
        let fns = collect_fns([&prog]);
        assert!(fns.is_empty());
    }

    // ── monomorphize (integration) ───────────────────────────────────────────

    fn parse(src: &str) -> Program {
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    #[test]
    fn monomorphize_identity_produces_two_instantiations() {
        let prog = parse(
            r#"
fn identity[T](x: T) -> T { x }
fn main() -> Unit {
    let n: Int    = identity(42);
    let s: String = identity("hello");
}
"#,
        );
        let check = crate::mvl::checker::check(&prog);
        let all_fns = collect_fns([&prog]);
        let mono = monomorphize(&prog, &all_fns, &check.expr_types);

        let mangled_names: Vec<&str> = mono.fns.iter().map(|f| f.mangled_name.as_str()).collect();
        assert!(
            mangled_names.contains(&"identity_Int"),
            "expected identity_Int in {:?}",
            mangled_names
        );
        assert!(
            mangled_names.contains(&"identity_String"),
            "expected identity_String in {:?}",
            mangled_names
        );
    }

    #[test]
    fn monomorphize_non_generic_fn_uses_original_name() {
        let prog = parse(
            r#"
fn add(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit { let r: Int = add(1, 2); }
"#,
        );
        let check = crate::mvl::checker::check(&prog);
        let all_fns = collect_fns([&prog]);
        let mono = monomorphize(&prog, &all_fns, &check.expr_types);

        let add_entry = mono.fns.iter().find(|f| f.original_name == "add");
        assert!(
            add_entry.is_some(),
            "non-generic fn should appear in MonoProgram"
        );
        let entry = add_entry.unwrap();
        assert_eq!(entry.mangled_name, "add");
        assert!(
            entry.type_subs.is_empty(),
            "non-generic fn must have empty type_subs"
        );
    }

    #[test]
    fn monomorphize_type_subs_recorded_correctly() {
        let prog = parse(
            r#"
fn identity[T](x: T) -> T { x }
fn main() -> Unit { let n: Int = identity(1); }
"#,
        );
        let check = crate::mvl::checker::check(&prog);
        let all_fns = collect_fns([&prog]);
        let mono = monomorphize(&prog, &all_fns, &check.expr_types);

        let inst = mono
            .fns
            .iter()
            .find(|f| f.mangled_name == "identity_Int")
            .expect("identity_Int must be in MonoProgram");
        assert_eq!(inst.original_name, "identity");
        let t_sub = inst.type_subs.get("T").expect("T must be in type_subs");
        assert_eq!(*t_sub, base("Int"));
    }

    #[test]
    fn monomorphize_same_instantiation_not_duplicated() {
        let prog = parse(
            r#"
fn identity[T](x: T) -> T { x }
fn main() -> Unit {
    let a: Int = identity(1);
    let b: Int = identity(2);
}
"#,
        );
        let check = crate::mvl::checker::check(&prog);
        let all_fns = collect_fns([&prog]);
        let mono = monomorphize(&prog, &all_fns, &check.expr_types);

        let count = mono
            .fns
            .iter()
            .filter(|f| f.mangled_name == "identity_Int")
            .count();
        assert_eq!(count, 1, "identical instantiation must not be duplicated");
    }
}
