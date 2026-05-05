# ADR-0020 — BDD as library naming convention, not language syntax

**Status:** Accepted
**Date:** 2026-05-05
**Issue:** #39

---

## Context

Issue #39 explored three options for adding BDD-style integration testing to MVL:

- **Option A** — library functions (`given()`, `when()`, `then()`) taking named functions as arguments
- **Option B** — test runner DSL (naming conventions, BDD reports from `mvl test`)
- **Option C** — external tool (Cucumber `.feature` files driving MVL scaffolds)

The hard constraint from ADR-0002 and ADR-0004: **do not extend the language**. BDD is a testing concern, not a language feature.

Python provides a useful reference. Its "native language" approach (e.g. `pytest`) avoids Cucumber feature files entirely: BDD structure lives in the language as regular functions with naming conventions. `pytest-bdd` formalises this with decorators; the underlying idea is that given/when/then are just functions bound by convention.

MVL has no decorators and no anonymous lambdas (dropped per ADR-0002). But it does have named functions, structs, and `test fn`. That is enough.

---

## Decision

BDD in MVL is **Option B (naming convention) implemented as Option A (library pattern)** — hybrid:

1. **Naming convention** (recognised by `mvl test --bdd`):
   - `given_*` — pure setup function, returns a context struct
   - `when_*` — pure transform function, takes a context struct, returns updated context
   - `then_*` — assertion function, takes a context struct, returns `Unit`
   - `test fn scenario_*` — chains the above, is the test entry point

2. **Context struct threading** — state flows through steps explicitly via a scenario-specific struct. No global mutable state, consistent with MVL's ownership model.

3. **No new keywords** — `given`, `when`, `then` remain identifiers, not reserved words.

4. **BDD report** — `mvl test --bdd` can extract scenario names from `scenario_*` test functions and emit Gherkin-style output based on naming conventions. This is purely a runner concern.

5. **Connection to openspec** — `.openspec/specs/` scenarios (Given-When-Then in English) are the authoritative BDD spec. `_test.mvl` files are the executable evidence. The naming convention creates traceable links: a spec scenario `Scenario: user adds two numbers` maps to `test fn scenario_user_adds_two_numbers`.

---

## Pattern

```mvl
// Context struct carries state between steps
type CalcCtx = struct {
    a: Int,
    b: Int,
    result: Int,
}

// Given: set up preconditions, return context
fn given_two_numbers(a: Int, b: Int) -> CalcCtx {
    CalcCtx { a: a, b: b, result: 0 }
}

// When: apply action, return updated context (pure transform)
fn when_added(ctx: CalcCtx) -> CalcCtx {
    CalcCtx { a: ctx.a, b: ctx.b, result: add(ctx.a, ctx.b) }
}

// Then: assert on context
fn then_result_equals(ctx: CalcCtx, expected: Int) -> Unit {
    assert_eq(ctx.result, expected)
}

// Scenario: chains steps — this is the test entry point
test fn scenario_adding_two_numbers() -> Unit {
    let ctx = given_two_numbers(2, 3)
    let ctx = when_added(ctx)
    then_result_equals(ctx, 5)
}
```

---

## Consequences

**Good:**
- Zero language changes — fully consistent with ADR-0002 and ADR-0004
- Pure functions with explicit state threading — consistent with MVL's ownership model
- Spec-to-test traceability via naming convention (`scenario_*` ↔ `Scenario:` in openspec)
- `mvl test --bdd` can produce Gherkin-style reports purely from function names — no parser changes needed
- Readable: the scenario body reads like a narrative

**Bad / trade-offs:**
- More verbose than Python `pytest-bdd` (no decorators, no fixture injection)
- Context struct must be defined per domain — but this is idiomatic MVL (no hidden state)
- Shadowing `let ctx` in a scenario body requires the checker to support re-binding of the same name (standard in most languages; MVL should support this)

---

## Alternatives rejected

- **Cucumber `.feature` files** (Option C): adds external tooling dependency, duplicates spec scenarios that already exist in `.openspec/`
- **New keywords** `given`/`when`/`then`: violates ADR-0004 (language size constraint); testing is not a language concern
- **Anonymous lambdas as step arguments**: dropped from MVL per ADR-0002
