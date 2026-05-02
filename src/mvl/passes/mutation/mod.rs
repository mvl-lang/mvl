//! Native behavioral mutation testing pass.
//!
//! Tracks mutation points and provides report formatting. The transform
//! module supplies operator/literal mutation tables and the `MutationMap`
//! accumulator consumed by the Rust-backend emitter.

pub mod transform;
pub use transform::*;
