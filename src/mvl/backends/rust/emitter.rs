// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust source emitter: string-builder with indentation tracking.
//!
//! [`RustEmitter`] is the single writer passed through every emit function.
//! All other `emit_*` modules take `&mut RustEmitter` and append to it.

use crate::mvl::backends::rust::borrow_params::build_borrow_params_map;
use crate::mvl::backends::rust::emit_functions::emit_fn_decl;
use crate::mvl::backends::rust::emit_impls::emit_impl_decl;
use crate::mvl::backends::rust::emit_types::emit_type_decl;
use crate::mvl::backends::rust::emit_types::{emit_security_preamble, emit_type_expr};
use crate::mvl::backends::rust::{collect_stdlib_modules, has_std_imports};
use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::{
    BinaryOp, Decl, ExternDecl, FieldDecl, FnDecl, Param, Program, TypeDecl, TypeExpr, Variant,
    VariantFields,
};
use crate::mvl::parser::lexer::Span;
use crate::mvl::passes::coverage::{BranchKind, CoverageMap};
use crate::mvl::passes::mcdc::transform::{DecisionKind, MCDCMap};
use crate::mvl::passes::mutation::{
    mutations_for_binary_op, mutations_for_int_literal, MutationMap,
};

/// Stdlib function names that shadow Rust built-ins or prelude items and must be
/// emitted as fully-qualified paths to avoid silent name resolution to the wrong symbol (#420).
const STDLIB_CONFLICTS: &[(&str, &[&str])] = &[(
    "regex",
    &["compile", "find", "find_all", "replace", "captures"],
)];

///// Code-generation context: accumulates Rust source text.
#[derive(Default)]
pub struct RustEmitter {
    buf: String,
    indent: usize,
    /// Names of functions declared in `extern` blocks — calls must be wrapped in `unsafe`.
    pub extern_fns: std::collections::HashSet<String>,
    /// True when the generated file uses `mvl_runtime::prelude::*`.
    ///
    /// When `true`, `ParseFromArgs`, `get_arg`, and other runtime symbols are
    /// in scope; the transpiler may emit impls that reference them.
    pub use_mvl_runtime: bool,
    /// Active coverage map — `Some` when transpiling with `--coverage`.
    pub coverage: Option<CoverageMap>,
    /// Active MC/DC map — `Some` when transpiling with `mvl mcdc`.
    pub mcdc: Option<MCDCMap>,
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
    /// * `emit_params` — wraps inferred-borrow param types in `&` / `&mut ` (Rust output).
    /// * `emit_args` at call sites — emits `&x` / `&mut x` instead of `x.clone()`.
    pub borrow_params_map: std::collections::HashMap<String, Vec<Option<bool>>>,
    /// Fully-qualified Rust paths for stdlib function names that would shadow
    /// built-in primitives in the generated file (#420: regex.replace / regex.find).
    ///
    /// Populated during preamble emission for each `use std.MODULE.*` import.
    /// At FnCall emission time, a hit in this map causes the call to be emitted
    /// as `mvl_runtime::stdlib::MODULE::fn_name(...)` instead of `fn_name(...)`.
    pub stdlib_fn_qualified: std::collections::HashMap<String, String>,
    /// Inferred type for every expression, keyed by span.
    ///
    /// Populated from [`CheckResult::expr_types`] before emission so that
    /// method-call sites can emit type-specific Rust (e.g. `.len() as i64` vs
    /// `.chars().count() as i64`) without needing trait dispatch (#554).
    pub expr_types: std::collections::HashMap<Span, Ty>,
}

