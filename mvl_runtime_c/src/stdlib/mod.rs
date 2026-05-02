//! C-ABI wrappers for MVL stdlib modules (ADR-0018).
//!
//! Each submodule mirrors a `mvl_runtime::stdlib` module, exposing the same
//! surface area as C-ABI symbols callable from LLVM IR.
//!
//! Status:
//! - `env`     — scaffolded; implements when `mvl_runtime::stdlib::env` lands (#414)
//! - `process` — scaffolded; implements when `mvl_runtime::stdlib::process` lands (#414)

pub mod env;
pub mod process;
