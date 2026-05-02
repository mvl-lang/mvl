# MVL — Minimum Verification Language

What if we turn things around? Code generation just became frictionless. LLMs write code in any language, at any verbosity, with any annotation burden — for free. So why are we still designing languages for humans to write? What if we designed a language that maximized everything a compiler can verify — type safety, memory safety, termination, data race freedom, information flow — and let the LLM handle the fact that it's verbose, heavily annotated, and ugly to write by hand?

That's the MVL. The smallest language where the compiler verifies the most. Not for human ergonomics — for maximum verification density per generated token.

## Why

Two forces converge:

- **Cybersecurity.** AI-speed attacks (Mythos: autonomous zero-day discovery for $50 in compute) need compile-time defenses. The MVL makes entire vulnerability classes — injection, secret leakage, buffer overflow, privilege escalation — structurally impossible. Code that Mythos would exploit doesn't compile.

- **Safety.** Mission-critical systems (avionics, industrial, automotive) require formal evidence. The MVL compiler generates that evidence automatically: every property proven at compile time is an audit artifact. The path from AAE-3 (spec-driven) to AAE-5 (externally certified).

## What

Eleven compiler-verified requirements. No existing language enforces all of them:

| # | Requirement | What the compiler proves |
|---|---|---|
| 1 | Type safety (ADTs) | No impossible states |
| 2 | Memory safety | No use-after-free, no buffer overflow |
| 3 | Totality (exhaustive match) | All cases handled |
| 4 | Null elimination (Option) | No null pointer dereference |
| 5 | Error visibility (Result) | All errors in the type signature |
| 6 | Ownership (linearity) | No double-free, no leaks |
| 7 | Effect tracking | Side effects visible in types |
| 8 | Termination checking | Functions provably halt (total by default) |
| 9 | Data race freedom | No concurrent access on shared mutable state |
| 10 | Refinement types | Values within valid ranges at compile time |
| 11 | Information flow control | Secret/tainted data tracked through types |

Code that compiles is **well-formed** (internal quality proven). Tests handle **validation** (external quality — does it do the right thing).

## How — Eight Pillars, Nine Phases

A language is "complete" along eight independent pillars: requirements, language constructs, stdlib, testing, packaging, backends, toolchain, verification. The roadmap delivers them in nine phases. Phases 1–4 are the foundation (done). Phases 5–9 are the path forward.

| Phase | Identity | What it proves |
|-------|----------|----------------|
| 1–4 | **Foundation** | MVL verifies its 11 requirements at compile time |
| 5 | **Compiles** | MVL owns the full compilation chain (LLVM, no host compiler) |
| 6 | **Works** | Real programs run — stdlib complete, testing matures |
| 7 | **Ships** | Packages distribute and are trustworthy |
| 8 | **Proves** | Concurrent programs verified — actors and model checking |
| 9 | **Proven** | Language formally verified — Lean/Coq metatheory |

**Status (May 2026):** Foundation complete. Phase 5 shipped May 1 (LLVM Phases A–E + ownership-based drop, v0.60–v0.65). Phase 6 in progress.

Full pillar/phase mapping and per-phase acceptance criteria: [spec 012](.openspec/specs/012-phases/spec.md). Architectural rationale: [ADR-0003](.openspec/adr/0003-compilation-strategy.md).

## Design Principles

1. **Explicit over implicit.** No hidden control flow, no implicit conversions.
2. **One way to do each thing.** One loop, one branch, one error mechanism.
3. **Vocabulary over syntax.** Stdlib functions, not macros or sugar.
4. **Total by default.** Functions must terminate. `partial` opts out.
5. **Immutable by default.** `mut` opts in.
6. **Effects in signatures.** Pure is the default.
7. **Security labels on all data.** `Public`, `Tainted`, `Secret`.
8. **Actors, not threads.** No shared mutable state, no locks, no deadlocks.
9. **Ownership, not GC.** Deterministic deallocation for real-time.
10. **Refinement types inline.** `x: Int where x > 0` is a first-class type.

## What the MVL Drops

No anonymous lambdas, no list comprehensions, no decorators, no operator overloading, no implicit conversions, no default arguments, no variadic arguments, no macros, no ternary operator, no string interpolation, no inheritance, no exceptions, no null, no global state, no `while` in total functions.

~10 statement forms. ~5 expression forms. ~3 declaration forms. The smallest general-purpose language.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [uv](https://github.com/astral-sh/uv) (for mkdocs documentation only)

### Setup

```bash
git clone git@github.com:LAB271/mvl_language.git
cd mvl_language
make setup    # installs git hooks, verifies tooling
```

`make setup` configures git to use `.githooks/` for pre-commit hooks. Every commit automatically runs:

1. `cargo fmt -- --check` — formatting
2. `cargo clippy -- -D warnings` — lint (warnings are errors)
3. `cargo test --quiet` — all tests pass

No Python dependencies — hooks are plain bash scripts.

### Build and test

```bash
make build    # cargo build
make test     # cargo test (unit + integration)
make lint     # cargo clippy
make format   # cargo fmt
```

### Documentation

```bash
make docs       # build mkdocs site
make docs-serve # serve locally at http://localhost:8000
make help       # show all available targets
```

## Project Structure

```
mvl_language/
├── .openspec/              # specs, ADRs, language reference (source of truth)
├── .githooks/              # pre-commit: fmt + clippy + test
├── .github/workflows/      # CI: same checks on push/PR
├── docs/                   # mkdocs site content
│   ├── introduction.md     # 1000-word introduction
│   ├── language.md         # language reference
│   ├── grammar.ebnf        # formal EBNF (~100 productions)
│   ├── stdlib.md           # three-tier stdlib spec
│   ├── references.md       # validated academic references
│   ├── adr/                # architectural decision records
│   └── specs/              # behavioral specifications
├── src/mvl/
│   ├── parser/             # MVL source → AST (Rust)
│   ├── checker/            # AST → typed AST, 11 requirements (Rust)
│   └── transpiler/         # typed AST → Rust source (Rust)
├── tests/
│   ├── corpus/             # MVL example programs (LLM training seed)
│   ├── integration/        # end-to-end: .mvl → compile → run → verify
│   └── spikes/             # experiments
├── Makefile                # make help for all targets
├── mkdocs.yml              # documentation site config
├── CHANGELOG.md
└── README.md
```

## Research

Full research, EBNF grammar, code examples, language scorecard, OWASP mapping, and references: see `docs/` or run `make docs-serve`.

## License

MIT
