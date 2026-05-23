// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL Abstract Syntax Tree — typed node definitions for every grammar construct.
//!
//! Every node carries a [`Span`] (line, col, byte offset, length) so downstream
//! passes can produce precise diagnostics.  The tree is intentionally verbose:
//! no information is discarded from the source.

use std::fmt;

use crate::mvl::parser::lexer::Span;

// ── Effect ─────────────────────────────────────────────────────────────────

/// A single effect in a function or method signature.
///
/// Example: `! FileRead + Net`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub name: String,
    pub span: Span,
}

impl Effect {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Effect {
            name: name.into(),
            span,
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

// ── EffectDecl ─────────────────────────────────────────────────────────────

/// `effect Name` or `effect Name > Parent + Parent …` (#852).
///
/// Declares a named effect, optionally subsuming one or more parent effects.
/// Base effects have an empty `subsumes` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDecl {
    pub name: String,
    /// Direct parents in the subsumption hierarchy (empty = base effect).
    pub subsumes: Vec<String>,
    pub span: Span,
}

// ── Program ────────────────────────────────────────────────────────────────

/// The root of every parse: a sequence of top-level declarations.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub declarations: Vec<Decl>,
    pub span: Span,
}

// ── Declarations ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Decl {
    Type(TypeDecl),
    Fn(FnDecl),
    Const(ConstDecl),
    /// `extern "rust" { … }` — foreign-function trust boundary (Req 11).
    Extern(ExternDecl),
    /// `use path::to::Item;` or `pub use path::to::Item;`
    Use(UseDecl),
    /// `impl Trait for Type { … }` — trait implementation.
    Impl(ImplDecl),
    /// `actor TypeName { fields* behaviors* }` — actor type declaration (Phase 8, #63).
    Actor(ActorDecl),
    /// `effect Name [> Parent [+ Parent]*]` — effect declaration (#852).
    EffectDecl(EffectDecl),
    /// `label Name` — user-defined IFC label declaration (#894).
    Label(LabelDecl),
    /// `relabel name: From -> To` — IFC relabel transition declaration (#894).
    Relabel(RelabelDecl),
}

impl Decl {
    pub fn span(&self) -> Span {
        match self {
            Decl::Type(d) => d.span,
            Decl::Fn(d) => d.span,
            Decl::Const(d) => d.span,
            Decl::Extern(d) => d.span,
            Decl::Use(d) => d.span,
            Decl::Impl(d) => d.span,
            Decl::Actor(d) => d.span,
            Decl::EffectDecl(d) => d.span,
            Decl::Label(d) => d.span,
            Decl::Relabel(d) => d.span,
        }
    }
}

// ── Use declaration ────────────────────────────────────────────────────────

/// `use path::to::Item;` or `pub use path::to::Item;`
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    /// Whether this is a re-export (`pub use …`)
    pub reexport: bool,
    /// Path segments, e.g. `["std", "io", "File"]`
    pub path: Vec<String>,
    /// True when the import has no brace group and `path.len() >= 2`.
    /// Signals a module-level import: `use std.json` creates the qualifier
    /// `json` so that `json.decode()` resolves as a qualified function call.
    pub module_only: bool,
    pub span: Span,
}

// ── Generic parameters ─────────────────────────────────────────────────────

/// A generic parameter in a type or function declaration.
///
/// - `Type("T")` — a regular type variable: `<T>`
/// - `Const("N", "Int")` — a const generic: `<const N: Int>`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericParam {
    Type(String),
    Const(String, String),
}

impl GenericParam {
    /// The parameter name (type variable or const name).
    pub fn name(&self) -> &str {
        match self {
            GenericParam::Type(n) | GenericParam::Const(n, _) => n,
        }
    }
}

