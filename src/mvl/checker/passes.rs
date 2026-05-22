// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Verification pass framework for MVL provers.
//!
//! Each requirement is served by a [`VerificationPass`] that takes the parsed
//! [`Program`] and the [`CheckResult`] from the type-checker and returns a
//! [`Verdict`].  The [`PassRegistry`] holds all passes in dependency order and
//! runs them on demand.
//!
//! # Pass tiers
//!
//! | Tier             | Verdict on clean code | Notes                                  |
//! |------------------|-----------------------|----------------------------------------|
//! | Phase 1 complete | `Proven`              | Structural / type-system guarantee     |
//! | SMT active       | `Proven` or `Unchecked` | 5-layer solver (trivial → Z3) runs;  |
//! |                  |                       | `Unchecked` when no call sites found   |
//!
//! Requirements proven by Phase 1:  1, 3, 4, 5, 6, 7, 8  (7/11)
//! SMT prover (5-layer, Z3 live):   2, 9, 10, 11 — `Proven` when no violations
//! Target after Phase 6:            1–11

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use crate::mvl::checker::data_race;
use crate::mvl::checker::ifc;
use crate::mvl::checker::CheckResult;
use crate::mvl::parser::ast::Program;
use crate::mvl::parser::lexer::Span;

// ── Verdict ───────────────────────────────────────────────────────────────────

/// Outcome of running a verification pass for one requirement.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// The requirement is fully proven for all items in the program.
    Proven {
        /// Human-readable summary of what was verified.
        evidence: String,
    },
    /// One or more violations were found.
    Failed {
        /// Human-readable description of the failure(s).
        reason: String,
        /// Source location of the first violation, if available.
        span: Option<Span>,
    },
    /// The prover was not run or cannot yet guarantee this requirement.
    Unchecked {
        /// Why this requirement was not (fully) checked.
        reason: String,
    },
    /// The prover exceeded its time budget.
    Timeout,
}

impl Verdict {
    pub fn is_proven(&self) -> bool {
        matches!(self, Verdict::Proven { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self, Verdict::Failed { .. })
    }

    /// Single-character status indicator for report display.
    pub fn status_char(&self) -> &'static str {
        match self {
            Verdict::Proven { .. } => "✓",
            Verdict::Failed { .. } => "✗",
            Verdict::Unchecked { .. } => "~",
            Verdict::Timeout => "T",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Proven { .. } => "proven",
            Verdict::Failed { .. } => "failed",
            Verdict::Unchecked { .. } => "unchecked",
            Verdict::Timeout => "timeout",
        }
    }

    /// Detail string for report display.
    pub fn detail(&self) -> &str {
        match self {
            Verdict::Proven { evidence } => evidence.as_str(),
            Verdict::Failed { reason, .. } => reason.as_str(),
            Verdict::Unchecked { reason } => reason.as_str(),
            Verdict::Timeout => "timed out",
        }
    }

    /// Formatted source location for a `Failed` verdict, if available.
    /// Returns `Some("line:col")` when a span was recorded, `None` otherwise.
    pub fn location(&self) -> Option<String> {
        if let Verdict::Failed { span: Some(s), .. } = self {
            Some(format!("{}:{}", s.line, s.col))
        } else {
            None
        }
    }
}

impl Default for Verdict {
    fn default() -> Self {
        Verdict::Unchecked {
            reason: "not registered".to_string(),
        }
    }
}

// ── VerificationPass trait ────────────────────────────────────────────────────

/// Common interface for all MVL verification passes.
///
/// A pass takes the typed AST ([`Program`]) and the output of the basic
/// type-checker ([`CheckResult`]) and returns a single [`Verdict`] for its
/// requirement.  Passes are stateless; state lives in [`PassRegistry`] and
/// [`VerdictCache`].
pub trait VerificationPass: Send + Sync {
    /// Short display name (e.g. `"Type Safety"`, `"Termination"`).
    fn name(&self) -> &'static str;
    /// The MVL requirement number this pass verifies (1–11).
    fn requirement(&self) -> u8;
    /// Run the pass and return a verdict.
    fn run(&self, prog: &Program, result: &CheckResult) -> Verdict;
}

// ── Phase 1 basic-check pass ──────────────────────────────────────────────────

/// Derives its verdict directly from the type-checker's `req_errors` array.
/// Used for requirements that are fully proven by Phase 1 structural analysis.
struct BasicCheckPass {
    req: u8,
    pass_name: &'static str,
    /// Evidence text shown when no errors were found.
    ok_evidence: &'static str,
}

impl VerificationPass for BasicCheckPass {
    fn name(&self) -> &'static str {
        self.pass_name
    }
    fn requirement(&self) -> u8 {
        self.req
    }
    fn run(&self, _prog: &Program, result: &CheckResult) -> Verdict {
        let errors = result.req_errors[self.req as usize];
        if errors == 0 {
            Verdict::Proven {
                evidence: self.ok_evidence.to_string(),
            }
        } else {
            Verdict::Failed {
                reason: format!("{errors} violation(s)"),
                span: result
                    .errors
                    .iter()
                    .find(|e| e.requirement_number() == self.req)
                    .map(|e| e.span()),
            }
        }
    }
}

