//! MVL Abstract Syntax Tree — typed node definitions for every grammar construct.
//!
//! Every node carries a [`Span`] (line, col, byte offset, length) so downstream
//! passes can produce precise diagnostics.  The tree is intentionally verbose:
//! no information is discarded from the source.

use crate::mvl::parser::lexer::Span;

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
    pub span: Span,
}

// ── Generic parameters ─────────────────────────────────────────────────────

/// A generic parameter in a type or function declaration.
///
/// - `Type("T")` — a regular type variable: `<T>`
/// - `Const("N", "Int")` — a const generic: `<const N: Int>`
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Struct(Vec<FieldDecl>),
    Enum(Vec<Variant>),
    /// Type alias (including refined alias `T where pred`).
    Alias(Box<TypeExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub mutable: bool,
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
    pub totality: Option<Totality>,
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub return_type: Box<TypeExpr>,
    /// Refinement on the return type: `-> Int where self > 0`
    pub return_refinement: Option<RefExpr>,
    /// Effect list: `! DB, Console`
    pub effects: Vec<String>,
    /// Where-clause constraints: `where T: Eq`
    pub constraints: Vec<Constraint>,
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
    pub mutable: bool,
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
    pub effects: Vec<String>,
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
    /// `&T` or `&mut T`
    Ref {
        mutable: bool,
        inner: Box<TypeExpr>,
        span: Span,
    },
    /// `Public<T>`, `Tainted<T>`, `Secret<T>`, `Clean<T>`
    Labeled {
        label: SecurityLabel,
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
        effects: Vec<String>,
        span: Span,
    },
    /// `(A, B, C)`
    Tuple { elems: Vec<TypeExpr>, span: Span },
    /// Integer literal used as a const generic argument: `Array<T, 16>`
    IntConst { value: i64, span: Span },
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
            | TypeExpr::IntConst { span, .. } => *span,
        }
    }
}

/// Security label keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLabel {
    Public,
    Tainted,
    Secret,
    Clean,
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
    Move {
        expr: Box<Expr>,
        span: Span,
    },
    Consume {
        expr: Box<Expr>,
        span: Span,
    },
    Declassify {
        expr: Box<Expr>,
        span: Span,
    },
    Sanitize {
        expr: Box<Expr>,
        span: Span,
    },
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
            | Expr::Move { span, .. }
            | Expr::Consume { span, .. }
            | Expr::Declassify { span, .. }
            | Expr::Sanitize { span, .. } => *span,
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
}

// ── Statements ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        mutable: bool,
        pattern: Pattern,
        ty: Option<TypeExpr>,
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
        body: Block,
        span: Span,
    },
    While {
        cond: Expr,
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
            mutable: false,
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
            mutable: false,
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
            totality: Some(Totality::Total),
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
        // Represents: `fn f(x: Tainted<String>) -> Clean<String>`
        let param = Param {
            capability: None,
            mutable: false,
            name: "x".into(),
            ty: TypeExpr::Labeled {
                label: SecurityLabel::Tainted,
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
            label: SecurityLabel::Clean,
            inner: Box::new(TypeExpr::Base {
                name: "String".into(),
                args: vec![],
                span: dummy(),
            }),
            span: dummy(),
        };

        assert!(
            matches!(
                &param.ty,
                TypeExpr::Labeled {
                    label: SecurityLabel::Tainted,
                    ..
                }
            ),
            "param type must be LabeledType(Tainted, String)"
        );
        assert!(
            matches!(
                &ret,
                TypeExpr::Labeled {
                    label: SecurityLabel::Clean,
                    ..
                }
            ),
            "return type must be LabeledType(Clean, String)"
        );
    }

    #[test]
    fn security_labels_all_variants() {
        // All four security labels are representable
        for label in [
            SecurityLabel::Public,
            SecurityLabel::Tainted,
            SecurityLabel::Secret,
            SecurityLabel::Clean,
        ] {
            let ty = TypeExpr::Labeled {
                label,
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
                mutable: false,
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
