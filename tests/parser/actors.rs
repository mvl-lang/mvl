//! Parser tests for Phase 8 actor syntax, select expression, and concurrently
//! block (issues #63 and #69).
//!
//! Tests cover:
//! - `actor Name { fields; pub fn behaviors; fn helpers }` declarations
//! - `actor Name { field: expr }` creation expressions (Expr::Spawn)
//! - `select { [binding =] expr => { body } … timeout(dur) => { body } }`
//! - `concurrently { … }` scope block

use mvl::mvl::parser::ast::{Decl, Expr};
use mvl::mvl::parser::Parser;

fn parse_decl(src: &str) -> Decl {
    let (mut p, lex_errs) = Parser::new(src);
    assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
    let d = p.parse_decl().expect("parse_decl failed");
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    d
}

fn parse_expr(src: &str) -> Expr {
    let (mut p, lex_errs) = Parser::new(src);
    assert!(lex_errs.is_empty(), "lex errors: {lex_errs:?}");
    let e = p.parse_expr().expect("parse_expr failed");
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    e
}

// ── Actor declaration (#63) ───────────────────────────────────────────────────

/// GIVEN: an actor with a pub fn behavior and a private fn helper
/// WHEN: parsed as a declaration
/// THEN: yields Decl::Actor with two methods
#[test]
fn actor_declaration_two_methods() {
    let src = r#"actor Counter {
        count: Int
        pub fn increment(val n: Int) { }
        fn get_count() -> Int { 0 }
    }"#;
    let d = parse_decl(src);
    let Decl::Actor(ad) = d else {
        panic!("expected Decl::Actor, got something else");
    };
    assert_eq!(ad.name, "Counter");
    assert_eq!(ad.methods.len(), 2, "expected 2 methods");
}

/// GIVEN: `pub fn` method inside actor
/// WHEN: parsed
/// THEN: `is_public` is true
#[test]
fn actor_pub_fn_is_public() {
    let src = r#"actor Counter {
        count: Int
        pub fn increment(val n: Int) { }
    }"#;
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    let method = &ad.methods[0];
    assert!(method.is_public, "pub fn should have is_public = true");
    assert_eq!(method.name, "increment");
}

/// GIVEN: `fn` method inside actor (private helper)
/// WHEN: parsed
/// THEN: `is_public` is false
#[test]
fn actor_private_fn_is_not_public() {
    let src = r#"actor Counter {
        count: Int
        fn get_count() -> Int { 0 }
    }"#;
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    let method = &ad.methods[0];
    assert!(!method.is_public, "fn should have is_public = false");
    assert_eq!(method.name, "get_count");
}

/// GIVEN: actor with no methods (field-only)
/// WHEN: parsed
/// THEN: methods vec is empty, fields are present
#[test]
fn actor_fields_only() {
    let src = "actor Store { value: Int }";
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    assert_eq!(ad.name, "Store");
    assert!(ad.methods.is_empty(), "expected no methods");
    assert_eq!(ad.fields.len(), 1);
}

/// GIVEN: actor with multiple fields separated by commas
/// WHEN: parsed
/// THEN: all fields captured
#[test]
fn actor_multiple_fields() {
    let src = "actor Config { host: String, port: Int }";
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    assert_eq!(ad.fields.len(), 2);
    assert_eq!(ad.fields[0].name, "host");
    assert_eq!(ad.fields[1].name, "port");
}

/// GIVEN: actor with `pub` visibility modifier
/// WHEN: parsed
/// THEN: visible = true
#[test]
fn actor_pub_visibility() {
    let src = "pub actor Logger { level: Int }";
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    assert!(ad.visible, "pub actor should have visible = true");
}

/// GIVEN: actor with `traps_exit` modifier
/// WHEN: parsed
/// THEN: `traps_exit` is true
#[test]
fn actor_traps_exit_flag_set() {
    let src = r#"actor Supervisor traps_exit {
        child_count: Int
        pub fn notify(val id: Int) { }
    }"#;
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    assert_eq!(ad.name, "Supervisor");
    assert!(
        ad.traps_exit,
        "actor with traps_exit should have traps_exit = true"
    );
}

/// GIVEN: actor without `traps_exit` modifier
/// WHEN: parsed
/// THEN: `traps_exit` is false
#[test]
fn actor_without_traps_exit_flag_is_false() {
    let src = "actor Worker { name: String }";
    let Decl::Actor(ad) = parse_decl(src) else {
        panic!("expected Decl::Actor");
    };
    assert!(
        !ad.traps_exit,
        "actor without traps_exit should have traps_exit = false"
    );
}

// ── Actor creation expression (#63) ──────────────────────────────────────────

/// GIVEN: `actor Counter { count: 0 }` in expression position
/// WHEN: parsed
/// THEN: yields Expr::Spawn with actor_type = "Counter" and one field
#[test]
fn actor_creation_expr_parsed() {
    let e = parse_expr("actor Counter { count: 0 }");
    let Expr::Spawn {
        actor_type, fields, ..
    } = e
    else {
        panic!("expected Expr::Spawn, got: {e:?}");
    };
    assert_eq!(actor_type, "Counter");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "count");
}