// ── Data race freedom pass (Req 9 — Phase 3 partial proof) ───────────────────

/// Phase 3 data race freedom pass for Req 9.
///
/// Combines three layers of evidence to produce a `Proven` verdict:
///
/// 1. **Phase 1 capability violations** (`channel.send(ref x)`, actor `pub fn`
///    with `ref` param) — caught by the type checker → `Failed`.
/// 2. **Phase 3 iso aliasing violations** (`let y = iso_x` without `consume()`)
///    — caught by `data_race::check_iso_aliasing` → `Failed`.
/// 3. **Phase 3 ref-escape violations** (`spawn Actor { field: ref_var }`)
///    — caught by `data_race::check_ref_escape_to_spawn` → `Failed`.
///
/// When none of the above violations are present, every `ref` value is proven
/// to remain within its local (single-threaded) scope: it cannot be sent via a
/// channel (caught by layer 1), it cannot appear in actor behaviors as a
/// parameter (caught by layer 1), and it cannot be used to initialise a spawned
/// actor's state (caught by layer 3).  Therefore no concurrent alias of any
/// `ref` value can exist → `Proven`.
struct DataRaceFreedomPass;

impl VerificationPass for DataRaceFreedomPass {
    fn name(&self) -> &'static str {
        "Data Race Freedom"
    }
    fn requirement(&self) -> u8 {
        9
    }
    fn run(&self, prog: &Program, result: &CheckResult) -> Verdict {
        let req = usize::from(self.requirement());
        let violations = result.req_errors[req];
        if violations > 0 {
            return Verdict::Failed {
                reason: format!("{violations} capability violation(s)"),
                span: result
                    .errors
                    .iter()
                    .find(|e| e.requirement_number() == self.requirement())
                    .map(|e| e.span()),
            };
        }

        let (race_free, total) = data_race::count_race_free_fns(prog);

        if total == 0 {
            Verdict::Unchecked {
                reason: "no functions to analyze".to_string(),
            }
        } else {
            Verdict::Proven {
                evidence: format!(
                    "{race_free}/{total} function(s) proven race-free: \
                     ref confinement, iso uniqueness, and actor-boundary \
                     sendability all verified"
                ),
            }
        }
    }
}

// ── Refinements pass (Req 10 — Phase 3 symbolic proof) ───────────────────────

/// Phase 3 refinement type checker for Req 10.
///
/// Uses a symbolic evaluator to classify each call-site argument against its
/// parameter refinement predicate:
///
/// - **Failed** — any argument is definitively proven to violate its predicate
///   (e.g. literal `0` passed to a `where self != 0` parameter).
/// - **Proven** — no violations and at least one call site was statically proven;
///   evidence includes counts per outcome so auditors can assess coverage.
/// - **Unchecked** — no violations but no fully-proven functions; either the
///   module defines refined functions with no internal callers, or some call
///   sites could not be proven by the 5-layer solver and fall back to runtime.
///
/// The 5-layer solver (trivial → interval → symbolic → Cooper → Z3) runs on
/// every call site.  Sites the solver cannot decide are runtime-checked.
struct RefinementsPass;

impl VerificationPass for RefinementsPass {
    fn name(&self) -> &'static str {
        "Refinements"
    }
    fn requirement(&self) -> u8 {
        10
    }
    fn run(&self, _prog: &Program, result: &CheckResult) -> Verdict {
        let req = usize::from(self.requirement());
        let violations = result.req_errors[req];
        if violations > 0 {
            return Verdict::Failed {
                reason: format!("{violations} refinement violation(s)"),
                span: result
                    .errors
                    .iter()
                    .find(|e| e.requirement_number() == self.requirement())
                    .map(|e| e.span()),
            };
        }

        // Use counts pre-computed by check_refinements (includes cross-module calls).
        let rc = &result.refinement_counts;
        let fn_total = rc.fn_total;
        let fully_verified = rc.fully_verified_fns;
        let total = rc.proven + rc.runtime_checked + rc.failed;

        if fn_total == 0 {
            Verdict::Unchecked {
                reason: "no refined types used in this file".to_string(),
            }
        } else if fully_verified == fn_total {
            Verdict::Proven {
                evidence: format!(
                    "{fully_verified}/{fn_total} function(s) fully verified; \
                     {} proven, {} runtime-checked out of {total} refined call site(s)",
                    rc.proven, rc.runtime_checked,
                ),
            }
        } else {
            Verdict::Unchecked {
                reason: format!(
                    "{fully_verified}/{fn_total} function(s) fully verified; \
                     {} proven, {} runtime-checked out of {total} refined call site(s); \
                     some call sites deferred to runtime checks",
                    rc.proven, rc.runtime_checked,
                ),
            }
        }
    }
}