// ── Type declaration ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    /// Whether the item is exported from this module (`pub`).
    pub visible: bool,
    pub name: String,
    /// Optional generic parameters: `type Map<K, V> = …` or `type Buf<T, const N: Int> = …`
    pub params: Vec<GenericParam>,
    pub body: TypeBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeBody {
    Struct {
        fields: Vec<FieldDecl>,
        /// Optional struct-level invariant: `with invariant <pred>` (Phase 6, #654).
        invariant: Option<RefExpr>,
    },
    Enum(Vec<Variant>),
    /// Type alias (including refined alias `T where pred`).
    Alias(Box<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeExpr,
    /// Inline refinement: `field: Int where self > 0`
    pub refinement: Option<RefExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: VariantFields,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VariantFields {
    Unit,
    Tuple(Vec<TypeExpr>),
    Struct(Vec<FieldDecl>),
}

// ── Function declaration ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    /// Whether the item is exported from this module (`pub`).
    pub visible: bool,
    /// Whether the function is a test (`test fn`).
    pub is_test: bool,
    /// Whether the function has a runtime-provided implementation (`builtin fn`).
    /// Builtin functions have no body; the compiler trusts the runtime to supply one.
    pub is_builtin: bool,
    /// Whether the function propagates security labels from arguments to return type (`transparent fn`).
    /// See ADR-0024: the checker joins argument labels and applies them to the return type.
    pub is_label_transparent: bool,
    pub totality: Option<Totality>,
    /// For `fn TypeName::method(self, ...)` declarations: the receiver type name (`"TypeName"`).
    /// `None` for ordinary top-level functions.
    pub receiver_type: Option<String>,
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_type: Box<TypeExpr>,
    /// Refinement on the return type: `-> Int where self > 0`
    pub return_refinement: Option<RefExpr>,
    /// Effect list: `! DB + Console` or `! FileRead("/path")`
    pub effects: Vec<Effect>,
    /// Where-clause constraints: `where T: Eq`
    pub constraints: Vec<Constraint>,
    /// Preconditions: `requires pred` — checked at call sites.
    /// The special identifier `self` in a pred refers to the argument value.
    /// Param names are normalised to `self` during contract checking.
    pub requires: Vec<RefExpr>,
    /// Postconditions: `ensures pred` — checked at return points.
    /// The special identifier `result` refers to the return value.
    pub ensures: Vec<RefExpr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Totality {
    Total,
    Partial,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub capability: Option<Capability>,
    pub name: String,
    pub ty: TypeExpr,
    pub refinement: Option<RefExpr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    Iso,
    Val,
    Ref,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: String,
    pub bound: String,
}

// ── Const / module declarations ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    /// Whether the item is exported from this module (`pub`).
    pub visible: bool,
    pub name: String,
    pub ty: TypeExpr,
    pub value: Expr,
    pub span: Span,
}

// ── Extern block ──────────────────────────────────────────────────────────

/// `extern "rust" { fn foo(…) -> T ! Effects; … }`
///
/// An explicit trust boundary: the compiler trusts the declared signatures
/// but does NOT verify the foreign implementation.  Each extern block is
/// counted by the assurance checker — minimising the extern surface
/// maximises the verifiable fraction of the program.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternDecl {
    /// The ABI string, e.g. `"rust"` or `"c"`.
    pub abi: String,
    /// The function declarations inside the block.
    pub fns: Vec<ExternFnDecl>,
    pub span: Span,
}

/// A single function signature inside an `extern` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternFnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Box<TypeExpr>,
    /// Declared effects — enforced on the MVL caller side.
    pub effects: Vec<Effect>,
    /// Optional totality annotation: `total fn foo(…)` inside an extern block.
    /// `None` means implicitly total (same as non-extern `fn` without keyword).
    pub totality: Option<Totality>,
    pub span: Span,
}

// ── Impl block ────────────────────────────────────────────────────────────

/// `impl TraitName for TypeName { fn … }` — a trait implementation block.
///
/// Phase 1 supports `Display` for user-defined string formatting.
/// Phase 2 adds `From<A>` for error-type conversion and user-defined coercions.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplDecl {
    /// The trait being implemented, e.g. `"Display"` or `"From"`.
    pub trait_name: String,
    /// Generic type arguments on the trait, e.g. `[IoError]` for `From<IoError>`.
    pub trait_type_args: Vec<TypeExpr>,
    /// The type implementing the trait, e.g. `"Point"`.
    pub type_name: String,
    /// Methods in the impl block.
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

