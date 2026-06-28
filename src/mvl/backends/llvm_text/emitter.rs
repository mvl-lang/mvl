// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `LlvmTextCompiler` — pure-string LLVM IR emitter (Phase 2, issue #1136).
//!
//! Extends Phase 1 with: string literals, println/assert/format builtins,
//! struct construction/field access, unit enums, match expressions,
//! method calls (to_string/len/concat), and for-range loops.

use std::collections::HashMap;

use super::context::{FnCtx, ModuleCtx, MonoQueue};
use super::BuiltinSymbolInfo;
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::TirProgram;
use crate::mvl::parser::ast::{Decl, FnDecl, Program, Stmt, TypeBody, TypeExpr, VariantFields};
use crate::mvl::parser::lexer::Span;

// ── Public API ────────────────────────────────────────────────────────────────

/// Pure-string LLVM IR compiler — Phase 2.
///
/// Generates LLVM IR text for programs using primitives, strings, structs,
/// unit enums, match, and method calls.  No inkwell, no unsafe.
pub struct LlvmTextCompiler {
    /// Target triple emitted in the module header.
    pub target_triple: String,
    /// `builtin fn` dispatch table: MVL name → [`BuiltinSymbolInfo`].
    ///
    /// Populated via `with_context()` or set directly before calling
    /// `compile_to_ir_with_prelude`.  The emitter uses this to route calls to
    /// `builtin fn` declarations to their C runtime symbols instead of generating
    /// a body for the empty block.
    pub builtin_symbols: HashMap<String, BuiltinSymbolInfo>,
    /// Checker-resolved expression types, keyed by source span.
    ///
    /// When populated (via [`crate::mvl::checker::check`] in the CLI pipeline),
    /// the emitter can look up accurate types for any expression by span instead
    /// of relying on AST-based type inference.  This enables type-dependent
    /// dispatch (IFC labels, generic resolution, method dispatch).
    pub expr_types: HashMap<Span, Ty>,
}

impl LlvmTextCompiler {
    /// Create a new compiler with the host target triple.
    pub fn new() -> Self {
        Self {
            target_triple: default_target_triple(),
            builtin_symbols: HashMap::new(),
            expr_types: HashMap::new(),
        }
    }

    /// Create a compiler pre-populated with builtin dispatch table and
    /// checker-resolved expression types.
    pub fn with_context(
        builtin_symbols: HashMap<String, BuiltinSymbolInfo>,
        expr_types: HashMap<Span, Ty>,
    ) -> Self {
        Self {
            target_triple: default_target_triple(),
            builtin_symbols,
            expr_types,
        }
    }

    /// Compile a MVL [`Program`] to LLVM IR text (no prelude, no builtin dispatch).
    pub fn compile_to_ir(&self, prog: &Program, module_name: &str) -> Result<String, String> {
        self.compile_to_ir_with_prelude(&[], prog, module_name)
    }

    /// Compile prelude programs merged with `prog` into a single LLVM IR module.
    ///
    /// Prelude programs are emitted first (stdlib bodies), then `prog`.
    /// `builtin_symbols` is pre-populated into the emitter so that calls to
    /// `builtin fn` names are routed to their C-ABI symbols automatically.
    ///
    /// Non-builtin extension methods in prelude programs (e.g. `String::contains`,
    /// `String::starts_with`) are stripped before emission — they are handled
    /// via hardcoded C-ABI dispatch in `emit_method_call` and must not be
    /// emitted as MVL bodies (they reference unsupported stdlib constructs).
    pub fn compile_to_ir_with_prelude(
        &self,
        prelude: &[Program],
        prog: &Program,
        module_name: &str,
    ) -> Result<String, String> {
        let mut emitter =
            TextEmitter::new_with_builtins(module_name, &self.target_triple, &self.builtin_symbols);
        emitter.set_expr_types(self.expr_types.clone());
        for p in prelude {
            let stripped = strip_prelude_extension_methods(p);
            emitter.emit_program(&stripped)?;
        }
        emitter.emit_program(prog)?;
        Ok(emitter.finish())
    }