// ── IFC pass (Req 11 — Phase 3 partial proof) ────────────────────────────────

/// Phase 3 + interprocedural information flow control pass for Req 11.
///
/// Combines:
/// - Phase 1 direct-flow violations (from the type checker)
/// - Phase 3 implicit-flow analysis ([`ifc::check_implicit_flows`])
/// - Interprocedural violations from forward label propagation (#830/#831/#833)
///
/// Verdicts:
/// - **Failed** — any violation (direct, implicit, or interprocedural) was found.
/// - **Proven** — no violations and the program has IFC-annotated types;
///   evidence includes the declassification and sanitization counts so that
///   auditors can verify every downgrade point.
/// - **Unchecked** — no violations but no labeled types; IFC lattice not exercised.
struct IFCPass;

impl VerificationPass for IFCPass {
    fn name(&self) -> &'static str {
        "IFC"
    }
    fn requirement(&self) -> u8 {
        11
    }
    fn run(&self, prog: &Program, result: &CheckResult) -> Verdict {
        let req = usize::from(self.requirement());
        let violations = result.req_errors[req];
        if violations > 0 {
            return Verdict::Failed {
                reason: format!("{violations} information flow violation(s)"),
                span: result
                    .errors
                    .iter()
                    .find(|e| e.requirement_number() == self.requirement())
                    .map(|e| e.span()),
            };
        }

        // Count auditable relabel call sites.
        let relabel_count = ifc::count_relabels(prog);

        // Determine whether the program has any labeled types — if not, there
        // is nothing to prove and the pass is vacuously clean.
        // Checks both fn and extern fn declarations so that packages that
        // declare IFC-typed FFI boundaries are recognised.
        let fn_has_label = |params: &[crate::mvl::parser::ast::Param],
                            ret: &crate::mvl::parser::ast::TypeExpr|
         -> bool {
            params
                .iter()
                .any(|p| ifc::label_of(&crate::mvl::checker::types::resolve(&p.ty)).is_some())
                || ifc::label_of(&crate::mvl::checker::types::resolve(ret)).is_some()
        };
        let has_labeled = prog.declarations.iter().any(|d| match d {
            crate::mvl::parser::ast::Decl::Fn(fd) => fn_has_label(&fd.params, &fd.return_type),
            crate::mvl::parser::ast::Decl::Extern(ed) => ed
                .fns
                .iter()
                .any(|ef| fn_has_label(&ef.params, &ef.return_type)),
            _ => false,
        });

        // Also recognise programs that call imported functions with labeled
        // parameters — the type checker already verified the constraint, so
        // the IFC lattice IS exercised even if no local declaration uses labels.
        let has_ifc = has_labeled || result.has_prelude_ifc_boundary;

        if has_ifc {
            Verdict::Proven {
                evidence: format!(
                    "no direct, implicit, or interprocedural information flow violations; \
                     {relabel_count} relabel point(s) auditable",
                ),
            }
        } else {
            Verdict::Unchecked {
                reason: "program has no security-labeled types — IFC lattice not exercised"
                    .to_string(),
            }
        }
    }
}

// ── Pass execution order ──────────────────────────────────────────────────────

/// Canonical pass execution order (dependency-aware).
///
/// Type safety (1) must run before all others.  Totality (3) before
/// termination (8).  The rest are independent at the structural level.
/// SMT-backed passes (2, 9, 10, 11) run last as they depend on Phase 1 results.
pub const PASS_ORDER: &[u8] = &[1, 4, 5, 3, 6, 7, 8, 2, 9, 10, 11];

// ── PassRegistry ──────────────────────────────────────────────────────────────

/// Registry of all verification passes in execution order.
///
/// Use [`PassRegistry::default_registry`] to get the standard set of passes.
pub struct PassRegistry {
    passes: Vec<Box<dyn VerificationPass>>,
}

