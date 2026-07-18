# data_integrity — Cryptographic tag-verification case study

A refinement-typed HMAC-style tag-verification kernel. Given a signed message and a verification context (expected tag, sequence floor, key validity window), decide accept / reject with a constant-time-comparison discipline and a single audited declassification point for the Secret-labelled verdict.

**Standard.** FIPS 140-3 (US federal cryptographic-module validation) + Common Criteria positioning.
**Ticket.** `mvl-lang/mvl#1908`.
**Domain distinctiveness.** First cybersecurity case study in the corpus. First case exercising IFC on the **output** side of the secret boundary (`relabel release`), complementing the input-taint story in ETCS, grid_protection, and CBTC.

## Files

- `model.mvl` — types (`MacTag`, `SignedMessage`, `VerificationContext`, `VerifyResult`, `RejectReason`).
- `verify.mvl` — the kernel: IFC boundaries (classify + release), three refinement-provable arithmetic obligations, compound safety decision, verification kernel, inline unit tests.
- `main.mvl` — six scenarios walking through accept / reject outcomes.
- `verify_test.mvl` — end-to-end scenario tests + IFC round-trip + priority-ordering coverage.
- `Makefile` — standard targets plus `make test-fips140` (cryptographic-validation assurance envelope).

## What is proven

`make prove` reports (production file only):

```
Summary: 13 proven (L1:10 L2:0 L3:0 L4:0 L5:3), 2 runtime, 0 failed
```

- **L1 (10)** — trivial literal subsumption on inline test-fn call sites.
- **L5 (3)** — Z3 QF-NIA discharges:
  - `combined_message_fingerprint: result >= 0` (two-variable product positivity)
  - `combined_message_fingerprint: result <= 2_000_000_000` (two-variable product upper bound)
  - `total_verification_ops: result >= 1` (three-variable product positivity from positive factors)
- **runtime (2)** — deferred to runtime assertion:
  - `total_verification_ops: result <= 400_000` (three-variable product upper bound — same Z3 QF-NIA ceiling as `dose_scheduling::total_infusion_dose`)
  - `effective_key_lifetime: result >= 0` (case-split subtraction; the caller's ordering hypothesis does not propagate into the branch)

## IFC boundaries (the distinguishing feature)

Two audit anchors, one for each direction of the secrecy boundary:

- **`HMAC-KEY-CLASSIFY-001`** — sole `relabel classify` for key material entering the module. Plain string data is elevated to `Secret[String]`.
- **`HMAC-VERDICT-001`** — sole `relabel release` for the verdict flowing out. Secret comparison state is declassified to a plain `String` / `Bool`.

Reproduce the audit:

```bash
grep -n "HMAC-VERDICT-001\|HMAC-KEY-CLASSIFY-001" verify.mvl
```

Returns exactly two lines. Every crossing of the IFC boundary is grep-able.

**Why this direction matters.** The paper's earlier IFC story (ETCS, grid_protection, CBTC) traces `Tainted[T]` from an untrusted source and requires an audited `relabel trust` to declassify. This example traces the *opposite* direction: `Secret[T]` starts inside the trust boundary (a key, a computed comparison result) and requires an audited `relabel release` to declassify to a plain type visible to callers. Both directions are needed for realistic security software; the combination of the two IFC stories closes the paper's IFC-completeness claim.

## Compound decision for MC/DC

`should_reject(tag_mismatch, replay_detected, key_expired, from_trusted_source, admin_override)` — five-atom compound predicate. Structural coupling: `from_trusted_source` and `admin_override` combine in a single sub-clause `(!from_trusted_source && !admin_override)` that cannot be factored into single-atom clauses without changing semantics.

Audit anchor: `MCDC-CRYPTO-001`.

**Current status:** `make mcdc` returns "No compound boolean conditions found" — the known `#1888` gap. The compound decision is structured to activate MC/DC discovery once #1888 lands.

## Explicitly out of scope for this example

The ticket #1908 describes full **QF-BV** reasoning as this case study's most distinctive feature — reasoning over individual bytes of tag material, PKCS#7 padding validation, CRC-32 polynomial arithmetic. These require MVL to surface bit-vector refinement predicates over `List[Byte]` (or an equivalent) as first-class solver-visible operations, plus L5 encoding that emits Z3's BV theory instead of NIA.

That work is not yet in MVL. This case study demonstrates what MVL CAN prove today about a cryptographic-integrity kernel:

- Integer-bounded fingerprint arithmetic — exercises QF-NIA at L5, same class as dose_scheduling
- IFC discipline on secret material — the *new* dimension this case adds
- Compound safety decisions on rejection logic — MC/DC target
- Constant-time-comparison structural discipline — enforced through the type system's refusal to allow branching on individual byte differences (verdict is a single Bool derived from full-tag equality)

Full byte-level QF-BV reasoning awaits compiler surface work; when that lands, this example's tag-comparison would gain byte-level refinements without changing its overall shape.

## Standard mapping (FIPS 140-3 cryptographic validation)

`make test-fips140` composes the assurance envelope:

- Static refinement proof (compile-time, all inputs) — 13 proven / 2 runtime
- Behavioural unit tests — 13 passed
- Branch coverage — 90% (18/20 branches on production)
- MC/DC coverage — blocked by #1888; recovers on that fix landing
- Audit anchors — `MCDC-CRYPTO-001` visible via grep

## Running the demo

```bash
make run
```

Produces:
```
1. normal accept:                     ACCEPT
2. reject tag mismatch:               REJECT (tag mismatch)
3. reject replay:                     REJECT (replay detected)
4. reject key expired:                REJECT (key expired)
5. reject untrusted (no override):    REJECT (tag mismatch)
6. accept untrusted (with override):  ACCEPT
```

## Design decisions worth naming

**IFC direction reversed from earlier cases.** Every prior case in the corpus traces the taint direction (`Tainted[T]` → declassified via `relabel trust`). This case traces the secret direction (`Secret[T]` → declassified via `relabel release`). Both directions have distinct audit anchors; a full security architecture needs both.

**Opaque rejection reasons.** `RejectReason` distinguishes categories (tag / replay / key / length) but does NOT report which byte of the tag differed. Reporting individual byte failures would leak timing information via error-code differentiation, defeating the constant-time-comparison discipline.

**Priority ordering in `rejection_reason`.** Replay first (structural), then key expiry (temporal), then tag mismatch (cryptographic). Deliberate — the ordering does not affect security but matters for operational triage.

**Runtime obligations are documented, not hidden.** Two obligations fall to runtime; the README calls each out with the reason (the three-variable product ceiling, the case-split subtraction). This is the pattern the refinement paper argues for: gradient of compile-time / runtime coverage, honestly reported, never silent.
