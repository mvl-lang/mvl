// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis
//
// cli/tir.rs — `mvl tir <file>` command
//
// Runs the full pipeline (parse → check → mono → lower) and emits
// TirProgram as hand-crafted JSON to stdout.
// Consumed by the MVL backend in tests/spikes/004-tir-backend/.
//
// JSON conventions:
//   - Enum variants: {"tag": "VariantName", ...fields}
//   - Option<T>:     null or the value directly
//   - Renamed fields use the MVL names from compiler/tir.mvl

use mvl::mvl::checker::types::Ty;
use mvl::mvl::ir::{
    TirActorDecl, TirActorMethod, TirBlock, TirConstDecl, TirElseBranch, TirExpr, TirExprKind,
    TirExternDecl, TirExternFn, TirFieldDecl, TirFn, TirImplDecl, TirMatchArm, TirMatchBody,
    TirParam, TirProgram, TirSelectArm, TirStmt, TirTypeBody, TirTypeDecl, TirVariant,
    TirVariantFields,
};
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{
    BinaryOp, Capability, Effect, EffectDecl, GenericParam, LValue, LabelDecl, LetKind, Literal,
    Pattern, RefExpr, RelabelDecl, Totality, UnaryOp, UseDecl,
};
use mvl::mvl::parser::lexer::Span;
use mvl::mvl::passes::mono;
use mvl::mvl::pipeline::assemble_expr_types;

// ── JSON helpers ─────────────────────────────────────────────────────────────

fn q(s: &str) -> String {
    format!("\"{}\"", super::json_escape(s))
}

fn obj(pairs: &[(&str, String)]) -> String {
    let inner: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("{}: {}", q(k), v))
        .collect();
    format!("{{{}}}", inner.join(", "))
}

