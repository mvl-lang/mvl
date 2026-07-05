// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl package check` — validate `mvl.toml` manifest completeness (#1698).
//!
//! Complements `mvl audit --supply-chain` (which already knows every
//! `[native]` / `[c-native]` entry a package declares) by requiring each
//! such entry to also carry a valid SPDX `license`. Without this, license
//! audits silently ignore the real license surface of a package that
//! wraps external code — see the motivating example in issue #1698 where
//! `pkg-sqlite` wraps `rusqlite` (MIT) + SQLite (blessing) but
//! `mvl audit --license` reports no findings.

use super::error::PackageError;
use super::manifest::{LicensePolicy, Manifest};
use std::path::Path;

/// A single problem detected by `mvl package check`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckIssue {
    /// Section the entry lives in: `native` or `c-native`.
    pub section: String,
    /// Dependency name.
    pub name: String,
    /// Human-readable description of the problem.
    pub message: String,
}

/// Result of a `mvl package check` run.
#[derive(Debug, Default)]
pub struct CheckReport {
    pub issues: Vec<CheckIssue>,
}

impl CheckReport {
    /// True when the manifest is clean.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// Render the report to a human-readable string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.issues.is_empty() {
            out.push_str("Package check: manifest OK\n");
            return out;
        }
        out.push_str("Package check: issues found\n");
        for iss in &self.issues {
            out.push_str(&format!(
                "  [{:<8}] {:<32} {}\n",
                iss.section, iss.name, iss.message
            ));
        }
        out.push_str(&format!("\n  {} issue(s)\n", self.issues.len()));
        out
    }
}

/// `mvl package check` — walk the manifest and report every external
/// dependency that lacks a valid license declaration.
///
/// Rules (each becomes a `CheckIssue`):
/// - Every `[native]` entry MUST have a `license` (SPDX id).
/// - Every `[c-native]` entry MUST have a `license` (SPDX id).
/// - Declared licenses MUST be a recognized SPDX identifier
///   (see `LicensePolicy::is_known_spdx`).
/// - A name appearing in both `[native]` and `[c-native]` is a conflict.
///
/// Detection is purely manifest-local: we walk the same entries
/// `mvl audit --supply-chain` walks, so if that command sees a dep, this
/// command will too. No build-graph inspection.
pub fn cmd_package_check(project_root: &Path) -> Result<CheckReport, PackageError> {
    let manifest = Manifest::load(project_root)?;
    Ok(check_manifest(&manifest))
}

/// Pure function form of `cmd_package_check` for unit testing.
pub fn check_manifest(manifest: &Manifest) -> CheckReport {
    let mut issues = Vec::new();

    // Collect and sort names so output is deterministic across HashMap iterations.
    let mut native_names: Vec<&String> = manifest.native.keys().collect();
    native_names.sort();
    let mut cnative_names: Vec<&String> = manifest.c_native.keys().collect();
    cnative_names.sort();

    for name in &native_names {
        match manifest.native_licenses.get(*name) {
            None => issues.push(CheckIssue {
                section: "native".to_string(),
                name: (*name).clone(),
                message: "missing 'license' field — declare the SPDX id of the wrapped crate"
                    .to_string(),
            }),
            Some(lic) if !LicensePolicy::is_known_spdx(lic) => issues.push(CheckIssue {
                section: "native".to_string(),
                name: (*name).clone(),
                message: format!("unrecognized SPDX license '{lic}'"),
            }),
            _ => {}
        }
    }

    for name in &cnative_names {
        let spec = &manifest.c_native[*name];
        match &spec.license {
            None => issues.push(CheckIssue {
                section: "c-native".to_string(),
                name: (*name).clone(),
                message: "missing 'license' field — declare the SPDX id of the linked C library"
                    .to_string(),
            }),
            Some(lic) if !LicensePolicy::is_known_spdx(lic) => issues.push(CheckIssue {
                section: "c-native".to_string(),
                name: (*name).clone(),
                message: format!("unrecognized SPDX license '{lic}'"),
            }),
            _ => {}
        }
    }

    // Cross-section duplicate: same name in both [native] and [c-native]
    // signals confusion about what kind of dep it is.
    for name in &native_names {
        if manifest.c_native.contains_key(*name) {
            issues.push(CheckIssue {
                section: "cross".to_string(),
                name: (*name).clone(),
                message: "declared in both [native] and [c-native] — pick one".to_string(),
            });
        }
    }

    CheckReport { issues }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Manifest {
        Manifest::parse(content).unwrap()
    }

    #[test]
    fn clean_manifest_produces_no_issues() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"
"#,
        );
        let r = check_manifest(&m);
        assert!(r.is_clean(), "got: {:?}", r.issues);
    }

    #[test]
    fn native_without_license_is_reported() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[native]
