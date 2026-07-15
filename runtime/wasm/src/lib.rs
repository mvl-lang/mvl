// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL runtime for the WASM backend (#1819, epic #1817 phase 2).
//!
//! Compiled to `wasm32-wasip1` as a `cdylib`. Loaded by `wasmtime` via
//! `--preload runtime=<path>` alongside emitted user code — the emitter's
//! `(import "runtime" "_mvl_string_eq" ...)` declarations resolve to the
//! symbols exported here.
//!
//! Phase 2a scope: byte-level string equality. Enough to unblock
//! `assert_eq(String, String)` in the WASM corpus, and enough to prove
//! the whole crate → preload → import → call pipeline works end-to-end.
//! Everything else (`MvlString` refcount layout, `.len` / `.concat` /
//! `.substring` and friends) is scheduled per follow-up commit.
//!
//! ## Symbol convention
//!
//! `#[unsafe(no_mangle)] pub extern "C" fn _mvl_*` — same prefix and ABI
//! as `runtime/llvm/`. This keeps the WASM emitter's dispatch logic close
//! to what the LLVM emitter already does in
//! `src/mvl/backends/llvm_text/dispatch.rs`.
//!
//! ## Calling convention
//!
//! Strings in the current emitter live on the WASM stack as bare
//! `(ptr: i32, len: i32)` pairs — no `MvlString` struct yet. Phase 2a
//! functions accept this shape directly. When we introduce heap-allocated
//! `MvlString` in a follow-up commit (needed for `.concat()` and other
//! ops that return fresh strings), the signatures will change to
//! `*MvlString` and existing call sites in the emitter will be updated
//! together.

/// Bytewise equality of two strings. Returns 1 when equal, 0 otherwise.
///
/// The emitter uses this for `assert_eq[String]` / `assert_ne[String]`
/// dispatch. Parameters are the exploded `(ptr, len)` representation
/// that the phase-1 emitter already leaves on the WASM stack for a
/// String value.
///
/// Safety: `ptr1..ptr1+len1` and `ptr2..ptr2+len2` must be readable
/// linear-memory ranges. The emitter always passes valid ranges — string
/// literals live in the module's data section, and `Int.to_string()`
/// output lives in the bump-allocated region.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _mvl_string_eq(ptr1: i32, len1: i32, ptr2: i32, len2: i32) -> i32 {
    if len1 != len2 {
        return 0;
    }
    // Two zero-length strings are equal by definition. Short-circuit
    // before touching pointers — the emitter never allocates a data-section
    // byte for `""`, so `ptr1` / `ptr2` may be 0 in that case, which
    // `slice::from_raw_parts` rejects under Rust's debug-assertion checks.
    if len1 == 0 {
        return 1;
    }
    let a = unsafe { core::slice::from_raw_parts(ptr1 as *const u8, len1 as usize) };
    let b = unsafe { core::slice::from_raw_parts(ptr2 as *const u8, len2 as usize) };
    if a == b {
        1
    } else {
        0
    }
}

// ── Tests ────────────────────────────────────────────────────────────────
//
// Compiled + run under wasm32-wasip1 so the i32-pointer ABI works as it
// does in production. `.cargo/config.toml` sets `runner = wasmtime run`
// for this target so `cargo test --target wasm32-wasip1 -p mvl_runtime_wasm`
// executes the test binaries under wasmtime.
//
// Host testing wouldn't work — WASM linear-memory offsets are i32, and
// truncating a 64-bit host pointer via `as i32` produces a bogus address
// that `slice::from_raw_parts` chokes on.

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    // Address any static byte string via its guest-memory offset — same
    // shape the emitter uses for string literals.
    fn addr(s: &'static [u8]) -> i32 {
        s.as_ptr() as usize as i32
    }

    #[test]
    fn eq_equal_strings() {
        let a = b"hello";
        let b = b"hello";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            1
        );
    }

    #[test]
    fn eq_different_content() {
        let a = b"hello";
        let b = b"world";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            0
        );
    }

    #[test]
    fn eq_different_lengths() {
        let a = b"hello";
        let b = b"hell";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), a.len() as i32, addr(b), b.len() as i32) },
            0
        );
    }

    #[test]
    fn eq_both_empty() {
        assert_eq!(unsafe { _mvl_string_eq(0, 0, 0, 0) }, 1);
    }

    #[test]
    fn eq_one_empty() {
        let a = b"x";
        assert_eq!(
            unsafe { _mvl_string_eq(addr(a), 1, 0, 0) },
            0
        );
    }
}
