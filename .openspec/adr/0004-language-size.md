# ADR-0004: Language Size — Deliberately the Smallest

**Status:** Accepted
**Date:** 2026-04-11
**Context:** How large should the MVL be? Programming languages tend to grow over time. Features are added, never removed. The MVL must resist this.

## Decision

The MVL SHALL be the smallest general-purpose language by surface area. ~10 statement forms, ~5 expression forms, ~3 declaration forms, ~25 keywords. One way to do each thing.

## The spectrum

| Language | Statement forms | Keywords | Spec pages | Design era |
|----------|----------------|----------|------------|------------|
| **MVL** | ~10 | ~25 | ~5 (EBNF) | 2026 — designed for LLMs |
| **Go** | ~15 | 25 | ~50 | 2009 — designed for simplicity |
| **Rust** | ~20 | 51 | ~300 | 2015 — designed for safety |
| **Python** | ~30 | 35 | ~700 | 1991 — designed for readability |
| **Java** | ~30 | 67 | ~800 | 1995 — designed for portability |
| **C++** | ~50+ | 97 | ~1,800 | 1985 — designed for everything |
| **C++ with STL** | ~50+ | 97 | ~3,500+ | 40 years of accretion |

C++ is the anti-MVL: maximum syntax surface, maximum ambiguity, maximum ways to do each thing. Templates, operator overloading, multiple inheritance, implicit conversions, exceptions AND error codes, raw pointers AND smart pointers AND references, macros AND templates AND constexpr. Every feature added, nothing removed in 40 years.

## Why small is better for LLM generation

**Fewer patterns to learn.** An LLM generating MVL code needs to know ~25 constructs. An LLM generating C++ needs to know hundreds of interacting features. Fewer patterns = higher interpolation accuracy = better code.

**Fewer ambiguities for the compiler.** Every language feature is a potential interaction. 10 features = 45 pairwise interactions. 50 features = 1,225 pairwise interactions. The compiler's verification power scales inversely with the number of features it must reason about.

**One way to do each thing.** In Python, there are 4 ways to format a string (`%`, `.format()`, `f""`, `Template`). In C++, there are 5 ways to initialize a variable. Each alternative is a pattern the LLM must choose between — and each choice can be wrong. The MVL eliminates the choice.

## Why small is better for verification

**Verification density = properties proven per token.** A `match` and a `switch` cost the same tokens, but `match` carries an exhaustiveness proof. The MVL only has `match` — every branching token carries verification.

**No feature interactions to reason about.** Operator overloading + implicit conversions + templates = unbounded complexity for the type checker. The MVL has none of these. The type checker is simpler, faster, and provably correct.

**Smaller trusted computing base.** The compiler itself is smaller. A smaller compiler is easier to verify (CompCert verified 100K lines of C compiler; verifying a C++ compiler is intractable). The MVL compiler will be small enough to audit.

## The bet

A language 1/5th the size of Go with 10x the verification power of C++. This is only possible because the LLM absorbs the verbosity cost that would make the MVL unbearable for humans to write by hand.

The MVL is verbose, heavily annotated, and ugly to type. That's the point. The LLM doesn't care about ergonomics. The compiler cares about explicitness. Optimizing for the compiler's needs instead of the programmer's comfort is the fundamental design inversion.

## Growth policy

Features SHALL NOT be added to the MVL unless they increase verification density. The bar: "does this feature let the compiler prove something it couldn't prove before?" If the answer is no — if the feature is convenience, sugar, or ergonomics — it does not enter the language. Languages grow by default. The MVL shrinks by policy.

## Consequences

- The MVL will feel alien to programmers accustomed to expressive languages
- Code will be 2-3x more verbose than equivalent Python or Go
- That verbosity is borne by the LLM, not the human
- The compiler is small enough to formally verify (future goal)
- The language spec fits in a single EBNF file (~100 productions, 111 lines)
