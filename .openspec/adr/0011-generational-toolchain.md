# ADR-0011: Generational Toolchain — Multi-Version MVL with Shared Caches

**Status:** Accepted
**Date:** 2026-04-15
**Context:** MVL needs to support multiple compiler versions coexisting on one machine. Each version ships its own stdlib (immutable). Dependencies (Rust crates) should be cached globally, not per-version. Projects pin their MVL version. Inspired by uv (Python), rustup (Rust), and bun (JavaScript).

**Supersedes:** Parts of ADR-0009 (which described a single-version layout). ADR-0009's XDG compliance and three-location resolution remain valid; this ADR extends the layout for multiple versions.

## Decision

### Full Directory Layout

```
# ── Config (XDG_CONFIG_HOME) ─────────────────────────────────────────────────
~/.config/mvl/
├── config.toml                  # bin_dir, trust thresholds, default effects
└── .mvl-version                 # global default version: "0.20.0"

# ── Binaries (user bin, on PATH) ─────────────────────────────────────────────
~/.local/bin/
├── mvl            → ~/.local/share/mvl/toolchains/0.20.0/bin/mvl   # active
├── mvl@0.19.0     → ~/.local/share/mvl/toolchains/0.19.0/bin/mvl
├── mvl@0.20.0     → ~/.local/share/mvl/toolchains/0.20.0/bin/mvl
└── mvl@0.21.0-nightly → ~/.local/share/mvl/toolchains/0.21.0-nightly/bin/mvl

# ── Data (XDG_DATA_HOME) ─────────────────────────────────────────────────────
~/.local/share/mvl/
├── toolchains/
│   ├── 0.19.0/
│   │   ├── bin/mvl              # compiler binary for this version
│   │   └── std/                 # stdlib source, IMMUTABLE, locked to version
│   │       ├── core.mvl
│   │       ├── fs.mvl
│   │       ├── json.mvl
│   │       └── .version         # "0.19.0"
│   ├── 0.20.0/
│   │   ├── bin/mvl
│   │   └── std/
│   └── 0.21.0-nightly/
│       ├── bin/mvl
│       └── std/
└── pkg/                         # shared package source + .mvlo cache
    ├── http/1.2.0/
    ├── http/1.3.0/
    └── postgres/0.9.1/

# ── Cache (XDG_CACHE_HOME) — disposable ──────────────────────────────────────
~/.cache/mvl/
├── cargo/                       # SHARED Cargo registry + git cache
│   ├── registry/
│   │   ├── index/
│   │   ├── cache/               # .crate tarballs
│   │   └── src/                 # unpacked sources
│   └── git/
└── downloads/                   # toolchain download cache

# ── Project-local ─────────────────────────────────────────────────────────────
myproject/
├── mvl.toml                     # project manifest
├── mvl.lock                     # deterministic lock
├── .mvl-version                 # "0.20.0" — overrides global
├── .mvl/                        # project cache (gitignored)
│   ├── build/                   # transpiled Rust output
│   ├── target/                  # cargo target dir for this project
│   └── pkg/                     # project-local package overrides (rare)
└── src/
    └── main.mvl
```

### Version Resolution Order

When `mvl` is invoked, it resolves which toolchain to use:

```
1. CLI flag:        mvl@0.19.0 run main.mvl          # explicit
2. Project pin:     .mvl-version in project root       # "0.20.0"
3. Manifest:        mvl.toml [project] mvl = "0.20.0"  # alternative to pin file
4. Global default:  ~/.config/mvl/.mvl-version          # "0.20.0"
5. Bare symlink:    mvl → active toolchain              # fallback
```

The first match wins. This follows rustup's precedent (rust-toolchain.toml > default).

### Compatibility declaration vs pin

Two separate concepts (learned from uv):

```toml
# mvl.toml
[project]
name = "myapp"
version = "0.1.0"
requires-mvl = ">=0.19.0"        # compatibility range (which versions CAN build this)

[dependencies]
http = "1.2.0"
```

```
# .mvl-version
0.20.0                            # exact pin (which version DOES build this)
```

`requires-mvl` is for consumers (libraries). `.mvl-version` is for developers (reproducibility).

### Toolchain Management Commands

```bash
# Install a version
mvl self install 0.21.0
# → Downloads compiler to ~/.local/share/mvl/toolchains/0.21.0/
# → Extracts embedded stdlib to toolchains/0.21.0/std/
# → Creates symlink: ~/.local/bin/mvl@0.21.0

# Set global default (updates bare `mvl` symlink)
mvl self use 0.20.0
# → ~/.local/bin/mvl → toolchains/0.20.0/bin/mvl

# Pin project version
mvl pin 0.20.0
# → Writes .mvl-version in current directory

# List installed versions
mvl self list
#   0.19.0
# * 0.20.0 (active)
#   0.21.0-nightly

# Remove a version
mvl self uninstall 0.19.0
# → Removes toolchain dir + symlink

# Update to latest
mvl self update
# → Downloads latest stable, updates active symlink
```

