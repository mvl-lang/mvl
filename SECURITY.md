# Security Policy — MVL compiler

This file covers `mvl-lang/mvl`: the compiler, its standard library, its runtime
crates, and the `mvl` CLI including the package manager. It takes precedence over
the [organisation policy](https://github.com/mvl-lang/.github/blob/main/SECURITY.md),
which covers reporting, timelines and disclosure — those are unchanged and not
repeated here.

Read the section below first. It is the part that makes this project different
from most, and it is where reports are most valuable.

---

## What MVL's guarantees actually cover today

MVL's premise is that a compiler can prove eleven properties statically, and that
doing so makes whole classes of vulnerability unreachable. We believe that
premise. But **the eleven requirements are not uniformly enforced across every
backend yet**, and a security policy that let you assume otherwise would be
dishonest.

The enforcement point is the **checker**. It runs for every backend and is where
all eleven requirements are decided. The backends then vary in how much of that
decision they carry into generated code:

| # | Requirement | Checker | Rust backend | LLVM backend |
|---|-------------|---------|--------------|--------------|
| 1 | Type safety (ADTs) | enforced | native (`rustc`) | native (LLVM types) |
| 2 | Memory safety | use-after-move and borrow lifetimes enforced | native (`rustc`) | `noalias` / `nonnull` metadata |
| 3 | Totality | enforced | native | native |
| 4 | Null elimination | enforced | native | native |
| 5 | Error visibility | enforced | native | native |
| 6 | Ownership / linearity | move tracking | native | `HeapKind` drop |
| 7 | Effect tracking | enforced | **doc comment only** | **planned** |
| 8 | Termination | `while` rejected; structural recursion planned | **doc comment only** | **planned** |
| 9 | Data-race freedom | capabilities checked; actor-boundary Phase 8 | **capability comment** | **planned** |
| 10 | Refinement types | static, with runtime fallback | `debug_assert!` | **SMT planned** |
| 11 | Information flow control | labels + `relabel` enforced | newtypes | **taint pass planned** |

Two consequences worth stating outright:

- **If you rely on Req 11 (IFC) to keep secrets out of logs, that is enforced by
  the checker, not by the emitted LLVM.** Compiling with the LLVM backend and
  bypassing the checker does not give you taint tracking.
- **Req 10 refinement predicates that cannot be discharged statically become
  runtime assertions.** On the Rust backend those are `debug_assert!` — they are
  compiled out in release builds. A refinement you assumed was proven may be
  neither proven nor checked in a release binary. `mvl assurance` reports which
  obligations were discharged statically and which were downgraded; read it.

The authoritative version of the table above lives in
[`docs/roadmap.md`](docs/roadmap.md#requirement-enforcement-status). If the two
disagree, the roadmap is correct and this file has drifted — please report that
too.

### A gap between a claimed guarantee and actual enforcement is a security bug

For most projects, "the tool is wrong" is a correctness issue. Here, if the
compiler accepts a program that violates a requirement it claims to enforce, that
is a **security vulnerability** and we want to hear about it through the private
channel, not as a public issue. Concretely, we consider these reportable:

- a program that passes `mvl check` but violates one of the eleven requirements
- an IFC label that can be removed without a `relabel` transition
- a refinement predicate reported as statically discharged that does not hold
- an `iso` value aliased after `consume`, or reachable from two actors
- an assurance report that overstates what was proven

## Known gaps, already public

We would rather you knew than rediscovered these. They are tracked openly:

- **[#2021](https://github.com/mvl-lang/mvl/issues/2021)** — `last_use.rs` keys
  last-use tracking by variable *name* rather than lexical binding identity, so
  shadowing can select the wrong occurrence. **Double-free / use-after-free risk
  in LLVM-backend output.**
- **[#2043](https://github.com/mvl-lang/mvl/issues/2043)** — MVL adopted Pony's
  `consume` without Pony's ephemeral type (`iso^`). Post-consume aliasing is
  reconstructed by dataflow analysis rather than typing, and that reconstruction
  is incomplete: ADR-0029 documents both deferred post-consume tracking and
  direct `iso` aliasing without `consume` as known limitations.

Reports that sharpen these into concrete exploits are welcome and useful.

## Trust boundaries

These are outside the guarantees by design, not by oversight:

- **`extern "rust"` and `extern "c"` blocks.** MVL types the boundary and tracks
  effects and IFC labels across it, but it cannot verify the foreign
  implementation. Everything reachable through an `extern` block is as safe as
  that foreign code, and no safer. `pkg-sqlite`, `pkg-tls` and `pkg-tui` all use
  one.
- **`builtin fn`** — runtime-provided implementations. Same reasoning.
- **Generated code, once compiled by `rustc` or LLVM.** We rely on both.
- **The compiler itself is not verified.** Formalising the type system in Lean 4
  and proving soundness is Phase 9, not done. The trust chain today terminates in
  a Rust codebase and an LLVM codebase, neither formalised.

## Additional in-scope areas specific to this repository

Beyond the organisation policy's scope:

- **Compiler robustness on untrusted input.** A crash, hang, unbounded memory
  growth, or non-terminating type-check triggered by a malicious `.mvl` file. If
  you build a service that compiles user-submitted MVL — the
  [playground](https://github.com/mvl-lang/mvl-playground) is exactly that — this
  matters to you. Fuzz targets exist (`make fuzz-rust`, `fuzz-llvm`,
  `fuzz-diff`); findings from them are welcome.
- **Package manager.** `mvl add` / `install` / `update` resolve by git URL and
  tag, verified against a SHA-256 in `mvl.lock`. Anything that lets a resolved
  dependency differ from its lockfile entry, or that weakens the lockfile check,
  is in scope. So is anything in `mvl sbom` or `mvl audit` that would under-report.
- **`mvl self install` and toolchain layout** — anything writing outside
  `$MVL_HOME` or executing content from a downloaded archive.
- **Assurance output.** `mvl assurance`, `mvl prove` and `mvl mcdc` produce
  evidence people may rely on for regulated work. Output that overstates
  verification is in scope.

## Supported versions

Latest minor release only; no backports. Note that the `1.x` version number
predates completion of Phases 7–9 — it does not indicate that the verification
story is finished. See [`docs/roadmap.md`](docs/roadmap.md).

## Reporting

As per the [organisation policy](https://github.com/mvl-lang/.github/blob/main/SECURITY.md):
email `abuse@schubergphilis.com` with `[mvl]` in the subject, or use GitHub
private vulnerability reporting. Targets are 5 business days to acknowledge, 15 to
confirm or close. Do not open a public issue.
