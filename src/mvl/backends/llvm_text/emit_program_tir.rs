// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Program- and function-level emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to the top of `emitter.rs::emit_program` (which iterates `Decl`s).
//! Walks a [`TirProgram`] directly: `tir.fns`, `tir.types`, `tir.actors`, etc.
//!
//! Built leaf-first — function-body emission is delegated to `emit_*_tir`
//! submodules. The TIR variants of helpers reuse the existing AST-side
//! helpers where the inputs are shared types (e.g. `TypeExpr`, `Literal`,
//! `Pattern`) re-exported via `crate::mvl::ir`. At the `Ty → TypeExpr`
//! boundary the helper [`super::emit_stmts::ty_to_type_expr`] is reused.

use crate::mvl::ir::{TirFn, TirProgram, TirTypeBody, TirTypeDecl, TirVariantFields, Ty, TypeExpr};
use crate::mvl::parser::lexer::Span;

use super::emit_stmts::ty_to_type_expr;
use super::{TextEmitter, MAIN_RET};

/// Convert a [`Ty`] back to a [`TypeExpr`] for use by AST-shaped helpers.
///
/// Falls back to `Unit` for `Ty` variants that don't have a clean `TypeExpr`
/// representation — these only appear in positions where the existing
/// emitter logic also treats them as opaque (e.g. `Ty::Session`).
pub(super) fn ty_to_type_expr_or_unit(ty: &Ty) -> TypeExpr {
    ty_to_type_expr(ty).unwrap_or(TypeExpr::Base {
        name: "Unit".into(),
        args: Vec::new(),
        span: Span::default(),
    })
}

impl TextEmitter {
    /// Walk a [`TirProgram`] and emit the LLVM IR module body.
    ///
    /// Mirror of `emit_program(&Program)` but consumes already-lowered TIR.
    /// Monomorphization is performed by `ir::lower::lower` upstream, so the
    /// `MonoQueue` infrastructure used by the AST path is no-op here.
    pub(super) fn emit_program_tir(&mut self, prog: &TirProgram) -> Result<(), String> {
        // Pre-pass: register enum variants so struct field type resolution via
        // `ty_to_llvm_ctx` can see enum types regardless of declaration order.
        for td in &prog.types {
            if let TirTypeBody::Enum(variants) = &td.body {
                let variant_names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                let variant_fields: Vec<Vec<TypeExpr>> = variants
                    .iter()
                    .map(|v| match &v.fields {
                        TirVariantFields::Tuple(tys) => {
                            tys.iter().map(ty_to_type_expr_or_unit).collect()
                        }
                        TirVariantFields::Struct(fields) => fields
                            .iter()
                            .map(|f| ty_to_type_expr_or_unit(&f.ty))
                            .collect(),
                        TirVariantFields::Unit => Vec::new(),
                    })
                    .collect();
                for v in variants {
                    if let TirVariantFields::Struct(fields) = &v.fields {
                        let qname = format!("{}::{}", td.name, v.name);
                        let names: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
                        self.module
                            .enum_struct_variant_field_names
                            .insert(qname, names);
                    }
                }
                self.module
                    .enum_variants
                    .insert(td.name.clone(), variant_names);
                self.module
                    .enum_variant_fields
                    .insert(td.name.clone(), variant_fields);
            }
        }

        // First pass: register fn signatures and emit struct/alias type defs.
        // Generic fns are stashed in `tir_generic_fns` and registered per
        // mangled instantiation when a call site enqueues them (#1612, Bug 4).
        for f in &prog.fns {
            if !f.type_params.is_empty() {
                self.mono.tir_generic_fns.insert(f.name.clone(), f.clone());
                continue;
            }
            self.register_fn_tir_sig(f);
        }
        for td in &prog.types {
            self.register_type_decl_tir(td);
        }

        // Extern blocks: emit `declare` for each `extern "c"` fn (#811).
        // Mirrors `emitter.rs::emit_program::Decl::Extern` handling.
        for ed in &prog.externs {
            if ed.abi != "c" {
                continue;
            }
            for lib in &ed.link_libs {
                self.ensure_extern(&format!("; link: {lib}"));
            }
            for ef in &ed.fns {
                let ret_te = ty_to_type_expr_or_unit(&ef.ret_ty);
                let ret_str = Self::llvm_ty(&ret_te);
                let param_tys: Vec<String> = ef
                    .params
                    .iter()
                    .map(|p| Self::llvm_ty(&ty_to_type_expr_or_unit(&p.ty)))
                    .collect();
                let decl = format!("declare {} @{}({})", ret_str, ef.name, param_tys.join(", "));
                self.ensure_extern(&decl);
                // Register return + param types so call emission works.
                self.module
                    .fn_ret_types
                    .insert(ef.name.clone(), ret_te.clone());
                self.module.fn_param_types.insert(
                    ef.name.clone(),
                    ef.params
                        .iter()
                        .map(|p| ty_to_type_expr_or_unit(&p.ty))
                        .collect(),
                );
            }
        }

        // Actor pre-pass: register state struct types + tir_actor_decls.
        for ad in &prog.actors {
            let state_name = format!("{}State", ad.name);
            let field_list: Vec<(String, TypeExpr)> = ad
                .fields
                .iter()
                .map(|f| (f.name.clone(), ty_to_type_expr_or_unit(&f.ty)))
                .collect();
            let field_types: Vec<String> = field_list
                .iter()
                .map(|(_, ty)| self.llvm_ty_ctx(ty))
                .collect();
            self.module.type_defs.push(format!(
                "%{state_name} = type {{ {} }}",
                field_types.join(", ")
            ));
            self.module.struct_fields.insert(state_name, field_list);
            self.module
                .tir_actor_decls
                .insert(ad.name.clone(), ad.clone());
        }

        // Actor pass: emit behavior + dispatch functions.
        if !prog.actors.is_empty() {
            self.ensure_actor_runtime_externs();
            // Sort keys for deterministic emission order — see the matching
            // sort in `emitter.rs::emit_program`.
            let mut actor_names: Vec<String> =
                self.module.tir_actor_decls.keys().cloned().collect();
            actor_names.sort();
            for name in actor_names {
                // Avoid re-emitting the same actor across multiple emit_program_tir
                // calls (prelude + entry) — #1610.
                if !self.module.actor_emitted.insert(name.clone()) {
                    continue;
                }
                let ad = self.module.tir_actor_decls[&name].clone();
                self.emit_actor_decl_tir(&ad)?;
            }
        }

        // Emit each non-test, non-builtin, non-generic function body. Generics
        // are emitted via the drain loop below, one mangled copy per concrete
        // instantiation (mirror of the AST emit_program path).
        for f in &prog.fns {
            if !f.is_test && !f.is_builtin && f.type_params.is_empty() {
                self.emit_fn_tir(f)?;
            }
        }

        // Drain the monomorphization queue. Each iteration may enqueue more
        // instantiations (a mangled body can call another generic fn), so loop
        // until the queue stabilizes. Limit guards against pathological
        // mutually-recursive generic chains.
        const TIR_MONO_LIMIT: usize = 10_000;
        let mut iters = 0usize;
        while !self.mono.tir_mono_queue.is_empty() {
            iters += 1;
            if iters > TIR_MONO_LIMIT {
                return Err(
                    "TIR monomorphization limit exceeded — possible infinite instantiation".into(),
                );
            }
            let queue = std::mem::take(&mut self.mono.tir_mono_queue);
            for (mangled, orig_name, concrete_tys) in queue {
                let gf = match self.mono.tir_generic_fns.get(&orig_name) {
                    Some(f) => f.clone(),
                    None => continue,
                };
                let mut subs: std::collections::HashMap<String, Ty> =
                    std::collections::HashMap::new();
                for (tp, ct) in gf.type_params.iter().zip(concrete_tys.iter()) {
                    subs.insert(tp.name().to_string(), ct.clone());
                }
                let mut mangled_fn = self.substitute_tir_fn(&gf, &subs);
                mangled_fn.name = mangled;
                mangled_fn.type_params.clear();
                self.emit_fn_tir(&mangled_fn)?;
            }
        }

        Ok(())
    }

