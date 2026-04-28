# ADR-0015: MC/DC Coverage Execution Model — Eager Evaluation, Unique-Cause, u16 Encoding

**Status:** Accepted
**Date:** 2026-04-28
**Context:** Issue #319 introduces `mvl mcdc`, MC/DC (Modified Condition/Decision Coverage)
analysis for DO-178C DAL-A certification. Several design decisions have non-obvious tradeoffs
and must be documented for future maintainers and DO-178C qualification reviewers.

---

## Decision 1: Unique-Cause MC/DC over Masking MC/DC

MVL implements **Unique-Cause MC/DC** (the DO-178B/DO-178C original criterion):

> For clause C in decision D, there exist test cases t1 and t2 where C differs,
> all other clauses are **identical**, and the outcome differs.

**Rejected alternative: Masking MC/DC** (accepted in DO-178C supplement) allows
other clauses to vary provided they cannot mask the effect of C via short-circuit
evaluation.

| Criterion | Other-clause constraint | Short-circuit semantics |
|-----------|------------------------|------------------------|
| Unique-Cause | All others identical | Not required |
| Masking | May vary (if non-masking) | Required for proof |

**Why Unique-Cause for MVL:**

MVL uses **eager evaluation** for MC/DC instrumentation — all clauses are evaluated
before the outcome is computed, regardless of `&&`/`||` structure. This is safe
because MVL expressions in conditions are **pure** (the effect system guarantees no
observable side effects). With eager evaluation, short-circuit masking cannot occur,
so independence pairs are always findable when they exist. Unique-Cause is therefore
not more restrictive in practice — every Masking pair is also a Unique-Cause pair
when all clauses are independently observed.

Using Unique-Cause also produces simpler qualification evidence for DO-178C Tool
Qualification: the independence criterion is a straightforward pairwise comparison
with no short-circuit reasoning required.

---

## Decision 2: Eager Clause Evaluation via Pre-Computed Locals

The instrumented Rust emits clause values as `let` bindings before the condition:

```rust
// Original: if a && b { … }
let __d0_c0: bool = a;      // clause 0
let __d0_c1: bool = b;      // clause 1
let __d0_outcome: bool = (__d0_c0 && __d0_c1);
#[cfg(test)] crate::__mvl_mcdc::record(0, encoded);
if __d0_outcome { … }
```

**Rejected alternative: instrument inside short-circuit** — intercept each clause
evaluation within the `&&`/`||` chain and record a partial observation, then
combine. This is the approach taken by LLVM's SanitizerCoverage MC/DC mode.

| Approach | Side-effect safety | Observation completeness | Complexity |
|----------|-------------------|--------------------------|-----------|
| **Eager pre-evaluation** | ✅ Pure MVL exprs only | Full truth table per test | Low |
| Short-circuit intercept | Works for impure exprs | Partial (early exits) | High |

**Why eager pre-evaluation:**

MVL's effect system guarantees that condition expressions in `if`/`while` are pure.
Eager pre-evaluation is therefore correct — there are no side effects to observe
out-of-order. It also captures the complete truth table per test case (all clause
values observed), which gives maximum coverage information for the independence check.
The generated code is simple and auditable by a DO-178C reviewer.

---

## Decision 3: u16 Observation Encoding

Each observation is encoded as a single `u16`:
- bits 0..N-1: clause values (bit i = 1 iff clause i was true)
- bit N: decision outcome (1 = true)

**Why u16:**
- Maximum practical clause count (15) fits comfortably in a u16 (16 bits minus 1 outcome bit)
- MVL conditions with 15+ clauses are pathological — the language's minimality ethos
  and DO-178C structural test requirements make such conditions unlikely
- HashSet deduplication bounds observation count to ≤ 2^(N+1) per decision; for N=15
  that is 65536 entries (64 KB of u16 values), acceptable in test memory

**Enforcement:** `MCDCMap::alloc` panics if `clause_count > 15`. This produces a
clear build-time error rather than silent data corruption.

**Rejected alternative: u32 encoding** — doubles memory per observation, no practical
benefit since N > 15 is considered a design defect in MVL code.

---

## Decision 4: Execution Model — Instrument, Compile, Run, Analyse

```
1. Transpile all *_test.mvl + source files with inline tests
   → inject __d{id}_c{i} locals + __mvl_mcdc::record() calls
2. Write combined Cargo project to tempfile::TempDir (random name, auto-cleanup)
3. cargo build --tests  (single compilation)
4. cargo test (single run, MVL_MCDC_OUT=<path> set)
5. Read hex-encoded observations from MVL_MCDC_OUT
6. Pairwise independence check per clause
7. Print score + optional verbose table; exit 0/1
```

This mirrors `mvl mutate` (ADR-0014) in structure but differs in that MC/DC
requires only **one compilation and one test run** (vs. N runs for mutation testing).
The overhead is comparable to a single `mvl test` invocation.

**Temp directory:** Uses `tempfile::TempDir` (random suffix, auto-delete on drop)
rather than a PID-based path. This prevents TOCTOU symlink attacks on shared
machines (CI runners with multiple concurrent users). The generated source and
observation files are therefore not retained after analysis completes.

**Cargo binary resolution:** The `$CARGO` environment variable is honoured before
falling back to `"cargo"`. This ensures the correct toolchain is used in rustup
multi-toolchain environments.

---

## Decision 5: Report Test Ordering via `zzz_` Prefix

The observation-writing test is named `zzz_mvl_mcdc_report` so that it sorts
last in cargo's default alphabetical test ordering, ensuring all other tests have
run and recorded their observations before the file is written.

**Known limitation:** cargo does not formally guarantee test execution order within
a module. If cargo's ordering changes in a future version, some observations may be
missing from the output file and clauses will be falsely reported as uncovered.

**Rejected alternatives:**
- `ctor`/`dtor` crates for process-exit hooks — adds a third-party dependency to
  the generated crate
- `cargo-nextest` hooks — requires nextest, not universally available in CI
- A separate binary target for the report — significant generated-code complexity

The `zzz_` convention is documented in the generated code and has been stable
across cargo versions since 1.0. If it regresses, the failure mode is false
negatives (under-reported coverage), not false positives.

---

## Consequences

- **Qualifiable for DO-178C DAL-A.** Unique-Cause criterion + eager evaluation +
  pure expression guarantee + instrumented binary evidence satisfies Section 6.4.4.2(d).
- **Clause count limit.** Functions with compound conditions of 16+ clauses fail
  the tool with a clear error. This is a deliberate quality gate, not a limitation.
- **Pure expressions required.** MC/DC instrumentation relies on MVL's effect system.
  `extern "rust"` functions called from conditions are not governed by the effect
  system; such uses should be avoided in MC/DC-sensitive code.
- **Single-run observation.** All observations come from one `cargo test` run.
  Non-deterministic tests may under-report coverage (same as `mvl coverage`).

## References

- Issue #319 — feat: MC/DC coverage analysis for MVL programs
- DO-178C / ED-12C Section 6.4.4.2(d) — MC/DC coverage criterion
- ISO 26262-6:2018 Table 10 — MC/DC at ASIL-D
- EN 50128:2011 Table A.5 — MC/DC at SIL 4
- ADR-0013 — Transpiler-mediated code generation (hidden generated code pattern)
- ADR-0014 — Mutation testing execution model (single-compile pattern)
- Spec 010 — MC/DC coverage analysis requirements
