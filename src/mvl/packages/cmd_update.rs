// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl update` — re-resolve versions, update mvl.lock.
//!
//! The CLI verb is split into:
//! - [`DepUpdatePlan`] — pure compute layer; given tags + filters → decision.
//!   Unit-testable without touching the network or filesystem.
//! - [`update_one_dep`] / [`refresh_locked`] — IO layer that executes the plan.

use super::config::{GlobalConfig, latest_semver_tag};
use super::error::PackageError;
use super::fetch::{self, fetch_package_opts, pkg_cache_dir};
use super::lock::LockFile;
use super::manifest::{self, DepSpec, Manifest};
use std::path::Path;

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

/// Reason a tag was filtered out during planning (pure compute, IO-free).
#[derive(Debug, PartialEq)]
pub(super) enum ExcludeReason {
    LocalExclude,
    GlobalExclude,
    MinAge { age_days: u64, min_days: u64 },
}

/// Pure-compute decision for updating one dependency.
///
/// Built from a tag list + filters, with no network or filesystem access.
/// This separates the "what should we do?" question from the "do it" step.
#[derive(Debug)]
pub(super) struct DepUpdatePlan {
    /// Tags eligible after filtering (kept in original list order).
    #[allow(dead_code)] // kept for diagnostic inspection; latest_tag is the chosen value
    pub eligible: Vec<String>,
    /// Tags filtered out, with the reason (for user-facing prints).
    pub excluded: Vec<(String, ExcludeReason)>,
    /// Chosen tag (highest semver among `eligible`), or `None` if none qualify.
    pub latest_tag: Option<String>,
    /// `latest_tag` with the optional `v` prefix stripped.
    pub latest_version: Option<String>,
}

