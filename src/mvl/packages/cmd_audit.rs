// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl audit` — supply-chain, license, and Dependency Paradox audits.

use super::audit;
use super::error::PackageError;
use super::fetch::resolve_pkg_dir;
use super::lock::LockFile;
use super::manifest::{load_cached_manifest, DepSpec, Manifest};
use std::path::Path;

/// `mvl audit --supply-chain`
///
/// Scans `[native]` and `[c-native]` dependencies against vulnerability
/// databases (NVD for C libs, OSV for both), mapping CVEs to MVL requirement
/// gaps (#633).
pub fn cmd_audit_supply_chain(
    project_root: &Path,
) -> Result<audit::SupplyChainAudit, PackageError> {
    let manifest = Manifest::load(project_root)?;
    Ok(audit::scan_all(&manifest.native, &manifest.c_native))
}

// ── License audit (#635) ─────────────────────────────────────────────────────

/// Result of auditing a single dependency for license compliance.
#[derive(Debug)]
pub struct LicenseEntry {
    /// Dependency name.
    pub name: String,
    /// Section: "dependency", "native", or "c-native".
    pub section: String,
    /// License expression, or "unknown" if absent.
    pub license: String,
    /// Policy check result.
    pub status: LicenseStatus,
    /// Name of the MVL package that introduced this entry, if transitive.
    /// `None` for project-direct entries (#1701).
    pub introduced_by: Option<String>,
}

/// License compliance status for a single dependency.
#[derive(Debug, PartialEq)]
pub enum LicenseStatus {
    /// License is compatible with policy.
    Compatible,
    /// License is incompatible with policy (reason provided).
    Rejected(String),
    /// License was rejected but overridden via `--allow-license`.
    Overridden(String),
    /// License is unknown (not declared).
    Unknown,
}

/// Summary of a license audit.
#[derive(Debug)]
pub struct LicenseAudit {
    pub entries: Vec<LicenseEntry>,
    pub policy_mode: String,
}

impl LicenseAudit {
    /// Number of rejected licenses (excluding overrides).
    pub fn rejected_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.status, LicenseStatus::Rejected(_)))
            .count()
    }

    /// Number of unknown licenses.
    pub fn unknown_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.status, LicenseStatus::Unknown))
            .count()
    }

    /// True when the audit should fail as a CI gate.
    ///
    /// Rejected licenses always fail. Unknown (undeclared) licenses also fail
    /// unless the policy mode is `"any"` — a package with no declared license
    /// is more uncertain than one with a known incompatible license, and a
    /// CI gate that ignores it can be bypassed by shipping a package without
    /// an `mvl.toml` license field.
    pub fn has_violations(&self) -> bool {
        if self.rejected_count() > 0 {
            return true;
        }
        self.policy_mode != "any" && self.unknown_count() > 0
    }

    /// Render the audit report to a string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("License audit (policy: {}):\n", self.policy_mode));

        let mut entries: Vec<&LicenseEntry> = self.entries.iter().collect();
        entries.sort_by(|a, b| a.section.cmp(&b.section).then(a.name.cmp(&b.name)));

        for e in &entries {
            let status_str = match &e.status {
                LicenseStatus::Compatible => "ok".to_string(),
                LicenseStatus::Rejected(reason) => format!("REJECTED ({reason})"),
                LicenseStatus::Overridden(reason) => format!("overridden ({reason})"),
                LicenseStatus::Unknown => "UNKNOWN".to_string(),
            };
            let name_col = match &e.introduced_by {
                Some(pkg) => format!("{} (via {})", e.name, pkg),
                None => e.name.clone(),
            };
            out.push_str(&format!(
                "  [{:<10}] {:<40} {:<20} {}\n",
                e.section, name_col, e.license, status_str
            ));
        }
        out.push('\n');

        let rejected = self.rejected_count();
        let unknown = self.unknown_count();
        if rejected > 0 {
            out.push_str(&format!(
                "  {} license{} rejected.\n",
                rejected,
                if rejected == 1 { "" } else { "s" }
            ));
        }
        if unknown > 0 {
            out.push_str(&format!(
                "  {} unknown license{}.\n",
                unknown,
                if unknown == 1 { "" } else { "s" }
            ));
        }
        if rejected == 0 && unknown == 0 {
            out.push_str("  All licenses compatible.\n");
        }
        out
    }
}

