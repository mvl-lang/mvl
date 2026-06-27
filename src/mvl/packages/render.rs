// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared manifest-walk helpers for `audit.rs` and `sbom.rs` (#1564).
//!
//! Both modules iterate the same `Manifest.native` / `Manifest.c_native`
//! HashMaps and the optional `SourceFile` slice; without a shared helper they
//! diverged on whether to sort (sbom did, audit did not — leaving audit output
//! nondeterministic across runs).  These helpers enforce alphabetical order
//! by name/path so every renderer produces a stable output sequence.
//!
//! The helpers intentionally do not unify the per-entry rendering — audit and
//! SBOM have genuinely different domain needs (CVE matching vs. hash + URL +
//! license) and a common struct would create more friction than it removes.
//! Sorted iteration is the only shared concern.

use std::collections::HashMap;

use super::manifest::CNativeSpec;
use super::sbom::SourceFile;

/// Iterate `[native]` Rust crate dependencies sorted by name.
///
/// Replaces the inline `sort_by_key` calls in `audit::scan_all`,
/// `sbom::cyclonedx`, and `sbom::spdx`.
pub fn iter_native_sorted(native: &HashMap<String, String>) -> Vec<(&str, &str)> {
    let mut entries: Vec<(&str, &str)> = native
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    entries.sort_by_key(|(k, _)| *k);
    entries
}

/// Iterate `[c-native]` C library dependencies sorted by name.
///
/// Audit previously iterated `c_native` in HashMap order — output was
/// nondeterministic across runs.  Sorting here is the canonical order.
pub fn iter_c_native_sorted(c_native: &HashMap<String, CNativeSpec>) -> Vec<(&str, &CNativeSpec)> {
    let mut entries: Vec<(&str, &CNativeSpec)> =
        c_native.iter().map(|(k, v)| (k.as_str(), v)).collect();
    entries.sort_by_key(|(k, _)| *k);
    entries
}

/// Sort source files by relative path.  Used by SBOM emitters; centralised
/// here so the rule lives next to its siblings.
pub fn iter_source_files_sorted(sources: &[SourceFile]) -> Vec<&SourceFile> {
    let mut sorted: Vec<&SourceFile> = sources.iter().collect();
    sorted.sort_by_key(|s| s.rel_path.as_str());
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iter_native_sorted_orders_alphabetically() {
        let mut m = HashMap::new();
        m.insert("zlib".to_string(), "1.3.0".to_string());
        m.insert("aho-corasick".to_string(), "1.1.2".to_string());
        m.insert("openssl".to_string(), "0.10.66".to_string());
        let v = iter_native_sorted(&m);
        assert_eq!(
            v.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec!["aho-corasick", "openssl", "zlib"]
        );
    }

    #[test]
    fn iter_c_native_sorted_orders_alphabetically() {
        let mut m = HashMap::new();
        m.insert(
            "zstd".to_string(),
            CNativeSpec {
                version: "1.5.0".to_string(),
                license: None,
            },
        );
        m.insert(
            "openssl".to_string(),
            CNativeSpec {
                version: "3.0.0".to_string(),
                license: Some("Apache-2.0".to_string()),
            },
        );
        let v = iter_c_native_sorted(&m);
        assert_eq!(
            v.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec!["openssl", "zstd"]
        );
    }

    #[test]
    fn iter_native_sorted_empty_map_returns_empty_vec() {
        let m: HashMap<String, String> = HashMap::new();
        assert!(iter_native_sorted(&m).is_empty());
    }
}
