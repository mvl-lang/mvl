// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Lint rules for the MVL language, organized by category.
//!
//! * [`style`] — source-level style rules (trailing whitespace, line length, indentation, final newline)
//! * [`naming`] — naming convention and function-length rules
//! * [`semantic`] — semantic / code-smell rules
//! * [`suggestions`] — refactoring-suggestion rules
//! * [`documentation`] — documentation quality rules
//! * [`complexity`] — complexity metric rules

pub mod complexity;
pub mod documentation;
pub mod naming;
pub mod semantic;
pub mod style;
pub mod suggestions;

pub use complexity::*;
pub use documentation::*;
pub use naming::*;
pub use semantic::*;
pub use style::*;
pub use suggestions::*;
