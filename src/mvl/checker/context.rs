//! Type environment: symbol tables for variables, types, and functions.
//!
//! This module owns the [`TypeEnv`] used throughout type checking.  It also
//! registers the built-in (no-import) standard library functions that every
//! MVL program can call without a `use` declaration.
//!
//! # Responsibility
//!
//! [`TypeEnv`] holds the three lookup tables needed by the type checker:
//! - Variable bindings (lexically scoped) — checked against Reqs 1, 2, 6 (type safety, ownership, immutability)
//! - Type declarations (global) — checked against Reqs 1, 3, 4 (ADTs, null elimination)
//! - Function signatures (global) — checked against Reqs 1, 7, 8, 11 (types, effects, totality, IFC)
//!
//! # Built-in functions
//!
//! [`TypeEnv::register_builtins`] populates the function table with the MVL tier-1 stdlib
//! functions that are available without any `use` import.  Each builtin's declared effects
//! and parameter types must satisfy the corresponding spec requirement:
//!
//! | Function    | Effects     | IFC constraint     | Spec ref                    |
//! |-------------|-------------|--------------------|-----------------------------|
//! | `println`   | `Console`   | args must be Public| 003-information-flow/Req 6  |
//! | `print`     | `Console`   | args must be Public| 003-information-flow/Req 6  |
//! | `assert_eq` | (none)      | —                  | 004-testing/Req 4           |
//! | `abs`       | (none)      | —                  | stdlib math                 |
//! | `max`       | (none)      | —                  | stdlib math                 |
//! | `min`       | (none)      | —                  | stdlib math                 |
//! | `parse_int` | (none)      | —                  | stdlib conversion           |
//!
//! Note: `println`/`print` are variadic (empty `params` vec as sentinel).  Arity checking
//! is skipped for them in the checker; IFC label checking is applied per-argument instead.
//! See also: ADR-0002 (language contraction — no variadic user functions), ADR-0003 (compilation).
//!
//! # Spec links
//!
//! - Builtin `println` / `print` — 002-effect-system Req 1 (Console effect),
//!   003-information-flow Req 6 (logging label constraint, Deferred Phase 2).
//! - Builtin `assert_eq` — 004-testing Req 1 (test assertions).
//! - Builtin math (`abs`, `max`, `min`) — 001-type-system Req 1 (numeric ops).
//! - Builtin `parse_int` — 001-type-system Req 5 (error visibility via Result).

use std::collections::{HashMap, HashSet};

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{Capability, FieldDecl, Totality, Variant};

// ── Variable binding ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VarInfo {
    pub ty: Ty,
    pub mutable: bool,
    pub moved: bool,
    /// Reference capability for actor-boundary checking (Req 9).
    pub capability: Option<Capability>,
}

impl VarInfo {
    pub fn new(ty: Ty, mutable: bool) -> Self {
        VarInfo {
            ty,
            mutable,
            moved: false,
            capability: None,
        }
    }

    pub fn with_capability(mut self, cap: Option<Capability>) -> Self {
        self.capability = cap;
        self
    }
}

// ── Type definition ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub params: Vec<String>,
    pub body: TypeBodyInfo,
}

#[derive(Debug, Clone)]
pub enum TypeBodyInfo {
    Struct(Vec<FieldInfo>),
    Enum(Vec<VariantInfo>),
    Alias(Ty),
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct VariantInfo {
    pub name: String,
    pub fields: VariantFieldsInfo,
}

#[derive(Debug, Clone)]
pub enum VariantFieldsInfo {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<FieldInfo>),
}

// ── Function signature ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FnInfo {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// Declared effects (Req 7): `! DB, Console`
    pub effects: Vec<String>,
    /// Totality annotation (Req 8): None = implicitly total, Some(Partial) = partial.
    pub totality: Option<Totality>,
}

// ── Type environment ─────────────────────────────────────────────────────────

