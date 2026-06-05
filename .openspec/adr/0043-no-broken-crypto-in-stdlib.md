# ADR-0043: No broken cryptographic primitives in stdlib

**Status:** Accepted
**Date:** 2026-06-05
**Issues:** #1279, #807

---

## Context

MVL's stdlib provides cryptographic primitives in `std/crypto.mvl`: SHA-256,
SHA-512, and CSPRNG (`crypto_random_bytes`). As the stdlib grows (UUID support
in #1279, distributed tracing in #807), the question arises whether to include
older hash functions like MD5 or SHA-1.

MD5 has been cryptographically broken since 2004 (collision attacks by Wang et
al.), and practically exploited since 2008 (Sotirov et al., rogue CA
certificate). SHA-1 was deprecated by NIST in 2011 and practically broken in
2017 (SHAttered, Stevens et al.). Both remain in widespread use for legacy
compatibility — checksums, cache keys, content addressing — but not for any
security purpose.

MVL is a safety-focused language. Its stdlib sets the default security posture
for all MVL programs. Including broken primitives in stdlib signals that they
are acceptable defaults, even if documented as "not for security."

---

## Decision

1. **The MVL stdlib SHALL NOT include cryptographic hash functions with known
   practical attacks.** This currently excludes MD5, SHA-1, and any function
   below 128-bit collision resistance.

2. **The stdlib provides only:** SHA-256, SHA-512, and CSPRNG. These are the
   minimum set for secure hashing and randomness. Additional secure algorithms
   (SHA-3, BLAKE3) may be added if use cases arise.

3. **Legacy/broken algorithms belong in `pkg/`.** If a user needs MD5 for
   protocol compatibility (e.g., HTTP Content-MD5 header, legacy API
   integration), they import a third-party package that makes the choice
   explicit and visible in their dependency list.

4. **UUID v4 generation uses CSPRNG, not a hash.** The `uuid_v4()` function
   (#1279) uses `crypto_random_bytes(16)` directly — no hash function involved.

---

## Consequences

**Positive:**
- Stdlib is secure by default. No MVL program accidentally uses a broken hash
  for security-sensitive operations.
- The `pkg/` boundary makes legacy crypto usage visible and auditable — it
  shows up in the dependency list and SBOM.
- Reduces stdlib surface area and maintenance burden.

**Negative:**
- Users porting code from languages with MD5 in stdlib (Python `hashlib`,
  Go `crypto/md5`, Java `MessageDigest`) must find or create a `pkg/` package.
- Non-security uses of MD5 (cache keys, content deduplication) are slightly
  less convenient — but SHA-256 works for all these cases with no downside
  except ~2× slower throughput, which is negligible for non-bulk operations.

**Follow-up:**
- If BLAKE3 demand emerges (faster than SHA-256 for large inputs), add it to
  `std/crypto.mvl` — it meets the collision resistance threshold.

---

## Rejected Alternatives

### Include MD5 with a deprecation warning

Rejected. A warning doesn't prevent use — developers suppress warnings. The
stdlib should not offer a footgun and hope developers read the label. If a
function is too dangerous to use by default, it shouldn't be a default import.

### Include MD5 behind an `! UnsafeCrypto` effect

Rejected. Overengineered. The effect system is for tracking side effects (IO,
network, randomness), not for gatekeeping algorithm quality. A bad hash
function is not a side effect — it's a wrong choice. The right boundary is
`std/` vs `pkg/`, not a new effect.

### Include all common hashes and let users choose

Rejected. "Let users choose" is the default in most languages, and it leads to
MD5 appearing in production security code because it was the first Google
result. MVL's value proposition is that the language makes the safe choice the
easy choice. Omitting broken primitives is part of that.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **REQ11 (Information Flow Control):** **Strengthens.** IFC prevents secrets
  from leaking to public sinks. Not including broken hashes prevents a
  different class of error: using a weak hash where a strong one is needed.
  Together they form defense in depth — IFC protects data flow, this ADR
  protects algorithm choice.
- All other requirements: unchanged.

### Design Principles (README)

- **Safe by default:** **Strengthens.** The stdlib only offers secure
  primitives. Unsafe alternatives require an explicit dependency.
- **Explicit over implicit:** **Consistent with.** Using MD5 requires an
  explicit `pkg/` import — the choice is visible in the dependency graph.
- **Correctness is non-negotiable:** **Strengthens.** A program using SHA-256
  for integrity checking is correct. A program using MD5 may not be.
- All other principles: consistent with.

### Specifications

No specs in `.openspec/specs/` are directly affected. If a crypto spec is
added in the future, it should reference this ADR for algorithm inclusion
criteria.
