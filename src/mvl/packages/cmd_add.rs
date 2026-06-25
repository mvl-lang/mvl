// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl add` — fetch a package, add to mvl.toml + mvl.lock.

use super::cmd_audit::read_package_license;
use super::config::latest_semver_tag;
use super::error::PackageError;
use super::fetch::{self, fetch_package};
use super::lock::LockFile;
use super::manifest::{DepSpec, Manifest};
use std::path::Path;

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

    // Reject pkg_ids containing characters that would corrupt mvl.toml when
    // used as a manifest key (the key is written verbatim by `Manifest::to_toml`).
    validate_pkg_id(pkg_id)?;

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

    // Record when we validated this entry against the remote (#1460).
    locked.last_checked = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();

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
            exclude: vec![],
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

/// Reject `pkg_id` strings that would corrupt `mvl.toml` if used verbatim as a
/// dependency key. The allowed alphabet covers everything found in legitimate
/// git URLs / package identifiers; anything else is rejected as TOML-unsafe.
fn validate_pkg_id(pkg_id: &str) -> Result<(), PackageError> {
    if pkg_id.is_empty() {
        return Err(PackageError::InvalidInput(
            "package id must not be empty".to_string(),
        ));
    }
    for c in pkg_id.chars() {
        let ok =
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | ':' | '@' | '+' | '~');
        if !ok {
            return Err(PackageError::InvalidInput(format!(
                "package id contains disallowed character {c:?}; allowed: [A-Za-z0-9.-_/:@+~]"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_add_rejects_http_url() {
        let tmp = tempfile::tempdir().unwrap();
        let result = cmd_add("http://example.com/pkg", None, None, None, tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("http://"), "error should mention the protocol");
    }

    #[test]
    fn cmd_add_rejects_toml_injection() {
        let tmp = tempfile::tempdir().unwrap();
        for bad in [
            "github.com/u/r\n[evil]",
            "github.com/u/r=evil",
            "github.com/u/r\"evil\"",
            "github.com/u/r[evil]",
            "github.com/u/r evil",
        ] {
            let result = cmd_add(bad, None, None, None, tmp.path());
            assert!(
                result.is_err(),
                "expected rejection for malicious pkg_id: {bad:?}"
            );
        }
    }

    #[test]
    fn validate_pkg_id_accepts_normal_ids() {
        assert!(validate_pkg_id("github.com/user/repo").is_ok());
        assert!(validate_pkg_id("https://gitlab.com/group/sub/repo.git").is_ok());
        assert!(validate_pkg_id("git@github.com:user/repo.git").is_ok());
    }
}
