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
use crate::mvl::parser::ast::{Program, TypeExpr};
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
    /// Internally lowers `prog` and each prelude to TIR (`mono::collect_fns` +
    /// `mono::monomorphize` + `ir::lower::lower`), then delegates to
    /// [`Self::compile_to_ir_with_prelude_tir`]. The legacy AST walker was
    /// deleted in #1612 Phase 3b PR 2 — this entry point survives only as an
    /// AST-input convenience for callers (CLI, tests) that haven't migrated.
    pub fn compile_to_ir_with_prelude(
        &self,
        prelude: &[Program],
        prog: &Program,
        module_name: &str,
    ) -> Result<String, String> {
        use crate::mvl::ir::lower;
        use crate::mvl::passes::mono;

        let entry_all_fns = mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
        let entry_mono = mono::monomorphize(prog, &entry_all_fns, &self.expr_types);
        let entry_tir = lower::lower(prog, &entry_mono, &self.expr_types);

        let prelude_tirs: Vec<TirProgram> = prelude
            .iter()
            .map(|p| {
                let fns = mono::collect_fns([p]);
                let m = mono::monomorphize(p, &fns, &self.expr_types);
                lower::lower(p, &m, &self.expr_types)
            })
            .collect();

        self.compile_to_ir_with_prelude_tir(&prelude_tirs, &entry_tir, module_name)
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
            // Stripped functions (e.g. find_all, replace) are absent from the
            // stripped program's `fns` list, so `emit_program_tir` never calls
            // `register_fn_tir_sig` for them.  Without an entry in `fn_ret_types`,
            // call sites default to `i64` return — wrong for ptr-returning fns.
            // Fix: register stripped signatures before body emission (#1645).
            for f in &p.fns {
                if stripped.fns.iter().all(|sf| sf.name != f.name) && f.type_params.is_empty() {
                    emitter.register_fn_tir_sig(f);
                }
            }
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

/// TIR variant of strip_prelude_extension_methods — walks `TirFn`s.
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

}

// ── Submodules ────────────────────────────────────────────────────────────────
//
// Each submodule uses `#[path]` to remain a child of this module, giving
// access to `TextEmitter`'s private fields without any visibility changes.

// Shared C-ABI dispatch helpers.
#[path = "c_call.rs"]
mod c_call;

// Shared low-level emit helpers — type mapping, heap-drop tracking, string
// globals, value conversion, closure infra, enum lookup, mangling, literals.
#[path = "emit_helpers.rs"]
mod emit_helpers;

// ── TIR-walking emitter (#1612, Phase 3b PR 2) ────────────────────────────────
//
// These submodules walk [`TirProgram`] and are the only emitter path. The
// AST-walking emit_*.rs modules were deleted in #1612 PR 2; the public AST
// entry point [`LlvmTextCompiler::compile_to_ir_with_prelude`] lowers AST →
// TIR internally and delegates to the TIR walker.

#[path = "emit_program_tir.rs"]
mod emit_program_tir;

#[path = "emit_exprs_tir.rs"]
mod emit_exprs_tir;

#[path = "emit_stmts_tir.rs"]
mod emit_stmts_tir;

#[path = "emit_closures_tir.rs"]
mod emit_closures_tir;

#[path = "emit_actors_tir.rs"]
mod emit_actors_tir;

#[path = "emit_mono_tir.rs"]
mod emit_mono_tir;

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

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "emitter_tests.rs"]
mod tests;
