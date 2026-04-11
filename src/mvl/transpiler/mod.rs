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
//! // let out = transpile(&prog);
//! // println!("{}", out.lib_rs);
//! ```

pub mod cargo;
pub mod codegen;
pub mod emit_exprs;
pub mod emit_functions;
pub mod emit_stmts;
pub mod emit_types;

use crate::mvl::parser::ast::Program;
use codegen::Codegen;

/// Output of a successful transpilation.
pub struct TranspileOutput {
    /// Contents of `src/lib.rs` (or `src/main.rs` for binary crates).
    pub lib_rs: String,
    /// Contents of `Cargo.toml`.
    pub cargo_toml: String,
}

/// Transpile a parsed [`Program`] to Rust source.
///
/// Always succeeds in Phase 1 — unknown constructs fall back to `todo!()`.
pub fn transpile(prog: &Program, crate_name: &str) -> TranspileOutput {
    let mut cg = Codegen::new();
    cg.emit_program(prog);
    let lib_rs = cg.finish();
    let cargo_toml = cargo::emit_cargo_toml(crate_name);
    TranspileOutput { lib_rs, cargo_toml }
}
