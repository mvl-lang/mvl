// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Dependency resolver — pre-build fetch of declared deps.

use super::error::PackageError;
use super::fetch::{self, fetch_package, pkg_cache_dir, resolve_pkg_dir, verify_hash};
use super::lock::LockFile;
use super::manifest::{self, Manifest};
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
