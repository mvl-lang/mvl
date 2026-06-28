// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust source emitter: string-builder with indentation tracking.
//!
//! [`RustEmitter`] is the single writer passed through every emit function.
//! All other `emit_*` modules take `&mut RustEmitter` and append to it.

use crate::mvl::backends::rust::capability_params::{
    build_capability_params_map_tir, explicit_borrow_flags_pub,
};
use crate::mvl::ir::{BinaryOp, TirFn, TirProgram};
use crate::mvl::parser::ast::Decl;
use crate::mvl::parser::lexer::Span;
use crate::mvl::passes::coverage::{BranchKind, CoverageMap};
use crate::mvl::passes::mcdc::transform::{DecisionKind, FnFieldReads, MCDCMap};
use crate::mvl::passes::mutation::{
    mutations_for_binary_op, mutations_for_int_literal, MutationMap,
};

/// Stdlib function names that shadow Rust built-ins or prelude items and must be
/// emitted as fully-qualified paths to avoid silent name resolution to the wrong symbol (#420).
const STDLIB_CONFLICTS: &[(&str, &[&str])] = &[
    (
        "regex",
        &["compile", "find", "find_all", "replace", "captures"],
    ),
    // #928: List[String]::join emits a prelude free function `fn join(...)` that
    // shadows the runtime's `io::join(Path, String) -> Path`. Same for `to_string`.
    ("io", &["join", "to_string"]),
];