// ── Actor declaration ──────────────────────────────────────────────────────

/// `actor TypeName { fields* behaviors* }` — an actor type declaration (Phase 8, #63).
///
/// Actors encapsulate private mutable state and expose it only through behaviors
/// (async message handlers).  All inter-actor communication uses sendable capabilities
/// (`iso`, `val`, `tag`).  See Spec 015 and ADR-0029.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorDecl {
    /// Whether the item is exported from this module (`pub`).
    pub visible: bool,
    pub name: String,
    /// Optional generic type parameters.
    pub type_params: Vec<GenericParam>,
    /// Private mutable state fields.
    pub fields: Vec<FieldDecl>,
    /// Methods: `pub fn` = async behavior, `fn` = private sync helper.
    pub methods: Vec<ActorMethod>,
    pub span: Span,
}

/// A method inside an actor declaration.
///
/// - `pub fn name(params) { … }` — public async behavior (message handler).
///   Parameters MUST carry sendable capabilities (`iso`, `val`, `tag`).
///   Return type defaults to `Unit` when omitted.
/// - `fn name(params) -> T { … }` — private synchronous helper (no async).
#[derive(Debug, Clone, PartialEq)]
pub struct ActorMethod {
    /// `true` for `pub fn` (async behavior), `false` for `fn` (private helper).
    pub is_public: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Box<TypeExpr>,
    pub effects: Vec<Effect>,
    pub body: Block,
    pub span: Span,
}

// ── Type expressions ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// Named type, optionally generic: `Int`, `List<T>`, `Result<A, B>`
    Base {
        name: String,
        args: Vec<TypeExpr>,
        span: Span,
    },
    /// `Option<T>`
    Option { inner: Box<TypeExpr>, span: Span },
    /// `Result<T, E>`
    Result {
        ok: Box<TypeExpr>,
        err: Box<TypeExpr>,
        span: Span,
    },
    /// `val T` (immutable reference) or `ref T` (mutable reference)
    Ref {
        mutable: bool,
        inner: Box<TypeExpr>,
        span: Span,
    },
    /// `Tainted[T]`, `Secret[T]`, or any user-declared label `L[T]` (#894).
    Labeled {
        label: String,
        inner: Box<TypeExpr>,
        span: Span,
    },
    /// `T where predicate`
    Refined {
        inner: Box<TypeExpr>,
        pred: RefExpr,
        span: Span,
    },
    /// `fn(A, B) -> C ! Effects`
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
        effects: Vec<Effect>,
        span: Span,
    },
    /// `(A, B, C)`
    Tuple { elems: Vec<TypeExpr>, span: Span },
    /// Integer literal used as a const generic argument: `Array<T, 16>`
    IntConst { value: i64, span: Span },
    /// Session type (Honda 1993): typed communication protocol.
    ///
    /// Describes the sequence of messages exchanged on a channel.
    /// Example: `!Request. ?Quote. +{ accept: !Payment. ?Receipt. end, reject: end }`
    Session { op: Box<SessionOp>, span: Span },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Base { span, .. }
            | TypeExpr::Option { span, .. }
            | TypeExpr::Result { span, .. }
            | TypeExpr::Ref { span, .. }
            | TypeExpr::Labeled { span, .. }
            | TypeExpr::Refined { span, .. }
            | TypeExpr::Fn { span, .. }
            | TypeExpr::Tuple { span, .. }
            | TypeExpr::IntConst { span, .. }
            | TypeExpr::Session { span, .. } => *span,
        }
    }
}

// ── Session types (Honda 1993) ──────────────────────────────────────────────

