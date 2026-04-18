//! Toolchain management: install, use, list, uninstall.
//!
//! Implements ADR-0009 Phase B — versioned side-by-side compiler layout:
//!
//! ```text
//! $XDG_DATA_HOME/mvl/toolchains/
//! ├── 0.19.0/
//! │   ├── bin/mvl
//! │   └── std/
//! └── 0.20.0/
//!     ├── bin/mvl
//!     └── std/
//! ```
//!
//! Symlinks in `~/.local/bin/`:
//!   `mvl`         → active toolchain binary (set by `mvl self use`)
//!   `mvl@{ver}`   → specific version (created by `mvl self install`)

use std::fs;
use std::path::PathBuf;
use std::process;

// ── Path helpers ─────────────────────────────────────────────────────────────

/// Returns `$MVL_HOME`, `$XDG_DATA_HOME/mvl`, or `~/.local/share/mvl`.
fn mvl_data_home() -> PathBuf {
    if let Ok(home) = std::env::var("MVL_HOME") {
        return PathBuf::from(home);
    }
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("mvl")
}

/// Returns the root toolchains directory: `$XDG_DATA_HOME/mvl/toolchains/`.
pub fn toolchain_root() -> PathBuf {
    mvl_data_home().join("toolchains")
}

/// Returns the directory for a specific version: `toolchains/{version}/`.
pub fn toolchain_dir(version: &str) -> PathBuf {
    toolchain_root().join(version)
}

/// Returns the binary path for a specific version: `toolchains/{version}/bin/mvl`.
pub fn toolchain_bin(version: &str) -> PathBuf {
    toolchain_dir(version).join("bin").join("mvl")
}

/// Returns `$HOME/.local/bin/` where `mvl` and `mvl@{version}` symlinks live.
pub fn local_bin_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local").join("bin"))
        .unwrap_or_else(|| PathBuf::from("/usr/local/bin"))
}

/// Returns the versioned symlink path: `~/.local/bin/mvl@{version}`.
pub fn version_symlink(version: &str) -> PathBuf {
    local_bin_dir().join(format!("mvl@{version}"))
}

/// Returns the active symlink path: `~/.local/bin/mvl`.
pub fn active_symlink() -> PathBuf {
    local_bin_dir().join("mvl")
}

/// Detect the current platform target triple.
///
/// Returns e.g. `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`.
pub fn target_triple() -> &'static str {
    // CARGO_CFG_TARGET_ARCH / CARGO_CFG_TARGET_OS not available at runtime;
    // use std::env::consts instead.
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        (os, arch) => {
            eprintln!("error: unsupported platform: {arch}-{os}");
            process::exit(1);
        }
    }
}

/// Validate a version string against strict semver (`MAJOR.MINOR.PATCH`).
///
/// Rejects path-traversal sequences (`..`, `/`), shell metacharacters, and
/// anything that is not a plain dotted numeric triple.
pub fn validate_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// `mvl self install <version>` — download release binary and create symlinks.
///
/// Layout after install:
/// - `toolchains/{version}/bin/mvl` — the downloaded binary (executable)
/// - `toolchains/{version}/std/`   — stdlib extracted via `{binary} init`
/// - `~/.local/bin/mvl@{version}`  — versioned symlink
pub fn cmd_self_install(version: &str) {
    if !validate_version(version) {
        eprintln!("error: invalid version '{version}' — expected MAJOR.MINOR.PATCH (e.g. 0.41.0)");
        process::exit(1);
    }

    let bin_path = toolchain_bin(version);

    if bin_path.exists() {
        println!(
            "mvl {version} is already installed at {}",
            bin_path.display()
        );
        return;
    }

    let triple = target_triple();
    let url = format!(
        "https://github.com/LAB271/mvl_language/releases/download/v{version}/mvl-{version}-{triple}"
    );

    println!("Downloading mvl {version} ({triple})...");
    println!("  from: {url}");

    // Create the bin directory.
    let bin_dir = bin_path.parent().expect("bin path must have parent");
    fs::create_dir_all(bin_dir).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {e}", bin_dir.display());
        process::exit(1);
    });

    // Download via ureq — writes to a .part file, renamed on success so that
    // a failed or interrupted download cannot leave a corrupt binary at the
    // final path and block a subsequent install attempt.
    download_binary(&url, &bin_path);

    // Make executable (Unix only).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path)
            .unwrap_or_else(|e| {
                eprintln!("error: cannot read permissions: {e}");
                process::exit(1);
            })
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin_path, perms).unwrap_or_else(|e| {
            eprintln!("error: cannot set executable bit: {e}");
            process::exit(1);
        });
    }

    // Extract stdlib via `{binary} init`.
    println!("Initialising stdlib for v{version}...");
    run_init(&bin_path, version);

    // Create versioned symlink: `~/.local/bin/mvl@{version}`.
    let link = version_symlink(version);
    fs::create_dir_all(local_bin_dir()).unwrap_or_else(|e| {
        eprintln!("warning: cannot create {}: {e}", local_bin_dir().display());
    });
    create_symlink(&bin_path, &link);

    println!("Installed mvl {version}.");
    println!("  binary:  {}", bin_path.display());
    println!("  symlink: {}", link.display());
    println!("Run `mvl self use {version}` to activate this version.");
}

