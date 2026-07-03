// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Text-based LLVM IR backend for MVL — Phase 1 (issue #1111).
//!
//! Generates LLVM IR as a plain string without inkwell or any C FFI.
//! This is the enabling step for full self-hosting: string generation
//! is rewritable in MVL, inkwell calls are not.
//!
//! # Supported in Phase 1
//!
//! - Primitive types: `Int` (i64), `Float` (f64), `Bool` (i1), `Unit` (void)
//! - Integer/float/bool literals
//! - Arithmetic: `+`, `-`, `*`, `/`, `%`
//! - Comparisons: `==`, `!=`, `<`, `>`, `<=`, `>=`
//! - Logical: `&&`, `||`, `!`
//! - If/else expressions (via phi nodes)
//! - `let` bindings (immutable → SSA alias; mutable `ref` → alloca/load/store)
//! - While loops (via branch + phi)
//! - Function declarations and calls (primitive types only)
//! - A minimal `main()` that returns i32 0
//!
//! # Not yet supported (Phase 2+)
//!
//! - String literals and the MVL runtime string API
//! - Structs, enums, match expressions
//! - Collections (List, Map, Set)
//! - Closures / lambdas
//! - Standard library / prelude integration
//! - Overflow-checking arithmetic (uses plain `add i64` in Phase 1)
//!
//! # Example
//!
//! ```
//! use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
//! use mvl::mvl::ir::lower;
//! use mvl::mvl::parser::Parser;
//! use mvl::mvl::passes::mono;
//! use mvl::mvl::pipeline::assemble_expr_types;
//!
//! let src = "fn add(a: Int, b: Int) -> Int { a + b }";
//! let (mut p, _) = Parser::new(src);
//! let prog = p.parse_program();
//! let expr_types = assemble_expr_types(&prog, &[]);
//! let all_fns = mono::collect_fns([&prog]);
//! let m = mono::monomorphize(&prog, &all_fns, &expr_types);
//! let tir = lower::lower(&prog, &m, &expr_types);
//! let mut compiler = LlvmTextCompiler::new();
//! compiler.expr_types = expr_types;
//! let ir = compiler.compile_to_ir_tir(&tir, "test").unwrap();
//! assert!(ir.contains("define i64 @add"));
//! ```

pub mod c_symbols;
mod context;
pub mod dispatch;
mod emitter;
pub mod lli;

pub use emitter::LlvmTextCompiler;

use crate::mvl::ir::TypeExpr;

/// Dispatch metadata for a single MVL `builtin fn` consumed by the LLVM-text
/// backend.  Populated by `loader::collect_llvm_text_builtins` and stored in
/// `LlvmTextCompiler::builtin_symbols`.
///
/// Replaces the legacy anonymous 3-tuple
/// `(String, TypeExpr, Vec<TypeExpr>)` so callers read named fields instead
/// of relying on positional unpacking.
#[derive(Debug, Clone)]
pub struct BuiltinSymbolInfo {
    /// C-ABI symbol used in `declare`/`call` instructions (e.g.
    /// `"_mvl_str_split"`).  Derived from the source module + receiver type
    /// via [`c_symbols::derive_builtin_c_symbol`].
    pub c_sym: String,
    /// MVL return type of the builtin — sets the LLVM call return type.
    pub ret_ty: TypeExpr,
    /// MVL parameter types in declaration order (excluding the implicit
    /// receiver).
    pub param_tys: Vec<TypeExpr>,
}
