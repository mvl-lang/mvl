
---

## Rejected Alternatives

### Central Registry (npm/crates.io style)

Rejected because it requires infrastructure we don't operate, a trust root we don't
control, and governance overhead that is premature for the project's current size.
The barrier to publishing (domain setup, account creation, review) adds friction
without adding security — git hosts already provide authenticated HTTPS and SSH.

### Embedded Proxy (Phase 5 from original #252 plan)

Deferred. A caching proxy is useful for air-gapped CI environments but is not
required for Phases A–D. It can be added later without changing the package identity
scheme or lock file format.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

This decision does not directly affect any of the eleven compiler-verified
requirements (Req 1–11). It concerns build tooling and distribution infrastructure,
not language semantics or type-system guarantees.

**Indirect strengthening of Req 6 (Supply Chain):** The SBOM command and lock-file
hash verification make it easier to audit and verify the provenance of every
dependency. Future phases (B: signing, C: CVE audit) will strengthen this further.

### Design Principles (README)

- **Explicit over implicit** — **consistent with**: package identity uses full git
  URLs, not short names that could resolve ambiguously.
- **The signature is the threat model** — **consistent with**: the lock file hash
  is an explicit, auditable record of every dependency's identity.
- **No hidden behavior** — **consistent with**: `mvl install` verifies hashes
  and fails loudly on mismatch; there is no silent fallback.
- **Batteries included** — **strengthens**: `mvl sbom`, `mvl add`, `mvl install`,
  `mvl init`, and `mvl self init` are all first-class CLI commands with no external
  tooling required.
- All other principles: **consistent with** — this ADR does not affect the type
  system, effect system, IFC labels, or verification passes.

### Specifications

- **Spec 024** (this PR) — new spec covering all package management and SBOM commands.
- **ADR-0009** (toolchain layout) — consistent; packages install under the existing
  XDG toolchain directory hierarchy.
- **ADR-0012** (extended package model) — consistent; this ADR refines the *distribution*
  story for the package model ADR-0012 already established.