/// `mvl audit --license`
///
/// Checks all dependency licenses against the project's license policy (#635).
/// - MVL deps: reads license from `mvl.lock` (set by `mvl add`).
/// - Project-direct `[native]` and `[c-native]`: reads license from the
///   manifest's inline table.
/// - Transitive `[native]` and `[c-native]` (#1701): reads each cached MVL
///   dep's `mvl.toml` and rolls those declarations up, tagging each entry
///   with the MVL package that introduced it.
pub fn cmd_audit_license(project_root: &Path) -> Result<LicenseAudit, PackageError> {
    let manifest = Manifest::load(project_root)?;
    let lockfile = LockFile::load_or_empty(project_root);
    Ok(audit_licenses(&manifest, &lockfile, &load_cached_manifest))
}

/// Pure form of `cmd_audit_license` with the cached-manifest loader injected.
///
/// Kept public within the crate so tests can supply an in-memory loader
/// instead of touching the on-disk global package cache.
pub(crate) fn audit_licenses(
    manifest: &Manifest,
    lockfile: &LockFile,
    load_dep_manifest: &dyn Fn(&str, &str) -> Option<Manifest>,
) -> LicenseAudit {
    let policy = &manifest.license_policy;

    let mut entries = Vec::new();

    // Check MVL dependencies (from lock file)
    for lp in &lockfile.packages {
        let license_str = match (&lp.license, &lp.allow_license_override) {
            (Some(lic), Some(reason)) => {
                entries.push(LicenseEntry {
                    name: lp.name.clone(),
                    section: "dependency".to_string(),
                    license: lic.clone(),
                    status: LicenseStatus::Overridden(reason.clone()),
                    introduced_by: None,
                });
                continue;
            }
            (Some(lic), None) => lic.clone(),
            (None, _) => {
                // Try reading from cached package mvl.toml
                match load_dep_manifest(&lp.name, &lp.version) {
                    Some(m) => m.package.license,
                    None => {
                        entries.push(LicenseEntry {
                            name: lp.name.clone(),
                            section: "dependency".to_string(),
                            license: "unknown".to_string(),
                            status: LicenseStatus::Unknown,
                            introduced_by: None,
                        });
                        continue;
                    }
                }
            }
        };

        let status = match policy.check(&license_str) {
            Ok(()) => LicenseStatus::Compatible,
            Err(reason) => LicenseStatus::Rejected(reason),
        };
        entries.push(LicenseEntry {
            name: lp.name.clone(),
            section: "dependency".to_string(),
            license: license_str,
            status,
            introduced_by: None,
        });
    }

    // Project-direct [native] license declarations (#1698).
    for name in manifest.native.keys() {
        entries.push(native_entry(
            name,
            manifest.native_licenses.get(name).map(|s| s.as_str()),
            policy,
            None,
        ));
    }

    // Project-direct [c-native] declarations.
    for (name, spec) in &manifest.c_native {
        entries.push(cnative_entry(name, spec.license.as_deref(), policy, None));
    }

    // Transitive [native] / [c-native] from each cached MVL dep (#1701).
    // Sorted iteration keeps output deterministic across HashMap iteration order.
    let mut sorted_deps: Vec<&super::lock::LockedPackage> = lockfile.packages.iter().collect();
    sorted_deps.sort_by(|a, b| a.name.cmp(&b.name));
    for lp in sorted_deps {
        let dep_manifest = match load_dep_manifest(&lp.name, &lp.version) {
            Some(m) => m,
            None => continue, // dep not in cache; already flagged above as unknown
        };

        let mut nat_names: Vec<&String> = dep_manifest.native.keys().collect();
        nat_names.sort();
        for name in nat_names {
            // Skip names the project already declares directly to avoid
            // double-reporting; the direct entry wins.
            if manifest.native.contains_key(name) {
                continue;
            }
            entries.push(native_entry(
                name,
                dep_manifest.native_licenses.get(name).map(|s| s.as_str()),
                policy,
                Some(&lp.name),
            ));
        }

        let mut cnat_names: Vec<&String> = dep_manifest.c_native.keys().collect();
        cnat_names.sort();
        for name in cnat_names {
            if manifest.c_native.contains_key(name) {
                continue;
            }
            let spec = &dep_manifest.c_native[name];
            entries.push(cnative_entry(
                name,
                spec.license.as_deref(),
                policy,
                Some(&lp.name),
            ));
        }
    }

    LicenseAudit {
        entries,
        policy_mode: policy.mode_str().to_string(),
    }
}