### Shared Cargo Cache

All MVL versions share one Cargo registry. The compiler sets `CARGO_HOME` before invoking cargo:

```rust
fn cargo_home() -> PathBuf {
    env("MVL_CARGO_HOME")
        .or(xdg_cache_home().join("mvl/cargo"))
}

// Before cargo invocation:
env::set_var("CARGO_HOME", cargo_home());
env::set_var("CARGO_TARGET_DIR", project_root().join(".mvl/target"));
```

This means:
- Crate downloads happen **once** (shared registry)
- Builds happen **per-project** (`.mvl/target/`)
- Different MVL versions reuse the same downloaded crates
- `mvl clean` wipes `.mvl/` but not the global cache

### Project `.mvl/` Directory

Project-local cache, analogous to `.venv/` (uv) or `target/` (cargo):

| Path | Purpose | Disposable |
|---|---|---|
| `.mvl/build/` | Transpiled Rust source | Yes |
| `.mvl/target/` | Cargo compilation artifacts | Yes |
| `.mvl/pkg/` | Local package overrides (dev, patches) | No — committed if intentional |

Default `.gitignore` entry:
```
.mvl/
```

`mvl clean` removes `.mvl/build/` and `.mvl/target/`. `mvl clean --all` removes entire `.mvl/`.

### Stdlib Immutability

Each toolchain's `std/` is **immutable** after installation. The version file inside ensures compiler↔stdlib match:

```
~/.local/share/mvl/toolchains/0.20.0/std/.version = "0.20.0"
```

If the compiler detects a version mismatch (e.g., someone manually edited stdlib files), it errors:

```
error: stdlib version mismatch
  compiler: 0.20.0
  stdlib:   0.19.0 (at ~/.local/share/mvl/toolchains/0.20.0/std/)
  hint: run `mvl self install 0.20.0 --force` to reinstall
```

No patching, no overriding. If you want to modify the stdlib, you're developing the compiler — use the repo, not an installed toolchain.

### config.toml

```toml
# ~/.config/mvl/config.toml

[toolchain]
default = "0.20.0"              # same as .mvl-version, either works
bin_dir = "~/.local/bin"        # where symlinks go

[cache]
cargo_home = "~/.cache/mvl/cargo"  # override shared Cargo cache location

[trust]
min_score = 8                   # minimum trust score for `mvl add` without --force
auto_audit = true               # run `mvl audit` on every build

[effects]
default_allow = ["Console"]     # effects allowed without explicit declaration
```

### Design Patterns Borrowed

| Pattern | Source | Why |
|---|---|---|
| XDG-native from day one | uv | No `~/.mvl` dotfile pollution |
| Toolchain dirs with embedded stdlib | rustup | Immutable, version-locked, isolated |
| Version symlinks with `@` suffix | uv python, nvm | `mvl@0.20.0` is explicit and discoverable |
| Shared global Cargo cache | cargo (CARGO_HOME) | Download once, build per-project |
| Project-local `.mvl/` | uv (.venv/) + cargo (target/) | Build artifacts near the code |
| Plain-text pin file | uv (.python-version) + rustup (rust-toolchain.toml) | `.mvl-version` — simple, greppable |
| Separate compatibility vs pin | uv (requires-python vs .python-version) | `requires-mvl` for libraries, `.mvl-version` for devs |
| Lock file with hashes | uv (uv.lock) | `mvl.lock` — deterministic, auditable |
| `self` subcommand for toolchain ops | rustup (rustup self) | `mvl self install/use/list/update/uninstall` |

## Consequences

- Multiple MVL versions coexist cleanly — no conflicts
- Projects are reproducible via `.mvl-version` + `mvl.lock`
- Rust crate downloads happen once globally — fast installs
- Stdlib is immutable per version — no "works on my machine" stdlib drift
- Project cache is local and disposable — `mvl clean` is safe
- XDG compliance means no dotfile pollution and clean backup/sync semantics
- `mvl@0.20.0` syntax enables CI pinning without config files

## Connected to

- ADR-0009: XDG paths (extended, not superseded — resolution logic still applies)
- ADR-0008: Compilation units and linking (Phase 4 .mvlo lives in pkg/)
- ADR-0007: Stdlib import model (prelude loaded from toolchain std/)
- #157: Embed stdlib in binary (Phase 2 bootstrap — precursor to this)
- #56: Supply chain safety (mvl.lock, SBOM, trust scoring)
