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
//! use mvl::mvl::parser::Parser;
//!
//! let src = "fn add(a: Int, b: Int) -> Int { a + b }";
//! let (mut p, _) = Parser::new(src);
//! let prog = p.parse_program();
//! let ir = LlvmTextCompiler::new().compile_to_ir(&prog, "test").unwrap();
//! assert!(ir.contains("define i64 @add"));
//! ```

mod emitter;
pub mod lli;

pub use emitter::LlvmTextCompiler;