fn native_entry(
    name: &str,
    license: Option<&str>,
    policy: &super::manifest::LicensePolicy,
    introduced_by: Option<&str>,
) -> LicenseEntry {
    match license {
        Some(lic) => {
            let status = match policy.check(lic) {
                Ok(()) => LicenseStatus::Compatible,
                Err(reason) => LicenseStatus::Rejected(reason),
            };
            LicenseEntry {
                name: name.to_string(),
                section: "native".to_string(),
                license: lic.to_string(),
                status,
                introduced_by: introduced_by.map(str::to_string),
            }
        }
        None => LicenseEntry {
            name: name.to_string(),
            section: "native".to_string(),
            license: "unknown".to_string(),
            status: LicenseStatus::Unknown,
            introduced_by: introduced_by.map(str::to_string),
        },
    }
}

fn cnative_entry(
    name: &str,
    license: Option<&str>,
    policy: &super::manifest::LicensePolicy,
    introduced_by: Option<&str>,
) -> LicenseEntry {
    match license {
        Some(lic) => {
            let status = match policy.check(lic) {
                Ok(()) => LicenseStatus::Compatible,
                Err(reason) => LicenseStatus::Rejected(reason),
            };
            LicenseEntry {
                name: name.to_string(),
                section: "c-native".to_string(),
                license: lic.to_string(),
                status,
                introduced_by: introduced_by.map(str::to_string),
            }
        }
        None => LicenseEntry {
            name: name.to_string(),
            section: "c-native".to_string(),
            license: "unknown".to_string(),
            status: LicenseStatus::Unknown,
            introduced_by: introduced_by.map(str::to_string),
        },
    }
}

// ── Dependency Paradox audit (#637) ──────────────────────────────────────────

/// Result of auditing a single dependency for the Dependency Paradox.
#[derive(Debug)]
pub struct ParadoxEntry {
    /// Dependency name.
    pub name: String,
    /// Estimated lines of code (from cached source tree), or `None` if unavailable.
    pub loc: Option<u64>,
    /// The rationale string, if provided in `mvl.toml`.
    pub rationale: Option<String>,
    /// Whether this dep is below the complexity threshold.
    pub below_threshold: bool,
}

/// Summary of a Dependency Paradox audit.
#[derive(Debug)]
pub struct ParadoxAudit {
    pub entries: Vec<ParadoxEntry>,
    pub threshold: u64,
    pub rationale_required: bool,
}