fn arr(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn jbool(b: bool) -> String {
    if b {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

fn jopt<T, F: Fn(&T) -> String>(o: &Option<T>, f: F) -> String {
    match o {
        None => "null".to_string(),
        Some(v) => f(v),
    }
}

fn jspan(s: &Span) -> String {
    obj(&[("line", s.line.to_string()), ("col", s.col.to_string())])
}

// ── Type serialization ────────────────────────────────────────────────────────

fn jty(ty: &Ty) -> String {
    match ty {
        Ty::Int => obj(&[("tag", q("Int"))]),
        Ty::Float => obj(&[("tag", q("Float"))]),
        Ty::String => obj(&[("tag", q("Str"))]),
        Ty::Bool => obj(&[("tag", q("Bool"))]),
        Ty::Char => obj(&[("tag", q("Char"))]),
        Ty::Byte => obj(&[("tag", q("Byte"))]),
        Ty::UByte => obj(&[("tag", q("UByte"))]),
        Ty::UInt => obj(&[("tag", q("UInt"))]),
        Ty::Unit => obj(&[("tag", q("Unit"))]),
        Ty::Never => obj(&[("tag", q("Never"))]),
        Ty::Unknown => obj(&[("tag", q("Unknown"))]),
        Ty::Named(name, args) => obj(&[
            ("tag", q("Named")),
            ("name", q(name)),
            ("args", arr(&args.iter().map(jty).collect::<Vec<_>>())),
        ]),
        Ty::Option(inner) => obj(&[("tag", q("Option")), ("inner", jty(inner))]),
        Ty::Result(ok, err) => obj(&[("tag", q("Result")), ("ok", jty(ok)), ("err", jty(err))]),
        Ty::Ref(mutable, inner) => obj(&[
            ("tag", q("Ref")),
            ("mutable", jbool(*mutable)),
            ("inner", jty(inner)),
        ]),
        Ty::Fn(params, ret, effects, totality) => obj(&[
            ("tag", q("TyFn")),
            ("params", arr(&params.iter().map(jty).collect::<Vec<_>>())),
            ("ret", jty(ret)),
            (
                "effects",
                arr(&effects.iter().map(jeffect).collect::<Vec<_>>()),
            ),
            ("totality", jopt(totality, jtotality)),
        ]),
        Ty::Tuple(elems) => obj(&[
            ("tag", q("Tuple")),
            ("elems", arr(&elems.iter().map(jty).collect::<Vec<_>>())),
        ]),
        Ty::List(inner) => obj(&[("tag", q("List")), ("inner", jty(inner))]),
        Ty::Array(elem, size) => obj(&[
            ("tag", q("Array")),
            ("elem", jty(elem)),
            ("size", size.to_string()),
        ]),
        Ty::Map(k, v) => obj(&[("tag", q("Map")), ("key", jty(k)), ("val", jty(v))]),
        Ty::Set(inner) => obj(&[("tag", q("Set")), ("inner", jty(inner))]),
        Ty::Refined(inner, _) => obj(&[("tag", q("Refined")), ("inner", jty(inner))]),
        Ty::Labeled(label, inner) => obj(&[
            ("tag", q("Labeled")),
            ("label", q(label)),
            ("inner", jty(inner)),
        ]),
        Ty::Ptr(inner) => obj(&[("tag", q("Ptr")), ("inner", jty(inner))]),
        Ty::Session(_) => obj(&[("tag", q("Session"))]),
        Ty::Ptr(inner) => obj(&[("tag", q("Ptr")), ("inner", jty(inner))]),
    }
}

// ── Primitive enum serialization ──────────────────────────────────────────────

fn jtotality(t: &Totality) -> String {
    q(match t {
        Totality::Total => "Total",
        Totality::Partial => "Partial",
    })
}

fn jcap(c: &Capability) -> String {
    q(match c {
        Capability::Iso => "Iso",
        Capability::Val => "Val",
        Capability::Ref => "Ref",
        Capability::Tag => "Tag",
    })
}

fn jletk(k: &LetKind) -> String {
    q(match k {
        LetKind::Regular => "Regular",
        LetKind::Ghost => "Ghost",
    })
}

fn jbinop(op: &BinaryOp) -> String {
    q(match op {
        BinaryOp::Add => "Add",
        BinaryOp::Sub => "Sub",
        BinaryOp::Mul => "Mul",
        BinaryOp::Div => "Div",
        BinaryOp::Rem => "Rem",
        BinaryOp::Eq => "Eq",
        BinaryOp::Ne => "Ne",
        BinaryOp::Lt => "Lt",
        BinaryOp::Gt => "Gt",
        BinaryOp::Le => "Le",
        BinaryOp::Ge => "Ge",
        BinaryOp::And => "And",
        BinaryOp::Or => "Or",
        BinaryOp::BitAnd => "BitAnd",
        BinaryOp::BitOr => "BitOr",
        BinaryOp::BitXor => "BitXor",
        BinaryOp::Shl => "Shl",
        BinaryOp::Shr => "Shr",
    })
}

fn junop(op: &UnaryOp) -> String {
    q(match op {
        UnaryOp::Neg => "Neg",
        UnaryOp::Not => "Not",
        UnaryOp::Deref => "Deref",
        UnaryOp::BitNot => "BitNot",
    })
}

fn jeffect(e: &Effect) -> String {
    obj(&[("name", q(&e.name))])
}

fn jtypeexpr(te: &mvl::mvl::parser::ast::TypeExpr) -> String {
    use mvl::mvl::parser::ast::TypeExpr;
    match te {
        TypeExpr::Base { name, args, .. } => obj(&[
            ("tag", q("Base")),
            ("name", q(name)),
            ("args", arr(&args.iter().map(jtypeexpr).collect::<Vec<_>>())),
        ]),
        TypeExpr::Option { inner, .. } => obj(&[("tag", q("Option")), ("inner", jtypeexpr(inner))]),
        TypeExpr::Result { ok, err, .. } => obj(&[
            ("tag", q("Result")),
            ("ok", jtypeexpr(ok)),
            ("err", jtypeexpr(err)),
        ]),
        TypeExpr::Ref { mutable, inner, .. } => obj(&[
            ("tag", q("Ref")),
            ("mutable", jbool(*mutable)),
            ("inner", jtypeexpr(inner)),
        ]),
        TypeExpr::Labeled { label, inner, .. } => obj(&[
            ("tag", q("Labeled")),
            ("label", q(label)),
            ("inner", jtypeexpr(inner)),
        ]),
        TypeExpr::Refined { inner, .. } => {
            obj(&[("tag", q("Refined")), ("inner", jtypeexpr(inner))])
        }
        TypeExpr::Fn {
            params,
            ret,
            effects,
            ..
        } => obj(&[
            ("tag", q("Fn")),
            (
                "params",
                arr(&params.iter().map(jtypeexpr).collect::<Vec<_>>()),
            ),
            ("ret", jtypeexpr(ret)),
            (
                "effects",
                arr(&effects.iter().map(jeffect).collect::<Vec<_>>()),
            ),
        ]),
        TypeExpr::Tuple { elems, .. } => obj(&[
            ("tag", q("Tuple")),
            (
                "elems",
                arr(&elems.iter().map(jtypeexpr).collect::<Vec<_>>()),
            ),
        ]),
        TypeExpr::IntConst { value, .. } => {
            obj(&[("tag", q("IntConst")), ("value", value.to_string())])
        }
        TypeExpr::Session { .. } => obj(&[("tag", q("Session"))]),
    }
}

fn jgeneric(g: &GenericParam) -> String {
    use mvl::mvl::parser::ast::GenericParam;
    match g {
        GenericParam::Type(name) => obj(&[("tag", q("Type")), ("name", q(name))]),
        GenericParam::Const(name, ty_str) => {
            obj(&[("tag", q("Const")), ("name", q(name)), ("ty", q(ty_str))])
        }
    }
}

fn jconstraint(c: &mvl::mvl::parser::ast::Constraint) -> String {
    obj(&[("name", q(&c.name)), ("bound", q(&c.bound))])
}

fn jliteral(lit: &Literal) -> String {
    match lit {
        Literal::Integer(n) => obj(&[("tag", q("Integer")), ("value", q(&n.to_string()))]),
        Literal::Float(f) => obj(&[("tag", q("Float")), ("value", q(&f.to_string()))]),
        Literal::Str(s) => obj(&[("tag", q("Str")), ("value", q(s))]),
        Literal::Char(c) => obj(&[("tag", q("Char")), ("value", q(&c.to_string()))]),
        Literal::Bool(b) => obj(&[("tag", q("Bool")), ("value", jbool(*b))]),
        Literal::Unit => obj(&[("tag", q("Unit"))]),
    }
}

fn jlvalue(lv: &LValue) -> String {
    match lv {
        LValue::Ident(name, span) => obj(&[
            ("tag", q("Ident")),
            ("name", q(name)),
            ("span", jspan(span)),
        ]),
        LValue::Field { base, field, span } => obj(&[
            ("tag", q("Field")),
            ("base", jlvalue(base)),
            ("field", q(field)),
            ("span", jspan(span)),
        ]),
    }
}

fn jpattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(span) => obj(&[("tag", q("Wildcard")), ("span", jspan(span))]),
        Pattern::Ident(name, span) => obj(&[
            ("tag", q("Ident")),
            ("name", q(name)),
            ("span", jspan(span)),
        ]),
        Pattern::Literal(lit, span) => obj(&[
            ("tag", q("Literal")),
            ("lit", jliteral(lit)),
            ("span", jspan(span)),
        ]),
        Pattern::Tuple { elems, span } => obj(&[
            ("tag", q("Tuple")),
            (
                "elems",
                arr(&elems.iter().map(jpattern).collect::<Vec<_>>()),
            ),
            ("span", jspan(span)),
        ]),
        Pattern::TupleStruct { name, fields, span } => obj(&[
            ("tag", q("TupleStruct")),
            ("name", q(name)),
            (
                "fields",
                arr(&fields.iter().map(jpattern).collect::<Vec<_>>()),
            ),
            ("span", jspan(span)),
        ]),
        Pattern::Struct { name, fields, span } => {
            let field_arr: Vec<String> = fields
                .iter()
                .map(|(k, v)| obj(&[("name", q(k)), ("pattern", jpattern(v))]))
                .collect();
            obj(&[
                ("tag", q("Struct")),
                ("name", q(name)),
                ("fields", arr(&field_arr)),
                ("span", jspan(span)),
            ])
        }
        Pattern::Some { inner, span } => obj(&[
            ("tag", q("Some")),
            ("inner", jpattern(inner)),
            ("span", jspan(span)),
        ]),
        Pattern::None(span) => obj(&[("tag", q("None")), ("span", jspan(span))]),
        Pattern::Ok { inner, span } => obj(&[
            ("tag", q("Ok")),
            ("inner", jpattern(inner)),
            ("span", jspan(span)),
        ]),
        Pattern::Err { inner, span } => obj(&[
            ("tag", q("Err")),
            ("inner", jpattern(inner)),
            ("span", jspan(span)),
        ]),
        Pattern::Or { patterns, span } => obj(&[
            ("tag", q("Or")),
            (
                "patterns",
                arr(&patterns.iter().map(jpattern).collect::<Vec<_>>()),
            ),
            ("span", jspan(span)),
        ]),
    }
}

fn juse(u: &UseDecl) -> String {
    obj(&[
        ("reexport", jbool(u.reexport)),
        (
            "path",
            arr(&u.path.iter().map(|s| q(s)).collect::<Vec<_>>()),
        ),
        (
            "items",
            arr(&u.items.iter().map(|s| q(s)).collect::<Vec<_>>()),
        ),
    ])
}

fn jeffectdecl(e: &EffectDecl) -> String {
    obj(&[
        ("name", q(&e.name)),
        (
            "subsumes",
            arr(&e.subsumes.iter().map(|s| q(s)).collect::<Vec<_>>()),
        ),
    ])
}

fn jlabeldecl(l: &LabelDecl) -> String {
    obj(&[("visible", jbool(l.visible)), ("name", q(&l.name))])
}

fn jrelabeldecl(r: &RelabelDecl) -> String {
    obj(&[
        ("visible", jbool(r.visible)),
        ("name", q(&r.name)),
        ("from", jopt(&r.from, |s| q(s))),
        ("to", jopt(&r.to, |s| q(s))),
        ("audit", jbool(r.audit)),
    ])
}

// ── RefExpr (stub — full serialization deferred) ─────────────────────────────

fn jrefexpr(_re: &RefExpr) -> String {
    obj(&[("tag", q("RefExpr"))])
}

// ── TIR expression serialization ─────────────────────────────────────────────

fn jexpr(e: &TirExpr) -> String {
    obj(&[
        ("kind", jexpr_kind(&e.kind)),
        ("ty", jty(&e.ty)),
        ("span", jspan(&e.span)),
    ])
}

fn jexpr_kind(k: &TirExprKind) -> String {
    match k {
        TirExprKind::Literal(lit) => obj(&[("tag", q("Literal")), ("lit", jliteral(lit))]),
        TirExprKind::Var(name) => obj(&[("tag", q("Var")), ("name", q(name))]),
        TirExprKind::FieldAccess { expr, field } => obj(&[
            ("tag", q("FieldAccess")),
            ("expr", jexpr(expr)),
            ("field", q(field)),
        ]),
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } => obj(&[
            ("tag", q("MethodCall")),
            ("receiver", jexpr(receiver)),
            ("method", q(method)),
            ("args", arr(&args.iter().map(jexpr).collect::<Vec<_>>())),
        ]),
        TirExprKind::FnCall {
            name,
            args,
            type_args,
        } => obj(&[
            ("tag", q("FnCall")),
            ("name", q(name)),
            ("args", arr(&args.iter().map(jexpr).collect::<Vec<_>>())),
            (
                "type_args",
                arr(&type_args.iter().map(jtypeexpr).collect::<Vec<_>>()),
            ),
        ]),
        TirExprKind::Unary { op, expr } => obj(&[
            ("tag", q("Unary")),
            ("op", junop(op)),
            ("expr", jexpr(expr)),
        ]),
        TirExprKind::Binary { op, left, right } => obj(&[
            ("tag", q("Binary")),
            ("op", jbinop(op)),
            ("left", jexpr(left)),
            ("right", jexpr(right)),
        ]),
        TirExprKind::If { cond, then, else_ } => obj(&[
            ("tag", q("If")),
            ("cond", jexpr(cond)),
            ("then", jblock(then)),
            ("else_br", jopt(else_, |e| jexpr(e))),
        ]),
        TirExprKind::Match { scrutinee, arms } => obj(&[
            ("tag", q("Match")),
            ("scrutinee", jexpr(scrutinee)),
            (
                "arms",
                arr(&arms.iter().map(jmatch_arm).collect::<Vec<_>>()),
            ),
        ]),
        TirExprKind::Block(block) => obj(&[("tag", q("Block")), ("block", jblock(block))]),
        TirExprKind::Lambda { params, body } => obj(&[
            ("tag", q("Lambda")),
            (
                "params",
                arr(&params.iter().map(jtir_param).collect::<Vec<_>>()),
            ),
            ("body", jexpr(body)),
        ]),
        TirExprKind::Propagate(expr) => obj(&[("tag", q("Propagate")), ("expr", jexpr(expr))]),
        TirExprKind::Construct { name, fields } => {
            let farr: Vec<String> = fields
                .iter()
                .map(|(k, v)| obj(&[("name", q(k)), ("expr", jexpr(v))]))
                .collect();
            obj(&[
                ("tag", q("Construct")),
                ("name", q(name)),
                ("fields", arr(&farr)),
            ])
        }
        TirExprKind::List { elems } => obj(&[
            ("tag", q("ListLit")),
            ("elems", arr(&elems.iter().map(jexpr).collect::<Vec<_>>())),
        ]),
        TirExprKind::Map { pairs } => {
            let parr: Vec<String> = pairs
                .iter()
                .map(|(k, v)| obj(&[("key", jexpr(k)), ("val", jexpr(v))]))
                .collect();
            obj(&[("tag", q("MapLit")), ("pairs", arr(&parr))])
        }
        TirExprKind::Set { elems } => obj(&[
            ("tag", q("SetLit")),
            ("elems", arr(&elems.iter().map(jexpr).collect::<Vec<_>>())),
        ]),
        TirExprKind::Consume(expr) => obj(&[("tag", q("Consume")), ("expr", jexpr(expr))]),
        TirExprKind::Relabel {
            name,
            expr,
            tag,
            audit,
        } => obj(&[
            ("tag", q("Relabel")),
            ("name", q(name)),
            ("expr", jexpr(expr)),
            ("audit_tag", q(tag)),
            ("audit", jbool(*audit)),
        ]),
        TirExprKind::Borrow { mutable, expr } => obj(&[
            ("tag", q("Borrow")),
            ("mutable", jbool(*mutable)),
            ("expr", jexpr(expr)),
        ]),
        TirExprKind::Spawn { actor_type, fields } => {
            let farr: Vec<String> = fields
                .iter()
                .map(|(k, v)| obj(&[("name", q(k)), ("expr", jexpr(v))]))
                .collect();
            obj(&[
                ("tag", q("Spawn")),
                ("actor_type", q(actor_type)),
                ("fields", arr(&farr)),
            ])
        }
        TirExprKind::Select { arms } => obj(&[
            ("tag", q("Select")),
            (
                "arms",
                arr(&arms.iter().map(jselect_arm).collect::<Vec<_>>()),
            ),
        ]),
        TirExprKind::Quantifier(re) => obj(&[("tag", q("Quantifier")), ("expr", jrefexpr(re))]),
    }
}

