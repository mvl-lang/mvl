// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL transpiler — emits Rust source from a parsed [`Program`].
//!
//! Phase 1: prototype transpilation to Rust.  Security labels become newtypes,
//! refinement predicates become `assert!` guards, effects and totality
//! are preserved as doc comments.
//!
//! # Pipeline position
//!
//! ```text
//! Parser → [Type Checker] → Transpiler → Rust source → rustc / cargo
//! ```
//!
//! # Entry point
//!
//! ```
//! use mvl::mvl::backends::rust::transpile;
//! use mvl::mvl::parser::ast::Program;
//!
//! // let prog: Program = …;
//! // let out = transpile(&prog, "my_crate");
//! // println!("{}", out.lib_rs);
//! ```

pub mod boundary_gen;
pub mod capability_params;
pub mod cargo;
pub mod config;
pub mod coverage_emit;
pub mod emit_actors;
pub mod emit_exprs;
pub mod emit_functions;
pub mod emit_impls;
pub mod emit_stmts;
pub mod emit_types;
pub mod emitter;
pub mod last_use;
pub mod mcdc_emit;
pub mod mutation_emit;
pub mod visitor;

use crate::mvl::parser::ast::{Decl, Program};
pub use crate::mvl::passes::coverage::{format_report, BranchInfo, CoverageMap};
use crate::mvl::passes::mcdc::transform as mcdc_instr;
pub use crate::mvl::passes::mcdc::transform::{detect_coupled_pairs, FnFieldReads, MCDCDecision};
pub use crate::mvl::passes::mutation::{format_mutation_report, MutantInfo, MutationMap};
pub use boundary_gen::format_boundary_report;
use cargo::CargoOptions;
pub use config::TranspileConfig;
pub use coverage_emit::{emit_cov_preamble, emit_cov_report_test};
use emitter::RustEmitter;
pub use mcdc_emit::{emit_mcdc_preamble, emit_mcdc_report_test};

/// Combined result of a transpilation pass.
///
/// Instrumentation metadata fields are non-empty only when the corresponding
/// mode was enabled via [`TranspileConfig`].
pub struct TranspileResult {
    /// The main transpile output (Rust source + Cargo.toml).
    pub output: TranspileOutput,
    /// Branch coverage metadata, non-empty when `coverage_start_id` was set.
    pub branches: Vec<BranchInfo>,
    /// Mutation testing metadata, non-empty when `mutation` was enabled.
    pub mutants: Vec<MutantInfo>,
    /// MC/DC decision metadata, non-empty when `mcdc_start_id` was set.
    pub decisions: Vec<MCDCDecision>,
}

/// Output of a successful transpilation.
pub struct TranspileOutput {
    /// Contents of `src/lib.rs` (library) or `src/main.rs` (binary with `fn main`).
    pub lib_rs: String,
    /// Contents of `Cargo.toml`.
    pub cargo_toml: String,
    /// True when the program declares `fn main` — the crate is a binary.
    pub has_main: bool,
    /// Number of extern trust boundaries (for assurance reporting).
    pub extern_count: usize,
    /// True when the program declares at least one `extern "rust"` block.
    pub has_extern_rust: bool,
}

/// Returns true if the program declares a top-level `fn main`.
pub fn has_main_fn(prog: &Program) -> bool {
    prog.declarations.iter().any(|d| {
        if let Decl::Fn(fd) = d {
            fd.name == "main"
        } else {
            false
        }
    })
}

/// Count extern declarations in a program.
pub fn count_extern_decls(prog: &Program) -> usize {
    prog.declarations
        .iter()
        .filter(|d| matches!(d, Decl::Extern(_)))
        .count()
}

/// True when any prelude program has `extern "rust"` or `pub builtin fn` declarations.
///
/// The string/list kernel is declared as `pub builtin fn` in std/strings.mvl and
/// std/lists.mvl. These are implemented in `mvl_runtime::stdlib::primitives` and
/// re-exported via `mvl_runtime::prelude::*`, so any program loading the implicit
/// prelude needs `mvl_runtime` as a Cargo dependency.
pub fn prelude_requires_runtime(prelude_progs: &[Program]) -> bool {
    prelude_progs.iter().any(|p| {
        p.declarations.iter().any(|d| match d {
            Decl::Extern(_) => true,
            Decl::Fn(fd) => fd.is_builtin,
            _ => false,
        })
    })
}

