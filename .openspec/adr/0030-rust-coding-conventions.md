# ADR-0030 — Rust coding conventions for the MVL compiler implementation

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** #793 #795
**Related:** ADR-0018 (pipeline module layout), ADR-0027 (multi-backend architecture)

---

## Context

The MVL compiler is implemented in Rust (~60k lines, ~86 files). As the codebase
grows and more contributors (human and AI) touch it, implicit conventions become a
liability. This ADR makes the established conventions explicit so they can be
referenced, enforced, and evolved deliberately.

---

## Decision

### 1. Rust edition: 2021

All crates use `edition = "2021"` in `Cargo.toml`. This enables:
- `use` paths that don't require explicit `mod` declarations in `main.rs`
- Improved closure capture semantics
- The Rust 2018 module system (see rule 2)

### 2. Module layout: `foo.rs` over `foo/mod.rs`

Use the Rust 2018 file-per-module style:

```
src/
├── mvl.rs          ← defines the `mvl` module (not mvl/mod.rs)
├── mvl/
│   ├── checker.rs  ← defines `mvl::checker`
│   └── ...
```

**Rationale:** Avoids having many files all named `mod.rs` open simultaneously
in editors and diffs. The file name carries the module name, making navigation
faster and grep more meaningful.

### 3. Formatting: `cargo fmt`

All code must pass `cargo fmt --check`. Enforced via pre-commit hook. No custom
`rustfmt.toml` — use defaults. Do not manually reformat code that `cargo fmt`
would not reformat.

### 4. Linting: `cargo clippy`

All code must pass `cargo clippy` with no warnings. Enforced via pre-commit hook.
Suppress individual lints only with `#[allow(clippy::...)]` and a comment
explaining why. Do not use `#![allow(...)]` at crate level to silence categories
of warnings.

### 5. Error handling: `anyhow::Result` at boundaries, typed errors internally

- CLI entry points and pipeline orchestration use `anyhow::Result<T>` for
  ergonomic error propagation with context.
- Internal modules (checker, resolver, refinements) define typed error enums
  so that call sites can match on specific failure modes.
- Do not use `.unwrap()` or `.expect()` in production paths. Panics are
  acceptable only in tests and unreachable branches with a comment.

### 6. No macros for code generation

Per ADR-0013, the transpiler generates Rust source code directly. Macros
(`macro_rules!`, proc macros) are not used for code generation inside the
compiler itself. This keeps the compiler's internal representation explicit
and inspectable.

---

## Consequences

**Good:**
- Module names are visible in filenames — `grep`, editor tabs, and git diffs
  become easier to read
- Pre-commit enforcement means violations never land on `main`
- Explicit conventions reduce review friction for AI-assisted contributions

**Bad / trade-offs:**
- Migrating `mod.rs` files to `foo.rs` style is mechanical but touches many
  files in one commit (done in #795)
- `anyhow` vs typed errors is a judgement call at module boundaries; the rule
  is a guideline, not a hard constraint

---

## Rejected alternatives

- **Custom `rustfmt.toml`**: adds maintenance burden; default formatting is
  good enough and universally understood
- **`#![deny(clippy::all)]` at crate level**: too aggressive during rapid
  development; per-site suppression with rationale is more useful signal
- **Keep `mod.rs` style**: no technical objection, but the new style is
  unambiguously clearer at scale

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This ADR governs the compiler implementation, not the MVL language itself.
None of the eleven language requirements are directly affected. Consistent
code conventions indirectly support requirement 11 (toolchain reliability)
by making the compiler easier to maintain and audit.

### Design Principles (README)

- **consistent with** all ten design principles — this is an implementation
  concern, not a language semantics concern

### Specifications

No specs in `.openspec/specs/` are affected. This ADR applies to the
Rust source of the compiler, not to MVL language behaviour.
