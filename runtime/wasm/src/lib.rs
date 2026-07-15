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

// ── Host tests ────────────────────────────────────────────────────────────
//
// End-to-end coverage lives in the WASM corpus (mvlr → wasmtime →
// runtime.wasm). We can't unit-test `_mvl_string_eq` on the host because
// its parameters are i32 (WASM linear-memory offsets), which don't map to
// host 64-bit pointers. `#[cfg(target_arch = "wasm32")]` would gate
// wasm-only host tests but there's nothing meaningful to check that the
// corpus doesn't already exercise.
//
// The zero-length short-circuit below is the one branch worth calling
// out — it's why we can't just blindly `from_raw_parts` on the incoming
// offsets, because the emitter never allocates a data-section byte for
// `""`, so `ptr` may be 0.