// ── TIR statement serialization ───────────────────────────────────────────────

fn jstmt(s: &TirStmt) -> String {
    match s {
        TirStmt::Let {
            kind,
            pattern,
            ty,
            init,
            span,
        } => obj(&[
            ("tag", q("Let")),
            ("kind", jletk(kind)),
            ("pattern", jpattern(pattern)),
            ("ty", jty(ty)),
            ("init", jexpr(init)),
            ("span", jspan(span)),
        ]),
        TirStmt::Assign {
            target,
            value,
            span,
        } => obj(&[
            ("tag", q("Assign")),
            ("target", jlvalue(target)),
            ("value", jexpr(value)),
            ("span", jspan(span)),
        ]),
        TirStmt::Return { value, span } => obj(&[
            ("tag", q("Return")),
            ("value", jopt(value, jexpr)),
            ("span", jspan(span)),
        ]),
        TirStmt::If {
            cond,
            then,
            else_,
            span,
        } => obj(&[
            ("tag", q("If")),
            ("cond", jexpr(cond)),
            ("then", jblock(then)),
            ("else_br", jopt(else_, jelse_branch)),
            ("span", jspan(span)),
        ]),
        TirStmt::Match {
            scrutinee,
            arms,
            span,
        } => obj(&[
            ("tag", q("Match")),
            ("scrutinee", jexpr(scrutinee)),
            (
                "arms",
                arr(&arms.iter().map(jmatch_arm).collect::<Vec<_>>()),
            ),
            ("span", jspan(span)),
        ]),
        TirStmt::For {
            pattern,
            iter,
            invariants,
            body,
            span,
        } => obj(&[
            ("tag", q("For")),
            ("pattern", jpattern(pattern)),
            ("iter", jexpr(iter)),
            (
                "invariants",
                arr(&invariants.iter().map(jexpr).collect::<Vec<_>>()),
            ),
            ("body", jblock(body)),
            ("span", jspan(span)),
        ]),
        TirStmt::While {
            cond,
            invariants,
            decreases,
            body,
            span,
        } => obj(&[
            ("tag", q("While")),
            ("cond", jexpr(cond)),
            (
                "invariants",
                arr(&invariants.iter().map(jexpr).collect::<Vec<_>>()),
            ),
            ("decrease_by", jopt(decreases, |d| jexpr(d))),
            ("body", jblock(body)),
            ("span", jspan(span)),
        ]),
        TirStmt::Expr { expr, span } => obj(&[
            ("tag", q("Expr")),
            ("expr", jexpr(expr)),
            ("span", jspan(span)),
        ]),
    }
}

