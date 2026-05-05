# ADR Conventions

Architectural Decision Records live here. Each ADR captures a significant design
decision: the context that forced it, the decision made, and its consequences.

## File naming

`NNNN-short-slug.md` — four-digit zero-padded number, hyphenated lowercase slug.
Numbers are allocated sequentially; never reuse a retired number.

Retired numbers (merged into another ADR) are marked in `index.md` but the files
are removed to prevent confusion.

## Template

Use `.openspec/adr/template.md` for every new ADR. The template is mandatory, not
advisory.

## The "Relation to language definition" section

Every ADR numbered 0017 and above must include a `## Relation to language definition`
section with three subsections:

1. **Eleven Requirements (ADR-0001)** — which requirements does this touch, and how?
2. **Design Principles (README)** — for each affected principle, state "consistent with",
   "strengthens", or "tension — explained below"
3. **Specifications** — which `.openspec/specs/` files are affected?

### Why this section exists

Issue #408 revealed that the language can drift across five small ADRs, each of which
passes local review because no single step is *visibly* contradicting the language
definition. The Design Principles and the eleven requirements live in separate documents
from the ADRs that erode them. Without a required moment of confrontation, drift is
invisible until it accumulates.

A checkbox at the end of the template can be ticked without thought. A required section
that asks specific questions about specific principles forces the author to either
(a) demonstrate consistency or (b) name the tension. Both are acceptable outcomes.
Silent drift is not.

### Retroactive updates

ADRs 0001–0016 predate this requirement. They may be retroactively updated when next
touched, but there is no backfill task. ADR-0017 is the worked example of what a
retroactively completed section looks like.

## CI enforcement

`tools/check_adr.py` runs in CI on every PR. It fails if:

- Any ADR numbered >= 0017 is missing the `## Relation to language definition` section.
- Two ADR files share the same four-digit number.

Run locally with:

```bash
python3 tools/check_adr.py --verbose
```

or via Make:

```bash
make check-adr
```
