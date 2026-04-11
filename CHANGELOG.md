# Changelog

All notable changes to the MVL language and compiler will be documented in this file.

Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Project structure: `src/mvl/{parser,checker,transpiler}`, test hierarchy
- OpenSpec: 3 specs (type system, effect system, IFC), 5 ADRs
- EBNF grammar (~100 productions, LL(1))
- Standard library specification (three tiers: core, standard, extended)
- Language reference and introduction documentation
- mkdocs site with Material theme
- Two corpus examples: auth_handler.mvl, safe_division.mvl
- 34 GitHub issues across 5 epics (Phase 1: Rust transpilation)
- Tree-sitter grammar story (#35)

## [0.1.0] — TBD (Phase 1 complete)

Target: both corpus examples compile via Rust transpilation, all 11 requirements demonstrated.

## [0.2.0] — TBD (Phase 2 complete)

Target: LLVM IR backend, self-hosting.

## [0.3.0] — TBD (Phase 3 complete)

Target: MVL compiler written in MVL, ecosystem (package manager, tooling).
