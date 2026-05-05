//! Internal type representation for the checker.
//!
//! [`Ty`] is the checker's resolved type — separate from the AST's [`TypeExpr`]
//! which is an unresolved syntactic form.  Conversion happens in [`resolve`].

use crate::mvl::checker::ifc;
use crate::mvl::parser::ast::{SecurityLabel, TypeExpr};

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
    UByte,
    UInt,
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
    /// Fixed-size array: element type + compile-time size constant.
    Array(Box<Ty>, u64),
    // Refined type wrapper: underlying type + predicate source text
    Refined(Box<Ty>, String),
    // Security label wrapper: label + inner type (Requirement 11)
    Labeled(SecurityLabel, Box<Ty>),
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
            Ty::UByte => "UByte".to_string(),
            Ty::UInt => "UInt".to_string(),
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
            Ty::Array(inner, size) => format!("Array<{}, {}>", inner.display(), size),
            Ty::Refined(inner, _pred) => inner.display(),
            Ty::Labeled(label, inner) => {
                format!("{}<{}>", ifc::label_name(*label), inner.display())
            }
            Ty::Unknown => "<unknown>".to_string(),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self.unlabeled(),
            Ty::Int | Ty::Float | Ty::Byte | Ty::UByte | Ty::UInt
        )
    }

    /// True if this type has unsigned integer semantics (affects shift direction).
    pub fn is_unsigned_int(&self) -> bool {
        matches!(self.unlabeled(), Ty::UByte | Ty::UInt)
    }

    pub fn is_bool(&self) -> bool {
        matches!(self.unlabeled(), Ty::Bool)
    }

    /// Strip Refined wrappers to get the base type (labels are preserved).
    pub fn base(&self) -> &Ty {
        match self {
            Ty::Refined(inner, _) => inner.base(),
            other => other,
        }
    }

    /// Strip both Refined and Labeled wrappers for structural operations.
    pub fn unlabeled(&self) -> &Ty {
        match self {
            Ty::Refined(inner, _) | Ty::Labeled(_, inner) => inner.unlabeled(),
            other => other,
        }
    }

    /// True if this type is or wraps an `Option`.
    pub fn is_option(&self) -> bool {
        matches!(self.unlabeled(), Ty::Option(_))
    }

    /// True if this type is or wraps a `Result`.
    pub fn is_result(&self) -> bool {
        matches!(self.unlabeled(), Ty::Result(_, _))
    }

    /// True if `?` can be applied (Option or Result).
    pub fn is_propagatable(&self) -> bool {
        self.is_option() || self.is_result()
    }

    /// Return the success type after unwrapping Result/Option for `?`.
    pub fn propagate_inner(&self) -> Ty {
        match self.unlabeled() {
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
            "UByte" => Ty::UByte,
            "UInt" => Ty::UInt,
            "Unit" => Ty::Unit,
            "Never" => Ty::Never,
            "List" if args.len() == 1 => Ty::List(Box::new(resolve(&args[0]))),
            "Array" if args.len() == 2 => {
                let elem = resolve(&args[0]);
                let size = match &args[1] {
                    TypeExpr::IntConst { value, .. } if *value >= 0 => *value as u64,
                    // Negative literal is invalid — propagate Unknown so the caller gets an error.
                    TypeExpr::IntConst { .. } => return Ty::Unknown,
                    // Type variable (e.g. `Array<T, N>` in a generic function): size not yet
                    // known at resolve-time. Phase-1 limitation: treat as unresolved.
                    // TODO(phase-2): track const-generic variables in the checker environment.
                    _ => 0,
                };
                Ty::Array(Box::new(elem), size)
            }
            // Array with wrong argument count — always an error.
            "Array" => Ty::Unknown,
            _ => Ty::Named(name.clone(), args.iter().map(resolve).collect()),
        },
        TypeExpr::Option { inner, .. } => Ty::Option(Box::new(resolve(inner))),
        TypeExpr::Result { ok, err, .. } => {
            Ty::Result(Box::new(resolve(ok)), Box::new(resolve(err)))
        }
        TypeExpr::Ref { mutable, inner, .. } => Ty::Ref(*mutable, Box::new(resolve(inner))),
        // Security labels are preserved as Ty::Labeled wrappers (Requirement 11)
        TypeExpr::Labeled { label, inner, .. } => Ty::Labeled(*label, Box::new(resolve(inner))),
        TypeExpr::Refined { inner, pred, .. } => {
            Ty::Refined(Box::new(resolve(inner)), format!("{pred:?}"))
        }
        TypeExpr::Fn { params, ret, .. } => {
            Ty::Fn(params.iter().map(resolve).collect(), Box::new(resolve(ret)))
        }
        TypeExpr::Tuple { elems, .. } => Ty::Tuple(elems.iter().map(resolve).collect()),
        // Integer const generics are not standalone types — they only appear inside Array<T, N>
        TypeExpr::IntConst { .. } => Ty::Unknown,
    }
}

