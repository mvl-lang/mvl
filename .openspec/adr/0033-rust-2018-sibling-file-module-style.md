# ADR-0033: Rust 2018 Sibling-File Module Style

**Status:** Accepted
**Date:** 2026-05-16
**Issues:** #794
**Related:** ADR-0002 (language contraction), ADR-0007 (stdlib import model), spec 005 (module system)

---

## Context

MVL previously used `mod.mvl` as the entry file for directory-based modules — the Rust 2015 convention:

```
math/
  mod.mvl       ← module math (entry)
  stats.mvl     ← module math::stats
```

This creates a practical problem: every directory module's entry file has the same name. In editors, tabs become a row of identically-labelled `mod.mvl` files. Navigation degrades as the project grows.

Rust 2018 adopted a better convention — the entry file sits alongside the directory as a sibling:

```
math.mvl        ← module math (entry)
math/
  stats.mvl     ← module math::stats
```

The sibling-file style is unambiguous, editor-friendly, and the convention most developers already expect from Rust 2018+. Issue #793 adopts this for the MVL Rust compiler codebase; this ADR adopts it for the MVL language specification and toolchain.

---

## Decision

### 1. Module resolution order

When the compiler loads an imported module named `foo`, it MUST search in this order:

1. `{entry_dir}/foo.mvl` — preferred (sibling file, Rust 2018 style)
2. `{entry_dir}/foo/mod.mvl` — deprecated fallback (Rust 2015 style)

If neither exists the module is considered absent.

### 2. Deprecation warning for `mod.mvl`

When path 2 is used, the compiler MUST emit:

```
warning: `foo/mod.mvl` is deprecated;
         rename to `foo.mvl` alongside the `foo/` directory
```

The module is still loaded; the warning is non-fatal. This gives existing code one release cycle to migrate.

### 3. `mod.mvl` as project entry is also deprecated

When `mvl build` receives a directory and finds `mod.mvl` as the entry point (fallback after `main.mvl`), the compiler MUST warn:

```
warning: `mod.mvl` as project entry is deprecated; rename to `lib.mvl`
```

### 4. `stem()` derives module name from directory for legacy paths

`loader::stem("foo/mod.mvl")` returns `"foo"` (the directory name), not `"mod"`, so the module name is always derived correctly regardless of which resolution path was taken.

### 5. Future hard switch

In a subsequent release the fallback (path 2) will become an error. The exact release is tracked in issue #794. No action is required now.

---

## Consequences

**Positive:**

- Editor tabs are now distinct per module (`math.mvl`, `geometry.mvl`, `io.mvl`) instead of a row of `mod.mvl`.
- Consistent with Rust 2018, reducing friction for Rust developers reading MVL code.
- LLM-generated code is less likely to create confusion between module entry files.
- `loader::stem` now gives the correct module name for both resolution paths — no special-casing at call sites.

**Negative / trade-offs:**

- Breaking change (deferred): when the hard switch lands, any project using `mod.mvl` must rename the file. The deprecation window exists to avoid a surprise.
- Two resolution paths exist simultaneously during the transition, adding a small amount of complexity to `find_module_file`.

---

## Rejected Alternatives

**Keep `mod.mvl` permanently:** The editor-UX problem is real and scales with project size. The Rust community changed for good reasons. Rejected.

**Hard switch immediately (no deprecation):** Would break any existing MVL code using `mod.mvl`. Rejected in favour of a one-release deprecation window.

**Support both indefinitely:** Violates the "one way to do each thing" principle (ADR-0002). Rejected.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

No compiler-verified requirement is directly affected. Module resolution is a toolchain concern, not a type-system or effect-system property.

| Req | Effect |
|-----|--------|
| All eleven | Unchanged |

### Design Principles (README)

- **Explicit over implicit** — **strengthens**: `math.mvl` alongside `math/` makes the module entry point visible in the directory listing rather than hidden inside the subdirectory.
- **One way to do each thing** — **consistent with** during the deprecation window; **strengthens** once the hard switch lands and `mod.mvl` is rejected.
- All other principles — **consistent with**.

### Specifications

- `005-modules/spec.md` — Requirement 1 updated: new resolution order documented, two new scenarios added (sibling preferred, legacy deprecated), implementation reference updated to `loader.rs::find_module_file` and `loader.rs::stem`. ✅ done in PR #812.
- All other specs — unaffected.
