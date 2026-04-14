//! Verification pass framework for MVL provers.
//!
//! Each requirement is served by a [`VerificationPass`] that takes the parsed
//! [`Program`] and the [`CheckResult`] from the type-checker and returns a
//! [`Verdict`].  The [`PassRegistry`] holds all passes in dependency order and
//! runs them on demand.
//!
//! # Pass tiers
//!
//! | Tier   | Verdict on clean code | Notes                                  |
//! |--------|-----------------------|----------------------------------------|
//! | Phase 1 complete | `Proven`   | Structural / type-system guarantee     |
//! | Phase 3 pending  | `Unchecked` | SMT / flow / borrow analysis needed   |
//!
//! Requirements proven by Phase 1: 1, 3, 4, 5, 6, 7, 8  (7/11)
//! Target after Phase 3 provers:   1–10                   (10/11)
//! Remaining (IFC full analysis):  11                     (1 pending)

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

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

// ── Phase 3 stub pass ─────────────────────────────────────────────────────────

/// Reports Phase 1 violations as `Failed`; returns `Unchecked` when Phase 1
/// found no errors, because a deeper Phase 3 prover is needed for a full proof.
struct Phase3StubPass {
    req: u8,
    pass_name: &'static str,
    /// Reason shown in the `Unchecked` verdict.
    stub_reason: &'static str,
}

impl VerificationPass for Phase3StubPass {
    fn name(&self) -> &'static str {
        self.pass_name
    }
    fn requirement(&self) -> u8 {
        self.req
    }
    fn run(&self, _prog: &Program, result: &CheckResult) -> Verdict {
        let errors = result.req_errors[self.req as usize];
        if errors > 0 {
            Verdict::Failed {
                reason: format!("{errors} violation(s)"),
                span: result
                    .errors
                    .iter()
                    .find(|e| e.requirement_number() == self.req)
                    .map(|e| e.span()),
            }
        } else {
            Verdict::Unchecked {
                reason: self.stub_reason.to_string(),
            }
        }
    }
}

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
    /// Phase 3 pending (Req 2, 9, 10, 11): `Phase3StubPass` — basic violations
    /// reported as `Failed`; `Unchecked` when clean (full proof pending).
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
                ok_evidence: "no immutability violations",
            }),
            Box::new(BasicCheckPass {
                req: 7,
                pass_name: "Effects",
                ok_evidence: "all effects declared and propagated",
            }),
            Box::new(BasicCheckPass {
                req: 8,
                pass_name: "Termination",
                ok_evidence: "no unbounded loops in total functions",
            }),
            // ── Phase 3 pending ─────────────────────────────────────────────
            Box::new(Phase3StubPass {
                req: 2,
                pass_name: "Memory Safety",
                stub_reason: "borrow lifetime analysis pending (Phase 3)",
            }),
            Box::new(Phase3StubPass {
                req: 9,
                pass_name: "Data Race Freedom",
                stub_reason: "full actor model analysis pending (Phase 3)",
            }),
            Box::new(Phase3StubPass {
                req: 10,
                pass_name: "Refinements",
                stub_reason: "SMT verification pending (Phase 3)",
            }),
            Box::new(Phase3StubPass {
                req: 11,
                pass_name: "IFC",
                stub_reason: "full information flow analysis pending (Phase 3)",
            }),
        ];
        PassRegistry { passes }
    }

    /// Run all passes in order and return verdicts indexed by requirement.
    /// Index 0 is unused; indices 1–11 hold per-requirement verdicts.
    pub fn run_all(&self, prog: &Program, result: &CheckResult) -> [Verdict; 12] {
        let mut verdicts = std::array::from_fn(|_| Verdict::default());
        for pass in &self.passes {
            let req = pass.requirement() as usize;
            verdicts[req] = pass.run(prog, result);
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

        // All proven → Proven
        if verdicts_for_req.iter().all(|v| v.is_proven()) {
            return verdicts_for_req[0].clone();
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
    fn clean_program_yields_seven_proven() {
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
        assert_eq!(proven.len(), 7, "expected 7 proven, got {proven:?}");
        assert!(verdicts[1].is_proven(), "Req 1 should be Proven");
        assert!(verdicts[3].is_proven(), "Req 3 should be Proven");
        assert!(verdicts[4].is_proven(), "Req 4 should be Proven");
        assert!(verdicts[5].is_proven(), "Req 5 should be Proven");
        assert!(verdicts[6].is_proven(), "Req 6 should be Proven");
        assert!(verdicts[7].is_proven(), "Req 7 should be Proven");
        assert!(verdicts[8].is_proven(), "Req 8 should be Proven");
    }

    #[test]
    fn phase3_requirements_are_unchecked_on_clean_code() {
        let src = r#"fn noop() -> Int { 42 }"#;
        let (prog, result) = check_src(src);
        let reg = PassRegistry::default_registry();
        let verdicts = reg.run_all(&prog, &result);
        // Req 2, 9, 10, 11 are Phase 3 stubs
        for req in [2u8, 9, 10, 11] {
            assert!(
                matches!(verdicts[req as usize], Verdict::Unchecked { .. }),
                "Req {req} should be Unchecked on clean code"
            );
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
}
