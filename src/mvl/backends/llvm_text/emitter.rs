// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `LlvmTextCompiler` — pure-string LLVM IR emitter (Phase 2, issue #1136).
//!
//! Extends Phase 1 with: string literals, println/assert/format builtins,
//! struct construction/field access, unit enums, match expressions,
//! method calls (to_string/len/concat), and for-range loops.

use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{
    ActorDecl, Decl, FnDecl, Program, Stmt, TypeBody, TypeExpr, VariantFields,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Pure-string LLVM IR compiler — Phase 2.
///
/// Generates LLVM IR text for programs using primitives, strings, structs,
/// unit enums, match, and method calls.  No inkwell, no unsafe.
pub struct LlvmTextCompiler {
    /// Target triple emitted in the module header.
    pub target_triple: String,
    /// `builtin fn` dispatch table: MVL name → (C-ABI symbol, return TypeExpr, param TypeExprs).
    ///
    /// Populated via `with_builtins()` or set directly before calling
    /// `compile_to_ir_with_prelude`.  The emitter uses this to route calls to
    /// `builtin fn` declarations to their C runtime symbols instead of generating
    /// a body for the empty block.
    pub builtin_symbols: HashMap<String, (String, TypeExpr, Vec<TypeExpr>)>,
}

impl LlvmTextCompiler {
    /// Create a new compiler with the host target triple.
    pub fn new() -> Self {
        Self {
            target_triple: default_target_triple(),
            builtin_symbols: HashMap::new(),
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
        for p in prelude {
            let stripped = strip_prelude_extension_methods(p);
            emitter.emit_program(&stripped)?;
        }
        emitter.emit_program(prog)?;
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

struct TextEmitter {
    module_name: String,
    target_triple: String,

    // ── Module-level output sections ──────────────────────────────────────
    fn_bodies: Vec<String>,
    str_counter: usize,
    str_globals: Vec<String>,
    type_defs: Vec<String>,
    extern_decls: Vec<String>,

    // ── Type registries (populated during first pass) ─────────────────────
    /// struct name → ordered list of (field_name, field_TypeExpr)
    struct_fields: HashMap<String, Vec<(String, TypeExpr)>>,
    /// enum name → ordered list of variant names (index = discriminant)
    enum_variants: HashMap<String, Vec<String>>,
    /// enum name → ordered list of variant payload field type lists (#1200).
    ///
    /// Parallel to `enum_variants`: index = discriminant. Each entry is the
    /// list of `TypeExpr`s for the variant's tuple-struct fields. Unit variants
    /// have an empty `Vec`. An enum is a "payload enum" if any variant has a
    /// non-empty field list — payload enums lower to `{ i8, ptr }` instead of
    /// a bare `i64` discriminant.
    enum_variant_fields: HashMap<String, Vec<Vec<TypeExpr>>>,

    // ── Per-function state (reset on every new function) ──────────────────
    fn_buf: Vec<String>,
    current_bb: String,
    terminated: bool,
    reg: usize,
    bb: usize,
    locals: HashMap<String, String>,
    ref_locals: HashMap<String, RefLocal>,
    current_ret_ty: TypeExpr,
    fn_ret_types: HashMap<String, TypeExpr>,
    /// Function name → ordered parameter types (for named-fn closure trampolines).
    fn_param_types: HashMap<String, Vec<TypeExpr>>,
    /// SSA register → LLVM type string (for phi type inference)
    reg_types: HashMap<String, String>,
    /// MVL variable name → TypeExpr (for struct field access)
    local_mvl_types: HashMap<String, TypeExpr>,

    // ── Helper-global presence flags ──────────────────────────────────────
    has_println_fmt: bool,
    has_int_fmt: bool,
    has_str_true: bool,
    has_str_false: bool,

    // ── Closure / lambda state (#1148) ────────────────────────────────────
    /// Monotonic counter for generating unique lambda function names.
    lambda_counter: usize,
    /// True once `%__closure_type = type { ptr, ptr }` has been emitted.
    closure_type_emitted: bool,

    // ── Actor state (#1149) ───────────────────────────────────────────────
    /// Actor declarations keyed by actor type name (populated in first pass).
    actor_decls: HashMap<String, ActorDecl>,
    /// True once actor runtime externs have been emitted.
    actor_runtime_declared: bool,
    /// True once `declare void @mvl_yield_check()` has been emitted (#1181).
    yield_check_declared: bool,

    // ── Builtin fn dispatch (#1160) ────────────────────────────────────────
    /// Maps MVL builtin function name → C-ABI symbol (e.g. `bytes` → `_mvl_random_bytes`).
    /// Populated from `LlvmTextCompiler::builtin_symbols` at construction time.
    builtin_syms: HashMap<String, String>,

    // ── Per-function flags ────────────────────────────────────────────────
    /// True while emitting the `main` function (affects `ret` instruction type).
    current_fn_is_main: bool,
    /// SSA registers of actor handles spawned in the current function.
    /// Emitted as `mvl_actor_drop` calls before `mvl_actor_join_all` in `main`.
    spawned_actor_handles: Vec<String>,

    // ── Generic monomorphization (#1156) ──────────────────────────────────
    /// Generic function declarations (type_params non-empty), keyed by name.
    generic_fns: HashMap<String, FnDecl>,
    /// Active type-parameter → concrete-type mapping during monomorphized emission.
    type_param_map: HashMap<String, TypeExpr>,
    /// Mangled names of monomorphized copies already emitted (avoid duplicates).
    mono_emitted: HashSet<String>,
    /// Queue of monomorphized functions to emit: (mangled_name, concrete_types).
    mono_queue: Vec<(String, String, Vec<TypeExpr>)>,

    // ── Heap drop tracking (#1185) ───────────────────────────────────────
    /// Heap-allocated locals in the current function.  Entries are emitted
    /// as `mvl_*_drop` calls before every `ret` instruction.
    /// Heap-allocated locals: (ssa_or_alloca, kind, is_ref).
    /// When `is_ref` is true, the SSA is a stack alloca that must be loaded
    /// before the drop call (the alloca holds the heap object pointer).
    heap_locals: Vec<(String, HeapKind, bool)>,
}

/// Tracks heap-allocated value types for automatic drop emission.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeapKind {
    String,
    Array,
    Map,
}

#[derive(Clone)]
struct RefLocal {
    ptr: String,
    elem_ty: TypeExpr,
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
    fn new_with_builtins(
        module_name: &str,
        target_triple: &str,
        builtin_map: &HashMap<String, (String, TypeExpr, Vec<TypeExpr>)>,
    ) -> Self {
        let mut fn_ret_types: HashMap<String, TypeExpr> = HashMap::new();
        let mut fn_param_types: HashMap<String, Vec<TypeExpr>> = HashMap::new();
        let mut builtin_syms: HashMap<String, String> = HashMap::new();

        for (fn_name, (c_sym, ret_ty, param_tys)) in builtin_map {
            fn_ret_types.insert(fn_name.clone(), ret_ty.clone());
            fn_param_types.insert(fn_name.clone(), param_tys.clone());
            builtin_syms.insert(fn_name.clone(), c_sym.clone());
        }

        Self {
            module_name: module_name.to_string(),
            target_triple: target_triple.to_string(),
            fn_bodies: Vec::new(),
            str_counter: 0,
            str_globals: Vec::new(),
            type_defs: Vec::new(),
            extern_decls: Vec::new(),
            struct_fields: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_variant_fields: HashMap::new(),
            fn_buf: Vec::new(),
            current_bb: String::new(),
            terminated: false,
            reg: 0,
            bb: 0,
            locals: HashMap::new(),
            ref_locals: HashMap::new(),
            current_ret_ty: TypeExpr::Base {
                name: "Unit".into(),
                args: vec![],
                span: Default::default(),
            },
            fn_ret_types,
            fn_param_types,
            reg_types: HashMap::new(),
            local_mvl_types: HashMap::new(),
            has_println_fmt: false,
            has_int_fmt: false,
            has_str_true: false,
            has_str_false: false,
            lambda_counter: 0,
            closure_type_emitted: false,
            actor_decls: HashMap::new(),
            actor_runtime_declared: false,
            yield_check_declared: false,
            builtin_syms,
            current_fn_is_main: false,
            spawned_actor_handles: Vec::new(),
            generic_fns: HashMap::new(),
            type_param_map: HashMap::new(),
            mono_emitted: HashSet::new(),
            mono_queue: Vec::new(),
            heap_locals: Vec::new(),
        }
    }

    // ── Finalise ──────────────────────────────────────────────────────────

    fn finish(self) -> String {
        let mut out = String::new();
        out.push_str(&format!("; ModuleID = '{}'\n", self.module_name));
        out.push_str(&format!("source_filename = \"{}\"\n", self.module_name));
        out.push_str(&format!("target triple = \"{}\"\n", self.target_triple));
        for td in &self.type_defs {
            out.push('\n');
            out.push_str(td);
        }
        for sg in &self.str_globals {
            out.push('\n');
            out.push_str(sg);
        }
        for body in &self.fn_bodies {
            out.push('\n');
            out.push_str(body);
        }
        for ext in &self.extern_decls {
            out.push('\n');
            out.push_str(ext);
        }
        out
    }

    // ── Counter helpers ───────────────────────────────────────────────────

    fn next_reg(&mut self) -> String {
        let n = self.reg;
        self.reg += 1;
        format!("%t{n}")
    }

    fn next_bb(&mut self, prefix: &str) -> String {
        let n = self.bb;
        self.bb += 1;
        format!("{prefix}_{n}")
    }

    // ── Instruction helpers ───────────────────────────────────────────────

    fn push_line(&mut self, line: &str) {
        self.fn_buf.push(line.to_string());
    }

    fn push_instr(&mut self, instr: &str) {
        self.fn_buf.push(format!("  {instr}"));
    }

    fn start_bb(&mut self, label: &str) {
        self.fn_buf.push(format!("{label}:"));
        self.current_bb = label.to_string();
        self.terminated = false;
    }

    // ── Per-function state reset ──────────────────────────────────────────

    fn reset_fn_state(&mut self, ret_ty: TypeExpr) {
        self.fn_buf.clear();
        self.current_bb = "entry".to_string();
        self.terminated = false;
        self.reg = 0;
        self.bb = 0;
        self.locals.clear();
        self.ref_locals.clear();
        self.reg_types.clear();
        self.local_mvl_types.clear();
        self.current_ret_ty = ret_ty;
        self.current_fn_is_main = false;
        self.spawned_actor_handles.clear();
        self.heap_locals.clear();
    }

    // ── Extern declaration helpers ────────────────────────────────────────

    fn ensure_extern(&mut self, decl: &str) {
        if !self.extern_decls.iter().any(|d| d == decl) {
            self.extern_decls.push(decl.to_string());
        }
    }

    /// Ensure `@mvl_yield_check` is declared once per module (#1181).
    ///
    /// Called at every loop back-edge insertion point. The flag avoids repeated
    /// linear scans through `extern_decls`.
    pub(super) fn ensure_yield_check_extern(&mut self) {
        if !self.yield_check_declared {
            self.ensure_extern("declare void @mvl_yield_check()");
            self.yield_check_declared = true;
        }
    }

    // ── Program emission ──────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) -> Result<(), String> {
        // First pass: register all function return types and type declarations.
        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) => {
                    let ret = fd.return_type.as_ref().clone();
                    let params: Vec<TypeExpr> = fd.params.iter().map(|p| p.ty.clone()).collect();
                    // Register under the short name (e.g. `from_chars`)
                    self.fn_ret_types.insert(fd.name.clone(), ret.clone());
                    self.fn_param_types.insert(fd.name.clone(), params.clone());
                    // Also register under the qualified name (e.g. `String::from_chars`)
                    // so that static call-site lookups like `fn_ret_types["String::from_chars"]`
                    // resolve correctly.
                    if let Some(recv) = &fd.receiver_type {
                        let qualified = format!("{}::{}", recv, fd.name);
                        self.fn_ret_types.insert(qualified.clone(), ret.clone());
                        self.fn_param_types.insert(qualified, params);
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
                            let field_types: Vec<String> =
                                field_list.iter().map(|(_, ty)| Self::llvm_ty(ty)).collect();
                            self.type_defs.push(format!(
                                "%{} = type {{ {} }}",
                                td.name,
                                field_types.join(", ")
                            ));
                            self.struct_fields.insert(td.name.clone(), field_list);
                        }
                    }
                    TypeBody::Enum(variants) => {
                        let variant_names: Vec<String> =
                            variants.iter().map(|v| v.name.clone()).collect();
                        let variant_fields: Vec<Vec<TypeExpr>> = variants
                            .iter()
                            .map(|v| match &v.fields {
                                VariantFields::Tuple(tys) => tys.clone(),
                                _ => Vec::new(),
                            })
                            .collect();
                        self.enum_variants.insert(td.name.clone(), variant_names);
                        self.enum_variant_fields
                            .insert(td.name.clone(), variant_fields);
                    }
                    TypeBody::Alias(_) => {}
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
                    self.type_defs.push(format!(
                        "%{state_name} = type {{ {} }}",
                        field_types.join(", ")
                    ));
                    self.struct_fields.insert(state_name, field_list);
                    self.actor_decls.insert(ad.name.clone(), ad.clone());
                }
                Decl::Extern(ed) if ed.abi == "c" => {
                    // Emit LLVM `declare` for each extern "c" function (#811).
                    // These are resolved at link time from a loaded shared library
                    // (e.g. `lli --load=libpkg_foo.{dylib,so}`).
                    for ef in &ed.fns {
                        let ret_ty = Self::llvm_ty(&ef.return_type);
                        let param_tys: Vec<String> =
                            ef.params.iter().map(|p| Self::llvm_ty(&p.ty)).collect();
                        let decl =
                            format!("declare {} @{}({})", ret_ty, ef.name, param_tys.join(", "));
                        self.ensure_extern(&decl);
                        // Register return type and param types so call emission works.
                        self.fn_ret_types
                            .insert(ef.name.clone(), ef.return_type.as_ref().clone());
                        self.fn_param_types.insert(
                            ef.name.clone(),
                            ef.params.iter().map(|p| p.ty.clone()).collect(),
                        );
                    }
                }
                _ => {}
            }
        }

        // Actor pass: emit behavior functions + dispatch for each actor.
        if !self.actor_decls.is_empty() {
            self.ensure_actor_runtime_externs();
            let actor_names: Vec<String> = self.actor_decls.keys().cloned().collect();
            for name in actor_names {
                let ad = self.actor_decls[&name].clone();
                self.emit_actor_decl(&ad)?;
            }
        }

        // Collect generic function declarations for on-demand monomorphization.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.type_params.is_empty() {
                    self.generic_fns.insert(fd.name.clone(), fd.clone());
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
        while !self.mono_queue.is_empty() {
            mono_iterations += 1;
            if mono_iterations > MONO_LIMIT {
                return Err(
                    "monomorphization limit exceeded — possible infinite instantiation".into(),
                );
            }
            let queue = std::mem::take(&mut self.mono_queue);
            for (mangled, orig_name, concrete_types) in queue {
                let gfd = match self.generic_fns.get(&orig_name) {
                    Some(fd) => fd.clone(),
                    None => continue,
                };
                // Set up type parameter → concrete type mapping.
                for (tp, ct) in gfd.type_params.iter().zip(concrete_types.iter()) {
                    self.type_param_map
                        .insert(tp.name().to_string(), ct.clone());
                }
                // Emit the function under its mangled name.
                let mut fd = gfd;
                fd.name = mangled;
                fd.type_params.clear();
                self.emit_fn(&fd)?;
                self.type_param_map.clear();
            }
        }

        Ok(())
    }

    // ── Function emission ─────────────────────────────────────────────────

    fn emit_fn(&mut self, fd: &FnDecl) -> Result<(), String> {
        let ret_ty = fd.return_type.as_ref();
        self.reset_fn_state(ret_ty.clone());
        self.current_fn_is_main = fd.name == "main";

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

        let sig = if self.current_fn_is_main {
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
                self.locals.insert(p.name.clone(), ssa.clone());
                self.reg_types.insert(ssa, ty_str);
                self.local_mvl_types.insert(p.name.clone(), p.ty.clone());
            }
        }

        let body_val = self.emit_block(&fd.body)?;

        if !self.terminated {
            // If the function returns a heap-allocated local, exclude it from
            // drops — ownership transfers to the caller (move semantics).
            if let Some(Stmt::Expr { expr, .. }) = fd.body.stmts.last() {
                self.exclude_returned_value(expr);
            }
            self.emit_heap_drops();
            if self.current_fn_is_main {
                if !self.actor_decls.is_empty() {
                    // Drop each handle to close the sender — this signals the
                    // actor thread's recv loop to exit.
                    for handle in std::mem::take(&mut self.spawned_actor_handles) {
                        self.push_instr(&format!("call void @mvl_actor_drop(ptr {handle})"));
                    }
                    self.push_instr("call void @mvl_actor_join_all()");
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
        let body_text = self.fn_buf.join("\n");
        self.fn_bodies.push(body_text);
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
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn compile(src: &str) -> String {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        LlvmTextCompiler::new()
            .compile_to_ir(&prog, "test")
            .expect("compile_to_ir failed")
    }

    #[test]
    fn simple_add_function() {
        let ir = compile("fn add(a: Int, b: Int) -> Int { a + b }");
        assert!(ir.contains("define i64 @add(i64 %a, i64 %b)"), "{ir}");
        assert!(ir.contains("add i64"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn integer_literal_returned() {
        let ir = compile("fn answer() -> Int { 42 }");
        assert!(ir.contains("define i64 @answer()"), "{ir}");
        assert!(ir.contains("ret i64 42"), "{ir}");
    }

    #[test]
    fn bool_literal_returned() {
        let ir = compile("fn always_true() -> Bool { true }");
        assert!(ir.contains("define i1 @always_true()"), "{ir}");
        assert!(ir.contains("ret i1 true"), "{ir}");
    }

    #[test]
    fn arithmetic_operators() {
        let ir = compile("fn f(a: Int, b: Int) -> Int { a - b }");
        assert!(ir.contains("sub i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a * b }");
        assert!(ir.contains("mul i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a / b }");
        assert!(ir.contains("sdiv i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a % b }");
        assert!(ir.contains("srem i64"), "{ir}");
    }

    #[test]
    fn comparison_operators_emit_icmp() {
        let ir = compile("fn lt(a: Int, b: Int) -> Bool { a < b }");
        assert!(ir.contains("icmp slt i64"), "{ir}");
        let ir = compile("fn gt(a: Int, b: Int) -> Bool { a > b }");
        assert!(ir.contains("icmp sgt i64"), "{ir}");
        let ir = compile("fn eq(a: Int, b: Int) -> Bool { a == b }");
        assert!(ir.contains("icmp eq i64"), "{ir}");
    }

    #[test]
    fn if_else_emits_phi() {
        let ir = compile("fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
        assert!(ir.contains("icmp sgt"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
        assert!(ir.contains("phi"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    /// Regression for #1155: a 3-way `else if` chain must emit PHI nodes for
    /// every branch so the correct value is selected at runtime. Before the fix,
    /// the `else if` condition was silently dropped and the merge block produced
    /// `ret i64 undef`.
    #[test]
    fn else_if_chain_emits_phi_for_all_branches() {
        let ir = compile(
            "fn classify(n: Int) -> Int {\n\
                 if n > 0 { 1 }\n\
                 else if n < 0 { -1 }\n\
                 else { 0 }\n\
             }",
        );
        // The `else if n < 0` condition must actually be evaluated.
        assert!(ir.contains("icmp slt"), "{ir}");
        // Two PHI nodes: inner selects between -1 and 0; outer selects between 1 and inner.
        let phi_count = ir.matches(" = phi ").count();
        assert!(
            phi_count >= 2,
            "else-if chain needs ≥2 phi nodes, got {phi_count}\n{ir}"
        );
        // Return must be a defined value, not undef.
        assert!(ir.contains("ret i64"), "{ir}");
        assert!(!ir.contains("ret i64 undef"), "{ir}");
    }

    #[test]
    fn unit_function_emits_ret_void() {
        let ir = compile("fn noop() -> Unit { }");
        assert!(ir.contains("define void @noop()"), "{ir}");
        assert!(ir.contains("ret void"), "{ir}");
    }

    #[test]
    fn main_emits_i32_return() {
        let ir = compile("fn main() -> Unit { }");
        assert!(ir.contains("define i32 @main()"), "{ir}");
        assert!(ir.contains("ret i32 0"), "{ir}");
    }

    #[test]
    fn main_explicit_return_emits_ret_i32_0() {
        let ir = compile("fn main() -> Unit { return; }");
        assert!(ir.contains("define i32 @main()"), "{ir}");
        assert!(ir.contains("ret i32 0"), "{ir}");
        assert!(!ir.contains("ret void"), "{ir}");
    }

    #[test]
    fn let_binding_aliases_ssa_value() {
        let ir = compile("fn f(x: Int) -> Int { let y: Int = x; y }");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn logical_not_emits_xor() {
        let ir = compile("fn f(b: Bool) -> Bool { !b }");
        assert!(ir.contains("xor i1"), "{ir}");
    }

    #[test]
    fn module_header_present() {
        let ir = compile("fn f() -> Int { 0 }");
        assert!(ir.contains("ModuleID = 'test'"), "{ir}");
        assert!(ir.contains("source_filename = \"test\""), "{ir}");
        assert!(ir.contains("target triple"), "{ir}");
    }

    #[test]
    fn multiple_functions_and_call() {
        let ir = compile(
            "fn add(a: Int, b: Int) -> Int { a + b }\n\
             fn double(n: Int) -> Int { add(n, n) }",
        );
        assert!(ir.contains("define i64 @add"), "{ir}");
        assert!(ir.contains("define i64 @double"), "{ir}");
        assert!(ir.contains("call i64 @add"), "{ir}");
    }

    #[test]
    fn negation_emits_sub_from_zero() {
        let ir = compile("fn neg(x: Int) -> Int { -x }");
        assert!(ir.contains("sub i64 0,"), "{ir}");
    }

    #[test]
    fn short_circuit_and_emits_phi() {
        let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a && b }");
        assert!(ir.contains("phi i1"), "{ir}");
        assert!(ir.contains("false"), "{ir}");
    }

    #[test]
    fn short_circuit_or_emits_phi() {
        let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a || b }");
        assert!(ir.contains("phi i1"), "{ir}");
        assert!(ir.contains("true"), "{ir}");
    }

    #[test]
    fn mutable_ref_uses_alloca_store_load() {
        let ir = compile(
            "partial fn counter(n: Int) -> Int {\
             let c: ref Int = 0;\
             while c < n {\
               c = c + 1;\
             }\
             c\
             }",
        );
        assert!(ir.contains("alloca i64"), "{ir}");
        assert!(ir.contains("store i64"), "{ir}");
        assert!(ir.contains("load i64"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
    }

    #[test]
    fn string_literal_emits_global_and_string_new() {
        let ir = compile("fn main() -> Unit ! Console { println(\"hello\") }");
        assert!(ir.contains("mvl_string_new"), "{ir}");
        assert!(ir.contains("hello"), "{ir}");
        assert!(ir.contains("dprintf"), "{ir}");
    }

    #[test]
    fn assert_emits_conditional_trap() {
        let ir = compile("fn main() -> Unit { assert(1 == 1) }");
        assert!(ir.contains("llvm.trap"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
    }

    #[test]
    fn struct_type_emits_type_def() {
        let ir = compile(
            "type Point = struct { x: Int, y: Int }\n\
             fn get_x(p: Point) -> Int { p.x }",
        );
        assert!(ir.contains("%Point = type { i64, i64 }"), "{ir}");
        assert!(ir.contains("define i64 @get_x(%Point %p)"), "{ir}");
        assert!(ir.contains("extractvalue %Point"), "{ir}");
    }

    #[test]
    fn enum_variant_emits_discriminant() {
        let ir = compile(
            "type Shape = enum { Circle, Square }\n\
             fn circle() -> Shape { Shape::Circle }",
        );
        assert!(ir.contains("ret i64 0"), "{ir}");
    }

    // ── Closure / lambda tests (#1148) ────────────────────────────────────

    #[test]
    fn closure_type_emitted_once() {
        // Two lambdas in the same program — closure type must appear exactly once.
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2];\n\
             let _a: List[Int] = xs.filter(|x: Int| x > 0);\n\
             let _b: Bool = xs.any(|x: Int| x > 1);\n\
             }",
        );
        let count = ir.matches("%__closure_type = type").count();
        assert_eq!(count, 1, "expected exactly one closure type def:\n{ir}");
    }

    #[test]
    fn non_capturing_lambda_emits_function_and_null_env() {
        // |x: Int| x * 2  — no free variables
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let _d: List[Int] = xs.filter(|x: Int| x > 0);\n\
             }",
        );
        // Lambda function emitted as a top-level define.
        assert!(
            ir.contains("define i1 @__lambda_0(ptr %__env, i64 %x)"),
            "{ir}"
        );
        // Closure struct built with null env ptr.
        assert!(ir.contains("store ptr null"), "{ir}");
        // fn_ptr field set to the lambda.
        assert!(ir.contains("store ptr @__lambda_0"), "{ir}");
    }

    #[test]
    fn capturing_lambda_emits_env_struct_and_getelementptr() {
        // |x: Int| x > threshold  — captures `threshold` from outer scope
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let threshold: Int = 2;\n\
             let _above: List[Int] = xs.filter(|x: Int| x > threshold);\n\
             }",
        );
        // Env struct type must be registered.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // Capture stored via GEP.
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
        // Lambda function has the env parameter.
        assert!(ir.contains("define i1 @__lambda_0(ptr %__env"), "{ir}");
        // Inside the lambda the captured value is loaded.
        assert!(ir.contains("load i64"), "{ir}");
    }

    #[test]
    fn hof_filter_with_lambda_emits_list_filter_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let evens: List[Int] = xs.filter(|x: Int| x > 0);\n\
             }",
        );
        assert!(ir.contains("declare ptr @List_filter(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call ptr @List_filter"), "{ir}");
        assert!(ir.contains("@__lambda_0"), "{ir}");
    }

    #[test]
    fn hof_any_with_lambda_emits_i1_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let b: Bool = xs.any(|x: Int| x > 0);\n\
             }",
        );
        assert!(ir.contains("declare i1 @List_any(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call i1 @List_any"), "{ir}");
    }

    #[test]
    fn named_fn_closure_wraps_in_closure_struct() {
        let ir = compile(
            "fn is_pos(x: Int) -> Bool { x > 0 }\n\
             fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let evens: List[Int] = xs.filter(is_pos);\n\
             }",
        );
        // Wrapper trampoline generated
        assert!(ir.contains("@__closure_wrap_is_pos"), "{ir}");
        // Closure struct built pointing to wrapper
        assert!(ir.contains("store ptr @__closure_wrap_is_pos"), "{ir}");
        assert!(ir.contains("call ptr @List_filter"), "{ir}");
        // Trampoline must forward the element argument, not call with zero args.
        assert!(
            ir.contains("define i1 @__closure_wrap_is_pos(ptr %__env, i64 %__arg0)"),
            "trampoline missing typed param:\n{ir}"
        );
        assert!(
            ir.contains("call i1 @is_pos(i64 %__arg0)"),
            "trampoline must forward arg to original:\n{ir}"
        );
    }

    #[test]
    fn hof_fold_emits_init_slot_and_list_fold_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let sum: Int = xs.fold(0, |acc: Int, x: Int| acc + x);\n\
             }",
        );
        assert!(ir.contains("declare ptr @List_fold(ptr, ptr, ptr)"), "{ir}");
        // Initial value must be stack-allocated and stored.
        assert!(ir.contains("alloca i64"), "{ir}");
        assert!(ir.contains("store i64 0"), "{ir}");
        assert!(ir.contains("call ptr @List_fold(ptr"), "{ir}");
        // Result loaded back as the accumulator type.
        assert!(ir.contains("load i64"), "{ir}");
        // Lambda for accumulator function has two typed params.
        assert!(
            ir.contains("define i64 @__lambda_0(ptr %__env, i64 %acc, i64 %x)"),
            "{ir}"
        );
    }

    #[test]
    fn capturing_lambda_with_two_captures() {
        // Captures both `lo` and `hi` — env struct must have two i64 fields.
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3, 4, 5];\n\
             let lo: Int = 1;\n\
             let hi: Int = 4;\n\
             let mid: List[Int] = xs.filter(|x: Int| x > lo);\n\
             }",
        );
        // Env struct type registered.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // GEP accesses for storing captures into env.
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
        // Two stores for the two captured values.
        let store_count = ir.matches("store i64").count();
        assert!(
            store_count >= 1,
            "expected at least one i64 store for env field, got {store_count}:\n{ir}"
        );
    }

    #[test]
    fn ref_local_capture_loads_before_storing_into_env() {
        // `count` is a mutable ref binding — must be loaded before capture.
        let ir = compile(
            "fn run() -> Int {\n\
             let count: ref Int = 0;\n\
             count = count + 1;\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let above: List[Int] = xs.filter(|x: Int| x > count);\n\
             above.len()\n\
             }",
        );
        // ref local alloca present.
        assert!(ir.contains("alloca i64"), "{ir}");
        // Env struct created for the capture.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // A load from the ref alloca must precede the GEP store into the env.
        assert!(ir.contains("load i64"), "{ir}");
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
    }

    // ── Actor emission tests (#1149) ──────────────────────────────────────

    #[test]
    fn actor_emits_state_struct_and_behavior_fn() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }",
        );
        // State struct typedef.
        assert!(ir.contains("%CounterState = type"), "{ir}");
        // Behavior function.
        assert!(
            ir.contains("define void @counter_increment(ptr %self, i64 %n)"),
            "{ir}"
        );
    }

    #[test]
    fn actor_emits_dispatch_function_with_switch() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
               pub fn reset() { }\n\
             }",
        );
        // Dispatch function signature.
        assert!(
            ir.contains("define void @counter_dispatch(ptr %state, i64 %disc, ptr %args)"),
            "{ir}"
        );
        // Switch with at least two case labels.
        assert!(ir.contains("switch i64 %disc, label %default"), "{ir}");
        assert!(ir.contains("i64 0, label %behavior_0"), "{ir}");
        assert!(ir.contains("i64 1, label %behavior_1"), "{ir}");
    }

    #[test]
    fn actor_runtime_externs_emitted() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }",
        );
        assert!(ir.contains("declare ptr @mvl_actor_spawn"), "{ir}");
        assert!(ir.contains("declare void @mvl_actor_send"), "{ir}");
        assert!(ir.contains("declare void @mvl_actor_join_all"), "{ir}");
    }

    #[test]
    fn spawn_emits_alloca_and_actor_spawn_call() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int {\n\
               let c: Counter = actor Counter { count: 0 };\n\
               0\n\
             }",
        );
        // State alloca.
        assert!(ir.contains("alloca %CounterState"), "{ir}");
        // Runtime spawn call.
        assert!(ir.contains("call ptr @mvl_actor_spawn"), "{ir}");
    }

    #[test]
    fn actor_method_call_emits_send() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int {\n\
               let c: Counter = actor Counter { count: 0 };\n\
               c.increment(1);\n\
               0\n\
             }",
        );
        // The send call must appear.
        assert!(ir.contains("call void @mvl_actor_send"), "{ir}");
    }

    #[test]
    fn join_all_emitted_in_main_when_actors_present() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int { 0 }",
        );
        assert!(ir.contains("call void @mvl_actor_join_all"), "{ir}");
    }

    // ── Generic monomorphization tests (#1156) ───────────────────────────

    /// Generic `identity[T]` must produce separate monomorphized copies for
    /// each concrete type argument used at call sites.
    #[test]
    fn generic_fn_monomorphized_per_concrete_type() {
        let ir = compile(
            "fn identity[T](x: T) -> T { x }\n\
             fn main() -> Unit {\n\
               let n: Int = identity(42);\n\
               let s: String = identity(\"hi\");\n\
             }",
        );
        // Two separate definitions with correct types.
        assert!(ir.contains("define i64 @identity__Int(i64 %x)"), "{ir}");
        assert!(ir.contains("define ptr @identity__String(ptr %x)"), "{ir}");
        // Call sites use mangled names.
        assert!(ir.contains("call i64 @identity__Int(i64 42)"), "{ir}");
        assert!(ir.contains("call ptr @identity__String("), "{ir}");
    }

    // ── Option constructor + match tests (#1156) ─────────────────────────

    /// `Some(val)` must emit a `{ i8, ptr }` tagged union with disc=0.
    #[test]
    fn some_constructor_emits_tagged_union() {
        let ir = compile("fn wrap(n: Int) -> Option[Int] { Some(n) }");
        assert!(
            ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 0, 0"),
            "{ir}"
        );
        assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
        assert!(ir.contains("define { i8, ptr } @wrap"), "{ir}");
    }

    /// `None` must emit a `{ i8, ptr }` tagged union with disc=1.
    #[test]
    fn none_constructor_emits_tagged_union() {
        let ir = compile("fn empty() -> Option[Int] { None }");
        assert!(
            ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 1, 0"),
            "{ir}"
        );
    }

    /// Match on `Option[Int]` must emit a switch on the discriminant byte.
    #[test]
    fn option_match_emits_switch_on_discriminant() {
        let ir = compile(
            "fn unwrap_or(opt: Option[Int], default: Int) -> Int {\n\
                 match opt {\n\
                     Some(v) => v,\n\
                     None => default,\n\
                 }\n\
             }",
        );
        assert!(ir.contains("switch i8"), "{ir}");
        assert!(ir.contains("i8 0, label"), "{ir}"); // Some arm
        assert!(ir.contains("i8 1, label"), "{ir}"); // None arm
        assert!(ir.contains("phi i64"), "{ir}");
    }

    // ── Map literal emission tests (#1184) ───────────────────────────────

    #[test]
    fn map_literal_emits_map_new_and_insert() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1, \"b\": 2};\n\
             }",
        );
        assert!(ir.contains("call ptr @mvl_map_new(i64"), "{ir}");
        assert!(ir.contains("call void @_mvl_map_insert(ptr"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_string_ptr(ptr"), "{ir}");
        assert!(ir.contains("call i64 @_mvl_str_len(ptr"), "{ir}");
    }

    #[test]
    fn empty_map_emits_map_new_only() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = Map::new();\n\
             }",
        );
        // Map::new() goes through FnCall, not Map literal — just verify no crash.
        assert!(ir.contains("define i32 @main()"), "{ir}");
    }

    #[test]
    fn map_len_emits_mvl_map_len() {
        let ir = compile(
            "fn main() -> Int {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             m.len()\n\
             }",
        );
        assert!(ir.contains("declare i64 @_mvl_map_len(ptr)"), "{ir}");
        assert!(ir.contains("call i64 @_mvl_map_len(ptr"), "{ir}");
    }

    #[test]
    fn map_keys_emits_mvl_map_keys() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             let _k: List[String] = m.keys();\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_map_keys(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_map_keys(ptr"), "{ir}");
    }

    #[test]
    fn map_contains_key_emits_null_check() {
        let ir = compile(
            "fn main() -> Bool {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             m.contains_key(\"a\")\n\
             }",
        );
        assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
        assert!(ir.contains("icmp ne ptr"), "{ir}");
    }

    #[test]
    fn map_get_emits_null_guard_before_load() {
        let ir = compile(
            "fn f(m: Map[String, Int]) -> Int {\n\
             m.get(\"key\")\n\
             }",
        );
        assert!(ir.contains("call ptr @_mvl_map_get(ptr"), "{ir}");
        // Must null-check before loading
        assert!(ir.contains("icmp eq ptr"), "{ir}");
        assert!(ir.contains("load i64, ptr"), "{ir}");
        assert!(ir.contains("phi i64"), "{ir}");
    }

    // ── HeapKind drop tracking tests (#1185) ─────────────────────────────

    #[test]
    fn string_local_emits_drop_before_ret() {
        let ir = compile(
            "fn greet() -> Unit {\n\
             let s: String = \"hello\";\n\
             }",
        );
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_string_drop(ptr)"), "{ir}");
    }

    #[test]
    fn list_local_emits_drop_before_ret() {
        let ir = compile(
            "fn nums() -> Unit {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             }",
        );
        assert!(ir.contains("call void @mvl_array_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_array_drop(ptr)"), "{ir}");
    }

    #[test]
    fn map_local_emits_drop_before_ret() {
        let ir = compile(
            "fn maps() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             }",
        );
        assert!(ir.contains("call void @mvl_map_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_map_drop(ptr)"), "{ir}");
    }

    #[test]
    fn multiple_heap_locals_all_dropped() {
        let ir = compile(
            "fn multi() -> Unit {\n\
             let s: String = \"hello\";\n\
             let xs: List[Int] = [1, 2];\n\
             }",
        );
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        assert!(ir.contains("call void @mvl_array_drop(ptr"), "{ir}");
    }

    #[test]
    fn primitive_locals_no_drop() {
        let ir = compile(
            "fn prims() -> Unit {\n\
             let x: Int = 42;\n\
             let b: Bool = true;\n\
             }",
        );
        assert!(!ir.contains("_drop"), "{ir}");
    }

    #[test]
    fn explicit_return_emits_drops() {
        let ir = compile(
            "fn early() -> Int {\n\
             let s: String = \"hello\";\n\
             return 42;\n\
             }",
        );
        // The drop should appear before the ret instruction.
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
    }

    #[test]
    fn shadowed_string_local_no_double_drop() {
        let ir = compile(
            "fn f() -> Unit {\n\
             let s: String = \"first\";\n\
             let s: String = \"second\";\n\
             }",
        );
        // Should have exactly 1 drop call (for the second binding only;
        // the first is removed from tracking when shadowed).
        let drop_count = ir.matches("call void @mvl_string_drop(ptr").count();
        assert_eq!(drop_count, 1, "expected 1 drop, got {drop_count}\n{ir}");
    }

    #[test]
    fn ref_string_local_emits_load_then_drop() {
        let ir = compile(
            "fn f() -> Unit {\n\
             let s: ref String = \"hello\";\n\
             }",
        );
        // ref local: must load from alloca, then drop the loaded value.
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        // Verify the load-before-drop pattern exists.
        assert!(ir.contains("load ptr, ptr"), "{ir}");
    }

    // ── String builtin kernel methods tests (#1186) ──────────────────────

    #[test]
    fn string_chars_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Unit {\n\
             let _cs: List[String] = s.chars();\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_string_chars(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_string_chars(ptr"), "{ir}");
    }

    #[test]
    fn string_byte_at_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Int {\n\
             s.byte_at(0)\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_byte_at(ptr, i64)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_byte_at(ptr"), "{ir}");
    }

    #[test]
    fn string_find_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Int {\n\
             s.find(\"x\")\n\
             }",
        );
        assert!(ir.contains("declare i64 @_mvl_str_find(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call i64 @_mvl_str_find(ptr"), "{ir}");
    }

    #[test]
    fn string_split_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Unit {\n\
             let _parts: List[String] = s.split(\",\");\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_split(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_split(ptr"), "{ir}");
    }

    #[test]
    fn string_substring_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.substring(0, 3)\n\
             }",
        );
        assert!(
            ir.contains("declare ptr @_mvl_str_substring(ptr, i64, i64)"),
            "{ir}"
        );
        assert!(ir.contains("call ptr @_mvl_str_substring(ptr"), "{ir}");
    }

    #[test]
    fn string_contains_emits_i64_to_bool() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.contains(\"x\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_contains(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("icmp ne i64"), "{ir}");
    }

    #[test]
    fn string_starts_with_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.starts_with(\"http\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_starts_with(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_starts_with(ptr"), "{ir}");
    }

    #[test]
    fn string_ends_with_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.ends_with(\".mvl\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_ends_with(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_ends_with(ptr"), "{ir}");
    }

    #[test]
    fn string_trim_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.trim()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_trim(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_trim(ptr"), "{ir}");
    }

    #[test]
    fn string_to_lower_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.to_lower()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_to_lower(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_to_lower(ptr"), "{ir}");
    }

    #[test]
    fn string_to_upper_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.to_upper()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_to_upper(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_to_upper(ptr"), "{ir}");
    }

    #[test]
    fn string_replace_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.replace(\"old\", \"new\")\n\
             }",
        );
        assert!(
            ir.contains("declare ptr @_mvl_str_replace(ptr, ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call ptr @_mvl_str_replace(ptr"), "{ir}");
    }

    /// `extern "c"` block emits LLVM `declare` instructions (#811).
    #[test]
    fn extern_c_emits_declare() {
        let ir = compile(
            "extern \"c\" {\n\
             fn sqlite_open(path: String) -> Int\n\
             fn sqlite_close(db: Int) -> Unit\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @sqlite_open(ptr)"),
            "missing sqlite_open declare: {ir}"
        );
        assert!(
            ir.contains("declare void @sqlite_close(i64)"),
            "missing sqlite_close declare: {ir}"
        );
    }

    /// `extern "rust"` block is NOT emitted by LLVM backend (handled by Rust backend only).
    #[test]
    fn extern_rust_not_emitted_by_llvm() {
        let ir = compile(
            "extern \"rust\" {\n\
             fn bridge_fn(x: Int) -> Int\n\
             }",
        );
        assert!(
            !ir.contains("declare") || !ir.contains("bridge_fn"),
            "extern rust should not emit declare: {ir}"
        );
    }
}
