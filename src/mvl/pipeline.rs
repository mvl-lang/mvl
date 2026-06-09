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

use crate::mvl::backends::rust::{
    transpile, transpile_project_with_options, TranspileConfig, TranspileResult,
};
use crate::mvl::backends::AssertMode;
use crate::mvl::checker::{self, CheckResult};
use crate::mvl::parser::ast::Program;

/// Compilation pipeline with composable instrumentation.
///
/// Constructed with [`Pipeline::new`] and configured via builder methods.
/// Provides [`check`](Self::check) and [`build`](Self::build) entry points.
pub struct Pipeline {
    coverage: bool,
    mcdc: bool,
    mutation: bool,
    assert_mode: AssertMode,
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
        let mut config = TranspileConfig::new(crate_name).with_prelude(prelude);
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
        let all_fns = crate::mvl::passes::mono::collect_fns(
            std::iter::once(prog).chain(config.prelude_progs.iter()),
        );
        let mono = crate::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(prog, &mono, &expr_types);
        transpile(&tir, config)
    }

    /// Transpile a multi-file project using this pipeline's settings.
    ///
    /// Delegates to [`transpile_project`] with the pipeline's assert mode.
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
    ) -> crate::mvl::backends::rust::ProjectOutput {
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
            false,
        )
    }
}

impl Default for Pipeline {
    fn default() -> Self {
        Self::new()
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
