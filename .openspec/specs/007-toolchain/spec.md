# Spec 007: Toolchain Layout and Version Management

**ADR:** [0009 — Toolchain Layout](../adr/0009-toolchain-layout.md)

---

### Requirement 1: XDG-Compliant Paths [MUST]

The MVL toolchain MUST store data, config, and cache in XDG-compliant locations. `$MVL_HOME` overrides all XDG paths.

**Implementation:** `src/mvl/toolchain/paths.rs` (planned)

#### Scenario: Default XDG paths used when no env vars set

- GIVEN no `$MVL_HOME`, `$XDG_DATA_HOME`, `$XDG_CONFIG_HOME`, or `$XDG_CACHE_HOME` is set
- WHEN the compiler resolves toolchain paths
- THEN data is at `~/.local/share/mvl/`
- AND config is at `~/.config/mvl/`
- AND cache is at `~/.cache/mvl/`

**Tests:** `tests/toolchain.rs::xdg_defaults` (planned)

#### Scenario: MVL_HOME overrides all paths

- GIVEN `$MVL_HOME=/opt/mvl` is set
- WHEN the compiler resolves toolchain paths
- THEN data is at `/opt/mvl/toolchains/`, `/opt/mvl/pkg/`
- AND config is at `/opt/mvl/config.toml`
- AND cache is at `/opt/mvl/cache/`

**Tests:** `tests/toolchain.rs::mvl_home_override` (planned)

---

### Requirement 2: Multiple Toolchain Versions [MUST]

Multiple MVL compiler versions MUST coexist in `$XDG_DATA_HOME/mvl/toolchains/{version}/`. Each toolchain contains its compiler binary and stdlib source.

**Implementation:** `src/mvl/toolchain/install.rs` (planned)

#### Scenario: Install a new toolchain version

- GIVEN version 0.21.0 is not installed
- WHEN the user runs `mvl self install 0.21.0`
- THEN `$XDG_DATA_HOME/mvl/toolchains/0.21.0/bin/mvl` exists
- AND `$XDG_DATA_HOME/mvl/toolchains/0.21.0/std/core.mvl` exists
- AND `$XDG_DATA_HOME/mvl/toolchains/0.21.0/std/.version` contains `0.21.0`
- AND `~/.local/bin/mvl@0.21.0` is a symlink to the installed binary

**Tests:** `tests/toolchain.rs::install_creates_dirs_and_symlink` (planned)

#### Scenario: Two versions coexist

- GIVEN versions 0.19.0 and 0.20.0 are both installed
- WHEN the user runs `mvl@0.19.0 --version`
- THEN it outputs `mvl 0.19.0`
- AND `mvl@0.20.0 --version` outputs `mvl 0.20.0`

**Tests:** `tests/toolchain.rs::versions_coexist` (planned)

---

### Requirement 3: Version Resolution [MUST]

When `mvl` is invoked, it MUST resolve the toolchain version in this order: CLI flag > `.mvl-version` (project) > `mvl.toml` > `.mvl-version` (global) > bare symlink.

**Implementation:** `src/mvl/toolchain/resolve.rs`

#### Scenario: Project pin overrides global default

- GIVEN global `.mvl-version` is `0.19.0`
- AND project `.mvl-version` is `0.20.0`
- WHEN `mvl run main.mvl` is invoked in the project
- THEN the 0.20.0 toolchain is used

**Tests:** `src/mvl/toolchain/resolve.rs::tests::project_version_found_in_cwd`

#### Scenario: CLI flag overrides everything

- GIVEN project `.mvl-version` is `0.20.0`
- WHEN `mvl@0.19.0 run main.mvl` is invoked
- THEN the 0.19.0 toolchain is used

**Tests:** `src/mvl/toolchain/resolve.rs::tests::argv0_wins_over_project_file`

---

### Requirement 4: Immutable Stdlib per Version [MUST]

Each toolchain's `std/` directory MUST be immutable after installation. The compiler MUST reject a stdlib whose `.version` file does not match the compiler version.

**Implementation:** `src/mvl/toolchain/stdlib.rs` (planned)

#### Scenario: Stdlib version matches compiler

- GIVEN toolchain 0.20.0 is installed with `std/.version` = `0.20.0`
- WHEN the compiler loads the stdlib
- THEN compilation proceeds normally

**Tests:** `tests/toolchain.rs::stdlib_version_match` (planned)

#### Scenario: Stdlib version mismatch

- GIVEN toolchain 0.20.0 is installed but `std/.version` was tampered to `0.19.0`
- WHEN the compiler loads the stdlib
- THEN compilation fails with error: "stdlib version mismatch: compiler 0.20.0, stdlib 0.19.0"

**Tests:** `tests/toolchain.rs::stdlib_version_mismatch_error` (planned)

