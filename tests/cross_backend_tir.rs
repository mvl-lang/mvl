// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! IR-diff oracle for the LLVM-text backend's TIR walker (#1612).
//!
//! Per-concern files mirror `src/mvl/backends/llvm_text/emitter_tests/`
//! — each AST substring test has a TIR parity twin using the same input.

#[path = "cross_backend_tir/common.rs"]
mod common;

#[path = "cross_backend_tir/actors.rs"]
mod actors;

#[path = "cross_backend_tir/closures.rs"]
mod closures;

#[path = "cross_backend_tir/construct.rs"]
mod construct;

#[path = "cross_backend_tir/exprs.rs"]
mod exprs;

#[path = "cross_backend_tir/method_call.rs"]
mod method_call;

#[path = "cross_backend_tir/mono.rs"]
mod mono;

#[path = "cross_backend_tir/program.rs"]
mod program;

#[path = "cross_backend_tir/stmts.rs"]
mod stmts;

#[path = "cross_backend_tir/types.rs"]
mod types;
