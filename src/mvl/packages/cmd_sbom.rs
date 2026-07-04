// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl sbom` — generate CycloneDX/SPDX SBOMs, snapshot baselines, and diff.

use super::error::PackageError;
use super::lock::LockFile;
use super::manifest::{load_cached_manifest_required, Manifest};
use super::{hash, sbom, sbom_diff};
use std::collections::BTreeMap;
use std::path::Path;

const BASELINE_SBOM_FILE: &str = ".mvl/sbom.baseline.json";
const BASELINE_META_FILE: &str = ".mvl/sbom.baseline.meta";

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

/// `mvl sbom [--format=<fmt>]`
///
/// Generates a software bill of materials from `mvl.toml` and `mvl.lock` and
/// returns it as a string so the caller can print or write it.
///
/// `format` defaults to `"cyclonedx"` if `None`.
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

    // Walk each locked package's cached mvl.toml. Fail hard if a cache entry
    // is missing: an incomplete SBOM is a supply-chain footgun, not a warning.
    let mut licenses = sbom::LicenseMap::new();
    let mut pkg_manifests: Vec<(String, Manifest)> = Vec::new();
    for lp in &lock.packages {
        let pkg_manifest = load_cached_manifest_required(&lp.name, &lp.version)?;
        licenses.insert(lp.name.clone(), pkg_manifest.package.license.clone());
        pkg_manifests.push((lp.name.clone(), pkg_manifest));
    }
    let transitive = collect_transitive_native(&manifest, &pkg_manifests);

    // Collect source files: walk project root for .mvl files and hash each one.
    let sources = collect_source_files(project_root);

    Ok(sbom::generate(
        &manifest,
        &lock,
        fmt,
        component_type,
        &licenses,
        &sources,
        &transitive,
    ))
}

/// `mvl sbom snapshot` — save the current SBOM as the baseline.
///
/// Writes two files:
/// - `.mvl/sbom.baseline.json` — full CycloneDX snapshot (for auditors)
/// - `.mvl/sbom.baseline.meta` — lightweight dep list + timestamp (used by diff)
pub fn cmd_sbom_snapshot(project_root: &Path) -> Result<(), PackageError> {
    let sbom_json = cmd_sbom(None, project_root)?;

    let mvl_dir = project_root.join(".mvl");
    std::fs::create_dir_all(&mvl_dir)
        .map_err(|e| PackageError::Io(".mvl/".to_string(), e.to_string()))?;

    std::fs::write(project_root.join(BASELINE_SBOM_FILE), &sbom_json)
        .map_err(|e| PackageError::Io(BASELINE_SBOM_FILE.to_string(), e.to_string()))?;

    // Build dep list from manifest + lock for the meta file
    let manifest = Manifest::load(project_root)?;
    let lock = LockFile::load_or_empty(project_root);

    let mut deps: Vec<sbom_diff::DepEntry> = Vec::new();
    for lp in &lock.packages {
        deps.push(sbom_diff::DepEntry {
            name: lp.name.clone(),
            version: lp.version.clone(),
            kind: sbom_diff::DepKind::Mvl,
        });
    }
    let mut native: Vec<_> = manifest.native.iter().collect();
    native.sort_by_key(|(k, _)| *k);
    for (name, version) in native {
        deps.push(sbom_diff::DepEntry {
            name: name.clone(),
            version: version.clone(),
            kind: sbom_diff::DepKind::Native,
        });
    }
    let mut cnative: Vec<_> = manifest.c_native.iter().collect();
    cnative.sort_by_key(|(k, _)| *k);
    for (name, spec) in cnative {
        deps.push(sbom_diff::DepEntry {
            name: name.clone(),
            version: spec.version.clone(),
            kind: sbom_diff::DepKind::CNative,
        });
    }

    let sources = collect_source_files(project_root);
    let meta = sbom_diff::BaselineMeta {
        timestamp_secs: sbom_diff::now_secs(),
        half_life_days: 90.0,
        trust_score: 10.0,
        deps,
        source_count: sources.len(),
    };

    std::fs::write(project_root.join(BASELINE_META_FILE), meta.serialize())
        .map_err(|e| PackageError::Io(BASELINE_META_FILE.to_string(), e.to_string()))?;

    Ok(())
}

