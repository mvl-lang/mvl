// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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
//! | `stdout`    | (none)      | —                  | std.io Fd factory (#976)    |
//! | `stderr`    | (none)      | —                  | std.io Fd factory (#976)    |
//! | `write`     | `Console`   | msg must be Public | 003-information-flow/Req 6  |
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
//! # IFC label propagation (#1007)
//!
//! All functions propagate security labels unconditionally: security labels from arguments are
//! joined (lattice least-upper-bound) and applied to the return type.  This ensures that
//! data derived from a `Secret[T]` argument is itself `Secret[T]` throughout the program.
//!
//! Labels are erased ONLY by `relabel` expressions (the sole IFC keyword beyond `label`).
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
use crate::mvl::parser::ast::{
    Capability, Effect, FieldDecl, GenericParam, RefExpr, Totality, Variant,
};
use crate::mvl::parser::lexer::Span;

// ── Variable binding ─────────────────────────────────────────────────────────

/// Capability state of a variable (Phase D, Spec 009 Req 2).
///
/// Tracks whether a variable currently has any outstanding references,
/// enforcing capability-based reference rules at the checker level.
///
/// # State machine
///
/// Driven by `val`/`ref` type annotations on `let` bindings (#306, #660).
/// The transitions below are enforced by the checker.
///
/// * `Owned` → `Val(n)` when `val x` is created.
/// * `Owned` → `Ref` when `ref x` is created.
/// * `Val(n)` → `Val(n-1)` when a `val` reference goes out of scope.
/// * `Val(0)` == `Owned`.
/// * `Ref` → `Owned` when the `ref` reference goes out of scope.
#[derive(Debug, Clone, PartialEq)]
pub enum CapabilityState {
    /// No active capabilities — value is exclusively owned.
    Owned,
    /// `n` shared (`val T`) references are live.  Value may still be read but not mutably referenced.
    Val(usize),
    /// Exactly one exclusive (`ref T`) reference is live.  Value may not be read or re-referenced.
    Ref,
}

#[derive(Debug, Clone)]
pub struct VarInfo {
    pub ty: Ty,
    pub mutable: bool,
    pub moved: bool,
    /// Reference capability for actor-boundary checking (Req 9).
    pub capability: Option<Capability>,
    /// Scope depth at which this variable was defined (Phase C, Spec 009 Req 2).
    ///
    /// Used for scope-based lifetime checking: a `val`/`ref` reference to this
    /// variable must not be assigned to a binding at a shallower scope depth.
    pub scope_depth: usize,
    /// Active capability state (Phase D, Spec 009 Req 2).
    ///
    /// Tracks outstanding shared and mutable capabilities to enforce alias safety.
    pub capability_state: CapabilityState,
    /// Name of the variable this binding references, if any (Phase D).
    ///
    /// Set when `let r = val x` or `let r = ref x` is bound.  Used by `pop_scope()`
    /// to release the reference on `x` when `r` goes out of scope.
    pub ref_var: Option<String>,
}

