//! MVL transpiler — emits Rust source from a parsed [`Program`].
//!
//! Phase 1: prototype transpilation to Rust.  Security labels become newtypes,
//! refinement predicates become `debug_assert!` guards, effects and totality
//! are preserved as doc comments.
//!
//! # Pipeline position
//!
//! ```text
//! Parser → [Type Checker] → Transpiler → Rust source → rustc / cargo
//! ```
//!
//! # Entry point
//!
//! ```
//! use mvl::mvl::transpiler::transpile;
//! use mvl::mvl::parser::ast::Program;
//!
//! // let prog: Program = …;
//! // let out = transpile(&prog, "my_crate");
//! // println!("{}", out.lib_rs);
//! ```

pub mod cargo;
pub mod codegen;
pub mod emit_exprs;
pub mod emit_functions;
pub mod emit_impls;
pub mod emit_stmts;
pub mod emit_types;

use crate::mvl::parser::ast::{Decl, Program};
use cargo::CargoOptions;
use codegen::Codegen;

/// Output of a successful transpilation.
pub struct TranspileOutput {
    /// Contents of `src/lib.rs` (library) or `src/main.rs` (binary with `fn main`).
    pub lib_rs: String,
    /// Contents of `Cargo.toml`.
    pub cargo_toml: String,
    /// True when the program declares `fn main` — the crate is a binary.
    pub has_main: bool,
    /// Number of extern trust boundaries (for assurance reporting).
    pub extern_count: usize,
    /// True when the program declares at least one `extern "rust"` block.
    pub has_extern_rust: bool,
}

/// Returns true if the program declares a top-level `fn main`.
pub fn has_main_fn(prog: &Program) -> bool {
    prog.declarations.iter().any(|d| {
        if let Decl::Fn(fd) = d {
            fd.name == "main"
        } else {
            false
        }
    })
}

/// Count extern declarations in a program.
pub fn count_extern_decls(prog: &Program) -> usize {
    prog.declarations
        .iter()
        .filter(|d| matches!(d, Decl::Extern(_)))
        .count()
}

/// Returns true if the program declares at least one `extern "rust"` block.
pub fn has_extern_rust_decls(prog: &Program) -> bool {
    prog.declarations
        .iter()
        .any(|d| matches!(d, Decl::Extern(ed) if ed.abi == "rust"))
}

/// Output of a successful multi-file project transpilation.
pub struct ProjectOutput {
    /// Contents of `src/main.rs` or `src/lib.rs` for the entry-point module.
    pub main_rs: String,
    /// Transpiled Rust source for each sibling module: `(module_name, source)`.
    /// Each entry should be written to `src/{module_name}.rs`.
    pub module_files: Vec<(String, String)>,
    /// Contents of `Cargo.toml`.
    pub cargo_toml: String,
    /// True when the entry program declares `fn main` — the crate is a binary.
    pub has_main: bool,
    /// Number of extern trust boundaries (for assurance reporting).
    pub extern_count: usize,
    /// True when the entry program declares at least one `extern "rust"` block.
    pub has_extern_rust: bool,
}