/// `mvl sbom diff` — compare the current SBOM state against the stored baseline.
///
/// Returns `(diff, regression)` where `regression` is `true` when the trust
/// score dropped by more than 0.5 points (suitable for CI exit-code enforcement).
pub fn cmd_sbom_diff(
    baseline: Option<&str>,
    project_root: &Path,
) -> Result<(sbom_diff::SbomDiff, bool), PackageError> {
    let meta_path = match baseline {
        Some(b) => {
            // Accept either the .json SBOM path or the .meta path directly
            let p = std::path::Path::new(b);
            let meta = p.with_extension("meta");
            if meta.exists() {
                meta
            } else if p.exists() {
                p.to_path_buf()
            } else {
                return Err(PackageError::InvalidInput(format!(
                    "baseline file not found: {b}"
                )));
            }
        }
        None => {
            let p = project_root.join(BASELINE_META_FILE);
            if !p.exists() {
                return Err(PackageError::InvalidInput(
                    "no baseline found — run 'mvl sbom snapshot' first".to_string(),
                ));
            }
            p
        }
    };

    let meta_content = std::fs::read_to_string(&meta_path)
        .map_err(|e| PackageError::Io(meta_path.to_string_lossy().to_string(), e.to_string()))?;
    let meta = sbom_diff::BaselineMeta::parse(&meta_content)
        .map_err(|e| PackageError::InvalidInput(format!("baseline meta: {e}")))?;

    // Build current dep list directly from manifest + lock (no JSON parsing needed)
    let manifest = Manifest::load(project_root)?;
    let lock = LockFile::load_or_empty(project_root);

    let mut current_deps: Vec<sbom_diff::DepEntry> = Vec::new();
    for lp in &lock.packages {
        current_deps.push(sbom_diff::DepEntry {
            name: lp.name.clone(),
            version: lp.version.clone(),
            kind: sbom_diff::DepKind::Mvl,
        });
    }
    for (name, version) in &manifest.native {
        current_deps.push(sbom_diff::DepEntry {
            name: name.clone(),
            version: version.clone(),
            kind: sbom_diff::DepKind::Native,
        });
    }
    for (name, spec) in &manifest.c_native {
        current_deps.push(sbom_diff::DepEntry {
            name: name.clone(),
            version: spec.version.clone(),
            kind: sbom_diff::DepKind::CNative,
        });
    }

    let current_sources = collect_source_files(project_root);
    let current_secs = sbom_diff::now_secs();

    let diff =
        sbom_diff::SbomDiff::compute(&meta, &current_deps, current_sources.len(), current_secs);
    let regression = diff.has_regression(0.5);

    Ok((diff, regression))
}