impl VarInfo {
    pub fn new(ty: Ty, mutable: bool) -> Self {
        VarInfo {
            ty,
            mutable,
            moved: false,
            capability: None,
            scope_depth: 0,
            capability_state: CapabilityState::Owned,
            ref_var: None,
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
    pub params: Vec<GenericParam>,
    pub body: TypeBodyInfo,
}

#[derive(Debug, Clone)]
pub enum TypeBodyInfo {
    Struct {
        fields: Vec<FieldInfo>,
        /// Struct-level invariant predicate (Phase 6, #654).
        invariant: Option<RefExpr>,
    },
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
    /// Declared effects (Req 7): `! DB + Console` or `! FileRead("/path")`
    pub effects: Vec<Effect>,
    /// Totality annotation (Req 8): None = implicitly total, Some(Partial) = partial.
    pub totality: Option<Totality>,
    /// Ordered type parameter names declared on this function (e.g. `["T"]` in `fn f[T](...)`).
    /// Non-empty iff the function is generic. Order preserved for type-arg substitution (#989).
    pub type_params: Vec<String>,
}

impl Default for FnInfo {
    fn default() -> Self {
        FnInfo {
            params: vec![],
            ret: Ty::Unit,
            effects: vec![],
            totality: None,
            type_params: vec![],
        }
    }
}

// ── Type environment ─────────────────────────────────────────────────────────

/// Lexically-scoped variable environment + global type/function tables.
#[derive(Debug)]
pub struct TypeEnv {
    /// Stack of variable scopes (innermost last).
    scopes: Vec<HashMap<String, VarInfo>>,
    /// User-defined type declarations.
    ///
    /// Scoped to `checker/` (#1393): readers in `call_graph` and
    /// `ifc_propagation` access via iter/get; mutations route through
    /// [`define_type`].
    pub(super) types: HashMap<String, TypeInfo>,
    /// Known function signatures.
    ///
    /// Scoped to `checker/` (#1393): mutations route through
    /// [`define_fn`] / [`undefine_fn`].  External read-only access is
    /// still provided via [`fns`] iterators.
    pub(super) fns: HashMap<String, FnInfo>,
    /// Registered `From` implementations: maps target type name → set of source type names.
    /// Populated from `impl From<A> for B` declarations.
    pub from_impls: HashMap<String, HashSet<String>>,
    /// Declared IFC labels (#894). Maps label name → unit.
    /// Pre-seeded with `Tainted` and `Secret` from std/ifc.mvl.
    pub known_labels: HashSet<String>,
    /// Declared IFC relabel transitions (#894, #896).
    /// Maps transition name → (from, to, audit).
    /// `None` = bare type; `Some(name)` = declared label.
    /// `audit` = true when declaration carries `audit` keyword (#896).
    pub relabels: HashMap<String, (Option<String>, Option<String>, bool)>,
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeEnv {
    pub fn new() -> Self {
        let mut known_labels = HashSet::new();
        known_labels.insert("Tainted".to_string());
        known_labels.insert("Secret".to_string());
        // Capability labels (#931): IFC labels as capability tokens (Req 13 → Req 11).
        known_labels.insert("ConfigPath".to_string());
        known_labels.insert("DbUrl".to_string());
        known_labels.insert("ApiEndpoint".to_string());
        known_labels.insert("AuditTarget".to_string());
        // Pre-seed the four standard relabel transitions from std/ifc.mvl (#894).
        // These are always available without an explicit `use std.ifc` import,
        // mirroring how `Tainted` and `Secret` are pre-seeded as known labels.
        let mut relabels: HashMap<String, (Option<String>, Option<String>, bool)> = HashMap::new();
        relabels.insert("classify".into(), (None, Some("Secret".into()), false)); // _ -> Secret
        relabels.insert("taint".into(), (None, Some("Tainted".into()), false)); // _ -> Tainted
        relabels.insert("trust".into(), (Some("Tainted".into()), None, false)); // Tainted -> _
        relabels.insert("release".into(), (Some("Secret".into()), None, false)); // Secret -> _

        // Capability relabel transitions (#931): IFC labels as capability tokens.
        relabels.insert(
            "config_path".into(),
            (None, Some("ConfigPath".into()), false),
        ); // _ -> ConfigPath
        relabels.insert(
            "unconfig_path".into(),
            (Some("ConfigPath".into()), None, false),
        ); // ConfigPath -> _
        relabels.insert("db_url".into(), (None, Some("DbUrl".into()), false)); // _ -> DbUrl
        relabels.insert("undb_url".into(), (Some("DbUrl".into()), None, false)); // DbUrl -> _
        relabels.insert(
            "api_endpoint".into(),
            (None, Some("ApiEndpoint".into()), false),
        ); // _ -> ApiEndpoint
        relabels.insert(
            "unapi_endpoint".into(),
            (Some("ApiEndpoint".into()), None, false),
        ); // ApiEndpoint -> _
        relabels.insert(
            "audit_target".into(),
            (None, Some("AuditTarget".into()), false),
        ); // _ -> AuditTarget
        relabels.insert(
            "unaudit_target".into(),
            (Some("AuditTarget".into()), None, false),
        ); // AuditTarget -> _
        let mut env = TypeEnv {
            scopes: vec![HashMap::new()],
            types: HashMap::new(),
            fns: HashMap::new(),
            from_impls: HashMap::new(),
            known_labels,
            relabels,
        };
        env.register_builtins();
        env
    }

    /// Look up a relabel transition by name.
    /// Returns `(from, to, audit)` where `None` = bare and `Some(name)` = label.
    pub fn lookup_relabel(&self, name: &str) -> Option<(Option<String>, Option<String>, bool)> {
        self.relabels.get(name).cloned()
    }

    /// Register a label declaration.
    pub fn register_label(&mut self, name: String) {
        self.known_labels.insert(name);
    }

    /// Rewrite `Ty::Named(n, [t])` → `Ty::Labeled(n, t)` when `n` is a known label.
    ///
    /// The parser seeds `known_labels` only from the current file's declarations,
    /// so cross-file user labels arrive as `Ty::Named` and must be normalized here,
    /// where the full label set from all prelude `collect_declarations` passes is available.
    ///
    /// Normalization is **shallow** — only the outermost head is rewritten.  Nested
    /// labels inside compound types (e.g. `Option[L[T]]`) are not normalized here;
    /// callers must not assume compound types are fully canonical.
    pub fn normalize_ty(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named(n, mut args) if args.len() == 1 && self.known_labels.contains(n.as_str()) => {
                Ty::Labeled(n, Box::new(args.pop().unwrap()))
            }
            other => other,
        }
    }

    /// Register a relabel transition.
    pub fn register_relabel(
        &mut self,
        name: String,
        from: Option<String>,
        to: Option<String>,
        audit: bool,
    ) {
        self.relabels.insert(name, (from, to, audit));
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
    /// Note: `builtin_effect` is a convenience for constructing unparametrized `Effect`
    /// values with a zero span (builtins have no source location).
    ///
    /// These correspond to the MVL standard library tier 1 (core) functions
    /// that every program has access to without an import.
    fn register_builtins(&mut self) {
        // Console I/O primitives (#839, #976) — the builtins that back the
        // pure-MVL println/print/eprintln/eprint wrappers in std/core.mvl.
        //
        // Registered globally (no `use std.io` needed) so that std/core.mvl's
        // wrappers can call them without an import.
        //
        // stdout()/stderr() are `pub builtin fn` in std/io.mvl that return Fd; registered
        // here so std/core.mvl can call them without `use std.io`.
        //
        // IFC NOTE (#1007): write(fd, msg) has `! Console` effect — type system
        // rejects labeled msg arguments; implicit flow check uses effect reachability.
        let console_eff = vec![Effect::new("Console", Span::new(0, 0, 0, 0))];
        let io_error_ty = Ty::Named("IoError".into(), vec![]);
        let fd_ty = Ty::Named("Fd".into(), vec![]);
        self.fns.insert(
            "stdout".into(),
            FnInfo {
                params: vec![],
                ret: fd_ty.clone(),
                ..Default::default()
            },
        );
        self.fns.insert(
            "stderr".into(),
            FnInfo {
                params: vec![],
                ret: fd_ty.clone(),
                ..Default::default()
            },
        );
        self.fns.insert(
            "write".into(),
            FnInfo {
                params: vec![fd_ty.clone(), Ty::String],
                ret: Ty::Result(Box::new(Ty::Unit), Box::new(io_error_ty)),
                effects: console_eff,
                ..Default::default()
            },
        );
        // assert — pure, panics if condition is false
        self.fns.insert(
            "assert".into(),
            FnInfo {
                params: vec![Ty::Bool],
                ret: Ty::Unit,
                ..Default::default()
            },
        );
        // panic — unconditional termination; return type is Never (the bottom type)
        // so it is compatible with any expected type in match arms, if-branches, etc.
        // Marked partial because it aborts rather than returning a value.
        self.fns.insert(
            "panic".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::Never,
                totality: Some(Totality::Partial),
                ..Default::default()
            },
        );
        // assert_eq / assert_ne — pure, polymorphic testing assertions (#902).
        // Generic [T] so the checker skips type-checking (accepts Int, Bool, String, …).
        // IFC label guard enforced separately in checker/calls.rs.
        self.fns.insert(
            "assert_eq".into(),
            FnInfo {
                params: vec![Ty::Unknown, Ty::Unknown],
                ret: Ty::Unit,
                type_params: vec!["T".to_string()],
                ..Default::default()
            },
        );
        self.fns.insert(
            "assert_ne".into(),
            FnInfo {
                params: vec![Ty::Unknown, Ty::Unknown],
                ret: Ty::Unit,
                type_params: vec!["T".to_string()],
                ..Default::default()
            },
        );
        // Standard math functions — pure, variadic (arity checked by special-case)
        self.fns.insert(
            "abs".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::Int,
                ..Default::default()
            },
        );
        self.fns.insert(
            "max".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::Int,
                ..Default::default()
            },
        );
        self.fns.insert(
            "min".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::Int,
                ..Default::default()
            },
        );
        // parse_int(s: String) -> Result[Int, String] — arity enforced (#902).
        self.fns.insert(
            "parse_int".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
                ..Default::default()
            },
        );
        // format — string interpolation (#901): format(template, values) -> String
        // #1007: labels propagate unconditionally — format("{}", [secret]) returns Secret[String]
        self.fns.insert(
            "format".into(),
            FnInfo {
                params: vec![Ty::String, Ty::List(Box::new(Ty::String))],
                ret: Ty::String,
                ..Default::default()
            },
        );
        // range(start, end) — generates [start, start+1, …, end-1] (exclusive upper bound)
        self.fns.insert(
            "range".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::List(Box::new(Ty::Int)),
                ..Default::default()
            },
        );
        // Time — std.time: Instant, DateTime, Duration types + functions (#46)
        for name in &["Instant", "DateTime", "Duration"] {
            self.types.insert(
                (*name).into(),
                TypeInfo {
                    params: vec![],
                    body: TypeBodyInfo::Struct {
                        fields: vec![],
                        invariant: None,
                    },
                },
            );
        }
        // now() -> Instant ! Clock
        self.fns.insert(
            "now".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Named("Instant".into(), vec![]),
                effects: vec![Effect::new("Clock", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        // sleep(d: Duration) -> Unit ! Clock
        self.fns.insert(
            "sleep".into(),
            FnInfo {
                params: vec![Ty::Named("Duration".into(), vec![])],
                ret: Ty::Unit,
                effects: vec![Effect::new("Clock", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        // _instant_epoch_seconds(t: Instant) -> Int — module-private builtin (#899)
        self.fns.insert(
            "_instant_epoch_seconds".into(),
            FnInfo {
                params: vec![Ty::Named("Instant".into(), vec![])],
                ret: Ty::Int,
                ..Default::default()
            },
        );
        // parse(s: String, fmt: String) -> Option<DateTime> — pure
        self.fns.insert(
            "parse".into(),
            FnInfo {
                params: vec![Ty::String, Ty::String],
                ret: Ty::Option(Box::new(Ty::Named("DateTime".into(), vec![]))),
                ..Default::default()
            },
        );
        // seconds(n: Int) -> Duration — pure
        self.fns.insert(
            "seconds".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::Named("Duration".into(), vec![]),
                ..Default::default()
            },
        );
        // millis(n: Int) -> Duration — pure
        self.fns.insert(
            "millis".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::Named("Duration".into(), vec![]),
                ..Default::default()
            },
        );
        // Regex — std.regex: Regex, Match, Captures types + functions (#46)
        // `match` renamed to `find` to avoid keyword collision
        for name in &["Regex", "Match", "Captures"] {
            self.types.insert(
                (*name).into(),
                TypeInfo {
                    params: vec![],
                    body: TypeBodyInfo::Struct {
                        fields: vec![],
                        invariant: None,
                    },
                },
            );
        }
        // compile(pattern: String) -> Result<Regex, String> — pure
        self.fns.insert(
            "compile".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::Result(
                    Box::new(Ty::Named("Regex".into(), vec![])),
                    Box::new(Ty::String),
                ),
                ..Default::default()
            },
        );
        // find(re: Regex, s: String) -> Option<Match> — pure
        self.fns.insert(
            "find".into(),
            FnInfo {
                params: vec![Ty::Named("Regex".into(), vec![]), Ty::String],
                ret: Ty::Option(Box::new(Ty::Named("Match".into(), vec![]))),
                ..Default::default()
            },
        );
        // find_all(re: Regex, s: String) -> List<Match> — pure
        self.fns.insert(
            "find_all".into(),
            FnInfo {
                params: vec![Ty::Named("Regex".into(), vec![]), Ty::String],
                ret: Ty::List(Box::new(Ty::Named("Match".into(), vec![]))),
                ..Default::default()
            },
        );
        // replace(re: Regex, s: String, replacement: String) -> String — pure
        self.fns.insert(
            "replace".into(),
            FnInfo {
                params: vec![Ty::Named("Regex".into(), vec![]), Ty::String, Ty::String],
                ret: Ty::String,
                ..Default::default()
            },
        );
        // captures(re: Regex, s: String) -> Option<Captures> — pure
        self.fns.insert(
            "captures".into(),
            FnInfo {
                params: vec![Ty::Named("Regex".into(), vec![]), Ty::String],
                ret: Ty::Option(Box::new(Ty::Named("Captures".into(), vec![]))),
                ..Default::default()
            },
        );
        // JSON — std.json: Value enum, encode(), decode() (#46)
        self.types.insert(
            "Value".into(),
            TypeInfo {
                params: vec![],
                body: TypeBodyInfo::Enum(vec![
                    VariantInfo {
                        name: "String".into(),
                        fields: VariantFieldsInfo::Tuple(vec![Ty::String]),
                    },
                    VariantInfo {
                        name: "Number".into(),
                        fields: VariantFieldsInfo::Tuple(vec![Ty::Float]),
                    },
                    VariantInfo {
                        name: "Null".into(),
                        fields: VariantFieldsInfo::Unit,
                    },
                ]),
            },
        );
        // encode(v: Value) -> String — pure
        // #1007: taint from the Value propagates to the encoded String
        self.fns.insert(
            "encode".into(),
            FnInfo {
                params: vec![Ty::Named("Value".into(), vec![])],
                ret: Ty::String,
                ..Default::default()
            },
        );
        // decode(s: String) -> Result<Value, String> — pure
        // #1007: taint from input string propagates to decoded Value
        self.fns.insert(
            "decode".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::Result(
                    Box::new(Ty::Named("Value".into(), vec![])),
                    Box::new(Ty::String),
                ),
                ..Default::default()
            },
        );
        // Random — std.random: int(), float(), bytes(), choice(), shuffle() (#46)
        // All require ! Random effect.
        self.fns.insert(
            "int".into(),
            FnInfo {
                params: vec![Ty::Int, Ty::Int],
                ret: Ty::Int,
                effects: vec![Effect::new("Random", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        // float() → Float — 0-arg, arity now enforced by the checker (#902).
        self.fns.insert(
            "float".into(),
            FnInfo {
                params: vec![],
                ret: Ty::Float,
                effects: vec![Effect::new("Random", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        self.fns.insert(
            "bytes".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::List(Box::new(Ty::Int)),
                effects: vec![Effect::new("Random", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        // choice(items: List[T]) -> Option[T] — generic, arity/type-check skipped via is_generic (#902).
        self.fns.insert(
            "choice".into(),
            FnInfo {
                params: vec![Ty::Unknown],
                ret: Ty::Option(Box::new(Ty::Unknown)),
                effects: vec![Effect::new("Random", Span::new(0, 0, 0, 0))],
                type_params: vec!["T".to_string()],
                ..Default::default()
            },
        );
        // shuffle(items: List[T]) -> List[T] — generic, arity/type-check skipped via is_generic (#902).
        self.fns.insert(
            "shuffle".into(),
            FnInfo {
                params: vec![Ty::Unknown],
                ret: Ty::List(Box::new(Ty::Unknown)),
                effects: vec![Effect::new("Random", Span::new(0, 0, 0, 0))],
                type_params: vec!["T".to_string()],
                ..Default::default()
            },
        );
        // Crypto — std.crypto (no import required at tier-1)
        // sha256/sha512 are pure hash functions (#46).
        // _sha256/_sha512 are private builtins (#899); sha256/sha512 are public wrappers.
        self.fns.insert(
            "_sha256".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::String,
                ..Default::default()
            },
        );
        self.fns.insert(
            "sha256".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::String,
                ..Default::default()
            },
        );
        self.fns.insert(
            "_sha512".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::String,
                ..Default::default()
            },
        );
        self.fns.insert(
            "sha512".into(),
            FnInfo {
                params: vec![Ty::String],
                ret: Ty::String,
                ..Default::default()
            },
        );
        // crypto_random_bytes requires ! CryptoRandom; returns Secret<List<Int>>
        self.fns.insert(
            "crypto_random_bytes".into(),
            FnInfo {
                params: vec![Ty::Int],
                ret: Ty::Labeled("Secret".to_string(), Box::new(Ty::List(Box::new(Ty::Int)))),
                effects: vec![Effect::new("CryptoRandom", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
        // std.log — all functions are pure MVL in std/log.mvl; no Rust-backed primitives.
        // All four severity methods (Logger::debug/info/warn/error) and the
        // format/level/timestamp helpers are loaded from std/log.mvl when imported.
        //   log_write(fd, line) — writes a pre-formatted line to an Fd under ! Log (#1152)
        // This registration keeps the checker aware of log_write even when std/log.mvl is
        // not fully loaded; the MVL-loaded declaration overrides this when it is.
        self.fns.insert(
            "log_write".into(),
            FnInfo {
                params: vec![Ty::Named("Fd".into(), vec![]), Ty::String],
                ret: Ty::Unit,
                effects: vec![Effect::new("Log", Span::new(0, 0, 0, 0))],
                ..Default::default()
            },
        );
    }

    // ── Scope management ─────────────────────────────────────────────────

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            // Phase D: release capabilities held by variables going out of scope.
            for info in scope.values() {
                if let Some(ref borrowed_name) = info.ref_var {
                    if let Some(target) = self.lookup_mut_var(borrowed_name) {
                        target.capability_state = match target.capability_state {
                            CapabilityState::Val(1) | CapabilityState::Ref => {
                                CapabilityState::Owned
                            }
                            CapabilityState::Val(n) => CapabilityState::Val(n - 1),
                            CapabilityState::Owned => CapabilityState::Owned,
                        };
                    }
                }
            }
        }
    }

    // ── Variable operations ──────────────────────────────────────────────

    pub fn define(&mut self, name: String, mut info: VarInfo) {
        // Record scope depth (0-based: outermost scope = 0) so lifetime checking can
        // compare referent depth vs binding depth.  Note: scope_depth() returns
        // scopes.len() (raw length) — a different convention; do not cross-compare.
        info.scope_depth = self.scopes.len().saturating_sub(1);
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

    /// Returns the raw scope stack height (= number of open scopes).
    /// Used by lambda-capture checking to record the depth at which a lambda was entered.
    ///
    /// Convention: returns `scopes.len()` (raw length, not 0-based).
    /// `VarInfo.scope_depth` uses `scopes.len() - 1` (0-based index); do not cross-compare.
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

    /// Remove a previously-defined function from the global table.
    ///
    /// Returns the removed `FnInfo`, mirroring [`HashMap::remove`].  Used by
    /// `decls::check_actor_decl` to drop temporary private-method
    /// registrations after type inference completes (#1393).
    pub fn undefine_fn(&mut self, name: &str) -> Option<FnInfo> {
        self.fns.remove(name)
    }

    /// Read-only iterator over `(name, FnInfo)` pairs in the global table.
    ///
    /// Exposed so callers in `checker/` can traverse the table without
    /// touching the underlying `HashMap` directly.
    pub fn fns(&self) -> impl Iterator<Item = (&String, &FnInfo)> {
        self.fns.iter()
    }

    /// Read-only iterator over function names in the global table.
    pub fn fn_names(&self) -> impl Iterator<Item = &String> {
        self.fns.keys()
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
            ty: {
                let r = resolve(&f.ty);
                match r {
                    crate::mvl::checker::types::Ty::Ref(_, inner) => *inner,
                    other => other,
                }
            },
            mutable: matches!(
                crate::mvl::checker::types::resolve(&f.ty).base(),
                crate::mvl::checker::types::Ty::Ref(true, _)
            ),
        })
        .collect()
}

/// Like `field_infos` but marks every field mutable — used for actor state fields,
/// which are always privately mutable by design (Spec 015).
pub fn actor_field_infos(fields: &[FieldDecl]) -> Vec<FieldInfo> {
    fields
        .iter()
        .map(|f| FieldInfo {
            name: f.name.clone(),
            ty: {
                let r = resolve(&f.ty);
                match r {
                    crate::mvl::checker::types::Ty::Ref(_, inner) => *inner,
                    other => other,
                }
            },
            mutable: true,
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
