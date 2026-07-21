// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Compilation pipeline — orchestrates Loader → Checker → Transpiler.
//!
//! [`Pipeline`] is the single entry point for check, build, test, and
//! analysis commands.  Build it with the fluent modifier methods, then
//! call [`check`](Pipeline::check) or [`build`](Pipeline::build).
//!
//! # Example
//!
//! ```
//! use mvl::mvl::pipeline::Pipeline;
//! use mvl::mvl::parser::ast::Program;
//!
//! // let prog: Program = …;
//! // let prelude: Vec<Program> = loader::load_implicit_prelude();
//! // let result = Pipeline::new().check(&prelude, &[prog]);
//! ```

use crate::mvl::backends::rust::{transpile, ProjectOutput, TranspileConfig, TranspileResult};
use crate::mvl::backends::AssertMode;
use crate::mvl::checker::{self, CheckResult};
use crate::mvl::parser::ast::Decl;
pub use crate::mvl::parser::ast::Program;

/// Compilation pipeline with composable instrumentation.
///
/// Constructed with [`Pipeline::new`] and configured via builder methods.
/// Provides [`check`](Self::check) and [`build`](Self::build) entry points.
pub struct Pipeline {
    coverage: bool,
    mcdc: bool,
    mutation: bool,
    assert_mode: AssertMode,
    optimize_proved: bool,
}

/// Result of a [`Pipeline::check`] call — one entry per checked program.
pub struct CheckResults {
    /// Per-program check results, in the same order as the input programs.
    pub results: Vec<CheckResult>,
}

impl CheckResults {
    /// Returns true when every program passed type-checking without errors.
    pub fn all_ok(&self) -> bool {
        self.results.iter().all(|r| r.is_ok())
    }

    /// Returns the total number of type errors across all programs.
    pub fn error_count(&self) -> usize {
        self.results.iter().map(|r| r.errors.len()).sum()
    }
}

impl Pipeline {
    /// Create a new pipeline with default settings: no instrumentation, assert mode = Always.
    pub fn new() -> Self {
        Self {
            coverage: false,
            mcdc: false,
            mutation: false,
            assert_mode: AssertMode::Always,
            optimize_proved: false,
        }
    }

    /// Enable branch coverage instrumentation.
    pub fn with_coverage(mut self) -> Self {
        self.coverage = true;
        self
    }

    /// Enable MC/DC condition instrumentation.
    pub fn with_mcdc(mut self) -> Self {
        self.mcdc = true;
        self
    }

    /// Enable mutation testing instrumentation.
    pub fn with_mutation(mut self) -> Self {
        self.mutation = true;
        self
    }

    /// Set the assert mode for refinement predicate emission.
    pub fn with_assert_mode(mut self, mode: AssertMode) -> Self {
        self.assert_mode = mode;
        self
    }

    /// Elide runtime bounds checks certified by the prover (#1891).
    pub fn with_optimize_proved(mut self) -> Self {
        self.optimize_proved = true;
        self
    }

    /// Type-check a set of programs with a shared prelude.
    ///
    /// Each program is checked independently against the same prelude.
    /// Returns aggregated [`CheckResults`] with one entry per program.
    pub fn check(&self, prelude: &[Program], programs: &[Program]) -> CheckResults {
        let results = programs
            .iter()
            .map(|prog| checker::check_with_prelude(prelude, prog))
            .collect();
        CheckResults { results }
    }

    /// Transpile a single program using this pipeline's instrumentation settings.
    ///
    /// Returns a [`TranspileResult`] with the Rust source and any instrumentation
    /// metadata (branches, mutants, decisions) depending on which modes are active.
    ///
    /// **Single-file builds only.** Coverage and MC/DC counters always start at ID 0.
    /// For multi-file coverage runs, use [`TranspileConfig::with_coverage(offset)`] directly
    /// and track the offset across files via `result.branches.len()`.
    pub fn build(
        &self,
        prog: &Program,
        crate_name: impl Into<String>,
        prelude: Vec<Program>,
    ) -> TranspileResult {
        let expr_types = assemble_expr_types(prog, &prelude);
        let prelude_tirs = lower_prelude(&prelude);
        let mut config = TranspileConfig::new(crate_name).with_prelude(prelude_tirs);
        if self.coverage {
            config = config.with_coverage(0);
        }
        if self.mcdc {
            config = config.with_mcdc(0);
        }
        if self.mutation {
            config = config.with_mutation();
        }
        config = config.with_assert_mode(self.assert_mode);
        if self.optimize_proved {
            config = config.with_optimize_proved();
        }
        let all_fns =
            crate::mvl::passes::mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
        let mono = crate::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(prog, &mono, &expr_types);
        transpile(&tir, config)
    }

