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

/// Re-exported so backends and passes import `Ty` from `ir`, not from `checker::types`.
/// This keeps the checker boundary clean: backends depend on `ir`, not on `checker` internals.
pub use crate::mvl::checker::types::Ty;

use crate::mvl::parser::ast::{
    BinaryOp, Capability, Effect, LValue, LetKind, Literal, Pattern, RefExpr, Totality, TypeExpr,
    UnaryOp,
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TirProgram {
    pub fns: Vec<TirFn>,
}

impl TirProgram {
    /// Build a `Span → Ty` index over all expression nodes in this program.
    ///
    /// Backends that still iterate AST nodes can use this map as a drop-in replacement
    /// for `CheckResult::expr_types` — types come from TIR (post-substitution) rather
    /// than directly from the checker.
    ///
    /// # Generic functions
    ///
    /// When a generic function has multiple monomorphized copies (e.g. `identity_Int`
    /// and `identity_String`), each body expression shares the same source span.
    /// The map stores the type from the **last** instantiation encountered — callers
    /// that need per-instantiation precision should iterate [`TirFn`] directly.
    pub fn span_types(&self) -> std::collections::HashMap<Span, Ty> {
        let mut map = std::collections::HashMap::new();
        for f in &self.fns {
            collect_block_spans(&f.body, &mut map);
        }
        map
    }
}

fn collect_block_spans(block: &TirBlock, map: &mut std::collections::HashMap<Span, Ty>) {
    for stmt in &block.stmts {
        collect_stmt_spans(stmt, map);
    }
}

fn collect_stmt_spans(stmt: &TirStmt, map: &mut std::collections::HashMap<Span, Ty>) {
    match stmt {
        TirStmt::Let { init, .. } => collect_expr_spans(init, map),
        TirStmt::Assign { value, .. } => collect_expr_spans(value, map),
        TirStmt::Return { value, .. } => {
            if let Some(e) = value {
                collect_expr_spans(e, map);
            }
        }
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_expr_spans(cond, map);
            collect_block_spans(then, map);
            if let Some(branch) = else_ {
                match branch {
                    TirElseBranch::Block(b) => collect_block_spans(b, map),
                    TirElseBranch::If(s) => collect_stmt_spans(s, map),
                }
            }
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_expr_spans(scrutinee, map);
            for arm in arms {
                collect_match_body_spans(&arm.body, map);
            }
        }
        TirStmt::For {
            iter,
            invariants,
            body,
            ..
        } => {
            collect_expr_spans(iter, map);
            for inv in invariants {
                collect_expr_spans(inv, map);
            }
            collect_block_spans(body, map);
        }
        TirStmt::While {
            cond,
            invariants,
            decreases,
            body,
            ..
        } => {
            collect_expr_spans(cond, map);
            for inv in invariants {
                collect_expr_spans(inv, map);
            }
            if let Some(d) = decreases {
                collect_expr_spans(d, map);
            }
            collect_block_spans(body, map);
        }
        TirStmt::Expr { expr, .. } => collect_expr_spans(expr, map),
    }
}

fn collect_expr_spans(expr: &TirExpr, map: &mut std::collections::HashMap<Span, Ty>) {
    map.insert(expr.span, expr.ty.clone());
    match &expr.kind {
        TirExprKind::Literal(_) | TirExprKind::Var(_) | TirExprKind::Quantifier(_) => {}
        TirExprKind::FieldAccess { expr, .. } => collect_expr_spans(expr, map),
        TirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr_spans(receiver, map);
            for a in args {
                collect_expr_spans(a, map);
            }
        }
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_expr_spans(a, map);
            }
            // type_args are TypeExpr (no TirExpr nodes) — nothing to collect.
        }
        TirExprKind::Unary { expr, .. } => collect_expr_spans(expr, map),
        TirExprKind::Binary { left, right, .. } => {
            collect_expr_spans(left, map);
            collect_expr_spans(right, map);
        }
        TirExprKind::If { cond, then, else_ } => {
            collect_expr_spans(cond, map);
            collect_block_spans(then, map);
            if let Some(e) = else_ {
                collect_expr_spans(e, map);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_expr_spans(scrutinee, map);
            for arm in arms {
                collect_match_body_spans(&arm.body, map);
            }
        }
        TirExprKind::Block(b) => collect_block_spans(b, map),
        TirExprKind::Lambda { body, .. } => collect_expr_spans(body, map),
        TirExprKind::Propagate(e)
        | TirExprKind::Consume(e)
        | TirExprKind::Relabel { expr: e, .. }
        | TirExprKind::Borrow { expr: e, .. } => collect_expr_spans(e, map),
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            for (_, e) in fields {
                collect_expr_spans(e, map);
            }
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_expr_spans(e, map);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_expr_spans(k, map);
                collect_expr_spans(v, map);
            }
        }
        TirExprKind::Select { arms } => {
            for arm in arms {
                collect_expr_spans(&arm.expr, map);
                collect_block_spans(&arm.body, map);
            }
        }
    }
}

fn collect_match_body_spans(body: &TirMatchBody, map: &mut std::collections::HashMap<Span, Ty>) {
    match body {
        TirMatchBody::Expr(e) => collect_expr_spans(e, map),
        TirMatchBody::Block(b) => collect_block_spans(b, map),
    }
}