fn jelse_branch(eb: &TirElseBranch) -> String {
    match eb {
        TirElseBranch::Block(block) => obj(&[("tag", q("Block")), ("block", jblock(block))]),
        TirElseBranch::If(stmt) => obj(&[("tag", q("If")), ("stmt", jstmt(stmt))]),
    }
}

fn jblock(b: &TirBlock) -> String {
    obj(&[
        ("stmts", arr(&b.stmts.iter().map(jstmt).collect::<Vec<_>>())),
        ("span", jspan(&b.span)),
    ])
}

fn jmatch_arm(arm: &TirMatchArm) -> String {
    obj(&[
        ("pattern", jpattern(&arm.pattern)),
        ("guard", jopt(&arm.guard, jrefexpr)),
        ("body", jmatch_body(&arm.body)),
        ("span", jspan(&arm.span)),
    ])
}

fn jmatch_body(mb: &TirMatchBody) -> String {
    match mb {
        TirMatchBody::Expr(e) => obj(&[("tag", q("Expr")), ("expr", jexpr(e))]),
        TirMatchBody::Block(b) => obj(&[("tag", q("Block")), ("block", jblock(b))]),
    }
}

fn jselect_arm(sa: &TirSelectArm) -> String {
    obj(&[
        ("binding", jopt(&sa.binding, |s| q(s))),
        ("expr", jexpr(&sa.expr)),
        ("is_timeout", jbool(sa.is_timeout)),
        ("body", jblock(&sa.body)),
        ("span", jspan(&sa.span)),
    ])
}

