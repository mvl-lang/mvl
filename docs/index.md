# MVL — Maximum Verifiable Language

**The smallest language where the compiler verifies the most.**

What if we turn things around? Code generation just became frictionless. LLMs write code in any language, at any verbosity, with any annotation burden — for free. So why are we still designing languages for humans to write?

The MVL is designed for LLMs to generate, compilers to verify, and humans to review where the compiler's guarantees end.

## Quick links

- [Introduction](introduction.md) — the full story (1000 words)
- [The Eleven Requirements](requirements.md) — what the compiler verifies, and why
- [Language Manual](manual/index.md) — complete language definition (21 chapters)
- [Language Reference](language.md) — grammar summary, types, effects, expressions
- [EBNF Grammar](grammar.md) — formal grammar (~100 productions)
- [Standard Library](stdlib.md) — three tiers: core, standard, extended
- [How MVL Compiles](compilation-model.md) — requirement preservation across Rust and LLVM targets
- [Roadmap](roadmap.md) — current version, phase status, and critical path
- [Rationale](mvl_rationale.md) — research program and design motivations
- [References](references.md) — academic foundations and bibliography

## The eleven requirements

| # | Requirement | What the compiler proves |
|---|---|---|
| 1 | [Type safety (ADTs)](requirements.md#req-1) | No impossible states |
| 2 | [Memory safety](requirements.md#req-2) | No use-after-free, no buffer overflow |
| 3 | [Totality (exhaustive match)](requirements.md#req-3) | All cases handled |
| 4 | [Null elimination (Option)](requirements.md#req-4) | No null pointer dereference |
| 5 | [Error visibility (Result)](requirements.md#req-5) | All errors in the type signature |
| 6 | [Ownership (linearity)](requirements.md#req-6) | No double-free, no leaks |
| 7 | [Effect tracking](requirements.md#req-7) | Side effects visible in types |
| 8 | [Termination checking](requirements.md#req-8) | Functions provably halt |
| 9 | [Data race freedom](requirements.md#req-9) | No concurrent access on shared mutable state |
| 10 | [Refinement types](requirements.md#req-10) | Values within valid ranges at compile time |
| 11 | [Information flow control](requirements.md#req-11) | Secret/tainted data tracked through types |

## Bootstrap sequence

```
Step 1:  MVL compiler in Rust, transpiles to Rust        ← WE ARE HERE
Step 2:  MVL compiler in Rust, emits LLVM IR
Step 3:  MVL compiler in MVL, compiled by Step 2         (self-hosting)
Step 4:  MVL compiler compiled by itself                 (bootstrap complete)
```
