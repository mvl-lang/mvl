# ADR-0009: XDG Paths and Multi-Location Source Resolution

**Status:** Accepted
**Date:** 2026-04-15
**Context:** MVL needs a standard location for the stdlib, package cache, config, and build artifacts. `mvl run` currently resolves source from a single location. It needs to resolve from project root + stdlib + packages. All paths follow the XDG Base Directory Specification.

## Decision

### XDG-Compliant Directory Layout

| Purpose | XDG variable | Default | MVL path |
|---|---|---|---|
| Stdlib + packages | `$XDG_DATA_HOME` | `~/.local/share` | `$XDG_DATA_HOME/mvl/std/`, `$XDG_DATA_HOME/mvl/pkg/` |
| Config | `$XDG_CONFIG_HOME` | `~/.config` | `$XDG_CONFIG_HOME/mvl/config.toml` |
| Build cache | `$XDG_CACHE_HOME` | `~/.cache` | `$XDG_CACHE_HOME/mvl/build/`, `$XDG_CACHE_HOME/mvl/cargo/` |
| Runtime (sockets, locks) | `$XDG_RUNTIME_DIR` | `/run/user/$UID` | Not used initially |

Override: `$MVL_HOME` overrides all XDG paths (for CI, containers, testing). If set, all MVL data lives under `$MVL_HOME/std/`, `$MVL_HOME/pkg/`, `$MVL_HOME/config.toml`, `$MVL_HOME/cache/`.

```
# Default layout (XDG)
~/.local/share/mvl/
├── std/                    # stdlib source (ships with mvl, verified 11/11)
│   ├── core.mvl            # prelude — auto-loaded by compiler
│   ├── fs.mvl              # use std.fs
│   ├── json.mvl            # use std.json
│   ├── time.mvl            # use std.time
│   ├── crypto.mvl          # use std.crypto
│   ├── net.mvl             # use std.net
│   ├── process.mvl         # use std.process
│   ├── log.mvl             # use std.log
│   └── test.mvl            # use std.test
├── pkg/                    # package cache (Phase 4)
│   ├── http/1.2.0/         # versioned
│   └── postgres/0.9.1/

~/.config/mvl/
└── config.toml             # user preferences, default effects policy, trust thresholds

~/.cache/mvl/
├── build/                  # transpiled Rust output, incremental
└── cargo/                  # Cargo registry/target cache (CARGO_HOME override)
```

### Source Resolution Order

`mvl run main.mvl` and `mvl build` resolve `use` statements from three locations, in order:

| Priority | Prefix | Resolves to | Example |
|---|---|---|---|
| 1 | (none / relative) | Project root | `use mylib` → `./mylib.mvl` or `./mylib/mod.mvl` |
| 2 | `std` | Stdlib root | `use std.fs` → `$XDG_DATA_HOME/mvl/std/fs.mvl` |
| 3 | `pkg` | Package cache | `use pkg.http` → `$XDG_DATA_HOME/mvl/pkg/http/latest/` |

No ambiguity: the prefix determines the search location. `use foo` is always project-local. `use std.foo` is always stdlib. `use pkg.foo` is always a package.

### Prelude loading

The compiler always loads `$XDG_DATA_HOME/mvl/std/core.mvl` as the implicit prelude (ADR-0007). This happens before any user source is parsed. Core types (Option, Result, Array, etc.) are available without import.

### Project config (`mvl.toml`)

```toml
[project]
name = "myapp"
version = "0.1.0"

[dependencies]
http = "1.2.0"
postgres = "0.9.1"

[effects]
# Default effect policy for this project
allow = ["FileRead", "FileWrite", "Net", "Console"]
```

Lives in the project root. Package versions pinned here, resolved from `$XDG_DATA_HOME/mvl/pkg/`.

### Path resolution in code

```rust
// Resolver logic (pseudo-code)
fn resolve_module(name: &str) -> Path {
    if name.starts_with("std.") {
        xdg_data_home() / "mvl" / "std" / to_path(name.strip_prefix("std."))
    } else if name.starts_with("pkg.") {
        let (pkg, version) = lookup_in_mvl_toml(name);
        xdg_data_home() / "mvl" / "pkg" / pkg / version / to_path(rest)
    } else {
        project_root() / to_path(name)
    }
}

fn xdg_data_home() -> Path {
    env("MVL_HOME").unwrap_or(
        env("XDG_DATA_HOME").unwrap_or(home() / ".local" / "share")
    )
}
```

### Installation

`mvl` ships as a single binary. On first run (or `mvl init --stdlib`), it copies stdlib source to `$XDG_DATA_HOME/mvl/std/`. Version-matched to the compiler.

Stdlib source files are embedded in the binary at compile time using Rust's `include_str!` macro — no archive or compression. Each `.mvl` file becomes a `&'static str`. On first run they are written to disk verbatim. **No compression is used.** Stdlib source is plain text and small (a few KB per file); the overhead of a compression/decompression step is not justified until stdlib grows to hundreds of files or megabytes of source. If that threshold is reached, switch to `include_bytes!` + a compressed blob decoded at runtime.

```bash
# Install
brew install mvl   # or cargo install mvl

# First run populates stdlib
mvl init --stdlib
# → Installed MVL stdlib v0.20.0 to ~/.local/share/mvl/std/

# Or set MVL_HOME for isolated environments
MVL_HOME=/opt/mvl mvl init --stdlib
```

### Why XDG

1. **Convention over configuration.** Every Linux/macOS tool uses XDG (or should). Dotfile pollution in `$HOME` is a known anti-pattern.
2. **Separation of concerns.** Data (std/pkg) ≠ config ≠ cache. XDG enforces this.
3. **Container/CI friendly.** Set `$MVL_HOME` or `$XDG_DATA_HOME` once, everything follows.
4. **Backup/sync friendly.** `~/.config/mvl/` is user preferences (sync). `~/.cache/mvl/` is disposable (don't sync). `~/.local/share/mvl/std/` is version-pinned (ships with compiler, don't sync).

### Why NOT a single `~/.mvl/`

- Violates XDG — config, data, and cache mixed in one directory
- Can't selectively back up config without cache
- Can't mount cache on fast storage separately
- Breaks `XDG_CACHE_HOME=/tmp/cache` patterns in CI

## Consequences

- Stdlib is real MVL source files, not hardcoded Rust stubs
- Resolver needs a three-location search (project → std → pkg)
- `mvl init --stdlib` needed after install (or auto-detect on first run)
- `$MVL_HOME` provides escape hatch for non-XDG environments
- Phase 4 `.mvlo` precompiled modules go in `$XDG_DATA_HOME/mvl/pkg/` alongside source

## Connected to

- ADR-0007: Stdlib import model (prelude + explicit tiers)
- ADR-0008: Compilation units and linking (source-only Phase 3, binary Phase 4)
- #42: Core types (first stdlib source files)
- #130: Phase 4 epic (stdlib generation)
- #56: Supply chain safety (package cache trust)
