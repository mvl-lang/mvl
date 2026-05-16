---
domain: build
version: 0.1.0
status: accepted
date: 2026-04-12
---

# 006 — Trust Boundary Bridge

The trust boundary bridge specification defines the `bridge.rs` convention: how MVL programs with `extern "rust"` blocks link to verified Rust implementations at build time. This is the Phase 2 mechanism that connects the MVL-verified world to the Rust ecosystem.

> **Origin:** Issue #121 / PR #122. Closes Phase 2 milestone (#92): a real MVL program that calls Rust crates via `extern "rust"` and runs end-to-end.

## Philosophy

`extern "rust"` is the explicit trust boundary. MVL verifies everything on its side of the boundary; Rust code on the other side is trusted but unverified. The smaller the bridge, the larger the verified fraction of the program.

The bridge convention makes this boundary greppable, auditable, and enforced at build time:
- One file per MVL source (`bridge.rs`, sibling to `main.mvl`)
- Declared in the MVL source via `extern "rust" { }` blocks
- Automatically linked by `mvl build` / `mvl run`
- Missing bridge → clear compile-time error (no silent linking failures)

**Assurance principle:** `extern_count` (number of extern blocks) measures the trust surface. A program with zero extern blocks is fully MVL-verified. Adding a bridge is an explicit, visible choice.

## Requirements

### Requirement 1: Bridge Discovery [MUST]

`mvl build` and `mvl run` MUST look for a `bridge.rs` file in the **same directory** as the input `.mvl` file. The file MUST be named exactly `bridge.rs`.

**Implementation:** `src/main.rs::build_project`

#### Scenario: Bridge found

- GIVEN `examples/log_analyzer/main.mvl` with an `extern "rust"` block
- AND `examples/log_analyzer/bridge.rs` exists
- WHEN `mvl build examples/log_analyzer/main.mvl` is run
- THEN `bridge.rs` is copied into the generated crate's `src/`

#### Scenario: Directory input

- GIVEN a directory `examples/log_analyzer/` passed to `mvl build`
- AND `examples/log_analyzer/bridge.rs` exists alongside `main.mvl`
- WHEN `mvl build examples/log_analyzer/` is run
- THEN bridge discovery uses the directory of the resolved entry file (`main.mvl`)

### Requirement 2: Bridge Injection [MUST]

When `bridge.rs` is found, `mvl build` MUST inject `mod bridge;` into the generated `main.rs` (or `lib.rs` for library crates) **immediately after** the `use mvl_runtime::prelude::*;` import line.

**Implementation:** `src/main.rs::inject_mod_bridge`, `src/main.rs::build_project`

#### Scenario: mod bridge; placement

- GIVEN a program with `extern "rust"` and a valid `bridge.rs`
- WHEN `mvl build` transpiles the program
- THEN the generated `src/main.rs` contains `mod bridge;` on the line following `use mvl_runtime::prelude::*;`

#### Scenario: Bridge module visible to extern declarations

- GIVEN the generated `src/main.rs` contains both `mod bridge;` and `extern "Rust" { fn foo(); }`
- WHEN `cargo build` is run on the generated crate
- THEN the extern symbol `foo` resolves to `bridge::foo` via the `#[no_mangle]` export in `bridge.rs`

### Requirement 3: Missing Bridge Error [MUST]

If a program declares `extern "rust"` blocks but no `bridge.rs` is found in the source directory, `mvl build` MUST emit a clear, actionable error and exit non-zero. It MUST NOT attempt cargo build.

**Implementation:** `src/main.rs::build_project`

**Tests:** `tests/compile_and_run.rs` (implicit: any corpus file with `extern "rust"` and no bridge.rs)

#### Scenario: Missing bridge.rs

- GIVEN `tests/corpus/09_full_programs/password_checker.mvl` declares `extern "rust"`
- AND no `bridge.rs` exists in `tests/corpus/09_full_programs/`
- WHEN `mvl build tests/corpus/09_full_programs/password_checker.mvl` is run
- THEN the error message MUST include the source file path and the expected bridge.rs location
- AND the process MUST exit with a non-zero status

