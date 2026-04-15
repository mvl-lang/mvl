# ADR-0009: Toolchain Layout — XDG, Versioning, Linking, Caches

**Status:** Accepted
**Date:** 2026-04-15 (condensed from ADR-0008, 0009, 0011)
**Context:** MVL needs a complete toolchain management story: where files live, how multiple compiler versions coexist, how stdlib is delivered, how dependencies are cached, and how precompiled libraries work. This ADR consolidates all file-layout and toolchain decisions into one place.

## Decision

### 1. XDG Compliance

All paths follow the XDG Base Directory Specification. `$MVL_HOME` overrides everything for CI/containers.

| Purpose | XDG variable | MVL path |
|---|---|---|
| Toolchains + packages | `$XDG_DATA_HOME` | `$XDG_DATA_HOME/mvl/toolchains/`, `$XDG_DATA_HOME/mvl/pkg/` |
| Config | `$XDG_CONFIG_HOME` | `$XDG_CONFIG_HOME/mvl/config.toml` |
| Build cache + Cargo | `$XDG_CACHE_HOME` | `$XDG_CACHE_HOME/mvl/build/`, `$XDG_CACHE_HOME/mvl/cargo/` |

### 2. Generational Toolchain (multiple versions coexist)

```
~/.local/share/mvl/toolchains/
├── 0.19.0/
│   ├── bin/mvl              # compiler binary
│   └── std/                 # stdlib source, IMMUTABLE, locked to version
├── 0.20.0/
│   ├── bin/mvl
│   └── std/
└── 0.21.0-nightly/
    ├── bin/mvl
    └── std/
```

Symlinks in `~/.local/bin/`:
- `mvl` → active version (set by `mvl self use`)
- `mvl@0.20.0` → specific version (always available after install)

Version resolution: CLI flag > `.mvl-version` (project) > `mvl.toml` > `.mvl-version` (global) > bare symlink.

Inspired by: rustup (toolchain dirs), uv (symlinks + XDG), bun (shared cache).

### 3. Stdlib per version

Each toolchain embeds its stdlib source. Immutable after installation. Version file inside prevents mismatch. Phase 2: `include_str!` in binary, extracted on first run (#157). Phase 4+: ships as files alongside binary.

### 4. Shared Cargo cache

All MVL versions share one Cargo registry at `$XDG_CACHE_HOME/mvl/cargo/`. Crate downloads happen once. Builds happen per-project in `.mvl/target/`.

### 5. Project-local `.mvl/`

```
myproject/
├── mvl.toml          # manifest (deps, requires-mvl)
├── mvl.lock          # deterministic lock
├── .mvl-version      # "0.20.0" (pin)
├── .mvl/             # project cache (gitignored)
│   ├── build/        # transpiled Rust
│   ├── target/       # cargo target
│   └── pkg/          # local package overrides
└── src/
```

### 6. Compilation units and linking

**Phase 3:** Source-only. All dependencies compile from source. The LLVM backend sees everything. No binary format.

**Phase 4:** Precompiled `.mvlo` modules: LLVM bitcode + verified metadata (types, effects, IFC labels, refinements, totality, trust score, source hash). The proof travels with the artifact — consumers verify at link time without needing source.

Three trust levels at link time:
- `.mvlo` (MVL verified): 11/11
- `.mvlo` with `extern`: degraded — trust score reflects extern ratio
- Foreign (`.rlib`, `.a`, `.so`): unverified — effects/IFC enforced at FFI boundary only

### 7. Toolchain commands

```bash
mvl self install 0.21.0     # download + symlink
mvl self use 0.20.0         # set active
mvl self list                # show installed
mvl self uninstall 0.19.0   # remove
mvl self update              # latest stable
mvl pin 0.20.0               # project pin
mvl clean                    # wipe .mvl/build + .mvl/target
```

## Rationale

- **uv** proved XDG-native toolchain management works and is the cleanest model
- **rustup** proved per-version stdlib isolation is essential for reproducibility
- **cargo** proved shared global cache with per-project build dirs is the right split
- **bun** proved copy-on-write links save space in package caches
- Source-only compilation for Phase 3 avoids binary format complexity during the LLVM transition
- `.mvlo` verified metadata for Phase 4 enables trust scoring without source access

## Consequences

- Multiple MVL versions coexist cleanly
- Projects are reproducible via `.mvl-version` + `mvl.lock`
- Stdlib is immutable — no "works on my machine" drift
- Cargo downloads happen once globally
- Project cache is local and disposable
- No `$HOME` dotfile pollution

## Connected to

- ADR-0007: Stdlib import model (prelude loaded from toolchain std/)
- ADR-0010: Corpus test structure
- #157: Embed stdlib in binary (Phase 2 bootstrap)
- #56: Supply chain safety (mvl.lock, SBOM, trust scoring)
- Spec: `.openspec/specs/007-toolchain/spec.md`
