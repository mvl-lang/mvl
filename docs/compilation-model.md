# How MVL Compiles — Requirement Preservation Across Targets

This page explains what happens to each of the eleven requirements as MVL
source travels through the compilation pipeline.  The answer differs by
target and phase.

## Five-Stage Compiler Pipeline

Every MVL program passes through exactly five stages before any code is emitted:

```
┌─────────────────────────────────────────────────────────────────────┐
│  MVL source                                                         │
│       │                                                             │
│  1. Parse    source → AST  (recursive descent, LL(1))              │
│       │                                                             │
│  2. Resolve  imports, modules, stdlib linking → scoped AST         │
│       │                                                             │
│  3. Check    type checking + all 11 compile-time guarantees        │
│       │                                                             │
│  4. Passes   coverage · MC/DC · mutation testing · linting         │
│       │                                                             │
│       │  (monomorphize + lower AST → TIR — resolved types          │
│       │   embedded in every expression node; ADR-0038)             │
│       ▼                                                             │
│  5. Emit     ──► Rust source  (backend 1, production today)        │
│              └─► LLVM IR      (backend 2, strategic target)        │
└─────────────────────────────────────────────────────────────────────┘
```

Stages 1–4 are backend-agnostic.  The MVL compiler is the sole proof gate —
all eleven requirements are fully verified before emit touches a single byte
of target code.  `mvl check` stops after stage 3; `mvl lint` through stage 4;
`mvl build` runs all five.

Between the last verification pass and code emission, the compiler runs an
explicit lowering step that monomorphizes generics and rewrites the AST into
the Typed IR (TIR).  Backends consume `TirProgram` only — no AST types
cross the emit boundary (ADR-0050).

## Two Backends — Why Both Exist

```
Backend 1 (Rust transpiler):
  MVL compiler ─► Rust source ─► rustc ─► binary
  • Production backend today
  • Rust's borrow checker re-verifies reqs 1–6 independently
  • Full access to the Rust/Cargo ecosystem via extern "rust"
  • Two compilers in the chain — edge-case disagreements are possible

Backend 2 (LLVM):
  MVL compiler ─► LLVM IR ─► LLVM ─► binary
  • Strategic target — one compiler, one trust chain
  • Effects, totality, refinements become IR-generation errors (not doc-comments)
  • SMT-verified refinements (Z3/CVC5) replace debug_assert! — zero runtime cost
  • No Rust dependency; full optimization control
```

Both backends compile the same MVL source.  The test suite differentially fuzzes
them against each other — if the two backends produce different output for the
same program, that is a compiler bug, not a language ambiguity.  The LLVM backend
is the long-term home; the Rust backend is the bridge that keeps the project
shipping while the LLVM backend matures.

## The Proof Gate

```
MVL source
    │
    ▼
MVL compiler  ◄── single trust anchor: all 11 requirements verified here
    │
    ├── Backend 1 ──► Rust source ──► rustc ──► binary
    │
    └── Backend 2 ──► LLVM IR ──────► LLVM  ──► binary
```

The MVL compiler is the **proof gate**.  Code that passes it is
well-formed with respect to all eleven requirements.  What the backend
does with those proofs depends on the target.

---

## Three tiers of enforcement

Once the MVL compiler accepts a program, the downstream target handles
each requirement in one of three ways:

| Tier | What the target does | Who enforces the requirement |
|------|---------------------|------------------------------|
| **Native** | Emits idiomatic target constructs | The target compiler (`rustc` / LLVM) re-enforces for free |
| **Documented** | Emits a `///` doc comment or attribute | MVL proved it; the doc is a machine-readable witness |
| **Asserted** | Emits a runtime check (`debug_assert!` / LLVM `!range`) | MVL proved it statically; the target enforces it at runtime in debug mode |

---

## Phase 1 — MVL → Rust

### Requirements enforced natively by rustc

These map directly to Rust idioms.  `rustc` re-verifies them
independently, providing a second line of defence.