    /// Transpile a multi-file project using this pipeline's settings.
    ///
    /// Delegates to [`transpile_project_with_options`] with the pipeline's assert mode.
    /// Instrumentation (coverage, MC/DC, mutation) is not supported for
    /// multi-file project builds — use [`build`](Self::build) per file instead.
    pub fn build_project(
        &self,
        entry_name: &str,
        entry_prog: &Program,
        siblings: &[(String, Program)],
        prelude: &[Program],
        expr_types: std::collections::HashMap<
            crate::mvl::parser::lexer::Span,
            crate::mvl::checker::types::Ty,
        >,
    ) -> ProjectOutput {
        let sibling_expr_types: Vec<_> = siblings
            .iter()
            .map(|(_, prog)| assemble_expr_types(prog, prelude))
            .collect();
        transpile_project_with_options(
            entry_name,
            entry_prog,
            siblings,
            prelude,
            expr_types,
            &sibling_expr_types,
            self.assert_mode,
            self.optimize_proved,
            false,
            &[],
        )
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Pipeline-specific prelude loading mode.
///
/// Two modes reflect the two shapes of prelude assembly a CLI subcommand
/// needs:
///
/// - [`TypeCheck`](PreludeMode::TypeCheck) — full stdlib module contents
///   (all `.mvl` files referenced by `use std.X`), disk-first with an
///   embedded fallback. Used by subcommands that only type-check the
///   program (`check`, `prove`, `assurance`, `build`'s checker phase).
///   Carries the disk stdlib path.
/// - [`Transpile`](PreludeMode::Transpile) — pure-MVL extras only. Filters
///   out `RUST_BACKED_STDLIB` and `IMPLICIT_PRELUDE_STEMS` modules, strips
///   type declarations from hybrid modules (types come from
///   `mvl_runtime::stdlib::X::*` on the Rust side). Loads embedded content
///   only. Used by subcommands that transpile or lower (`build`, `test`,
///   `tir`, `mutate`, `mcdc`, `fuzz`, `llvm_text`, `wasm_text`).
///
/// New CLI subcommands must not call the underlying loaders in
/// `crate::mvl::loader` directly — see [`load_full_prelude`] and #1803.
pub enum PreludeMode<'a> {
    /// Type-check pipelines: `mvl check`, `mvl prove`, `mvl assurance`, and
    /// `mvl build`'s checker phase.  Disk-first resolution requires the
    /// stdlib directory.
    TypeCheck { stdlib_dir: &'a std::path::Path },
    /// Transpile pipelines: `mvl build`, `mvl test`, `mvl tir`, `mvl mutate`,
    /// `mvl mcdc`, `mvl fuzz`, and the `llvm_text` / `wasm_text` emitters.
    /// Reads embedded stdlib content only — no disk path needed.
    Transpile,
}

/// Canonical prelude assembler for CLI subcommands.
///
/// Every CLI subcommand that assembles a stdlib prelude for the checker or
/// the transpile pipeline routes through this function. It dispatches on
/// [`PreludeMode`] to one of the internal loaders in `crate::mvl::loader`.
///
/// This is the contract established by ADR-0050's 2026-07-16 extension.
/// Historically each subcommand chose between `load_stdlib_prelude` and
/// `load_mvl_native_stdlib_extras` on its own, which produced three
/// silent-failure incidents (`mvl mcdc`, `mvl tir` / `mvl mutate` — #1788,
/// and the drift pattern tracked as #1803).
pub fn load_full_prelude<'a, 'b>(
    progs: impl IntoIterator<Item = &'a Program>,
    mode: PreludeMode<'b>,
) -> Vec<Program> {
    match mode {
        PreludeMode::TypeCheck { stdlib_dir } => {
            crate::mvl::loader::load_stdlib_prelude(progs.into_iter(), stdlib_dir)
        }
        PreludeMode::Transpile => {
            let owned: Vec<Program> = progs.into_iter().cloned().collect();
            crate::mvl::loader::load_mvl_native_stdlib_extras(&owned)
        }
    }
}

/// Assemble a fully-merged expression type map for `prog` against `prelude`.
///
/// Combines prelude expression types (from `collect_prelude_expr_types`) with
/// the program's own types (from `check_with_prelude` / `check`).  This is the
/// single canonical place where the checker is invoked for transpilation; the
/// backend receives the result and does not re-invoke the checker.
pub fn assemble_expr_types(
    prog: &Program,
    prelude: &[Program],
) -> std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::checker::types::Ty> {
    let mut types = checker::collect_prelude_expr_types(prelude);
    let result = if prelude.is_empty() {
        checker::check(prog)
    } else {
        checker::check_with_prelude(prelude, prog)
    };
    types.extend(result.expr_types);
    types
}

/// Lower a slice of AST programs to TIR (checker + mono + lower in one step).
///
/// Callers that have a `Vec<Program>` prelude and need to pass it to
/// [`TranspileConfig::with_prelude`] should call this first.
pub fn lower_prelude(progs: &[Program]) -> Vec<crate::mvl::ir::TirProgram> {
    let expr_types = checker::collect_prelude_expr_types(progs);
    progs
        .iter()
        .map(|p| {
            let all_fns = crate::mvl::passes::mono::collect_fns([p]);
            let m = crate::mvl::passes::mono::monomorphize(p, &all_fns, &expr_types);
            crate::mvl::ir::lower::lower(p, &m, &expr_types)
        })
        .collect()
}

// ── AST program inspector helpers (moved from backends/rust.rs) ───────────────

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
pub fn has_extern_or_type_decls(prog: &Program) -> bool {
    prog.declarations
        .iter()
        .any(|d| matches!(d, Decl::Extern(_) | Decl::Type(_)))
}

/// Returns true if the program imports any `use std.*` stdlib modules.
pub fn has_std_imports(prog: &Program) -> bool {
    prog.declarations.iter().any(|d| {
        if let Decl::Use(ud) = d {
            ud.path.first().map(|s| s == "std").unwrap_or(false)
        } else {
            false
        }
    })
}

/// Returns the deduplicated list of `std.*` sub-module names used in this program
/// that have a Rust implementation in `mvl_runtime::stdlib`.
pub fn collect_stdlib_modules(prog: &Program) -> Vec<String> {
    use crate::mvl::backends::rust::RUST_RUNTIME_IMPORTS;
    let mut modules: Vec<String> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
                let module = ud.path[1].as_str();
                if RUST_RUNTIME_IMPORTS.contains(&module) && !modules.contains(&module.to_string())
                {
                    modules.push(module.to_string());
                }
            }
        }
    }
    modules
}

