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
pub mod sbom_diff;
pub mod version;

use fetch::{fetch_package, fetch_package_opts, pkg_cache_dir, resolve_pkg_dir, verify_hash};
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
            // If it doesn't match, it's a manual override (ADR-0039) — leave it alone.
            if verify_hash(&local_dir, &pkg.hash).is_ok() {
                already_local += 1;
            } else if newly_fetched {
                from_network += 1;
            } else {
                from_cache += 1;
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

/// Options for `mvl update` (#1456).
#[derive(Debug, Default, Clone)]
pub struct UpdateOptions {
    /// Re-clone cached packages instead of trusting on-disk content (#1455).
    pub force: bool,
    /// Skip all network calls; report cache-vs-lock state only.
    pub offline: bool,
    /// Compute the update plan but do not write mvl.lock or mvl.toml.
    pub dry_run: bool,
    /// Restrict the update to a single dependency name.
    pub only: Option<String>,
}

/// Outcome of attempting to update a single dependency.
enum DepOutcome {
    /// Lock file was updated (version or commit SHA changed).
    Updated,
    /// Already at the latest published version and content matches the remote.
    UpToDate,
    /// User excluded all candidate versions (or none had a semver tag).
    NoEligible,
    /// Network or git failure — caller should warn and continue (#1458).
    Skipped(String),
    /// Plan-only mode (dry-run / offline): no mutation performed.
    Planned,
}

/// `mvl update`
///
/// Re-resolves versions for all git dependencies, fetches any newer tags,
/// and rewrites `mvl.lock` with updated versions and hashes.
///
/// Respects:
/// - `[security] min-age-days` in `mvl.toml` or global XDG config
/// - `exclude = [...]` per-dependency in `mvl.toml`
/// - `[exclusions]` global table in `$XDG_CONFIG_HOME/mvl/config.toml`
///
/// Behavioral flags live on `UpdateOptions` (#1456).
pub fn cmd_update(project_root: &Path, opts: &UpdateOptions) -> Result<(), PackageError> {
    let manifest = Manifest::load(project_root)?;

    if manifest.dependencies.is_empty() {
        println!("No dependencies in mvl.toml.");
        return Ok(());
    }

    let global = GlobalConfig::load();

    let min_age_days = if manifest.security.min_age_days > 0 {
        manifest.security.min_age_days
    } else {
        global.min_age_days
    };

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let min_age_secs = min_age_days * 86_400;

    let mut lockfile = LockFile::load_or_empty(project_root);
    let mut manifest_tag_updates: Vec<(String, String)> = Vec::new();

    let mut updated = 0usize;
    let mut up_to_date = 0usize;
    let mut skipped = 0usize;
    let mut planned = 0usize;
    let mut total = 0usize;

    for (name, spec) in &manifest.dependencies {
        if let Some(ref only) = opts.only {
            if only != name {
                continue;
            }
        }
        total += 1;

        let outcome = update_one_dep(
            name,
            spec,
            &mut lockfile,
            &mut manifest_tag_updates,
            &global,
            min_age_secs,
            min_age_days,
            now_secs,
            opts,
        );
        match outcome {
            DepOutcome::Updated => updated += 1,
            DepOutcome::UpToDate => up_to_date += 1,
            DepOutcome::NoEligible => skipped += 1,
            DepOutcome::Skipped(reason) => {
                skipped += 1;
                eprintln!("warning: {name}: {reason}");
            }
            DepOutcome::Planned => planned += 1,
        }
    }

    if opts.only.is_some() && total == 0 {
        return Err(PackageError::InvalidInput(format!(
            "no dependency named '{}' in mvl.toml",
            opts.only.as_deref().unwrap_or("")
        )));
    }

    if opts.dry_run || opts.offline {
        println!(
            "(dry-run) Would update {updated} package(s), {up_to_date} unchanged, {skipped} skipped, {planned} planned.",
        );
        return Ok(());
    }

    // ── Phase 2: transitive dependency resolution ─────────────────────────────
    //
    // BFS over each resolved package's own mvl.toml to discover and lock
    // their transitive dependencies.  Packages already in the lockfile are
    // in `queued` from the start and are not re-fetched.
    {
        use std::collections::{HashSet, VecDeque};

        let mut transitive_added = 0usize;
        let mut queued: HashSet<String> =
            lockfile.packages.iter().map(|p| p.name.clone()).collect();
        let mut work_queue: VecDeque<String> = queued.iter().cloned().collect();
        let mut dummy_tag_updates: Vec<(String, String)> = Vec::new();

        while let Some(pkg_name) = work_queue.pop_front() {
            let (cached_name, cached_version) = match lockfile.get(&pkg_name) {
                Some(p) => (p.name.clone(), p.version.clone()),
                None => continue,
            };
            let cache_dir = pkg_cache_dir(&cached_name, &cached_version);
            let pkg_manifest = match Manifest::load(&cache_dir) {
                Ok(m) => m,
                Err(_) => continue,
            };
            for (dep_name, dep_spec) in &pkg_manifest.dependencies {
                if queued.contains(dep_name) {
                    continue;
                }
                queued.insert(dep_name.clone());
                let outcome = update_one_dep(
                    dep_name,
                    dep_spec,
                    &mut lockfile,
                    &mut dummy_tag_updates,
                    &global,
                    min_age_secs,
                    min_age_days,
                    now_secs,
                    opts,
                );
                match outcome {
                    DepOutcome::Updated => {
                        transitive_added += 1;
                        work_queue.push_back(dep_name.clone());
                    }
                    DepOutcome::UpToDate | DepOutcome::NoEligible | DepOutcome::Planned => {
                        work_queue.push_back(dep_name.clone());
                    }
                    DepOutcome::Skipped(reason) => {
                        eprintln!("  warning: transitive dep {dep_name}: {reason}");
                    }
                }
            }
        }

        if transitive_added > 0 {
            println!("Resolved {transitive_added} transitive package(s).");
            updated += transitive_added;
        }
    }

    // Persist mvl.lock (mvl.toml tag sync is handled below, #1459).
    lockfile.write(project_root)?;

    if !manifest_tag_updates.is_empty() {
        match sync_manifest_tags(project_root, &manifest_tag_updates) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("warning: could not sync mvl.toml tags: {e}");
                for (n, v) in &manifest_tag_updates {
                    eprintln!("  hint: bump '{n}' tag in mvl.toml to v{v}");
                }
            }
        }
    }

    if total == 0 {
        println!("No dependencies matched (filter active).");
    } else if updated > 0 || skipped > 0 {
        println!("Updated {updated} package(s), {up_to_date} unchanged, {skipped} skipped.",);
    } else {
        println!("All packages are up to date.");
    }

    // Exit non-zero only when *every* dep failed (#1458).
    if total > 0 && skipped == total {
        return Err(PackageError::NoVersion(format!(
            "all {total} dependencies failed to update — see warnings above"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_one_dep(
    name: &str,
    spec: &DepSpec,
    lockfile: &mut LockFile,
    manifest_tag_updates: &mut Vec<(String, String)>,
    global: &GlobalConfig,
    min_age_secs: u64,
    min_age_days: u64,
    now_secs: u64,
    opts: &UpdateOptions,
) -> DepOutcome {
    let (git_url, local_exclude) = match spec {
        DepSpec::Git { git, exclude, .. } => (git.clone(), exclude.as_slice()),
        DepSpec::Version(constraint) => {
            eprintln!(
                "warning: cannot update '{name}' (version constraint '{constraint}' has no git URL)"
            );
            return DepOutcome::Skipped("no git URL".to_string());
        }
    };

    let global_exclude = global
        .exclusions
        .get(&git_url)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    println!("Checking {name}...");

    // Offline mode (#1456): use whatever the lockfile already has, warn on age.
    if opts.offline {
        let entry = lockfile.get(name);
        match entry {
            Some(p) => {
                if let Some(last) = p.last_checked {
                    let age_days = now_secs.saturating_sub(last) / 86_400;
                    println!(
                        "  {name}: locked at {} (last checked {} day(s) ago — offline mode, not refreshing)",
                        p.version, age_days
                    );
                } else {
                    println!(
                        "  {name}: locked at {} (no last-checked timestamp — offline mode)",
                        p.version
                    );
                }
                return DepOutcome::Planned;
            }
            None => return DepOutcome::Skipped("not in lockfile (offline)".to_string()),
        }
    }

    // Optionally fetch tag publish dates for the min-age-days policy.
    let tag_dates: std::collections::HashMap<String, u64> = if min_age_secs > 0 {
        match fetch::fetch_tag_dates(&git_url) {
            Ok(d) => d,
            Err(e) if e.is_recoverable() => {
                eprintln!(
                    "  warning: could not fetch tag dates for {name}: {e} — proceeding without age filter"
                );
                std::collections::HashMap::new()
            }
            Err(e) => return DepOutcome::Skipped(e.to_string()),
        }
    } else {
        std::collections::HashMap::new()
    };

    let tags = match fetch::list_git_tags(&git_url) {
        Ok(t) => t,
        Err(e) if e.is_recoverable() => return DepOutcome::Skipped(e.to_string()),
        Err(e) => return DepOutcome::Skipped(e.to_string()),
    };

    let eligible: Vec<String> = tags
        .into_iter()
        .filter(|tag| {
            let vstr = tag.strip_prefix('v').unwrap_or(tag);
            if local_exclude.iter().any(|e| e == vstr || e == tag.as_str()) {
                println!("  skipped {name}@{vstr} (excluded in mvl.toml)");
                return false;
            }
            if global_exclude.iter().any(|e| e == vstr || e == tag.as_str()) {
                println!("  skipped {name}@{vstr} (excluded in global config)");
                return false;
            }
            if min_age_secs > 0 {
                if let Some(&published) = tag_dates.get(tag.as_str()) {
                    let age_secs = now_secs.saturating_sub(published);
                    let age_days = age_secs / 86_400;
                    if age_secs < min_age_secs {
                        println!(
                            "  skipped {name}@{vstr} (published {age_days} day(s) ago, min_age_days={min_age_days})"
                        );
                        return false;
                    }
                }
            }
            true
        })
        .collect();

    let latest = match latest_semver_tag(&eligible) {
        Some(t) => t,
        None => {
            println!("  {name}: no eligible versions available (all filtered)");
            return DepOutcome::NoEligible;
        }
    };

    let current_version = lockfile
        .get(name)
        .map(|p| p.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());
    let latest_version = latest.strip_prefix('v').unwrap_or(&latest).to_string();

    if latest_version == current_version {
        // #1461: even on "up to date", cross-check remote tag SHA against locked commit.
        if let Some(locked) = lockfile.get(name).cloned() {
            match fetch::ls_remote_tag_sha(&git_url, &latest) {
                Ok(Some(remote_sha)) => {
                    if let Some(ref locked_sha) = locked.commit {
                        if !shas_equal(locked_sha, &remote_sha) {
                            eprintln!(
                                "warning: {name}@{current_version} remote commit ({remote_sha}) differs from locked ({locked_sha})"
                            );
                            eprintln!(
                                "  hint: re-run with --force to refresh cache, or investigate upstream tag manipulation"
                            );
                            if opts.force {
                                return refresh_locked(
                                    name,
                                    &git_url,
                                    &latest,
                                    lockfile,
                                    manifest_tag_updates,
                                    &current_version,
                                    &latest_version,
                                    now_secs,
                                    opts,
                                );
                            }
                        }
                    }
                }
                Ok(None) => {
                    eprintln!(
                        "warning: {name}: remote tag {latest} not found (may have been deleted)"
                    );
                }
                Err(e) if e.is_recoverable() => {
                    eprintln!("  warning: could not cross-check remote SHA for {name}: {e}");
                }
                Err(e) => return DepOutcome::Skipped(e.to_string()),
            }
        }
        println!("  {name} is up to date ({current_version})");
        // Refresh last-checked even when version unchanged.
        if !opts.dry_run {
            if let Some(existing) = lockfile.get(name).cloned() {
                let mut refreshed = existing;
                refreshed.last_checked = Some(now_secs);
                lockfile.upsert(refreshed);
            }
        }
        return DepOutcome::UpToDate;
    }

    println!("  {name}: {current_version} → {latest_version}");
    refresh_locked(
        name,
        &git_url,
        &latest,
        lockfile,
        manifest_tag_updates,
        &current_version,
        &latest_version,
        now_secs,
        opts,
    )
}

#[allow(clippy::too_many_arguments)]
fn refresh_locked(
    name: &str,
    git_url: &str,
    latest_tag: &str,
    lockfile: &mut LockFile,
    manifest_tag_updates: &mut Vec<(String, String)>,
    current_version: &str,
    latest_version: &str,
    now_secs: u64,
    opts: &UpdateOptions,
) -> DepOutcome {
    if opts.dry_run {
        println!("  (would update) {name}: {current_version} → {latest_version}");
        return DepOutcome::Planned;
    }
    let mut locked = match fetch_package_opts(name, git_url, latest_tag, opts.force) {
        Ok(p) => p,
        Err(e) if e.is_recoverable() => return DepOutcome::Skipped(e.to_string()),
        Err(e) => return DepOutcome::Skipped(e.to_string()),
    };
    locked.last_checked = Some(now_secs);
    lockfile.upsert(locked);
    if current_version != latest_version {
        manifest_tag_updates.push((name.to_string(), latest_version.to_string()));
    }
    DepOutcome::Updated
}

/// Best-effort comparison between two git SHAs of possibly-different lengths
/// (e.g. abbreviated vs full). Matches when one is a prefix of the other.
fn shas_equal(a: &str, b: &str) -> bool {
    let a = a.trim();
    let b = b.trim();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.starts_with(b) || b.starts_with(a)
}

/// Rewrite the `tag = "..."` field for each updated dep in `mvl.toml` (#1459).
///
/// Uses a line-surgical edit (not a full to_toml roundtrip) so comments and
/// formatting in the user's manifest are preserved.
fn sync_manifest_tags(
    project_root: &Path,
    updates: &[(String, String)],
) -> Result<(), PackageError> {
    let path = project_root.join("mvl.toml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| PackageError::Io(path.display().to_string(), e.to_string()))?;

    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let mut replaced = line.to_string();
        let trimmed = line.trim_start();
        // Match lines that begin with `"<dep-name>" = { ... }` and contain a
        // `tag = "..."` entry.
        if let Some(after_quote) = trimmed.strip_prefix('"') {
            if let Some(close_quote) = after_quote.find('"') {
                let dep_name = &after_quote[..close_quote];
                if let Some((_, new_ver)) = updates.iter().find(|(n, _)| n == dep_name) {
                    replaced = rewrite_tag_in_line(line, new_ver);
                }
            }
        }
        out.push_str(&replaced);
        out.push('\n');
    }
    // Preserve original trailing newline behavior — only strip the one we added
    // if the original didn't end with a newline.
    if !content.ends_with('\n') {
        out.pop();
    }
    std::fs::write(&path, out)
        .map_err(|e| PackageError::Io(path.display().to_string(), e.to_string()))?;
    Ok(())
}

/// Replace a `tag = "vX.Y.Z"` substring in `line` with the new version,
/// preserving an existing `v` prefix if present.
fn rewrite_tag_in_line(line: &str, new_version: &str) -> String {
    let needle = "tag = \"";
    let start = match line.find(needle) {
        Some(i) => i + needle.len(),
        None => return line.to_string(),
    };
    let rest = &line[start..];
    let end_off = match rest.find('"') {
        Some(i) => i,
        None => return line.to_string(),
    };
    let current_tag = &rest[..end_off];
    let new_tag = if current_tag.starts_with('v') {
        format!("v{new_version}")
    } else {
        new_version.to_string()
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..start]);
    out.push_str(&new_tag);
    out.push_str(&line[start + end_off..]);
    out
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

// ── SBOM snapshot and diff (#636) ────────────────────────────────────────────

const BASELINE_SBOM_FILE: &str = ".mvl/sbom.baseline.json";
const BASELINE_META_FILE: &str = ".mvl/sbom.baseline.meta";

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
/// Global config loaded from `$XDG_CONFIG_HOME/mvl/config.toml`.
struct GlobalConfig {
    /// Global `min-age-days` default (overridden by project-level `[security]`).
    min_age_days: u64,
    /// Global exclusion lists keyed by git URL.
    exclusions: std::collections::HashMap<String, Vec<String>>,
}

impl GlobalConfig {
    fn load() -> Self {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| std::path::PathBuf::from(h).join(".config"))
            })
            .unwrap_or_else(|| std::path::PathBuf::from(".config"));
        let path = config_dir.join("mvl").join("config.toml");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        Self::parse(&content)
    }

    fn parse(content: &str) -> Self {
        let mut min_age_days = 0u64;
        let mut exclusions: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut current_section = String::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                current_section = line[1..line.len() - 1].trim().to_string();
                continue;
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim().trim_matches('"');
                let val = line[eq + 1..].trim();
                match current_section.as_str() {
                    "security" if key == "min-age-days" => {
                        if let Ok(n) = val.parse::<u64>() {
                            min_age_days = n;
                        }
                    }
                    "exclusions" => {
                        // key = ["ver1", "ver2"]
                        let git_url = key.to_string();
                        let versions = parse_string_array(val);
                        exclusions.insert(git_url, versions);
                    }
                    _ => {}
                }
            }
        }
        Self {
            min_age_days,
            exclusions,
        }
    }

    fn default() -> Self {
        Self {
            min_age_days: 0,
            exclusions: std::collections::HashMap::new(),
        }
    }
}