    /// Register a [`TirFn`]'s signature into the module-level dispatch tables.
    pub(super) fn register_fn_tir_sig(&mut self, f: &TirFn) {
        let ret = ty_to_type_expr_or_unit(&f.ret_ty);
        let params: Vec<TypeExpr> = f
            .params
            .iter()
            .map(|p| ty_to_type_expr_or_unit(&p.ty))
            .collect();
        self.module.fn_ret_types.insert(f.name.clone(), ret.clone());
        self.module
            .fn_param_types
            .insert(f.name.clone(), params.clone());
        if let Some(recv) = &f.receiver_type {
            let qualified = format!("{}::{}", recv, f.name);
            self.module.fn_ret_types.insert(qualified.clone(), ret);
            self.module.fn_param_types.insert(qualified, params);
        }
    }

    /// Register a [`TirTypeDecl`] — emit struct type defs, store fields, capture
    /// fn-type aliases. Enums are handled in the pre-pass above.
    fn register_type_decl_tir(&mut self, td: &TirTypeDecl) {
        match &td.body {
            TirTypeBody::Struct { fields, .. } => {
                if fields.is_empty() {
                    // Opaque handle — don't register; ty_to_llvm_ctx falls back to "ptr".
                    return;
                }
                let field_list: Vec<(String, TypeExpr)> = fields
                    .iter()
                    .map(|f| (f.name.clone(), ty_to_type_expr_or_unit(&f.ty)))
                    .collect();
                let field_types: Vec<String> = field_list
                    .iter()
                    .map(|(_, ty)| self.llvm_ty_ctx(ty))
                    .collect();
                self.module.type_defs.push(format!(
                    "%{} = type {{ {} }}",
                    td.name,
                    field_types.join(", ")
                ));
                self.module
                    .struct_fields
                    .insert(td.name.clone(), field_list);
            }
            TirTypeBody::Enum(_) => {
                // Already registered in pre-pass.
            }
            TirTypeBody::Alias(inner) => {
                let inner_te = ty_to_type_expr_or_unit(inner);
                if matches!(inner_te, TypeExpr::Fn { .. }) {
                    self.module.fn_aliases.insert(td.name.clone(), inner_te);
                }
            }
        }
    }

