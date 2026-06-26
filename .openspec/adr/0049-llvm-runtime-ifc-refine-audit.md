# ADR-0049: IFC, refine, and audit runtime parity — codegen-only on LLVM

**Status:** Accepted
**Date:** 2026-06-26
**Issues:** #1545, #1547

---

## Context

A parity audit (#1545) of `runtime/rust/src/` versus `runtime/llvm/src/` found three modules present on the Rust side with no LLVM counterpart:

| Module | Rust path | Role |
|--------|-----------|------|
| IFC label wrappers | `runtime/rust/src/ifc.rs` | `Tainted<T>` / `Clean<T>` / `Public<T>` / `Secret<T>` `repr(transparent)` newtypes |
| Refinement assertions | `runtime/rust/src/refine.rs` | `mvl_refine!` macro expanding to `debug_assert!` |
| Audit-trail builtin | `runtime/rust/src/stdlib/audit.rs` | `emit_relabel_event(...)` — writes JSONL to `MVL_AUDIT_SINK` or stderr |

Sub-issue #1547 asked: **port** these to `runtime/llvm/src/` so the LLVM backend has the same surface, or **document** that on LLVM these concerns live in codegen, not runtime?

Port is acceptable only if codegen does not already provide an equivalent guarantee. Each module needs an independent answer.

---

## Decision

### 1. IFC label wrappers — **codegen-only on LLVM**

The Rust transpiler emits actual `Secret(v)` / `Tainted(v)` constructors so the Rust compiler enforces IFC at compile time. The newtypes are `repr(transparent)` and erase to the inner value at the ABI; they exist purely so the Rust type system reproduces MVL's compile-time label discipline.

The LLVM backend has no equivalent type system to leverage. It already strips labels in the emitter: `Expr::Relabel { expr, .. }` is treated as a transparent unwrap (`src/mvl/backends/llvm_text/emit_exprs.rs:315`). MVL's IFC discipline is enforced in the checker (`src/mvl/checker/`), which runs ahead of both backends; codegen does not need to re-enforce it. The runtime behaviour — passing the inner value through unchanged — is identical on both backends.

**No LLVM port. `runtime/rust/src/ifc.rs` stays where it is** as a Rust-only artifact of how the Rust backend reuses the host type checker.

### 2. Refinement assertions — **codegen-only on LLVM, debug divergence accepted**

`mvl_refine!` expands to `debug_assert!`: in release builds it is a no-op on the Rust path, and an absent no-op on the LLVM path — equivalent. In debug builds the Rust path panics on violation; the LLVM path does not check.

Refinement contracts are validated by the checker (#1025 / ADR-0025); the runtime macro is a defensive check, not the primary enforcement. The release-mode divergence is zero; the debug-mode divergence is a small loss of safety-net coverage during local development, not a correctness gap.

**No LLVM port for now.** If a need arises (e.g. debug runs on LLVM that should panic on refinement violation), the LLVM backend can emit an explicit `_mvl_refine_panic` call gated on a `--debug-refine` flag; tracked when it bites.

### 3. Audit-trail builtin — **defer; not codegen-equivalent**

The Rust backend emits a real call to `mvl_runtime::stdlib::audit::emit_relabel_event(...)` whenever a relabel is marked `audit` (`src/mvl/backends/rust/emit_exprs.rs:459`). The LLVM backend currently ignores the `audit` flag and emits a plain relabel — programs run, but no audit record is produced.

This is a real divergence, not a stylistic one: `audit` is the canonical evidence channel for assurance pipelines (#896). It cannot honestly be ADR-documented as codegen-only.

The fix is a small port:

- `runtime/llvm/src/stdlib/audit.rs` exporting `_mvl_audit_emit_relabel(transition, from, to, tag, location)` with `*const MvlString` args, plus an entry in the LLVM stdlib registry.
- LLVM emitter: in the `Expr::Relabel` arm, when `audit` is set (or the relabel declaration is `audit`-marked), build the five string literals and emit the C-ABI call before the transparent unwrap.

Sized as a follow-up rather than blocking this ADR. Tracked in a separate sub-issue of #1545 so the assurance gap is visible.

---

## Consequences

**Easier:**
- Reviewers no longer have to wonder why `runtime/llvm/src/` lacks IFC and refine modules; the answer is documented and grounded in how the checker carries enforcement.
- Future audits can use this ADR as the precedent for "type-system-leveraged" Rust runtime modules that intentionally don't cross to LLVM.

**Harder:**
- Audit-trail support on LLVM remains absent until the follow-up lands. Programs that rely on `audit` records as evidence must use the Rust backend in the interim.
- New IFC label or refinement features added to the Rust runtime need a parity assessment against this ADR: is the new behaviour also type-system enforced (codegen-only OK), or is it observable at runtime (port required)?

---

## Rejected Alternatives

**Port everything to LLVM unconditionally.** Would add `runtime/llvm/src/ifc.rs` and `runtime/llvm/src/refine.rs` for symmetry, but the LLVM backend would never call them — IFC is stripped pre-codegen and refine is debug-only. Dead code with maintenance cost.

**Delete the Rust-side modules and treat all three as codegen.** Would force the Rust backend to inline newtype constructors and refinement asserts at every call site, regressing the readability of generated Rust code and giving up the Rust compiler's structural enforcement of label distinctions. Net loss.

**Block this ADR on the audit port.** Keeps three open questions coupled when one of them is genuinely independent. Audit is a real porting task; IFC and refine are not — separating them lets the documentation land now and the porting work proceed on its own timeline.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

- **Req 11 (IFC labels)** — **leaves unchanged**. Both backends enforce labels at the checker level (same `check::ifc` pass). The Rust newtype machinery reproduces that enforcement at the host-language level; the LLVM backend takes the checker's verdict at face value. No change in what the source program must prove.
- **Req 6 (refinement types)** — **leaves unchanged** in release; minor debug-mode divergence noted above. Primary enforcement remains the checker (#1025 / ADR-0025).
- Other requirements unaffected.

### Design Principles (README)

- **Explicit over implicit** — consistent with. The ADR explicitly distinguishes type-system-enforced from runtime-observable.
- **One way to do it** — strengthens. The default for IFC/refine on any new backend is "trust the checker, no runtime"; only audit-style observable behaviour earns a runtime port.
- **The signature IS the threat model** — consistent with. Labels and refinements are visible in source; the runtime modules are implementation detail.
- Other principles consistent-with.

### Specifications

- `.openspec/specs/011-ifc/` (Req 11) — no spec change needed; this ADR documents an implementation strategy, not new behaviour. The audit gap is a known-deferred runtime artifact tracked in #1545's follow-ups.
- No other specs affected.
