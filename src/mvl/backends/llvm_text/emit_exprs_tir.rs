// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Expression emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_exprs.rs`. Built leaf-first:
//! 1. `Literal`, `Var` (this commit — minimal leaves)
//! 2. `Unary`, `Binary`, `FieldAccess`
//! 3. `If`, `Match`, `Block`, `FnCall`, `Lambda`, …
//! 4. Composite walkers (`MethodCall`, `Construct`, `Spawn`, …)
//!
//! TIR nodes embed `.ty: Ty` directly — no `module.expr_types.get(span)` lookup
//! is needed. The `Expr::As` variant has been erased by lowering; the inner
//! expression's `.ty` carries the cast destination type.

use crate::mvl::ir::{
    BinaryOp, Pattern, TirExpr, TirExprKind, TirMatchArm, TirMatchBody, Ty, UnaryOp,
};

use super::{TextEmitter, RESULT_LLVM_TY};

/// Recursively check if a TIR expression's static type is `Float`.
///
/// Mirrors `expr_is_float(&Expr)` on the AST side but uses the embedded
/// `.ty` on each `TirExpr` node — no walk needed past the root in the
/// common case.
fn tir_expr_is_float(expr: &TirExpr) -> bool {
    matches!(expr.ty, Ty::Float)
        || match &expr.kind {
            TirExprKind::Binary { left, .. } => tir_expr_is_float(left),
            _ => false,
        }
}

