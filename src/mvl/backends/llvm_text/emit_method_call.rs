// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Method call dispatch for the `llvm_text` backend.

use crate::mvl::backends::llvm_symbol_by_name;
use crate::mvl::parser::ast::{Expr, TypeExpr};

use super::TextEmitter;

/// Look up the LLVM C-ABI symbol for a builtin method that has its
/// `llvm_symbol` hint populated in the shared `BUILTINS` registry.
///
/// Panics if the symbol is missing — callers in this file only invoke this
/// for the 13 methods explicitly tagged in `backends.rs`, so a missing entry
/// indicates the registry and emitter have drifted.
fn builtin_sym(name: &'static str) -> &'static str {
    llvm_symbol_by_name(name).unwrap_or_else(|| {
        panic!("BUILTINS missing llvm_symbol for '{name}' — drift between backends.rs and emit_method_call.rs");
    })
}

impl TextEmitter {
    pub(super) fn emit_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        // ── Actor method call: fire-and-forget send ───────────────────────
        if let Some(actor_name) = self.resolve_actor_type_name(receiver) {
            let handle_val = match self.emit_expr(receiver)? {
                Some(v) => v,
                None => return Ok(None),
            };
            return self.emit_actor_method_call(&handle_val, &actor_name.clone(), method, args);
        }

        let recv_ty = self.type_of_expr(receiver);
        let val = match self.emit_expr(receiver)? {
            Some(v) => v,
            None => return Ok(None),
        };

        match (method, recv_ty.as_str()) {
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
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("to_string", _) => {
                // String.to_string() is identity
                self.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }

            // ── Int (i64) numeric methods ─────────────────────────────────────
            ("abs", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call i64 @llvm.abs.i64(i64 {val}, i1 0)"));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("is_positive", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp sgt i64 {val}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_negative", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp slt i64 {val}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_zero", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp eq i64 {val}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("to_float", "i64") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = sitofp i64 {val} to double"));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("min", "i64") if args.len() == 1 => {
                let other = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @llvm.smin.i64(i64 {val}, i64 {other})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("max", "i64") if args.len() == 1 => {
                let other = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @llvm.smax.i64(i64 {val}, i64 {other})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("clamp", "i64") if args.len() == 2 => {
                let lo = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let hi = match self.emit_expr(&args[1])? {
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
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("pow", "i64") if args.len() == 1 => {
                let exp = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_int_pow(i64, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_int_pow(i64 {val}, i64 {exp})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // ── Float (double) numeric methods ────────────────────────────────
            ("abs", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.fabs.f64(double {val})"));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("ceil", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.ceil.f64(double {val})"));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("floor", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.floor.f64(double {val})"
                ));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("round", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.round.f64(double {val})"
                ));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("sqrt", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call double @llvm.sqrt.f64(double {val})"));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("to_int", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fptosi double {val} to i64"));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("is_nan", "double") => {
                // fcmp uno: true if either operand is a NaN
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp uno double {val}, 0.0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_finite", "double") => {
                // finite = not NaN and not infinite: fcmp ord (not NaN) AND fabs < +inf
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
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_infinite", "double") => {
                // infinite = fabs == +inf
                let abs_reg = self.next_reg();
                self.push_instr(&format!(
                    "{abs_reg} = call double @llvm.fabs.f64(double {val})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = fcmp oeq double {abs_reg}, 0x7FF0000000000000"
                ));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_positive", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp ogt double {val}, 0.0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("is_negative", "double") => {
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = fcmp olt double {val}, 0.0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("min", "double") if args.len() == 1 => {
                let other = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.minnum.f64(double {val}, double {other})"
                ));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("max", "double") if args.len() == 1 => {
                let other = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.maxnum.f64(double {val}, double {other})"
                ));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("clamp", "double") if args.len() == 2 => {
                let lo = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let hi = match self.emit_expr(&args[1])? {
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
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }
            ("pow", "double") if args.len() == 1 => {
                let exp = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call double @llvm.pow.f64(double {val}, double {exp})"
                ));
                self.reg_types.insert(reg.clone(), "double".into());
                Ok(Some(reg))
            }

            ("len", "ptr") => {
                let kind = self.mvl_receiver_kind(receiver);
                let is_list = matches!(kind, Some("List") | Some("Array") | Some("Set"));
                let is_map = matches!(kind, Some("Map"));
                let reg = self.next_reg();
                if is_list {
                    self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @_mvl_array_len(ptr {val})"));
                } else if is_map {
                    self.ensure_extern("declare i64 @_mvl_map_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @_mvl_map_len(ptr {val})"));
                } else {
                    self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @_mvl_str_len(ptr {val})"));
                }
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("concat", "ptr") => {
                self.ensure_extern("declare ptr @_mvl_string_concat(ptr, ptr)");
                let other = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_string_concat(ptr {val}, ptr {other})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            // ── Map methods ─────────────────────────────────────────────
            ("get", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_expr = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let key_ty = self.type_of_expr(key_expr);
                let key_arg = match self.emit_expr(key_expr)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_map_get(ptr, ptr, i64)");
                let (kp, kl) = if key_ty == "i64" {
                    // Integer key: stack-allocate the 8-byte key.
                    let slot = self.next_reg();
                    self.push_instr(&format!("{slot} = alloca i64"));
                    self.push_instr(&format!("store i64 {key_arg}, ptr {slot}"));
                    (slot, "8".to_string())
                } else {
                    // String key: use string pointer + length.
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
                // Build Option[T] as { i8, ptr }: disc=0 for Some (raw ptr IS the payload
                // pointer), disc=1 for None.  _mvl_map_get returns a pointer to the
                // stored value bytes — exactly the payload ptr unwrap_or expects.
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
                    "{opt_some} = insertvalue {{ i8, ptr }} {{ i8 0, ptr null }}, ptr {raw}, 1"
                ));
                self.push_instr(&format!("br label %{merge_bb}"));
                let some_end = self.current_bb.clone();
                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                let none_end = self.current_bb.clone();
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {{ i8, ptr }} [ {opt_some}, %{some_end} ], [ {{ i8 1, ptr null }}, %{none_end} ]"
                ));
                self.reg_types.insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }
            ("insert", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                if args.len() < 2 {
                    return Ok(None);
                }
                let key_arg = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let val_arg = match self.emit_expr(&args[1])? {
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
            ("keys", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @_mvl_map_keys(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_map_keys(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("values", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @_mvl_map_values(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_map_values(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("contains_key", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_expr = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let key_ty = self.type_of_expr(key_expr);
                let key_arg = match self.emit_expr(key_expr)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_map_get(ptr, ptr, i64)");
                let (kp, kl) = if key_ty == "i64" {
                    // Integer key: stack-allocate the 8-byte key.
                    let slot = self.next_reg();
                    self.push_instr(&format!("{slot} = alloca i64"));
                    self.push_instr(&format!("store i64 {key_arg}, ptr {slot}"));
                    (slot, "8".to_string())
                } else {
                    // String key: use string pointer + length.
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
                // null → false, non-null → true
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne ptr {raw}, null"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("contains", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Set")) => {
                let needle = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i1 @_mvl_set_contains_i64(ptr, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i1 @_mvl_set_contains_i64(ptr {val}, i64 {needle})"
                ));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            // Set algebra — intersection / difference / union all share the same
            // (ptr, ptr) -> ptr C-ABI shape against the i64-element array runtime.
            ("intersection" | "difference" | "union", "ptr")
                if args.len() == 1 && matches!(self.mvl_receiver_kind(receiver), Some("Set")) =>
            {
                let other = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let sym = match method {
                    "intersection" => "_mvl_set_intersection",
                    "difference" => "_mvl_set_difference",
                    "union" => "_mvl_set_union",
                    _ => unreachable!(),
                };
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val}, ptr {other})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            // List/Array/Set slice(start, end) / take(n) / skip(n) — all
            // lower to `_mvl_list_slice(ptr, i64, i64)`.
            ("slice", "ptr") if args.len() == 2 && self.is_list_array_set(receiver) => {
                let start = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let end = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_list_slice_call(&val, &start, &end)))
            }
            ("take", "ptr") if args.len() == 1 && self.is_list_array_set(receiver) => {
                let n = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                Ok(Some(self.emit_list_slice_call(&val, "0", &n)))
            }
            ("skip", "ptr") if args.len() == 1 && self.is_list_array_set(receiver) => {
                let n = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
                let len_reg = self.next_reg();
                self.push_instr(&format!("{len_reg} = call i64 @_mvl_array_len(ptr {val})"));
                Ok(Some(self.emit_list_slice_call(&val, &n, &len_reg)))
            }
            ("remove", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
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

            // ── HOF: filter / map / any / all / find / take_while / skip_while ──
            // Guard: only match when the argument is closure-like (Lambda or a
            // module-level function reference).  String::find takes a plain
            // String argument, not a closure, so it must not match this arm.
            ("filter" | "map" | "find" | "take_while" | "skip_while", "ptr")
                if args.len() == 1 && self.is_closure_arg(&args[0]) =>
            {
                // Runtime passes element by pointer: fn(env, elem_ptr) -> ...
                let closure = match self.emit_as_hof_closure(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("_mvl_list_{method}");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @{sym}(ptr {val}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("any" | "all", "ptr") if args.len() == 1 => {
                // Runtime passes element by pointer: fn(env, elem_ptr) -> i1
                let closure = match self.emit_as_hof_closure(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("_mvl_list_{method}");
                self.ensure_extern(&format!("declare i1 @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call i1 @{sym}(ptr {val}, ptr {closure})"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("fold", "ptr") if args.len() == 2 => {
                let init_ty = self.type_of_expr(&args[0]);
                let init_val = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // Fold closure: fn(env, acc_val, elem_ptr) -> acc_val
                // param 0 (acc) is by-value, param 1 (elem) is by-pointer
                let closure = match self.emit_as_hof_closure(&args[1], &[1])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                // Fold passes init by pointer so the runtime can return the
                // same type.  For scalar inits, stack-allocate a slot.
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {init_ty}"));
                self.push_instr(&format!("store {init_ty} {init_val}, ptr {slot}"));
                self.ensure_extern("declare ptr @_mvl_list_fold(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_list_fold(ptr {val}, ptr {slot}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                // Load the result back out as the init type.
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {init_ty}, ptr {reg}"));
                self.reg_types.insert(result.clone(), init_ty);
                Ok(Some(result))
            }

            // ── Category-D: sort / windows / chunks / partition / group_by ─
            // (#1290) All five promoted to `pub builtin fn` with C-ABI impls.

            // List::sort() → List[T] — returns a new sorted copy
            ("sort", "ptr") if args.is_empty() => {
                let sym = builtin_sym("sort");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // List::windows(n) → List[List[T]]
            ("windows", "ptr") if args.len() == 1 => {
                let n = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let sym = builtin_sym("windows");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, i64)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val}, i64 {n})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // List::chunks(n) → List[List[T]]
            ("chunks", "ptr") if args.len() == 1 => {
                let n = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let sym = builtin_sym("chunks");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, i64)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val}, i64 {n})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // List::partition(f) → Partitioned[T]   (struct, not tuple — #1380)
            // Runtime returns ptr to a 2-slot array of MvlArray* pointers; load
            // the two slots and assemble a `%Partitioned = { ptr, ptr }` struct
            // value so downstream `result.matching` / `result.rest` work as
            // ordinary `extractvalue` field accesses.
            ("partition", _) if args.len() == 1 && self.is_closure_arg(&args[0]) => {
                let closure = match self.emit_as_hof_closure(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = builtin_sym("partition");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @{sym}(ptr {val}, ptr {closure})"
                ));
                // Load slot 0 -> matching.
                let slot0 = self.next_reg();
                self.push_instr(&format!("{slot0} = getelementptr ptr, ptr {raw}, i64 0"));
                let matching = self.next_reg();
                self.push_instr(&format!("{matching} = load ptr, ptr {slot0}"));
                // Load slot 1 -> rest.
                let slot1 = self.next_reg();
                self.push_instr(&format!("{slot1} = getelementptr ptr, ptr {raw}, i64 1"));
                let rest = self.next_reg();
                self.push_instr(&format!("{rest} = load ptr, ptr {slot1}"));
                // Build Partitioned { matching, rest }.
                let tmp = self.next_reg();
                self.push_instr(&format!(
                    "{tmp} = insertvalue %Partitioned undef, ptr {matching}, 0"
                ));
                let val_reg = self.next_reg();
                self.push_instr(&format!(
                    "{val_reg} = insertvalue %Partitioned {tmp}, ptr {rest}, 1"
                ));
                self.reg_types
                    .insert(val_reg.clone(), "%Partitioned".into());
                Ok(Some(val_reg))
            }

            // List::group_by(f) → Map[K, List[T]]
            // Key closure signature: fn(env, elem_ptr) -> i64.
            ("group_by", "ptr") if args.len() == 1 && self.is_closure_arg(&args[0]) => {
                let closure = match self.emit_as_hof_closure(&args[0], &[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = builtin_sym("group_by");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @{sym}(ptr {val}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // ── List::push(item) → List (in-place) ───────────────────────
            ("push", "ptr") => {
                let item_arg = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let item_ty = self.type_of_expr(item_arg);
                let item_val = match self.emit_expr(item_arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // mvl_array_push expects a pointer to the item.
                let item_slot = self.next_reg();
                self.push_instr(&format!("{item_slot} = alloca {item_ty}"));
                self.push_instr(&format!("store {item_ty} {item_val}, ptr {item_slot}"));
                self.ensure_extern("declare void @_mvl_array_push(ptr, ptr)");
                self.push_instr(&format!(
                    "call void @_mvl_array_push(ptr {val}, ptr {item_slot})"
                ));
                // push returns the array (modified in place — same pointer).
                self.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }

            // ── List::set(i, value) → Unit ────────────────────────────────
            ("set", "ptr") if args.len() == 2 => {
                let idx = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let item_arg = &args[1];
                let item_ty = self.type_of_expr(item_arg);
                let item_val = match self.emit_expr(item_arg)? {
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

            // ── String::parse_int / parse_float → Result[T, String] ───────
            ("parse_int", "ptr") => self.emit_str_parse(&val, "i64", "_mvl_str_parse_int"),
            ("parse_float", "ptr") => self.emit_str_parse(&val, "double", "_mvl_str_parse_float"),

            // ── String::char_at(i) → Option[String] ──────────────────────
            ("char_at", "ptr") => {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let sym = builtin_sym("char_at");
                self.ensure_extern(&format!("declare i8 @{sym}(ptr, i64, ptr)"));
                let out = self.next_reg();
                self.push_instr(&format!("{out} = alloca ptr"));
                let tag = self.next_reg();
                self.push_instr(&format!(
                    "{tag} = call i8 @{sym}(ptr {val}, i64 {idx}, ptr {out})"
                ));
                let payload = self.next_reg();
                self.push_instr(&format!("{payload} = load ptr, ptr {out}"));
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca ptr"));
                self.push_instr(&format!("store ptr {payload}, ptr {slot}"));
                let r = self.wrap_result_pair(&tag, &slot);
                Ok(Some(r))
            }

            // ── String kernel builtins (#1186) ───────────────────────────

            // chars() → List[String]
            ("chars", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let sym = builtin_sym("chars");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // byte_at(i) → Option[Byte]
            ("byte_at", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let sym = builtin_sym("byte_at");
                self.ensure_extern(&format!("declare i8 @{sym}(ptr, i64, ptr)"));
                let out = self.next_reg();
                self.push_instr(&format!("{out} = alloca i64"));
                let tag = self.next_reg();
                self.push_instr(&format!(
                    "{tag} = call i8 @{sym}(ptr {val}, i64 {idx}, ptr {out})"
                ));
                let byte_val = self.next_reg();
                self.push_instr(&format!("{byte_val} = load i64, ptr {out}"));
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca i64"));
                self.push_instr(&format!("store i64 {byte_val}, ptr {slot}"));
                let r = self.wrap_result_pair(&tag, &slot);
                Ok(Some(r))
            }

            // find(sub) → Int  (-1 if not found)
            ("find", "ptr") if args.len() == 1 && !self.is_closure_arg(&args[0]) => {
                let sub = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let sym = builtin_sym("find");
                self.ensure_extern(&format!("declare i64 @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call i64 @{sym}(ptr {val}, ptr {sub})"));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // split(delimiter) → List[String]
            ("split", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let delim = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let sym = builtin_sym("split");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val}, ptr {delim})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // substring(start, end) → String
            ("substring", "ptr") if args.len() >= 2 => {
                let start = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let end = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let sym = builtin_sym("substring");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, i64, i64)"));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @{sym}(ptr {val}, i64 {start}, i64 {end})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // contains(sub) → Bool
            ("contains", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let sub = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_contains(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_contains(ptr {val}, ptr {sub})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // starts_with(prefix) → Bool
            ("starts_with", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let prefix = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_starts_with(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_starts_with(ptr {val}, ptr {prefix})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // ends_with(suffix) → Bool
            ("ends_with", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let suffix = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_ends_with(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_ends_with(ptr {val}, ptr {suffix})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // trim() → String
            ("trim", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @_mvl_str_trim(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_trim(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // to_lower() → String
            ("to_lower", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let sym = builtin_sym("to_lower");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // to_upper() → String
            ("to_upper", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let sym = builtin_sym("to_upper");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @{sym}(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // replace(old, new) → String
            ("replace", "ptr")
                if args.len() >= 2
                    && !matches!(
                        self.mvl_receiver_kind(receiver),
                        Some("List") | Some("Array") | Some("Set") | Some("Map")
                    ) =>
            {
                let old_s = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let new_s = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_replace(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_replace(ptr {val}, ptr {old_s}, ptr {new_s})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // ── List.get(i) → Option[T] ─────────────────────────────────
            ("get", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("List")) => {
                let idx_val = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };

                // Determine element LLVM type from MVL type annotation.
                let elem_llvm_ty = if let Some(Expr::Ident(name, _)) = Some(receiver) {
                    if let Some(TypeExpr::Base { args, .. }) =
                        self.local_mvl_types.get(name.as_str())
                    {
                        if let Some(inner) = args.first() {
                            self.llvm_ty_ctx(inner)
                        } else {
                            "i64".into()
                        }
                    } else {
                        "i64".into()
                    }
                } else {
                    "i64".into()
                };

                // Bounds check: index < len.
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

                // None branch.
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
                self.terminated = true;

                // Some branch.
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
                self.terminated = true;

                // Merge — load result.
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {{ i8, ptr }}, ptr {result_slot}"));
                self.reg_types.insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }

            // ── List.first() → Option[T] ─────────────────────────────
            ("first", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("List")) => {
                // Equivalent to List.get(0)
                let elem_llvm_ty = if let Some(Expr::Ident(name, _)) = Some(receiver) {
                    if let Some(TypeExpr::Base { args, .. }) =
                        self.local_mvl_types.get(name.as_str())
                    {
                        if let Some(inner) = args.first() {
                            self.llvm_ty_ctx(inner)
                        } else {
                            "i64".into()
                        }
                    } else {
                        "i64".into()
                    }
                } else {
                    "i64".into()
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

                // None.
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
                self.terminated = true;

                // Some.
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
                self.terminated = true;

                // Merge.
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {{ i8, ptr }}, ptr {result_slot}"));
                self.reg_types.insert(result.clone(), "{ i8, ptr }".into());
                Ok(Some(result))
            }

            // ── Option.unwrap_or(default) → T ──────────────────────────
            ("unwrap_or", "{ i8, ptr }") => {
                let default_val = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let default_ty = self.type_of_expr(&args[0]);

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

                // Some branch — load value from payload pointer.
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
                let some_end = self.current_bb.clone();

                // None branch — use default.
                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                let none_end = self.current_bb.clone();

                // Merge with phi.
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {default_ty} [ {some_val}, %{some_end} ], [ {default_val}, %{none_end} ]"
                ));
                self.reg_types.insert(result.clone(), default_ty);
                Ok(Some(result))
            }

            // ── Option.is_some() / is_none() → Bool ─────────────────────
            ("is_some", "{ i8, ptr }") => {
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {{ i8, ptr }} {val}, 0"));
                let result = self.next_reg();
                self.push_instr(&format!("{result} = icmp eq i8 {disc}, 0"));
                self.reg_types.insert(result.clone(), "i1".into());
                Ok(Some(result))
            }
            ("is_none", "{ i8, ptr }") => {
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {{ i8, ptr }} {val}, 0"));
                let result = self.next_reg();
                self.push_instr(&format!("{result} = icmp eq i8 {disc}, 1"));
                self.reg_types.insert(result.clone(), "i1".into());
                Ok(Some(result))
            }

            _ => Ok(None),
        }
    }

    // ── Struct construction ───────────────────────────────────────────────
}
