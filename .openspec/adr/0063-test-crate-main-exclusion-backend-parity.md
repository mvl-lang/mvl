# ADR-0063: Test-crate `fn main` exclusion — LLVM/WASM parity

**Status:** Accepted
**Date:** 2026-08-05
**Issues:** #2198, #2207

---

## Context

`mvl test <file>` compiles the file under test together with any "sibling"
modules it needs. Both the LLVM and WASM backends resolve siblings the same
way — a transitive, import-scoped walk of `use` declarations
(`loader::load_sibling_modules_transitive`) — and both emit **one flat,
single-namespace module**: every top-level function shares one symbol
table. The Rust backend uses its own, separately-implemented
sibling-inclusion logic in `src/cli/test.rs`, and always compiles test
crates as a **library crate** (`src/lib.rs`), never a binary.

`fn main` is not special MVL syntax. It's an ordinary function that happens
to be the whole program's entry point. Nothing in the language stops a file
that declares `fn main` from also being a legitimate sibling dependency of
some other file under test — MVL compiles whole modules, not individual
functions, so importing *anything* from a file drags its `fn main` along
with it.

**Bug 1 (origin, #2198):** the file under test itself declared both
embedded `test fn`s and `fn main` (`examples/log_analyzer/main.mvl`, the
real program entry point). The LLVM test-crate builder emitted that `fn
main` verbatim *and* synthesized its own dispatch `main` — needed because
`lli` (the LLVM interpreter `mvl test --backend=llvm` drives) only knows
how to execute a function literally named `main`; there is no `lli --invoke
<test_name>` equivalent. Two `@main` definitions in one module → `lli`
refuses to load it ("invalid redefinition of function 'main'").

**Bug 2 (sibling variant, #2207):** `examples/bzip/roundtrip_test.mvl` has
no `fn main` of its own, but genuinely does
`use main.{compress_bytes, decompress_bytes, ...}` — a real dependency, not
directory-scoped noise. `main.mvl`'s `fn main` comes along for the ride as
a sibling and collides with the same synthesized dispatch `main`.

**Bug 3 (WASM variant, #2207):** WASM never needs a synthesized dispatch
`main` — `wasmtime run --invoke <test_name>` calls any exported function by
name directly. But an earlier, upfront "duplicate function name across
entry+siblings" check (added by #2036 to catch genuine naming conflicts
before they surfaced as opaque `wasm-tools` errors) doesn't distinguish
"two files that happen to each declare their own unrelated `fn main`,
neither needed by any test" from a real naming conflict. It hard-fails and
tells the user to rename a file — for what is really just an artifact of
this backend's single-namespace test-build strategy.

**Why Rust never hits any of this:** `mvl test`'s Rust-backend path always
compiles the test crate to `src/lib.rs`. In a library crate, `fn main` —
wherever it's declared, in whatever module — is an ordinary function name
scoped to that module. It has no special meaning to `rustc`, and `cargo
test`/`rustc --test` never looks for it. Rust's module system namespaces
each file's declarations (`helper::main`, `entry::main`, ... are distinct
paths); LLVM-text/WASM-text's flat emission has no equivalent namespacing.

## Decision

When compiling a **test crate** — never a real `mvl build`/`mvl run`, and
never the `// expect:` corpus-style runner that actually executes `main`
and checks its stdout — both LLVM and WASM backends now:

1. **Drop every `fn main` they encounter** before emission: the entry
   file's own, and every sibling's. Nothing in test mode ever legitimately
   calls `main()` — it's the whole-program entry point, never a callee —
   and test execution has its own dispatch mechanism (LLVM's synthesized
   `main`; WASM's `--invoke <test_name>`).
2. **Exempt `main` specifically from the upfront duplicate-function-name
   check**, in test-crate mode only. Once (1) drops both copies, two files
   each declaring their own unrelated `fn main` is not a real conflict. A
   genuine duplicate of any *other* function name still hard-fails with the
   existing, unchanged diagnostic.

Mechanically: a single boolean, `for_test_dispatch`, threaded through each
backend's shared "prepare TIR for entry+siblings" function —
`prepare_llvm_text_tir_multi` (LLVM) and `compile_wat_multi` /
`build_and_assemble` (WASM). `false` for every real-build/run/expect-test
call site; `true` only for each backend's `test fn` dispatch path.

The Rust backend needs no code change — already correct by construction.

## Consequences

### Positive

- `log_analyzer` (#2198) and `bzip` (#2207) now compile and pass their full
  test suites on both LLVM and WASM; both previously failed with either a
  cryptic `lli` crash or, on WASM, a needless hard-error demanding an
  unrelated file rename.
- Sibling inclusion itself (import-scoped, `use`-driven) is unchanged —
  this decision affects only what happens to `fn main` once it has already
  been pulled in for testing, not what gets pulled in.
- All three backends now agree on user-visible behavior: a project with a
  sibling `fn main` tests cleanly everywhere, even though the underlying
  mechanism differs per backend (LLVM/WASM: explicit filtering at
  test-crate-assembly time; Rust: no filtering needed, architecturally
  immune via library-crate compilation).

### Negative

- Two independent boolean-threading changes (LLVM, WASM) rather than one
  shared fix — the backends' CLI modules (`src/cli/llvm_text.rs`,
  `src/cli/wasm_text.rs`) have no shared "prepare test crate" abstraction
  today. A future refactor could hoist a common builder if a third backend
  ever needs the same shape.
- `fn main`'s special-casing is now duplicated per backend (once in the
  dup-check exemption, once in the `fns.retain()` filter) — a small,
  well-commented maintenance surface, not a large one.

## Rejected Alternatives

**Rename the colliding `fn main`** (e.g. mangle to `__main_<file>`) instead
of dropping it. Rejected: nothing in a test crate ever needs to call `main`
by any name, mangled or not — keeping it under a synthetic name is pure
dead weight with no benefit over dropping it outright.

**Require the user to rename one file's `fn main`** — WASM's pre-existing
behavior before this fix. Rejected as a general policy: forces a cosmetic
change to working source purely to satisfy a test-only compiler
limitation, and doesn't generalize to the entry-file case (`log_analyzer`
can't "rename" `main.mvl`'s `main` without changing the real program's
actual entry point).