    /// TIR-walking entry point (#1612, Phase 3b PR 1).
    ///
    /// Parallel to [`Self::compile_to_ir_with_prelude`] but consumes already-lowered
    /// [`TirProgram`]s. This is the destination implementation for the MVL self-hosting
    /// port — the emitter does not need to walk AST at all.
    ///
    /// During the parallel-tree migration, this path is used by the
    /// `cross_backend_tir` test target to diff its output against the AST path
    /// across the corpus.
    pub fn compile_to_ir_tir(
        &self,
        prog: &TirProgram,
        module_name: &str,
    ) -> Result<String, String> {
        self.compile_to_ir_with_prelude_tir(&[], prog, module_name)
    }

    /// Like [`Self::compile_to_ir_with_prelude`] but consumes [`TirProgram`]s.
    pub fn compile_to_ir_with_prelude_tir(
        &self,
        prelude: &[TirProgram],
        prog: &TirProgram,
        module_name: &str,
    ) -> Result<String, String> {
        let mut emitter =
            TextEmitter::new_with_builtins(module_name, &self.target_triple, &self.builtin_symbols);
        emitter.set_expr_types(self.expr_types.clone());
        for p in prelude {
            let stripped = strip_prelude_extension_methods_tir(p);
            emitter.emit_program_tir(&stripped)?;
        }
        emitter.emit_program_tir(prog)?;
        Ok(emitter.finish())
    }
}

impl Default for LlvmTextCompiler {
    fn default() -> Self {
        Self::new()
    }
}

// Pure-MVL stdlib functions replaced by direct C-ABI dispatch in the LLVM backend.
// Their bodies use while-loops / iterators that violate SSA dominance when lowered
// naively, so we strip them and emit custom dispatch arms instead.
const STDLIB_REPLACED_BY_DISPATCH: &[&str] = &[
    // std.time — replaced by _mvl_time_format_datetime / _mvl_time_format_instant
    "format_datetime",
    "format_instant",
    "instant_to_datetime",
    "epoch_secs_to_datetime",
    "dt_digit",
    "dt_pad2",
    "dt_pad4",
    // std.regex — replaced by _mvl_regex_find_all / _mvl_regex_replace
    "find_all",
    "replace",
];

/// Remove prelude function bodies that the llvm_text emitter cannot handle.
///
/// Strips non-builtin functions that fall into either category:
///
/// 1. **Extension methods** (`receiver_type.is_some()`) — e.g. `String::contains`,
///    `List::is_empty`.  These call other String/List kernel methods via patterns
///    the emitter cannot yet fully lower (e.g. `self.find(sub).is_some()`).
///    Method calls on these types are handled via hardcoded C-ABI dispatch in
///    `emit_method_call` instead.
///
/// 2. **Option/Result return types** — prelude functions like `env.get_secret` or
///    `env.env_var` that return `Option[T]` or `Result[T, E]` may call runtime
///    symbols not available in the lli runtime.  User-defined functions with
///    Option/Result are handled correctly by the emitter — this filter only
///    applies to prelude functions.
fn strip_prelude_extension_methods(prog: &Program) -> Program {
    let mut out = prog.clone();
    out.declarations.retain(|d| {
        if let Decl::Fn(fd) = d {
            if fd.is_builtin {
                return true; // keep builtin declarations
            }
            // Drop non-builtin extension methods.
            if fd.receiver_type.is_some() {
                return false;
            }
            // Drop stdlib functions replaced by direct C-ABI dispatch (#1202).
            if STDLIB_REPLACED_BY_DISPATCH.contains(&fd.name.as_str()) {
                return false;
            }
            // Drop non-builtin prelude functions whose return type is Option or
            // Result — they may call runtime symbols not available in lli.
            if return_type_needs_option_abi(&fd.return_type) {
                return false;
            }
        }
        true
    });
    out
}

