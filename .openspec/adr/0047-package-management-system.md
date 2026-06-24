# ADR-0047: Package Management System

**Status:** Accepted
**Date:** 2026-06-24
**Supersedes:** ADR-0012 (extended package model), ADR-0039 (package distribution),
ADR-0046 (transitive dependency resolution)
**Related:** ADR-0009 (toolchain layout — XDG paths), ADR-0006 (FFI / bridge.rs),
ADR-0007 (stdlib import model)
**Issues:** #57, #56, #252, #1120, #1129, #1131, #1135, #1245, #1510, #1511

---

## Context

MVL needs a complete package management story: how packages are structured, how
they are identified and distributed, how the lock file works, how transitive
dependencies are resolved, and how supply chain security is enforced.

Three earlier ADRs captured pieces of this as the design evolved:

- **ADR-0012** (2026-04-15): established the package format, `pub`/`internal/`
  visibility, git-only registry, versioning, and `mvl build` integration.
- **ADR-0039** (2026-05-29): nailed down the repository-less distribution model,
  lock file hash format, SBOM generation, and the four-phase security roadmap.
- **ADR-0046** (2026-06-24): added BFS-based transitive dependency resolution
  to `mvl update` after discovering that transitive deps were absent from lock files.

This ADR consolidates all three into a single authoritative reference, updating
sections where the design has evolved (e.g. ADR-0012 §5 said "no separate
`mvl install`" — that is no longer true).

---

## Decision

### 1. Package Format — `mvl.toml`

Packages use the same `mvl.toml` manifest as projects. No second file format.

```toml
[package]
name         = "http"
version      = "0.5.0"
license      = "Apache-2.0"
requires-mvl = ">=0.205.0"

# Required when any extern "rust" block exists in the package
extern-rationale = "wraps hyper for async HTTP; no pure-MVL HTTP stack yet"

[dependencies]
"github.com/mvl-lang/pkg-tls" = {
  git      = "https://github.com/mvl-lang/pkg-tls",
  tag      = "v0.2.1",
  rationale = "HTTPS transport for REST client"
}
```

**Required fields:** `name`, `version`, `license`, `requires-mvl`.  
**Required when any `extern "rust"` block exists:** `extern-rationale`.

### 2. Package Structure — `src/` and `src/internal/`

```
pkg-http/
├── mvl.toml
├── bridge.rs          # Rust implementations of extern fns
├── src/
│   ├── http.mvl       # public API — full 11-requirement verification
│   ├── rest.mvl
│   └── internal/
│       └── ffi.mvl    # extern "rust" { hyper } — hidden from users
└── tests/
    └── http_test.mvl  # package-local tests
```

**Visibility rules:**

| Location | Exported to users? |
|---|---|
| `src/*.mvl` | Yes — package public API |
| `src/internal/*.mvl` | No — package-private; the compiler MUST reject `use pkg.http.internal.ffi` from outside |

Both `pub`/private and `internal/` work together:
- `pub` controls item-level visibility (one function, one type)
- `internal/` makes an entire subtree private (one subsystem, visible at a glance)

### 3. Package Identity — Repository-less, Git-Native

A package is identified by its full git URL and a semver tag. There is no central
registry. Every `https://github.com/<owner>/<repo>` is a resolvable package address.

```toml
[dependencies]
"github.com/mvl-lang/pkg-sqlite" = {
  git = "https://github.com/mvl-lang/pkg-sqlite",
  tag = "v0.2.1",
  rationale = "SQLite FFI bindings"
}
```

Short form via `mvl add`:
```bash
mvl add github.com/mvl-lang/pkg-sqlite v0.2.1
# → writes the full [dependencies] entry above
```

Rationale for Git-only: full URLs prevent name squatting; the trust root (GitHub
HTTPS/SSH) is already trusted by most organizations; no registry infrastructure to
operate. A central registry at `registry.mvl-lang.org` is deferred to a future
phase when the language has stabilised.

### 4. Lock File — `mvl.lock`

Every resolved dependency (direct and transitive) is recorded with its exact
git URL, version, commit SHA, and SHA-256 archive hash:

```toml
[[package]]
name         = "github.com/mvl-lang/pkg-sqlite"
version      = "0.2.1"
git          = "https://github.com/mvl-lang/pkg-sqlite"
commit       = "817e8bcb92d8dc04be7d73c4dfe18d671bfbbfb0"
hash         = "sha256:612af84482fe4d92f7e81f557abce0794cc6f2182eefea55f1d6d1ce4f8947b5"
last-checked = 1782321591
```

The lock file is the integrity anchor. `mvl install` verifies hashes before
extracting packages. Any hash mismatch is a hard error.

Hash implementation: `src/mvl/packages/hash.rs` — pure-Rust FIPS 180-4 SHA-256
with no external crate dependencies (#1245). Normalization: raw bytes, forward-slash
paths, lexicographic sort for tree digests, lowercase hex.

`mvl.lock` MUST be committed to source control. `mvl build --locked` fails if the
lock is stale (CI enforcement).

### 5. Cache Layout

See ADR-0009 for the full XDG directory hierarchy. Package-relevant paths:

```
$XDG_DATA_HOME/mvl/pkg/
└── github.com_mvl-lang_pkg-sqlite/
    └── 0.2.1/
        ├── mvl.toml
        ├── src/
        └── bridge.rs

myproject/
├── mvl.toml
├── mvl.lock
└── .mvl/
    └── pkg/
        └── github.com_mvl-lang_pkg-sqlite/  ← local copy (project-isolated)
```

Two-tier resolution: global XDG cache (shared, avoids re-download) + local
project copy (isolation, auditability). Local overrides in `.mvl/pkg/<name>/`
take precedence and bypass the lock file (monorepo use case).

### 6. CLI Commands

| Command | Role |
|---------|------|
| `mvl add <git-url>[@<tag>]` | Fetch one package, add to `mvl.toml` + `mvl.lock` |
| `mvl install` | Verify hashes and copy all locked packages from global cache to `.mvl/pkg/`. Deterministic, offline-capable — does NOT do version resolution. |
| `mvl update [<name>]` | Re-resolve versions, write updated `mvl.lock`. Network required. |
| `mvl sbom` | Generate CycloneDX / SPDX SBOM from `mvl.toml` + `mvl.lock` |
| `mvl audit` | *(planned Phase C)* Check deps against OSV.dev CVE database |

`mvl install` and `mvl update` are intentionally separate:
- `install` is idempotent and offline — it only reads `mvl.lock`.
- `update` owns version resolution and network access — it writes `mvl.lock`.

This split matches the Cargo model and prevents silent version drift during CI.

### 7. Transitive Dependency Resolution

`mvl update` resolves the full transitive closure, not just direct dependencies.

**Algorithm (Phase 2 of `cmd_update` in `src/mvl/packages.rs`):**

1. Seed a work-queue and a `queued` set with all packages now in the lockfile
   (direct deps just resolved + anything there from prior runs).
2. While the queue is non-empty:
   - Dequeue a package name.
   - Read its `mvl.toml` from the XDG cache (`pkg_cache_dir(name, version)`).
   - For each dep in that manifest **not** already in `queued`:
     - Add to `queued`; call `update_one_dep`; push onto the work-queue.
3. Write `mvl.lock` once with all resolved packages.

Diamond dependencies (A → C ← B) are handled naturally: the second path hits
`queued` and skips. The BFS visits every package exactly once regardless of
dep-tree depth.

**Limitations (current):**
- Adds new transitive deps but does not update existing ones to a newer version
  in the same pass. A second `mvl update` after bumping a direct dep will update
  its constrained transitives.
- No version conflict detection yet: when two packages require different versions
  of the same transitive dep, the last-resolved version wins silently. Minimum
  Version Selection (MVS, see §Rejected Alternatives) is the planned fix.

**Example output:**
```
Checking github.com/mvl-lang/pkg-tls...
  github.com/mvl-lang/pkg-tls: 0.0.0 → 0.2.1
Resolved 1 transitive package(s).
Updated 1 package(s), 6 unchanged, 0 skipped.
```

### 8. Supply Chain Security — Four-Phase Roadmap

**Phase A: SBOM Generation** *(implemented — #1120, #1135)*

`mvl sbom` generates a software bill of materials from `mvl.toml` + `mvl.lock`.

Formats:
- **CycloneDX 1.5 JSON** (default) — CI-friendly
- **SPDX 2.3 tag-value** — compliance toolchain format

Component type: `application` when `main.mvl` exists in the project root;
otherwise `library`.

**Phase B: Package Signing via GitHub Attestations** *(planned)*
```bash
mvl publish --sign      # signs with GitHub OIDC (ephemeral keys)
mvl verify <pkg>@<tag>  # checks GitHub Attestations transparency log
```
No key management; ephemeral keys tied to OIDC identity. Compatible with
`gh attestation verify` and Sigstore Rekor.

**Phase C: CVE Audit via OSV.dev** *(planned)*
```bash
mvl audit                       # checks against OSV.dev batch API
mvl audit --fail-on-critical    # CI gate
```

**Phase D: SLSA Provenance Workflow** *(planned)*
```bash
mvl publish --generate-ci   # emit GitHub Actions workflow for SLSA 3
```
MVL achieves SLSA 3 nearly for free: no macros, no build scripts, no conditional
compilation. Same source + same compiler = reproducible output.

### 9. Assurance — Trust Score

`mvl audit` reports a trust score per package:

```
Dependency    Version  License  Extern lines  MVL lines  Trust score
pkg-http      0.5.0    MIT      312           1840       85.5%
pkg-sqlite    0.2.1    Apache   89            430        82.8%
```

**Trust score** = MVL-verified lines / (MVL lines + extern lines).

`extern-rationale` is printed for any package with trust score < 100%, so
consumers can evaluate whether the justification is acceptable. `mvl audit`
informs; it does not block the build.

---

## Consequences

**Positive:**
- One ADR captures the full package management story: format, identity,
  distribution, lock file, transitive resolution, and supply chain.
- `mvl.lock` contains the full transitive closure — `mvl install` on a clean
  machine produces exactly the same package set.
- No registry infrastructure to operate; trust root is GitHub HTTPS.
- Diamond dependencies, arbitrary dep-tree depth, and offline installs all work.

**Negative / Limitations:**
- Version conflict resolution (MVS) is not yet implemented; last-resolved wins.
- Local path overrides in `.mvl/pkg/` bypass the lock file and are not
  reflected in the SBOM (#252 follow-up).
- Phase B–D supply chain features (signing, CVE audit, SLSA) are deferred.

---

## Rejected Alternatives

**Central Registry (crates.io / npm style):** Requires infrastructure,
governance, and a trust root we don't control. Premature for a language still
evolving. Deferred to a future phase; short names remain reserved.

**Require downstream projects to declare all transitives:** Error-prone, verbose,
leaks implementation details of library packages into consumers. Early Go modules
showed this approach does not scale.

**Resolve transitives in `mvl install`:** `install` is deterministic and
offline-capable. Adding network resolution there would break offline use and
blur the roles of `install` vs `update`.

**Lazy resolution at build time:** Detecting missing transitive deps during
`load_pkg_modules` and auto-fetching couples the compiler to the network, adds
build latency, and makes builds non-reproducible.

**Minimum Version Selection (MVS) for conflict resolution:** MVS (Go modules'
approach) is the planned algorithm for version conflict detection. Deferred
because it requires knowing the full constraint set before resolution, which
adds complexity without immediate benefit while the ecosystem is small.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

No compiler-verified requirement is directly affected. Package management is
build tooling, not language semantics. All eleven requirements are **left unchanged**.

**Indirect strengthening of Req 6 (Supply Chain):** Hash-locked `mvl.lock`,
`extern-rationale` fields, SBOM generation, and the four-phase security roadmap
all strengthen supply chain auditability.

### Design Principles (README)

- **Explicit over implicit** — **strengthens**: full git URLs (no short names);
  transitive deps appear explicitly in `mvl.lock`; nothing is resolved silently
  at build time.
- **Reproducible builds** — **strengthens**: full transitive closure in `mvl.lock`
  means `mvl install` on any machine produces the same package set.
- **The signature is the threat model** — **consistent with**: `mvl.lock` hashes
  are an explicit, auditable record of every dependency's identity.
- **No hidden behavior** — **consistent with**: `mvl install` verifies hashes and
  fails loudly on mismatch; `mvl update` prints every resolved transitive dep.
- **Batteries included** — **strengthens**: `mvl add`, `mvl install`, `mvl update`,
  `mvl sbom`, and `mvl audit` are first-class CLI commands; no external tooling.
- All other principles: **consistent with**.

### Specifications

- **Spec 008** (`.openspec/specs/008-packages/spec.md`) — covers the package
  format and resolution rules established here.
- **Spec 024** (`.openspec/specs/024-sbom/spec.md`) — covers SBOM generation.
- ADR-0009 (toolchain layout) and ADR-0006 (bridge.rs FFI) remain unchanged.