---

### Requirement 5: Shared Cargo Cache [MUST]

All MVL versions MUST share a single Cargo registry cache at `$XDG_CACHE_HOME/mvl/cargo/`. Crate downloads happen once. Build artifacts are per-project in `.mvl/target/`.

**Implementation:** `src/mvl/transpiler/cargo.rs` (existing, needs `CARGO_HOME` override)

#### Scenario: Two projects share Cargo downloads

- GIVEN project A and project B both depend on `serde 1.0`
- WHEN project A builds first, then project B builds
- THEN `serde` is downloaded once (in the shared cache)
- AND each project has its own `.mvl/target/` with compiled artifacts

**Tests:** `tests/toolchain.rs::shared_cargo_cache` (planned)

---

### Requirement 6: Project-Local .mvl/ Directory [MUST]

Each project MUST store build artifacts in a `.mvl/` directory at the project root. This directory is disposable and gitignored by default.

**Implementation:** `src/mvl/transpiler/cargo.rs` (needs CARGO_TARGET_DIR override)

#### Scenario: mvl build creates .mvl/ directory

- GIVEN a project with `mvl.toml` and `src/main.mvl`
- WHEN `mvl build` is invoked
- THEN `.mvl/build/` contains transpiled Rust source
- AND `.mvl/target/` contains Cargo compilation artifacts

**Tests:** `tests/compile_and_run.rs::build_creates_mvl_dir` (planned)

#### Scenario: mvl clean removes build artifacts

- GIVEN a project with `.mvl/build/` and `.mvl/target/` populated
- WHEN `mvl clean` is invoked
- THEN `.mvl/build/` and `.mvl/target/` are removed
- AND `.mvl/pkg/` is preserved (if it exists)

**Tests:** `tests/toolchain.rs::clean_removes_build_preserves_pkg` (planned)

---

### Requirement 7: Source Resolution from Multiple Locations [MUST]

`use` statements MUST resolve from three locations based on prefix: project root (no prefix), stdlib (`std.*`), packages (`pkg.*`).

**Implementation:** `src/mvl/resolver/mod.rs` (existing, needs filesystem integration)

#### Scenario: use std.fs resolves from toolchain stdlib

- GIVEN toolchain 0.20.0 is active with `std/fs.mvl` present
- WHEN a source file contains `use std.fs`
- THEN the resolver loads `$XDG_DATA_HOME/mvl/toolchains/0.20.0/std/fs.mvl`

**Tests:** `tests/resolver.rs::use_std_resolves_from_toolchain` (planned)

#### Scenario: use mylib resolves from project root

- GIVEN a project with `src/mylib.mvl`
- WHEN a source file contains `use mylib`
- THEN the resolver loads `./src/mylib.mvl`

**Tests:** `tests/resolver.rs::use_local_resolves_from_project` (planned)

---

### Requirement 8: Precompiled Modules — Phase 4 [SHOULD]

In Phase 4, the compiler SHOULD support `.mvlo` precompiled modules containing LLVM bitcode and verified metadata. Consumers verify requirements from metadata without needing source.

**Implementation:** `src/mvl/compiler/mvlo.rs` (Phase 4, planned)

#### Scenario: Compile a library to .mvlo

- GIVEN a library `lib.mvl` with public functions
- WHEN `mvl build --lib` is invoked
- THEN `lib.mvlo` is produced containing LLVM bitcode and metadata
- AND metadata includes: type signatures, effect annotations, IFC labels, trust score

**Tests:** `tests/compiler.rs::build_lib_produces_mvlo` (Phase 4, planned)

#### Scenario: Link against .mvlo without source

- GIVEN `lib.mvlo` exists with metadata showing 11/11 trust score
- WHEN a project uses `use pkg.lib` and `mvl build` is invoked
- THEN the compiler type-checks against `.mvlo` metadata
- AND links against `.mvlo` bitcode
- AND does NOT require `lib.mvl` source

**Tests:** `tests/compiler.rs::link_mvlo_without_source` (Phase 4, planned)

---

### Requirement 9: Trust Reporting at Link Time [SHOULD]

`mvl audit` SHOULD report the trust composition of the final binary: percentage of MVL-verified code vs extern vs foreign.

**Implementation:** `src/mvl/audit/mod.rs` (planned)

#### Scenario: Audit a project with mixed trust levels

- GIVEN a project using MVL stdlib (11/11), one MVL package with extern (9/11), and one Rust crate via FFI
- WHEN `mvl audit` is invoked
- THEN the output shows percentage breakdown by trust tier
- AND lists CVE exposure for foreign dependencies (if #151 is implemented)

**Tests:** `tests/audit.rs::trust_report_mixed_project` (planned)
