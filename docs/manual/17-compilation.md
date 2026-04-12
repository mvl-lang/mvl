# 17. Compilation Model

The MVL compiler verifies all eleven requirements and produces executable code through a multi-phase strategy.

## 17.1 Compiler Phases

```
Source (.mvl)
    │
    ├── Lexing → Tokens
    ├── Parsing → AST (LL(1), recursive descent)
    ├── Name resolution → Scoped AST
    ├── Type checking → Typed AST (Req 1, 3, 4, 5)
    ├── Ownership checking → Owned AST (Req 2, 6)
    ├── Effect checking → Effect-annotated AST (Req 7)
    ├── Totality checking → Termination-verified AST (Req 8)
    ├── Capability checking → Race-free AST (Req 9)
    ├── Refinement checking → Refined AST (Req 10, SMT)
    ├── IFC checking → Flow-verified AST (Req 11)
    │
    └── Code generation (target-dependent)
```

Each checking phase adds a guarantee. Code that passes all phases is *well-formed* — structurally correct with respect to all eleven requirements.

## 17.2 Phase 1: Rust Transpilation

```
MVL source → MVL compiler → Rust source → cargo build → binary
```

The MVL compiler translates to Rust source code. Cargo handles compilation to native code. This leverages Rust's ecosystem (crates, tooling, debugger support) while adding MVL's verification layer on top.

**Advantages:** Fast to implement, access to Rust ecosystem, proven backend.
**Limitation:** Two compilers in the chain — MVL's verification and Rust's borrow checker may disagree on edge cases.

## 17.3 Phase 2: LLVM Backend

```
MVL source → MVL compiler → LLVM IR → LLVM → binary
```

Direct LLVM IR generation. One compiler, one trust chain. The MVL compiler proves all properties and emits IR that LLVM optimizes to native code.

**Advantages:** Single trust boundary, full optimization control, no Rust dependency.

## 17.4 Phase 3: Self-Hosting

```
MVL source → MVL compiler (written in MVL) → binary
```

The MVL compiler rewritten in MVL, compiled by the Phase 2 compiler. The ultimate validation: if the language can express its own compiler with all eleven requirements verified, it's general-purpose enough for anything.

## 17.5 Build Command

```bash
mvl build                           # compile the project
mvl check                           # verify without generating code
mvl test                            # run tests
mvl assurance                       # generate assurance report
```

## 17.6 Requirement Preservation

Each of the eleven requirements travels through the backend in one of three ways:
native enforcement by the target compiler, documentation (the MVL compiler proved it;
a doc comment is the witness), or runtime assertion (the MVL checker verified it
statically; the backend enforces it in debug builds).

See [How MVL Compiles](../compilation-model.md) for the full breakdown across
Phase 1 (Rust) and Phase 2 (LLVM), including the SMT-verified refinements story.

## 17.7 Assurance Report

`mvl assurance` generates a report showing:
- Requirements verified per module
- Extern function count (trust boundary surface)
- Refinement types verified statically vs. runtime
- Effect coverage
- Test coverage linked to specifications
