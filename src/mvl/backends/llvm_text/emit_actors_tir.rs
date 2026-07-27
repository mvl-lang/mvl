// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor lowering for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_actors.rs`. Mirrors the AST helpers
//! [`TextEmitter::emit_actor_decl`], [`emit_actor_spawn`],
//! [`emit_actor_method_call`], [`resolve_actor_type_name`]:
//!
//! - Reads actor metadata from `module.tir_actor_decls` (populated by
//!   `emit_program_tir`) — a parallel field to `actor_decls` while the
//!   AST and TIR walkers coexist.
//! - Field/param/return types come from `TirParam.ty: Ty` /
//!   `TirActorMethod.ret_ty: Ty` directly; `ty_to_type_expr` converts
//!   once at the LLVM emission boundary so the existing `llvm_ty_ctx`
//!   helpers apply unchanged.
//! - Method bodies walk through `emit_block_tir`; spawn/method-call
//!   field initialiser and arg expressions walk through `emit_expr_tir`.

use crate::mvl::ir::{MailboxConfig, MailboxPolicy, TirActorDecl, TirExpr, Ty, TypeExpr};
use crate::mvl::parser::lexer::Span;

use super::emit_helpers::ty_to_type_expr;
use super::{RefLocal, TextEmitter};

const MAX_ACTOR_ARGS: usize = 8;

/// `true` if `ty` is an LLVM aggregate type that doesn't round-trip via i64.
fn is_aggregate_llvm_ty(ty: &str) -> bool {
    ty.starts_with('%') || ty.starts_with('{')
}

fn sizeof_llvm_ty_expr(ty: &str) -> String {
    format!("ptrtoint (ptr getelementptr ({ty}, ptr null, i32 1) to i64)")
}

fn ty_or_unit(ty: &Ty) -> TypeExpr {
    ty_to_type_expr(ty).unwrap_or(TypeExpr::Base {
        name: "Unit".into(),
        args: vec![],
        span: Span::default(),
    })
}

impl TextEmitter {
    /// TIR variant of [`Self::emit_actor_decl`]. Emits behavior functions
    /// plus the dispatch function for one [`TirActorDecl`].
    pub(super) fn emit_actor_decl_tir(&mut self, ad: &TirActorDecl) -> Result<(), String> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let actor_snake = actor_name_to_snake(&ad.name);
        let state_name = format!("{}State", ad.name);

