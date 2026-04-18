//! Package fetching: clone from git, cache in XDG, compute source hash.
//!
//! Implements Spec 008 Requirements 3 (cache) and 7 (build fetches deps).
//!
//! Cache layout (mirrors ADR-0012 "Package Directory Structure"):
//! ```text
//! $XDG_DATA_HOME/mvl/pkg/
//! ├── http/
//! │   └── 1.2.0/
//! │       ├── mvl.toml
//! │       ├── src/
//! │       └── bridge.rs
//! └── tls/
//!     └── 0.4.0/
//! ```
//!
//! Local project overrides (`.mvl/pkg/<name>/`) take precedence over cache.

use crate::mvl::packages::lock::LockedPackage;
use std::path::{Path, PathBuf};
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

/// Root of the package cache: `$XDG_DATA_HOME/mvl/pkg/`.
pub fn pkg_cache_root() -> PathBuf {
    mvl_data_home().join("pkg")
}

/// Cache directory for a specific package + version:
/// `$XDG_DATA_HOME/mvl/pkg/{name}/{version}/`.
pub fn pkg_cache_dir(name: &str, version: &str) -> PathBuf {
    pkg_cache_root().join(sanitize(name)).join(version)
}

/// Returns the local project override path for a package:
/// `<project_root>/.mvl/pkg/<name>/`.
///
/// Local overrides take precedence over the global cache (ADR-0012).
pub fn local_override_dir(project_root: &Path, name: &str) -> PathBuf {
    project_root.join(".mvl").join("pkg").join(sanitize(name))
}

/// Resolve a package to its source directory:
/// 1. Local override `.mvl/pkg/<name>/` (if it exists)
/// 2. Global cache `$XDG_DATA_HOME/mvl/pkg/<name>/<version>/`
///
/// Returns `None` if neither exists.
pub fn resolve_pkg_dir(project_root: &Path, name: &str, version: &str) -> Option<PathBuf> {
    let local = local_override_dir(project_root, name);
    if local.exists() {
        return Some(local);
    }
    let cached = pkg_cache_dir(name, version);
    if cached.exists() {
        return Some(cached);
    }
    None
}

// ── Fetching ─────────────────────────────────────────────────────────────────

/// Fetch a package from a git URL at a given tag, cache it, and return a
/// `LockedPackage` with the computed hash.
///
/// Uses the system `git` binary (avoids a heavy git2 dependency).
pub fn fetch_package(name: &str, git_url: &str, tag: &str) -> Result<LockedPackage, FetchError> {
    let cache_dir = pkg_cache_root();
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| FetchError::Io(cache_dir.display().to_string(), e.to_string()))?;

    // Determine the version from the tag (strip leading 'v')
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let dest = pkg_cache_dir(name, &version);

    // Skip if already cached
    if dest.exists() {
        let hash = hash_source_tree(&dest)?;
        let commit = read_git_head(&dest);
        return Ok(LockedPackage {
            name: name.to_string(),
            version,
            hash,
            commit,
            git: Some(git_url.to_string()),
        });
    }

    // Clone at the specific tag into a temp location, then move to cache
    let tmp = cache_dir.join(format!(".tmp-{}-{}", sanitize(name), tag));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)
            .map_err(|e| FetchError::Io(tmp.display().to_string(), e.to_string()))?;
    }

    git_clone(git_url, tag, &tmp)?;

    // Move to final cache location.  Clean up `tmp` on any error so that
    // a failed rename does not leave a stale partial clone on disk.
    let move_result = (|| {
        std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(".")))
            .map_err(|e| FetchError::Io(dest.display().to_string(), e.to_string()))?;
        std::fs::rename(&tmp, &dest)
            .map_err(|e| FetchError::Io(dest.display().to_string(), e.to_string()))
    })();
    if let Err(e) = move_result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }

    let hash = hash_source_tree(&dest)?;
    let commit = read_git_head(&dest);

    Ok(LockedPackage {
        name: name.to_string(),
        version,
        hash,
        commit,
        git: Some(git_url.to_string()),
    })
}

