# ADR-0052: Qualified Module Paths for Nested Files

**Status:** Accepted
**Date:** 2026-07-07
**Issues:** #1714
**Related:** ADR-0033 (sibling-file module style), spec 005 (module system)

---

## Context

MVL derives module names from file basenames (`file_stem()`). Two `.mvl` files in different directories that share a basename (e.g. `compiler/context.mvl` and `compiler/backends/llvm/context.mvl`) both produce the name `"context"`. The resolver registered whichever was enumerated first and silently discarded the other, producing misleading errors like:

```
error[resolver]: backend_llvm: `LocalRef` is not exported from `context`
```

`LocalRef` *is* exported — from `backends/llvm/context.mvl` — but the resolver handed back the wrong `context`. The collision was silent and the diagnostic gave no hint that a wrong module was selected.

This surfaced during issue #1693 (modular LLVM backend split). The workaround was renaming `context.mvl` to `emit_context.mvl` — an arbitrary rename driven by a toolchain limitation, not by the problem domain.

---

## Decision

### 1. Module names are derived from the relative path, not the basename

The compiler derives a module's name by computing the file's path relative to the **base directory** (the directory passed to `mvl check`/`mvl build`) and replacing path separators with dots:

| File (relative to base `compiler/`) | Module name |
|--------------------------------------|-------------|
| `context.mvl` | `"context"` |
| `backends/llvm/context.mvl` | `"backends.llvm.context"` |
| `math/mod.mvl` | `"math"` (trailing `mod` is transparent, per ADR-0033) |

Implementation: `src/mvl/loader.rs::qualified_stem(base_dir, file_path)`.

### 2. Import syntax uses the dot-qualified module name

`use` declarations already accept dot-separated paths (`use std.io.{read}`). The same syntax applies to user modules:

```mvl
use context::TypeEnv;                // "context" → compiler/context.mvl
use backends.llvm.context::EmitCtx; // "backends.llvm.context" → compiler/backends/llvm/context.mvl
```

The dot-path is the canonical identifier for the module. There is no separate disambiguation syntax — the path *is* the name.

### 3. `find_module_file` resolves dot-paths to filesystem paths

When the compiler needs to load a module named `"backends.llvm.context"`, it converts dots to path separators and appends `.mvl`:

```
"backends.llvm.context" → entry_dir/backends/llvm/context.mvl
```

The legacy `mod.mvl` fallback is preserved for single-segment names only.

### 4. The base directory is the directory passed to the CLI command

`mvl check src/` → base = `src/`
`mvl check src/main.mvl` → base = `src/` (parent of the file)
`mvl build src/main.mvl` → base = `src/`

This makes the qualified name stable: a file's module name depends only on its position within the project tree, not on which file imports it.

### 5. Resolver lookup uses dot-joined source paths

The resolver previously joined `source_module` segments with `"::"` for HashMap lookup. It now joins with `"."` to match the qualified module keys:

```
use backends.llvm.context::EmitCtx
→ source_module = ["backends", "llvm", "context"]
→ source_key    = "backends.llvm.context"   ← matches registry key
```

---

## Consequences

**Positive:**

- Files can keep their natural names. `context.mvl` at different nesting levels are distinct modules without renaming.
- The import path mirrors the filesystem hierarchy — readable and predictable.
- No new syntax: dot-separated paths already work for `std.*` imports.
- The `NotExported` diagnostic already cites the resolved file path (added in the same PR), so users can confirm which file the compiler picked.

**Negative / trade-offs:**

- Module names are now position-dependent: moving a file changes its qualified name (and breaks imports that used it). This is intentional — the path is the identity — but requires discipline when refactoring.
- The `base_dir` concept must be propagated to all CLI entry points. A wrong `base_dir` would produce wrong module names. This is mitigated by the simple rule: it is always the directory passed to the command.
- `collect_imported_module_names` now returns multi-segment dot-paths instead of first segments. Callers that assumed single-segment names (e.g. `already_loaded.contains(...)`) must be updated to use `qualified_stem` for consistency.

---

## Rejected alternatives

**Hard-error on basename collision (original PR approach):** Detects the problem but doesn't solve it. The user still has to rename a file. Rejected in favour of enabling coexistence via qualification.

**Separate disambiguation syntax (`use [backends/llvm]context::X`):** Introduces new syntax for a problem solvable by the existing dot-path convention. Rejected per ADR-0002 (language contraction).

**First-match wins with a warning:** Silent wrong-module binding is the original bug. Making it loud but still wrong doesn't help. Rejected.

---

## Relation to language definition

### Spec 005 — Module System

- **Requirement 1** updated: module name is now the relative dot-path, not the basename. New scenarios added for qualified names and same-basename coexistence.
- **Requirement 3** updated: `use sub.dir.module::Item;` is now explicitly documented as valid import syntax for nested modules.

### ADR-0033 — Rust 2018 Sibling-File Module Style

Complementary. ADR-0033 defines the resolution order for a single-segment module name (`foo.mvl` preferred over `foo/mod.mvl`). This ADR extends resolution to multi-segment names by pre-processing dots into path separators before that lookup.
