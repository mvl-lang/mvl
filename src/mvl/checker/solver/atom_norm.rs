// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Atom normalization for the layered refinement solver (#1805).
//!
//! L2–L5 are gated on `Expr::Ident` (interval, Cooper, Z3) or `Expr::FnCall`
//! (symbolic).  Real MVL programs pass `Expr::FieldAccess` (`ball.vx`),
//! `Expr::MethodCall` (`xs.len()`), etc. as arguments, so every proof lands
//! at L1's structural-equality fallback.
//!
//! This pass rewrites non-arithmetic subtrees to fresh `Ident("__atom_N")`
//! atoms so L2–L5 receive a pure integer formula.  Two occurrences of the
//! same subtree — whether in the argument [`Expr`] or in a hypothesis
//! [`RefExpr`] — map to the same atom name, preserving atom identity across
//! the goal / hypothesis boundary.
//!
//! Design notes:
//!
//! - Arithmetic subtrees (`Ident`, `Literal`, `Unary`, `Binary`, `ArithOp`,
//!   comparison / logic operators) are recursed into so their leaves can
//!   still be normalized.  Their shape is preserved.
//! - `FnCall` is **not** normalized: L3 (symbolic path analysis) requires it
//!   as-is to unfold pure function bodies.  L3 continues to receive the
//!   original argument via a separate dispatch branch.
//! - Predicates in `var_refs` are rewritten with the *same* atom map so the
//!   hypothesis for `field.height` lines up with the goal atom for
//!   `field.height`.

use std::collections::HashMap;

use crate::mvl::parser::ast::{Expr, RefExpr};

use super::dummy_span;

/// Normalizer state.  One instance handles a single dispatch call and its
/// hypothesis universe.
pub(crate) struct AtomNormalizer {
    /// Canonical string form → synthesized atom identifier.
    map: HashMap<String, String>,
    next: usize,
}

