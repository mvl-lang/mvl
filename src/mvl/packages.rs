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

pub mod fetch;
pub mod lock;
pub mod manifest;
pub mod mvs;
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
        }
    }
}

// ── CLI entry points ──────────────────────────────────────────────────────────

/// `mvl add <git-url-or-pkg-id> [<tag>]`
///
/// Fetches a package from a git URL, adds it to `mvl.toml` and `mvl.lock`.
/// If `tag` is omitted, queries the git remote for the latest semver tag.
pub fn cmd_add(pkg_id: &str, tag: Option<&str>, project_root: &Path) -> Result<(), PackageError> {
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

    let locked = fetch_package(&pkg_name, &git_url, &resolved_tag)?;

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

    manifest.dependencies.insert(
        pkg_name.clone(),
        DepSpec::Git {
            git: git_url,
            tag: resolved_tag,
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
            continue;
        }

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
        let result = cmd_add("http://example.com/pkg", None, tmp.path());
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
}
