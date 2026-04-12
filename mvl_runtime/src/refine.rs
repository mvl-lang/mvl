//! Refinement type assertion macro.
//!
//! MVL Requirement 6: refinement types express value-level invariants.
//! This macro generates a `debug_assert!` in debug builds; in release builds
//! it compiles to nothing (zero overhead).
//!
//! # Usage
//!
//! ```rust
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
/// # use mvl_runtime::refine::mvl_refine;
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