/// GIVEN: actor creation with multiple fields
/// WHEN: parsed
/// THEN: all fields are captured
#[test]
fn actor_creation_multiple_fields() {
    let e = parse_expr(r#"actor Config { port: 8080, debug: false }"#);
    let Expr::Spawn {
        actor_type, fields, ..
    } = e
    else {
        panic!("expected Expr::Spawn");
    };
    assert_eq!(actor_type, "Config");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].0, "port");
    assert_eq!(fields[1].0, "debug");
}

// ── select expression (#69) ──────────────────────────────────────────────────

/// GIVEN: `select { expr => { body } }` with one regular arm
/// WHEN: parsed
/// THEN: yields Expr::Select with one non-timeout arm
#[test]
fn select_single_arm_parsed() {
    let e = parse_expr("select { actor_ref.recv() => { 0 } }");
    let Expr::Select { arms, .. } = e else {
        panic!("expected Expr::Select, got: {e:?}");
    };
    assert_eq!(arms.len(), 1);
    assert!(!arms[0].is_timeout, "regular arm should not be timeout");
    assert!(arms[0].binding.is_none(), "no binding expected");
}

/// GIVEN: `select { result = expr => { body } }` with a binding
/// WHEN: parsed
/// THEN: arm has binding = Some("result")
#[test]
fn select_arm_with_binding_parsed() {
    let e = parse_expr("select { result = actor_ref.recv() => { result } }");
    let Expr::Select { arms, .. } = e else {
        panic!("expected Expr::Select");
    };
    assert_eq!(arms.len(), 1);
    assert_eq!(arms[0].binding.as_deref(), Some("result"));
    assert!(!arms[0].is_timeout);
}

/// GIVEN: `select { … timeout(5) => { body } }` with timeout arm
/// WHEN: parsed
/// THEN: last arm has is_timeout = true
#[test]
fn select_with_timeout_arm_parsed() {
    let e = parse_expr("select { actor_ref.recv() => { 1 }  timeout(5) => { 0 } }");
    let Expr::Select { arms, .. } = e else {
        panic!("expected Expr::Select");
    };
    assert_eq!(arms.len(), 2);
    assert!(!arms[0].is_timeout);
    assert!(arms[1].is_timeout, "last arm should be timeout");
}

/// GIVEN: `select` with multiple regular arms
/// WHEN: parsed
/// THEN: all arms captured, none marked as timeout
#[test]
fn select_multiple_arms_parsed() {
    let e = parse_expr("select { a.recv() => { 1 }  b.recv() => { 2 }  c.recv() => { 3 } }");
    let Expr::Select { arms, .. } = e else {
        panic!("expected Expr::Select");
    };
    assert_eq!(arms.len(), 3);
    assert!(arms.iter().all(|a| !a.is_timeout));
}

/// GIVEN: timeout-only select (no regular arms)
/// WHEN: parsed
/// THEN: one arm, is_timeout = true
#[test]
fn select_timeout_only_parsed() {
    let e = parse_expr("select { timeout(100) => { 0 } }");
    let Expr::Select { arms, .. } = e else {
        panic!("expected Expr::Select");
    };
    assert_eq!(arms.len(), 1);
    assert!(arms[0].is_timeout);
}

// ── pub test fn (#1506) ───────────────────────────────────────────────────────

/// GIVEN: actor with `pub test fn get() -> Int`
/// WHEN: parsed
/// THEN: method has is_public=true, is_test=true, non-Unit return type
#[test]
fn actor_pub_test_fn_parsed() {
    let src = "actor Counter { count: Int pub test fn get_count() -> Int { 0 } }";
    let d = parse_decl(src);
    let actor = match d {
        mvl::mvl::parser::ast::Decl::Actor(a) => a,
        other => panic!("expected Actor, got {other:?}"),
    };
    assert_eq!(actor.methods.len(), 1);
    let method = &actor.methods[0];
    assert!(method.is_public, "pub test fn should have is_public = true");
    assert!(method.is_test, "pub test fn should have is_test = true");
    assert_eq!(method.name, "get_count");
}

/// GIVEN: actor with `pub fn` (regular behavior)
/// WHEN: parsed
/// THEN: method has is_test=false
#[test]
fn actor_regular_pub_fn_has_is_test_false() {
    let src = "actor Counter { count: Int pub fn increment(val n: Int) { } }";
    let d = parse_decl(src);
    let actor = match d {
        mvl::mvl::parser::ast::Decl::Actor(a) => a,
        other => panic!("expected Actor, got {other:?}"),
    };
    let method = &actor.methods[0];
    assert!(!method.is_test, "pub fn should have is_test = false");
}

// ── concurrently removed (#1048) ─────────────────────────────────────────────
// `concurrently { }` is no longer a keyword. fn main() implicitly drains actors.

/// GIVEN: `concurrently` identifier (keyword removed in #1048)
/// WHEN: parsed as an expression
/// THEN: parsed as a plain identifier — concurrently is no longer a reserved keyword
#[test]
fn concurrently_keyword_removed() {
    let (mut p, _) = Parser::new("concurrently");
    let e = p.parse_expr().expect("should parse as identifier");
    assert!(
        matches!(e, Expr::Ident(ref name, _) if name == "concurrently"),
        "expected Expr::Ident('concurrently'), got: {e:?}"
    );
}