/// Structural compatibility with label-aware flow checking.
///
/// `Unknown` unifies with anything at any depth (error recovery).
/// Security labels enforce the IFC lattice: upward flows are allowed,
/// downward flows are rejected (require explicit declassify/sanitize).
///
/// # INVARIANT
/// `a` = expected type, `b` = found type. Reversing the arguments silently
/// inverts security enforcement (can_flow is asymmetric). All call sites MUST
/// maintain this order.
pub fn types_compatible(a: &Ty, b: &Ty) -> bool {
    // Strip Refined wrappers but preserve Labeled
    let a = a.base();
    let b = b.base();
    if matches!(a, Ty::Unknown) || matches!(b, Ty::Unknown) {
        return true;
    }
    match (a, b) {
        // Both labeled: enforce lattice flow from b (found) to a (expected),
        // then check structural compatibility of the inner types.
        (Ty::Labeled(la, ia), Ty::Labeled(lb, ib)) => {
            ifc::can_flow(*lb, *la) && types_compatible(ia, ib)
        }
        // Expected labeled, found unlabeled: check inner of expected vs found.
        // Unlabeled data may be assigned to a labeled context (treated as Public).
        (Ty::Labeled(_, ia), _) => types_compatible(ia, b),
        // Expected unlabeled (treated as Public context), found labeled:
        // Only allow if the found label can flow to Public — i.e., found = Public<T>.
        // Secret<T>, Tainted<T>, Clean<T> must NOT flow silently to an unlabeled context;
        // that would make any untyped parameter an implicit declassification sink.
        (_, Ty::Labeled(lb, ib)) => {
            ifc::can_flow(*lb, SecurityLabel::Public) && types_compatible(a, ib)
        }
        // Structural cases
        (Ty::Option(ai), Ty::Option(bi)) => types_compatible(ai, bi),
        (Ty::Result(ao, ae), Ty::Result(bo, be)) => {
            types_compatible(ao, bo) && types_compatible(ae, be)
        }
        (Ty::List(ai), Ty::List(bi)) => types_compatible(ai, bi),
        (Ty::Array(ae, an), Ty::Array(be, bn)) => an == bn && types_compatible(ae, be),
        (Ty::Ref(am, ai), Ty::Ref(bm, bi)) => am == bm && types_compatible(ai, bi),
        (Ty::Tuple(aes), Ty::Tuple(bes)) => {
            aes.len() == bes.len()
                && aes
                    .iter()
                    .zip(bes.iter())
                    .all(|(x, y)| types_compatible(x, y))
        }
        (Ty::Named(an, aa), Ty::Named(bn, ba)) => {
            an == bn
                && aa.len() == ba.len()
                && aa
                    .iter()
                    .zip(ba.iter())
                    .all(|(x, y)| types_compatible(x, y))
        }
        _ => a == b,
    }
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
            ("UByte", Ty::UByte),
            ("UInt", Ty::UInt),
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
    fn resolve_labeled_preserves_label() {
        let expr = TypeExpr::Labeled {
            label: SecurityLabel::Secret,
            inner: Box::new(TypeExpr::Base {
                name: "String".to_string(),
                args: vec![],
                span: s(),
            }),
            span: s(),
        };
        assert_eq!(
            resolve(&expr),
            Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::String))
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

    #[test]
    fn types_compatible_upward_flow_allowed() {
        // Public<String> may flow to Secret<String> slot (upward)
        let public_str = Ty::Labeled(SecurityLabel::Public, Box::new(Ty::String));
        let secret_str = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::String));
        assert!(types_compatible(&secret_str, &public_str)); // expected=Secret, found=Public → ok
    }

    #[test]
    fn types_compatible_downward_flow_rejected() {
        // Secret<String> must NOT flow to Public<String> slot (downward)
        let public_str = Ty::Labeled(SecurityLabel::Public, Box::new(Ty::String));
        let secret_str = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::String));
        assert!(!types_compatible(&public_str, &secret_str)); // expected=Public, found=Secret → rejected
    }

    #[test]
    fn types_compatible_same_label() {
        let s1 = Ty::Labeled(SecurityLabel::Tainted, Box::new(Ty::String));
        let s2 = Ty::Labeled(SecurityLabel::Tainted, Box::new(Ty::String));
        assert!(types_compatible(&s1, &s2));
    }

    #[test]
    fn types_compatible_label_inner_mismatch() {
        // Secret<Int> vs Secret<String> — same label, different inner → incompatible
        let si = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::Int));
        let ss = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::String));
        assert!(!types_compatible(&si, &ss));
    }

    #[test]
    fn unlabeled_strips_label() {
        let labeled = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::Int));
        assert_eq!(labeled.unlabeled(), &Ty::Int);
    }

    #[test]
    fn is_numeric_through_label() {
        let labeled = Ty::Labeled(SecurityLabel::Secret, Box::new(Ty::Int));
        assert!(labeled.is_numeric());
    }

    #[test]
    fn is_bool_through_label() {
        let labeled = Ty::Labeled(SecurityLabel::Tainted, Box::new(Ty::Bool));
        assert!(labeled.is_bool());
    }

    // ── Const generics / Array<T, N> (Issue #68) ──────────────────────────

    #[test]
    fn resolve_array_with_int_const() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Array".to_string(),
            args: vec![
                TypeExpr::Base {
                    name: "Int".to_string(),
                    args: vec![],
                    span,
                },
                TypeExpr::IntConst { value: 16, span },
            ],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Array(Box::new(Ty::Int), 16));
    }

    #[test]
    fn array_display() {
        let ty = Ty::Array(Box::new(Ty::Int), 8);
        assert_eq!(ty.display(), "Array<Int, 8>");
    }

    #[test]
    fn array_same_size_compatible() {
        let a = Ty::Array(Box::new(Ty::Int), 16);
        let b = Ty::Array(Box::new(Ty::Int), 16);
        assert!(types_compatible(&a, &b));
    }

    #[test]
    fn array_different_size_incompatible() {
        let a = Ty::Array(Box::new(Ty::Int), 16);
        let b = Ty::Array(Box::new(Ty::Int), 32);
        assert!(!types_compatible(&a, &b));
    }

    #[test]
    fn array_different_elem_incompatible() {
        let a = Ty::Array(Box::new(Ty::Int), 16);
        let b = Ty::Array(Box::new(Ty::Bool), 16);
        assert!(!types_compatible(&a, &b));
    }

    #[test]
    fn resolve_array_negative_size_is_unknown() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Array".to_string(),
            args: vec![
                TypeExpr::Base {
                    name: "Int".to_string(),
                    args: vec![],
                    span,
                },
                TypeExpr::IntConst { value: -1, span },
            ],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Unknown);
    }

    #[test]
    fn resolve_array_wrong_arg_count_is_unknown() {
        let span = s();
        let one_arg = TypeExpr::Base {
            name: "Array".to_string(),
            args: vec![TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span,
            }],
            span,
        };
        assert_eq!(resolve(&one_arg), Ty::Unknown);
    }

    #[test]
    fn resolve_array_zero_size() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Array".to_string(),
            args: vec![
                TypeExpr::Base {
                    name: "Int".to_string(),
                    args: vec![],
                    span,
                },
                TypeExpr::IntConst { value: 0, span },
            ],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Array(Box::new(Ty::Int), 0));
    }

    #[test]
    fn ubyte_and_uint_are_unsigned() {
        assert!(Ty::UByte.is_unsigned_int());
        assert!(Ty::UInt.is_unsigned_int());
        assert!(!Ty::Byte.is_unsigned_int());
        assert!(!Ty::Int.is_unsigned_int());
    }

    #[test]
    fn ubyte_and_uint_are_numeric() {
        assert!(Ty::UByte.is_numeric());
        assert!(Ty::UInt.is_numeric());
    }

    #[test]
    fn ubyte_uint_display() {
        assert_eq!(Ty::UByte.display(), "UByte");
        assert_eq!(Ty::UInt.display(), "UInt");
    }

    #[test]
    fn ubyte_uint_incompatible_with_signed() {
        assert!(!types_compatible(&Ty::Int, &Ty::UInt));
        assert!(!types_compatible(&Ty::Byte, &Ty::UByte));
        assert!(!types_compatible(&Ty::UInt, &Ty::Int));
    }
}
