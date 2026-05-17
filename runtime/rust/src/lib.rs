// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl_runtime` — the thin Rust crate that every MVL-generated file depends on.
//!
//! # Design constraints
//!
//! - **Zero runtime overhead** — all newtypes are `#[repr(transparent)]`.
//! - **Minimal dependencies** — only `sha2` and `hex` for crypto stdlib backing.
//! - **No unsafe code** — pure safe Rust.
//! - **Prelude** — generated files `use mvl_runtime::prelude::*` to get everything.
//!
//! # Contents
//!
//! | Module | What it provides |
//! |--------|-----------------|
//! | [`ifc`] | Security label newtypes: `Public<T>`, `Tainted<T>`, `Secret<T>`, `Clean<T>` |
//! | [`refine`] | `mvl_refine!` macro — debug assert for refinement predicates |
//! | [`prelude`] | Flat re-export of everything a generated file needs |

// `deny` (not `forbid`) so individual functions can use `#[allow(unsafe_code)]`
// for targeted POSIX C calls (e.g. getuid/getgid) where no safe std alternative exists.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod ifc;
pub mod prelude;
pub mod refine;
pub mod stdlib;