// ── Multi-file project transpilation (moved from backends/rust.rs) ────────────

/// Transpile a multi-file project to Rust source.
pub fn transpile_project(
    entry_name: &str,
    entry_prog: &Program,
    siblings: &[(String, Program)],
    prelude_progs: &[Program],
    expr_types: std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::ir::Ty>,
    sibling_expr_types: Vec<
        std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::ir::Ty>,
    >,
    assert_mode: AssertMode,
) -> ProjectOutput {
    transpile_project_with_options(
        entry_name,
        entry_prog,
        siblings,
        prelude_progs,
        expr_types,
        &sibling_expr_types,
        assert_mode,
        false,
        false,
        &[],
    )
}

/// Like [`transpile_project`] but with package-name tracking (#1475).
#[allow(clippy::too_many_arguments)]
pub fn transpile_project_with_pkg_names(
    entry_name: &str,
    entry_prog: &Program,
    siblings: &[(String, Program)],
    prelude_progs: &[Program],
    expr_types: std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::ir::Ty>,
    sibling_expr_types: Vec<
        std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::ir::Ty>,
    >,
    assert_mode: AssertMode,
    optimize_proved: bool,
    prelude_pkg_names: &[Option<String>],
) -> ProjectOutput {
    transpile_project_with_options(
        entry_name,
        entry_prog,
        siblings,
        prelude_progs,
        expr_types,
        &sibling_expr_types,
        assert_mode,
        optimize_proved,
        false,
        prelude_pkg_names,
    )
}

