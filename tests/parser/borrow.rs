//! Tests for Phase A + Phase B: last-use move elision and borrow inference
//! (Spec 009 Req 2, issues #234 and #365/#304).
//!
//! Phase A elides `.clone()` when a variable is used for the last time.
//! Phase B infers `&T` for read-only parameters, eliminating clones entirely
//! at call sites by emitting `&x` instead of `x` or `x.clone()`.
//!
//! Test taxonomy:
//! - `borrow_*`  — Phase B: callee param read-only → inferred as &T → call site emits &x
//! - `move_*`    — Phase A: callee NOT inferred (returned, etc.) → last use is a move
//! - `clone_*`   — cases where Phase B cannot infer a borrow, so Phase A still applies

use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler::transpile;

fn transpile_src(src: &str) -> String {
    let (mut parser, lex_errors) = Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {lex_errors:?}");
    let prog = parser.parse_program();
    assert!(
        parser.errors().is_empty(),
        "parse errors: {:?}",
        parser.errors()
    );
    transpile(&prog, "test_crate").lib_rs
}

fn assert_contains(src: &str, snippet: &str) {
    assert!(
        src.contains(snippet),
        "expected to find:\n  {snippet:?}\nin:\n{src}"
    );
}

fn assert_not_contains(src: &str, snippet: &str) {
    assert!(
        !src.contains(snippet),
        "expected NOT to find:\n  {snippet:?}\nin:\n{src}"
    );
}

// ── Phase B: read-only param → inferred borrow → &x at call sites ────────────

/// Phase B: single-use function param — callee only reads field → inferred &T.
/// Call site emits &b, not a move.
#[test]
fn borrow_single_use_param() {
    let src = r#"
type Buf = struct { data: String }
fn use_buf(b: Buf) -> String { b.data }
fn process(b: Buf) -> String { use_buf(b) }
"#;
    let rust = transpile_src(src);
    // use_buf's b is read-only (field access only) → inferred as &Buf.
    // process's b is passed to use_buf → disqualified → owned.
    assert_contains(&rust, "use_buf(&b)");
    assert_not_contains(&rust, "use_buf(b.clone())");
    assert_not_contains(&rust, "use_buf(b)");
}

/// Phase B: single-use local variable — callee inferred as &T → call emits &b.
#[test]
fn borrow_single_use_let_binding() {
    let src = r#"
type Buf = struct { data: String }
fn use_buf(b: Buf) -> String { b.data }
fn process(s: String) -> String {
    let b = Buf { data: s };
    use_buf(b)
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "use_buf(&b)");
    assert_not_contains(&rust, "use_buf(b.clone())");
}

/// Phase B: multiple uses — callee inferred as &T → all call sites use &b, no clones.
#[test]
fn borrow_multi_use_param_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn peek(b: Buf) -> String { b.data }
fn double(b: Buf) -> String {
    let a = peek(b);
    let c = peek(b);
    a
}
"#;
    let rust = transpile_src(src);
    // peek is inferred as &Buf → both call sites emit &b, no clone needed.
    assert_contains(&rust, "peek(&b)");
    assert_not_contains(&rust, "peek(b.clone())");
}

/// Phase B: sequence of calls — callee inferred as &T → no clone on any call.
#[test]
fn borrow_sequence_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn peek(b: Buf) -> String { b.data }
fn take_buf(b: Buf) -> String { b.data }
fn pipeline(b: Buf) -> String {
    let _ = peek(b);
    take_buf(b)
}
"#;
    let rust = transpile_src(src);
    // Both peek and take_buf are inferred as &Buf → all calls use &b.
    assert_contains(&rust, "peek(&b)");
    assert_contains(&rust, "take_buf(&b)");
    assert_not_contains(&rust, "peek(b.clone())");
    assert_not_contains(&rust, "take_buf(b.clone())");
}

/// Phase B: param used inside a for-loop — callee inferred as &T → &b at every call.
#[test]
fn borrow_var_used_in_loop_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn inspect(b: Buf) -> String { b.data }
fn repeat_n(b: Buf, n: Int) -> Unit {
    for _ in range(0, n) {
        inspect(b);
        ()
    }
}
"#;
    let rust = transpile_src(src);
    // inspect inferred as &Buf → call site emits &b, no clone.
    assert_contains(&rust, "inspect(&b)");
    assert_not_contains(&rust, "inspect(b.clone())");
}

/// Phase B: param used before and inside a for-loop — both call sites use &b.
#[test]
fn borrow_var_used_before_and_in_loop_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn size_of(b: Buf) -> Int { 0 }
fn inspect(b: Buf) -> String { b.data }
fn process(b: Buf) -> Unit {
    let n = size_of(b);
    for _ in range(0, n) {
        inspect(b);
        ()
    }
}
"#;
    let rust = transpile_src(src);
    // Both size_of and inspect inferred as &Buf → all calls use &b.
    assert_contains(&rust, "size_of(&b)");
    assert_contains(&rust, "inspect(&b)");
    assert_not_contains(&rust, "size_of(b.clone())");
    assert_not_contains(&rust, "inspect(b.clone())");
}

