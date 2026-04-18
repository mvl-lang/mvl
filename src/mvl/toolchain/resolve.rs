//! Version resolution chain for ADR-0009 Phase C.
//!
//! Priority order (highest wins):
//!  1. `argv[0]` — binary invoked as `mvl@X.Y.Z`
//!  2. `.mvl-version` file — walk up from CWD
//!  3. `mvl.toml` `requires-mvl` field — walk up from CWD
//!  4. `~/.mvl-version` — global user default
//!  5. Active symlink (`~/.local/bin/mvl`) — fallback

use std::path::{Path, PathBuf};

use super::{installed_versions, toolchain_bin, validate_version};

// ── Public API ────────────────────────────────────────────────────────────────

/// Extract a version string from `argv[0]`, e.g. `mvl@0.34.0` → `"0.34.0"`.
///
/// Handles bare names, paths (`./bin/mvl@0.34.0`), and symlinks.  Returns
/// `None` when `argv[0]` is the plain `mvl` binary without a version suffix.
pub fn extract_version_from_argv0(argv0: &str) -> Option<String> {
    // Work with just the final path component.
    let basename = Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(argv0);

    let version = basename.strip_prefix("mvl@")?;
    if validate_version(version) {
        Some(version.to_owned())
    } else {
        None
    }
}

/// Walk up from `start` looking for a `.mvl-version` file.
///
/// Returns the trimmed contents of the first file found, or `None` when the
/// filesystem root is reached without finding one.
pub fn find_project_mvl_version(start: &Path) -> Option<String> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(".mvl-version");
        if let Ok(content) = std::fs::read_to_string(&candidate) {
            let version = content.trim().to_owned();
            if !version.is_empty() && validate_version(&version) {
                return Some(version);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Walk up from `start` looking for `mvl.toml` and extract its `requires-mvl`
/// field from the `[package]` section.
///
/// The field may be an exact version (`"0.24.0"`) or a minimum constraint
/// (`">=0.24.0"`).  Returns the resolved exact version, or `None` when not
/// found or no installed version satisfies the constraint.
pub fn find_manifest_requires_mvl(start: &Path) -> Option<String> {
    let manifest_path = find_mvl_toml(start)?;
    let raw = extract_requires_mvl_field(&manifest_path)?;
    resolve_constraint(&raw)
}

/// Read `~/.mvl-version` (the global user default).
pub fn find_global_mvl_version() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".mvl-version");
    let content = std::fs::read_to_string(path).ok()?;
    let version = content.trim().to_owned();
    if version.is_empty() || !validate_version(&version) {
        None
    } else {
        Some(version)
    }
}

/// Run the full resolution chain and return the target version, or `None` when
/// no version can be determined (caller should fall through to the active
/// symlink / current binary).
///
/// `argv0` is `std::env::args().next()` (or equivalent in tests).
/// `cwd` is the working directory for file-system walk-up.
pub fn resolve_version(argv0: &str, cwd: &Path) -> Option<String> {
    // 1. argv[0] override.
    if let Some(v) = extract_version_from_argv0(argv0) {
        return Some(v);
    }
    // 2. Project .mvl-version.
    if let Some(v) = find_project_mvl_version(cwd) {
        return Some(v);
    }
    // 3. mvl.toml requires-mvl.
    if let Some(v) = find_manifest_requires_mvl(cwd) {
        return Some(v);
    }
    // 4. Global ~/.mvl-version.
    if let Some(v) = find_global_mvl_version() {
        return Some(v);
    }
    // 5. No pin found — caller uses current binary.
    None
}