impl ParadoxAudit {
    /// Number of deps below threshold that lack a rationale.
    pub fn missing_rationale_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.below_threshold && e.rationale.is_none())
            .count()
    }

    /// Number of deps below threshold (regardless of rationale).
    pub fn below_threshold_count(&self) -> usize {
        self.entries.iter().filter(|e| e.below_threshold).count()
    }

    /// True when the audit should fail as a CI gate.
    pub fn has_violations(&self) -> bool {
        self.rationale_required && self.missing_rationale_count() > 0
    }

    /// Render the audit report to a string.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("Dependency Paradox audit:\n");
        let mut entries: Vec<&ParadoxEntry> = self.entries.iter().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for e in &entries {
            let loc_str = match e.loc {
                Some(n) => format!("{:>6} LOC", n),
                None => "     ? LOC".to_string(),
            };
            let rationale_str = match &e.rationale {
                Some(r) => format!("rationale: \"{}\"", r),
                None => "rationale: missing".to_string(),
            };
            let status = if e.rationale.is_some() {
                "ok"
            } else if e.below_threshold {
                "MISSING"
            } else {
                "warn"
            };
            out.push_str(&format!(
                "  {:<40} {loc_str}  — {rationale_str}  {status}\n",
                e.name
            ));
        }
        out.push('\n');
        let below = self.below_threshold_count();
        if below > 0 {
            out.push_str(&format!(
                "  {} dependenc{} below complexity threshold ({} LOC).\n",
                below,
                if below == 1 { "y" } else { "ies" },
                self.threshold
            ));
        }
        let missing = self.missing_rationale_count();
        if missing > 0 {
            out.push_str(&format!(
                "  {} missing rationale{}.\n",
                missing,
                if missing == 1 { "" } else { "s" }
            ));
        }
        if below == 0 && missing == 0 {
            out.push_str("  All dependencies above complexity threshold or have rationale.\n");
        }
        out
    }
}

/// `mvl audit --paradox`
///
/// Audits all dependencies for the Dependency Paradox policy (#637).
/// Returns an audit result that the caller can render and use as a CI gate.
pub fn cmd_audit_paradox(project_root: &Path) -> Result<ParadoxAudit, PackageError> {
    let manifest = Manifest::load(project_root)?;
    let policy = &manifest.dependency_policy;

    let mut entries = Vec::new();

    for (name, spec) in &manifest.dependencies {
        let rationale = spec.rationale().map(|s| s.to_string());

        // Try to estimate LOC from the cached source tree
        let loc = estimate_dep_loc(project_root, name, spec);

        let below_threshold = match loc {
            Some(n) => n < policy.complexity_threshold,
            None => false, // unknown → don't flag
        };

        entries.push(ParadoxEntry {
            name: name.clone(),
            loc,
            rationale,
            below_threshold,
        });
    }

    Ok(ParadoxAudit {
        entries,
        threshold: policy.complexity_threshold,
        rationale_required: policy.rationale_required,
    })
}

/// Estimate LOC for a dependency by counting non-blank lines in its cached source tree.
fn estimate_dep_loc(project_root: &Path, name: &str, spec: &DepSpec) -> Option<u64> {
    let version = spec
        .version_str()
        .strip_prefix('v')
        .unwrap_or(spec.version_str());

    // Check local override first, then global cache
    let dir = resolve_pkg_dir(project_root, name, version)?;
    Some(count_source_lines(&dir))
}

/// Count non-blank source lines (`.mvl` + `.rs`) recursively in a directory.
fn count_source_lines(dir: &Path) -> u64 {
    let mut total = 0u64;
    count_lines_recursive(dir, &mut total);
    total
}

