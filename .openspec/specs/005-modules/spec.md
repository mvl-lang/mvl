---
domain: language
version: 0.1.0
status: draft
date: 2026-04-12
---

# 005 — Module System

The MVL module system provides the unit of compilation and namespace management for multi-file programs. It follows ADR-0002 (language contraction): one import syntax, one visibility rule, no complex hierarchies.

## Philosophy

Modules are namespaces, not encapsulation strategies. The module system exists so the compiler can verify cross-file invariants — information flow labels, totality guarantees, type safety — across a whole program. Every decision here serves verifiability over expressivity.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| File structure | One file = one module | Eliminates ambiguity; compiler knows exactly where to look |
| Visibility default | Private by default; `pub` exports | Explicit over implicit — matches MVL's "no hidden state" principle |
| Import syntax | `use path::to::Item;` | One syntax, one meaning; qualified path resolves name unambiguously |
| Wildcard imports | Not allowed | Forces explicit imports; LLM-generated code must name what it uses |
| Directory modules | Directories as module groups | `src/geometry/` → `geometry` module, `mod.mvl` is the entry point |
| Re-exports | `pub use sub::Item;` | Allowed, explicit, one mechanism |
| Circular imports | Rejected at compile time | Acyclicity required for layer ordering and totality checking |

## Requirements

### Requirement 1: File-Module Correspondence [MUST]

Each `.mvl` source file MUST correspond to exactly one module. The module name MUST be the filename without extension (e.g., `geometry.mvl` → module `geometry`). A directory module MUST have a `mod.mvl` entry file that declares which sub-modules are visible.

**Implementation:** `src/mvl/resolver/mod.rs`

**Tests:** `tests/module_resolver.rs::file_module_correspondence`

#### Scenario: Single-file module

- GIVEN a file `math/stats.mvl` in the source tree
- WHEN the compiler resolves modules
- THEN the module name MUST be `stats`, accessible as `math::stats`

#### Scenario: Directory module entry

- GIVEN a directory `math/` with `mod.mvl` and `stats.mvl`
- WHEN `mod.mvl` contains `pub use stats::mean;`
- THEN `mean` MUST be accessible as `math::mean` from outside the directory

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

Imports MUST use the `use` keyword with a fully-qualified path ending in a specific item name. Wildcard imports (`use module::*`) MUST NOT be permitted. All `use` declarations MUST appear at the top of the file, before any other declarations.

**Implementation:** `src/mvl/parser/mod.rs`, `src/mvl/resolver/mod.rs`

**Tests:** `tests/module_resolver.rs::use_at_top`, `tests/module_resolver.rs::wildcard_rejected`, `tests/module_resolver.rs::name_collision_rejected`, `tests/module_resolver.rs::missing_module_rejected`

#### Scenario: Named import

- GIVEN `use std::collections::Map;` at top of file
- WHEN `Map` is used in the file body
- THEN it MUST resolve to `std::collections::Map`

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

**Implementation:** `src/mvl/resolver/mod.rs`

**Tests:** `tests/module_resolver.rs::reexport_public`, `tests/module_resolver.rs::reexport_private_rejected`

#### Scenario: Re-export from sub-module

- GIVEN `mod.mvl` containing `pub use stats::mean;` and `stats.mvl` with `pub fn mean(...)`
- WHEN an external module writes `use math::mean;`
- THEN `mean` MUST resolve to `stats::mean`

#### Scenario: Re-export of private item rejected

- GIVEN `stats.mvl` with `fn internal_helper() -> Int`
- WHEN `mod.mvl` contains `pub use stats::internal_helper;`
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

**Implementation:** `src/mvl/stdlib/mod.rs` *(Deferred — Phase 2)* Phase 1 uses `extern "rust"` wrappers via the transpiler; the verified MVL `std` module tree is a Phase 2 goal. Tracked in #67.

**Tests:** `tests/module_resolver.rs::stdlib_explicit_import` *(Deferred — Phase 2)*

#### Scenario: No implicit imports

- GIVEN a file that uses `List` without importing it
- WHEN the compiler resolves names
- THEN it MUST reject: "`List` is not in scope; add `use std::collections::List;`"

#### Scenario: Explicit stdlib import

- GIVEN `use std::collections::List;` at top of file
- WHEN `List[Int]` is used in the file
- THEN it MUST resolve correctly

## EBNF Updates

The following productions extend the grammar in `docs/grammar.ebnf`:

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
  math/
    mod.mvl       ← entry point, re-exports
    stats.mvl     ← statistics functions
    linalg.mvl    ← linear algebra
  main.mvl
```

```mvl
// file: src/math/mod.mvl
pub use stats::mean;
pub use stats::variance;
pub use linalg::Matrix;
```

```mvl
// file: src/main.mvl
use math::mean;
use math::Matrix;
```