| Req | MVL construct | Rust emission | Enforced by |
|-----|---------------|---------------|-------------|
| 1 — Type safety (ADTs) | `type T = struct { … }` / `type T = enum { … }` | `pub struct T { … }` / `pub enum T { … }` | rustc |
| 2 — Memory safety | ownership model | normal Rust ownership | rustc borrow checker |
| 3 — Exhaustive match | `match` over ADT | `match` | rustc exhaustiveness |
| 4 — Null elimination | `Option[T]` | `Option<T>` | rustc |
| 5 — Error visibility | `Result[T, E]` | `Result<T, E>` | rustc |
| 6 — Linearity / ownership | move semantics | Rust move semantics | rustc borrow checker |

### Requirements preserved as documentation

Rust has no syntax for these.  The MVL compiler verified them; the
emitted doc comment is the certificate.  A human reviewer — or a future
tool — can audit it.

```rust
// MVL: total fn safe_divide(...) -> Public[Float]
/// # Totality
/// This function is declared `total` in MVL: it must terminate for all inputs.
pub fn safe_divide(numerator: Public<Amount>, denominator: Public<NonZero>) -> Public<f64> {
    …
}

// MVL: fn authenticate(…) -> Result[Session, AuthError] ! IO, Console
/// # Effects: IO, Console
/// MVL effect annotations — informational in Phase 1.
pub fn authenticate(…) -> Result<Session, AuthError> {
    …
}
```

| Req | MVL annotation | Rust emission |
|-----|---------------|---------------|
| 7 — Effect tracking | `! IO, Console` | `/// # Effects: IO, Console` |
| 8 — Termination | `total fn` | `/// # Totality` |
| 9 — Data race freedom | `iso store: val UserStore` | `/* iso */` comment on parameter |

### Requirements preserved as runtime assertions

Rust's type system cannot express these statically in Phase 1.  The
MVL checker verified them; the transpiler emits `debug_assert!` guards
that fire in debug builds if the proof assumptions are ever violated at
runtime.

```rust
// MVL: type Amount = Float where self >= 0.0
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub struct Amount(pub f64);

impl Amount {
    /// Construct `Amount` — panics in debug mode if refinement is violated.
    pub fn new(v: f64) -> Self {
        debug_assert!((v >= 0.0), "refinement violated: Amount({})", v);
        Self(v)
    }
}
```

```rust
// MVL: Tainted[String] cannot flow to Console effect
// Rust Phase 1: newtype wrapper + explicit sanitize()/declassify()
pub struct Tainted<T>(pub T);
pub fn sanitize<T>(v: Tainted<T>) -> Clean<T> { Clean(v.0) }

// The MVL checker already rejected any path where Tainted data flows
// to a Console effect.  The newtype documents the label; explicit
// conversion functions enforce correct flow at the call site.
```

| Req | MVL construct | Rust emission |
|-----|--------------|---------------|
| 10 — Refinement types | `type T = Base where pred` | newtype `struct T(Base)` with `debug_assert!(pred)` constructor |
| 10 — Field refinements | `field: Int where self >= 0` | `debug_assert!` in struct `new()` |
| 11 — IFC labels | `Public[T]`, `Tainted[T]`, `Secret[T]`, `Clean[T]` | generic newtypes with `Copy`/`Display`/arithmetic impls; `sanitize()`/`declassify()` for allowed flows |

### External type stubs (Phase 1)

Types referenced in MVL function signatures but not defined in the current module
(e.g. an opaque handle from an external library) are collected and emitted as
placeholder structs.  Method calls on those types are also stubbed using return-type
information inferred from the call-site let-binding:

```rust
// MVL: iso store: val UserStore  (UserStore not defined in this module)
/// Placeholder for external type `UserStore` (not defined in this module).
pub struct UserStore;

impl UserStore {
    // inferred from: let user: Option[User] = store.find_user(user_id)?
    pub fn find_user(&self, _: Public<UserId>) -> Result<Option<User>, AuthError> { todo!() }
}
```

This lets reference examples that depend on library types compile to Rust without
manual scaffolding.  The `todo!()` body panics at runtime; a real library would
replace the stub with an actual implementation.

### Summary table — Phase 1

