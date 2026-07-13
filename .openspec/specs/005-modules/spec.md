---
domain: language
version: 0.1.0
status: draft
date: 2026-04-12
---

# 005 — Module System

The MVL module system provides the unit of compilation and namespace management for multi-file programs. It follows ADR-0002 (language contraction): one import syntax, one visibility rule, no complex hierarchies. Directory module entry files use the sibling-file pattern per ADR-0033.

## Philosophy

Modules are namespaces, not encapsulation strategies. The module system exists so the compiler can verify cross-file invariants — information flow labels, totality guarantees, type safety — across a whole program. Every decision here serves verifiability over expressivity.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| File structure | One file = one module | Eliminates ambiguity; compiler knows exactly where to look |
| Visibility default | Private by default; `pub` exports | Explicit over implicit — matches MVL's "no hidden state" principle |
| Import syntax | `use path::to::Item;` or `use sub.dir.module::Item;` | One syntax; bare name for top-level modules, dot-qualified path for nested modules |
| Wildcard imports | Not allowed | Forces explicit imports; LLM-generated code must name what it uses |
| Directory modules | Directories as module groups | `src/geometry/` → `geometry` module; entry is `geometry.mvl` (sibling file, preferred) or `geometry/mod.mvl` (deprecated) |
| Re-exports | `pub use sub::Item;` | Allowed, explicit, one mechanism |
| Circular imports | Rejected at compile time | Acyclicity required for layer ordering and totality checking |

## Requirements

### Requirement 1: File-Module Correspondence [MUST]

Each `.mvl` source file MUST correspond to exactly one module. The module name MUST be derived from the file's path relative to the base directory (the directory passed to `mvl check`/`mvl build`), with path separators replaced by dots and the `.mvl` extension stripped (ADR-0052).

- A file directly in the base directory uses a bare name: `context.mvl` → `"context"`.
- A file in a subdirectory uses a dot-qualified name: `backends/llvm/context.mvl` → `"backends.llvm.context"`.
- Two files that share a basename but live in different subdirectories get distinct qualified names and MUST NOT collide.

A directory module MUST use the sibling-file pattern: `math.mvl` alongside `math/` is the entry for the `math` module (Rust 2018 style, ADR-0033). The compiler MUST also accept `math/mod.mvl` for backward compatibility but MUST emit a deprecation warning.

**Resolution order for a `use` import:**
1. `{base_dir}/{dot/path}.mvl` — preferred; dots converted to path separators
2. `{base_dir}/{mod_name}/mod.mvl` — single-segment only, deprecated; accepted with a warning

**Implementation:** `src/mvl/loader.rs::find_module_file`, `src/mvl/loader.rs::stem`, `src/mvl/loader.rs::qualified_stem`

**Tests:** `tests/module_resolver.rs::file_module_correspondence`, `tests/module_resolver.rs::qualified_module_path_no_collision`

#### Scenario: Bare module name (top-level file)

- GIVEN a base directory `src/` and a file `src/context.mvl`
- WHEN the compiler derives the module name
- THEN the module name MUST be `"context"` and MUST be importable as `use context::X`

#### Scenario: Qualified module name (nested file)

- GIVEN a base directory `src/` and a file `src/backends/llvm/context.mvl`
- WHEN the compiler derives the module name
- THEN the module name MUST be `"backends.llvm.context"` and MUST be importable as `use backends.llvm.context::X`

#### Scenario: Same basename in different subdirectories — no collision

- GIVEN `src/context.mvl` (module `"context"`) and `src/backends/llvm/context.mvl` (module `"backends.llvm.context"`)
- WHEN both are loaded by `mvl check src/`
- THEN they MUST coexist without collision, each importable via its distinct qualified name

#### Scenario: Directory module entry — sibling file (preferred)

- GIVEN a file `math.mvl` alongside a directory `math/` containing `stats.mvl`
- WHEN `math.mvl` contains `pub use stats::mean;`
- THEN `mean` MUST be accessible as `math::mean` from outside

#### Scenario: Directory module entry — legacy `mod.mvl` (deprecated)

- GIVEN a directory `math/` with `mod.mvl` and `stats.mvl` (no sibling `math.mvl`)
- WHEN `mod.mvl` contains `pub use stats::mean;`
- THEN `mean` MUST be accessible as `math::mean` AND the compiler MUST emit a deprecation warning
- AND the warning MUST say: "`math/mod.mvl` is deprecated; rename to `math.mvl` alongside the `math/` directory"

### Requirement 2: Visibility [MUST]

All items (functions, types, constants) MUST be private by default. An item MUST be marked `pub` to be visible outside its module. Items without `pub` MUST NOT be accessible from other modules. For struct types, `pub` on the type makes the type name visible; struct fields are always accessible when the struct itself is in scope (no per-field visibility). This keeps the grammar simple and matches the principle that structs are transparent value containers.

**Implementation:** `src/mvl/resolver/visibility.rs`

**Tests:** `tests/module_resolver.rs::private_item_rejected`, `tests/module_resolver.rs::pub_item_accessible`, `tests/module_resolver.rs::struct_fields_accessible`