/// `mvl self use <version>` — repoint `~/.local/bin/mvl` to the given version.
pub fn cmd_self_use(version: &str) {
    if !validate_version(version) {
        eprintln!("error: invalid version '{version}' — expected MAJOR.MINOR.PATCH (e.g. 0.41.0)");
        process::exit(1);
    }

    let bin_path = toolchain_bin(version);
    if !bin_path.exists() {
        eprintln!("error: mvl {version} is not installed");
        eprintln!("  Run `mvl self install {version}` first.");
        process::exit(1);
    }

    let link = active_symlink();
    fs::create_dir_all(local_bin_dir()).unwrap_or_else(|e| {
        eprintln!("warning: cannot create {}: {e}", local_bin_dir().display());
    });
    create_symlink(&bin_path, &link);

    println!("Now using mvl {version}.");
    println!("  {} → {}", link.display(), bin_path.display());
}

/// `mvl self list` — print installed toolchain versions.
pub fn cmd_self_list() {
    let versions = installed_versions();
    let root = toolchain_root();

    if versions.is_empty() {
        println!("No toolchains installed ({})", root.display());
        return;
    }

    println!("Installed toolchains:");
    for (ver, is_active) in &versions {
        let marker = if *is_active { " (active)" } else { "" };
        println!("  mvl {ver}{marker}");
    }
}

/// Returns installed toolchain versions sorted by semver, each paired with an
/// `is_active` flag indicating whether the active symlink points to that version.
///
/// Extracted for testability.
pub fn installed_versions() -> Vec<(String, bool)> {
    let root = toolchain_root();
    if !root.exists() {
        return Vec::new();
    }

    let active = active_symlink_target();

    let mut versions: Vec<String> = fs::read_dir(&root)
        .unwrap_or_else(|e| {
            eprintln!("error: cannot read {}: {e}", root.display());
            process::exit(1);
        })
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let ver = entry.file_name().to_string_lossy().into_owned();
            // Only list versions that have a binary installed.
            if toolchain_bin(&ver).exists() {
                Some(ver)
            } else {
                None
            }
        })
        .collect();

    // Sort by parsed (major, minor, patch) tuples — lexicographic order is
    // wrong for semver (e.g. "0.9.0" > "0.41.0" lexicographically).
    versions.sort_by_key(|v| parse_semver(v));

    versions
        .into_iter()
        .map(|ver| {
            let bin = toolchain_bin(&ver);
            let canonical_bin = fs::canonicalize(&bin)
                .unwrap_or(bin)
                .to_string_lossy()
                .into_owned();
            let is_active = active.as_deref() == Some(&canonical_bin);
            (ver, is_active)
        })
        .collect()
}

