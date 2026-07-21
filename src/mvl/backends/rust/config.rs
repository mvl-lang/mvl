// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Builder-pattern configuration for the Rust transpiler.
//!
//! Replaces the 12 `transpile_*` function variants with a single configuration
//! object that encodes all transpilation modes.
//!
//! # Example
//!
//! ```
//! use mvl::mvl::backends::rust::config::TranspileConfig;
//!
//! // Plain transpilation (no prelude, no instrumentation)
//! let config = TranspileConfig::new("my_crate");
//!
//! // Coverage instrumentation for a test crate source file
//! let config = TranspileConfig::new("my_crate")
//!     .with_coverage(0)
//!     .for_test_crate();
//!
//! // MC/DC instrumentation
//! let config = TranspileConfig::new("my_crate")
//!     .with_mcdc(0);
//!
//! // Mutation testing (test file)
//! let config = TranspileConfig::new("my_crate")
//!     .with_mutation()
//!     .for_test_file();
//! ```

use crate::mvl::backends::AssertMode;
use crate::mvl::ir::TirProgram;

/// Configuration for a single transpilation pass.
///
/// Build with [`TranspileConfig::new`] and the fluent builder methods.
pub struct TranspileConfig {
    /// Crate name used in `Cargo.toml` generation.
    pub(crate) crate_name: String,
    /// File stem for instrumentation reports (coverage, mutation, MC/DC).
    pub(crate) file_stem: String,
    /// Prelude programs (pre-lowered to TIR) prepended before emission.
    pub(crate) prelude_progs: Vec<TirProgram>,
    /// Optional file stems for each prelude program (parallel to `prelude_progs`).
    /// `Some(stem)` enables per-program coverage metadata routing; `None` falls back
    /// to the primary `file_stem`. When empty, treated as all-`None` (#1489).
    pub(crate) prelude_stems: Vec<Option<String>>,
    /// Package name for each prelude program (parallel to `prelude_progs`).
    /// `Some("http")` for programs from `pkg.http`; `None` for stdlib entries.
    /// When present, functions from different packages with the same name are emitted
    /// with package-prefixed Rust names to avoid symbol collisions (#1475).
    pub(crate) prelude_pkg_names: Vec<Option<String>>,
    /// File stems among `prelude_stems` whose functions should receive coverage
    /// instrumentation. Used so sibling library files (e.g. `json.mvl` paired with
    /// `json_test.mvl`) get branch probes rather than being emitted as silent
    /// uninstrumented prelude (#1489).
    pub(crate) coverage_instrument_prelude: std::collections::HashSet<String>,
    /// Coverage instrumentation start counter ID, if enabled.
    pub(crate) coverage_start_id: Option<usize>,
    /// MC/DC instrumentation start counter ID, if enabled.
    pub(crate) mcdc_start_id: Option<usize>,
    /// Enable mutation instrumentation.
    pub(crate) mutation: bool,
    /// Emit `todo!()` stubs for `extern "rust"` blocks (used for source files
    /// compiled into the test crate, where the real FFI dependency is absent).
    pub(crate) test_extern_stubs: bool,
    /// Mark this file as a test file (`*_test.mvl`). Enables `current_file_is_test`
    /// in the emitter, which controls mutation instrumentation scope.
    pub(crate) is_test_file: bool,
    /// How refinement predicates are emitted (`assert!`, `debug_assert!`, or assume).
    pub(crate) assert_mode: AssertMode,
    /// Elide runtime bounds/refinement checks that the prover certified at type-check time.
    ///
    /// Off by default until the optimization stabilises (#1891).
    pub(crate) optimize_proved: bool,
}

impl TranspileConfig {
    /// Create a new config for the given crate name with defaults:
    /// no prelude, no instrumentation, assert mode = Always.
    pub fn new(crate_name: impl Into<String>) -> Self {
        Self {
            crate_name: crate_name.into(),
            file_stem: String::new(),
            prelude_progs: Vec::new(),
            prelude_stems: Vec::new(),
            prelude_pkg_names: Vec::new(),
            coverage_instrument_prelude: std::collections::HashSet::new(),
            coverage_start_id: None,
            mcdc_start_id: None,
            mutation: false,
            test_extern_stubs: false,
            is_test_file: false,
            assert_mode: AssertMode::Always,
            optimize_proved: false,
        }
    }

    /// Set the prelude programs (already lowered to TIR).
    pub fn with_prelude(mut self, progs: Vec<TirProgram>) -> Self {
        self.prelude_progs = progs;
        self
    }

    /// Set package names for each prelude program (parallel to `prelude_progs`).
    /// `Some("http")` for programs from `pkg.http`; `None` for stdlib entries.
    /// Drives cross-package deduplication to avoid symbol collisions (#1475).
    pub fn with_prelude_pkg_names(mut self, names: Vec<Option<String>>) -> Self {
        self.prelude_pkg_names = names;
        self
    }

    /// Set per-prelude file stems and the subset of stems to instrument with coverage.
    ///
    /// `stems` is parallel to `prelude_progs` (set via [`with_prelude`]); `Some(stem)`
    /// enables file-aware coverage metadata for that prelude entry. `instrument` is the
    /// subset of stems whose functions should receive branch probes — used so sibling
    /// library files paired with test files appear in the coverage report (#1489).
    pub fn with_coverage_prelude(
        mut self,
        stems: Vec<Option<String>>,
        instrument: std::collections::HashSet<String>,
    ) -> Self {
        self.prelude_stems = stems;
        self.coverage_instrument_prelude = instrument;
        self
    }

    /// Enable branch coverage instrumentation starting at `start_id`.
    pub fn with_coverage(mut self, start_id: usize) -> Self {
        self.coverage_start_id = Some(start_id);
        self
    }

    /// Enable MC/DC condition instrumentation starting at `start_id`.
    pub fn with_mcdc(mut self, start_id: usize) -> Self {
        self.mcdc_start_id = Some(start_id);
        self
    }

    /// Enable mutation testing instrumentation.
    pub fn with_mutation(mut self) -> Self {
        self.mutation = true;
        self
    }

    /// Set the file stem used in instrumentation reports.
    pub fn with_file_stem(mut self, stem: impl Into<String>) -> Self {
        self.file_stem = stem.into();
        self
    }

    /// Mark this as a source file compiled into the test crate.
    /// Sets `test_extern_stubs = true` so `extern "rust"` blocks become `todo!()`.
    pub fn for_test_crate(mut self) -> Self {
        self.test_extern_stubs = true;
        self
    }

    /// Mark this as a `*_test.mvl` file.
    /// Sets `is_test_file = true` which enables `current_file_is_test` in the emitter,
    /// controlling mutation instrumentation scope.
    pub fn for_test_file(mut self) -> Self {
        self.is_test_file = true;
        self
    }

    /// Set the assert mode for refinement predicate emission.
    pub fn with_assert_mode(mut self, mode: AssertMode) -> Self {
        self.assert_mode = mode;
        self
    }

    /// Enable elision of runtime bounds checks certified by the prover (#1891).
    pub fn with_optimize_proved(mut self) -> Self {
        self.optimize_proved = true;
        self
    }
}
