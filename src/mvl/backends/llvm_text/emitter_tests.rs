// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Segmented emitter tests — partitioned by source-file concern.
//!
//! | Source file              | Test file                   |
//! |--------------------------|-----------------------------|
//! | `emitter.rs` (top-level) | `emitter_tests/program.rs`  |
//! | `emit_exprs.rs`          | `emitter_tests/exprs.rs`    |
//! | `emit_stmts.rs`          | `emitter_tests/stmts.rs`    |
//! | `emit_types.rs`          | `emitter_tests/types.rs`    |
//! | `emit_construct.rs`      | `emitter_tests/construct.rs`|
//! | `emit_closures.rs`       | `emitter_tests/closures.rs` |
//! | `emit_actors.rs`         | `emitter_tests/actors.rs`   |
//! | `emit_method_call.rs`    | `emitter_tests/method_call.rs` |
//! | `emit_mono.rs`           | `emitter_tests/mono.rs`     |
//!
//! When PR 2 of #1612 deletes each `emit_<concern>.rs`, the matching
//! `emitter_tests/<concern>.rs` is co-deletable.

#[path = "emitter_tests/common.rs"]
mod common;

#[path = "emitter_tests/actors.rs"]
mod actors;

#[path = "emitter_tests/closures.rs"]
mod closures;

#[path = "emitter_tests/construct.rs"]
mod construct;

#[path = "emitter_tests/exprs.rs"]
mod exprs;

#[path = "emitter_tests/method_call.rs"]
mod method_call;

#[path = "emitter_tests/mono.rs"]
mod mono;

#[path = "emitter_tests/program.rs"]
mod program;

#[path = "emitter_tests/stmts.rs"]
mod stmts;

#[path = "emitter_tests/types.rs"]
mod types;