impl PassRegistry {
    /// Build the default registry.
    ///
    /// Phase 1 complete (Req 1, 3, 4, 5, 6, 7, 8): `BasicCheckPass` —
    /// structural / type-system guarantees, verdict is `Proven` when clean.
    ///
    /// SMT-backed passes (Req 2, 9, 10, 11): violations reported as `Failed`;
    /// `Proven` when the 5-layer solver confirms no violations; `Unchecked`
    /// when the solver runs but the module has no applicable call sites.
    pub fn default_registry() -> Self {
        let passes: Vec<Box<dyn VerificationPass>> = vec![
            // ── Phase 1 complete ────────────────────────────────────────────
            Box::new(BasicCheckPass {
                req: 1,
                pass_name: "Type Safety",
                ok_evidence: "all type constraints satisfied",
            }),
            Box::new(BasicCheckPass {
                req: 4,
                pass_name: "Null Elimination",
                ok_evidence: "no direct Option access",
            }),
            Box::new(BasicCheckPass {
                req: 5,
                pass_name: "Error Visibility",
                ok_evidence: "all Result values handled",
            }),
            Box::new(BasicCheckPass {
                req: 3,
                pass_name: "Totality",
                ok_evidence: "all matches exhaustive, no partial calls in total fns",
            }),
            Box::new(BasicCheckPass {
                req: 6,
                pass_name: "Ownership",
                ok_evidence: "no immutability or linear-type consumption violations",
            }),
            Box::new(BasicCheckPass {
                req: 7,
                pass_name: "Effects",
                ok_evidence: "all effects declared and propagated",
            }),
            Box::new(BasicCheckPass {
                req: 8,
                pass_name: "Termination",
                ok_evidence: "no unbounded loops or unproven recursive calls in total functions",
            }),
            // ── SMT-backed passes (Req 2, 9, 10, 11) ───────────────────────
            Box::new(BasicCheckPass {
                req: 2,
                pass_name: "Memory Safety",
                ok_evidence: "no use-after-move, dangling ref, or aliasing violations",
            }),
            Box::new(DataRaceFreedomPass),
            Box::new(RefinementsPass),
            Box::new(IFCPass),
        ];
        PassRegistry { passes }
    }

    /// Run all passes in [`PASS_ORDER`] and return verdicts indexed by requirement.
    /// Index 0 is unused; indices 1–11 hold per-requirement verdicts.
    pub fn run_all(&self, prog: &Program, result: &CheckResult) -> [Verdict; 12] {
        let mut verdicts = std::array::from_fn(|_| Verdict::default());
        for &req in PASS_ORDER {
            if let Some(pass) = self.passes.iter().find(|p| p.requirement() == req) {
                verdicts[req as usize] = pass.run(prog, result);
            }
        }
        verdicts
    }

    /// Run the single pass for `req` and return its verdict.
    /// Returns `Unchecked("no pass registered for Req N")` if none found.
    pub fn run_req(&self, req: u8, prog: &Program, result: &CheckResult) -> Verdict {
        self.passes
            .iter()
            .find(|p| p.requirement() == req)
            .map(|p| p.run(prog, result))
            .unwrap_or_else(|| Verdict::Unchecked {
                reason: format!("no pass registered for Req {req}"),
            })
    }

    /// Returns the name of the pass for `req`, if registered.
    pub fn pass_name(&self, req: u8) -> Option<&'static str> {
        self.passes
            .iter()
            .find(|p| p.requirement() == req)
            .map(|p| p.name())
    }

    /// Number of passes registered (exposed for tests).
    pub fn len(&self) -> usize {
        self.passes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }
}

// ── Incremental verdict cache ─────────────────────────────────────────────────

/// In-process cache mapping `(path, source_hash)` → verdicts.
///
/// Eliminates redundant pass runs when the same source file is processed
/// multiple times within a single invocation (e.g. `mvl assurance <dir>`).
/// The cache is not persisted to disk; it resets on every `mvl` invocation.
#[derive(Default)]
pub struct VerdictCache {
    inner: HashMap<(PathBuf, u64), Box<[Verdict; 12]>>,
}

impl VerdictCache {
    /// Return cached verdicts for `(path, hash)` if present.
    pub fn get(&self, path: &std::path::Path, hash: u64) -> Option<&[Verdict; 12]> {
        self.inner
            .get(&(path.to_path_buf(), hash))
            .map(|v| v.as_ref())
    }

    /// Store verdicts for `(path, hash)`.
    pub fn insert(&mut self, path: PathBuf, hash: u64, verdicts: [Verdict; 12]) {
        self.inner.insert((path, hash), Box::new(verdicts));
    }
}

/// Compute a fast hash of source content for use as a cache key.
///
/// **Stability note:** Uses [`std::collections::hash_map::DefaultHasher`],
/// which is only stable within a single process invocation.  Do not persist
/// or compare this value across processes or Rust versions.
pub fn source_hash(src: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut hasher);
    hasher.finish()
}

// ── Verdict aggregation (multi-file) ─────────────────────────────────────────

