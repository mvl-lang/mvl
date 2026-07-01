// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! AST-walker type inference helpers — `type_of_expr` and friends.
//!
//! Note: the shared `ty_to_llvm`, `llvm_ty_ctx`, heap-drop, and value-conversion
//! helpers were extracted to `emit_helpers.rs` in #1612 Phase 3b PR 2 prep.
//! Only the functions that take `&Expr` / `&Stmt` (and therefore can't survive
//! the AST walker's deletion) remain here. PR 2 deletes this entire file.

use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, Stmt, TypeExpr, UnaryOp};

use super::{TextEmitter, RESULT_LLVM_TY};

impl TextEmitter {
    /// Remove the heap-local entry for a value that is about to be returned
    /// (moved out of the function), preventing `emit_heap_drops` from freeing it.
    pub(super) fn exclude_returned_value(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name, _) => {
                // Check ref locals first — the alloca ptr is tracked in heap_locals.
                if let Some(loc) = self.fn_ctx.ref_locals.get(name) {
                    let ptr = loc.ptr.clone();
                    self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != ptr);
                    return;
                }
                // Check regular (non-ref) locals — the SSA itself is tracked.
                if let Some(ssa) = self.fn_ctx.locals.get(name) {
                    let ssa = ssa.clone();
                    self.fn_ctx.heap_locals.retain(|(s, _, _)| *s != ssa);
                }
            }
            // Consume / Relabel are transparent wrappers — recurse.
            Expr::Consume { expr: inner, .. } | Expr::Relabel { expr: inner, .. } => {
                self.exclude_returned_value(inner);
            }
            _ => {}
        }
    }

    /// Infer the LLVM type of an expression without emitting instructions.
    ///
    /// When checker-resolved `expr_types` are available (#1302), looks up
    /// the expression's span for an accurate type before falling back to
    /// AST-based inference.
    pub(super) fn type_of_expr(&self, expr: &Expr) -> String {
        // Try checker-resolved type first (available when checker ran in the pipeline).
        if let Some(ty) = self.module.expr_types.get(&expr.span()) {
            return self.ty_to_llvm_ctx(ty);
        }
        match expr {
            Expr::Literal(Literal::Integer(_), _) => "i64".into(),
            Expr::Literal(Literal::Float(_), _) => "double".into(),
            Expr::Literal(Literal::Bool(_), _) => "i1".into(),
            Expr::Literal(Literal::Str(_), _) => "ptr".into(),
            Expr::Literal(Literal::Unit, _) => "void".into(),
            Expr::Ident(name, _) => {
                if name == "None" {
                    return RESULT_LLVM_TY.into();
                }
                // Qualified enum variant "Type::Variant"
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = &name[..pos];
                        if self.module.enum_variants.contains_key(type_name) {
                            if self.enum_has_payloads(type_name) {
                                return RESULT_LLVM_TY.into();
                            }
                            return "i64".into();
                        }
                    }
                }
                if let Some(loc) = self.fn_ctx.ref_locals.get(name) {
                    return self.llvm_ty_ctx(&loc.elem_ty);
                }
                if let Some(mvl_ty) = self.fn_ctx.local_mvl_types.get(name) {
                    return self.llvm_ty_ctx(mvl_ty);
                }
                if let Some(ssa) = self.fn_ctx.locals.get(name) {
                    if let Some(ty) = self.fn_ctx.reg_types.get(ssa) {
                        return ty.clone();
                    }
                }
                "i64".into()
            }
            Expr::Binary {
                op:
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Gt
                    | BinaryOp::Le
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or,
                ..
            } => "i1".into(),
            Expr::Binary { .. } => "i64".into(),
            Expr::Unary {
                op: UnaryOp::Not, ..
            } => "i1".into(),
            Expr::FnCall { name, .. } => {
                // Enum variant constructor
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = &name[..pos];
                        if self.module.enum_variants.contains_key(type_name) {
                            if self.enum_has_payloads(type_name) {
                                return RESULT_LLVM_TY.into();
                            }
                            return "i64".into();
                        }
                    }
                }
                match name.as_str() {
                    "assert" | "println" | "print" | "eprintln" => "void".into(),
                    "format" => "ptr".into(),
                    "Some" | "None" | "Ok" | "Err" => RESULT_LLVM_TY.into(),
                    // Stdlib C-ABI dispatch functions whose return types are
                    // stripped from fn_ret_types by the prelude filter (#1202).
                    "path" | "format_datetime" | "format_instant" | "find_all" | "replace" => {
                        "ptr".into()
                    }
                    _ => {
                        if let Some(ret) = self.module.fn_ret_types.get(name) {
                            self.llvm_ty_ctx(ret)
                        } else {
                            "i64".into()
                        }
                    }
                }
            }
            Expr::MethodCall {
                method,
                receiver,
                args: margs,
                ..
            } => {
                let recv_ty = self.type_of_expr(receiver);
                match (method.as_str(), recv_ty.as_str()) {
                    // Int (i64) methods returning i64
                    ("abs" | "min" | "max" | "clamp" | "pow", "i64") => "i64".into(),
                    // Int (i64) predicates returning i1
                    ("is_positive" | "is_negative" | "is_zero", "i64") => "i1".into(),
                    // Int → Float conversion
                    ("to_float", "i64") => "double".into(),
                    // Float (double) methods returning double
                    (
                        "abs" | "ceil" | "floor" | "round" | "sqrt" | "min" | "max" | "clamp"
                        | "pow",
                        "double",
                    ) => "double".into(),
                    // Float → Int conversion
                    ("to_int", "double") => "i64".into(),
                    // Float predicates returning i1
                    (
                        "is_nan" | "is_finite" | "is_infinite" | "is_positive" | "is_negative",
                        "double",
                    ) => "i1".into(),
                    // String-returning methods
                    ("to_string" | "concat" | "to_lower" | "to_upper" | "trim", _) => "ptr".into(),
                    // Length: always i64
                    ("len", _) => "i64".into(),
                    // Boolean predicates on collections/strings
                    ("is_some" | "is_none" | "is_empty" | "contains" | "contains_key", _) => {
                        "i1".into()
                    }
                    // Option/Result unwrap: type comes from default argument
                    ("unwrap_or", _) => {
                        if let Some(a) = margs.first() {
                            self.type_of_expr(a)
                        } else {
                            "i64".into()
                        }
                    }
                    // Indexed access returning Option
                    ("get", _)
                        if matches!(
                            self.mvl_receiver_kind(receiver),
                            Some("List") | Some("Map")
                        ) =>
                    {
                        "{ i8, ptr }".into()
                    }
                    ("first" | "last", _) => "{ i8, ptr }".into(),
                    _ => "ptr".into(),
                }
            }
            Expr::Construct { name, .. } => {
                if self.module.struct_fields.contains_key(name) {
                    format!("%{name}")
                } else {
                    "ptr".into()
                }
            }
            Expr::FieldAccess { expr, field, .. } => self
                .field_type_of(expr, field)
                .unwrap_or_else(|| "i64".into()),
            Expr::List { .. } | Expr::Map { .. } | Expr::Set { .. } => "ptr".into(),
            Expr::Consume { expr, .. } | Expr::Relabel { expr, .. } => self.type_of_expr(expr),
            Expr::Unary {
                op: UnaryOp::Deref,
                expr: inner,
                ..
            } => self
                .box_inner_llvm_ty(inner)
                .unwrap_or_else(|| "i64".into()),
            Expr::If { then, .. } => self.type_of_block_tail(then),
            Expr::Block(b) => self.type_of_block_tail(b),
            // A lambda expression is a closure pointer.
            Expr::Lambda { .. } => "ptr".into(),
            // A spawn expression produces an opaque actor handle pointer.
            Expr::Spawn { .. } => "ptr".into(),
            _ => "i64".into(),
        }
    }

    /// Infer the LLVM type from the tail of a block (the last statement).
    /// Handles `Stmt::Expr`, `Stmt::If`, and `Stmt::Match` as tail positions.
    ///
    /// For `Stmt::If`, only the `then` branch is inspected — MVL requires both
    /// branches to have the same type, so the `then` branch is sufficient.
    pub(super) fn type_of_block_tail(&self, b: &crate::mvl::parser::ast::Block) -> String {
        match b.stmts.last() {
            Some(Stmt::Expr { expr, .. }) => self.type_of_expr(expr),
            Some(Stmt::If { then, .. }) => self.type_of_block_tail(then),
            Some(Stmt::Match { arms, .. }) => {
                for arm in arms {
                    let t = match &arm.body {
                        crate::mvl::parser::ast::MatchBody::Expr(e) => self.type_of_expr(e),
                        crate::mvl::parser::ast::MatchBody::Block(b) => self.type_of_block_tail(b),
                    };
                    if t != "i64" {
                        return t;
                    }
                }
                "i64".into()
            }
            _ => "i64".into(),
        }
    }

    /// Look up the struct type name (e.g. "Point") of an expression, if known.
    pub(super) fn struct_name_of_expr(&self, expr: &Expr) -> Option<String> {
        // Peel `val`/`ref`/`Labeled`/`Refined` wrappers — they don't change the
        // underlying struct identity for field-access purposes.
        let mut mvl_ty = self.mvl_type_of_expr(expr);
        while let TypeExpr::Ref { inner, .. }
        | TypeExpr::Labeled { inner, .. }
        | TypeExpr::Refined { inner, .. } = mvl_ty
        {
            mvl_ty = *inner;
        }
        if let TypeExpr::Base { name: tn, .. } = &mvl_ty {
            if self.module.struct_fields.contains_key(tn) {
                return Some(tn.clone());
            }
        }
        None
    }

    /// Resolve the LLVM type of a struct field access without emitting code.
    fn field_type_of(&self, receiver: &Expr, field: &str) -> Option<String> {
        let sn = self.struct_name_of_expr(receiver)?;
        let fields = self.module.struct_fields.get(&sn)?;
        let (_, field_ty) = fields.iter().find(|(f, _)| f == field)?;
        Some(self.llvm_ty_ctx(field_ty))
    }

    /// Return the MVL base type name of a receiver expression when it can be
    /// determined statically from `local_mvl_types`.
    ///
    /// Returns `"String"`, `"List"`, `"Map"`, `"Set"`, etc.  Returns `None`
    /// when the type is unknown (e.g. an SSA value with no MVL annotation).
    pub(super) fn mvl_receiver_kind(&self, expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Literal(Literal::Str(_), _) => Some("String"),
            Expr::Ident(name, _) => {
                let mvl_ty = self.fn_ctx.local_mvl_types.get(name.as_str())?;
                // Peel `val`/`ref`/`Labeled`/`Refined` wrappers to reach the base name.
                let mut cur: &TypeExpr = mvl_ty;
                while let TypeExpr::Ref { inner, .. }
                | TypeExpr::Labeled { inner, .. }
                | TypeExpr::Refined { inner, .. } = cur
                {
                    cur = inner.as_ref();
                }
                match cur {
                    TypeExpr::Base { name: tn, .. } => Some(tn.as_str()),
                    _ => None,
                }
            }
            // Relabel/Consume strip the IFC label — recurse into the inner expr.
            Expr::Relabel { expr: inner, .. } | Expr::Consume { expr: inner, .. } => {
                self.mvl_receiver_kind(inner)
            }
            _ => None,
        }
    }

    /// True if `receiver`'s MVL type is `List`, `Array`, or `Set` — used to
    /// guard dispatch arms that lower to the shared `_mvl_list_slice` runtime.
    pub(super) fn is_list_array_set(&self, receiver: &Expr) -> bool {
        matches!(
            self.mvl_receiver_kind(receiver),
            Some("List") | Some("Array") | Some("Set")
        )
    }

    /// If `expr` has MVL type `Box[T]`, return the LLVM IR type of `T`.
    /// Used to emit `load T, ptr %box` when emitting `*box` deref (#1154).
    pub(super) fn box_inner_llvm_ty(&self, expr: &Expr) -> Option<String> {
        let mvl_ty: TypeExpr = match expr {
            Expr::Ident(name, _) => self.fn_ctx.local_mvl_types.get(name.as_str()).cloned()?,
            Expr::FieldAccess {
                expr: receiver,
                field,
                ..
            } => {
                if let Expr::Ident(name, _) = receiver.as_ref() {
                    let recv_ty = self.fn_ctx.local_mvl_types.get(name.as_str())?;
                    if let TypeExpr::Base { name: tn, .. } = recv_ty {
                        let fields = self.module.struct_fields.get(tn)?;
                        fields
                            .iter()
                            .find(|(fname, _)| fname == field)
                            .map(|(_, ty)| ty.clone())?
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        if let TypeExpr::Base { name, args, .. } = mvl_ty {
            if name == "Box" && args.len() == 1 {
                return Some(self.llvm_ty_ctx(&args[0]));
            }
        }
        None
    }
}