/// Transpile a multi-file project to Rust source.
///
/// `entry_name` is the crate/module name for the entry program.
/// `siblings` is a list of `(module_name, Program)` pairs for all other modules
/// reachable from the entry point (e.g. sibling `.mvl` files).
///
/// The entry module's output includes `pub mod name;` declarations for each sibling,
/// so the Rust compiler can resolve cross-module items.
pub fn transpile_project(
    entry_name: &str,
    entry_prog: &Program,
    siblings: &[(String, Program)],
) -> ProjectOutput {
    let has_main = has_main_fn(entry_prog);
    let extern_count = count_extern_decls(entry_prog);
    let has_extern_rust = has_extern_rust_decls(entry_prog);
    let use_runtime = extern_count > 0;

    let sibling_names: Vec<&str> = siblings.iter().map(|(n, _)| n.as_str()).collect();
    let mut cg = Codegen::new();
    cg.emit_program_with_mods(entry_prog, &sibling_names);
    let main_rs = cg.finish();

    // Sibling modules share the runtime prelude with the entry point so type
    // definitions don't conflict (e.g. `Tainted` from mvl_runtime vs inline).
    let entry_uses_runtime = extern_count > 0;
    let module_files: Vec<(String, String)> = siblings
        .iter()
        .map(|(name, prog)| {
            let mut cg = Codegen::new();
            if entry_uses_runtime {
                cg.emit_sibling_module(prog);
            } else {
                cg.emit_program(prog);
            }
            (name.clone(), cg.finish())
        })
        .collect();

    let opts = CargoOptions {
        crate_name: entry_name,
        use_mvl_runtime: use_runtime,
        extern_crates: Vec::new(),
    };
    let cargo_toml = if has_main {
        cargo::emit_cargo_toml_binary_opts(&opts)
    } else {
        cargo::emit_cargo_toml_library_opts(&opts)
    };

    ProjectOutput {
        main_rs,
        module_files,
        cargo_toml,
        has_main,
        extern_count,
        has_extern_rust,
    }
}

/// Transpile a parsed [`Program`] to Rust source.
///
/// Always succeeds in Phase 1 — unknown constructs fall back to `todo!()`.
pub fn transpile(prog: &Program, crate_name: &str) -> TranspileOutput {
    let has_main = has_main_fn(prog);
    let extern_count = count_extern_decls(prog);
    let has_extern_rust = has_extern_rust_decls(prog);
    let use_runtime = extern_count > 0;

    let mut cg = Codegen::new();
    cg.emit_program(prog);
    let lib_rs = cg.finish();

    let opts = CargoOptions {
        crate_name,
        use_mvl_runtime: use_runtime,
        extern_crates: Vec::new(),
    };
    let cargo_toml = if has_main {
        cargo::emit_cargo_toml_binary_opts(&opts)
    } else {
        cargo::emit_cargo_toml_library_opts(&opts)
    };
    TranspileOutput {
        lib_rs,
        cargo_toml,
        has_main,
        extern_count,
        has_extern_rust,
    }
}

// ── has_extern_rust unit tests ─────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    /// `has_extern_rust` is `true` when program contains `extern "rust"` block.
    #[test]
    fn has_extern_rust_true_for_rust_abi() {
        let prog = parse(r#"extern "rust" { fn foo() -> Int; }"#);
        assert!(has_extern_rust_decls(&prog));
        assert!(transpile(&prog, "crate").has_extern_rust);
    }

    /// `has_extern_rust` is `false` when program has no extern blocks at all.
    #[test]
    fn has_extern_rust_false_on_plain_program() {
        let prog = parse("fn add(a: Int, b: Int) -> Int { a + b }");
        assert!(!has_extern_rust_decls(&prog));
        let out = transpile(&prog, "crate");
        assert!(!out.has_extern_rust);
        // Regression guard: `mod bridge;` must NOT appear in output for non-extern programs.
        assert!(
            !out.lib_rs.contains("mod bridge;"),
            "mod bridge; must not appear for non-extern programs"
        );
    }

    /// `extern "c"` block does NOT set `has_extern_rust` (ABI discrimination).
    #[test]
    fn has_extern_rust_false_for_c_abi() {
        let prog = parse(r#"extern "c" { fn bar() -> Int; }"#);
        assert!(!has_extern_rust_decls(&prog));
        assert!(!transpile(&prog, "crate").has_extern_rust);
    }

    /// `has_extern_rust` is `false` when only `extern "c"` is present; `extern_count` is non-zero.
    #[test]
    fn extern_count_nonzero_but_has_extern_rust_false() {
        let prog = parse(r#"extern "c" { fn baz() -> Int; }"#);
        let out = transpile(&prog, "crate");
        assert_eq!(out.extern_count, 1);
        assert!(!out.has_extern_rust);
    }
}
