// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI wrappers for MVL stdlib modules.
//!
//! Each sub-module mirrors a stdlib module from `mvl_runtime::stdlib::*`.
//! Every public function in the Rust implementation has a corresponding
//! `_mvl_*` export here that is callable from LLVM-generated code.

pub mod args;
pub mod crypto;
pub mod env;
pub mod io;
pub mod log;
pub mod net;
pub mod process;
pub mod random;
pub mod regex;
pub mod set;
pub mod time;