        // ── 1. Behavior functions ─────────────────────────────────────────
        for method in &ad.methods.clone() {
            let fn_name = format!("{actor_snake}_{}", method.name);
            let ret_ty_te = ty_or_unit(&method.ret_ty);
            let is_void = Self::is_void(&ret_ty_te);

            let mut param_parts = vec!["ptr %self".to_string()];
            for p in &method.params {
                let p_te = ty_or_unit(&p.ty);
                let ty_str = self.llvm_ty_ctx(&p_te);
                if ty_str != "void" {
                    param_parts.push(format!("{ty_str} %{}", p.name));
                }
            }
            let params_str = param_parts.join(", ");

            let define_ret = if is_void {
                "void".to_string()
            } else {
                self.llvm_ty_ctx(&ret_ty_te)
            };

            let state_name = state_name.clone();
            let method = method.clone();
            self.with_fresh_fn_ctx(ret_ty_te.clone(), |this| -> Result<(), String> {
                this.fn_ctx
                    .fn_buf
                    .push(format!("define {define_ret} @{fn_name}({params_str})"));
                this.fn_ctx.fn_buf.push("{".into());
                this.fn_ctx.fn_buf.push("entry:".into());

                let field_defs = this
                    .module
                    .struct_fields
                    .get(&state_name)
                    .cloned()
                    .unwrap_or_default();
                for (i, (field_name, field_ty)) in field_defs.iter().enumerate() {
                    let field_llvm_ty = this.llvm_ty_ctx(field_ty);
                    if field_llvm_ty == "void" {
                        continue;
                    }
                    let gep_reg = this.next_reg();
                    this.push_instr(&format!(
                        "{gep_reg} = getelementptr %{state_name}, ptr %self, i32 0, i32 {i}"
                    ));
                    this.fn_ctx.reg_types.insert(gep_reg.clone(), "ptr".into());
                    this.fn_ctx.ref_locals.insert(
                        field_name.clone(),
                        RefLocal {
                            ptr: gep_reg,
                            elem_ty: field_ty.clone(),
                        },
                    );
                    this.fn_ctx
                        .local_mvl_types
                        .insert(field_name.clone(), field_ty.clone());
                }

                for p in &method.params {
                    let p_te = ty_or_unit(&p.ty);
                    let ty_str = this.llvm_ty_ctx(&p_te);
                    if ty_str != "void" {
                        let ssa = format!("%{}", p.name);
                        this.fn_ctx.locals.insert(p.name.clone(), ssa.clone());
                        this.fn_ctx.reg_types.insert(ssa, ty_str);
                        this.fn_ctx.local_mvl_types.insert(p.name.clone(), p_te);
                    }
                }

                let body_val = this.emit_block_tir(&method.body)?;

                if !this.fn_ctx.terminated {
                    let llvm_ret = this.llvm_ty_ctx(&ret_ty_te);
                    if is_void {
                        this.push_instr("ret void");
                    } else if let Some(v) = body_val {
                        this.push_instr(&format!("ret {llvm_ret} {v}"));
                    } else {
                        this.push_instr(&format!("ret {llvm_ret} undef"));
                    }
                }

                this.fn_ctx.fn_buf.push("}".into());
                let fn_text = this.finish_fn_body();
                this.module.fn_bodies.push(fn_text);
                Ok(())
            })?;
        }

