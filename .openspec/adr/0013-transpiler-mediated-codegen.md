# ADR-0013: Transpiler-Mediated Type-Directed Code Generation

**Status:** Accepted
**Date:** 2026-04-17
**Context:** MVL has no macros (ADR-0002, language contraction) and no runtime reflection. Some stdlib features — `parse<T>()`, derives (`Debug`, `Clone`, `PartialEq`), and future serialization/formatting — require iterating over struct fields at compile time. This ADR documents the mechanism MVL uses instead of macros or reflection.

---

## Decision

MVL uses **transpiler-mediated type-directed code generation** as its mechanism for features that require compile-time knowledge of type structure.

The transpiler sees the type definition (a struct or enum) and emits the appropriate Rust code — derives, parsing boilerplate, formatting logic, comparison operators. No macro system, no reflection API, no user-written code generation.

## The Three Paths (and why MVL chose the third)

| Mechanism | Who writes the generation logic | Example languages | MVL? |
|-----------|-------------------------------|-------------------|------|
| **Macros** | The programmer (or library author) | Rust `#[derive]`, Lisp, Elixir | ❌ Dropped (ADR-0002) |
| **Runtime reflection** | The runtime, queried by the programmer | Java, Python, Go `reflect` | ❌ Not applicable (compiles to native) |
| **Transpiler-generated code** | The compiler, from type definitions | MVL | ✅ |

## Rationale

1. **Macros resist verification.** Macro-generated code is opaque to the type checker at authoring time. The expansion happens before checking, creating a gap between what the programmer sees and what the compiler verifies. MVL's thesis is that every line of code is checkable — macros break this.

2. **Reflection is runtime overhead.** MVL targets mission-critical systems where runtime introspection adds unpredictable overhead and violates the "no hidden effects" principle (Req 7).

3. **The transpiler already knows the types.** The MVL type checker has the full struct layout. The transpiler emits Rust code from that layout. Generating derives, parsers, and formatters is the same operation — type-directed code emission. No new mechanism needed.

4. **Consistent with the LLM generation model.** In the ISPE model: the struct definition is S (Specification), the transpiler-generated code is P (Program), and the MVL checker verifies alignment. The programmer writes the type; the toolchain generates the implementation; the compiler verifies it. This is the same loop as LLM-generated code, just with the transpiler as the generator instead of the LLM.

## Current scope — features using this mechanism

| Feature | What the transpiler generates | Status |
|---------|------------------------------|--------|
| `Debug`, `Clone`, `PartialEq` derives | Rust `#[derive(...)]` on transpiled structs/enums | ✅ Implemented |
| `parse<T>()` for arg parsing | Struct field iteration → flag matching → type conversion | ✅ Implemented (#55) |
| IFC newtypes (`Public<T>`, `Secret<T>`, ...) | Newtype wrappers with arithmetic, Display, Copy | ✅ Implemented |

## Future scope — features that will use this mechanism

| Feature | What the transpiler would generate | Phase |
|---------|-----------------------------------|-------|
| `json.encode<T>()` / `json.decode<T>()` | Struct field → JSON key mapping | Phase 5 |
| `Display` / `format()` for custom types | Field-by-field string formatting | Phase 5 |
| `Eq`, `Ord` for custom types | Field-by-field comparison | Phase 5 |
| `Serialize` / `Deserialize` (general) | Format-agnostic struct traversal | Phase 6 |

## The boundary rule

**A feature crosses the reflection boundary when it requires iterating over struct/enum fields at compile time.** When this happens:

1. The feature is implemented in the **transpiler**, not in MVL source
2. The generated code is **tracked in the assurance report** as transpiler-generated (not user-written, not extern)
3. The generated code is **still verified** by the MVL checker — it must pass all 11 requirements
4. An entry is added to **this table** documenting the feature

This keeps the language surface area small (ADR-0004) while allowing the compiler to handle the patterns that traditionally require macros.

## Consequences

- **Positive:** No macro system to design, maintain, or teach. No macro hygiene problems. No macro-generated code that bypasses the checker.
- **Positive:** Every stdlib feature either has a real MVL body (verified) or is transpiler-generated (verified) or is extern (trusted). Three categories, no ambiguity.
- **Negative:** Features that other languages implement as derive macros require transpiler changes in MVL. This makes the transpiler thicker.
- **Negative:** Users cannot define their own derives or codegen — only the transpiler can. This is a deliberate trade-off: language simplicity over user extensibility.
- **Trade-off accepted:** MVL is not designed for user-extensible metaprogramming. It is designed for verified code generation. The transpiler is the single point of code generation; the checker is the single point of verification.

## ISPE connection

The type definition is **S** (Specification). The transpiler-generated code is **P** (Program). The checker verifies **S↔P** alignment. The assurance report is **E** (Evidence). All four ISPE layers are covered without adding language surface area — the mechanism is invisible to the MVL programmer.

---

**References:**
- ADR-0002 (language contraction — why no macros)
- ADR-0004 (language size — deliberately smallest)
- ADR-0006 (stdlib strategy — extern + MVL layers)
- #55 (arg parsing via struct types + refinements)