impl DepUpdatePlan {
    /// Compute the plan. Pure: no IO, no time-of-day calls.
    pub(super) fn compute(
        tags: Vec<String>,
        tag_dates: &std::collections::HashMap<String, u64>,
        local_exclude: &[String],
        global_exclude: &[String],
        min_age_secs: u64,
        min_age_days: u64,
        now_secs: u64,
    ) -> Self {
        let mut eligible: Vec<String> = Vec::new();
        let mut excluded: Vec<(String, ExcludeReason)> = Vec::new();

        for tag in tags {
            let vstr = tag.strip_prefix('v').unwrap_or(&tag);
            if local_exclude.iter().any(|e| e == vstr || e == tag.as_str()) {
                excluded.push((tag, ExcludeReason::LocalExclude));
                continue;
            }
            if global_exclude
                .iter()
                .any(|e| e == vstr || e == tag.as_str())
            {
                excluded.push((tag, ExcludeReason::GlobalExclude));
                continue;
            }
            if min_age_secs > 0 {
                if let Some(&published) = tag_dates.get(tag.as_str()) {
                    let age_secs = now_secs.saturating_sub(published);
                    if age_secs < min_age_secs {
                        let age_days = age_secs / 86_400;
                        excluded.push((
                            tag,
                            ExcludeReason::MinAge {
                                age_days,
                                min_days: min_age_days,
                            },
                        ));
                        continue;
                    }
                }
            }
            eligible.push(tag);
        }

        let latest_tag = latest_semver_tag(&eligible);
        let latest_version = latest_tag
            .as_ref()
            .map(|t| t.strip_prefix('v').unwrap_or(t).to_string());

        Self {
            eligible,
            excluded,
            latest_tag,
            latest_version,
        }
    }
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
        match manifest::sync_manifest_tags(project_root, &manifest_tag_updates) {
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

    let plan = DepUpdatePlan::compute(
        tags,
        &tag_dates,
        local_exclude,
        global_exclude,
        min_age_secs,
        min_age_days,
        now_secs,
    );

    // Surface the planner's filtering decisions to the user.
    for (tag, reason) in &plan.excluded {
        let vstr = tag.strip_prefix('v').unwrap_or(tag);
        match reason {
            ExcludeReason::LocalExclude => {
                println!("  skipped {name}@{vstr} (excluded in mvl.toml)");
            }
            ExcludeReason::GlobalExclude => {
                println!("  skipped {name}@{vstr} (excluded in global config)");
            }
            ExcludeReason::MinAge { age_days, min_days } => {
                println!(
                    "  skipped {name}@{vstr} (published {age_days} day(s) ago, min_age_days={min_days})"
                );
            }
        }
    }

    let (latest, latest_version) = match (plan.latest_tag, plan.latest_version) {
        (Some(t), Some(v)) => (t, v),
        _ => {
            println!("  {name}: no eligible versions available (all filtered)");
            return DepOutcome::NoEligible;
        }
    };

    let current_version = lockfile
        .get(name)
        .map(|p| p.version.clone())
        .unwrap_or_else(|| "0.0.0".to_string());

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

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

    // --- DepUpdatePlan pure-compute tests ---

    #[test]
    fn dep_plan_picks_highest_semver_when_no_filters() {
        let plan = DepUpdatePlan::compute(
            tags(&["v0.1.0", "v0.3.0", "v0.2.0"]),
            &HashMap::new(),
            &[],
            &[],
            0,
            0,
            0,
        );
        assert_eq!(plan.latest_tag.as_deref(), Some("v0.3.0"));
        assert_eq!(plan.latest_version.as_deref(), Some("0.3.0"));
        assert!(plan.excluded.is_empty());
    }

    #[test]
    fn dep_plan_applies_local_exclude_by_version_string() {
        let plan = DepUpdatePlan::compute(
            tags(&["v0.1.0", "v0.2.0", "v0.3.0"]),
            &HashMap::new(),
            &["0.3.0".to_string()],
            &[],
            0,
            0,
            0,
        );
        assert_eq!(plan.latest_tag.as_deref(), Some("v0.2.0"));
        assert_eq!(plan.excluded.len(), 1);
        assert_eq!(plan.excluded[0].1, ExcludeReason::LocalExclude);
    }

    #[test]
    fn dep_plan_applies_global_exclude_by_tag_form() {
        let plan = DepUpdatePlan::compute(
            tags(&["v0.1.0", "v0.2.0", "v0.3.0"]),
            &HashMap::new(),
            &[],
            &["v0.3.0".to_string()],
            0,
            0,
            0,
        );
        assert_eq!(plan.latest_tag.as_deref(), Some("v0.2.0"));
        assert_eq!(plan.excluded.len(), 1);
        assert_eq!(plan.excluded[0].1, ExcludeReason::GlobalExclude);
    }

    #[test]
    fn dep_plan_applies_min_age_filter() {
        // A tag published "now" must be filtered when min-age is 7 days.
        let now: u64 = 100_000_000;
        let mut dates = HashMap::new();
        dates.insert("v0.2.0".to_string(), now); // just published
        dates.insert("v0.1.0".to_string(), now - 30 * 86_400); // 30 days old
        let plan = DepUpdatePlan::compute(
            tags(&["v0.1.0", "v0.2.0"]),
            &dates,
            &[],
            &[],
            7 * 86_400,
            7,
            now,
        );
        // v0.2.0 too young → v0.1.0 wins
        assert_eq!(plan.latest_tag.as_deref(), Some("v0.1.0"));
        assert!(
            plan.excluded
                .iter()
                .any(|(t, r)| t == "v0.2.0" && matches!(r, ExcludeReason::MinAge { .. }))
        );
    }

    #[test]
    fn dep_plan_returns_none_when_all_excluded() {
        let plan = DepUpdatePlan::compute(
            tags(&["v0.1.0", "v0.2.0"]),
            &HashMap::new(),
            &["0.1.0".to_string(), "0.2.0".to_string()],
            &[],
            0,
            0,
            0,
        );
        assert_eq!(plan.latest_tag, None);
        assert_eq!(plan.latest_version, None);
        assert_eq!(plan.excluded.len(), 2);
    }

    #[test]
    fn dep_plan_ignores_non_semver_tags() {
        let plan = DepUpdatePlan::compute(
            tags(&["main", "v0.1.0", "latest"]),
            &HashMap::new(),
            &[],
            &[],
            0,
            0,
            0,
        );
        assert_eq!(plan.latest_tag.as_deref(), Some("v0.1.0"));
    }
}
