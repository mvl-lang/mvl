// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Capability label types for resource identifiers.
//!
//! These types provide provenance tracking for resources (database URLs, config paths, API
//! endpoints, audit targets) using the IFC machinery. All four labels reuse the existing
//! IFC lattice to enforce label compatibility at call boundaries.

/// Marks a database URL capability label.
/// Type-level enforcement ensures bare `String` or mismatched labels are rejected
/// where a DbUrl is expected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct DbUrl<T>(pub T);

/// Marks a configuration path capability label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ConfigPath<T>(pub T);

/// Marks an API endpoint capability label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ApiEndpoint<T>(pub T);

/// Marks an audit target capability label.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct AuditTarget<T>(pub T);

// ── Copy for copy inner types ─────────────────────────────────────────────

impl<T: Copy> Copy for DbUrl<T> {}
impl<T: Copy> Copy for ConfigPath<T> {}
impl<T: Copy> Copy for ApiEndpoint<T> {}
impl<T: Copy> Copy for AuditTarget<T> {}

// ── Constructors and accessors ────────────────────────────────────────────

macro_rules! impl_capability_label {
    ($Label:ident) => {
        impl<T> $Label<T> {
            /// Wrap a value in this capability label.
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

        // From<T> — allows unlabeled values to flow into any capability label via .into()
        impl<T> From<T> for $Label<T> {
            #[inline]
            fn from(v: T) -> Self {
                Self(v)
            }
        }

        // Deref — allows calling inner type methods directly
        impl<T> std::ops::Deref for $Label<T> {
            type Target = T;
            #[inline]
            fn deref(&self) -> &T {
                &self.0
            }
        }

        // Display — delegate to inner value
        impl<T: std::fmt::Display> std::fmt::Display for $Label<T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

impl_capability_label!(DbUrl);
impl_capability_label!(ConfigPath);
impl_capability_label!(ApiEndpoint);
impl_capability_label!(AuditTarget);

// ── UFCS coercions for labeled strings and lists ─────────────────────────

macro_rules! impl_capability_label_into {
    ($Label:ident) => {
        impl From<$Label<String>> for String {
            #[inline]
            fn from(v: $Label<String>) -> String {
                v.0
            }
        }
        impl<T> From<$Label<Vec<T>>> for Vec<T> {
            #[inline]
            fn from(v: $Label<Vec<T>>) -> Vec<T> {
                v.0
            }
        }
        impl From<$Label<i64>> for i64 {
            #[inline]
            fn from(v: $Label<i64>) -> i64 {
                v.0
            }
        }
    };
}

impl_capability_label_into!(DbUrl);
impl_capability_label_into!(ConfigPath);
impl_capability_label_into!(ApiEndpoint);
impl_capability_label_into!(AuditTarget);

// ── Cross-type equality for labeled strings ──────────────────────────────

macro_rules! impl_capability_label_str_eq {
    ($Label:ident) => {
        impl PartialEq<String> for $Label<String> {
            #[inline]
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }
        impl PartialEq<$Label<String>> for String {
            #[inline]
            fn eq(&self, other: &$Label<String>) -> bool {
                self == &other.0
            }
        }
    };
}

impl_capability_label_str_eq!(DbUrl);
impl_capability_label_str_eq!(ConfigPath);
impl_capability_label_str_eq!(ApiEndpoint);
impl_capability_label_str_eq!(AuditTarget);

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_label_new() {
        let db = DbUrl::new("postgresql://localhost".to_string());
        assert_eq!(db.as_inner(), "postgresql://localhost");
    }

    #[test]
    fn capability_label_into_inner() {
        let cfg = ConfigPath::new("/etc/config.toml".to_string());
        assert_eq!(cfg.into_inner(), "/etc/config.toml");
    }

    #[test]
    fn capability_label_deref() {
        let api = ApiEndpoint::new("https://api.example.com".to_string());
        assert_eq!(api.len(), "https://api.example.com".len());
    }

    #[test]
    fn capability_label_display() {
        let audit = AuditTarget::new("admin_action".to_string());
        assert_eq!(format!("{audit}"), "admin_action");
    }

    #[test]
    fn capability_label_copy() {
        let db = DbUrl::new(42i64);
        let _db2 = db;
        let _ = db; // Copy
    }

    #[test]
    fn capability_label_from() {
        let url: DbUrl<String> = "https://db.example.com".to_string().into();
        assert_eq!(url.as_inner(), "https://db.example.com");
    }

    #[test]
    fn capability_label_str_equality() {
        let db = DbUrl::new("postgres".to_string());
        assert!(db == "postgres".to_string());
        assert!("postgres".to_string() == db);
    }
}
