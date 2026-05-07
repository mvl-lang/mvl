//! Expression emission for the MVL LLVM backend.
//!
//! Covers all `Expr` variants: literals, identifiers, binary/unary operators,
//! function calls, struct/enum construction, field access, collection literals,
//! method calls, `if` expressions, `?` propagation, and Option/Result helpers.

use inkwell::{
    types::BasicTypeEnum, values::BasicValueEnum, AddressSpace, FloatPredicate, IntPredicate,
};

use crate::mvl::parser::ast::{
    BinaryOp, Block, Expr, LValue, Literal, TypeExpr, UnaryOp, VariantFields,
};

use super::{stmts, LlvmBackend};

impl<'ctx> LlvmBackend<'ctx> {
    // ── Expression emission ──────────────────────────────────────────────────

    pub(crate) fn emit_expr(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        match expr {
            Expr::Literal(lit, _) => self.emit_literal(lit),

            Expr::Ident(name, _) => self.emit_ident(name),

            Expr::Binary {
                op, left, right, ..
            } => self.emit_binary(op, left, right),

            Expr::Unary { op, expr, .. } => self.emit_unary(op, expr),

            Expr::FnCall { name, args, .. } => self.emit_fn_call(name, args),

            Expr::Block(block) => self.emit_block(block),

            // move/consume/declassify/sanitize: transparent at IR level.
            Expr::Move { expr, .. }
            | Expr::Consume { expr, .. }
            | Expr::Declassify { expr, .. }
            | Expr::Sanitize { expr, .. } => self.emit_expr(expr),

            Expr::If {
                cond, then, else_, ..
            } => self.emit_if_expr(cond, then, else_.as_deref()),

            // L5-11: match expression
            Expr::Match {
                scrutinee, arms, ..
            } => self.emit_match(scrutinee, arms),

            // L5-05: struct construction
            Expr::Construct { name, fields, .. } => self.emit_construct(name, fields),

            // L5-05: field access
            Expr::FieldAccess { expr, field, .. } => self.emit_field_access(expr, field),

            // L5-12: ? propagation
            Expr::Propagate { expr, .. } => self.emit_propagate(expr),

            // Collection literals
            Expr::List { elems, .. } => self.emit_list_literal(elems),
            Expr::Map { pairs, .. } => self.emit_map_literal(pairs),
            Expr::Set { elems, .. } => self.emit_set_literal(elems),

            // Method calls: minimal support for .len() on range and .to_string() on Int
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.emit_method_call(receiver, method, args),

            _other => {
                // Unhandled Expr variant: return None so the caller can propagate failure.
                // In debug builds, print a notice to help catch missing codegen arms early.
                #[cfg(debug_assertions)]
                eprintln!(
                    "[llvm-backend] unhandled Expr variant in emit_expr: {:?}",
                    std::mem::discriminant(_other)
                );
                None
            }
        }
    }

    pub(crate) fn emit_ident(&mut self, name: &str) -> Option<BasicValueEnum<'ctx>> {
        // L5-06: qualified enum variant reference, e.g. `Shape::Circle`
        if name.contains("::") {
            if let Some(pos) = name.find("::") {
                let type_name = name[..pos].to_string();
                let variant_name = name[pos + 2..].to_string();
                return self.emit_enum_variant_construct(&type_name, &variant_name, &[]);
            }
        }

        // Local variable.
        if let Some((alloca, ty)) = self.locals.get(name).copied() {
            let val = self.builder.build_load(ty, alloca, name).unwrap();
            return Some(val);
        }

        // L5-08: None as an expression → { disc=1, payload=null_ptr }
        if name == "None" {
            return self.emit_none_val();
        }

        // L5-06: unqualified unit enum variant (e.g. `Circle` without `Shape::`).
        let found = self.enum_variants.iter().find_map(|(etype, variants)| {
            variants
                .iter()
                .position(|(vn, _)| vn == name)
                .map(|_| etype.clone())
        });
        if let Some(etype) = found {
            return self.emit_enum_variant_construct(&etype, name, &[]);
        }

