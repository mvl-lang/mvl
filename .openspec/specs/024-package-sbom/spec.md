---
domain: toolchain
version: 0.1.0
status: accepted
date: 2026-05-29
---

# 024 — Package Management and SBOM

Package distribution, dependency resolution, and supply chain visibility
for MVL projects. Implements ADR-0039 Phase A.

## Requirements

### Requirement 1: Package Identity via Git URL [MUST]

Packages MUST be identified by their full git URL (https or ssh) plus an optional
version tag. Short `github.com/user/repo` identifiers MUST expand to the full
`https://` URL.

**Implementation:** `src/mvl/packages/fetch.rs`

#### Scenario: Add a package by full URL

- GIVEN a project directory containing `mvl.toml`
- WHEN `mvl add https://github.com/lab271/mvl_sqlite v0.3.0` is run
- THEN `mvl.toml` MUST gain a `[dependencies]` entry with `git` and `tag` fields
- AND `mvl.lock` MUST be created or updated with `name`, `version`, `git`, `tag`, `commit`, `hash`

#### Scenario: Add a package by short identifier

- GIVEN `mvl add github.com/lab271/mvl_sqlite v0.3.0`
- WHEN the command resolves the identifier
- THEN it MUST expand to `https://github.com/lab271/mvl_sqlite`

**Tests:** `src/mvl/packages/tests.rs`

---

### Requirement 2: Lock File Integrity [MUST]

`mvl install` MUST verify the SHA-256 hash recorded in `mvl.lock` against the
downloaded archive before extracting any package. A hash mismatch MUST be a hard
error (non-zero exit, no partial install).

**Implementation:** `src/mvl/packages/fetch.rs`, `src/mvl/packages/hash.rs`, `src/mvl/packages.rs::cmd_install`

#### Scenario: Hash mismatch on install

- GIVEN `mvl.lock` records `hash = "sha256:abc..."` for a package
- WHEN the downloaded archive has a different hash
- THEN `mvl install` MUST exit with an error message including the package name and both hashes
- AND no files from that package MUST be written to the toolchain directory

---

### Requirement 3: Deterministic Lock File [MUST]

`mvl.lock` MUST record the exact git commit SHA and archive hash for every resolved
dependency, ensuring reproducible installs across machines and CI runs.

**Implementation:** `src/mvl/packages.rs::cmd_add`, `src/mvl/packages.rs::cmd_update`

---

### Requirement 4: SBOM Generation [MUST]

`mvl sbom` MUST generate a software bill of materials from `mvl.toml` + `mvl.lock`.
It MUST NOT require external network access.

**Implementation:** `src/mvl/packages/sbom.rs`, `src/mvl/packages.rs::cmd_sbom`, `src/cli/meta.rs::cmd_sbom`

#### Scenario: CycloneDX output (default)

- GIVEN a project with `mvl.toml` and `mvl.lock`
- WHEN `mvl sbom` is run
- THEN output MUST be valid CycloneDX 1.5 JSON
- AND the `serialNumber` field MUST be a deterministic UUID-like value
- AND each dependency MUST appear as a `component` with `type`, `name`, `version`, `purl`, `hashes`, and `externalReferences`

#### Scenario: SPDX output

- GIVEN a project with `mvl.toml` and `mvl.lock`
- WHEN `mvl sbom --format=spdx` is run
- THEN output MUST be valid SPDX 2.3 tag-value format
- AND `DataLicense: CC0-1.0` MUST appear
- AND each dependency MUST have a `PackageName`, `SPDXID`, `PackageVersion`, and `PrimaryPackagePurpose`

#### Scenario: SBOM outside a project directory

- GIVEN a directory with no `mvl.toml`
- WHEN `mvl sbom` is run
- THEN it MUST exit with a non-zero code
- AND the error MUST include the hint `run 'mvl sbom' from a project directory containing mvl.toml`

**Tests:** `src/mvl/packages/sbom.rs` (inline unit tests — 23 tests)

---

### Requirement 5: Component Type Detection [MUST]

The SBOM root component type MUST reflect the project's intended use:

| Condition | CycloneDX `type` | SPDX `PrimaryPackagePurpose` |
|-----------|-----------------|------------------------------|
| `main.mvl` or `src/main.mvl` exists | `application` | `APPLICATION` |
| neither file exists | `library` | `LIBRARY` |

**Implementation:** `src/mvl/packages.rs::cmd_sbom` (detection), `src/mvl/packages/sbom.rs::ComponentType`

#### Scenario: Application project

- GIVEN a project with `src/main.mvl`
- WHEN `mvl sbom` is run
- THEN the root component MUST have `"type": "application"` (CycloneDX) or `PrimaryPackagePurpose: APPLICATION` (SPDX)

