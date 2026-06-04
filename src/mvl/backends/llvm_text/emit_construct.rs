// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Algebraic type construction, struct construction, and collection literal emission for the `llvm_text` backend.

use crate::mvl::parser::ast::{Expr, MatchArm, MatchBody, Pattern, TypeExpr};

use super::{TextEmitter, RESULT_LLVM_TY};

impl TextEmitter {
    // ── Result[T,E] helpers ───────────────────────────────────────────────

    /// Build a `{ i8, ptr }` Result aggregate from a discriminant byte and a payload slot pointer.
    ///
    /// Both fields are immediately overwritten, so `zeroinitializer` is used as the base
    /// (safe if the struct ever gains padding fields, unlike `undef`).
    pub(super) fn wrap_result_pair(&mut self, disc: &str, slot: &str) -> String {
        let r0 = self.next_reg();
        self.push_instr(&format!(
            "{r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 {disc}, 0"
        ));
        self.reg_types.insert(r0.clone(), RESULT_LLVM_TY.into());
        let r1 = self.next_reg();
        self.push_instr(&format!(
            "{r1} = insertvalue {RESULT_LLVM_TY} {r0}, ptr {slot}, 1"
        ));
        self.reg_types.insert(r1.clone(), RESULT_LLVM_TY.into());
        r1
    }