/// A session type operation describing one step (or branching point) in a
/// typed communication protocol.
///
/// The `.` combinator chains operations: `!Int. ?Bool. end` means
/// "send an Int, then receive a Bool, then the protocol terminates".
///
/// Duality: every protocol has two complementary sides.
/// `!T` pairs with `?T`, `+{...}` pairs with `&{...}`.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionOp {
    /// `!T. S` — send a value of type `T`, then continue as `S`.
    Send {
        msg: Box<TypeExpr>,
        cont: Box<SessionOp>,
        span: Span,
    },
    /// `?T. S` — receive a value of type `T`, then continue as `S`.
    Receive {
        msg: Box<TypeExpr>,
        cont: Box<SessionOp>,
        span: Span,
    },
    /// `+{ l1: S1, l2: S2, ... }` — internal choice: this side selects a branch.
    InternalChoice {
        branches: Vec<(String, SessionOp)>,
        span: Span,
    },
    /// `&{ l1: S1, l2: S2, ... }` — external choice: the other side selects.
    ExternalChoice {
        branches: Vec<(String, SessionOp)>,
        span: Span,
    },
    /// `end` — protocol terminated; the channel is closed.
    End { span: Span },
}

impl SessionOp {
    pub fn span(&self) -> Span {
        match self {
            SessionOp::Send { span, .. }
            | SessionOp::Receive { span, .. }
            | SessionOp::InternalChoice { span, .. }
            | SessionOp::ExternalChoice { span, .. }
            | SessionOp::End { span } => *span,
        }
    }
}

/// A user-declared IFC label (`label Tainted`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDecl {
    /// Whether the label is exported from this module (`pub`).
    pub visible: bool,
    pub name: String,
    pub span: Span,
}

/// A relabel transition declaration (`relabel trust: Tainted -> _`).
///
/// `from` / `to` use `None` to represent the bare type (`_`) and
/// `Some(name)` to represent a declared label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelabelDecl {
    /// Whether the transition is exported from this module (`pub`).
    pub visible: bool,
    pub name: String,
    /// Source label: `None` = bare `_`, `Some("Tainted")` = that label.
    pub from: Option<String>,
    /// Destination label: `None` = bare `_`, `Some("Secret")` = that label.
    pub to: Option<String>,
    pub span: Span,
}

// ── Refinement predicates ──────────────────────────────────────────────────