/// Lexically-scoped variable environment + global type/function tables.
pub struct TypeEnv {
    /// Stack of variable scopes (innermost last).
    scopes: Vec<HashMap<String, VarInfo>>,
    /// User-defined type declarations.
    pub types: HashMap<String, TypeInfo>,
    /// Known function signatures.
    pub fns: HashMap<String, FnInfo>,
    /// Registered `From` implementations: maps target type name → set of source type names.
    /// Populated from `impl From<A> for B` declarations.
    pub from_impls: HashMap<String, HashSet<String>>,
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeEnv {
    pub fn new() -> Self {
        let mut env = TypeEnv {
            scopes: vec![HashMap::new()],
            types: HashMap::new(),
            fns: HashMap::new(),
            from_impls: HashMap::new(),
        };
        env.register_builtins();
        env
    }

    /// Register a `From<source>` impl for `target`.
    pub fn register_from_impl(&mut self, target: String, source: String) {
        self.from_impls.entry(target).or_default().insert(source);
    }

    /// Returns true if `From<source> for target` has been registered.
    pub fn has_from_impl(&self, target: &str, source: &str) -> bool {
        self.from_impls
            .get(target)
            .is_some_and(|sources| sources.contains(source))
    }

    /// Register built-in stdlib functions so the checker accepts them.
    ///
    /// These correspond to the MVL standard library tier 1 (core) functions
    /// that every program has access to without an import.
    fn register_builtins(&mut self) {
        // Console I/O — require ! Console effect (002-effect-system Req 1).
        // params: Vec<Ty> is empty here because println/print are variadic;
        // the checker special-cases them to skip arity checking.
        //
        // IFC NOTE (003-information-flow Req 6, Deferred — Phase 2):
        // Per spec, logging functions MUST accept only `Public<T>` arguments.
        // Enforcing this label constraint requires stdlib `log` module integration
        // and is deferred to Phase 2.  For now, println/print accept any argument.
        self.fns.insert(
            "println".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Unit,
                effects: vec!["Console".into()],
                totality: None,
            },
        );
        self.fns.insert(
            "print".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Unit,
                effects: vec!["Console".into()],
                totality: None,
            },
        );
        // assert_eq — pure, for testing.
        // TODO: assert_eq accepts Secret/Tainted arguments without an IFC label check.
        // Assertion failures may expose secret values via panic messages (observable covert
        // channel). Tracked as a known gap; full enforcement requires Phase 2 IFC propagation.
        self.fns.insert(
            "assert_eq".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Unit,
                effects: vec![],
                totality: None,
            },
        );
        // Standard math functions — pure, variadic (arity checked by special-case)
        self.fns.insert(
            "abs".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::Int,
                effects: vec![],
                totality: None,
            },
        );
        self.fns.insert(
            "max".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::Int,
                effects: vec![],
                totality: None,
            },
        );
        self.fns.insert(
            "min".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::Int,
                effects: vec![],
                totality: None,
            },
        );
        // parse_int — converts String to Result<Int, String>; variadic-flagged to skip arity check
        self.fns.insert(
            "parse_int".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
                effects: vec![],
                totality: None,
            },
        );
        // format — string interpolation, variadic (template + args), pure
        self.fns.insert(
            "format".into(),
            FnInfo {
                params: vec![],
                ret: Ty::String,
                effects: vec![],
                totality: None,
            },
        );
    }

    // ── Scope management ─────────────────────────────────────────────────

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    // ── Variable operations ──────────────────────────────────────────────

    pub fn define(&mut self, name: String, info: VarInfo) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, info);
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    /// Like [`lookup`], but also returns the scope index (0 = outermost) where the
    /// variable was found.  Used by lambda-capture checking to distinguish captured
    /// outer variables from locally-defined ones.
    pub fn lookup_with_scope_index(&self, name: &str) -> Option<(usize, &VarInfo)> {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if let Some(info) = scope.get(name) {
                return Some((i, info));
            }
        }
        None
    }

    /// Returns the current scope stack depth.  Used by lambda-capture checking to
    /// record the depth at which a lambda was entered.
    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }

    pub fn lookup_mut_var(&mut self, name: &str) -> Option<&mut VarInfo> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                return scope.get_mut(name);
            }
        }
        None
    }

    pub fn mark_moved(&mut self, name: &str) {
        if let Some(info) = self.lookup_mut_var(name) {
            info.moved = true;
        }
    }

    // ── Type table ───────────────────────────────────────────────────────

    pub fn define_type(&mut self, name: String, info: TypeInfo) {
        self.types.insert(name, info);
    }

    pub fn lookup_type(&self, name: &str) -> Option<&TypeInfo> {
        self.types.get(name)
    }

    // ── Function table ───────────────────────────────────────────────────

    pub fn define_fn(&mut self, name: String, info: FnInfo) {
        self.fns.insert(name, info);
    }

    pub fn lookup_fn(&self, name: &str) -> Option<&FnInfo> {
        self.fns.get(name)
    }
}

// ── Helpers to build TypeInfo from AST ──────────────────────────────────────

use crate::mvl::checker::types::resolve;

pub fn field_infos(fields: &[FieldDecl]) -> Vec<FieldInfo> {
    fields
        .iter()
        .map(|f| FieldInfo {
            name: f.name.clone(),
            ty: resolve(&f.ty),
            mutable: f.mutable,
        })
        .collect()
}

pub fn variant_infos(variants: &[Variant]) -> Vec<VariantInfo> {
    use crate::mvl::parser::ast::VariantFields;
    variants
        .iter()
        .map(|v| VariantInfo {
            name: v.name.clone(),
            fields: match &v.fields {
                VariantFields::Unit => VariantFieldsInfo::Unit,
                VariantFields::Tuple(tys) => {
                    VariantFieldsInfo::Tuple(tys.iter().map(resolve).collect())
                }
                VariantFields::Struct(fields) => VariantFieldsInfo::Struct(field_infos(fields)),
            },
        })
        .collect()
}