    /// Emit `Ok(val)` or `Err(val)` — builds a `{ i8, ptr }` tagged union.
    pub(super) fn emit_result_constructor(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let disc: i64 = if name == "Ok" { 0 } else { 1 };
        let arg_ty;
        let slot;
        if let Some(arg) = args.first() {
            let inferred_ty = self.type_of_expr(arg);
            if inferred_ty == "void" {
                // Ok(()) / Err(()) — Unit payload; use a dummy slot (no store).
                let _ = self.emit_expr(arg)?;
                arg_ty = "i8".into();
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca i8"));
            } else {
                arg_ty = inferred_ty;
                let arg_val = match self.emit_expr(arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {arg_ty}"));
                self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
            }
        } else {
            arg_ty = "i8".into();
            slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca i8"));
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &slot);
        let _ = arg_ty; // used above
        Ok(Some(r1))
    }

    // ── Enum variant constructor (payload enums, #1200) ──────────────────

    /// Build a `{ i8, ptr }` tagged union for a payload-enum variant.
    ///
    /// Unit variants → payload ptr is null.
    /// Tuple variants → allocate one slot per field on the stack, store the args,
    /// then point the payload at the first slot. Match-arm extraction GEPs across
    /// these slots by index (each slot is `ptr`-sized so GEP is uniform).
    pub(super) fn emit_enum_variant_constructor(
        &mut self,
        qualified_name: &str,
        disc: i64,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let field_tys: Vec<TypeExpr> = self
            .variant_payload_types(qualified_name)
            .map(|s| s.to_vec())
            .unwrap_or_default();

        let payload_ptr: String = if field_tys.is_empty() {
            // Unit variant in a payload enum — null payload.
            "null".to_string()
        } else {
            if args.len() != field_tys.len() {
                return Err(format!(
                    "variant {qualified_name}: expected {} fields, got {}",
                    field_tys.len(),
                    args.len()
                ));
            }
            // Allocate a flat array of ptr-sized slots (one per field). Each slot
            // is typed by the field's LLVM type at store/load time. This matches
            // Option/Result's `alloca` + `store` pattern but extends it to N fields.
            let n = field_tys.len();
            let base = self.next_reg();
            self.push_instr(&format!("{base} = alloca [{n} x i64]"));
            for (i, (ty_expr, arg)) in field_tys.iter().zip(args.iter()).enumerate() {
                let field_llvm = self.llvm_ty_ctx(ty_expr);
                let arg_val = match self.emit_expr(arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let slot = self.next_reg();
                self.push_instr(&format!(
                    "{slot} = getelementptr [{n} x i64], ptr {base}, i32 0, i32 {i}"
                ));
                self.push_instr(&format!("store {field_llvm} {arg_val}, ptr {slot}"));
            }
            base
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &payload_ptr);
        Ok(Some(r1))
    }

    // ── Option[T] helpers (#1156) ────────────────────────────────────────

    /// Emit `Some(val)` — builds a `{ i8, ptr }` tagged union with disc=0.
    pub(super) fn emit_option_constructor(
        &mut self,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let arg = match args.first() {
            Some(a) => a,
            None => return self.emit_none_constructor(),
        };
        let arg_ty = self.type_of_expr(arg);
        let arg_val = match self.emit_expr(arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca {arg_ty}"));
        self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
        let r1 = self.wrap_result_pair("0", &slot);
        Ok(Some(r1))
    }

    /// Emit `None` — builds a `{ i8, ptr }` tagged union with disc=1 and null payload.
    pub(super) fn emit_none_constructor(&mut self) -> Result<Option<String>, String> {
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca i8"));
        let r1 = self.wrap_result_pair("1", &slot);
        Ok(Some(r1))
    }

    /// Emit a `match` where at least one arm has `Pattern::Some` / `Pattern::None`.
    pub(super) fn emit_option_match(
        &mut self,
        scrutinee: &Expr,
        scrut_val: &str,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        // Determine the inner MVL and LLVM types from the scrutinee's MVL type.
        let mvl_ty = match scrutinee {
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned(),
            Expr::FnCall { name, .. } => self.fn_ret_types.get(name.as_str()).cloned(),
            _ => None,
        };
        let (inner_load_ty, inner_mvl_ty) = match &mvl_ty {
            Some(TypeExpr::Option { inner, .. }) => {
                (self.llvm_ty_ctx(inner), Some(inner.as_ref().clone()))
            }
            _ => ("ptr".into(), None),
        };

        // Extract discriminant byte from the { i8, ptr } struct.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Build switch on i8 discriminant: Some=0, None=1.
        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Some { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::None(_) => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        // Emit arm blocks (skip wildcard/ident arms — emitted from default_bb).
        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            // Skip wildcard arms here; they are emitted via the default block.
            if Some(idx) == wildcard_arm {
                continue;
            }

            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            let mut bound_var: Option<String> = None;

            match &arm.pattern {
                Pattern::Some { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!(
                        "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    let some_val = self.next_reg();
                    self.push_instr(&format!("{some_val} = load {inner_load_ty}, ptr {pp}"));
                    self.reg_types
                        .insert(some_val.clone(), inner_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.locals.insert(var_name.clone(), some_val.clone());
                            if let Some(ref imty) = inner_mvl_ty {
                                self.local_mvl_types.insert(var_name.clone(), imty.clone());
                            }
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::None(_) => {
                    // Nothing to bind.
                }
                _ => {}
            }

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

            if let Some(ref var_name) = bound_var {
                self.locals.remove(var_name);
                self.local_mvl_types.remove(var_name);
            }
        }

        // Default block — either jumps to wildcard arm body or traps.
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            // Emit the wildcard arm body inline in the default block.
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                self.locals.insert(name.clone(), scrut_val.to_string());
                bound_var = Some(name.clone());
            }
            let arm_val = match &wild_arm.body {
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
            if let Some(ref var_name) = bound_var {
                self.locals.remove(var_name);
                self.local_mvl_types.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        // Merge block + phi.
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            // All arms terminated (e.g. all `return`) — no merge block needed.
            self.fn_buf.push(format!("{merge_bb}:"));
            self.current_bb = merge_bb.clone();
            self.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
            return Ok(None);
        }
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries[0].1.clone();
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
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

    // ── Payload enum match (#1200) ────────────────────────────────────────

    /// Identify whether `scrutinee` is a payload-enum expression and return
    /// the enum type name. Returns `None` for non-enum or pure-unit-enum
    /// scrutinees (those go through the legacy i64-switch path).
    pub(super) fn scrutinee_payload_enum(&self, scrutinee: &Expr) -> Option<String> {
        let mvl_ty = match scrutinee {
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned()?,
            Expr::FnCall { name, .. } => self.fn_ret_types.get(name.as_str()).cloned()?,
            _ => return None,
        };
        // Unwrap label/refinement wrappers.
        let mut cur = &mvl_ty;
        while let TypeExpr::Labeled { inner, .. }
        | TypeExpr::Refined { inner, .. }
        | TypeExpr::Ref { inner, .. } = cur
        {
            cur = inner.as_ref();
        }
        if let TypeExpr::Base { name, .. } = cur {
            if self.enum_variants.contains_key(name) && self.enum_has_payloads(name) {
                return Some(name.clone());
            }
        }
        None
    }

    /// Emit match arms for a payload enum (#1200).
    ///
    /// Scrutinee is `{ i8 tag, ptr payload }`. Each arm is dispatched on the
    /// tag byte; payload fields are loaded by GEP-ing into the payload slot
    /// array (see `emit_enum_variant_constructor` for the storage layout).
    pub(super) fn emit_payload_enum_match(
        &mut self,
        enum_name: &str,
        scrut_val: &str,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        // Extract discriminant byte.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Build switch on i8 discriminant.
        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            let disc_opt = match &arm.pattern {
                Pattern::TupleStruct { name, .. } => self.pattern_discriminant(name),
                Pattern::Ident(name, _) if name.contains("::") => self.pattern_discriminant(name),
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                    continue;
                }
                _ => None,
            };
            if let Some(disc) = disc_opt {
                switch_str.push_str(&format!("    i8 {disc}, label %{}\n", arm_bbs[idx]));
            } else {
                wildcard_arm = Some(idx);
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            if Some(idx) == wildcard_arm {
                continue;
            }
            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            let mut bound_vars: Vec<String> = Vec::new();

            if let Pattern::TupleStruct { name, fields, .. } = &arm.pattern {
                let field_tys: Vec<TypeExpr> = self
                    .variant_payload_types(name)
                    .map(|s| s.to_vec())
                    .unwrap_or_default();
                if !fields.is_empty() && !field_tys.is_empty() {
                    let payload_ptr = self.next_reg();
                    self.push_instr(&format!(
                        "{payload_ptr} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    self.reg_types.insert(payload_ptr.clone(), "ptr".into());
                    let n_slots = field_tys.len();
                    for (i, inner_pat) in fields.iter().enumerate() {
                        let Some(field_ty_expr) = field_tys.get(i) else {
                            continue;
                        };
                        let field_llvm = self.llvm_ty_ctx(field_ty_expr);
                        let slot = self.next_reg();
                        self.push_instr(&format!(
                            "{slot} = getelementptr [{n_slots} x i64], ptr {payload_ptr}, i32 0, i32 {i}"
                        ));
                        let val = self.next_reg();
                        self.push_instr(&format!("{val} = load {field_llvm}, ptr {slot}"));
                        self.reg_types.insert(val.clone(), field_llvm);
                        if let Pattern::Ident(var_name, _) = inner_pat {
                            if var_name != "_" {
                                self.locals.insert(var_name.clone(), val.clone());
                                self.local_mvl_types
                                    .insert(var_name.clone(), field_ty_expr.clone());
                                bound_vars.push(var_name.clone());
                            }
                        }
                    }
                }
            }

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
            for var_name in &bound_vars {
                self.locals.remove(var_name);
                self.local_mvl_types.remove(var_name);
            }
        }

        // Default block — either jumps to wildcard arm body or traps.
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                if !name.contains("::") {
                    self.locals.insert(name.clone(), scrut_val.to_string());
                    bound_var = Some(name.clone());
                }
            }
            let arm_val = match &wild_arm.body {
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
            if let Some(ref var_name) = bound_var {
                self.locals.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        let _ = enum_name; // currently only used implicitly via pattern_discriminant
                           // Merge block + phi.
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            self.fn_buf.push(format!("{merge_bb}:"));
            self.current_bb = merge_bb.clone();
            self.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
            return Ok(None);
        }
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries[0].1.clone();
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
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

    pub(super) fn emit_construct(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Result<Option<String>, String> {
        let field_defs = match self.struct_fields.get(name).cloned() {
            Some(f) => f,
            None => return Ok(None),
        };

        let mut field_vals: Vec<(String, String)> = Vec::new(); // (llvm_ty, val)
        for (field_name, field_ty) in &field_defs {
            let llvm_t = self.llvm_ty_ctx(field_ty);
            // Find the value for this field in the construct expr
            let val = fields
                .iter()
                .find(|(n, _)| n == field_name)
                .and_then(|(_, e)| self.emit_expr(e).ok().flatten())
                .unwrap_or_else(|| "undef".into());
            field_vals.push((llvm_t, val));
        }

        let struct_ty = format!("%{name}");
        let mut acc = "undef".to_string();
        for (i, (field_ty, val)) in field_vals.iter().enumerate() {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = insertvalue {struct_ty} {acc}, {field_ty} {val}, {i}"
            ));
            self.reg_types.insert(reg.clone(), struct_ty.clone());
            acc = reg;
        }
        Ok(Some(acc))
    }

    // ── Field access ──────────────────────────────────────────────────────

    pub(super) fn emit_field_access(
        &mut self,
        expr: &Expr,
        field: &str,
    ) -> Result<Option<String>, String> {
        // In actor method bodies, `self.field` maps to a ref_local GEP pointer.
        // Check this before falling through to extractvalue-based struct access.
        if matches!(expr, Expr::Ident(name, _) if name == "self") {
            if let Some(loc) = self.ref_locals.get(field).cloned() {
                let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                self.reg_types.insert(reg.clone(), ty_str);
                return Ok(Some(reg));
            }
        }

        let struct_name = self.struct_name_of_expr(expr);
        let base_val = match self.emit_expr(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };

        if let Some(sn) = struct_name {
            if let Some(fields) = self.struct_fields.get(&sn).cloned() {
                if let Some(idx) = fields.iter().position(|(f, _)| f == field) {
                    let field_ty = self.llvm_ty_ctx(&fields[idx].1.clone());
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = extractvalue %{sn} {base_val}, {idx}"));
                    self.reg_types.insert(reg.clone(), field_ty);
                    return Ok(Some(reg));
                }
            }
        }
        Ok(None)
    }

    pub(super) fn emit_list_literal(&mut self, elems: &[Expr]) -> Result<Option<String>, String> {
        // Determine element LLVM type from the first expression (default ptr).
        let elem_ty = elems
            .first()
            .map(|e| self.type_of_expr(e))
            .unwrap_or_else(|| "ptr".into());

        // Emit all element values first
        let mut elem_vals: Vec<String> = Vec::new();
        for e in elems {
            if let Some(v) = self.emit_expr(e)? {
                elem_vals.push(v);
            }
        }

        let n = elem_vals.len().max(4) as i64;
        self.ensure_extern("declare ptr @mvl_array_new(i64, i64)");
        self.ensure_extern("declare void @mvl_array_push(ptr, ptr)");

        let arr = self.next_reg();
        // elem_size=8 for all scalar types (i64, ptr, double)
        self.push_instr(&format!("{arr} = call ptr @mvl_array_new(i64 8, i64 {n})"));
        self.reg_types.insert(arr.clone(), "ptr".into());

        for v in &elem_vals {
            let slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca {elem_ty}"));
            self.push_instr(&format!("store {elem_ty} {v}, ptr {slot}"));
            self.push_instr(&format!("call void @mvl_array_push(ptr {arr}, ptr {slot})"));
        }

        Ok(Some(arr))
    }

    // ── Map literal ──────────────────────────────────────────────────────

    pub(super) fn emit_map_literal(
        &mut self,
        pairs: &[(Expr, Expr)],
    ) -> Result<Option<String>, String> {
        let n = pairs.len().max(4) as i64;
        self.ensure_extern("declare ptr @mvl_map_new(i64)");
        self.ensure_extern("declare void @_mvl_map_insert(ptr, ptr, i64, ptr, i64)");
        self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
        self.ensure_extern("declare i64 @_mvl_str_len(ptr)");

        let map = self.next_reg();
        self.push_instr(&format!("{map} = call ptr @mvl_map_new(i64 {n})"));
        self.reg_types.insert(map.clone(), "ptr".into());

        for (key_expr, val_expr) in pairs {
            // Emit key (expected to be a String → ptr)
            let key_val = match self.emit_expr(key_expr)? {
                Some(v) => v,
                None => continue,
            };
            // Get raw pointer and length from the MvlString key
            let key_ptr = self.next_reg();
            self.push_instr(&format!(
                "{key_ptr} = call ptr @_mvl_string_ptr(ptr {key_val})"
            ));
            let key_len = self.next_reg();
            self.push_instr(&format!(
                "{key_len} = call i64 @_mvl_str_len(ptr {key_val})"
            ));

            // Emit value and store to stack slot
            let val_val = match self.emit_expr(val_expr)? {
                Some(v) => v,
                None => continue,
            };
            let val_ty = self.infer_val_type(&val_val);
            let val_slot = self.next_reg();
            self.push_instr(&format!("{val_slot} = alloca {val_ty}"));
            self.push_instr(&format!("store {val_ty} {val_val}, ptr {val_slot}"));

            // val_size = 8 for all scalar types (i64, ptr, double)
            self.push_instr(&format!(
                "call void @_mvl_map_insert(ptr {map}, ptr {key_ptr}, i64 {key_len}, ptr {val_slot}, i64 8)"
            ));
        }

        Ok(Some(map))
    }
}
