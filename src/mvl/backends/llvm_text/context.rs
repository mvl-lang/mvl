// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Split state types for the LLVM text emitter (#1523).
//!
//! [`TextEmitter`](super::emitter::TextEmitter) used to be a 41-field god-object
//! whose state crossed several lifetimes:
//!
//! - **module-global** — type registries, output buffers, helper flags, actor
//!   declarations. Live for the whole compilation unit.
//! - **per-function** — SSA register counters, locals, basic-block bookkeeping.
//!   Reset on every new function via an error-prone manual reset method.
//! - **monomorphization** — the generic-fn queue and active type-parameter map.
//!
//! Splitting these into [`ModuleCtx`], [`FnCtx`], and [`MonoQueue`] lets the
//! Rust compiler enforce the lifecycle: replacing `self.fn_ctx` at the start of
//! every function emission eliminates the need for a hand-maintained reset
//! method.

use std::collections::{HashMap, HashSet};

use super::emitter::{HeapKind, RefLocal};
use super::BuiltinSymbolInfo;
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{TirFn, TypeExpr};

// ── Module-global state ──────────────────────────────────────────────────────

/// State that lives for the entire compilation unit: output buffers, type
/// registries, helper-global presence flags, monomorphic dispatch tables.
pub(super) struct ModuleCtx {
    pub module_name: String,
    pub target_triple: String,

    // ── Output sections ───────────────────────────────────────────────────
    pub fn_bodies: Vec<String>,
    pub str_counter: usize,
    pub str_globals: Vec<String>,
    pub type_defs: Vec<String>,
    pub extern_decls: Vec<String>,

    // ── Type registries (populated during first pass) ─────────────────────
    /// struct name → ordered list of (field_name, field_TypeExpr)
    pub struct_fields: HashMap<String, Vec<(String, TypeExpr)>>,
    /// enum name → ordered list of variant names (index = discriminant)
    pub enum_variants: HashMap<String, Vec<String>>,
    /// enum name → ordered list of variant payload field type lists (#1200).
    pub enum_variant_fields: HashMap<String, Vec<Vec<TypeExpr>>>,
    /// "EnumType::VariantName" → ordered field names for struct variants (#1357).
    pub enum_struct_variant_field_names: HashMap<String, Vec<String>>,

    // ── Cross-function fn-signature registry ──────────────────────────────
    /// Function name → return type. Module-wide, not per-fn.
    pub fn_ret_types: HashMap<String, TypeExpr>,
    /// Function name → ordered parameter types (for closure trampolines).
    pub fn_param_types: HashMap<String, Vec<TypeExpr>>,

    // ── Helper-global presence flags ──────────────────────────────────────
    pub has_println_fmt: bool,
    pub has_int_fmt: bool,
    pub has_str_true: bool,
    pub has_str_false: bool,

    // ── Closure / lambda state (#1148) ────────────────────────────────────
    /// Monotonic counter for generating unique lambda function names.
    pub lambda_counter: usize,
    /// True once `%__closure_type = type { ptr, ptr }` has been emitted.
    pub closure_type_emitted: bool,

    /// Named fn-type aliases: `type Dispatcher = fn(...) -> ...`.
    pub fn_aliases: HashMap<String, TypeExpr>,

    // ── Actor state (#1149) ───────────────────────────────────────────────
    /// Actor declarations keyed by actor type name (populated in first pass).
    pub tir_actor_decls: HashMap<String, crate::mvl::ir::TirActorDecl>,
    /// True once actor runtime externs have been emitted.
    pub actor_runtime_declared: bool,
    /// Names of actors whose behavior + dispatch functions have already been
    /// emitted. The emitter runs the actor pass once per `emit_program` call;
    /// without dedupe, std.actors actors get emitted N times (#1610).
    pub actor_emitted: HashSet<String>,
    /// True once `declare void @_mvl_yield_check()` has been emitted (#1181).
    pub yield_check_declared: bool,

    // ── Builtin fn dispatch (#1160) ────────────────────────────────────────
    /// Maps MVL builtin function name → C-ABI symbol (e.g. `bytes` → `_mvl_random_bytes`).
    pub builtin_syms: HashMap<String, String>,

    // ── Audit-marked relabel declarations (#896, #1554) ──────────────────
    /// Map from relabel transition name → (from_label, to_label) for every
    /// `relabel` declaration carrying the `audit` keyword. Populated during
    /// the first pass; consulted by the `Expr::Relabel` emit arm to decide
    /// whether every call site of the transition needs a runtime audit call.
    pub audit_relabels: HashMap<String, (Option<String>, Option<String>)>,
}

