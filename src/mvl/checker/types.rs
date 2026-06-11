// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Internal type representation for the checker.
//!
//! [`Ty`] is the checker's resolved type — separate from the AST's [`TypeExpr`]
//! which is an unresolved syntactic form.  Conversion happens in [`resolve`].

use crate::mvl::parser::ast::{Effect, RefExpr, SessionOp, Totality, TypeExpr};

/// Sentinel for an unresolved const-generic array size (e.g. `N` in `Array[T, N]`).
/// Treated as size-compatible with any concrete size in `types_compatible`.
pub const ARRAY_SIZE_UNKNOWN: u64 = u64::MAX;

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
    /// Function type: params, return, declared effects, totality.
    /// Effects are preserved from `TypeExpr::Fn` so HOF call sites can enforce Req 7/8.
    /// `totality` is `None` unless the HOF parameter was created from a named function
    /// reference (see `infer.rs`).
    Fn(Vec<Ty>, Box<Ty>, Vec<Effect>, Option<Totality>),
    Tuple(Vec<Ty>),
    List(Box<Ty>),
    /// Fixed-size array: element type + compile-time size constant.
    Array(Box<Ty>, u64),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    // Raw pointer for C FFI: `Ptr[T]`. Not tracked by MVL's ownership system.
    // `Ptr[Unit]` / `Ptr[Void]` is the MVL spelling of C `void*`.
    Ptr(Box<Ty>),
    // Refined type wrapper: underlying type + predicate AST node
    Refined(Box<Ty>, Box<RefExpr>),
    // Security label wrapper: label name + inner type (Requirement 11, #894)
    Labeled(String, Box<Ty>),
    // Session type: typed communication protocol (Honda 1993, Phase 8)
    Session(Box<SessionTy>),
    // Placeholder for inference failures (error propagation)
    Unknown,
}

// ── Session type representation (Honda 1993) ──────────────────────────────

/// Resolved session type used in the checker.
///
/// Describes the sequence of messages exchanged on a typed channel.
/// Every session type has a dual: `!T.S` is dual to `?T.S'`, `+{...}` is
/// dual to `&{...}`, and `end` is self-dual.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionTy {
    /// `!T. S` — send T, then continue as S.
    Send(Box<Ty>, Box<SessionTy>),
    /// `?T. S` — receive T, then continue as S.
    Receive(Box<Ty>, Box<SessionTy>),
    /// `+{ l1: S1, ... }` — internal choice: this side picks a branch.
    InternalChoice(Vec<(String, SessionTy)>),
    /// `&{ l1: S1, ... }` — external choice: the other side picks a branch.
    ExternalChoice(Vec<(String, SessionTy)>),
    /// `end` — protocol complete; channel closed.
    End,
}

impl SessionTy {
    /// Compute the dual of this session type.
    ///
    /// Rules: `!` ↔ `?`, `+` ↔ `&`, `end` ↔ `end`.
    /// Used to verify that two protocol participants are complementary.
    pub fn dual(&self) -> SessionTy {
        match self {
            SessionTy::Send(t, cont) => SessionTy::Receive(t.clone(), Box::new(cont.dual())),
            SessionTy::Receive(t, cont) => SessionTy::Send(t.clone(), Box::new(cont.dual())),
            SessionTy::InternalChoice(branches) => SessionTy::ExternalChoice(
                branches
                    .iter()
                    .map(|(l, s)| (l.clone(), s.dual()))
                    .collect(),
            ),
            SessionTy::ExternalChoice(branches) => SessionTy::InternalChoice(
                branches
                    .iter()
                    .map(|(l, s)| (l.clone(), s.dual()))
                    .collect(),
            ),
            SessionTy::End => SessionTy::End,
        }
    }

    /// True if `other` is the dual of `self`.
    pub fn is_dual_of(&self, other: &SessionTy) -> bool {
        &self.dual() == other
    }