impl TextEmitter {
    /// Walk a [`TirExpr`] and emit LLVM IR for it, returning the SSA register
    /// holding the value (or `None` if the expression diverges / has no value).
    ///
    /// TIR-walking counterpart of `emit_expr(&Expr)`. Built incrementally —
    /// unimplemented variants return an error so the `cross_backend_tir` test
    /// target surfaces gaps.
    pub(super) fn emit_expr_tir(&mut self, expr: &TirExpr) -> Result<Option<String>, String> {
        match &expr.kind {
            TirExprKind::Literal(lit) => self.emit_literal(lit),

            TirExprKind::Var(name) => {
                // `None` as a bare identifier → Option None constructor.
                if name == "None" {
                    return self.emit_none_constructor();
                }
                // Qualified enum variant: "Shape::Circle" → discriminant i64,
                // or "LinkedList::Nil" (payload enum, unit variant) → { i8, ptr }.
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
                if let Some(loc) = self.fn_ctx.ref_locals.get(name).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.fn_ctx.reg_types.insert(reg.clone(), ty_str);
                    return Ok(Some(reg));
                }
                if let Some(val) = self.fn_ctx.locals.get(name).cloned() {
                    return Ok(Some(val));
                }
                // Bare reference to a top-level function (e.g. `my_handler`
                // passed as an argument) — emit as the function symbol so it
                // can be used as a function pointer.
                if self.module.fn_ret_types.contains_key(name) {
                    return Ok(Some(format!("@{name}")));
                }
                Ok(None)
            }

            TirExprKind::Binary { op, left, right } => self.emit_binary_tir(op, left, right),

            TirExprKind::Unary { op, expr: inner } => self.emit_unary_tir(op, inner),

            TirExprKind::Block(block) => self.emit_block_tir(block),

            TirExprKind::If { cond, then, else_ } => {
                self.emit_if_expr_tir(cond, then, else_.as_deref())
            }

            TirExprKind::FnCall { name, args, .. } => self.emit_fn_call_tir(name, args),

            TirExprKind::Construct { name, fields } => self.emit_construct_tir(name, fields),

            TirExprKind::FieldAccess { expr: inner, field } => {
                self.emit_field_access_tir(inner, field)
            }

            // Set lowers to a List in this backend (sets are arrays at the IR level;
            // dedup is the runtime's responsibility).
            TirExprKind::List { elems } | TirExprKind::Set { elems } => {
                self.emit_list_literal_tir(elems)
            }

            TirExprKind::Map { pairs } => self.emit_map_literal_tir(pairs),

            TirExprKind::Match { scrutinee, arms } => self.emit_match_expr_tir(scrutinee, arms),

            TirExprKind::MethodCall {
                receiver,
                method,
                args,
            } => self.emit_method_call_tir(receiver, method, args),

            TirExprKind::Propagate(inner) => self.emit_propagate_tir(inner),

            TirExprKind::Lambda { params, body } => self.emit_lambda_tir(params, body),

            TirExprKind::Spawn { actor_type, fields } => {
                self.emit_actor_spawn_tir(actor_type, fields)
            }

            TirExprKind::Relabel {
                name,
                expr: inner,
                tag,
                audit,
            } => self.emit_relabel_tir(name, inner, tag, *audit),

            // Select and Quantifier have no LLVM lowering — Select is spec-only
            // for the LLVM backend (concurrent dispatch lives in the runtime),
            // Quantifier is erased. Both yield None like the AST path's catch-all.
            TirExprKind::Select { .. } | TirExprKind::Quantifier(_) => Ok(None),

            // Consume is a move marker — lowers to the inner value unchanged.
            TirExprKind::Consume(inner) => self.emit_expr_tir(inner),

            // Borrow is a capability marker — lowers to the inner value.
            TirExprKind::Borrow { expr: inner, .. } => self.emit_expr_tir(inner),
        }
    }

    /// TIR variant of [`Self::emit_if_expr`].
    fn emit_if_expr_tir(
        &mut self,
        cond: &TirExpr,
        then: &crate::mvl::ir::TirBlock,
        else_: Option<&TirExpr>,
    ) -> Result<Option<String>, String> {
        match else_ {
            Some(e) => match &e.kind {
                TirExprKind::Block(b) => self.emit_if_phi_tir_from_blocks(cond, then, Some(b)),
                TirExprKind::If { .. } => {
                    let cond_val = match self.emit_expr_tir(cond)? {
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
                    let then_val = self.emit_block_tir(then)?;
                    let then_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.push_instr(&format!("br label %{merge_bb}"));
                    }
                    self.start_bb(&else_bb);
                    let else_val = self.emit_expr_tir(e)?;
                    let else_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
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
                            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
                            Ok(Some(result))
                        }
                        _ => Ok(None),
                    }
                }
                _ => self.emit_if_phi_tir_from_blocks(cond, then, None),
            },
            None => self.emit_if_phi_tir_from_blocks(cond, then, None),
        }
    }

    /// TIR variant of [`Self::emit_fn_call`] — minimum-viable port.
    ///
    /// Handles direct user-defined function calls (the most common case).
    /// Builtins (assert/println/format/Ok/Err/Some/None), enum variant
    /// constructors, Box::new, stdlib C-ABI dispatch, generics, fn-alias
    /// indirect calls, and local closure calls fall through to errors and
    /// will be ported in subsequent commits.
    fn emit_fn_call_tir(&mut self, name: &str, args: &[TirExpr]) -> Result<Option<String>, String> {
        use crate::mvl::ir::TypeExpr;

        // Builtins ported so far.
        match name {
            "assert" => return self.emit_assert_builtin_tir(args),
            "println" | "print" | "eprintln" => return self.emit_println_builtin_tir(name, args),
            "Ok" | "Err" => return self.emit_result_constructor_tir(name, args),
            "Some" => return self.emit_option_constructor_tir(args),
            "None" => return self.emit_none_constructor(),
            "format" => return self.emit_format_builtin_tir(args),
            _ => {}
        }

        // Enum variant constructors: "Shape::Circle" or "LinkedList::Cons(...)".
        if name.contains("::") {
            if let Some(disc) = self.pattern_discriminant(name) {
                let (type_name, _variant_name) = Self::split_qualified(name)
                    .ok_or_else(|| format!("malformed qualified name: {name}"))?;
                if self.enum_has_payloads(type_name) {
                    return self.emit_enum_variant_constructor_tir(name, disc, args);
                }
                return Ok(Some(format!("{disc}")));
            }
        }

        // Stdlib C-ABI builtins (mirrors `emit_exprs.rs::emit_fn_call`).
        // Routes generic stdlib calls whose pure-MVL bodies are stripped from
        // the prelude (to avoid SSA dominance bugs) or whose return type isn't
        // registered (opaque types).
        match name {
            "path" if args.len() == 1 => return self.emit_path_builtin_tir(&args[0]),
            "format_datetime" if args.len() == 2 => {
                return self.emit_format_datetime_tir(&args[0], &args[1]);
            }
            "format_instant" if args.len() == 2 => {
                return self.emit_format_instant_tir(&args[0], &args[1]);
            }
            "choice" if args.len() == 1 => return self.emit_choice_call_tir(&args[0]),
            "List::filled" if args.len() == 2 => {
                return self.emit_list_filled_tir(&args[0], &args[1]);
            }
            "float_checked_to_int" if args.len() == 1 => {
                return self.emit_float_checked_to_int_tir(&args[0]);
            }
            // `Box::new`, `find_all`, `replace` still need ports (each is used
            // in 0-1 corpus files). Falls through to the user-fn call path,
            // which will treat them like generic-fn calls and emit either a
            // mangled call or `Ok(None)` depending on monomorphization state.
            _ => {}
        }
        if name.contains("::") && self.pattern_discriminant(name).is_some() {
            return Err(format!(
                "emit_fn_call_tir: enum variant constructor `{name}` not yet ported"
            ));
        }
        // Generic function call — route through the TIR mono path (#1612, Bug 4).
        if self.mono.tir_generic_fns.contains_key(name) {
            return self.emit_monomorphized_call_tir(name, args);
        }

        // Indirect call through a fn-type alias local. Mirror of the AST path
        // at `emit_exprs.rs::emit_fn_call`: a binding `d: Dispatcher` where
        // `Dispatcher = fn(...) -> ...` resolves to a `Base` annotation that
        // `fn_aliases` maps to `Fn`. Emits a `%reg = call <ret> <fn_ptr>(...)`
        // through the loaded pointer, no closure environment. Closures (fn-typed
        // params with a direct `TypeExpr::Fn` annotation) are handled by the
        // arm below.
        let local_ty = self.fn_ctx.local_mvl_types.get(name).cloned();
        let is_alias = matches!(&local_ty, Some(t) if !matches!(t, TypeExpr::Fn { .. }))
            && local_ty
                .as_ref()
                .and_then(|t| self.resolve_fn_alias(t))
                .is_some();
        if is_alias {
            if let Some(fn_ptr) = self.fn_ctx.locals.get(name).cloned() {
                let alias_fn_ty = local_ty.as_ref().and_then(|t| self.resolve_fn_alias(t));
                if let Some(TypeExpr::Fn { ret, .. }) = alias_fn_ty {
                    let mut call_args: Vec<String> = Vec::new();
                    for arg in args {
                        let ty = self.ty_to_llvm_ctx(&arg.ty);
                        if let Some(v) = self.emit_expr_tir(arg)? {
                            call_args.push(format!("{ty} {v}"));
                        }
                    }
                    let args_str = call_args.join(", ");
                    let llvm_ret = self.llvm_ty_ctx(&ret);
                    let is_void = Self::is_void(&ret);
                    if is_void {
                        self.push_instr(&format!("call void {fn_ptr}({args_str})"));
                        return Ok(None);
                    } else {
                        let reg = self.next_reg();
                        self.push_instr(&format!("{reg} = call {llvm_ret} {fn_ptr}({args_str})"));
                        self.fn_ctx.reg_types.insert(reg.clone(), llvm_ret);
                        return Ok(Some(reg));
                    }
                }
            }
        }

        // Local closure call (closure-over-closure / fn-typed parameter).
        // Mirror of AST: load fn_ptr + env_ptr from the `%__closure_type` slot
        // and call indirectly with the env as the first argument.
        if let Some(closure_ptr) = self.fn_ctx.locals.get(name).cloned() {
            if local_ty
                .as_ref()
                .is_some_and(|t| matches!(t, TypeExpr::Fn { .. }))
            {
                self.ensure_closure_type();
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

                let mut call_args = vec![format!("ptr {env_ptr}")];
                for arg in args {
                    let ty = self.ty_to_llvm_ctx(&arg.ty);
                    if let Some(v) = self.emit_expr_tir(arg)? {
                        call_args.push(format!("{ty} {v}"));
                    }
                }
                let args_str = call_args.join(", ");

                let (llvm_ret, is_void) = if let Some(TypeExpr::Fn { ret, .. }) = local_ty.as_ref()
                {
                    (self.llvm_ty_ctx(ret), Self::is_void(ret))
                } else {
                    ("i64".into(), false)
                };

                if is_void {
                    self.push_instr(&format!("call void {fn_ptr}({args_str})"));
                    return Ok(None);
                } else {
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = call {llvm_ret} {fn_ptr}({args_str})"));
                    self.fn_ctx.reg_types.insert(reg.clone(), llvm_ret);
                    return Ok(Some(reg));
                }
            }
        }

        // User-defined function call.
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.ty_to_llvm_ctx(&arg.ty);
            if let Some(v) = self.emit_expr_tir(arg)? {
                arg_vals.push((ty, v));
            }
        }
        let ret_ty = self
            .module
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

        // Builtin C-ABI dispatch (e.g. `mvl_str_split`) — same rewrite rules as
        // the AST path: opaque-handle struct args become `ptr`.
        let (effective_name, is_c_builtin, args_str): (String, bool, String) =
            if let Some(c_sym) = self.module.builtin_syms.get(name).cloned() {
                let c_abi_args: Vec<(String, &str)> = arg_vals
                    .iter()
                    .map(|(ty, v)| {
                        let actual_ty = self.fn_ctx.reg_types.get(v).cloned();
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
            self.fn_ctx.reg_types.insert(reg.clone(), llvm_ret.clone());

            if is_c_builtin && llvm_ret == super::RESULT_LLVM_TY {
                let disc = self.next_reg();
                self.push_instr(&format!(
                    "{disc} = extractvalue {} {reg}, 0",
                    super::RESULT_LLVM_TY
                ));
                self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());
                let raw_payload = self.next_reg();
                self.push_instr(&format!(
                    "{raw_payload} = extractvalue {} {reg}, 1",
                    super::RESULT_LLVM_TY
                ));
                self.fn_ctx
                    .reg_types
                    .insert(raw_payload.clone(), "ptr".into());
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca ptr"));
                self.push_instr(&format!("store ptr {raw_payload}, ptr {slot}"));
                let r1 = self.wrap_result_pair(&disc, &slot);
                return Ok(Some(r1));
            }

            Ok(Some(reg))
        }
    }

    /// TIR variant of [`Self::emit_binary`].
    ///
    /// Uses each operand's embedded `.ty` to drive float/string dispatch
    /// instead of the AST `expr_is_float` walker.
    fn emit_binary_tir(
        &mut self,
        op: &BinaryOp,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        if op.is_short_circuit() {
            return match op {
                BinaryOp::And => self.emit_short_circuit_and_tir(left, right),
                BinaryOp::Or => self.emit_short_circuit_or_tir(left, right),
                _ => unreachable!("is_short_circuit but not And or Or"),
            };
        }

        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let lhs_ty = self.ty_to_llvm_ctx(&left.ty);
        let is_float = lhs_ty == "double" || tir_expr_is_float(left);

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
                self.fn_ctx.reg_types.insert(neg.clone(), "i1".into());
                return Ok(Some(neg));
            }
            self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
            return Ok(Some(reg));
        }

        let instr = Self::binary_instr(op, is_float, &lhs_ty, &lv, &rv);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = {instr}"));

        let result_ty = if op.is_comparison() {
            "i1"
        } else if is_float {
            "double"
        } else {
            "i64"
        };
        self.fn_ctx.reg_types.insert(reg.clone(), result_ty.into());
        Ok(Some(reg))
    }

    fn emit_short_circuit_and_tir(
        &mut self,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("and_rhs");
        let merge_bb = self.next_bb("and_merge");
        let left_end = self.fn_ctx.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{rhs_bb}, label %{merge_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.fn_ctx.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ false, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    fn emit_short_circuit_or_tir(
        &mut self,
        left: &TirExpr,
        right: &TirExpr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr_tir(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("or_rhs");
        let merge_bb = self.next_bb("or_merge");
        let left_end = self.fn_ctx.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{merge_bb}, label %{rhs_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr_tir(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.fn_ctx.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ true, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    /// TIR variant of [`Self::emit_unary`].
    fn emit_unary_tir(&mut self, op: &UnaryOp, expr: &TirExpr) -> Result<Option<String>, String> {
        let val = match self.emit_expr_tir(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let is_float = tir_expr_is_float(expr);
        let reg = self.next_reg();
        match op {
            UnaryOp::Neg if is_float => {
                self.push_instr(&format!("{reg} = fneg double {val}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
            }
            UnaryOp::Neg => {
                self.push_instr(&format!("{reg} = sub i64 0, {val}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Not => {
                self.push_instr(&format!("{reg} = xor i1 {val}, true"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
            }
            UnaryOp::BitNot => {
                self.push_instr(&format!("{reg} = xor i64 {val}, -1"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Deref => {
                // Box[T] deref: load T through the pointer. The inner type is
                // available directly from the TIR `.ty` (no AST walk needed).
                if let Ty::Named(name, args) = &expr.ty {
                    if name == "Box" {
                        if let Some(inner) = args.first() {
                            let load_ty = self.ty_to_llvm_ctx(inner);
                            let loaded = self.next_reg();
                            self.push_instr(&format!("{loaded} = load {load_ty}, ptr {val}"));
                            self.fn_ctx.reg_types.insert(loaded.clone(), load_ty);
                            return Ok(Some(loaded));
                        }
                    }
                }
                return Ok(Some(val));
            }
        }
        Ok(Some(reg))
    }

    /// TIR variant of [`Self::emit_match_expr`].
    ///
    /// Three delegate paths — Option, Result, payload-enum — handled by
    /// dedicated TIR helpers below. Falls through to the generic unit-enum +
    /// wildcard case for everything else.
    pub(super) fn emit_match_expr_tir(
        &mut self,
        scrutinee: &TirExpr,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        let scrut_val = match self.emit_expr_tir(scrutinee)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let has_ok_err = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Ok { .. } | Pattern::Err { .. }));
        if has_ok_err {
            return self.emit_result_match_tir(scrutinee, &scrut_val, arms);
        }

        let has_some_none = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Some { .. } | Pattern::None(_)));
        if has_some_none {
            return self.emit_option_match_tir(scrutinee, &scrut_val, arms);
        }

        if self.scrutinee_payload_enum_tir(scrutinee).is_some() {
            return self.emit_payload_enum_match_tir(&scrut_val, arms);
        }

        // Generic unit-enum / scalar match.
        let scrut_ty = self.ty_to_llvm_ctx(&scrutinee.ty);

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);

        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        let mut switch_arms: Vec<(i64, usize)> = Vec::new();
        let mut wildcard_arm: Option<usize> = None;

        for (idx, arm) in arms.iter().enumerate() {
            if self.collect_or_discriminants_tir(
                &arm.pattern,
                idx,
                &mut switch_arms,
                &mut wildcard_arm,
            ) {
                continue;
            }
            wildcard_arm = Some(idx);
        }

        let use_switch = !switch_arms.is_empty();
        if use_switch {
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
            self.push_instr(&format!("br label %{default_bb}"));
        }

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    self.fn_ctx.locals.insert(name.clone(), scrut_val.clone());
                }
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;

            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
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
                    self.fn_ctx.locals.remove(name);
                }
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;

        if let Some(wild_idx) = wildcard_arm {
            self.push_instr(&format!("br label %{}", arm_bbs[wild_idx]));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// Identify whether `scrutinee` is a payload-enum expression — TIR-side.
    ///
    /// Uses `scrutinee.ty` directly (no AST walk needed) — cleaner than the
    /// AST equivalent which threads through `local_mvl_types` / `fn_ret_types`.
    fn scrutinee_payload_enum_tir(&self, scrutinee: &TirExpr) -> Option<String> {
        let mut cur = &scrutinee.ty;
        loop {
            match cur {
                Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                    cur = inner;
                }
                Ty::Named(name, _) => {
                    if self.module.enum_variants.contains_key(name) && self.enum_has_payloads(name)
                    {
                        return Some(name.clone());
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }

    /// Helper: emit a [`TirMatchBody`] and return its value.
    fn emit_match_arm_body_tir(&mut self, body: &TirMatchBody) -> Result<Option<String>, String> {
        match body {
            TirMatchBody::Expr(e) => self.emit_expr_tir(e),
            TirMatchBody::Block(b) => self.emit_block_tir(b),
        }
    }

    /// TIR variant of [`collect_or_discriminants`] — collects discriminant values
    /// from a (possibly Or-flattened) pattern. Returns `true` if all sub-patterns
    /// matched as concrete discriminants; `false` if any sub-pattern was a
    /// wildcard or unsupported variant.
    fn collect_or_discriminants_tir(
        &self,
        pattern: &Pattern,
        arm_idx: usize,
        switch_arms: &mut Vec<(i64, usize)>,
        wildcard_arm: &mut Option<usize>,
    ) -> bool {
        match pattern {
            Pattern::Ident(name, _) if name.contains("::") => {
                if let Some(disc) = self.pattern_discriminant(name) {
                    switch_arms.push((disc, arm_idx));
                    true
                } else {
                    false
                }
            }
            Pattern::Literal(crate::mvl::ir::Literal::Integer(n), _) => {
                switch_arms.push((*n, arm_idx));
                true
            }
            Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                *wildcard_arm = Some(arm_idx);
                true
            }
            Pattern::Or { patterns: alts, .. } => {
                for alt in alts {
                    if !self.collect_or_discriminants_tir(alt, arm_idx, switch_arms, wildcard_arm) {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// TIR variant of [`Self::emit_option_match`].
    fn emit_option_match_tir(
        &mut self,
        scrutinee: &TirExpr,
        scrut_val: &str,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        // Inner type of Option[T] — read directly from scrutinee.ty.
        let inner_ty: Option<Ty> = match unwrap_labels(&scrutinee.ty) {
            Ty::Option(inner) => Some((**inner).clone()),
            _ => None,
        };
        let inner_load_ty = inner_ty
            .as_ref()
            .map(|t| self.ty_to_llvm_ctx(t))
            .unwrap_or_else(|| "ptr".into());

        // Extract discriminant byte from { i8, ptr }.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

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
                _ => {
                    wildcard_arm = Some(idx);
                }
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
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            let mut bound_var: Option<String> = None;
            if let Pattern::Some { inner, .. } = &arm.pattern {
                let pp = self.next_reg();
                self.push_instr(&format!(
                    "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                ));
                let some_val = self.next_reg();
                self.push_instr(&format!("{some_val} = load {inner_load_ty}, ptr {pp}"));
                self.fn_ctx
                    .reg_types
                    .insert(some_val.clone(), inner_load_ty.clone());
                if let Pattern::Ident(var_name, _) = inner.as_ref() {
                    if var_name != "_" {
                        self.fn_ctx
                            .locals
                            .insert(var_name.clone(), some_val.clone());
                        if let Some(ref imty) = inner_ty {
                            if let Some(te) = super::emit_stmts::ty_to_type_expr(imty) {
                                self.fn_ctx.local_mvl_types.insert(var_name.clone(), te);
                            }
                        }
                        bound_var = Some(var_name.clone());
                    }
                }
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }
            if let Some(ref var_name) = bound_var {
                self.fn_ctx.locals.remove(var_name);
                self.fn_ctx.local_mvl_types.remove(var_name);
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                self.fn_ctx
                    .locals
                    .insert(name.clone(), scrut_val.to_string());
                bound_var = Some(name.clone());
            }
            let arm_val = self.emit_match_arm_body_tir(&wild_arm.body)?;
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }
            if let Some(ref var_name) = bound_var {
                self.fn_ctx.locals.remove(var_name);
                self.fn_ctx.local_mvl_types.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
            self.fn_ctx.current_bb = merge_bb.clone();
            self.fn_ctx.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
            return Ok(None);
        }
        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;
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
            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// TIR variant of [`Self::emit_result_match`].
    fn emit_result_match_tir(
        &mut self,
        scrutinee: &TirExpr,
        scrut_val: &str,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        let (ok_load_ty, err_load_ty) = match unwrap_labels(&scrutinee.ty) {
            Ty::Result(ok, err) => (self.ty_to_llvm_ctx(ok), self.ty_to_llvm_ctx(err)),
            _ => ("i64".to_string(), "ptr".to_string()),
        };

        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Ok { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Err { .. } => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            let mut bound_var: Option<String> = None;

            match &arm.pattern {
                Pattern::Ok { inner, .. } if ok_load_ty != "void" => {
                    let pp = self.next_reg();
                    self.push_instr(&format!(
                        "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    let ok_val = self.next_reg();
                    self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {pp}"));
                    self.fn_ctx
                        .reg_types
                        .insert(ok_val.clone(), ok_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.fn_ctx.locals.insert(var_name.clone(), ok_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Ok { .. } => {}
                Pattern::Err { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!(
                        "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    let err_val = self.next_reg();
                    self.push_instr(&format!("{err_val} = load {err_load_ty}, ptr {pp}"));
                    self.fn_ctx
                        .reg_types
                        .insert(err_val.clone(), err_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.fn_ctx.locals.insert(var_name.clone(), err_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    if let Pattern::Ident(name, _) = &arm.pattern {
                        self.fn_ctx
                            .locals
                            .insert(name.clone(), scrut_val.to_string());
                        bound_var = Some(name.clone());
                    }
                }
                _ => {}
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Some(var_name) = bound_var {
                self.fn_ctx.locals.remove(&var_name);
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let arm_bb = &arm_bbs[wild_idx];
            self.push_instr(&format!("br label %{arm_bb}"));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// TIR variant of [`Self::emit_payload_enum_match`].
    fn emit_payload_enum_match_tir(
        &mut self,
        scrut_val: &str,
        arms: &[TirMatchArm],
    ) -> Result<Option<String>, String> {
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

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
            self.fn_ctx.fn_buf.push(format!("{arm_bb}:"));
            self.fn_ctx.current_bb = arm_bb.clone();
            self.fn_ctx.terminated = false;

            let mut bound_vars: Vec<String> = Vec::new();

            if let Pattern::TupleStruct { name, fields, .. } = &arm.pattern {
                let field_tys: Vec<crate::mvl::ir::TypeExpr> = self
                    .variant_payload_types(name)
                    .map(|s| s.to_vec())
                    .unwrap_or_default();
                if !fields.is_empty() && !field_tys.is_empty() {
                    let payload_ptr = self.next_reg();
                    self.push_instr(&format!(
                        "{payload_ptr} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    self.fn_ctx
                        .reg_types
                        .insert(payload_ptr.clone(), "ptr".into());
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
                        self.fn_ctx.reg_types.insert(val.clone(), field_llvm);
                        if let Pattern::Ident(var_name, _) = inner_pat {
                            if var_name != "_" {
                                self.fn_ctx.locals.insert(var_name.clone(), val.clone());
                                self.fn_ctx
                                    .local_mvl_types
                                    .insert(var_name.clone(), field_ty_expr.clone());
                                bound_vars.push(var_name.clone());
                            }
                        }
                    }
                }
            }

            let arm_val = self.emit_match_arm_body_tir(&arm.body)?;
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }
            for var_name in &bound_vars {
                self.fn_ctx.locals.remove(var_name);
                self.fn_ctx.local_mvl_types.remove(var_name);
            }
        }

        self.fn_ctx.fn_buf.push(format!("{default_bb}:"));
        self.fn_ctx.current_bb = default_bb.clone();
        self.fn_ctx.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                if !name.contains("::") {
                    self.fn_ctx
                        .locals
                        .insert(name.clone(), scrut_val.to_string());
                    bound_var = Some(name.clone());
                }
            }
            let arm_val = self.emit_match_arm_body_tir(&wild_arm.body)?;
            let end_bb = self.fn_ctx.current_bb.clone();
            if !self.fn_ctx.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }
            if let Some(ref var_name) = bound_var {
                self.fn_ctx.locals.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
        }

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
            self.fn_ctx.current_bb = merge_bb.clone();
            self.fn_ctx.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.fn_ctx.terminated = true;
            return Ok(None);
        }
        self.fn_ctx.fn_buf.push(format!("{merge_bb}:"));
        self.fn_ctx.current_bb = merge_bb.clone();
        self.fn_ctx.terminated = false;
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
            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// TIR variant of [`Self::emit_list_literal`].
    fn emit_list_literal_tir(&mut self, elems: &[TirExpr]) -> Result<Option<String>, String> {
        let elem_ty = elems
            .first()
            .map(|e| self.ty_to_llvm_ctx(&e.ty))
            .unwrap_or_else(|| "ptr".into());

        let mut elem_vals: Vec<String> = Vec::new();
        for e in elems {
            if let Some(v) = self.emit_expr_tir(e)? {
                elem_vals.push(v);
            }
        }

        let n = elem_vals.len().max(4) as i64;
        self.ensure_extern("declare ptr @_mvl_array_new(i64, i64)");
        self.ensure_extern("declare void @_mvl_array_push(ptr, ptr)");

        let arr = self.next_reg();
        let elem_size = Self::llvm_type_size(&elem_ty);
        self.push_instr(&format!(
            "{arr} = call ptr @_mvl_array_new(i64 {elem_size}, i64 {n})"
        ));
        self.fn_ctx.reg_types.insert(arr.clone(), "ptr".into());

        for v in &elem_vals {
            let slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca {elem_ty}"));
            self.push_instr(&format!("store {elem_ty} {v}, ptr {slot}"));
            self.push_instr(&format!(
                "call void @_mvl_array_push(ptr {arr}, ptr {slot})"
            ));
        }

        Ok(Some(arr))
    }

    /// TIR variant of [`Self::emit_map_literal`].
    fn emit_map_literal_tir(
        &mut self,
        pairs: &[(TirExpr, TirExpr)],
    ) -> Result<Option<String>, String> {
        let n = pairs.len().max(4) as i64;
        self.ensure_extern("declare ptr @_mvl_map_new(i64)");
        self.ensure_extern("declare void @_mvl_map_insert(ptr, ptr, i64, ptr, i64)");
        self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
        self.ensure_extern("declare i64 @_mvl_str_len(ptr)");

        let map = self.next_reg();
        self.push_instr(&format!("{map} = call ptr @_mvl_map_new(i64 {n})"));
        self.fn_ctx.reg_types.insert(map.clone(), "ptr".into());

        for (key_expr, val_expr) in pairs {
            let key_val = match self.emit_expr_tir(key_expr)? {
                Some(v) => v,
                None => continue,
            };
            let key_ptr = self.next_reg();
            self.push_instr(&format!(
                "{key_ptr} = call ptr @_mvl_string_ptr(ptr {key_val})"
            ));
            let key_len = self.next_reg();
            self.push_instr(&format!(
                "{key_len} = call i64 @_mvl_str_len(ptr {key_val})"
            ));

            let val_val = match self.emit_expr_tir(val_expr)? {
                Some(v) => v,
                None => continue,
            };
            let val_ty = self.infer_val_type(&val_val);
            let val_slot = self.next_reg();
            self.push_instr(&format!("{val_slot} = alloca {val_ty}"));
            self.push_instr(&format!("store {val_ty} {val_val}, ptr {val_slot}"));

            self.push_instr(&format!(
                "call void @_mvl_map_insert(ptr {map}, ptr {key_ptr}, i64 {key_len}, ptr {val_slot}, i64 8)"
            ));
        }

        Ok(Some(map))
    }

    /// TIR variant of [`Self::emit_result_constructor`] (Ok/Err).
    fn emit_result_constructor_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let disc: i64 = if name == "Ok" { 0 } else { 1 };
        let slot;
        if let Some(arg) = args.first() {
            let inferred_ty = self.ty_to_llvm_ctx(&arg.ty);
            if inferred_ty == "void" {
                let _ = self.emit_expr_tir(arg)?;
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca i8"));
            } else {
                let arg_val = match self.emit_expr_tir(arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {inferred_ty}"));
                self.push_instr(&format!("store {inferred_ty} {arg_val}, ptr {slot}"));
            }
        } else {
            slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca i8"));
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &slot);
        Ok(Some(r1))
    }

    /// TIR variant of [`Self::emit_option_constructor`] (Some).
    fn emit_option_constructor_tir(&mut self, args: &[TirExpr]) -> Result<Option<String>, String> {
        let arg = match args.first() {
            Some(a) => a,
            None => return self.emit_none_constructor(),
        };
        let arg_ty = self.ty_to_llvm_ctx(&arg.ty);
        let arg_val = match self.emit_expr_tir(arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca {arg_ty}"));
        self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
        let r1 = self.wrap_result_pair("0", &slot);
        Ok(Some(r1))
    }

    /// TIR variant of [`Self::emit_enum_variant_constructor`] (payload enums).
    fn emit_enum_variant_constructor_tir(
        &mut self,
        qualified_name: &str,
        disc: i64,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let field_tys: Vec<crate::mvl::ir::TypeExpr> = self
            .variant_payload_types(qualified_name)
            .map(|s| s.to_vec())
            .unwrap_or_default();

        let payload_ptr: String = if field_tys.is_empty() {
            "null".to_string()
        } else {
            if args.len() != field_tys.len() {
                return Err(format!(
                    "variant {qualified_name}: expected {} fields, got {}",
                    field_tys.len(),
                    args.len()
                ));
            }
            let n = field_tys.len();
            let base = self.next_reg();
            self.push_instr(&format!("{base} = alloca [{n} x i64]"));
            for (i, (ty_expr, arg)) in field_tys.iter().zip(args.iter()).enumerate() {
                let field_llvm = self.llvm_ty_ctx(ty_expr);
                let arg_val = match self.emit_expr_tir(arg)? {
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

    /// TIR variant of [`Self::emit_assert_builtin`].
    fn emit_assert_builtin_tir(&mut self, args: &[TirExpr]) -> Result<Option<String>, String> {
        let cond = match args.first() {
            Some(a) => a,
            None => return Ok(None),
        };
        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let ok_bb = self.next_bb("assert_ok");
        let fail_bb = self.next_bb("assert_fail");
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{ok_bb}, label %{fail_bb}"
        ));
        self.fn_ctx.fn_buf.push(format!("{fail_bb}:"));
        self.fn_ctx.current_bb = fail_bb.clone();
        self.fn_ctx.terminated = false;
        self.ensure_extern("declare void @llvm.trap()");
        self.push_instr("call void @llvm.trap()");
        self.push_instr("unreachable");
        self.fn_ctx.terminated = true;
        self.fn_ctx.fn_buf.push(format!("{ok_bb}:"));
        self.fn_ctx.current_bb = ok_bb;
        self.fn_ctx.terminated = false;
        Ok(None)
    }

    /// TIR variant of [`Self::emit_println_builtin`].
    fn emit_println_builtin_tir(
        &mut self,
        name: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let fd = if name == "eprintln" { 2i32 } else { 1i32 };
        if args.is_empty() {
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
        let val = match self.emit_expr_tir(&args[0])? {
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

    /// TIR variant of [`Self::emit_format_builtin`].
    fn emit_format_builtin_tir(&mut self, args: &[TirExpr]) -> Result<Option<String>, String> {
        if args.len() < 2 {
            return Ok(None);
        }
        let template = match self.emit_expr_tir(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let list = match self.emit_expr_tir(&args[1])? {
            Some(v) => v,
            None => return Ok(None),
        };
        self.ensure_extern("declare ptr @_mvl_format(ptr, ptr)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @_mvl_format(ptr {template}, ptr {list})"
        ));
        self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
        Ok(Some(reg))
    }

    /// TIR variant of [`Self::emit_fn_call`] `"path"` arm.
    fn emit_path_builtin_tir(&mut self, arg: &TirExpr) -> Result<Option<String>, String> {
        let s = match self.emit_expr_tir(arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        self.ensure_extern("declare ptr @_mvl_io_path(ptr)");
        let r = self.next_reg();
        self.push_instr(&format!("{r} = call ptr @_mvl_io_path(ptr {s})"));
        self.fn_ctx.reg_types.insert(r.clone(), "ptr".into());
        Ok(Some(r))
    }

    /// TIR variant of [`Self::emit_fn_call`] `"format_datetime"` arm.
    fn emit_format_datetime_tir(
        &mut self,
        dt_arg: &TirExpr,
        pattern_arg: &TirExpr,
    ) -> Result<Option<String>, String> {
        let dt = match self.emit_expr_tir(dt_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let pattern = match self.emit_expr_tir(pattern_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let mut fields = Vec::new();
        for i in 0..6usize {
            let r = self.next_reg();
            self.push_instr(&format!("{r} = extractvalue %DateTime {dt}, {i}"));
            self.fn_ctx.reg_types.insert(r.clone(), "i64".into());
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
        self.fn_ctx.reg_types.insert(r.clone(), "ptr".into());
        Ok(Some(r))
    }

    /// TIR variant of [`Self::emit_fn_call`] `"format_instant"` arm.
    fn emit_format_instant_tir(
        &mut self,
        handle_arg: &TirExpr,
        pattern_arg: &TirExpr,
    ) -> Result<Option<String>, String> {
        let handle = match self.emit_expr_tir(handle_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let pattern = match self.emit_expr_tir(pattern_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        self.ensure_extern("declare ptr @_mvl_time_format_instant(ptr, ptr)");
        let r = self.next_reg();
        self.push_instr(&format!(
            "{r} = call ptr @_mvl_time_format_instant(ptr {handle}, ptr {pattern})"
        ));
        self.fn_ctx.reg_types.insert(r.clone(), "ptr".into());
        Ok(Some(r))
    }

    /// TIR variant of [`Self::emit_choice_call`].
    fn emit_choice_call_tir(&mut self, list_arg: &TirExpr) -> Result<Option<String>, String> {
        // Element LLVM type comes from the list's resolved element Ty.
        let elem_llvm_ty = match unwrap_labels(&list_arg.ty) {
            Ty::List(inner) | Ty::Array(inner, _) | Ty::Set(inner) => self.ty_to_llvm_ctx(inner),
            _ => "i64".to_string(),
        };
        let arr = match self.emit_expr_tir(list_arg)? {
            Some(v) => v,
            None => return Ok(None),
        };

        self.ensure_extern("declare i64 @_mvl_random_choice_index(ptr)");
        let idx = self.next_reg();
        self.push_instr(&format!(
            "{idx} = call i64 @_mvl_random_choice_index(ptr {arr})"
        ));
        self.fn_ctx.reg_types.insert(idx.clone(), "i64".into());

        let is_none = self.next_reg();
        self.push_instr(&format!("{is_none} = icmp eq i64 {idx}, -1"));
        self.fn_ctx.reg_types.insert(is_none.clone(), "i1".into());

        let none_bb = self.next_bb("choice_none");
        let some_bb = self.next_bb("choice_some");
        let merge_bb = self.next_bb("choice_merge");

        let result_slot = self.next_reg();
        self.push_instr(&format!("{result_slot} = alloca {RESULT_LLVM_TY}"));
        self.fn_ctx
            .reg_types
            .insert(result_slot.clone(), "ptr".into());

        self.push_instr(&format!(
            "br i1 {is_none}, label %{none_bb}, label %{some_bb}"
        ));

        // None branch
        self.start_bb(&none_bb);
        let none_r0 = self.next_reg();
        self.push_instr(&format!(
            "{none_r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 1, 0"
        ));
        self.fn_ctx
            .reg_types
            .insert(none_r0.clone(), RESULT_LLVM_TY.into());
        let none_r1 = self.next_reg();
        self.push_instr(&format!(
            "{none_r1} = insertvalue {RESULT_LLVM_TY} {none_r0}, ptr null, 1"
        ));
        self.fn_ctx
            .reg_types
            .insert(none_r1.clone(), RESULT_LLVM_TY.into());
        self.push_instr(&format!(
            "store {RESULT_LLVM_TY} {none_r1}, ptr {result_slot}"
        ));
        self.push_instr(&format!("br label %{merge_bb}"));
        self.fn_ctx.terminated = true;

        // Some branch
        self.start_bb(&some_bb);
        self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");
        let elem_ptr = self.next_reg();
        self.push_instr(&format!(
            "{elem_ptr} = call ptr @_mvl_array_get(ptr {arr}, i64 {idx})"
        ));
        self.fn_ctx.reg_types.insert(elem_ptr.clone(), "ptr".into());
        let elem_val = self.next_reg();
        self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
        self.fn_ctx
            .reg_types
            .insert(elem_val.clone(), elem_llvm_ty.clone());
        let elem_slot = self.next_reg();
        self.push_instr(&format!("{elem_slot} = alloca {elem_llvm_ty}"));
        self.push_instr(&format!("store {elem_llvm_ty} {elem_val}, ptr {elem_slot}"));
        let some_r0 = self.next_reg();
        self.push_instr(&format!(
            "{some_r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 0, 0"
        ));
        self.fn_ctx
            .reg_types
            .insert(some_r0.clone(), RESULT_LLVM_TY.into());
        let some_r1 = self.next_reg();
        self.push_instr(&format!(
            "{some_r1} = insertvalue {RESULT_LLVM_TY} {some_r0}, ptr {elem_slot}, 1"
        ));
        self.fn_ctx
            .reg_types
            .insert(some_r1.clone(), RESULT_LLVM_TY.into());
        self.push_instr(&format!(
            "store {RESULT_LLVM_TY} {some_r1}, ptr {result_slot}"
        ));
        self.push_instr(&format!("br label %{merge_bb}"));
        self.fn_ctx.terminated = true;

        // Merge
        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = load {RESULT_LLVM_TY}, ptr {result_slot}"
        ));
        self.fn_ctx
            .reg_types
            .insert(result.clone(), RESULT_LLVM_TY.into());
        Ok(Some(result))
    }

    /// TIR variant of [`Self::emit_list_filled`].
    fn emit_list_filled_tir(
        &mut self,
        n_expr: &TirExpr,
        val_expr: &TirExpr,
    ) -> Result<Option<String>, String> {
        let elem_ty = self.ty_to_llvm_ctx(&val_expr.ty);
        let n_val = match self.emit_expr_tir(n_expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let val = match self.emit_expr_tir(val_expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let item_slot = self.next_reg();
        self.push_instr(&format!("{item_slot} = alloca {elem_ty}"));
        self.push_instr(&format!("store {elem_ty} {val}, ptr {item_slot}"));
        let arr = self.next_reg();
        let elem_size = Self::llvm_type_size(&elem_ty);
        self.ensure_extern("declare ptr @_mvl_array_filled(i64, i64, ptr)");
        self.push_instr(&format!(
            "{arr} = call ptr @_mvl_array_filled(i64 {elem_size}, i64 {n_val}, ptr {item_slot})"
        ));
        self.fn_ctx.reg_types.insert(arr.clone(), "ptr".into());
        Ok(Some(arr))
    }

    /// TIR variant of [`Self::emit_fn_call`] `"float_checked_to_int"` arm.
    fn emit_float_checked_to_int_tir(&mut self, arg: &TirExpr) -> Result<Option<String>, String> {
        let v = match self.emit_expr_tir(arg)? {
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
        let val = self.next_reg();
        self.push_instr(&format!("{val} = load i64, ptr {out}"));
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca i64"));
        self.push_instr(&format!("store i64 {val}, ptr {slot}"));
        let r = self.wrap_result_pair(&tag, &slot);
        Ok(Some(r))
    }

    /// TIR variant of [`Self::emit_construct`].
    fn emit_construct_tir(
        &mut self,
        name: &str,
        fields: &[(String, TirExpr)],
    ) -> Result<Option<String>, String> {
        // Named-field enum variant construction: `Shape::Circle { radius: 2.0 }`
        // (#1357). Mirror of `emit_construct` — reorder the named fields into
        // declaration order, then delegate to `emit_enum_variant_constructor_tir`
        // which handles the discriminant + payload-slot allocation.
        if let Some((type_name, _)) = name.split_once("::") {
            if self.module.enum_variants.contains_key(type_name) {
                let disc = self.pattern_discriminant(name).unwrap_or(0);
                let ordered_names = self
                    .module
                    .enum_struct_variant_field_names
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                let args: Vec<TirExpr> = if ordered_names.is_empty() {
                    fields.iter().map(|(_, e)| e.clone()).collect()
                } else {
                    ordered_names
                        .iter()
                        .map(|fname| {
                            fields
                                .iter()
                                .find(|(n, _)| n == fname)
                                .map(|(_, e)| e.clone())
                                .unwrap_or_else(|| TirExpr {
                                    kind: TirExprKind::Var("undef".to_string()),
                                    ty: crate::mvl::checker::types::Ty::Unknown,
                                    span: crate::mvl::parser::lexer::Span::default(),
                                })
                        })
                        .collect()
                };
                return self.emit_enum_variant_constructor_tir(name, disc, &args);
            }
        }

        let field_defs = match self.module.struct_fields.get(name).cloned() {
            Some(f) => f,
            None => return Ok(None),
        };

        let mut field_vals: Vec<(String, String)> = Vec::new();
        for (field_name, field_ty) in &field_defs {
            let llvm_t = self.llvm_ty_ctx(field_ty);
            let val = match fields.iter().find(|(n, _)| n == field_name) {
                Some((_, e)) => self.emit_expr_tir(e)?.unwrap_or_else(|| "undef".into()),
                None => "undef".into(),
            };
            field_vals.push((llvm_t, val));
        }

        let struct_ty = format!("%{name}");
        let mut acc = "undef".to_string();
        for (i, (field_ty, val)) in field_vals.iter().enumerate() {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = insertvalue {struct_ty} {acc}, {field_ty} {val}, {i}"
            ));
            self.fn_ctx.reg_types.insert(reg.clone(), struct_ty.clone());
            acc = reg;
        }
        Ok(Some(acc))
    }

    /// TIR variant of [`Self::emit_field_access`].
    ///
    /// Uses the receiver's `expr.ty` directly to find the struct name —
    /// no AST `struct_name_of_expr` walk needed.
    fn emit_field_access_tir(
        &mut self,
        expr: &TirExpr,
        field: &str,
    ) -> Result<Option<String>, String> {
        // In actor method bodies, `self.field` maps to a ref_local GEP pointer.
        if let TirExprKind::Var(name) = &expr.kind {
            if name == "self" {
                if let Some(loc) = self.fn_ctx.ref_locals.get(field).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.fn_ctx.reg_types.insert(reg.clone(), ty_str);
                    return Ok(Some(reg));
                }
            }
        }

        // Determine the struct name from the receiver's resolved type.
        let struct_name = match &expr.ty {
            Ty::Named(n, _) => Some(n.clone()),
            Ty::Ref(_, inner) => match inner.as_ref() {
                Ty::Named(n, _) => Some(n.clone()),
                _ => None,
            },
            _ => None,
        };

        let base_val = match self.emit_expr_tir(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };

        if let Some(sn) = struct_name {
            if let Some(fields) = self.module.struct_fields.get(&sn).cloned() {
                if let Some(idx) = fields.iter().position(|(f, _)| f == field) {
                    let field_ty = self.llvm_ty_ctx(&fields[idx].1.clone());
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = extractvalue %{sn} {base_val}, {idx}"));
                    self.fn_ctx.reg_types.insert(reg.clone(), field_ty);
                    return Ok(Some(reg));
                }
            }
        }
        Ok(None)
    }
}

impl TextEmitter {
    /// TIR variant of [`Self::emit_relabel`].
    ///
    /// IFC relabel transitions lower to the inner expression unchanged
    /// (the LLVM type system strips label wrappers via ty_to_llvm), with
    /// an optional `_mvl_audit_emit_relabel` audit-event call when the
    /// expression carries `audit` or the declaration is `audit`-marked.
    pub(super) fn emit_relabel_tir(
        &mut self,
        name: &str,
        expr: &TirExpr,
        tag: &str,
        audit: bool,
    ) -> Result<Option<String>, String> {
        let inner = self.emit_expr_tir(expr)?;
        let needs_audit = audit || self.module.audit_relabels.contains_key(name);
        if needs_audit {
            let (from_lbl, to_lbl) = self.relabel_label_strings_tir(name);
            let r_name = self.emit_string_literal(name);
            let r_from = self.emit_string_literal(&from_lbl);
            let r_to = self.emit_string_literal(&to_lbl);
            let r_tag = self.emit_string_literal(tag);
            let r_loc = self.emit_string_literal("");
            self.ensure_extern("declare void @_mvl_audit_emit_relabel(ptr, ptr, ptr, ptr, ptr)");
            self.push_instr(&format!(
                "call void @_mvl_audit_emit_relabel(ptr {r_name}, ptr {r_from}, ptr {r_to}, ptr {r_tag}, ptr {r_loc})"
            ));
        }
        Ok(inner)
    }

    /// (from_label, to_label) display strings for a relabel transition.
    /// Mirrors `relabel_label_strings` in `emit_exprs.rs`.
    fn relabel_label_strings_tir(&self, name: &str) -> (String, String) {
        if let Some((from, to)) = self.module.audit_relabels.get(name) {
            let f = from.as_deref().unwrap_or("_").to_string();
            let t = to.as_deref().unwrap_or("_").to_string();
            return (f, t);
        }
        let (f, t) = match name {
            "classify" => ("_", "Secret"),
            "taint" => ("_", "Tainted"),
            "trust" => ("Tainted", "_"),
            "release" => ("Secret", "_"),
            "config_path" => ("_", "ConfigPath"),
            "unconfig_path" => ("ConfigPath", "_"),
            "db_url" => ("_", "DbUrl"),
            "undb_url" => ("DbUrl", "_"),
            "api_endpoint" => ("_", "ApiEndpoint"),
            "unapi_endpoint" => ("ApiEndpoint", "_"),
            "audit_target" => ("_", "AuditTarget"),
            "unaudit_target" => ("AuditTarget", "_"),
            _ => ("_", "_"),
        };
        (f.to_string(), t.to_string())
    }

    /// TIR variant of [`Self::emit_propagate`].
    ///
    /// Cleaner than the AST version: the inner expression's `.ty` directly
    /// carries the Result type, so the Ok-payload LLVM type comes from a
    /// single `Ty::Result(ok, _)` match — no `result_ok_llvm_ty` lookup
    /// through fn_ret_types or per-method special cases.
    pub(super) fn emit_propagate_tir(&mut self, inner: &TirExpr) -> Result<Option<String>, String> {
        let result_val = match self.emit_expr_tir(inner)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = extractvalue {RESULT_LLVM_TY} {result_val}, 0"
        ));
        self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());

        let is_ok = self.next_reg();
        self.push_instr(&format!("{is_ok} = icmp eq i8 {disc}, 0"));
        self.fn_ctx.reg_types.insert(is_ok.clone(), "i1".into());

        let ok_bb = self.next_bb("prop_ok");
        let err_bb = self.next_bb("prop_err");
        self.push_instr(&format!("br i1 {is_ok}, label %{ok_bb}, label %{err_bb}"));

        // Err path: propagate the Result upwards.
        self.start_bb(&err_bb);
        self.emit_heap_drops();
        let ret_ty = self.fn_ctx.current_ret_ty.clone();
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        self.push_instr(&format!("ret {llvm_ret} {result_val}"));
        self.fn_ctx.terminated = true;

        // Ok path: extract and load the success payload.
        self.start_bb(&ok_bb);
        let ok_load_ty = match unwrap_labels(&inner.ty) {
            Ty::Result(ok, _) => self.ty_to_llvm_ctx(ok),
            _ => {
                // Fallback: when the checker didn't resolve `inner.ty` to
                // `Ty::Result` (common for stdlib FnCalls whose return type
                // lives in `module.fn_ret_types` rather than the
                // checker-derived `expr_types` map), look up the FnCall's
                // declared return type and use its `Ok` payload's static
                // LLVM type — same fallback the AST `result_ok_llvm_ty`
                // helper uses.
                if let TirExprKind::FnCall { name, .. } = &inner.kind {
                    if let Some(crate::mvl::ir::TypeExpr::Result { ok, .. }) =
                        self.module.fn_ret_types.get(name)
                    {
                        Self::llvm_ty(ok)
                    } else {
                        "i64".to_string()
                    }
                } else {
                    "i64".to_string()
                }
            }
        };
        if ok_load_ty == "void" {
            return Ok(None);
        }
        let payload_ptr = self.next_reg();
        self.push_instr(&format!(
            "{payload_ptr} = extractvalue {RESULT_LLVM_TY} {result_val}, 1"
        ));
        let ok_val = self.next_reg();
        self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {payload_ptr}"));
        self.fn_ctx.reg_types.insert(ok_val.clone(), ok_load_ty);
        Ok(Some(ok_val))
    }

    /// TIR variant of [`Self::emit_method_call`].
    ///
    /// Mirrors the AST dispatcher: actor send fast-path, then receiver type
    /// + value evaluation, then a (method, recv_ty) dispatch table.
    ///
    /// Built incrementally — only the most common method cases are ported in
    /// the initial commit (to_string variants, String runtime kernels, Map
    /// basics, Option is_some/is_none/unwrap_or, simple Int/Float/Bool
    /// numerics). Unimplemented cases return an error to keep the
    /// cross_backend_tir lenient parity helper honest.
    pub(super) fn emit_method_call_tir(
        &mut self,
        receiver: &TirExpr,
        method: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        // Actor method call — fire-and-forget send.
        if let Some(actor_name) = self.resolve_actor_type_name_tir(receiver) {
            let handle_val = match self.emit_expr_tir(receiver)? {
                Some(v) => v,
                None => return Ok(None),
            };
            return self.emit_actor_method_call_tir(&handle_val, &actor_name.clone(), method, args);
        }

        let recv_ty = self.ty_to_llvm_ctx(&receiver.ty);
        let val = match self.emit_expr_tir(receiver)? {
            Some(v) => v,
            None => return Ok(None),
        };

        match (method, recv_ty.as_str()) {
            // ── to_string family ──────────────────────────────────────────
            ("to_string", "i64") | ("to_string", "i1") => {
                let s = if recv_ty == "i64" {
                    self.emit_int_to_string(&val)
                } else {
                    self.emit_bool_to_string(&val)
                };
                Ok(Some(s))
            }
            ("to_string", "double") => {
                self.ensure_extern("declare ptr @_mvl_float_to_string(double)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_float_to_string(double {val})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("to_string", "i8") => {
                let widened = self.next_reg();
                self.push_instr(&format!("{widened} = zext i8 {val} to i64"));
                self.fn_ctx.reg_types.insert(widened.clone(), "i64".into());
                Ok(Some(self.emit_int_to_string(&widened)))
            }
            ("to_string", _) => {
                self.fn_ctx.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }

            // ── String runtime kernels (#1186) ────────────────────────────
            ("chars", "ptr") => Ok(Some(self.emit_c_call_simple("chars", &val, &[]))),
            ("trim", "ptr") => Ok(Some(self.emit_c_call_simple("trim", &val, &[]))),
            ("to_lower", "ptr") => Ok(Some(self.emit_c_call_simple("to_lower", &val, &[]))),
            ("to_upper", "ptr") => Ok(Some(self.emit_c_call_simple("to_upper", &val, &[]))),
            ("find", "ptr") if args.len() == 1 => {
                let needle = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "find",
                    &val,
                    &[("ptr", &needle)],
                )))
            }
            ("contains", "ptr") if args.len() == 1 => {
                let needle = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_bool_from_i64(
                    "contains",
                    &val,
                    &[("ptr", &needle)],
                )))
            }
            ("starts_with", "ptr") if args.len() == 1 => {
                let needle = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_bool_from_i64(
                    "starts_with",
                    &val,
                    &[("ptr", &needle)],
                )))
            }
            ("ends_with", "ptr") if args.len() == 1 => {
                let needle = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_bool_from_i64(
                    "ends_with",
                    &val,
                    &[("ptr", &needle)],
                )))
            }
            ("split", "ptr") if args.len() == 1 => {
                let delim = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "split",
                    &val,
                    &[("ptr", &delim)],
                )))
            }
            ("substring", "ptr") if args.len() == 2 => {
                let start = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let end = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "substring",
                    &val,
                    &[("i64", &start), ("i64", &end)],
                )))
            }
            ("byte_at", "ptr") if args.len() == 1 => {
                let idx = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_option_out_ptr(
                    "byte_at",
                    &val,
                    &[("i64", &idx)],
                )))
            }
            ("replace", "ptr") if args.len() == 2 => {
                let old = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let new = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "replace",
                    &val,
                    &[("ptr", &old), ("ptr", &new)],
                )))
            }

            // ── Length-style methods (String/List/Map/Set lower to ptr) ───
            ("len", "ptr") => {
                // Distinguish by receiver's TIR type — String uses _mvl_str_len,
                // List/Array/Set use _mvl_array_len, Map uses _mvl_map_len.
                match unwrap_labels(&receiver.ty) {
                    Ty::String => {
                        self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                        let reg = self.next_reg();
                        self.push_instr(&format!("{reg} = call i64 @_mvl_str_len(ptr {val})"));
                        self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                        Ok(Some(reg))
                    }
                    Ty::List(_) | Ty::Array(_, _) | Ty::Set(_) => {
                        self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                        let reg = self.next_reg();
                        self.push_instr(&format!("{reg} = call i64 @_mvl_array_len(ptr {val})"));
                        self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                        Ok(Some(reg))
                    }
                    Ty::Map(_, _) => {
                        self.ensure_extern("declare i64 @_mvl_map_len(ptr)");
                        let reg = self.next_reg();
                        self.push_instr(&format!("{reg} = call i64 @_mvl_map_len(ptr {val})"));
                        self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                        Ok(Some(reg))
                    }
                    _ => Err(format!(
                        "emit_method_call_tir: len() on unsupported type {:?}",
                        receiver.ty
                    )),
                }
            }

            // ── Map methods ───────────────────────────────────────────────
            ("insert", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                if args.len() < 2 {
                    return Ok(None);
                }
                let key_arg = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let val_arg = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @_mvl_map_insert(ptr, ptr, i64, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @_mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let val_ty = self.infer_val_type(&val_arg);
                let vs = self.next_reg();
                self.push_instr(&format!("{vs} = alloca {val_ty}"));
                self.push_instr(&format!("store {val_ty} {val_arg}, ptr {vs}"));
                self.push_instr(&format!(
                    "call void @_mvl_map_insert(ptr {val}, ptr {kp}, i64 {kl}, ptr {vs}, i64 8)"
                ));
                // insert returns the map (modified in place)
                Ok(Some(val))
            }
            ("keys", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                Ok(Some(self.emit_c_call_simple("keys", &val, &[])))
            }
            ("values", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                Ok(Some(self.emit_c_call_simple("values", &val, &[])))
            }
            ("contains_key", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                let key_expr = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let key_ty = self.ty_to_llvm_ctx(&key_expr.ty);
                let key_arg = match self.emit_expr_tir(key_expr)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_map_get(ptr, ptr, i64)");
                let (kp, kl) = if key_ty == "i64" {
                    let slot = self.next_reg();
                    self.push_instr(&format!("{slot} = alloca i64"));
                    self.push_instr(&format!("store i64 {key_arg}, ptr {slot}"));
                    (slot, "8".to_string())
                } else {
                    self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
                    self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                    let kp = self.next_reg();
                    self.push_instr(&format!("{kp} = call ptr @_mvl_string_ptr(ptr {key_arg})"));
                    let kl_reg = self.next_reg();
                    self.push_instr(&format!("{kl_reg} = call i64 @_mvl_str_len(ptr {key_arg})"));
                    (kp, kl_reg)
                };
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @_mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                let result = self.next_reg();
                self.push_instr(&format!("{result} = icmp ne ptr {raw}, null"));
                self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
                Ok(Some(result))
            }
            ("get", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                let key_expr = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let key_ty = self.ty_to_llvm_ctx(&key_expr.ty);
                let key_arg = match self.emit_expr_tir(key_expr)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_map_get(ptr, ptr, i64)");
                let (kp, kl) = if key_ty == "i64" {
                    let slot = self.next_reg();
                    self.push_instr(&format!("{slot} = alloca i64"));
                    self.push_instr(&format!("store i64 {key_arg}, ptr {slot}"));
                    (slot, "8".to_string())
                } else {
                    self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
                    self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                    let kp = self.next_reg();
                    self.push_instr(&format!("{kp} = call ptr @_mvl_string_ptr(ptr {key_arg})"));
                    let kl_reg = self.next_reg();
                    self.push_instr(&format!("{kl_reg} = call i64 @_mvl_str_len(ptr {key_arg})"));
                    (kp, kl_reg)
                };
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @_mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                let is_null = self.next_reg();
                self.push_instr(&format!("{is_null} = icmp eq ptr {raw}, null"));
                let some_bb = self.next_bb("map_get_some");
                let none_bb = self.next_bb("map_get_none");
                let merge_bb = self.next_bb("map_get_merge");
                self.push_instr(&format!(
                    "br i1 {is_null}, label %{none_bb}, label %{some_bb}"
                ));
                self.start_bb(&some_bb);
                let opt_some = self.next_reg();
                self.push_instr(&format!(
                    "{opt_some} = insertvalue {RESULT_LLVM_TY} {{ i8 0, ptr null }}, ptr {raw}, 1"
                ));
                self.push_instr(&format!("br label %{merge_bb}"));
                let some_end = self.fn_ctx.current_bb.clone();
                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                let none_end = self.fn_ctx.current_bb.clone();
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {RESULT_LLVM_TY} [ {opt_some}, %{some_end} ], [ {{ i8 1, ptr null }}, %{none_end} ]"
                ));
                self.fn_ctx
                    .reg_types
                    .insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }
            ("remove", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr_tir(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @_mvl_map_remove(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @_mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                self.push_instr(&format!(
                    "call void @_mvl_map_remove(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                // remove returns the map (modified in place)
                Ok(Some(val))
            }

            // ── Int (i64) numeric methods ─────────────────────────────────
            ("abs", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call i64 @llvm.abs.i64(i64 {val}, i1 0)"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("is_positive", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp sgt i64 {val}, 0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_negative", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp slt i64 {val}, 0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_zero", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp eq i64 {val}, 0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("to_float", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = sitofp i64 {val} to double"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("min", "i64") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @llvm.smin.i64(i64 {val}, i64 {other})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("max", "i64") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @llvm.smax.i64(i64 {val}, i64 {other})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("clamp", "i64") if args.len() == 2 => {
                let lo = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let hi = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let clamped_lo = self.next_reg();
                self.push_instr(&format!(
                    "{clamped_lo} = call i64 @llvm.smax.i64(i64 {val}, i64 {lo})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @llvm.smin.i64(i64 {clamped_lo}, i64 {hi})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("pow", "i64") if args.len() == 1 => {
                let exp = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_int_pow(i64, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_int_pow(i64 {val}, i64 {exp})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // ── Byte (i8) primitive methods (#1615) ──────────────────────
            ("to_int", "i8") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = zext i8 {val} to i64"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("bit_and", "i8")
            | ("bit_or", "i8")
            | ("bit_xor", "i8")
            | ("wrapping_add", "i8")
            | ("wrapping_sub", "i8")
            | ("wrapping_mul", "i8")
                if args.len() == 1 =>
            {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let op = match method {
                    "bit_and" => "and",
                    "bit_or" => "or",
                    "bit_xor" => "xor",
                    "wrapping_add" => "add",
                    "wrapping_sub" => "sub",
                    "wrapping_mul" => "mul",
                    _ => unreachable!(),
                };
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = {op} i8 {val}, {other}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i8".into());
                Ok(Some(reg))
            }
            ("bit_not", "i8") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = xor i8 {val}, -1"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i8".into());
                Ok(Some(reg))
            }
            ("shift_left", "i8") | ("shift_right", "i8") if args.len() == 1 => {
                let amount = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let op = if method == "shift_left" {
                    "shl"
                } else {
                    "lshr"
                };
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = {op} i8 {val}, {amount}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i8".into());
                Ok(Some(reg))
            }

            // ── Float (double) numeric methods ────────────────────────────
            ("abs", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.fabs.f64(double {val})"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("ceil", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.ceil.f64(double {val})"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("floor", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.floor.f64(double {val})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("round", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.round.f64(double {val})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("sqrt", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.sqrt.f64(double {val})"));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("to_int", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fptosi double {val} to i64"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("is_nan", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp uno double {val}, 0.0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_finite", "double") => {
                let abs_reg = self.next_reg();
                self.push_instr(&format!(
                    "{abs_reg} = call double @llvm.fabs.f64(double {val})"
                ));
                let not_nan = self.next_reg();
                self.push_instr(&format!("{not_nan} = fcmp ord double {val}, 0.0"));
                let not_inf = self.next_reg();
                self.push_instr(&format!(
                    "{not_inf} = fcmp olt double {abs_reg}, 0x7FF0000000000000"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = and i1 {not_nan}, {not_inf}"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_infinite", "double") => {
                let abs_reg = self.next_reg();
                self.push_instr(&format!(
                    "{abs_reg} = call double @llvm.fabs.f64(double {val})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = fcmp oeq double {abs_reg}, 0x7FF0000000000000"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_positive", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp ogt double {val}, 0.0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_negative", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp olt double {val}, 0.0"));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("min", "double") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.minnum.f64(double {val}, double {other})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("max", "double") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.maxnum.f64(double {val}, double {other})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("clamp", "double") if args.len() == 2 => {
                let lo = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let hi = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let clamped_lo = self.next_reg();
                self.push_instr(&format!(
                    "{clamped_lo} = call double @llvm.maxnum.f64(double {val}, double {lo})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.minnum.f64(double {clamped_lo}, double {hi})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("pow", "double") if args.len() == 1 => {
                let exp = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.pow.f64(double {val}, double {exp})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }

            // ── Option.unwrap_or(default) → T ─────────────────────────────
            ("unwrap_or", "{ i8, ptr }") if args.len() == 1 => {
                let default_val = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let default_ty = self.ty_to_llvm_ctx(&args[0].ty);

                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {{ i8, ptr }} {val}, 0"));
                let is_some = self.next_reg();
                self.push_instr(&format!("{is_some} = icmp eq i8 {disc}, 0"));

                let some_bb = self.next_bb("unwrap_some");
                let none_bb = self.next_bb("unwrap_none");
                let merge_bb = self.next_bb("unwrap_merge");

                self.push_instr(&format!(
                    "br i1 {is_some}, label %{some_bb}, label %{none_bb}"
                ));

                self.start_bb(&some_bb);
                let payload_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{payload_ptr} = extractvalue {{ i8, ptr }} {val}, 1"
                ));
                let some_val = self.next_reg();
                self.push_instr(&format!(
                    "{some_val} = load {default_ty}, ptr {payload_ptr}"
                ));
                self.push_instr(&format!("br label %{merge_bb}"));
                let some_end = self.fn_ctx.current_bb.clone();

                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                let none_end = self.fn_ctx.current_bb.clone();

                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {default_ty} [ {some_val}, %{some_end} ], [ {default_val}, %{none_end} ]"
                ));
                self.fn_ctx.reg_types.insert(result.clone(), default_ty);
                Ok(Some(result))
            }

            // ── HOF: filter / map / take_while / skip_while / any / all ───
            ("filter" | "map" | "take_while" | "skip_while", "ptr")
                if args.len() == 1 && self.is_closure_arg_tir(&args[0]) =>
            {
                let closure = match self.emit_as_hof_closure_tir(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    method,
                    &val,
                    &[("ptr", &closure)],
                )))
            }
            ("any" | "all", "ptr") if args.len() == 1 && self.is_closure_arg_tir(&args[0]) => {
                let closure = match self.emit_as_hof_closure_tir(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    method,
                    &val,
                    &[("ptr", &closure)],
                )))
            }
            ("group_by", "ptr") if args.len() == 1 && self.is_closure_arg_tir(&args[0]) => {
                let closure = match self.emit_as_hof_closure_tir(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "group_by",
                    &val,
                    &[("ptr", &closure)],
                )))
            }
            ("fold", "ptr") if args.len() == 2 => {
                let init_ty = self.ty_to_llvm_ctx(&args[0].ty);
                let init_val = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // Fold closure: fn(env, acc_val, elem_ptr) -> acc_val
                // param 0 (acc) by-value, param 1 (elem) by-pointer.
                let closure = match self.emit_as_hof_closure_tir(&args[1], &[1])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {init_ty}"));
                self.push_instr(&format!("store {init_ty} {init_val}, ptr {slot}"));
                self.ensure_extern("declare ptr @_mvl_list_fold(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_list_fold(ptr {val}, ptr {slot}, ptr {closure})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {init_ty}, ptr {reg}"));
                self.fn_ctx.reg_types.insert(result.clone(), init_ty);
                Ok(Some(result))
            }

            // ── List/Array/Set non-HOF methods ────────────────────────────
            ("push", "ptr") if args.len() == 1 => {
                let elem = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let elem_ty = self.ty_to_llvm_ctx(&args[0].ty);
                self.ensure_extern("declare void @_mvl_array_push(ptr, ptr)");
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {elem_ty}"));
                self.push_instr(&format!("store {elem_ty} {elem}, ptr {slot}"));
                self.push_instr(&format!(
                    "call void @_mvl_array_push(ptr {val}, ptr {slot})"
                ));
                Ok(Some(val))
            }
            ("get", "ptr")
                if matches!(unwrap_labels(&receiver.ty), Ty::List(_) | Ty::Array(_, _))
                    && args.len() == 1 =>
            {
                let idx_val = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let elem_llvm_ty = match unwrap_labels(&receiver.ty) {
                    Ty::List(e) | Ty::Array(e, _) => self.ty_to_llvm_ctx(e),
                    _ => "i64".to_string(),
                };

                // Bounds check: 0 <= index < len. Mirror of AST emit_method_call's
                // ("get", "ptr") arm — alloca + store + load shape (not the
                // null-check + phi shape) so corpus IR diffs stay byte-equal.
                self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                let len = self.next_reg();
                self.push_instr(&format!("{len} = call i64 @_mvl_array_len(ptr {val})"));
                let in_bounds = self.next_reg();
                self.push_instr(&format!("{in_bounds} = icmp slt i64 {idx_val}, {len}"));
                let non_neg = self.next_reg();
                self.push_instr(&format!("{non_neg} = icmp sge i64 {idx_val}, 0"));
                let ok = self.next_reg();
                self.push_instr(&format!("{ok} = and i1 {in_bounds}, {non_neg}"));

                let some_bb = self.next_bb("list_get_some");
                let none_bb = self.next_bb("list_get_none");
                let merge_bb = self.next_bb("list_get_merge");

                let result_slot = self.next_reg();
                self.push_instr(&format!("{result_slot} = alloca {{ i8, ptr }}"));

                self.push_instr(&format!("br i1 {ok}, label %{some_bb}, label %{none_bb}"));

                self.start_bb(&none_bb);
                let none_r0 = self.next_reg();
                self.push_instr(&format!(
                    "{none_r0} = insertvalue {{ i8, ptr }} zeroinitializer, i8 1, 0"
                ));
                let none_r1 = self.next_reg();
                self.push_instr(&format!(
                    "{none_r1} = insertvalue {{ i8, ptr }} {none_r0}, ptr null, 1"
                ));
                self.push_instr(&format!("store {{ i8, ptr }} {none_r1}, ptr {result_slot}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.fn_ctx.terminated = true;

                self.start_bb(&some_bb);
                self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");
                let elem_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{elem_ptr} = call ptr @_mvl_array_get(ptr {val}, i64 {idx_val})"
                ));
                let elem_val = self.next_reg();
                self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
                let elem_slot = self.next_reg();
                self.push_instr(&format!("{elem_slot} = alloca {elem_llvm_ty}"));
                self.push_instr(&format!("store {elem_llvm_ty} {elem_val}, ptr {elem_slot}"));
                let some_r0 = self.next_reg();
                self.push_instr(&format!(
                    "{some_r0} = insertvalue {{ i8, ptr }} zeroinitializer, i8 0, 0"
                ));
                let some_r1 = self.next_reg();
                self.push_instr(&format!(
                    "{some_r1} = insertvalue {{ i8, ptr }} {some_r0}, ptr {elem_slot}, 1"
                ));
                self.push_instr(&format!("store {{ i8, ptr }} {some_r1}, ptr {result_slot}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.fn_ctx.terminated = true;

                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {{ i8, ptr }}, ptr {result_slot}"));
                self.fn_ctx
                    .reg_types
                    .insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }
            ("contains", "ptr")
                if matches!(
                    unwrap_labels(&receiver.ty),
                    Ty::List(_) | Ty::Array(_, _) | Ty::Set(_)
                ) && args.len() == 1 =>
            {
                let needle = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let needle_ty = self.ty_to_llvm_ctx(&args[0].ty);
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {needle_ty}"));
                self.push_instr(&format!("store {needle_ty} {needle}, ptr {slot}"));
                self.ensure_extern("declare i1 @_mvl_array_contains(ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i1 @_mvl_array_contains(ptr {val}, ptr {slot})"
                ));
                self.fn_ctx.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("sort", "ptr") if args.is_empty() => {
                Ok(Some(self.emit_c_call_simple("sort", &val, &[])))
            }
            ("reverse", "ptr") if args.is_empty() => {
                Ok(Some(self.emit_c_call_simple("reverse", &val, &[])))
            }
            ("enumerate", "ptr") if args.is_empty() => {
                Ok(Some(self.emit_c_call_simple("enumerate", &val, &[])))
            }
            ("entries", "ptr")
                if args.is_empty() && matches!(unwrap_labels(&receiver.ty), Ty::Map(_, _)) =>
            {
                Ok(Some(self.emit_c_call_simple("entries", &val, &[])))
            }
            ("zip", "ptr") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "zip",
                    &val,
                    &[("ptr", &other)],
                )))
            }
            ("windows", "ptr") if args.len() == 1 => {
                let n = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "windows",
                    &val,
                    &[("i64", &n)],
                )))
            }
            ("chunks", "ptr") if args.len() == 1 => {
                let n = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_simple(
                    "chunks",
                    &val,
                    &[("i64", &n)],
                )))
            }
            ("first", "ptr")
                if matches!(unwrap_labels(&receiver.ty), Ty::List(_) | Ty::Array(_, _)) =>
            {
                let elem_llvm_ty = match unwrap_labels(&receiver.ty) {
                    Ty::List(e) | Ty::Array(e, _) => self.ty_to_llvm_ctx(e),
                    _ => "i64".to_string(),
                };

                self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                let len = self.next_reg();
                self.push_instr(&format!("{len} = call i64 @_mvl_array_len(ptr {val})"));
                let not_empty = self.next_reg();
                self.push_instr(&format!("{not_empty} = icmp sgt i64 {len}, 0"));

                let some_bb = self.next_bb("first_some");
                let none_bb = self.next_bb("first_none");
                let merge_bb = self.next_bb("first_merge");

                let result_slot = self.next_reg();
                self.push_instr(&format!("{result_slot} = alloca {{ i8, ptr }}"));

                self.push_instr(&format!(
                    "br i1 {not_empty}, label %{some_bb}, label %{none_bb}"
                ));

                self.start_bb(&none_bb);
                let none_r0 = self.next_reg();
                self.push_instr(&format!(
                    "{none_r0} = insertvalue {{ i8, ptr }} zeroinitializer, i8 1, 0"
                ));
                let none_r1 = self.next_reg();
                self.push_instr(&format!(
                    "{none_r1} = insertvalue {{ i8, ptr }} {none_r0}, ptr null, 1"
                ));
                self.push_instr(&format!("store {{ i8, ptr }} {none_r1}, ptr {result_slot}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.fn_ctx.terminated = true;

                self.start_bb(&some_bb);
                self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");
                let elem_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{elem_ptr} = call ptr @_mvl_array_get(ptr {val}, i64 0)"
                ));
                let elem_val = self.next_reg();
                self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
                let elem_slot = self.next_reg();
                self.push_instr(&format!("{elem_slot} = alloca {elem_llvm_ty}"));
                self.push_instr(&format!("store {elem_llvm_ty} {elem_val}, ptr {elem_slot}"));
                let some_r0 = self.next_reg();
                self.push_instr(&format!(
                    "{some_r0} = insertvalue {{ i8, ptr }} zeroinitializer, i8 0, 0"
                ));
                let some_r1 = self.next_reg();
                self.push_instr(&format!(
                    "{some_r1} = insertvalue {{ i8, ptr }} {some_r0}, ptr {elem_slot}, 1"
                ));
                self.push_instr(&format!("store {{ i8, ptr }} {some_r1}, ptr {result_slot}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.fn_ctx.terminated = true;

                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {{ i8, ptr }}, ptr {result_slot}"));
                self.fn_ctx
                    .reg_types
                    .insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }

            // ── String::parse_int / parse_float → Result[T, String] ───────
            ("parse_int", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::String) => {
                self.emit_str_parse(&val, "i64", "_mvl_str_parse_int")
            }
            ("parse_float", "ptr") if matches!(unwrap_labels(&receiver.ty), Ty::String) => {
                self.emit_str_parse(&val, "double", "_mvl_str_parse_float")
            }

            // ── String::char_at(i) → Option[String] ──────────────────────
            ("char_at", "ptr") => {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr_tir(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                Ok(Some(self.emit_c_call_option_out_ptr(
                    "char_at",
                    &val,
                    &[("i64", &idx)],
                )))
            }

            ("set", "ptr")
                if args.len() == 2
                    && matches!(
                        unwrap_labels(&receiver.ty),
                        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_)
                    ) =>
            {
                let idx = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let item_ty = self.ty_to_llvm_ctx(&args[1].ty);
                let item_val = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let item_slot = self.next_reg();
                self.push_instr(&format!("{item_slot} = alloca {item_ty}"));
                self.push_instr(&format!("store {item_ty} {item_val}, ptr {item_slot}"));
                self.ensure_extern("declare void @_mvl_array_set(ptr, i64, ptr)");
                self.push_instr(&format!(
                    "call void @_mvl_array_set(ptr {val}, i64 {idx}, ptr {item_slot})"
                ));
                Ok(None)
            }
            ("slice", "ptr")
                if args.len() == 2
                    && matches!(
                        unwrap_labels(&receiver.ty),
                        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_)
                    ) =>
            {
                let start = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let end = match self.emit_expr_tir(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // `slice` has no `LLVM_DISPATCH` row — emit `_mvl_list_slice`
                // inline via the shared helper (matches the AST emit_method_call
                // path used by `slice` / `take` / `skip`).
                Ok(Some(self.emit_list_slice_call(&val, &start, &end)))
            }
            ("take", "ptr")
                if args.len() == 1
                    && matches!(
                        unwrap_labels(&receiver.ty),
                        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_)
                    ) =>
            {
                let n = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_list_slice_call(&val, "0", &n)))
            }
            ("skip", "ptr")
                if args.len() == 1
                    && matches!(
                        unwrap_labels(&receiver.ty),
                        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_)
                    ) =>
            {
                let n = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                let len_reg = self.next_reg();
                self.push_instr(&format!("{len_reg} = call i64 @_mvl_array_len(ptr {val})"));
                Ok(Some(self.emit_list_slice_call(&val, &n, &len_reg)))
            }
            ("concat", "ptr") if args.len() == 1 => {
                let other = match self.emit_expr_tir(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // List::concat → list_concat; String::concat → concat.
                let dispatch_key =
                    if matches!(unwrap_labels(&receiver.ty), Ty::List(_) | Ty::Array(_, _)) {
                        "list_concat"
                    } else {
                        "concat"
                    };
                Ok(Some(self.emit_c_call_simple(
                    dispatch_key,
                    &val,
                    &[("ptr", &other)],
                )))
            }

            // ── Option.is_some / is_none ──────────────────────────────────
            ("is_some", "{ i8, ptr }") => {
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {{ i8, ptr }} {val}, 0"));
                let result = self.next_reg();
                self.push_instr(&format!("{result} = icmp eq i8 {disc}, 0"));
                self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
                Ok(Some(result))
            }
            ("is_none", "{ i8, ptr }") => {
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {{ i8, ptr }} {val}, 0"));
                let result = self.next_reg();
                self.push_instr(&format!("{result} = icmp eq i8 {disc}, 1"));
                self.fn_ctx.reg_types.insert(result.clone(), "i1".into());
                Ok(Some(result))
            }

            // Fallback matches AST behavior — see #1612 follow-up tracking
            // the pre-existing silent-drop on user-defined extension methods
            // (Map::is_empty, Wrapper::peek, etc.) that affects BOTH walkers.
            _ => Ok(None),
        }
    }
}

fn unwrap_labels(ty: &Ty) -> &Ty {
    let mut cur = ty;
    while let Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) = cur {
        cur = inner;
    }
    cur
}