/// Returns true if the program declares at least one `extern "rust"` block.
pub fn has_extern_rust_decls(prog: &Program) -> bool {
    prog.declarations
        .iter()
        .any(|d| matches!(d, Decl::Extern(ed) if ed.abi == "rust"))
}

/// Returns true if the program contains extern blocks or type declarations.
/// Used to exclude pure-function demo/helper files from the stdlib-prelude
/// when running `mvl test`, so that demo functions cannot shadow runtime
/// primitives.  Note: only checks Extern/Type decls — not builtin fn decls.
pub fn has_extern_or_type_decls(prog: &Program) -> bool {
    prog.declarations
        .iter()
        .any(|d| matches!(d, Decl::Extern(_) | Decl::Type(_)))
}

/// Returns true if the program imports any `use std.*` stdlib modules.
///
/// When a program uses stdlib functions (e.g. `use std.io.{read_file}`), the
/// generated code needs explicit `use mvl_runtime::stdlib::X::*` imports (#488/#489),
/// so `mvl_runtime` must be linked even when no `extern "rust"` block is present.
pub fn has_std_imports(prog: &Program) -> bool {
    prog.declarations.iter().any(|d| {
        if let Decl::Use(ud) = d {
            ud.path.first().map(|s| s == "std").unwrap_or(false)
        } else {
            false
        }
    })
}

/// Modules that have a Rust implementation in `mvl_runtime::stdlib`.
/// Pure-MVL modules (json, collections, strings, lists, math, …) are excluded:
/// their symbols arrive via the prelude and need no `use mvl_runtime::stdlib::X::*` import.
pub const RUST_BACKED_STDLIB: &[&str] = &[
    "crypto", "env", "io", "log", "net", "process", "random", "regex", "time",
];

/// Returns the deduplicated list of `std.*` sub-module names used in this program
/// that have a Rust implementation in `mvl_runtime::stdlib`.
///
/// For example, `use std.io.*;` and `use std.env.*;` produce `["io", "env"]`.
/// Used by the emitter to emit `use mvl_runtime::stdlib::X::*;` for each module.
/// Pure-MVL stdlib modules (json, collections, …) are excluded — their symbols
/// reach the generated code via the prelude, not via `mvl_runtime::stdlib`.
pub fn collect_stdlib_modules(prog: &Program) -> Vec<String> {
    let mut modules: Vec<String> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
                let module = ud.path[1].as_str();
                if RUST_BACKED_STDLIB.contains(&module) && !modules.contains(&module.to_string()) {
                    modules.push(module.to_string());
                }
            }
        }
    }
    modules
}

/// Output of a successful multi-file project transpilation.
pub struct ProjectOutput {
    /// Contents of `src/main.rs` or `src/lib.rs` for the entry-point module.
    pub main_rs: String,
    /// Transpiled Rust source for each sibling module: `(module_name, source)`.
    /// Each entry should be written to `src/{module_name}.rs`.
    pub module_files: Vec<(String, String)>,
    /// Contents of `Cargo.toml`.
    pub cargo_toml: String,
    /// True when the entry program declares `fn main` — the crate is a binary.
    pub has_main: bool,
    /// Number of extern trust boundaries (for assurance reporting).
    pub extern_count: usize,
    /// True when the entry program declares at least one `extern "rust"` block.
    pub has_extern_rust: bool,
    /// True when the generated code uses `use mvl_runtime::prelude::*` and the
    /// runtime crate must be present as a path dependency.
    pub use_mvl_runtime: bool,
}

