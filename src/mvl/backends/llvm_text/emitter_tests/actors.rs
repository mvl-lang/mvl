// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emitter tests for `emit_actors.rs` (#1612 segmentation).
//!
//! When PR 2 of #1612 deletes the AST source file, the matching
//! `cross_backend_tir/actors.rs` substring tests cover the same
//! concern against the TIR walker.

use super::common::compile;

#[test]
fn actor_emits_state_struct_and_behavior_fn() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
    // State struct typedef.
    assert!(ir.contains("%CounterState = type"), "{ir}");
    // Behavior function.
    assert!(
        ir.contains("define void @counter_increment(ptr %self, i64 %n)"),
        "{ir}"
    );
}

#[test]
fn actor_emits_dispatch_function_with_switch() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
           pub fn reset() { }\n\
         }",
    );
    // Dispatch function signature.
    assert!(
        ir.contains("define void @counter_dispatch(ptr %state, i64 %disc, ptr %args)"),
        "{ir}"
    );
    // Switch with at least two case labels.
    assert!(ir.contains("switch i64 %disc, label %default"), "{ir}");
    assert!(ir.contains("i64 0, label %behavior_0"), "{ir}");
    assert!(ir.contains("i64 1, label %behavior_1"), "{ir}");
}

#[test]
fn actor_runtime_externs_emitted() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }",
    );
    assert!(ir.contains("declare ptr @_mvl_actor_spawn"), "{ir}");
    assert!(ir.contains("declare void @_mvl_actor_send"), "{ir}");
    assert!(ir.contains("declare void @_mvl_actor_join_all"), "{ir}");
}

#[test]
fn spawn_emits_alloca_and_actor_spawn_call() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           0\n\
         }",
    );
    // State alloca.
    assert!(ir.contains("alloca %CounterState"), "{ir}");
    // Runtime spawn call.
    assert!(ir.contains("call ptr @_mvl_actor_spawn"), "{ir}");
}

#[test]
fn actor_method_call_emits_send() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int {\n\
           let c: Counter = actor Counter { count: 0 };\n\
           c.increment(1);\n\
           0\n\
         }",
    );
    // The send call must appear.
    assert!(ir.contains("call void @_mvl_actor_send"), "{ir}");
}

#[test]
fn join_all_emitted_in_main_when_actors_present() {
    let ir = compile(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { }\n\
         }\n\
         fn main() -> Int { 0 }",
    );
    assert!(ir.contains("call void @_mvl_actor_join_all"), "{ir}");
}

// ── Synchronous `pub test fn` reads (#2012) ────────────────────────────────

/// `pub test fn` on an actor is a synchronous read, so the call site must go
/// through `_mvl_actor_sync_call` and yield a value. Emitting a plain
/// fire-and-forget `_mvl_actor_send` made every `12_actors` assertion a no-op.
#[test]
fn actor_test_fn_read_emits_sync_call() {
    let ir = super::common::compile_test_crate(
        "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { self.count = self.count + n }\n\
           pub test fn get_count() -> Int { self.count }\n\
         }\n\
         test fn reads_state() -> Unit ! Spawn + Send {\n\
             let c: Counter = actor Counter { count: 0 };\n\
             c.increment(5);\n\
             assert_eq(c.get_count(), 5);\n\
         }",
    );
    assert!(
        ir.contains("call i64 @_mvl_actor_sync_call"),
        "sync read must block on a reply, not fire-and-forget\n{ir}"
    );
    assert!(
        ir.contains("declare i64 @_mvl_actor_sync_call(ptr, i64, i64, ptr)"),
        "{ir}"
    );
    // The dispatch arm must hand the behavior's result back to the caller.
    assert!(
        ir.contains("call void @_mvl_actor_sync_reply"),
        "dispatch must answer the reply cell\n{ir}"
    );
    // And the assertion must actually compare something.
    assert!(
        ir.contains("icmp eq i64"),
        "assert_eq on an actor read must emit a comparison\n{ir}"
    );
}