// ── TIR function/parameter serialization ─────────────────────────────────────

fn jtir_param(p: &TirParam) -> String {
    obj(&[
        ("name", q(&p.name)),
        ("ty", jty(&p.ty)),
        ("capability", jopt(&p.capability, jcap)),
        ("span", jspan(&p.span)),
    ])
}

fn jtir_fn(f: &TirFn) -> String {
    obj(&[
        ("name", q(&f.name)),
        ("original_name", q(&f.original_name)),
        ("visible", jbool(f.visible)),
        ("is_test", jbool(f.is_test)),
        ("is_builtin", jbool(f.is_builtin)),
        ("receiver_type", jopt(&f.receiver_type, |s| q(s))),
        (
            "type_params",
            arr(&f.type_params.iter().map(jgeneric).collect::<Vec<_>>()),
        ),
        (
            "constraints",
            arr(&f.constraints.iter().map(jconstraint).collect::<Vec<_>>()),
        ),
        ("totality", jopt(&f.totality, jtotality)),
        (
            "params",
            arr(&f.params.iter().map(jtir_param).collect::<Vec<_>>()),
        ),
        ("ret_ty", jty(&f.ret_ty)),
        ("return_refinement", jopt(&f.return_refinement, jrefexpr)),
        (
            "effects",
            arr(&f.effects.iter().map(jeffect).collect::<Vec<_>>()),
        ),
        (
            "pre_conds",
            arr(&f.requires.iter().map(jrefexpr).collect::<Vec<_>>()),
        ),
        (
            "post_conds",
            arr(&f.ensures.iter().map(jrefexpr).collect::<Vec<_>>()),
        ),
        ("body", jblock(&f.body)),
        ("span", jspan(&f.span)),
    ])
}