        None
    }

    pub(crate) fn emit_if_expr(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Expr>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let cond_val = self.emit_expr(cond)?;
        let cond_int = match cond_val {
            BasicValueEnum::IntValue(v) => {
                if v.get_type().get_bit_width() != 1 {
                    self.builder
                        .build_int_truncate(v, self.context.bool_type(), "cond_trunc")
                        .unwrap()
                } else {
                    v
                }
            }
            _ => return None,
        };

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let then_bb = self.context.append_basic_block(parent_fn, "if_then");
        let else_bb = self.context.append_basic_block(parent_fn, "if_else");
        let merge_bb = self.context.append_basic_block(parent_fn, "if_merge");

        self.builder
            .build_conditional_branch(cond_int, then_bb, else_bb)
            .unwrap();

        // then block
        self.builder.position_at_end(then_bb);
        let then_val = self.emit_block(then);
        let then_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        // else block
        self.builder.position_at_end(else_bb);
        let else_val = else_.and_then(|e| self.emit_expr(e));
        let else_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);

        // Build phi if both branches produce a value of the same type.
        match (then_val, else_val) {
            (Some(tv), Some(ev)) if tv.get_type() == ev.get_type() => {
                let phi = self.builder.build_phi(tv.get_type(), "if_result").unwrap();
                phi.add_incoming(&[(&tv, then_end), (&ev, else_end)]);
                Some(phi.as_basic_value())
            }
            _ => None,
        }
    }

    // ── L5-05: Struct emission ────────────────────────────────────────────────

    /// Emit `Name { field: expr, ... }` struct construction.
    pub(crate) fn emit_construct(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        // Enum struct variant: "EnumType::Variant { fields }"
        if let Some(pos) = name.find("::") {
            let type_name = name[..pos].to_string();
            let variant_name = name[pos + 2..].to_string();
            if self.enum_variants.contains_key(&type_name) {
                return self.emit_enum_struct_variant(&type_name, &variant_name, fields);
            }
        }

        // Regular struct construction.
        let field_info: Vec<(String, TypeExpr)> = self.struct_fields.get(name)?.clone();
        let struct_ty = *self.llvm_struct_types.get(name)?;

        // L5-13: use alloca+GEP to build the struct so every value type has an
        // address.  This is the foundation L5-14 heap types rely on for consistent
        // pointer semantics (ADR-0016).
        let alloca = self.builder.build_alloca(struct_ty, "struct_tmp").unwrap();

        for (idx, (fname, _)) in field_info.iter().enumerate() {
            if let Some((_, fexpr)) = fields.iter().find(|(n, _)| n == fname) {
                if let Some(fval) = self.emit_expr(fexpr) {
                    let field_ptr = self
                        .builder
                        .build_struct_gep(struct_ty, alloca, idx as u32, &format!("f{idx}_ptr"))
                        .unwrap();
                    self.builder.build_store(field_ptr, fval).unwrap();
                }
            }
        }

        Some(
            self.builder
                .build_load(struct_ty, alloca, "struct_val")
                .unwrap(),
        )
    }

    /// Emit a struct-variant enum construction: `AuthError::AccountLocked { attempts: 3 }`.
    pub(crate) fn emit_enum_struct_variant(
        &mut self,
        type_name: &str,
        variant_name: &str,
        fields: &[(String, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        let variants = self.enum_variants.get(type_name)?.clone();
        let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;
        let struct_ty = *self.llvm_struct_types.get(type_name)?;
        let alloca = self.builder.build_alloca(struct_ty, "enum_sv").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store struct fields into payload area.
        let variant_fields = variants
            .iter()
            .find(|(vn, _)| vn == variant_name)
            .map(|(_, vf)| vf.clone())?;
        if let VariantFields::Struct(field_decls) = &variant_fields {
            let payload_ptr = self
                .builder
                .build_struct_gep(struct_ty, alloca, 1, "payload_ptr")
                .unwrap();

            let mut offset = 0usize;
            for fd in field_decls {
                if let Some((_, fexpr)) = fields.iter().find(|(n, _)| n == &fd.name) {
                    if let Some(fval) = self.emit_expr(fexpr) {
                        if let Some(llvm_ty) = self.mvl_type_to_llvm(&fd.ty) {
                            let field_ptr = if offset == 0 {
                                payload_ptr
                            } else {
                                let off = self.context.i64_type().const_int(offset as u64, false);
                                unsafe {
                                    self.builder
                                        .build_gep(
                                            self.context.i8_type(),
                                            payload_ptr,
                                            &[off],
                                            "sv_field",
                                        )
                                        .unwrap()
                                }
                            };
                            self.builder.build_store(field_ptr, fval).unwrap();
                            offset += Self::type_size_bytes_static(&fd.ty);
                            let _ = llvm_ty; // used above
                        }
                    }
                }
            }
        }

        Some(
            self.builder
                .build_load(struct_ty, alloca, "enum_sv_val")
                .unwrap(),
        )
    }

    /// Emit `expr.field` field access.
    pub(crate) fn emit_field_access(
        &mut self,
        obj: &Expr,
        field: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let obj_val = self.emit_expr(obj)?;
        let BasicValueEnum::StructValue(sv) = obj_val else {
            return None;
        };

        // Look up the struct type name → field index.
        let ty = sv.get_type();
        let type_name = ty.get_name()?.to_str().ok()?.to_string();
        let field_info = self.struct_fields.get(&type_name)?.clone();
        let idx = field_info.iter().position(|(n, _)| n == field)? as u32;

        self.builder.build_extract_value(sv, idx, field).ok()
    }

    /// Emit a field assignment `lvalue.field = val`.
    pub(crate) fn emit_field_assign(
        &mut self,
        base: &LValue,
        field: &str,
        new_val: BasicValueEnum<'ctx>,
    ) {
        // Only handle the simple case: `ident.field = val`.
        let LValue::Ident(var_name, _) = base else {
            return;
        };
        let Some((alloca, ty)) = self.locals.get(var_name.as_str()).copied() else {
            return;
        };
        let BasicTypeEnum::StructType(st) = ty else {
            return;
        };
        let type_name = match st.get_name() {
            Some(n) => n.to_str().unwrap_or("").to_string(),
            None => return,
        };
        let field_info = match self.struct_fields.get(&type_name) {
            Some(fi) => fi.clone(),
            None => return,
        };
        let idx = match field_info.iter().position(|(n, _)| n == field) {
            Some(i) => i as u32,
            None => return,
        };

        // Load → insert → store.
        let cur = self.builder.build_load(ty, alloca, "cur").unwrap();
        let BasicValueEnum::StructValue(sv) = cur else {
            return;
        };
        let updated = self
            .builder
            .build_insert_value(sv, new_val, idx, "updated")
            .unwrap()
            .into_struct_value();
        self.builder.build_store(alloca, updated).unwrap();
    }

    // ── L5-06: Enum variant construction ────────────────────────────────────

    /// Construct an enum variant value.
    ///
    /// - Unit enum (all variants are Unit): returns `i8` discriminant.
    /// - Payload enum: allocas a tagged union `{ i8, [N×i8] }`, stores discriminant
    ///   and payload, then loads and returns the struct value.
    pub(crate) fn emit_enum_variant_construct(
        &mut self,
        type_name: &str,
        variant_name: &str,
        payload_args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let variants = self.enum_variants.get(type_name)?.clone();
        let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;

        if Self::is_unit_enum_variants(&variants) {
            // Unit enum: just the i8 discriminant.
            return Some(self.context.i8_type().const_int(disc, false).into());
        }

        // Payload enum: build tagged union on the stack.
        let struct_ty = *self.llvm_struct_types.get(type_name)?;
        let alloca = self.builder.build_alloca(struct_ty, "enum_tmp").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store payload if arguments were provided.
        if !payload_args.is_empty() {
            let variant_fields = variants
                .iter()
                .find(|(vn, _)| vn == variant_name)
                .map(|(_, vf)| vf.clone())?;

            if let VariantFields::Tuple(field_types) = &variant_fields {
                let payload_ptr = self
                    .builder
                    .build_struct_gep(struct_ty, alloca, 1, "payload_ptr")
                    .unwrap();

                let mut offset = 0usize;
                for (arg, fty) in payload_args.iter().zip(field_types.iter()) {
                    if let Some(fval) = self.emit_expr(arg) {
                        // Heap move: when a heap-allocated identifier is stored into an enum
                        // variant, ownership transfers — remove it from heap_locals so the
                        // original alloca is not dropped at function exit (double-free).
                        if let Expr::Ident(src, _) = arg {
                            self.heap_locals.remove(src.as_str());
                        }
                        let field_ptr = if offset == 0 {
                            payload_ptr
                        } else {
                            let off = self.context.i64_type().const_int(offset as u64, false);
                            unsafe {
                                self.builder
                                    .build_gep(
                                        self.context.i8_type(),
                                        payload_ptr,
                                        &[off],
                                        "pf_ptr",
                                    )
                                    .unwrap()
                            }
                        };
                        self.builder.build_store(field_ptr, fval).unwrap();
                        offset += Self::type_size_bytes_static(fty);
                    }
                }
            }
        }

        Some(
            self.builder
                .build_load(struct_ty, alloca, "enum_val")
                .unwrap(),
        )
    }

    // ── Result/Option construction ────────────────────────────────────────────

    /// Emit `Ok(val)` (disc=0), `Some(val)` (disc=0), or `Err(val)` (disc=1).
    ///
    /// Layout: `{ i8 disc, ptr payload }` where payload points to a stack alloca of the value.
    /// This pointer-based approach supports any payload size (L5-08).
    pub(crate) fn emit_result_variant(
        &mut self,
        disc: u64,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "res_tmp").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "res_disc")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store payload via pointer: alloca the value, store it, save ptr at field 1.
        let payload_slot = self
            .builder
            .build_struct_gep(result_ty, alloca, 1, "res_payload_slot")
            .unwrap();
        if let Some(arg) = args.first() {
            if let Some(val) = self.emit_expr(arg) {
                let val_alloca = self
                    .builder
                    .build_alloca(val.get_type(), "payload_tmp")
                    .unwrap();
                self.builder.build_store(val_alloca, val).unwrap();
                self.builder.build_store(payload_slot, val_alloca).unwrap();
            }
        } else {
            // No payload (e.g. unit Err) — store null.
            self.builder
                .build_store(payload_slot, ptr_ty.const_null())
                .unwrap();
        }

        Some(
            self.builder
                .build_load(result_ty, alloca, "res_val")
                .unwrap(),
        )
    }

    // ── L5-12: ? propagation ─────────────────────────────────────────────────

    /// Emit `expr?` — evaluate expr (must return `Result[T, E]` tagged union),
    /// branch to ok/err: on Err, return early; on Ok, yield the payload value.
    ///
    /// When the Ok type is `Unit` (`infer_result_ok_llvm_ty` returns `None`) the
    /// payload pointer is null and must not be dereferenced; we return `i64 0`
    /// as the unit sentinel instead.
    pub(crate) fn emit_propagate(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let ok_ty_opt = self.infer_result_ok_llvm_ty(expr);
        let result_val = self.emit_expr(expr)?;
        let BasicValueEnum::StructValue(sv) = result_val else {
            return None;
        };

        // Extract i8 discriminant (field 0).
        let disc = self.builder.build_extract_value(sv, 0, "prop_disc").ok()?;
        let BasicValueEnum::IntValue(disc_i) = disc else {
            return None;
        };

        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
        let ok_bb = self.context.append_basic_block(parent_fn, "prop_ok");
        let err_bb = self.context.append_basic_block(parent_fn, "prop_err");

        let zero = self.context.i8_type().const_int(0, false);
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, disc_i, zero, "is_ok")
            .unwrap();
        self.builder
            .build_conditional_branch(is_ok, ok_bb, err_bb)
            .unwrap();

        // Err branch: return the Result struct unchanged (propagate the error).
        self.builder.position_at_end(err_bb);
        self.builder.build_return(Some(&result_val)).unwrap();

        // Ok branch: extract payload ptr (field 1) and load the actual value.
        self.builder.position_at_end(ok_bb);
        let payload_ptr_val = self
            .builder
            .build_extract_value(sv, 1, "prop_payload_ptr")
            .ok()?;

        // Unit-result guard: when ok_ty is None the Ok payload is null (no value).
        // Return i64 0 as the unit sentinel without dereferencing the pointer.
        let ok_val = match ok_ty_opt {
            None => self.context.i64_type().const_zero().into(),
            Some(ok_ty) => {
                let payload_ptr = payload_ptr_val.into_pointer_value();
                self.builder
                    .build_load(ok_ty, payload_ptr, "prop_ok_val")
                    .unwrap()
            }
        };
        Some(ok_val)
    }

    // ── Literal emission ─────────────────────────────────────────────────────

    pub(crate) fn emit_literal(&self, lit: &Literal) -> Option<BasicValueEnum<'ctx>> {
        match lit {
            Literal::Integer(n) => {
                let v = self.context.i64_type().const_int(*n as u64, *n < 0);
                Some(v.into())
            }
            Literal::Float(f) => {
                let v = self.context.f64_type().const_float(*f);
                Some(v.into())
            }
            Literal::Bool(b) => {
                let v = self.context.bool_type().const_int(u64::from(*b), false);
                Some(v.into())
            }
            Literal::Str(s) => {
                // L5-14: allocate a heap MvlString so String values have RC semantics.
                // The global provides stable bytes; mvl_string_new copies them to the heap.
                let global = self.builder.build_global_string_ptr(s, "str_lit").unwrap();
                let len = self.context.i64_type().const_int(s.len() as u64, false);
                let new_fn = self.get_mvl_string_new();
                let call = self
                    .builder
                    .build_call(
                        new_fn,
                        &[global.as_pointer_value().into(), len.into()],
                        "str_new",
                    )
                    .unwrap();
                use inkwell::values::AnyValue;
                BasicValueEnum::try_from(call.as_any_value_enum()).ok()
            }
            Literal::Char(c) => {
                let v = self.context.i32_type().const_int(*c as u64, false);
                Some(v.into())
            }
            Literal::Unit => None,
        }
    }

    // ── Binary operators (L5-10) ─────────────────────────────────────────────

    pub(crate) fn emit_binary(
        &mut self,
        op: &BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Option<BasicValueEnum<'ctx>> {
        let lhs = self.emit_expr(left)?;
        let rhs = self.emit_expr(right)?;

        match (lhs, rhs) {
            (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                self.emit_int_binop(op, l, r)
            }
            (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => {
                self.emit_float_binop(op, l, r)
            }
            _ => None,
        }
    }

    pub(crate) fn emit_int_binop(
        &mut self,
        op: &BinaryOp,
        l: inkwell::values::IntValue<'ctx>,
        r: inkwell::values::IntValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        // Use checked arithmetic intrinsics for Add/Sub/Mul (L5-10: overflow detection).
        let result = match op {
            BinaryOp::Add => self.emit_checked_int_arith(l, r, "llvm.sadd.with.overflow", "add")?,
            BinaryOp::Sub => self.emit_checked_int_arith(l, r, "llvm.ssub.with.overflow", "sub")?,
            BinaryOp::Mul => self.emit_checked_int_arith(l, r, "llvm.smul.with.overflow", "mul")?,
            BinaryOp::Div => self
                .builder
                .build_int_signed_div(l, r, "div")
                .unwrap()
                .into(),
            BinaryOp::Rem => self
                .builder
                .build_int_signed_rem(l, r, "rem")
                .unwrap()
                .into(),
            BinaryOp::Eq => self
                .builder
                .build_int_compare(IntPredicate::EQ, l, r, "eq")
                .unwrap()
                .into(),
            BinaryOp::Ne => self
                .builder
                .build_int_compare(IntPredicate::NE, l, r, "ne")
                .unwrap()
                .into(),
            BinaryOp::Lt => self
                .builder
                .build_int_compare(IntPredicate::SLT, l, r, "lt")
                .unwrap()
                .into(),
            BinaryOp::Gt => self
                .builder
                .build_int_compare(IntPredicate::SGT, l, r, "gt")
                .unwrap()
                .into(),
            BinaryOp::Le => self
                .builder
                .build_int_compare(IntPredicate::SLE, l, r, "le")
                .unwrap()
                .into(),
            BinaryOp::Ge => self
                .builder
                .build_int_compare(IntPredicate::SGE, l, r, "ge")
                .unwrap()
                .into(),
            BinaryOp::And => self.builder.build_and(l, r, "and").unwrap().into(),
            BinaryOp::Or => self.builder.build_or(l, r, "or").unwrap().into(),
            BinaryOp::BitAnd => self.builder.build_and(l, r, "bitand").unwrap().into(),
            BinaryOp::BitOr => self.builder.build_or(l, r, "bitor").unwrap().into(),
            BinaryOp::BitXor => self.builder.build_xor(l, r, "bitxor").unwrap().into(),
            BinaryOp::Shl => self.builder.build_left_shift(l, r, "shl").unwrap().into(),
            // Default to arithmetic (signed) right shift; logical shift for unsigned
            // types requires type-level signedness tracking (follow-up: #484).
            BinaryOp::Shr => self
                .builder
                .build_right_shift(l, r, true, "shr")
                .unwrap()
                .into(),
        };
        Some(result)
    }

    /// Emit a checked arithmetic intrinsic (`llvm.sadd.with.overflow.i64`, etc.).
    ///
    /// Extracts the result value and traps (unreachable) on overflow.
    pub(crate) fn emit_checked_int_arith(
        &mut self,
        l: inkwell::values::IntValue<'ctx>,
        r: inkwell::values::IntValue<'ctx>,
        intrinsic_name: &str,
        result_name: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let i1_ty = self.context.bool_type();
        // LLVM intrinsic names use dots: e.g. "llvm.sadd.with.overflow.i64".
        let full_name = format!("{intrinsic_name}.i64");
        let intrinsic_fn = self.module.get_function(&full_name).unwrap_or_else(|| {
            let struct_ty = self
                .context
                .struct_type(&[i64_ty.into(), i1_ty.into()], false);
            let fn_ty = struct_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
            // Declare with no explicit linkage so LLVM recognises this as a built-in intrinsic.
            self.module.add_function(&full_name, fn_ty, None)
        });

        let call = self
            .builder
            .build_call(intrinsic_fn, &[l.into(), r.into()], result_name)
            .unwrap();
        use inkwell::values::AnyValue;
        let result_struct = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;

        let val = self
            .builder
            .build_extract_value(
                result_struct.into_struct_value(),
                0,
                &format!("{result_name}_val"),
            )
            .unwrap();
        let overflow = self
            .builder
            .build_extract_value(
                result_struct.into_struct_value(),
                1,
                &format!("{result_name}_ovf"),
            )
            .unwrap();

        // On overflow: trap via llvm.trap and unreachable.
        let trap_fn = self.module.get_function("llvm.trap").unwrap_or_else(|| {
            let trap_ty = self.context.void_type().fn_type(&[], false);
            self.module.add_function("llvm.trap", trap_ty, None)
        });
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let overflow_bb = self.context.append_basic_block(parent_fn, "overflow");
        let ok_bb = self.context.append_basic_block(parent_fn, "ok");
        self.builder
            .build_conditional_branch(overflow.into_int_value(), overflow_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(overflow_bb);
        self.builder.build_call(trap_fn, &[], "trap").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);

        Some(val)
    }

    pub(crate) fn emit_float_binop(
        &mut self,
        op: &BinaryOp,
        l: inkwell::values::FloatValue<'ctx>,
        r: inkwell::values::FloatValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let v = match op {
            BinaryOp::Add => self.builder.build_float_add(l, r, "fadd").unwrap().into(),
            BinaryOp::Sub => self.builder.build_float_sub(l, r, "fsub").unwrap().into(),
            BinaryOp::Mul => self.builder.build_float_mul(l, r, "fmul").unwrap().into(),
            BinaryOp::Div => self.builder.build_float_div(l, r, "fdiv").unwrap().into(),
            BinaryOp::Rem => self.builder.build_float_rem(l, r, "frem").unwrap().into(),
            BinaryOp::Eq => self
                .builder
                .build_float_compare(FloatPredicate::OEQ, l, r, "feq")
                .unwrap()
                .into(),
            BinaryOp::Ne => self
                .builder
                .build_float_compare(FloatPredicate::ONE, l, r, "fne")
                .unwrap()
                .into(),
            BinaryOp::Lt => self
                .builder
                .build_float_compare(FloatPredicate::OLT, l, r, "flt")
                .unwrap()
                .into(),
            BinaryOp::Gt => self
                .builder
                .build_float_compare(FloatPredicate::OGT, l, r, "fgt")
                .unwrap()
                .into(),
            BinaryOp::Le => self
                .builder
                .build_float_compare(FloatPredicate::OLE, l, r, "fle")
                .unwrap()
                .into(),
            BinaryOp::Ge => self
                .builder
                .build_float_compare(FloatPredicate::OGE, l, r, "fge")
                .unwrap()
                .into(),
            _ => return None,
        };
        Some(v)
    }

    // ── Unary operators ──────────────────────────────────────────────────────

    pub(crate) fn emit_unary(&mut self, op: &UnaryOp, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let val = self.emit_expr(expr)?;
        match op {
            UnaryOp::Neg => match val {
                BasicValueEnum::IntValue(v) => {
                    Some(self.builder.build_int_neg(v, "neg").unwrap().into())
                }
                BasicValueEnum::FloatValue(v) => {
                    Some(self.builder.build_float_neg(v, "fneg").unwrap().into())
                }
                _ => None,
            },
            UnaryOp::Not => match val {
                BasicValueEnum::IntValue(v) => {
                    Some(self.builder.build_not(v, "not").unwrap().into())
                }
                _ => None,
            },
            UnaryOp::Deref => Some(val),
            UnaryOp::BitNot => match val {
                // LLVM bitwise NOT = XOR with all-ones (-1).
                BasicValueEnum::IntValue(v) => {
                    let ones = v.get_type().const_all_ones();
                    Some(self.builder.build_xor(v, ones, "bitnot").unwrap().into())
                }
                _ => None,
            },
        }
    }

    // ── #508: IFC codegen invariant helpers ──────────────────────────────────

    /// Returns true if `expr` names a local variable labeled `Secret[_]`.
    ///
    /// Used in asserts to catch codegen bugs that would route a Secret
    /// value to a public sink (print, println, log_*) without a `declassify` node.
    /// The MVL static checker enforces this before codegen runs; this is
    /// defense-in-depth against future codegen regressions.
    fn is_secret_labeled(&self, expr: &Expr) -> bool {
        if let Expr::Ident(name, _) = expr {
            if let Some(ty) = self.local_mvl_types.get(name.as_str()) {
                return matches!(
                    ty,
                    TypeExpr::Labeled {
                        label: crate::mvl::parser::ast::SecurityLabel::Secret,
                        ..
                    }
                );
            }
        }
        false
    }

    // ── Function call emission (L5-07 + L5-17) ──────────────────────────────

    pub(crate) fn emit_fn_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        match name {
            "println" => {
                // #508: IFC invariant — static checker guarantees no Secret arg reaches println.
                assert!(
                    args.iter().all(|a| !self.is_secret_labeled(a)),
                    "codegen bug: Secret-labeled value routed to println without declassify"
                );
                self.emit_println(args)
            }
            "print" => {
                // #508: IFC invariant — same guard as println (both are public sinks).
                assert!(
                    args.iter().all(|a| !self.is_secret_labeled(a)),
                    "codegen bug: Secret-labeled value routed to print without declassify"
                );
                self.emit_print(args)
            }
            "eprintln" => {
                // #508: IFC invariant — stderr is a public sink; no Secret args allowed.
                assert!(
                    args.iter().all(|a| !self.is_secret_labeled(a)),
                    "codegen bug: Secret-labeled value routed to eprintln without declassify"
                );
                self.emit_eprintln(args)
            }
            "eprint" => {
                // #508: IFC invariant — stderr is a public sink; no Secret args allowed.
                assert!(
                    args.iter().all(|a| !self.is_secret_labeled(a)),
                    "codegen bug: Secret-labeled value routed to eprint without declassify"
                );
                self.emit_eprint(args)
            }
            "format" => self.emit_format(args),
            "eprintln" => self.emit_eprintln(args),
            "eprint" => self.emit_eprint(args),
            // assert(condition) — trap if condition is false.
            "assert" if args.len() == 1 => {
                let cond = match self.emit_expr(&args[0])? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                let trap_fn = self.module.get_function("llvm.trap").unwrap_or_else(|| {
                    let trap_ty = self.context.void_type().fn_type(&[], false);
                    self.module.add_function("llvm.trap", trap_ty, None)
                });
                let parent = self
                    .builder
                    .get_insert_block()
                    .unwrap()
                    .get_parent()
                    .unwrap();
                let fail_bb = self.context.append_basic_block(parent, "assert_fail");
                let ok_bb = self.context.append_basic_block(parent, "assert_ok");
                let not_cond = self.builder.build_not(cond, "not_cond").unwrap();
                self.builder
                    .build_conditional_branch(not_cond, fail_bb, ok_bb)
                    .unwrap();
                self.builder.position_at_end(fail_bb);
                self.builder.build_call(trap_fn, &[], "trap").unwrap();
                self.builder.build_unreachable().unwrap();
                self.builder.position_at_end(ok_bb);
                None
            }
            // panic(message) — print to stderr, then trap unconditionally.
            "panic" => {
                self.emit_eprintln(args);
                let trap_fn = self.module.get_function("llvm.trap").unwrap_or_else(|| {
                    let trap_ty = self.context.void_type().fn_type(&[], false);
                    self.module.add_function("llvm.trap", trap_ty, None)
                });
                self.builder.build_call(trap_fn, &[], "trap").unwrap();
                self.builder.build_unreachable().unwrap();
                self.terminated = true;
                None
            }
            // assert_eq / assert_ne — polymorphic comparisons.
            // core.mvl declares assert_eq(String, String) but call sites may pass Int or Bool.
            // Emit a type-appropriate comparison and trap on failure.
            "assert_eq" | "assert_ne" if args.len() == 2 => {
                let expect_eq = name == "assert_eq";
                let left = self.emit_expr(&args[0])?;
                let right = self.emit_expr(&args[1])?;
                let fail_cond: Option<inkwell::values::IntValue<'ctx>> = match (left, right) {
                    (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                        let pred = if expect_eq {
                            inkwell::IntPredicate::NE
                        } else {
                            inkwell::IntPredicate::EQ
                        };
                        Some(
                            self.builder
                                .build_int_compare(pred, l, r, "assert_cmp")
                                .unwrap(),
                        )
                    }
                    (BasicValueEnum::PointerValue(l), BasicValueEnum::PointerValue(r)) => {
                        let eq_fn = self.get_mvl_string_eq();
                        let call = self
                            .builder
                            .build_call(eq_fn, &[l.into(), r.into()], "str_eq")
                            .unwrap();
                        use inkwell::values::AnyValue;
                        let eq_i32 = BasicValueEnum::try_from(call.as_any_value_enum())
                            .ok()
                            .and_then(|v| {
                                if let BasicValueEnum::IntValue(i) = v {
                                    Some(i)
                                } else {
                                    None
                                }
                            })?;
                        let zero = self.context.i32_type().const_int(0, false);
                        let is_eq = self
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                eq_i32,
                                zero,
                                "str_eq_bool",
                            )
                            .unwrap();
                        let pred = if expect_eq {
                            // fail when not equal → invert is_eq
                            self.builder.build_not(is_eq, "assert_cmp").unwrap()
                        } else {
                            // fail when equal → is_eq itself is the fail condition
                            is_eq
                        };
                        Some(pred)
                    }
                    _ => None,
                };
                if let Some(cond) = fail_cond {
                    let trap_fn = self.module.get_function("llvm.trap").unwrap_or_else(|| {
                        let trap_ty = self.context.void_type().fn_type(&[], false);
                        self.module.add_function("llvm.trap", trap_ty, None)
                    });
                    let parent = self
                        .builder
                        .get_insert_block()
                        .unwrap()
                        .get_parent()
                        .unwrap();
                    let fail_bb = self.context.append_basic_block(parent, "assert_fail");
                    let ok_bb = self.context.append_basic_block(parent, "assert_ok");
                    self.builder
                        .build_conditional_branch(cond, fail_bb, ok_bb)
                        .unwrap();
                    self.builder.position_at_end(fail_bb);
                    self.builder.build_call(trap_fn, &[], "trap").unwrap();
                    self.builder.build_unreachable().unwrap();
                    self.builder.position_at_end(ok_bb);
                }
                None
            }
            // range(start, end) as a value → { i64 start, i64 end } range struct
            "range" if args.len() == 2 => {
                let start = match self.emit_expr(&args[0])? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                let end = match self.emit_expr(&args[1])? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                let range_ty = self.context.struct_type(
                    &[
                        self.context.i64_type().into(),
                        self.context.i64_type().into(),
                    ],
                    false,
                );
                let alloca = self.builder.build_alloca(range_ty, "range_tmp").unwrap();
                let s_ptr = self
                    .builder
                    .build_struct_gep(range_ty, alloca, 0, "range_start")
                    .unwrap();
                let e_ptr = self
                    .builder
                    .build_struct_gep(range_ty, alloca, 1, "range_end")
                    .unwrap();
                self.builder.build_store(s_ptr, start).unwrap();
                self.builder.build_store(e_ptr, end).unwrap();
                Some(
                    self.builder
                        .build_load(range_ty, alloca, "range_val")
                        .unwrap(),
                )
            }
            _ => {
                // Built-in Result/Option constructors: Ok(v), Err(e), Some(v)
                if matches!(name, "Ok" | "Some") && args.len() == 1 {
                    return self.emit_result_variant(0, args);
                }
                if name == "Err" && args.len() == 1 {
                    return self.emit_result_variant(1, args);
                }
                // L5-06: enum tuple variant constructor, e.g. `Shape::Circle(r)`
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = name[..pos].to_string();
                        let variant_name = name[pos + 2..].to_string();
                        // Qualified Result/Option constructors: Result::Ok, Result::Err, Option::Some.
                        if matches!(variant_name.as_str(), "Ok" | "Some") && args.len() == 1 {
                            return self.emit_result_variant(0, args);
                        }
                        if variant_name == "Err" && args.len() == 1 {
                            return self.emit_result_variant(1, args);
                        }
                        if self.enum_variants.contains_key(&type_name) {
                            return self.emit_enum_variant_construct(
                                &type_name,
                                &variant_name,
                                args,
                            );
                        }
                    }
                }
                // list_push[T](arr, elem) → call mvl_array_push(arr, &elem), return arr.
                // Handles the generic case where T may be a struct (e.g. Value), avoiding
                // the type mismatch from the i64-defaulted pre-declaration.
                if name == "list_push" && args.len() == 2 {
                    let arr_val = self.emit_expr(&args[0])?;
                    let elem_val = self.emit_expr(&args[1])?;
                    let arr_ptr = arr_val.into_pointer_value();
                    let slot = self
                        .builder
                        .build_alloca(elem_val.get_type(), "push_slot")
                        .unwrap();
                    self.builder.build_store(slot, elem_val).unwrap();
                    let push_fn = self.get_mvl_array_push();
                    self.builder
                        .build_call(push_fn, &[arr_ptr.into(), slot.into()], "list_push")
                        .unwrap();
                    return Some(arr_ptr.into());
                }

                // #583: generic builtins — inline emit using expr_types.
                // Excluded from StdlibSig table (no single C-ABI calling convention).
                if name == "choice" && args.len() == 1 {
                    return self.emit_random_choice(&args[0]);
                }
                if name == "shuffle" && args.len() == 1 {
                    return self.emit_random_shuffle(&args[0]);
                }

                // #587: set algebra — inline-emit without HOF.
                if name == "set_intersection" && args.len() == 2 {
                    return self.emit_set_intersection(&args[0], &args[1]);
                }
                if name == "set_difference" && args.len() == 2 {
                    return self.emit_set_difference(&args[0], &args[1]);
                }
                if name == "set_union" && args.len() == 2 {
                    return self.emit_set_union(&args[0], &args[1]);
                }

                // ADR-0019: dispatch to libmvl_runtime_c C-ABI for stdlib imports.
                if let Some(sig) = self.stdlib_imports.get(name).cloned() {
                    use crate::mvl::codegen::StdlibSig;
                    match &sig {
                        StdlibSig::I64NoArg(sym) if args.is_empty() => {
                            return self.emit_stdlib_call_i64(sym);
                        }
                        StdlibSig::F64NoArg(sym) if args.is_empty() => {
                            return self.emit_stdlib_call_f64(sym);
                        }
                        // #557: ptr return, no args (env.args, args.get_args)
                        StdlibSig::PtrNoArg(sym) if args.is_empty() => {
                            return self.emit_stdlib_call_ptr_no_arg(sym);
                        }
                        StdlibSig::I64TwoI64Args(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let a = self.emit_expr(&args[0])?;
                            let b = self.emit_expr(&args[1])?;
                            return self.emit_stdlib_call_i64_two_args(&sym, a, b);
                        }
                        StdlibSig::VoidDurationArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let d = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_void_duration_arg(&sym, d);
                        }
                        StdlibSig::VoidStringMapArg(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            // #508: IFC invariant — static checker guarantees no Secret arg
                            // reaches log sinks (log_debug/info/warn/error) without declassify.
                            assert!(
                                !self.is_secret_labeled(&args[0])
                                    && !self.is_secret_labeled(&args[1]),
                                "codegen bug: Secret-labeled value routed to log sink without declassify"
                            );
                            let msg = self.emit_expr(&args[0])?;
                            let fields = self.emit_expr(&args[1])?;
                            return self.emit_stdlib_call_void_string_map(&sym, msg, fields);
                        }
                        // #435: io stdlib
                        StdlibSig::PtrIdentArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_ptr_identity(&sym, arg);
                        }
                        StdlibSig::ResultUnitOnePtrArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_result_one_ptr_arg(&sym, arg);
                        }
                        StdlibSig::ResultUnitTwoPtrArgs(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let a = self.emit_expr(&args[0])?;
                            let b = self.emit_expr(&args[1])?;
                            return self.emit_stdlib_call_result_two_ptr_args(&sym, a, b);
                        }
                        StdlibSig::ResultStringOnePtrArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_result_one_ptr_arg(&sym, arg);
                        }
                        StdlibSig::StringThreePtrArgs(sym) if args.len() == 3 => {
                            let sym = sym.clone();
                            let a = self.emit_expr(&args[0])?;
                            let b = self.emit_expr(&args[1])?;
                            let c = self.emit_expr(&args[2])?;
                            return self.emit_stdlib_call_string_three_ptr_args(&sym, a, b, c);
                        }
                        StdlibSig::OptionMatchTwoPtrArgs(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let handle = self.emit_expr(&args[0])?;
                            let input = self.emit_expr(&args[1])?;
                            return self
                                .emit_stdlib_call_option_match_two_ptr_args(&sym, handle, input);
                        }
                        // #584: (ptr, ptr) → ptr (e.g. regex.find_all → MvlArray*)
                        StdlibSig::PtrTwoPtrArgs(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let a = self.emit_expr(&args[0])?;
                            let b = self.emit_expr(&args[1])?;
                            return self.emit_stdlib_call_ptr_two_ptr_args(&sym, a, b);
                        }
                        // #507: i64 → ptr (e.g. crypto_random_bytes(n) → *mut MvlArray)
                        StdlibSig::I64ReturnsPtrArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_i64_returns_ptr(&sym, arg);
                        }
                        // #536: ptr → i64 (exists, is_file, is_dir)
                        StdlibSig::I64OnePtrArg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_i64_one_ptr_arg(&sym, arg);
                        }
                        // #536: (ptr, i64) → Result[Unit, String] (chmod)
                        StdlibSig::ResultUnitPtrI64Args(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let a = self.emit_expr(&args[0])?;
                            let b = self.emit_expr(&args[1])?;
                            return self.emit_stdlib_call_result_unit_ptr_i64_args(&sym, a, b);
                        }
                        // #536: i64 → void/noreturn (exit)
                        StdlibSig::VoidI64Arg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_void_i64_arg(&sym, arg);
                        }
                        // #586: i8 → void (signal_ignore, signal_reset)
                        StdlibSig::VoidI8Arg(sym) if args.len() == 1 => {
                            let sym = sym.clone();
                            let arg = self.emit_expr(&args[0])?;
                            return self.emit_stdlib_call_void_i8_arg(&sym, arg);
                        }
                        // #586: (i8, fn_ptr) → void (signal_on)
                        StdlibSig::VoidI8FnPtrArg(sym) if args.len() == 2 => {
                            let sym = sym.clone();
                            let sig_arg = self.emit_expr(&args[0])?;
                            let fn_ptr = match &args[1] {
                                crate::mvl::parser::ast::Expr::Ident(fn_name, _) => {
                                    self.module.get_function(fn_name.as_str()).map(|f| {
                                        inkwell::values::BasicValueEnum::PointerValue(
                                            f.as_global_value().as_pointer_value(),
                                        )
                                    })
                                }
                                _ => None,
                            }?;
                            return self.emit_stdlib_call_void_i8_fn_ptr_arg(&sym, sig_arg, fn_ptr);
                        }
                        _ => {}
                    }
                }

                // L5-15 + L5-08: single lookup for user-defined function metadata.
                if let Some(fd) = self.fn_decls.get(name).cloned() {
                    // L5-15: mark heap-typed value arguments as moved (ownership
                    // transfers to the callee). Borrow params (val T/ref T) are skipped.
                    for (arg, param) in args.iter().zip(fd.params.iter()) {
                        if matches!(&param.ty, crate::mvl::parser::ast::TypeExpr::Ref { .. }) {
                            continue;
                        }
                        if stmts::heap_kind_of(&param.ty).is_some() {
                            let src = match arg {
                                Expr::Ident(s, _) => Some(s.as_str()),
                                Expr::Move { expr, .. } => match expr.as_ref() {
                                    Expr::Ident(s, _) => Some(s.as_str()),
                                    _ => None,
                                },
                                _ => None,
                            };
                            if let Some(src) = src {
                                self.heap_locals.remove(src);
                            }
                        }
                    }
                    // L5-08: generic function → monomorphize JIT and call the mangled version.
                    // Builtin generic functions (e.g. list_get[T], list_len[T]) already have a
                    // concrete body emitted by the fourth pass of emit_program using pointer-typed
                    // parameters, so no monomorphization is needed — just call the base symbol.
                    if !fd.type_params.is_empty() && !fd.is_builtin {
                        // Emit all arguments first to get their concrete LLVM types.
                        let arg_vals: Vec<BasicValueEnum<'ctx>> =
                            args.iter().filter_map(|a| self.emit_expr(a)).collect();
                        if arg_vals.len() != args.len() {
                            return None;
                        }
                        let type_subs = self.infer_type_subs(&fd, &arg_vals);
                        let mangled = self.mangle_fn_name(&fd, &type_subs);
                        self.ensure_monomorphized(fd, type_subs, &mangled.clone());
                        let fn_val = self.module.get_function(&mangled)?;
                        let meta_args: Vec<inkwell::values::BasicMetadataValueEnum> =
                            arg_vals.iter().map(|v| (*v).into()).collect();
                        let call = self.builder.build_call(fn_val, &meta_args, "call").unwrap();
                        use inkwell::values::AnyValue;
                        return BasicValueEnum::try_from(call.as_any_value_enum()).ok();
                    }
                }
                // Forward call to a user-defined function (already declared).
                let fn_val = self.module.get_function(name)?;
                // If any argument fails to emit, propagate the failure rather than
                // silently substituting undef, which would produce undefined behaviour.
                let meta_args: Vec<inkwell::values::BasicMetadataValueEnum> = args
                    .iter()
                    .map(|a| self.emit_expr(a).map(Into::into))
                    .collect::<Option<Vec<_>>>()?;
                let call = self.builder.build_call(fn_val, &meta_args, "call").unwrap();
                use inkwell::values::AnyValue;
                BasicValueEnum::try_from(call.as_any_value_enum()).ok()
            }
        }
    }

    // ── Collection literals ──────────────────────────────────────────────────

    /// Return the byte size of an LLVM basic type on a 64-bit platform.
    fn llvm_type_byte_size(&self, ty: BasicTypeEnum<'ctx>) -> usize {
        match ty {
            BasicTypeEnum::IntType(it) => it.get_bit_width().div_ceil(8) as usize,
            BasicTypeEnum::FloatType(ft) => {
                // inkwell exposes bit-width via get_bit_width (not available directly);
                // distinguish f32 vs f64 by comparing to the known types.
                if ft == self.context.f64_type() {
                    8
                } else {
                    4
                }
            }
            BasicTypeEnum::PointerType(_) => 8, // 64-bit pointer
            BasicTypeEnum::StructType(st) => {
                // Sum field sizes — good enough for the fixed {i8, [N x i8]} patterns used here.
                st.get_field_types()
                    .iter()
                    .map(|f| self.llvm_type_byte_size(*f))
                    .sum()
            }
            BasicTypeEnum::ArrayType(at) => {
                at.len() as usize * self.llvm_type_byte_size(at.get_element_type())
            }
            BasicTypeEnum::VectorType(_) | BasicTypeEnum::ScalableVectorType(_) => 8,
        }
    }

    /// Emit `[e1, ..., eN]` → `ptr` to a heap-allocated `MvlArray` via `mvl_array_new`.
    ///
    /// L5-14: replaces the stack-allocated stub with proper heap allocation.
    /// All element types are 8 bytes on x86-64/arm64 (i64, f64, ptr are all 8).
    pub(crate) fn emit_list_literal(&mut self, elems: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = elems.len();
        // Emit all element values first (before calling mvl_array_new, in case
        // any element expr has side effects we don't want to reorder).
        let elem_vals: Vec<BasicValueEnum<'ctx>> =
            elems.iter().filter_map(|e| self.emit_expr(e)).collect();

        // elem_size: derive from the first element's LLVM type if available.
        // For empty literals, fall back to the let-binding annotation (pending_let_ty)
        // so that List[Value] (9-byte elements) gets the correct size instead of 8.
        let elem_size = {
            let sz = if let Some(first) = elem_vals.first() {
                self.llvm_type_byte_size(first.get_type())
            } else if let Some(crate::mvl::parser::ast::TypeExpr::Base { args, .. }) =
                self.pending_let_ty.as_ref()
            {
                // Extract element type from List[T] / Array[T] annotation so that
                // empty literals use the correct size (e.g. List[Value] = 9 bytes).
                args.first()
                    .and_then(|elem_ty| self.mvl_type_to_llvm(elem_ty))
                    .map(|t| self.llvm_type_byte_size(t))
                    .unwrap_or(8)
            } else {
                8
            };
            i64_ty.const_int(sz.max(1) as u64, false)
        };
        let initial_cap = i64_ty.const_int(n.max(4) as u64, false);
        let new_fn = self.get_mvl_array_new();
        let call = self
            .builder
            .build_call(new_fn, &[elem_size.into(), initial_cap.into()], "arr_new")
            .unwrap();
        use inkwell::values::AnyValue;
        let arr_ptr = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;

        // Push each element.
        let push_fn = self.get_mvl_array_push();
        for val in elem_vals {
            // Store the element in a temporary stack slot so we can pass a `ptr`.
            let slot = self
                .builder
                .build_alloca(val.get_type(), "elem_slot")
                .unwrap();
            self.builder.build_store(slot, val).unwrap();
            self.builder
                .build_call(push_fn, &[arr_ptr.into(), slot.into()], "arr_push")
                .unwrap();
        }

        Some(arr_ptr)
    }

    /// Emit `{"k": v, ...}` → `ptr` to a heap-allocated `MvlMap` via `mvl_map_new`.
    ///
    /// L5-14: replaces the stub with proper heap allocation.
    pub(crate) fn emit_map_literal(
        &mut self,
        pairs: &[(Expr, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = pairs.len();
        let initial_cap = i64_ty.const_int(n.max(8) as u64, false);
        let new_fn = self.get_mvl_map_new();
        let call = self
            .builder
            .build_call(new_fn, &[initial_cap.into()], "map_new")
            .unwrap();
        use inkwell::values::AnyValue;
        let map_ptr = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;

        let insert_fn = self.get_mvl_map_insert();
        for (key_expr, val_expr) in pairs {
            let key_val = self.emit_expr(key_expr)?;
            let val_val = self.emit_expr(val_expr)?;
            // Keys are String (MvlString*) — pass data ptr + len as key bytes.
            // For non-string keys, store in a slot and use raw bytes.
            let (key_ptr, key_len) = match key_val {
                BasicValueEnum::PointerValue(p) => {
                    // Assume String key: use mvl_string_ptr + mvl_string_len.
                    let sp = self.get_mvl_string_ptr();
                    let sl = self.get_mvl_string_len();
                    let cstr = self
                        .builder
                        .build_call(sp, &[p.into()], "map_key_ptr")
                        .unwrap();
                    let cstr_ptr = BasicValueEnum::try_from(cstr.as_any_value_enum())
                        .ok()?
                        .into_pointer_value();
                    let slen = self
                        .builder
                        .build_call(sl, &[p.into()], "map_key_len")
                        .unwrap();
                    let slen_val = BasicValueEnum::try_from(slen.as_any_value_enum()).ok()?;
                    (cstr_ptr, slen_val)
                }
                other => {
                    let slot = self
                        .builder
                        .build_alloca(other.get_type(), "key_slot")
                        .unwrap();
                    self.builder.build_store(slot, other).unwrap();
                    let size = i64_ty.const_int(8, false);
                    (slot, size.into())
                }
            };
            let val_slot = self
                .builder
                .build_alloca(val_val.get_type(), "val_slot")
                .unwrap();
            self.builder.build_store(val_slot, val_val).unwrap();
            let val_size = i64_ty.const_int(8, false);
            self.builder
                .build_call(
                    insert_fn,
                    &[
                        map_ptr.into(),
                        key_ptr.into(),
                        key_len.into(),
                        val_slot.into(),
                        val_size.into(),
                    ],
                    "map_insert",
                )
                .unwrap();
        }

        Some(map_ptr)
    }

    /// Emit `{e1, ..., eN}` → `ptr` to a heap-allocated `MvlArray` via `mvl_array_new`.
    ///
    /// L5-14: Set uses the same MvlArray backend as List.
    pub(crate) fn emit_set_literal(&mut self, elems: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = elems.len();
        let elem_vals: Vec<BasicValueEnum<'ctx>> =
            elems.iter().filter_map(|e| self.emit_expr(e)).collect();

        let elem_size = {
            let sz = elem_vals
                .first()
                .map(|v| self.llvm_type_byte_size(v.get_type()))
                .unwrap_or(8)
                .max(1) as u64;
            i64_ty.const_int(sz, false)
        };
        let initial_cap = i64_ty.const_int(n.max(4) as u64, false);
        let new_fn = self.get_mvl_array_new();
        let call = self
            .builder
            .build_call(new_fn, &[elem_size.into(), initial_cap.into()], "set_new")
            .unwrap();
        use inkwell::values::AnyValue;
        let set_ptr = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;

        let push_fn = self.get_mvl_array_push();
        for val in elem_vals {
            let slot = self
                .builder
                .build_alloca(val.get_type(), "set_elem_slot")
                .unwrap();
            self.builder.build_store(slot, val).unwrap();
            self.builder
                .build_call(push_fn, &[set_ptr.into(), slot.into()], "set_push")
                .unwrap();
        }

        Some(set_ptr)
    }

    // ── Method call emission ─────────────────────────────────────────────────

    /// Emit `receiver.method(args)`.
    pub(crate) fn emit_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let recv_val = self.emit_expr(receiver)?;
        match method {
            // ── len ──────────────────────────────────────────────────────────
            "len" => match recv_val {
                // L5-14: all collection ptrs use runtime len functions.
                // Dispatch by MVL type; default to mvl_string_len for unknown ptrs.
                BasicValueEnum::PointerValue(ptr) => {
                    let recv_mvl_ty = match receiver {
                        Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned(),
                        _ => None,
                    };
                    // Strip IFC labels (Secret[List[T]] → List[T]) before dispatching.
                    let base_name = recv_mvl_ty.as_ref().and_then(|t| {
                        let inner = match t {
                            TypeExpr::Labeled { inner, .. } => inner.as_ref(),
                            other => other,
                        };
                        match inner {
                            TypeExpr::Base { name, .. } => Some(name.as_str()),
                            _ => None,
                        }
                    });
                    let len_fn = match base_name {
                        Some("List") | Some("Array") | Some("Set") => self.get_mvl_array_len(),
                        Some("Map") => self.get_mvl_map_len(),
                        _ => self.get_mvl_string_len(), // String or unknown ptr
                    };
                    let call = self
                        .builder
                        .build_call(len_fn, &[ptr.into()], "coll_len")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                }
                BasicValueEnum::StructValue(sv) => {
                    // Legacy Range struct: { i64 start, i64 end } → end - start
                    let n = sv.get_type().count_fields();
                    if n == 2 {
                        let f1_ty = sv.get_type().get_field_type_at_index(1).unwrap();
                        if matches!(f1_ty, BasicTypeEnum::IntType(_)) {
                            let s = self
                                .builder
                                .build_extract_value(sv, 0, "r_s")
                                .ok()?
                                .into_int_value();
                            let e = self
                                .builder
                                .build_extract_value(sv, 1, "r_e")
                                .ok()?
                                .into_int_value();
                            return Some(
                                self.builder
                                    .build_int_sub(e, s, "range_len")
                                    .unwrap()
                                    .into(),
                            );
                        }
                    }
                    None
                }
                _ => None,
            },

            // ── concat (String) ───────────────────────────────────────────────
            "concat" => {
                let arg = self.emit_expr(args.first()?)?;
                match (recv_val, arg) {
                    (BasicValueEnum::PointerValue(a), BasicValueEnum::PointerValue(b)) => {
                        let concat_fn = self.get_mvl_string_concat();
                        let call = self
                            .builder
                            .build_call(concat_fn, &[a.into(), b.into()], "str_concat")
                            .unwrap();
                        use inkwell::values::AnyValue;
                        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                    }
                    _ => None,
                }
            }

            // ── to_lower / to_upper (String) ─────────────────────────────────
            "to_lower" => match recv_val {
                BasicValueEnum::PointerValue(ptr) => {
                    let f = self.get_mvl_str_to_lower();
                    let call = self
                        .builder
                        .build_call(f, &[ptr.into()], "to_lower")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                }
                _ => None,
            },

            "to_upper" => match recv_val {
                BasicValueEnum::PointerValue(ptr) => {
                    let f = self.get_mvl_str_to_upper();
                    let call = self
                        .builder
                        .build_call(f, &[ptr.into()], "to_upper")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                }
                _ => None,
            },

            // ── parse_int / parse_float (String → Result) ─────────────────────
            "parse_int" => match recv_val {
                BasicValueEnum::PointerValue(ptr) => self.emit_parse_int(ptr),
                _ => None,
            },

            "parse_float" => match recv_val {
                BasicValueEnum::PointerValue(ptr) => self.emit_parse_float(ptr),
                _ => None,
            },

            // ── clamp (Int) ───────────────────────────────────────────────────
            "clamp" if args.len() == 2 => {
                let lo = self.emit_expr(&args[0])?.into_int_value();
                let hi = self.emit_expr(&args[1])?.into_int_value();
                match recv_val {
                    BasicValueEnum::IntValue(n) => {
                        use inkwell::IntPredicate;
                        let gt_hi = self
                            .builder
                            .build_int_compare(IntPredicate::SGT, n, hi, "gt_hi")
                            .unwrap();
                        let after_hi = self
                            .builder
                            .build_select(gt_hi, hi, n, "min_hi")
                            .unwrap()
                            .into_int_value();
                        let lt_lo = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, after_hi, lo, "lt_lo")
                            .unwrap();
                        let result = self
                            .builder
                            .build_select(lt_lo, lo, after_hi, "clamped")
                            .unwrap();
                        Some(result)
                    }
                    _ => None,
                }
            }

            // ── push (Array / List) ───────────────────────────────────────────
            "push" => {
                let elem = self.emit_expr(args.first()?)?;
                match recv_val {
                    BasicValueEnum::PointerValue(arr) => {
                        let slot = self
                            .builder
                            .build_alloca(elem.get_type(), "push_slot")
                            .unwrap();
                        self.builder.build_store(slot, elem).unwrap();
                        let push_fn = self.get_mvl_array_push();
                        self.builder
                            .build_call(push_fn, &[arr.into(), slot.into()], "arr_push")
                            .unwrap();
                        None
                    }
                    _ => None,
                }
            }

            // ── get (Array / List) ────────────────────────────────────────────
            "get" => {
                let key_val = self.emit_expr(args.first()?)?;
                match (recv_val, key_val) {
                    // List/Array.get(i: i64) → mvl_array_get returns raw ptr (Option<T>)
                    (BasicValueEnum::PointerValue(arr), BasicValueEnum::IntValue(i)) => {
                        let get_fn = self.get_mvl_array_get();
                        let call = self
                            .builder
                            .build_call(get_fn, &[arr.into(), i.into()], "arr_get")
                            .unwrap();
                        use inkwell::values::AnyValue;
                        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                    }
                    // Map.get(key: String) → mvl_map_get + build Option<V>
                    (BasicValueEnum::PointerValue(map), BasicValueEnum::PointerValue(key_ptr)) => {
                        self.emit_map_get(receiver, map, key_ptr, args)
                    }
                    _ => None,
                }
            }

            // ── Map.insert(k, v) ─────────────────────────────────────────────
            "insert" if args.len() == 2 => {
                let k_val = self.emit_expr(&args[0])?;
                let v_val = self.emit_expr(&args[1])?;
                match recv_val {
                    BasicValueEnum::PointerValue(map) => {
                        // Map.insert(k: String, v) — store key as string bytes, value as 8-byte slot
                        let i64_ty = self.context.i64_type();
                        let insert_fn = self.get_mvl_map_insert();
                        let (key_ptr, key_len) = match k_val {
                            BasicValueEnum::PointerValue(p) => {
                                let sp = self.get_mvl_string_ptr();
                                let sl = self.get_mvl_string_len();
                                use inkwell::values::AnyValue;
                                let cp = BasicValueEnum::try_from(
                                    self.builder
                                        .build_call(sp, &[p.into()], "ins_kp")
                                        .unwrap()
                                        .as_any_value_enum(),
                                )
                                .ok()?
                                .into_pointer_value();
                                let cl = BasicValueEnum::try_from(
                                    self.builder
                                        .build_call(sl, &[p.into()], "ins_kl")
                                        .unwrap()
                                        .as_any_value_enum(),
                                )
                                .ok()?;
                                (cp, cl)
                            }
                            other => {
                                let slot = self
                                    .builder
                                    .build_alloca(other.get_type(), "ins_k_slot")
                                    .unwrap();
                                self.builder.build_store(slot, other).unwrap();
                                (slot, i64_ty.const_int(8, false).into())
                            }
                        };
                        let val_slot = self
                            .builder
                            .build_alloca(v_val.get_type(), "ins_v_slot")
                            .unwrap();
                        self.builder.build_store(val_slot, v_val).unwrap();
                        let val_size = i64_ty.const_int(8, false);
                        self.builder
                            .build_call(
                                insert_fn,
                                &[
                                    map.into(),
                                    key_ptr.into(),
                                    key_len.into(),
                                    val_slot.into(),
                                    val_size.into(),
                                ],
                                "map_insert",
                            )
                            .unwrap();
                        None // Unit return
                    }
                    _ => None,
                }
            }

            // ── Set.insert(x) ────────────────────────────────────────────────
            "insert" if args.len() == 1 => {
                let elem = self.emit_expr(args.first()?)?;
                match recv_val {
                    BasicValueEnum::PointerValue(arr) => {
                        let slot = self
                            .builder
                            .build_alloca(elem.get_type(), "set_ins_slot")
                            .unwrap();
                        self.builder.build_store(slot, elem).unwrap();
                        let push_fn = self.get_mvl_array_push();
                        self.builder
                            .build_call(push_fn, &[arr.into(), slot.into()], "set_ins")
                            .unwrap();
                        None // Unit return
                    }
                    _ => None,
                }
            }

            // ── Map.contains_key(key) ─────────────────────────────────────────
            "contains_key" if args.len() == 1 => {
                let key_val = self.emit_expr(args.first()?)?;
                match (recv_val, key_val) {
                    (BasicValueEnum::PointerValue(map), BasicValueEnum::PointerValue(key_str)) => {
                        let sp = self.get_mvl_string_ptr();
                        let sl = self.get_mvl_string_len();
                        use inkwell::values::AnyValue;
                        let key_data = BasicValueEnum::try_from(
                            self.builder
                                .build_call(sp, &[key_str.into()], "ck_kp")
                                .unwrap()
                                .as_any_value_enum(),
                        )
                        .ok()?
                        .into_pointer_value();
                        let key_len_val = BasicValueEnum::try_from(
                            self.builder
                                .build_call(sl, &[key_str.into()], "ck_kl")
                                .unwrap()
                                .as_any_value_enum(),
                        )
                        .ok()?
                        .into_int_value();
                        let get_fn = self.get_mvl_map_get();
                        let raw_ptr_call = self
                            .builder
                            .build_call(
                                get_fn,
                                &[map.into(), key_data.into(), key_len_val.into()],
                                "ck_raw",
                            )
                            .unwrap();
                        let raw_ptr = BasicValueEnum::try_from(raw_ptr_call.as_any_value_enum())
                            .ok()?
                            .into_pointer_value();
                        let null = self
                            .context
                            .ptr_type(inkwell::AddressSpace::default())
                            .const_null();
                        Some(
                            self.builder
                                .build_int_compare(
                                    IntPredicate::NE,
                                    raw_ptr,
                                    null,
                                    "contains_key_res",
                                )
                                .unwrap()
                                .into(),
                        )
                    }
                    _ => None,
                }
            }

            // ── Map.keys() ───────────────────────────────────────────────────
            "keys" if args.is_empty() => match recv_val {
                BasicValueEnum::PointerValue(map) => {
                    let keys_fn = self.get_mvl_map_keys();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(
                        self.builder
                            .build_call(keys_fn, &[map.into()], "map_keys")
                            .unwrap()
                            .as_any_value_enum(),
                    )
                    .ok()
                }
                _ => None,
            },

            // ── Map.remove(key) ──────────────────────────────────────────────
            "remove" if args.len() == 1 => {
                let key_val = self.emit_expr(args.first()?)?;
                match (recv_val, key_val) {
                    (BasicValueEnum::PointerValue(map), BasicValueEnum::PointerValue(key_str)) => {
                        let sp = self.get_mvl_string_ptr();
                        let sl = self.get_mvl_string_len();
                        use inkwell::values::AnyValue;
                        let key_data = BasicValueEnum::try_from(
                            self.builder
                                .build_call(sp, &[key_str.into()], "rm_kp")
                                .unwrap()
                                .as_any_value_enum(),
                        )
                        .ok()?
                        .into_pointer_value();
                        let key_len_val = BasicValueEnum::try_from(
                            self.builder
                                .build_call(sl, &[key_str.into()], "rm_kl")
                                .unwrap()
                                .as_any_value_enum(),
                        )
                        .ok()?
                        .into_int_value();
                        let remove_fn = self.get_mvl_map_remove();
                        self.builder
                            .build_call(
                                remove_fn,
                                &[map.into(), key_data.into(), key_len_val.into()],
                                "map_remove",
                            )
                            .unwrap();
                        // remove() returns Unit; yield a dummy i64 0
                        Some(self.context.i64_type().const_int(0, false).into())
                    }
                    _ => None,
                }
            }

            // ── is_empty() ───────────────────────────────────────────────────
            // Reuse the len arm result and compare to zero.
            "is_empty" if args.is_empty() => {
                let len_val = self.emit_method_call(receiver, "len", &[])?;
                match len_val {
                    BasicValueEnum::IntValue(n) => {
                        let zero = self.context.i64_type().const_int(0, false);
                        Some(
                            self.builder
                                .build_int_compare(IntPredicate::EQ, n, zero, "is_empty_res")
                                .unwrap()
                                .into(),
                        )
                    }
                    _ => None,
                }
            }

            // ── Set.to_list() ────────────────────────────────────────────────
            // Set is backed by MvlArray; clone it and return as List.
            "to_list" if args.is_empty() => match recv_val {
                BasicValueEnum::PointerValue(arr) => {
                    let clone_fn = self.get_mvl_array_clone();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(
                        self.builder
                            .build_call(clone_fn, &[arr.into()], "set_to_list")
                            .unwrap()
                            .as_any_value_enum(),
                    )
                    .ok()
                }
                _ => None,
            },

            // ── Int math ─────────────────────────────────────────────────────
            "abs" => match recv_val {
                BasicValueEnum::IntValue(v) => {
                    let zero = self.context.i64_type().const_int(0, false);
                    let is_neg = self
                        .builder
                        .build_int_compare(IntPredicate::SLT, v, zero, "is_neg")
                        .unwrap();
                    let neg = self.builder.build_int_neg(v, "neg_v").unwrap();
                    Some(self.builder.build_select(is_neg, neg, v, "abs_v").unwrap())
                }
                _ => None,
            },
            "min" => {
                let arg = match self.emit_expr(args.first()?)? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                match recv_val {
                    BasicValueEnum::IntValue(a) => {
                        let lt = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, a, arg, "lt")
                            .unwrap();
                        Some(self.builder.build_select(lt, a, arg, "min_v").unwrap())
                    }
                    _ => None,
                }
            }
            "max" => {
                let arg = match self.emit_expr(args.first()?)? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                match recv_val {
                    BasicValueEnum::IntValue(a) => {
                        let gt = self
                            .builder
                            .build_int_compare(IntPredicate::SGT, a, arg, "gt")
                            .unwrap();
                        Some(self.builder.build_select(gt, a, arg, "max_v").unwrap())
                    }
                    _ => None,
                }
            }

            // ── Float intrinsics ──────────────────────────────────────────────
            "ceil" | "floor" | "sqrt" => match recv_val {
                BasicValueEnum::FloatValue(v) => {
                    let name = format!("llvm.{method}.f64");
                    let f64_ty = self.context.f64_type();
                    let fn_val = self.module.get_function(&name).unwrap_or_else(|| {
                        let fn_ty = f64_ty.fn_type(&[f64_ty.into()], false);
                        self.module.add_function(&name, fn_ty, None)
                    });
                    let call = self
                        .builder
                        .build_call(fn_val, &[v.into()], "fintrinsic")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                }
                _ => None,
            },

            // ── List.first() → Option[Int] ────────────────────────────────────
            "first" => match recv_val {
                BasicValueEnum::StructValue(sv) if sv.get_type().count_fields() == 2 => {
                    // Pre-Phase C struct layout (kept for compatibility)
                    let len = self
                        .builder
                        .build_extract_value(sv, 0, "lst_len")
                        .ok()?
                        .into_int_value();
                    let data_ptr = self
                        .builder
                        .build_extract_value(sv, 1, "lst_data")
                        .ok()?
                        .into_pointer_value();
                    let i64_ty = self.context.i64_type();
                    let first = self
                        .builder
                        .build_load(i64_ty, data_ptr, "first_elem")
                        .unwrap()
                        .into_int_value();
                    let parent_fn = self.builder.get_insert_block()?.get_parent()?;
                    let some_bb = self.context.append_basic_block(parent_fn, "first_some");
                    let none_bb = self.context.append_basic_block(parent_fn, "first_none");
                    let merge_bb = self.context.append_basic_block(parent_fn, "first_merge");
                    let zero = i64_ty.const_int(0, false);
                    let nonempty = self
                        .builder
                        .build_int_compare(IntPredicate::SGT, len, zero, "nonempty")
                        .unwrap();
                    self.builder
                        .build_conditional_branch(nonempty, some_bb, none_bb)
                        .unwrap();
                    // Some branch
                    self.builder.position_at_end(some_bb);
                    let some_val = self.emit_some_from_val(first.into())?;
                    let some_end = self.builder.get_insert_block()?;
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    // None branch
                    self.builder.position_at_end(none_bb);
                    let none_val = self.emit_none_val()?;
                    let none_end = self.builder.get_insert_block()?;
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    // Merge
                    self.builder.position_at_end(merge_bb);
                    if some_val.get_type() == none_val.get_type() {
                        let phi = self
                            .builder
                            .build_phi(some_val.get_type(), "first_result")
                            .unwrap();
                        phi.add_incoming(&[(&some_val, some_end), (&none_val, none_end)]);
                        Some(phi.as_basic_value())
                    } else {
                        None
                    }
                }
                // Post-Phase C: List is MvlArray* (heap pointer). Use runtime
                // mvl_array_len + mvl_array_get to access element 0.
                BasicValueEnum::PointerValue(arr_ptr) => {
                    use inkwell::values::AnyValue;
                    let i64_ty = self.context.i64_type();
                    // Get length via runtime call
                    let len_fn = self.get_mvl_array_len();
                    let len_call = self
                        .builder
                        .build_call(len_fn, &[arr_ptr.into()], "arr_len")
                        .ok()?;
                    let len = BasicValueEnum::try_from(len_call.as_any_value_enum())
                        .ok()?
                        .into_int_value();
                    let parent_fn = self.builder.get_insert_block()?.get_parent()?;
                    let some_bb = self.context.append_basic_block(parent_fn, "first_some");
                    let none_bb = self.context.append_basic_block(parent_fn, "first_none");
                    let merge_bb = self.context.append_basic_block(parent_fn, "first_merge");
                    let zero = i64_ty.const_int(0, false);
                    let nonempty = self
                        .builder
                        .build_int_compare(IntPredicate::SGT, len, zero, "nonempty")
                        .unwrap();
                    self.builder
                        .build_conditional_branch(nonempty, some_bb, none_bb)
                        .unwrap();
                    // Some branch: get element pointer at index 0, load i64
                    self.builder.position_at_end(some_bb);
                    let get_fn = self.get_mvl_array_get();
                    let elem_ptr_call = self
                        .builder
                        .build_call(get_fn, &[arr_ptr.into(), zero.into()], "elem_ptr")
                        .ok()?;
                    let elem_ptr = BasicValueEnum::try_from(elem_ptr_call.as_any_value_enum())
                        .ok()?
                        .into_pointer_value();
                    let first = self
                        .builder
                        .build_load(i64_ty, elem_ptr, "first_elem")
                        .unwrap()
                        .into_int_value();
                    let some_val = self.emit_some_from_val(first.into())?;
                    let some_end = self.builder.get_insert_block()?;
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    // None branch
                    self.builder.position_at_end(none_bb);
                    let none_val = self.emit_none_val()?;
                    let none_end = self.builder.get_insert_block()?;
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                    // Merge
                    self.builder.position_at_end(merge_bb);
                    if some_val.get_type() == none_val.get_type() {
                        let phi = self
                            .builder
                            .build_phi(some_val.get_type(), "first_result")
                            .unwrap();
                        phi.add_incoming(&[(&some_val, some_end), (&none_val, none_end)]);
                        Some(phi.as_basic_value())
                    } else {
                        None
                    }
                }
                _ => None,
            },

            // ── Set.contains(v) → Bool ────────────────────────────────────────
            "contains" => {
                let needle = match self.emit_expr(args.first()?)? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                match recv_val {
                    BasicValueEnum::StructValue(sv) if sv.get_type().count_fields() == 2 => {
                        // Pre-Phase C struct layout (kept for compatibility)
                        let len = self
                            .builder
                            .build_extract_value(sv, 0, "set_len")
                            .ok()?
                            .into_int_value();
                        let data_ptr = self
                            .builder
                            .build_extract_value(sv, 1, "set_data")
                            .ok()?
                            .into_pointer_value();
                        let i64_ty = self.context.i64_type();
                        let bool_ty = self.context.bool_type();
                        let found_alloca = self.builder.build_alloca(bool_ty, "found").unwrap();
                        let i_alloca = self.builder.build_alloca(i64_ty, "set_i").unwrap();
                        self.builder
                            .build_store(found_alloca, bool_ty.const_int(0, false))
                            .unwrap();
                        self.builder
                            .build_store(i_alloca, i64_ty.const_int(0, false))
                            .unwrap();
                        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
                        let cond_bb = self.context.append_basic_block(parent_fn, "set_cond");
                        let body_bb = self.context.append_basic_block(parent_fn, "set_body");
                        let exit_bb = self.context.append_basic_block(parent_fn, "set_exit");
                        self.builder.build_unconditional_branch(cond_bb).unwrap();
                        // Cond: i < len && !found
                        self.builder.position_at_end(cond_bb);
                        let i = self
                            .builder
                            .build_load(i64_ty, i_alloca, "i")
                            .unwrap()
                            .into_int_value();
                        let found = self
                            .builder
                            .build_load(bool_ty, found_alloca, "f")
                            .unwrap()
                            .into_int_value();
                        let i_lt = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, i, len, "i_lt")
                            .unwrap();
                        let not_found = self.builder.build_not(found, "nf").unwrap();
                        let go = self.builder.build_and(i_lt, not_found, "go").unwrap();
                        self.builder
                            .build_conditional_branch(go, body_bb, exit_bb)
                            .unwrap();
                        // Body
                        self.builder.position_at_end(body_bb);
                        let elem_ptr = unsafe {
                            self.builder
                                .build_gep(i64_ty, data_ptr, &[i], "ep")
                                .unwrap()
                        };
                        let elem = self
                            .builder
                            .build_load(i64_ty, elem_ptr, "elem")
                            .unwrap()
                            .into_int_value();
                        let eq = self
                            .builder
                            .build_int_compare(IntPredicate::EQ, elem, needle, "eq")
                            .unwrap();
                        self.builder.build_store(found_alloca, eq).unwrap();
                        let i_next = self
                            .builder
                            .build_int_add(i, i64_ty.const_int(1, false), "i_next")
                            .unwrap();
                        self.builder.build_store(i_alloca, i_next).unwrap();
                        self.builder.build_unconditional_branch(cond_bb).unwrap();
                        // Exit
                        self.builder.position_at_end(exit_bb);
                        Some(
                            self.builder
                                .build_load(bool_ty, found_alloca, "contains_res")
                                .unwrap(),
                        )
                    }
                    // Post-Phase C: Set is MvlArray* (heap pointer). Use runtime
                    // mvl_array_len + mvl_array_get to iterate and compare elements.
                    BasicValueEnum::PointerValue(arr_ptr) => {
                        use inkwell::values::AnyValue;
                        let i64_ty = self.context.i64_type();
                        let bool_ty = self.context.bool_type();
                        // Get length via runtime call
                        let len_fn = self.get_mvl_array_len();
                        let len_call = self
                            .builder
                            .build_call(len_fn, &[arr_ptr.into()], "arr_len")
                            .ok()?;
                        let len = BasicValueEnum::try_from(len_call.as_any_value_enum())
                            .ok()?
                            .into_int_value();
                        let found_alloca = self.builder.build_alloca(bool_ty, "found").unwrap();
                        let i_alloca = self.builder.build_alloca(i64_ty, "set_i").unwrap();
                        self.builder
                            .build_store(found_alloca, bool_ty.const_int(0, false))
                            .unwrap();
                        self.builder
                            .build_store(i_alloca, i64_ty.const_int(0, false))
                            .unwrap();
                        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
                        let cond_bb = self.context.append_basic_block(parent_fn, "set_cond");
                        let body_bb = self.context.append_basic_block(parent_fn, "set_body");
                        let exit_bb = self.context.append_basic_block(parent_fn, "set_exit");
                        self.builder.build_unconditional_branch(cond_bb).unwrap();
                        // Cond: i < len && !found
                        self.builder.position_at_end(cond_bb);
                        let i = self
                            .builder
                            .build_load(i64_ty, i_alloca, "i")
                            .unwrap()
                            .into_int_value();
                        let found = self
                            .builder
                            .build_load(bool_ty, found_alloca, "f")
                            .unwrap()
                            .into_int_value();
                        let i_lt = self
                            .builder
                            .build_int_compare(IntPredicate::SLT, i, len, "i_lt")
                            .unwrap();
                        let not_found = self.builder.build_not(found, "nf").unwrap();
                        let go = self.builder.build_and(i_lt, not_found, "go").unwrap();
                        self.builder
                            .build_conditional_branch(go, body_bb, exit_bb)
                            .unwrap();
                        // Body: fetch element pointer via runtime, load i64, compare
                        self.builder.position_at_end(body_bb);
                        let get_fn = self.get_mvl_array_get();
                        let elem_ptr_call = self
                            .builder
                            .build_call(get_fn, &[arr_ptr.into(), i.into()], "ep")
                            .ok()?;
                        let elem_ptr = BasicValueEnum::try_from(elem_ptr_call.as_any_value_enum())
                            .ok()?
                            .into_pointer_value();
                        let elem = self
                            .builder
                            .build_load(i64_ty, elem_ptr, "elem")
                            .unwrap()
                            .into_int_value();
                        let eq = self
                            .builder
                            .build_int_compare(IntPredicate::EQ, elem, needle, "eq")
                            .unwrap();
                        self.builder.build_store(found_alloca, eq).unwrap();
                        let i_next = self
                            .builder
                            .build_int_add(i, i64_ty.const_int(1, false), "i_next")
                            .unwrap();
                        self.builder.build_store(i_alloca, i_next).unwrap();
                        self.builder.build_unconditional_branch(cond_bb).unwrap();
                        // Exit
                        self.builder.position_at_end(exit_bb);
                        Some(
                            self.builder
                                .build_load(bool_ty, found_alloca, "contains_res")
                                .unwrap(),
                        )
                    }
                    _ => None,
                }
            }

            // ── to_string ────────────────────────────────────────────────────
            "to_string" => match recv_val {
                BasicValueEnum::IntValue(v) => Some(self.emit_int_to_string(v)),
                BasicValueEnum::FloatValue(v) => Some(self.emit_float_to_string(v)),
                BasicValueEnum::PointerValue(p) => Some(p.into()),
                _ => None,
            },

            // ── to_float (Int → f64, needed by json decode number parser) ────
            "to_float" => match recv_val {
                BasicValueEnum::IntValue(v) => {
                    let f64_ty = self.context.f64_type();
                    Some(
                        self.builder
                            .build_signed_int_to_float(v, f64_ty, "itof")
                            .unwrap()
                            .into(),
                    )
                }
                BasicValueEnum::FloatValue(v) => Some(v.into()),
                _ => None,
            },

            _ => None,
        }
    }

    /// Emit `Map[K, V].get(key)` → `Option[V]` via `mvl_map_get`.
    ///
    /// Returns a tagged-union `{ i8 disc, ptr payload }` (L5-08 layout):
    ///   disc=0 → Some, disc=1 → None.
    /// The value type is inferred from the receiver's tracked MVL type; falls
    /// back to pointer (String/opaque) when unknown.
    fn emit_map_get(
        &mut self,
        receiver: &Expr,
        map: inkwell::values::PointerValue<'ctx>,
        key_str: inkwell::values::PointerValue<'ctx>,
        _args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        use inkwell::values::AnyValue;
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        // Extract key bytes from MvlString.
        let sp = self.get_mvl_string_ptr();
        let sl = self.get_mvl_string_len();
        let key_data = BasicValueEnum::try_from(
            self.builder
                .build_call(sp, &[key_str.into()], "mg_kp")
                .unwrap()
                .as_any_value_enum(),
        )
        .ok()?
        .into_pointer_value();
        let key_len = BasicValueEnum::try_from(
            self.builder
                .build_call(sl, &[key_str.into()], "mg_kl")
                .unwrap()
                .as_any_value_enum(),
        )
        .ok()?
        .into_int_value();

        // Call mvl_map_get → *const u8 (null if absent).
        let get_fn = self.get_mvl_map_get();
        let raw_ptr_call = self
            .builder
            .build_call(
                get_fn,
                &[map.into(), key_data.into(), key_len.into()],
                "mg_raw",
            )
            .unwrap();
        let raw_ptr = BasicValueEnum::try_from(raw_ptr_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();

        // Determine the value LLVM type from the receiver's tracked MVL type.
        let recv_mvl_ty = match receiver {
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned(),
            _ => None,
        };
        let val_llvm_ty: Option<inkwell::types::BasicTypeEnum<'ctx>> =
            recv_mvl_ty.as_ref().and_then(|t| {
                if let TypeExpr::Base { args, .. } = t {
                    args.get(1).and_then(|vty| self.mvl_type_to_llvm(vty))
                } else {
                    None
                }
            });

        // Branch: null → None, non-null → Some(load value).
        // Uses the same { i8 disc, ptr payload } Option layout as emit_some_from_val.
        let null = ptr_ty.const_null();
        let is_null = self
            .builder
            .build_int_compare(IntPredicate::EQ, raw_ptr, null, "mg_isnull")
            .unwrap();

        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
        let some_bb = self.context.append_basic_block(parent_fn, "mg_some");
        let none_bb = self.context.append_basic_block(parent_fn, "mg_none");
        let merge_bb = self.context.append_basic_block(parent_fn, "mg_merge");

        self.builder
            .build_conditional_branch(is_null, none_bb, some_bb)
            .unwrap();

        // Some branch: load the value bytes, wrap in Some{disc=0}.
        self.builder.position_at_end(some_bb);
        let loaded: BasicValueEnum<'ctx> = match val_llvm_ty {
            Some(inkwell::types::BasicTypeEnum::IntType(it)) => {
                self.builder.build_load(it, raw_ptr, "mg_load").unwrap()
            }
            Some(inkwell::types::BasicTypeEnum::FloatType(ft)) => {
                self.builder.build_load(ft, raw_ptr, "mg_load").unwrap()
            }
            _ => {
                // Default: pointer value (String or opaque struct).
                self.builder.build_load(ptr_ty, raw_ptr, "mg_load").unwrap()
            }
        };
        let some_val = self.emit_some_from_val(loaded)?;
        self.builder.build_unconditional_branch(merge_bb).unwrap();
        let some_end = self.builder.get_insert_block()?;

        // None branch: emit None{disc=1}.
        self.builder.position_at_end(none_bb);
        let none_val = self.emit_none_val()?;
        self.builder.build_unconditional_branch(merge_bb).unwrap();
        let none_end = self.builder.get_insert_block()?;

        // Merge: phi over the same Option struct type ({i8, ptr}).
        self.builder.position_at_end(merge_bb);
        if some_val.get_type() == none_val.get_type() {
            let phi = self
                .builder
                .build_phi(some_val.get_type(), "mg_opt")
                .unwrap();
            phi.add_incoming(&[(&some_val, some_end), (&none_val, none_end)]);
            Some(phi.as_basic_value())
        } else {
            // Should not happen: Some/None layouts must match (both use {i8, ptr}).
            // Emit unreachable so merge_bb has a terminator and the IR stays valid.
            self.builder.build_unreachable().unwrap();
            None
        }
    }

    /// Build a `Some(val)` tagged union `{ i8 disc=0, ptr }` (L5-08 pointer layout).
    pub(crate) fn emit_some_from_val(
        &mut self,
        val: BasicValueEnum<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "some_tmp").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "some_disc")
            .unwrap();
        self.builder
            .build_store(disc_ptr, self.context.i8_type().const_int(0, false))
            .unwrap();
        let val_alloca = self
            .builder
            .build_alloca(val.get_type(), "some_payload_tmp")
            .unwrap();
        self.builder.build_store(val_alloca, val).unwrap();
        let payload_slot = self
            .builder
            .build_struct_gep(result_ty, alloca, 1, "some_payload")
            .unwrap();
        self.builder.build_store(payload_slot, val_alloca).unwrap();
        Some(
            self.builder
                .build_load(result_ty, alloca, "some_val")
                .unwrap(),
        )
    }

    /// Build a `None` tagged union `{ i8 disc=1, ptr=null }` (L5-08 pointer layout).
    pub(crate) fn emit_none_val(&mut self) -> Option<BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "none_tmp").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "none_disc")
            .unwrap();
        self.builder
            .build_store(disc_ptr, self.context.i8_type().const_int(1, false))
            .unwrap();
        let payload_slot = self
            .builder
            .build_struct_gep(result_ty, alloca, 1, "none_payload")
            .unwrap();
        self.builder
            .build_store(payload_slot, ptr_ty.const_null())
            .unwrap();
        Some(
            self.builder
                .build_load(result_ty, alloca, "none_val")
                .unwrap(),
        )
    }

    // ── #587: set algebra inline helpers ─────────────────────────────────────

    /// Push `val` (i64) into `arr_ptr` (MvlArray*) by storing to a stack slot.
    fn emit_set_push_i64_slot(
        &mut self,
        arr_ptr: inkwell::values::PointerValue<'ctx>,
        val: inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.context.i64_type();
        let slot = self.builder.build_alloca(i64_ty, "set_slot").unwrap();
        self.builder.build_store(slot, val).unwrap();
        let push_fn = self.get_mvl_array_push();
        self.builder
            .build_call(push_fn, &[arr_ptr.into(), slot.into()], "")
            .unwrap();
    }

    /// Emit a linear-scan contains check for `arr_ptr` (MvlArray*).
    ///
    /// Returns an `i1` IntValue: 1 if `needle` is found, 0 otherwise.
    /// `label` is used as a prefix for basic-block names to avoid collisions.
    fn emit_set_contains_loop(
        &mut self,
        arr_ptr: inkwell::values::PointerValue<'ctx>,
        needle: inkwell::values::IntValue<'ctx>,
        label: &str,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        use inkwell::values::AnyValue;
        let i64_ty = self.context.i64_type();
        let bool_ty = self.context.bool_type();

        let len_fn = self.get_mvl_array_len();
        let len_call = self
            .builder
            .build_call(len_fn, &[arr_ptr.into()], "len")
            .ok()?;
        let len = inkwell::values::BasicValueEnum::try_from(len_call.as_any_value_enum())
            .ok()?
            .into_int_value();

        let found_slot = self.builder.build_alloca(bool_ty, "found").unwrap();
        let j_slot = self.builder.build_alloca(i64_ty, "j").unwrap();
        self.builder
            .build_store(found_slot, bool_ty.const_int(0, false))
            .unwrap();
        self.builder
            .build_store(j_slot, i64_ty.const_int(0, false))
            .unwrap();

        let parent = self.builder.get_insert_block()?.get_parent()?;
        let cond_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_cond"));
        let body_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_body"));
        let exit_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_exit"));

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Cond: j < len && !found
        self.builder.position_at_end(cond_bb);
        let j = self
            .builder
            .build_load(i64_ty, j_slot, "j")
            .unwrap()
            .into_int_value();
        let found = self
            .builder
            .build_load(bool_ty, found_slot, "found")
            .unwrap()
            .into_int_value();
        let j_lt = self
            .builder
            .build_int_compare(IntPredicate::SLT, j, len, "j_lt")
            .unwrap();
        let not_found = self.builder.build_not(found, "nf").unwrap();
        let go = self.builder.build_and(j_lt, not_found, "go").unwrap();
        self.builder
            .build_conditional_branch(go, body_bb, exit_bb)
            .unwrap();

        // Body: load element, compare, update found, advance j
        self.builder.position_at_end(body_bb);
        let get_fn = self.get_mvl_array_get();
        let ep_call = self
            .builder
            .build_call(get_fn, &[arr_ptr.into(), j.into()], "ep")
            .ok()?;
        let ep = inkwell::values::BasicValueEnum::try_from(ep_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        let elem = self
            .builder
            .build_load(i64_ty, ep, "elem")
            .unwrap()
            .into_int_value();
        let eq = self
            .builder
            .build_int_compare(IntPredicate::EQ, elem, needle, "eq")
            .unwrap();
        // found |= eq  (once set, stays set)
        let found_new = self.builder.build_or(found, eq, "found_new").unwrap();
        self.builder.build_store(found_slot, found_new).unwrap();
        let j_next = self
            .builder
            .build_int_add(j, i64_ty.const_int(1, false), "j_next")
            .unwrap();
        self.builder.build_store(j_slot, j_next).unwrap();
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Exit
        self.builder.position_at_end(exit_bb);
        Some(
            self.builder
                .build_load(bool_ty, found_slot, "contains_r")
                .unwrap()
                .into_int_value(),
        )
    }

    /// Iterate `src` (MvlArray*); push each element to `dst` (MvlArray*) if
    /// `include_if_found == contains(filter, elem)`.
    ///
    /// Used for both intersection (`include_if_found = true`) and
    /// difference (`include_if_found = false`).
    fn emit_set_filter_append(
        &mut self,
        src: inkwell::values::PointerValue<'ctx>,
        filter: inkwell::values::PointerValue<'ctx>,
        include_if_found: bool,
        dst: inkwell::values::PointerValue<'ctx>,
        label: &str,
    ) -> Option<()> {
        use inkwell::values::AnyValue;
        let i64_ty = self.context.i64_type();

        let len_fn = self.get_mvl_array_len();
        let len_call = self
            .builder
            .build_call(len_fn, &[src.into()], "src_len")
            .ok()?;
        let src_len = inkwell::values::BasicValueEnum::try_from(len_call.as_any_value_enum())
            .ok()?
            .into_int_value();

        let i_slot = self.builder.build_alloca(i64_ty, "i").unwrap();
        self.builder
            .build_store(i_slot, i64_ty.const_int(0, false))
            .unwrap();

        let parent = self.builder.get_insert_block()?.get_parent()?;
        let outer_cond = self
            .context
            .append_basic_block(parent, &format!("{label}_ocond"));
        let outer_body = self
            .context
            .append_basic_block(parent, &format!("{label}_obody"));
        let outer_exit = self
            .context
            .append_basic_block(parent, &format!("{label}_oexit"));

        self.builder.build_unconditional_branch(outer_cond).unwrap();

        // Outer cond: i < src_len
        self.builder.position_at_end(outer_cond);
        let i = self
            .builder
            .build_load(i64_ty, i_slot, "i")
            .unwrap()
            .into_int_value();
        let i_lt = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, src_len, "i_lt")
            .unwrap();
        self.builder
            .build_conditional_branch(i_lt, outer_body, outer_exit)
            .unwrap();

        // Outer body: fetch element, run contains, conditionally push
        self.builder.position_at_end(outer_body);
        let get_fn = self.get_mvl_array_get();
        let ep_call = self
            .builder
            .build_call(get_fn, &[src.into(), i.into()], "ep")
            .ok()?;
        let ep = inkwell::values::BasicValueEnum::try_from(ep_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        let elem = self
            .builder
            .build_load(i64_ty, ep, "elem")
            .unwrap()
            .into_int_value();

        let found = self.emit_set_contains_loop(filter, elem, &format!("{label}_inner"))?;

        // Branch: push to dst if (found == include_if_found)
        let push_cond = if include_if_found {
            found
        } else {
            self.builder.build_not(found, "not_found").unwrap()
        };

        let do_push = self
            .context
            .append_basic_block(parent, &format!("{label}_push"));
        let skip_push = self
            .context
            .append_basic_block(parent, &format!("{label}_skip"));

        self.builder
            .build_conditional_branch(push_cond, do_push, skip_push)
            .unwrap();

        // do_push: push elem to dst
        self.builder.position_at_end(do_push);
        self.emit_set_push_i64_slot(dst, elem);
        self.builder.build_unconditional_branch(skip_push).unwrap();

        // skip_push: advance i, loop
        self.builder.position_at_end(skip_push);
        let i_next = self
            .builder
            .build_int_add(i, i64_ty.const_int(1, false), "i_next")
            .unwrap();
        self.builder.build_store(i_slot, i_next).unwrap();
        self.builder.build_unconditional_branch(outer_cond).unwrap();

        // Exit
        self.builder.position_at_end(outer_exit);
        Some(())
    }

    /// Copy all elements of `src` (MvlArray*) to `dst` (MvlArray*) unconditionally.
    fn emit_set_copy_all(
        &mut self,
        src: inkwell::values::PointerValue<'ctx>,
        dst: inkwell::values::PointerValue<'ctx>,
        label: &str,
    ) -> Option<()> {
        use inkwell::values::AnyValue;
        let i64_ty = self.context.i64_type();

        let len_fn = self.get_mvl_array_len();
        let len_call = self
            .builder
            .build_call(len_fn, &[src.into()], "src_len")
            .ok()?;
        let src_len = inkwell::values::BasicValueEnum::try_from(len_call.as_any_value_enum())
            .ok()?
            .into_int_value();

        let i_slot = self.builder.build_alloca(i64_ty, "i").unwrap();
        self.builder
            .build_store(i_slot, i64_ty.const_int(0, false))
            .unwrap();

        let parent = self.builder.get_insert_block()?.get_parent()?;
        let cond_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_cond"));
        let body_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_body"));
        let exit_bb = self
            .context
            .append_basic_block(parent, &format!("{label}_exit"));

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(cond_bb);
        let i = self
            .builder
            .build_load(i64_ty, i_slot, "i")
            .unwrap()
            .into_int_value();
        let i_lt = self
            .builder
            .build_int_compare(IntPredicate::SLT, i, src_len, "i_lt")
            .unwrap();
        self.builder
            .build_conditional_branch(i_lt, body_bb, exit_bb)
            .unwrap();

        self.builder.position_at_end(body_bb);
        let get_fn = self.get_mvl_array_get();
        let ep_call = self
            .builder
            .build_call(get_fn, &[src.into(), i.into()], "ep")
            .ok()?;
        let ep = inkwell::values::BasicValueEnum::try_from(ep_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        let elem = self
            .builder
            .build_load(i64_ty, ep, "elem")
            .unwrap()
            .into_int_value();
        self.emit_set_push_i64_slot(dst, elem);
        let i_next = self
            .builder
            .build_int_add(i, i64_ty.const_int(1, false), "i_next")
            .unwrap();
        self.builder.build_store(i_slot, i_next).unwrap();
        self.builder.build_unconditional_branch(cond_bb).unwrap();

        self.builder.position_at_end(exit_bb);
        Some(())
    }

    /// Emit `set_intersection(a, b)` as inline LLVM IR.
    /// Returns elements of `a` that are also in `b`.
    pub(crate) fn emit_set_intersection(
        &mut self,
        a: &Expr,
        b: &Expr,
    ) -> Option<BasicValueEnum<'ctx>> {
        let a_ptr = self.emit_expr(a)?.into_pointer_value();
        let b_ptr = self.emit_expr(b)?.into_pointer_value();
        let i64_ty = self.context.i64_type();
        let new_fn = self.get_mvl_array_new();
        use inkwell::values::AnyValue;
        let r_call = self
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(8, false).into(),
                    i64_ty.const_int(4, false).into(),
                ],
                "result",
            )
            .ok()?;
        let result_ptr = inkwell::values::BasicValueEnum::try_from(r_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        self.emit_set_filter_append(a_ptr, b_ptr, true, result_ptr, "intr")?;
        Some(result_ptr.into())
    }

    /// Emit `set_difference(a, b)` as inline LLVM IR.
    /// Returns elements of `a` that are NOT in `b`.
    pub(crate) fn emit_set_difference(
        &mut self,
        a: &Expr,
        b: &Expr,
    ) -> Option<BasicValueEnum<'ctx>> {
        let a_ptr = self.emit_expr(a)?.into_pointer_value();
        let b_ptr = self.emit_expr(b)?.into_pointer_value();
        let i64_ty = self.context.i64_type();
        let new_fn = self.get_mvl_array_new();
        use inkwell::values::AnyValue;
        let r_call = self
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(8, false).into(),
                    i64_ty.const_int(4, false).into(),
                ],
                "result",
            )
            .ok()?;
        let result_ptr = inkwell::values::BasicValueEnum::try_from(r_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        self.emit_set_filter_append(a_ptr, b_ptr, false, result_ptr, "diff")?;
        Some(result_ptr.into())
    }

    /// Emit `set_union(a, b)` as inline LLVM IR.
    /// Returns all elements of `a` plus elements of `b` not already in `a`.
    pub(crate) fn emit_set_union(&mut self, a: &Expr, b: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let a_ptr = self.emit_expr(a)?.into_pointer_value();
        let b_ptr = self.emit_expr(b)?.into_pointer_value();
        let i64_ty = self.context.i64_type();
        let new_fn = self.get_mvl_array_new();
        use inkwell::values::AnyValue;
        let r_call = self
            .builder
            .build_call(
                new_fn,
                &[
                    i64_ty.const_int(8, false).into(),
                    i64_ty.const_int(4, false).into(),
                ],
                "result",
            )
            .ok()?;
        let result_ptr = inkwell::values::BasicValueEnum::try_from(r_call.as_any_value_enum())
            .ok()?
            .into_pointer_value();
        // Copy all of a, then add elements from b not in a.
        self.emit_set_copy_all(a_ptr, result_ptr, "union_a")?;
        self.emit_set_filter_append(b_ptr, a_ptr, false, result_ptr, "union_b")?;
        Some(result_ptr.into())
    }
}