fn count_lines_recursive(dir: &Path, total: &mut u64) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            count_lines_recursive(&path, total);
        } else {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "mvl" || ext == "rs" {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    *total += content.lines().filter(|l| !l.trim().is_empty()).count() as u64;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest_with_deps(root: &Path, deps: &str) {
        let content = format!(
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nrequires-mvl = \">=0.1.0\"\n\n{deps}"
        );
        std::fs::write(root.join("mvl.toml"), content).unwrap();
    }

    // ── License audit ────────────────────────────────────────────────────────

    fn license_entry(name: &str, status: LicenseStatus) -> LicenseEntry {
        LicenseEntry {
            name: name.to_string(),
            section: "dependency".to_string(),
            license: "unknown".to_string(),
            status,
            introduced_by: None,
        }
    }

    #[test]
    fn has_violations_flags_unknown_under_permissive_policy() {
        // A package with no declared license must fail the audit under the
        // default permissive policy — otherwise a supply-chain attacker can
        // bypass the gate by omitting the license field. See #1536.
        let audit = LicenseAudit {
            entries: vec![license_entry("ghost", LicenseStatus::Unknown)],
            policy_mode: "permissive".to_string(),
        };
        assert!(audit.has_violations());
    }

    #[test]
    fn has_violations_ignores_unknown_under_any_policy() {
        // The `any` policy explicitly disables license enforcement, so an
        // unknown license is not a violation.
        let audit = LicenseAudit {
            entries: vec![license_entry("ghost", LicenseStatus::Unknown)],
            policy_mode: "any".to_string(),
        };
        assert!(!audit.has_violations());
    }

    #[test]
    fn has_violations_flags_rejected_regardless_of_policy() {
        let audit = LicenseAudit {
            entries: vec![license_entry(
                "bad",
                LicenseStatus::Rejected("not permissive".to_string()),
            )],
            policy_mode: "any".to_string(),
        };
        assert!(audit.has_violations());
    }

    #[test]
    fn has_violations_clean_audit_passes() {
        let audit = LicenseAudit {
            entries: vec![license_entry("ok", LicenseStatus::Compatible)],
            policy_mode: "permissive".to_string(),
        };
        assert!(!audit.has_violations());
    }

    // ── License audit — transitive rollup (#1701) ────────────────────────────

    use super::super::lock::LockedPackage;
    use std::collections::HashMap;

    fn parse_manifest(content: &str) -> Manifest {
        Manifest::parse(content).unwrap()
    }

    /// Empty project manifest with an optional `[license-policy]` section.
    fn empty_project_manifest() -> Manifest {
        parse_manifest(
            r#"
[package]
name = "app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"
"#,
        )
    }

    /// Build a lockfile with one MVL dep, license set (so the top-level dep
    /// audit doesn't error out on unknown).
    fn single_dep_lockfile(name: &str, version: &str) -> LockFile {
        LockFile {
            packages: vec![LockedPackage {
                name: name.to_string(),
                version: version.to_string(),
                hash: "sha256:test".to_string(),
                commit: None,
                git: None,
                license: Some("Apache-2.0".to_string()),
                allow_license_override: None,
                last_checked: None,
            }],
        }
    }

    fn find_entry<'a>(audit: &'a LicenseAudit, name: &str) -> Option<&'a LicenseEntry> {
        audit.entries.iter().find(|e| e.name == name)
    }

    #[test]
    fn transitive_c_native_license_rolled_up_with_introducer() {
        // Simulates the #1701 motivating example: a project depends on pkg-sqlite,
        // which declares sqlite3 (blessing) in its [c-native]. That entry must
        // appear in the project's audit with `introduced_by = "pkg-sqlite"`.
        let project = empty_project_manifest();
        let lockfile = single_dep_lockfile("pkg-sqlite", "0.2.2");

        let dep_manifest = parse_manifest(
            r#"
[package]
name = "pkg-sqlite"
version = "0.2.2"
license = "Apache-2.0"
requires-mvl = ">=0.1.0"
extern-rationale = "wraps rusqlite"

[c-native]
sqlite3 = { version = "3.44", license = "blessing" }
"#,
        );
        let mut cache: HashMap<String, Manifest> = HashMap::new();
        cache.insert("pkg-sqlite@0.2.2".to_string(), dep_manifest);

        let audit = audit_licenses(&project, &lockfile, &|name, version| {
            cache.get(&format!("{name}@{version}")).cloned()
        });

        let sqlite = find_entry(&audit, "sqlite3").expect("sqlite3 rolled up");
        assert_eq!(sqlite.section, "c-native");
        assert_eq!(sqlite.license, "blessing");
        assert_eq!(sqlite.introduced_by.as_deref(), Some("pkg-sqlite"));
        assert!(matches!(sqlite.status, LicenseStatus::Compatible));
    }

    #[test]
    fn transitive_native_license_rolled_up() {
        let project = empty_project_manifest();
        let lockfile = single_dep_lockfile("pkg-sqlite", "0.2.2");

        let dep_manifest = parse_manifest(
            r#"
[package]
name = "pkg-sqlite"
version = "0.2.2"
license = "Apache-2.0"
requires-mvl = ">=0.1.0"

[native]
rusqlite = { version = "0.31", license = "MIT" }
"#,
        );
        let mut cache: HashMap<String, Manifest> = HashMap::new();
        cache.insert("pkg-sqlite@0.2.2".to_string(), dep_manifest);

        let audit = audit_licenses(&project, &lockfile, &|name, version| {
            cache.get(&format!("{name}@{version}")).cloned()
        });

        let rusqlite = find_entry(&audit, "rusqlite").expect("rusqlite rolled up");
        assert_eq!(rusqlite.section, "native");
        assert_eq!(rusqlite.license, "MIT");
        assert_eq!(rusqlite.introduced_by.as_deref(), Some("pkg-sqlite"));
    }

    #[test]
    fn transitive_entry_without_license_is_unknown() {
        let project = empty_project_manifest();
        let lockfile = single_dep_lockfile("pkg-sqlite", "0.2.2");

        let dep_manifest = parse_manifest(
            r#"
[package]
name = "pkg-sqlite"
version = "0.2.2"
license = "Apache-2.0"
requires-mvl = ">=0.1.0"

[native]
rusqlite = "0.31"
"#,
        );
        let mut cache: HashMap<String, Manifest> = HashMap::new();
        cache.insert("pkg-sqlite@0.2.2".to_string(), dep_manifest);

        let audit = audit_licenses(&project, &lockfile, &|name, version| {
            cache.get(&format!("{name}@{version}")).cloned()
        });

        let rusqlite = find_entry(&audit, "rusqlite").expect("rusqlite emitted");
        assert!(matches!(rusqlite.status, LicenseStatus::Unknown));
    }

    #[test]
    fn direct_native_wins_over_transitive() {
        // If the project declares a [native] entry with the same name as a
        // transitive dep, the direct entry is emitted and the transitive one
        // is dropped — no double-reporting.
        let project = parse_manifest(
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
        let lockfile = single_dep_lockfile("pkg-sqlite", "0.2.2");

        let dep_manifest = parse_manifest(
            r#"
[package]
name = "pkg-sqlite"
version = "0.2.2"
license = "Apache-2.0"
requires-mvl = ">=0.1.0"

[native]
rusqlite = { version = "0.31", license = "MIT" }
"#,
        );
        let mut cache: HashMap<String, Manifest> = HashMap::new();
        cache.insert("pkg-sqlite@0.2.2".to_string(), dep_manifest);

        let audit = audit_licenses(&project, &lockfile, &|name, version| {
            cache.get(&format!("{name}@{version}")).cloned()
        });

        let rusqlite_entries: Vec<&LicenseEntry> = audit
            .entries
            .iter()
            .filter(|e| e.name == "rusqlite")
            .collect();
        assert_eq!(rusqlite_entries.len(), 1, "no double-reporting");
        assert!(
            rusqlite_entries[0].introduced_by.is_none(),
            "direct entry has no introducer"
        );
    }

    #[test]
    fn render_shows_introducer_in_name_column() {
        let audit = LicenseAudit {
            entries: vec![LicenseEntry {
                name: "sqlite3".to_string(),
                section: "c-native".to_string(),
                license: "blessing".to_string(),
                status: LicenseStatus::Compatible,
                introduced_by: Some("pkg-sqlite".to_string()),
            }],
            policy_mode: "permissive".to_string(),
        };
        let out = audit.render();
        assert!(
            out.contains("sqlite3 (via pkg-sqlite)"),
            "expected provenance in render, got:\n{out}"
        );
    }

    // ── Paradox audit ────────────────────────────────────────────────────────

    #[test]
    fn audit_paradox_no_deps() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest_with_deps(tmp.path(), "");
        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert!(audit.entries.is_empty());
        assert!(!audit.has_violations());
    }

    #[test]
    fn audit_paradox_with_rationale_no_violations() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest_with_deps(
            tmp.path(),
            r#"[dependencies]
ring = { git = "https://example.com/ring", tag = "v0.17.8", rationale = "Crypto" }
"#,
        );
        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert_eq!(audit.entries.len(), 1);
        assert_eq!(audit.entries[0].rationale.as_deref(), Some("Crypto"));
        assert!(!audit.has_violations());
    }

    #[test]
    fn audit_paradox_missing_rationale_with_small_dep() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest_with_deps(
            tmp.path(),
            r#"[dependencies]
small = { git = "https://example.com/small", tag = "v1.0.0" }
"#,
        );

        // Create a local override with a small source tree (< 1000 LOC)
        let pkg_dir = tmp.path().join(".mvl").join("pkg").join("small");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("lib.mvl"),
            "fn hello() -> Unit ! Console {\n    println(\"hi\")\n}\n",
        )
        .unwrap();

        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert_eq!(audit.entries.len(), 1);
        assert!(audit.entries[0].below_threshold);
        assert!(audit.entries[0].rationale.is_none());
        assert!(audit.has_violations());
        assert_eq!(audit.missing_rationale_count(), 1);
    }

    #[test]
    fn audit_paradox_small_dep_with_rationale_passes() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest_with_deps(
            tmp.path(),
            r#"[dependencies]
small = { git = "https://example.com/small", tag = "v1.0.0", rationale = "RFC compliance" }
"#,
        );

        let pkg_dir = tmp.path().join(".mvl").join("pkg").join("small");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("lib.mvl"), "fn f() -> Int { 1 }\n").unwrap();

        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert!(audit.entries[0].below_threshold);
        assert!(audit.entries[0].rationale.is_some());
        assert!(!audit.has_violations());
    }

    #[test]
    fn audit_paradox_rationale_required_false_disables_violations() {
        let tmp = tempfile::tempdir().unwrap();
        let content = r#"
[package]
name = "test"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
small = { git = "https://example.com/small", tag = "v1.0.0" }

[dependency-policy]
rationale-required = false
"#;
        std::fs::write(tmp.path().join("mvl.toml"), content).unwrap();

        let pkg_dir = tmp.path().join(".mvl").join("pkg").join("small");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("lib.mvl"), "fn f() -> Int { 1 }\n").unwrap();

        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert!(!audit.rationale_required);
        assert!(!audit.has_violations()); // violations suppressed
    }

    #[test]
    fn audit_paradox_custom_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let content = r#"