/// Transpile a multi-file project to Rust source.
///
/// `entry_name` is the crate/module name for the entry program.
/// `siblings` is a list of `(module_name, Program)` pairs for all other modules
/// reachable from the entry point (e.g. sibling `.mvl` files).
///
/// The entry module's output includes `pub mod name;` declarations for each sibling,
/// so the Rust compiler can resolve cross-module items.
pub fn transpile_project(
    entry_name: &str,
    entry_prog: &Program,
    siblings: &[(String, Program)],
    prelude_progs: &[Program],
    expr_types: std::collections::HashMap<
        crate::mvl::parser::lexer::Span,
        crate::mvl::checker::types::Ty,
    >,
    assert_mode: crate::mvl::backends::AssertMode,
) -> ProjectOutput {
    let has_main = has_main_fn(entry_prog);
    let extern_count = count_extern_decls(entry_prog);
    let has_extern_rust =
        has_extern_rust_decls(entry_prog) || prelude_progs.iter().any(has_extern_rust_decls);
    // Link mvl_runtime when extern "rust" is used OR when stdlib is imported.
    // Stdlib functions (e.g. read_file, get_arg) are re-exported from
    // mvl_runtime::prelude::* and require the runtime crate to be present.
    let use_runtime = extern_count > 0
        || has_std_imports(entry_prog)
        || siblings.iter().any(|(_, p)| has_std_imports(p))
        || prelude_requires_runtime(prelude_progs);

    let sibling_names: Vec<&str> = siblings.iter().map(|(n, _)| n.as_str()).collect();
    let mut cg = RustEmitter::new();
    cg.expr_types = expr_types;
    cg.assert_mode = assert_mode;
    cg.emit_program_with_mods(entry_prog, &sibling_names, prelude_progs);
    let main_rs = cg.finish();

    // Sibling modules share the runtime prelude with the entry point so type
    // definitions don't conflict (e.g. `Tainted` from mvl_runtime vs inline).
    let entry_uses_runtime = use_runtime;
    let module_files: Vec<(String, String)> = siblings
        .iter()
        .map(|(name, prog)| {
            let sibling_check = crate::mvl::checker::check_with_prelude(prelude_progs, prog);
            let mut cg = RustEmitter::new();
            // Include prelude expr_types so prelude functions emitted inside the
            // sibling module (via emit_sibling_module) have correct type information.
            // Without this, generic operations like map indexing fall back to the
            // integer-array pattern instead of the correct string-key HashMap access.
            let mut sibling_expr_types =
                crate::mvl::checker::collect_prelude_expr_types(prelude_progs);
            sibling_expr_types.extend(sibling_check.expr_types);
            cg.expr_types = sibling_expr_types;
            cg.assert_mode = assert_mode;
            if entry_uses_runtime {
                cg.emit_sibling_module(prog, prelude_progs);
            } else {
                cg.emit_program(prog);
            }
            (name.clone(), cg.finish())
        })
        .collect();

    let opts = CargoOptions {
        crate_name: entry_name,
        use_mvl_runtime: use_runtime,
        extern_crates: Vec::new(),
        native_dep_lines: Vec::new(),
    };
    let cargo_toml = if has_main {
        cargo::emit_cargo_toml_binary_opts(&opts)
    } else {
        cargo::emit_cargo_toml_library_opts(&opts)
    };

    ProjectOutput {
        main_rs,
        module_files,
        cargo_toml,
        has_main,
        extern_count,
        has_extern_rust,
        use_mvl_runtime: use_runtime,
    }
}

