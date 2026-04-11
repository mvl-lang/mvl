# MVL — Minimum Verification Language

**The smallest language where the compiler verifies the most.**

What if we turn things around? Code generation just became frictionless. LLMs write code in any language, at any verbosity, with any annotation burden — for free. So why are we still designing languages for humans to write?

The MVL is designed for LLMs to generate, compilers to verify, and humans to review where the compiler's guarantees end.

## Quick links

- [Introduction](introduction.md) — the full story (1000 words)
- [Language Reference](language.md) — grammar summary, types, effects, expressions
- [EBNF Grammar](grammar.md) — formal grammar (~100 productions)
- [Standard Library](stdlib.md) — three tiers: core, standard, extended

## The eleven requirements

| # | Requirement | What the compiler proves |
|---|---|---|
| 1 | Type safety (ADTs) | No impossible states |
| 2 | Memory safety | No use-after-free, no buffer overflow |
| 3 | Totality (exhaustive match) | All cases handled |
| 4 | Null elimination (Option) | No null pointer dereference |
| 5 | Error visibility (Result) | All errors in the type signature |
| 6 | Ownership (linearity) | No double-free, no leaks |
| 7 | Effect tracking | Side effects visible in types |
| 8 | Termination checking | Functions provably halt |
| 9 | Data race freedom | No concurrent access on shared mutable state |
| 10 | Refinement types | Values within valid ranges at compile time |
| 11 | Information flow control | Secret/tainted data tracked through types |

## Bootstrap sequence

```
Step 1:  MVL compiler in Rust, transpiles to Rust        ← WE ARE HERE
Step 2:  MVL compiler in Rust, emits LLVM IR
Step 3:  MVL compiler in MVL, compiled by Step 2         (self-hosting)
Step 4:  MVL compiler compiled by itself                 (bootstrap complete)
```