/// Build the transitive native-dep list from each locked package's cached
/// manifest. Entries whose (name, version) match a project-direct declaration
/// in `manifest.native` / `manifest.c_native` are dropped — the project-direct
/// components are already emitted separately without provenance.
fn collect_transitive_native(
    manifest: &Manifest,
    pkg_manifests: &[(String, Manifest)],
) -> Vec<sbom::TransitiveNativeDep> {
    let mut index: BTreeMap<(String, String, sbom::TransitiveKind), Vec<String>> = BTreeMap::new();
    for (pkg_name, pkg_manifest) in pkg_manifests {
        for (n, v) in &pkg_manifest.native {
            index
                .entry((n.clone(), v.clone(), sbom::TransitiveKind::Native))
                .or_default()
                .push(pkg_name.clone());
        }
        for (n, spec) in &pkg_manifest.c_native {
            index
                .entry((
                    n.clone(),
                    spec.version.clone(),
                    sbom::TransitiveKind::CNative,
                ))
                .or_default()
                .push(pkg_name.clone());
        }
    }
    index
        .into_iter()
        .filter(|((n, v, kind), _)| match kind {
            sbom::TransitiveKind::Native => manifest.native.get(n).is_none_or(|proj_v| proj_v != v),
            sbom::TransitiveKind::CNative => manifest
                .c_native
                .get(n)
                .is_none_or(|proj_spec| &proj_spec.version != v),
        })
        .map(|((name, version, kind), mut introducers)| {
            introducers.sort();
            introducers.dedup();
            sbom::TransitiveNativeDep {
                name,
                version,
                kind,
                introduced_by: introducers,
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::packages::manifest::{CNativeSpec, PackageInfo};
    use std::collections::HashMap;

    fn mk_manifest(name: &str, native: Vec<(&str, &str)>, c_native: Vec<(&str, &str)>) -> Manifest {
        Manifest {
            package: PackageInfo {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                license: "MIT".to_string(),
                requires_mvl: ">=0.60.0".to_string(),
                extern_rationale: None,
            },
            dependencies: HashMap::new(),
            native: native
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            c_native: c_native
                .into_iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        CNativeSpec {
                            version: v.to_string(),
                            license: None,
                        },
                    )
                })
                .collect(),
            dependency_policy: Default::default(),
            license_policy: Default::default(),
            security: Default::default(),
        }
    }

    #[test]
    fn collect_transitive_harvests_native_and_cnative() {
        let project = mk_manifest("proj", vec![], vec![]);
        let pkgs = vec![(
            "pkg-sqlite".to_string(),
            mk_manifest(
                "pkg-sqlite",
                vec![("rusqlite", "0.31")],
                vec![("libssl", "3.0")],
            ),
        )];
        let out = collect_transitive_native(&project, &pkgs);
        assert_eq!(out.len(), 2);
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"rusqlite"));
        assert!(names.contains(&"libssl"));
    }

    #[test]
    fn collect_transitive_dedups_by_name_and_version() {
        let project = mk_manifest("proj", vec![], vec![]);
        let pkgs = vec![
            (
                "pkg-a".to_string(),
                mk_manifest("pkg-a", vec![("rusqlite", "0.31")], vec![]),
            ),
            (
                "pkg-b".to_string(),
                mk_manifest("pkg-b", vec![("rusqlite", "0.31")], vec![]),
            ),
        ];
        let out = collect_transitive_native(&project, &pkgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "rusqlite");
        assert_eq!(out[0].introduced_by, vec!["pkg-a", "pkg-b"]);
    }

    #[test]
    fn collect_transitive_different_versions_kept_separate() {
        let project = mk_manifest("proj", vec![], vec![]);
        let pkgs = vec![
            (
                "pkg-a".to_string(),
                mk_manifest("pkg-a", vec![("rusqlite", "0.31")], vec![]),
            ),
            (
                "pkg-b".to_string(),
                mk_manifest("pkg-b", vec![("rusqlite", "0.32")], vec![]),
            ),
        ];
        let out = collect_transitive_native(&project, &pkgs);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collect_transitive_filters_out_project_shadowed_deps() {
        // Project directly depends on rusqlite@0.31 — transitive entry for the
        // same (name, version) must be dropped to avoid double-emission.
        let project = mk_manifest("proj", vec![("rusqlite", "0.31")], vec![]);
        let pkgs = vec![(
            "pkg-sqlite".to_string(),
            mk_manifest("pkg-sqlite", vec![("rusqlite", "0.31")], vec![]),
        )];
        let out = collect_transitive_native(&project, &pkgs);
        assert!(
            out.is_empty(),
            "project-direct (name, version) must shadow transitive"
        );
    }

    #[test]
    fn collect_transitive_project_different_version_does_not_shadow() {
        // Project depends on rusqlite@0.31 but pkg-sqlite pulls in 0.32 —
        // both must appear (project as direct, pkg's as transitive).
        let project = mk_manifest("proj", vec![("rusqlite", "0.31")], vec![]);
        let pkgs = vec![(
            "pkg-sqlite".to_string(),
            mk_manifest("pkg-sqlite", vec![("rusqlite", "0.32")], vec![]),
        )];
        let out = collect_transitive_native(&project, &pkgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].version, "0.32");
    }
}
