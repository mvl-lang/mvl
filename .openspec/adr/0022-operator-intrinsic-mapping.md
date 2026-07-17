# ADR-0022: Operator → Intrinsic Mapping and Stdlib Category Model

**Status:** Accepted
**Date:** 2026-05-08
**Issues:** #558, #559, #560
**Related:** ADR-0019 (two-path stdlib), ADR-0021 (primitives redesign)

---

## Context

Issue #552 (stdlib architecture cleanup) established `pub builtin fn` as the trust boundary
pattern.  After closing sub-tickets #553–#557 the question remained: how do operators like
`+`, `*`, `<<`, `&&` fit into the category model?  They are not `pub builtin fn` declarations
and they are not in any `.mvl` file — yet they are fundamental to the language.

This ADR formalises the three-category model and documents the operator → instruction mapping
for both backends.

---

## Decision

### Three-Category Model

All executable constructs in MVL fall into exactly one of three categories:

| Category | Name | Source of truth | Rust backend | LLVM backend |
|----------|------|-----------------|--------------|--------------|
| 1 | **Operators** | Grammar / type checker | Direct Rust expression | Direct LLVM instruction or intrinsic |
| 2 | **OS Builtins** | `pub builtin fn` (core.mvl) | `println!` / `eprintln!` macros, `assert!`, etc. | Inline LLVM codegen (`dprintf`, `llvm.trap`) |
| 3 | **Type Operations** | `pub builtin fn` (strings, lists, …) | mvl_runtime crate (native Rust) | mvl_runtime_c crate (C-ABI cdylib) |

Category 1 operators are **compiler intrinsics**: the code generator emits the instruction
directly from the AST node without any function call.  They are never `pub builtin fn` and
never appear in the C-ABI dispatch table.

---

### Category 1: Operator → Instruction Mapping

#### Binary operators

| MVL | AST variant | Rust transpiler | LLVM (Int / Float) |
|-----|------------|-----------------|---------------------|
| `a + b` | `BinaryOp::Add` | `a + b` | `llvm.sadd.with.overflow.i64` (checked) / `fadd` |
| `a - b` | `BinaryOp::Sub` | `a - b` | `llvm.ssub.with.overflow.i64` (checked) / `fsub` |
| `a * b` | `BinaryOp::Mul` | `a * b` | `llvm.smul.with.overflow.i64` (checked) / `fmul` |
| `a / b` | `BinaryOp::Div` | `a / b` | `sdiv` / `fdiv` |
| `a % b` | `BinaryOp::Rem` | `a % b` | `srem` / `frem` |
| `a == b` | `BinaryOp::Eq` | `a == b` | `icmp eq` / `fcmp oeq` |
| `a != b` | `BinaryOp::Ne` | `a != b` | `icmp ne` / `fcmp one` |
| `a < b` | `BinaryOp::Lt` | `a < b` | `icmp slt` / `fcmp olt` |
| `a > b` | `BinaryOp::Gt` | `a > b` | `icmp sgt` / `fcmp ogt` |
| `a <= b` | `BinaryOp::Le` | `a <= b` | `icmp sle` / `fcmp ole` |
| `a >= b` | `BinaryOp::Ge` | `a >= b` | `icmp sge` / `fcmp oge` |
| `a && b` | `BinaryOp::And` | `a && b` | `and i1` |
| `a \|\| b` | `BinaryOp::Or` | `a \|\| b` | `or i1` |
| `a & b` | `BinaryOp::BitAnd` | `a & b` | `and i64` |
| `a \| b` | `BinaryOp::BitOr` | `a \| b` | `or i64` |
| `a ^ b` | `BinaryOp::BitXor` | `a ^ b` | `xor i64` |
| `a << b` | `BinaryOp::Shl` | `a << b` | `shl i64` |
| `a >> b` | `BinaryOp::Shr` | `a >> b` | `ashr i64` (arithmetic) |

