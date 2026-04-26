# MVL Roadmap

**Current version:** 0.10.0 (Phase 2 — It's useful)
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
| 4 | End-to-end: corpus compiles via rustc | [#33](https://github.com/LAB271/mvl_language/issues/33) | **Done** (v0.5.6, all 7 full programs build) |
| 5 | Cargo integration (`mvl build`) | [#34](https://github.com/LAB271/mvl_language/issues/34) | **Done** (v0.5.5) |
| 6 | IFC → Rust newtypes | [#31](https://github.com/LAB271/mvl_language/issues/31) | **Done** (v0.5.6, Copy/Display/arithmetic + external type stubs) |
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
fn handle_request(req: Tainted[Request]) -> Result[Response, AppError] ! Net, DB {
    let user = authenticate(sanitize(req.token))?;
    let data = pg_query(&db, "SELECT ...", [user.id])?;
    Ok(Response { body: data })
}

// Trust boundary — explicit, auditable
extern "rust" {
    fn pg_query(conn: &DbConn, sql: String, params: Array[SqlParam]) -> Result[Rows, DbError] ! DB
    fn authenticate(token: Clean[Token]) -> Result[User, AuthError] ! Net
}
```

**Primitive type mapping** (built into the transpiler, not extern):

| MVL | Rust |
|-----|------|
| `Int` | `i64` |
| `String` | `String` |
| `Array[T]` | `Vec<T>` |
| `Map[K,V]` | `HashMap<K,V>` |
| `Option[T]` | `Option<T>` |
| `Result[T,E]` | `Result<T,E>` |
| `Public[T]` | Newtype `pub struct Public<T>(T)` |
| `Tainted[T]` | Newtype `pub struct Tainted<T>(T)` |
| `Secret[T]` | Newtype `pub struct Secret<T>(T)` |
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
| Generics | [#48](https://github.com/LAB271/mvl_language/issues/48) | `Array[T]`, `Option[T]`, `Result[T,E]` emit correctly |
| Test transpilation | [#38](https://github.com/LAB271/mvl_language/issues/38) | `_test.mvl` → Rust `#[test]` |
| Assurance reports | [#73](https://github.com/LAB271/mvl_language/issues/73) | Compiler tracks verified vs trusted (extern) ratio |

## Phase 3 — It's trustworthy ✅

**Goal:** 10/11 requirements proven at compile time via verification passes on the AST.

**Done when:** All provers run, linter complete. Epic [#129](https://github.com/LAB271/mvl_language/issues/129) closed.

| Component | Issues | Status |
|-----------|--------|--------|
| Prover infrastructure | [#139](https://github.com/LAB271/mvl_language/issues/139) | ✅ Done |
| Termination checker (Req 8) | [#135](https://github.com/LAB271/mvl_language/issues/135) | ✅ Done |
| Data race freedom (Req 9 partial) | [#138](https://github.com/LAB271/mvl_language/issues/138) | ✅ Done |
| IFC verifier (Req 11) | [#137](https://github.com/LAB271/mvl_language/issues/137) | ✅ Done |
| Refinement type solver (Req 10) | [#136](https://github.com/LAB271/mvl_language/issues/136) | ✅ Done |
| Linter (style + semantic + LLM) | [#127](https://github.com/LAB271/mvl_language/issues/127), [#132](https://github.com/LAB271/mvl_language/issues/132), [#133](https://github.com/LAB271/mvl_language/issues/133) | ✅ Done |
| Complexity analysis | [#208](https://github.com/LAB271/mvl_language/issues/208) | ✅ Done |

## Phase 4 — It's practical 🔴

**Goal:** Verified standard library. Real programs without FFI. Toolchain maturity.

**Done when:** Stdlib modules have real bodies (generated from specs, verified by compiler). Trust Triangle KPIs measured. Epic [#130](https://github.com/LAB271/mvl_language/issues/130).

| Component | Issues | Status |
|-----------|--------|--------|
| Iterator trait + lazy ops | [#219](https://github.com/LAB271/mvl_language/issues/219) | Open |
| Stdlib extraction from bridge | [#217](https://github.com/LAB271/mvl_language/issues/217) | Open |
| Generics constraint enforcement | [#225](https://github.com/LAB271/mvl_language/issues/225) | Open |
| Toolchain versioning (A–D) | [#220](https://github.com/LAB271/mvl_language/issues/220)–[#223](https://github.com/LAB271/mvl_language/issues/223) | Open |
| File I/O stdlib | [#44](https://github.com/LAB271/mvl_language/issues/44) | Open |
| Process lifecycle stdlib | [#45](https://github.com/LAB271/mvl_language/issues/45) | Open |
| Arg parsing stdlib | [#55](https://github.com/LAB271/mvl_language/issues/55) | Open |
| BDD testing | [#39](https://github.com/LAB271/mvl_language/issues/39) | Open |
| Property-based testing | [#40](https://github.com/LAB271/mvl_language/issues/40) | Open |
| Package model + SBOM | [#56](https://github.com/LAB271/mvl_language/issues/56), [#57](https://github.com/LAB271/mvl_language/issues/57) | Open |
| CVE-aware auditing | [#151](https://github.com/LAB271/mvl_language/issues/151) | Open |
| Behavioral coverage | [#209](https://github.com/LAB271/mvl_language/issues/209) | Open |
| Mutation testing | [#210](https://github.com/LAB271/mvl_language/issues/210) | Open |

## Phase 5 — It's independent

**Goal:** LLVM backend. One compiler, one trust boundary. No Rust dependency.

**Done when:** MVL → LLVM IR → native binary. WASM target works. Epic [#131](https://github.com/LAB271/mvl_language/issues/131).

| Component | Description |
|-----------|-------------|
| LLVM IR codegen | Replace Rust transpiler with direct LLVM codegen |
| SMT integration | [Req 10](requirements.md#req-10) moves from runtime asserts to compile-time proofs (Z3) |
| Native IFC | [Req 11](requirements.md#req-11) flow analysis in the compiler, not via Rust newtypes |
| Borrow lifetimes | Full [Req 2](requirements.md#req-2) enforcement (beyond use-after-move) |
| Linear resources | Full [Req 6](requirements.md#req-6) enforcement (must-consume semantics) |
| Structural recursion | Full [Req 8](requirements.md#req-8) proof (not just while-rejection) |
| Model checker | [#37](https://github.com/LAB271/mvl_language/issues/37) — invariants, pre/post, deadlock detection |
| Property testing | [#40](https://github.com/LAB271/mvl_language/issues/40) — Refinement types as SMT-driven generators |
| WASM target | Sandboxed execution for The Cog and edge |

## Phase 6 — It's complete

**Goal:** Full 11/11 at compile time. Self-hosting. Certification-ready. Epic [#134](https://github.com/LAB271/mvl_language/issues/134).

**Done when:** Self-hosting complete. Actors work. AAE-5 evidence generated.

| Component | Issues | Description |
|-----------|--------|-------------|
| Actor syntax | [#63](https://github.com/LAB271/mvl_language/issues/63) | Behaviors, messages, lifecycle |
| Structured concurrency | [#69](https://github.com/LAB271/mvl_language/issues/69) | Select, timeout, cancellation |
| Model checker | [#37](https://github.com/LAB271/mvl_language/issues/37) | Pre/post conditions, invariants, WCET |
| Self-hosting | [#187](https://github.com/LAB271/mvl_language/issues/187) | MVL compiler in MVL |
| Package manager | [#56](https://github.com/LAB271/mvl_language/issues/56) | Dependency resolution, SBOM, trust scoring |
| Verified MVL stdlib | — | Replaces extern wrappers — assurance ratio toward 90%+ |
| Transpilation corpus | — | Seed for LLM training on MVL generation quality |
| BDD framework | [#39](https://github.com/LAB271/mvl_language/issues/39) | Scenario tests linked to specs |
| AAE-5 pipeline | — | IEC 61508, DO-178C certification evidence |

## The six phases

```
Phase 1: It compiles        MVL → Rust transpilation                          ✅ Done
                             Parse, check, transpile, cargo build, binary.
                             9/11 at compile time, 2 via Rust runtime.

Phase 2: It's useful         Rust FFI ecosystem, zero stdlib                  ✅ Done
                             extern "rust" blocks. Modules, generics, tests.
                             Rust's crates.io IS the standard library.

Phase 3: It's trustworthy    Core provers (10/11 at compile time)             ✅ Done
                             Termination, data race, IFC, refinement provers.
                             Linter (style + semantic + LLM corpus quality).

Phase 4: It's practical      Verified stdlib (10/11)                          🔴 Active
                             Real stdlib bodies replacing stubs. Iterators,
                             lambdas, toolchain versioning. Package model.
                             Generate the stdlib, don't write it.

Phase 5: It's independent    LLVM backend (10/11)                             Future
                             One compiler, one trust boundary.
                             Native codegen. WASM target. No Rust dependency.

Phase 6: It's complete       Actors, concurrency, model checker (11/11)       Future
                             Actor syntax. Structured concurrency.
                             Pre/post conditions. Full Req 9 proof.
                             Self-hosting. AAE-5 certification pipeline.
```

## Architecture decisions

| ADR | Decision | Status |
|-----|----------|--------|
| [ADR-0001](adr/0001-eleven-requirements.md) | Eleven compiler-verified requirements | Accepted |
| [ADR-0002](adr/0002-language-contraction.md) | Language contraction — what to drop and why | Accepted |
| [ADR-0003](adr/0003-compilation-strategy.md) | Six phases: compile → useful → trustworthy → practical → independent → complete | Accepted |
| [ADR-0004](adr/0004-language-size.md) | Language size — deliberately the smallest | Accepted |
| [ADR-0005](adr/0005-recursive-descent-parser.md) | Hand-written recursive descent parser (LL(1)) | Accepted |

## Design principles

1. **Verification density:** Every feature exists to increase properties proven per token
2. **Contraction:** Remove features that resist verification. The language shrinks by policy.
3. **One way:** One way to branch, one way to loop, one way to handle errors
4. **Stdlib grows, language doesn't:** New functionality via library, not language extensions
5. **Zero dependencies:** The compiler is a single binary. No external crates.