impl AtomNormalizer {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            next: 0,
        }
    }

    fn atom_for(&mut self, key: String) -> String {
        if let Some(name) = self.map.get(&key) {
            return name.clone();
        }
        let name = format!("__atom_{}", self.next);
        self.next += 1;
        self.map.insert(key, name.clone());
        name
    }

    /// Rewrite an `Expr`.  `FieldAccess` and `MethodCall` subtrees become
    /// synthesized atoms; arithmetic operators recurse.
    pub fn rewrite_expr(&mut self, expr: &Expr) -> Expr {
        match expr {
            Expr::FieldAccess { .. } | Expr::MethodCall { .. } => {
                let key = canon_expr(expr);
                let name = self.atom_for(key);
                Expr::Ident(name, expr.span())
            }
            Expr::Unary {
                op,
                expr: inner,
                span,
            } => Expr::Unary {
                op: *op,
                expr: Box::new(self.rewrite_expr(inner)),
                span: *span,
            },
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => Expr::Binary {
                op: *op,
                left: Box::new(self.rewrite_expr(left)),
                right: Box::new(self.rewrite_expr(right)),
                span: *span,
            },
            // Everything else (Ident, Literal, FnCall, etc.) is left untouched.
            // FnCall in particular must reach Layer 3 verbatim.
            _ => expr.clone(),
        }
    }

    /// Rewrite a `RefExpr`.  `FieldAccess` and `Len` become atoms; logical /
    /// arithmetic / comparison operators recurse.
    pub fn rewrite_refexpr(&mut self, r: &RefExpr) -> RefExpr {
        match r {
            RefExpr::FieldAccess { span, .. } => {
                let key = canon_refexpr(r);
                let name = self.atom_for(key);
                RefExpr::Ident { name, span: *span }
            }
            RefExpr::Len { ident, span } => {
                let key = format!("{ident}.__len__");
                let name = self.atom_for(key);
                RefExpr::Ident { name, span: *span }
            }
            // list.get(i) — opaque array element; normalise to a fresh atom (#1916).
            RefExpr::ArrayGet { span, .. } => {
                let key = canon_refexpr(r);
                let name = self.atom_for(key);
                RefExpr::Ident { name, span: *span }
            }
            RefExpr::LogicOp {
                op,
                left,
                right,
                span,
            } => RefExpr::LogicOp {
                op: *op,
                left: Box::new(self.rewrite_refexpr(left)),
                right: Box::new(self.rewrite_refexpr(right)),
                span: *span,
            },
            RefExpr::Compare {
                op,
                left,
                right,
                span,
            } => RefExpr::Compare {
                op: *op,
                left: Box::new(self.rewrite_refexpr(left)),
                right: Box::new(self.rewrite_refexpr(right)),
                span: *span,
            },
            RefExpr::ArithOp {
                op,
                left,
                right,
                span,
            } => RefExpr::ArithOp {
                op: *op,
                left: Box::new(self.rewrite_refexpr(left)),
                right: Box::new(self.rewrite_refexpr(right)),
                span: *span,
            },
            RefExpr::Not { inner, span } => RefExpr::Not {
                inner: Box::new(self.rewrite_refexpr(inner)),
                span: *span,
            },
            RefExpr::Grouped { inner, span } => RefExpr::Grouped {
                inner: Box::new(self.rewrite_refexpr(inner)),
                span: *span,
            },
            RefExpr::Old { inner, span } => RefExpr::Old {
                inner: Box::new(self.rewrite_refexpr(inner)),
                span: *span,
            },
            // StringOp nodes are left as-is — they are opaque to the arithmetic
            // layers and are handled by L1 (literal strings) and L5 QF-S.
            // Idents, literals, and quantifiers are also left as-is.
            _ => r.clone(),
        }
    }

    /// Rewrite every hypothesis value in a `var_refs` map, then bridge each
    /// synthesized atom name to any hypothesis stored under its canonical key.
    ///
    /// The dispatch site injects hypotheses under keys like `"b.size"` when
    /// a struct-typed param `b: Box` has a field with a refinement.  After
    /// rewriting an argument `b.size` → `__atom_N`, this method looks up
    /// `var_refs["b.size"]` and, if present, mirrors its rewritten value
    /// under `var_refs["__atom_N"]` so L2/L4/L5 can find the hypothesis
    /// via the atom name.
    pub fn rewrite_var_refs(
        &mut self,
        var_refs: &HashMap<String, Option<RefExpr>>,
    ) -> HashMap<String, Option<RefExpr>> {
        let mut out: HashMap<String, Option<RefExpr>> = var_refs
            .iter()
            .map(|(k, v)| (k.clone(), v.as_ref().map(|r| self.rewrite_refexpr(r))))
            .collect();

        // Bridge: for each atom introduced during expr / pred rewriting,
        // copy any hypothesis stored under its canonical key into the atom
        // key.  Snapshot the map first because `rewrite_refexpr` mutates
        // `self.map` and we would otherwise iterate over live state.
        let atoms: Vec<(String, String)> = self
            .map
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (canon_key, atom_name) in atoms {
            if let Some(entry) = var_refs.get(&canon_key) {
                let rewritten = entry.as_ref().map(|r| self.rewrite_refexpr(r));
                out.insert(atom_name, rewritten);
            }
        }
        out
    }

    /// Number of distinct atoms synthesized.  Used only for diagnostics; the
    /// normalizer is a no-op when this stays zero.
    #[cfg(test)]
    pub fn atom_count(&self) -> usize {
        self.next
    }

    /// Reverse-lookup: given an atom name (`__atom_N`), return the canonical
    /// source-level key it was synthesized for (e.g. `"b.size"`).
    ///
    /// Used by the Z3 counter-example extractor to project internal atom names
    /// back to source-level variable names in diagnostic output.
    pub fn source_name_for(&self, atom: &str) -> Option<&str> {
        self.map
            .iter()
            .find(|(_, v)| v.as_str() == atom)
            .map(|(k, _)| k.as_str())
    }
}

/// Deterministic string form of an `Expr` subtree, independent of source
/// position.  Two `Expr` nodes with the same shape / identifiers produce the
/// same key regardless of where they appear.
fn canon_expr(e: &Expr) -> String {
    match e {
        Expr::Ident(name, _) => name.clone(),
        Expr::Literal(lit, _) => canon_literal(lit),
        Expr::FieldAccess { expr, field, .. } => {
            format!("{}.{field}", canon_expr(expr))
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let args_s = args.iter().map(canon_expr).collect::<Vec<_>>().join(",");
            format!("{}.{method}({args_s})", canon_expr(receiver))
        }
        Expr::FnCall { name, args, .. } => {
            let args_s = args.iter().map(canon_expr).collect::<Vec<_>>().join(",");
            format!("{name}({args_s})")
        }
        Expr::Unary { op, expr, .. } => format!("({op:?} {})", canon_expr(expr)),
        Expr::Binary {
            op, left, right, ..
        } => format!("({} {op:?} {})", canon_expr(left), canon_expr(right)),
        // Fall-through: anything unusual (Match, Lambda, Block, …) gets a
        // shape-tag prefix plus its debug form.  Distinct nodes are unlikely
        // to collide meaningfully at this point.
        other => format!("#{other:?}"),
    }
}