/// Predicate language for refinement types.
/// `self > 0`, `len(s) < 256`, `x >= 0 && x <= 100`
#[derive(Debug, Clone, PartialEq)]
pub enum RefExpr {
    LogicOp {
        op: LogicOp,
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
    Compare {
        op: CmpOp,
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
    ArithOp {
        op: ArithOp,
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
    Not {
        inner: Box<RefExpr>,
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    /// Field access in an invariant/refinement predicate: `self.size` (Phase 6, #654).
    FieldAccess {
        object: Box<RefExpr>,
        field: String,
        span: Span,
    },
    Integer {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    Len {
        ident: String,
        span: Span,
    },
    Grouped {
        inner: Box<RefExpr>,
        span: Span,
    },
    /// `old(expr)` — refers to the entry-time value of `expr` inside `ensures` (Phase 4, #627).
    Old {
        inner: Box<RefExpr>,
        span: Span,
    },
    /// `forall x: T, pred` — universal quantifier; ghost/contract context only (Phase 5, #628).
    Forall {
        var: String,
        ty: Box<TypeExpr>,
        body: Box<RefExpr>,
        span: Span,
    },
    /// `exists x: T, pred` — existential quantifier; ghost/contract context only (Phase 5, #628).
    Exists {
        var: String,
        ty: Box<TypeExpr>,
        body: Box<RefExpr>,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

impl CmpOp {
    /// Flip the operator to swap left/right operands (`<` ↔ `>`, `<=` ↔ `>=`).
    /// Used when normalising `n op self` patterns to `self flipped_op n`.
    pub fn flip(self) -> Self {
        match self {
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Ge => CmpOp::Le,
            CmpOp::Eq => CmpOp::Eq,
            CmpOp::Ne => CmpOp::Ne,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

// ── Expressions ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Literal(Literal, Span),
    Ident(String, Span),
    FieldAccess {
        expr: Box<Expr>,
        field: String,
        span: Span,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        span: Span,
    },
    FnCall {
        name: String,
        type_args: Vec<TypeExpr>,
        args: Vec<Expr>,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then: Block,
        else_: Option<Box<Expr>>,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Lambda {
        params: Vec<Param>,
        ret_type: Option<Box<TypeExpr>>,
        body: Box<Expr>,
        span: Span,
    },
    Block(Block),
    /// `expr?` — propagate Result/Option failure
    Propagate {
        expr: Box<Expr>,
        span: Span,
    },
    /// `Name { field: value, … }`
    Construct {
        name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// `[e1, e2, …]`
    List {
        elems: Vec<Expr>,
        span: Span,
    },
    /// `{"k": v, …}` — map literal
    Map {
        pairs: Vec<(Expr, Expr)>,
        span: Span,
    },
    /// `{"a", "b", …}` — set literal (two or more elements, or trailing comma)
    Set {
        elems: Vec<Expr>,
        span: Span,
    },
    Consume {
        expr: Box<Expr>,
        span: Span,
    },
    /// `relabel name(expr, "audit-tag")` — IFC relabel expression (#894).
    ///
    /// Applies a declared relabel transition to `expr`.
    /// `name` must match a `relabel` declaration in scope.
    /// `tag` must be a string literal (audit trail).
    Relabel {
        name: String,
        expr: Box<Expr>,
        tag: String,
        span: Span,
    },
    /// Expression-level borrow: `val expr` (shared) or `ref expr` (mutable).
    Borrow {
        mutable: bool,
        expr: Box<Expr>,
        span: Span,
    },
    /// `actor ActorType { field: value, … }` — create an actor, returns an ActorRef (Phase 8, #63).
    Spawn {
        actor_type: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// `select { binding = expr => { body } … timeout(dur) => { body } }` (Phase 8, #69).
    ///
    /// Evaluates to `Unit`.  The first ready branch fires.  At most one
    /// `timeout(duration)` arm is allowed; it fires when no other arm is ready
    /// within the given duration.
    Select {
        arms: Vec<SelectArm>,
        span: Span,
    },
    /// `concurrently { … }` — structured concurrency scope (Phase 8, #69).
    ///
    /// Actors created inside cannot outlive this block.  When the block exits,
    /// all spawned actors are terminated.
    Concurrently {
        body: Block,
        span: Span,
    },
}

/// One arm of a `select` expression.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectArm {
    /// Optional result binding: `result = actor.behavior()`.
    pub binding: Option<String>,
    /// The actor behavior call expression, or the duration for `timeout(dur)`.
    pub expr: Box<Expr>,
    /// `true` when this is the `timeout(duration)` arm.
    pub is_timeout: bool,
    /// Handler block executed when this arm fires.
    pub body: Block,
    pub span: Span,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal(_, s) => *s,
            Expr::Ident(_, s) => *s,
            Expr::FieldAccess { span, .. }
            | Expr::MethodCall { span, .. }
            | Expr::FnCall { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Propagate { span, .. }
            | Expr::Construct { span, .. }
            | Expr::List { span, .. }
            | Expr::Map { span, .. }
            | Expr::Set { span, .. }
            | Expr::Consume { span, .. }
            | Expr::Relabel { span, .. }
            | Expr::Borrow { span, .. }
            | Expr::Spawn { span, .. }
            | Expr::Select { span, .. }
            | Expr::Concurrently { span, .. } => *span,
            Expr::Block(b) => b.span,
        }
    }
}

/// Fix #14: `Literal` intentionally derives only `PartialEq`, not `Eq`, because
/// `Float(f64)` does not have a total equality relation (NaN != NaN).
/// Use `PartialEq` for comparisons, or match on the variant to handle floats.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Integer(i64),
    Float(f64),
    Str(String),
    Char(char),
    Bool(bool),
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Deref,
    BitNot, // ~
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    // Bitwise
    BitAnd, // &
    BitOr,  // |
    BitXor, // ^
    Shl,    // <<
    Shr,    // >>
}

// ── Statements ─────────────────────────────────────────────────────────────

/// Binding kind for `let` statements.
#[derive(Debug, Clone, PartialEq)]
pub enum LetKind {
    /// Ordinary `let` binding emitted at runtime.
    /// Mutability is encoded in the type annotation (`ref T` = mutable, `val T` / bare type = immutable).
    Regular,
    /// `ghost let` — specification-only binding, erased before transpilation/codegen (Phase 4, #627).
    /// Ghost bindings are type-checked normally but never appear in emitted code.
    Ghost,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        kind: LetKind,
        pattern: Pattern,
        ty: TypeExpr,
        init: Expr,
        span: Span,
    },
    Assign {
        target: LValue,
        value: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    If {
        cond: Expr,
        then: Block,
        else_: Option<ElseBranch>,
        span: Span,
    },
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
        span: Span,
    },
    For {
        pattern: Pattern,
        iter: Expr,
        /// Loop invariant predicates — `invariant pred` clauses (Phase 3, #621).
        invariants: Vec<RefExpr>,
        body: Block,
        span: Span,
    },
    While {
        cond: Expr,
        /// Loop invariant predicates — `invariant pred` clauses (Phase 3, #621).
        invariants: Vec<RefExpr>,
        /// Optional termination measure — `decreases expr` clause (Phase 5, #628).
        decreases: Option<Box<Expr>>,
        body: Block,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::Match { span, .. }
            | Stmt::For { span, .. }
            | Stmt::While { span, .. }
            | Stmt::Expr { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElseBranch {
    Block(Block),
    /// `else if cond { … }`
    If(Box<Stmt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// Assignable location: a bare identifier or a field path.
#[derive(Debug, Clone, PartialEq)]
pub enum LValue {
    Ident(String, Span),
    Field {
        base: Box<LValue>,
        field: String,
        span: Span,
    },
}

// ── Match arms ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<RefExpr>,
    pub body: MatchBody,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchBody {
    Expr(Expr),
    Block(Block),
}

// ── Patterns ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard(Span),
    Ident(String, Span),
    Literal(Literal, Span),
    Tuple {
        elems: Vec<Pattern>,
        span: Span,
    },
    /// `Name(p1, p2)` — tuple-struct or enum-variant with positional fields
    TupleStruct {
        name: String,
        fields: Vec<Pattern>,
        span: Span,
    },
    /// `Name { field: pat, … }` — struct or enum-variant with named fields
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
        span: Span,
    },
    /// `Some(p)`
    Some {
        inner: Box<Pattern>,
        span: Span,
    },
    None(Span),
    /// `Ok(p)`
    Ok {
        inner: Box<Pattern>,
        span: Span,
    },
    /// `Err(p)`
    Err {
        inner: Box<Pattern>,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s) | Pattern::None(s) => *s,
            Pattern::Ident(_, s) | Pattern::Literal(_, s) => *s,
            Pattern::Tuple { span, .. }
            | Pattern::TupleStruct { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Some { span, .. }
            | Pattern::Ok { span, .. }
            | Pattern::Err { span, .. } => *span,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy() -> Span {
        Span::default()
    }

    // ── Requirement 2 / Scenario: AST node for function declaration ───────

    #[test]
    fn fn_decl_node_structure() {
        // Represents: `total fn add(a: Int, b: Int) -> Int { a + b }`
        let a_param = Param {
            capability: None,
            name: "a".into(),
            ty: TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            },
            refinement: None,
            span: dummy(),
        };
        let b_param = Param {
            capability: None,
            name: "b".into(),
            ty: TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            },
            refinement: None,
            span: dummy(),
        };
        let body = Block {
            stmts: vec![Stmt::Expr {
                expr: Expr::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(Expr::Ident("a".into(), dummy())),
                    right: Box::new(Expr::Ident("b".into(), dummy())),
                    span: dummy(),
                },
                span: dummy(),
            }],
            span: dummy(),
        };
        let decl = FnDecl {
            visible: false,
            is_test: false,
            is_builtin: false,
            is_label_transparent: false,
            totality: Some(Totality::Total),
            receiver_type: None,
            name: "add".into(),
            type_params: vec![],
            params: vec![a_param, b_param],
            return_type: Box::new(TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }),
            return_refinement: None,
            effects: vec![],
            constraints: vec![],
            requires: vec![],
            ensures: vec![],
            body,
            span: dummy(),
        };

        assert_eq!(decl.totality, Some(Totality::Total));
        assert_eq!(decl.name, "add");
        assert_eq!(decl.params.len(), 2);
        assert_eq!(decl.params[0].name, "a");
        assert_eq!(decl.params[1].name, "b");
        assert!(decl.effects.is_empty());
        assert_eq!(decl.body.stmts.len(), 1);
    }

    // ── Requirement 2 / Scenario: AST node with security labels ──────────

    #[test]
    fn labeled_type_node_structure() {
        // Represents: `fn f(x: Tainted<String>) -> Secret<String>` (post-#894)
        let param = Param {
            capability: None,
            name: "x".into(),
            ty: TypeExpr::Labeled {
                label: "Tainted".to_string(),
                inner: Box::new(TypeExpr::Base {
                    name: "String".into(),
                    args: vec![],
                    span: dummy(),
                }),
                span: dummy(),
            },
            refinement: None,
            span: dummy(),
        };
        let ret = TypeExpr::Labeled {
            label: "Secret".to_string(),
            inner: Box::new(TypeExpr::Base {
                name: "String".into(),
                args: vec![],
                span: dummy(),
            }),
            span: dummy(),
        };

        assert!(
            matches!(&param.ty, TypeExpr::Labeled { label, .. } if label == "Tainted"),
            "param type must be LabeledType(Tainted, String)"
        );
        assert!(
            matches!(&ret, TypeExpr::Labeled { label, .. } if label == "Secret"),
            "return type must be LabeledType(Secret, String)"
        );
    }

    #[test]
    fn security_labels_all_variants() {
        // User-defined labels are now plain strings (#894)
        for label in ["Tainted", "Secret", "CustomLabel"] {
            let ty = TypeExpr::Labeled {
                label: label.to_string(),
                inner: Box::new(TypeExpr::Base {
                    name: "T".into(),
                    args: vec![],
                    span: dummy(),
                }),
                span: dummy(),
            };
            assert!(matches!(ty, TypeExpr::Labeled { .. }));
        }
    }

    #[test]
    fn capabilities_all_variants() {
        for cap in [
            Capability::Iso,
            Capability::Val,
            Capability::Ref,
            Capability::Tag,
        ] {
            let p = Param {
                capability: Some(cap),
                name: "x".into(),
                ty: TypeExpr::Base {
                    name: "T".into(),
                    args: vec![],
                    span: dummy(),
                },
                refinement: None,
                span: dummy(),
            };
            assert!(p.capability.is_some());
        }
    }

    #[test]
    fn refinement_type_node_structure() {
        // `Int where self > 0`
        let ty = TypeExpr::Refined {
            inner: Box::new(TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: dummy(),
            }),
            pred: RefExpr::Compare {
                op: CmpOp::Gt,
                left: Box::new(RefExpr::Ident {
                    name: "self".into(),
                    span: dummy(),
                }),
                right: Box::new(RefExpr::Integer {
                    value: 0,
                    span: dummy(),
                }),
                span: dummy(),
            },
            span: dummy(),
        };
        assert!(matches!(ty, TypeExpr::Refined { .. }));
    }

    #[test]
    fn block_and_stmt_construction() {
        let block = Block {
            stmts: vec![Stmt::Return {
                value: None,
                span: dummy(),
            }],
            span: dummy(),
        };
        assert_eq!(block.stmts.len(), 1);
        assert!(matches!(block.stmts[0], Stmt::Return { value: None, .. }));
    }

    #[test]
    fn pattern_variants_constructible() {
        let _ = Pattern::Wildcard(dummy());
        let _ = Pattern::Ident("x".into(), dummy());
        let _ = Pattern::Literal(Literal::Integer(0), dummy());
        let _ = Pattern::None(dummy());
        let _ = Pattern::Some {
            inner: Box::new(Pattern::Wildcard(dummy())),
            span: dummy(),
        };
        let _ = Pattern::Ok {
            inner: Box::new(Pattern::Wildcard(dummy())),
            span: dummy(),
        };
        let _ = Pattern::Err {
            inner: Box::new(Pattern::Wildcard(dummy())),
            span: dummy(),
        };
    }
}