#### Scenario: Private by default

- GIVEN a module `utils` with `fn helper() -> Int { 42 }`
- WHEN another module writes `use utils::helper;`
- THEN the compiler MUST reject: "`helper` is private in module `utils`"

#### Scenario: Pub exports

- GIVEN a module `utils` with `pub fn helper() -> Int { 42 }`
- WHEN another module writes `use utils::helper;`
- THEN `helper` MUST be callable without error

#### Scenario: Pub type

- GIVEN `pub type Point = struct { x: Float64, y: Float64 }`
- WHEN another module imports `use geometry::Point;`
- THEN `Point` MUST be usable as a type in the importing module

#### Scenario: Struct fields are accessible once type is imported

- GIVEN `pub type Point = struct { x: Float64, y: Float64 }` in module `geometry`
- AND another module has `use geometry::Point;`
- WHEN that module writes `let p = Point { x: 1.0, y: 2.0 }; let v = p.x;`
- THEN both construction and field access MUST succeed — fields are not separately `pub`-gated

### Requirement 3: Import Syntax [MUST]

Imports MUST use the `use` keyword. The module path before `::` MUST be a dot-separated qualified name matching the module's path relative to the base directory (ADR-0052). Wildcard imports (`use module::*`) MUST NOT be permitted. All `use` declarations MUST appear at the top of the file, before any other declarations.

Three forms are accepted:
- `use module::Item;` — bare name for a top-level module
- `use sub.dir.module::Item;` — dot-qualified for a nested module
- `use module::{A, B};` — brace group for multiple items from one module

**Implementation:** `src/mvl/parser.rs`, `src/mvl/resolver.rs`, `src/mvl/loader.rs::collect_imported_module_names`

**Tests:** `tests/module_resolver.rs::use_at_top`, `tests/module_resolver.rs::wildcard_rejected`, `tests/module_resolver.rs::name_collision_rejected`, `tests/module_resolver.rs::missing_module_rejected`, `tests/module_resolver.rs::qualified_module_import_resolves`, `tests/module_resolver.rs::bare_and_qualified_names_coexist`

#### Scenario: Named import (bare module)

- GIVEN `use geometry::Point;` at top of file, and `geometry.mvl` is in the base directory
- WHEN `Point` is used in the file body
- THEN it MUST resolve to the `Point` type exported by module `geometry`

#### Scenario: Qualified import (nested module)

- GIVEN `use backends.llvm.context::EmitCtx;` at top of file
- AND `backends/llvm/context.mvl` exists relative to the base directory
- WHEN `EmitCtx` is used in the file body
- THEN it MUST resolve to the `EmitCtx` type exported by module `backends.llvm.context`

#### Scenario: Multiple imports from same module

- GIVEN `use geometry::Point;` and `use geometry::Line;` at top of file
- WHEN both types are used
- THEN both MUST resolve correctly — two `use` lines, not one wildcard

#### Scenario: Wildcard rejected

- GIVEN `use geometry::*;`
- WHEN the compiler processes the file
- THEN it MUST reject: "wildcard imports are not permitted; name each item explicitly"

#### Scenario: Import after declaration rejected

- GIVEN a `fn` declaration followed by a `use` statement
- WHEN the compiler processes the file
- THEN it MUST reject: "`use` declarations must appear before all other declarations"

#### Scenario: Name collision between imports

- GIVEN `use geometry::Point;` and `use graphics::Point;` in the same file
- WHEN the compiler resolves names
- THEN it MUST reject: "name collision: `Point` imported from both `geometry` and `graphics`"

#### Scenario: Missing module

- GIVEN `use nonexistent::Foo;`
- WHEN the compiler resolves modules
- THEN it MUST reject: "module `nonexistent` not found"

### Requirement 4: Re-exports [MUST]

A module MAY re-export items from its sub-modules using `pub use path::Item;`. Re-exported items MUST satisfy the same visibility rules as direct exports. Re-exporting a private item from another module MUST be rejected.

**Implementation:** `src/mvl/resolver.rs`

**Tests:** `tests/module_resolver.rs::reexport_public`, `tests/module_resolver.rs::reexport_private_rejected`

#### Scenario: Re-export from sub-module

- GIVEN `math.mvl` containing `pub use stats::mean;` and `math/stats.mvl` with `pub fn mean(...)`
- WHEN an external module writes `use math::mean;`
- THEN `mean` MUST resolve to `stats::mean`

#### Scenario: Re-export of private item rejected

- GIVEN `math/stats.mvl` with `fn internal_helper() -> Int`
- WHEN `math.mvl` contains `pub use stats::internal_helper;`
- THEN the compiler MUST reject: "`internal_helper` is private in `stats`"

### Requirement 5: Circular Import Rejection [MUST]

The compiler MUST detect and reject circular module dependencies at compile time. A module dependency graph MUST be acyclic. The error MUST name the cycle.

**Implementation:** `src/mvl/resolver/cycle_check.rs`

