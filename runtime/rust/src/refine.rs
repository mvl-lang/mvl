// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Refinement type assertion macro.
//!
//! MVL Requirement 6: refinement types express value-level invariants.
//! This macro generates a `debug_assert!` in debug builds; in release builds
//! it compiles to nothing (zero overhead).
//!
//! # Usage
//!
//! Transpiler-emitted code depends on the crate under the name `mvl_runtime`
//! (via a rename in the generated `Cargo.toml`), so illustrative snippets in
//! docs use that name. Doctests run against the actual crate name.
//!
//! ```text
//! use mvl_runtime::refine::mvl_refine;
//!
//! fn positive_int(x: i64) -> i64 {
//!     mvl_refine!(x > 0, "expected positive, got {}", x);
//!     x
//! }
//! ```

/// Assert a refinement predicate in debug mode, no-op in release mode.
///
/// Equivalent to `debug_assert!` but prefixed so it's easy to grep for
/// refinement checks in generated code.
///
/// ```rust
/// # use mvl_runtime_rust::refine::mvl_refine;
/// let x: i64 = 5;
/// mvl_refine!(x > 0);
/// ```
#[macro_export]
macro_rules! mvl_refine {
    ($cond:expr) => {
        debug_assert!($cond, "refinement violated: {}", stringify!($cond))
    };
    ($cond:expr, $fmt:literal $(, $arg:expr)*) => {
        debug_assert!($cond, $fmt $(, $arg)*)
    };
}

// Re-export at module path so users can do `use mvl_runtime::refine::mvl_refine`
pub use mvl_refine;

/// Runtime regex-membership check for refinement predicates (#1921).
///
/// Returns `true` if `haystack` matches the compiled `pattern`, `false` otherwise.
/// Panics if the pattern is invalid — but MVL's parse-time regex-fragment
/// validator (see `parser/regex_frag.rs`) rejects malformed / irregular patterns
/// before this point, so a panic here indicates a compiler bug.
///
/// Called from `mvl_refine!` expansions emitted for `self.matches("...")`
/// runtime checks. In release builds those asserts compile out, so the
/// per-call `Regex::new` cost is not on the hot path.
pub fn mvl_regex_matches(haystack: &str, pattern: &str) -> bool {
    ::regex::Regex::new(pattern)
        .expect(
            "mvl_regex_matches: pattern rejected by regex crate but accepted by MVL — compiler bug",
        )
        .is_match(haystack)
}

#[cfg(test)]
mod tests {
    #[test]
    fn refine_passes_on_true() {
        mvl_refine!(1 + 1 == 2);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "refinement violated")]
    fn refine_panics_in_debug_on_false() {
        mvl_refine!(1 + 1 == 3);
    }

    #[test]
    fn refine_with_message() {
        let x: i64 = 5;
        mvl_refine!(x > 0, "expected positive, got {}", x);
    }
}
