# ADR-0008: Compilation Units and Linking — Source-First, Verified Binary Later

**Status:** Accepted
**Date:** 2026-04-14
**Context:** MVL has no concept of precompiled binary libraries or linking. Phase 1-2 delegates this entirely to Cargo (Rust transpilation). Phase 3 (LLVM backend) requires a native compilation and linking model.

## Decision

**Phase 3: Source-only compilation (Go model).** All dependencies compile from source. The LLVM backend sees everything. No binary distribution, no ABI stability concerns. Package manager distributes source.

**Phase 4: Precompiled modules with verified metadata (Lean 4 / OCaml model).** Compile to `.mvlo` (MVL object) containing LLVM bitcode + verified type/effect/IFC signatures. The metadata IS the proof — consumers don't need source to verify the module satisfies the 11 requirements. Linking combines verified objects.

## Compilation Units

### Phase 3 — Source compilation

```
foo.mvl                    → parse → check (11 req) → LLVM IR → .o
bar.mvl  (uses foo)        → parse → check (11 req) → LLVM IR → .o
                                                                  ↓
                                                              link → binary
```

Every `.mvl` file is a compilation unit. The compiler sees all source, checks all requirements, emits LLVM IR per file, links to a single binary. No separate compilation — the compiler needs the full program for IFC analysis (Req 11) and termination checking (Req 8).

**Trade-off:** Slow for large programs. Acceptable for Phase 3 — correctness over speed.

### Phase 4 — Precompiled modules (.mvlo)

```
lib.mvl  → compile → lib.mvlo  (LLVM bitcode + verified metadata)
                         │
                         ├── types: full type signatures
                         ├── effects: effect annotations per public fn
                         ├── ifc: security labels on all public types
                         ├── refinements: value constraints on public API
                         ├── termination: total/partial per public fn
                         ├── trust: requirement score (11/11, 9/11, etc.)
                         └── bitcode: LLVM IR (opaque, optimized)
```

A `.mvlo` file is a verified artifact. It carries:

| Section | Content | Used for |
|---|---|---|
| **Signature** | Public type signatures, generics, trait impls | Type checking against consumers |
| **Effects** | Effect annotations per public function | Effect checking at call sites |
| **IFC labels** | Security labels on all public types and parameters | Information flow verification |
| **Refinements** | Value constraints on public API boundaries | Refinement type checking at call sites |
| **Totality** | `total` / `partial` per public function | Termination checking in callers |
| **Trust score** | Requirement satisfaction: 11/11, contains extern, etc. | `mvl audit`, SBOM generation |
| **LLVM bitcode** | Optimized IR, ready to link | Code generation |
| **Source hash** | SHA-256 of source that produced this artifact | Reproducibility, SBOM integrity |

### Linking model

```
app.mvl          → compile with lib.mvlo metadata → check all 11 req → app.o
                                                                          │
lib.mvlo (bitcode) ───────────────────────────────────────────────────────┤
extern_dep.rlib (Rust) ──────────────────────────────────────────────────┤
                                                                          ↓
                                                                      link → binary
```

The compiler uses `.mvlo` metadata for verification but `.mvlo` bitcode for linking. It never needs the source of a precompiled dependency. Requirements are verified at the boundary — the proof travels with the artifact.

### Trust boundary at link time

Three categories of linked objects:

| Object type | Verified | Trust level |
|---|---|---|
| `.mvlo` (MVL precompiled) | 11/11 by MVL compiler | Full — proof in metadata |
| `.mvlo` with `extern` | Partial — extern blocks are unverified | Degraded — trust score reflects extern ratio |
| `.rlib` / `.a` / `.so` (foreign) | Not verified by MVL | Untrusted — effects and IFC enforced at FFI boundary only |

`mvl audit` reports the trust composition of the final binary:
```
Trust report for app:
  MVL verified:     12,400 lines (78%)  — 11/11
  MVL with extern:   2,100 lines (13%)  — 9/11 avg (extern blocks: 340 lines)
  Foreign (Rust):    1,500 lines  (9%)  — unverified, FFI boundary enforced

  Overall trust score: 9.2/11
  CVE exposure: 3 advisories in foreign deps (R2, R10)
```

## Why not binary-first from Phase 3?

1. **IFC requires whole-program analysis.** Information flow labels propagate across module boundaries. With source, the compiler traces flow globally. With binaries, it must trust the metadata — correct, but Phase 3 should prove the analysis works on source first.
2. **Simpler bootstrap.** Phase 3 is already a major effort (LLVM backend). Adding a binary format and linker simultaneously doubles the risk.
3. **Go proved source-only scales.** Go compiles 100K+ LOC projects in seconds from source. MVL programs will be smaller (the language is smaller). Source-only is viable until the ecosystem demands otherwise.

## Why add binary in Phase 4?

1. **Package distribution.** Source distribution exposes implementation. Some packages (commercial, security-sensitive) need binary distribution with verified interfaces.
2. **Build speed.** As the ecosystem grows, recompiling all transitive dependencies from source becomes slow. Precompiled modules with verified metadata are the cache.
3. **SBOM by construction.** Each `.mvlo` carries its trust score and source hash. The SBOM is the set of metadata from all linked objects — generated automatically, not scanned after the fact.
4. **Self-hosting prerequisite.** The MVL compiler (Phase 4) must be distributable as a precompiled artifact. It should eat its own dog food.

## Alternatives considered

| Approach | Rejected because |
|---|---|
| **C model (header + object)** | Separate interface files (.mvli) are a maintenance burden. Metadata-in-object is better — one artifact, not two. |
| **Rust model (rlib + rmeta)** | Tightly coupled to Cargo. MVL needs its own format that carries IFC and effect proofs — Rust's rmeta doesn't have these. |
| **JVM model (bytecode + classfiles)** | JVM bytecode has no ownership, no effects, no IFC. Wrong abstraction level. |
| **No binary libraries ever** | Viable but limits ecosystem. Commercial packages, build speed, and self-hosting all need it eventually. |

## File format (Phase 4, sketch)

```
.mvlo file layout:
  [magic: "MVL\x00"]
  [version: u32]
  [metadata section]
    - module name
    - public signatures (types, effects, IFC, refinements, totality)
    - trust score
    - source hash (SHA-256)
    - dependency list (other .mvlo / extern)
  [bitcode section]
    - LLVM bitcode (possibly compressed)
```

Details deferred to Phase 4 design. The key invariant: **metadata is sufficient for full requirement verification without source or bitcode.**

## Consequences

- Phase 3 is simpler — source-only, no binary format to design
- Phase 4 gets a clean binary format designed around the 11 requirements
- `.mvlo` metadata enables `mvl audit` to report trust composition without source access
- SBOM generation is a byproduct of the compilation model, not a separate tool
- The proof travels with the artifact — consumers verify at link time, not at trust-me time

## Connected to

- ADR-0003: Compilation strategy (phases)
- ADR-0006: FFI extern "rust" bridge (trust boundary)
- ADR-0007: Stdlib import model (tiers map to trust levels)
- Phase 4 (#130): Verified standard library
- #56: Supply chain safety / SBOM
- #57: Package model
- #151: CVE-aware dependency auditing
