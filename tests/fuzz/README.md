# Fuzzing the MVL Compiler

Grammar-based fuzzing for the MVL compiler using [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html).

## Prerequisites

```bash
# Nightly Rust is required by cargo-fuzz / libFuzzer
rustup toolchain install nightly

# Install cargo-fuzz
cargo install cargo-fuzz
```

## Targets

| Target | Phase | Backend | Command |
|--------|-------|---------|---------|
| `transpile_rust` | 1 (now) | Rust transpiler | `make fuzz-rust` |
| `transpile_llvm` | 2 (gated) | LLVM codegen | `make fuzz-llvm` |
| `transpile_diff` | 3 (gated) | Both (differential) | `make fuzz-diff` |

Phase 2 is gated on the `mvl_runtime` cross-backend symbol issues being resolved (#406, #421) and Phase 5 settling.
Phase 3 requires Phase 2.

## Running (Phase 1)

```bash
# Short smoke run — verify harness works (a few seconds)
make fuzz-rust FUZZ_TIMEOUT=10

# Standard run — coverage-guided, run until interrupted or timeout
make fuzz-rust

# Long overnight run
make fuzz-rust FUZZ_TIMEOUT=86400
```

The default `fuzz-rust` target runs with a 5-second timeout to serve as a quick sanity check.
For a proper fuzzing session, use a larger timeout or omit it entirely (Ctrl-C to stop).

## Corpus

The initial corpus is seeded from `tests/corpus/**/*.mvl` — real MVL programs that cover
most grammar productions. libFuzzer grows the corpus automatically as it discovers new coverage.

Corpus files live in `fuzz/corpus/transpile_rust/`. Add interesting programs there to improve coverage.

## What is being checked

Each iteration:
1. Raw bytes from libFuzzer are fed to the grammar-guided generator (`fuzz/src/generator.rs`)
   to produce a syntactically valid MVL source string.
2. The source is parsed with the standard error-tolerant `Parser`.
3. `transpile()` is called on the parsed AST.
4. **Assertion**: no panic, non-empty Rust output.

A panic from the parser or transpiler on any grammar-valid input is a bug.

## Triaging a crash

When cargo-fuzz finds a crash it writes an artifact to `fuzz/artifacts/transpile_rust/`:

```bash
# Reproduce the crash
cargo +nightly fuzz run transpile_rust fuzz/artifacts/transpile_rust/crash-<hash>

# Minimize the crashing input to smallest reproducer
cargo +nightly fuzz tmin transpile_rust fuzz/artifacts/transpile_rust/crash-<hash>
```

After minimization, inspect the minimized file to understand the crashing program,
then file a bug ticket with the minimized MVL program attached.

## Checking coverage

```bash
cargo +nightly fuzz coverage transpile_rust
# Report is written to fuzz/coverage/transpile_rust/
```

## Corpus minimization

After a long run, the corpus can grow large with redundant entries:

```bash
cargo +nightly fuzz cmin transpile_rust
```
