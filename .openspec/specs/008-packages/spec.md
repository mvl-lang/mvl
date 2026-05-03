# Spec 008: Extended Package Model

**ADR:** [0012 — Extended Package Model](../adr/0012-extended-package-model.md)

---

### Requirement 1: Package Manifest [MUST]

Every extended package MUST have a `mvl.toml` in its root with `[package]` fields: `name`, `version` (semver), `license`, and `requires-mvl` (version range). Packages containing any `extern "rust"` block MUST also include `extern-rationale`.

**Implementation:** `src/mvl/packages/manifest.rs` (planned)

#### Scenario: Valid minimal manifest accepted

- GIVEN a package directory with `mvl.toml` containing `name`, `version`, `license`, `requires-mvl`
- WHEN `mvl build` reads the manifest
- THEN it is parsed without error

**Tests:** `tests/packages.rs::manifest_valid_minimal` (planned)

#### Scenario: Missing extern-rationale rejected

- GIVEN a package that contains an `extern "rust"` block
- AND its `mvl.toml` has no `extern-rationale` field
- WHEN `mvl build` validates the manifest
- THEN it emits error `E700: extern-rationale required when extern blocks are present`
- AND the build fails

**Tests:** `tests/packages.rs::manifest_extern_rationale_required` (planned)

#### Scenario: Incompatible requires-mvl rejected

- GIVEN a package with `requires-mvl = ">=0.30.0"`
- AND the current compiler is version `0.24.0`
- WHEN `mvl build` resolves the dependency
- THEN it emits error `E701: package 'http' requires mvl >=0.30.0, current is 0.24.0`

**Tests:** `tests/packages.rs::manifest_version_incompatible` (planned)

---

### Requirement 2: Internal Directory Boundary [MUST]

The compiler MUST enforce that files under `src/internal/` within a package are inaccessible from outside the package. A `use pkg.NAME.internal.*` import from user code MUST be rejected at resolve time.

**Implementation:** `src/mvl/resolver/mod.rs` (planned — add `internal/` path check)

#### Scenario: Internal module not importable from user code

- GIVEN package `http` with `src/internal/ffi.mvl`
- AND user code contains `use pkg.http.internal.ffi.{connect}`
- WHEN the resolver processes the import
- THEN it emits error `E702: 'pkg.http.internal' is not part of the public API`
- AND the build fails

**Tests:** `tests/packages.rs::internal_not_importable` (planned)

#### Scenario: Internal module usable within the package

- GIVEN package `http` with `src/internal/ffi.mvl` containing `pub fn raw_connect(...)`
- AND `src/server.mvl` (within same package) contains `use internal.ffi.{raw_connect}`
- WHEN the resolver processes the import
- THEN it resolves successfully

**Tests:** `tests/packages.rs::internal_usable_within_package` (planned)

---

### Requirement 3: Package Resolution [MUST]

`use pkg.NAME` MUST resolve by searching in order:
1. `.mvl/pkg/<name>/` (project-local override)
2. `$XDG_DATA_HOME/mvl/pkg/<name>/<version>/` (global cache)

The resolved version MUST match the version pinned in `mvl.lock`.

**Implementation:** `src/mvl/resolver/mod.rs` (planned — third resolution path)

#### Scenario: pkg.* resolves to global cache

- GIVEN `mvl.lock` pins `http = "1.2.0"`
- AND `$XDG_DATA_HOME/mvl/pkg/http/1.2.0/src/server.mvl` exists
- AND no `.mvl/pkg/http/` override exists
- WHEN `use pkg.http.{Server}` is resolved
- THEN it resolves to the cached package source

**Tests:** `tests/packages.rs::pkg_resolves_global_cache` (planned)

#### Scenario: Local override takes precedence

- GIVEN `mvl.lock` pins `http = "1.2.0"`
- AND `.mvl/pkg/http/src/server.mvl` exists (local override)
- AND global cache also contains `http 1.2.0`
- WHEN `use pkg.http.{Server}` is resolved
- THEN it resolves to the local override

**Tests:** `tests/packages.rs::pkg_local_override_wins` (planned)

---

### Requirement 4: Lock File [MUST]

`mvl build` MUST generate `mvl.lock` on first build, pinning exact versions and checksums for all transitive dependencies. `mvl build --locked` MUST fail if `mvl.lock` is absent or stale.