| Req | Enforcement tier | Enforced by |
|-----|-----------------|-------------|
| 1 ADTs | Native | rustc |
| 2 Memory safety | Native | rustc borrow checker |
| 3 Exhaustive match | Native | rustc |
| 4 Option | Native | rustc |
| 5 Result | Native | rustc |
| 6 Ownership | Native | rustc borrow checker |
| 7 Effects | Documented | MVL (doc comment) |
| 8 Totality | Documented | MVL (doc comment) |
| 9 Race freedom | Documented | MVL (capability comment) |
| 10 Refinements | Asserted | MVL (debug_assert!) |
| 11 IFC | Partially native + documented | MVL + newtype discipline |

---

## Phase 2 — MVL → LLVM IR (planned)

LLVM IR is a typed intermediate representation with no borrow checker and
no built-in ownership model.  Everything the MVL compiler proves must be
encoded directly into the IR.

### One compiler, one trust chain

The key improvement over Phase 1: LLVM does not re-verify anything.  The
MVL compiler is the sole source of correctness guarantees.  There is no
borrow-checker friction and no risk of the two compilers disagreeing.

```
Phase 1:  MVL compiler ─► Rust source ─► rustc ─► binary
          (two compilers may disagree on edge cases)

Phase 2:  MVL compiler ─► LLVM IR ─► LLVM backend ─► binary
          (one proof chain, full control)
```

### How each tier changes in Phase 2

**Native tier** — same requirements, different mechanism.  LLVM IR
enforces types within basic blocks; memory safety is encoded as LLVM
`noalias`, `nonnull`, and lifetime metadata rather than Rust's borrow
checker.

**Documented tier → becomes static** — the MVL compiler emits LLVM
metadata annotations that tools can inspect.  More importantly, **effect
checking and totality checking move into the IR generation step**: an
effectful call to a non-declared effect generates a compile-time error
rather than a doc comment.

**Asserted tier → becomes static** — this is the major gain.

| Req | Phase 1 | Phase 2 |
|-----|---------|---------|
| 10 Refinements | `debug_assert!` at runtime | SMT solver (Z3) at IR generation time; no runtime cost |
| 11 IFC labels | newtypes + sanitize/declassify | LLVM type system encodes labels; forbidden flows become `unreachable` IR |
| 7 Effects | doc comment | IR generation refuses to emit call if effect not declared |
| 8 Totality | doc comment | termination checker integrated into IR lowering |

### SMT-verified refinements

In Phase 2, `type Amount = Float where self >= 0.0` does not emit a
`debug_assert!`.  Instead, the MVL compiler queries an SMT solver (Z3 or
CVC5) at compile time.  Every constructor call site is checked: if the
solver cannot prove `value >= 0.0` from the surrounding context, it is
a compile-time error.  No runtime overhead, no `debug_assert!`.

### IFC in LLVM IR

LLVM's type system does not have `Public[T]` and `Secret[T]` built in.
Phase 2 encodes the lattice using distinct LLVM struct types and a
taint-propagation pass that runs on the IR.  Any value flow that crosses
a label boundary without an explicit declassify/sanitize emits a
diagnostic during compilation, not at runtime.

---

## The LLM connection

The design goal is:

> LLMs write MVL.  The compiler verifies.  Humans review where the compiler's guarantees end.

The three-tier model shows exactly where the compiler's guarantee ends at
each phase:

- **Phase 1:** Tier 3 (runtime assertions) and partial Tier 2
  (doc comments) represent the gap — things the MVL compiler verified but
  the Rust backend cannot statically re-enforce.

- **Phase 2:** That gap closes.  All eleven requirements become statically
  enforced in the single compilation chain.

An LLM that generates MVL which passes the compiler has produced code with
a *machine-checked proof* of all eleven requirements — not a stylistic
convention, not a test suite, but a proof.  The compilation target carries
that proof to the binary as efficiently as the target allows.

---

## See also

- [ADR-0003: Compilation Strategy](adr/0003-compilation-strategy.md) — architectural decision record
- [Introduction](introduction.md) — the eleven requirements
- [Manual §17: Compilation Model](manual/17-compilation.md) — build commands and phase overview