impl RustEmitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finish generation and return the accumulated source string.
    pub fn finish(self) -> String {
        self.buf
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

    /// Emit a complete Rust file from a [`Program`].
    pub fn emit_program(&mut self, prog: &Program) {
        self.emit_program_core(prog, &[], false, &[]);
    }

    /// Emit a complete Rust file from a [`Program`], prepending `pub mod` declarations
    /// for each sibling module name in `sibling_mods` (used by multi-file project builds).
    /// `prelude_progs` are stdlib programs whose non-stub functions are emitted before
    /// the user program's declarations so they are available at the call site.
    pub fn emit_program_with_mods(
        &mut self,
        prog: &Program,
        sibling_mods: &[&str],
        prelude_progs: &[Program],
    ) {
        self.emit_program_core(prog, sibling_mods, false, prelude_progs);
    }

    /// Emit a sibling module file that shares the `mvl_runtime` prelude with the crate root.
    ///
    /// Use when the project entry point has `extern` blocks — the sibling must use the same
    /// runtime types (`Tainted`, `Clean`, etc.) to avoid duplicate-definition conflicts.
    /// `prelude_progs` are the same stdlib prelude programs passed to the entry point so that
    /// stdlib functions (e.g. `to_lower`, `range`) are available in sibling modules too.
    pub fn emit_sibling_module(&mut self, prog: &Program, prelude_progs: &[Program]) {
        self.emit_program_core(prog, &[], true, prelude_progs);
    }

    fn emit_program_core(
        &mut self,
        prog: &Program,
        sibling_mods: &[&str],
        force_runtime: bool,
        prelude_progs: &[Program],
    ) {
        // File header
        self.line("// Generated by MVL transpiler — do not edit manually.");
        self.line("#![allow(dead_code, unused_variables, unused_imports, unused_parens)]");
        self.blank();

        // MVL runtime prelude: security labels, effect markers, refinement macro,
        // and stdlib function implementations (read_file, get_arg, etc.).
        // When mvl_runtime is available as a dependency we use it directly;
        // otherwise fall back to the inlined preamble for standalone files.
        // `force_runtime` is set for sibling modules in a project that uses mvl_runtime,
        // so all modules share the same type definitions.
        // Also check prelude programs for builtin declarations: std/strings.mvl and
        // std/lists.mvl have `pub builtin fn` declarations for the kernel primitives,
        // so any program that loads the Phase 4 prelude needs `use mvl_runtime::prelude::*;`
        // to resolve them (implemented in mvl_runtime::stdlib::primitives).
        let prelude_has_extern = prelude_progs.iter().any(|p| {
            p.declarations.iter().any(|d| match d {
                Decl::Extern(_) => true,
                Decl::Fn(fd) => fd.is_builtin,
                _ => false,
            })
        });
        let has_runtime = force_runtime
            || prelude_has_extern
            || prog
                .declarations
                .iter()
                .any(|d| matches!(d, Decl::Extern(_)))
            || has_std_imports(prog);
        if has_runtime {
            self.use_mvl_runtime = true;
            self.line("use mvl_runtime::prelude::*;");
            self.blank();
            // Emit targeted stdlib imports for each `use std.X.*` in the MVL source (#488/#489).
            // The prelude no longer re-exports OS modules; each module is imported explicitly.
            // Also include Rust-backed modules needed by prelude programs (e.g. pbt uses random
            // internally — #555).
            let mut all_modules = collect_stdlib_modules(prog);
            for pp in prelude_progs {
                for m in collect_stdlib_modules(pp) {
                    if !all_modules.contains(&m) {
                        all_modules.push(m);
                    }
                }
            }
            for module in all_modules {
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
            emit_security_preamble(self);
        }
        self.blank();

        // Declare sibling modules so the Rust compiler can resolve cross-module items.
        for mod_name in sibling_mods {
            self.line(&format!("pub mod {mod_name};"));
        }
        if !sibling_mods.is_empty() {
            self.blank();
        }

        // Emit stdlib prelude functions that have real bodies (non-stubs).
        // Stubs (empty body) are skipped; built-in names handled as Rust macros
        // (println, print, eprintln, format) are skipped; test functions are skipped.
        // Functions already declared in the user program are skipped to prevent
        // duplicate Rust definitions when the user shadows a prelude function.
        const MACRO_HANDLED: &[&str] = &["println", "print", "eprintln", "eprint", "format"];
        let user_fn_names: std::collections::HashSet<&str> = prog
            .declarations
            .iter()
            .filter_map(|d| {
                if let Decl::Fn(fd) = d {
                    Some(fd.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        let user_type_names: std::collections::HashSet<&str> = prog
            .declarations
            .iter()
            .filter_map(|d| {
                if let Decl::Type(td) = d {
                    Some(td.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        let prelude_fns: Vec<&FnDecl> = prelude_progs
            .iter()
            .flat_map(|p| p.declarations.iter())
            .filter_map(|d| if let Decl::Fn(fd) = d { Some(fd) } else { None })
            .filter(|fd| !fd.is_builtin) // builtin fns have no body; runtime provides them
            .filter(|fd| !fd.body.stmts.is_empty())
            .filter(|fd| !MACRO_HANDLED.contains(&fd.name.as_str()))
            .filter(|fd| !fd.is_test)
            .filter(|fd| !user_fn_names.contains(fd.name.as_str()))
            .collect();
        // Pure-MVL stdlib modules (no extern "rust") may define types (e.g. json.mvl's Value).
        // Emit those type declarations before the functions that use them.
        // Rust-backed modules (io, env, …) are excluded: their types come from mvl_runtime.
        let prelude_types: Vec<&TypeDecl> = prelude_progs
            .iter()
            .filter(|p| !p.declarations.iter().any(|d| matches!(d, Decl::Extern(_))))
            .flat_map(|p| p.declarations.iter())
            .filter_map(|d| {
                if let Decl::Type(td) = d {
                    Some(td)
                } else {
                    None
                }
            })
            .filter(|td| !user_type_names.contains(td.name.as_str()))
            .collect();

        // Phase B: build the borrow-params map from all known functions so that
        // call sites can emit `&x` instead of `x.clone()` for reference params.
        self.borrow_params_map = build_borrow_params_map(prog, &prelude_fns);

        if !prelude_types.is_empty() || !prelude_fns.is_empty() {
            self.line(
                "// ── stdlib prelude (transpiled from MVL source) ──────────────────────────",
            );
            let saved_coverage = self.coverage.take();
            let saved_mutation = self.mutation.take();
            let saved_mcdc = self.mcdc.take(); // don't instrument stdlib prelude
            for td in prelude_types {
                emit_type_decl(self, td);
                self.blank();
            }
            for fd in prelude_fns {
                emit_fn_decl(self, fd);
                self.blank();
            }
            self.coverage = saved_coverage;
            self.mutation = saved_mutation;
            self.mcdc = saved_mcdc;
        }

        // Emit placeholder structs for external types referenced but not defined
        // in this module (e.g. `DbConn` from library code), along with any
        // method stubs inferred from call sites.
        let stubs = collect_undefined_types(prog);
        if !stubs.is_empty() {
            self.line(
                "// ── External type stubs (Phase 1 placeholders) ──────────────────────────",
            );
            for name in &stubs {
                self.line(&format!(
                    "/// Placeholder for external type `{name}` (not defined in this module)."
                ));
                self.line("#[allow(dead_code)]");
                self.line(&format!("pub struct {name};"));
                self.blank();

                // Emit impl block with any method stubs collected from call sites
                let methods = collect_method_stubs_for_type(prog, name, &stubs);
                if !methods.is_empty() {
                    self.line(&format!("impl {name} {{"));
                    for m in &methods {
                        self.push_indent();
                        self.line(&format!(
                            "pub fn {}(&self, {}) -> {} {{ todo!() }}",
                            m.name, m.args_str, m.return_type
                        ));
                        self.pop_indent();
                    }
                    self.line("}");
                    self.blank();
                }
            }
        }

        // Top-level declarations (non-test)
        for decl in &prog.declarations {
            match decl {
                Decl::Type(td) => emit_type_decl(self, td),
                Decl::Fn(fd) if !fd.is_test => emit_fn_decl(self, fd),
                Decl::Fn(_) => continue, // test fns emitted below
                Decl::Extern(ed) => emit_extern_decl(self, ed),
                Decl::Const(_) => {
                    // Phase 1: const decls emitted as-is in emit_types if needed
                    // skip for now — const support is limited
                }
                Decl::Use(ud) => {
                    // Emit Rust `use` for local module imports (non-std).
                    // std imports are handled in the preamble via targeted
                    // `use mvl_runtime::stdlib::X::*;` lines (#488/#489).
                    // Use `crate::` prefix for Rust 2018 edition path clarity.
                    if ud.path.len() > 1 {
                        let source_mod = ud.path[..ud.path.len() - 1].join("::");
                        if source_mod != "std" {
                            // In test-stub mode (source file included in test crate) skip
                            // cross-module imports — the test crate re-declares types locally
                            // (workaround for #96) and emitting `use crate::mod::Type` for a
                            // locally re-declared type causes name conflicts.
                            if !self.test_extern_stubs {
                                self.line(&format!("use crate::{};", ud.path.join("::")));
                            }
                        }
                    }
                    continue; // use decls don't get a trailing blank line
                }
                Decl::Impl(id) => emit_impl_decl(self, id),
            }
            self.blank();
        }

        // Collect test functions and emit inside #[cfg(test)] mod
        let test_fns: Vec<&FnDecl> = prog
            .declarations
            .iter()
            .filter_map(|d| if let Decl::Fn(fd) = d { Some(fd) } else { None })
            .filter(|fd| fd.is_test)
            .collect();
        if !test_fns.is_empty() {
            self.line("#[cfg(test)]");
            self.line("mod tests {");
            self.push_indent();
            self.line("use super::*;");
            self.blank();
            for fd in test_fns {
                emit_fn_decl(self, fd);
                self.blank();
            }
            self.pop_indent();
            self.line("}");
        }
    }
}

// ── Extern block emission ─────────────────────────────────────────────────

/// Emit an `extern "abi" { … }` block as Rust extern declarations.
///
/// For `extern "rust"`: the functions are declared as regular `extern "Rust"`
/// Rust items — the linker resolves them from the crate in `Cargo.toml`.
/// For `extern "c"`: standard C ABI extern block.
fn emit_extern_decl(cg: &mut RustEmitter, ed: &ExternDecl) {
    // Register extern function names so calls can be wrapped in unsafe
    for f in &ed.fns {
        cg.extern_fns.insert(f.name.clone());
    }

    if cg.test_extern_stubs {
        // In test mode, emit stub functions instead of real extern declarations
        // so the test crate links without the external dependency.
        cg.line(&format!(
            "// ── extern \"{}\" stubs (test mode) ──────────────────────────────────────────",
            ed.abi
        ));
        for f in &ed.fns {
            let params_str: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, emit_type_expr(&p.ty)))
                .collect();
            let ret_str = emit_type_expr(&f.return_type);
            cg.line(&format!(
                "#[allow(dead_code)] pub fn {}({}) -> {} {{ todo!(\"extern stub\") }}",
                f.name,
                params_str.join(", "),
                ret_str
            ));
        }
        return;
    }

    cg.line(&format!(
        "// ── extern \"{}\" trust boundary ({} fn{}) ──────────────────────────────────",
        ed.abi,
        ed.fns.len(),
        if ed.fns.len() == 1 { "" } else { "s" }
    ));
    // Unknown ABIs are rejected by the checker; skip codegen for them.
    let rust_abi = match ed.abi.as_str() {
        "rust" => "Rust",
        "c" => "C",
        other => {
            cg.line(&format!(
                "// extern \"{other}\" block skipped — unsupported ABI (checker error)"
            ));
            return;
        }
    };
    cg.line(&format!("extern \"{rust_abi}\" {{"));
    cg.push_indent();
    for f in &ed.fns {
        // Emit effects as a doc comment (not enforced by Rust's type system yet)
        if !f.effects.is_empty() {
            cg.line(&format!(
                "// ! {}",
                f.effects
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let params_str: Vec<String> = f
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, emit_type_expr(&p.ty)))
            .collect();
        let ret_str = emit_type_expr(&f.return_type);
        // Note: `pub` is not valid inside extern blocks in Rust.
        cg.line(&format!(
            "fn {}({}) -> {};",
            f.name,
            params_str.join(", "),
            ret_str
        ));
    }
    cg.pop_indent();
    cg.line("}");
}

// ── Undefined type collection ─────────────────────────────────────────────

/// Collect the names of all base types referenced in the program that are
/// not defined by a `TypeDecl` in this program and are not MVL built-ins.
/// Returns a sorted, deduplicated list suitable for emitting stub structs.
fn collect_undefined_types(prog: &Program) -> Vec<String> {
    // Collect defined type names
    let mut defined: std::collections::HashSet<String> = std::collections::HashSet::new();
    for decl in &prog.declarations {
        if let Decl::Type(td) = decl {
            defined.insert(td.name.clone());
        }
    }

    // Types from Rust-backed stdlib modules (e.g. Path/File from std.io) are
    // provided by the emitted `use mvl_runtime::stdlib::X::*;` wildcard import.
    // Add their names to `defined` so they are never emitted as placeholder stubs.
    for module in crate::mvl::backends::rust::collect_stdlib_modules(prog) {
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

    // MVL built-in primitive types (already mapped in emit_type_expr)
    let builtins: std::collections::HashSet<&str> = [
        "Int", "Float", "Bool", "String", "Char", "Byte", "Unit", "Never", "List",
        // Security labels (handled by preamble)
        "Public", "Tainted", "Secret", "Clean", // Common Rust types that may appear
        "Option", "Result", "Vec",
        // Rust built-ins used directly in MVL (Box<T> for recursive ADTs)
        "Box",
    ]
    .iter()
    .copied()
    .collect();

    // Collect type names imported via `use module::Type` declarations.
    // These are provided by sibling modules — no stub needed.
    let mut imported: std::collections::HashSet<String> = std::collections::HashSet::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if let Some(item) = ud.path.last() {
                imported.insert(item.clone());
            }
        }
    }

    // Walk all type expressions in the program to collect referenced base types
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Type(td) => collect_types_in_type_decl(td, &mut referenced),
            Decl::Fn(fd) => collect_types_in_fn_decl(fd, &mut referenced),
            _ => {}
        }
    }

    // Undefined = referenced but not defined, not a built-in, and not imported
    let mut stubs: Vec<String> = referenced
        .into_iter()
        .filter(|name| {
            !defined.contains(name) && !builtins.contains(name.as_str()) && !imported.contains(name)
        })
        .collect();
    stubs.sort();
    stubs
}

fn collect_types_in_type_expr(ty: &TypeExpr, out: &mut std::collections::HashSet<String>) {
    match ty {
        TypeExpr::Base { name, args, .. } => {
            out.insert(name.clone());
            for a in args {
                collect_types_in_type_expr(a, out);
            }
        }
        TypeExpr::Option { inner, .. } | TypeExpr::Ref { inner, .. } => {
            collect_types_in_type_expr(inner, out);
        }
        TypeExpr::Result { ok, err, .. } => {
            collect_types_in_type_expr(ok, out);
            collect_types_in_type_expr(err, out);
        }
        TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
            collect_types_in_type_expr(inner, out);
        }
        TypeExpr::Fn { params, ret, .. } => {
            for p in params {
                collect_types_in_type_expr(p, out);
            }
            collect_types_in_type_expr(ret, out);
        }
        TypeExpr::Tuple { elems, .. } => {
            for e in elems {
                collect_types_in_type_expr(e, out);
            }
        }
        TypeExpr::IntConst { .. } => {}
    }
}

fn collect_types_in_field(f: &FieldDecl, out: &mut std::collections::HashSet<String>) {
    collect_types_in_type_expr(&f.ty, out);
}

fn collect_types_in_variant(v: &Variant, out: &mut std::collections::HashSet<String>) {
    match &v.fields {
        VariantFields::Unit => {}
        VariantFields::Tuple(tys) => {
            for ty in tys {
                collect_types_in_type_expr(ty, out);
            }
        }
        VariantFields::Struct(fields) => {
            for f in fields {
                collect_types_in_field(f, out);
            }
        }
    }
}

fn collect_types_in_type_decl(td: &TypeDecl, out: &mut std::collections::HashSet<String>) {
    use crate::mvl::parser::ast::TypeBody;
    match &td.body {
        TypeBody::Struct(fields) => {
            for f in fields {
                collect_types_in_field(f, out);
            }
        }
        TypeBody::Enum(variants) => {
            for v in variants {
                collect_types_in_variant(v, out);
            }
        }
        TypeBody::Alias(ty) => collect_types_in_type_expr(ty, out),
    }
}

fn collect_types_in_param(p: &Param, out: &mut std::collections::HashSet<String>) {
    collect_types_in_type_expr(&p.ty, out);
}

fn collect_types_in_fn_decl(fd: &FnDecl, out: &mut std::collections::HashSet<String>) {
    for p in &fd.params {
        collect_types_in_param(p, out);
    }
    collect_types_in_type_expr(&fd.return_type, out);
}

// ── Method stub collection ─────────────────────────────────────────────────

struct MethodStub {
    name: String,
    args_str: String,
    return_type: String,
}

/// For a given external stub type name, scan all function bodies in the program
/// for method calls on parameters of that type. Returns inferred method stubs.
fn collect_method_stubs_for_type(
    prog: &Program,
    stub_type: &str,
    all_stubs: &[String],
) -> Vec<MethodStub> {
    use crate::mvl::backends::rust::emit_types::emit_type_expr;
    use crate::mvl::parser::ast::{Expr, Stmt, TypeExpr};

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<MethodStub> = Vec::new();

    for decl in &prog.declarations {
        let Decl::Fn(fd) = decl else { continue };

        // Build a map from param name → base type name (for stub-typed params)
        let mut param_to_type: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for p in &fd.params {
            let base = base_type_name(&p.ty);
            if all_stubs.iter().any(|s| s == base) && base == stub_type {
                param_to_type.insert(&p.name, base);
            }
        }
        if param_to_type.is_empty() {
            continue;
        }

        // Build param name → type string for arg type resolution
        let param_types: std::collections::HashMap<&str, String> = fd
            .params
            .iter()
            .map(|p| (p.name.as_str(), emit_type_expr(&p.ty)))
            .collect();

        // Infer the function's error type (for Result-returning fns with `?`)
        let fn_err_type = match fd.return_type.as_ref() {
            TypeExpr::Result { err, .. } => emit_type_expr(err),
            _ => String::from("()"),
        };

        for stmt in &fd.body.stmts {
            if let Stmt::Let {
                ty: let_ty, init, ..
            } = stmt
            {
                let (method_call, has_prop) = match init {
                    Expr::Propagate { expr, .. } => (expr.as_ref(), true),
                    other => (other, false),
                };
                if let Expr::MethodCall {
                    receiver,
                    method,
                    args,
                    ..
                } = method_call
                {
                    let recv_name = match receiver.as_ref() {
                        Expr::Ident(n, _) => n.as_str(),
                        _ => continue,
                    };
                    if !param_to_type.contains_key(recv_name) {
                        continue;
                    }
                    if seen.contains(method) {
                        continue;
                    }
                    seen.insert(method.clone());

                    // Build args string: resolve each arg's type from param list.
                    let args_str: String = args
                        .iter()
                        .enumerate()
                        .map(|(i, arg)| {
                            let ty = match arg {
                                Expr::Ident(n, _) => param_types
                                    .get(n.as_str())
                                    .cloned()
                                    .unwrap_or_else(|| format!("_T{i}")),
                                _ => format!("_T{i}"),
                            };
                            format!("_: {ty}")
                        })
                        .collect::<Vec<_>>()
                        .join(", ");

                    // Return type: wrap in Result if propagated
                    let let_ty_str = emit_type_expr(let_ty);
                    let return_type = if has_prop {
                        format!("Result<{let_ty_str}, {fn_err_type}>")
                    } else {
                        let_ty_str
                    };

                    result.push(MethodStub {
                        name: method.clone(),
                        args_str,
                        return_type,
                    });
                }
            }
        }
    }
    result
}

/// Extract the outermost base type name from a TypeExpr, stripping ref/label wrappers.
fn base_type_name(ty: &TypeExpr) -> &str {
    match ty {
        TypeExpr::Base { name, .. } => name,
        TypeExpr::Ref { inner, .. } => base_type_name(inner),
        TypeExpr::Labeled { inner, .. } => base_type_name(inner),
        TypeExpr::Refined { inner, .. } => base_type_name(inner),
        _ => "",
    }
}
