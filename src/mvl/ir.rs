// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Typed Intermediate Representation (TIR) — post-checker, post-monomorphization IR.
//!
//! TIR is produced by lowering a [`MonoProgram`] with the checker's resolved type
//! map.  Every expression node carries its fully-resolved [`Ty`] inline, so
//! downstream consumers (backends, analysis passes) do not need to carry a
//! separate `Span → Ty` lookup table.
//!
//! # Position in the pipeline
//!
//! ```text
//! parser → resolver → checker → mono → TIR lower → backends
//! ```
//!
//! # Design
//!
//! - [`TirExpr`] = `{ kind: TirExprKind, ty: Ty, span: Span }` — type embedded at every node.
//! - [`TirFn`] covers only concrete (monomorphized) functions; generic originals are absent.
//! - AST nodes that are not relevant to backends (ghost lets, quantifiers) are preserved
//!   structurally but backends may ignore them.
//!
//! # Related
//!
//! ADR-0034 (monomorphization pass — TIR supersedes MonoProgram for backend consumption),
//! issue #1096.

pub mod lower;
pub mod visit;

/// Re-exported so backends and passes import `Ty` from `ir`, not from `checker::types`.
/// This keeps the checker boundary clean: backends depend on `ir`, not on `checker` internals.
pub use crate::mvl::checker::types::Ty;

// Primitive AST types re-exported so backends can import from `crate::mvl::ir`
// and have zero direct dependencies on `parser::ast`.  These types carry no
// `TypeExpr` fields — they are structural primitives reused unchanged through
// the pipeline.
pub use crate::mvl::parser::ast::{
    ArithOp, BinaryOp, BitwiseOp, Capability, CmpOp, Constraint, Effect, EffectDecl, GenericParam,
    LValue, LabelDecl, LetKind, Literal, LogicOp, MailboxConfig, MailboxPolicy, Pattern, RefExpr,
    RelabelDecl, StringOp, Totality, TypeExpr, UnaryOp, UseDecl,
};
use crate::mvl::parser::lexer::Span;

// ── Expressions ───────────────────────────────────────────────────────────────

