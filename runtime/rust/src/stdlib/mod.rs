// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of MVL standard library functions.
//!
//! These are the real Rust-backed implementations for the stubs declared in
//! `std/*.mvl`. They are imported via explicit `use mvl_runtime::stdlib::X::*`
//! lines emitted by the transpiler for each `use std.X.*` declaration in the
//! MVL source (#488/#489). OS modules are NOT re-exported from `prelude`.
//!
//! # Design
//!
//! - Phase 2: stubs in `.mvl` files gain real Rust implementations here.
//! - Phase 3: implementations move to MVL source compiled from `.mvl` files.
//! - Minimal dependencies — `sha2`, `hex`, `getrandom` for crypto backing; everything else uses `std` only.

pub mod args;
pub mod crypto;
pub mod env;
pub mod io;
pub mod log;
pub mod net;
pub mod primitives;
pub mod process;
pub mod random;
pub mod regex;
pub mod time;
