// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Package management: manifest, lock file, fetch, version resolution.
//!
//! Implements Spec 008 (Extended Package Model) and ADR-0012.
//!
//! # CLI commands
//! - `mvl add <git-url>[@<tag>]`  — fetch a package, add to mvl.toml + mvl.lock
//! - `mvl install`                 — fetch all deps from mvl.lock, verify hashes
//! - `mvl update`                  — re-resolve versions, update mvl.lock
//! - `mvl sbom`                    — generate CycloneDX/SPDX SBOM from mvl.lock
//! - `mvl audit --paradox`         — Dependency Paradox audit (#637)

pub mod audit;
pub mod fetch;
pub mod hash;
pub mod lock;
pub mod manifest;
pub mod mvs;
pub mod sbom;
pub mod version;

use fetch::{fetch_package, pkg_cache_dir, resolve_pkg_dir, verify_hash};
use lock::LockFile;
use manifest::{DepSpec, Manifest};
use std::path::{Path, PathBuf};

// ── Public re-exports for use by the resolver ─────────────────────────────────

pub use fetch::{local_override_dir, pkg_cache_root};

// ── Unified error type ───────────────────────────────────────────────────────

/// Errors that can occur during package operations.
#[derive(Debug)]
pub enum PackageError {
    Fetch(fetch::FetchError),
    Manifest(manifest::ManifestError),
    Lock(lock::LockError),
    /// A required field is missing from a data structure (e.g. no git URL in lock entry).
    MissingData(String),
    /// A write to the filesystem failed.
    Io(String, String),
    /// An HTTP-safety or input-validation error.
    InvalidInput(String),
    /// No matching version/tag was found.
    NoVersion(String),
    /// License policy rejected the package (#635).
    LicenseRejected {
        package: String,
        license: String,
        reason: String,
    },
}

impl From<fetch::FetchError> for PackageError {
    fn from(e: fetch::FetchError) -> Self {
        PackageError::Fetch(e)
    }
}

impl From<manifest::ManifestError> for PackageError {
    fn from(e: manifest::ManifestError) -> Self {
        PackageError::Manifest(e)
    }
}

impl From<lock::LockError> for PackageError {
    fn from(e: lock::LockError) -> Self {
        PackageError::Lock(e)
    }
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageError::Fetch(e) => write!(f, "{e}"),
            PackageError::Manifest(e) => write!(f, "{e}"),
            PackageError::Lock(e) => write!(f, "{e}"),
            PackageError::MissingData(msg) => write!(f, "{msg}"),
            PackageError::Io(path, e) => write!(f, "IO error at {path}: {e}"),
            PackageError::InvalidInput(msg) => write!(f, "{msg}"),
            PackageError::NoVersion(msg) => write!(f, "{msg}"),
            PackageError::LicenseRejected {
                package,
                license,
                reason,
            } => write!(
                f,
                "license rejected for '{package}': {license} — {reason}. Use --allow-license to override."
            ),
        }
    }
}

// ── CLI entry points ──────────────────────────────────────────────────────────