/// Phase B: param used inside a while-loop body — inferred borrow → &b at call.
#[test]
fn borrow_var_used_in_while_loop_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn inspect(b: Buf) -> String { b.data }
partial fn repeat_while(b: Buf, mut n: Int) -> Unit {
    while n > 0 {
        inspect(b);
        n = n - 1;
        ()
    }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "inspect(&b)");
    assert_not_contains(&rust, "inspect(b.clone())");
}

/// Phase B: param used only in while condition — callee inferred as &T → &b.
#[test]
fn borrow_var_used_only_in_while_condition_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn should_continue(b: Buf) -> Bool { true }
partial fn process(b: Buf, mut n: Int) -> Unit {
    while should_continue(b) {
        n = n - 1;
        ()
    }
}
"#;
    let rust = transpile_src(src);
    // should_continue is inferred as &Buf → &b, no clone.
    assert_contains(&rust, "should_continue(&b)");
    assert_not_contains(&rust, "should_continue(b.clone())");
}

/// Phase B: param used in both if-branches — inferred borrow → &b everywhere.
#[test]
fn borrow_var_used_in_both_if_branches_no_clone_needed() {
    let src = r#"
type Buf = struct { data: String }
fn take(b: Buf) -> String { b.data }
fn process(b: Buf, flag: Bool) -> String {
    if flag { take(b) } else { take(b) }
}
"#;
    let rust = transpile_src(src);
    // take inferred as &Buf → both branches use &b, no clone needed.
    assert_contains(&rust, "take(&b)");
    assert_not_contains(&rust, "take(b.clone())");
}

/// Phase B: param used in then-branch only — inferred borrow → &b.
#[test]
fn borrow_var_used_only_in_then_branch() {
    let src = r#"
type Buf = struct { data: String }
fn take(b: Buf) -> String { b.data }
fn process(b: Buf, flag: Bool) -> String {
    if flag { take(b) } else { "none" }
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "take(&b)");
    assert_not_contains(&rust, "take(b.clone())");
    assert_not_contains(&rust, "take(b)");
}

/// Phase B: param passed to functions in a match expression — inferred &T at call sites.
#[test]
fn borrow_scrutinee_call_and_arm_call_no_clone() {
    let src = r#"
type Buf = struct { data: String }
fn size(b: Buf) -> Int { 0 }
fn take(b: Buf) -> String { b.data }
fn process(b: Buf) -> String {
    match size(b) {
        0 => take(b),
        _ => "other"
    }
}
"#;
    let rust = transpile_src(src);
    // size and take are both inferred as &Buf → all calls use &b.
    assert_contains(&rust, "size(&b)");
    assert_contains(&rust, "take(&b)");
    assert_not_contains(&rust, "size(b.clone())");
    assert_not_contains(&rust, "take(b.clone())");
}

/// Phase B: struct field access — inferred borrow eliminates clones for multi-use.
#[test]
fn borrow_multi_use_field_access_no_clone() {
    let src = r#"
type Counter = struct { value: Int }
fn get(c: Counter) -> Int { c.value }
fn twice(c: Counter) -> Int {
    let a = get(c);
    let b = get(c);
    a
}
"#;
    let rust = transpile_src(src);
    // get inferred as &Counter → both calls emit &c, no clone.
    assert_contains(&rust, "get(&c)");
    assert_not_contains(&rust, "get(c.clone())");
}

/// Phase B: single use of field access — inferred borrow → &c.
#[test]
fn borrow_single_use_field_access() {
    let src = r#"
type Counter = struct { value: Int }
fn get(c: Counter) -> Int { c.value }
fn once(c: Counter) -> Int { get(c) }
"#;
    let rust = transpile_src(src);
    // get inferred as &Counter → single call emits &c.
    assert_contains(&rust, "get(&c)");
    assert_not_contains(&rust, "get(c.clone())");
    assert_not_contains(&rust, "get(c)");
}

// ── Phase A: move elision (callee NOT inferred as borrow) ─────────────────────

/// Phase A: callee returns its param → param disqualified from borrow inference.
/// The caller's single use of b results in a move (last-use elision).
#[test]
fn move_return_position() {
    let src = r#"
type Blob = struct { bytes: String }
fn wrap(b: Blob) -> Blob { b }
fn forward(b: Blob) -> Blob { wrap(b) }
"#;
    let rust = transpile_src(src);
    // wrap returns b → not inferred as borrow → forward's single use is a move.
    assert_contains(&rust, "wrap(b)");
    assert_not_contains(&rust, "wrap(b.clone())");
}

/// Phase A: String param returned directly → callee disqualified, caller moves.
#[test]
fn move_string_single_use() {
    let src = r#"
fn greet(name: String) -> String { name }
fn make_greeting(s: String) -> String { greet(s) }
"#;
    let rust = transpile_src(src);
    // greet returns name → not in borrow map → single use moved.
    assert_contains(&rust, "greet(s)");
    assert_not_contains(&rust, "greet(s.clone())");
}

// ── Phase A: outer var move with lambda ──────────────────────────────────────

/// Outer variable passed alongside a lambda is moved (used once outside any closure).
#[test]
fn move_outer_var_alongside_lambda() {
    let src = r#"
fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }
fn double(n: Int) -> Int {
    apply(|x: Int| x, n)
}
"#;
    let rust = transpile_src(src);
    // n: Int is Copy — never cloned regardless of phase.
    assert_not_contains(&rust, "n.clone()");
}
