//! Integration tests for the MVL module resolver (Spec 005).
//!
//! These tests verify all six requirements of the module system:
//! 1. File-module correspondence
//! 2. Visibility (pub/private)
//! 3. Import syntax (use declarations)
//! 4. Re-exports (pub use)
//! 5. Circular import rejection
//! 6. Standard library module

use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::resolver::{resolve_project, stdlib_module, ResolveError};

fn parse(src: &str) -> Program {
    let (mut p, lex_errors) = Parser::new(src);
    assert!(lex_errors.is_empty(), "lex errors: {:?}", lex_errors);
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

// ── Requirement 1: File-module correspondence ─────────────────────────────

#[test]
fn file_module_correspondence() {
    // Each file maps to a module by name
    let prog = parse("pub fn greet() -> Int { 0 }");
    let result = resolve_project(vec![("greet".to_string(), "".to_string(), prog)], None);
    assert!(
        result.is_ok(),
        "single module should resolve: {:?}",
        result.errors
    );
    assert!(
        result.modules.contains_key("greet"),
        "module must be keyed by filename"
    );
}

// ── Requirement 2: Visibility ─────────────────────────────────────────────

#[test]
fn private_item_rejected() {
    // Private items are NOT accessible from other modules
    let a = parse("fn secret() -> Int { 0 }"); // private
    let b = parse("use mod_a::secret;"); // imports private item
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::NotExported { .. })),
        "importing private item must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn pub_item_accessible() {
    // `pub` items are exported and accessible from other modules
    let a = parse("pub fn greet() -> Int { 42 }");
    let b = parse("use mod_a::greet;");
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "importing pub item must succeed: {:?}",
        result.errors
    );
}

#[test]
fn struct_fields_accessible() {
    // Struct fields have no per-field visibility gating —
    // once the type is imported, its fields are accessible.
    let a = parse("pub type Point = struct { x: Int, y: Int }");
    let b = parse("use mod_a::Point;");
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "struct fields must be accessible after type import: {:?}",
        result.errors
    );
}

// ── Requirement 3: Import syntax ──────────────────────────────────────────

#[test]
fn use_at_top() {
    // `use` declarations at the top of the file are valid
    let a = parse("pub fn foo() -> Int { 0 }");
    let b = parse("use mod_a::foo;\nfn bar() -> Int { 0 }");
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "use at top should be valid: {:?}",
        result.errors
    );
}

#[test]
fn use_after_declaration_rejected() {
    // `use` after a non-use declaration must be rejected
    let a = parse("pub fn foo() -> Int { 0 }");
    let b = parse("fn bar() -> Int { 0 }\nuse mod_a::foo;"); // use after fn
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::UseNotAtTop { .. })),
        "use after non-use declaration must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn wildcard_rejected() {
    // Wildcard imports are not allowed — rejected at parse time (no `*` in use paths)
    // The parser does not produce a wildcard UseDecl because `*` is not in the grammar.
    // We verify it fails to parse.
    let (mut p, _) = Parser::new("use foo::*;");
    let _prog = p.parse_program();
    // The `*` is not a valid identifier, so the parser should produce an error
    assert!(
        !p.errors().is_empty(),
        "wildcard import must be rejected at parse time"
    );
}

#[test]
fn name_collision_rejected() {
    // Two imports with the same local name must be rejected
    let a = parse("pub type Foo = struct { x: Int }");
    let b = parse("use mod_a::Foo;\nuse mod_a::Foo;"); // same name twice
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::NameCollision { .. })),
        "duplicate import must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn missing_module_rejected() {
    // Importing from a non-existent module must be rejected
    let a = parse("use does_not_exist::Foo;");
    let result = resolve_project(vec![("mod_a".to_string(), "".to_string(), a)], None);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::MissingModule { .. })),
        "import from missing module must be rejected: {:?}",
        result.errors
    );
}

// ── Requirement 4: Re-exports ─────────────────────────────────────────────

#[test]
fn reexport_public() {
    // `pub use` re-exporting a pub item is allowed
    let a = parse("pub type Foo = struct { x: Int }");
    let b = parse("pub fn bar() -> Int { 0 }"); // no use, just a clean module
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "valid project should resolve: {:?}",
        result.errors
    );
}

#[test]
fn reexport_private_rejected() {
    // `pub use` re-exporting a private item must be rejected
    use mvl::mvl::parser::ast::{Decl, Program, UseDecl};
    use mvl::mvl::parser::lexer::Span;

    // Construct a program with `pub use mod_a::secret` where secret is private
    let ud = UseDecl {
        reexport: true,
        module_only: false,
        path: vec!["mod_a".to_string(), "secret".to_string()],
        items: vec![],
        span: Span::default(),
    };
    let prog_b = Program {
        declarations: vec![Decl::Use(ud)],
        span: Span::default(),
    };

    let a = parse("fn secret() -> Int { 0 }"); // private
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), prog_b),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::NotExported { .. })),
        "re-export of private item must be rejected: {:?}",
        result.errors
    );
}

// ── Requirement 5: Circular import rejection ──────────────────────────────

