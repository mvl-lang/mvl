//! Native behavioral coverage pass.
//!
//! Tracks branch hit counts at runtime. The transform module injects
//! `crate::__mvl_cov::hit(id)` calls at each decision branch and provides
//! the preamble/report helpers consumed by the Rust-backend emitter.

pub mod transform;
pub use transform::*;