**Fix sibling inclusion instead of `fn main` handling** — considered, since
the symptom (`main.mvl` pulled in) looks like an over-inclusion bug at
first glance. Rejected as the explanation here: `load_sibling_modules_transitive`
is already import-scoped (a transitive `use`-walk), and `roundtrip_test.mvl`'s
import of `main.mvl` is genuine, not noise. A separate, real
directory-vs-import-scoping issue exists elsewhere in the toolchain (raised
during this investigation), but fixing it would not have prevented either
#2198 or #2207 — both examples' sibling inclusion was already correct.

## Relation to language definition

### Eleven Requirements (ADR-0001)

Unaffected. `fn main`'s test-crate handling is a build-orchestration /
test-harness concern, not a change to any of the eleven compiler-verified
properties. No requirement's checking, transpilation, or runtime behavior
changes for non-test builds.

### Design Principles (README)

- **Honest over silent** — strengthens. The duplicate-name check for
  genuine conflicts (any function other than `main`) is unchanged and
  still hard-fails loudly; `main`'s exemption doesn't weaken diagnostics
  for real conflicts, and dropping `main` in test builds is an explicit,
  documented step, not a silent skip.
- **One syntax per concept** — consistent with. `fn main` remains ordinary
  MVL syntax; this decision is purely compiler-internal test-crate
  assembly, not a language change.
- Other principles — unaffected.

### Specifications

- `.openspec/specs/004-testing/spec.md` — new requirement: test-crate
  assembly must exclude every `fn main` from entry + siblings, with
  LLVM/WASM/Rust parity scenarios (this PR).

## Related

- Origin: #2198 (`examples/log_analyzer` LLVM+WASM fixes).
- This decision: #2207 (sibling-file variant, both backends).
- Follow-up (explicitly not fixed here): sibling-loading over-inclusion
  in toolchain paths outside `load_sibling_modules_transitive` may be
  directory-scoped rather than import-scoped in places — tracked
  separately; unrelated to the bugs this ADR addresses.