/// TIR variant of [`strip_prelude_extension_methods`] — walks `TirFn`s.
///
/// Mirrors the filtering rules of the AST version: drops non-builtin extension
/// methods, stdlib functions replaced by C-ABI dispatch, and non-builtin prelude
/// functions returning `Option`/`Result`.
fn strip_prelude_extension_methods_tir(prog: &TirProgram) -> TirProgram {
    let mut out = prog.clone();
    out.fns.retain(|f| {
        if f.is_builtin {
            return true;
        }
        if f.receiver_type.is_some() {
            return false;
        }
        if STDLIB_REPLACED_BY_DISPATCH.contains(&f.original_name.as_str()) {
            return false;
        }
        if return_type_needs_option_abi_ty(&f.ret_ty) {
            return false;
        }
        true
    });
    out
}

/// Return `true` if `ty` is `Option[_]` or `Result[_, _]` (TIR-side variant
/// of [`return_type_needs_option_abi`]).
fn return_type_needs_option_abi_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Option(_) | Ty::Result(_, _) => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            return_type_needs_option_abi_ty(inner)
        }
        _ => false,
    }
}

/// Return `true` if `ty` is `Option[_]` or `Result[_, _]` (possibly wrapped
/// in `Labeled` or `Refined`).  Used to skip prelude functions that may
/// reference runtime symbols unavailable in the lli runtime.
fn return_type_needs_option_abi(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Option { .. } | TypeExpr::Result { .. } => true,
        TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
            return_type_needs_option_abi(inner)
        }
        TypeExpr::Ref { inner, .. } => return_type_needs_option_abi(inner),
        _ => false,
    }
}

// ── Internal emitter ──────────────────────────────────────────────────────────

/// LLVM type for `Result[T, E]` tagged unions (discriminant byte + payload pointer).
const RESULT_LLVM_TY: &str = "{ i8, ptr }";
/// LLVM return instruction for the C-ABI `main` entry point.
const MAIN_RET: &str = "ret i32 0";

/// LLVM IR emitter (#1523).
///
/// State is split into three composable parts:
/// - [`ModuleCtx`] — module-global state (output buffers, type registries,
///   helper flags). Lives for the whole compilation unit.
/// - [`FnCtx`] — per-function state (SSA bookkeeping, locals). Replaced
///   wholesale at the start of every [`emit_fn`](Self::emit_fn) — this
///   eliminates the error-prone manual `reset_fn_state` method.
/// - [`MonoQueue`] — generic-function discovery + emission queue.
pub(super) struct TextEmitter {
    pub(super) module: ModuleCtx,
    pub(super) fn_ctx: FnCtx,
    pub(super) mono: MonoQueue,
}

/// Tracks heap-allocated value types for automatic drop emission.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HeapKind {
    String,
    Array,
    Map,
}

#[derive(Clone)]
pub(super) struct RefLocal {
    pub ptr: String,
    pub elem_ty: TypeExpr,
}

impl TextEmitter {
    #[allow(dead_code)]
    fn new(module_name: &str, target_triple: &str) -> Self {
        Self::new_with_builtins(module_name, target_triple, &HashMap::new())
    }

    /// Construct with pre-populated builtin dispatch table.
    ///
    /// `builtin_map` entries are pre-loaded into `fn_ret_types` / `fn_param_types`
    /// so call sites see the correct LLVM types even when the prelude doesn't
    /// include `builtin fn` declarations (e.g. RUST_BACKED_STDLIB modules strip them).
    pub(super) fn new_with_builtins(
        module_name: &str,
        target_triple: &str,
        builtin_map: &HashMap<String, BuiltinSymbolInfo>,
    ) -> Self {
        Self {
            module: ModuleCtx::new(module_name, target_triple, builtin_map),
            fn_ctx: FnCtx::initial(),
            mono: MonoQueue::new(),
        }
    }