#### Scenario: Library project

- GIVEN a project with no `main.mvl` and no `src/main.mvl`
- WHEN `mvl sbom` is run
- THEN the root component MUST have `"type": "library"` (CycloneDX) or `PrimaryPackagePurpose: LIBRARY` (SPDX)

**Tests:** `src/mvl/packages/sbom.rs::test_cyclonedx_application`, `::test_spdx_application`

---

### Requirement 6: SBOM File Output [SHOULD]

`mvl sbom --output=<file>` SHOULD write the SBOM to the specified file instead of
stdout, and SHOULD print a confirmation message to stdout.

**Implementation:** `src/cli/meta.rs::cmd_sbom`

#### Scenario: Write to file

- GIVEN `mvl sbom --output=sbom.json`
- WHEN the command completes successfully
- THEN `sbom.json` MUST contain the SBOM content
- AND stdout MUST contain the message `SBOM written to sbom.json`

---

### Requirement 7: Project Scaffolding [MUST]

`mvl init [<name>]` MUST create a minimal project skeleton in the current directory.
If `mvl.toml` already exists, it MUST exit with a non-zero code and a descriptive error.

**Implementation:** `src/cli/meta.rs::cmd_init`

#### Scenario: Fresh init

- GIVEN an empty directory named `myapp`
- WHEN `mvl init` is run (no name argument)
- THEN `mvl.toml` MUST be created with `name = "myapp"`
- AND `src/main.mvl` MUST be created with a `fn main() -> Unit ! Console` entry point

#### Scenario: Init with explicit name

- GIVEN an empty directory
- WHEN `mvl init awesome-lib` is run
- THEN `mvl.toml` MUST have `name = "awesome-lib"`

#### Scenario: Init in existing project

- GIVEN a directory already containing `mvl.toml`
- WHEN `mvl init` is run
- THEN it MUST exit with an error and suggest `mvl add` for dependency management

---

### Requirement 8: Stdlib Extraction [MUST]

`mvl self init` MUST extract the bundled stdlib to the XDG toolchain directory
without overwriting the project's `mvl.toml`. This MUST be separate from `mvl init`.

**Implementation:** `src/cli/meta.rs::cmd_self_init`, `src/mvl/stdlib.rs`

#### Scenario: Stdlib not yet extracted

- GIVEN the toolchain stdlib directory does not exist
- WHEN `mvl self init` is run
- THEN the stdlib MUST be extracted to `$XDG_DATA_HOME/mvl/toolchains/<version>/std/`
- AND stdout MUST print `mvl stdlib v<version> ready at <path>`

---

### Requirement 9: SBOM Help Flag [SHOULD]

`mvl sbom --help` SHOULD print usage information including available formats and
the `--output` option, then exit with code 0 (no error).

**Implementation:** `src/cli/meta.rs::cmd_sbom`

---

### Requirement 10: Source File Hash Model [MUST]

A shared SHA-256 hashing module MUST provide the cryptographic primitives used by
lock file verification (Req 2), SBOM generation (Req 4), and runtime manifest
source digest embedding. The hash model defines normalization rules for
deterministic, cross-platform hashing. (#1245)

**Implementation:** `src/mvl/packages/hash.rs::sha256_hex`, `src/mvl/packages/hash.rs::sha256_file`, `src/mvl/packages/hash.rs::sha256_source_tree`

#### Scenario: Per-file hash uses raw bytes

- GIVEN a `.mvl` source file on disk
- WHEN `sha256_file(path)` is called
- THEN the result MUST be the SHA-256 of the raw file bytes (no line-ending normalization)
- AND the result MUST be prefixed with `"sha256:"` and use lowercase hex

#### Scenario: Source tree digest is order-independent

- GIVEN a set of `(relative_path, file_sha256)` pairs in any order
- WHEN `sha256_source_tree(files)` is called
- THEN the result MUST be identical regardless of input order
- AND the hash input per entry MUST be `"<rel_path>:<file_sha256>\n"` with paths sorted lexicographically

#### Scenario: Known test vectors

- GIVEN the empty string `""`
- WHEN `sha256_hex(b"")` is called
- THEN the result MUST be `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`

**Tests:** `src/mvl/packages/hash.rs::tests::sha256_hex_empty`, `::sha256_hex_hello`, `::sha256_file_round_trip`, `::sha256_source_tree_sorted_deterministic`, `::sha256_source_tree_differs_on_content_change`, `::sha256_source_tree_differs_on_path_change`, `::sha256_source_tree_empty_returns_sha256_prefix`
