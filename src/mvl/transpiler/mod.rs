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

/// Transpile a parsed [`Program`] to Rust source.
///
/// Always succeeds in Phase 1 — unknown constructs fall back to `todo!()`.
pub fn transpile(prog: &Program, crate_name: &str) -> TranspileOutput {
    let has_main = has_main_fn(prog);
    let extern_count = count_extern_decls(prog);
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
    }
}