/// `mvl self uninstall <version>` — remove toolchain directory and symlinks.
pub fn cmd_self_uninstall(version: &str) {
    if !validate_version(version) {
        eprintln!("error: invalid version '{version}' — expected MAJOR.MINOR.PATCH (e.g. 0.41.0)");
        process::exit(1);
    }

    let dir = toolchain_dir(version);
    if !dir.exists() {
        eprintln!("error: mvl {version} is not installed");
        process::exit(1);
    }

    // Remove versioned symlink if it points into this toolchain.
    let vlink = version_symlink(version);
    if vlink.exists() || vlink.symlink_metadata().is_ok() {
        fs::remove_file(&vlink).unwrap_or_else(|e| {
            eprintln!("warning: cannot remove {}: {e}", vlink.display());
        });
    }

    // Remove active symlink if it points into this toolchain.
    let alink = active_symlink();
    if let Ok(target) = fs::read_link(&alink) {
        if target.starts_with(&dir) {
            fs::remove_file(&alink).unwrap_or_else(|e| {
                eprintln!(
                    "warning: cannot remove active symlink {}: {e}",
                    alink.display()
                );
            });
            eprintln!(
                "warning: removed active `mvl` symlink — run `mvl self use <version>` to re-activate"
            );
        }
    }

    // Remove the toolchain directory.
    fs::remove_dir_all(&dir).unwrap_or_else(|e| {
        eprintln!("error: cannot remove {}: {e}", dir.display());
        process::exit(1);
    });

    println!("Uninstalled mvl {version}.");
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse a semver string into a `(major, minor, patch)` tuple for sorting.
///
/// Non-numeric components are treated as 0 so that garbage versions sort
/// consistently rather than panicking.
fn parse_semver(v: &str) -> (u32, u32, u32) {
    let mut parts = v.splitn(3, '.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Run `{binary} init` to extract the stdlib after install.
///
/// Uses an explicit `bool` flag rather than `ExitStatus::default()` (whose
/// value is unspecified by the standard) to avoid a potentially spurious
/// "exited non-zero" warning on platforms where the default is non-zero.
fn run_init(bin_path: &std::path::Path, version: &str) {
    match process::Command::new(bin_path).arg("init").status() {
        Err(e) => {
            eprintln!("warning: could not run `mvl init` for v{version}: {e}");
            eprintln!("  Run `mvl@{version} init` manually to extract the stdlib.");
        }
        Ok(status) if !status.success() => {
            eprintln!("warning: `mvl init` exited non-zero for v{version}");
            eprintln!("  Run `mvl@{version} init` manually to extract the stdlib.");
        }
        Ok(_) => {}
    }
}

/// Download `url` to `dest`, exiting on failure.
///
/// Writes to `dest.part` first, then renames atomically to `dest` on success.
/// Any failure removes the `.part` file so that a subsequent install attempt
/// starts with a clean slate rather than seeing a corrupt partial binary.
///
/// # Note on integrity verification
///
/// This download is currently unauthenticated. A future release should publish
/// a SHA-256 manifest alongside each release asset and verify it here before
/// renaming the file to its final path. Track as a follow-up issue.
fn download_binary(url: &str, dest: &std::path::Path) {
    // Work with a sibling `.part` file so the final path only appears after
    // a successful, complete download.
    let part_path = dest.with_extension("part");

    let response = ureq::get(url).call().unwrap_or_else(|e| {
        eprintln!("error: download failed: {e}");
        eprintln!("  URL: {url}");
        eprintln!("  Check that version exists: https://github.com/LAB271/mvl_language/releases");
        process::exit(1);
    });

    if response.status() != 200 {
        eprintln!(
            "error: download returned HTTP {} for {url}",
            response.status()
        );
        eprintln!("  Check that version exists: https://github.com/LAB271/mvl_language/releases");
        process::exit(1);
    }

    let mut file = fs::File::create(&part_path).unwrap_or_else(|e| {
        eprintln!("error: cannot create {}: {e}", part_path.display());
        process::exit(1);
    });

    if let Err(e) = std::io::copy(&mut response.into_reader(), &mut file) {
        eprintln!("error: write failed: {e}");
        let _ = fs::remove_file(&part_path);
        process::exit(1);
    }

    // Rename atomically to the final path.
    fs::rename(&part_path, dest).unwrap_or_else(|e| {
        eprintln!("error: cannot finalise download: {e}");
        let _ = fs::remove_file(&part_path);
        process::exit(1);
    });
}

/// Create a symlink at `link` pointing to `target`, removing any existing link first.
fn create_symlink(target: &std::path::Path, link: &std::path::Path) {
    // Remove existing file or symlink.
    if link.exists() || link.symlink_metadata().is_ok() {
        fs::remove_file(link).unwrap_or_else(|e| {
            eprintln!("warning: cannot remove existing {}: {e}", link.display());
        });
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).unwrap_or_else(|e| {
            eprintln!("warning: cannot create symlink {}: {e}", link.display());
        });
    }
    #[cfg(not(unix))]
    {
        eprintln!(
            "warning: symlinks not supported on this platform; skipping {}",
            link.display()
        );
    }
}

/// Resolve the canonical target of the active symlink.
fn active_symlink_target() -> Option<String> {
    fs::canonicalize(active_symlink())
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}
