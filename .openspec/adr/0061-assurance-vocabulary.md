# ADR-0061: Assurance Vocabulary — the Case and Three Levels Below It

**Status:** Accepted
**Date:** 2026-07-30
**Issues:** #2051

---

## Context

"Assurance" names four incompatible things in this repo simultaneously:

| Usage | What it actually is |
|---|---|
| `tools/assurance.py`, `make assurance` / `make assurance-gate` | a **traceability + evidence** measurement (ISPE links, corpus presence, line coverage) |
| `mvl assurance` CLI (`src/cli/assurance.rs`), spec `023-assurance` | per-file compiler **verdicts** (Proven / Failed / Unchecked / N/A for the 11 requirements) |
| ISPE's own `assurance` ratio (E/P, computed inside `tools/assurance.py`) | one derived ratio: of implemented requirements, how many have linked tests |

The sharpest collision is the last one: the word names both the whole dashboard and one derived ratio inside it — and that ratio is the conjunction of the other two (`assured = impl_exists AND tests_linked`), so it mathematically cannot fall below either one while carrying the name of the whole model.

`mvl-lang/mvl-rust` hit the identical collision and resolved it in `mvl-rust` ADR-0007 (mvl-lang/mvl-rust#58). That ADR explicitly flags this repo as having "the same collision" and recommends applying the same vocabulary here rather than letting the two implementations diverge on what the word means. This ADR does that — but re-derives the mapping from what each piece of code in *this* repo actually carries, rather than copying the port's renames verbatim. ADR-0007 itself records a caution about this: its first draft proposed renaming `AssuranceReport` → `EvidenceReport` and `assurance.py` → `traceability.py` from the names alone, and both were wrong once the fields were actually read.

## Decision

### 1. Adopt the vocabulary: assurance is the argument, three levels support it

| Level | Question | Verb | Artefact |
|---|---|---|---|
| **Assurance case** | why should you believe this is fit for purpose? | argue | a case (claim → argument → evidence) |
| **Verification** | does the program satisfy its specification? | verify | verdicts |
| **Traceability** | do intent, spec, program and evidence connect? | trace | link ratios |
| **Evidence** | what artefacts back the claims? | collect | records |

Compliance (DO-178C, ISO 26262, CRA) is not a fourth level — one case maps onto N standards, so compliance consumes the case rather than composing it. This repo has no `make compliance` target and none is added.

### 2. Map this repo's code onto the vocabulary, by content

- **`tools/assurance.py` keeps its name and role.** Read end to end (`tools/assurance.py:1-245`), it currently reports two independent link ratios — Completeness (S→P: spec has an implementation that exists) and Coverage (E→P: spec has linked tests) — plus a Corpus presence count and an opportunistic line-coverage read. That is Traceability plus a slice of Evidence under one heading, which is exactly the dashboard's job. It does not run the compiler and does not produce verdicts, so it does not measure Verification, and it should not claim to.

- **The derived `Assurance (E/P)` ratio is dropped**, not renamed. `assured = impl_exists AND tests_linked` is the conjunction of Completeness and Coverage; it cannot fall below either, so it carries no information the other two don't already show, while occupying the name of the whole model. Removed in this change (`tools/assurance.py`).

- **`mvl assurance` (`src/cli/assurance.rs`) and spec `023-assurance` are, by content, the Verification level** — per-file verdicts (Proven/Failed/Unchecked/N/A) aggregated across the 11 compiler-verified requirements (ADR-0001), plus function/type counts. Nothing about it aggregates traceability or evidence the way `AssuranceReport` does in the Rust port (which bundles `check` + `prove` + `test`/`mcdc`/`coverage` + `assurance` sections into one envelope — this repo has no equivalent single envelope type). Unlike the port, this command does *not* earn the word "assurance" by content.

  **This ADR does not rename `mvl assurance` / spec `023-assurance`.** Renaming a stable CLI subcommand is a breaking change with its own blast radius (docs, `tests/assurance.rs`, `Makefile:317 assure-compiler`, CI) and was explicitly excluded from this change's scope (tracked as follow-up in #2051 if the vocabulary proves out here). Spec `023-assurance` is updated in place to name what it actually measures — Verification — without moving the file.

### 3. Traceability moves from requirement-level to scenario-level linkage

A requirement's single `**Tests:**` line was previously treated as a binary "covered" signal for the whole requirement, regardless of how many `#### Scenario:` blocks it contained. `.openspec/specs/000-parser/spec.md` Requirement 1 is a representative case: five scenarios (keywords, operators, security labels, literals, source locations), one `**Tests:**` line pointing at inline lexer tests — previously scored as 100% coverage for that requirement.

`tools/assurance.py` now weights coverage by scenario count: a requirement's contribution to Coverage is `min(tests_listed, scenario_count) / max(scenario_count, 1)`, not a flat 0/1. Requirements with no `#### Scenario:` blocks (several specs, e.g. `023-assurance`, predate the scenario format) fall back to the previous binary behavior — there is nothing finer-grained to weight against.

This is expected to lower the reported Coverage number. That is the point: a lower, honest number is preferred over a higher one that doesn't hold up. Per the port's experience (`mvl-rust` ADR-0007), the equivalent change there moved reported coverage 100% → 0% → 75% (0% immediately after the metric changed, 75% after actually doing the linking work). Backfilling this repo's spec corpus with per-scenario `**Tests:**` links to recover the score is out of scope for this change and is a natural follow-up once the new number is visible.

### 4. Each level gets its own runnable target

| Target | Level | What it runs |
|---|---|---|
| `make traceability` | Traceability | `tools/assurance.py --traceability-only` — no `cargo`, no coverage cache, ~0.1s |
| `make verification` | Verification | alias for the existing `make test` (already the pre-PR gate: unit + type-checker + rust/rust backend + solver + stdlib) |
| `make evidence` | Evidence | alias for the existing `make coverage` |
| `make assurance` | the case | unchanged — `tools/assurance.py` dashboard, all levels under one heading |
| `make assurance-gate` | the case, as a gate | unchanged entry point; internal thresholds now read the restructured metrics |

`verification` and `evidence` are thin aliases rather than new logic — `make test` and `make coverage` already do the work; the alias makes the vocabulary discoverable without duplicating or renaming established targets that CI and contributors already depend on.

Hygiene (`format-check`, `lint`) is not a level for the same reason it isn't in the Rust port: it gates whether the code is well-formed enough to be worth verifying, and folding it into `verification` would make "verification: pass" mean two unrelated things.

### 5. CI: the assurance job runs after verification, not in parallel with it

`.github/workflows/ci.yml`'s `assurance` job previously depended only on `changes` (path detection) and ran independently of the `check` job (which is this repo's verification gate — build, clippy, `cargo test`, corpus matrix). A green traceability ratio reported over code that fails to build is worse than no ratio, because it reads as progress. `assurance` now `needs: [changes, check]` and only proceeds when `check` succeeded or was skipped (path-filtered out) — it does not block on `check` being skipped, only on `check` failing.

