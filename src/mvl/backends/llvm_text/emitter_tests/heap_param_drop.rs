// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Regression tests for #1994 — heap-typed function parameters used to leak
//! unconditionally (never entered into `heap_locals`, so the callee never
//! dropped its own owned parameter), and call-site argument handling always
//! excluded the caller's binding from its own drop tracking regardless of
//! last-use or the callee's declared parameter capability.
//!
//! `assert_eq`-based corpus tests cannot observe a pure memory leak (no
//! crash, no wrong value — see `tests/corpus/07_ownership/heap_param_reuse_test.mvl`
//! for the functional-correctness counterpart), so these assert directly on
//! the shape of the emitted LLVM IR instead.

use super::common::compile;

/// Extract one function's `define ... { ... }` body from emitted IR text,
/// by name (e.g. `"@main"`). `declare` externs always trail all `define`s in
/// this emitter's output, so bounding at the first subsequent `declare` line
/// keeps the last function's chunk from swallowing them (a bare
/// `.split("define").find(...)` on the last function otherwise runs to EOF).
fn fn_body<'a>(ir: &'a str, name: &str) -> &'a str {
    let chunk = ir
        .split("define")
        .find(|f| f.contains(name))
        .unwrap_or_else(|| panic!("fn {name} not found in:\n{ir}"));
    chunk.split("\ndeclare").next().unwrap_or(chunk)
}

#[test]
fn owned_string_param_is_dropped_by_callee() {
    let ir = compile(
        r#"
        fn make() -> String { "hello" }
        fn show(s: String) -> Unit ! Console { println(s) }
        fn main() -> Unit ! Console {
            let s: String = make();
            show(s)
        }
        "#,
    );
    let show_fn = fn_body(&ir, "@show");
    assert!(
        show_fn.contains("call void @_mvl_string_drop"),
        "owned String parameter must be dropped by its own function: {ir}"
    );
}

#[test]
fn borrow_string_param_is_never_dropped_by_callee() {
    let ir = compile(
        r#"
        fn make() -> String { "hello" }
        fn peek(s: val String) -> Unit ! Console { println(s) }
        fn main() -> Unit ! Console {
            let s: String = make();
            peek(s);
            peek(s)
        }
        "#,
    );
    let peek_fn = fn_body(&ir, "@peek");
    assert!(
        !peek_fn.contains("_mvl_string_drop"),
        "a `val` (borrow) parameter must never be dropped by the callee: {ir}"
    );
    // No clone needed for a pure borrow, even across two calls — the
    // caller's binding is never at risk of being moved away.
    assert!(
        !ir.contains("_mvl_string_clone"),
        "a `val` (borrow) argument must never be cloned: {ir}"
    );
    // The caller drops its own binding exactly once, at scope exit.
    let main_fn = fn_body(&ir, "@main");
    assert_eq!(
        main_fn.matches("call void @_mvl_string_drop").count(),
        1,
        "caller must drop its own String local exactly once: {ir}"
    );
}

#[test]
fn owned_string_local_read_by_two_calls_clones_on_non_last_use() {
    let ir = compile(
        r#"
        fn make() -> String { "hello" }
        fn log(s: String) -> Unit ! Console { println(s) }
        fn process(s: String) -> Unit ! Console { println(s) }
        fn main() -> Unit ! Console {
            let s: String = make();
            log(s);
            process(s)
        }
        "#,
    );
    // Exactly one clone: the first (non-last) use.
    assert_eq!(
        ir.matches("call ptr @_mvl_string_clone").count(),
        1,
        "the first of two reads must clone, the second must move: {ir}"
    );
    let log_fn = fn_body(&ir, "@log(");
    let process_fn = fn_body(&ir, "@process(");
    assert!(
        log_fn.contains("call void @_mvl_string_drop"),
        "log must drop the clone it received: {ir}"
    );
    assert!(
        process_fn.contains("call void @_mvl_string_drop"),
        "process must drop the moved original it received: {ir}"
    );
    // `main` itself must not double-drop — ownership of the original moved
    // to `process` at its true last use.
    let main_fn = fn_body(&ir, "@main");
    assert!(
        !main_fn.contains("_mvl_string_drop"),
        "caller must not drop a value it moved away at last use: {ir}"
    );
}

#[test]
fn heap_local_read_inside_loop_always_clones_never_moves() {
    // A heap-typed local passed as a call argument inside a loop body has
    // only one textual occurrence, but that occurrence executes on every
    // iteration — `compute_last_uses` excludes any variable read inside a
    // loop entirely (never eligible as a last use), so the call site must
    // always clone, and the caller must retain and drop its own binding.
    let ir = compile(
        r#"
        fn make() -> String { "hello" }
        fn show(s: String) -> Int { s.len() }
        partial fn main() -> Unit ! Console {
            let s: String = make();
            let i: ref Int = 0;
            let sum: ref Int = 0;
            while i < 2 {
                sum = sum + show(s);
                i = i + 1;
            }
            println("done")
        }
        "#,
    );
    assert!(
        ir.contains("call ptr @_mvl_string_clone"),
        "a heap-typed local read inside a loop body must clone at the call site: {ir}"
    );
    let main_fn = fn_body(&ir, "@main");
    assert!(
        main_fn.contains("call void @_mvl_string_drop"),
        "caller must still own and drop its local after a loop-only read, since a looped read is never a last use: {ir}"
    );
}

#[test]
fn owned_heap_arg_passed_through_closure_call_is_not_double_freed() {
    // Regression test for a double-free found while reviewing #1994's fix:
    // the closure/HOF indirect-call path (`f(s)` where `f` is a fn-typed
    // parameter) never applied the caller-side last-use exclusion or
    // borrow/clone gating that direct calls got. Pre-#1994 this was safe by
    // coincidence (callees never dropped their own params, so the caller's
    // single scope-exit drop was correct). Once #1994 made callees drop
    // their own owned heap-typed params, the same pointer routed through an
    // indirect call got dropped twice: once by the wrapped callee's own
    // param tracking, once by the indirect caller's untouched heap_locals.
    let ir = compile(
        r#"
        fn make() -> String { "hello" }
        fn show(s: String) -> Int { s.len() }
        fn call_with(f: fn(String) -> Int, s: String) -> Int { f(s) }
        fn main() -> Unit ! Console {
            let s: String = make();
            let n: Int = call_with(show, s);
            if n > 0 { println("done") }
        }
        "#,
    );
    // Exactly one drop of the string across the whole program: inside
    // `show`, the true owner once the value is passed through. `call_with`
    // must not also drop it — it excluded `s` from its own heap_locals at
    // its true last use (the indirect call), the same as a direct call would.
    let call_with_fn = fn_body(&ir, "@call_with(");
    assert!(
        !call_with_fn.contains("_mvl_string_drop"),
        "call_with must not double-drop a value it passed away through an indirect call: {ir}"
    );
    let show_fn = fn_body(&ir, "@show(");
    assert_eq!(
        show_fn.matches("call void @_mvl_string_drop").count(),
        1,
        "show must drop the value it actually owns exactly once: {ir}"
    );
}