/// Transpile a parsed [`Program`] to Rust source using the given configuration.
///
/// This single function replaces the previous `transpile_*` variant family.
/// Use [`TranspileConfig`] to configure prelude, instrumentation, and emit options.
///
/// Instrumentation metadata in the returned [`TranspileResult`] is non-empty only
/// when the corresponding mode was enabled in the config.
pub fn transpile(prog: &Program, config: TranspileConfig) -> TranspileResult {
    let has_main = has_main_fn(prog);
    let extern_count = count_extern_decls(prog);
    let has_extern_rust = has_extern_rust_decls(prog);
    let has_prelude = !config.prelude_progs.is_empty();
    let use_runtime = extern_count > 0
        || has_std_imports(prog)
        || (has_prelude && prelude_requires_runtime(&config.prelude_progs));

    let check_result = if has_prelude {
        crate::mvl::checker::check_with_prelude(&config.prelude_progs, prog)
    } else {
        crate::mvl::checker::check(prog)
    };

    let expr_types = if has_prelude {
        let mut types = crate::mvl::checker::collect_prelude_expr_types(&config.prelude_progs);
        types.extend(check_result.expr_types);
        types
    } else {
        check_result.expr_types
    };

    let mut cg = RustEmitter::new();
    cg.expr_types = expr_types;
    cg.assert_mode = config.assert_mode;
    cg.current_file = config.file_stem.clone();
    // Set test_extern_stubs: explicitly from config OR when prelude contains extern blocks.
    // Extern blocks in the prelude (e.g. pkg FFI bindings) cannot be linked in test
    // crates, so they are replaced with `todo!()` stubs.
    cg.test_extern_stubs = config.test_extern_stubs
        || (has_prelude
            && config
                .prelude_progs
                .iter()
                .any(|p| p.declarations.iter().any(|d| matches!(d, Decl::Extern(_)))));
    cg.current_file_is_test = config.is_test_file;

    if let Some(start_id) = config.coverage_start_id {
        cg.coverage = Some(CoverageMap::new(start_id));
    }
    if let Some(start_id) = config.mcdc_start_id {
        cg.mcdc = Some(mcdc_instr::MCDCMap::new(start_id));
        cg.mcdc_fn_field_reads = mcdc_instr::build_fn_field_reads(prog);
    }
    if config.mutation {
        cg.mutation = Some(MutationMap::new());
    }

    if has_prelude {
        cg.emit_program_with_mods(prog, &[], &config.prelude_progs);
    } else {
        cg.emit_program(prog);
    }

    let branches = cg.coverage.take().map(|c| c.branches).unwrap_or_default();
    let mutants = cg.mutation.take().map(|m| m.mutants).unwrap_or_default();
    let decisions = cg.mcdc.take().map(|m| m.decisions).unwrap_or_default();
    let lib_rs = cg.finish();

    let opts = CargoOptions {
        crate_name: &config.crate_name,
        use_mvl_runtime: use_runtime,
        extern_crates: Vec::new(),
        native_dep_lines: Vec::new(),
    };
    let cargo_toml = if has_main {
        cargo::emit_cargo_toml_binary_opts(&opts)
    } else {
        cargo::emit_cargo_toml_library_opts(&opts)
    };

    TranspileResult {
        output: TranspileOutput {
            lib_rs,
            cargo_toml,
            has_main,
            extern_count,
            has_extern_rust,
        },
        branches,
        mutants,
        decisions,
    }
}