///// Code-generation context: accumulates Rust source text.
#[derive(Default)]
pub struct RustEmitter {
    buf: String,
    indent: usize,
    /// Names of functions declared in `extern` blocks — calls must be wrapped in `unsafe`.
    ///
    /// Scoped to the `rust/` sub-module: read with [`has_extern_fn`] and
    /// mutate with [`register_extern_fn`].  External callers should not touch
    /// the underlying `HashSet` (#1394).
    pub(super) extern_fns: std::collections::HashSet<String>,
    /// True when the generated file uses `mvl_runtime::prelude::*`.
    ///
    /// When `true`, `ParseFromArgs`, `get_arg`, and other runtime symbols are
    /// in scope; the transpiler may emit impls that reference them.
    pub(super) use_mvl_runtime: bool,
    /// Active coverage map — `Some` when transpiling with `--coverage`.
    pub(super) coverage: Option<CoverageMap>,
    /// Active MC/DC map — `Some` when transpiling with `mvl mcdc`.
    pub(super) mcdc: Option<MCDCMap>,
    /// Per-function field-read sets for interprocedural MC/DC coupling analysis.
    /// Built from the current program before emission starts; empty when MC/DC
    /// instrumentation is inactive or for non-MC/DC transpilation passes.
    pub(super) mcdc_fn_field_reads: FnFieldReads,
    /// Active mutation map — `Some` when transpiling with `mvl mutate`.
    pub mutation: Option<MutationMap>,
    /// Name of the function currently being transpiled (for coverage metadata).
    pub current_fn: String,
    /// Stem of the source file being transpiled (for coverage metadata).
    pub current_file: String,
    /// True when the current function is a `test fn` (excluded from coverage report).
    pub current_fn_is_test: bool,
    /// True when transpiling a `_test.mvl` file.
    ///
    /// Non-test `fn` bodies in test files are re-declarations of source functions
    /// (workaround for #96) and must not generate mutation points.
    pub current_file_is_test: bool,
    /// When true, `extern "rust"` blocks are emitted as stub functions (using
    /// `todo!`) instead of real extern declarations.  Used when compiling source
    /// files into the test crate so the crate can link without the external dep.
    pub test_extern_stubs: bool,
    /// Tracks stub function names already emitted in test mode, preventing duplicate
    /// stub definitions when multiple prelude programs declare the same extern fn.
    pub(crate) emitted_extern_stub_fns: std::collections::HashSet<String>,
    /// Spans of last uses for the current function body (Phase A, Spec 009 Req 2).
    ///
    /// Populated by [`last_use::compute_last_uses`] before each function body is
    /// emitted.  [`emit_expr_as_arg`] suppresses `.clone()` when the argument's
    /// span appears here — emitting a Rust move instead.
    pub last_uses: std::collections::HashSet<Span>,
    /// Per-function borrow kinds (Phase B, Spec 009 Req 2).
    ///
    /// Maps function name → `Vec<Option<bool>>` where:
    /// * `Some(false)` at index `i` — parameter `i` is `val T` (read-only borrow); emit `&x` at call sites.
    /// * `Some(true)`  at index `i` — parameter `i` is `ref T` (mutable borrow); emit `&mut x`.
    /// * `None`        at index `i` — pass by value (clone / move as normal).
    ///
    /// Built once before emission from all [`FnDecl`] nodes in the program.
    ///
    /// Used in two places:
    /// * `emit_params` — wraps inferred-capability param types in `&` / `&mut ` (Rust output).
    /// * `emit_args` at call sites — emits `&x` / `&mut x` instead of `x.clone()`.
    pub capability_params_map: std::collections::HashMap<String, Vec<Option<bool>>>,
    /// Names of capability parameters in the function currently being emitted.
    ///
    /// Set at the start of each function body, cleared at the end.
    /// Used by let-binding emission to add `.clone()` when reading a field
    /// from a capability parameter (`acc.items` where `acc: &ParseAcc`).
    pub capability_param_names: std::collections::HashSet<String>,
    /// Fully-qualified Rust paths for stdlib function names that would shadow
    /// built-in primitives in the generated file (#420: regex.replace / regex.find).
    ///
    /// Populated during preamble emission for each `use std.MODULE.*` import.
    /// At FnCall emission time, a hit in this map causes the call to be emitted
    /// as `mvl_runtime::stdlib::MODULE::fn_name(...)` instead of `fn_name(...)`.
    pub stdlib_fn_qualified: std::collections::HashMap<String, String>,
    /// Controls how struct invariants are enforced at runtime (issue #662).
    pub assert_mode: crate::mvl::backends::AssertMode,
    /// Names of all methods (pub fn and fn) in the actor currently being emitted.
    ///
    /// Set by `emit_actor_decl` before emitting method bodies; cleared after.
    /// When non-empty, free calls to any name in this set are prefixed with
    /// `self.` so that `log(seq)` in an actor body becomes `self.log(seq)`.
    pub actor_methods: std::collections::HashSet<String>,
    /// Name of the actor handle struct being emitted (e.g. `"Pong"`).
    ///
    /// Non-empty only while emitting actor method bodies.  When non-empty,
    /// `Expr::Ident("self")` used as a call argument is replaced with
    /// `self._self_ref.as_ref().unwrap().clone()` — the actor's own handle
    /// stored in the state by `_start_<name>`.
    pub actor_self_type: String,
    /// #928: True when emitting a free-function body for an extension method on a
    /// built-in type. Causes `self` identifiers to be emitted as `self_`.
    pub self_as_free_param: bool,
    /// (struct_type_name, field_name) pairs where the field type is a function type.
    ///
    /// Used to distinguish `(obj.field)(args)` (fn-pointer call) from
    /// `obj.method(args)` (regular method call) in Rust output (#959).
    pub fn_typed_struct_fields: std::collections::HashSet<(String, String)>,
    /// True when the program contains at least one actor declaration.
    /// Set before top-level declarations are emitted; used to inject
    /// `_mvl_join_actors()` at the end of `fn main()` (#1048).
    pub has_actors: bool,
    /// When true, `emit_fn_body` appends `_mvl_join_actors()` as the
    /// implicit return expression after the last function statement.
    /// Set by `emit_fn_decl` for `fn main()` when `has_actors` is true.
    pub inject_actor_join: bool,
    /// IFC relabel transitions that carry the declaration-level `audit` keyword (#896).
    /// Maps transition name → (from_label, to_label) where `None` = bare `_`.
    /// Populated from `TirProgram.relabel_decls` at the start of `emit_program_core`.
    pub audit_relabels: std::collections::HashMap<String, (Option<String>, Option<String>)>,
    /// Refined type alias names → base type (e.g., `"Port" → Ty::Int`).
    ///
    /// Populated from `TirProgram.types` where `body == TirTypeBody::Alias(Ty::Refined(..))`.
    /// Used to insert newtype wrapping (`Type::new(expr)`) and unwrapping (`expr.0`) at
    /// coercion points: let bindings, assignments, function args, returns (#1326).
    pub refined_aliases: std::collections::HashMap<String, crate::mvl::ir::Ty>,
    /// Function name → parameter types.
    ///
    /// Built from `TirProgram.fns` at the start of emission. Used to detect when a
    /// call-site argument needs refined alias wrapping (#1326).
    pub fn_param_types: std::collections::HashMap<String, Vec<crate::mvl::ir::Ty>>,
    /// Fn-type alias name → resolved `Ty::Fn(..)`.
    ///
    /// Populated from `TirProgram.types` where `body == TirTypeBody::Alias(Ty::Fn(..))`.
    /// Used so HOF parameter borrow-flag propagation (#960) also fires when the param
    /// type is a named fn-type alias such as `type Dispatcher = fn(val T) -> U` (#1467).
    pub fn_aliases: std::collections::HashMap<String, crate::mvl::ir::Ty>,
    /// Per-prelude file stems, parallel to the `prelude_tirs` slice passed to
    /// `emit_program_core`. `Some(stem)` enables file-aware coverage metadata for
    /// that prelude entry; `None` falls back to `current_file` (#1489).
    pub prelude_stems: Vec<Option<String>>,
    /// Subset of `prelude_stems` whose functions should be emitted with branch
    /// probes — used so sibling library files paired with test files appear in
    /// the coverage report (#1489).
    pub coverage_instrument_prelude: std::collections::HashSet<String>,
    /// Dispatch table for cross-package function name collisions (#1475).
    ///
    /// When two packages export a function with the same name (e.g. `status_reason`
    /// from both `pkg.http` and `pkg.health`), their Rust functions are emitted with
    /// package-prefixed names (`http__status_reason`, `health__status_reason`).
    ///
    /// Key: `(original_fn_name, return_ty_rust_str)` — the return type string
    /// uniquely identifies which variant the checker resolved a given call to.
    /// Value: the Rust function name to emit (`"http__status_reason"`).
    pub pkg_fn_dispatch: std::collections::HashMap<(String, String), String>,
}