/// `mvl add <git-url-or-pkg-id> [<tag>] [--rationale "..."] [--allow-license]`
///
/// Fetches a package from a git URL, adds it to `mvl.toml` and `mvl.lock`.
/// If `tag` is omitted, queries the git remote for the latest semver tag.
/// If `rationale` is provided, it is stored alongside the dependency spec.
/// If `allow_license` is provided, it overrides a license policy rejection.
pub fn cmd_add(
    pkg_id: &str,
    tag: Option<&str>,
    rationale: Option<&str>,
    allow_license: Option<&str>,
    project_root: &Path,
) -> Result<(), PackageError> {
    // Reject plain-HTTP URLs — they are vulnerable to MITM at fetch time.
    if pkg_id.starts_with("http://") {
        return Err(PackageError::InvalidInput(
            "plain http:// is not allowed; use https:// to prevent MITM attacks".to_string(),
        ));
    }

    // Derive the git URL from the pkg-id (strip optional leading scheme)
    let git_url = if pkg_id.starts_with("https://") || pkg_id.starts_with("git@") {
        pkg_id.to_string()
    } else {
        format!("https://{pkg_id}")
    };

    // Determine the package name (last two path components for github.com/user/repo style)
    let pkg_name = pkg_id.trim_end_matches('/').to_string();

    // Resolve tag
    let resolved_tag = match tag {
        Some(t) => t.to_string(),
        None => {
            eprintln!("Querying tags for {git_url}...");
            let tags = fetch::list_git_tags(&git_url)?;
            latest_semver_tag(&tags).ok_or_else(|| {
                PackageError::NoVersion(format!("no semver tags found for {git_url}"))
            })?
        }
    };

    let version_str = resolved_tag
        .strip_prefix('v')
        .unwrap_or(&resolved_tag)
        .to_string();
    println!("Fetching {pkg_name} @ {resolved_tag}...");

    let mut locked = fetch_package(&pkg_name, &git_url, &resolved_tag)?;

    // Read the fetched package's license from its mvl.toml (#635)
    let pkg_license = read_package_license(&pkg_name, &version_str);
    if let Some(ref lic) = pkg_license {
        locked.license = Some(lic.clone());
    }

    // Update or create mvl.toml
    let manifest_path = project_root.join("mvl.toml");
    let mut manifest = if manifest_path.exists() {
        Manifest::load(project_root)?
    } else {
        let name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project");
        Manifest::new_project(name, env!("CARGO_PKG_VERSION"))
    };

    // Check license policy (#635)
    if let Some(ref lic) = pkg_license {
        let policy = &manifest.license_policy;
        if let Err(reason) = policy.check(lic) {
            if let Some(override_reason) = allow_license {
                eprintln!(
                    "  License: {lic} — incompatible with project policy ({})",
                    policy.mode_str()
                );
                eprintln!("  Overridden: {override_reason}");
                locked.allow_license_override = Some(override_reason.to_string());
            } else {
                return Err(PackageError::LicenseRejected {
                    package: pkg_name,
                    license: lic.clone(),
                    reason,
                });
            }
        } else {
            println!(
                "  License: {lic} — compatible with project policy ({})",
                policy.mode_str()
            );
        }
    }

    manifest.dependencies.insert(
        pkg_name.clone(),
        DepSpec::Git {
            git: git_url,
            tag: resolved_tag,
            rationale: rationale.map(|s| s.to_string()),
        },
    );

    std::fs::write(&manifest_path, manifest.to_toml())
        .map_err(|e| PackageError::Io(manifest_path.display().to_string(), e.to_string()))?;

    // Update mvl.lock
    let mut lockfile = LockFile::load_or_empty(project_root);
    lockfile.upsert(locked);
    lockfile.write(project_root)?;

    println!("Added {pkg_name} {version_str} to mvl.toml and mvl.lock");
    Ok(())
}

/// `mvl install`
///
/// Installs all dependencies listed in `mvl.lock`:
/// 1. Reads `mvl.lock` (fails if absent)
/// 2. For each package, checks if it is already cached
/// 3. If not cached, fetches it from its git URL
/// 4. Verifies the hash matches what's in the lock file (fails hard on mismatch)
pub fn cmd_install(project_root: &Path) -> Result<(), PackageError> {
    let lockfile = LockFile::load(project_root)?;

    if lockfile.packages.is_empty() {
        println!("No dependencies in mvl.lock.");
        return Ok(());
    }

    let mut installed = 0usize;
    let mut cached = 0usize;

    for pkg in &lockfile.packages {
        let dest = pkg_cache_dir(&pkg.name, &pkg.version);
        if dest.exists() {
            // Verify hash even for cached packages
            verify_hash(&dest, &pkg.hash)?;
            cached += 1;
        } else {
            println!("Installing {} {}...", pkg.name, pkg.version);
            let git_url = pkg.git.as_deref().ok_or_else(|| {
                PackageError::MissingData(format!(
                    "no git URL in mvl.lock for '{}' — cannot install",
                    pkg.name
                ))
            })?;
            // Always clone by version tag.  The `commit` field is informational
            // only — `git clone --branch` does not accept raw SHAs.
            let tag = format!("v{}", pkg.version);
            let tag = tag.as_str();

            let locked = fetch_package(&pkg.name, git_url, tag)?;

            // Verify hash after fetch
            if locked.hash != pkg.hash {
                return Err(PackageError::Fetch(fetch::FetchError::HashMismatch {
                    path: pkg.name.clone(),
                    expected: pkg.hash.clone(),
                    actual: locked.hash,
                }));
            }

            installed += 1;
        }
    }

    println!(
        "Installed {} package(s), {} already cached.",
        installed, cached
    );
    Ok(())
}