    /// Mutator used by [`LlvmTextCompiler::compile_to_ir_with_prelude`] to
    /// install checker-resolved expression types after construction.
    pub(super) fn set_expr_types(&mut self, expr_types: HashMap<Span, Ty>) {
        self.module.expr_types = expr_types;
    }

    /// Emit a nested top-level function (actor method, dispatch fn, lambda,
    /// closure trampoline) with a fresh [`FnCtx`]. The outer [`FnCtx`] is
    /// swapped out via [`std::mem::replace`] and restored on both success and
    /// error paths, so per-function state cannot leak between the outer and
    /// nested emission contexts (#1535).
    ///
    /// The closure is responsible for pushing the completed IR text to
    /// `self.module.fn_bodies` before returning — only the [`FnCtx`] is
    /// scoped by this helper, not the module-level output.
    pub(super) fn with_fresh_fn_ctx<R, E>(
        &mut self,
        ret_ty: TypeExpr,
        f: impl FnOnce(&mut Self) -> Result<R, E>,
    ) -> Result<R, E> {
        let saved = std::mem::replace(&mut self.fn_ctx, FnCtx::new(ret_ty));
        let result = f(self);
        self.fn_ctx = saved;
        result
    }

    // ── Finalise ──────────────────────────────────────────────────────────

    fn finish(self) -> String {
        let m = &self.module;
        let mut out = String::new();
        out.push_str(&format!("; ModuleID = '{}'\n", m.module_name));
        out.push_str(&format!("source_filename = \"{}\"\n", m.module_name));
        out.push_str(&format!("target triple = \"{}\"\n", m.target_triple));
        for td in &m.type_defs {
            out.push('\n');
            out.push_str(td);
        }
        for sg in &m.str_globals {
            out.push('\n');
            out.push_str(sg);
        }
        for body in &m.fn_bodies {
            out.push('\n');
            out.push_str(body);
        }
        for ext in &m.extern_decls {
            out.push('\n');
            out.push_str(ext);
        }
        out
    }

    // ── Counter helpers ───────────────────────────────────────────────────

    pub(super) fn next_reg(&mut self) -> String {
        let n = self.fn_ctx.reg;
        self.fn_ctx.reg += 1;
        format!("%t{n}")
    }

    pub(super) fn next_bb(&mut self, prefix: &str) -> String {
        let n = self.fn_ctx.bb;
        self.fn_ctx.bb += 1;
        format!("{prefix}_{n}")
    }

    // ── Instruction helpers ───────────────────────────────────────────────

    pub(super) fn push_line(&mut self, line: &str) {
        self.fn_ctx.fn_buf.push(line.to_string());
    }

    pub(super) fn push_instr(&mut self, instr: &str) {
        self.fn_ctx.fn_buf.push(format!("  {instr}"));
    }

    pub(super) fn start_bb(&mut self, label: &str) {
        self.fn_ctx.fn_buf.push(format!("{label}:"));
        self.fn_ctx.current_bb = label.to_string();
        self.fn_ctx.terminated = false;
    }

    // ── Extern declaration helpers ────────────────────────────────────────

    pub(super) fn ensure_extern(&mut self, decl: &str) {
        if !self.module.extern_decls.iter().any(|d| d == decl) {
            self.module.extern_decls.push(decl.to_string());
        }
    }

    /// Ensure `@_mvl_yield_check` is declared once per module (#1181).
    ///
    /// Called at every loop back-edge insertion point. The flag avoids repeated
    /// linear scans through `extern_decls`.
    pub(super) fn ensure_yield_check_extern(&mut self) {
        if !self.module.yield_check_declared {
            self.ensure_extern("declare void @_mvl_yield_check()");
            self.module.yield_check_declared = true;
        }
    }

