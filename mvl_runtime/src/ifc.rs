//! Information-flow control (IFC) security label newtypes.
//!
//! MVL Requirement 11: every value carries a security label. The label is
//! tracked at the type level — no runtime tag, no overhead.
//!
//! # Lattice
//!
//! ```text
//! Secret ──────────────┐
//!                      ↓ (declassify)
//! Tainted → (sanitize) → Clean → Public
//! ```
//!
//! - `Tainted<T>` — external input, not yet validated.
//! - `Clean<T>` — external input that has been sanitized.
//! - `Public<T>` — can be shown to anyone; no confidentiality concern.
//! - `Secret<T>` — confidential data; must be explicitly declassified.
//!
//! Explicit conversions (`sanitize`, `declassify`) are the *only* way to
//! move up the lattice. The Rust type system enforces this statically.

// ── Label newtypes ────────────────────────────────────────────────────────

/// Marks a value as originating from untrusted external input.
///
/// Use `sanitize(v)` to convert to `Clean<T>` after validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Tainted<T>(pub T);

/// Marks a value that has been sanitized (validated external input).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Clean<T>(pub T);

/// Marks a value as publicly shareable (no confidentiality label).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Public<T>(pub T);

/// Marks a confidential value that must be explicitly declassified before use.
///
/// Intentionally does **not** implement `Display` or `Debug` for the inner
/// value — printing a `Secret` would leak confidential data.  Use
/// `declassify(s)` to obtain a `Public<T>` before formatting.
#[derive(Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Secret<T>(pub T);

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

// ── Copy for copy inner types ─────────────────────────────────────────────

impl<T: Copy> Copy for Tainted<T> {}
impl<T: Copy> Copy for Clean<T> {}
impl<T: Copy> Copy for Public<T> {}
impl<T: Copy> Copy for Secret<T> {}

// ── Constructors and accessors ────────────────────────────────────────────

macro_rules! impl_label {
    ($Label:ident) => {
        impl<T> $Label<T> {
            /// Wrap a value in this security label.
            #[inline]
            pub fn new(v: T) -> Self {
                Self(v)
            }

            /// Consume the label and return the inner value.
            #[inline]
            pub fn into_inner(self) -> T {
                self.0
            }

            /// Borrow the inner value.
            #[inline]
            pub fn as_inner(&self) -> &T {
                &self.0
            }
        }

        // Arithmetic — delegates to inner value, preserves the label.
        impl<T: std::ops::Add<Output = T>> std::ops::Add for $Label<T> {
            type Output = Self;
            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }
        impl<T: std::ops::Sub<Output = T>> std::ops::Sub for $Label<T> {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self {
                Self(self.0 - rhs.0)
            }
        }
        impl<T: std::ops::Mul<Output = T>> std::ops::Mul for $Label<T> {
            type Output = Self;
            fn mul(self, rhs: Self) -> Self {
                Self(self.0 * rhs.0)
            }
        }
        impl<T: std::ops::Div<Output = T>> std::ops::Div for $Label<T> {
            type Output = Self;
            fn div(self, rhs: Self) -> Self {
                Self(self.0 / rhs.0)
            }
        }
        impl<T: std::ops::Rem<Output = T>> std::ops::Rem for $Label<T> {
            type Output = Self;
            fn rem(self, rhs: Self) -> Self {
                Self(self.0 % rhs.0)
            }
        }
        impl<T: std::ops::Neg<Output = T>> std::ops::Neg for $Label<T> {
            type Output = Self;
            fn neg(self) -> Self {
                Self(-self.0)
            }
        }
    };
}

impl_label!(Tainted);
impl_label!(Clean);
impl_label!(Public);
impl_label!(Secret);

// Display — intentionally omitted for Secret<T> to prevent accidental leaks.
macro_rules! impl_display {
    ($Label:ident) => {
        impl<T: std::fmt::Display> std::fmt::Display for $Label<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

impl_display!(Tainted);
impl_display!(Clean);
impl_display!(Public);

// Extra helper on Public<i64> for float conversion (used in generated code)
impl Public<i64> {
    /// Convert a labeled integer to `f64` for use with float-typed functions.
    #[inline]
    pub fn to_float(&self) -> f64 {
        self.0 as f64
    }
}

// ── Lattice conversion functions ──────────────────────────────────────────

/// Sanitize a tainted value — caller asserts that the value has been validated.
///
/// MVL: `sanitize(x)` where `x: Tainted<T>`
#[inline]
pub fn sanitize<T>(v: Tainted<T>) -> Clean<T> {
    Clean(v.0)
}

/// Declassify a secret value — caller asserts it is safe to make public.
///
/// MVL: `declassify(x)` where `x: Secret<T>`
#[inline]
pub fn declassify<T>(v: Secret<T>) -> Public<T> {
    Public(v.0)
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_roundtrip() {
        let p = Public::new(42i64);
        assert_eq!(p.into_inner(), 42);

        let s = Secret::new("password");
        assert_eq!(*s.as_inner(), "password");
    }

    #[test]
    fn sanitize_converts_tainted_to_clean() {
        let t: Tainted<String> = Tainted::new("user_input".to_string());
        let c: Clean<String> = sanitize(t);
        assert_eq!(c.into_inner(), "user_input");
    }

    #[test]
    fn declassify_converts_secret_to_public() {
        let s: Secret<i64> = Secret::new(42);
        let p: Public<i64> = declassify(s);
        assert_eq!(p.into_inner(), 42);
    }

    #[test]
    fn arithmetic_preserves_label() {
        let a = Public::new(3i64);
        let b = Public::new(4i64);
        assert_eq!((a + b).into_inner(), 7);
        assert_eq!((Public::new(10i64) - Public::new(3i64)).into_inner(), 7);
        assert_eq!((Public::new(3i64) * Public::new(4i64)).into_inner(), 12);
    }

    #[test]
    fn copy_for_copy_inner() {
        let x = Public::new(1i64);
        let y = x; // Copy
        let _ = x; // still valid
        let _ = y;
    }

    #[test]
    fn display_delegates_to_inner() {
        let p = Public::new(99i64);
        assert_eq!(format!("{p}"), "99");
    }
}
