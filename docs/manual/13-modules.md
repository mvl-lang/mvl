# 13. Module System

Modules organize code into namespaces with controlled visibility.

## 13.1 Module Declaration

```mvl
module http {
    type Request = struct { ... }
    type Response = struct { ... }

    fn get(url: Clean<Url>) -> Result<Tainted<Response>, NetError> ! Net {
        // ...
    }

    // Private helper — not visible outside this module
    fn build_headers() -> Map<String, String> {
        // ...
    }
}
```

## 13.2 Visibility

- **Public (default):** Declarations at the top of a module are visible to importers
- **Private:** Prefix with `_` or use a nested module for internal details

The visibility model is deliberately simple. No `pub`, `pub(crate)`, `pub(super)` hierarchy.

## 13.3 Imports

```mvl
use http.Request;                    // import a specific type
use http.{Request, Response};        // import multiple
use http;                            // import module — use as http.get()
```

## 13.4 File Organization

Each `.mvl` file is a module. The filename determines the module name:

```
src/
  main.mvl              // entry point
  http.mvl              // module http
  http/
    client.mvl          // module http.client
    server.mvl          // module http.server
  db.mvl                // module db
```

## 13.5 Packages

A package is a collection of modules with a `dependency.toml`:

```toml
[package]
name = "my_service"
version = "0.1.0"

[dependencies]
http = "1.0"
json = "2.3"
```

Packages are the trust boundary for the standard library's extended tier.
