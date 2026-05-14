// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor LLVM IR emission for the MVL LLVM backend (Phase 8, #696).
//!
//! Separates actor-specific codegen from the main `mod.rs` and `exprs.rs`.
//!
//! Public entry points on `LlvmBackend`:
//! - [`LlvmBackend::declare_actor_runtime_fns`] — declare the three C-ABI symbols once.
//! - [`LlvmBackend::emit_actor_decl`] — emit behavior functions + dispatch function.
//! - [`LlvmBackend::emit_actor_spawn`] — emit actor creation (`Expr::Spawn`).
//! - [`LlvmBackend::emit_actor_method_call`] — emit fire-and-forget behavior send.
//! - [`LlvmBackend::resolve_actor_type_name`] — detect whether a receiver is an actor handle.

use inkwell::{
    module::Linkage,
    types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum},
    values::BasicValueEnum,
    AddressSpace,
};

use crate::mvl::parser::ast::{ActorDecl, Expr, TypeExpr};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── Runtime declarations ──────────────────────────────────────────────────

    /// Declare the three C-ABI actor runtime functions once per module.
    ///
    /// - `mvl_actor_spawn(dispatch_fn_ptr, state_ptr, state_size) -> ptr`
    /// - `mvl_actor_send(handle_ptr, disc, argc, args_ptr) -> void`
    /// - `mvl_actor_drop(handle_ptr) -> void`
    pub(crate) fn declare_actor_runtime_fns(&self) {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let i64_ty = self.context.i64_type();

        // mvl_actor_spawn: (ptr dispatch_fn, ptr state, i64 state_size) -> ptr handle
        if self.module.get_function("mvl_actor_spawn").is_none() {
            let spawn_ty = ptr_ty.fn_type(&[ptr_ty.into(), ptr_ty.into(), i64_ty.into()], false);
            self.module
                .add_function("mvl_actor_spawn", spawn_ty, Some(Linkage::External));
        }

        // mvl_actor_send: (ptr handle, i64 disc, i64 argc, ptr args) -> void
        if self.module.get_function("mvl_actor_send").is_none() {
            let send_ty = self.context.void_type().fn_type(
                &[ptr_ty.into(), i64_ty.into(), i64_ty.into(), ptr_ty.into()],
                false,
            );
            self.module
                .add_function("mvl_actor_send", send_ty, Some(Linkage::External));
        }

        // mvl_actor_drop: (ptr handle) -> void
        if self.module.get_function("mvl_actor_drop").is_none() {
            let drop_ty = self.context.void_type().fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function("mvl_actor_drop", drop_ty, Some(Linkage::External));
        }
    }

    // ── Actor declaration emission ────────────────────────────────────────────

    /// Emit LLVM IR for an actor declaration (Phase 8, #696).
    ///
    /// Emits:
    /// 1. Individual behavior functions: `void @{actor}_{behavior}(ptr state, params…)`
    /// 2. Dispatch function: `void @{actor}_dispatch(ptr state, i64 disc, ptr args)`
    pub(crate) fn emit_actor_decl(&mut self, ad: &ActorDecl) {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;

        let actor_snake = actor_name_to_snake(&ad.name);
        let state_name = format!("{}State", ad.name);
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let void_ty = self.context.void_type();

        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).collect();

        // ── 1. Behavior functions ──────────────────────────────────────────
        for method in &ad.methods {
            let fn_name = format!("{actor_snake}_{}", method.name);
            if self.module.get_function(&fn_name).is_some() {
                continue;
            }

            // Params: (ptr state) + method params
            let mut param_types: Vec<BasicMetadataTypeEnum<'ctx>> = vec![ptr_ty.into()];
            for p in &method.params {
                if let Some(ty) = self.mvl_type_to_llvm(&p.ty) {
                    param_types.push(ty.into());
                } else {
                    param_types.push(i64_ty.into()); // fallback
                }
            }

            let fn_val = if self.is_unit_type(&method.return_type) {
                let fn_ty = void_ty.fn_type(&param_types, false);
                self.module.add_function(&fn_name, fn_ty, None)
            } else if let Some(ret_ty) = self.mvl_type_to_llvm(&method.return_type) {
                let fn_ty = ret_ty.fn_type(&param_types, false);
                self.module.add_function(&fn_name, fn_ty, None)
            } else {
                let fn_ty = void_ty.fn_type(&param_types, false);
                self.module.add_function(&fn_name, fn_ty, None)
            };

            // Emit body: set up locals for state fields and params, then emit block.
            let entry = self.context.append_basic_block(fn_val, "entry");
            self.builder.position_at_end(entry);
            self.locals.clear();
            self.local_mvl_types.clear();
            self.heap_locals.clear();
            self.terminated = false;
            self.current_fn = Some(fn_val);

            // param[0] is the state ptr — register each field as a GEP-based local.
            if let Some(state_param) = fn_val.get_nth_param(0) {
                state_param.set_name("self");
                if let Some(&state_ty) = self.llvm_struct_types.get(&state_name) {
                    let field_defs: Vec<_> = self
                        .struct_fields
                        .get(&state_name)
                        .cloned()
                        .unwrap_or_default();
                    let state_ptr_val = state_param.into_pointer_value();
                    for (i, (field_name, field_ty_expr)) in field_defs.iter().enumerate() {
                        if let Some(field_llvm_ty) = self.mvl_type_to_llvm(field_ty_expr) {
                            let gep = unsafe {
                                self.builder.build_in_bounds_gep(
                                    state_ty,
                                    state_ptr_val,
                                    &[i64_ty.const_zero(), i64_ty.const_int(i as u64, false)],
                                    field_name,
                                )
                            }
                            .unwrap();
                            self.locals.insert(field_name.clone(), (gep, field_llvm_ty));
                            self.local_mvl_types
                                .insert(field_name.clone(), field_ty_expr.clone());
                        }
                    }
                }
            }

            // Remaining params (index 1+): alloca + store.
            for (i, param) in method.params.iter().enumerate() {
                if let Some(param_val) = fn_val.get_nth_param((i + 1) as u32) {
                    param_val.set_name(&param.name);
                    if let Some(ty) = self.mvl_type_to_llvm(&param.ty) {
                        let alloca = self.builder.build_alloca(ty, &param.name).unwrap();
                        self.builder.build_store(alloca, param_val).unwrap();
                        self.locals.insert(param.name.clone(), (alloca, ty));
                    }
                    self.local_mvl_types
                        .insert(param.name.clone(), param.ty.clone());
                }
            }

            let body_val = self.emit_block(&method.body);

            if !self.terminated {
                if self.is_unit_type(&method.return_type) {
                    self.builder.build_return(None).unwrap();
                } else if let Some(val) = body_val {
                    self.builder.build_return(Some(&val)).unwrap();
                } else {
                    self.builder.build_return(None).unwrap();
                }
            }
        }

        // ── 2. Dispatch function ───────────────────────────────────────────
        // void @{actor}_dispatch(ptr state, i64 disc, ptr args)
        let dispatch_name = format!("{actor_snake}_dispatch");
        if self.module.get_function(&dispatch_name).is_some() {
            return;
        }

        let dispatch_ty = void_ty.fn_type(&[ptr_ty.into(), i64_ty.into(), ptr_ty.into()], false);
        let dispatch_fn = self.module.add_function(&dispatch_name, dispatch_ty, None);

        let entry = self.context.append_basic_block(dispatch_fn, "entry");
        self.builder.position_at_end(entry);
        self.locals.clear();
        self.heap_locals.clear();
        self.terminated = false;
        self.current_fn = Some(dispatch_fn);

        let state_arg = dispatch_fn.get_nth_param(0).unwrap().into_pointer_value();
        let disc_arg = dispatch_fn.get_nth_param(1).unwrap().into_int_value();
        let args_arg = dispatch_fn.get_nth_param(2).unwrap().into_pointer_value();

        if pub_methods.is_empty() {
            self.builder.build_return(None).unwrap();
            return;
        }

        // Build a switch on disc: one BB per public behavior.
        let default_bb = self.context.append_basic_block(dispatch_fn, "default");
        let cases: Vec<_> = pub_methods
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let bb = self
                    .context
                    .append_basic_block(dispatch_fn, &format!("behavior_{i}"));
                (i64_ty.const_int(i as u64, false), bb)
            })
            .collect();

        self.builder
            .build_switch(disc_arg, default_bb, &cases)
            .unwrap();

        // default: just return
        self.builder.position_at_end(default_bb);
        self.builder.build_return(None).unwrap();

        // Each case: load args from the i64 array and call the behavior function.
        for (i, method) in pub_methods.iter().enumerate() {
            let fn_name = format!("{actor_snake}_{}", method.name);
            let behavior_fn = self.module.get_function(&fn_name).unwrap();
            let bb = cases[i].1;
            self.builder.position_at_end(bb);

            let mut call_args: Vec<BasicValueEnum<'ctx>> = vec![state_arg.into()];
            for (j, param) in method.params.iter().enumerate() {
                let gep = unsafe {
                    self.builder.build_in_bounds_gep(
                        i64_ty,
                        args_arg,
                        &[i64_ty.const_int(j as u64, false)],
                        &format!("arg_{j}"),
                    )
                }
                .unwrap();
                let raw = self
                    .builder
                    .build_load(i64_ty, gep, &format!("raw_{j}"))
                    .unwrap();
                // Cast ptr-typed params from their i64 representation.
                let coerced = if let Some(BasicTypeEnum::PointerType(pt)) =
                    self.mvl_type_to_llvm(&param.ty)
                {
                    self.builder
                        .build_int_to_ptr(raw.into_int_value(), pt, &format!("ptr_{j}"))
                        .unwrap()
                        .into()
                } else {
                    raw
                };
                call_args.push(coerced);
            }

            self.builder
                .build_call(
                    behavior_fn,
                    &call_args.iter().map(|v| (*v).into()).collect::<Vec<_>>(),
                    "call",
                )
                .unwrap();
            self.builder.build_return(None).unwrap();
        }
    }

    // ── Spawn expression emission ─────────────────────────────────────────────

    /// Emit `Spawn { actor_type, fields }` as:
    /// ```llvm
    /// %state = alloca %CounterState
    /// store i64 <init>, ptr (GEP %state field N)
    /// %handle = call ptr @mvl_actor_spawn(@counter_dispatch, %state, <byte_size>)
    /// ```
    pub(crate) fn emit_actor_spawn(
        &mut self,
        actor_type: &str,
        fields: &[(String, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        use crate::mvl::backends::rust::emit_actors::actor_name_to_snake;
        let actor_snake = actor_name_to_snake(actor_type);
        let state_name = format!("{actor_type}State");
        let dispatch_name = format!("{actor_snake}_dispatch");

        let i64_ty = self.context.i64_type();
        let &state_ty = self.llvm_struct_types.get(&state_name)?;
        let state_alloca = self.builder.build_alloca(state_ty, "actor_state").ok()?;

        // Initialize each field from the provided initializers.
        let field_defs: Vec<_> = self
            .struct_fields
            .get(&state_name)
            .cloned()
            .unwrap_or_default();
        for (i, (field_name, _)) in field_defs.iter().enumerate() {
            if let Some((_, val_expr)) = fields.iter().find(|(n, _)| n == field_name) {
                if let Some(val) = self.emit_expr(val_expr) {
                    let gep = unsafe {
                        self.builder.build_in_bounds_gep(
                            state_ty,
                            state_alloca,
                            &[i64_ty.const_zero(), i64_ty.const_int(i as u64, false)],
                            &format!("field_{field_name}"),
                        )
                    }
                    .ok()?;
                    self.builder.build_store(gep, val).ok()?;
                }
            }
        }

        let dispatch_fn = self.module.get_function(&dispatch_name)?;
        let dispatch_ptr = dispatch_fn.as_global_value().as_pointer_value();
        let state_size = self.llvm_type_byte_size(state_ty.into()) as i64;
        let size_val = i64_ty.const_int(state_size as u64, false);

        let spawn_fn = self.module.get_function("mvl_actor_spawn")?;
        let call = self
            .builder
            .build_call(
                spawn_fn,
                &[dispatch_ptr.into(), state_alloca.into(), size_val.into()],
                "actor_handle",
            )
            .ok()?;

        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    // ── Method call emission ──────────────────────────────────────────────────

    /// Resolve whether `receiver` is a local bound to an actor type.
    ///
    /// Returns `Some(actor_type_name)` if the receiver is an identifier whose
    /// tracked MVL type is a known actor, `None` otherwise.
    pub(crate) fn resolve_actor_type_name(&self, receiver: &Expr) -> Option<String> {
        let Expr::Ident(name, _) = receiver else {
            return None;
        };
        let ty = self.local_mvl_types.get(name.as_str())?;
        let TypeExpr::Base { name: tn, .. } = ty else {
            return None;
        };
        self.actor_decls
            .contains_key(tn.as_str())
            .then(|| tn.clone())
    }

    /// Emit an actor method call `actor_handle.behavior(args...)` as:
    /// ```llvm
    /// %args = alloca [N x i64]
    /// store i64 <arg0>, ptr (GEP %args 0)
    /// …
    /// call void @mvl_actor_send(%handle, <disc>, N, %args)
    /// ```
    ///
    /// Returns `None` (unit) since behaviors are fire-and-forget.
    pub(crate) fn emit_actor_method_call(
        &mut self,
        handle_val: BasicValueEnum<'ctx>,
        actor_name: &str,
        method: &str,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());

        let ad = self.actor_decls.get(actor_name)?.clone();
        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).collect();
        let disc = pub_methods.iter().position(|m| m.name == method)?;
        let argc = args.len();

        // Build args array on the stack (null ptr when argc == 0).
        let args_alloca = if argc > 0 {
            let arr_ty = i64_ty.array_type(argc as u32);
            let alloca = self.builder.build_alloca(arr_ty, "actor_args").ok()?;
            for (j, arg_expr) in args.iter().enumerate() {
                let val = self.emit_expr(arg_expr)?;
                // Coerce every argument to i64 for the flat args array.
                let i64_val: BasicValueEnum<'ctx> = match val {
                    BasicValueEnum::IntValue(iv) => {
                        if iv.get_type().get_bit_width() != 64 {
                            self.builder
                                .build_int_z_extend(iv, i64_ty, "arg_i64")
                                .ok()?
                                .into()
                        } else {
                            iv.into()
                        }
                    }
                    BasicValueEnum::PointerValue(pv) => self
                        .builder
                        .build_ptr_to_int(pv, i64_ty, "arg_i64")
                        .ok()?
                        .into(),
                    BasicValueEnum::FloatValue(fv) => self
                        .builder
                        .build_float_to_signed_int(fv, i64_ty, "arg_i64")
                        .ok()?
                        .into(),
                    _ => BasicValueEnum::IntValue(i64_ty.const_zero()),
                };
                let gep = unsafe {
                    self.builder.build_in_bounds_gep(
                        i64_ty,
                        alloca,
                        &[i64_ty.const_int(j as u64, false)],
                        &format!("arg_ptr_{j}"),
                    )
                }
                .ok()?;
                self.builder
                    .build_store(gep, BasicValueEnum::IntValue(i64_val.into_int_value()))
                    .ok()?;
            }
            alloca
        } else {
            ptr_ty.const_null()
        };

        let send_fn = self.module.get_function("mvl_actor_send")?;
        let disc_val = i64_ty.const_int(disc as u64, false);
        let argc_val = i64_ty.const_int(argc as u64, false);
        let handle_ptr = handle_val.into_pointer_value();

        self.builder
            .build_call(
                send_fn,
                &[
                    handle_ptr.into(),
                    disc_val.into(),
                    argc_val.into(),
                    args_alloca.into(),
                ],
                "send",
            )
            .ok()?;

        None // behaviors return Unit
    }
}
