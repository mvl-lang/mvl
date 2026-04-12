# 13. Module System

One file = one module. The filename is the module name.

## 13.1 File-Module Correspondence

```
src/
  main.mvl              // module: main
  geometry.mvl          // module: geometry
  math/
    mod.mvl             // module: math (entry — re-exports from sub-modules)
    stats.mvl           // module: math::stats
    linalg.mvl          // module: math::linalg
```

A directory module needs a `mod.mvl` entry file that controls what the directory exposes.

### Binary vs library

The presence of `fn main() -> ()` determines the crate type:

| File has `fn main`? | Compiles to | Rust equivalent |
|---------------------|------------|-----------------|
| Yes | Binary (executable) | `src/main.rs` |
| No | Library (reusable module) | `src/lib.rs` |

No attribute or annotation needed. The compiler infers this from the AST.

## 13.2 Visibility

Everything is **private by default**. Use `pub` to export:

```mvl
// geometry.mvl
pub type Point = struct { x: Float64, y: Float64 }  // visible to importers

pub total fn distance(a: Point, b: Point) -> Float64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    return (dx * dx + dy * dy).sqrt();
}

fn validate(p: Point) -> Bool {  // private — not importable
    return p.x.is_finite() && p.y.is_finite();
}
```

Struct fields are not separately gated — if the type is imported, all fields are accessible. Encapsulation is at the type boundary, not the field boundary.

## 13.3 Imports

One syntax: `use path::to::Item;`. All imports at the top of the file, one item per line.

```mvl
use geometry::Point;
use geometry::distance;
use math::stats::mean;
```

Wildcards are not permitted. Name each item explicitly — the LLM must be precise about what it depends on, and the compiler can track every dependency.

## 13.4 Re-exports

A `mod.mvl` entry file assembles a module's public surface:

```mvl
// math/mod.mvl
pub use stats::mean;
pub use stats::variance;
pub use linalg::Matrix;
```

Callers can then write `use math::mean;` without knowing it lives in `math::stats`.

## 13.5 Circular Imports

Rejected at compile time. The module dependency graph must be acyclic. The compiler names the cycle in the error:

```
error: circular dependency detected: geometry → utils → geometry
```

## 13.6 Standard Library

The standard library is rooted at `std`. No items are imported automatically — there is no Prelude. Every dependency is explicit:

```mvl
use std::collections::List;
use std::collections::Map;
use std::io::File;
```

The suggestion appears in error messages: `` `List` is not in scope; add `use std::collections::List;` ``

## 13.7 Packages

A package is a directory tree with a `package.toml` at the root:

```toml
[package]
name = "my_service"
version = "0.1.0"

[dependencies]
http = "1.0"
json = "2.3"
```

Packages are the compilation and linking unit. External packages are imported using the same `use` syntax as local modules.