// ── TIR type declaration serialization ───────────────────────────────────────

fn jtir_field(fd: &TirFieldDecl) -> String {
    obj(&[
        ("name", q(&fd.name)),
        ("ty", jty(&fd.ty)),
        ("refinement", jopt(&fd.refinement, jrefexpr)),
        ("span", jspan(&fd.span)),
    ])
}

fn jtir_variant(v: &TirVariant) -> String {
    let fields_json = match &v.fields {
        TirVariantFields::Unit => obj(&[("tag", q("Unit"))]),
        TirVariantFields::Tuple(tys) => obj(&[
            ("tag", q("Tuple")),
            ("tys", arr(&tys.iter().map(jty).collect::<Vec<_>>())),
        ]),
        TirVariantFields::Struct(fds) => obj(&[
            ("tag", q("Struct")),
            (
                "fields",
                arr(&fds.iter().map(jtir_field).collect::<Vec<_>>()),
            ),
        ]),
    };
    obj(&[
        ("name", q(&v.name)),
        ("fields", fields_json),
        ("span", jspan(&v.span)),
    ])
}

fn jtir_type_body(tb: &TirTypeBody) -> String {
    match tb {
        TirTypeBody::Struct { fields, invariant } => obj(&[
            ("tag", q("Struct")),
            (
                "fields",
                arr(&fields.iter().map(jtir_field).collect::<Vec<_>>()),
            ),
            ("type_invariant", jopt(invariant, jrefexpr)),
        ]),
        TirTypeBody::Enum(variants) => obj(&[
            ("tag", q("Enum")),
            (
                "variants",
                arr(&variants.iter().map(jtir_variant).collect::<Vec<_>>()),
            ),
        ]),
        TirTypeBody::Alias(ty) => obj(&[("tag", q("Alias")), ("ty", jty(ty))]),
    }
}

