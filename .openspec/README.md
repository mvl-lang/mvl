# OpenSpec — MVL Language

Specifications, architectural decisions, and design documents for the Minimum Verification Language.

## Specs

| # | Spec | Focus | Status |
|---|------|-------|--------|
| [001](specs/001-type-system/spec.md) | Type System | ADTs, Option, Result, refinement types, security labels | Draft |
| [002](specs/002-effect-system/spec.md) | Effect System | Effect tracking, capabilities, purity | Draft |
| [003](specs/003-information-flow/spec.md) | Information Flow Control | Tainted/Clean/Secret labels, security lattice, declassification | Draft |

## ADRs

| # | ADR | Status |
|---|-----|--------|
| [0001](adr/0001-eleven-requirements.md) | Eleven compiler-verified requirements | Accepted |
| [0002](adr/0002-language-contraction.md) | Language contraction — what to drop and why | Accepted |
| [0003](adr/0003-compilation-strategy.md) | Compilation strategy — prototype Rust, production LLVM | Accepted |

## Language Reference

See [language.md](language.md) for the complete language reference including EBNF grammar, stdlib, and design decisions.

## Research

Full research: `my-brain/study/mvl_research.md`
References: `my-brain/study/mvl_references.bib`