    // ── Program emission ──────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) -> Result<(), String> {
        // Pre-pass: register all enums so that struct field type resolution
        // via `llvm_ty_ctx` can see enum types regardless of declaration order.
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                if let TypeBody::Enum(variants) = &td.body {
                    let variant_names: Vec<String> =
                        variants.iter().map(|v| v.name.clone()).collect();
                    let variant_fields: Vec<Vec<TypeExpr>> = variants
                        .iter()
                        .map(|v| match &v.fields {
                            VariantFields::Tuple(tys) => tys.clone(),
                            VariantFields::Struct(fields) => {
                                fields.iter().map(|f| f.ty.clone()).collect()
                            }
                            VariantFields::Unit => Vec::new(),
                        })
                        .collect();
                    // Register field names for struct variants so emit_construct
                    // can reorder named fields to declaration order (#1357).
                    for v in variants {
                        if let VariantFields::Struct(fields) = &v.fields {
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
        }

        // First pass: register all function return types and type declarations.
        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) => {
                    let ret = fd.return_type.as_ref().clone();
                    let params: Vec<TypeExpr> = fd.params.iter().map(|p| p.ty.clone()).collect();
                    // Register under the short name (e.g. `from_chars`)
                    self.module
                        .fn_ret_types
                        .insert(fd.name.clone(), ret.clone());
                    self.module
                        .fn_param_types
                        .insert(fd.name.clone(), params.clone());
                    // Also register under the qualified name (e.g. `String::from_chars`)
                    // so that static call-site lookups like `fn_ret_types["String::from_chars"]`
                    // resolve correctly.
                    if let Some(recv) = &fd.receiver_type {
                        let qualified = format!("{}::{}", recv, fd.name);
                        self.module
                            .fn_ret_types
                            .insert(qualified.clone(), ret.clone());
                        self.module.fn_param_types.insert(qualified, params);
                    }
                }
                Decl::Type(td) => match &td.body {
                    TypeBody::Struct { fields, .. } => {
                        // Zero-field structs are opaque handles (e.g. `Instant = struct {}`).
                        // Treat them as `ptr` instead of registering a named struct type so
                        // that C-ABI functions returning `*mut c_void` are typed correctly.
                        if fields.is_empty() {
                            // Don't register — llvm_ty_ctx falls back to "ptr" for unknown names.
                        } else {
                            let field_list: Vec<(String, TypeExpr)> = fields
                                .iter()
                                .map(|f| (f.name.clone(), f.ty.clone()))
                                .collect();
                            // Emit type definition: %Name = type { ty0, ty1, ... }
                            // Use llvm_ty_ctx to resolve enum/struct field types correctly.
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
                    }
                    // Enums already registered in pre-pass above.
                    TypeBody::Enum(_) => {}
                    TypeBody::Alias(inner) => {
                        // Register fn-type aliases so indirect calls through
                        // an aliased type (e.g. `d: Dispatcher`) can resolve
                        // to their underlying `Fn` signature (#1467 LLVM port).
                        if matches!(inner.as_ref(), TypeExpr::Fn { .. }) {
                            self.module
                                .fn_aliases
                                .insert(td.name.clone(), (**inner).clone());
                        }
                    }
                },
                Decl::Actor(ad) => {
                    let state_name = format!("{}State", ad.name);
                    let field_list: Vec<(String, TypeExpr)> = ad
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
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
                    self.module.actor_decls.insert(ad.name.clone(), ad.clone());
                }
                Decl::Extern(ed) if ed.abi == "c" => {
                    // Emit LLVM `declare` for each extern "c" function (#811).
                    // These are resolved at link time from a loaded shared library
                    // (e.g. `lli --load=libpkg_foo.{dylib,so}`).
                    for lib in &ed.link_libs {
                        self.ensure_extern(&format!("; link: {lib}"));
                    }
                    for ef in &ed.fns {
                        let ret_ty = Self::llvm_ty(&ef.return_type);
                        let param_tys: Vec<String> =
                            ef.params.iter().map(|p| Self::llvm_ty(&p.ty)).collect();
                        let decl =
                            format!("declare {} @{}({})", ret_ty, ef.name, param_tys.join(", "));
                        self.ensure_extern(&decl);
                        // Register return type and param types so call emission works.
                        self.module
                            .fn_ret_types
                            .insert(ef.name.clone(), ef.return_type.as_ref().clone());
                        self.module.fn_param_types.insert(
                            ef.name.clone(),
                            ef.params.iter().map(|p| p.ty.clone()).collect(),
                        );
                    }
                }
                Decl::Relabel(rd) if rd.audit => {
                    // Track declaration-level `audit` relabels so call sites of
                    // these transitions emit a runtime audit event even when the
                    // expression itself does not carry `audit` (#896, #1554).
                    self.module
                        .audit_relabels
                        .insert(rd.name.clone(), (rd.from.clone(), rd.to.clone()));
                }
                _ => {}
            }
        }