fn jtir_type_decl(td: &TirTypeDecl) -> String {
    obj(&[
        ("visible", jbool(td.visible)),
        ("name", q(&td.name)),
        (
            "params",
            arr(&td.params.iter().map(jgeneric).collect::<Vec<_>>()),
        ),
        ("body", jtir_type_body(&td.body)),
        ("span", jspan(&td.span)),
    ])
}

fn jtir_extern_fn(ef: &TirExternFn) -> String {
    obj(&[
        ("name", q(&ef.name)),
        (
            "params",
            arr(&ef.params.iter().map(jtir_param).collect::<Vec<_>>()),
        ),
        ("ret_ty", jty(&ef.ret_ty)),
        (
            "effects",
            arr(&ef.effects.iter().map(jeffect).collect::<Vec<_>>()),
        ),
        ("totality", jopt(&ef.totality, jtotality)),
    ])
}

fn jtir_extern_decl(ed: &TirExternDecl) -> String {
    obj(&[
        ("abi", q(&ed.abi)),
        (
            "fns",
            arr(&ed.fns.iter().map(jtir_extern_fn).collect::<Vec<_>>()),
        ),
    ])
}

fn jtir_actor_method(m: &TirActorMethod) -> String {
    obj(&[
        ("is_public", jbool(m.is_public)),
        ("name", q(&m.name)),
        (
            "params",
            arr(&m.params.iter().map(jtir_param).collect::<Vec<_>>()),
        ),
        ("ret_ty", jty(&m.ret_ty)),
        (
            "effects",
            arr(&m.effects.iter().map(jeffect).collect::<Vec<_>>()),
        ),
        ("body", jblock(&m.body)),
    ])
}