/// `mvl update`
///
/// Re-resolves versions for all git dependencies, fetches any newer tags,
/// and rewrites `mvl.lock` with updated versions and hashes.
pub fn cmd_update(project_root: &Path) -> Result<(), PackageError> {
    let manifest = Manifest::load(project_root)?;

    if manifest.dependencies.is_empty() {
        println!("No dependencies in mvl.toml.");
        return Ok(());
    }

    let mut lockfile = LockFile::load_or_empty(project_root);
    let mut updated = 0usize;

    for (name, spec) in &manifest.dependencies {
        let git_url = match spec {
            DepSpec::Git { git, .. } => git.clone(),
            DepSpec::Version(constraint) => {
                // For version-only deps without a git URL, skip with a warning
                eprintln!(
                    "warning: cannot update '{name}' (version constraint '{constraint}' has no git URL)"
                );
                continue;
            }
        };

        println!("Checking {name}...");
        let tags = fetch::list_git_tags(&git_url)?;

        // Find the latest tag compatible with the current constraint
        let latest = latest_semver_tag(&tags)
            .ok_or_else(|| PackageError::NoVersion(format!("no semver tags found for {name}")))?;

        let current_version = lockfile
            .get(name)
            .map(|p| p.version.as_str())
            .unwrap_or("0.0.0");
        let latest_version = latest.strip_prefix('v').unwrap_or(&latest);

        if latest_version == current_version {
            println!("  {name} is up to date ({current_version})");
            continue;
        }

        println!("  {name}: {current_version} → {latest_version}");
        let locked = fetch_package(name, &git_url, &latest)?;
        lockfile.upsert(locked);
        updated += 1;
    }

    lockfile.write(project_root)?;

    if updated > 0 {
        println!("Updated {updated} package(s).");
    } else {
        println!("All packages are up to date.");
    }
    Ok(())
}