fn parse_string_array(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return vec![];
    }
    let inner = &s[1..s.len() - 1];
    inner
        .split(',')
        .filter_map(|part| {
            let p = part.trim();
            if p.starts_with('"') && p.ends_with('"') && p.len() >= 2 {
                Some(p[1..p.len() - 1].to_string())
            } else {
                None
            }
        })
        .collect()
}

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
        cmd_install(tmp.path(), false).unwrap();
    }

    #[test]
    fn cmd_install_missing_lockfile_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.lock → should return Err, not panic
        let result = cmd_install(tmp.path(), false);
        assert!(result.is_err());
    }

    // --- cmd_update ---

    #[test]
    fn cmd_update_no_deps_returns_early() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        cmd_update(tmp.path(), &UpdateOptions::default()).unwrap();
    }

    #[test]
    fn cmd_update_version_only_dep_skips_without_network() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"some-pkg\" = \">=1.0.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        // Version-only dep has no git URL → warn-and-skip path (#1458). With only one
        // dep and it failing, the run is "all skipped" which is an error.
        let err = cmd_update(tmp.path(), &UpdateOptions::default()).unwrap_err();
        assert!(matches!(err, PackageError::NoVersion(_)));
    }

    #[test]
    fn cmd_update_missing_manifest_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.toml → should return Err, not panic
        let result = cmd_update(tmp.path(), &UpdateOptions::default());
        assert!(result.is_err());
    }

    // --- cmd_update flag/behavior tests (#1456 / #1458 / #1459) ---

    #[test]
    fn cmd_update_offline_does_not_write_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"x\" = { git = \"https://example.invalid/x\", tag = \"v1.0.0\" }\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        let opts = UpdateOptions {
            offline: true,
            ..Default::default()
        };
        // Offline mode prints the plan and exits without writing mvl.lock.
        let res = cmd_update(tmp.path(), &opts);
        assert!(res.is_ok());
        assert!(!tmp.path().join("mvl.lock").exists());
    }

    #[test]
    fn cmd_update_dry_run_does_not_write_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        let opts = UpdateOptions {
            dry_run: true,
            ..Default::default()
        };
        cmd_update(tmp.path(), &opts).unwrap();
        assert!(!tmp.path().join("mvl.lock").exists());
    }

    #[test]
    fn cmd_update_package_filter_unknown_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\nname = \"proj\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"some-pkg\" = \">=1.0.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        let opts = UpdateOptions {
            only: Some("does-not-exist".to_string()),
            ..Default::default()
        };
        let err = cmd_update(tmp.path(), &opts).unwrap_err();
        assert!(matches!(err, PackageError::InvalidInput(_)));
    }

    // --- rewrite_tag_in_line / sync_manifest_tags (#1459) ---

    #[test]
    fn rewrite_tag_preserves_v_prefix() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"v0.2.3\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("tag = \"v0.3.0\""), "got: {out}");
    }

    #[test]
    fn rewrite_tag_no_v_prefix_kept() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"0.2.3\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("tag = \"0.3.0\""), "got: {out}");
        assert!(!out.contains("v0.3.0"));
    }

    #[test]
    fn rewrite_tag_preserves_rest_of_line() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"v0.2.3\", rationale = \"x\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("rationale = \"x\""), "got: {out}");
    }

    #[test]
    fn rewrite_tag_no_match_returns_unchanged() {
        let line = "\"pkg\" = \">=1.0.0\"";
        let out = rewrite_tag_in_line(line, "2.0.0");
        assert_eq!(out, line);
    }

    #[test]
    fn sync_manifest_tags_rewrites_only_matching_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let original = "[package]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"foo\" = { git = \"https://e.x/foo\", tag = \"v0.1.0\" }\n\"bar\" = { git = \"https://e.x/bar\", tag = \"v0.5.0\" }\n";
        std::fs::write(tmp.path().join("mvl.toml"), original).unwrap();
        let updates = vec![("foo".to_string(), "0.2.0".to_string())];
        sync_manifest_tags(tmp.path(), &updates).unwrap();
        let result = std::fs::read_to_string(tmp.path().join("mvl.toml")).unwrap();
        assert!(result.contains("\"foo\" = { git = \"https://e.x/foo\", tag = \"v0.2.0\" }"));
        assert!(result.contains("\"bar\" = { git = \"https://e.x/bar\", tag = \"v0.5.0\" }"));
    }

    // --- shas_equal ---

    #[test]
    fn shas_equal_full_match() {
        assert!(shas_equal("abc123def456", "abc123def456"));
    }

    #[test]
    fn shas_equal_prefix_match() {
        assert!(shas_equal("abc123", "abc123def456"));
        assert!(shas_equal("abc123def456", "abc123"));
    }

    #[test]
    fn shas_equal_no_match() {
        assert!(!shas_equal("abc123", "def456"));
    }

    #[test]
    fn shas_equal_empty_strings_never_match() {
        assert!(!shas_equal("", "abc"));
        assert!(!shas_equal("abc", ""));
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

    // --- install_local ---

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
