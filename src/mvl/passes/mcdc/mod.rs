//! MC/DC (Modified Condition/Decision Coverage) pass.
//!
//! Split into two modules:
//! - `analysis`   — static obligation analysis (walks typed AST, no emission)
//! - `transform`  — runtime types, coupling analysis, and preamble/report helpers
//!
//! Note: both modules define a `DecisionKind` enum with overlapping but distinct
//! variants. Import from the appropriate submodule directly.

pub mod analysis;
pub mod transform;
