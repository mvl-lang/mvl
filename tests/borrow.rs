//! Tests for Phase A: last-use move elision (Spec 009 Req 2, issue #234).
//!
//! Phase A elides `.clone()` when the transpiler can prove a variable is used
//! for the last time in its scope.  This turns clone-on-pass into a move,
//! saving allocation for non-Copy types (structs, Lists, Strings, …).
//!
//! Test taxonomy:
//! - `move_*`  — cases where clone should be elided
//! - `clone_*` — cases where clone must be kept (multi-use, loops, lambdas)

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

// ── Phase A: move elision (single use) ───────────────────────────────────────

/// Single-use function parameter is moved into the call, not cloned.
#[test]
fn move_single_use_param() {
    let src = r#"
type Buf = struct { data: String }
fn use_buf(b: Buf) -> String { b.data }
fn process(b: Buf) -> String { use_buf(b) }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "use_buf(b)");
    assert_not_contains(&rust, "use_buf(b.clone())");
}

/// Single-use local variable is moved, not cloned.
#[test]
fn move_single_use_let_binding() {
    let src = r#"
type Buf = struct { data: String }
fn use_buf(b: Buf) -> String { b.data }
fn process(s: String) -> String {
    let b = Buf { data: s };
    use_buf(b)
}
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "use_buf(b)");
    assert_not_contains(&rust, "use_buf(b.clone())");
}

/// Last use in a sequence of calls: first call clones, last call moves.
#[test]
fn move_last_in_sequence() {
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
    // First use clones, second (last) use moves.
    assert_contains(&rust, "peek(b.clone())");
    assert_contains(&rust, "take_buf(b)");
    assert_not_contains(&rust, "take_buf(b.clone())");
}

/// Param used only in the return position is moved.
#[test]
fn move_return_position() {
    let src = r#"
type Blob = struct { bytes: String }
fn wrap(b: Blob) -> Blob { b }
fn forward(b: Blob) -> Blob { wrap(b) }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "wrap(b)");
    assert_not_contains(&rust, "wrap(b.clone())");
}

/// String variable used once is moved (no allocation on pass).
#[test]
fn move_string_single_use() {
    let src = r#"
fn greet(name: String) -> String { name }
fn make_greeting(s: String) -> String { greet(s) }
"#;
    let rust = transpile_src(src);
    assert_contains(&rust, "greet(s)");
    assert_not_contains(&rust, "greet(s.clone())");
}

// ── Phase A: clone kept (multi-use, loops, lambdas) ──────────────────────────

/// Variable used twice: first use clones, second moves.
#[test]
fn clone_multi_use_param() {
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
    // At least one of the two calls must clone.
    assert_contains(&rust, "peek(b.clone())");
}

/// Variable used inside a loop: always cloned (loop may iterate many times).
#[test]
fn clone_var_used_in_loop() {
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
    // b appears inside the for-loop — must always clone.
    assert_contains(&rust, "inspect(b.clone())");
}

/// Variable used both outside and inside a loop: the outside use must also clone
/// because the loop iteration keeps accessing the value.
#[test]
fn clone_var_used_before_and_in_loop() {
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
    // Both uses must clone — b is needed across all iterations.
    assert_contains(&rust, "size_of(b.clone())");
    assert_contains(&rust, "inspect(b.clone())");
}

/// Variable used inside a while loop body: always cloned.
#[test]
fn clone_var_used_in_while_loop() {
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
    assert_contains(&rust, "inspect(b.clone())");
}

/// Outer variable passed alongside a lambda is still subject to last-use move elision.
/// Phase A conservatively excludes lambda bodies from last-use tracking, but the
/// outer variable `n` (used once) is still eligible for move elision.
#[test]
fn move_outer_var_alongside_lambda() {
    let src = r#"
fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }
fn double(n: Int) -> Int {
    apply(|x: Int| x, n)
}
"#;
    let rust = transpile_src(src);
    // Phase A: `n` is used exactly once (outside any lambda/loop) — it is moved.
    // Note: the lambda body `x` picks up `.clone()` from the rvalue branch — that is
    // pre-existing behaviour, not a Phase A concern.
    assert_not_contains(&rust, "n.clone()");
}

// ── Correctness: value semantics preserved ───────────────────────────────────

/// After move elision, multi-use variables still clone to preserve MVL value semantics.
#[test]
fn value_semantics_preserved_multi_use() {
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
    // multi-use → clone on first, move on last
    assert_contains(&rust, "get(c.clone())");
}

/// After move elision, single-use variables are moved (no clone).
#[test]
fn value_semantics_preserved_single_use() {
    let src = r#"
type Counter = struct { value: Int }
fn get(c: Counter) -> Int { c.value }
fn once(c: Counter) -> Int { get(c) }
"#;
    let rust = transpile_src(src);
    // single use → move
    assert_contains(&rust, "get(c)");
    assert_not_contains(&rust, "get(c.clone())");
}