fn canon_literal(l: &crate::mvl::parser::ast::Literal) -> String {
    use crate::mvl::parser::ast::Literal;
    match l {
        Literal::Integer(n) => n.to_string(),
        Literal::Float(f) => format!("f{f}"),
        Literal::Str(s) => format!("\"{s}\""),
        Literal::Char(c) => format!("'{c}'"),
        Literal::Bool(b) => b.to_string(),
        Literal::Unit => "()".to_string(),
    }
}

/// Deterministic string form for a `RefExpr` subtree, aligned with
/// [`canon_expr`] so that `Expr::FieldAccess { object=x, field=y }` and
/// `RefExpr::FieldAccess { object=x, field=y }` map to the same key.
fn canon_refexpr(r: &RefExpr) -> String {
    match r {
        RefExpr::Ident { name, .. } => name.clone(),
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => format!("f{value}"),
        RefExpr::Bool { value, .. } => value.to_string(),
        RefExpr::FieldAccess { object, field, .. } => {
            format!("{}.{field}", canon_refexpr(object))
        }
        RefExpr::Len { ident, .. } => format!("{ident}.__len__"),
        RefExpr::ArithOp {
            op, left, right, ..
        } => format!("({} {op:?} {})", canon_refexpr(left), canon_refexpr(right)),
        RefExpr::Compare {
            op, left, right, ..
        } => format!("({} {op:?} {})", canon_refexpr(left), canon_refexpr(right)),
        RefExpr::LogicOp {
            op, left, right, ..
        } => format!("({} {op:?} {})", canon_refexpr(left), canon_refexpr(right)),
        RefExpr::Not { inner, .. } => format!("(! {})", canon_refexpr(inner)),
        RefExpr::Grouped { inner, .. } => canon_refexpr(inner),
        RefExpr::Old { inner, .. } => format!("old({})", canon_refexpr(inner)),
        RefExpr::Forall { var, body, .. } => {
            format!("∀{var}. {}", canon_refexpr(body))
        }
        RefExpr::Exists { var, body, .. } => {
            format!("∃{var}. {}", canon_refexpr(body))
        }
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => format!("({} {op:?} {})", canon_refexpr(left), canon_refexpr(right)),
        RefExpr::BitwiseNot { inner, .. } => format!("(~ {})", canon_refexpr(inner)),
        RefExpr::BoundedForall {
            var, lo, hi, body, ..
        } => {
            format!("∀{var}∈[{lo}..{hi}]. {}", canon_refexpr(body))
        }
        RefExpr::BoundedExists {
            var, lo, hi, body, ..
        } => {
            format!("∃{var}∈[{lo}..{hi}]. {}", canon_refexpr(body))
        }
        RefExpr::StringOp {
            op,
            receiver,
            literal,
            ..
        } => {
            use crate::mvl::parser::ast::StringOp;
            let m = match op {
                StringOp::Contains => "contains",
                StringOp::StartsWith => "starts_with",
                StringOp::EndsWith => "ends_with",
            };
            format!("{}.{}({literal:?})", canon_refexpr(receiver), m)
        }
        RefExpr::ArrayGet { list, index, .. } => {
            format!("{}[{}]", canon_refexpr(list), canon_refexpr(index))
        }
    }
}

