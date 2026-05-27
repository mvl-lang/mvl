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

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{
    BinaryOp, Capability, Effect, LValue, LetKind, Literal, Pattern, RefExpr, Totality, UnaryOp,
};
use crate::mvl::parser::lexer::Span;

// ── Expressions ───────────────────────────────────────────────────────────────

/// A typed expression node.  The resolved type of the expression is in `ty`.
#[derive(Debug, Clone)]
pub struct TirExpr {
    pub kind: TirExprKind,
    pub ty: Ty,
    pub span: Span,
}

impl TirExpr {
    pub fn ty(&self) -> &Ty {
        &self.ty
    }
}

#[derive(Debug, Clone)]
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
    FnCall {
        name: String,
        args: Vec<TirExpr>,
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
    Quantifier(Box<RefExpr>),
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
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
        body: TirBlock,
        span: Span,
    },
    While {
        cond: TirExpr,
        body: TirBlock,
        span: Span,
    },
    Expr {
        expr: TirExpr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum TirElseBranch {
    Block(TirBlock),
    If(Box<TirStmt>),
}

// ── Blocks and arms ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TirBlock {
    pub stmts: Vec<TirStmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TirMatchArm {
    pub pattern: Pattern,
    pub body: TirMatchBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TirMatchBody {
    Expr(TirExpr),
    Block(TirBlock),
}

#[derive(Debug, Clone)]
pub struct TirSelectArm {
    pub binding: Option<String>,
    pub expr: Box<TirExpr>,
    pub is_timeout: bool,
    pub body: TirBlock,
    pub span: Span,
}

// ── Function level ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TirParam {
    pub name: String,
    pub ty: Ty,
    pub capability: Option<Capability>,
    pub span: Span,
}

/// A concrete (monomorphized), fully-typed function.
#[derive(Debug, Clone)]
pub struct TirFn {
    /// Mangled symbol, e.g. `"map_Int_String"` for `map[T=Int, U=String]`.
    pub name: String,
    /// Original unmangled name.
    pub original_name: String,
    /// Totality annotation: `None` = unknown/partial, `Some(Total)` = proved terminating.
    pub totality: Option<Totality>,
    pub params: Vec<TirParam>,
    pub ret_ty: Ty,
    pub effects: Vec<Effect>,
    pub body: TirBlock,
    pub span: Span,
}

// ── Program ───────────────────────────────────────────────────────────────────

/// Output of the TIR lowering pass — typed, monomorphized functions ready for backends.
#[derive(Debug, Clone, Default)]
pub struct TirProgram {
    pub fns: Vec<TirFn>,
}
