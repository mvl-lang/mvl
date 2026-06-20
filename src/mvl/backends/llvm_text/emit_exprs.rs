// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Expression emission for the `llvm_text` backend.

use crate::mvl::parser::ast::{
    BinaryOp, Block, Expr, Literal, MatchArm, MatchBody, Pattern, TypeExpr, UnaryOp,
};

use super::{TextEmitter, RESULT_LLVM_TY};

impl TextEmitter {
    pub(super) fn emit_match_expr(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        let scrut_val = match self.emit_expr(scrutinee)? {
            Some(v) => v,
            None => return Ok(None),
        };

        // Delegate to Result-specific match when Ok/Err patterns are present.
        let has_ok_err = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Ok { .. } | Pattern::Err { .. }));
        if has_ok_err {
            return self.emit_result_match(scrutinee, &scrut_val, arms);
        }

        // Delegate to Option-specific match when Some/None patterns are present.
        let has_some_none = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Some { .. } | Pattern::None(_)));
        if has_some_none {
            return self.emit_option_match(scrutinee, &scrut_val, arms);
        }

        // Delegate to payload-enum match if the scrutinee is a payload enum (#1200).
        if let Some(enum_name) = self.scrutinee_payload_enum(scrutinee) {
            return self.emit_payload_enum_match(&enum_name, &scrut_val, arms);
        }

        let scrut_ty = self.type_of_expr(scrutinee);

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);

        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Determine which patterns are enum discriminants vs wildcards
        let mut switch_arms: Vec<(i64, usize)> = Vec::new();
        let mut wildcard_arm: Option<usize> = None;

        for (idx, arm) in arms.iter().enumerate() {
            if collect_or_discriminants(
                &arm.pattern,
                idx,
                self,
                &mut switch_arms,
                &mut wildcard_arm,
            ) {
                continue;
            }
            wildcard_arm = Some(idx);
        }

        let use_switch = !switch_arms.is_empty();
        let _has_default = wildcard_arm.is_some();

        if use_switch {
            // Emit switch instruction
            let mut switch_str = format!("switch {scrut_ty} {scrut_val}, label %{default_bb} [\n");
            for (disc, arm_idx) in &switch_arms {
                switch_str.push_str(&format!(
                    "    {scrut_ty} {disc}, label %{}\n",
                    arm_bbs[*arm_idx]
                ));
            }
            switch_str.push_str("  ]");
            self.push_instr(&switch_str);
        } else {
            // Fallback: just branch to default
            self.push_instr(&format!("br label %{default_bb}"));
        }

        // Emit each arm block
        let mut phi_entries: Vec<(String, String, String)> = Vec::new(); // (val, ty, from_bb)
                                                                         // Arms that branch to merge_bb but produced no value (need undef phi entries).
        let mut no_val_arms: Vec<String> = Vec::new(); // from_bb

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            // Bind wildcard pattern if present
            let _binding = if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    let bound = self.next_reg();
                    // For enum scrutinee: the bound value is the scrutinee itself
                    self.reg_types.insert(bound.clone(), scrut_ty.clone());
                    self.locals.insert(name.clone(), scrut_val.clone());
                    Some(name.clone())
                } else {
                    None
                }
            } else {
                None
            };

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };

            let end_bb = self.current_bb.clone();
            if !self.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    self.locals.remove(name);
                }
            }
        }

        // Default block
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;

        if let Some(wild_idx) = wildcard_arm {
            let arm_bb = &arm_bbs[wild_idx];
            // Jump to the wildcard arm (it was already emitted above — but wait, arm_bbs covers all arms)
            // Actually the wildcard arm was already emitted in the loop above.
            // Default just branches to the wildcard arm's block.
            // But that arm has already been emitted as its own block.
            // We need to route default to that arm block.
            // However, the wildcard arm already has a branch to merge_bb...
            // The issue is the default block references the wildcard arm BB.
            // The simplest fix: emit wildcard arm code in the default block directly.
            // But we already emitted it in arm_bbs[wild_idx]...
            // Let's just branch to it from default.
            self.push_instr(&format!("br label %{arm_bb}"));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        // Merge block + phi
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            // Use the first non-i64 type found (e.g. ptr for String arms), else i64.
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            // Add undef entries for arms that branch here but produced no value.
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// Returns true if any variant of `enum_name` has tuple payload fields (#1200).
    ///
    /// Payload enums lower to `{ i8, ptr }`; pure unit enums stay as `i64` discriminants.
    pub(super) fn enum_has_payloads(&self, enum_name: &str) -> bool {
        self.enum_variant_fields
            .get(enum_name)
            .is_some_and(|vs| vs.iter().any(|f| !f.is_empty()))
    }

    /// Split a qualified variant name `"Type::Variant"` into `(type, variant)`.
    pub(super) fn split_qualified(name: &str) -> Option<(&str, &str)> {
        let pos = name.find("::")?;
        Some((&name[..pos], &name[pos + 2..]))
    }

    /// Look up the tuple payload types for `Type::Variant` (#1200).
    pub(super) fn variant_payload_types(&self, qualified_name: &str) -> Option<&[TypeExpr]> {
        let (type_name, variant_name) = Self::split_qualified(qualified_name)?;
        let names = self.enum_variants.get(type_name)?;
        let idx = names.iter().position(|n| n == variant_name)?;
        let fields = self.enum_variant_fields.get(type_name)?;
        fields.get(idx).map(|v| v.as_slice())
    }

    /// Resolve a pattern name like "Shape::Circle" to its discriminant i64.
    pub(super) fn pattern_discriminant(&self, name: &str) -> Option<i64> {
        if let Some(pos) = name.find("::") {
            let type_name = &name[..pos];
            let variant_name = &name[pos + 2..];
            if let Some(variants) = self.enum_variants.get(type_name) {
                if let Some(idx) = variants.iter().position(|v| v == variant_name) {
                    return Some(idx as i64);
                }
            }
        }
        None
    }

    // ── Expression emission ───────────────────────────────────────────────

    pub(super) fn emit_expr(&mut self, expr: &Expr) -> Result<Option<String>, String> {
        match expr {
            Expr::Literal(lit, _) => self.emit_literal(lit),

            Expr::Ident(name, _) => {
                // `None` as a bare identifier → Option None constructor.
                if name == "None" {
                    return self.emit_none_constructor();
                }
                // Qualified enum variant: "Shape::Circle" → discriminant i64,
                // or "LinkedList::Nil" (payload enum, unit variant) → { i8, ptr } (#1200).
                if name.contains("::") {
                    if let Some(disc) = self.pattern_discriminant(name) {
                        if let Some((type_name, _)) = Self::split_qualified(name) {
                            if self.enum_has_payloads(type_name) {
                                return self.emit_enum_variant_constructor(name, disc, &[]);
                            }
                        }
                        return Ok(Some(format!("{disc}")));
                    }
                }
                if let Some(loc) = self.ref_locals.get(name).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.reg_types.insert(reg.clone(), ty_str);
                    return Ok(Some(reg));
                }
                if let Some(val) = self.locals.get(name).cloned() {
                    return Ok(Some(val));
                }
                Ok(None)
            }

            Expr::Binary {
                op, left, right, ..
            } => self.emit_binary(op, left, right),

            Expr::Unary { op, expr, .. } => self.emit_unary(op, expr),

            Expr::If {
                cond, then, else_, ..
            } => self.emit_if_expr(cond, then, else_.as_deref()),

            Expr::Block(block) => self.emit_block(block),

            Expr::FnCall { name, args, .. } => self.emit_fn_call(name, args),

            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.emit_method_call(receiver, method, args),

            Expr::Construct { name, fields, .. } => self.emit_construct(name, fields),

            Expr::FieldAccess { expr, field, .. } => self.emit_field_access(expr, field),

            Expr::Match {
                scrutinee, arms, ..
            } => self.emit_match_expr(scrutinee, arms),

            Expr::List { elems, .. } => self.emit_list_literal(elems),

            Expr::Set { elems, .. } => self.emit_list_literal(elems),

            Expr::Map { pairs, .. } => self.emit_map_literal(pairs),

            Expr::Consume { expr, .. } | Expr::Relabel { expr, .. } | Expr::As { expr, .. } => {
                self.emit_expr(expr)
            }

            Expr::Propagate { expr, .. } => self.emit_propagate(expr),

            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => self.emit_lambda(params, ret_type.as_deref(), body),

            Expr::Spawn {
                actor_type, fields, ..
            } => self.emit_actor_spawn(actor_type, fields),

            _ => Ok(None),
        }
    }

    // ── Literal emission ──────────────────────────────────────────────────

    pub(super) fn emit_literal(&mut self, lit: &Literal) -> Result<Option<String>, String> {
        match lit {
            Literal::Integer(n) => Ok(Some(format!("{n}"))),
            Literal::Float(f) => Ok(Some(if f.fract() == 0.0 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            })),
            Literal::Bool(b) => Ok(Some(if *b {
                "true".to_string()
            } else {
                "false".to_string()
            })),
            Literal::Str(s) => Ok(Some(self.emit_string_literal(s))),
            Literal::Unit => Ok(None),
            Literal::Char(c) => Ok(Some(format!("{}", *c as u32))),
        }
    }

    // ── Binary operators ──────────────────────────────────────────────────

    pub(super) fn emit_binary(
        &mut self,
        op: &BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        if op.is_short_circuit() {
            return match op {
                BinaryOp::And => self.emit_short_circuit_and(left, right),
                BinaryOp::Or => self.emit_short_circuit_or(left, right),
                _ => unreachable!("is_short_circuit but not And or Or"),
            };
        }

        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let lhs_ty = self.type_of_expr(left);
        // Use the resolved LLVM type to detect float arithmetic: a parameter of
        // type Float is resolved to "double" by type_of_expr (via checker types
        // when available, or AST TypeExpr fallback), while expr_is_float only
        // checks for float literals and binary expressions.
        let is_float = lhs_ty == "double" || Self::expr_is_float(left);

        // String equality/inequality: delegate to runtime via mvl_string_eq.
        if lhs_ty == "ptr" && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            self.ensure_extern("declare i1 @_mvl_string_eq(ptr, ptr)");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call i1 @_mvl_string_eq(ptr {lv}, ptr {rv})"
            ));
            if matches!(op, BinaryOp::Ne) {
                let neg = self.next_reg();
                self.push_instr(&format!("{neg} = xor i1 {reg}, true"));
                self.reg_types.insert(neg.clone(), "i1".into());
                return Ok(Some(neg));
            }
            self.reg_types.insert(reg.clone(), "i1".into());
            return Ok(Some(reg));
        }

        let instr = Self::binary_instr(op, is_float, &lhs_ty, &lv, &rv);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = {instr}"));

        // Track type: comparison ops → i1, others → i64/double
        let result_ty = if op.is_comparison() {
            "i1"
        } else if is_float {
            "double"
        } else {
            "i64"
        };
        self.reg_types.insert(reg.clone(), result_ty.into());
        Ok(Some(reg))
    }

    pub(super) fn binary_instr(
        op: &BinaryOp,
        is_float: bool,
        lhs_ty: &str,
        lv: &str,
        rv: &str,
    ) -> String {
        let is_bool = lhs_ty == "i1";
        match op {
            BinaryOp::Add if is_float => format!("fadd double {lv}, {rv}"),
            BinaryOp::Sub if is_float => format!("fsub double {lv}, {rv}"),
            BinaryOp::Mul if is_float => format!("fmul double {lv}, {rv}"),
            BinaryOp::Div if is_float => format!("fdiv double {lv}, {rv}"),
            BinaryOp::Add => format!("add i64 {lv}, {rv}"),
            BinaryOp::Sub => format!("sub i64 {lv}, {rv}"),
            BinaryOp::Mul => format!("mul i64 {lv}, {rv}"),
            BinaryOp::Div => format!("sdiv i64 {lv}, {rv}"),
            BinaryOp::Rem => format!("srem i64 {lv}, {rv}"),
            BinaryOp::Eq if is_float => format!("fcmp oeq double {lv}, {rv}"),
            BinaryOp::Ne if is_float => format!("fcmp one double {lv}, {rv}"),
            BinaryOp::Lt if is_float => format!("fcmp olt double {lv}, {rv}"),
            BinaryOp::Gt if is_float => format!("fcmp ogt double {lv}, {rv}"),
            BinaryOp::Le if is_float => format!("fcmp ole double {lv}, {rv}"),
            BinaryOp::Ge if is_float => format!("fcmp oge double {lv}, {rv}"),
            BinaryOp::Eq if is_bool => format!("icmp eq i1 {lv}, {rv}"),
            BinaryOp::Ne if is_bool => format!("icmp ne i1 {lv}, {rv}"),
            BinaryOp::Eq => format!("icmp eq i64 {lv}, {rv}"),
            BinaryOp::Ne => format!("icmp ne i64 {lv}, {rv}"),
            BinaryOp::Lt => format!("icmp slt i64 {lv}, {rv}"),
            BinaryOp::Gt => format!("icmp sgt i64 {lv}, {rv}"),
            BinaryOp::Le => format!("icmp sle i64 {lv}, {rv}"),
            BinaryOp::Ge => format!("icmp sge i64 {lv}, {rv}"),
            BinaryOp::BitAnd => format!("and i64 {lv}, {rv}"),
            BinaryOp::BitOr => format!("or i64 {lv}, {rv}"),
            BinaryOp::BitXor => format!("xor i64 {lv}, {rv}"),
            BinaryOp::Shl => format!("shl i64 {lv}, {rv}"),
            BinaryOp::Shr => format!("ashr i64 {lv}, {rv}"),
            BinaryOp::And | BinaryOp::Or => unreachable!("handled before binary_instr"),
        }
    }

    pub(super) fn expr_is_float(expr: &Expr) -> bool {
        match expr {
            Expr::Literal(Literal::Float(_), _) => true,
            Expr::Binary { left, .. } => Self::expr_is_float(left),
            _ => false,
        }
    }

    // ── Short-circuit && / || ─────────────────────────────────────────────

    pub(super) fn emit_short_circuit_and(
        &mut self,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("and_rhs");
        let merge_bb = self.next_bb("and_merge");
        let left_end = self.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{rhs_bb}, label %{merge_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ false, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    pub(super) fn emit_short_circuit_or(
        &mut self,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("or_rhs");
        let merge_bb = self.next_bb("or_merge");
        let left_end = self.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{merge_bb}, label %{rhs_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ true, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    // ── Unary operators ───────────────────────────────────────────────────

    pub(super) fn emit_unary(
        &mut self,
        op: &UnaryOp,
        expr: &Expr,
    ) -> Result<Option<String>, String> {
        let val = match self.emit_expr(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let is_float = Self::expr_is_float(expr);
        let reg = self.next_reg();
        match op {
            UnaryOp::Neg if is_float => {
                self.push_instr(&format!("{reg} = fneg double {val}"));
                self.reg_types.insert(reg.clone(), "double".into());
            }
            UnaryOp::Neg => {
                self.push_instr(&format!("{reg} = sub i64 0, {val}"));
                self.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Not => {
                self.push_instr(&format!("{reg} = xor i1 {val}, true"));
                self.reg_types.insert(reg.clone(), "i1".into());
            }
            UnaryOp::BitNot => {
                self.push_instr(&format!("{reg} = xor i64 {val}, -1"));
                self.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Deref => {
                // Box[T] is represented as `ptr` to a heap T (#571). The deref
                // needs to load T. We infer the load type from the receiver's
                // MVL type if it's Box[T]; otherwise we treat `*x` as identity
                // (e.g. when type info is unavailable in the LLVM emitter).
                if let Some(load_ty) = self.box_inner_llvm_ty(expr) {
                    let loaded = self.next_reg();
                    self.push_instr(&format!("{loaded} = load {load_ty}, ptr {val}"));
                    self.reg_types.insert(loaded.clone(), load_ty);
                    return Ok(Some(loaded));
                }
                return Ok(Some(val));
            }
        }
        Ok(Some(reg))
    }

    // ── If expression (phi) ───────────────────────────────────────────────

    pub(super) fn emit_if_phi(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Block>,
    ) -> Result<Option<String>, String> {
        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        let then_val = self.emit_block(then)?;
        let then_end = self.current_bb.clone();
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        let else_val = if let Some(b) = else_ {
            self.emit_block(b)?
        } else {
            None
        };
        let else_end = self.current_bb.clone();
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);

        match (then_val, else_val) {
            (Some(tv), Some(ev)) => {
                let phi_ty = self.infer_val_type(&tv).clone();
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                ));
                self.reg_types.insert(result.clone(), phi_ty);
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn emit_if_expr(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Expr>,
    ) -> Result<Option<String>, String> {
        match else_ {
            Some(Expr::Block(b)) => self.emit_if_phi(cond, then, Some(b)),
            Some(nested_if @ Expr::If { .. }) => {
                let cond_val = match self.emit_expr(cond)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let then_bb = self.next_bb("then");
                let else_bb = self.next_bb("else");
                let merge_bb = self.next_bb("merge");
                self.push_instr(&format!(
                    "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
                ));
                self.start_bb(&then_bb);
                let then_val = self.emit_block(then)?;
                let then_end = self.current_bb.clone();
                if !self.terminated {
                    self.push_instr(&format!("br label %{merge_bb}"));
                }
                self.start_bb(&else_bb);
                let else_val = self.emit_expr(nested_if)?;
                let else_end = self.current_bb.clone();
                if !self.terminated {
                    self.push_instr(&format!("br label %{merge_bb}"));
                }
                self.start_bb(&merge_bb);
                match (then_val, else_val) {
                    (Some(tv), Some(ev)) => {
                        let phi_ty = self.infer_val_type(&tv);
                        let result = self.next_reg();
                        self.push_instr(&format!(
                            "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                        ));
                        self.reg_types.insert(result.clone(), phi_ty);
                        Ok(Some(result))
                    }
                    _ => Ok(None),
                }
            }
            None => self.emit_if_phi(cond, then, None),
            Some(_) => self.emit_if_phi(cond, then, None),
        }
    }

    // ── Function call emission ────────────────────────────────────────────

    pub(super) fn emit_fn_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        // ── Builtins ──────────────────────────────────────────────────────
        match name {
            "assert" => return self.emit_assert_builtin(args),
            "println" | "print" | "eprintln" => return self.emit_println_builtin(name, args),
            "format" => return self.emit_format_builtin(args),
            "Ok" | "Err" => return self.emit_result_constructor(name, args),
            "Some" => return self.emit_option_constructor(args),
            "None" => return self.emit_none_constructor(),
            _ => {}
        }

        // ── Enum variant constructors: "Shape::Circle" or "LinkedList::Cons(...)" ─
        if name.contains("::") {
            if let Some(disc) = self.pattern_discriminant(name) {
                let (type_name, _variant_name) = Self::split_qualified(name)
                    .ok_or_else(|| format!("malformed qualified name: {name}"))?;
                if self.enum_has_payloads(type_name) {
                    return self.emit_enum_variant_constructor(name, disc, args);
                }
                // Pure unit enum — bare i64 discriminant (legacy path).
                return Ok(Some(format!("{disc}")));
            }
        }

        // ── Box::new(x) — heap-allocate and store x, return ptr ──────────
        // Supports primitive payloads only: i64/ptr/double (8B), i32 (4B),
        // i8/i1 (1B). Aggregate payloads (structs, enums) need a real
        // sizeof — emit a hard error instead of guessing 8B (would be a
        // heap buffer overflow when the struct is larger).
        if name == "Box::new" && args.len() == 1 {
            let arg_ty = self.type_of_expr(&args[0]);
            let size: i64 = match arg_ty.as_str() {
                "i64" | "ptr" | "double" => 8,
                "i32" => 4,
                "i8" | "i1" => 1,
                // Payload-enum tagged union { i8 tag, ptr payload } (#1200).
                // On 64-bit: i8 (1) + 7B padding + ptr (8) = 16 bytes.
                t if t == RESULT_LLVM_TY => 16,
                other => {
                    return Err(format!(
                        "Box::new: unsupported payload type `{other}` — only primitive \
                         types are supported by llvm_text. Aggregate types need real \
                         sizeof support (#1154 follow-up)."
                    ));
                }
            };
            let val = match self.emit_expr(&args[0])? {
                Some(v) => v,
                None => return Ok(None),
            };
            self.ensure_extern("declare ptr @_mvl_box_new(i64)");
            let ptr = self.next_reg();
            self.push_instr(&format!("{ptr} = call ptr @_mvl_box_new(i64 {size})"));
            self.push_instr(&format!("store {arg_ty} {val}, ptr {ptr}"));
            self.reg_types.insert(ptr.clone(), "ptr".into());
            return Ok(Some(ptr));
        }

        // ── Stdlib C-ABI dispatch (#1202) ────────────────────────────────
        // Must run before generic_fns check: generic builtins like `choice` are
        // also in generic_fns but have no MVL body to monomorphize.
        // Functions whose pure-MVL bodies are stripped from the prelude (to avoid
        // SSA dominance bugs) or whose return type isn't registered (opaque types).
        match name {
            "path" if args.len() == 1 => {
                let s = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_io_path(ptr)");
                let r = self.next_reg();
                self.push_instr(&format!("{r} = call ptr @_mvl_io_path(ptr {s})"));
                self.reg_types.insert(r.clone(), "ptr".into());
                return Ok(Some(r));
            }
            "format_datetime" if args.len() == 2 => {
                let dt = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let pattern = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // Flatten DateTime { year, month, day, hour, minute, second } → 6 × i64.
                let mut fields = Vec::new();
                for i in 0..6usize {
                    let r = self.next_reg();
                    self.push_instr(&format!("{r} = extractvalue %DateTime {dt}, {i}"));
                    self.reg_types.insert(r.clone(), "i64".into());
                    fields.push(r);
                }
                let args_str = format!(
                    "i64 {}, i64 {}, i64 {}, i64 {}, i64 {}, i64 {}, ptr {}",
                    fields[0], fields[1], fields[2], fields[3], fields[4], fields[5], pattern
                );
                self.ensure_extern(
                    "declare ptr @_mvl_time_format_datetime(i64, i64, i64, i64, i64, i64, ptr)",
                );
                let r = self.next_reg();
                self.push_instr(&format!(
                    "{r} = call ptr @_mvl_time_format_datetime({args_str})"
                ));
                self.reg_types.insert(r.clone(), "ptr".into());
                return Ok(Some(r));
            }
            "format_instant" if args.len() == 2 => {
                let handle = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let pattern = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_time_format_instant(ptr, ptr)");
                let r = self.next_reg();
                self.push_instr(&format!(
                    "{r} = call ptr @_mvl_time_format_instant(ptr {handle}, ptr {pattern})"
                ));
                self.reg_types.insert(r.clone(), "ptr".into());
                return Ok(Some(r));
            }
            "find_all" if args.len() == 2 => {
                let re = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let s = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_regex_find_all(ptr, ptr)");
                let r = self.next_reg();
                self.push_instr(&format!(
                    "{r} = call ptr @_mvl_regex_find_all(ptr {re}, ptr {s})"
                ));
                self.reg_types.insert(r.clone(), "ptr".into());
                return Ok(Some(r));
            }
            "replace" if args.len() == 3 => {
                let re = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let s = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let repl = match self.emit_expr(&args[2])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_regex_replace(ptr, ptr, ptr)");
                let r = self.next_reg();
                self.push_instr(&format!(
                    "{r} = call ptr @_mvl_regex_replace(ptr {re}, ptr {s}, ptr {repl})"
                ));
                self.reg_types.insert(r.clone(), "ptr".into());
                return Ok(Some(r));
            }
            "choice" if args.len() == 1 => {
                return self.emit_choice_call(&args[0]);
            }
            "List::filled" if args.len() == 2 => {
                return self.emit_list_filled(&args[0], &args[1]);
            }
            "float_checked_to_int" if args.len() == 1 => {
                let v = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i8 @mvl_float_checked_to_int(double, ptr)");
                let out = self.next_reg();
                self.push_instr(&format!("{out} = alloca i64"));
                let tag = self.next_reg();
                self.push_instr(&format!(
                    "{tag} = call i8 @mvl_float_checked_to_int(double {v}, ptr {out})"
                ));
                // Load the value (only meaningful when tag==0, i.e. Some).
                let val = self.next_reg();
                self.push_instr(&format!("{val} = load i64, ptr {out}"));
                // Alloca for the payload pointer expected by { i8, ptr }.
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca i64"));
                self.push_instr(&format!("store i64 {val}, ptr {slot}"));
                let r = self.wrap_result_pair(&tag, &slot);
                return Ok(Some(r));
            }
            _ => {}
        }

        // ── Generic function monomorphization ───────────────────────────
        if self.generic_fns.contains_key(name) {
            return self.emit_monomorphized_call(name, args);
        }

        // ── Local closure call (closure-over-closure capture) ─────────
        // If `name` is a local binding holding a `%__closure_type` (e.g. a
        // captured closure), do an indirect call through its fn_ptr field,
        // passing its env_ptr as the first argument.
        if let Some(closure_ptr) = self.locals.get(name).cloned() {
            if self
                .local_mvl_types
                .get(name)
                .is_some_and(|t| matches!(t, TypeExpr::Fn { .. }))
            {
                // Load fn_ptr (field 0) and env_ptr (field 1).
                let fn_field = self.next_reg();
                self.push_instr(&format!(
                    "{fn_field} = getelementptr %__closure_type, ptr {closure_ptr}, i32 0, i32 0"
                ));
                let fn_ptr = self.next_reg();
                self.push_instr(&format!("{fn_ptr} = load ptr, ptr {fn_field}"));
                let env_field = self.next_reg();
                self.push_instr(&format!(
                    "{env_field} = getelementptr %__closure_type, ptr {closure_ptr}, i32 0, i32 1"
                ));
                let env_ptr = self.next_reg();
                self.push_instr(&format!("{env_ptr} = load ptr, ptr {env_field}"));

                // Emit arguments.
                let mut call_args = vec![format!("ptr {env_ptr}")];
                for arg in args {
                    let ty = self.type_of_expr(arg);
                    if let Some(v) = self.emit_expr(arg)? {
                        call_args.push(format!("{ty} {v}"));
                    }
                }
                let args_str = call_args.join(", ");

                // Determine return type from the fn type annotation.
                let (llvm_ret, is_void) =
                    if let Some(TypeExpr::Fn { ret, .. }) = self.local_mvl_types.get(name) {
                        let r = self.llvm_ty_ctx(ret);
                        let v = Self::is_void(ret);
                        (r, v)
                    } else {
                        ("i64".into(), false)
                    };

                if is_void {
                    self.push_instr(&format!("call void {fn_ptr}({args_str})"));
                    return Ok(None);
                } else {
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = call {llvm_ret} {fn_ptr}({args_str})"));
                    self.reg_types.insert(reg.clone(), llvm_ret);
                    return Ok(Some(reg));
                }
            }
        }

        // ── User-defined functions ─────────────────────────────────────
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.type_of_expr(arg);
            if let Some(v) = self.emit_expr(arg)? {
                arg_vals.push((ty, v));
            }
        }
        let ret_ty = self
            .fn_ret_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });

        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        // If this is a builtin fn, dispatch to the C-ABI symbol directly.
        // For opaque handle types (where the SSA register actually holds `ptr`
        // but type_of_expr reports `%StructName`), rewrite the argument type to
        // `ptr` so the LLVM declare and call match the register's true type.
        // Inline struct values (e.g. Duration from a constructor) keep their
        // struct type — reg_types will hold `%Duration`, not `ptr`.
        let (effective_name, is_c_builtin, args_str): (String, bool, String) =
            if let Some(c_sym) = self.builtin_syms.get(name).cloned() {
                let c_abi_args: Vec<(String, &str)> = arg_vals
                    .iter()
                    .map(|(ty, v)| {
                        let actual_ty = self.reg_types.get(v).cloned();
                        let abi_ty = if ty.starts_with('%') && actual_ty.as_deref() == Some("ptr") {
                            "ptr".to_string()
                        } else {
                            ty.clone()
                        };
                        (abi_ty, v.as_str())
                    })
                    .collect();
                let param_tys = c_abi_args
                    .iter()
                    .map(|(ty, _)| ty.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.ensure_extern(&format!("declare {llvm_ret} @{c_sym}({param_tys})"));
                let abi_args_str = c_abi_args
                    .iter()
                    .map(|(ty, v)| format!("{ty} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (c_sym, true, abi_args_str)
            } else {
                let args_str = arg_vals
                    .iter()
                    .map(|(ty, v)| format!("{ty} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                (name.to_string(), false, args_str)
            };

        if is_void {
            self.push_instr(&format!("call void @{effective_name}({args_str})"));
            Ok(None)
        } else {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call {llvm_ret} @{effective_name}({args_str})"
            ));
            self.reg_types.insert(reg.clone(), llvm_ret.clone());

            // C-ABI builtins that return `{ i8, ptr }` store the raw value directly
            // in the payload field.  MVL-constructed Ok/Err store a slot pointer in
            // field 1 (see emit_result_constructor).  Wrap the C payload into a slot
            // so emit_result_match can use a uniform `load T, ptr payload` convention.
            if is_c_builtin && llvm_ret == RESULT_LLVM_TY {
                // C-ABI builtins store the raw value directly in field 1.
                // MVL-constructed Ok/Err store a slot pointer in field 1 (see
                // emit_result_constructor).  Wrap the C payload into a slot so
                // emit_result_match can use a uniform `load T, ptr payload` convention.
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {RESULT_LLVM_TY} {reg}, 0"));
                self.reg_types.insert(disc.clone(), "i8".into());
                let raw_payload = self.next_reg();
                self.push_instr(&format!(
                    "{raw_payload} = extractvalue {RESULT_LLVM_TY} {reg}, 1"
                ));
                self.reg_types.insert(raw_payload.clone(), "ptr".into());
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca ptr"));
                self.push_instr(&format!("store ptr {raw_payload}, ptr {slot}"));
                let r1 = self.wrap_result_pair(&disc, &slot);
                return Ok(Some(r1));
            }

            Ok(Some(reg))
        }
    }

    pub(super) fn emit_assert_builtin(&mut self, args: &[Expr]) -> Result<Option<String>, String> {
        let cond = match args.first() {
            Some(a) => a,
            None => return Ok(None),
        };
        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };
        // Widen i1 to i1 — it already is, but make sure we're treating it as i1
        let ok_bb = self.next_bb("assert_ok");
        let fail_bb = self.next_bb("assert_fail");
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{ok_bb}, label %{fail_bb}"
        ));
        self.fn_buf.push(format!("{fail_bb}:"));
        self.current_bb = fail_bb.clone();
        self.terminated = false;
        self.ensure_extern("declare void @llvm.trap()");
        self.push_instr("call void @llvm.trap()");
        self.push_instr("unreachable");
        self.terminated = true;
        self.fn_buf.push(format!("{ok_bb}:"));
        self.current_bb = ok_bb;
        self.terminated = false;
        Ok(None)
    }

    pub(super) fn emit_println_builtin(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let fd = if name == "eprintln" { 2i32 } else { 1i32 };
        if args.is_empty() {
            // println() with no args — just print newline
            let fmt = self.ensure_println_fmt();
            self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
            let empty_g = self.emit_str_global("");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call ptr @_mvl_string_new(ptr @{empty_g}, i64 0)"
            ));
            self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
            let raw = self.next_reg();
            self.push_instr(&format!("{raw} = call ptr @_mvl_string_ptr(ptr {reg})"));
            self.push_instr(&format!(
                "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
            ));
            return Ok(None);
        }
        let val = match self.emit_expr(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let fmt = self.ensure_println_fmt();
        self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
        self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
        let raw = self.next_reg();
        self.push_instr(&format!("{raw} = call ptr @_mvl_string_ptr(ptr {val})"));
        self.push_instr(&format!(
            "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
        ));
        Ok(None)
    }

    // ── random.choice custom codegen (#1202) ─────────────────────────────

    /// Emit `choice[T](list)` using `_mvl_random_choice_index`.
    ///
    /// Returns `Option[T]` as `{ i8, ptr }`: disc=0 for Some, disc=1 for None.
    /// Index −1 from the runtime signals an empty list → None.
    pub(super) fn emit_choice_call(&mut self, list_arg: &Expr) -> Result<Option<String>, String> {
        // Determine the element LLVM type from the MVL type of the argument.
        let elem_llvm_ty = match list_arg {
            Expr::Ident(name, _) => {
                if let Some(mvl_ty) = self.local_mvl_types.get(name.as_str()) {
                    match mvl_ty {
                        TypeExpr::Base {
                            args: type_args, ..
                        } if !type_args.is_empty() => self.llvm_ty_ctx(&type_args[0].clone()),
                        _ => "i64".to_string(),
                    }
                } else {
                    "i64".to_string()
                }
            }
            _ => "i64".to_string(),
        };

        let arr = match self.emit_expr(list_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };

        self.ensure_extern("declare i64 @_mvl_random_choice_index(ptr)");
        let idx = self.next_reg();
        self.push_instr(&format!(
            "{idx} = call i64 @_mvl_random_choice_index(ptr {arr})"
        ));
        self.reg_types.insert(idx.clone(), "i64".into());

        let is_none = self.next_reg();
        self.push_instr(&format!("{is_none} = icmp eq i64 {idx}, -1"));
        self.reg_types.insert(is_none.clone(), "i1".into());

        let none_bb = self.next_bb("choice_none");
        let some_bb = self.next_bb("choice_some");
        let merge_bb = self.next_bb("choice_merge");

        // Allocate a result slot shared by both branches.
        let result_slot = self.next_reg();
        self.push_instr(&format!("{result_slot} = alloca {RESULT_LLVM_TY}"));
        self.reg_types.insert(result_slot.clone(), "ptr".into());

        self.push_instr(&format!(
            "br i1 {is_none}, label %{none_bb}, label %{some_bb}"
        ));

        // ── None branch ──────────────────────────────────────────────────
        self.start_bb(&none_bb);
        let none_r0 = self.next_reg();
        self.push_instr(&format!(
            "{none_r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 1, 0"
        ));
        self.reg_types
            .insert(none_r0.clone(), RESULT_LLVM_TY.into());
        let none_r1 = self.next_reg();
        self.push_instr(&format!(
            "{none_r1} = insertvalue {RESULT_LLVM_TY} {none_r0}, ptr null, 1"
        ));
        self.reg_types
            .insert(none_r1.clone(), RESULT_LLVM_TY.into());
        self.push_instr(&format!(
            "store {RESULT_LLVM_TY} {none_r1}, ptr {result_slot}"
        ));
        self.push_instr(&format!("br label %{merge_bb}"));
        self.terminated = true;

        // ── Some branch ──────────────────────────────────────────────────
        self.start_bb(&some_bb);
        self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");
        let elem_ptr = self.next_reg();
        self.push_instr(&format!(
            "{elem_ptr} = call ptr @_mvl_array_get(ptr {arr}, i64 {idx})"
        ));
        self.reg_types.insert(elem_ptr.clone(), "ptr".into());
        let elem_val = self.next_reg();
        self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
        self.reg_types
            .insert(elem_val.clone(), elem_llvm_ty.clone());
        let elem_slot = self.next_reg();
        self.push_instr(&format!("{elem_slot} = alloca {elem_llvm_ty}"));
        self.push_instr(&format!("store {elem_llvm_ty} {elem_val}, ptr {elem_slot}"));
        let some_r0 = self.next_reg();
        self.push_instr(&format!(
            "{some_r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 0, 0"
        ));
        self.reg_types
            .insert(some_r0.clone(), RESULT_LLVM_TY.into());
        let some_r1 = self.next_reg();
        self.push_instr(&format!(
            "{some_r1} = insertvalue {RESULT_LLVM_TY} {some_r0}, ptr {elem_slot}, 1"
        ));
        self.reg_types
            .insert(some_r1.clone(), RESULT_LLVM_TY.into());
        self.push_instr(&format!(
            "store {RESULT_LLVM_TY} {some_r1}, ptr {result_slot}"
        ));
        self.push_instr(&format!("br label %{merge_bb}"));
        self.terminated = true;

        // ── Merge ────────────────────────────────────────────────────────
        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = load {RESULT_LLVM_TY}, ptr {result_slot}"
        ));
        self.reg_types.insert(result.clone(), RESULT_LLVM_TY.into());
        Ok(Some(result))
    }
}

/// Classify an arm pattern for LLVM switch generation.
/// Returns `true` if the pattern was fully classified (as a discriminant or wildcard).
/// Handles `Pattern::Or` by recursively classifying each alternative.
fn collect_or_discriminants(
    pattern: &Pattern,
    idx: usize,
    emitter: &TextEmitter,
    switch_arms: &mut Vec<(i64, usize)>,
    wildcard_arm: &mut Option<usize>,
) -> bool {
    match pattern {
        Pattern::Or { patterns, .. } => {
            let mut any = false;
            for p in patterns {
                any |= collect_or_discriminants(p, idx, emitter, switch_arms, wildcard_arm);
            }
            any
        }
        Pattern::TupleStruct { name, .. } => {
            if let Some(disc) = emitter.pattern_discriminant(name) {
                switch_arms.push((disc, idx));
                true
            } else {
                false
            }
        }
        Pattern::Ident(name, _) if name.contains("::") => {
            if let Some(disc) = emitter.pattern_discriminant(name) {
                switch_arms.push((disc, idx));
                true
            } else {
                false
            }
        }
        Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
            *wildcard_arm = Some(idx);
            true
        }
        _ => false,
    }
}
