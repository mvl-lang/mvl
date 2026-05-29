# ADR-0039: Repository-less Package Distribution and Supply Chain Security

**Status:** Accepted
**Date:** 2026-05-29
**Issues:** #252, #56, #1120, #1129, #1131, #1135
**Related:** ADR-0009 (toolchain layout), ADR-0012 (extended package model)

---

## Context

MVL needs a package distribution model and supply chain security story.
The initial issue (#252) proposed Sigstore/SLSA integration. Before implementing
signing infrastructure, we need to decide *what is the registry* and *what is the
distribution unit*.

Three options were considered:

1. **Central registry** (like crates.io, npm) — requires infrastructure, governance,
   availability guarantees, and a trust root we don't control.

2. **Repository-less / Git-native** (like Go modules) — GitHub/GitLab are the
   registry. Every valid `https://github.com/<user>/<repo>` is a resolvable package.
   Versions are git tags. Hashes are SHA-256 of the archive. The trust root is the
   git host (HTTPS + SSH).

3. **Hybrid** (GitHub + mirroring proxy) — repository-less for development,
   optional proxy for CI environments and compliance regimes.

## Decision

**Use a repository-less, Git-native distribution model** (option 2), with a
structured four-phase security roadmap to add verifiable supply chain guarantees.

### Package Identity

A package is identified by its git URL and optional tag:

```toml
[dependencies]
sqlite = { git = "https://github.com/lab271/mvl_sqlite", tag = "v0.3.0" }
http   = { git = "https://github.com/lab271/mvl_http",   tag = "v1.2.0" }
```

Short form via `mvl add`:
```bash
mvl add github.com/lab271/mvl_sqlite v0.3.0
# expands to: git = "https://github.com/lab271/mvl_sqlite", tag = "v0.3.0"
```

### Lock File (`mvl.lock`)

Every resolved dependency is recorded with its exact git URL, tag, commit SHA,
and SHA-256 archive hash:

```toml
[[package]]
name    = "sqlite"
version = "0.3.0"
git     = "https://github.com/lab271/mvl_sqlite"
tag     = "v0.3.0"
commit  = "abc1234567890abcdef..."
hash    = "sha256:e3b0c44298fc1c149afb..."
```

The lock file is the integrity anchor. `mvl install` verifies hashes before
extracting packages. Any hash mismatch is a hard error.

### Toolchain Layout

Packages are installed to the XDG cache directory under the toolchain version:
```
$XDG_DATA_HOME/mvl/toolchains/<version>/pkgs/<name>@<version>/
```

Local development overrides (for monorepo use) are supported via `.mvl/pkg/<name>/`
symlinks or directories. This bypasses the lock file and is not reflected in the SBOM.
A future improvement will integrate local overrides into the SBOM (#252 follow-up).

---

## Four-Phase Security Roadmap

### Phase A: SBOM Generation (implemented — PR #1120, #1135)

`mvl sbom` generates a software bill of materials from `mvl.toml` + `mvl.lock`.
No external services. No new dependencies.

Formats:
- **CycloneDX 1.5 JSON** (default) — machine-readable, CI-friendly
- **SPDX 2.3 tag-value** — compliance toolchain format

Component type detection: the root component is tagged `application` when
`main.mvl` or `src/main.mvl` exists in the project root; otherwise `library`.

### Phase B: Package Signing via GitHub Attestations (planned)

```bash
mvl publish --sign        # signs with GitHub OIDC (ephemeral keys)
mvl verify <pkg>@<tag>    # checks GitHub Attestations transparency log
```

No key management — ephemeral keys tied to OIDC identity.
Compatible with `gh attestation verify` and Sigstore Rekor.

### Phase C: CVE Audit via OSV.dev (planned)

```bash
mvl audit     # checks dependencies against OSV.dev (batch API, no key)
```

Offline mode: compare lock-file hashes against locally cached advisory DB.
CI gate: `mvl audit --fail-on-critical` exits non-zero on known CVEs.

### Phase D: SLSA Provenance Workflow (planned)

```bash
mvl publish --generate-ci    # emit GitHub Actions workflow for SLSA 3
```

MVL achieves SLSA 3 nearly for free: no macros, no build scripts, no conditional
compilation. Same source + same compiler = reproducible output. The generated
workflow records provenance attestations automatically.

---

## Component Type Detection

The SBOM root component type reflects the project's intended use:

| Condition | CycloneDX type | SPDX purpose |
|-----------|---------------|--------------|
| `main.mvl` or `src/main.mvl` exists | `application` | `APPLICATION` |
| neither exists | `library` | `LIBRARY` |

This allows SBOM consumers (e.g. CI scanners, compliance dashboards) to apply
appropriate policies per component type.

---

## Project Scaffolding (`mvl init`)

`mvl init [<name>]` creates a minimal project skeleton in the current directory:

```
mvl.toml          [package] with name, version, license, requires-mvl
src/main.mvl      entry-point fn main() -> Unit ! Console
```

This is distinct from `mvl self init`, which extracts the bundled stdlib to the
toolchain directory. The previous overloading of `mvl init` for both purposes
was resolved by PR #1131.

---

## Consequences

**Positive:**
- No registry infrastructure to operate
- Trust root is GitHub HTTPS (already trusted by most organizations)
- `mvl.lock` provides deterministic, reproducible builds
- CycloneDX / SPDX SBOM is a standards-based deliverable for compliance
- Local path overrides support monorepo development without a proxy

**Negative / mitigations:**
- No centralized name squatting protection — mitigated by full URLs (no short names)
- Git host availability affects `mvl install` — mitigated by lock file + local cache
- Local path overrides bypass SBOM — tracked for future fix (#252)

**Deferred:**
- Phase B (signing), Phase C (audit), Phase D (SLSA) — each is a standalone feature
- Proxy / mirror support for air-gapped environments — not needed for Phases A–D
