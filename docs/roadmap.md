# MVL Roadmap

**Current version:** 0.5.5 (Phase 1 — Rust transpilation)
**Updated:** 2026-04-12

## Where we are

The parser, type checker, and Phase 1 transpiler are complete. All 11 requirements are enforced by the type checker. The transpiler produces working Rust binaries from `.mvl` source.

```
.mvl source
  → Lexer            ✓ complete
  → Parser (LL(1))   ✓ complete — full EBNF, error recovery, multi-error
  → Type Checker     ✓ 11/11 enforced
  → Transpiler       ✓ complete — .mvl → Rust source (v0.5.5, Epic 5)
  → cargo build      ✓ complete — `mvl build` / `mvl run`
  → native binary    ✓ complete — corpus programs run end-to-end
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
| 9 | [Data race freedom](requirements.md#req-9) | ✓ | ✓ partial (capabilities parsed, actor-boundary check: Phase 2) | — | Phase 2 |
| 10 | [Refinement types](requirements.md#req-10) | ✓ | ✓ enforced (static, runtime assert on call-site) | — | Phase 2 (SMT solver) |
| 11 | [IFC](requirements.md#req-11) | ✓ | ✓ enforced (lattice, declassify/sanitize) | — | Phase 2 (full flow analysis) |

## Phase 1 — Rust Transpilation

**Goal:** `.mvl` → Rust source → `cargo build` → native binary

**Done when:** Both reference examples (`auth_handler.mvl`, `safe_division.mvl`) compile to working binaries with all 11 requirements enforced.

### Critical path

| Step | What | Issues | Status |
|------|------|--------|--------|
| 1 | Transpile type declarations → Rust | [#29](https://github.com/LAB271/mvl_language/issues/29) | **Done** (v0.5.5) |
| 2 | Transpile functions → Rust | [#30](https://github.com/LAB271/mvl_language/issues/30) | **Done** (v0.5.5) |
| 3 | Core stdlib bridge (types map to Rust std) | [#42](https://github.com/LAB271/mvl_language/issues/42), [#43](https://github.com/LAB271/mvl_language/issues/43) | **Done** (v0.5.5, built-ins registered) |
| 4 | End-to-end: corpus compiles via rustc | [#33](https://github.com/LAB271/mvl_language/issues/33) | **Done** (v0.5.5) |
| 5 | Cargo integration (`mvl build`) | [#34](https://github.com/LAB271/mvl_language/issues/34) | **Done** (v0.5.5) |
| 6 | IFC → Rust newtypes | [#31](https://github.com/LAB271/mvl_language/issues/31) | **Done** (v0.5.5, Public/Secret/Tainted/Clean newtypes) |
| 7 | Refinements → Rust runtime asserts | [#32](https://github.com/LAB271/mvl_language/issues/32) | **Done** (v0.5.5, debug_assert! in constructors) |
| 8 | Module system | [#47](https://github.com/LAB271/mvl_language/issues/47) | Not started |
| 9 | Generics | [#48](https://github.com/LAB271/mvl_language/issues/48) | Not started |

### Supporting work

| What | Issues | Priority |
|------|--------|----------|
| **Rust FFI as stdlib** | [#91](https://github.com/LAB271/mvl_language/issues/91) | **Critical** — this IS the stdlib strategy |
| Extern blocks / FFI spec | [#52](https://github.com/LAB271/mvl_language/issues/52) | **Critical** — #91 depends on this |
| Unit test transpilation (`_test.mvl` → `#[test]`) | [#38](https://github.com/LAB271/mvl_language/issues/38) | High |
| Assurance gate in CI (--min threshold) | [#36](https://github.com/LAB271/mvl_language/issues/36) | Medium |
| ISPE report on PRs | [#76](https://github.com/LAB271/mvl_language/issues/76) | **Done** (v0.5.1) |
| Compiler-emitted assurance report | [#73](https://github.com/LAB271/mvl_language/issues/73) | Phase 1 late / Phase 2 |

### Stdlib strategy for Phase 1: Zero stdlib — Rust FFI is the standard library

Phase 1 ships with **no MVL stdlib**. The entire standard library is Rust's ecosystem, accessed through `extern "rust"` blocks. This makes the language immediately useful.

**How it works:**

- MVL code above the `extern` boundary: **11 requirements verified**
- Rust code below: **7/11** (Rust's native guarantees)
- The boundary is typed, effect-tracked, IFC-labeled, greppable, and counted in assurance reports

```mvl
// MVL code — verified (11/11)
fn handle_request(req: Tainted<Request>) -> Result<Response, AppError> ! Net, DB {
    let user = authenticate(sanitize(req.token))?;
    let data = pg_query(&db, "SELECT ...", [user.id])?;
    Ok(Response { body: data })
}

// Trust boundary — explicit, auditable
extern "rust" {
    fn pg_query(conn: &DbConn, sql: String, params: Array<SqlParam>) -> Result<Rows, DbError> ! DB
    fn authenticate(token: Clean<Token>) -> Result<User, AuthError> ! Net
}
```

**Primitive type mapping** (built into the transpiler, not extern):

| MVL | Rust |
|-----|------|
| `Int` | `i64` |
| `String` | `String` |
| `Array<T>` | `Vec<T>` |
| `Map<K,V>` | `HashMap<K,V>` |
| `Option<T>` | `Option<T>` |
| `Result<T,E>` | `Result<T,E>` |
| `Public<T>` | Newtype `pub struct Public<T>(T)` |
| `Tainted<T>` | Newtype `pub struct Tainted<T>(T)` |
| `Secret<T>` | Newtype `pub struct Secret<T>(T)` |
| `Int where x > 0` | `debug_assert!(x > 0)` |

**Stdlib growth path:**

1. **Phase 2:** `extern "rust"` only. Cargo.toml pulls Rust crates. Zero MVL stdlib.
2. **Phase 4:** Verified MVL wrappers replace extern calls. Each wrapper moves code from "trusted" to "verified."
3. **The assurance ratio** (verified / total) becomes the metric: start at 60% MVL + 40% extern, push toward 90%.

## Phase 2 — It's useful

**Goal:** Real programs in MVL, calling Rust ecosystem via FFI.

**Done when:** A non-trivial program (e.g., a CLI tool or web handler) runs in production using MVL + Rust crates.

| Component | Issues | Description |
|-----------|--------|-------------|
| Rust FFI | [#91](https://github.com/LAB271/mvl_language/issues/91), [#52](https://github.com/LAB271/mvl_language/issues/52) | `extern "rust"` blocks — typed, effect-tracked, IFC-labeled trust boundary |
| Module system | [#47](https://github.com/LAB271/mvl_language/issues/47) | Multi-file programs with `module` and `use` |
| Generics | [#48](https://github.com/LAB271/mvl_language/issues/48) | `Array<T>`, `Option<T>`, `Result<T,E>` emit correctly |
| Test transpilation | [#38](https://github.com/LAB271/mvl_language/issues/38) | `_test.mvl` → Rust `#[test]` |
| Assurance reports | [#73](https://github.com/LAB271/mvl_language/issues/73) | Compiler tracks verified vs trusted (extern) ratio |
| Property testing | [#40](https://github.com/LAB271/mvl_language/issues/40) | Refinement types as generators |
| BDD framework | [#39](https://github.com/LAB271/mvl_language/issues/39) | Scenario tests linked to specs |

## Phase 3 — It's trustworthy

**Goal:** All 11 requirements proven at compile time. One compiler, one trust chain.

**Done when:** All 11 enforced without Rust as intermediary. WASM target works.

| Component | Description |
|-----------|-------------|
| LLVM IR codegen | Replace Rust transpiler with direct LLVM codegen |
| SMT integration | [Req 10](requirements.md#req-10) moves from runtime asserts to compile-time proofs (Z3) |
| Native IFC | [Req 11](requirements.md#req-11) flow analysis in the compiler, not via Rust newtypes |
| Borrow lifetimes | Full [Req 2](requirements.md#req-2) enforcement (beyond use-after-move) |
| Linear resources | Full [Req 6](requirements.md#req-6) enforcement (must-consume semantics) |
| Structural recursion | Full [Req 8](requirements.md#req-8) proof (not just while-rejection) |
| Model checker | [#37](https://github.com/LAB271/mvl_language/issues/37) — invariants, pre/post, deadlock detection |
| WASM target | Sandboxed execution for The Cog and edge |

## Phase 4 — It's self-sufficient

**Goal:** MVL compiler compiles itself. Full ecosystem. Certification-ready.

**Done when:** Self-hosting complete. Package ecosystem functional. AAE-5 evidence generated.

| Component | Description |
|-----------|-------------|
| Self-hosting | MVL compiler rewritten in MVL, compiled by Phase 3 compiler |
| Package manager | [#56](https://github.com/LAB271/mvl_language/issues/56) — dependency resolution, SBOM, trust scoring |
| Verified MVL stdlib | Replaces extern wrappers — pushes assurance ratio toward 90%+ |
| Concurrency model | Actors, reference capabilities, WCET refinements |
| Transpilation corpus | Seed for LLM training on MVL generation quality |
| AAE-5 pipeline | Automated evidence for IEC 61508, DO-178C certification |

## The four phases

```
Phase 1: It compiles          parse → check → transpile → cargo → binary
Phase 2: It's useful          FFI, modules, generics, tests, real programs
Phase 3: It's trustworthy     LLVM, SMT, all 11 proven, WASM
Phase 4: It's self-sufficient  self-hosting, packages, stdlib, certification
```

## Architecture decisions

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](adr/0001-eleven-requirements.md) | Eleven compiler-verified requirements | Accepted |
| [ADR-0002](adr/0002-language-contraction.md) | Language contraction — what to drop and why | Accepted |
| [ADR-0003](adr/0003-compilation-strategy.md) | Four phases: Rust → FFI → LLVM → Self-hosting | Accepted |
| [ADR-0004](adr/0004-language-size.md) | Language size — deliberately the smallest | Accepted |
| [ADR-0005](adr/0005-recursive-descent-parser.md) | Hand-written recursive descent parser (LL(1)) | Accepted |

## Design principles

1. **Verification density:** Every feature exists to increase properties proven per token
2. **Contraction:** Remove features that resist verification. The language shrinks by policy.
3. **One way:** One way to branch, one way to loop, one way to handle errors
4. **Stdlib grows, language doesn't:** New functionality via library, not language extensions
5. **Zero dependencies:** The compiler is a single binary. No external crates.
