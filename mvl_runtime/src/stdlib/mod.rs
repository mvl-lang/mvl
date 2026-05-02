//! Rust implementations of MVL standard library functions.
//!
//! These are the real Rust-backed implementations for the stubs declared in
//! `std/*.mvl`. They are re-exported via `mvl_runtime::prelude::*` so that
//! any generated MVL program that imports `use std.*` can call them without
//! needing a per-program `bridge.rs`.
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
pub mod primitives;
pub mod process;
