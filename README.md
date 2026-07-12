# MVL — Maximum Verifiable Language

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

## How — Compiler Pipeline

Every MVL program passes through five stages before any code is emitted:

```
1. Parse   — source → AST (recursive descent, LL(1))
2. Resolve — imports, modules, stdlib linking
3. Check   — type checking + all 11 compile-time guarantees
4. Passes  — coverage, MC/DC, mutation testing, linting
5. Emit    — Rust source (backend 1) or LLVM IR (backend 2)
```

The MVL compiler is the single proof gate — all eleven requirements are fully verified before emit touches any target code.

## Design Principles

Nine principles across two tiers. Tier 1 is the *why* — cross-cutting philosophy that guides every decision. Tier 2 is the *what* — concrete structural choices those principles produced. See `docs/principles.md` for the canonical page with full sourcing.

### Tier 1 — Meta principles

1. **Explicit over implicit.** No hidden control flow, no implicit conversions, no silent coercion. Every relevant property lives in the signature or the source.
2. **One syntax per concept.** One loop, one branch, one error mechanism, one form of mutation. Regularity beats variety.
3. **Vocabulary over syntax.** Grow the stdlib, not the grammar. Compression comes from named, typed, verifiable functions — never from sugar the compiler can't see through.
4. **The signature IS the threat model.** Effects, IFC labels, ownership, refinements, termination, errors — all visible in the type signature. Nothing is ambient.
5. **Honest over silent.** The compiler must either verify a property or reject the program. Never silently drop, accept, or defer.

### Tier 2 — Structural decisions

6. **Eleven requirements — no more, no less.** The compiler verifies exactly eleven properties. A twelfth is added only if it catches bugs no combination of the other eleven catches. (ADR-0001)
7. **Language contraction.** Features are dropped whenever they prioritise writability over verifiability. (ADR-0002)
8. **LL(1) grammar, hand-written recursive descent.** ~100 EBNF productions, no lookahead beyond one token, no parser generator. (ADR-0005)
9. **Ownership, not GC.** Memory safety via linearity and reference capabilities (`val`, `ref`, `iso`, `tag`) adapted from Pony. Deterministic deallocation, no garbage collector. (ADR-0029)

## What the MVL Drops

No mutable closures, no list comprehensions, no decorators, no operator overloading, no implicit conversions, no default arguments, no variadic arguments, no macros, no ternary operator, no string interpolation, no inheritance, no exceptions, no null, no global state, no `while` in total functions.

Anonymous lambdas with immutable captures are supported (`|x| x + 1`) — they power the higher-order functions in the stdlib (`map`, `filter`, `fold`).

~10 statement forms. ~5 expression forms. ~3 declaration forms. The smallest general-purpose language.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [uv](https://github.com/astral-sh/uv) (for mkdocs documentation only)

### Setup

```bash
git clone git@github.com:mvl-lang/mvl.git
cd mvl
make setup    # installs git hooks, verifies tooling
```

`make setup` configures git to use `.githooks/` for pre-commit hooks. Every commit automatically runs:

1. `cargo fmt -- --check` — formatting
2. `cargo clippy -- -D warnings` — lint (warnings are errors)
3. `cargo test --quiet` — all tests pass

No Python dependencies — hooks are plain bash scripts.

### Build and test

```bash
make build    # build the MVL compiler
make test     # run all 7 suites: unit, corpus, stdlib, transpiler, LLVM, tree-sitter, grammar
make lint     # cargo clippy
make format   # cargo fmt
make help     # show all targets grouped by section
```

### Stdlib profiles

MVL supports two stdlib profiles (see `docs/stdlib-profiles.md`):

```bash
mvl build myapp.mvl                  # trusted profile (default)
mvl build myapp.mvl --stdlib=proven  # proven profile (pending #538)
```

The `trusted` profile verifies all 11 requirements on your code.  The `proven`
profile extends verification into the standard library itself, for
safety-critical systems (DO-178C, IEC 61508, ISO 26262).

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
├── src/
│   ├── main.rs             # entry point: version resolution + dispatch
│   ├── cli/                # CLI command modules (one file per subcommand)
│   │   ├── mod.rs          # shared helpers + dispatch()
│   │   ├── args.rs         # argument parsing utilities
│   │   ├── check.rs        # mvl check
│   │   ├── build.rs        # mvl build / run
│   │   ├── test.rs         # mvl test
│   │   ├── mutate.rs       # mvl mutate
│   │   ├── mcdc.rs         # mvl mcdc
│   │   ├── lint.rs         # mvl lint
│   │   ├── assurance.rs    # mvl assurance
│   │   ├── complexity.rs   # mvl complexity
│   │   ├── transpile.rs    # mvl transpile
│   │   ├── meta.rs         # mvl init / self / add
│   │   └── llvm.rs         # mvl build|run|test --backend=llvm
│   └── mvl/
│       ├── loader.rs       # stage 2: file loading, stdlib wiring
│       ├── pipeline.rs     # orchestrates loader → checker → transpiler
│       ├── parser/         # stage 1: MVL source → AST
│       ├── checker/        # stage 3: typed AST, 11 requirements
│       ├── passes/         # stage 4: coverage, MC/DC, mutation, linting
│       └── backends/       # stage 5: code generation (ADR-0027)
│           ├── mod.rs      # Backend trait + AssertMode
│           ├── rust/       # stage 5a: typed AST → Rust source
│           └── llvm/       # stage 5b: typed AST → LLVM IR
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

Apache-2.0
