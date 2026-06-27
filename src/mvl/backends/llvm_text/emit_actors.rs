// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor LLVM IR emission for the `llvm_text` backend (Phase 3B, issue #1149).
//!
//! Each `actor Counter { count: Int; pub fn increment(val n: Int) { } }` lowers to:
//!
//! ```llvm
//! %CounterState = type { i64 }
//!
//! define void @counter_increment(ptr %self, i64 %n) { ... }
//!
//! define void @counter_dispatch(ptr %state, i64 %disc, ptr %args) {
//! entry:
//!   switch i64 %disc, label %default [i64 0, label %behavior_0]
//! default:
//!   ret void
//! behavior_0:
//!   %gep_0 = getelementptr i64, ptr %args, i64 0
//!   %arg_0 = load i64, ptr %gep_0
//!   call void @counter_increment(ptr %state, i64 %arg_0)
//!   ret void
//! }
//! ```
//!
//! Runtime symbols (C-ABI from `runtime/llvm/src/actors.rs`):
//! - `mvl_actor_spawn(dispatch_fn, state_ptr, state_size, capacity, policy) -> ptr`
//! - `mvl_actor_send(handle, disc, argc, args_ptr) -> void`
//! - `mvl_actor_drop(handle) -> void`
//! - `mvl_actor_self() -> ptr`
//! - `mvl_actor_join_all() -> void`

use crate::mvl::parser::ast::{ActorDecl, Expr, MailboxConfig, MailboxPolicy, TypeExpr};

use super::{RefLocal, TextEmitter};

/// Maximum behavior parameter count — must match `MAX_ARGS` in `runtime/llvm/src/actors.rs`.
const MAX_ACTOR_ARGS: usize = 8;

impl TextEmitter {
    // ── Runtime extern declarations ───────────────────────────────────────

    /// Emit all actor runtime extern declarations exactly once.
    pub(super) fn ensure_actor_runtime_externs(&mut self) {
        if self.module.actor_runtime_declared {
            return;
        }
        self.ensure_extern("declare ptr @_mvl_actor_spawn(ptr, ptr, i64, i64, i64)");
        self.ensure_extern("declare void @_mvl_actor_send(ptr, i64, i64, ptr)");
        self.ensure_extern("declare void @_mvl_actor_drop(ptr)");
        self.ensure_extern("declare ptr @_mvl_actor_self()");
        self.ensure_extern("declare void @_mvl_actor_join_all()");
        self.ensure_extern("declare i64 @_mvl_actor_get_id(ptr)");
        // Link/monitor externs (#1599): ID-based C-ABI matching the MVL surface
        // (`std.actors.{link, unlink, monitor, demonitor}`). Declared via the
        // standard `c_symbols` builtin path; nothing else to declare here.
        self.module.actor_runtime_declared = true;
    }

    // ── Actor declaration emission ────────────────────────────────────────

    /// Emit LLVM IR for an actor declaration.
    ///
    /// Emits:
    /// 1. One behavior function per method: `void @{snake}_{method}(ptr %self, params…)`
    /// 2. Dispatch function: `void @{snake}_dispatch(ptr %state, i64 %disc, ptr %args)`
    pub(super) fn emit_actor_decl(&mut self, ad: &ActorDecl) -> Result<(), String> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let actor_snake = actor_name_to_snake(&ad.name);
        let state_name = format!("{}State", ad.name);

