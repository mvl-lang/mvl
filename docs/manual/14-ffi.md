# 14. Foreign Function Interface

The FFI allows MVL to call functions written in other languages. Extern blocks are explicit trust boundaries — the compiler does not verify their implementation.

## 14.1 Extern Blocks

```mvl
extern "rust" {
    fn sha256(data: &Array[Byte]) -> Array[Byte];
    fn aes_encrypt(key: Secret[Array[Byte]], data: Array[Byte]) -> Array[Byte];
}
```

## 14.2 Trust Boundaries

Every `extern` function is:
- **Greppable:** `grep extern` finds all trust boundaries
- **Tracked in assurance:** Assurance reports count extern functions separately
- **Excluded from verification:** The compiler trusts the declaration but cannot verify the implementation
- **Counted in coverage:** `make assurance` reports extern-to-total ratio

## 14.3 Supported Targets

Phase 1 (Rust transpilation): `extern "rust"` calls Rust functions directly.

Phase 2 (LLVM): `extern "c"` follows the C ABI for broader interoperability.

## 14.4 Safety Contract

The caller is responsible for ensuring:
- The extern function's actual behavior matches its declared signature
- Side effects are correctly declared
- Security labels are respected

The MVL compiler trusts the declaration. If the extern function lies (e.g., declares pure but performs I/O), the soundness guarantee breaks at that boundary.

## 14.5 Minimizing FFI Surface

The stdlib wraps common extern functions in verified MVL interfaces. Prefer stdlib over raw extern:

```mvl
// Prefer this (stdlib, verified wrapper):
use crypto.sha256;
let hash = sha256(data);

// Over this (raw extern, unverified):
extern "rust" { fn sha256(data: &Array[Byte]) -> Array[Byte]; }
```