/// Re-exec `target_binary` with `args`, replacing the current process.
///
/// On Unix this is a true `execv` — control never returns on success.
/// On other platforms a child process is spawned; the current process exits
/// with the child's status code after it finishes.
///
/// Exits with a clear error message if the binary does not exist or cannot be
/// executed.
///
/// Does not perform an `exists()` pre-check — that would introduce a TOCTOU
/// race between the check and the exec.  Instead, `execv` / `Command` is
/// called directly and `NotFound` is handled in the error path.
pub fn reexec(target_binary: &Path, args: &[String]) -> ! {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_binary = CString::new(target_binary.as_os_str().as_bytes())
            .expect("binary path contains null byte");

        let mut c_args: Vec<CString> = Vec::with_capacity(args.len() + 1);
        c_args.push(c_binary.clone());
        for a in args.iter().skip(1) {
            match CString::new(a.as_bytes()) {
                Ok(s) => c_args.push(s),
                Err(_) => {
                    eprintln!("error: argument contains null byte, cannot exec");
                    std::process::exit(1);
                }
            }
        }
        let c_argv: Vec<*const libc::c_char> = c_args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        unsafe { libc::execv(c_binary.as_ptr(), c_argv.as_ptr()) };
        // execv only returns on error.
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::NotFound {
            let ver = version_from_path(target_binary);
            eprintln!("error: mvl {ver} is not installed");
            eprintln!("  Run `mvl self install {ver}` to install it.");
        } else {
            eprintln!("error: execv failed: {err}");
        }
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        match std::process::Command::new(target_binary)
            .args(args.iter().skip(1))
            .status()
        {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let ver = version_from_path(target_binary);
                eprintln!("error: mvl {ver} is not installed");
                eprintln!("  Run `mvl self install {ver}` to install it.");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("error: cannot execute {}: {e}", target_binary.display());
                std::process::exit(1);
            }
            Ok(status) => {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract the toolchain version from a binary path (`toolchains/{ver}/bin/mvl`).
fn version_from_path(p: &Path) -> &str {
    p.parent() // bin/
        .and_then(|p| p.parent()) // {version}/
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
}

/// Walk up from `start` looking for `mvl.toml`.
fn find_mvl_toml(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("mvl.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Extract the raw `requires-mvl` value from a `mvl.toml` file by scanning
/// for it in the `[package]` section.  Returns `None` if the field is absent.
fn extract_requires_mvl_field(manifest: &Path) -> Option<String> {
    let content = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("requires-mvl") {
                let rest = rest.trim();
                if let Some(rest) = rest.strip_prefix('=') {
                    // Extract the value, respecting quoted strings and stripping
                    // trailing inline comments (`# ...`).
                    let value_raw = rest.trim();
                    let value = if let Some(inner) = value_raw.strip_prefix('"') {
                        // Double-quoted: take everything up to the closing quote.
                        inner.split_once('"').map(|(v, _)| v).unwrap_or(inner).to_owned()
                    } else if let Some(inner) = value_raw.strip_prefix('\'') {
                        // Single-quoted: take everything up to the closing quote.
                        inner.split_once('\'').map(|(v, _)| v).unwrap_or(inner).to_owned()
                    } else {
                        // Unquoted: take until whitespace or `#` comment.
                        value_raw
                            .split(|c: char| c.is_whitespace() || c == '#')
                            .next()
                            .unwrap_or("")
                            .to_owned()
                    };
                    if !value.is_empty() {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

/// Resolve a `requires-mvl` constraint to the best installed version.
///
/// Supports:
/// - Exact version: `"0.24.0"` → use that version
/// - Minimum constraint: `">=0.24.0"` → highest installed version ≥ that
/// - Minimum-only constraint: `">0.24.0"` → highest installed version > that
///
/// Returns `None` when no installed version satisfies the constraint.
fn resolve_constraint(raw: &str) -> Option<String> {
    let raw = raw.trim();

    if let Some(min) = raw.strip_prefix(">=") {
        let min = min.trim();
        let min_tuple = parse_semver(min);
        installed_versions()
            .into_iter()
            .filter(|(v, _)| parse_semver(v) >= min_tuple)
            .filter(|(v, _)| toolchain_bin(v).exists())
            .map(|(v, _)| v)
            .next_back()
    } else if let Some(min) = raw.strip_prefix('>') {
        let min = min.trim();
        let min_tuple = parse_semver(min);
        installed_versions()
            .into_iter()
            .filter(|(v, _)| parse_semver(v) > min_tuple)
            .filter(|(v, _)| toolchain_bin(v).exists())
            .map(|(v, _)| v)
            .next_back()
    } else if validate_version(raw) && toolchain_bin(raw).exists() {
        // Exact version (or unrecognised constraint — treat as exact).
        Some(raw.to_owned())
    } else {
        None
    }
}

/// Parse a semver string into `(major, minor, patch)` for comparison.
pub(super) fn parse_semver(v: &str) -> (u32, u32, u32) {
    let mut parts = v.splitn(3, '.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_version_from_argv0 ────────────────────────────────────────────

    #[test]
    fn argv0_bare_mvl_returns_none() {
        assert_eq!(extract_version_from_argv0("mvl"), None);
    }

    #[test]
    fn argv0_plain_name_with_version() {
        assert_eq!(
            extract_version_from_argv0("mvl@0.34.0"),
            Some("0.34.0".to_owned())
        );
    }

    #[test]
    fn argv0_absolute_path_with_version() {
        assert_eq!(
            extract_version_from_argv0("/home/user/.local/bin/mvl@0.19.0"),
            Some("0.19.0".to_owned())
        );
    }

    #[test]
    fn argv0_relative_path_with_version() {
        assert_eq!(
            extract_version_from_argv0("./bin/mvl@0.41.2"),
            Some("0.41.2".to_owned())
        );
    }

    #[test]
    fn argv0_invalid_version_returns_none() {
        assert_eq!(extract_version_from_argv0("mvl@notaversion"), None);
        assert_eq!(extract_version_from_argv0("mvl@1.2"), None);
        assert_eq!(extract_version_from_argv0("mvl@1.2.3.4"), None);
    }

    #[test]
    fn argv0_path_traversal_rejected() {
        assert_eq!(extract_version_from_argv0("mvl@../etc/passwd"), None);
    }

    // ── find_project_mvl_version ──────────────────────────────────────────────

    #[test]
    fn project_version_found_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".mvl-version"), "0.20.0\n").unwrap();
        assert_eq!(
            find_project_mvl_version(dir.path()),
            Some("0.20.0".to_owned())
        );
    }

    #[test]
    fn project_version_found_in_parent() {
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("sub").join("project");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(parent.path().join(".mvl-version"), "0.21.0").unwrap();
        assert_eq!(find_project_mvl_version(&child), Some("0.21.0".to_owned()));
    }

    #[test]
    fn project_version_not_found() {
        let dir = tempfile::tempdir().unwrap();
        // No .mvl-version in temp dir or any parent up to /tmp.
        // We can only assert no panic; result depends on filesystem state.
        let _ = find_project_mvl_version(dir.path());
    }

    // ── extract_requires_mvl_field ────────────────────────────────────────────

    #[test]
    fn manifest_exact_version_extracted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mvl.toml"),
            "[package]\nname = \"myapp\"\nrequires-mvl = \"0.24.0\"\n",
        )
        .unwrap();
        assert_eq!(
            extract_requires_mvl_field(&dir.path().join("mvl.toml")),
            Some("0.24.0".to_owned())
        );
    }

    #[test]
    fn manifest_constraint_extracted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mvl.toml"),
            "[package]\nname = \"myapp\"\nrequires-mvl = \">=0.24.0\"\n",
        )
        .unwrap();
        assert_eq!(
            extract_requires_mvl_field(&dir.path().join("mvl.toml")),
            Some(">=0.24.0".to_owned())
        );
    }

    #[test]
    fn manifest_no_requires_mvl_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("mvl.toml"), "[package]\nname = \"myapp\"\n").unwrap();
        assert_eq!(
            extract_requires_mvl_field(&dir.path().join("mvl.toml")),
            None
        );
    }

    #[test]
    fn manifest_field_outside_package_section_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mvl.toml"),
            "[other]\nrequires-mvl = \"0.24.0\"\n[package]\nname = \"myapp\"\n",
        )
        .unwrap();
        assert_eq!(
            extract_requires_mvl_field(&dir.path().join("mvl.toml")),
            None
        );
    }

    // ── resolve_constraint ────────────────────────────────────────────────────

    #[test]
    fn constraint_unrecognised_format_returns_none() {
        // Not installed, not a valid exact version.
        assert_eq!(resolve_constraint("^0.24.0"), None);
    }

    // ── resolve_version priority ──────────────────────────────────────────────

    #[test]
    fn argv0_wins_over_project_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".mvl-version"), "0.20.0").unwrap();
        // argv[0] version always wins regardless of installation state.
        let result = resolve_version("mvl@0.19.0", dir.path());
        assert_eq!(result, Some("0.19.0".to_owned()));
    }

    #[test]
    fn no_pin_anywhere_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        // No .mvl-version, no mvl.toml, no ~/.mvl-version.
        // Can't guarantee HOME has no .mvl-version, so just check no panic.
        let _ = resolve_version("mvl", dir.path());
    }

    // ── parse_semver ──────────────────────────────────────────────────────────

    #[test]
    fn semver_ordering() {
        assert!(parse_semver("0.20.0") > parse_semver("0.19.0"));
        assert!(parse_semver("1.0.0") > parse_semver("0.99.99"));
        assert_eq!(parse_semver("0.20.0"), parse_semver("0.20.0"));
    }
}
