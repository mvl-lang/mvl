---
domain: compiler
version: 0.1.0
status: draft
date: 2026-05-15
epic: phase-8-compiler-refactor
---

# 018 — Visitor-Based Emission

This spec defines the **Visitor pattern refactor** for the Rust backend emitter.
Instrumentation (coverage, MC/DC, mutation) is currently woven inline into every
emit function via conditional field checks on `RustEmitter`.  The visitor pattern
separates these concerns so each instrumentation mode is a self-contained decorator.

## Motivation

The current `RustEmitter` struct mixes three concerns:

1. **Base code generation** — emit Rust source for MVL AST nodes
2. **Coverage instrumentation** — inject `__mvl_cov::hit()` into branches
3. **MC/DC instrumentation** — inject `__mvl_mcdc::record()` into decisions
4. **Mutation instrumentation** — wrap operators/literals in `match MVL_MUTANT`

This leads to:

- **Coupling**: Every emit function (`emit_stmts.rs`, `emit_exprs.rs`) contains
  coverage/MC/DC/mutation conditional branches.
- **Combinatorial growth**: Adding a new instrumentation mode requires touching
  all emit files.
- **Testing complexity**: Instrumentation cannot be tested independently of the
  base emitter.

**Goal:** Establish a `EmitVisitor` trait + decorator pattern so instrumentation
is additive and composable.

**Implementation:** `src/mvl/backends/rust/visitor.rs`

---

## Requirements

### Requirement 1: EmitVisitor Trait [MUST]

A public `EmitVisitor` trait MUST define the instrumentation injection surface —
the nodes where coverage, MC/DC, and mutation inject calls.

**Implementation:** `src/mvl/backends/rust/visitor.rs::EmitVisitor`

#### Scenario: Base emission without instrumentation

- GIVEN a `BaseEmitter` implementing `EmitVisitor`
- WHEN `visit_if("x > 0", "{ 1 }", Some("{ 0 }"))` is called
- THEN the result is `"if x > 0 { { 1 } } else { { 0 } }"` with no hit calls

#### Scenario: Coverage decorator wraps base emitter

- GIVEN `CoverageVisitor::new(BaseEmitter, 0)`
- WHEN `visit_if("x > 0", "{ 1 }", Some("{ 0 }"))` is called
- THEN the result contains `__mvl_cov::hit(0)` in the true branch
- AND `__mvl_cov::hit(1)` in the else branch
- AND `branch_count()` returns `2`

### Requirement 2: Decorator Composition [MUST]

Instrumentation visitors MUST compose via generic wrapping: `CoverageVisitor<MCDCVisitor<BaseEmitter>>`.

**Implementation:** `src/mvl/backends/rust/visitor.rs::CoverageVisitor`

#### Scenario: Stacked decorators

- GIVEN `CoverageVisitor::new(BaseEmitter, 0)`
- WHEN `visit_match_arm("Foo", "body", 0)` is called
- THEN the arm body includes a `__mvl_cov::hit(N)` call
- AND the surrounding if/match structure is unchanged

### Requirement 3: Zero-Cost Base [MUST]

`BaseEmitter` with no wrapping MUST produce identical output to the current
inline emission for the same inputs.

#### Scenario: No overhead on plain transpile

- GIVEN `BaseEmitter` without any visitor wrapping
- WHEN any `visit_*` method is called
- THEN output matches current `emit_stmts`/`emit_exprs` output for the same node

### Requirement 4: Migration Path [SHOULD]

Existing `emit_stmts.rs`/`emit_exprs.rs` inline instrumentation SHOULD be
replaced file-by-file using the visitor interface.

---

## Migration Plan

The full migration replaces inline `cg.alloc_branch()`/`cg.emit_cov_hit()` calls
in `emit_stmts.rs` and `emit_exprs.rs` with `EmitVisitor` trait dispatch.

### Phase 1 — Foundation (this spec, #773)

- [x] Define `EmitVisitor` trait in `visitor.rs`
- [x] Implement `BaseEmitter` (string-fragment based)
- [x] Implement `CoverageVisitor<V>` decorator
- [ ] Add `MCDCVisitor<V>` decorator
- [ ] Add `MutationVisitor<V>` decorator

### Phase 2 — If/Match migration

Target files: `emit_stmts.rs` (Stmt::If, Stmt::Match, Stmt::For, Stmt::While)

Replace:
```rust
// Before
let true_id = cg.alloc_branch(span.line, BranchKind::IfTrue);
cg.push("if ");
emit_expr(cg, cond);
cg.push(" {");
if let Some(id) = true_id { cg.emit_cov_hit(id); }

// After
let then_body = emit_block_fragment(cg, then);
let else_body = else_.map(|e| emit_else_fragment(cg, e));
let fragment = cg.visitor.visit_if(&emit_expr_fragment(cg, cond), &then_body, else_body.as_deref());
cg.push(&fragment);
```

### Phase 3 — Expression migration

Target files: `emit_exprs.rs` (Expr::If, Expr::Match, mutation injection points)

### Phase 4 — Remove instrumentation fields from RustEmitter

Once all emit functions use the visitor, remove from `RustEmitter`:
- `pub coverage: Option<CoverageMap>`
- `pub mcdc: Option<MCDCMap>`
- `pub mutation: Option<MutationMap>`
- `pub fn alloc_branch(...)`
- `pub fn emit_cov_hit(...)`
- `pub fn alloc_mcdc_decision(...)`
- `pub fn alloc_binary_mutations(...)`

---

## Performance Comparison

### Current (inline conditionals)

```
emit_stmts: O(1) branch check per node — `if self.coverage.is_some()`
```

No dynamic dispatch. Single struct. All instrumentation paths in same cache line.

### Visitor decorator pattern

```
CoverageVisitor<BaseEmitter>: vtable dispatch per visit_* call
```

Dynamic dispatch (`Box<dyn EmitVisitor>`) adds one pointer indirection per node.
Static dispatch (`CoverageVisitor<BaseEmitter>` as concrete type) is zero-cost
after monomorphization — equivalent to the current inline approach.

**Recommendation:** Use static dispatch (concrete generic types) at the call site.
The `transpile()` function is the only entry point; the emitter type is always
known at compile time.  Avoid `Box<dyn EmitVisitor>` in the hot path.

---

## See also

- [Spec 009 — Transpiler & Code Generation](../009-transpiler-codegen/spec.md)
- [ADR-0013 — Transpiler-Mediated Codegen](../../adr/0013-transpiler-mediated-codegen.md)
- [ADR-0027 — Multi-Backend Architecture](../../adr/0027-multi-backend-architecture.md)
- Issue #773 — Visitor-based emission (design)
