//! Statement and control-flow emission for the MVL LLVM backend.
//!
//! Covers: `let`, assignment, `if`/`else`, `while`, `for`, `match`, `return`,
//! and the pattern-binding helpers used by `match` arm bodies.

use inkwell::{types::BasicTypeEnum, values::BasicValueEnum, AddressSpace, IntPredicate};

use crate::mvl::codegen::HeapKind;
use crate::mvl::parser::ast::TypeExpr;

use crate::mvl::parser::ast::{
    Block, ElseBranch, Expr, LValue, Literal, MatchArm, MatchBody, Pattern, Stmt, VariantFields,
};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── Block / statement emission ───────────────────────────────────────────

    pub(crate) fn emit_block(&mut self, block: &Block) -> Option<BasicValueEnum<'ctx>> {
        let mut last: Option<BasicValueEnum<'ctx>> = None;
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            last = self.emit_stmt(stmt);
        }
        last
    }

    pub(crate) fn emit_stmt(&mut self, stmt: &Stmt) -> Option<BasicValueEnum<'ctx>> {
        match stmt {
            Stmt::Let {
                pattern, init, ty, ..
            } => {
                self.pending_let_ty = Some(ty.clone());
                let val = self.emit_expr(init);
                self.pending_let_ty = None;
                let val = val?;
                // Determine the LLVM type: use the annotation type only when it matches the
                // actual value type (annotation may fall back to i64 for unknown generics
                // like List[T], Map[K,V] — in that case trust the value's own type).
                let ann_ty = self.mvl_type_to_llvm(ty);
                let llvm_ty = ann_ty
                    .filter(|&t| t == val.get_type())
                    .unwrap_or_else(|| val.get_type());
                let name = match pattern {
                    Pattern::Ident(name, _) => name.clone(),
                    Pattern::Wildcard(_) => "_".to_string(),
                    _ => "_".to_string(),
                };
                let alloca = self.builder.build_alloca(llvm_ty, &name).unwrap();
                self.builder.build_store(alloca, val).unwrap();
                self.locals.insert(name.clone(), (alloca, llvm_ty));
                // L5-08: record MVL type annotation for Ok/Some payload inference.
                self.local_mvl_types.insert(name.clone(), ty.clone());
                // L5-15: ownership transfer for heap moves.
                // If init is a bare identifier or move(ident), transfer ownership from
                // the source variable — remove it from heap_locals so it is not dropped
                // at the original scope exit (the new binding becomes the sole owner).
                let move_src_kind = {
                    let src = match init {
                        Expr::Ident(src, _) => Some(src.as_str()),
                        Expr::Move { expr, .. } => match expr.as_ref() {
                            Expr::Ident(src, _) => Some(src.as_str()),
                            _ => None,
                        },
                        _ => None,
                    };
                    src.and_then(|s| self.heap_locals.remove(s))
                };
                // Register new binding: prefer the transferred kind, fall back to type annotation.
                // Only register for drop if the alloca is in the entry block — allocas in branch
                // blocks (match arms, loops) don't dominate the function exit and would produce
                // invalid IR ("Instruction does not dominate all uses").
                let heap_kind = move_src_kind.or_else(|| heap_kind_of(ty));
                if let Some(kind) = heap_kind {
                    if matches!(llvm_ty, BasicTypeEnum::PointerType(_)) && self.in_entry_block() {
                        self.heap_locals.insert(name, kind);
                    }
                }
                None
            }
            Stmt::Return { value, .. } => {
                let ret_val = value.as_ref().and_then(|e| self.emit_expr(e));
                // L5-14: drop heap locals before returning, but skip the returned variable.
                // Returning a heap pointer transfers ownership to the caller — dropping it
                // here would be a use-after-free.
                let ret_heap_name: Option<String> = value.as_ref().and_then(|e| {
                    if let Expr::Ident(name, _) = e {
                        if self.heap_locals.contains_key(name.as_str()) {
                            return Some(name.clone());
                        }
                    }
                    None
                });
                self.emit_heap_drops_except(ret_heap_name.as_deref());
                if let Some(v) = ret_val {
                    self.builder.build_return(Some(&v)).unwrap();
                } else {
                    self.builder.build_return(None).unwrap();
                }
                self.terminated = true;
                None
            }
            Stmt::Expr { expr, .. } => self.emit_expr(expr),
            Stmt::Assign { target, value, .. } => {
                let val = self.emit_expr(value)?;
                match target {
                    LValue::Ident(n, _) => {
                        if let Some((alloca, _)) = self.locals.get(n).copied() {
                            // L5-14: drop the old heap value before overwriting to prevent a
                            // memory leak (the previous pointer is unreachable after the store).
                            let kind_opt = self.heap_locals.get(n.as_str()).copied();
                            if let Some(kind) = kind_opt {
                                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                                if let Ok(old_ptr) =
                                    self.builder.build_load(ptr_ty, alloca, "old_heap")
                                {
                                    let drop_fn = match kind {
                                        HeapKind::String => self.get_mvl_string_drop(),
                                        HeapKind::Array | HeapKind::Set => {
                                            self.get_mvl_array_drop()
                                        }
                                        HeapKind::Map => self.get_mvl_map_drop(),
                                        HeapKind::StringPtrArray => {
                                            self.get_mvl_string_ptr_array_drop()
                                        }
                                    };
                                    let _ = self.builder.build_call(
                                        drop_fn,
                                        &[old_ptr.into()],
                                        "assign_drop",
                                    );
                                }
                            }
                            self.builder.build_store(alloca, val).unwrap();
                            // L5-15: if RHS is a heap variable being moved, transfer
                            // ownership — remove it from heap_locals so it is not dropped
                            // twice (the target n is already tracked and will drop it).
                            let move_src = match value {
                                Expr::Ident(src, _) => Some(src.as_str()),
                                Expr::Move { expr, .. } => match expr.as_ref() {
                                    Expr::Ident(src, _) => Some(src.as_str()),
                                    _ => None,
                                },
                                _ => None,
                            };
                            if let Some(src) = move_src.filter(|&s| s != n.as_str()) {
                                self.heap_locals.remove(src);
                            }
                        }
                    }
                    LValue::Field { base, field, .. } => {
                        self.emit_field_assign(base, field, val);
                    }
                }
                None
            }
            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt(cond, then, else_),

            // L5-11: match — returns value when in tail/expression position
            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match(scrutinee, arms),

            // L5-12: while loop
            Stmt::While { cond, body, .. } => {
                self.emit_while(cond, body);
                None
            }

            // L5-12: for loop (range-based)
            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => {
                self.emit_for(pattern, iter, body);
                None
            }
        }
    }

    pub(crate) fn emit_if_stmt(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: &Option<ElseBranch>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let cond_val = self.emit_expr(cond)?;
        let cond_int = match cond_val {
            BasicValueEnum::IntValue(v) => {
                // Truncate to i1 if wider (e.g. comparing i64 booleans).
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
        let then_bb = self.context.append_basic_block(parent_fn, "then");
        let merge_bb = self.context.append_basic_block(parent_fn, "merge");
        let else_bb = if else_.is_some() {
            self.context.append_basic_block(parent_fn, "else")
        } else {
            merge_bb
        };

        self.builder
            .build_conditional_branch(cond_int, then_bb, else_bb)
            .unwrap();

        // Emit `then` block.
        self.builder.position_at_end(then_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        let then_val = self.emit_block(then);
        let then_end = self.builder.get_insert_block().unwrap();
        if !self.terminated {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // Emit `else` block (if present).
        let else_val = if let Some(eb) = else_ {
            self.terminated = false;
            self.builder.position_at_end(else_bb);
            let ev = match eb {
                ElseBranch::Block(blk) => self.emit_block(blk),
                ElseBranch::If(if_stmt) => self.emit_stmt(if_stmt),
            };
            let else_end = self.builder.get_insert_block().unwrap();
            if !self.terminated {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
            ev.map(|v| (v, else_end))
        } else {
            None
        };

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);

        // Build phi when both branches produce values of the same type.
        match (then_val, else_val) {
            (Some(tv), Some((ev, else_end))) if tv.get_type() == ev.get_type() => {
                let phi = self.builder.build_phi(tv.get_type(), "if_val").unwrap();
                phi.add_incoming(&[(&tv, then_end), (&ev, else_end)]);
                Some(phi.as_basic_value())
            }
            _ => None,
        }
    }

    // ── L5-12: While loop ─────────────────────────────────────────────────────

    pub(crate) fn emit_while(&mut self, cond: &Expr, body: &Block) {
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let cond_bb = self.context.append_basic_block(parent_fn, "while_cond");
        let body_bb = self.context.append_basic_block(parent_fn, "while_body");
        let exit_bb = self.context.append_basic_block(parent_fn, "while_exit");

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Condition block.
        self.builder.position_at_end(cond_bb);
        let cond_val = self.emit_expr(cond);
        if let Some(BasicValueEnum::IntValue(cv)) = cond_val {
            let cv_bool = if cv.get_type().get_bit_width() != 1 {
                self.builder
                    .build_int_truncate(cv, self.context.bool_type(), "w_cond")
                    .unwrap()
            } else {
                cv
            };
            self.builder
                .build_conditional_branch(cv_bool, body_bb, exit_bb)
                .unwrap();
        } else {
            self.builder.build_unconditional_branch(exit_bb).unwrap();
        }

        // Body block.
        self.builder.position_at_end(body_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        self.emit_block(body);
        if !self.terminated {
            self.builder.build_unconditional_branch(cond_bb).unwrap();
        }
        self.terminated = prev_terminated;

        // Exit block.
        self.builder.position_at_end(exit_bb);
    }

    // ── L5-11: Match ─────────────────────────────────────────────────────────

    /// Emit a match expression or statement, returning the phi-merged result value (if any).
    pub(crate) fn emit_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Option<BasicValueEnum<'ctx>> {
        let ok_ty_opt = self.infer_result_ok_llvm_ty(scrutinee);
        let scrutinee_val = self.emit_expr(scrutinee)?;

        // String literal match: arms contain Pattern::Literal(Str) patterns.
        if let BasicValueEnum::PointerValue(scrutinee_ptr) = scrutinee_val {
            let has_str_arm = arms
                .iter()
                .any(|a| matches!(&a.pattern, Pattern::Literal(Literal::Str(_), _)));
            if has_str_arm {
                return self.emit_string_match(scrutinee_ptr, arms);
            }
        }

        // Extract i8 discriminant from the scrutinee value.
        let disc_val = self.extract_discriminant(scrutinee_val)?;

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let merge_bb = self.context.append_basic_block(parent_fn, "match_merge");
        let fallback_bb = self.context.append_basic_block(parent_fn, "match_default");

        // Determine discriminant and basic block for each arm.
        let mut arm_blocks: Vec<inkwell::basic_block::BasicBlock<'ctx>> = Vec::new();
        let mut switch_cases: Vec<(
            inkwell::values::IntValue<'ctx>,
            inkwell::basic_block::BasicBlock<'ctx>,
        )> = Vec::new();
        let mut default_bb: Option<inkwell::basic_block::BasicBlock<'ctx>> = None;

        for (i, arm) in arms.iter().enumerate() {
            let arm_bb = self
                .context
                .append_basic_block(parent_fn, &format!("arm{i}"));
            arm_blocks.push(arm_bb);

            if let Some(disc) = self.pattern_to_discriminant(&arm.pattern) {
                switch_cases.push((disc, arm_bb));
            } else if default_bb.is_none() {
                default_bb = Some(arm_bb);
            }
        }

        let actual_default = default_bb.unwrap_or(fallback_bb);
        self.builder
            .build_switch(disc_val, actual_default, &switch_cases)
            .unwrap();

        // Emit each arm body.
        let prev_terminated = self.terminated;
        let mut phi_incoming: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::new();

        let mut arms_reaching_merge = 0usize;

        for (i, arm) in arms.iter().enumerate() {
            let arm_bb = arm_blocks[i];
            self.builder.position_at_end(arm_bb);
            self.terminated = false;

            // Bind pattern variables if needed (Phase B: simple cases only).
            self.bind_pattern_vars(&arm.pattern, scrutinee_val, ok_ty_opt);

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e),
                MatchBody::Block(b) => self.emit_block(b),
            };

            let arm_end = self.builder.get_insert_block().unwrap();
            if !self.terminated {
                arms_reaching_merge += 1;
                if let Some(val) = arm_val {
                    phi_incoming.push((val, arm_end));
                }
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        // Fallback block: unreachable for exhaustive match.
        self.builder.position_at_end(fallback_bb);
        self.builder.build_unreachable().unwrap();

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);

        // Only build a phi if every arm that reaches merge_bb produced a value.
        // Fewer phi entries than predecessors would produce invalid LLVM IR.
        if phi_incoming.is_empty() || phi_incoming.len() < arms_reaching_merge {
            return None;
        }

        // All arms must produce the same type for phi to work.
        let first_ty = phi_incoming[0].0.get_type();
        if phi_incoming.iter().all(|(v, _)| v.get_type() == first_ty) {
            let phi = self.builder.build_phi(first_ty, "match_val").unwrap();
            for (val, bb) in &phi_incoming {
                phi.add_incoming(&[(val, *bb)]);
            }
            Some(phi.as_basic_value())
        } else {
            None
        }
    }

    /// Match a String scrutinee against literal string arms using `mvl_string_eq`.
    ///
    /// Emits an if-else chain: each arm with a `Pattern::Literal(Str)` gets a
    /// comparison; wildcard / ident arms become the final else branch.
    fn emit_string_match(
        &mut self,
        scrutinee_ptr: inkwell::values::PointerValue<'ctx>,
        arms: &[MatchArm],
    ) -> Option<BasicValueEnum<'ctx>> {
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let merge_bb = self
            .context
            .append_basic_block(parent_fn, "str_match_merge");

        let prev_terminated = self.terminated;
        let mut phi_incoming: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::new();
        let mut arms_reaching_merge = 0usize;

        // Each comparison lives in its own block; start with the current one.
        let mut check_bb = self.builder.get_insert_block().unwrap();

        for (i, arm) in arms.iter().enumerate() {
            let arm_bb = self
                .context
                .append_basic_block(parent_fn, &format!("str_arm{i}"));

            match &arm.pattern {
                Pattern::Literal(Literal::Str(s), _) => {
                    // Build the literal string for comparison.
                    self.builder.position_at_end(check_bb);
                    let global = self
                        .builder
                        .build_global_string_ptr(s, "match_lit")
                        .unwrap();
                    let len_val = self.context.i64_type().const_int(s.len() as u64, false);
                    let new_fn = self.get_mvl_string_new();
                    let lit_ptr = {
                        use inkwell::values::AnyValue;
                        let call = self
                            .builder
                            .build_call(
                                new_fn,
                                &[global.as_pointer_value().into(), len_val.into()],
                                "lit_str",
                            )
                            .unwrap();
                        BasicValueEnum::try_from(call.as_any_value_enum())
                            .ok()?
                            .into_pointer_value()
                    };

                    let eq_fn = self.get_mvl_string_eq();
                    let eq_call = self
                        .builder
                        .build_call(eq_fn, &[scrutinee_ptr.into(), lit_ptr.into()], "str_eq")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    let eq_i32 = BasicValueEnum::try_from(eq_call.as_any_value_enum())
                        .ok()?
                        .into_int_value();
                    let cond = self
                        .builder
                        .build_int_compare(
                            IntPredicate::NE,
                            eq_i32,
                            self.context.i32_type().const_zero(),
                            "str_cond",
                        )
                        .unwrap();

                    // Branch to arm_bb or the next check block.
                    let next_check = self
                        .context
                        .append_basic_block(parent_fn, &format!("str_check{}", i + 1));
                    self.builder
                        .build_conditional_branch(cond, arm_bb, next_check)
                        .unwrap();
                    check_bb = next_check;
                }
                // Wildcard / binding / default: emit without a guard.
                _ => {
                    // Jump from last check block into this arm unconditionally.
                    self.builder.position_at_end(check_bb);
                    self.builder.build_unconditional_branch(arm_bb).unwrap();
                }
            }

            // Emit arm body.
            self.builder.position_at_end(arm_bb);
            self.terminated = false;
            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e),
                MatchBody::Block(b) => self.emit_block(b),
            };
            let arm_end = self.builder.get_insert_block().unwrap();
            if !self.terminated {
                arms_reaching_merge += 1;
                if let Some(val) = arm_val {
                    phi_incoming.push((val, arm_end));
                }
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        // Any remaining check block (no default arm) → unreachable.
        if self.builder.get_insert_block() == Some(check_bb) && check_bb != merge_bb {
            self.builder.position_at_end(check_bb);
            self.builder.build_unreachable().unwrap();
        }

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);

        if phi_incoming.is_empty() || phi_incoming.len() < arms_reaching_merge {
            return None;
        }
        let first_ty = phi_incoming[0].0.get_type();
        if phi_incoming.iter().all(|(v, _)| v.get_type() == first_ty) {
            let phi = self.builder.build_phi(first_ty, "str_match_val").unwrap();
            for (val, bb) in &phi_incoming {
                phi.add_incoming(&[(val, *bb)]);
            }
            Some(phi.as_basic_value())
        } else {
            None
        }
    }

    /// Extract an i8 discriminant from a value.
    ///
    /// - `i8` value (unit enum) → use directly.
    /// - Struct value (tagged union) → extractvalue at index 0.
    pub(crate) fn extract_discriminant(
        &mut self,
        val: BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        match val {
            BasicValueEnum::IntValue(v) if v.get_type().get_bit_width() == 8 => Some(v),
            BasicValueEnum::StructValue(sv) => {
                let disc = self.builder.build_extract_value(sv, 0, "disc").unwrap();
                if let BasicValueEnum::IntValue(iv) = disc {
                    Some(iv)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Map a match pattern to its i8 discriminant constant (None for wildcards / bindings).
    pub(crate) fn pattern_to_discriminant(
        &self,
        pat: &Pattern,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        match pat {
            Pattern::Ident(name, _) | Pattern::TupleStruct { name, .. } => {
                self.lookup_enum_variant_disc(name)
            }
            // Built-in Result/Option patterns.
            Pattern::Ok { .. } | Pattern::Some { .. } => {
                Some(self.context.i8_type().const_int(0, false))
            }
            Pattern::Err { .. } | Pattern::None(_) => {
                Some(self.context.i8_type().const_int(1, false))
            }
            _ => None,
        }
    }

    /// Look up the discriminant for an enum variant by name.
    ///
    /// Accepts both qualified (`Shape::Circle`) and unqualified (`Circle`) names.
    pub(crate) fn lookup_enum_variant_disc(
        &self,
        name: &str,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        // Built-in Result/Option variants — both unqualified and qualified forms.
        match name {
            "Ok" | "Some" | "Result::Ok" | "Option::Some" => {
                return Some(self.context.i8_type().const_int(0, false))
            }
            "Err" | "None" | "Result::Err" | "Option::None" => {
                return Some(self.context.i8_type().const_int(1, false))
            }
            _ => {}
        }
        // Qualified: "Shape::Circle"
        if let Some(pos) = name.find("::") {
            let type_name = &name[..pos];
            let variant_name = &name[pos + 2..];
            if let Some(variants) = self.enum_variants.get(type_name) {
                let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;
                return Some(self.context.i8_type().const_int(disc, false));
            }
            return None;
        }
        // Unqualified: search all enums.
        for variants in self.enum_variants.values() {
            if let Some(disc) = variants.iter().position(|(vn, _)| vn == name) {
                return Some(self.context.i8_type().const_int(disc as u64, false));
            }
        }
        None
    }

    /// Bind pattern-introduced variables into `self.locals` before emitting arm body.
    ///
    /// For tuple-variant patterns like `Some(v)`, extracts the payload and stores it.
    /// `ok_ty` is the expected LLVM type of an Ok/Some payload (defaults to i64 if None).
    pub(crate) fn bind_pattern_vars(
        &mut self,
        pat: &Pattern,
        scrutinee: BasicValueEnum<'ctx>,
        ok_ty: Option<BasicTypeEnum<'ctx>>,
    ) {
        let default_ok_ty: BasicTypeEnum = self.context.i64_type().into();
        let ok_llvm_ty = ok_ty.unwrap_or(default_ok_ty);

        // Built-in Pattern::Ok(inner), Pattern::Err(inner), Pattern::Some(inner).
        let (inner_pat, is_err) = match pat {
            Pattern::Ok { inner, .. } | Pattern::Some { inner, .. } => {
                (Some(inner.as_ref()), false)
            }
            Pattern::Err { inner, .. } => (Some(inner.as_ref()), true),
            _ => (None, false),
        };
        if let Some(inner) = inner_pat {
            if let Pattern::Ident(bind_name, _) = inner {
                let BasicValueEnum::StructValue(sv) = scrutinee else {
                    return;
                };
                // L5-08: layout is {i8, ptr} — field 1 is a pointer to the payload value.
                let payload_ptr_val = match self.builder.build_extract_value(sv, 1, "payload_ptr") {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let llvm_ty: BasicTypeEnum = if is_err {
                    self.context.ptr_type(AddressSpace::default()).into()
                } else {
                    ok_llvm_ty
                };
                let payload_ptr = payload_ptr_val.into_pointer_value();
                let loaded = self
                    .builder
                    .build_load(llvm_ty, payload_ptr, bind_name)
                    .unwrap();
                let alloca = self.builder.build_alloca(llvm_ty, bind_name).unwrap();
                self.builder.build_store(alloca, loaded).unwrap();
                self.locals.insert(bind_name.clone(), (alloca, llvm_ty));
            }
            return;
        }

        if let Pattern::TupleStruct { name, fields, .. } = pat {
            // Extract payload from tagged union.
            let BasicValueEnum::StructValue(sv) = scrutinee else {
                return;
            };
            // payload is at index 1 (byte array).
            let payload_arr = match self.builder.build_extract_value(sv, 1, "payload") {
                Ok(v) => v,
                Err(_) => return,
            };

            // Built-in Result/Option variants — both unqualified and qualified forms.
            // L5-08: layout is {i8, ptr} — field 1 is a pointer to the payload value.
            if matches!(
                name.as_str(),
                "Ok" | "Some"
                    | "Err"
                    | "Result::Ok"
                    | "Option::Some"
                    | "Result::Err"
                    | "Option::None"
            ) {
                let bind_name = match fields.first() {
                    Some(Pattern::Ident(n, _)) => n.clone(),
                    _ => return,
                };
                let payload_ptr_val = match self.builder.build_extract_value(sv, 1, "payload_ptr") {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let is_err_variant =
                    matches!(name.as_str(), "Err" | "Result::Err" | "Option::None");
                let llvm_ty: BasicTypeEnum = if is_err_variant {
                    self.context.ptr_type(AddressSpace::default()).into()
                } else {
                    ok_llvm_ty
                };
                let payload_ptr = payload_ptr_val.into_pointer_value();
                let loaded = self
                    .builder
                    .build_load(llvm_ty, payload_ptr, &bind_name)
                    .unwrap();
                let alloca = self.builder.build_alloca(llvm_ty, &bind_name).unwrap();
                self.builder.build_store(alloca, loaded).unwrap();
                self.locals.insert(bind_name, (alloca, llvm_ty));
                return;
            }

            // Determine variant payload types.
            let (type_name, variant_name) = if let Some(pos) = name.find("::") {
                (name[..pos].to_string(), name[pos + 2..].to_string())
            } else {
                // Search for unqualified variant name.
                let found = self.enum_variants.iter().find_map(|(tn, variants)| {
                    variants
                        .iter()
                        .any(|(vn, _)| vn == name)
                        .then(|| tn.clone())
                });
                match found {
                    Some(tn) => (tn, name.clone()),
                    None => return,
                }
            };

            let variants = match self.enum_variants.get(&type_name) {
                Some(v) => v.clone(),
                None => return,
            };
            let variant_fields = match variants.iter().find(|(vn, _)| vn == &variant_name) {
                Some((_, vf)) => vf.clone(),
                None => return,
            };

            if let VariantFields::Tuple(field_types) = &variant_fields {
                for (i, (pat_field, field_ty)) in fields.iter().zip(field_types.iter()).enumerate()
                {
                    let Pattern::Ident(bind_name, _) = pat_field else {
                        continue;
                    };
                    let Some(llvm_ty) = self.mvl_type_to_llvm(field_ty) else {
                        continue;
                    };

                    // Alloca a slot for the extracted value.
                    let alloca = self.builder.build_alloca(llvm_ty, bind_name).unwrap();

                    // Bitcast the payload array into a pointer to the field type,
                    // then load. For the first field we use the payload base; for
                    // subsequent fields we GEP forward by the accumulated offset.
                    let offset: usize = (0..i)
                        .map(|j| Self::type_size_bytes_static(&field_types[j]))
                        .sum();

                    // Store payload_arr into a temporary alloca so we can GEP into it.
                    let payload_ty = payload_arr.get_type();
                    let tmp = self
                        .builder
                        .build_alloca(payload_ty, "payload_tmp")
                        .unwrap();
                    self.builder.build_store(tmp, payload_arr).unwrap();

                    let field_ptr = if offset == 0 {
                        tmp
                    } else {
                        let off_val = self.context.i64_type().const_int(offset as u64, false);
                        unsafe {
                            self.builder
                                .build_gep(self.context.i8_type(), tmp, &[off_val], "field_ptr")
                                .unwrap()
                        }
                    };

                    let loaded = self
                        .builder
                        .build_load(llvm_ty, field_ptr, bind_name)
                        .unwrap();
                    self.builder.build_store(alloca, loaded).unwrap();
                    self.locals.insert(bind_name.clone(), (alloca, llvm_ty));
                    // Register MVL type so Deref (*box_val) can load the inner type (#571).
                    self.local_mvl_types
                        .insert(bind_name.clone(), field_ty.clone());
                }
            }
        }
    }

    // ── L5-12: for loop ───────────────────────────────────────────────────────

    /// Emit `for pat in range(a, b) { body }` as a counted LLVM loop.
    ///
    /// Only `range(a, b)` iterators are supported for now.
    pub(crate) fn emit_for(&mut self, pattern: &Pattern, iter: &Expr, body: &Block) {
        // Only handle `for x in range(a, b)`.
        let (var_name, start_expr, end_expr) = match iter {
            Expr::FnCall { name, args, .. } if name == "range" && args.len() == 2 => {
                let var = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => return,
                };
                (var, &args[0], &args[1])
            }
            _ => return,
        };

        let start_val = match self.emit_expr(start_expr) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => return,
        };
        let end_val = match self.emit_expr(end_expr) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => return,
        };

        let i64_ty = self.context.i64_type();
        let alloca = self.builder.build_alloca(i64_ty, &var_name).unwrap();
        self.builder.build_store(alloca, start_val).unwrap();
        self.locals
            .insert(var_name.clone(), (alloca, i64_ty.into()));

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let cond_bb = self.context.append_basic_block(parent_fn, "for_cond");
        let body_bb = self.context.append_basic_block(parent_fn, "for_body");
        let exit_bb = self.context.append_basic_block(parent_fn, "for_exit");

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Condition: i < end
        self.builder.position_at_end(cond_bb);
        let cur = self
            .builder
            .build_load(i64_ty, alloca, "for_i")
            .unwrap()
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(IntPredicate::SLT, cur, end_val, "for_lt")
            .unwrap();
        self.builder
            .build_conditional_branch(cond, body_bb, exit_bb)
            .unwrap();

        // Body: execute, then increment and loop back.
        self.builder.position_at_end(body_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        self.emit_block(body);
        if !self.terminated {
            let cur = self
                .builder
                .build_load(i64_ty, alloca, "for_i_inc")
                .unwrap()
                .into_int_value();
            let one = i64_ty.const_int(1, false);
            let next = self.builder.build_int_add(cur, one, "for_next").unwrap();
            self.builder.build_store(alloca, next).unwrap();
            self.builder.build_unconditional_branch(cond_bb).unwrap();
        }
        self.terminated = prev_terminated;

        // Exit block.
        self.builder.position_at_end(exit_bb);
    }
}

/// Map a MVL TypeExpr to the HeapKind it represents, if any.
/// Used to register `let` bindings that hold heap-allocated collection values
/// for automatic drop emission at function exit (L5-14).
pub(crate) fn heap_kind_of(ty: &TypeExpr) -> Option<HeapKind> {
    let base = match ty {
        TypeExpr::Base { name, .. } => name.as_str(),
        TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
            return heap_kind_of(inner);
        }
        _ => return None,
    };
    match base {
        "String" => Some(HeapKind::String),
        "List" | "Array" => {
            // List[String] owns its elements (each is an mvl_string_new allocation);
            // use the deep-drop variant so element strings are freed with the array.
            if let TypeExpr::Base { args, .. } = ty {
                if matches!(
                    args.first(),
                    Some(TypeExpr::Base { name, .. }) if name == "String"
                ) {
                    return Some(HeapKind::StringPtrArray);
                }
            }
            Some(HeapKind::Array)
        }
        "Map" => Some(HeapKind::Map),
        "Set" => Some(HeapKind::Set),
        _ => None,
    }
}
