//! Internal type representation for the checker.
//!
//! [`Ty`] is the checker's resolved type — separate from the AST's [`TypeExpr`]
//! which is an unresolved syntactic form.  Conversion happens in [`resolve`].

use crate::mvl::parser::ast::TypeExpr;

/// Resolved type used throughout the checker.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    // Primitives
    Int,
    Float,
    String,
    Bool,
    Char,
    Byte,
    Unit,
    Never,
    // Compound
    Named(String, Vec<Ty>),
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    Ref(bool, Box<Ty>), // (mutable, inner)
    Fn(Vec<Ty>, Box<Ty>),
    Tuple(Vec<Ty>),
    List(Box<Ty>),
    // Refined type wrapper: underlying type + predicate source text
    Refined(Box<Ty>, String),
    // Placeholder for inference failures (error propagation)
    Unknown,
}

impl Ty {
    pub fn display(&self) -> String {
        match self {
            Ty::Int => "Int".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::String => "String".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::Char => "Char".to_string(),
            Ty::Byte => "Byte".to_string(),
            Ty::Unit => "Unit".to_string(),
            Ty::Never => "Never".to_string(),
            Ty::Named(name, args) if args.is_empty() => name.clone(),
            Ty::Named(name, args) => {
                let args_str = args.iter().map(Ty::display).collect::<Vec<_>>().join(", ");
                format!("{name}<{args_str}>")
            }
            Ty::Option(inner) => format!("Option<{}>", inner.display()),
            Ty::Result(ok, err) => format!("Result<{}, {}>", ok.display(), err.display()),
            Ty::Ref(true, inner) => format!("&mut {}", inner.display()),
            Ty::Ref(false, inner) => format!("&{}", inner.display()),
            Ty::Fn(params, ret) => {
                let params_str = params
                    .iter()
                    .map(Ty::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("fn({params_str}) -> {}", ret.display())
            }
            Ty::Tuple(elems) => {
                let elems_str = elems.iter().map(Ty::display).collect::<Vec<_>>().join(", ");
                format!("({elems_str})")
            }
            Ty::List(inner) => format!("List<{}>", inner.display()),
            Ty::Refined(inner, _pred) => inner.display(),
            Ty::Unknown => "<unknown>".to_string(),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    pub fn is_bool(&self) -> bool {
        matches!(self, Ty::Bool)
    }

    /// Strip refinement wrappers to get the base type.
    pub fn base(&self) -> &Ty {
        match self {
            Ty::Refined(inner, _) => inner.base(),
            other => other,
        }
    }

    /// True if this type is or wraps an `Option`.
    pub fn is_option(&self) -> bool {
        matches!(self.base(), Ty::Option(_))
    }

    /// True if this type is or wraps a `Result`.
    pub fn is_result(&self) -> bool {
        matches!(self.base(), Ty::Result(_, _))
    }

    /// True if `?` can be applied (Option or Result).
    pub fn is_propagatable(&self) -> bool {
        self.is_option() || self.is_result()
    }

    /// Return the success type after unwrapping Result/Option for `?`.
    pub fn propagate_inner(&self) -> Ty {
        match self.base() {
            Ty::Result(ok, _) => *ok.clone(),
            Ty::Option(inner) => *inner.clone(),
            _ => Ty::Unknown,
        }
    }
}

/// Convert an AST [`TypeExpr`] to a checker [`Ty`].
/// Unknown user-defined types become `Ty::Named(name, args)`.
pub fn resolve(expr: &TypeExpr) -> Ty {
    match expr {
        TypeExpr::Base { name, args, .. } => match name.as_str() {
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "String" => Ty::String,
            "Bool" => Ty::Bool,
            "Char" => Ty::Char,
            "Byte" => Ty::Byte,
            "Unit" => Ty::Unit,
            "Never" => Ty::Never,
            "List" if args.len() == 1 => Ty::List(Box::new(resolve(&args[0]))),
            _ => Ty::Named(name.clone(), args.iter().map(resolve).collect()),
        },
        TypeExpr::Option { inner, .. } => Ty::Option(Box::new(resolve(inner))),
        TypeExpr::Result { ok, err, .. } => {
            Ty::Result(Box::new(resolve(ok)), Box::new(resolve(err)))
        }
        TypeExpr::Ref { mutable, inner, .. } => Ty::Ref(*mutable, Box::new(resolve(inner))),
        // Security labels are transparent to the type checker in Phase 1
        TypeExpr::Labeled { inner, .. } => resolve(inner),
        TypeExpr::Refined { inner, pred, .. } => {
            Ty::Refined(Box::new(resolve(inner)), format!("{pred:?}"))
        }
        TypeExpr::Fn { params, ret, .. } => {
            Ty::Fn(params.iter().map(resolve).collect(), Box::new(resolve(ret)))
        }
        TypeExpr::Tuple { elems, .. } => Ty::Tuple(elems.iter().map(resolve).collect()),
    }
}

/// Structural equality ignoring refinement wrappers and Unknown.
/// `Unknown` unifies with anything (error recovery).
pub fn types_compatible(a: &Ty, b: &Ty) -> bool {
    if matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown) {
        return true;
    }
    a.base() == b.base()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::TypeExpr;
    use crate::mvl::parser::lexer::Span;

    fn s() -> Span {
        Span::default()
    }

    #[test]
    fn resolve_primitives() {
        for (name, expected) in [
            ("Int", Ty::Int),
            ("Float", Ty::Float),
            ("String", Ty::String),
            ("Bool", Ty::Bool),
            ("Char", Ty::Char),
            ("Byte", Ty::Byte),
        ] {
            let expr = TypeExpr::Base {
                name: name.to_string(),
                args: vec![],
                span: s(),
            };
            assert_eq!(resolve(&expr), expected);
        }
    }

    #[test]
    fn resolve_option() {
        let expr = TypeExpr::Option {
            inner: Box::new(TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span: s(),
            }),
            span: s(),
        };
        assert_eq!(resolve(&expr), Ty::Option(Box::new(Ty::Int)));
    }

    #[test]
    fn resolve_result() {
        let expr = TypeExpr::Result {
            ok: Box::new(TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span: s(),
            }),
            err: Box::new(TypeExpr::Base {
                name: "String".to_string(),
                args: vec![],
                span: s(),
            }),
            span: s(),
        };
        assert_eq!(
            resolve(&expr),
            Ty::Result(Box::new(Ty::Int), Box::new(Ty::String))
        );
    }

    #[test]
    fn types_compatible_with_unknown() {
        assert!(types_compatible(&Ty::Unknown, &Ty::Int));
        assert!(types_compatible(&Ty::Int, &Ty::Unknown));
        assert!(types_compatible(&Ty::Unknown, &Ty::Unknown));
    }

    #[test]
    fn types_compatible_same() {
        assert!(types_compatible(&Ty::Int, &Ty::Int));
        assert!(types_compatible(&Ty::Bool, &Ty::Bool));
    }

    #[test]
    fn types_compatible_different() {
        assert!(!types_compatible(&Ty::Int, &Ty::String));
        assert!(!types_compatible(&Ty::Bool, &Ty::Int));
    }
}