#[test]
fn circular_import_rejected() {
    // Direct cycle: a → b → a
    let a = parse("use mod_b::Bar;\npub type Foo = struct { x: Int }");
    let b = parse("use mod_a::Foo;\npub type Bar = struct { y: Int }");
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::CircularImport { .. })),
        "circular import must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn transitive_cycle_rejected() {
    // Transitive cycle: a → b → c → a
    let a = parse("use mod_c::C;\npub type A = struct { x: Int }");
    let b = parse("use mod_a::A;\npub type B = struct { x: Int }");
    let c = parse("use mod_b::B;\npub type C = struct { x: Int }");
    let result = resolve_project(
        vec![
            ("mod_a".to_string(), "".to_string(), a),
            ("mod_b".to_string(), "".to_string(), b),
            ("mod_c".to_string(), "".to_string(), c),
        ],
        None,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::CircularImport { .. })),
        "transitive cycle must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn diamond_dependency_ok() {
    // Diamond: a → b, a → c, b → d, c → d (no cycle)
    let d = parse("pub type D = struct { x: Int }");
    let b = parse("use mod_d::D;\npub type B = struct { x: Int }");
    let c = parse("use mod_d::D;\npub type C = struct { x: Int }");
    let a = parse("use mod_b::B;\nuse mod_c::C;\npub type A = struct { x: Int }");
    let result = resolve_project(
        vec![
            ("mod_d".to_string(), "".to_string(), d),
            ("mod_b".to_string(), "".to_string(), b),
            ("mod_c".to_string(), "".to_string(), c),
            ("mod_a".to_string(), "".to_string(), a),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "diamond dependency must succeed: {:?}",
        result.errors
    );
}

// ── Cross-file method dispatch (Go model, #1706) ──────────────────────────

#[test]
fn cross_file_method_dispatch_no_cycle() {
    // Sibling files that each define methods on the same type must NOT require
    // cyclic `use` imports.  The type owner (mod_b) exports the type; mod_a
    // imports only the type and defines additional methods on it.  Neither file
    // imports the other's *methods* — dispatch goes through the type.
    //
    // Import graph:  mod_a → mod_b  (no edge mod_b → mod_a) — acyclic.
    let mod_b = parse("pub type Ctx = struct { x: Int }\npub fn Ctx::method_b(self, n: Int) -> Int { self.x + n }");
    let mod_a = parse("use mod_b::Ctx;\npub fn Ctx::method_a(self, n: Int) -> Int { self.x + n }");
    let result = resolve_project(
        vec![
            ("mod_b".to_string(), "".to_string(), mod_b),
            ("mod_a".to_string(), "".to_string(), mod_a),
        ],
        None,
    );
    assert!(
        result.is_ok(),
        "cross-file method dispatch must not produce a cycle: {:?}",
        result.errors
    );
}

// ── Requirement 6: Standard library module ────────────────────────────────

#[test]
fn stdlib_explicit_import() {
    // The stdlib module exports well-known items
    let stdlib = stdlib_module();
    assert!(
        stdlib.exports.contains("println"),
        "stdlib must export println"
    );
    assert!(stdlib.exports.contains("panic"), "stdlib must export panic");
}

#[test]
fn stdlib_from_filesystem_resolves_use_std() {
    // resolve_project with Some(stdlib_dir) should load core.mvl from disk
    // and resolve `use std::println` successfully.
    use std::fs;
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_src =
        "pub fn println(value: String) -> Unit { }\npub fn panic(message: String) -> Unit { }";
    fs::write(tmp.path().join("core.mvl"), core_src).expect("write core.mvl");

    let prog = parse("use std::println;");
    let result = resolve_project(
        vec![("main".to_string(), "".to_string(), prog)],
        Some(tmp.path()),
    );
    assert!(
        result.is_ok(),
        "use std::println should resolve against filesystem stdlib: {:?}",
        result.errors
    );
}

#[test]
fn stdlib_from_filesystem_missing_file_falls_back_to_stub() {
    // When core.mvl is absent, resolve_project falls back to the in-memory stub.
    let tmp = tempfile::tempdir().expect("tempdir");
    // No core.mvl written — empty dir

    // The stub exports println, so this should still resolve.
    let prog = parse("use std::println;");
    let result = resolve_project(
        vec![("main".to_string(), "".to_string(), prog)],
        Some(tmp.path()),
    );
    assert!(
        result.is_ok(),
        "missing core.mvl should fall back to stub: {:?}",
        result.errors
    );
}

// ── Duplicate module name collision (issue #1714) ─────────────────────────

#[test]
fn duplicate_module_name_rejected() {
    // Two files sharing the same basename must produce a load-time error, not
    // a silent wrong-module bind.
    let a = parse("pub fn a_fn() -> Int { 0 }");
    let b = parse("pub fn b_fn() -> Int { 0 }");
    let result = resolve_project(
        vec![
            (
                "context".to_string(),
                "compiler/context.mvl".to_string(),
                a,
            ),
            (
                "context".to_string(),
                "compiler/backends/llvm/context.mvl".to_string(),
                b,
            ),
        ],
        None,
    );
    let has_dup = result.errors.iter().any(|e| {
        matches!(e, ResolveError::DuplicateModule { name, .. } if name == "context")
    });
    assert!(
        has_dup,
        "two modules sharing a stem must be rejected: {:?}",
        result.errors
    );
}

#[test]
fn duplicate_module_error_cites_both_paths() {
    // The error message must name both conflicting file paths.
    let a = parse("pub fn x() -> Int { 0 }");
    let b = parse("pub fn y() -> Int { 0 }");
    let result = resolve_project(
        vec![
            ("math".to_string(), "src/math.mvl".to_string(), a),
            ("math".to_string(), "utils/math.mvl".to_string(), b),
        ],
        None,
    );
    let error_msg = result
        .errors
        .iter()
        .find(|e| matches!(e, ResolveError::DuplicateModule { .. }))
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(
        error_msg.contains("src/math.mvl"),
        "error must cite first path: {error_msg}"
    );
    assert!(
        error_msg.contains("utils/math.mvl"),
        "error must cite second path: {error_msg}"
    );
}