    pub fn display(&self) -> String {
        match self {
            SessionTy::Send(t, cont) => format!("!{}. {}", t.display(), cont.display()),
            SessionTy::Receive(t, cont) => format!("?{}. {}", t.display(), cont.display()),
            SessionTy::InternalChoice(branches) => {
                let bs = branches
                    .iter()
                    .map(|(l, s)| format!("{l}: {}", s.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("+{{ {bs} }}")
            }
            SessionTy::ExternalChoice(branches) => {
                let bs = branches
                    .iter()
                    .map(|(l, s)| format!("{l}: {}", s.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("&{{ {bs} }}")
            }
            SessionTy::End => "end".to_string(),
        }
    }
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
            Ty::Ref(true, inner) => format!("ref {}", inner.display()),
            Ty::Ref(false, inner) => format!("val {}", inner.display()),
            Ty::Fn(params, ret, effects, _) => {
                let params_str = params
                    .iter()
                    .map(Ty::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                let effects_str = if effects.is_empty() {
                    String::new()
                } else {
                    let e = effects
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(" ! {e}")
                };
                format!("fn({params_str}) -> {}{effects_str}", ret.display())
            }
            Ty::Tuple(elems) => {
                let elems_str = elems.iter().map(Ty::display).collect::<Vec<_>>().join(", ");
                format!("({elems_str})")
            }
            Ty::List(inner) => format!("List<{}>", inner.display()),
            Ty::Array(inner, size) if *size == ARRAY_SIZE_UNKNOWN => {
                format!("Array<{}, _>", inner.display())
            }
            Ty::Array(inner, size) => format!("Array<{}, {}>", inner.display(), size),
            Ty::Map(k, v) => format!("Map<{}, {}>", k.display(), v.display()),
            Ty::Set(t) => format!("Set<{}>", t.display()),
            Ty::Ptr(inner) => format!("Ptr[{}]", inner.display()),
            Ty::Refined(inner, _pred) => inner.display(),
            Ty::Labeled(label, inner) => {
                format!("{}<{}>", label, inner.display())
            }
            Ty::Session(s) => s.display(),
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

    /// True if this is an integer type (valid target for bit operators).
    /// Excludes Float.
    pub fn is_integer(&self) -> bool {
        matches!(self.unlabeled(), Ty::Int | Ty::Byte | Ty::UByte | Ty::UInt)
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

    /// True if this type requires explicit `consume()` for ownership transfer.
    /// Only the primitively heap-allocated types that the checker knows are non-Copy.
    /// Named types (structs/enums) are excluded — use `is_linear_in_env` for recursive check.
    pub fn is_linear(&self) -> bool {
        matches!(
            self.unlabeled(),
            Ty::String | Ty::List(_) | Ty::Map(_, _) | Ty::Set(_)
        )
    }

    /// True if this type requires explicit `consume()` for ownership transfer,
    /// recursively checking named types (structs/enums) via the type environment.
    ///
    /// A named type is linear if any of its fields is linear (transitively).
    /// Uses `visited` internally to avoid infinite recursion on cyclic type defs.
    pub fn is_linear_in_env(
        &self,
        types: &std::collections::HashMap<String, super::context::TypeInfo>,
    ) -> bool {
        let mut visited = std::collections::HashSet::new();
        self.is_linear_rec(types, &mut visited)
    }

    fn is_linear_rec(
        &self,
        types: &std::collections::HashMap<String, super::context::TypeInfo>,
        visited: &mut std::collections::HashSet<String>,
    ) -> bool {
        match self.unlabeled() {
            Ty::String | Ty::List(_) | Ty::Map(_, _) | Ty::Set(_) => true,
            Ty::Named(name, _) => {
                if !visited.insert(name.clone()) {
                    return false; // cycle guard
                }
                if let Some(info) = types.get(name) {
                    match &info.body {
                        super::context::TypeBodyInfo::Struct { fields, .. } => {
                            fields.iter().any(|f| f.ty.is_linear_rec(types, visited))
                        }
                        super::context::TypeBodyInfo::Enum(variants) => {
                            variants.iter().any(|v| match &v.fields {
                                super::context::VariantFieldsInfo::Unit => false,
                                super::context::VariantFieldsInfo::Tuple(tys) => {
                                    tys.iter().any(|t| t.is_linear_rec(types, visited))
                                }
                                super::context::VariantFieldsInfo::Struct(fields) => {
                                    fields.iter().any(|f| f.ty.is_linear_rec(types, visited))
                                }
                            })
                        }
                        super::context::TypeBodyInfo::Alias(inner) => {
                            inner.is_linear_rec(types, visited)
                        }
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
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
                    // Type variable (e.g. `Array[T, N]` in a generic function): size is
                    // unresolved at resolve-time. Use ARRAY_SIZE_UNKNOWN so that
                    // types_compatible() treats it as size-flexible rather than silently
                    // producing Array[T, 0] (which would cause incorrect mismatch errors).
                    _ => ARRAY_SIZE_UNKNOWN,
                };
                Ty::Array(Box::new(elem), size)
            }
            // Array with wrong argument count — always an error.
            "Array" => Ty::Unknown,
            "Map" if args.len() == 2 => {
                Ty::Map(Box::new(resolve(&args[0])), Box::new(resolve(&args[1])))
            }
            "Map" => Ty::Unknown,
            "Set" if args.len() == 1 => Ty::Set(Box::new(resolve(&args[0]))),
            "Set" => Ty::Unknown,
            "Ptr" if args.len() == 1 => Ty::Ptr(Box::new(resolve(&args[0]))),
            "Ptr" => Ty::Unknown,
            // `Void` is an alias for Unit, valid in FFI pointer context: `Ptr[Void]` = C `void*`.
            "Void" => Ty::Unit,
            _ => Ty::Named(name.clone(), args.iter().map(resolve).collect()),
        },
        TypeExpr::Option { inner, .. } => Ty::Option(Box::new(resolve(inner))),
        TypeExpr::Result { ok, err, .. } => {
            Ty::Result(Box::new(resolve(ok)), Box::new(resolve(err)))
        }
        TypeExpr::Ref { mutable, inner, .. } => Ty::Ref(*mutable, Box::new(resolve(inner))),
        // Security labels are preserved as Ty::Labeled wrappers (Requirement 11, #894)
        TypeExpr::Labeled { label, inner, .. } => {
            Ty::Labeled(label.clone(), Box::new(resolve(inner)))
        }
        TypeExpr::Refined { inner, pred, .. } => {
            Ty::Refined(Box::new(resolve(inner)), Box::new(pred.clone()))
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => Ty::Fn(
            params.iter().map(resolve).collect(),
            Box::new(resolve(ret)),
            effects.clone(),
            None, // Totality not expressible in TypeExpr::Fn; use None (Phase 2: add parser support)
        ),
        TypeExpr::Tuple { elems, .. } => Ty::Tuple(elems.iter().map(resolve).collect()),
        // Integer const generics are not standalone types — they only appear inside Array<T, N>
        TypeExpr::IntConst { .. } => Ty::Unknown,
        // Session types: resolve the AST SessionOp tree into a SessionTy tree.
        TypeExpr::Session { op, .. } => Ty::Session(Box::new(resolve_session_op(op))),
    }
}

/// Convert an AST [`SessionOp`] to a checker [`SessionTy`].
pub fn resolve_session_op(op: &SessionOp) -> SessionTy {
    match op {
        SessionOp::Send { msg, cont, .. } => {
            SessionTy::Send(Box::new(resolve(msg)), Box::new(resolve_session_op(cont)))
        }
        SessionOp::Receive { msg, cont, .. } => {
            SessionTy::Receive(Box::new(resolve(msg)), Box::new(resolve_session_op(cont)))
        }
        SessionOp::InternalChoice { branches, .. } => SessionTy::InternalChoice(
            branches
                .iter()
                .map(|(l, s)| (l.clone(), resolve_session_op(s)))
                .collect(),
        ),
        SessionOp::ExternalChoice { branches, .. } => SessionTy::ExternalChoice(
            branches
                .iter()
                .map(|(l, s)| (l.clone(), resolve_session_op(s)))
                .collect(),
        ),
        SessionOp::End { .. } => SessionTy::End,
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
    // Never is the bottom type: a diverging expression (panic, infinite loop) satisfies
    // any expected type because the expression never actually produces a value.
    if matches!(b, Ty::Never) {
        return true;
    }
    match (a, b) {
        // Both labeled: labels must match exactly — no lattice, no implicit flow (#894).
        // `Tainted[T]` is a distinct type from `Secret[T]` and from bare `T`.
        (Ty::Labeled(la, ia), Ty::Labeled(lb, ib)) => la == lb && types_compatible(ia, ib),
        // Expected labeled, found unlabeled: NOT compatible — labeled ≠ bare (#894).
        (Ty::Labeled(..), _) => false,
        // Expected unlabeled, found labeled: NOT compatible — relabel() required (#894).
        (_, Ty::Labeled(..)) => false,
        // Structural cases
        (Ty::Option(ai), Ty::Option(bi)) => types_compatible(ai, bi),
        (Ty::Result(ao, ae), Ty::Result(bo, be)) => {
            types_compatible(ao, bo) && types_compatible(ae, be)
        }
        (Ty::List(ai), Ty::List(bi)) => types_compatible(ai, bi),
        (Ty::Array(ae, an), Ty::Array(be, bn)) => {
            // ARRAY_SIZE_UNKNOWN means the size is an unresolved const-generic variable;
            // treat it as compatible with any concrete size (both element types must match).
            (*an == *bn || *an == ARRAY_SIZE_UNKNOWN || *bn == ARRAY_SIZE_UNKNOWN)
                && types_compatible(ae, be)
        }
        (Ty::Map(ak, av), Ty::Map(bk, bv)) => types_compatible(ak, bk) && types_compatible(av, bv),
        (Ty::Set(ai), Ty::Set(bi)) => types_compatible(ai, bi),
        (Ty::Ref(am, ai), Ty::Ref(bm, bi)) => am == bm && types_compatible(ai, bi),
        // A plain value T is compatible where val/ref T is expected (env types are stripped).
        (Ty::Ref(_, ai), _) => types_compatible(ai, b),
        (Ty::Tuple(aes), Ty::Tuple(bes)) => {
            aes.len() == bes.len()
                && aes
                    .iter()
                    .zip(bes.iter())
                    .all(|(x, y)| types_compatible(x, y))
        }
        // Positional[T] is a CLI annotation — transparent in the type system.
        // Strip it in both directions so `Positional[Int]` satisfies an `Int` slot and
        // an `Int` literal satisfies a `Positional[Int]` field (e.g. unwrap_or default).
        (Ty::Named(an, aa), _) if an == "Positional" && aa.len() == 1 => {
            types_compatible(&aa[0], b)
        }
        (_, Ty::Named(bn, ba)) if bn == "Positional" && ba.len() == 1 => {
            types_compatible(a, &ba[0])
        }
        (Ty::Named(an, aa), Ty::Named(bn, ba)) => {
            an == bn
                && aa.len() == ba.len()
                && aa
                    .iter()
                    .zip(ba.iter())
                    .all(|(x, y)| types_compatible(x, y))
        }
        // Session types are compatible when structurally equal.
        // Duality is checked separately (both sides must be declared as duals).
        (Ty::Session(sa), Ty::Session(sb)) => session_types_compatible(sa, sb),
        // Function types: compare structurally. Effects are compared by name only,
        // not span — two identical effect names parsed from different locations
        // must be treated as equal (#954).
        (Ty::Fn(ap, ar, ae, at), Ty::Fn(bp, br, be, bt)) => {
            ap.len() == bp.len()
                && ap
                    .iter()
                    .zip(bp.iter())
                    .all(|(x, y)| types_compatible(x, y))
                && types_compatible(ar, br)
                && effects_name_eq(ae, be)
                && at == bt
        }
        _ => a == b,
    }
}

/// Compare two effect lists by name only, order-insensitive.
/// Effect spans differ depending on parse site but are semantically irrelevant (#954).
fn effects_name_eq(a: &[Effect], b: &[Effect]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    if a.is_empty() {
        return true;
    }
    let mut a_names: Vec<&str> = a.iter().map(|e| e.name.as_str()).collect();
    let mut b_names: Vec<&str> = b.iter().map(|e| e.name.as_str()).collect();
    a_names.sort_unstable();
    b_names.sort_unstable();
    a_names == b_names
}

/// Structural compatibility for session types.
/// Two session types are compatible when they have the same structure and payload types.
pub fn session_types_compatible(a: &SessionTy, b: &SessionTy) -> bool {
    match (a, b) {
        (SessionTy::Send(ta, ca), SessionTy::Send(tb, cb)) => {
            types_compatible(ta, tb) && session_types_compatible(ca, cb)
        }
        (SessionTy::Receive(ta, ca), SessionTy::Receive(tb, cb)) => {
            types_compatible(ta, tb) && session_types_compatible(ca, cb)
        }
        (SessionTy::InternalChoice(bsa), SessionTy::InternalChoice(bsb)) => {
            session_branches_compatible(bsa, bsb)
        }
        (SessionTy::ExternalChoice(bsa), SessionTy::ExternalChoice(bsb)) => {
            session_branches_compatible(bsa, bsb)
        }
        (SessionTy::End, SessionTy::End) => true,
        _ => false,
    }
}

fn session_branches_compatible(a: &[(String, SessionTy)], b: &[(String, SessionTy)]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().all(|(la, sa)| {
        b.iter()
            .find(|(lb, _)| lb == la)
            .is_some_and(|(_, sb)| session_types_compatible(sa, sb))
    })
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
            label: "Secret".to_string(),
            inner: Box::new(TypeExpr::Base {
                name: "String".to_string(),
                args: vec![],
                span: s(),
            }),
            span: s(),
        };
        assert_eq!(
            resolve(&expr),
            Ty::Labeled("Secret".to_string(), Box::new(Ty::String))
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
    fn types_compatible_different_labels_rejected() {
        // Tainted[String] ≠ Secret[String] — no lattice, exact match required (#894)
        let tainted_str = Ty::Labeled("Tainted".to_string(), Box::new(Ty::String));
        let secret_str = Ty::Labeled("Secret".to_string(), Box::new(Ty::String));
        assert!(!types_compatible(&secret_str, &tainted_str));
        assert!(!types_compatible(&tainted_str, &secret_str));
    }

    #[test]
    fn types_compatible_labeled_vs_bare_rejected() {
        // Tainted[String] ≠ String — labeled ≠ bare (#894)
        let tainted_str = Ty::Labeled("Tainted".to_string(), Box::new(Ty::String));
        assert!(!types_compatible(&Ty::String, &tainted_str));
        assert!(!types_compatible(&tainted_str, &Ty::String));
    }

    #[test]
    fn types_compatible_same_label() {
        let s1 = Ty::Labeled("Tainted".to_string(), Box::new(Ty::String));
        let s2 = Ty::Labeled("Tainted".to_string(), Box::new(Ty::String));
        assert!(types_compatible(&s1, &s2));
    }

    #[test]
    fn types_compatible_label_inner_mismatch() {
        // Secret<Int> vs Secret<String> — same label, different inner → incompatible
        let si = Ty::Labeled("Secret".to_string(), Box::new(Ty::Int));
        let ss = Ty::Labeled("Secret".to_string(), Box::new(Ty::String));
        assert!(!types_compatible(&si, &ss));
    }

    #[test]
    fn unlabeled_strips_label() {
        let labeled = Ty::Labeled("Secret".to_string(), Box::new(Ty::Int));
        assert_eq!(labeled.unlabeled(), &Ty::Int);
    }

    #[test]
    fn is_numeric_through_label() {
        let labeled = Ty::Labeled("Secret".to_string(), Box::new(Ty::Int));
        assert!(labeled.is_numeric());
    }

    #[test]
    fn is_bool_through_label() {
        let labeled = Ty::Labeled("Tainted".to_string(), Box::new(Ty::Bool));
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
    fn resolve_map_two_args() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Map".to_string(),
            args: vec![
                TypeExpr::Base {
                    name: "String".to_string(),
                    args: vec![],
                    span,
                },
                TypeExpr::Base {
                    name: "Int".to_string(),
                    args: vec![],
                    span,
                },
            ],
            span,
        };
        assert_eq!(
            resolve(&expr),
            Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))
        );
    }

    #[test]
    fn resolve_map_wrong_arity_is_unknown() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Map".to_string(),
            args: vec![TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span,
            }],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Unknown);
    }

    #[test]
    fn resolve_set_one_arg() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Set".to_string(),
            args: vec![TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span,
            }],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Set(Box::new(Ty::Int)));
    }

