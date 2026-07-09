# ADR-0054: Method receiver clone semantics

**Status:** Accepted
**Date:** 2026-07-06
**Issues:** #1693

---

## Context

MVL supports type-attached methods via the `pub fn Type::method(self, ...)`
syntax.  Method calls at call sites use `receiver.method(args)`.  The Rust
backend emits these as native Rust method calls after transpilation.

MVL's borrow inference (`capability_params_for_tir_fn`) analyses each fn's
body to decide whether params (including `self`) should be emitted as
`&T` (read-only borrow), `&mut T`, or `T` (owned).  A read-only-inferred
`self` becomes `&self` in the Rust signature.

At **call sites**, MVL performs Phase A last-use analysis
(`last_use::compute_last_uses`) that marks each variable's final read
position.  Free-fn arg emission (`emit_expr_as_value_arg`) inserts
`.clone()` before non-last-use `Var` arguments so the caller's binding
stays alive.  The same treatment was previously missing for method
receivers.

**Symptom:** A user-defined `EmitCtx::method(self, ...)` called as
`ctx.method(...)` from inside a fn that used `ctx` many times raised
`error[E0382]: borrow of moved value: ctx` — MVL emitted bare
`ctx.method(...)` with no clone.  Free-fn form `method(ctx, ...)`
worked because free-fn arg emission handles the clone.

## Decision

Method-receiver clone-insertion is **per-dispatch-path**, not universal:

1. **Stdlib fast-path methods** (`.push`, `.map`, `.get`, `.filter`,
   `.len`, `.map_values`, `.into_iter`, …) route through the shared
   helper `RustEmitter::emit_method_receiver` in `emit_exprs.rs`.
   That helper does **NO** clone insertion.  Each specific dispatch
   arm in `emit_method_call.rs` decides its own borrow semantics:
   - `&mut self` methods (`.push`, `.insert`, `.set`) rely on Rust's
     auto-ref for `let mut` locals.  Cloning would snapshot the pointee
     and drop the write — silent bug that broke `range()`'s
     `result.push(current)`.
   - Consuming methods (`.into_iter().collect::<Vec>()`, ...) emit
     `.clone()` explicitly in their pushed suffix when they know the
     receiver is used again.
   - `&self` methods (`.get`, `.contains_key`, `.len`) auto-ref from
     owned locals; no clone needed.

2. **User-defined method calls** (the generic fallthrough in
   `emit_method_call.rs`) route through the new helper
   `RustEmitter::emit_user_method_receiver`.  It wraps
   `emit_method_receiver` and appends `.clone()` when the receiver is
   a `Var` NOT at its last use — mirroring `emit_expr_as_value_arg`'s
   treatment of free-fn args.

The rule: **the shared helper stays borrow-neutral; call-site clone
knowledge lives with the caller who knows the callee's contract**.

## Consequences

**Positive:**

- `ctx.method(...)` now works even when `ctx` is used many times in
  the same fn body — no more E0382 move errors for user methods.
- Free-fn workarounds (`method(ctx, ...)`) are no longer required for
  read-only accessor methods on shared state.  Design freedom to use
  the more readable method syntax.
- Enables #1693 to use `EmitCtx::method` accessors in `context.mvl`,
  matching Rust's `impl RustEmitter { ... }` idiom as closely as MVL
  supports today.
- Stdlib mutating methods (`.push`, `.insert`, …) continue to work
  in-place on `ref` locals — no regression to `range()` and similar
  builders.

**Negative:**

- Clone-insertion is duplicated across two paths (`emit_expr_as_value_arg`
  for free-fn args, `emit_user_method_receiver` for user methods).
  Future refactoring could hoist a common `emit_var_with_last_use_clone`
  primitive.
- The user-method path clones unconditionally on non-last-use, ignoring
  whether the method's `self` is inferred `&self` (in which case the
  clone is redundant, though correct).  LLVM opts elide most redundant
  clones after `-O`.  A future improvement could consult the method's
  inferred borrow flags and skip the clone when `&self`.

**Follow-up work:**

- MVL doesn't yet track "mutable ref local" scopes explicitly in the
  emitter.  The `range()` breakage during development was detected via
  the corpus, not statically.  Adding a `mutable_locals` HashSet to
  `RustEmitter` would let a future refactor unify the two clone-insertion
  paths safely.

