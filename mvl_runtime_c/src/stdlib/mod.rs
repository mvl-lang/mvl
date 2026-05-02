//! C-ABI wrappers for MVL stdlib modules.
//!
//! Each sub-module mirrors a stdlib module from `mvl_runtime::stdlib::*`.
//! Every public function in the Rust implementation has a corresponding
//! `_mvl_*` export here that is callable from LLVM-generated code.

pub mod env;
pub mod process;