/// A typed expression node.  The resolved type of the expression is in `ty`.
#[derive(Debug, Clone, PartialEq)]
pub struct TirExpr {
    pub kind: TirExprKind,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TirExprKind {
    Literal(Literal),
    Var(String),
    FieldAccess {
        expr: Box<TirExpr>,
        field: String,
    },
    MethodCall {
        receiver: Box<TirExpr>,
        method: String,
        args: Vec<TirExpr>,
    },
    /// Direct function call.  `name` is the mangled symbol when the callee was generic.
    /// `type_args` preserves the original syntactic type arguments (e.g. for extern/stdlib
    /// calls that require turbofish syntax in Rust codegen). Empty for ordinary calls.
    FnCall {
        name: String,
        args: Vec<TirExpr>,
        type_args: Vec<TypeExpr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<TirExpr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<TirExpr>,
        right: Box<TirExpr>,
    },
    If {
        cond: Box<TirExpr>,
        then: TirBlock,
        else_: Option<Box<TirExpr>>,
    },
    Match {
        scrutinee: Box<TirExpr>,
        arms: Vec<TirMatchArm>,
    },
    Block(TirBlock),
    Lambda {
        params: Vec<TirParam>,
        body: Box<TirExpr>,
    },
    /// `expr?` — propagate Result/Option failure.
    Propagate(Box<TirExpr>),
    /// `Name { field: value, … }` struct/enum-variant construction.
    Construct {
        name: String,
        fields: Vec<(String, TirExpr)>,
    },
    List {
        elems: Vec<TirExpr>,
    },
    Map {
        pairs: Vec<(TirExpr, TirExpr)>,
    },
    Set {
        elems: Vec<TirExpr>,
    },
    /// `consume expr` — move out of an `iso` binding.
    Consume(Box<TirExpr>),
    /// `relabel name(expr, "tag")` — IFC relabel transition.
    Relabel {
        name: String,
        expr: Box<TirExpr>,
        tag: String,
        /// Combined audit flag: true if expression-level `audit` OR declaration-level `audit`.
        audit: bool,
    },
    /// `val expr` / `ref expr` — expression-level borrow.
    Borrow {
        mutable: bool,
        expr: Box<TirExpr>,
    },
    /// `actor ActorType { … }` — actor spawn, evaluates to `ActorRef[ActorType]`.
    Spawn {
        actor_type: String,
        fields: Vec<(String, TirExpr)>,
    },
    /// `select { … }` — concurrent branch selection.
    Select {
        arms: Vec<TirSelectArm>,
    },
    /// `forall`/`exists` quantifier — specification only, erased before codegen.
    ///
    /// The `RefExpr` payload is a parser AST node, retained intentionally: quantifier
    /// predicates are consumed by spec-output and contract-checking passes that already
    /// depend on `ast::RefExpr`. Backends that do not need the predicate can ignore it.
    Quantifier(Box<RefExpr>),
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TirStmt {
    Let {
        kind: LetKind,
        pattern: Pattern,
        /// Declared type of the binding (fully resolved and substituted).
        ty: Ty,
        init: TirExpr,
        span: Span,
    },
    Assign {
        target: LValue,
        value: TirExpr,
        span: Span,
    },
    Return {
        value: Option<TirExpr>,
        span: Span,
    },
    If {
        cond: TirExpr,
        then: TirBlock,
        else_: Option<TirElseBranch>,
        span: Span,
    },
    Match {
        scrutinee: TirExpr,
        arms: Vec<TirMatchArm>,
        span: Span,
    },
    For {
        pattern: Pattern,
        iter: TirExpr,
        /// Loop invariant predicates — `invariant pred` clauses.
        invariants: Vec<TirExpr>,
        body: TirBlock,
        span: Span,
    },
    While {
        cond: TirExpr,
        /// Loop invariant predicates — `invariant pred` clauses.
        invariants: Vec<TirExpr>,
        /// Optional termination measure — `decreases expr` clause.
        decreases: Option<Box<TirExpr>>,
        body: TirBlock,
        span: Span,
    },
    Expr {
        expr: TirExpr,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TirElseBranch {
    Block(TirBlock),
    If(Box<TirStmt>),
}

// ── Blocks and arms ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TirBlock {
    pub stmts: Vec<TirStmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TirMatchArm {
    pub pattern: Pattern,
    /// Match guard — `if pred` clause attached to this arm.  `None` for unguarded arms.
    pub guard: Option<RefExpr>,
    pub body: TirMatchBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TirMatchBody {
    Expr(TirExpr),
    Block(TirBlock),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TirSelectArm {
    pub binding: Option<String>,
    pub expr: Box<TirExpr>,
    pub is_timeout: bool,
    pub body: TirBlock,
    pub span: Span,
}

// ── Function level ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TirParam {
    pub name: String,
    pub ty: Ty,
    pub capability: Option<Capability>,
    pub span: Span,
}

/// A concrete (monomorphized), fully-typed function.
#[derive(Debug, Clone, PartialEq)]
pub struct TirFn {
    /// Mangled symbol, e.g. `"map_Int_String"` for `map[T=Int, U=String]`.
    pub name: String,
    /// Original unmangled name.
    pub original_name: String,
    /// Source package name (e.g. `"http"` for `pkg.http`), `None` for user code and stdlib.
    /// Set by the Rust backend after TIR lowering to drive cross-package deduplication.
    pub pkg_name: Option<String>,
    /// Whether the item is exported from this module (`pub`).
    pub visible: bool,
    /// Whether the function is a test (`test fn`).
    pub is_test: bool,
    /// Whether the function has a runtime-provided implementation (`builtin fn`).
    pub is_builtin: bool,
    /// For extension methods: the receiver type name (e.g. `"String"` for `fn String::len`).
    pub receiver_type: Option<String>,
    /// Generic type parameters (preserved for backends that emit generic defs).
    pub type_params: Vec<GenericParam>,
    /// Where-clause constraints (`where T: Eq`).
    pub constraints: Vec<Constraint>,
    /// Totality annotation: `None` = unknown/partial, `Some(Total)` = proved terminating.
    pub totality: Option<Totality>,
    pub params: Vec<TirParam>,
    pub ret_ty: Ty,
    /// Refinement predicate on the return type (`-> Int where self > 0`).
    pub return_refinement: Option<RefExpr>,
    pub effects: Vec<Effect>,
    /// Preconditions lowered to `RefExpr` — ready for backend assertion emission.
    pub requires: Vec<RefExpr>,
    /// Postconditions lowered to `RefExpr`.
    pub ensures: Vec<RefExpr>,
    pub body: TirBlock,
    pub span: Span,
}

// ── Declaration-level TIR types ───────────────────────────────────────────────

/// A struct field or actor field with a resolved type.
#[derive(Debug, Clone, PartialEq)]
pub struct TirFieldDecl {
    pub name: String,
    pub ty: Ty,
    /// Inline refinement predicate — kept as AST `RefExpr` (spec-only, erased before codegen).
    pub refinement: Option<RefExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TirVariant {
    pub name: String,
    pub fields: TirVariantFields,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TirVariantFields {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<TirFieldDecl>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TirTypeBody {
    Struct {
        fields: Vec<TirFieldDecl>,
        /// Struct-level invariant — kept as AST `RefExpr` (spec-only).
        invariant: Option<RefExpr>,
    },
    Enum(Vec<TirVariant>),
    Alias(Ty),
}

/// A type declaration with all `TypeExpr` fields resolved to `Ty`.
#[derive(Debug, Clone, PartialEq)]
pub struct TirTypeDecl {
    pub visible: bool,
    pub name: String,
    /// Generic type parameters — kept for backends that emit generic type definitions.
    pub params: Vec<GenericParam>,
    pub body: TirTypeBody,
    pub span: Span,
}

/// A single function signature inside an `extern` block, with resolved types.
#[derive(Debug, Clone, PartialEq)]
pub struct TirExternFn {
    pub name: String,
    pub params: Vec<TirParam>,
    pub ret_ty: Ty,
    pub effects: Vec<Effect>,
    pub totality: Option<Totality>,
    pub span: Span,
}

/// An `extern "abi" { … }` block with resolved function signatures.
#[derive(Debug, Clone, PartialEq)]
pub struct TirExternDecl {
    pub abi: String,
    /// Libraries to link against (from `link("m", "pthread")`).
    pub link_libs: Vec<String>,
    pub fns: Vec<TirExternFn>,
    pub span: Span,
}

/// A method inside an actor declaration, with a fully-typed body.
#[derive(Debug, Clone, PartialEq)]
pub struct TirActorMethod {
    /// `true` = public async behavior or `pub test fn`, `false` = private sync helper.
    pub is_public: bool,
    /// `true` = `pub test fn` — test-only synchronous, non-Unit return allowed (#1506).
    pub is_test: bool,
    pub name: String,
    pub params: Vec<TirParam>,
    pub ret_ty: Ty,
    pub effects: Vec<Effect>,
    pub body: TirBlock,
    pub span: Span,
}

/// An actor type declaration with resolved field types and typed method bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct TirActorDecl {
    pub visible: bool,
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub fields: Vec<TirFieldDecl>,
    pub methods: Vec<TirActorMethod>,
    /// `None` = default mailbox (256, DropNewest). Kept as AST type — no types inside.
    pub mailbox: Option<MailboxConfig>,
    pub traps_exit: bool,
    pub span: Span,
}

/// An `impl Trait for Type { … }` block with fully-typed method bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct TirImplDecl {
    pub trait_name: String,
    /// Resolved trait type arguments.
    pub trait_type_args: Vec<Ty>,
    pub type_name: String,
    /// Methods lowered as `TirFn`; `name == original_name` (impl methods are not mangled).
    pub methods: Vec<TirFn>,
    pub span: Span,
}

/// A `const` declaration with a fully-typed value expression.
#[derive(Debug, Clone, PartialEq)]
pub struct TirConstDecl {
    pub visible: bool,
    pub name: String,
    pub ty: Ty,
    pub value: TirExpr,
    pub span: Span,
}

// ── Program ───────────────────────────────────────────────────────────────────

/// Output of the TIR lowering pass — complete program representation for backends.
///
/// All type annotations are resolved to [`Ty`]; generic function bodies are
/// monomorphized (one [`TirFn`] per instantiation). Non-function declarations
/// carry resolved types but are not monomorphized — backends emit them once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TirProgram {
    /// Monomorphized, fully-typed functions (one copy per generic instantiation).
    pub fns: Vec<TirFn>,
    /// Type declarations (`struct`, `enum`, `alias`) with resolved field types.
    pub types: Vec<TirTypeDecl>,
    /// `extern "abi" { … }` blocks with resolved signatures.
    pub externs: Vec<TirExternDecl>,
    /// Actor type declarations with typed method bodies.
    pub actors: Vec<TirActorDecl>,
    /// `impl Trait for Type { … }` blocks with typed method bodies.
    pub impls: Vec<TirImplDecl>,
    /// `const` declarations with typed value expressions.
    pub consts: Vec<TirConstDecl>,
    /// `use` declarations — kept as AST nodes (no types to resolve).
    pub uses: Vec<UseDecl>,
    /// `effect` declarations — kept as AST nodes.
    pub effect_decls: Vec<EffectDecl>,
    /// `label` declarations — kept as AST nodes.
    pub label_decls: Vec<LabelDecl>,
    /// `relabel` declarations — kept as AST nodes.
    pub relabel_decls: Vec<RelabelDecl>,
}
