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
    TirExpr, TirExprKind, TirFn, TirProgram, TirStmt, TirTypeBody, TirTypeDecl, TirVariantFields,
    Ty, TypeExpr,
};
use crate::mvl::parser::lexer::Span;
use std::collections::HashSet;

use super::emit_helpers::ty_to_type_expr;
use super::{TextEmitter, MAIN_RET};

/// Compute the transitive closure of functions that call (directly or indirectly)
/// any name in `rust_extern_names`. Returns a set of function names to EXCLUDE
/// from the test crate so lli's eager JIT never encounters unresolvable symbols.
///
/// Fixed-point iteration: start with direct callers, then expand to callers of
/// callers until no new functions are added.
fn compute_extern_rust_exclusion_set(
    prog: &TirProgram,
    rust_extern_names: &HashSet<String>,
) -> HashSet<String> {
    let mut excluded: HashSet<String> = HashSet::new();
    // Seed: direct callers of extern-rust functions.
    for f in &prog.fns {
        if fn_calls_any(&f.body.stmts, rust_extern_names) {
            excluded.insert(f.name.clone());
        }
    }
    // Expand until fixed point.
    loop {
        let prev = excluded.len();
        for f in &prog.fns {
            if !excluded.contains(&f.name) && fn_calls_any(&f.body.stmts, &excluded) {
                excluded.insert(f.name.clone());
            }
        }
        if excluded.len() == prev {
            break;
        }
    }
    excluded
}

/// Returns true if any statement in `stmts` (or nested exprs) contains a
/// direct `FnCall` to any name in `targets`. Used to detect production functions
/// that call extern-rust symbols so they can be excluded from test crates.
fn fn_calls_any(stmts: &[TirStmt], targets: &HashSet<String>) -> bool {
    stmts.iter().any(|s| stmt_calls_any(s, targets))
}

fn else_calls_any(else_: &crate::mvl::ir::TirElseBranch, targets: &HashSet<String>) -> bool {
    match else_ {
        crate::mvl::ir::TirElseBranch::Block(b) => fn_calls_any(&b.stmts, targets),
        crate::mvl::ir::TirElseBranch::If(s) => stmt_calls_any(s, targets),
    }
}

fn match_body_calls_any(body: &crate::mvl::ir::TirMatchBody, targets: &HashSet<String>) -> bool {
    match body {
        crate::mvl::ir::TirMatchBody::Expr(e) => expr_calls_any(e, targets),
        crate::mvl::ir::TirMatchBody::Block(b) => fn_calls_any(&b.stmts, targets),
    }
}

fn stmt_calls_any(stmt: &TirStmt, targets: &HashSet<String>) -> bool {
    match stmt {
        TirStmt::Let { init, .. } => expr_calls_any(init, targets),
        TirStmt::Assign { value, .. } => expr_calls_any(value, targets),
        TirStmt::Return { value, .. } => {
            value.as_ref().is_some_and(|e| expr_calls_any(e, targets))
        }
        TirStmt::Expr { expr, .. } => expr_calls_any(expr, targets),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            expr_calls_any(cond, targets)
                || fn_calls_any(&then.stmts, targets)
                || else_.as_ref().is_some_and(|b| else_calls_any(b, targets))
        }
        TirStmt::While { cond, body, .. } => {
            expr_calls_any(cond, targets) || fn_calls_any(&body.stmts, targets)
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_any(scrutinee, targets)
                || arms.iter().any(|a| match_body_calls_any(&a.body, targets))
        }
        TirStmt::For { iter, body, .. } => {
            expr_calls_any(iter, targets) || fn_calls_any(&body.stmts, targets)
        }
    }
}

