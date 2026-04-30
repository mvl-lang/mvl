# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.54.0] — 2026-04-30

### Added

- **Rust backing for std/crypto stdlib (closes #349)** — Real implementations for `sha256`, `sha512`, and `crypto_random_bytes` in `mvl_runtime/src/stdlib/crypto.rs` using `sha2` and `hex` crates. CSPRNG uses `getrandom` for cross-platform support (Unix, Windows, WASI). Includes 11 comprehensive unit tests: NIST vectors for SHA-256/512 (empty and "abc"), determinism, output format, and randomness uniqueness.

### Changed

- **Removed std/tui stdlib (closes #349)** — TUI module deleted from stdlib; it belongs in userspace, not the language's core stdlib. The `Terminal` effect marker remains a valid language-level concept for programs that interact with raw terminal control. Aligned with stdlib scope decisions in #217.
- `mvl_runtime/Cargo.toml` — added `getrandom = "0.2"` alongside `sha2 = "0.10"` and `hex = "0.4"`.

### Fixed

- **CSPRNG security hardening** — Replaced `/dev/urandom` direct open with `getrandom` crate: now panics on CSPRNG unavailability (unrecoverable failure) instead of silently returning zero-filled bytes. Cross-platform support on Unix, Windows, WASI, and beyond.

## [0.53.0] — 2026-04-29

### Added

- **Boundary value analysis for mutation testing (closes #331)** — New `mvl mutate --gen-boundary` flag prints a targeted report identifying surviving `IntLiteral` and comparison-operator mutants that can be killed with boundary value tests. For each survivor, shows the field name extracted from source, the exact kill value that distinguishes the original threshold from the mutant, and N-1/N/N+1 boundary sweep hints. Phase 1 (IntLiteral mutants) fully implemented; Phase 2 (comparison operator mutants) fully implemented.

### Fixed

- **Stdlib test accuracy and coverage (closes #342)** — Corrected test documentation for real implementations (`get_arg`, `get_env`, `get_args`) mischaracterized as Phase 2 stubs. Removed 11 redundant/duplicate tests from args, io, and log suites with no coverage loss. Fixed empty-base join comment to document Rust runtime vs MVL source divergence. Added STUB markers to all vacuous tests. Standardized log section headers.

## [0.52.0] — 2026-04-29