#### Scenario: Error message content

- GIVEN a missing bridge.rs scenario (above)
- THEN the error MUST contain the text `extern "rust"` to make the cause greppable
- AND MUST suggest where to place `bridge.rs`

### Requirement 4: Non-Extern Programs Unaffected [MUST]

`mvl build` and `mvl run` MUST behave identically to the pre-bridge behaviour when the program has no `extern "rust"` blocks. The presence of a `bridge.rs` in the same directory MUST be silently ignored.

**Implementation:** `src/main.rs::build_project` (guarded by `out.has_extern_rust`)

#### Scenario: No extern rust — no bridge lookup

- GIVEN `tests/corpus/09_full_programs/hello_world.mvl` with no `extern "rust"` blocks
- WHEN `mvl build tests/corpus/09_full_programs/hello_world.mvl` is run
- THEN the build MUST succeed without looking for or copying any `bridge.rs`

#### Scenario: Bridge file present but not needed

- GIVEN a program with no `extern "rust"` blocks
- AND a `bridge.rs` happens to exist in the same directory
- WHEN `mvl build` is run
- THEN `bridge.rs` is NOT copied into the generated crate
- AND `mod bridge;` is NOT injected into the generated source

### Requirement 5: Bridge File Convention [SHOULD]

A `bridge.rs` file SHOULD follow these conventions to integrate correctly with the generated crate:

1. `use mvl_runtime::prelude::*;` at the top (for access to `Clean[T]`, `Tainted[T]`, etc.)
2. Each function declared in the MVL `extern "rust"` block MUST have a matching `pub extern "Rust" fn` with `#[no_mangle]`
3. The function signatures MUST match exactly the types emitted by the MVL transpiler

**Implementation:** `examples/log_analyzer/bridge.rs`

#### Scenario: Bridge function signature alignment

- GIVEN MVL declares `extern "rust" { fn read_log_file(path: String) -> Tainted[String]; }`
- THEN the transpiler emits `extern "Rust" { fn read_log_file(path: String) -> Tainted[String]; }`
- AND `bridge.rs` provides `#[no_mangle] pub extern "Rust" fn read_log_file(path: String) -> Tainted[String]`
- WHEN `cargo build` links the generated crate
- THEN the extern symbol resolves without linker errors

### Requirement 6: has_extern_rust Transpiler Flag [MUST]

The transpiler MUST expose a `has_extern_rust: bool` field on `TranspileOutput` that is `true` iff the program contains at least one `extern "rust"` block. Build tooling MUST use this flag (not `extern_count`) for bridge decisions.

**Implementation:** `src/mvl/backends/rust.rs::TranspileOutput`, `src/mvl/backends/rust.rs::has_extern_rust_decls`

> **Rationale:** `extern_count` counts all ABI blocks (`"rust"` and `"c"`). Only `extern "rust"` blocks require a bridge. The dedicated flag is precise and audit-friendly.

#### Scenario: Rust-only extern sets flag

- GIVEN a program with `extern "rust" { fn foo(); }`
- WHEN transpiled
- THEN `has_extern_rust` is `true`

#### Scenario: C extern does not set flag

- GIVEN a program with only `extern "c" { fn bar(); }`
- WHEN transpiled
- THEN `has_extern_rust` is `false`
- AND bridge discovery is NOT triggered

## Reference: examples/log_analyzer

The canonical example demonstrating the full convention:

```
examples/log_analyzer/
  main.mvl        ← MVL source; declares extern "rust" trust boundary
  bridge.rs       ← Rust implementations; ships with the example
  Makefile        ← make run creates sample.log and runs end-to-end
```

IFC flow in the example:
```
CLI args / filesystem (Rust, unverified)
    │
    │  extern "rust" boundary
    ▼
Tainted[String]   ← read_log_file() returns raw file contents
    │
    │  sanitize()  (MVL built-in, explicit lattice step)
    ▼
Clean[String]     ← count_and_format() receives verified-clean content
    │
    ▼
String            ← returned to fn main, printed to stdout
```

**Tests:** `make run` in `examples/log_analyzer/` (end-to-end smoke test)