/// Full multi-file project transpilation with all options.
#[allow(clippy::too_many_arguments)]
pub fn transpile_project_with_options(
    entry_name: &str,
    entry_prog: &Program,
    siblings: &[(String, Program)],
    prelude_progs: &[Program],
    expr_types: std::collections::HashMap<crate::mvl::parser::lexer::Span, crate::mvl::ir::Ty>,
    sibling_expr_types: &[std::collections::HashMap<
        crate::mvl::parser::lexer::Span,
        crate::mvl::ir::Ty,
    >],
    assert_mode: AssertMode,
    optimize_proved: bool,
    extern_stubs: bool,
    prelude_pkg_names: &[Option<String>],
) -> ProjectOutput {
    use crate::mvl::backends::rust::{annotate_prelude_pkg_names, cargo, emitter::RustEmitter};
    let has_main = has_main_fn(entry_prog);
    let extern_count = count_extern_decls(entry_prog);
    let has_extern_rust =
        has_extern_rust_decls(entry_prog) || prelude_progs.iter().any(has_extern_rust_decls);
    let use_runtime = extern_count > 0
        || has_std_imports(entry_prog)
        || siblings.iter().any(|(_, p)| has_std_imports(p))
        || prelude_requires_runtime(prelude_progs);

    let sibling_names: Vec<&str> = siblings.iter().map(|(n, _)| n.as_str()).collect();

    // Lower entry program to TIR.
    let entry_all_fns = crate::mvl::passes::mono::collect_fns(
        std::iter::once(entry_prog).chain(prelude_progs.iter()),
    );
    let entry_mono =
        crate::mvl::passes::mono::monomorphize(entry_prog, &entry_all_fns, &expr_types);
    let entry_tir = crate::mvl::ir::lower::lower(entry_prog, &entry_mono, &expr_types);

    // Lower prelude programs to TIR.
    let mut prelude_tirs = lower_prelude(prelude_progs);
    annotate_prelude_pkg_names(&mut prelude_tirs, prelude_pkg_names);

    // #1695: lower all sibling TIRs upfront so the entry emitter sees
    // cross-module capability-param inference (used by
    // `build_capability_params_map_tir_with_siblings`).  Previously siblings
    // were lowered lazily inside the per-sibling map below, which meant the
    // entry emitter had no visibility into sibling fn signatures.
    let sibling_tirs: Vec<crate::mvl::ir::TirProgram> = siblings
        .iter()
        .enumerate()
        .map(|(idx, (_name, prog))| {
            let sib_et = sibling_expr_types.get(idx).cloned().unwrap_or_default();
            let sib_all_fns = crate::mvl::passes::mono::collect_fns([prog]);
            let sib_mono = crate::mvl::passes::mono::monomorphize(prog, &sib_all_fns, &sib_et);
            crate::mvl::ir::lower::lower(prog, &sib_mono, &sib_et)
        })
        .collect();

    let mut cg = RustEmitter::new();
    cg.assert_mode = assert_mode;
    cg.optimize_proved = optimize_proved;
    cg.test_extern_stubs = extern_stubs;
    cg.emit_program_with_mods_and_siblings(
        &entry_tir,
        &sibling_names,
        &sibling_tirs,
        &prelude_tirs,
    );
    let main_rs = cg.finish();

    let entry_uses_runtime = use_runtime;
    // Peer names/TIRs for each sibling = all OTHER siblings + entry program.
    // Passed to emit_sibling_module so extension-method imports from peer
    // sibling modules are suppressed in the generated Rust (#1706).
    let module_files: Vec<(String, String)> = siblings
        .iter()
        .zip(sibling_tirs.iter())
        .enumerate()
        .map(|(i, ((name, _prog), sib_tir))| {
            let (before, after_with_self) = sibling_tirs.split_at(i);
            let after = &after_with_self[1..];
            let peer_tirs: Vec<crate::mvl::ir::TirProgram> = std::iter::once(&entry_tir)
                .chain(before.iter())
                .chain(after.iter())
                .cloned()
                .collect();
            let (before_names, after_with_self_names) = sibling_names.split_at(i);
            let after_names = &after_with_self_names[1..];
            let peer_names_vec: Vec<&str> = std::iter::once(entry_name)
                .chain(before_names.iter().copied())
                .chain(after_names.iter().copied())
                .collect();
            let mut cg = RustEmitter::new();
            cg.assert_mode = assert_mode;
            cg.optimize_proved = optimize_proved;
            cg.test_extern_stubs = extern_stubs;
            if entry_uses_runtime {
                cg.emit_sibling_module(sib_tir, &peer_names_vec, &peer_tirs, &prelude_tirs);
            } else {
                cg.emit_program(sib_tir);
            }
            (name.clone(), cg.finish())
        })
        .collect();

    let opts = cargo::CargoOptions {
        crate_name: entry_name,
        use_mvl_runtime: use_runtime,
        extern_crates: Vec::new(),
        native_dep_lines: Vec::new(),
        mvl_runtime_path: None,
        use_tokio: false,
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
