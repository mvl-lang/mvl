// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl sbom` — generate CycloneDX/SPDX SBOMs, snapshot baselines, and diff.

use super::error::PackageError;
use super::fetch::pkg_cache_dir;
use super::lock::LockFile;
use super::manifest::Manifest;
use super::{hash, sbom, sbom_diff};
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
