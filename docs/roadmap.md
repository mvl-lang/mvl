# MVL Roadmap

**Current version:** 0.4.0 (Phase 1 — Rust transpilation)
**Updated:** 2026-04-12

## Where we are

The parser and type checker are complete. All 11 requirements are represented in the grammar. 9/11 have active enforcement in the type checker. The transpiler is next.

```
.mvl source
  → Lexer            ✓ complete
  → Parser (LL(1))   ✓ complete — full EBNF, error recovery, multi-error
  → Type Checker     ✓ 9/11 enforced — Req 10+11 parse-only
  → Transpiler       ✗ not started — Epic 5 (#28)
  → cargo build      ✗ blocked on transpiler
  → native binary    ✗ blocked on cargo
```

## Requirement enforcement status

| # | Requirement | Parse | Check | Transpile | Target |
|---|------------|-------|-------|-----------|--------|
| 1 | [Type safety](requirements.md#req-1) | ✓ | ✓ enforced | — | Phase 1 |
| 2 | [Memory safety](requirements.md#req-2) | ✓ | ✓ partial (use-after-move) | — | Phase 1 (borrow lifetimes: Phase 2) |
| 3 | [Totality](requirements.md#req-3) | ✓ | ✓ enforced | — | Phase 1 |
| 4 | [Null elimination](requirements.md#req-4) | ✓ | ✓ enforced | — | Phase 1 |
| 5 | [Error visibility](requirements.md#req-5) | ✓ | ✓ enforced | — | Phase 1 |
| 6 | [Ownership](requirements.md#req-6) | ✓ | ✓ partial (move tracking) | — | Phase 1 (linear resources: Phase 2) |
| 7 | [Effect tracking](requirements.md#req-7) | ✓ | ✓ enforced | — | Phase 1 |
| 8 | [Termination](requirements.md#req-8) | ✓ | ✓ partial (while rejected) | — | Phase 1 (structural recursion proof: Phase 2) |
| 9 | [Data race freedom](requirements.md#req-9) | ✓ | ✓ enforced (capabilities) | — | Phase 1 |
| 10 | [Refinement types](requirements.md#req-10) | ✓ | ○ parse-only | — | Phase 1 as runtime asserts (#32) |
| 11 | [IFC](requirements.md#req-11) | ✓ | ○ parse-only | — | Phase 1 as Rust newtypes (#31) |

## Phase 1 — Rust Transpilation

**Goal:** `.mvl` → Rust source → `cargo build` → native binary

**Done when:** Both reference examples (`auth_handler.mvl`, `safe_division.mvl`) compile to working binaries with all 11 requirements enforced.

### Critical path

| Step | What | Issues | Status |
|------|------|--------|--------|
| 1 | Transpile type declarations → Rust | [#29](https://github.com/LAB271/mvl_language/issues/29) | Not started |
| 2 | Transpile functions → Rust | [#30](https://github.com/LAB271/mvl_language/issues/30) | Not started |
| 3 | Core stdlib bridge (types map to Rust std) | [#42](https://github.com/LAB271/mvl_language/issues/42), [#43](https://github.com/LAB271/mvl_language/issues/43) | Not started |
| 4 | End-to-end: corpus compiles via rustc | [#33](https://github.com/LAB271/mvl_language/issues/33) | Not started |
| 5 | Cargo integration (`mvl build`) | [#34](https://github.com/LAB271/mvl_language/issues/34) | Not started |
| 6 | IFC → Rust newtypes | [#31](https://github.com/LAB271/mvl_language/issues/31) | Not started |
| 7 | Refinements → Rust runtime asserts | [#32](https://github.com/LAB271/mvl_language/issues/32) | Not started |
| 8 | Module system | [#47](https://github.com/LAB271/mvl_language/issues/47) | Not started |
| 9 | Generics | [#48](https://github.com/LAB271/mvl_language/issues/48) | Not started |

### Supporting work

| What | Issues | Priority |
|------|--------|----------|
| Extern blocks / FFI | [#52](https://github.com/LAB271/mvl_language/issues/52) | High (stdlib bridge needs it) |
| Unit test transpilation (`_test.mvl` → `#[test]`) | [#38](https://github.com/LAB271/mvl_language/issues/38) | High |
| Assurance gate in CI | [#36](https://github.com/LAB271/mvl_language/issues/36) | Medium |
| ISPE report on PRs | [#76](https://github.com/LAB271/mvl_language/issues/76) | Medium |
| Compiler-emitted assurance report | [#73](https://github.com/LAB271/mvl_language/issues/73) | Phase 1 late / Phase 2 |

### Stdlib strategy for Phase 1

The transpiler maps MVL stdlib to Rust std. No custom Rust runtime library.

| MVL | Maps to Rust |
|-----|-------------|
| `Int` | `i64` (or `num::BigInt` for arbitrary precision) |
| `String` | `String` |
| `Array<T>` | `Vec<T>` |
| `Map<K,V>` | `std::collections::HashMap<K,V>` |
| `Set<T>` | `std::collections::HashSet<T>` |
| `Option<T>` | `Option<T>` |
| `Result<T,E>` | `Result<T,E>` |
| `println()` | `println!()` |
| `Public<T>` | Newtype `pub struct Public<T>(T)` |
| `Tainted<T>` | Newtype `pub struct Tainted<T>(T)` |
| `Secret<T>` | Newtype `pub struct Secret<T>(T)` |
| `Int where x > 0` | `assert!(x > 0)` at call site |

This is intentionally simple. Phase 2 replaces it with native codegen.

## Phase 2 — LLVM Backend

**Goal:** One compiler, one proof chain. MVL → LLVM IR → native binary.

**Done when:** The MVL compiler compiles itself (self-hosting).

| Component | Description |
|-----------|-------------|
| LLVM IR codegen | Replace Rust transpiler with LLVM IR emitter |
| SMT integration | Req 10 moves from runtime asserts to compile-time proofs (Z3) |
| Native IFC | Req 11 flow analysis in the compiler, not via Rust newtypes |
| Borrow lifetimes | Full Req 2 enforcement (beyond use-after-move) |
| Linear resources | Full Req 6 enforcement (must-consume semantics) |
| Structural recursion | Full Req 8 proof (not just while-rejection) |
| Model checker | [#37](https://github.com/LAB271/mvl_language/issues/37) — invariants, pre/post, deadlock detection |
| WASM target | Sandboxed execution for The Cog and edge |
| Self-hosting | MVL compiler rewritten in MVL, compiled by itself |

## Phase 3 — Ecosystem

**Goal:** MVL is usable for real projects with full tooling.

| Component | Description |
|-----------|-------------|
| Package manager | [#56](https://github.com/LAB271/mvl_language/issues/56) — dependency resolution, SBOM, trust scoring |
| Extended stdlib | Networking, HTTP, TLS, crypto, database drivers |
| Property testing | [#40](https://github.com/LAB271/mvl_language/issues/40) — refinement types as generators |
| BDD framework | [#39](https://github.com/LAB271/mvl_language/issues/39) — scenario tests linked to specs |
| Assurance reports | [#73](https://github.com/LAB271/mvl_language/issues/73) — per-module requirement satisfaction |
| Transpilation corpus | Seed for LLM training on MVL code generation |
| AAE integration | Automated evidence for ISO 42001 / DO-178C certification |

## Architecture decisions

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](adr/0001-eleven-requirements.md) | Eleven compiler-verified requirements | Accepted |
| [ADR-0002](adr/0002-language-contraction.md) | Language contraction — what to drop and why | Accepted |
| [ADR-0003](adr/0003-compilation-strategy.md) | Compilation: Rust (Phase 1) → LLVM (Phase 2) | Accepted |
| [ADR-0004](adr/0004-language-size.md) | Language size — deliberately the smallest | Accepted |
| [ADR-0005](adr/0005-recursive-descent-parser.md) | Hand-written recursive descent parser (LL(1)) | Accepted |

## Design principles

1. **Verification density:** Every feature exists to increase properties proven per token
2. **Contraction:** Remove features that resist verification. The language shrinks by policy.
3. **One way:** One way to branch, one way to loop, one way to handle errors
4. **Stdlib grows, language doesn't:** New functionality via library, not language extensions
5. **Zero dependencies:** The compiler is a single binary. No external crates.