**Implementation:** `src/mvl/packages/lock.rs` (planned)

#### Scenario: Lock file generated on first build

- GIVEN `mvl.toml` declares a dependency on `http`
- AND no `mvl.lock` exists
- WHEN `mvl build` runs
- THEN `mvl.lock` is created with pinned version and sha256 checksum for `http`

**Tests:** `tests/packages.rs::lock_generated_on_first_build` (planned)

#### Scenario: --locked fails when lock is stale

- GIVEN `mvl.lock` pins `http = "1.1.0"`
- AND `mvl.toml` now requires `http = "1.2.0"`
- WHEN `mvl build --locked` runs
- THEN it emits error `E703: mvl.lock is out of date; run 'mvl build' to update`

**Tests:** `tests/packages.rs::locked_flag_rejects_stale_lock` (planned)

---

### Requirement 5: Trust Score in Audit Output [MUST]

`mvl audit` MUST display per-package trust scores: MVL verified lines, extern lines, and the computed ratio. `extern-rationale` MUST be printed for any package where extern lines > 0.

**Implementation:** `src/mvl/audit/trust.rs` (planned)

#### Scenario: Audit shows trust scores for all dependencies

- GIVEN a project with dependencies `mvl_http` (312 extern, 1840 MVL lines) and `mvl_tls` (89 extern, 430 MVL lines)
- WHEN `mvl audit` runs
- THEN output includes trust scores: `mvl_http 85.5%`, `mvl_tls 82.8%`
- AND `extern-rationale` text is printed for both packages

**Tests:** `tests/packages.rs::audit_shows_trust_scores` (planned)

#### Scenario: Audit --sbom emits CycloneDX JSON

- GIVEN a project with at least one dependency
- WHEN `mvl audit --sbom` runs
- THEN it writes a CycloneDX JSON file to `mvl-sbom.json`
- AND the file includes `components` entries for all MVL and native (Cargo) dependencies with their version, license, and extern-line count

**Tests:** `tests/packages.rs::audit_sbom_cyclonedx` (planned)

---

### Requirement 6: Semver Enforcement [MUST]

`version` in `mvl.toml` MUST parse as valid semver. `mvl build` MUST reject a dependency declaration if the resolved version does not satisfy the declared constraint.

**Implementation:** `src/mvl/packages/version.rs` (planned)

#### Scenario: Version satisfying constraint accepted

- GIVEN `mvl.toml` declares `http = ">=1.0.0, <2.0.0"`
- AND the latest available version is `1.2.0`
- WHEN `mvl build` resolves the dependency
- THEN `1.2.0` is selected and written to `mvl.lock`

**Tests:** `tests/packages.rs::version_constraint_satisfied` (planned)

#### Scenario: No satisfying version fails build

- GIVEN `mvl.toml` declares `http = ">=2.0.0"`
- AND the only available version is `1.2.0`
- WHEN `mvl build` resolves the dependency
- THEN it emits error `E704: no version of 'http' satisfies >=2.0.0 (latest: 1.2.0)`

**Tests:** `tests/packages.rs::version_constraint_unsatisfied` (planned)

---

### Requirement 7: Build Fetches Dependencies [MUST]

`mvl build` MUST automatically fetch declared dependencies before transpilation if they are absent from the local cache. No separate `mvl install` command is required.

**Implementation:** `src/mvl/packages/fetch.rs` (planned), `src/main.rs` (build orchestration)

#### Scenario: Missing dependency fetched automatically

- GIVEN `mvl.toml` declares `http = { git = "...", tag = "v1.2.0" }`
- AND the package is not in `$XDG_DATA_HOME/mvl/pkg/http/`
- WHEN `mvl build` runs
- THEN the package is fetched from the git URL before transpilation
- AND the build completes successfully

**Tests:** `tests/packages.rs::build_fetches_missing_dep` (planned)

#### Scenario: Package source included in transpilation

- GIVEN a project that imports `use pkg.http.{Server}`
- AND the package source is present in cache
- WHEN `mvl build` transpiles
- THEN the package's MVL source is included in the generated Rust crate
- AND the package's `bridge.rs` (if present) is injected alongside the project's bridge

**Tests:** `tests/packages.rs::package_source_transpiled` (planned)