        // ── 1. Behavior functions ──────────────────────────────────────────
        for method in &ad.methods.clone() {
            let fn_name = format!("{actor_snake}_{}", method.name);
            let ret_ty = method.return_type.as_ref().clone();
            let is_void = Self::is_void(&ret_ty);

            // (ptr %self, param0, param1, ...)
            let mut param_parts = vec!["ptr %self".to_string()];
            for p in &method.params {
                let ty_str = self.llvm_ty_ctx(&p.ty);
                if ty_str != "void" {
                    param_parts.push(format!("{ty_str} %{}", p.name));
                }
            }
            let params_str = param_parts.join(", ");

            let define_ret = if is_void {
                "void".to_string()
            } else {
                self.llvm_ty_ctx(&ret_ty)
            };

            // Emit the actor method as a separate top-level function with a
            // fresh FnCtx — see TextEmitter::with_fresh_fn_ctx (#1535).
            let state_name = state_name.clone();
            let method = method.clone();
            self.with_fresh_fn_ctx(ret_ty.clone(), |this| -> Result<(), String> {
                this.fn_ctx
                    .fn_buf
                    .push(format!("define {define_ret} @{fn_name}({params_str})"));
                this.fn_ctx.fn_buf.push("{".into());
                this.fn_ctx.fn_buf.push("entry:".into());

                // Register state fields as ref-locals (GEP into %self) so that
                // reads load and writes store through the state pointer.
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

                // Register user parameters as SSA locals.
                for p in &method.params {
                    let ty_str = this.llvm_ty_ctx(&p.ty);
                    if ty_str != "void" {
                        let ssa = format!("%{}", p.name);
                        this.fn_ctx.locals.insert(p.name.clone(), ssa.clone());
                        this.fn_ctx.reg_types.insert(ssa, ty_str);
                        this.fn_ctx
                            .local_mvl_types
                            .insert(p.name.clone(), p.ty.clone());
                    }
                }

                let body_val = this.emit_block(&method.body)?;

                if !this.fn_ctx.terminated {
                    let llvm_ret = this.llvm_ty_ctx(&ret_ty);
                    if is_void {
                        this.push_instr("ret void");
                    } else if let Some(v) = body_val {
                        this.push_instr(&format!("ret {llvm_ret} {v}"));
                    } else {
                        this.push_instr(&format!("ret {llvm_ret} undef"));
                    }
                }

                this.fn_ctx.fn_buf.push("}".into());
                let fn_text = this.fn_ctx.fn_buf.join("\n");
                this.module.fn_bodies.push(fn_text);
                Ok(())
            })?;
        }

