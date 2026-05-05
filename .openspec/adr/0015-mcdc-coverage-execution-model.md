# ADR-0015: MC/DC Coverage Execution Model — Eager Evaluation, Unique-Cause, u32 Encoding

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

MVL uses **short-circuit evaluation** with per-clause eval flags. Masked clauses
(not evaluated due to `&&`/`||` short-circuiting) are tracked and treated as
unconstrained for independence matching — they impose no requirement on the pair.
This makes the Unique-Cause criterion practical: the "all other clauses identical"
rule applies only to clauses that were actually evaluated in both observations.

Using Unique-Cause produces simpler qualification evidence for DO-178C Tool
Qualification: the independence criterion is a pairwise comparison with a
well-defined mask-handling rule, requiring no reasoning about which clauses
could mask others.

---

## Decision 2: Short-Circuit Clause Evaluation with Eval-Flag Arrays

The instrumented Rust emits clause value arrays and eval-flag arrays, then
evaluates the boolean tree using faithful short-circuit semantics:

```rust
// Original: if a && b { … }
let mut __d0_c = [false; 2];   // clause values
let mut __d0_e = [false; 2];   // evaluation flags
let __d0_outcome: bool = {
    let __d0_t0 = { __d0_e[0] = true; __d0_c[0] = a; __d0_c[0] };
    if __d0_t0 { { __d0_e[1] = true; __d0_c[1] = b; __d0_c[1] } } else { false }
};
#[cfg(test)] crate::__mvl_mcdc::record(0, encoded);
if __d0_outcome { … }
```

Clauses not reached (e.g. `b` when `a` is false) remain `e[i] = false`.
The independence check treats masked clauses as unconstrained.

**Rejected alternative: eager pre-evaluation** — evaluate all clauses before the
condition regardless of `&&`/`||` structure.

| Approach | Faithful semantics | Masked clause info | Complexity |
|----------|-------------------|--------------------|------------|
| **Short-circuit + eval flags** | ✅ Matches runtime | ✅ Precisely tracked | Moderate |
| Eager pre-evaluation | ✗ All clauses always set | ✗ None | Low |

**Why short-circuit evaluation:**

Although MVL expressions are pure, short-circuit evaluation matters for
**observation correctness**. With eager evaluation, observations for `a=false,b=false`
and `a=true,b=true` both have both clauses fully observed — but these cannot form a
Unique-Cause pair for clause A (B also differs). With short-circuit evaluation,
`a=false` masks clause B (e[1]=false), making it unconstrained for independence
matching. This faithfully reflects DO-178C semantics where "tested" means
"evaluated under that test case's execution path."

---

## Decision 3: u32 Observation Encoding

Each observation is encoded as a single `u32`:
- bits 0..N-1: clause values (bit i = 1 iff clause i was true)
- bits N..2N-1: eval flags (bit N+i = 1 iff clause i was evaluated)
- bit 2N: decision outcome (1 = true)

**Why u32:**
- N ≤ 15 clauses require 2N+1 = 31 bits, fitting in a u32
- Maximum practical clause count (15) fits comfortably (31 bits used of 32)
- MVL conditions with 15+ clauses are pathological — the language's minimality ethos
  and DO-178C structural test requirements make such conditions unlikely
- HashSet deduplication bounds observation count to ≤ 2^(2N+1) per decision
- For N=15 that is at most 2^31 ≈ 2 billion entries; in practice far fewer because
  short-circuit masking means many bit combinations are unreachable

**Enforcement:** `MCDCMap::alloc` panics if `clause_count > 15`. This produces a
clear build-time error rather than silent data corruption.

**Rejected alternative: u16 encoding** — insufficient bits; 16 bits can only encode
7 clauses with eval flags (2×7+1 = 15 ≤ 16), excluding the N=8..15 range.

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

- **Qualifiable for DO-178C DAL-A.** Unique-Cause criterion + short-circuit
  evaluation + eval-flag tracking + pure expression guarantee + instrumented
  binary evidence satisfies Section 6.4.4.2(d).
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