// ── has_extern_rust unit tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // ── collect_stdlib_modules tests (#488 #489) ───────────────────────────

    #[test]
    fn collect_stdlib_modules_single_import() {
        let prog = parse("use std.io.{read_file}");
        let modules = collect_stdlib_modules(&prog);
        assert_eq!(modules, vec!["io".to_string()]);
    }

    #[test]
    fn collect_stdlib_modules_deduplicates() {
        let prog = parse("use std.io.{read_file}\nuse std.io.{write_file}");
        let modules = collect_stdlib_modules(&prog);
        assert_eq!(
            modules,
            vec!["io".to_string()],
            "duplicates should be removed"
        );
    }

    #[test]
    fn collect_stdlib_modules_multiple_modules() {
        let prog = parse("use std.io.{read_file}\nuse std.env.{getuid}");
        let mut modules = collect_stdlib_modules(&prog);
        modules.sort();
        assert_eq!(modules, vec!["env".to_string(), "io".to_string()]);
    }

    #[test]
    fn collect_stdlib_modules_non_std_ignored() {
        let prog = parse("use mylib.utils.{helper}");
        let modules = collect_stdlib_modules(&prog);
        assert!(
            modules.is_empty(),
            "non-std imports must not appear: {modules:?}"
        );
    }

    #[test]
    fn collect_stdlib_modules_empty_program() {
        let prog = parse("fn f() -> Int { 1 }");
        let modules = collect_stdlib_modules(&prog);
        assert!(modules.is_empty());
    }

    #[test]
    fn transpile_emits_stdlib_use_for_std_import() {
        let prog = parse("use std.env.{getuid}\nfn main() -> Unit ! Env { }");
        let out = transpile(&prog, TranspileConfig::new("crate")).output;
        assert!(
            out.lib_rs.contains("use mvl_runtime::stdlib::env::*"),
            "emitted Rust must contain targeted stdlib import, got:\n{}",
            out.lib_rs
        );
    }

    // ── MC/DC codegen structural tests ────────────────────────────────────

    /// Compound `if (A && B)` emits clause arrays, outcome var, and record call.
    #[test]
    fn mcdc_if_emits_clause_locals_and_record() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert_eq!(decisions.len(), 1, "one compound decision");
        assert_eq!(decisions[0].kind, mcdc_instr::DecisionKind::If);
        let rs = &out.lib_rs;
        assert!(
            rs.contains("let mut __d0_c = [false; 2]"),
            "missing clause array: {rs}"
        );
        assert!(
            rs.contains("let mut __d0_e = [false; 2]"),
            "missing eval array: {rs}"
        );
        assert!(
            rs.contains("let __d0_outcome: bool ="),
            "missing outcome var: {rs}"
        );
        assert!(
            rs.contains("__mvl_mcdc::record(0usize,"),
            "missing record call: {rs}"
        );
        assert!(
            rs.contains("if __d0_outcome {"),
            "missing instrumented if: {rs}"
        );
    }

    /// The short-circuit tree sets clause array entries and uses sc semantics.
    #[test]
    fn mcdc_if_recomposed_uses_clause_vars() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let out = result.output;
        let rs = &out.lib_rs;
        // Short-circuit: left evaluated first, right only if left is true
        assert!(
            rs.contains("__d0_e[0] = true"),
            "missing eval-flag for clause 0: {rs}"
        );
        assert!(
            rs.contains("__d0_e[1] = true"),
            "missing eval-flag for clause 1: {rs}"
        );
    }

    /// Three-clause `A || B || C` emits arrays of size 3.
    #[test]
    fn mcdc_if_three_clauses_emits_three_locals() {
        let prog =
            parse("fn f(a: Bool, b: Bool, c: Bool) -> Int { if a || b || c { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert_eq!(decisions[0].clause_count, 3);
        let rs = &out.lib_rs;
        assert!(rs.contains("let mut __d0_c = [false; 3]"), "{rs}");
        assert!(rs.contains("let mut __d0_e = [false; 3]"), "{rs}");
    }

    /// `emit_mcdc_record` encodes clause vals, eval flags, and outcome as u32.
    #[test]
    fn mcdc_record_encoding_present() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let out = result.output;
        let rs = &out.lib_rs;
        // Clause vals: bits 0 and 1; eval flags: bits 2 and 3; outcome: bit 4
        assert!(
            rs.contains("(__d0_c[0] as u32) << 0u32"),
            "missing bit-0 val encoding: {rs}"
        );
        assert!(
            rs.contains("(__d0_c[1] as u32) << 1u32"),
            "missing bit-1 val encoding: {rs}"
        );
        assert!(
            rs.contains("(__d0_e[0] as u32) << 2u32"),
            "missing eval-0 encoding: {rs}"
        );
        assert!(
            rs.contains("(__d0_e[1] as u32) << 3u32"),
            "missing eval-1 encoding: {rs}"
        );
        assert!(
            rs.contains("(__d0_outcome as u32) << 4u32"),
            "missing outcome encoding: {rs}"
        );
        assert!(
            rs.contains("#[cfg(test)] crate::__mvl_mcdc::record("),
            "missing cfg(test) guard: {rs}"
        );
    }

    /// Compound `while` is restructured as `loop { … if !outcome { break; } … }`.
    #[test]
    fn mcdc_while_restructured_as_loop() {
        let prog = parse(
            "partial fn f(a: Bool, b: Bool) -> Int { let x: ref Int = 0; while a && b { x = x + 1; } x }",
        );
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].kind, mcdc_instr::DecisionKind::While);
        let rs = &out.lib_rs;
        assert!(rs.contains("loop {"), "missing loop restructuring: {rs}");
        assert!(
            rs.contains("if !__d0_outcome { break; }"),
            "missing break guard: {rs}"
        );
        assert!(rs.contains("let mut __d0_c = [false; 2]"), "{rs}");
        assert!(rs.contains("let mut __d0_e = [false; 2]"), "{rs}");
    }

    /// Simple (single-clause) conditions are NOT instrumented for MC/DC.
    #[test]
    fn mcdc_simple_condition_not_instrumented() {
        let prog = parse("fn f(x: Int) -> Int { if x > 0 { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert!(
            decisions.is_empty(),
            "simple condition must not be instrumented"
        );
        assert!(!out.lib_rs.contains("__d0_c"), "no clause arrays expected");
    }

    /// Test functions are excluded from MC/DC instrumentation.
    #[test]
    fn mcdc_test_fn_excluded() {
        let prog =
            parse("test fn t(a: Bool, b: Bool) -> Bool { if a && b { true } else { false } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert!(
            decisions.is_empty(),
            "test fn must not generate MC/DC decisions"
        );
    }

    /// Compound boolean return expressions in `Bool`-valued functions are
    /// instrumented as `DecisionKind::Return` decisions.
    #[test]
    fn mcdc_bool_return_expr_instrumented() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Bool { a && b }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert_eq!(decisions.len(), 1, "compound bool return is one decision");
        assert_eq!(decisions[0].kind, mcdc_instr::DecisionKind::Return);
        assert_eq!(decisions[0].clause_count, 2);
        assert!(
            out.lib_rs.contains("let mut __d0_c = [false; 2]"),
            "clause array emitted"
        );
        assert!(
            out.lib_rs.contains("__d0_outcome"),
            "outcome variable emitted"
        );
        assert!(
            out.lib_rs.contains("__mvl_mcdc::record(0usize,"),
            "record call emitted"
        );
    }

    /// Non-Bool return expressions are NOT instrumented even if compound.
    #[test]
    fn mcdc_non_bool_return_not_instrumented() {
        let prog = parse("fn f(a: Int, b: Int) -> Int { a + b }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert!(
            decisions.is_empty(),
            "non-Bool return must not be instrumented"
        );
    }

    /// Clauses that share a variable are detected as a coupled pair.
    #[test]
    fn mcdc_coupled_pairs_detected() {
        // f(a) and g(a, b) both take `a` — they are coupled.
        // h(b) and g(a, b) both take `b` — also coupled.
        let prog =
            parse("fn d(a: Bool, b: Bool, c: Bool) -> Bool { f(a) && g(a, b) && h(b) && k(c) }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert_eq!(decisions.len(), 1);
        // Expect at least: (0,1) via "a" and (1,2) via "b"
        let pairs = &decisions[0].coupled_pairs;
        let has_01 = pairs
            .iter()
            .any(|(i, j, v)| *i == 0 && *j == 1 && v.contains(&"a".to_string()));
        let has_12 = pairs
            .iter()
            .any(|(i, j, v)| *i == 1 && *j == 2 && v.contains(&"b".to_string()));
        assert!(has_01, "clauses 0 and 1 share variable 'a'");
        assert!(has_12, "clauses 1 and 2 share variable 'b'");
        // Clause 3 (k(c)) is independent — not coupled with others
        let has_3 = pairs.iter().any(|(i, j, _)| *i == 3 || *j == 3);
        assert!(!has_3, "clause 3 (k(c)) must not be coupled");
    }

    /// Clauses with disjoint variable sets are not coupled.
    #[test]
    fn mcdc_independent_clauses_not_coupled() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Bool { a && b }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert_eq!(decisions.len(), 1);
        assert!(
            decisions[0].coupled_pairs.is_empty(),
            "a and b are independent — no coupling"
        );
    }

    /// Field-level coupling: same struct param, disjoint fields → NOT coupled.
    #[test]
    fn mcdc_disjoint_field_access_not_coupled() {
        // f(v.breathing) and g(v.oxygen_sat) share param `v` but access different fields.
        let prog = parse("fn d(v: Vitals) -> Bool { f(v.breathing) && g(v.oxygen_sat) }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert_eq!(decisions.len(), 1);
        assert!(
            decisions[0].coupled_pairs.is_empty(),
            "disjoint fields v.breathing vs v.oxygen_sat must not be coupled"
        );
    }

    /// Field-level coupling: same field accessed by two clauses → genuinely coupled.
    #[test]
    fn mcdc_shared_field_access_is_coupled() {
        // Both clauses use v.bp — toggling it affects both simultaneously.
        let prog = parse("fn d(v: V) -> Bool { f(v.bp) && g(v.bp) }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert_eq!(decisions.len(), 1);
        let pairs = &decisions[0].coupled_pairs;
        assert_eq!(pairs.len(), 1, "one coupled pair expected");
        assert_eq!(pairs[0].0, 0);
        assert_eq!(pairs[0].1, 1);
        assert!(
            pairs[0].2.contains(&"v.bp".to_string()),
            "shared path must be v.bp"
        );
    }

    /// Nested field access: p.vitals.pulse vs p.vitals.bp → disjoint → NOT coupled.
    #[test]
    fn mcdc_nested_field_access_not_coupled() {
        let prog = parse("fn d(p: Patient) -> Bool { f(p.vitals.pulse) && g(p.vitals.bp) }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        assert_eq!(decisions.len(), 1);
        assert!(
            decisions[0].coupled_pairs.is_empty(),
            "p.vitals.pulse vs p.vitals.bp are disjoint nested paths — not coupled"
        );
    }

    // ── Interprocedural coupling tests (#562) ─────────────────────────────

    /// Callees defined in the same file reading disjoint fields via bare param → NOT coupled.
    ///
    /// `f(p)` reads `p.x` and `g(p)` reads `p.y`; the bare-arg `p` is resolved
    /// to the actual field paths before the overlap check.
    #[test]
    fn mcdc_interproc_disjoint_fields_not_coupled() {
        let prog = parse(
            "fn f(p: T) -> Bool { p.x == 1 } \
             fn g(p: T) -> Bool { p.y == 2 } \
             fn d(p: T) -> Bool { f(p) || g(p) }",
        );
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        let d = decisions.iter().find(|d| d.fn_name == "d").unwrap();
        assert!(
            d.coupled_pairs.is_empty(),
            "f reads p.x, g reads p.y — disjoint field reads via bare arg must not be coupled"
        );
    }

    /// Callees reading the same field via bare param → still coupled.
    #[test]
    fn mcdc_interproc_shared_field_still_coupled() {
        let prog = parse(
            "fn f(p: T) -> Bool { p.x == 1 } \
             fn g(p: T) -> Bool { p.x == 2 } \
             fn d(p: T) -> Bool { f(p) && g(p) }",
        );
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        let d = decisions.iter().find(|d| d.fn_name == "d").unwrap();
        assert!(
            !d.coupled_pairs.is_empty(),
            "both read p.x — shared field read via bare arg must still be coupled"
        );
        let has_px = d
            .coupled_pairs
            .iter()
            .any(|(_, _, v)| v.contains(&"p.x".to_string()));
        assert!(has_px, "shared path must be reported as p.x");
    }

    /// Callee forwarding the bare param (not field-decomposing it) → conservative fallback.
    ///
    /// When a callee passes `p` whole to another call, the field-read set contains
    /// the bare param name, so the coupling detector falls back to the conservative
    /// (syntactic) behaviour and still flags coupling via the raw variable.
    #[test]
    fn mcdc_interproc_bare_param_forwarding_falls_back_to_conservative() {
        let prog = parse(
            "fn h(p: T) -> Bool { helper(p) } \
             fn k(p: T) -> Bool { helper(p) } \
             fn d(p: T) -> Bool { h(p) && k(p) }",
        );
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        let d = decisions.iter().find(|d| d.fn_name == "d").unwrap();
        assert!(
            !d.coupled_pairs.is_empty(),
            "callee forwards bare p — conservative coupling must be reported"
        );
    }

    /// External callee (not defined in program) → conservative fallback, same as before.
    #[test]
    fn mcdc_interproc_external_callee_conservative() {
        // `ext_a` and `ext_b` are not defined in the program → conservative coupling via "p".
        let prog = parse("fn d(p: T) -> Bool { ext_a(p) && ext_b(p) }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(0),
        );
        let decisions = result.decisions;
        let d = decisions.iter().find(|d| d.fn_name == "d").unwrap();
        assert!(
            !d.coupled_pairs.is_empty(),
            "unknown callees with shared bare arg must fall back to conservative coupling"
        );
    }

    /// `start_id` offsets decision IDs correctly for multi-file projects.
    #[test]
    fn mcdc_start_id_offset_applied() {
        let prog = parse("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let result = transpile(
            &prog,
            TranspileConfig::new("crate")
                .with_file_stem("test")
                .with_mcdc(5),
        );
        let (out, decisions) = (result.output, result.decisions);
        assert_eq!(decisions[0].id, 5, "decision ID should be start_id");
        assert!(
            out.lib_rs.contains("__mvl_mcdc::record(5usize,"),
            "record call must use offset id"
        );
    }

    /// `has_extern_rust` is `true` when program contains `extern "rust"` block.
    #[test]
    fn has_extern_rust_true_for_rust_abi() {
        let prog = parse(r#"extern "rust" { fn foo() -> Int; }"#);
        assert!(has_extern_rust_decls(&prog));
        assert!(
            transpile(&prog, TranspileConfig::new("crate"))
                .output
                .has_extern_rust
        );
    }

    /// `has_extern_rust` is `false` when program has no extern blocks at all.
    #[test]
    fn has_extern_rust_false_on_plain_program() {
        let prog = parse("fn add(a: Int, b: Int) -> Int { a + b }");
        assert!(!has_extern_rust_decls(&prog));
        let out = transpile(&prog, TranspileConfig::new("crate")).output;
        assert!(!out.has_extern_rust);
        // Regression guard: `mod bridge;` must NOT appear in output for non-extern programs.
        assert!(
            !out.lib_rs.contains("mod bridge;"),
            "mod bridge; must not appear for non-extern programs"
        );
    }

    /// `extern "c"` block does NOT set `has_extern_rust` (ABI discrimination).
    #[test]
    fn has_extern_rust_false_for_c_abi() {
        let prog = parse(r#"extern "c" { fn bar() -> Int; }"#);
        assert!(!has_extern_rust_decls(&prog));
        assert!(
            !transpile(&prog, TranspileConfig::new("crate"))
                .output
                .has_extern_rust
        );
    }

    /// `has_extern_rust` is `false` when only `extern "c"` is present; `extern_count` is non-zero.
    #[test]
    fn extern_count_nonzero_but_has_extern_rust_false() {
        let prog = parse(r#"extern "c" { fn baz() -> Int; }"#);
        let out = transpile(&prog, TranspileConfig::new("crate")).output;
        assert_eq!(out.extern_count, 1);
        assert!(!out.has_extern_rust);
    }
}

// ── Backend trait implementation ─────────────────────────────────────────────

/// Unit struct implementing the [`Backend`] trait for the Rust transpiler.
pub struct RustBackend;

impl crate::mvl::backends::Backend for RustBackend {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn file_extension(&self) -> &'static str {
        "rs"
    }

    fn emit_program(&self, prog: &crate::mvl::parser::ast::Program, crate_name: &str) -> String {
        transpile(prog, TranspileConfig::new(crate_name))
            .output
            .lib_rs
    }
}