        // ── 2. Dispatch function ───────────────────────────────────────────
        // void @{snake}_dispatch(ptr %state, i64 %disc, ptr %args)
        let dispatch_name = format!("{actor_snake}_dispatch");
        // Signal handlers: private on_exit / on_down methods routed via disc -2/-3 (#1597).
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
        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).collect();

        // Dispatch fn emitted with a fresh FnCtx (#1535). Return type is Unit
        // (the function always emits `ret void`) so the inner current_ret_ty
        // is unused.
        let pub_methods: Vec<_> = pub_methods.into_iter().cloned().collect();
        let unit_ret = TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: Default::default(),
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
                // switch i64 %disc, label %default [ i64 0, label %behavior_0 ... ]
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

                // default: just return
                this.fn_ctx.fn_buf.push("default:".into());
                this.push_instr("ret void");

                // Each case BB: load typed args from flat i64 array, call behavior.
                for (disc, method) in pub_methods.iter().enumerate() {
                    let fn_name = format!("{actor_snake}_{}", method.name);
                    this.fn_ctx.fn_buf.push(format!("behavior_{disc}:"));

                    let mut call_parts = vec!["ptr %state".to_string()];
                    for (j, p) in method.params.iter().enumerate() {
                        let ty_str = this.llvm_ty_ctx(&p.ty);
                        if ty_str == "void" {
                            continue;
                        }
                        let gep = this.next_reg();
                        this.push_instr(&format!("{gep} = getelementptr i64, ptr %args, i64 {j}"));
                        let raw = this.next_reg();
                        this.push_instr(&format!("{raw} = load i64, ptr {gep}"));

                        // Rehydrate: ptr args were stored as i64 (ptrtoint).
                        if ty_str == "ptr" {
                            let coerced = this.next_reg();
                            this.push_instr(&format!("{coerced} = inttoptr i64 {raw} to ptr"));
                            call_parts.push(format!("ptr {coerced}"));
                        } else if ty_str == "i1" {
                            // Bool was zero-extended to i64; truncate back.
                            let truncated = this.next_reg();
                            this.push_instr(&format!("{truncated} = trunc i64 {raw} to i1"));
                            call_parts.push(format!("i1 {truncated}"));
                        } else {
                            call_parts.push(format!("{ty_str} {raw}"));
                        }
                    }

                    let call_args_str = call_parts.join(", ");
                    this.push_instr(&format!("call void @{fn_name}({call_args_str})"));
                    this.push_instr("ret void");
                }

                // ExitSignal block: disc=-2, args=[from_id, reason] (#1597).
                if let Some(m) = &on_exit {
                    let fn_name = format!("{actor_snake}_{}", m.name);
                    this.fn_ctx.fn_buf.push("sys_exit_signal:".into());
                    let gep0 = this.next_reg();
                    this.push_instr(&format!("{gep0} = getelementptr i64, ptr %args, i64 0"));
                    let from_id = this.next_reg();
                    this.push_instr(&format!("{from_id} = load i64, ptr {gep0}"));
                    let gep1 = this.next_reg();
                    this.push_instr(&format!("{gep1} = getelementptr i64, ptr %args, i64 1"));
                    let reason = this.next_reg();
                    this.push_instr(&format!("{reason} = load i64, ptr {gep1}"));
                    this.push_instr(&format!(
                        "call void @{fn_name}(ptr %state, i64 {from_id}, i64 {reason})"
                    ));
                    this.push_instr("ret void");
                }

                // DownSignal block: disc=-3, args=[from_id, reason, monitor_id] (#1597).
                if let Some(m) = &on_down {
                    let fn_name = format!("{actor_snake}_{}", m.name);
                    this.fn_ctx.fn_buf.push("sys_down_signal:".into());
                    let gep0 = this.next_reg();
                    this.push_instr(&format!("{gep0} = getelementptr i64, ptr %args, i64 0"));
                    let from_id = this.next_reg();
                    this.push_instr(&format!("{from_id} = load i64, ptr {gep0}"));
                    let gep1 = this.next_reg();
                    this.push_instr(&format!("{gep1} = getelementptr i64, ptr %args, i64 1"));
                    let reason = this.next_reg();
                    this.push_instr(&format!("{reason} = load i64, ptr {gep1}"));
                    let gep2 = this.next_reg();
                    this.push_instr(&format!("{gep2} = getelementptr i64, ptr %args, i64 2"));
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

    /// Emit `Expr::Spawn { actor_type, fields }`.
    ///
    /// Allocates the state struct on the stack, stores field initializers,
    /// and calls `@_mvl_actor_spawn` returning the opaque handle pointer.
    pub(super) fn emit_actor_spawn(
        &mut self,
        actor_type: &str,
        fields: &[(String, Expr)],
    ) -> Result<Option<String>, String> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let actor_snake = actor_name_to_snake(actor_type);
        let state_name = format!("{actor_type}State");
        let dispatch_name = format!("{actor_snake}_dispatch");

        // Alloca the state struct.
        let state_alloca = self.next_reg();
        self.push_instr(&format!("{state_alloca} = alloca %{state_name}"));
        self.fn_ctx
            .reg_types
            .insert(state_alloca.clone(), "ptr".into());

        // Store each field initializer via GEP.
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
            // Find the initializer for this field (if provided).
            if let Some((_, init_expr)) = fields.iter().find(|(n, _)| n == field_name) {
                if let Some(val) = self.emit_expr(init_expr)? {
                    let field_ptr = self.next_reg();
                    self.push_instr(&format!(
                        "{field_ptr} = getelementptr %{state_name}, ptr {state_alloca}, i32 0, i32 {i}"
                    ));
                    self.push_instr(&format!("store {field_llvm_ty} {val}, ptr {field_ptr}"));
                }
            }
        }

        // State byte size: 8 bytes per field (all fields are i64 or ptr on 64-bit).
        let state_size = (field_defs.len() as i64) * 8;

        // Resolve mailbox config from the actor declaration.
        let mailbox = self
            .module
            .actor_decls
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
            None => (256i64, 0i64), // default: 256-capacity, DropNewest
        };

        let handle = self.next_reg();
        self.push_instr(&format!(
            "{handle} = call ptr @_mvl_actor_spawn(ptr @{dispatch_name}, ptr {state_alloca}, i64 {state_size}, i64 {capacity}, i64 {policy})"
        ));
        self.fn_ctx.reg_types.insert(handle.clone(), "ptr".into());
        // Track for drop before mvl_actor_join_all (closes the sender so the
        // actor thread's recv loop terminates).
        self.fn_ctx.spawned_actor_handles.push(handle.clone());

        // Wire `traps_exit` flag from the actor declaration (#1599). When set,
        // a linked actor's death delivers ExitSignal to this actor's on_exit
        // handler instead of cascading the kill.
        let traps_exit = self
            .module
            .actor_decls
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

    /// Resolve `receiver` to an actor type name if it is a known actor handle.
    ///
    /// Checks `local_mvl_types` for the receiver identifier; returns the type
    /// name if it matches a known actor declaration.
    pub(super) fn resolve_actor_type_name(&self, receiver: &Expr) -> Option<String> {
        let type_name = match receiver {
            Expr::Ident(name, _) => {
                let ty = self.fn_ctx.local_mvl_types.get(name.as_str())?;
                if let crate::mvl::parser::ast::TypeExpr::Base { name: tn, .. } = ty {
                    tn.clone()
                } else {
                    return None;
                }
            }
            Expr::FieldAccess { field, .. } => {
                let ty = self.fn_ctx.local_mvl_types.get(field.as_str())?;
                if let crate::mvl::parser::ast::TypeExpr::Base { name: tn, .. } = ty {
                    tn.clone()
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        self.module
            .actor_decls
            .contains_key(type_name.as_str())
            .then_some(type_name)
    }

    /// Emit an actor behavior call `handle.behavior(args…)` as a fire-and-forget send.
    ///
    /// Packs arguments into a flat `[N x i64]` array (coercing all types to i64),
    /// then calls `@_mvl_actor_send(handle, disc, argc, args_ptr)`.
    /// Returns `Ok(None)` — behaviors produce Unit.
    pub(super) fn emit_actor_method_call(
        &mut self,
        handle_val: &str,
        actor_name: &str,
        method: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let ad = match self.module.actor_decls.get(actor_name).cloned() {
            Some(a) => a,
            None => return Ok(None),
        };

        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).cloned().collect();
        let disc = match pub_methods.iter().position(|m| m.name == method) {
            Some(d) => d,
            None => return Ok(None), // unknown behavior or private method
        };

        let argc = args.len();
        if argc > MAX_ACTOR_ARGS {
            return Err(format!(
                "actor behavior '{method}' has {argc} parameters; maximum is {MAX_ACTOR_ARGS}"
            ));
        }

        // Build flat [N x i64] args array on the stack.
        let args_ptr = if argc > 0 {
            let arr_alloca = self.next_reg();
            self.push_instr(&format!("{arr_alloca} = alloca [{argc} x i64]"));
            self.fn_ctx
                .reg_types
                .insert(arr_alloca.clone(), "ptr".into());

            for (j, arg_expr) in args.iter().enumerate() {
                let val = match self.emit_expr(arg_expr)? {
                    Some(v) => v,
                    None => continue,
                };
                // Coerce to i64: ptr → ptrtoint, i1 → zext, i64 → identity.
                let arg_ty = self.type_of_expr(arg_expr);
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
                    _ => val,
                };

                let gep = self.next_reg();
                self.push_instr(&format!(
                    "{gep} = getelementptr i64, ptr {arr_alloca}, i64 {j}"
                ));
                self.push_instr(&format!("store i64 {i64_val}, ptr {gep}"));
            }
            arr_alloca
        } else {
            "null".to_string()
        };

        self.push_instr(&format!(
            "call void @_mvl_actor_send(ptr {handle_val}, i64 {disc}, i64 {argc}, ptr {args_ptr})"
        ));

        Ok(None) // fire-and-forget returns Unit
    }
}
