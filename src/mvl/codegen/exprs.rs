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

use super::LlvmBackend;

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

            _ => None,
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

        let mut sv = struct_ty.get_undef();
        for (idx, (fname, _)) in field_info.iter().enumerate() {
            if let Some((_, fexpr)) = fields.iter().find(|(n, _)| n == fname) {
                if let Some(fval) = self.emit_expr(fexpr) {
                    sv = self
                        .builder
                        .build_insert_value(sv, fval, idx as u32, &format!("s{idx}"))
                        .unwrap()
                        .into_struct_value();
                }
            }
        }
        Some(sv.into())
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

    /// Emit `Ok(val)` (disc=0) or `Err(val)` (disc=1) as a tagged union `{i8, [8 x i8]}`.
    pub(crate) fn emit_result_variant(
        &mut self,
        disc: u64,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let payload_ty = self.context.i8_type().array_type(8);
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), payload_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "res_tmp").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "res_disc")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store payload value.
        if let Some(arg) = args.first() {
            if let Some(val) = self.emit_expr(arg) {
                let payload_ptr = self
                    .builder
                    .build_struct_gep(result_ty, alloca, 1, "res_payload")
                    .unwrap();
                self.builder.build_store(payload_ptr, val).unwrap();
            }
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
    pub(crate) fn emit_propagate(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let ok_ty = self.infer_result_ok_llvm_ty(expr);
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

        // Ok branch: extract payload and yield with the correct type.
        self.builder.position_at_end(ok_bb);
        let payload = self
            .builder
            .build_extract_value(sv, 1, "prop_payload")
            .ok()?;
        let payload_ty = payload.get_type();
        let tmp = self.builder.build_alloca(payload_ty, "prop_tmp").unwrap();
        self.builder.build_store(tmp, payload).unwrap();
        let ok_val = self.builder.build_load(ok_ty, tmp, "prop_ok_val").unwrap();
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
                // Create a global null-terminated string constant and return its pointer.
                let global = self.builder.build_global_string_ptr(s, "str_lit").unwrap();
                Some(global.as_pointer_value().into())
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
        }
    }

    // ── Function call emission (L5-07 + L5-17) ──────────────────────────────

    pub(crate) fn emit_fn_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        match name {
            "println" => self.emit_println(args),
            "print" => self.emit_print(args),
            "format" => self.emit_format(args),
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
                        if self.enum_variants.contains_key(&type_name) {
                            return self.emit_enum_variant_construct(
                                &type_name,
                                &variant_name,
                                args,
                            );
                        }
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

    /// Emit `[e1, ..., eN]` → `{ i64 len, ptr data }` struct with a stack-allocated array.
    pub(crate) fn emit_list_literal(&mut self, elems: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = elems.len() as u32;
        let arr_ty = i64_ty.array_type(n.max(1));
        let arr = self.builder.build_alloca(arr_ty, "list_data").unwrap();
        for (i, elem) in elems.iter().enumerate() {
            if let Some(BasicValueEnum::IntValue(v)) = self.emit_expr(elem) {
                let idx = i64_ty.const_int(i as u64, false);
                let ptr = unsafe {
                    self.builder
                        .build_gep(i64_ty, arr, &[idx], "elem_ptr")
                        .unwrap()
                };
                self.builder.build_store(ptr, v).unwrap();
            }
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let list_ty = self
            .context
            .struct_type(&[i64_ty.into(), ptr_ty.into()], false);
        let alloca = self.builder.build_alloca(list_ty, "list_tmp").unwrap();
        let len_ptr = self
            .builder
            .build_struct_gep(list_ty, alloca, 0, "ll_ptr")
            .unwrap();
        let data_ptr = self
            .builder
            .build_struct_gep(list_ty, alloca, 1, "ld_ptr")
            .unwrap();
        self.builder
            .build_store(len_ptr, i64_ty.const_int(n as u64, false))
            .unwrap();
        self.builder.build_store(data_ptr, arr).unwrap();
        Some(
            self.builder
                .build_load(list_ty, alloca, "list_val")
                .unwrap(),
        )
    }

    /// Emit `{"k": v, ...}` → `{ i64 len }` struct (only `.len()` supported).
    pub(crate) fn emit_map_literal(
        &mut self,
        pairs: &[(Expr, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = pairs.len() as u64;
        let map_ty = self.context.struct_type(&[i64_ty.into()], false);
        let alloca = self.builder.build_alloca(map_ty, "map_tmp").unwrap();
        let len_ptr = self
            .builder
            .build_struct_gep(map_ty, alloca, 0, "ml_ptr")
            .unwrap();
        self.builder
            .build_store(len_ptr, i64_ty.const_int(n, false))
            .unwrap();
        Some(self.builder.build_load(map_ty, alloca, "map_val").unwrap())
    }

    /// Emit `{e1, ..., eN}` → `{ i64 len, ptr data }` (same shape as List).
    pub(crate) fn emit_set_literal(&mut self, elems: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let n = elems.len() as u32;
        let arr_ty = i64_ty.array_type(n.max(1));
        let arr = self.builder.build_alloca(arr_ty, "set_data").unwrap();
        for (i, elem) in elems.iter().enumerate() {
            if let Some(BasicValueEnum::IntValue(v)) = self.emit_expr(elem) {
                let idx = i64_ty.const_int(i as u64, false);
                let ptr = unsafe {
                    self.builder
                        .build_gep(i64_ty, arr, &[idx], "set_ep")
                        .unwrap()
                };
                self.builder.build_store(ptr, v).unwrap();
            }
        }
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let set_ty = self
            .context
            .struct_type(&[i64_ty.into(), ptr_ty.into()], false);
        let alloca = self.builder.build_alloca(set_ty, "set_tmp").unwrap();
        let len_ptr = self
            .builder
            .build_struct_gep(set_ty, alloca, 0, "sl_ptr")
            .unwrap();
        let data_ptr = self
            .builder
            .build_struct_gep(set_ty, alloca, 1, "sd_ptr")
            .unwrap();
        self.builder
            .build_store(len_ptr, i64_ty.const_int(n as u64, false))
            .unwrap();
        self.builder.build_store(data_ptr, arr).unwrap();
        Some(self.builder.build_load(set_ty, alloca, "set_val").unwrap())
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
                // String (ptr) → strlen
                BasicValueEnum::PointerValue(ptr) => {
                    let strlen = self.get_strlen();
                    let call = self
                        .builder
                        .build_call(strlen, &[ptr.into()], "strlen_res")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(call.as_any_value_enum()).ok()
                }
                BasicValueEnum::StructValue(sv) => {
                    let n = sv.get_type().count_fields();
                    if n == 1 {
                        // Map { i64 } → field 0
                        self.builder.build_extract_value(sv, 0, "map_len").ok()
                    } else if n == 2 {
                        let f1_ty = sv.get_type().get_field_type_at_index(1).unwrap();
                        if matches!(f1_ty, BasicTypeEnum::IntType(_)) {
                            // Range { i64 start, i64 end } → end - start
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
                            Some(
                                self.builder
                                    .build_int_sub(e, s, "range_len")
                                    .unwrap()
                                    .into(),
                            )
                        } else {
                            // List/Set { i64 len, ptr } → field 0
                            self.builder.build_extract_value(sv, 0, "coll_len").ok()
                        }
                    } else {
                        None
                    }
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

            _ => None,
        }
    }

    /// Build a `Some(val)` tagged union `{ i8 disc=0, [8 x i8] payload }`.
    pub(crate) fn emit_some_from_val(
        &mut self,
        val: BasicValueEnum<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let payload_ty = self.context.i8_type().array_type(8);
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), payload_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "some_tmp").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "some_disc")
            .unwrap();
        self.builder
            .build_store(disc_ptr, self.context.i8_type().const_int(0, false))
            .unwrap();
        let payload_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 1, "some_payload")
            .unwrap();
        self.builder.build_store(payload_ptr, val).unwrap();
        Some(
            self.builder
                .build_load(result_ty, alloca, "some_val")
                .unwrap(),
        )
    }

    /// Build a `None` tagged union `{ i8 disc=1, [8 x i8] payload=undef }`.
    pub(crate) fn emit_none_val(&mut self) -> Option<BasicValueEnum<'ctx>> {
        let payload_ty = self.context.i8_type().array_type(8);
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), payload_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "none_tmp").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "none_disc")
            .unwrap();
        self.builder
            .build_store(disc_ptr, self.context.i8_type().const_int(1, false))
            .unwrap();
        Some(
            self.builder
                .build_load(result_ty, alloca, "none_val")
                .unwrap(),
        )
    }
}