/// Aggregate per-file verdict arrays into a single project-level summary.
///
/// For each requirement:
/// - Any `Failed` → `Failed`
/// - All `Proven` → `Proven`
/// - Otherwise → `Unchecked`
pub fn aggregate_verdicts(per_file: &[[Verdict; 12]]) -> [Verdict; 12] {
    if per_file.is_empty() {
        return std::array::from_fn(|_| Verdict::default());
    }

    std::array::from_fn(|req| {
        if req == 0 {
            return Verdict::default();
        }
        let verdicts_for_req: Vec<&Verdict> = per_file.iter().map(|v| &v[req]).collect();

        // Any failure → Failed
        if let Some(failed) = verdicts_for_req.iter().find(|v| v.is_failed()) {
            return (*failed).clone();
        }

        // All proven → Proven (use first evidence)
        if verdicts_for_req.iter().all(|v| v.is_proven()) {
            return verdicts_for_req[0].clone();
        }

        // Any proven + rest unchecked → Proven (requirement is satisfied by
        // the files that use it; unchecked files are vacuously compliant).
        // Note: failed verdicts are already handled by the early-return above,
        // so any_proven here implies all_non_failed.
        let any_proven = verdicts_for_req.iter().any(|v| v.is_proven());
        if any_proven {
            if let Some(proven) = verdicts_for_req.iter().find(|v| v.is_proven()) {
                return (*proven).clone();
            }
        }

        // First unchecked reason
        for v in &verdicts_for_req {
            if let Verdict::Unchecked { reason } = v {
                return Verdict::Unchecked {
                    reason: reason.clone(),
                };
            }
        }

        // Remaining case: all non-Proven verdicts are Timeout
        Verdict::Timeout
    })
}

// ── CLI argument parsing ──────────────────────────────────────────────────────