**Note:** Integer add/sub/mul use LLVM checked-arithmetic intrinsics that trap on overflow,
matching MVL's memory-safety requirement (Req 2).  Float operations are IEEE-754 ordered
comparisons (`o`-prefixed predicates) so NaN comparisons return false.

**Precedence note:** MVL's Pratt parser assigns `&`=50, `|`=30, `^`=40, `<<`/`>>`=70,
all comparisons=60.  This means `&`, `|`, `^` have **lower** precedence than `==` (same
as C).  Write `(a & b) == c`, not `a & b == c`.  Shift operators (`<<`/`>>`) have
**higher** precedence than `==`, so `a << b == c` = `(a << b) == c` (no parentheses needed).

#### Unary operators

| MVL | AST variant | Rust transpiler | LLVM |
|-----|------------|-----------------|------|
| `-a` | `UnaryOp::Neg` | `-a` | `neg i64` / `fneg double` |
| `!a` | `UnaryOp::Not` | `!a` | `xor i1, 1` (bool invert) |
| `*a` | `UnaryOp::Deref` | `*(a)` | load through pointer |
| `~a` | `UnaryOp::BitNot` | `!a` (Rust bitwise NOT) | `xor i64, -1` |

#### Numeric methods (compiler-dispatched, not C-ABI)

| MVL | Rust | LLVM |
|-----|------|------|
| `n.pow(e)` | `n.pow(e as u32)` / `n.powf(e)` | inline branch dispatch on type |
| `n.abs()` | `n.abs()` | `icmp slt` + `neg` + `select` |
| `n.min(m)` | `n.min(m)` | `icmp slt` + `select` |
| `n.max(m)` | `n.max(m)` | `icmp sgt` + `select` |
| `n.clamp(lo, hi)` | `n.clamp(lo, hi)` | chained `select` |
| `x.ceil()` | `x.ceil()` | `llvm.ceil.f64` |
| `x.floor()` | `x.floor()` | `llvm.floor.f64` |
| `x.sqrt()` | `x.sqrt()` | `llvm.sqrt.f64` |

---

### Category 2: OS Builtins

Declared in `std/core.mvl` as `pub builtin fn`.  Both backends implement these inline
(no C-ABI round-trip required):

| Function | Rust transpiler | LLVM backend |
|----------|-----------------|--------------|
| `println(s)` | `println!("{}", s)` macro | `printf` + newline |
| `print(s)` | `print!("{}", s)` macro | `printf` (no newline) |
| `eprintln(s)` | `eprintln!("{}", s)` macro | `dprintf(2, ...)` + newline |
| `eprint(s)` | `eprint!("{}", s)` macro | `dprintf(2, ...)` |
| `format(s)` | `format!("{}", s)` macro | identity (String → String) |
| `assert(c)` | `assert!(c)` macro | `llvm.trap` if `!c` |
| `assert_eq(l, r)` | `assert_eq!(l, r)` macro | string compare + `llvm.trap` |
| `panic(s)` | `panic!("{}", s)` macro | `eprintln` + `llvm.trap` + `unreachable` |

---

### Backend Parity Gap (#560)

As of 2026-05-08, the following Category 3 (Type Operations) functions are available in
the Rust transpiler backend but **not yet in the LLVM backend**:

| Module | Functions | Reason deferred |
|--------|-----------|-----------------|
| `std/random.mvl` | `bytes(n)` | C-ABI returns wrong layout (needs `MvlArray*`) |
| `std/random.mvl` | `choice[T]`, `shuffle[T]` | Generic type parameter, no C-ABI encoding |
| `std/time.mvl` | `now()`, `format_instant()`, `format_datetime()`, `parse()` | Opaque `Instant`/`DateTime` types need handle pattern |
| `std/env.mvl` | `all()` | Returns `List[(String,String)]` tuples — no tuple C-ABI |
| `std/env.mvl` | `args()` | Needs `PtrNoArg` StdlibSig variant |
| `std/args.mvl` | `get_arg()`, `get_args()`, `get_env()` | Needs C-ABI + `PtrNoArg` variant |
| `std/args.mvl` | `parse[T]()` | Generic |
| `std/env.mvl` | `signal_on(sig, handler)` | Requires function pointer callback |
| `std/regex.mvl` | `find_all()`, `captures()` | Returns `List[Match]` / `Option[Captures]` — complex return type |
| `std/collections.mvl` | `set_intersection()`, `set_difference()`, `set_union()` | Uses higher-order MVL functions (lambda support not yet in LLVM) |