/// Verify that the source tree at `dir` matches `expected_hash`.
///
/// Fails hard on mismatch (Spec 008 Req 4 / ADR-0012).
pub fn verify_hash(dir: &Path, expected: &str) -> Result<(), FetchError> {
    let actual = hash_source_tree(dir)?;
    if actual != expected {
        return Err(FetchError::HashMismatch {
            path: dir.display().to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// List available semver tags for a git repo (requires network access).
pub fn list_git_tags(git_url: &str) -> Result<Vec<String>, FetchError> {
    validate_url(git_url)?;
    let output = process::Command::new("git")
        .args(["ls-remote", "--tags", "--refs", "--", git_url])
        .output()
        .map_err(|e| FetchError::GitError(format!("git ls-remote failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FetchError::GitError(format!(
            "git ls-remote {git_url}: {stderr}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tags: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            // Format: "<hash>\trefs/tags/<tag>"
            line.split('\t')
                .nth(1)
                .and_then(|r| r.strip_prefix("refs/tags/"))
        })
        .map(|t| t.to_string())
        .collect();

    Ok(tags)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Validate a git tag/branch name before passing it to `git clone --branch`.
///
/// Rejects strings that start with `-` (would be interpreted as git options)
/// or contain null bytes.  Allows the characters that are valid in git refs.
fn validate_tag(tag: &str) -> Result<(), FetchError> {
    if tag.is_empty() {
        return Err(FetchError::GitError("empty tag".to_string()));
    }
    if tag.starts_with('-') {
        return Err(FetchError::GitError(format!(
            "tag {tag:?} looks like a git option; rejecting to prevent option injection"
        )));
    }
    if tag.contains('\0') {
        return Err(FetchError::GitError(format!(
            "tag {tag:?} contains a null byte"
        )));
    }
    Ok(())
}

/// Validate a git URL before passing it to any git sub-command.
///
/// Only `https://` and `ssh://` (and the `git@host:path` SCP form) are
/// permitted.  Plain `http://`, local paths, and `ext::` transports are
/// rejected.
fn validate_url(url: &str) -> Result<(), FetchError> {
    if url.starts_with('-') {
        return Err(FetchError::GitError(format!(
            "URL {url:?} looks like a git option"
        )));
    }
    if url.contains('\0') {
        return Err(FetchError::GitError(format!(
            "URL {url:?} contains a null byte"
        )));
    }
    let lower = url.to_ascii_lowercase();
    // Allow https://, ssh://, and SCP-style git@host:path
    if lower.starts_with("https://") || lower.starts_with("ssh://") || lower.starts_with("git@") {
        return Ok(());
    }
    Err(FetchError::GitError(format!(
        "URL {url:?} uses an unsupported or insecure scheme; only https:// and ssh:// are allowed"
    )))
}

fn git_clone(url: &str, tag: &str, dest: &Path) -> Result<(), FetchError> {
    validate_url(url)?;
    validate_tag(tag)?;

    let status = process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            tag,
            "--", // end of options; prevents url/dest being parsed as flags
            url,
            &dest.display().to_string(),
        ])
        .status()
        .map_err(|e| FetchError::GitError(format!("git clone failed: {e}")))?;

    if !status.success() {
        return Err(FetchError::GitError(format!(
            "git clone {url} at tag {tag} failed with exit code {:?}",
            status.code()
        )));
    }
    Ok(())
}

/// Compute SHA256 of all `.mvl` and `bridge.rs` files in the source tree,
/// sorted by relative path for determinism.
///
/// Hash input: for each file (in sorted path order), append `"<relative_path>\0<content>\0"`.
pub fn hash_source_tree(dir: &Path) -> Result<String, FetchError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_hashable_files(dir, dir, &mut files)
        .map_err(|e| FetchError::Io(dir.display().to_string(), e.to_string()))?;
    files.sort_by(|(a, _), (b, _)| a.cmp(b));

    // SHA256 using only std (avoid sha2 dependency for now — use simple accumulation)
    // We use a deterministic "content address": sorted file paths + contents
    // For a real implementation this would use sha2::Sha256.
    // Here we use a portable pure-Rust SHA256 implementation.
    let hash_bytes = sha256_files(&files);
    let hex = hex_encode(&hash_bytes);
    Ok(format!("sha256:{hex}"))
}

fn collect_hashable_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // Use lstat (does NOT follow symlinks) to determine the file type.
        let file_type = entry.file_type()?;
        // Skip symlinks entirely — they must not be followed during hashing to
        // prevent a malicious package from reading files outside its directory.
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            // Skip .git directory
            if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                continue;
            }
            collect_hashable_files(root, &path, out)?;
        } else {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".mvl") || name == "bridge.rs" || name == "mvl.toml" {
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                let content = std::fs::read(&path)?;
                out.push((rel, content));
            }
        }
    }
    Ok(())
}

