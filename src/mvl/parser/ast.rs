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
    /// Items imported via brace group, e.g. `use models::{User, Req}` → `["User", "Req"]`.
    /// Empty when the import has no brace group.
    pub items: Vec<String>,
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
    pub requires: Vec<Expr>,
    /// Postconditions: `ensures pred` — checked at return points.
    /// The special identifier `result` refers to the return value.
    pub ensures: Vec<Expr>,
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
    /// Optional library names to link against: `extern "C" link("m") { … }`.
    /// Rust backend emits `#[link(name = "...")]`; LLVM backend notes them as comments.
    pub link_libs: Vec<String>,
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

/// Mailbox overflow policy for bounded actor mailboxes (#1127).
#[derive(Debug, Clone, PartialEq)]
pub enum MailboxPolicy {
    /// Block the sender until space is available — no message loss.
    Block,
    /// Drop the newest message when the mailbox is full (fire-and-forget).
    DropNewest,
}

/// Mailbox configuration attached to an actor declaration via `with mailbox(...)` (#1127).
///
/// Syntax:
/// - `with mailbox(256)` — bounded, default policy (DropNewest)
/// - `with mailbox(256, block)` — bounded, blocking sender
/// - `with mailbox(256, drop_newest)` — bounded, drop newest on full
/// - `with mailbox(unbounded)` — unbounded, never drops
#[derive(Debug, Clone, PartialEq)]
pub enum MailboxConfig {
    /// Fixed-size mailbox.
    Bounded {
        capacity: u64,
        policy: MailboxPolicy,
    },
    /// Unbounded mailbox — grows without limit.
    Unbounded,
}

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
    /// Optional mailbox configuration. `None` = default (256 capacity, DropNewest policy).
    pub mailbox: Option<MailboxConfig>,
    /// Whether this actor traps exit signals from linked actors instead of dying.
    /// Parsed from `actor Foo traps_exit { ... }`. Phase 9, #1177.
    pub traps_exit: bool,
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
    /// `true` for `pub fn` / `pub test fn`, `false` for `fn` (private helper).
    pub is_public: bool,
    /// `true` for `pub test fn` — test-only, synchronous, can return non-Unit.
    pub is_test: bool,
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
    /// Whether every call site emits a runtime audit event (`relabel trust: T -> _ audit`).
    pub audit: bool,
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
    /// Boolean literal in a predicate, e.g. `result.alive == true` (#1540).
    Bool {
        value: bool,
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
    /// Bitwise binary operation in a predicate (#1928): `self.bit_and(15) == self` etc.
    /// Emitted to Z3 via QF-BV when any bitwise op appears in the predicate.
    BitwiseOp {
        op: BitwiseOp,
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
    /// Bitwise NOT in a predicate (#1928): `self.bit_not()`.
    BitwiseNot {
        inner: Box<RefExpr>,
        span: Span,
    },
    /// `forall x in [lo..hi]. pred` — bounded universal quantifier over an integer range
    /// (inclusive on both ends). Discharged by L3 expansion into a conjunction of
    /// instantiated bodies. See #1915.
    BoundedForall {
        var: String,
        lo: i64,
        hi: i64,
        body: Box<RefExpr>,
        span: Span,
    },
    /// `exists x in [lo..hi]. pred` — bounded existential quantifier over an integer range
    /// (inclusive on both ends). Discharged by L3 expansion into a disjunction of
    /// instantiated bodies. See #1915.
    BoundedExists {
        var: String,
        lo: i64,
        hi: i64,
        body: Box<RefExpr>,
        span: Span,
    },
    /// String-content predicate in a refinement: `self.contains("needle")`,
    /// `self.starts_with("prefix")`, `self.ends_with("suffix")`. The argument
    /// must be a compile-time string literal. Discharged by L1 (literal haystack)
    /// or L5 Z3 QF-S (symbolic). See #1919.
    StringOp {
        op: StringOp,
        receiver: Box<RefExpr>,
        literal: String,
        span: Span,
    },
    /// `list.get(index)` — array-index access in a refinement predicate (#1916).
    /// The index must be provably in-bounds from the surrounding context.
    /// Discharged via L2 (interval length propagation) or L5 (Z3 QF-Arrays `select`).
    ArrayGet {
        list: Box<RefExpr>,
        index: Box<RefExpr>,
        span: Span,
    },
    /// Regex-membership predicate in a refinement: `self.matches("pattern")`.
    /// The pattern must be a compile-time string literal from the admitted
    /// regular fragment (no backrefs, lookahead, recursion; see ADR-0057).
    /// Discharged by L1 (literal haystack), L2 (length extraction from
    /// anchored fixed-quantifier patterns), or L5 Z3 RegLan (symbolic).
    /// See #1921.
    RegexMatch {
        receiver: Box<RefExpr>,
        pattern: String,
        span: Span,
    },
    /// `abs(x)` or `x.abs()` — absolute value in a refinement predicate (#1936).
    /// Accepted in both function-call and method-call forms; desugared to this
    /// node so downstream layers are uniform.
    Abs {
        inner: Box<RefExpr>,
        span: Span,
    },
    /// `min(x, y)` or `x.min(y)` — minimum of two values (#1936).
    Min {
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
    /// `max(x, y)` or `x.max(y)` — maximum of two values (#1936).
    Max {
        left: Box<RefExpr>,
        right: Box<RefExpr>,
        span: Span,
    },
}

/// String-content operations supported in refinement predicates (#1919).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringOp {
    Contains,
    StartsWith,
    EndsWith,
}

/// Bitwise operators supported in refinement predicates (#1928).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitwiseOp {
    And, // bit_and / &
    Or,  // bit_or  / |
    Xor, // bit_xor / ^
    Shl, // shift_left  / <<
    Shr, // shift_right / >>
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
        /// Whether this call site emits a runtime audit event — set by the `audit`
        /// keyword on the expression, or propagated from a declaration-level `audit`.
        audit: bool,
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
    /// `forall`/`exists` quantifier — valid only in `requires`/`ensures`/`invariant`
    /// contract positions (#983).  Wraps the `RefExpr` produced by `parse_ref_expr()`.
    Quantifier(Box<RefExpr>, Span),
    /// `expr as Type` — checked cast to a refined type alias (#1324).
    ///
    /// For refined aliases like `type Port = Int where self >= 1 && self <= 65535`:
    /// - If the compiler can prove the refinement statically → no runtime check.
    /// - Otherwise → runtime assertion (panics if the refinement fails).
    As {
        expr: Box<Expr>,
        target: Box<TypeExpr>,
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
            | Expr::As { span, .. } => *span,
            Expr::Block(b) => b.span,
            Expr::Quantifier(_, s) => *s,
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

/// Broad classification of a [`BinaryOp`] by evaluation semantics.
///
/// Adding a new [`BinaryOp`] variant forces an update to the exhaustive `match` in
/// [`BinaryOp::category`], which then propagates correct classification to all call
/// sites that use `is_*` helpers — the compiler enforces no site is forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOpCategory {
    Arithmetic,   // Add, Sub, Mul, Div, Rem
    Comparison,   // Eq, Ne, Lt, Gt, Le, Ge
    Bitwise,      // BitAnd, BitOr, BitXor, Shl, Shr
    ShortCircuit, // And, Or — short-circuit boolean evaluation
}

impl BinaryOp {
    pub fn category(self) -> BinOpCategory {
        match self {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                BinOpCategory::Arithmetic
            }
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => BinOpCategory::Comparison,
            BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::Shl
            | BinaryOp::Shr => BinOpCategory::Bitwise,
            BinaryOp::And | BinaryOp::Or => BinOpCategory::ShortCircuit,
        }
    }

    pub fn is_short_circuit(self) -> bool {
        matches!(self.category(), BinOpCategory::ShortCircuit)
    }

    pub fn is_comparison(self) -> bool {
        matches!(self.category(), BinOpCategory::Comparison)
    }

    pub fn is_arithmetic(self) -> bool {
        matches!(self.category(), BinOpCategory::Arithmetic)
    }
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
        invariants: Vec<Expr>,
        body: Block,
        span: Span,
    },
    While {
        cond: Expr,
        /// Loop invariant predicates — `invariant pred` clauses (Phase 3, #621).
        invariants: Vec<Expr>,
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
        rest: bool,
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
    /// `P1 | P2 | …` — OR pattern; all alternatives must bind identical names/types
    Or {
        patterns: Vec<Pattern>,
        span: Span,
    },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s) | Pattern::None(s) => *s,
            Pattern::Ident(_, s) | Pattern::Literal(_, s) => *s,
            Pattern::TupleStruct { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Some { span, .. }
            | Pattern::Ok { span, .. }
            | Pattern::Err { span, .. }
            | Pattern::Or { span, .. } => *span,
        }
    }
}

