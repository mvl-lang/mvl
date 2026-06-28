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

use crate::mvl::ir::{
    Ty, TirFn, TirProgram, TirTypeBody, TirTypeDecl, TirVariantFields, TypeExpr,
};
use crate::mvl::parser::lexer::Span;

use super::emit_stmts::ty_to_type_expr;
use super::{TextEmitter, MAIN_RET};

/// Convert a [`Ty`] back to a [`TypeExpr`] for use by AST-shaped helpers.
///
/// Falls back to `Unit` for `Ty` variants that don't have a clean `TypeExpr`
/// representation — these only appear in positions where the existing
/// emitter logic also treats them as opaque (e.g. `Ty::Session`).
fn ty_to_type_expr_or_unit(ty: &Ty) -> TypeExpr {
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
                let variant_names: Vec<String> =
                    variants.iter().map(|v| v.name.clone()).collect();
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
                        let names: Vec<String> =
                            fields.iter().map(|f| f.name.clone()).collect();
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
        for f in &prog.fns {
            self.register_fn_tir_sig(f);
        }
        for td in &prog.types {
            self.register_type_decl_tir(td);
        }

        // Actor pass — not yet wired up for the TIR path.
        if !prog.actors.is_empty() {
            return Err(
                "emit_program_tir: actors not yet implemented (#1612 in progress)".into(),
            );
        }

        // Emit each non-test, non-builtin function body. No generic collection
        // needed — `ir::lower::lower` already produced monomorphized copies.
        for f in &prog.fns {
            if !f.is_test && !f.is_builtin {
                self.emit_fn_tir(f)?;
            }
        }

        Ok(())
    }

    /// Register a [`TirFn`]'s signature into the module-level dispatch tables.
    fn register_fn_tir_sig(&mut self, f: &TirFn) {
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
            self.emit_heap_drops();
            if self.fn_ctx.current_fn_is_main {
                if !self.module.actor_decls.is_empty() {
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
        let body_text = self.fn_ctx.fn_buf.join("\n");
        self.module.fn_bodies.push(body_text);
        Ok(())
    }
}
