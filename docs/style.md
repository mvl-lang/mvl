# MVL File Documentation Style Guide

Every `.mvl` file — whether stdlib module, example, or user code — should
follow this documentation convention.  Comments in MVL are stripped by the
lexer and have no runtime cost; clear docs help humans and LLMs understand
intent without reading every line.

---

## Module Header (`//!`)

Every `.mvl` file starts with a module-level doc block using `//!` lines.

```mvl
//! std.log — structured logging with IFC enforcement.
//!
//! Provides four severity levels: debug, info, warn, error.
//! All functions declare `! Log` and are total (no Result return).
//! IFC: Secret[T] and Tainted[T] arguments are rejected at compile time.
//!
//! Import:
//!   use std.log.{log_debug, log_info, log_warn, log_error}
//!
//! Effects: Log (all functions).
```

### Required fields

| Field | Description |
|-------|-------------|
| First line | `//! <module-path> — <one-line purpose>` |
| Body | What it provides; what it does NOT do |
| Import | Canonical `use` snippet |
| Effects | Summary of declared effects, or `none` for pure modules |

### Optional fields

- **Dependencies** — other stdlib modules this one relies on
- **Assurance note** — extern block count and trust-boundary summary
- **Phase / ADR** — if the module was introduced by a specific ADR or phase

---

## Item Docs (`///`)

Public functions, types, and constants use `///` doc comments immediately
above the declaration.

```mvl
/// Parse a JSONL file line by line, returning each valid JSON object.
///
/// Returns `Err` if the file cannot be opened; individual malformed lines
/// are silently skipped (use `parse_strict` for fail-fast behaviour).
pub fn parse_jsonl(path: Clean[String]) -> Result[List[String], String] ! FileRead {
    ...
}
```

### Principles

1. **Purpose first** — what does this do? One sentence is usually enough.
2. **No redundancy** — don't echo the type signature.  `fn add(a: Int, b: Int) -> Int` does not need `/// Adds a to b`.
3. **Edge cases** — document non-obvious behaviour, error conditions, empty-collection semantics.
4. **Effect notes** — if the function's effect is surprising or constrained, say so.
5. **Examples** — for complex functions, a short GIVEN/WHEN/THEN snippet helps.

---

## Requirement References

When a function or module is specifically designed to satisfy a safety
requirement, reference it with the shorthand `R<N>`:

```mvl
// R3 (Totality): all branches are exhaustive; no partial calls.
// R7 (Effects): this function is pure — no ! annotations.
// R2 (Memory Safety): input borrowed via val — no clone of the block.
```

Use these in both module headers and inline comments.  The full requirement
names are in `docs/requirements.md`.

---

## Inline Comments

Use plain `//` for non-doc explanatory comments:

```mvl
// ── section separator ────────────────────────────────────────────────────────

// Phase N: rationale for a design decision.

// NOTE: edge-case that isn't obvious from the code.
```

Section separators (`// ── Name ─────`) are encouraged for files longer than
~50 lines to aid navigation.

---

## Enforcement

Run `mvl check` to validate requirement coverage.  A linter rule for
`missing-module-doc` and `missing-public-doc` is tracked in #727 (optional,
Phase 7).

---

## Quick Reference

```
//! module-path — one-line summary.
//!
//! What it provides.  What it does NOT do.
//!
//! Import:
//!   use module.{fn1, fn2}
//!
//! Effects: EffectA, EffectB (or `none`).

/// One-line description of what this item does.
///
/// Detail for non-obvious behaviour.
pub fn example(...) -> ... { ... }

// Plain // for inline rationale, section separators, and NOTE: blocks.
```
