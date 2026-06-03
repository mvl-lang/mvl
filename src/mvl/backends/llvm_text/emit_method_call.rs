// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Method call dispatch for the `llvm_text` backend.

use crate::mvl::parser::ast::Expr;

use super::TextEmitter;

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
            ("len", "ptr") => {
                let kind = self.mvl_receiver_kind(receiver);
                let is_list = matches!(kind, Some("List") | Some("Array") | Some("Set"));
                let is_map = matches!(kind, Some("Map"));
                let reg = self.next_reg();
                if is_list {
                    self.ensure_extern("declare i64 @mvl_array_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @mvl_array_len(ptr {val})"));
                } else if is_map {
                    self.ensure_extern("declare i64 @mvl_map_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @mvl_map_len(ptr {val})"));
                } else {
                    self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @_mvl_str_len(ptr {val})"));
                }
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("concat", "ptr") => {
                self.ensure_extern("declare ptr @mvl_string_concat(ptr, ptr)");
                let other = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @mvl_string_concat(ptr {val}, ptr {other})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            // ── Map methods ─────────────────────────────────────────────
            ("get", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare ptr @mvl_map_get(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                // Null-guard: mvl_map_get returns null if key not found.
                let is_null = self.next_reg();
                self.push_instr(&format!("{is_null} = icmp eq ptr {raw}, null"));
                let some_bb = self.next_bb("map_get_some");
                let none_bb = self.next_bb("map_get_none");
                let merge_bb = self.next_bb("map_get_merge");
                self.push_instr(&format!(
                    "br i1 {is_null}, label %{none_bb}, label %{some_bb}"
                ));
                self.start_bb(&some_bb);
                let loaded = self.next_reg();
                self.push_instr(&format!("{loaded} = load i64, ptr {raw}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi i64 [ {loaded}, %{some_bb} ], [ 0, %{none_bb} ]"
                ));
                self.reg_types.insert(result.clone(), "i64".into());
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
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @mvl_map_insert(ptr, ptr, i64, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let val_ty = self.infer_val_type(&val_arg);
                let vs = self.next_reg();
                self.push_instr(&format!("{vs} = alloca {val_ty}"));
                self.push_instr(&format!("store {val_ty} {val_arg}, ptr {vs}"));
                self.push_instr(&format!(
                    "call void @mvl_map_insert(ptr {val}, ptr {kp}, i64 {kl}, ptr {vs}, i64 8)"
                ));
                // insert returns the map (modified in place)
                Ok(Some(val))
            }
            ("keys", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @mvl_map_keys(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_map_keys(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("values", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @mvl_map_values(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_map_values(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("contains_key", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare ptr @mvl_map_get(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
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
                // FIXME: mvl_array_len returns u64 in Rust but is declared i64
                // here — same as the pre-existing `len` dispatch. Safe at
                // realistic array sizes; revisit when fixing the u64/i64 ABI
                // mismatch across all callers.
                self.ensure_extern("declare i64 @mvl_array_len(ptr)");
                let len_reg = self.next_reg();
                self.push_instr(&format!("{len_reg} = call i64 @mvl_array_len(ptr {val})"));
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
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @mvl_map_remove(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                self.push_instr(&format!(
                    "call void @mvl_map_remove(ptr {val}, ptr {kp}, i64 {kl})"
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
                let closure = match self.emit_as_closure(&args[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("List_{method}");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @{sym}(ptr {val}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("any" | "all", "ptr") if args.len() == 1 => {
                let closure = match self.emit_as_closure(&args[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("List_{method}");
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
                let closure = match self.emit_as_closure(&args[1])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                // Fold passes init by pointer so the runtime can return the
                // same type.  For scalar inits, stack-allocate a slot.
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {init_ty}"));
                self.push_instr(&format!("store {init_ty} {init_val}, ptr {slot}"));
                self.ensure_extern("declare ptr @List_fold(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @List_fold(ptr {val}, ptr {slot}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                // Load the result back out as the init type.
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {init_ty}, ptr {reg}"));
                self.reg_types.insert(result.clone(), init_ty);
                Ok(Some(result))
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
                self.ensure_extern("declare void @mvl_array_push(ptr, ptr)");
                self.push_instr(&format!(
                    "call void @mvl_array_push(ptr {val}, ptr {item_slot})"
                ));
                // push returns the array (modified in place — same pointer).
                self.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }

            // ── String::parse_int / parse_float → Result[T, String] ───────
            ("parse_int", "ptr") => self.emit_str_parse(&val, "i64", "_mvl_str_parse_int"),
            ("parse_float", "ptr") => self.emit_str_parse(&val, "double", "_mvl_str_parse_float"),

            // ── String::char_at(i) → String ───────────────────────────────
            ("char_at", "ptr") => {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_char_at(ptr, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_char_at(ptr {val}, i64 {idx})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // ── String kernel builtins (#1186) ───────────────────────────

            // chars() → List[String]
            ("chars", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @mvl_string_chars(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_string_chars(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // byte_at(i) → Int
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
                self.ensure_extern("declare i64 @_mvl_str_byte_at(ptr, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_str_byte_at(ptr {val}, i64 {idx})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // find(sub) → Int  (-1 if not found)
            ("find", "ptr") if args.len() == 1 && !self.is_closure_arg(&args[0]) => {
                let sub = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_find(ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_str_find(ptr {val}, ptr {sub})"
                ));
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
                self.ensure_extern("declare ptr @_mvl_str_split(ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_split(ptr {val}, ptr {delim})"
                ));
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
                self.ensure_extern("declare ptr @_mvl_str_substring(ptr, i64, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_substring(ptr {val}, i64 {start}, i64 {end})"
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
                self.ensure_extern("declare ptr @_mvl_str_to_lower(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_to_lower(ptr {val})"));
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
                self.ensure_extern("declare ptr @_mvl_str_to_upper(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_to_upper(ptr {val})"));
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

            _ => Ok(None),
        }
    }

    // ── Struct construction ───────────────────────────────────────────────
}