// ── Contract predicate conversion ──────────────────────────────────────────

/// Convert a contract `Expr` to a `RefExpr` for static solver use.
///
/// Handles comparisons, logical ops, arithmetic, literals, identifiers,
/// field access, and the `x.len()` pattern.  Returns `None` for unsupported
/// shapes (e.g. arbitrary method calls), which callers treat as `RuntimeCheck`.
pub(crate) fn expr_to_ref_expr_ext(expr: &Expr, fallback_span: Span) -> Option<RefExpr> {
    match expr {
        Expr::Literal(Literal::Integer(n), span) => Some(RefExpr::Integer {
            value: *n,
            span: *span,
        }),
        Expr::Literal(Literal::Float(f), span) => Some(RefExpr::Float {
            value: *f,
            span: *span,
        }),
        // Bool literal — enables `result.alive == true` to convert to a RefExpr (#1540).
        Expr::Literal(Literal::Bool(b), span) => Some(RefExpr::Bool {
            value: *b,
            span: *span,
        }),
        Expr::Ident(name, span) => Some(RefExpr::Ident {
            name: name.clone(),
            span: *span,
        }),
        Expr::FieldAccess {
            expr: inner,
            field,
            span,
        } => {
            let inner_ref = expr_to_ref_expr_ext(inner, *span)?;
            Some(RefExpr::FieldAccess {
                object: Box::new(inner_ref),
                field: field.clone(),
                span: *span,
            })
        }
        // x.len() → RefExpr::Len
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if method == "len" && args.is_empty() => {
            if let Expr::Ident(name, _) = receiver.as_ref() {
                Some(RefExpr::Len {
                    ident: name.clone(),
                    span: *span,
                })
            } else {
                None
            }
        }
        // x.get(i) → RefExpr::ArrayGet (#1916)
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if method == "get" && args.len() == 1 => {
            let list = expr_to_ref_expr_ext(receiver, fallback_span)?;
            let index = expr_to_ref_expr_ext(&args[0], fallback_span)?;
            Some(RefExpr::ArrayGet {
                list: Box::new(list),
                index: Box::new(index),
                span: *span,
            })
        }
        // x.bit_and(y) / x.bit_or(y) / … → RefExpr::BitwiseOp (#1928)
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if matches!(
            method.as_str(),
            "bit_and" | "bit_or" | "bit_xor" | "shift_left" | "shift_right"
        ) && args.len() == 1 =>
        {
            let recv = expr_to_ref_expr_ext(receiver, fallback_span)?;
            let rhs = expr_to_ref_expr_ext(&args[0], *span)?;
            let op = match method.as_str() {
                "bit_and" => BitwiseOp::And,
                "bit_or" => BitwiseOp::Or,
                "bit_xor" => BitwiseOp::Xor,
                "shift_left" => BitwiseOp::Shl,
                "shift_right" => BitwiseOp::Shr,
                _ => unreachable!(),
            };
            Some(RefExpr::BitwiseOp {
                op,
                left: Box::new(recv),
                right: Box::new(rhs),
                span: *span,
            })
        }
        // x.bit_not() → RefExpr::BitwiseNot (#1928)
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if method == "bit_not" && args.is_empty() => {
            let recv = expr_to_ref_expr_ext(receiver, fallback_span)?;
            Some(RefExpr::BitwiseNot {
                inner: Box::new(recv),
                span: *span,
            })
        }
        // x.contains("lit") / x.starts_with("lit") / x.ends_with("lit") → RefExpr::StringOp (#1919)
        // The argument must be a compile-time string literal; non-literals are rejected here.
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if matches!(method.as_str(), "contains" | "starts_with" | "ends_with")
            && args.len() == 1 =>
        {
            let recv = expr_to_ref_expr_ext(receiver, fallback_span)?;
            let literal = match &args[0] {
                Expr::Literal(Literal::Str(s), _) => s.clone(),
                _ => return None, // non-literal argument — not in the admitted fragment
            };
            let op = match method.as_str() {
                "contains" => StringOp::Contains,
                "starts_with" => StringOp::StartsWith,
                "ends_with" => StringOp::EndsWith,
                _ => unreachable!(),
            };
            Some(RefExpr::StringOp {
                op,
                receiver: Box::new(recv),
                literal,
                span: *span,
            })
        }
        // x.matches("regex") → RefExpr::RegexMatch (#1921)
        // Pattern must be a compile-time string literal from the admitted regular
        // fragment; irregular features (backrefs, lookaround, recursion) are
        // rejected by the regex-fragment validator invoked in the where-clause
        // parser (see parser/types.rs). Here, non-literal args are silently
        // rejected (return None) as with StringOp.
        Expr::MethodCall {
            receiver,
            method,
            args,
            span,
        } if method.as_str() == "matches" && args.len() == 1 => {
            let recv = expr_to_ref_expr_ext(receiver, fallback_span)?;
            let pattern = match &args[0] {
                Expr::Literal(Literal::Str(s), _) => s.clone(),
                _ => return None,
            };
            // Validate the pattern here too — contract callers reach this path
            // without going through parse_ref_atom, so a bad pattern would slip
            // past parser validation otherwise.
            if crate::mvl::parser::regex_frag::validate(&pattern).is_err() {
                return None;
            }
            Some(RefExpr::RegexMatch {
                receiver: Box::new(recv),
                pattern,
                span: *span,
            })
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            // Map each BinaryOp directly to its RefExpr variant; the wildcard
            // tail covers operators that have no refinement-language form.
            let arith = |aop: ArithOp| -> Option<RefExpr> {
                let l = expr_to_ref_expr_ext(left, *span)?;
                let r = expr_to_ref_expr_ext(right, *span)?;
                Some(RefExpr::ArithOp {
                    op: aop,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                })
            };
            let cmp = |cop: CmpOp| -> Option<RefExpr> {
                let l = expr_to_ref_expr_ext(left, *span)?;
                let r = expr_to_ref_expr_ext(right, *span)?;
                Some(RefExpr::Compare {
                    op: cop,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                })
            };
            let logic = |lop: LogicOp| -> Option<RefExpr> {
                let l = expr_to_ref_expr_ext(left, *span)?;
                let r = expr_to_ref_expr_ext(right, *span)?;
                Some(RefExpr::LogicOp {
                    op: lop,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                })
            };
            let bitwise = |bop: BitwiseOp| -> Option<RefExpr> {
                let l = expr_to_ref_expr_ext(left, *span)?;
                let r = expr_to_ref_expr_ext(right, *span)?;
                Some(RefExpr::BitwiseOp {
                    op: bop,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                })
            };
            match op {
                BinaryOp::Add => arith(ArithOp::Add),
                BinaryOp::Sub => arith(ArithOp::Sub),
                BinaryOp::Mul => arith(ArithOp::Mul),
                BinaryOp::Div => arith(ArithOp::Div),
                BinaryOp::Rem => arith(ArithOp::Rem),
                BinaryOp::Eq => cmp(CmpOp::Eq),
                BinaryOp::Ne => cmp(CmpOp::Ne),
                BinaryOp::Lt => cmp(CmpOp::Lt),
                BinaryOp::Gt => cmp(CmpOp::Gt),
                BinaryOp::Le => cmp(CmpOp::Le),
                BinaryOp::Ge => cmp(CmpOp::Ge),
                BinaryOp::And => logic(LogicOp::And),
                BinaryOp::Or => logic(LogicOp::Or),
                BinaryOp::BitAnd => bitwise(BitwiseOp::And),
                BinaryOp::BitOr => bitwise(BitwiseOp::Or),
                BinaryOp::BitXor => bitwise(BitwiseOp::Xor),
                BinaryOp::Shl => bitwise(BitwiseOp::Shl),
                BinaryOp::Shr => bitwise(BitwiseOp::Shr),
            }
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr: inner,
            span,
        } => {
            let inner_ref = expr_to_ref_expr_ext(inner, *span)?;
            Some(RefExpr::ArithOp {
                op: ArithOp::Sub,
                left: Box::new(RefExpr::Integer {
                    value: 0,
                    span: *span,
                }),
                right: Box::new(inner_ref),
                span: *span,
            })
        }
        Expr::Unary {
            op: UnaryOp::Not,
            expr: inner,
            span,
        } => {
            let inner_ref = expr_to_ref_expr_ext(inner, *span)?;
            Some(RefExpr::Not {
                inner: Box::new(inner_ref),
                span: *span,
            })
        }
        Expr::Unary {
            op: UnaryOp::BitNot,
            expr: inner,
            span,
        } => {
            let inner_ref = expr_to_ref_expr_ext(inner, *span)?;
            Some(RefExpr::BitwiseNot {
                inner: Box::new(inner_ref),
                span: *span,
            })
        }
        // old(expr) in ensures — map to RefExpr::Old for runtime postcondition asserts.
        Expr::FnCall {
            name, args, span, ..
        } if name == "old" && args.len() == 1 => {
            let inner = expr_to_ref_expr_ext(&args[0], *span)?;
            Some(RefExpr::Old {
                inner: Box::new(inner),
                span: *span,
            })
        }
        // forall/exists quantifiers — pass through directly.
        Expr::Quantifier(ref_expr, _) => Some(*ref_expr.clone()),
        // old(expr) in ensures: for runtime purposes treat as the current value of expr.
        Expr::FnCall { name, args, .. } if name == "old" && args.len() == 1 => {
            expr_to_ref_expr_ext(&args[0], fallback_span)
        }
        _ => {
            let _ = fallback_span;
            None
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