    #[test]
    fn resolve_set_wrong_arity_is_unknown() {
        let span = s();
        let expr = TypeExpr::Base {
            name: "Set".to_string(),
            args: vec![],
            span,
        };
        assert_eq!(resolve(&expr), Ty::Unknown);
    }

    #[test]
    fn map_display() {
        let ty = Ty::Map(Box::new(Ty::String), Box::new(Ty::Int));
        assert_eq!(ty.display(), "Map<String, Int>");
    }

    #[test]
    fn set_display() {
        let ty = Ty::Set(Box::new(Ty::Int));
        assert_eq!(ty.display(), "Set<Int>");
    }

    #[test]
    fn map_types_compatible() {
        let a = Ty::Map(Box::new(Ty::String), Box::new(Ty::Int));
        let b = Ty::Map(Box::new(Ty::String), Box::new(Ty::Int));
        assert!(types_compatible(&a, &b));
        let c = Ty::Map(Box::new(Ty::Int), Box::new(Ty::Int));
        assert!(!types_compatible(&a, &c));
    }

    #[test]
    fn set_types_compatible() {
        let a = Ty::Set(Box::new(Ty::Int));
        let b = Ty::Set(Box::new(Ty::Int));
        assert!(types_compatible(&a, &b));
        let c = Ty::Set(Box::new(Ty::String));
        assert!(!types_compatible(&a, &c));
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

    // ── Session type tests ────────────────────────────────────────────────

    fn send_int_end() -> SessionTy {
        SessionTy::Send(Box::new(Ty::Int), Box::new(SessionTy::End))
    }

    fn recv_int_end() -> SessionTy {
        SessionTy::Receive(Box::new(Ty::Int), Box::new(SessionTy::End))
    }

    #[test]
    fn session_send_dual_is_receive() {
        let s = send_int_end();
        assert_eq!(s.dual(), recv_int_end());
    }

    #[test]
    fn session_receive_dual_is_send() {
        let s = recv_int_end();
        assert_eq!(s.dual(), send_int_end());
    }

    #[test]
    fn session_end_dual_is_end() {
        assert_eq!(SessionTy::End.dual(), SessionTy::End);
    }

    #[test]
    fn session_internal_choice_dual_is_external() {
        let s = SessionTy::InternalChoice(vec![
            ("ok".to_string(), SessionTy::End),
            ("err".to_string(), SessionTy::End),
        ]);
        let d = s.dual();
        assert!(matches!(d, SessionTy::ExternalChoice(_)));
    }

    #[test]
    fn session_external_choice_dual_is_internal() {
        let s = SessionTy::ExternalChoice(vec![("go".to_string(), SessionTy::End)]);
        assert!(matches!(s.dual(), SessionTy::InternalChoice(_)));
    }

    #[test]
    fn session_is_dual_of_roundtrip() {
        let ping = SessionTy::Send(
            Box::new(Ty::Int),
            Box::new(SessionTy::Receive(
                Box::new(Ty::Bool),
                Box::new(SessionTy::End),
            )),
        );
        let pong = ping.dual();
        assert!(ping.is_dual_of(&pong));
        assert!(pong.is_dual_of(&ping));
    }

    #[test]
    fn session_types_compatible_same() {
        assert!(session_types_compatible(&send_int_end(), &send_int_end()));
    }

    #[test]
    fn session_types_incompatible_different_dir() {
        assert!(!session_types_compatible(&send_int_end(), &recv_int_end()));
    }

    #[test]
    fn session_display_send() {
        let s = send_int_end();
        assert_eq!(s.display(), "!Int. end");
    }

    #[test]
    fn session_display_receive() {
        let s = recv_int_end();
        assert_eq!(s.display(), "?Int. end");
    }

    #[test]
    fn session_display_internal_choice() {
        let s = SessionTy::InternalChoice(vec![
            ("accept".to_string(), SessionTy::End),
            ("reject".to_string(), SessionTy::End),
        ]);
        let d = s.display();
        assert!(d.starts_with("+{"));
        assert!(d.contains("accept: end"));
        assert!(d.contains("reject: end"));
    }

    #[test]
    fn resolve_session_send_int_end() {
        use crate::mvl::parser::ast::SessionOp;
        let op = SessionOp::Send {
            msg: Box::new(TypeExpr::Base {
                name: "Int".to_string(),
                args: vec![],
                span: s(),
            }),
            cont: Box::new(SessionOp::End { span: s() }),
            span: s(),
        };
        let ty = resolve_session_op(&op);
        assert_eq!(ty, send_int_end());
    }
}
