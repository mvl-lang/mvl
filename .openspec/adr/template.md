# ADR-NNNN: Title

**Status:** Proposed | Accepted | Superseded by ADR-XXXX
**Date:** YYYY-MM-DD
**Issues:** #N

---

## Context

What is the situation that forces a decision? Describe the technical, organizational,
or design forces at play. Include constraints that are not negotiable.

---

## Decision

What is the change being proposed or accepted? Be concrete. If there are multiple
parts to the decision, number them.

---

## Consequences

What becomes easier or harder after this decision? Include both positive and negative
consequences. Note any follow-up work this decision creates.

---

## Rejected Alternatives

What other options were considered and why were they rejected?

---

## Relation to language definition

Every ADR must complete this section before acceptance. Its purpose is to make the
contradiction surface with the language definition explicit — preventing silent drift
across small, locally-reasonable decisions (see #429, #408).

### Eleven Requirements (ADR-0001)

Which of the eleven compiler-verified requirements does this decision touch?
For each requirement affected, state whether this decision **strengthens**,
**weakens**, or **leaves unchanged** that requirement.

If no requirement is directly affected, state that explicitly rather than omitting
the section.

### Design Principles (README)

List each of the ten Design Principles this decision interacts with. For each,
state one of:

- **consistent with** — decision neither adds nor removes support for this principle
- **strengthens** — decision makes this principle easier to enforce or more visible
- **tension — explained below** — decision creates pressure against this principle;
  explain why the trade-off is acceptable

Principles not mentioned are assumed consistent-with. Silence is not permitted for
principles that are directly affected.

### Specifications

Which specs in `.openspec/specs/` are affected by this decision?
Are any spec files or requirement links in need of update?

If no specs are affected, state that explicitly.