fn jtir_actor_decl(ad: &TirActorDecl) -> String {
    obj(&[
        ("visible", jbool(ad.visible)),
        ("name", q(&ad.name)),
        (
            "fields",
            arr(&ad.fields.iter().map(jtir_field).collect::<Vec<_>>()),
        ),
        (
            "methods",
            arr(&ad.methods.iter().map(jtir_actor_method).collect::<Vec<_>>()),
        ),
        ("traps_exit", jbool(ad.traps_exit)),
    ])
}

fn jtir_impl_decl(id: &TirImplDecl) -> String {
    obj(&[
        ("trait_name", q(&id.trait_name)),
        ("type_name", q(&id.type_name)),
        (
            "trait_type_args",
            arr(&id.trait_type_args.iter().map(jty).collect::<Vec<_>>()),
        ),
        (
            "methods",
            arr(&id.methods.iter().map(jtir_fn).collect::<Vec<_>>()),
        ),
    ])
}

fn jtir_const_decl(cd: &TirConstDecl) -> String {
    obj(&[
        ("visible", jbool(cd.visible)),
        ("name", q(&cd.name)),
        ("ty", jty(&cd.ty)),
        ("value", jexpr(&cd.value)),
    ])
}

// ── Top-level TirProgram serialization ───────────────────────────────────────

fn serialize(tir: &TirProgram) -> String {
    obj(&[
        ("fns", arr(&tir.fns.iter().map(jtir_fn).collect::<Vec<_>>())),
        (
            "types",
            arr(&tir.types.iter().map(jtir_type_decl).collect::<Vec<_>>()),
        ),
        (
            "externs",
            arr(&tir.externs.iter().map(jtir_extern_decl).collect::<Vec<_>>()),
        ),
        (
            "actors",
            arr(&tir.actors.iter().map(jtir_actor_decl).collect::<Vec<_>>()),
        ),
        (
            "impls",
            arr(&tir.impls.iter().map(jtir_impl_decl).collect::<Vec<_>>()),
        ),
        (
            "consts",
            arr(&tir.consts.iter().map(jtir_const_decl).collect::<Vec<_>>()),
        ),
        ("uses", arr(&tir.uses.iter().map(juse).collect::<Vec<_>>())),
        (
            "effect_decls",
            arr(&tir.effect_decls.iter().map(jeffectdecl).collect::<Vec<_>>()),
        ),
        (
            "label_decls",
            arr(&tir.label_decls.iter().map(jlabeldecl).collect::<Vec<_>>()),
        ),
        (
            "relabel_decls",
            arr(&tir
                .relabel_decls
                .iter()
                .map(jrelabeldecl)
                .collect::<Vec<_>>()),
        ),
    ])
}

// ── CLI entry point ───────────────────────────────────────────────────────────

pub fn run(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let prelude = loader::load_implicit_prelude();
    let expr_types = assemble_expr_types(&prog, &prelude);
    let all_fns = mono::collect_fns(std::iter::once(&prog).chain(prelude.iter()));
    let mono_prog = mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = mvl::mvl::ir::lower::lower(&prog, &mono_prog, &expr_types);
    println!("{}", serialize(&tir));
}