## Rejected Alternatives

### 1. Universal clone in `emit_method_receiver`

Attempted first.  Cloned all method receivers unconditionally when
non-last-use.  Broke `range()`: `result: ref List[Int]` calls
`result.push(current)` in a loop; the emitted `result.clone().push(current)`
pushed to a fresh clone each iteration, discarding writes.  Output
changed from `5` to `0`.  Impossible to fix without adding mutable-local
tracking to the emitter (see follow-up).

### 2. Free functions only

Route all EmitCtx accessors as free fns (`string_global_of(ctx, k)`)
instead of methods (`ctx.string_global_of(k)`).  Works today because
free-fn arg emission handles the clone.  Rejected as the final answer
because:
- Loses the readable method-call chain syntax.
- Loses parity with Rust's `impl EmitterCtx { ... }` idiom that #1693
  aims to mirror.
- Requires 19 call-site refactors AND every future accessor to remember
  the pattern.

Used as a workaround pre-fix; superseded once the transpiler fix landed.

### 3. Clone only when method takes `self` (not `&self`)

Consult the callee's inferred borrow flag and skip the clone when the
method is `&self` (Rust would auto-ref).  Cleaner but requires the
call-site to resolve the callee's fn signature — a cross-cutting
concern in the transpiler.  Extra clones on `&self` calls are correctness-
preserving and typically LLVM-elided; the simpler unconditional-clone-
on-non-last-use path was chosen.

Filed as a future optimisation opportunity.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

1. **Implicit—explicit tradeoff** — **strengthens**. Makes method-receiver clone semantics explicit at call sites via the transpiler's last-use analysis, preventing silent borrow errors.
2. **Concurrency primitive for safe parallelism** — **unchanged**. Does not affect async/concurrent semantics.
3. **Correct borrow checker semantics for memory safety** — **strengthens**. Eliminates E0382 move errors on method-receiver calls by applying the same last-use clone logic as free functions.
4. **Type system ensuring effect locality** — **unchanged**. Effect-system handling is independent of method-call codegen.
5. **Typeclasses with instance resolution** — **unchanged**. No relation to method dispatch (which uses type-attached methods, not typeclasses).
6. **Pattern matching with exhaustiveness** — **unchanged**. Pattern semantics are independent of method-call lowering.
7. **Predictable performance model** — **weakens slightly**. Method-call clone insertion is now conditional (per last-use), adding a runtime cost that depends on the flow analysis. However, this is correct and LLVM typically elides redundant clones; the alternative (universal clone) would be worse.
8. **Compile-time termination guarantee** — **unchanged**. Clone insertion is a codegen choice, not a termination concern.
9. **Symbol-name hygiene** — **unchanged**. Does not affect name mangling or symbol generation.
10. **Audit-trail accuracy** — **unchanged**. IFC and audit labeling are independent of method-call lowering.
11. **Semantics clarity for self-hosting** — **strengthens**. Formalizing method-call clone semantics explicitly is necessary for the self-hosted compiler (#1693) to mirror Rust emitter behavior faithfully.

### Design Principles (README)

- **Explicit over implicit** — **strengthens**. The rule "call-site clone knowledge lives with the caller" makes clone insertion explicit and localized.
- **One way to do it** — **consistent with**. There is one rule per dispatch path; user methods follow free-fn clone semantics.
- **The signature IS the threat model** — **consistent with**. Method borrow flags remain visible in the signature; clone insertion follows the inferred flags.
- **No UFCS** — **unchanged**. Method call syntax is unchanged by this decision.
- **No bare unwrap** — **unchanged**. This decision does not affect error handling.

### Specifications

This decision affects the self-hosting specification for the Rust backend:
- `.openspec/specs/010-tir-backend/spec.md` — clarifies that method-call codegen must apply the same last-use clone logic as free-function argument emission.
- `.openspec/specs/011-self-hosting-phase3-llvm/spec.md` (if created) — would specify the method-receiver clone semantics for the MVL-hosted LLVM backend (see #1693).

Currently, these specs are not detailed enough to reference this decision explicitly. A follow-up improvement would add a scenario covering method-call clone insertion.