## Consequences

- The reported Coverage / traceability percentage in `make assurance` and the CI PR comment will likely drop once scenario-level weighting lands — this is intentional (§3) and should be called out in the PR, not treated as a regression to hide.
- `mvl assurance` (Rust CLI) and spec `023-assurance` keep their current names despite being, by content, the Verification level. This is a deliberate, documented divergence from a literal reading of the vocabulary, not an oversight — renaming them is tracked as potential follow-up, not done here.
- No `make compliance` target exists or is added; DO-178C/ISO 26262/CRA mapping remains unbuilt until something downstream actually asks for it.
- `assurance-gate`'s threshold semantics change (no more E/P ratio) — `--min` now gates on completeness and scenario-weighted coverage directly; see `tools/assurance.py`.

## Rejected Alternatives

- **Renaming `mvl assurance` → `mvl verify` / spec `023-assurance` → `023-verification` in this change.** Correct by content per §2, but a breaking CLI rename with its own review surface (docs, tests, Makefile, CI, any external scripts). Left for a follow-up once the traceability/evidence restructuring here has settled.
- **Renaming `tools/assurance.py` → `traceability.py`.** Rejected for the same reason ADR-0007 rejected it in the port: the script's scope is traceability *and* evidence under one dashboard heading, which is the case-assembly role, not a single-level tool.
- **Backfilling scenario-level `**Tests:**` links across the spec corpus in this change.** Real work, but separable from the metric change that reveals the need for it; doing both at once risks quietly tuning the metric to hit a target number rather than reporting an honest one.

## Relation to language definition

### Eleven Requirements (ADR-0001)

Not directly affected. This change touches reporting and tooling around the eleven requirements, not the requirements themselves or how the checker verifies them.

### Design Principles (README)

- **Explicit over implicit** — strengthens: a metric named for what it measures (traceability vs. verification vs. evidence) is more explicit than one overloaded name standing in for all three.
- **One way to do it** — consistent with: no new parallel reporting mechanism is introduced; existing tools are relabeled and one derived ratio is removed.
- Other principles: consistent with, not directly engaged.

### Specifications

- `.openspec/specs/023-assurance/spec.md` — updated to state explicitly that it specifies the Verification level (per-file verdicts), cross-referencing this ADR. Not renamed or moved.
- No other spec's requirements change; specs using the `#### Scenario:` format are affected only in how `tools/assurance.py` scores their existing `**Tests:**` links (§3), not in their content.

## Links

- `mvl-rust` ADR-0007 (mvl-lang/mvl-rust#58), applied in mvl-lang/mvl-rust#59
- #2051 (this repo's issue proposing the split), #2050 ("ADR-0060: a declared source of truth must be executable and falsifiable" — same spirit as making each level runnable; that number is reserved for #2050's ADR, hence this one is 0061)
- #2010 (assurance CLI `--format=json` epic — inherits this vocabulary)