**Already working in LLVM** (C-ABI + StdlibSig wired):
- `random.int`, `random.float` (verified by `tests/intrinsics/`)
- `time.sleep` (via `VoidDurationArg` + `_mvl_time_thread_sleep`)
- All of `std/io`, `std/env` (except `all`, `args`, `signal_on`), `std/process`, `std/log`, `std/crypto`, `std/regex` (compile, find, replace)

Follow-up tracking:
- `_mvl_random_bytes` layout fix → tracked in #560 (this issue)
- `PtrNoArg` StdlibSig variant → tracked in #560
- LLVM lambda/HOF support → tracked in #421
- Time handle pattern → new issue to be filed

---

## Test Coverage

The `tests/intrinsics/` directory contains MVL programs that exercise Category 1
operators on both backends:

| File | Operators covered |
|------|-------------------|
| `01_arithmetic.mvl` | `+`, `-`, `*`, `/`, `%` |
| `02_comparison.mvl` | `==`, `!=`, `<`, `>`, `<=`, `>=` |
| `03_logical.mvl` | `&&`, `\|\|`, `!` |
| `04_bitwise.mvl` | `&`, `\|`, `^`, `~`, `<<`, `>>` |

These are also verified by the corpus matrix harness (`make test-rust-llvm`) via `tests/corpus/01_expressions/`.

---

## Relation to language definition

### Eleven Requirements (ADR-0001)

**Requirement 2 — Memory safety:** strengthens. Integer arithmetic operators use
checked LLVM intrinsics (`llvm.sadd.with.overflow.i64`, etc.) that trap on overflow,
directly enforcing the memory-safety requirement at the instruction level.

**Requirement 5 — Information flow control:** consistent with. Operator intrinsics
do not widen or narrow the security label of their operands; the type checker enforces
label propagation before codegen.

Requirements 1, 3, 4, 6–11 are not directly affected by this decision.

### Design Principles (README)

**Principle 3 — Two backends, one semantic:** strengthens. This ADR makes the
operator→instruction mapping explicit and testable for both backends, reducing
the risk of silent semantic divergence between the Rust transpiler and LLVM paths.

**Principle 5 — Minimal stdlib:** consistent with. The three-category model clarifies
what belongs in the stdlib (Category 3 type operations) versus what is intrinsic to
the language (Category 1 operators, Category 2 OS builtins). This prevents feature
creep into the Category 3 C-ABI dispatch path.

**Principle 1 — Explicit over implicit:** strengthens. The trusted/proven profile
manifests make the trust boundary explicit in source files rather than relying on
implicit runtime dispatch.

### Specifications

No specs in `.openspec/specs/` directly enumerate operator semantics at this level.
The three-category model is a cross-cutting concern addressed by ADR-0022 directly.
Follow-up: a spec update may be needed once #533 (stdlib profile switching) is
implemented to capture the profile selection semantics.

---

## Evidence

- `src/mvl/backends/rust/emit_exprs.rs` — `emit_binary_op()`, `emit_unary()`
- `src/mvl/backends/llvm/exprs.rs` — `emit_int_binop()`, `emit_float_binop()`, `emit_unary()`
- `src/mvl/backends/llvm/builtins.rs` — `emit_println`, `emit_eprintln`, `emit_dprintf`
- `std/core.mvl`, `std/random.mvl`, `std/time.mvl`
- `runtime/llvm/src/stdlib/random.rs`, `runtime/llvm/src/stdlib/time.rs`
- `tests/intrinsics/` (new — this ADR)