        // Actor pass: emit behavior functions + dispatch for each actor.
        // Dedupe across `emit_program` calls — the pass runs once per program
        // (prelude + user) and `actor_decls` accumulates, so without
        // `actor_emitted` std.actors actors would be emitted N times (#1610).
        if !self.module.actor_decls.is_empty() {
            self.ensure_actor_runtime_externs();
            let actor_names: Vec<String> = self.module.actor_decls.keys().cloned().collect();
            for name in actor_names {
                if !self.module.actor_emitted.insert(name.clone()) {
                    continue;
                }
                let ad = self.module.actor_decls[&name].clone();
                self.emit_actor_decl(&ad)?;
            }
        }

        // Collect generic function declarations for on-demand monomorphization.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.type_params.is_empty() {
                    self.mono.generic_fns.insert(fd.name.clone(), fd.clone());
                }
            }
        }

        // Second pass: emit each non-generic function body.
        // Skip: test fns, builtin fns (no MVL body — dispatched via C-ABI),
        //        and generic fns (monomorphized lazily when called).
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test && !fd.is_builtin && fd.type_params.is_empty() {
                    self.emit_fn(fd)?;
                }
            }
        }

        // Third pass: emit monomorphized copies queued during call emission.
        // Loop because a monomorphized body may itself call another generic fn.
        const MONO_LIMIT: usize = 10_000;
        let mut mono_iterations = 0usize;
        while !self.mono.mono_queue.is_empty() {
            mono_iterations += 1;
            if mono_iterations > MONO_LIMIT {
                return Err(
                    "monomorphization limit exceeded — possible infinite instantiation".into(),
                );
            }
            let queue = std::mem::take(&mut self.mono.mono_queue);
            for (mangled, orig_name, concrete_types) in queue {
                let gfd = match self.mono.generic_fns.get(&orig_name) {
                    Some(fd) => fd.clone(),
                    None => continue,
                };
                // Set up type parameter → concrete type mapping.
                for (tp, ct) in gfd.type_params.iter().zip(concrete_types.iter()) {
                    self.mono
                        .type_param_map
                        .insert(tp.name().to_string(), ct.clone());
                }
                // Emit the function under its mangled name.
                let mut fd = gfd;
                fd.name = mangled;
                fd.type_params.clear();
                self.emit_fn(&fd)?;
                self.mono.type_param_map.clear();
            }
        }

        Ok(())
    }

    // ── Function emission ─────────────────────────────────────────────────

    pub(super) fn emit_fn(&mut self, fd: &FnDecl) -> Result<(), String> {
        let ret_ty = fd.return_type.as_ref();
        // Replace the per-fn context wholesale — the old state is dropped here,
        // making it impossible to forget resetting a field (#1523).
        self.fn_ctx = FnCtx::new(ret_ty.clone());
        self.fn_ctx.current_fn_is_main = fd.name == "main";

        let params: Vec<String> = fd
            .params
            .iter()
            .filter_map(|p| {
                let ty_str = self.llvm_ty_ctx(&p.ty);
                if ty_str == "void" {
                    None
                } else {
                    Some(format!("{ty_str} %{}", p.name))
                }
            })
            .collect();
        let params_str = params.join(", ");

        let llvm_ret = self.llvm_ty_ctx(ret_ty);

        let sig = if self.fn_ctx.current_fn_is_main {
            "define i32 @main()".to_string()
        } else {
            format!(
                "define {llvm_ret} @{fn_name}({params_str})",
                fn_name = fd.name
            )
        };

        self.push_line(&sig);
        self.push_line("{");
        self.push_line("entry:");

        // Bind parameters as SSA locals, track MVL types for struct access
        for p in &fd.params {
            let ty_str = self.llvm_ty_ctx(&p.ty);
            if ty_str != "void" {
                let ssa = format!("%{}", p.name);
                self.fn_ctx.locals.insert(p.name.clone(), ssa.clone());
                self.fn_ctx.reg_types.insert(ssa, ty_str);
                self.fn_ctx
                    .local_mvl_types
                    .insert(p.name.clone(), p.ty.clone());
            }
        }

        let body_val = self.emit_block(&fd.body)?;

        if !self.fn_ctx.terminated {
            // If the function returns a heap-allocated local, exclude it from
            // drops — ownership transfers to the caller (move semantics).
            if let Some(Stmt::Expr { expr, .. }) = fd.body.stmts.last() {
                self.exclude_returned_value(expr);
            }
            self.emit_heap_drops();
            if self.fn_ctx.current_fn_is_main {
                if !self.module.actor_decls.is_empty() {
                    // Drop each handle to close the sender — this signals the
                    // actor thread's recv loop to exit.
                    for handle in std::mem::take(&mut self.fn_ctx.spawned_actor_handles) {
                        self.push_instr(&format!("call void @_mvl_actor_drop(ptr {handle})"));
                    }
                    self.push_instr("call void @_mvl_actor_join_all()");
                }
                self.push_instr(MAIN_RET);
            } else if Self::is_void(ret_ty) {
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

// ── Submodules (split from monolithic emitter.rs) ─────────────────────────────
//
// Each submodule uses `#[path]` to remain a child of this module, giving
// access to `TextEmitter`'s private fields without any visibility changes.

#[path = "emit_types.rs"]
mod emit_types;

#[path = "emit_stmts.rs"]
mod emit_stmts;

#[path = "emit_exprs.rs"]
mod emit_exprs;

#[path = "emit_construct.rs"]
mod emit_construct;

#[path = "emit_mono.rs"]
mod emit_mono;

#[path = "emit_method_call.rs"]
mod emit_method_call;

#[path = "emit_closures.rs"]
mod emit_closures;

// ── TIR-walking parallel implementation (#1612, Phase 3b PR 1) ────────────────
//
// These submodules implement a parallel emitter that walks [`TirProgram`] instead
// of [`Program`]. Built leaf-first (see ADR-0050) so each commit compiles. When
// IR parity is reached across the corpus, the AST modules above will be deleted.

#[path = "emit_program_tir.rs"]
mod emit_program_tir;

#[path = "emit_exprs_tir.rs"]
mod emit_exprs_tir;

#[path = "emit_stmts_tir.rs"]
mod emit_stmts_tir;

// ── Target triple ─────────────────────────────────────────────────────────────

fn default_target_triple() -> String {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    return "arm64-apple-darwin".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    return "x86_64-pc-linux-gnu".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    return "x86_64-apple-darwin".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    return "x86_64-pc-windows-msvc".to_string();
    #[allow(unreachable_code)]
    "x86_64-pc-linux-gnu".to_string()
}

// ── Actor emission submodule ──────────────────────────────────────────────────

// `emit_actors.rs` lives in the same directory as `emitter.rs`.  Using
// `#[path]` keeps it a child of `emitter` so that the private types
// `TextEmitter` and `RefLocal` (and their private fields) remain accessible
// without any visibility changes.
#[path = "emit_actors.rs"]
mod emit_actors;

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "emitter_tests.rs"]
mod tests;
