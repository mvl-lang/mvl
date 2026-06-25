// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl install` — fetch all deps from `mvl.lock`, verify hashes, populate
//! `.mvl/pkg/` from the global cache.

use super::error::PackageError;
use super::fetch::{self, fetch_package, pkg_cache_dir, verify_hash};
use super::lock::LockFile;
use std::path::Path;

/// `mvl install`
///
/// Installs all dependencies listed in `mvl.lock`:
/// 1. Reads `mvl.lock` (fails if absent)
/// 2. For each package, ensures it is in the global XDG cache (fetch if missing)
/// 3. Verifies the hash matches what's in the lock file (fails hard on mismatch)
/// 4. Unless `global_only`, copies/hardlinks from global cache into `.mvl/pkg/`
///
/// Two-tier resolution (ADR-0009 §5):
///   `.mvl/pkg/<name>/`         — local project install (isolation, auditability)
///   `$XDG_DATA_HOME/mvl/pkg/` — global cache (shared, avoids re-download)
///
/// `--global` skips the local install step (CI layer-caching use case).
pub fn cmd_install(project_root: &Path, global_only: bool) -> Result<(), PackageError> {
    let lockfile = LockFile::load(project_root)?;

    if lockfile.packages.is_empty() {
        println!("No dependencies in mvl.lock.");
        return Ok(());
    }

    let mut from_network = 0usize;
    let mut from_cache = 0usize;
    let mut already_local = 0usize;

    for pkg in &lockfile.packages {
        let cache_dest = pkg_cache_dir(&pkg.name, &pkg.version);

        // Step 1: ensure package is in global cache
        let newly_fetched = if cache_dest.exists() {
            verify_hash(&cache_dest, &pkg.hash)?;
            false
        } else {
            println!("Fetching {} {}...", pkg.name, pkg.version);
            let git_url = pkg.git.as_deref().ok_or_else(|| {
                PackageError::MissingData(format!(
                    "no git URL in mvl.lock for '{}' — cannot install",
                    pkg.name
                ))
            })?;
            // Always clone by version tag.  The `commit` field is informational
            // only — `git clone --branch` does not accept raw SHAs.
            let tag = format!("v{}", pkg.version);
            let locked = fetch_package(&pkg.name, git_url, tag.as_str())?;

            if locked.hash != pkg.hash {
                return Err(PackageError::Fetch(fetch::FetchError::HashMismatch {
                    path: pkg.name.clone(),
                    expected: pkg.hash.clone(),
                    actual: locked.hash,
                }));
            }
            true
        };

        if global_only {
            if newly_fetched {
                from_network += 1;
            } else {
                from_cache += 1;
            }
            continue;
        }

        // Step 2: populate .mvl/pkg/<name>/ from global cache
        let local_dir = fetch::local_override_dir(project_root, &pkg.name);

        if local_dir.exists() {
            // If the hash matches, it's already a valid local install — skip.
            // If it doesn't match, it's a manual override (ADR-0039) — leave it
            // alone, but warn loudly so a tampered cache isn't silently trusted.
            if verify_hash(&local_dir, &pkg.hash).is_ok() {
                already_local += 1;
            } else {
                eprintln!(
                    "warning: {} {}: local override hash differs from mvl.lock — \
                     treating as manual override (ADR-0039); verify this is intentional",
                    pkg.name, pkg.version,
                );
                if newly_fetched {
                    from_network += 1;
                } else {
                    from_cache += 1;
                }
            }
            continue;
        }

        let source = if newly_fetched {
            "network -> cache -> local"
        } else {
            "cache -> local"
        };
        println!("Installing {} {} [{}]...", pkg.name, pkg.version, source);
        install_local(&cache_dest, &local_dir)?;

        if newly_fetched {
            from_network += 1;
        } else {
            from_cache += 1;
        }
    }

    if global_only {
        println!(
            "Installed {} package(s) to global cache, {} already cached.",
            from_network, from_cache
        );
    } else {
        println!(
            "Installed {} package(s) — {} from cache, {} from network, {} already local.",
            from_cache + from_network,
            from_cache,
            from_network,
            already_local,
        );
    }
    Ok(())
}

/// Recursively copy `src` into `dst`, using hardlinks where possible.
///
/// Hardlinks avoid duplicate disk usage when the global cache and project are
/// on the same filesystem (APFS, ext4, btrfs, xfs). Falls back to a real copy
/// on cross-device moves or filesystems that don't support hardlinks (FAT).
/// Never uses symlinks — they bypass lock-file hash verification.
fn install_local(src: &Path, dst: &Path) -> Result<(), PackageError> {
    std::fs::create_dir_all(dst)
        .map_err(|e| PackageError::Io(dst.display().to_string(), e.to_string()))?;

    for entry in std::fs::read_dir(src)
        .map_err(|e| PackageError::Io(src.display().to_string(), e.to_string()))?
    {
        let entry =
            entry.map_err(|e| PackageError::Io(src.display().to_string(), e.to_string()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            install_local(&src_path, &dst_path)?;
        } else {
            // Hardlink preferred; fall back to copy on cross-device or unsupported fs
            if std::fs::hard_link(&src_path, &dst_path).is_err() {
                std::fs::copy(&src_path, &dst_path)
                    .map_err(|e| PackageError::Io(dst_path.display().to_string(), e.to_string()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_install_empty_lockfile_returns_early() {
        let tmp = tempfile::tempdir().unwrap();
        // Write an empty lock file (no packages) — cmd_install should return Ok
        std::fs::write(tmp.path().join("mvl.lock"), "# Generated by mvl\n").unwrap();
        cmd_install(tmp.path(), false).unwrap();
    }

    #[test]
    fn cmd_install_missing_lockfile_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.lock → should return Err, not panic
        let result = cmd_install(tmp.path(), false);
        assert!(result.is_err());
    }

    #[test]
    fn install_local_copies_files_to_dst() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("lib.mvl"), "fn foo() -> Int { 1 }").unwrap();

        install_local(src.path(), dst.path()).unwrap();

        let content = std::fs::read_to_string(dst.path().join("lib.mvl")).unwrap();
        assert_eq!(content, "fn foo() -> Int { 1 }");
    }

    #[test]
    fn install_local_recurses_into_subdirs() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir(src.path().join("sub")).unwrap();
        std::fs::write(src.path().join("sub").join("nested.mvl"), "nested").unwrap();

        install_local(src.path(), dst.path()).unwrap();

        let content = std::fs::read_to_string(dst.path().join("sub").join("nested.mvl")).unwrap();
        assert_eq!(content, "nested");
    }

    #[test]
    fn install_local_creates_dst_if_absent() {
        let src = tempfile::tempdir().unwrap();
        let dst_parent = tempfile::tempdir().unwrap();
        let dst = dst_parent.path().join("new_dir");
        std::fs::write(src.path().join("f.mvl"), "x").unwrap();

        install_local(src.path(), &dst).unwrap();

        assert!(dst.join("f.mvl").exists());
    }

    #[test]
    fn install_local_preserves_file_content() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let content = "fn main() -> Unit ! Console {\n    println(\"hi\")\n}";
        std::fs::write(src.path().join("main.mvl"), content).unwrap();

        install_local(src.path(), dst.path()).unwrap();

        let got = std::fs::read_to_string(dst.path().join("main.mvl")).unwrap();
        assert_eq!(got, content);
    }
}