[package]
name = "test"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
medium = { git = "https://example.com/medium", tag = "v1.0.0" }

[dependency-policy]
complexity-threshold = 5
"#;
        std::fs::write(tmp.path().join("mvl.toml"), content).unwrap();

        // 3 non-blank lines
        let pkg_dir = tmp.path().join(".mvl").join("pkg").join("medium");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("lib.mvl"),
            "fn a() -> Int { 1 }\nfn b() -> Int { 2 }\nfn c() -> Int { 3 }\n",
        )
        .unwrap();

        let audit = cmd_audit_paradox(tmp.path()).unwrap();
        assert_eq!(audit.threshold, 5);
        assert!(audit.entries[0].below_threshold);
        assert_eq!(audit.entries[0].loc, Some(3));
    }

    #[test]
    fn audit_paradox_render_output() {
        let audit = ParadoxAudit {
            entries: vec![
                ParadoxEntry {
                    name: "ring".to_string(),
                    loc: Some(42000),
                    rationale: Some("Crypto".to_string()),
                    below_threshold: false,
                },
                ParadoxEntry {
                    name: "uuid".to_string(),
                    loc: Some(847),
                    rationale: None,
                    below_threshold: true,
                },
            ],
            threshold: 1000,
            rationale_required: true,
        };
        let output = audit.render();
        assert!(output.contains("ring"));
        assert!(output.contains("42000 LOC"));
        assert!(output.contains("Crypto"));
        assert!(output.contains("uuid"));
        assert!(output.contains("847 LOC"));
        assert!(output.contains("missing"));
        assert!(output.contains("1 missing rationale"));
    }

    #[test]
    fn count_source_lines_counts_mvl_and_rs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("lib.mvl"),
            "fn a() -> Int { 1 }\n\nfn b() -> Int { 2 }\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("bridge.rs"), "pub fn c() {}\n").unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not counted\n").unwrap();
        assert_eq!(count_source_lines(tmp.path()), 3);
    }
}