#[allow(dead_code)]
fn _dummy_span_use() -> crate::mvl::parser::lexer::Span {
    // Kept so `use super::dummy_span` is not flagged in future edits.
    dummy_span()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::ast::{BinaryOp, CmpOp, Literal};
    use crate::mvl::parser::lexer::Span;

    fn s() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.to_string(), s())
    }

    fn field(base: Expr, name: &str) -> Expr {
        Expr::FieldAccess {
            expr: Box::new(base),
            field: name.to_string(),
            span: s(),
        }
    }

    #[test]
    fn field_access_maps_to_ident_atom() {
        let mut norm = AtomNormalizer::new();
        let e = field(ident("field"), "height");
        let out = norm.rewrite_expr(&e);
        match out {
            Expr::Ident(ref n, _) => assert!(n.starts_with("__atom_")),
            _ => panic!("expected Ident atom, got {:?}", out),
        }
        assert_eq!(norm.atom_count(), 1);
    }

    #[test]
    fn same_subtree_maps_to_same_atom() {
        let mut norm = AtomNormalizer::new();
        let a = norm.rewrite_expr(&field(ident("field"), "height"));
        let b = norm.rewrite_expr(&field(ident("field"), "height"));
        let (Expr::Ident(na, _), Expr::Ident(nb, _)) = (&a, &b) else {
            panic!("expected two idents");
        };
        assert_eq!(na, nb);
        assert_eq!(norm.atom_count(), 1);
    }

    #[test]
    fn different_subtrees_map_to_different_atoms() {
        let mut norm = AtomNormalizer::new();
        let a = norm.rewrite_expr(&field(ident("field"), "height"));
        let b = norm.rewrite_expr(&field(ident("field"), "width"));
        let (Expr::Ident(na, _), Expr::Ident(nb, _)) = (&a, &b) else {
            panic!("expected two idents");
        };
        assert_ne!(na, nb);
        assert_eq!(norm.atom_count(), 2);
    }

    #[test]
    fn expr_and_refexpr_share_atom_names() {
        let mut norm = AtomNormalizer::new();
        // Expr side: field.height
        let e = field(ident("field"), "height");
        let out_e = norm.rewrite_expr(&e);
        // RefExpr side: field.height (as a predicate atom)
        let r = RefExpr::FieldAccess {
            object: Box::new(RefExpr::Ident {
                name: "field".to_string(),
                span: s(),
            }),
            field: "height".to_string(),
            span: s(),
        };
        let out_r = norm.rewrite_refexpr(&r);
        let (Expr::Ident(ne, _), RefExpr::Ident { name: nr, .. }) = (&out_e, &out_r) else {
            panic!("expected idents from both sides");
        };
        assert_eq!(ne, nr, "same subtree must map to same atom across sides");
    }

    #[test]
    fn binary_recurses_into_field_atoms() {
        let mut norm = AtomNormalizer::new();
        // field.height - 1
        let e = Expr::Binary {
            op: BinaryOp::Sub,
            left: Box::new(field(ident("field"), "height")),
            right: Box::new(Expr::Literal(Literal::Integer(1), s())),
            span: s(),
        };
        let out = norm.rewrite_expr(&e);
        let Expr::Binary { left, right, .. } = &out else {
            panic!("expected Binary");
        };
        assert!(matches!(**left, Expr::Ident(ref n, _) if n.starts_with("__atom_")));
        assert!(matches!(**right, Expr::Literal(Literal::Integer(1), _)));
    }

    #[test]
    fn arithmetic_expr_is_untouched() {
        let mut norm = AtomNormalizer::new();
        let e = Expr::Binary {
            op: BinaryOp::Add,
            left: Box::new(ident("x")),
            right: Box::new(Expr::Literal(Literal::Integer(1), s())),
            span: s(),
        };
        let out = norm.rewrite_expr(&e);
        assert_eq!(norm.atom_count(), 0, "no atoms should be synthesized");
        assert_eq!(format!("{out:?}"), format!("{e:?}"));
    }

    #[test]
    fn refexpr_field_access_maps_to_atom() {
        let mut norm = AtomNormalizer::new();
        let r = RefExpr::Compare {
            op: CmpOp::Lt,
            left: Box::new(RefExpr::FieldAccess {
                object: Box::new(RefExpr::Ident {
                    name: "field".to_string(),
                    span: s(),
                }),
                field: "height".to_string(),
                span: s(),
            }),
            right: Box::new(RefExpr::Integer {
                value: 10,
                span: s(),
            }),
            span: s(),
        };
        let out = norm.rewrite_refexpr(&r);
        let RefExpr::Compare { left, .. } = &out else {
            panic!("expected Compare");
        };
        assert!(matches!(**left, RefExpr::Ident { .. }));
        assert_eq!(norm.atom_count(), 1);
    }
}
