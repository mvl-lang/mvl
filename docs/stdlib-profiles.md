# Stdlib Profiles

MVL ships with two stdlib profiles that control how much of the standard library
is compiler-verified versus assumed correct at compile time.

## Choosing a profile

| Profile | Flag | Use case | What the compiler verifies |
|---------|------|----------|---------------------------|
| `trusted` | *(default)* | Development, most production | 11 requirements on your code |
| `proven` | `--stdlib=proven` | Safety-critical systems | 11 requirements on your code and the stdlib itself |

```bash
mvl build myapp.mvl                    # trusted (default)
mvl build myapp.mvl --stdlib=proven    # full verification
mvl check myapp.mvl --stdlib=proven    # check without building
mvl run   myapp.mvl --stdlib=proven    # run with proven profile
```

## What "trusted" means

In the `trusted` profile (the default) the stdlib is backed by `pub builtin fn`
declarations.  Each builtin is verified to have the correct *type signature* and
*capability annotations* — but its *implementation* is not an MVL program; it
lives in the `mvl_runtime` / `mvl_runtime_c` Rust crates and is taken on trust.

The compiler still enforces all 11 requirements on *your* code.  The only thing
trusted without proof is the stdlib implementation itself.

For most programs this is entirely acceptable: the Rust runtime is well-tested,
memory-safe by construction, and reviewed by the MVL team.

## What "proven" means

In the `proven` profile the stdlib is implemented in MVL itself, so the compiler
can apply all 11 requirements to *everything* — your code and the standard
library.

A small set of irreducible builtins (OS syscalls, hardware intrinsics, and other
constructs that cannot be expressed in MVL) remains trusted even in `proven` mode.
These are clearly documented and minimised by design (see ADR-0023).

> **Status (2026-05):** the `proven` profile CLI flag is accepted but the MVL
> stdlib implementations are not yet available — see issue #538.  Until #538
> lands, `--stdlib=proven` prints an informational note and falls back to
> `trusted` mode.  The output is identical; the flag is a no-op today.

## Certification guidance

For safety-critical systems targeting DO-178C, IEC 61508, or ISO 26262:

1. Build and test with `--stdlib=proven` to maximise the scope of compiler proof.
2. Audit the irreducible builtins (documented in the trust boundary section of
   each stdlib module).
3. The compiler's 11-requirement proof is an audit artefact: every property
   proven at compile time is evidence that can be submitted to a certifier.
4. Pair compile-time proof with MC/DC coverage (`make test-coverage`) and
   mutation testing (`make test-mutation`) for structural coverage evidence.

See `docs/requirements.md` for the full list of eleven requirements and what each
one guarantees at compile time.