rusqlite = "0.31"
"#,
        );
        let r = check_manifest(&m);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].section, "native");
        assert_eq!(r.issues[0].name, "rusqlite");
        assert!(r.issues[0].message.contains("missing 'license'"));
    }

    #[test]
    fn native_with_license_is_ok() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[native]
rusqlite = { version = "0.31", license = "MIT" }
"#,
        );
        let r = check_manifest(&m);
        assert!(r.is_clean(), "got: {:?}", r.issues);
    }

    #[test]
    fn native_with_unknown_spdx_is_reported() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[native]
rusqlite = { version = "0.31", license = "MIT-Typo" }
"#,
        );
        let r = check_manifest(&m);
        assert_eq!(r.issues.len(), 1);
        assert!(r.issues[0].message.contains("unrecognized SPDX"));
    }

    #[test]
    fn c_native_without_license_is_reported() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"
extern-rationale = "links openssl"

[c-native]
openssl = "3.0"
"#,
        );
        let r = check_manifest(&m);
        assert_eq!(r.issues.len(), 1);
        assert_eq!(r.issues[0].section, "c-native");
        assert_eq!(r.issues[0].name, "openssl");
    }

    #[test]
    fn c_native_with_license_is_ok() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"
extern-rationale = "links sqlite"

[c-native]
sqlite3 = { version = "3.44", license = "blessing" }
"#,
        );
        let r = check_manifest(&m);
        assert!(r.is_clean(), "got: {:?}", r.issues);
    }

    #[test]
    fn full_pkg_sqlite_example_is_clean() {
        // The motivating example from #1698: pkg-sqlite wraps rusqlite (MIT)
        // and SQLite (blessing). With both declared, check must pass.
        let m = parse(
            r#"
[package]
name = "pkg-sqlite"
version = "0.1.0"
license = "Apache-2.0"
requires-mvl = ">=0.1.0"
extern-rationale = "wraps rusqlite + embedded SQLite"

[native]
rusqlite = { version = "0.31", license = "MIT" }

[c-native]
sqlite3 = { version = "3.44", license = "blessing" }
"#,
        );
        let r = check_manifest(&m);
        assert!(r.is_clean(), "got: {:?}", r.issues);
    }

    #[test]
    fn cross_section_duplicate_is_reported() {
        let m = parse(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"
extern-rationale = "test"

[native]
foo = { version = "1.0", license = "MIT" }

[c-native]
foo = { version = "1.0", license = "MIT" }
"#,
        );
        let r = check_manifest(&m);
        let cross = r
            .issues
            .iter()
            .find(|i| i.section == "cross")
            .expect("expected cross-section issue");
        assert_eq!(cross.name, "foo");
    }

    #[test]
    fn render_clean_report() {
        let r = CheckReport::default();
        let s = r.render();
        assert!(s.contains("manifest OK"));
    }

    #[test]
    fn render_report_with_issues() {
        let r = CheckReport {
            issues: vec![CheckIssue {
                section: "native".to_string(),
                name: "rusqlite".to_string(),
                message: "missing 'license' field".to_string(),
            }],
        };
        let s = r.render();
        assert!(s.contains("issues found"));
        assert!(s.contains("rusqlite"));
        assert!(s.contains("1 issue"));
    }
}
