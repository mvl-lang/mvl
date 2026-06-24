# ADR-0046: Transitive Dependency Resolution in `mvl update`

**Status:** Superseded by ADR-0047
**Date:** 2026-06-24
**Issues:** #1510, #1511
**Related:** ADR-0012 (extended package model), ADR-0039 (package distribution)

---

## Context

`mvl update` resolved only the packages listed directly in a project's `mvl.toml`.
It never read the `mvl.toml` of those packages to discover their own dependencies.

This meant transitive dependencies were absent from `mvl.lock`. The compiler
loaded all source files from each direct dependency as a prelude; if any of those
files imported symbols from a transitive dep (`use pkg.tls.https.{HttpsError, ...}`),
the compiler could not find the package and the build failed.

**Concrete failure:** `crud_api` depends on `pkg-rest`. `pkg-rest` depends on
`pkg-tls`. `pkg-tls` was never in `crud_api`'s `mvl.lock`. When `mvl test` ran,
it loaded `pkg-rest/src/client.mvl` as a prelude; that file imports from
`pkg.tls.https`; the types `HttpsError`, `HttpsResponse`, and `https_error_msg`
were not resolved. The generated Rust failed to compile with 90+ errors.

---

## Decision

After resolving all direct dependencies in `cmd_update`, run a **BFS over each
newly-locked package's own `mvl.toml`** to discover and lock transitive
dependencies.

### Algorithm

1. Seed a work-queue with all packages now in the lockfile (direct deps just
   resolved plus anything already there from prior runs).
2. Seed a `queued` set with the same names to track what has been seen.
3. While the queue is non-empty:
   - Dequeue a package name.
   - Read its `mvl.toml` from the XDG cache (`pkg_cache_dir(name, version)`).
   - For each dependency declared in that manifest that is **not** already in
     `queued`:
     - Add the name to `queued`.
     - Call `update_one_dep` — same function used for direct deps — which fetches,
       hashes, and locks the package.
     - Push the name onto the work-queue so ITS deps are explored next.
4. After the BFS, write `mvl.lock` once with all resolved packages.

Diamond dependencies (two packages sharing the same transitive dep) are handled
naturally: the second path finds the name already in `queued` and skips it.

### Implementation

`src/mvl/packages.rs` — `cmd_update` function, Phase 2 block added between the
direct-dep loop and `lockfile.write`.  Uses the existing `update_one_dep` helper
unchanged; all options (dry-run, offline, force) flow through unmodified.

### Output

```
Checking github.com/mvl-lang/pkg-tls...
  github.com/mvl-lang/pkg-tls: 0.0.0 → 0.2.1
Resolved 1 transitive package(s).
Updated 1 package(s), 6 unchanged, 0 skipped.
```

---

## Consequences

**Positive:**
- Projects no longer need to manually declare transitive dependencies in
  `mvl.toml` to avoid build failures.
- `mvl test` can compile test files from dependency packages without missing
  symbols from their own transitive deps.
- The BFS naturally handles arbitrary dep-tree depth and diamond deps.

**Negative / Limitations:**
- This run only adds **new** transitive deps; it does not yet update existing
  transitive deps that are in the lockfile at an outdated version. A subsequent
  `mvl update` call will correct outdated transitive deps once direct deps that
  constrain them are themselves updated.
- Packages that cannot be fetched emit a warning and are skipped rather than
  failing hard, matching the existing behavior of direct-dep resolution failures.

**Follow-up work:**
- Version conflict detection: two packages requiring different versions of the
  same transitive dep currently allows the last-resolved version to win silently.
  Minimum Version Selection (ADR-0039 §MVS) should be applied to transitive deps.
- `mvl install` verifies hashes for packages already in `mvl.lock` but does not
  itself resolve transitive deps — it relies on `mvl update` having already
  populated the lock file. This is intentional: `install` is a verification and
  local-copy step, `update` is the resolution step.

---

## Rejected Alternatives

**Resolve transitives in `mvl install`:** `install` is a deterministic, offline-
capable operation that only reads `mvl.lock`. Adding network resolution there
would blur the roles of the two commands and break offline installs.

**Require downstream projects to declare all transitives in `mvl.toml`:** This
is how some early Go tools worked. It is error-prone, verbose, and leaks
implementation details of library packages into consumers. Rejected.

**Lazy resolution at build time:** Detect missing transitive deps during
`load_pkg_modules` and auto-fetch them. This couples the compiler to the network,
adds latency to every build, and makes builds non-reproducible. Rejected.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

No compiler-verified requirement is directly affected. The change is in the
package manager layer, not the type checker or code generator. All eleven
requirements are **left unchanged**.

### Design Principles (README)

- **Explicit over implicit** — consistent with. Transitive deps are now locked
  explicitly in `mvl.lock`, visible and auditable. Nothing is resolved silently
  at build time.
- **Reproducible builds** — strengthens. The lock file now contains the full
  transitive closure, so `mvl install` on a fresh machine produces exactly the
  same package set as the original developer's environment.
- **Supply chain security** (ADR-0039) — strengthens. Every transitive dep is
  hash-locked. A transitive dep introduced silently via a direct dep's update
  now appears in `mvl.lock` with its `sha256` hash and commit SHA.
- All other principles — consistent with.

### Specifications

No specs in `.openspec/specs/` directly cover package resolution depth.
ADR-0039 §Lock File describes the lock file format; the new entries written
for transitive deps follow the same schema and require no format change.
