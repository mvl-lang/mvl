---
domain: toolchain
version: 0.1.0
status: draft
date: 2026-05-27
---

# 023 — Assurance Report

The `mvl assurance` command generates a per-module, per-requirement verification
report.  It aggregates type-checker verdicts for the 11 compiler-verified
requirements (ADR-0001) and presents them as a structured dashboard.

Referenced in ADRs 0006, 0012, 0014 but previously had no behavioral specification.

## Requirements

### Requirement 1: Per-Module Requirement Verdicts [MUST]

For each `.mvl` source file, the assurance report MUST display a verdict for each
of the 11 requirements: Proven, Failed, Unchecked (~), or N/A (—).

**Implementation:** `src/cli/assurance.rs`, `src/mvl/checker/passes.rs`

### Requirement 2: Aggregate Summary [MUST]

The report MUST display aggregate counts: total functions, verified functions,
partial functions, extern functions, test functions, struct/enum types, and
per-requirement error counts.

**Implementation:** `src/cli/assurance.rs::run`

### Requirement 3: JSON Output [SHOULD]

The `--json` flag SHOULD produce machine-readable JSON output suitable for CI
integration and downstream tooling.

**Implementation:** `src/cli/assurance.rs` (json parameter)

### Requirement 4: Verbose Mode [SHOULD]

The `--verbose` flag SHOULD display per-function detail including totality,
effects, and individual requirement verdicts.

**Implementation:** `src/cli/assurance.rs` (verbose parameter)

### Requirement 5: Verdict Caching [SHOULD]

Verdicts SHOULD be cached by source hash so that unchanged files are not
re-verified on subsequent runs.

**Implementation:** `src/mvl/checker/passes.rs::VerdictCache`, `src/mvl/checker/passes.rs::source_hash`

## Known Limitations

- **L1**: No incremental re-verification — the cache is session-local, not persisted to disk.
- **L2**: `mvl assurance` currently requires a project directory, not individual files.