impl RustEmitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finish generation and return the accumulated source string.
    pub fn finish(self) -> String {
        self.buf
    }

    /// Register `name` as an extern-block function; returns `true` if newly
    /// inserted (matches [`HashSet::insert`] semantics).  Callers in sibling
    /// modules (`emit_types`) use this to dedup re-emission of the same
    /// extern across multiple prelude programs.
    pub(super) fn register_extern_fn(&mut self, name: String) -> bool {
        self.extern_fns.insert(name)
    }

    /// True when `name` was registered via [`register_extern_fn`] — call
    /// sites use this to decide whether to wrap the call in `unsafe { … }`.
    pub(super) fn has_extern_fn(&self, name: &str) -> bool {
        self.extern_fns.contains(name)
    }

    /// Check if a `Ty::Named` refers to a refined alias; return the base type if so.
    pub fn refined_alias_base(&self, ty: &crate::mvl::ir::Ty) -> Option<&crate::mvl::ir::Ty> {
        if let crate::mvl::ir::Ty::Named(name, _) = ty {
            self.refined_aliases.get(name.as_str())
        } else {
            None
        }
    }

    /// If `ty` is a named alias for a `Ty::Fn(..)`, return the resolved Fn type (#1467).
    /// Returns `None` for non-alias types or aliases to non-Fn types.
    pub fn resolve_fn_alias<'a>(
        &'a self,
        ty: &'a crate::mvl::ir::Ty,
    ) -> Option<&'a crate::mvl::ir::Ty> {
        if let crate::mvl::ir::Ty::Named(name, _) = ty {
            self.fn_aliases.get(name.as_str())
        } else {
            None
        }
    }

    /// Return (from_label, to_label) strings for a relabel transition (#896).
    ///
    /// Used when emitting `emit_relabel_event` calls for audited relabels.
    /// Built-in transitions are hardcoded; user-defined ones are looked up
    /// from `self.audit_relabels`.
    pub fn relabel_label_strings(&self, name: &str) -> (String, String) {
        // Check user-defined first (covers case where built-in was redeclared).
        if let Some((from, to)) = self.audit_relabels.get(name) {
            let f = from.as_deref().unwrap_or("_").to_string();
            let t = to.as_deref().unwrap_or("_").to_string();
            return (f, t);
        }
        // Built-in transitions (#894, #931).
        let (f, t) = match name {
            "classify" => ("_", "Secret"),
            "taint" => ("_", "Tainted"),
            "trust" => ("Tainted", "_"),
            "release" => ("Secret", "_"),
            "config_path" => ("_", "ConfigPath"),
            "unconfig_path" => ("ConfigPath", "_"),
            "db_url" => ("_", "DbUrl"),
            "undb_url" => ("DbUrl", "_"),
            "api_endpoint" => ("_", "ApiEndpoint"),
            "unapi_endpoint" => ("ApiEndpoint", "_"),
            "audit_target" => ("_", "AuditTarget"),
            "unaudit_target" => ("AuditTarget", "_"),
            _ => ("_", "_"),
        };
        (f.to_string(), t.to_string())
    }

    // ── Low-level writers ────────────────────────────────────────────────

    pub fn push(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    pub fn push_char(&mut self, c: char) {
        self.buf.push(c);
    }

    /// Emit a newline.
    pub fn nl(&mut self) {
        self.buf.push('\n');
    }

    /// Emit current indentation (spaces).
    pub fn indent(&mut self) {
        for _ in 0..self.indent * 4 {
            self.buf.push(' ');
        }
    }

    /// Emit indentation + text + newline.
    pub fn line(&mut self, s: &str) {
        self.indent();
        self.buf.push_str(s);
        self.buf.push('\n');
    }

    /// Emit a blank line (just a newline).
    pub fn blank(&mut self) {
        self.buf.push('\n');
    }

    // ── Coverage helpers ──────────────────────────────────────────────────

    /// Allocate a coverage counter for a decision branch.
    ///
    /// Returns `Some(id)` when coverage is active, `None` otherwise.
    /// Separating allocation from emission avoids simultaneous borrow conflicts.
    pub fn alloc_branch(&mut self, line: u32, kind: BranchKind) -> Option<usize> {
        let fn_name = self.current_fn.clone();
        let file = self.current_file.clone();
        let is_test_fn = self.current_fn_is_test;
        self.coverage
            .as_mut()
            .map(|c| c.alloc(fn_name, file, line, kind, is_test_fn))
    }

    /// Emit a `#[cfg(test)] crate::__mvl_cov::hit(id);` statement at current indent.
    pub fn emit_cov_hit(&mut self, id: usize) {
        self.line(&format!("#[cfg(test)] crate::__mvl_cov::hit({id});"));
    }

    // ── MC/DC helpers ────────────────────────────────────────────────────

    /// Allocate an MC/DC decision slot for a compound boolean condition.
    ///
    /// Returns `Some(id)` when MC/DC instrumentation is active, `None` otherwise.
    /// Test functions are excluded (`current_fn_is_test`).  The `current_file_is_test`
    /// guard is retained for defence-in-depth but is effectively dead code today:
    /// only `transpile_mutated_with_prelude` sets that flag, and it does not enable
    /// MC/DC.  Mutation helpers use only `current_fn_is_test` (see
    /// `alloc_binary_mutations`).
    pub fn alloc_mcdc_decision(
        &mut self,
        line: u32,
        clause_count: usize,
        kind: DecisionKind,
        coupled_pairs: Vec<(usize, usize, Vec<String>)>,
    ) -> Option<usize> {
        if self.current_fn_is_test || self.current_file_is_test {
            return None;
        }
        let fn_name = self.current_fn.clone();
        let file = self.current_file.clone();
        self.mcdc
            .as_mut()
            .map(|m| m.alloc(fn_name, file, line, clause_count, kind, coupled_pairs))
    }

    // ── Mutation helpers ──────────────────────────────────────────────────

    /// Allocate mutation variants for a binary operator.
    ///
    /// Returns `Some(vec)` of `(mutant_id, rust_op_str)` pairs when mutation is
    /// active and the operator has behavioral alternatives.  Returns `None` when
    /// mutation is inactive or no alternatives exist.
    pub fn alloc_binary_mutations(
        &mut self,
        op: BinaryOp,
        line: u32,
    ) -> Option<Vec<(String, &'static str)>> {
        if self.current_fn_is_test {
            return None;
        }
        let alts = mutations_for_binary_op(op);
        if alts.is_empty() {
            return None;
        }
        let fn_name = self.current_fn.clone();
        let file = self.current_file.clone();
        let mutation_map = self.mutation.as_mut()?;
        let mut result = Vec::new();
        for (op_str, desc) in alts {
            let id = mutation_map.alloc(fn_name.clone(), file.clone(), line, desc.to_string());
            result.push((id, *op_str));
        }
        Some(result)
    }

    /// Allocate a mutation variant for a boolean literal flip.
    ///
    /// Returns `Some(mutant_id)` when mutation is active, `None` otherwise.
    pub fn alloc_bool_mutation(&mut self, original: bool, line: u32) -> Option<String> {
        if self.current_fn_is_test {
            return None;
        }
        let fn_name = self.current_fn.clone();
        let file = self.current_file.clone();
        let desc = format!("BoolLiteral({original} → {})", !original);
        let id = self.mutation.as_mut()?.alloc(fn_name, file, line, desc);
        Some(id)
    }

    /// Allocate mutation variants for an integer literal.
    ///
    /// Returns `Some(vec)` of `(mutant_id, replacement_value)` when mutation is
    /// active and alternatives exist.  Returns `None` when mutation is inactive
    /// or the literal has no distinct alternatives.
    pub fn alloc_int_mutations(&mut self, original: i64, line: u32) -> Option<Vec<(String, i64)>> {
        if self.current_fn_is_test {
            return None;
        }
        let alts = mutations_for_int_literal(original);
        if alts.is_empty() {
            return None;
        }
        let fn_name = self.current_fn.clone();
        let file = self.current_file.clone();
        let mutation_map = self.mutation.as_mut()?;
        let mut result = Vec::new();
        for alt in alts {
            let id = mutation_map.alloc(
                fn_name.clone(),
                file.clone(),
                line,
                format!("IntLiteral({original} → {alt})"),
            );
            result.push((id, alt));
        }
        Some(result)
    }

    /// Increase indent level.
    pub fn push_indent(&mut self) {
        self.indent += 1;
    }

    /// Decrease indent level (saturating).
    pub fn pop_indent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    // ── Program-level emit ───────────────────────────────────────────────

    /// Emit a complete Rust file from a [`TirProgram`].
    pub fn emit_program(&mut self, tir: &TirProgram) {
        self.emit_program_core(tir, &[], false, &[]);
    }

    /// Emit a complete Rust file from a [`TirProgram`], prepending `pub mod` declarations
    /// for each sibling module name in `sibling_mods` (used by multi-file project builds).
    /// `prelude_progs` are stdlib programs whose non-stub functions are emitted before
    /// the user program's declarations so they are available at the call site.
    pub fn emit_program_with_mods(
        &mut self,
        tir: &TirProgram,
        sibling_mods: &[&str],
        prelude_tirs: &[TirProgram],
    ) {
        self.emit_program_core(tir, sibling_mods, false, prelude_tirs);
    }

    /// Emit a sibling module file using the TIR-based path.
    ///
    /// Use when the project entry point has `extern` blocks — the sibling must use the same
    /// runtime types (`Tainted`, `Clean`, etc.) to avoid duplicate-definition conflicts.
    /// `prelude_tirs` are the prelude programs pre-lowered to TIR, so that stdlib functions
    /// (e.g. `to_lower`, `range`) are available in sibling modules too.
    pub fn emit_sibling_module(&mut self, tir: &TirProgram, prelude_tirs: &[TirProgram]) {
        self.emit_program_core(tir, &[], true, prelude_tirs);
    }

    fn emit_program_core(
        &mut self,
        tir: &TirProgram,
        sibling_mods: &[&str],
        force_runtime: bool,
        prelude_tirs: &[TirProgram],
    ) {
        // Populate audit_relabels from declaration-level `audit` keywords (#896).
        self.audit_relabels = tir
            .relabel_decls
            .iter()
            .filter(|rd| rd.audit)
            .map(|rd| (rd.name.clone(), (rd.from.clone(), rd.to.clone())))
            .collect();

        // Populate refined alias registry (#1326).
        // Maps alias name → base type for refined aliases (emitted as newtypes).
        for td in &tir.types {
            if let crate::mvl::ir::TirTypeBody::Alias(crate::mvl::ir::Ty::Refined(inner, _)) =
                &td.body
            {
                self.refined_aliases
                    .insert(td.name.clone(), inner.as_ref().clone());
            }
        }
        for pt in prelude_tirs {
            for td in &pt.types {
                if let crate::mvl::ir::TirTypeBody::Alias(crate::mvl::ir::Ty::Refined(inner, _)) =
                    &td.body
                {
                    self.refined_aliases
                        .insert(td.name.clone(), inner.as_ref().clone());
                }
            }
        }

        // Populate fn-alias registry (#1467).
        // Maps alias name → resolved `Ty::Fn(..)` so HOF cap-propagation (#960) sees
        // through aliases like `type Dispatcher = fn(val T) -> U`.
        for td in &tir.types {
            if let crate::mvl::ir::TirTypeBody::Alias(fn_ty @ crate::mvl::ir::Ty::Fn(..)) = &td.body
            {
                self.fn_aliases.insert(td.name.clone(), fn_ty.clone());
            }
        }
        for pt in prelude_tirs {
            for td in &pt.types {
                if let crate::mvl::ir::TirTypeBody::Alias(fn_ty @ crate::mvl::ir::Ty::Fn(..)) =
                    &td.body
                {
                    self.fn_aliases.insert(td.name.clone(), fn_ty.clone());
                }
            }
        }

        // Populate fn_param_types for refined alias wrapping at call sites (#1326).
        for f in &tir.fns {
            self.fn_param_types.insert(
                f.name.clone(),
                f.params.iter().map(|p| p.ty.clone()).collect(),
            );
        }
        for pt in prelude_tirs {
            for f in &pt.fns {
                self.fn_param_types.insert(
                    f.name.clone(),
                    f.params.iter().map(|p| p.ty.clone()).collect(),
                );
            }
        }

        self.line("// Generated by MVL transpiler — do not edit manually.");
        self.line("#![allow(dead_code, unused_variables, unused_imports, unused_parens, unpredictable_function_pointer_comparisons)]");
        self.blank();

        let prelude_has_extern = prelude_tirs
            .iter()
            .any(|t| !t.externs.is_empty() || t.fns.iter().any(|f| f.is_builtin));
        let has_runtime = force_runtime
            || prelude_has_extern
            || !tir.externs.is_empty()
            || tir
                .uses
                .iter()
                .any(|ud| ud.path.first().map(|s| s == "std").unwrap_or(false));

        if has_runtime {
            self.use_mvl_runtime = true;
            self.line("use mvl_runtime::prelude::*;");
            self.blank();

            let mut all_modules: Vec<String> = {
                let mut seen = std::collections::HashSet::new();
                tir.uses
                    .iter()
                    .filter_map(|ud| {
                        if ud.path.first().map(|s| s == "std").unwrap_or(false)
                            && ud.path.len() >= 2
                        {
                            let m = ud.path[1].as_str();
                            if crate::mvl::backends::rust::RUST_RUNTIME_IMPORTS.contains(&m) {
                                Some(ud.path[1].clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .filter(|m| seen.insert(m.clone()))
                    .collect()
            };
            for pt in prelude_tirs {
                for ud in &pt.uses {
                    if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
                        let m = ud.path[1].as_str();
                        if crate::mvl::backends::rust::RUST_RUNTIME_IMPORTS.contains(&m)
                            && !all_modules.contains(&m.to_string())
                        {
                            all_modules.push(m.to_string());
                        }
                    }
                }
            }
            if !all_modules.contains(&"io".to_string()) {
                all_modules.push("io".to_string());
            }
            for module in &all_modules {
                self.line(&format!("use mvl_runtime::stdlib::{}::*;", module));
                for (m, fns) in STDLIB_CONFLICTS {
                    if *m == module.as_str() {
                        for fn_name in *fns {
                            self.stdlib_fn_qualified.insert(
                                (*fn_name).to_string(),
                                format!("mvl_runtime::stdlib::{module}::{fn_name}"),
                            );
                        }
                    }
                }
            }
        } else {
            self.emit_security_preamble();
        }
        self.blank();

        for mod_name in sibling_mods {
            self.line(&format!("pub mod {mod_name};"));
        }
        if !sibling_mods.is_empty() {
            self.blank();
        }

        // Qualify by (receiver_type, original_name) for extension methods on user-defined
        // types, so a free function `fail` and `AuditEvent::fail` are treated as distinct
        // symbols and don't shadow each other during prelude dedup.
        //
        // Extension methods on builtin types (String, List, Map, …) are emitted as UFCS-style
        // free Rust functions (not impl methods), so they still use a bare name key — allowing
        // the existing dedup to prevent multiple definitions of the same Rust free function.
        const BUILTIN_RECEIVER_TYPES: &[&str] = &[
            "String", "Int", "Float", "Bool", "Byte", "UByte", "UInt", "List", "Map", "Set",
            "Option", "Result",
        ];
        let fn_key = |f: &crate::mvl::ir::TirFn| -> String {
            let base = match &f.receiver_type {
                Some(r) if !BUILTIN_RECEIVER_TYPES.contains(&r.as_str()) => {
                    format!("{}::{}", r, f.original_name)
                }
                _ => f.original_name.clone(),
            };
            // Include package name so colliding functions from different packages
            // are treated as distinct entries and both get emitted (#1475).
            match &f.pkg_name {
                Some(pkg) => format!("{}/{}", pkg, base),
                None => base,
            }
        };
        let user_fn_keys: std::collections::HashSet<String> = tir.fns.iter().map(fn_key).collect();
        let user_type_names: std::collections::HashSet<&str> =
            tir.types.iter().map(|t| t.name.as_str()).collect();
        let user_actor_names: std::collections::HashSet<&str> =
            tir.actors.iter().map(|a| a.name.as_str()).collect();

        let mut seen_prelude_fns: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Pair each prelude fn with the index of the TIR it came from so we can
        // route per-file coverage state below (#1489).
        let prelude_fns: Vec<(usize, &crate::mvl::ir::TirFn)> = prelude_tirs
            .iter()
            .enumerate()
            .flat_map(|(idx, t)| t.fns.iter().map(move |f| (idx, f)))
            .filter(|(_, f)| !f.is_builtin)
            .filter(|(_, f)| !f.body.stmts.is_empty())
            .filter(|(_, f)| !f.is_test)
            .filter(|(_, f)| !user_fn_keys.contains(&fn_key(f)))
            .filter(|(_, f)| seen_prelude_fns.insert(fn_key(f)))
            .collect();

        // Build dispatch table for function name collisions.
        // For each function name that appears in 2+ packages (#1475) OR is
        // shadowed by a user-defined function (#1587), map
        // (original_name, return_ty_rust) → pkg_prefixed_rust_name so the
        // pkg variant emits and is called via its prefixed name while the
        // user (or sole-pkg) variant keeps the bare name.
        self.pkg_fn_dispatch.clear();
        {
            use crate::mvl::backends::rust::emit_types::emit_ty;
            let user_fn_names: std::collections::HashSet<&str> = tir
                .fns
                .iter()
                .filter(|f| f.pkg_name.is_none())
                .map(|f| f.original_name.as_str())
                .collect();
            let mut name_to_variants: std::collections::HashMap<
                &str,
                Vec<(&str, &crate::mvl::ir::TirFn)>,
            > = std::collections::HashMap::new();
            for (_, f) in &prelude_fns {
                if let Some(ref pkg) = f.pkg_name {
                    name_to_variants
                        .entry(f.original_name.as_str())
                        .or_default()
                        .push((pkg.as_str(), f));
                }
            }
            for (name, variants) in &name_to_variants {
                let shadowed_by_user = user_fn_names.contains(name);
                if variants.len() > 1 || shadowed_by_user {
                    for (pkg, f) in variants {
                        let ret_key = emit_ty(&f.ret_ty);
                        let prefixed = format!("{}__{}", pkg, name);
                        self.pkg_fn_dispatch
                            .insert((name.to_string(), ret_key), prefixed);
                    }
                }
            }
        }

        let mut seen_prelude_types: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        let prelude_types: Vec<&crate::mvl::ir::TirTypeDecl> = prelude_tirs
            .iter()
            .flat_map(|t| t.types.iter())
            .filter(|t| !user_type_names.contains(t.name.as_str()))
            .filter(|t| seen_prelude_types.insert(t.name.as_str()))
            .collect();

        // Prelude actors — actors defined in library files loaded as preludes.
        // Entry-TIR actors take precedence: if the same actor appears in both, the
        // entry-TIR copy is used and the prelude copy is skipped. Matches the
        // dedup behaviour of prelude_fns and prelude_types.
        let mut seen_prelude_actors: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        let prelude_actors: Vec<&crate::mvl::ir::TirActorDecl> = prelude_tirs
            .iter()
            .flat_map(|t| t.actors.iter())
            .filter(|a| !user_actor_names.contains(a.name.as_str()))
            .filter(|a| seen_prelude_actors.insert(a.name.as_str()))
            .collect();

        // Phase B: capability params map from TIR
        self.capability_params_map = build_capability_params_map_tir(tir, prelude_tirs);

        // Rust-backed stdlib modules — scan for capability params
        {
            use crate::mvl::backends::rust::RUST_BACKED_STDLIB;
            use crate::mvl::parser::Parser;
            use crate::mvl::stdlib;
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for ud in &tir.uses {
                if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
                    let m = ud.path[1].as_str();
                    if RUST_BACKED_STDLIB.contains(&m) && seen.insert(m.to_string()) {
                        let filename = format!("{m}.mvl");
                        if let Some(content) = stdlib::stdlib_content(&filename) {
                            let (mut p, _) = Parser::new(content);
                            let loaded = p.parse_program();
                            for d in &loaded.declarations {
                                if let Decl::Fn(fd) = d {
                                    if !self.capability_params_map.contains_key(&fd.name) {
                                        let flags = explicit_borrow_flags_pub(&fd.params);
                                        if flags.iter().any(|b| b.is_some()) {
                                            self.capability_params_map
                                                .insert(fd.name.clone(), flags);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // #959: fn-typed struct fields
        self.fn_typed_struct_fields = collect_fn_typed_struct_fields_tir(tir, prelude_tirs);

        // Prelude extern blocks (from TIR)
        let prelude_externs: Vec<&crate::mvl::ir::TirExternDecl> =
            prelude_tirs.iter().flat_map(|t| t.externs.iter()).collect();

        if !prelude_types.is_empty()
            || !prelude_fns.is_empty()
            || !prelude_externs.is_empty()
            || !prelude_actors.is_empty()
        {
            self.line(
                "// ── stdlib prelude (transpiled from MVL source) ──────────────────────────",
            );
            // Move instrumentation state out by default; per-fn loop below
            // re-enables it for prelude entries marked for coverage (#1489).
            let mut coverage_holder = self.coverage.take();
            let mut mutation_holder = self.mutation.take();
            let mut mcdc_holder = self.mcdc.take();
            let saved_current_file = self.current_file.clone();
            for ed in prelude_externs {
                self.emit_tir_extern_decl(ed);
                self.blank();
            }
            // Names of types re-exported from the crate root in sibling modules.
            // Methods on these types must not be re-emitted to avoid E0592.
            let reexported_type_names: std::collections::HashSet<&str> = if force_runtime {
                prelude_types.iter().map(|t| t.name.as_str()).collect()
            } else {
                std::collections::HashSet::new()
            };
            for td in &prelude_types {
                if force_runtime {
                    self.line(&format!("pub use crate::{};", td.name));
                } else {
                    self.emit_tir_type_decl(td);
                }
                self.blank();
            }
            for (tir_idx, fd) in prelude_fns {
                if let Some(rty) = &fd.receiver_type {
                    if reexported_type_names.contains(rty.as_str()) {
                        continue;
                    }
                }
                let stem = self
                    .prelude_stems
                    .get(tir_idx)
                    .and_then(|s| s.as_deref())
                    .map(|s| s.to_owned());
                let instrument = stem
                    .as_deref()
                    .map(|s| self.coverage_instrument_prelude.contains(s))
                    .unwrap_or(false);
                if instrument {
                    // Restore instrumentation state and route metadata to the
                    // library file stem so probes are reported under e.g.
                    // `json.mvl` rather than the enclosing test crate stem.
                    self.coverage = coverage_holder.take();
                    self.mutation = mutation_holder.take();
                    self.mcdc = mcdc_holder.take();
                    if let Some(s) = &stem {
                        self.current_file = s.clone();
                    }
                    self.emit_fn_decl(fd);
                    self.blank();
                    coverage_holder = self.coverage.take();
                    mutation_holder = self.mutation.take();
                    mcdc_holder = self.mcdc.take();
                    self.current_file = saved_current_file.clone();
                } else {
                    self.emit_fn_decl(fd);
                    self.blank();
                }
            }
            self.coverage = coverage_holder;
            self.mutation = mutation_holder;
            self.mcdc = mcdc_holder;
            self.current_file = saved_current_file;
        }

        // Actor runtime preamble — needed for both entry-TIR and prelude actors.
        if !tir.actors.is_empty() || !prelude_actors.is_empty() {
            self.has_actors = true;
            self.emit_actor_runtime_preamble();
            self.blank();
        }

        // Top-level TIR declarations
        for td in &tir.types {
            self.emit_tir_type_decl(td);
            self.blank();
        }
        for ed in &tir.externs {
            self.emit_tir_extern_decl(ed);
            self.blank();
        }
        for ud in &tir.uses {
            if ud.path.len() > 1 {
                let is_std = ud.path.first().map(|s| s == "std").unwrap_or(false);
                let is_pkg = ud.path.first().map(|s| s == "pkg").unwrap_or(false);
                if !is_std && !is_pkg && !self.test_extern_stubs {
                    self.line(&format!("use crate::{};", ud.path.join("::")));
                }
            } else if ud.path.len() == 1 {
                let mod_name = &ud.path[0];
                let is_std = mod_name == "std";
                let is_pkg = mod_name == "pkg";
                if !is_std && !is_pkg && !self.test_extern_stubs {
                    self.line(&format!("use crate::{}::*;", mod_name));
                }
            }
        }
        // Actor bodies carry no coverage/mutation probes, so no per-file
        // instrumentation routing is needed here (unlike prelude_fns).
        for ad in prelude_actors {
            self.emit_actor_decl(ad);
            self.blank();
        }
        for ad in &tir.actors {
            self.emit_actor_decl(ad);
            self.blank();
        }
        for id in &tir.impls {
            self.emit_impl_decl(id);
            self.blank();
        }
        for fd in tir.fns.iter().filter(|f| !f.is_test) {
            self.emit_fn_decl(fd);
            self.blank();
        }

        // Emit placeholder stubs for external types referenced but not defined.
        let stubs = collect_undefined_types_tir(tir, prelude_tirs);
        if !stubs.is_empty() {
            self.line(
                "// ── External type stubs (Phase 1 placeholders) ──────────────────────────",
            );
            for name in &stubs {
                self.line("#[derive(Debug, Clone, PartialEq)]");
                self.line(&format!("pub struct {name};"));
                self.blank();
            }
        }

        // Test functions
        let test_fns: Vec<&TirFn> = tir.fns.iter().filter(|f| f.is_test).collect();
        if !test_fns.is_empty() {
            self.line("#[cfg(test)]");
            self.line("mod tests {");
            self.push_indent();
            self.line("use super::*;");
            self.blank();
            for fd in test_fns {
                self.emit_fn_decl(fd);
                self.blank();
            }
            self.pop_indent();
            self.line("}");
        }
    }
}

// ── Undefined type collection ─────────────────────────────────────────────

/// Collect the names of all base types referenced in the program that are
/// TIR-based version: collect undefined types from [`TirProgram`] function signatures.
fn collect_undefined_types_tir(tir: &TirProgram, prelude_tirs: &[TirProgram]) -> Vec<String> {
    use crate::mvl::ir::Ty;

    let builtins: std::collections::HashSet<&str> = [
        "Int",
        "Float",
        "Bool",
        "String",
        "Char",
        "Byte",
        "Unit",
        "Never",
        "List",
        "Public",
        "Tainted",
        "Secret",
        "Clean",
        "Option",
        "Result",
        "Vec",
        "Box",
        "Positional",
        "UByte",
        "UInt",
        "Map",
        "Set",
        "Array",
    ]
    .iter()
    .copied()
    .collect();

    let mut defined: std::collections::HashSet<String> = std::collections::HashSet::new();
    for td in &tir.types {
        defined.insert(td.name.clone());
    }
    for ad in &tir.actors {
        defined.insert(ad.name.clone());
    }
    for pt in prelude_tirs {
        for td in &pt.types {
            defined.insert(td.name.clone());
        }
        for ad in &pt.actors {
            defined.insert(ad.name.clone());
        }
    }

    // Types imported from sibling modules (e.g. `use game::Direction`, `use models::{User, Req}`)
    for ud in &tir.uses {
        let is_std = ud.path.first().map(|s| s == "std").unwrap_or(false);
        let is_pkg = ud.path.first().map(|s| s == "pkg").unwrap_or(false);
        if !is_std && !is_pkg {
            // `use game::Direction` → path = ["game", "Direction"], last segment is the type/fn
            if ud.path.len() >= 2 {
                let imported_name = ud.path.last().unwrap();
                defined.insert(imported_name.clone());
            }
            // `use models::{User, Req}` → items = ["User", "Req"]
            for item in &ud.items {
                defined.insert(item.clone());
            }
        }
    }

    // Types from Rust-backed stdlib modules
    for ud in &tir.uses {
        if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
            let module = &ud.path[1];
            let filename = format!("{module}.mvl");
            if let Some(content) = crate::mvl::stdlib::stdlib_content(&filename) {
                let (mut p, _) = crate::mvl::parser::Parser::new(content);
                let stdlib_prog = p.parse_program();
                for d in &stdlib_prog.declarations {
                    if let Decl::Type(td) = d {
                        defined.insert(td.name.clone());
                    }
                }
            }
        }
    }

    fn collect_ty_names(ty: &Ty, out: &mut std::collections::HashSet<String>) {
        match ty {
            Ty::Named(name, args) => {
                out.insert(name.clone());
                for a in args {
                    collect_ty_names(a, out);
                }
            }
            Ty::List(inner) | Ty::Set(inner) | Ty::Option(inner) | Ty::Labeled(_, inner) => {
                collect_ty_names(inner, out);
            }
            Ty::Map(k, v) | Ty::Result(k, v) => {
                collect_ty_names(k, out);
                collect_ty_names(v, out);
            }
            Ty::Ref(_, inner) => collect_ty_names(inner, out),
            Ty::Fn(params, ret, _, _) => {
                for p in params {
                    collect_ty_names(p, out);
                }
                collect_ty_names(ret, out);
            }
            _ => {}
        }
    }

    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in &tir.fns {
        for p in &f.params {
            collect_ty_names(&p.ty, &mut referenced);
        }
        collect_ty_names(&f.ret_ty, &mut referenced);
    }

    let mut stubs: Vec<String> = referenced
        .into_iter()
        .filter(|name| !defined.contains(name) && !builtins.contains(name.as_str()))
        .collect();
    stubs.sort();
    stubs
}

/// not defined by a `TypeDecl` in this program and are not MVL built-ins.
/// Returns a sorted, deduplicated list suitable for emitting stub structs.
/// `prelude_progs` types are treated as already-defined so they are never stubbed.
fn collect_fn_typed_struct_fields_tir(
    tir: &TirProgram,
    prelude_tirs: &[TirProgram],
) -> std::collections::HashSet<(String, String)> {
    let mut out = std::collections::HashSet::new();
    let all_tirs = std::iter::once(tir).chain(prelude_tirs.iter());
    for t in all_tirs {
        for td in &t.types {
            if let crate::mvl::ir::TirTypeBody::Struct { fields, .. } = &td.body {
                for field in fields {
                    if matches!(&field.ty, crate::mvl::checker::types::Ty::Fn(..)) {
                        out.insert((td.name.clone(), field.name.clone()));
                    }
                }
            }
        }
    }
    out
}