fn expr_calls_any(expr: &TirExpr, targets: &HashSet<String>) -> bool {
    match &expr.kind {
        TirExprKind::FnCall { name, args, .. } => {
            targets.contains(name.as_str()) || args.iter().any(|a| expr_calls_any(a, targets))
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            expr_calls_any(receiver, targets) || args.iter().any(|a| expr_calls_any(a, targets))
        }
        TirExprKind::Binary { left, right, .. } => {
            expr_calls_any(left, targets) || expr_calls_any(right, targets)
        }
        TirExprKind::Unary { expr: inner, .. }
        | TirExprKind::Propagate(inner)
        | TirExprKind::Consume(inner)
        | TirExprKind::Borrow { expr: inner, .. }
        | TirExprKind::FieldAccess { expr: inner, .. }
        | TirExprKind::Relabel { expr: inner, .. } => expr_calls_any(inner, targets),
        TirExprKind::Block(block) => fn_calls_any(&block.stmts, targets),
        TirExprKind::If {
            cond, then, else_, ..
        } => {
            expr_calls_any(cond, targets)
                || fn_calls_any(&then.stmts, targets)
                || else_.as_ref().is_some_and(|e| expr_calls_any(e, targets))
        }
        TirExprKind::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_any(scrutinee, targets)
                || arms.iter().any(|a| match_body_calls_any(&a.body, targets))
        }
        TirExprKind::Construct { fields, .. } => {
            fields.iter().any(|(_, e)| expr_calls_any(e, targets))
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            elems.iter().any(|e| expr_calls_any(e, targets))
        }
        TirExprKind::Map { pairs } => pairs
            .iter()
            .any(|(k, v)| expr_calls_any(k, targets) || expr_calls_any(v, targets)),
        TirExprKind::Spawn { fields, .. } => fields.iter().any(|(_, e)| expr_calls_any(e, targets)),
        TirExprKind::Lambda { body, .. } => expr_calls_any(body, targets),
        TirExprKind::Select { arms } => arms.iter().any(|a| fn_calls_any(&a.body.stmts, targets)),
        // Leaf nodes — no nested calls
        TirExprKind::Literal(_) | TirExprKind::Var(_) | TirExprKind::Quantifier(_) => false,
    }
}

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
    /// Register types and function signatures from `prog` without emitting bodies.
    ///
    /// Used for sibling modules that have `extern "rust"` blocks: their function
    /// bodies call Rust-ABI externs that the LLVM emitter cannot declare correctly,
    /// so we only expose their types and signatures to the test crate. This gives
    /// call sites correct type information while avoiding broken IR from bodies
    /// that call undeclared extern-rust functions.
    pub(super) fn emit_program_tir_types_and_sigs(&mut self, prog: &TirProgram) {
        // Register enum variants (needed for discriminant lookups at call sites).
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
                self.module
                    .enum_variants
                    .insert(td.name.clone(), variant_names);
                self.module
                    .enum_variant_fields
                    .insert(td.name.clone(), variant_fields);
            }
        }
        // Register struct fields and type aliases into the lookup registries so
        // ty_to_llvm_ctx returns correct types (%Struct, i64, etc.) when pass 2
        // emits function bodies. Critically, do NOT push to `module.type_defs`
        // (a Vec) here — that is done once by emit_program_tir in pass 2.
        // Pushing twice would produce LLVM IR "redefinition of type" errors.
        for td in &prog.types {
            match &td.body {
                TirTypeBody::Struct { fields, .. } => {
                    if fields.is_empty() {
                        continue;
                    }
                    let field_list: Vec<(String, TypeExpr)> = fields
                        .iter()
                        .map(|f| (f.name.clone(), ty_to_type_expr_or_unit(&f.ty)))
                        .collect();
                    // struct_fields is a HashMap — insert is idempotent.
                    self.module
                        .struct_fields
                        .insert(td.name.clone(), field_list);
                }
                TirTypeBody::Alias(inner) => {
                    let inner_te = ty_to_type_expr_or_unit(inner);
                    if matches!(inner_te, TypeExpr::Fn { .. }) {
                        self.module.fn_aliases.insert(td.name.clone(), inner_te);
                    } else {
                        self.module
                            .type_aliases
                            .insert(td.name.clone(), inner.clone());
                    }
                }
                TirTypeBody::Enum(_) => {} // already handled in the pre-pass above
            }
        }
        // Register function signatures so call sites emit correct types.
        for f in &prog.fns {
            if f.type_params.is_empty() {
                self.register_fn_tir_sig(f);
            }
        }
    }

    /// Emit a sibling module that has `extern "rust"` blocks: type definitions
    /// are always emitted, and function bodies are emitted only for functions
    /// that do NOT directly call any extern-rust function.
    ///
    /// This lets pure helpers (e.g. `parse_level`) be available to test
    /// functions while excluding impure callers (e.g. `dispatch`) whose
    /// unresolvable extern symbols would cause lli JIT to fail.
    pub(super) fn emit_program_tir_without_extern_rust_callers(
        &mut self,
        prog: &TirProgram,
    ) -> Result<(), String> {
        let rust_extern_names: HashSet<String> = prog
            .externs
            .iter()
            .filter(|ed| ed.abi == "rust")
            .flat_map(|ed| ed.fns.iter().map(|ef| ef.name.clone()))
            .collect();
        let excluded = compute_extern_rust_exclusion_set(prog, &rust_extern_names);
        let mut filtered = prog.clone();
        filtered
            .fns
            .retain(|f| !f.is_test && !excluded.contains(&f.name));
        self.emit_program_tir(&filtered)
    }

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
        // Also emit opaque `declare` stubs for `extern "rust"` fns so their
        // callers produce valid IR (correct return type, not the i64 default).
        // lli validates the whole file statically — without these stubs, any
        // function that calls a rust-extern gets a type mismatch error that
        // rejects the whole IR file, even when the test being run never calls
        // that function. The stubs carry no body; lli's lazy JIT only resolves
        // them if the test actually calls the function at runtime.
        for ed in &prog.externs {
            if ed.abi != "c" && ed.abi != "rust" {
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
        // Only register the short name for free functions (no receiver).
        // Extension methods register ONLY their qualified name to avoid
        // clobbering unrelated free functions with the same short name
        // (e.g. String::find returning Option[Int] must not overwrite
        // the regex free-function find returning Option[Match]).
        if f.receiver_type.is_none() {
            self.module.fn_ret_types.insert(f.name.clone(), ret.clone());
            self.module
                .fn_param_types
                .insert(f.name.clone(), params.clone());
        }
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
                // Guard: push the type def only once. Two-pass sibling emission
                // calls register_type_decl_tir in both passes; the guard prevents
                // duplicate `%Foo = type { ... }` definitions that lli rejects.
                if self.module.emitted_type_def_names.insert(td.name.clone()) {
                    self.module.type_defs.push(format!(
                        "%{} = type {{ {} }}",
                        td.name,
                        field_types.join(", ")
                    ));
                }
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
                } else {
                    // #1851: register non-fn aliases (`type Port = Int where ...`,
                    // `type ShortString = String where len(self) < 256`) so
                    // both llvm_ty_ctx variants can resolve the alias name
                    // back to the underlying scalar in fn signatures and IR
                    // emission.
                    self.module
                        .type_aliases
                        .insert(td.name.clone(), inner.clone());
                }
            }
        }
    }

    /// Emit the body of a single [`TirFn`]. Mirrors `emit_fn(&FnDecl)`.
    pub(super) fn emit_fn_tir(&mut self, f: &TirFn) -> Result<(), String> {
        // Guard: skip functions whose bodies have already been emitted.
        // This prevents "invalid redefinition" when both a sibling module and
        // the entry test file define a function with the same name (e.g. a
        // shared helper `fn b` that appears in both mtf.mvl and mtf_test.mvl).
        if !self.module.emitted_fn_names.insert(f.name.clone()) {
            return Ok(());
        }

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
                    // `entry` conflicts with the `entry:` basic block label in LLVM's
                    // symbol table — rename to `entry_p` in the signature.
                    let pname = if p.name == "entry" {
                        "entry_p"
                    } else {
                        &p.name
                    };
                    Some(format!("{ty_str} %{pname}"))
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
        // Guard: if a parameter is named `entry`, it would shadow the `entry:` basic
        // block label in LLVM's symbol table, producing "unable to create block named
        // 'entry'" errors. Use `entry_p` as the SSA name in that case; any access to
        // the binding still goes through `fn_ctx.locals` so it's transparent.
        for p in &f.params {
            let ty_str = self.ty_to_llvm_ctx(&p.ty);
            if ty_str != "void" {
                let ssa_name = if p.name == "entry" {
                    "entry_p".to_string()
                } else {
                    p.name.clone()
                };
                let ssa = format!("%{ssa_name}");
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
                let has_actors = !self.module.tir_actor_decls.is_empty();
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

    /// Like [`emit_program_tir`] but emits `test fn` bodies as regular
    /// functions (dropping the `is_test` filter). Used by the test-crate
    /// path (`compile_to_ir_test_crate`) so the dispatch main can call each
    /// test fn by name via `lli <file.ll> <test_name>`.
    pub(super) fn emit_program_tir_test_crate(&mut self, prog: &TirProgram) -> Result<(), String> {
        // Programs with `extern "rust"` blocks may have production functions that
        // call Rust-ABI symbols not in the test runtime library. lli's ORC JIT
        // materializes all functions in the module when the first function is
        // called, so any unresolvable extern causes the whole module to fail even
        // when the test being run never calls that extern.
        //
        // Filter: keep test functions unconditionally; keep production functions
        // only if they do NOT directly call any extern-rust function. Pure helpers
        // like `parse_level` (called by tests) are kept; impure ones like `dispatch`
        // (which calls `verify_request_auth`) are dropped.
        let rust_extern_names: HashSet<String> = prog
            .externs
            .iter()
            .filter(|ed| ed.abi == "rust")
            .flat_map(|ed| ed.fns.iter().map(|ef| ef.name.clone()))
            .collect();
        let mut test_prog = prog.clone();
        if !rust_extern_names.is_empty() {
            let excluded = compute_extern_rust_exclusion_set(prog, &rust_extern_names);
            test_prog
                .fns
                .retain(|f| f.is_test || !excluded.contains(&f.name));
        }
        for f in &mut test_prog.fns {
            f.is_test = false;
        }
        self.emit_program_tir(&test_prog)
    }

    /// Emit a `define i32 @main(i32 %argc, i8** %argv)` that reads `argv[1]`
    /// and dispatches to the named test function using `strcmp`.
    ///
    /// Exit code 0 = test passed (function returned normally).
    /// The process aborts (SIGILL via `llvm.trap`) if `assert` / `assert_eq`
    /// fires inside the test — that maps to a non-zero exit code.
    /// Exit code 2 = unknown test name (argv[1] not matched).
    pub(super) fn emit_test_dispatch_main(&mut self, test_names: &[String]) {
        if test_names.is_empty() {
            return;
        }

        // Emit string constants for each test name (C strings, null-terminated).
        let name_globals: Vec<(String, usize)> = test_names
            .iter()
            .map(|n| {
                let g = self.emit_str_global(n);
                let len = n.len() + 1; // +1 null terminator
                (g, len)
            })
            .collect();

        self.ensure_extern("declare i32 @strcmp(ptr, ptr)");
        self.ensure_extern("declare void @exit(i32)");

        let mut body: Vec<String> = Vec::new();
        body.push("define i32 @main(i32 %argc, ptr %argv) {".to_string());
        body.push("entry:".to_string());
        // argv[1] = the test name (if argc >= 2).
        let arg_ptr = "%arg_ptr";
        body.push(format!(
            "  {arg_ptr} = getelementptr inbounds ptr, ptr %argv, i64 1"
        ));
        let arg = "%arg";
        body.push(format!("  {arg} = load ptr, ptr {arg_ptr}, align 8"));

        // For each test name: strcmp(argv[1], name) == 0 → call fn → ret 0.
        for (i, name) in test_names.iter().enumerate() {
            let (ref global, len) = name_globals[i];
            let name_ptr = format!("%name_{i}");
            body.push(format!(
                "  {name_ptr} = getelementptr inbounds [{len} x i8], ptr @{global}, i64 0, i64 0"
            ));
            let cmp = format!("%cmp_{i}");
            body.push(format!(
                "  {cmp} = call i32 @strcmp(ptr {arg}, ptr {name_ptr})"
            ));
            let eq = format!("%eq_{i}");
            body.push(format!("  {eq} = icmp eq i32 {cmp}, 0"));
            let then_lbl = format!("test_{i}_run");
            let next_lbl = if i + 1 < test_names.len() {
                format!("test_{}_check", i + 1)
            } else {
                "unknown_test".to_string()
            };
            body.push(format!(
                "  br i1 {eq}, label %{then_lbl}, label %{next_lbl}"
            ));
            body.push(format!("{then_lbl}:"));
            body.push(format!("  call void @{name}()"));
            body.push("  ret i32 0".to_string());
            if i + 1 < test_names.len() {
                body.push(format!("{}:", next_lbl));
            }
        }

        body.push("unknown_test:".to_string());
        body.push("  call void @exit(i32 2)".to_string());
        body.push("  unreachable".to_string());
        body.push("}".to_string());

        self.module.fn_bodies.push(body.join("\n"));
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
