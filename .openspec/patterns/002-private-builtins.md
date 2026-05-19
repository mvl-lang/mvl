# Pattern 002: Private Builtins (`_prefix`)

**Status:** Accepted
**Related:** #899, #893 (IFC epic)

## Rule

All module-private builtins use underscore prefix: `builtin fn _name(...)`.

## Structure

```mvl
// Private: raw OS boundary, returns bare types
builtin fn _env_read(name: String) -> Option[String] ! Env

// Public: applies IFC label, user-facing API
pub fn get(name: String) -> Option[Tainted[String]] ! Env {
    match _env_read(name) {
        Some(v) => Some(relabel taint(v, "ENV-INPUT")),
        None => None,
    }
}
```

## Why

1. **No unlabeled external input in public API.** Raw builtins return bare `String` from OS calls. Public wrappers apply IFC labels (`Tainted`, `Secret`) via `relabel`. Users cannot bypass labeling.

2. **Grep-able.** `grep "builtin fn _"` finds all raw OS boundary functions across stdlib. `grep "pub builtin fn"` finds public builtins (should be rare).

3. **Minimizes trust surface.** The `_prefix` builtin is the only code that crosses the Rust FFI boundary. Everything above it is pure MVL — verifiable, auditable.

## Convention

| Declaration | Visibility | Returns | Use |
|-------------|-----------|---------|-----|
| `builtin fn _name(...)` | Private | Bare types | Raw OS/runtime call |
| `pub fn name(...)` | Public | Labeled types | User-facing API |
| `pub builtin fn name(...)` | Public | Labeled types | Only when label applied in Rust runtime (rare) |

## Examples

```mvl
// std/env.mvl
builtin fn _env_read(name: String) -> Option[String] ! Env
pub fn get(name: String) -> Option[Tainted[String]] ! Env { ... }
pub fn get_secret(name: String) -> Option[Secret[String]] ! Env { ... }

// std/io.mvl
builtin fn _raw_read(path: String) -> Option[String] ! FileRead
pub fn read_file(path: String) -> Option[Tainted[String]] ! FileRead { ... }

// std/net.mvl
builtin fn _tcp_read(stream: TcpStream) -> Option[String] ! Net
pub fn tcp_read(stream: TcpStream) -> Option[Tainted[String]] ! Net { ... }

// std/strings.mvl
builtin fn _str_len(s: String) -> Int
pub fn str_len(s: String) -> Int { _str_len(s) }
// (or via Type::method: fn String::len(self) -> Int { _str_len(self) })
```

## Assurance

The assurance report tracks:
- Total `builtin fn _*` count (raw trust boundary)
- Total `pub builtin fn` count (should trend to zero)
- Ratio: private builtins / total builtins
