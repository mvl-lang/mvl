# ADR-0044: Self-Hosting Strategy — TIR-First Phase Plan

**Status:** Accepted
**Date:** 2026-06-10
**Issues:** #1113 (epic), #1114 (Phase 1)
**Related:** ADR-0038 (Typed IR), ADR-0034 (monomorphization), ADR-0027 (multi-backend)

---

## Context

The MVL compiler is ~76 K lines of Rust.  The goal of the self-hosting epic (#1113) is to
rewrite it in MVL so that the compiler verifies its own source — MVL becomes its own first
customer.

### Architecture insight

Both backends are fundamentally `TirProgram → String` generators:

- **Rust backend:** `TirProgram → Rust source → rustc`
- **LLVM backend:** `TirProgram → LLVM IR text → llc`

No C FFI needed.  No unsafe.  No inkwell.  Pure string generation, verifiable by MVL.

### Original plan and why it changed

The original plan ported types AST-first, starting with the parser's 1,431-line AST.
After reviewing what actually blocks backend self-hosting, the plan was revised:

| Aspect | AST | TIR |
|--------|-----|-----|
| Lines of source | 1,431 | 605 |
| Type info | Separate `Span → Ty` map | Embedded in every node |
| Generics | Present | Already monomorphized |
| Backend coupling | Requires checker | Self-contained |

Porting TIR first (rather than the full AST) enables **partial self-hosting** before the
parser and checker are ported.

---

## Decision

### Phase plan

| Phase | Ticket | Scope | Status |
|-------|--------|-------|--------|
| 1 | #1114 | Shared types — `compiler/tir.mvl` (TIR-first) | ✅ Done (this ADR) |
| 2 | #1115 | Leaf stages — Resolver, Mono, TIR Lower | ⬜ |
| A | #1118 | Backends — MVL emitters consuming TIR (parallel-track) | ⬜ |
| 3 | #1116 | Parser — Lexer + recursive descent (full AST port) | ⬜ |
| 4 | #1117 | Checker — type checker + 11 requirement passes | ⬜ |
| 6 | #1119 | Bootstrap — compiler compiles itself, 3-stage verify | ⬜ |

**Key insight:** Phase A (#1118, backends) is unblocked immediately after Phase 1.  It does
not need the parser or checker port.  This gives a partial self-hosting milestone — MVL
emitters consuming Rust-produced TIR — before the full port is complete.

### Stage A architecture (partial self-hosting)

```
Stage 0:  Rust parser + checker + mono + lower → TirProgram
                                                    ↓ serialize (JSON or binary)
Stage A:  TirProgram ← MVL backend → LLVM IR / Rust source
                                        ↓
                                      binary
```

### What stays external (invoked via `std/process`)

- `rustc` — compiles emitted Rust source
- `llc` — compiles emitted LLVM IR
- `cc` — links object files
- Z3 — SMT solving (Layer 5, feature-gated, extern FFI)

### `compiler/tir.mvl` — single unified types file

Phase 1 establishes `compiler/tir.mvl` as the single source of truth for all shared
types in the self-hosting port.  It absorbs `compiler/ast.mvl` (deleted) and adds:

- **TIR primitive types** — ported from `src/mvl/parser/ast.rs`:
  `BinaryOp`, `UnaryOp`, `Literal`, `Pattern`, `LetKind`, `LValue`, `RefExpr`,
  `SessionOp`, `MailboxConfig`, `EffectDecl`, `LabelDecl`, `RelabelDecl`, and the
  operator enums `LogicOp`, `CmpOp`, `ArithOp`.
- **Resolved type system** — ported from `src/mvl/checker/types.rs`:
  `Ty`, `SessionTy`.
- **TIR node types** — ported from `src/mvl/ir.rs`:
  `TirExpr`, `TirExprKind`, `TirStmt`, `TirElseBranch`, `TirBlock`, `TirMatchArm`,
  `TirMatchBody`, `TirSelectArm`, `TirParam`, `TirFn`, `TirFieldDecl`, `TirVariant`,
  `TirVariantFields`, `TirTypeBody`, `TirTypeDecl`, `TirExternFn`, `TirExternDecl`,
  `TirActorMethod`, `TirActorDecl`, `TirImplDecl`, `TirConstDecl`, `TirProgram`.

The parser-stage types (`Span`, `Token`, `TypeExpr` simplified struct, `GenericParam`
simplified struct, `FnDecl`, `Program`, etc.) are kept in their simplified forms so that
`compiler/lexer.mvl`, `compiler/parser.mvl`, and `compiler/main.mvl` continue to compile
unchanged (only their `use ast::X` imports are updated to `use tir::X`).

### Naming renames (keyword and collision avoidance)

| Rust name | MVL name | Reason |
|-----------|----------|--------|
| `Ty::String` | `Ty::Str` | `String` is a builtin type |
| `Literal::Float(f64)` | `Literal::Floating(Float)` | avoids `Ty::Float` collision |
| `Literal::Char(char)` | `Literal::Character(Char)` | avoids `Ty::Char` collision |
| `Capability::Ref` | `Capability::RefCap` | avoids `Ty::Ref` collision |
| `TirFn.requires` | `TirFn.pre_conds` | `requires` is a keyword |
| `TirFn.ensures` | `TirFn.post_conds` | `ensures` is a keyword |
| `TirStmt::While.decreases` | `.decrease_by` | `decreases` is a keyword |
| `TirStmt::If.else_` | `.else_br` | `else` is a keyword |
| `TirExprKind::Relabel.tag` | `.audit_tag` | `tag` is a keyword |
| `TirTypeBody::Struct.invariant` | `.type_invariant` | `invariant` is a keyword |
| `Ty::Fn(…)` tuple variant | `Ty::TyFn { params, ret, effects, totality }` | named fields for clarity |
| `Ty::Array(Box<Ty>, u64)` tuple | `Ty::Array { elem, size }` | named fields for clarity |
| `TirExprKind::List/Map/Set` | `ListLit/MapLit/SetLit` | avoids ambiguity with builtin names |
| `RefExpr::LogicOp/ArithOp` | `Logic/Arith` | avoids confusion with the standalone enum names |

### Rust tuple fields → MVL

Rust uses `Vec<(A, B)>` for association lists.  MVL has native tuple types, so:

```
Vec<(String, Pattern)>   →  List[(String, Pattern)]
Vec<(String, SessionOp)> →  List[(String, SessionOp)]
Vec<(String, TirExpr)>   →  List[(String, TirExpr)]
Vec<(TirExpr, TirExpr)>  →  List[(TirExpr, TirExpr)]
Vec<(String, SessionTy)> →  List[(String, SessionTy)]
```

No struct wrappers are needed; MVL's tuple type syntax handles these directly.

---

## Consequences

**Positive:**

- **Partial self-hosting unblocked:** Phase A (#1118) can start immediately; MVL backend
  emitters consuming Rust-produced TIR are now possible without a full front-end port.
- **Smaller initial surface:** TIR (605 lines Rust) is far smaller than the full AST
  (1,431 lines), reducing porting risk and iteration time.
- **Single source of truth:** `compiler/tir.mvl` is the one file for all shared types;
  no cross-file type duplication.
- **Compiler is the oracle:** `cargo run -- check compiler/tir.mvl` passes (8/11 reqs
  proven); all 153 corpus tests pass.

**Negative / trade-offs:**

- **Simplified parser-stage types:** `TypeExpr`, `GenericParam`, `FnDecl`, etc. remain
  in simplified form (struct instead of the full Rust enum).  The full AST port is
  deferred to Phase 3 (#1116).  This means the self-hosting compiler's parser cannot
  yet express the full AST.
- **Two-phase bootstrap:** Until Phase 3 is complete, the self-hosting front-end cannot
  parse the full MVL surface language; it relies on the Rust front-end to produce TIR.

---

## Eleven Requirements (ADR-0001)

| Req | Effect |
|-----|--------|
| 1 — Type safety | **proven** — `tir.mvl` checks with 8/11 reqs proven |
| 2–9 — Memory, effects, linearity, … | **consistent** — pure data types, no I/O |
| 10 — Refinements | **consistent** — `RefExpr` preserved in TIR |
| 11 — IFC | **consistent** — `Ty::Labeled`, `TirExprKind::Relabel` preserved |