        // ── 2. Dispatch function ──────────────────────────────────────────
        let dispatch_name = format!("{actor_snake}_dispatch");
        let on_exit = ad
            .methods
            .iter()
            .find(|m| !m.is_public && m.name == "on_exit")
            .cloned();
        let on_down = ad
            .methods
            .iter()
            .find(|m| !m.is_public && m.name == "on_down")
            .cloned();
        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).cloned().collect();

        let unit_ret = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: Span::default(),
        };
        self.with_fresh_fn_ctx(unit_ret, |this| -> Result<(), String> {
            this.fn_ctx.fn_buf.push(format!(
                "define void @{dispatch_name}(ptr %state, i64 %disc, ptr %args)"
            ));
            this.fn_ctx.fn_buf.push("{".into());
            this.fn_ctx.fn_buf.push("entry:".into());

            let has_any_case =
                !pub_methods.is_empty() || on_exit.is_some() || on_down.is_some();

            if !has_any_case {
                this.push_instr("ret void");
            } else {
                let mut cases: Vec<String> = pub_methods
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("i64 {i}, label %behavior_{i}"))
                    .collect();
                if on_exit.is_some() {
                    cases.push("i64 -2, label %sys_exit_signal".to_string());
                }
                if on_down.is_some() {
                    cases.push("i64 -3, label %sys_down_signal".to_string());
                }
                this.push_instr(&format!(
                    "switch i64 %disc, label %default [ {} ]",
                    cases.join(" ")
                ));

                this.fn_ctx.fn_buf.push("default:".into());
                this.push_instr("ret void");

                let mut struct_used = false;
                for (disc, method) in pub_methods.iter().enumerate() {
                    let fn_name = format!("{actor_snake}_{}", method.name);
                    this.fn_ctx.fn_buf.push(format!("behavior_{disc}:"));

                    let mut call_parts = vec!["ptr %state".to_string()];
                    let mut struct_frees: Vec<(String, String)> = Vec::new();
                    for (j, p) in method.params.iter().enumerate() {
                        let p_te = ty_or_unit(&p.ty);
                        let ty_str = this.llvm_ty_ctx(&p_te);
                        if ty_str == "void" {
                            continue;
                        }
                        let gep = this.next_reg();
                        this.push_instr(&format!(
                            "{gep} = getelementptr i64, ptr %args, i64 {j}"
                        ));
                        let raw = this.next_reg();
                        this.push_instr(&format!("{raw} = load i64, ptr {gep}"));

                        if ty_str == "ptr" {
                            let coerced = this.next_reg();
                            this.push_instr(&format!(
                                "{coerced} = inttoptr i64 {raw} to ptr"
                            ));
                            call_parts.push(format!("ptr {coerced}"));
                        } else if ty_str == "i1" {
                            let truncated = this.next_reg();
                            this.push_instr(&format!("{truncated} = trunc i64 {raw} to i1"));
                            call_parts.push(format!("i1 {truncated}"));
                        } else if is_aggregate_llvm_ty(&ty_str) {
                            struct_used = true;
                            let hp = this.next_reg();
                            this.push_instr(&format!("{hp} = inttoptr i64 {raw} to ptr"));
                            let loaded = this.next_reg();
                            this.push_instr(&format!(
                                "{loaded} = load {ty_str}, ptr {hp}"
                            ));
                            call_parts.push(format!("{ty_str} {loaded}"));
                            let sz = sizeof_llvm_ty_expr(&ty_str);
                            struct_frees.push((hp, sz));
                        } else {
                            call_parts.push(format!("{ty_str} {raw}"));
                        }
                    }

                    let call_args_str = call_parts.join(", ");

                    // `pub test fn` methods are synchronous reads: call them for
                    // their value and hand it back through the reply cell whose
                    // address `_mvl_actor_sync_call` parked in args[argc] (#2012).
                    // Everything else is fire-and-forget, so its result (if any)
                    // is discarded.
                    let ret_te = ty_or_unit(&method.ret_ty);
                    let ret_ty_str = this.llvm_ty_ctx(&ret_te);
                    let returns_value = !Self::is_void(&ret_te) && ret_ty_str != "void";

                    if method.is_test && returns_value {
                        let ret_val = this.next_reg();
                        this.push_instr(&format!(
                            "{ret_val} = call {ret_ty_str} @{fn_name}({call_args_str})"
                        ));
                        let as_i64 = match ret_ty_str.as_str() {
                            "i64" => ret_val,
                            "ptr" => {
                                let c = this.next_reg();
                                this.push_instr(&format!(
                                    "{c} = ptrtoint ptr {ret_val} to i64"
                                ));
                                c
                            }
                            "i1" => {
                                let c = this.next_reg();
                                this.push_instr(&format!("{c} = zext i1 {ret_val} to i64"));
                                c
                            }
                            "double" => {
                                let c = this.next_reg();
                                this.push_instr(&format!(
                                    "{c} = bitcast double {ret_val} to i64"
                                ));
                                c
                            }
                            "i32" => {
                                let c = this.next_reg();
                                this.push_instr(&format!("{c} = sext i32 {ret_val} to i64"));
                                c
                            }
                            s if is_aggregate_llvm_ty(s) => {
                                // Box the aggregate and pass its address; the
                                // caller loads it back out.
                                struct_used = true;
                                let sz = sizeof_llvm_ty_expr(&ret_ty_str);
                                let hp = this.next_reg();
                                this.push_instr(&format!(
                                    "{hp} = call ptr @_mvl_alloc(i64 {sz})"
                                ));
                                this.push_instr(&format!(
                                    "store {ret_ty_str} {ret_val}, ptr {hp}"
                                ));
                                let c = this.next_reg();
                                this.push_instr(&format!("{c} = ptrtoint ptr {hp} to i64"));
                                c
                            }
                            _ => ret_val,
                        };
                        let reply_slot = method.params.len();
                        let rgep = this.next_reg();
                        this.push_instr(&format!(
                            "{rgep} = getelementptr i64, ptr %args, i64 {reply_slot}"
                        ));
                        let raw_reply = this.next_reg();
                        this.push_instr(&format!("{raw_reply} = load i64, ptr {rgep}"));
                        let reply_ptr = this.next_reg();
                        this.push_instr(&format!(
                            "{reply_ptr} = inttoptr i64 {raw_reply} to ptr"
                        ));
                        this.push_instr(&format!(
                            "call void @_mvl_actor_sync_reply(ptr {reply_ptr}, i64 {as_i64})"
                        ));
                        this.ensure_extern("declare void @_mvl_actor_sync_reply(ptr, i64)");
                    } else if returns_value {
                        let discarded = this.next_reg();
                        this.push_instr(&format!(
                            "{discarded} = call {ret_ty_str} @{fn_name}({call_args_str})"
                        ));
                    } else {
                        this.push_instr(&format!("call void @{fn_name}({call_args_str})"));
                    }

                    for (hp, sz) in struct_frees {
                        this.push_instr(&format!(
                            "call void @_mvl_free(ptr {hp}, i64 {sz})"
                        ));
                    }
                    this.push_instr("ret void");
                }
                if struct_used {
                    this.ensure_extern("declare void @_mvl_free(ptr, i64)");
                }

                if let Some(m) = &on_exit {
                    let fn_name = format!("{actor_snake}_{}", m.name);
                    this.fn_ctx.fn_buf.push("sys_exit_signal:".into());
                    let gep0 = this.next_reg();
                    this.push_instr(&format!(
                        "{gep0} = getelementptr i64, ptr %args, i64 0"
                    ));
                    let from_id = this.next_reg();
                    this.push_instr(&format!("{from_id} = load i64, ptr {gep0}"));
                    let gep1 = this.next_reg();
                    this.push_instr(&format!(
                        "{gep1} = getelementptr i64, ptr %args, i64 1"
                    ));
                    let reason = this.next_reg();
                    this.push_instr(&format!("{reason} = load i64, ptr {gep1}"));
                    this.push_instr(&format!(
                        "call void @{fn_name}(ptr %state, i64 {from_id}, i64 {reason})"
                    ));
                    this.push_instr("ret void");
                }

                if let Some(m) = &on_down {
                    let fn_name = format!("{actor_snake}_{}", m.name);
                    this.fn_ctx.fn_buf.push("sys_down_signal:".into());
                    let gep0 = this.next_reg();
                    this.push_instr(&format!(
                        "{gep0} = getelementptr i64, ptr %args, i64 0"
                    ));
                    let from_id = this.next_reg();
                    this.push_instr(&format!("{from_id} = load i64, ptr {gep0}"));
                    let gep1 = this.next_reg();
                    this.push_instr(&format!(
                        "{gep1} = getelementptr i64, ptr %args, i64 1"
                    ));
                    let reason = this.next_reg();
                    this.push_instr(&format!("{reason} = load i64, ptr {gep1}"));
                    let gep2 = this.next_reg();
                    this.push_instr(&format!(
                        "{gep2} = getelementptr i64, ptr %args, i64 2"
                    ));
                    let monitor_id = this.next_reg();
                    this.push_instr(&format!("{monitor_id} = load i64, ptr {gep2}"));
                    this.push_instr(&format!(
                        "call void @{fn_name}(ptr %state, i64 {from_id}, i64 {reason}, i64 {monitor_id})"
                    ));
                    this.push_instr("ret void");
                }
            }

            this.fn_ctx.fn_buf.push("}".into());
            let dispatch_text = this.fn_ctx.fn_buf.join("\n");
            this.module.fn_bodies.push(dispatch_text);
            Ok(())
        })
    }

    // ── Spawn expression emission ─────────────────────────────────────────

    /// TIR variant of [`Self::emit_actor_spawn`].
    pub(super) fn emit_actor_spawn_tir(
        &mut self,
        actor_type: &str,
        fields: &[(String, TirExpr)],
    ) -> Result<Option<String>, String> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let actor_snake = actor_name_to_snake(actor_type);
        let state_name = format!("{actor_type}State");
        let dispatch_name = format!("{actor_snake}_dispatch");

        let state_alloca = self.next_reg();
        self.push_instr(&format!("{state_alloca} = alloca %{state_name}"));
        self.fn_ctx
            .reg_types
            .insert(state_alloca.clone(), "ptr".into());

        let field_defs = self
            .module
            .struct_fields
            .get(&state_name)
            .cloned()
            .unwrap_or_default();
        for (i, (field_name, field_ty)) in field_defs.iter().enumerate() {
            let field_llvm_ty = self.llvm_ty_ctx(field_ty);
            if field_llvm_ty == "void" {
                continue;
            }
            if let Some((_, init_expr)) = fields.iter().find(|(n, _)| n == field_name) {
                if let Some(val) = self.emit_expr_tir(init_expr)? {
                    let field_ptr = self.next_reg();
                    self.push_instr(&format!(
                        "{field_ptr} = getelementptr %{state_name}, ptr {state_alloca}, i32 0, i32 {i}"
                    ));
                    self.push_instr(&format!("store {field_llvm_ty} {val}, ptr {field_ptr}"));
                }
            }
        }

        let state_size = (field_defs.len() as i64) * 8;

        // Mailbox config from the TIR actor decl.
        let mailbox = self
            .module
            .tir_actor_decls
            .get(actor_type)
            .and_then(|ad| ad.mailbox.as_ref())
            .cloned();
        let (capacity, policy) = match &mailbox {
            Some(MailboxConfig::Unbounded) => (0i64, 0i64),
            Some(MailboxConfig::Bounded { capacity, policy }) => {
                let pol = if matches!(policy, MailboxPolicy::Block) {
                    1i64
                } else {
                    0i64
                };
                (*capacity as i64, pol)
            }
            None => (256i64, 0i64),
        };

        let handle = self.next_reg();
        self.push_instr(&format!(
            "{handle} = call ptr @_mvl_actor_spawn(ptr @{dispatch_name}, ptr {state_alloca}, i64 {state_size}, i64 {capacity}, i64 {policy})"
        ));
        self.fn_ctx.reg_types.insert(handle.clone(), "ptr".into());
        self.fn_ctx.spawned_actor_handles.push(handle.clone());

        let traps_exit = self
            .module
            .tir_actor_decls
            .get(actor_type)
            .map(|ad| ad.traps_exit)
            .unwrap_or(false);
        if traps_exit {
            let id_reg = self.next_reg();
            self.push_instr(&format!(
                "{id_reg} = call i64 @_mvl_actor_get_id(ptr {handle})"
            ));
            self.push_instr(&format!(
                "call void @_mvl_actors_set_trap_exit(i64 {id_reg})"
            ));
            self.ensure_extern("declare void @_mvl_actors_set_trap_exit(i64)");
        }

        Ok(Some(handle))
    }

    // ── Actor method call (fire-and-forget send) ──────────────────────────

    /// TIR variant of [`Self::resolve_actor_type_name`].
    pub(super) fn resolve_actor_type_name_tir(&self, receiver: &TirExpr) -> Option<String> {
        let type_name = match unwrap_labels_for_actor(&receiver.ty) {
            Ty::Named(n, _) => n.clone(),
            _ => return None,
        };
        self.module
            .tir_actor_decls
            .contains_key(type_name.as_str())
            .then_some(type_name)
    }

    /// TIR variant of [`Self::emit_actor_method_call`].
    /// Emit `self.method(args…)` inside an actor method body as a direct call
    /// on the state pointer.
    ///
    /// Intra-actor calls are synchronous: the actor already holds exclusive
    /// access to its own state while dispatching, so there is nothing to
    /// serialise, and routing through the mailbox would race the caller's own
    /// pending messages non-deterministically. The Rust backend has always done
    /// this (all methods live in `impl {Name}State`); WASM's drain-at-send makes
    /// it observationally identical. LLVM used to drop the call entirely (#2012).
    pub(super) fn emit_actor_self_call_tir(
        &mut self,
        actor_name: &str,
        method: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let ad = match self.module.tir_actor_decls.get(actor_name).cloned() {
            Some(a) => a,
            None => return Ok(None),
        };
        let Some(callee) = ad.methods.iter().find(|m| m.name == method).cloned() else {
            return Ok(None);
        };

        let fn_name = format!("{}_{}", actor_name_to_snake(actor_name), callee.name);
        let mut call_parts = vec!["ptr %self".to_string()];
        for (param, arg) in callee.params.iter().zip(args.iter()) {
            let ty_str = self.llvm_ty_ctx(&ty_or_unit(&param.ty));
            if ty_str == "void" {
                continue;
            }
            let Some(val) = self.emit_expr_tir(arg)? else {
                continue;
            };
            call_parts.push(format!("{ty_str} {val}"));
        }
        let call_args = call_parts.join(", ");

        let ret_te = ty_or_unit(&callee.ret_ty);
        let ret_ty_str = self.llvm_ty_ctx(&ret_te);
        if Self::is_void(&ret_te) || ret_ty_str == "void" {
            self.push_instr(&format!("call void @{fn_name}({call_args})"));
            return Ok(None);
        }
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call {ret_ty_str} @{fn_name}({call_args})"
        ));
        self.fn_ctx.reg_types.insert(reg.clone(), ret_ty_str);
        Ok(Some(reg))
    }

    pub(super) fn emit_actor_method_call_tir(
        &mut self,
        handle_val: &str,
        actor_name: &str,
        method: &str,
        args: &[TirExpr],
    ) -> Result<Option<String>, String> {
        let ad = match self.module.tir_actor_decls.get(actor_name).cloned() {
            Some(a) => a,
            None => return Ok(None),
        };

        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).cloned().collect();
        let disc = match pub_methods.iter().position(|m| m.name == method) {
            Some(d) => d,
            None => return Ok(None),
        };
        let callee = pub_methods[disc].clone();

        // `pub test fn` is a synchronous read, not a behavior: it must return a
        // value and must observe every message enqueued before it. The reply
        // cell address takes a slot in the message's arg array (#2012).
        let is_sync = callee.is_test;
        let arg_budget = if is_sync {
            MAX_ACTOR_ARGS - 1
        } else {
            MAX_ACTOR_ARGS
        };

        let argc = args.len();
        if argc > arg_budget {
            return Err(format!(
                "actor {} '{method}' has {argc} parameters; maximum is {arg_budget}",
                if is_sync { "test method" } else { "behavior" }
            ));
        }

        let args_ptr = if argc > 0 {
            let arr_alloca = self.next_reg();
            self.push_instr(&format!("{arr_alloca} = alloca [{argc} x i64]"));
            self.fn_ctx
                .reg_types
                .insert(arr_alloca.clone(), "ptr".into());

            let mut struct_used = false;
            for (j, arg_expr) in args.iter().enumerate() {
                let val = match self.emit_expr_tir(arg_expr)? {
                    Some(v) => v,
                    None => continue,
                };
                let arg_ty = self.ty_to_llvm_ctx(&arg_expr.ty);
                let i64_val = match arg_ty.as_str() {
                    "ptr" => {
                        let coerced = self.next_reg();
                        self.push_instr(&format!("{coerced} = ptrtoint ptr {val} to i64"));
                        coerced
                    }
                    "i1" => {
                        let coerced = self.next_reg();
                        self.push_instr(&format!("{coerced} = zext i1 {val} to i64"));
                        coerced
                    }
                    s if is_aggregate_llvm_ty(s) => {
                        struct_used = true;
                        let sz = sizeof_llvm_ty_expr(&arg_ty);
                        let hp = self.next_reg();
                        self.push_instr(&format!("{hp} = call ptr @_mvl_alloc(i64 {sz})"));
                        self.push_instr(&format!("store {arg_ty} {val}, ptr {hp}"));
                        let coerced = self.next_reg();
                        self.push_instr(&format!("{coerced} = ptrtoint ptr {hp} to i64"));
                        coerced
                    }
                    _ => val,
                };

                let gep = self.next_reg();
                self.push_instr(&format!(
                    "{gep} = getelementptr i64, ptr {arr_alloca}, i64 {j}"
                ));
                self.push_instr(&format!("store i64 {i64_val}, ptr {gep}"));
            }
            if struct_used {
                self.ensure_extern("declare ptr @_mvl_alloc(i64)");
            }
            arr_alloca
        } else {
            "null".to_string()
        };

        if !is_sync {
            self.push_instr(&format!(
                "call void @_mvl_actor_send(ptr {handle_val}, i64 {disc}, i64 {argc}, ptr {args_ptr})"
            ));
            return Ok(None);
        }

        // Synchronous path: block on the reply and widen the i64 back to the
        // method's declared return type. `expr.ty` is `Unknown` for actor method
        // calls, so the actor registry is the only source for this type.
        let raw = self.next_reg();
        self.push_instr(&format!(
            "{raw} = call i64 @_mvl_actor_sync_call(ptr {handle_val}, i64 {disc}, i64 {argc}, ptr {args_ptr})"
        ));
        self.ensure_extern("declare i64 @_mvl_actor_sync_call(ptr, i64, i64, ptr)");
        self.fn_ctx.reg_types.insert(raw.clone(), "i64".into());

        let ret_te = ty_or_unit(&callee.ret_ty);
        let ret_ty_str = self.llvm_ty_ctx(&ret_te);
        if Self::is_void(&ret_te) || ret_ty_str == "void" {
            return Ok(None);
        }

        let value = match ret_ty_str.as_str() {
            "i64" => raw,
            "ptr" => {
                let c = self.next_reg();
                self.push_instr(&format!("{c} = inttoptr i64 {raw} to ptr"));
                c
            }
            "i1" => {
                let c = self.next_reg();
                self.push_instr(&format!("{c} = trunc i64 {raw} to i1"));
                c
            }
            "double" => {
                let c = self.next_reg();
                self.push_instr(&format!("{c} = bitcast i64 {raw} to double"));
                c
            }
            "i32" => {
                let c = self.next_reg();
                self.push_instr(&format!("{c} = trunc i64 {raw} to i32"));
                c
            }
            s if is_aggregate_llvm_ty(s) => {
                // The dispatch arm boxed the aggregate; load it back and release
                // the box.
                let hp = self.next_reg();
                self.push_instr(&format!("{hp} = inttoptr i64 {raw} to ptr"));
                let loaded = self.next_reg();
                self.push_instr(&format!("{loaded} = load {ret_ty_str}, ptr {hp}"));
                let sz = sizeof_llvm_ty_expr(&ret_ty_str);
                self.push_instr(&format!("call void @_mvl_free(ptr {hp}, i64 {sz})"));
                self.ensure_extern("declare void @_mvl_free(ptr, i64)");
                loaded
            }
            _ => raw,
        };
        self.fn_ctx.reg_types.insert(value.clone(), ret_ty_str);
        Ok(Some(value))
    }
}

/// Strip label/refinement/ref wrappers off `ty` for actor-type detection.
fn unwrap_labels_for_actor(ty: &Ty) -> &Ty {
    let mut cur = ty;
    while let Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) = cur {
        cur = inner;
    }
    cur
}