    /// Emit the body of a single [`TirFn`]. Mirrors `emit_fn(&FnDecl)`.
    pub(super) fn emit_fn_tir(&mut self, f: &TirFn) -> Result<(), String> {
        use crate::mvl::backends::llvm_text::context::FnCtx;

        let ret_ty_te = ty_to_type_expr_or_unit(&f.ret_ty);
        self.fn_ctx = FnCtx::new(ret_ty_te.clone());
        self.fn_ctx.current_fn_is_main = f.name == "main";

        let params: Vec<String> = f
            .params
            .iter()
            .filter_map(|p| {
                let ty_str = self.ty_to_llvm_ctx(&p.ty);
                if ty_str == "void" {
                    None
                } else {
                    Some(format!("{ty_str} %{}", p.name))
                }
            })
            .collect();
        let params_str = params.join(", ");

        let llvm_ret = self.ty_to_llvm_ctx(&f.ret_ty);

        let sig = if self.fn_ctx.current_fn_is_main {
            "define i32 @main()".to_string()
        } else {
            format!(
                "define {llvm_ret} @{fn_name}({params_str})",
                fn_name = f.name
            )
        };

        self.push_line(&sig);
        self.push_line("{");
        self.push_line("entry:");

        // Bind parameters as SSA locals, track MVL types for downstream lookups.
        for p in &f.params {
            let ty_str = self.ty_to_llvm_ctx(&p.ty);
            if ty_str != "void" {
                let ssa = format!("%{}", p.name);
                self.fn_ctx.locals.insert(p.name.clone(), ssa.clone());
                self.fn_ctx.reg_types.insert(ssa, ty_str);
                self.fn_ctx
                    .local_mvl_types
                    .insert(p.name.clone(), ty_to_type_expr_or_unit(&p.ty));
            }
        }

        let body_val = self.emit_block_tir(&f.body)?;

        if !self.fn_ctx.terminated {
            if let Some(crate::mvl::ir::TirStmt::Expr { expr, .. }) = f.body.stmts.last() {
                self.exclude_returned_value_tir(expr);
            }
            self.emit_heap_drops();
            if self.fn_ctx.current_fn_is_main {
                // TODO(#1612 PR 2): drop `actor_decls` fallback once the AST walker
                // is deleted — only `tir_actor_decls` will be populated on the TIR path.
                let has_actors =
                    !self.module.actor_decls.is_empty() || !self.module.tir_actor_decls.is_empty();
                if has_actors {
                    for handle in std::mem::take(&mut self.fn_ctx.spawned_actor_handles) {
                        self.push_instr(&format!("call void @_mvl_actor_drop(ptr {handle})"));
                    }
                    self.push_instr("call void @_mvl_actor_join_all()");
                }
                self.push_instr(MAIN_RET);
            } else if matches!(f.ret_ty, Ty::Unit) {
                self.push_instr("ret void");
            } else if let Some(val) = body_val {
                self.push_instr(&format!("ret {llvm_ret} {val}"));
            } else {
                self.push_instr(&format!("ret {llvm_ret} undef"));
            }
        }

        self.push_line("}");
        let body_text = self.finish_fn_body();
        self.module.fn_bodies.push(body_text);
        Ok(())
    }

    /// Flush `fn_ctx.fn_buf` into a single string, injecting any deferred
    /// `pre_allocas` right after the `entry:` label so they dominate all uses
    /// even when the binding is inside a branch (#1645).
    pub(super) fn finish_fn_body(&mut self) -> String {
        let pre = std::mem::take(&mut self.fn_ctx.pre_allocas);
        if pre.is_empty() {
            return self.fn_ctx.fn_buf.join("\n");
        }
        // fn_buf layout: ["define ...", "{", "entry:", ...instructions...]
        // Insert pre_allocas right after the "entry:" label.
        let entry_idx = self
            .fn_ctx
            .fn_buf
            .iter()
            .position(|l| l == "entry:")
            .map(|i| i + 1)
            .unwrap_or(self.fn_ctx.fn_buf.len());
        let mut buf = Vec::with_capacity(self.fn_ctx.fn_buf.len() + pre.len());
        buf.extend_from_slice(&self.fn_ctx.fn_buf[..entry_idx]);
        buf.extend(pre);
        buf.extend_from_slice(&self.fn_ctx.fn_buf[entry_idx..]);
        buf.join("\n")
    }
}
