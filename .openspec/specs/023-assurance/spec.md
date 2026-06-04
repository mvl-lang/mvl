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

**Tests:** `tests/assurance.rs::assurance_shows_requirement_verdicts`, `src/cli/assurance.rs::assurance_tests::req_errors_populated_from_checker`, `src/cli/assurance.rs::assurance_tests::req_errors_zero_on_clean_program`

### Requirement 2: Aggregate Summary [MUST]

The report MUST display aggregate counts: total functions, verified functions,
partial functions, extern functions, test functions, struct/enum types, and
per-requirement error counts.

**Implementation:** `src/cli/assurance.rs::run`

**Tests:** `tests/assurance.rs::assurance_shows_aggregate_summary`, `src/cli/assurance.rs::assurance_tests::test_fn_count_is_separate_from_fn_count`, `src/cli/assurance.rs::assurance_tests::no_test_fns_means_zero_count`, `src/cli/assurance.rs::assurance_tests::struct_and_enum_types_counted`, `src/cli/assurance.rs::assurance_tests::effects_fn_counted`

### Requirement 3: JSON Output [SHOULD]

The `--json` flag SHOULD produce machine-readable JSON output suitable for CI
integration and downstream tooling.

**Implementation:** `src/cli/assurance.rs` (json parameter)

**Tests:** `tests/assurance.rs::assurance_json_is_valid`, `tests/assurance.rs::assurance_json_requirements_array`

### Requirement 4: Verbose Mode [SHOULD]

The `--verbose` flag SHOULD display per-function detail including totality,
effects, and individual requirement verdicts.

**Implementation:** `src/cli/assurance.rs` (verbose parameter)

**Tests:** `tests/assurance.rs::assurance_verbose_shows_per_requirement_detail`, `src/cli/assurance.rs::assurance_tests::fn_details_populated`

### Requirement 5: Verdict Caching [SHOULD]

Verdicts SHOULD be cached by source hash so that unchanged files are not
re-verified on subsequent runs.

**Implementation:** `src/mvl/checker/passes.rs::VerdictCache`, `src/mvl/checker/passes.rs::source_hash`

**Tests:** `src/mvl/checker/passes.rs::tests::source_hash_is_deterministic`, `src/mvl/checker/passes.rs::tests::source_hash_differs_for_different_sources`, `src/mvl/checker/passes.rs::tests::verdict_cache_roundtrip`

## Known Limitations

- **L1**: No incremental re-verification — the cache is session-local, not persisted to disk.
- **L2**: `mvl assurance` currently requires a project directory, not individual files.