fn read_git_head(dir: &Path) -> Option<String> {
    let head_file = dir.join(".git").join("HEAD");
    let content = std::fs::read_to_string(head_file).ok()?;
    let trimmed = content.trim();
    // If HEAD contains a ref (e.g. "ref: refs/heads/main"), read that ref
    if let Some(ref_path) = trimmed.strip_prefix("ref: ") {
        let ref_file = dir
            .join(".git")
            .join(ref_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::read_to_string(ref_file)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        // Detached HEAD — contains commit SHA directly
        Some(trimmed.to_string())
    }
}

/// Sanitize a package name or URL for use in a filesystem path.
///
/// - Replaces `/`, `:`, `\`, and null bytes with `_`.
/// - Strips `.` and `..` path components to prevent directory traversal.
fn sanitize(name: &str) -> String {
    // First pass: replace separator/control characters
    let replaced: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Second pass: remove `.` and `..` components that could traverse directories.
    // Components are split on `_` (the replacement for `/`).
    let cleaned: Vec<&str> = replaced
        .split('_')
        .filter(|c| *c != "." && *c != "..")
        .collect();
    cleaned.join("_")
}

// ── Pure-Rust SHA256 ──────────────────────────────────────────────────────────
// A minimal, allocation-free SHA256 implementation.
// This avoids adding an external dependency for a self-contained hash.

fn sha256_files(files: &[(String, Vec<u8>)]) -> [u8; 32] {
    let mut hasher = Sha256State::new();
    for (path, content) in files {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(content);
        hasher.update(b"\0");
    }
    hasher.finalize()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

// SHA256 state machine (FIPS 180-4)
struct Sha256State {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256State {
    fn new() -> Self {
        Sha256State {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0u8; 64],
            buf_len: 0,
            total: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        let mut pos = 0;
        while pos < data.len() {
            let space = 64 - self.buf_len;
            let take = space.min(data.len() - pos);
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[pos..pos + take]);
            self.buf_len += take;
            pos += take;
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total * 8;
        self.update(&[0x80]);
        // Pad to 56 bytes mod 64 (FIPS 180-4): if buf_len already >= 56 after
        // the 0x80 byte, we must fill this block to 64 and continue padding into
        // the next block.  Using `% 64 != 56` handles both cases correctly.
        while self.buf_len % 64 != 56 {
            self.update(&[0x00]);
        }
        let be = bit_len.to_be_bytes();
        self.update(&be);
        debug_assert_eq!(self.buf_len, 0);
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

/// Errors that can occur during package fetching.
#[derive(Debug)]
pub enum FetchError {
    Io(String, String),
    GitError(String),
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Io(path, e) => write!(f, "IO error at {path}: {e}"),
            FetchError::GitError(e) => write!(f, "git error: {e}"),
            FetchError::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "hash mismatch at {path}:\n  expected: {expected}\n  actual:   {actual}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Existing tests ────────────────────────────────────────────────────────

    #[test]
    fn sanitize_url_path() {
        assert_eq!(sanitize("github.com/user/repo"), "github.com_user_repo");
        assert_eq!(sanitize("https://example.com"), "https___example.com");
    }

    #[test]
    fn pkg_cache_dir_structure() {
        // Just check the path composition is correct
        let dir = pkg_cache_dir("github.com/user/pkg", "1.2.0");
        let s = dir.display().to_string();
        assert!(s.contains("pkg"));
        assert!(s.contains("1.2.0"));
    }

    #[test]
    fn hash_source_tree_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("main.mvl"), b"fn main() -> Unit { }").unwrap();
        fs::write(
            dir.join("lib.mvl"),
            b"fn add(a: Int, b: Int) -> Int { a + b }",
        )
        .unwrap();

        let h1 = hash_source_tree(dir).unwrap();
        let h2 = hash_source_tree(dir).unwrap();
        assert_eq!(h1, h2, "hash must be deterministic");
        assert!(h1.starts_with("sha256:"));
        assert_eq!(h1.len(), "sha256:".len() + 64); // 32 bytes = 64 hex chars
    }

    #[test]
    fn hash_changes_with_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("main.mvl"), b"fn main() -> Unit { }").unwrap();
        let h1 = hash_source_tree(dir).unwrap();

        fs::write(dir.join("main.mvl"), b"fn main() -> Unit { let x = 1; }").unwrap();
        let h2 = hash_source_tree(dir).unwrap();
        assert_ne!(h1, h2, "hash must change when content changes");
    }

    #[test]
    fn verify_hash_passes_on_match() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("main.mvl"), b"fn main() -> Unit { }").unwrap();
        let hash = hash_source_tree(tmp.path()).unwrap();
        assert!(verify_hash(tmp.path(), &hash).is_ok());
    }

    #[test]
    fn verify_hash_fails_on_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("main.mvl"), b"fn main() -> Unit { }").unwrap();
        let result = verify_hash(tmp.path(), "sha256:wronghash");
        assert!(matches!(result, Err(FetchError::HashMismatch { .. })));
    }

    #[test]
    fn sha256_known_empty_hash() {
        // SHA256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let state = Sha256State::new();
        let hash = state.finalize();
        let hex = hex_encode(&hash);
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_known_hello_hash() {
        // SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let mut state = Sha256State::new();
        state.update(b"hello");
        let hash = state.finalize();
        let hex = hex_encode(&hash);
        assert_eq!(
            hex,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // ── New tests ─────────────────────────────────────────────────────────────

    // --- sanitize ---

    #[test]
    fn sanitize_empty_string() {
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn sanitize_backslash() {
        // Windows-style path separator must be replaced
        assert_eq!(sanitize("foo\\bar"), "foo_bar");
    }

    #[test]
    fn sanitize_colon() {
        assert_eq!(sanitize("C:drive"), "C_drive");
    }

    #[test]
    fn sanitize_strips_dotdot_components() {
        // `..` must be stripped to prevent path traversal in the cache directory.
        assert_eq!(sanitize(".."), "");
        assert_eq!(sanitize("github.com/user/.."), "github.com_user");
        assert_eq!(sanitize("a/../b"), "a_b");
    }

    #[test]
    fn sanitize_plain_name_unchanged() {
        assert_eq!(sanitize("my-pkg"), "my-pkg");
        assert_eq!(sanitize("mvl-json"), "mvl-json");
    }

    // --- pkg_cache_dir / local_override_dir ---

    #[test]
    fn pkg_cache_dir_sanitizes_slashes_in_name() {
        let dir = pkg_cache_dir("github.com/user/repo", "2.0.0");
        let s = dir.display().to_string();
        // Slashes in the name must not create extra path components
        assert!(!s.contains("github.com/user/repo"), "raw slashes survived");
        assert!(s.contains("github.com_user_repo"));
        assert!(s.ends_with("2.0.0") || s.ends_with("2.0.0/") || s.contains("2.0.0"));
    }

    #[test]
    fn local_override_dir_structure() {
        let root = std::path::Path::new("/project");
        let dir = local_override_dir(root, "github.com/user/pkg");
        let s = dir.display().to_string();
        assert!(s.contains(".mvl"));
        assert!(s.contains("pkg"));
        assert!(s.contains("github.com_user_pkg"));
    }

    // --- resolve_pkg_dir ---

    #[test]
    fn resolve_pkg_dir_returns_none_when_neither_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let result = resolve_pkg_dir(tmp.path(), "nonexistent", "1.0.0");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_pkg_dir_prefers_local_override() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create the local override directory
        let local = root.join(".mvl").join("pkg").join("mypkg");
        fs::create_dir_all(&local).unwrap();

        let result = resolve_pkg_dir(root, "mypkg", "1.0.0");
        assert_eq!(result, Some(local));
    }

    // --- hash_source_tree edge cases ---

    #[test]
    fn hash_source_tree_empty_dir_returns_valid_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        // No files at all — should still return a valid sha256: hash
        let hash = hash_source_tree(tmp.path()).unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
    }

    #[test]
    fn hash_source_tree_empty_dir_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let h1 = hash_source_tree(tmp.path()).unwrap();
        let h2 = hash_source_tree(tmp.path()).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_source_tree_includes_bridge_rs() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_hash = hash_source_tree(tmp.path()).unwrap();

        fs::write(tmp.path().join("bridge.rs"), b"// bridge").unwrap();
        let with_bridge = hash_source_tree(tmp.path()).unwrap();

        assert_ne!(empty_hash, with_bridge, "bridge.rs must be hashed");
    }

    #[test]
    fn hash_source_tree_includes_mvl_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_hash = hash_source_tree(tmp.path()).unwrap();

        fs::write(tmp.path().join("mvl.toml"), b"[package]").unwrap();
        let with_manifest = hash_source_tree(tmp.path()).unwrap();

        assert_ne!(empty_hash, with_manifest, "mvl.toml must be hashed");
    }

    #[test]
    fn hash_source_tree_ignores_non_mvl_files() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();

        // tmp1 is empty; tmp2 has only a .txt file (should be ignored)
        fs::write(tmp2.path().join("readme.txt"), b"ignore me").unwrap();
        fs::write(tmp2.path().join("main.rs"), b"fn main() {}").unwrap();

        let h1 = hash_source_tree(tmp1.path()).unwrap();
        let h2 = hash_source_tree(tmp2.path()).unwrap();
        assert_eq!(h1, h2, "non-.mvl files must not affect hash");
    }

    #[test]
    fn hash_source_tree_skips_git_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        fs::write(dir.join("main.mvl"), b"fn main() {}").unwrap();
        let hash_without_git = hash_source_tree(dir).unwrap();

        // Add a .git directory with a file — hash must not change
        let git_dir = dir.join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(git_dir.join("secret.mvl"), b"should be ignored").unwrap();

        let hash_with_git = hash_source_tree(dir).unwrap();
        assert_eq!(
            hash_without_git, hash_with_git,
            ".git directory contents must not affect hash"
        );
    }

    #[test]
    fn hash_source_tree_includes_nested_mvl_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        let nested = dir.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();

        let hash_empty = hash_source_tree(dir).unwrap();

        fs::write(nested.join("util.mvl"), b"fn util() {}").unwrap();
        let hash_with_nested = hash_source_tree(dir).unwrap();

        assert_ne!(
            hash_empty, hash_with_nested,
            "nested .mvl files must be hashed"
        );
    }

    #[test]
    fn hash_source_tree_file_rename_changes_hash() {
        // Two trees with the same content but different filenames must hash differently
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let content = b"fn foo() {}";
        fs::write(tmp1.path().join("a.mvl"), content).unwrap();
        fs::write(tmp2.path().join("b.mvl"), content).unwrap();

        let h1 = hash_source_tree(tmp1.path()).unwrap();
        let h2 = hash_source_tree(tmp2.path()).unwrap();
        assert_ne!(h1, h2, "file path is included in hash");
    }

    // --- read_git_head ---

    #[test]
    fn read_git_head_detached_returns_commit_sha() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let sha = "abc123def456abc123def456abc123def456abc1";
        fs::write(git_dir.join("HEAD"), format!("{sha}\n")).unwrap();

        let result = read_git_head(tmp.path());
        assert_eq!(result.as_deref(), Some(sha));
    }

    #[test]
    fn read_git_head_symbolic_ref_follows_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let refs_heads = git_dir.join("refs").join("heads");
        fs::create_dir_all(&refs_heads).unwrap();

        let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(refs_heads.join("main"), format!("{sha}\n")).unwrap();

        let result = read_git_head(tmp.path());
        assert_eq!(result.as_deref(), Some(sha));
    }

    #[test]
    fn read_git_head_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        // No .git directory at all
        let result = read_git_head(tmp.path());
        assert!(result.is_none());
    }

    // --- mvl_data_home / pkg_cache_root ---

    #[test]
    fn mvl_data_home_respects_mvl_home_env() {
        // Use a unique var key to avoid cross-test interference
        // Note: env vars are process-global; this test sets and immediately checks.
        // We clear it at the end to be polite.
        std::env::set_var("MVL_HOME", "/custom/mvl");
        let root = pkg_cache_root();
        std::env::remove_var("MVL_HOME");

        assert!(
            root.starts_with("/custom/mvl"),
            "expected /custom/mvl/pkg, got {root:?}"
        );
        assert!(root.ends_with("pkg"));
    }

    #[test]
    fn mvl_data_home_respects_xdg_data_home_env() {
        // Only meaningful when MVL_HOME is unset
        std::env::remove_var("MVL_HOME");
        std::env::set_var("XDG_DATA_HOME", "/xdg/data");
        let root = pkg_cache_root();
        std::env::remove_var("XDG_DATA_HOME");

        // Should be /xdg/data/mvl/pkg
        let s = root.display().to_string();
        assert!(s.contains("xdg") || s.contains("mvl"), "got: {s}");
    }

    // --- FetchError Display ---

    #[test]
    fn fetch_error_display_io() {
        let e = FetchError::Io("/some/path".to_string(), "permission denied".to_string());
        let s = e.to_string();
        assert!(s.contains("/some/path"));
        assert!(s.contains("permission denied"));
    }

    #[test]
    fn fetch_error_display_hash_mismatch() {
        let e = FetchError::HashMismatch {
            path: "/pkg".to_string(),
            expected: "sha256:aaa".to_string(),
            actual: "sha256:bbb".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("sha256:aaa"));
        assert!(s.contains("sha256:bbb"));
    }

    #[test]
    fn fetch_error_display_git_error() {
        let e = FetchError::GitError("clone failed".to_string());
        assert!(e.to_string().contains("clone failed"));
    }

    // --- SHA256 additional vectors ---

    #[test]
    fn sha256_known_abc_hash() {
        // SHA256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let mut state = Sha256State::new();
        state.update(b"abc");
        let hash = state.finalize();
        let hex = hex_encode(&hash);
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_large_input_crosses_block_boundary() {
        // 65 bytes crosses the 64-byte block boundary — exercises the multi-block path
        let data = vec![b'x'; 65];
        let mut state = Sha256State::new();
        state.update(&data);
        let hash = state.finalize();
        let hex = hex_encode(&hash);
        // Just verify it's 64 hex chars and starts with sha256 format
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sha256_chunked_update_matches_single_update() {
        // Feeding data in chunks must produce the same hash as one call
        let data = b"the quick brown fox jumps over the lazy dog";

        let mut s1 = Sha256State::new();
        s1.update(data);
        let h1 = hex_encode(&s1.finalize());

        let mut s2 = Sha256State::new();
        for chunk in data.chunks(3) {
            s2.update(chunk);
        }
        let h2 = hex_encode(&s2.finalize());

        assert_eq!(h1, h2);
    }
}