/// `mvl sbom [--format=<fmt>]`
///
/// Generates a software bill of materials from `mvl.toml` and `mvl.lock` and
/// returns it as a string so the caller can print or write it.
///
/// `format` defaults to `"cyclonedx"` if `None`.
/// Check whether a project directory contains any `.mvl` file with `fn main()`.
fn has_main_entry(dir: &Path) -> bool {
    // Fast path: conventional main.mvl
    if dir.join("main.mvl").exists() || dir.join("src").join("main.mvl").exists() {
        return true;
    }
    // Scan top-level .mvl files for `fn main(`
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "mvl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if content.contains("fn main(") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

pub fn cmd_sbom(format: Option<&str>, project_root: &Path) -> Result<String, PackageError> {
    let manifest = Manifest::load(project_root)?;
    let lock = LockFile::load_or_empty(project_root);

    let fmt_str = format.unwrap_or("cyclonedx");
    let fmt = sbom::SbomFormat::parse(fmt_str).ok_or_else(|| {
        PackageError::InvalidInput(format!(
            "unknown SBOM format '{fmt_str}'; supported: cyclonedx, spdx"
        ))
    })?;

    // Detect application vs library: presence of any .mvl file with fn main().
    let component_type = if has_main_entry(project_root) {
        sbom::ComponentType::Application
    } else {
        sbom::ComponentType::Library
    };

    // Build license map: read each cached package's mvl.toml for its license field.
    let mut licenses = sbom::LicenseMap::new();
    for lp in &lock.packages {
        let cache_dir = pkg_cache_dir(&lp.name, &lp.version);
        if let Ok(content) = std::fs::read_to_string(cache_dir.join("mvl.toml")) {
            if let Ok(pkg_manifest) = Manifest::parse(&content) {
                licenses.insert(lp.name.clone(), pkg_manifest.package.license);
            }
        }
    }

    // Collect source files: walk project root for .mvl files and hash each one.
    let sources = collect_source_files(project_root);

    Ok(sbom::generate(
        &manifest,
        &lock,
        fmt,
        component_type,
        &licenses,
        &sources,
    ))
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
    pub fn has_violations(&self) -> bool {
        self.rejected_count() > 0
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
            out.push_str(&format!(
                "  [{:<10}] {:<40} {:<20} {}\n",
                e.section, e.name, e.license, status_str
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
/// - MVL deps: reads license from `mvl.lock` (set by `mvl add`)
/// - C-native: reads license from `[c-native]` inline table
/// - Native (Rust): not enforced yet (would need Cargo metadata)
pub fn cmd_audit_license(project_root: &Path) -> Result<LicenseAudit, PackageError> {
    let manifest = Manifest::load(project_root)?;
    let lockfile = LockFile::load_or_empty(project_root);
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
                });
                continue;
            }
            (Some(lic), None) => lic.clone(),
            (None, _) => {
                // Try reading from cached package mvl.toml
                let cached_license = read_package_license(&lp.name, &lp.version);
                match cached_license {
                    Some(lic) => lic,
                    None => {
                        entries.push(LicenseEntry {
                            name: lp.name.clone(),
                            section: "dependency".to_string(),
                            license: "unknown".to_string(),
                            status: LicenseStatus::Unknown,
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
        });
    }

    // Check C-native dependencies
    for (name, spec) in &manifest.c_native {
        match &spec.license {
            Some(lic) => {
                let status = match policy.check(lic) {
                    Ok(()) => LicenseStatus::Compatible,
                    Err(reason) => LicenseStatus::Rejected(reason),
                };
                entries.push(LicenseEntry {
                    name: name.clone(),
                    section: "c-native".to_string(),
                    license: lic.clone(),
                    status,
                });
            }
            None => {
                entries.push(LicenseEntry {
                    name: name.clone(),
                    section: "c-native".to_string(),
                    license: "unknown".to_string(),
                    status: LicenseStatus::Unknown,
                });
            }
        }
    }

    Ok(LicenseAudit {
        entries,
        policy_mode: policy.mode_str().to_string(),
    })
}

/// Read the license field from a cached package's `mvl.toml`.
fn read_package_license(name: &str, version: &str) -> Option<String> {
    let cache_dir = fetch::pkg_cache_dir(name, version);
    let toml_path = cache_dir.join("mvl.toml");
    let content = std::fs::read_to_string(toml_path).ok()?;
    let pkg_manifest = Manifest::parse(&content).ok()?;
    Some(pkg_manifest.package.license)
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

/// Walk `root` recursively for `.mvl` files and return a sorted list of
/// `SourceFile` entries with canonical relative paths and SHA-256 digests.
fn collect_source_files(root: &Path) -> Vec<sbom::SourceFile> {
    let mut out = Vec::new();
    collect_mvl_files_recursive(root, root, &mut out);
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

fn collect_mvl_files_recursive(root: &Path, dir: &Path, out: &mut Vec<sbom::SourceFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories (e.g. .git)
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            collect_mvl_files_recursive(root, &path, out);
        } else if path.extension().is_some_and(|x| x == "mvl") {
            if let Ok(digest) = hash::sha256_file(&path) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(sbom::SourceFile {
                    rel_path: rel,
                    digest,
                });
            }
        }
    }
}

/// Ensure all dependencies in `mvl.toml` are fetched before build.
///
/// Called by `mvl build` before transpilation (ADR-0012 Build Integration step 2).
/// Returns a map from package name → source directory.
pub fn ensure_dependencies(
    project_root: &Path,
) -> Result<std::collections::HashMap<String, PathBuf>, PackageError> {
    let manifest = match Manifest::load(project_root) {
        Ok(m) => m,
        // No mvl.toml → no dependencies.  Emit a warning for parse/IO errors
        // so users aren't silently left without packages they declared.
        Err(e) => {
            use manifest::ManifestError;
            match e {
                ManifestError::Io(_, _) => {} // file absent is fine
                other => eprintln!("warning: could not read mvl.toml: {other}"),
            }
            return Ok(std::collections::HashMap::new());
        }
    };

    if manifest.dependencies.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let lockfile = LockFile::load(project_root)?;

    let mut pkg_dirs = std::collections::HashMap::new();

    for name in manifest.dependencies.keys() {
        let pinned = lockfile.get(name).ok_or_else(|| {
            PackageError::MissingData(format!(
                "'{name}' in mvl.toml is not in mvl.lock — run 'mvl install'"
            ))
        })?;

        // Try local override first, then global cache
        let dir = match resolve_pkg_dir(project_root, name, &pinned.version) {
            Some(d) => d,
            None => {
                // Auto-fetch if missing
                let git_url = pinned.git.as_deref().ok_or_else(|| {
                    PackageError::MissingData(format!(
                        "'{name}' not in cache and no git URL in mvl.lock"
                    ))
                })?;
                let tag = format!("v{}", pinned.version);
                eprintln!("Fetching missing dependency: {name} {}...", pinned.version);
                fetch_package(name, git_url, &tag)?;
                pkg_cache_dir(name, &pinned.version)
            }
        };

        // Verify hash (fail hard on mismatch)
        if !is_local_override(project_root, name, &dir) {
            verify_hash(&dir, &pinned.hash)?;
        }

        pkg_dirs.insert(name.clone(), dir);
    }

    Ok(pkg_dirs)
}

/// Check whether `dir` is the local override directory for `name`.
fn is_local_override(project_root: &Path, name: &str, dir: &Path) -> bool {
    let local = fetch::local_override_dir(project_root, name);
    dir.starts_with(&local)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the latest tag that parses as a semver version (with optional `v` prefix).
fn latest_semver_tag(tags: &[String]) -> Option<String> {
    use version::Version;
    let mut best: Option<(Version, String)> = None;
    for tag in tags {
        let vstr = tag.strip_prefix('v').unwrap_or(tag);
        if let Some(v) = Version::parse(vstr) {
            if best.as_ref().map(|(bv, _)| &v > bv).unwrap_or(true) {
                best = Some((v, tag.clone()));
            }
        }
    }
    best.map(|(_, tag)| tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // --- latest_semver_tag ---

    #[test]
    fn latest_semver_tag_empty_list_returns_none() {
        assert!(latest_semver_tag(&[]).is_none());
    }

    #[test]
    fn latest_semver_tag_picks_highest() {
        let t = tags(&["v1.0.0", "v2.0.0", "v1.5.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v2.0.0"));
    }

    #[test]
    fn latest_semver_tag_ignores_non_semver_entries() {
        let t = tags(&["nightly", "v1.0.0", "beta", "latest"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn latest_semver_tag_all_non_semver_returns_none() {
        let t = tags(&["nightly", "beta", "latest", "stable"]);
        assert!(latest_semver_tag(&t).is_none());
    }

    #[test]
    fn latest_semver_tag_without_v_prefix() {
        // Tags without a leading 'v' should also parse as semver
        let t = tags(&["1.0.0", "2.0.0", "1.5.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("2.0.0"));
    }

    #[test]
    fn latest_semver_tag_mixed_v_prefix() {
        // Both "v1.0.0" and "2.0.0" forms present — picks the highest
        let t = tags(&["v1.0.0", "2.0.0", "v1.5.0"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("2.0.0"));
    }

    #[test]
    fn latest_semver_tag_single_entry() {
        let t = tags(&["v3.2.1"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v3.2.1"));
    }

    #[test]
    fn latest_semver_tag_preserves_original_tag_string() {
        // The returned tag must be the original string (with 'v'), not the stripped version
        let t = tags(&["v1.2.3"]);
        assert_eq!(latest_semver_tag(&t).as_deref(), Some("v1.2.3"));
    }

    // --- is_local_override ---

    #[test]
    fn is_local_override_true_when_dir_is_under_local_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let local = root.join(".mvl").join("pkg").join("mypkg");
        std::fs::create_dir_all(&local).unwrap();

        assert!(is_local_override(root, "mypkg", &local));
    }

    #[test]
    fn is_local_override_false_when_dir_is_not_under_local_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // A path outside the .mvl tree
        let other = tmp.path().join("some").join("other").join("path");
        std::fs::create_dir_all(&other).unwrap();

        assert!(!is_local_override(root, "mypkg", &other));
    }

    #[test]
    fn is_local_override_false_for_cache_path() {
        // A typical global cache path must not be mistaken for a local override
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cache_path = std::path::PathBuf::from("/home/user/.local/share/mvl/pkg/mypkg/1.0.0");

        assert!(!is_local_override(root, "mypkg", &cache_path));
    }

    // --- ensure_dependencies ---

    #[test]
    fn ensure_deps_no_manifest_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.toml present — IO error branch returns empty map silently
        let dirs = ensure_dependencies(tmp.path()).unwrap();
        assert!(dirs.is_empty());
    }

    #[test]
    fn ensure_deps_empty_dependencies_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), content).unwrap();
        let dirs = ensure_dependencies(tmp.path()).unwrap();
        assert!(dirs.is_empty());
    }

    #[test]
    fn ensure_deps_invalid_manifest_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        // Exists but fails TOML parsing (non-IO error) → warning + empty map
        std::fs::write(tmp.path().join("mvl.toml"), "key = bare_value\n").unwrap();
        let dirs = ensure_dependencies(tmp.path()).unwrap();
        assert!(dirs.is_empty());
    }

    #[test]
    fn ensure_deps_local_override_skips_hash_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let manifest = "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\nmypkg = { git = \"https://example.com/mypkg\", tag = \"v1.0.0\" }\n";
        std::fs::write(root.join("mvl.toml"), manifest).unwrap();

        let lock = "[[package]]\nname = \"mypkg\"\nversion = \"1.0.0\"\nhash = \"sha256:abc123\"\ngit = \"https://example.com/mypkg\"\n";
        std::fs::write(root.join("mvl.lock"), lock).unwrap();

        // Create the local override directory — hash verification is skipped for local overrides
        let override_dir = root.join(".mvl").join("pkg").join("mypkg");
        std::fs::create_dir_all(&override_dir).unwrap();

        let dirs = ensure_dependencies(root).unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs.get("mypkg").unwrap(), &override_dir);
    }

    // --- cmd_install ---

    #[test]
    fn cmd_install_empty_lockfile_returns_early() {
        let tmp = tempfile::tempdir().unwrap();
        // Write an empty lock file (no packages) — cmd_install should return Ok
        std::fs::write(tmp.path().join("mvl.lock"), "# Generated by mvl\n").unwrap();
        cmd_install(tmp.path()).unwrap();
    }

    #[test]
    fn cmd_install_missing_lockfile_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.lock → should return Err, not panic
        let result = cmd_install(tmp.path());
        assert!(result.is_err());
    }

    // --- cmd_update ---

    #[test]
    fn cmd_update_no_deps_returns_early() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        cmd_update(tmp.path()).unwrap();
    }

    #[test]
    fn cmd_update_version_only_dep_skips_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"some-pkg\" = \">=1.0.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        // Version-only dep has no git URL → emits warning and skips; writes mvl.lock
        cmd_update(tmp.path()).unwrap();
        assert!(tmp.path().join("mvl.lock").exists());
    }

    #[test]
    fn cmd_update_missing_manifest_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.toml → should return Err, not panic
        let result = cmd_update(tmp.path());
        assert!(result.is_err());
    }

    // --- cmd_add error cases ---

    #[test]
    fn cmd_add_rejects_http_url() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_add("http://example.com/pkg", None, None, None, tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("http://"), "error should mention the protocol");
    }

    // --- PackageError Display ---

    #[test]
    fn package_error_display_missing_data() {
        let e = PackageError::MissingData("no git URL".to_string());
        assert!(e.to_string().contains("no git URL"));
    }

    #[test]
    fn package_error_display_io() {
        let e = PackageError::Io("/path".to_string(), "permission denied".to_string());
        assert!(e.to_string().contains("/path"));
        assert!(e.to_string().contains("permission denied"));
    }

    #[test]
    fn package_error_from_fetch() {
        let fetch_err = fetch::FetchError::GitError("clone failed".to_string());
        let pkg_err: PackageError = fetch_err.into();
        assert!(matches!(pkg_err, PackageError::Fetch(_)));
        assert!(pkg_err.to_string().contains("clone failed"));
    }

    #[test]
    fn package_error_from_manifest() {
        let manifest_err = manifest::ManifestError::MissingField("name".to_string());
        let pkg_err: PackageError = manifest_err.into();
        assert!(matches!(pkg_err, PackageError::Manifest(_)));
    }

    #[test]
    fn package_error_from_lock() {
        let lock_err = lock::LockError::MissingField("version".to_string());
        let pkg_err: PackageError = lock_err.into();
        assert!(matches!(pkg_err, PackageError::Lock(_)));
    }

    // --- Dependency Paradox audit (#637) ---

    fn write_manifest_with_deps(root: &Path, deps: &str) {
        let content = format!(
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\nlicense = \"MIT\"\nrequires-mvl = \">=0.1.0\"\n\n{deps}"
        );
        std::fs::write(root.join("mvl.toml"), content).unwrap();
    }

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