**Tests:** `tests/module_resolver.rs::circular_import_rejected`

#### Scenario: Direct cycle

- GIVEN module `A` imports from module `B` and module `B` imports from module `A`
- WHEN the compiler resolves the dependency graph
- THEN it MUST reject: "circular dependency detected: A → B → A"

#### Scenario: Transitive cycle

- GIVEN modules A → B → C → A
- WHEN the compiler resolves the dependency graph
- THEN it MUST reject: "circular dependency detected: A → B → C → A"

#### Scenario: Diamond (no cycle)

- GIVEN modules A → B, A → C, B → D, C → D (diamond)
- WHEN the compiler resolves the dependency graph
- THEN it MUST succeed — diamonds are acyclic

### Requirement 6: Standard Library Module [MUST]

The MVL standard library MUST be organized as a module tree rooted at `std`. All standard library items MUST be imported explicitly using `use std::...`. There MUST be no implicit imports (no Haskell-style Prelude auto-import).

**Implementation:** `src/mvl/stdlib.rs`, `std/*.mvl` — The stdlib module tree is implemented as `.mvl` source files loaded by the compiler. Stdlib modules are imported via `use std.*` syntax; the loader resolves them from the `std/` directory.

**Tests:** `tests/module_resolver.rs::stdlib_explicit_import`, `tests/stdlib/`

#### Scenario: No implicit imports

- GIVEN a file that uses `List` without importing it
- WHEN the compiler resolves names
- THEN it MUST reject: "`List` is not in scope; add `use std::collections::List;`"

#### Scenario: Explicit stdlib import

- GIVEN `use std::collections::List;` at top of file
- WHEN `List[Int]` is used in the file
- THEN it MUST resolve correctly

## EBNF Updates

The following productions extend the grammar in [`mvl-spec/grammar/grammar.ebnf`](https://github.com/mvl-lang/mvl-spec/blob/main/grammar/grammar.ebnf):

```ebnf
(* === Top-level with module imports === *)
(* "pub" is factored out so each decl_body alternative starts with a   *)
(* distinct keyword — preserves LL(1) property (ADR-0005).             *)
(* Semantic constraint: "pub" is required before reexport_decl.        *)
program        = { use_decl } { declaration } ;
declaration    = [ "pub" ] decl_body ;
decl_body      = type_decl | fn_decl | const_decl | reexport_decl ;

(* === Modules and imports === *)
use_decl       = "use" module_path ";" ;
reexport_decl  = "use" module_path ";" ;  (* "pub" required — enforced by type checker *)
module_path    = IDENT { "." IDENT } [ "." "{" IDENT { "," IDENT } "}" ] ;

(* === Declarations (no leading "pub" — hoisted to declaration) === *)
type_decl      = "type" IDENT [ type_params ] "=" type_body ;
fn_decl        = [ totality ] [ security ] "fn" IDENT [ type_params ]
                 "(" [ param_list ] ")" "->" return_type
                 [ "!" effect_list ] [ "where" constraints ]
                 block ;
const_decl     = "const" IDENT ":" type_expr "=" expr ";" ;
```

Note: The `module_decl` production (`module Name { ... }`) is removed. File = module eliminates the need for inline module blocks, which would create two competing namespacing mechanisms.

## Examples

### Single file usage

```mvl
// file: src/geometry.mvl
pub type Point = struct { x: Float64, y: Float64 }

pub total fn distance(a: Point, b: Point) -> Float64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    return (dx * dx + dy * dy).sqrt();
}

fn validate(p: Point) -> bool {  // private — internal only
    return p.x.is_finite() && p.y.is_finite();
}
```

```mvl
// file: src/main.mvl
use geometry::Point;
use geometry::distance;

total fn main() -> Int {
    let a = Point { x: 0.0, y: 0.0 };
    let b = Point { x: 3.0, y: 4.0 };
    return 0;
}
```

### Directory module

```
src/
  math.mvl          ← entry point (sibling file, Rust 2018 style)
  math/
    stats.mvl       ← statistics functions
    linalg.mvl      ← linear algebra
  main.mvl
```

```mvl
// file: src/math.mvl  (entry — re-exports from sub-modules)
pub use stats::mean;
pub use stats::variance;
pub use linalg::Matrix;
```

```mvl
// file: src/main.mvl
use math::mean;
use math::Matrix;
```

### Qualified paths for nested modules (ADR-0052)

When two modules share a basename but live in different subdirectories, each is imported via its unique dot-qualified path derived from the base directory:

```
compiler/
  context.mvl               ← module "context"      (TypeEnv lives here)
  backends/
    llvm/
      context.mvl           ← module "backends.llvm.context"  (EmitCtx lives here)
  main.mvl
```

```mvl
// file: compiler/main.mvl
use context::TypeEnv;                // → compiler/context.mvl
use backends.llvm.context::EmitCtx; // → compiler/backends/llvm/context.mvl
```

The dot-path mirrors the directory hierarchy relative to the base directory (`compiler/`). No renaming is required; the path is the disambiguator.