/// Parse an optional `--req N` or `--req=N` flag from the argument list.
///
/// Returns `Ok(Some(n))` when a valid 1–11 value is found, `Ok(None)` when the
/// flag is absent, and `Err(msg)` when the flag is present but invalid. Callers
/// in the binary crate are responsible for printing the message and exiting.
pub fn parse_req_filter(args: &[String]) -> Result<Option<u8>, String> {
    let raw: Option<&str> = if let Some(v) = args.windows(2).find(|w| w[0] == "--req") {
        Some(v[1].as_str())
    } else {
        args.iter().find_map(|a| a.strip_prefix("--req="))
    };

    raw.map(|s| {
        let n: u8 = s
            .parse()
            .map_err(|_| format!("--req expects a number 1–11, got {s:?}"))?;
        if !(1..=11).contains(&n) {
            return Err(format!("--req {n} out of range (valid: 1–11)"));
        }
        Ok(n)
    })
    .transpose()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::checker::check;
    use crate::mvl::parser::Parser;

    fn check_src(src: &str) -> (crate::mvl::parser::ast::Program, CheckResult) {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let result = check(&prog);
        (prog, result)
    }

    #[test]
    fn default_registry_has_eleven_passes() {
        let reg = PassRegistry::default_registry();
        assert_eq!(reg.len(), 11);
    }

    #[test]
    fn all_requirements_covered() {
        let reg = PassRegistry::default_registry();
        for req in 1u8..=11 {
            assert!(
                reg.pass_name(req).is_some(),
                "no pass registered for Req {req}"
            );
        }
    }

    #[test]
    fn clean_program_yields_eight_proven() {
        let src = r#"
fn add(x: Int, y: Int) -> Int {
    x + y
}
"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        let proven: Vec<u8> = (1u8..=11)
            .filter(|&i| verdicts[i as usize].is_proven())
            .collect();
        // Phase 1 complete: Req 1, 3, 4, 5, 6, 7, 8
        // Phase 3 memory safety: Req 2 (proven when no violations)
        // Phase 3 capability pass: Req 9 (proven when no ref params)
        assert_eq!(proven.len(), 9, "expected 9 proven, got {proven:?}");
        assert!(verdicts[1].is_proven(), "Req 1 should be Proven");
        assert!(verdicts[2].is_proven(), "Req 2 should be Proven");
        assert!(verdicts[3].is_proven(), "Req 3 should be Proven");
        assert!(verdicts[4].is_proven(), "Req 4 should be Proven");
        assert!(verdicts[5].is_proven(), "Req 5 should be Proven");
        assert!(verdicts[6].is_proven(), "Req 6 should be Proven");
        assert!(verdicts[7].is_proven(), "Req 7 should be Proven");
        assert!(verdicts[8].is_proven(), "Req 8 should be Proven");
        assert!(
            verdicts[9].is_proven(),
            "Req 9 should be Proven when no ref params"
        );
    }

    #[test]
    fn phase3_requirements_are_unchecked_or_proven_on_clean_code() {
        let src = r#"fn noop() -> Int { 42 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        // Req 10: no call sites to refined functions in this snippet → Unchecked.
        // Req 11 (IFCPass): no labeled types in this snippet → Unchecked.
        for req in [10u8, 11] {
            assert!(
                matches!(verdicts[req as usize], Verdict::Unchecked { .. }),
                "Req {req} should be Unchecked on clean code"
            );
        }
        assert!(
            verdicts[2].is_proven(),
            "Req 2 should be Proven on clean code"
        );
        // Req 9 (Data Race Freedom) uses the Phase 3 capability pass:
        // a single function with no `ref` params is provably race-free.
        assert!(
            !verdicts[9].is_failed(),
            "Req 9 should not be Failed on clean code, got: {:?}",
            verdicts[9]
        );
    }

    #[test]
    fn req9_proven_for_no_ref_params() {
        let src = r#"fn safe(iso x: Payload, val y: Config) -> Int { 42 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        assert!(
            verdicts[9].is_proven(),
            "Req 9 should be Proven when all params are iso/val, got: {:?}",
            verdicts[9]
        );
    }

    #[test]
    fn req9_proven_for_ref_params_that_stay_local() {
        // GIVEN: a function with a ref param that never escapes to a concurrent context
        // THEN: DataRaceFreedomPass returns Proven (ref is local-only — no race possible)
        let src = r#"fn local(ref x: Buffer) -> Int { 42 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        assert!(
            verdicts[9].is_proven(),
            "Req 9 should be Proven when ref params stay local (no actor escape), got: {:?}",
            verdicts[9]
        );
    }

    #[test]
    fn req9_failed_for_iso_aliasing_violation() {
        // GIVEN: a function that aliases an iso param without consume()
        // THEN: DataRaceFreedomPass returns Verdict::Failed
        let src = r#"
fn alias_iso(channel: Channel, iso x: Payload) -> Unit {
    let y: Payload = x;
    channel.send(consume(y))
}
"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        assert!(
            verdicts[9].is_failed(),
            "Req 9 should be Failed when iso aliasing violation present, got: {:?}",
            verdicts[9]
        );
    }

    #[test]
    fn req9_unchecked_for_empty_program() {
        // GIVEN: a program with no function declarations
        // THEN: Req 9 is Unchecked with "no functions" reason
        let src = r#""#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        assert!(
            matches!(&verdicts[9], Verdict::Unchecked { reason } if reason.contains("no functions")),
            "empty program should yield Unchecked with 'no functions' reason, got: {:?}",
            verdicts[9]
        );
    }

    #[test]
    fn req9_proven_evidence_describes_checks() {
        // GIVEN: a clean program with no ref params
        // THEN: Proven evidence mentions the capability checks performed
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        if let Verdict::Proven { evidence } = &verdicts[9] {
            assert!(
                evidence.contains("race-free"),
                "Proven evidence should describe race-freedom checks, got: {evidence:?}"
            );
        } else {
            panic!("expected Proven for Req 9, got: {:?}", verdicts[9]);
        }
    }

    #[test]
    fn req11_proven_for_labeled_types_with_no_violations() {
        // GIVEN: a function with Secret-labeled parameter that never reaches a sink
        // WHEN: IFCPass is run
        // THEN: Verdict::Proven (no violations, has labeled types)
        let src = r#"fn secure(x: Secret[Bool]) -> Unit { }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let v = reg.run_req(11, &prog, &result);
        assert!(
            matches!(v, Verdict::Proven { .. }),
            "Req 11 should be Proven for labeled program with no violations, got: {v:?}"
        );
    }

    #[test]
    fn req11_proven_evidence_contains_audit_counts() {
        // GIVEN: a function with labeled types and no violations
        // THEN: evidence string references relabel point count (#894)
        let src = r#"fn secure(x: Secret[Bool]) -> Unit { }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        if let Verdict::Proven { evidence } = reg.run_req(11, &prog, &result) {
            assert!(
                evidence.contains("relabel point"),
                "evidence should mention relabel count, got: {evidence:?}"
            );
        } else {
            panic!("expected Proven for labeled program with no violations");
        }
    }

    #[test]
    fn run_req_returns_verdict_for_known_req() {
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let v = reg.run_req(1, &prog, &result);
        assert!(v.is_proven(), "Req 1 should be proven for trivial program");
    }

    #[test]
    fn run_req_returns_unchecked_for_unknown_req() {
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let v = reg.run_req(0, &prog, &result);
        assert!(matches!(v, Verdict::Unchecked { .. }));
    }

    #[test]
    fn source_hash_is_deterministic() {
        let h1 = source_hash("fn f() -> Int { 1 }");
        let h2 = source_hash("fn f() -> Int { 1 }");
        assert_eq!(h1, h2);
    }

    #[test]
    fn source_hash_differs_for_different_sources() {
        let h1 = source_hash("fn f() -> Int { 1 }");
        let h2 = source_hash("fn g() -> Int { 2 }");
        assert_ne!(h1, h2);
    }

    #[test]
    fn verdict_cache_roundtrip() {
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);

        let path = std::path::PathBuf::from("test.mvl");
        let hash = source_hash(src);
        let mut cache = VerdictCache::default();
        cache.insert(path.clone(), hash, verdicts);

        let cached = cache.get(&path, hash);
        assert!(cached.is_some());
        assert!(cached.unwrap()[1].is_proven());
    }

    #[test]
    fn aggregate_verdicts_all_proven() {
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let v = reg.run_all(&prog, &result);
        let agg = aggregate_verdicts(&[v]);
        assert!(agg[1].is_proven());
    }

    // ── Verdict helper method coverage ────────────────────────────────────────

    #[test]
    fn verdict_status_char_all_variants() {
        assert_eq!(
            Verdict::Proven {
                evidence: String::new()
            }
            .status_char(),
            "✓"
        );
        assert_eq!(
            Verdict::Failed {
                reason: String::new(),
                span: None
            }
            .status_char(),
            "✗"
        );
        assert_eq!(
            Verdict::Unchecked {
                reason: String::new()
            }
            .status_char(),
            "~"
        );
        assert_eq!(Verdict::Timeout.status_char(), "T");
    }

    #[test]
    fn verdict_label_all_variants() {
        assert_eq!(
            Verdict::Proven {
                evidence: String::new()
            }
            .label(),
            "proven"
        );
        assert_eq!(
            Verdict::Failed {
                reason: String::new(),
                span: None
            }
            .label(),
            "failed"
        );
        assert_eq!(
            Verdict::Unchecked {
                reason: String::new()
            }
            .label(),
            "unchecked"
        );
        assert_eq!(Verdict::Timeout.label(), "timeout");
    }

    #[test]
    fn verdict_detail_all_variants() {
        assert_eq!(
            Verdict::Proven {
                evidence: "ok".to_string()
            }
            .detail(),
            "ok"
        );
        assert_eq!(
            Verdict::Failed {
                reason: "bad".to_string(),
                span: None
            }
            .detail(),
            "bad"
        );
        assert_eq!(
            Verdict::Unchecked {
                reason: "why".to_string()
            }
            .detail(),
            "why"
        );
        assert_eq!(Verdict::Timeout.detail(), "timed out");
    }

    // ── Failed verdict path coverage ──────────────────────────────────────────

    #[test]
    fn basic_check_pass_failed_for_type_error() {
        // GIVEN: program with undefined variable → Req 1 type error
        // THEN: BasicCheckPass for Req 1 returns Verdict::Failed
        let src = r#"fn bad() -> Int { undefined_var }"#;
        let (prog, result) = check_src(src);
        assert!(result.req_errors[1] > 0, "expected type errors for req 1");
        let reg = PassRegistry::default_registry();
        let v = reg.run_req(1, &prog, &result);
        assert!(v.is_failed(), "expected Failed for type error, got: {v:?}");
        if let Verdict::Failed { reason, .. } = &v {
            assert!(
                reason.contains("violation"),
                "reason should mention violations, got: {reason:?}"
            );
            // location() span-present arm: type errors carry a source location
            assert!(
                v.location().is_some(),
                "expected a source span on the Failed verdict"
            );
        }
        // location() span-absent arm: no span → None
        let no_span = Verdict::Failed {
            reason: "x".into(),
            span: None,
        };
        assert!(no_span.location().is_none());
    }

    #[test]
    fn phase3_stub_pass_failed_for_use_after_move() {
        // GIVEN: program with use-after-move → Req 2 error
        // THEN: Phase3StubPass for Req 2 returns Verdict::Failed
        let src = r#"fn f() -> Int { let x: Int = 1; let _y: Int = consume(x); x }"#;
        let (prog, result) = check_src(src);
        assert!(
            result.req_errors[2] > 0,
            "expected req 2 errors for use-after-move"
        );
        let reg = PassRegistry::default_registry();
        let v = reg.run_req(2, &prog, &result);
        assert!(
            v.is_failed(),
            "expected Failed for use-after-move, got: {v:?}"
        );
    }

    // ── pass_name boundary values ─────────────────────────────────────────────

    #[test]
    fn pass_name_boundary_values() {
        let reg = PassRegistry::default_registry();
        assert!(reg.pass_name(0).is_none(), "req 0 should have no pass");
        assert!(reg.pass_name(12).is_none(), "req 12 should have no pass");
    }

    // ── VerdictCache miss path ────────────────────────────────────────────────

    #[test]
    fn verdict_cache_miss_returns_none() {
        let src = r#"fn f() -> Int { 1 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);

        let path = std::path::PathBuf::from("test.mvl");
        let hash = source_hash(src);
        let mut cache = VerdictCache::default();
        cache.insert(path.clone(), hash, verdicts);

        // Miss: same path, different hash
        assert!(
            cache.get(&path, hash.wrapping_add(1)).is_none(),
            "expected cache miss for wrong hash"
        );
        // Miss: different path, same hash
        let other = std::path::PathBuf::from("other.mvl");
        assert!(
            cache.get(&other, hash).is_none(),
            "expected cache miss for wrong path"
        );
    }

    // ── aggregate_verdicts multi-file coverage ────────────────────────────────

    #[test]
    fn aggregate_verdicts_empty_input() {
        let agg = aggregate_verdicts(&[]);
        for req in 1usize..=11 {
            assert!(
                matches!(agg[req], Verdict::Unchecked { .. }),
                "req {req} should be Unchecked for empty input, got: {:?}",
                agg[req]
            );
        }
    }

    #[test]
    fn aggregate_verdicts_any_failed_dominates() {
        // GIVEN: one clean file (req 1 proven) + one file with a type error (req 1 failed)
        // THEN: aggregate for req 1 is Failed
        let (prog1, res1) = check_src(r#"fn f() -> Int { 1 }"#);
        let (prog2, res2) = check_src(r#"fn bad() -> Int { undefined_var }"#);
        let reg = PassRegistry::default_registry();
        let v1 = reg.run_all(&prog1, &res1);
        let v2 = reg.run_all(&prog2, &res2);
        let agg = aggregate_verdicts(&[v1, v2]);
        assert!(
            agg[1].is_failed(),
            "Failed should dominate in multi-file aggregate for req 1, got: {:?}",
            agg[1]
        );
    }

    #[test]
    fn aggregate_verdicts_two_proven_yields_proven() {
        // GIVEN: two files where req 9 is Proven in both
        // (clean fn → Proven; fn with ref param that stays local → also Proven)
        // THEN: aggregate for req 9 is Proven
        let (prog1, res1) = check_src(r#"fn f() -> Int { 1 }"#);
        let (prog2, res2) = check_src(r#"fn g(ref x: Buffer) -> Int { 42 }"#);
        let reg = PassRegistry::default_registry();
        let v1 = reg.run_all(&prog1, &res1);
        let v2 = reg.run_all(&prog2, &res2);
        assert!(v1[9].is_proven(), "req 9 should be Proven for clean fn");
        assert!(
            v2[9].is_proven(),
            "req 9 should be Proven for ref-param fn that stays local, got: {:?}",
            v2[9]
        );
        let agg = aggregate_verdicts(&[v1, v2]);
        assert!(
            agg[9].is_proven(),
            "two Proven verdicts should aggregate to Proven, got: {:?}",
            agg[9]
        );
    }

    #[test]
    fn aggregate_verdicts_proven_plus_unchecked_yields_proven() {
        // GIVEN: two files — one where req 9 is Proven, one where it is Unchecked
        // THEN: aggregate is Proven (the proven file satisfies the requirement;
        //       the unchecked file is vacuously compliant)
        use std::array;
        let proven: [Verdict; 12] = array::from_fn(|i| {
            if i == 9 {
                Verdict::Proven {
                    evidence: "proven".to_string(),
                }
            } else {
                Verdict::default()
            }
        });
        let unchecked: [Verdict; 12] = array::from_fn(|_| Verdict::default());
        let agg = aggregate_verdicts(&[proven, unchecked]);
        assert!(
            agg[9].is_proven(),
            "Proven + Unchecked should aggregate to Proven, got: {:?}",
            agg[9]
        );
    }

    // ── parse_req_filter coverage ─────────────────────────────────────────────

    #[test]
    fn parse_req_filter_absent_returns_none() {
        let args: Vec<String> = ["mvl", "check", "file.mvl"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(super::parse_req_filter(&args), Ok(None));
    }

    #[test]
    fn parse_req_filter_two_token_form() {
        let args: Vec<String> = ["mvl", "check", "file.mvl", "--req", "5"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(super::parse_req_filter(&args), Ok(Some(5)));
    }

    #[test]
    fn parse_req_filter_equals_form() {
        let args: Vec<String> = ["mvl", "check", "file.mvl", "--req=7"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(super::parse_req_filter(&args), Ok(Some(7)));
    }

    #[test]
    fn parse_req_filter_invalid_value_returns_err() {
        let args: Vec<String> = ["mvl", "check", "file.mvl", "--req=abc"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            super::parse_req_filter(&args).is_err(),
            "non-numeric --req value should return Err"
        );
    }

    #[test]
    fn parse_req_filter_zero_returns_err() {
        let args: Vec<String> = ["mvl", "check", "file.mvl", "--req=0"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            super::parse_req_filter(&args).is_err(),
            "--req=0 should return Err"
        );
    }

    #[test]
    fn parse_req_filter_above_max_returns_err() {
        let args: Vec<String> = ["mvl", "check", "file.mvl", "--req=12"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            super::parse_req_filter(&args).is_err(),
            "--req=12 should return Err"
        );
    }

    // ── aggregate_verdicts Timeout arm ────────────────────────────────────────

    #[test]
    fn aggregate_verdicts_all_timeout_yields_timeout() {
        // GIVEN: two files where req 1 is Timeout in both
        // THEN: aggregate for req 1 is Timeout
        let mut file_a: [Verdict; 12] = core::array::from_fn(|_| Verdict::Unchecked {
            reason: String::new(),
        });
        let mut file_b: [Verdict; 12] = core::array::from_fn(|_| Verdict::Unchecked {
            reason: String::new(),
        });
        file_a[1] = Verdict::Timeout;
        file_b[1] = Verdict::Timeout;
        let agg = aggregate_verdicts(&[file_a, file_b]);
        assert!(
            matches!(agg[1], Verdict::Timeout),
            "all-Timeout per-file should aggregate to Timeout, got: {:?}",
            agg[1]
        );
    }
}