impl ModuleCtx {
    pub(super) fn new(
        module_name: &str,
        target_triple: &str,
        builtin_map: &HashMap<String, BuiltinSymbolInfo>,
    ) -> Self {
        let mut fn_ret_types: HashMap<String, TypeExpr> = HashMap::new();
        let mut fn_param_types: HashMap<String, Vec<TypeExpr>> = HashMap::new();
        let mut builtin_syms: HashMap<String, String> = HashMap::new();

        for (fn_name, info) in builtin_map {
            fn_ret_types.insert(fn_name.clone(), info.ret_ty.clone());
            fn_param_types.insert(fn_name.clone(), info.param_tys.clone());
            builtin_syms.insert(fn_name.clone(), info.c_sym.clone());
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
            enum_struct_variant_field_names: HashMap::new(),
            fn_ret_types,
            fn_param_types,
            has_println_fmt: false,
            has_int_fmt: false,
            has_str_true: false,
            has_str_false: false,
            lambda_counter: 0,
            closure_type_emitted: false,
            fn_aliases: HashMap::new(),
            tir_actor_decls: HashMap::new(),
            actor_runtime_declared: false,
            actor_emitted: HashSet::new(),
            yield_check_declared: false,
            builtin_syms,
            audit_relabels: HashMap::new(),
        }
    }
}

// ── Per-function state ───────────────────────────────────────────────────────

/// State for the function currently being emitted: SSA-register bookkeeping,
/// local-variable maps, heap-drop tracking. Replaced on every new function via
/// [`FnCtx::new`] — there is no manual reset method to keep in sync.
pub(super) struct FnCtx {
    // ── SSA bookkeeping ──────────────────────────────────────────────────
    pub fn_buf: Vec<String>,
    pub current_bb: String,
    pub terminated: bool,
    pub reg: usize,
    pub bb: usize,

    // ── Variable / type tracking ─────────────────────────────────────────
    pub locals: HashMap<String, String>,
    pub ref_locals: HashMap<String, RefLocal>,
    pub current_ret_ty: TypeExpr,
    /// SSA register → LLVM type string (for phi type inference).
    pub reg_types: HashMap<String, String>,
    /// MVL variable name → TypeExpr (for struct field access).
    pub local_mvl_types: HashMap<String, TypeExpr>,

    // ── Per-function flags ───────────────────────────────────────────────
    /// True while emitting `main` (affects `ret` instruction type).
    pub current_fn_is_main: bool,
    /// SSA registers of actor handles spawned in the current function.
    pub spawned_actor_handles: Vec<String>,

    // ── Heap drop tracking (#1185) ──────────────────────────────────────
    /// Heap-allocated locals: (ssa_or_alloca, kind, is_ref).
    pub heap_locals: Vec<(String, HeapKind, bool)>,

    // ── Entry-block alloca hoisting ──────────────────────────────────────
    /// Alloca instructions for `ref` locals that must live in the entry block
    /// so they dominate all uses, even when the binding is inside a branch (#1645).
    pub pre_allocas: Vec<String>,
}

impl FnCtx {
    /// Construct an empty per-function context. Called at the start of every
    /// `emit_fn` to replace the previous function's state — eliminates the
    /// need for a hand-maintained `reset_fn_state`.
    pub(super) fn new(ret_ty: TypeExpr) -> Self {
        Self {
            fn_buf: Vec::new(),
            current_bb: "entry".to_string(),
            terminated: false,
            reg: 0,
            bb: 0,
            locals: HashMap::new(),
            ref_locals: HashMap::new(),
            current_ret_ty: ret_ty,
            reg_types: HashMap::new(),
            local_mvl_types: HashMap::new(),
            current_fn_is_main: false,
            spawned_actor_handles: Vec::new(),
            heap_locals: Vec::new(),
            pre_allocas: Vec::new(),
        }
    }

    /// Initial state used before any function is being emitted (e.g. during
    /// program-level first/second passes that don't touch SSA state).
    pub(super) fn initial() -> Self {
        Self::new(TypeExpr::Base {
            name: "Unit".into(),
            args: vec![],
            span: Default::default(),
        })
    }
}

// ── Monomorphization queue ───────────────────────────────────────────────────

/// Generic-function discovery + emission queue (#1156, #1523).
pub(super) struct MonoQueue {
    /// Active type-parameter → concrete-type mapping during a monomorphized
    /// function's emission.
    pub type_param_map: HashMap<String, TypeExpr>,
    /// Generic TIR fn declarations (type_params non-empty), keyed by name.
    pub tir_generic_fns: HashMap<String, TirFn>,
    /// Mangled names of monomorphized TIR copies already emitted.
    pub tir_mono_emitted: HashSet<String>,
    /// Queue of monomorphized TIR fns to emit: (mangled, orig_name, concrete_types).
    pub tir_mono_queue: Vec<(String, String, Vec<Ty>)>,
}

impl MonoQueue {
    pub(super) fn new() -> Self {
        Self {
            type_param_map: HashMap::new(),
            tir_generic_fns: HashMap::new(),
            tir_mono_emitted: HashSet::new(),
            tir_mono_queue: Vec::new(),
        }
    }
}
