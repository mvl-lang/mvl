// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.io` stdlib functions.
//!
//! Provides real file I/O backing for the stubs declared in `std/io.mvl`.
//! These are re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::Tainted;

/// Filesystem path — mirrors the `Path` struct declared in `std/io.mvl`.
///
/// Construct with [`path`]. The path string is not accessible as a raw field;
/// use [`Path::as_str`] to read it if needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    // Private — all construction must go through `path()` to prevent callers
    // from stripping IFC labels (e.g. `Path { inner: tainted.0 }`).
    inner: String,
}

impl Path {
    /// Return the raw path string.
    pub fn as_str(&self) -> &str {
        &self.inner
    }
}

/// A file descriptor — stdout (1), stderr (2), stdin (0), or any open file.
///
/// Mirrors the `Fd` struct declared in `std/io.mvl`.
/// Obtain standard streams via the pure MVL functions `stdout()/stderr()/stdin()`.
/// Obtain file descriptors via [`open`]. Close with [`close`].
pub struct Fd {
    /// The raw Unix file descriptor number (0 = stdin, 1 = stdout, 2 = stderr, ≥3 = open file).
    pub inner: i64,
}

/// Returns the standard input file descriptor.
pub fn stdin() -> Fd {
    Fd { inner: 0 }
}

/// Returns the standard output file descriptor.
pub fn stdout() -> Fd {
    Fd { inner: 1 }
}

/// Returns the standard error file descriptor.
pub fn stderr() -> Fd {
    Fd { inner: 2 }
}

/// A single directory entry — mirrors `DirEntry` in `std/io.mvl`.
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Path to this entry.
    pub path: Path,
    /// True if this entry is a regular file.
    pub is_file: bool,
    /// True if this entry is a directory.
    pub is_dir: bool,
    /// True if this entry is a symbolic link.
    pub is_symlink: bool,
}

/// File or directory metadata — mirrors `Metadata` in `std/io.mvl`.
///
/// Does not follow symbolic links (`lstat` semantics).
#[derive(Debug, Clone)]
pub struct Metadata {
    /// File size in bytes.
    pub len: i64,
    /// True if this is a regular file.
    pub is_file: bool,
    /// True if this is a directory.
    pub is_dir: bool,
    /// True if this is a symbolic link.
    pub is_symlink: bool,
    /// Unix permission bits (e.g. 0o644). Zero on non-Unix platforms.
    pub permissions: i64,
}

// ── Path construction (pure) ───────────────────────────────────────────────

/// Construct a [`Path`] from a `String`.
///
/// Pure — no filesystem access.
pub fn path(s: String) -> Path {
    Path { inner: s }
}

/// Append a path segment to a base path, inserting the platform separator.
///
/// Pure — no filesystem access.
pub fn join(base: Path, segment: String) -> Path {
    let mut p = std::path::PathBuf::from(&base.inner);
    p.push(&segment);
    Path {
        inner: p.to_string_lossy().into_owned(),
    }
}

/// Return the string representation of a path.
///
/// Pure — no filesystem access.
pub fn to_string(p: Path) -> String {
    p.inner
}

// ── Path queries (! FileRead) ──────────────────────────────────────────────

/// Return true if the path exists on the filesystem.
/// Renamed from `exists` — `exists` is a reserved keyword in MVL (Phase 5, #628).
pub fn path_exists(p: Path) -> bool {
    std::fs::metadata(&p.inner).is_ok()
}

/// Return true if the path refers to a regular file.
pub fn is_file(p: Path) -> bool {
    std::fs::metadata(&p.inner)
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Return true if the path refers to a directory.
pub fn is_dir(p: Path) -> bool {
    std::fs::metadata(&p.inner)
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

// ── File descriptor functions ──────────────────────────────────────────────

/// Open a file for reading and writing, creating it if it does not exist.
///
/// Returns an [`Fd`] on success, or an `IoError` on failure.
/// The returned fd is a raw Unix file descriptor integer stored in `Fd.inner`.
pub fn open(p: Path) -> Result<Fd, IoError> {
    use std::os::unix::io::IntoRawFd as _;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&p.inner)
        .map(|f| Fd {
            inner: f.into_raw_fd() as i64,
        })
        .map_err(|e| sanitize_io_error(&e))
}

/// Close a file descriptor and release it.
///
/// Takes [`Fd`] by value to prevent use-after-close.
/// Only closes fds that were opened via [`open`] (fd > 2); stdin/stdout/stderr are never closed.
/// Out-of-range fd values are silently ignored (no panic).
#[allow(unsafe_code)]
pub fn close(fd: Fd) -> () {
    if fd.inner > 2 && fd.inner <= i32::MAX as i64 {
        // Reconstruct the File and let it drop, which closes the underlying fd.
        // SAFETY: fd.inner was obtained from IntoRawFd via open(), so it is valid and owned.
        let _ = unsafe {
            <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd.inner as i32)
        };
    }
}

/// Write a string to a file descriptor (stdout, stderr, or any open file).
///
/// Returns `Ok(())` on success, `Err(IoError)` on failure.
/// Returns `Err(IoError::PermissionDenied)` when `fd` is stdin (fd 0) — writing to
/// stdin is meaningless and is almost certainly a caller bug.
#[allow(unsafe_code)]
pub fn write(fd: Fd, msg: String) -> Result<(), IoError> {
    use std::io::Write as _;
    if fd.inner < 0 || fd.inner > i32::MAX as i64 {
        return Err(IoError::Other("fd out of range".into()));
    }
    if fd.inner == 0 {
        return Err(IoError::PermissionDenied);
    }
    // SAFETY: fd.inner is either a well-known fd (1-2) or was returned by open().
    let mut f =
        unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd.inner as i32) };
    let result = f
        .write_all(msg.as_bytes())
        .map_err(|e| sanitize_io_error(&e));
    // Prevent Rust from closing the fd when `f` is dropped — the fd is still owned by MVL.
    std::mem::forget(f);
    result
}

/// Read up to `max_bytes` bytes from a file descriptor.
///
/// Returns `Ok(Tainted<String>)` on success, `Ok(Tainted(""))` at EOF.
/// Returns `Err(IoError::PermissionDenied)` when `fd` is stdout or stderr (fds 1/2).
#[allow(unsafe_code)]
pub fn read(fd: Fd, max_bytes: i64) -> Result<Tainted<String>, IoError> {
    use std::io::Read as _;
    if fd.inner < 0 || fd.inner > i32::MAX as i64 {
        return Err(IoError::Other("fd out of range".into()));
    }
    if fd.inner == 1 || fd.inner == 2 {
        return Err(IoError::PermissionDenied);
    }
    // SAFETY: fd.inner is either stdin (0) or was returned by open().
    let mut f =
        unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd.inner as i32) };
    let mut buf = vec![0u8; max_bytes.max(0) as usize];
    let result = f
        .read(&mut buf)
        .map(|n| Tainted(String::from_utf8_lossy(&buf[..n]).into_owned()))
        .map_err(|e| sanitize_io_error(&e));
    std::mem::forget(f);
    result
}

/// Read one line (up to and including `\n`) from a file descriptor.
///
/// Returns `Ok(Tainted<String>)` on success, `Ok(Tainted(""))` at EOF.
/// Returns `Err(IoError::PermissionDenied)` when `fd` is stdout or stderr (fds 1/2).
#[allow(unsafe_code)]
pub fn read_line(fd: Fd) -> Result<Tainted<String>, IoError> {
    use std::io::{BufRead as _, BufReader};
    if fd.inner < 0 || fd.inner > i32::MAX as i64 {
        return Err(IoError::Other("fd out of range".into()));
    }
    if fd.inner == 1 || fd.inner == 2 {
        return Err(IoError::PermissionDenied);
    }
    // SAFETY: fd.inner is either stdin (0) or was returned by open().
    let f =
        unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd.inner as i32) };
    let mut reader = BufReader::new(f);
    let mut line = String::new();
    let result = reader
        .read_line(&mut line)
        .map(|_| Tainted(line))
        .map_err(|e| sanitize_io_error(&e));
    // Prevent Rust from closing the fd when the BufReader (and inner File) is dropped.
    let inner = reader.into_inner();
    std::mem::forget(inner);
    result
}

// ── File I/O ───────────────────────────────────────────────────────────────

/// Read the entire contents of a file into a string.
///
/// Returns `Ok(Tainted<String>)` on success.
/// Returns `Err(IoError)` on failure.
pub fn read_to_string(p: Path) -> Result<Tainted<String>, IoError> {
    std::fs::read_to_string(p.as_str())
        .map(Tainted)
        .map_err(|e| sanitize_io_error(&e))
}

/// Raw private builtin: read a file, return bare `String` (#894 Pattern 002).
///
/// Module-private in MVL (`builtin fn _read_file`) — callers use `read_file`.
pub fn _read_file(p: String) -> Result<String, IoError> {
    std::fs::read_to_string(&p).map_err(|e| sanitize_io_error(&e))
}

/// Read the entire contents of a file given a path string.
///
/// Returns `Tainted[String]` — file contents are external (untrusted) data.
pub fn read_file(p: String) -> Result<Tainted<String>, IoError> {
    _read_file(p).map(Tainted)
}

/// Write a string to a file path, truncating it if it already exists.
///
/// For writing to an open file descriptor, use [`write`] instead.
pub fn write_file(p: Path, content: String) -> Result<(), IoError> {
    std::fs::write(&p.inner, content.as_bytes()).map_err(|e| sanitize_io_error(&e))
}

/// Append a string to a file, creating it if it does not exist.
pub fn append(p: Path, content: String) -> Result<(), IoError> {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.inner)
        .and_then(|mut f| f.write_all(content.as_bytes()))
        .map_err(|e| sanitize_io_error(&e))
}

// ── Filesystem operations ──────────────────────────────────────────────────

/// Create a directory and all its missing parents.
///
/// No-op if the directory already exists.
pub fn create_dir_all(p: Path) -> Result<(), IoError> {
    std::fs::create_dir_all(&p.inner).map_err(|e| sanitize_io_error(&e))
}

/// Remove a file or empty directory.
pub fn remove(p: Path) -> Result<(), IoError> {
    let is_dir = std::fs::symlink_metadata(&p.inner)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    if is_dir {
        std::fs::remove_dir(&p.inner)
    } else {
        std::fs::remove_file(&p.inner)
    }
    .map_err(|e| sanitize_io_error(&e))
}

/// List the entries in a directory.
pub fn read_dir(p: Path) -> Result<Vec<DirEntry>, IoError> {
    let entries = std::fs::read_dir(&p.inner).map_err(|e| sanitize_io_error(&e))?;
    entries
        .map(|entry| {
            let entry = entry.map_err(|e| sanitize_io_error(&e))?;
            let meta = entry.metadata().map_err(|e| sanitize_io_error(&e))?;
            Ok(DirEntry {
                path: Path {
                    inner: entry.path().to_string_lossy().into_owned(),
                },
                is_file: meta.is_file(),
                is_dir: meta.is_dir(),
                is_symlink: meta.file_type().is_symlink(),
            })
        })
        .collect()
}

/// Return metadata for a path without following symbolic links (`lstat` semantics).
pub fn metadata(p: Path) -> Result<Metadata, IoError> {
    let m = std::fs::symlink_metadata(&p.inner).map_err(|e| sanitize_io_error(&e))?;
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt as _;
        m.permissions().mode() as i64
    };
    #[cfg(not(unix))]
    let permissions = 0i64;
    Ok(Metadata {
        len: m.len() as i64,
        is_file: m.is_file(),
        is_dir: m.is_dir(),
        is_symlink: m.file_type().is_symlink(),
        permissions,
    })
}

/// Set the Unix permission bits of a file or directory.
///
/// No-op (returns `Ok(())`) on non-Unix platforms.
pub fn chmod(p: Path, mode: i64) -> Result<(), IoError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let perms = std::fs::Permissions::from_mode(mode as u32);
        std::fs::set_permissions(&p.inner, perms).map_err(|e| sanitize_io_error(&e))
    }
    #[cfg(not(unix))]
    {
        let _ = (p, mode);
        Ok(())
    }
}

/// Create a symbolic link: `link` will point to `target`.
///
/// Returns an error on non-Unix platforms.
pub fn create_symlink(target: Path, link: Path) -> Result<(), IoError> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target.inner, &link.inner).map_err(|e| sanitize_io_error(&e))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(IoError::Other(
            "symlinks not supported on this platform".to_string(),
        ))
    }
}

/// Read the target path of a symbolic link.
///
/// Returns `Tainted<String>` — symlink targets are external data.
pub fn read_link(p: Path) -> Result<Tainted<String>, IoError> {
    std::fs::read_link(&p.inner)
        .map(|pb| Tainted(pb.to_string_lossy().into_owned()))
        .map_err(|e| sanitize_io_error(&e))
}

// ── Temporary files and directories (#1042) ──────────────────────────────

/// A temporary file — mirrors `TempFile` in `std/io.mvl`.
///
/// Linear type: must be consumed via [`delete_temp`] or [`persist`].
pub struct TempFile {
    pub fd: Fd,
    pub path: Path,
}

/// A temporary directory — mirrors `TempDir` in `std/io.mvl`.
///
/// Linear type: must be consumed via [`delete_temp_dir`].
pub struct TempDir {
    pub path: Path,
}

/// Global counter for generating unique temp file names.
static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Generate a unique temp path with the given prefix directory.
fn make_temp_path(dir: &str, prefix: &str) -> String {
    let pid = std::process::id();
    let seq = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut p = std::path::PathBuf::from(dir);
    p.push(format!("{prefix}_{pid}_{seq}"));
    p.to_string_lossy().into_owned()
}

/// Create a temporary file in the system temp directory.
pub fn temp_file() -> Result<TempFile, IoError> {
    let dir = std::env::temp_dir().to_string_lossy().into_owned();
    temp_file_in(Path { inner: dir })
}

/// Create a temporary file in a specific directory.
pub fn temp_file_in(dir: Path) -> Result<TempFile, IoError> {
    let tmp_path = make_temp_path(&dir.inner, "mvl_tmp");
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map(|f| {
            use std::os::unix::io::IntoRawFd as _;
            TempFile {
                fd: Fd {
                    inner: f.into_raw_fd() as i64,
                },
                path: Path { inner: tmp_path },
            }
        })
        .map_err(|e| sanitize_io_error(&e))
}

/// Create a temporary directory in the system temp directory.
pub fn temp_dir() -> Result<TempDir, IoError> {
    let parent = std::env::temp_dir().to_string_lossy().into_owned();
    let tmp_path = make_temp_path(&parent, "mvl_tmpd");
    std::fs::create_dir(&tmp_path)
        .map(|()| TempDir {
            path: Path { inner: tmp_path },
        })
        .map_err(|e| sanitize_io_error(&e))
}

/// Write data to an open temporary file.
#[allow(unsafe_code)]
pub fn temp_write(tf: &TempFile, data: String) -> Result<(), IoError> {
    use std::io::Write as _;
    if tf.fd.inner < 0 || tf.fd.inner > i32::MAX as i64 {
        return Err(IoError::Other("fd out of range".into()));
    }
    let mut f =
        unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(tf.fd.inner as i32) };
    let result = f
        .write_all(data.as_bytes())
        .map_err(|e| sanitize_io_error(&e));
    std::mem::forget(f);
    result
}

/// Read the full contents of a temporary file.
#[allow(unsafe_code)]
pub fn temp_read(tf: &TempFile) -> Result<Tainted<String>, IoError> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    if tf.fd.inner < 0 || tf.fd.inner > i32::MAX as i64 {
        return Err(IoError::Other("fd out of range".into()));
    }
    let mut f =
        unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(tf.fd.inner as i32) };
    let result = f
        .seek(SeekFrom::Start(0))
        .map_err(|e| sanitize_io_error(&e));
    if let Err(e) = result {
        std::mem::forget(f);
        return Err(e);
    }
    let mut buf = String::new();
    let result = f
        .read_to_string(&mut buf)
        .map(|_| Tainted(buf))
        .map_err(|e| sanitize_io_error(&e));
    std::mem::forget(f);
    result
}

/// Delete a temporary file — closes fd and removes from disk.
#[allow(unsafe_code)]
pub fn delete_temp(tf: TempFile) -> Result<(), IoError> {
    // Close the fd first
    if tf.fd.inner > 2 && tf.fd.inner <= i32::MAX as i64 {
        let _ = unsafe {
            <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(tf.fd.inner as i32)
        };
    }
    std::fs::remove_file(&tf.path.inner).map_err(|e| sanitize_io_error(&e))
}

/// Delete a temporary directory (recursively).
pub fn delete_temp_dir(td: TempDir) -> Result<(), IoError> {
    std::fs::remove_dir_all(&td.path.inner).map_err(|e| sanitize_io_error(&e))
}

/// Move a temporary file to a permanent location.
///
/// Tries rename first (atomic on same filesystem), falls back to copy+delete.
#[allow(unsafe_code)]
pub fn persist(tf: TempFile, dest: Path) -> Result<(), IoError> {
    // Close the fd first
    if tf.fd.inner > 2 && tf.fd.inner <= i32::MAX as i64 {
        let _ = unsafe {
            <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(tf.fd.inner as i32)
        };
    }
    // Try atomic rename
    match std::fs::rename(&tf.path.inner, &dest.inner) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Cross-filesystem: copy then delete
            std::fs::copy(&tf.path.inner, &dest.inner).map_err(|e| sanitize_io_error(&e))?;
            std::fs::remove_file(&tf.path.inner).map_err(|e| sanitize_io_error(&e))
        }
    }
}

// ── Error type ────────────────────────────────────────────────────────────

/// Mirrors the `IoError` enum declared in `std/io.mvl`.
/// Variant order and names must stay in sync with the MVL definition.
#[derive(Debug, Clone, PartialEq)]
pub enum IoError {
    /// The file or directory was not found.
    NotFound,
    /// The operation was denied due to insufficient permissions.
    PermissionDenied,
    /// The file or directory already exists.
    AlreadyExists,
    /// An unclassified I/O error with a description.
    Other(String),
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn sanitize_io_error(e: &std::io::Error) -> IoError {
    match e.kind() {
        std::io::ErrorKind::NotFound => IoError::NotFound,
        std::io::ErrorKind::PermissionDenied => IoError::PermissionDenied,
        std::io::ErrorKind::AlreadyExists => IoError::AlreadyExists,
        _ => IoError::Other(e.kind().to_string()),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    // ── Path construction ──────────────────────────────────────────────────

    #[test]
    fn path_stores_inner_string() {
        let p = path("foo/bar.txt".to_string());
        assert_eq!(p.as_str(), "foo/bar.txt");
    }

    #[test]
    fn join_appends_segment() {
        let p = join(path("/etc/app".to_string()), "config.toml".to_string());
        assert_eq!(p.as_str(), "/etc/app/config.toml");
    }

    #[test]
    fn to_string_returns_inner() {
        let p = path("/tmp/test".to_string());
        assert_eq!(to_string(p), "/tmp/test");
    }

    // ── read_to_string / read_file ─────────────────────────────────────────

    #[test]
    fn read_to_string_missing_file_returns_err() {
        let p = path("/tmp/mvl_nonexistent_file_xyz_12345".to_string());
        assert!(read_to_string(p).is_err());
    }

    #[test]
    fn read_to_string_real_file() {
        let path_str = tmp("mvl_test_read_to_string.txt");
        std::fs::write(&path_str, "hello mvl").unwrap();
        let p = path(path_str.clone());
        let result = read_to_string(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "hello mvl");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_file_missing_file_returns_err() {
        let p = "/tmp/mvl_nonexistent_file_xyz_12345".to_string();
        assert!(read_file(p).is_err());
    }

    #[test]
    fn read_file_real_file() {
        let path_str = tmp("mvl_test_read_file.txt");
        std::fs::write(&path_str, "world mvl").unwrap();
        let p = path_str.clone();
        let result = read_file(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "world mvl");
        std::fs::remove_file(&path_str).ok();
    }

    // ── Path queries ───────────────────────────────────────────────────────

    #[test]
    fn exists_returns_true_for_existing_file() {
        let path_str = tmp("mvl_test_exists.txt");
        std::fs::write(&path_str, "").unwrap();
        assert!(path_exists(path(path_str.clone())));
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn exists_returns_false_for_missing_file() {
        assert!(!path_exists(path("/tmp/mvl_no_such_file_xyz".to_string())));
    }

    #[test]
    fn is_file_and_is_dir() {
        let path_str = tmp("mvl_test_is_file.txt");
        std::fs::write(&path_str, "").unwrap();
        assert!(is_file(path(path_str.clone())));
        assert!(!is_dir(path(path_str.clone())));
        std::fs::remove_file(&path_str).ok();
    }

    // ── write_file / append / read roundtrip ──────────────────────────────

    #[test]
    fn write_file_creates_file_with_content() {
        let path_str = tmp("mvl_test_write_file.txt");
        let p = path(path_str.clone());
        write_file(p.clone(), "hello".to_string()).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "hello");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn write_file_truncates_existing_file() {
        let path_str = tmp("mvl_test_write_file_trunc.txt");
        std::fs::write(&path_str, "old content with more bytes").unwrap();
        let p = path(path_str.clone());
        write_file(p, "new".to_string()).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "new");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn append_adds_to_existing_content() {
        let path_str = tmp("mvl_test_append.txt");
        std::fs::write(&path_str, "line1\n").unwrap();
        let p = path(path_str.clone());
        append(p, "line2\n".to_string()).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "line1\nline2\n");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn write_file_read_roundtrip() {
        let path_str = tmp("mvl_test_roundtrip.txt");
        let p = path(path_str.clone());
        write_file(p.clone(), "roundtrip content".to_string()).unwrap();
        let result = read_to_string(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "roundtrip content");
        std::fs::remove_file(&path_str).ok();
    }

    // ── open / close / fd write / read_line ───────────────────────────────

    #[test]
    fn open_missing_dir_returns_err() {
        let p_dir = path("/tmp/mvl_no_such_dir_xyz/file.txt".to_string());
        assert!(open(p_dir).is_err());
    }

    #[test]
    fn fd_write_and_read_line_roundtrip() {
        let path_str = tmp("mvl_test_fd_write.txt");
        std::fs::write(&path_str, "").unwrap();
        let fd = open(path(path_str.clone())).unwrap();
        write(Fd { inner: fd.inner }, "first line\n".to_string()).unwrap();
        close(fd);
        // Re-open for reading
        let fd2 = open(path(path_str.clone())).unwrap();
        let line = read_line(fd2).unwrap();
        assert_eq!(line.0, "first line\n");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_line_eof_returns_empty_string() {
        let path_str = tmp("mvl_test_read_line_eof.txt");
        std::fs::write(&path_str, "").unwrap();
        let fd = open(path(path_str.clone())).unwrap();
        let line = read_line(fd).unwrap();
        assert_eq!(line.0, "", "EOF should return empty string");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn write_to_stdout_fd_succeeds() {
        // write(stdout(), msg) must succeed — verifies unified Fd works for well-known fds.
        let result = write(stdout(), "".to_string());
        assert!(result.is_ok(), "write to stdout fd should succeed");
    }

    #[test]
    fn write_to_stderr_fd_succeeds() {
        let result = write(stderr(), "".to_string());
        assert!(result.is_ok(), "write to stderr fd should succeed");
    }

    #[test]
    fn write_to_stdin_fd_is_rejected() {
        let result = write(stdin(), "hello".to_string());
        assert!(result.is_err(), "write to stdin fd should be rejected");
        assert!(matches!(result.unwrap_err(), IoError::PermissionDenied));
    }

    #[test]
    fn read_partial_bytes_from_file() {
        let path_str = tmp("mvl_test_read_partial.txt");
        std::fs::write(&path_str, "abcdefgh").unwrap();
        let fd = open(path(path_str.clone())).unwrap();
        let result = read(fd, 4).unwrap();
        assert_eq!(result.0, "abcd", "read(fd, 4) should return first 4 bytes");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_zero_bytes_returns_empty() {
        let path_str = tmp("mvl_test_read_zero.txt");
        std::fs::write(&path_str, "content").unwrap();
        let fd = open(path(path_str.clone())).unwrap();
        let result = read(fd, 0).unwrap();
        assert_eq!(result.0, "", "read(fd, 0) should return empty string");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_more_than_file_size_returns_all() {
        let path_str = tmp("mvl_test_read_over.txt");
        std::fs::write(&path_str, "hi").unwrap();
        let fd = open(path(path_str.clone())).unwrap();
        let result = read(fd, 1024).unwrap();
        assert_eq!(result.0, "hi", "read(fd, n>size) should return all content");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_from_stdout_fd_is_rejected() {
        let result = read(stdout(), 10);
        assert!(result.is_err(), "read from stdout fd should be rejected");
        assert!(matches!(result.unwrap_err(), IoError::PermissionDenied));
    }

    #[test]
    fn close_well_known_fds_is_noop() {
        // Closing stdin/stdout/stderr must not close the actual fds.
        close(stdin());
        close(stdout());
        close(stderr());
        // If fds were accidentally closed, this write would fail.
        assert!(write(stdout(), "".to_string()).is_ok());
    }

    #[test]
    fn write_out_of_range_fd_returns_err() {
        let result = write(Fd { inner: -1 }, "x".to_string());
        assert!(result.is_err());
        let result2 = write(
            Fd {
                inner: i32::MAX as i64 + 1,
            },
            "x".to_string(),
        );
        assert!(result2.is_err());
    }

    // ── Filesystem operations ──────────────────────────────────────────────

    #[test]
    fn create_dir_all_and_remove() {
        let dir_str = tmp("mvl_test_dir_xyz/nested");
        let dir_p = path(dir_str.clone());
        create_dir_all(dir_p).unwrap();
        assert!(std::path::Path::new(&dir_str).is_dir());
        // Remove nested first, then parent
        remove(path(dir_str.clone())).unwrap();
        remove(path(tmp("mvl_test_dir_xyz"))).unwrap();
    }

    #[test]
    fn remove_file_works() {
        let path_str = tmp("mvl_test_remove.txt");
        std::fs::write(&path_str, "").unwrap();
        let p = path(path_str.clone());
        remove(p).unwrap();
        assert!(!std::path::Path::new(&path_str).exists());
    }

    #[test]
    fn remove_missing_file_returns_err() {
        let p = path("/tmp/mvl_no_such_file_to_remove_xyz".to_string());
        assert!(remove(p).is_err());
    }

    #[test]
    fn read_dir_lists_entries() {
        let dir_str = tmp("mvl_test_read_dir");
        std::fs::create_dir_all(&dir_str).unwrap();
        std::fs::write(format!("{dir_str}/a.txt"), "").unwrap();
        std::fs::write(format!("{dir_str}/b.txt"), "").unwrap();
        let entries = read_dir(path(dir_str.clone())).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.is_file));
        std::fs::remove_dir_all(&dir_str).ok();
    }

    #[test]
    fn metadata_returns_file_info() {
        let path_str = tmp("mvl_test_metadata.txt");
        std::fs::write(&path_str, "hello").unwrap();
        let m = metadata(path(path_str.clone())).unwrap();
        assert_eq!(m.len, 5);
        assert!(m.is_file);
        assert!(!m.is_dir);
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn chmod_does_not_error_on_valid_path() {
        let path_str = tmp("mvl_test_chmod.txt");
        std::fs::write(&path_str, "").unwrap();
        let result = chmod(path(path_str.clone()), 0o644);
        assert!(result.is_ok());
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn create_symlink_and_read_link() {
        let target_str = tmp("mvl_test_symlink_target.txt");
        let link_str = tmp("mvl_test_symlink_link.txt");
        std::fs::write(&target_str, "target content").unwrap();
        std::fs::remove_file(&link_str).ok();
        let result = create_symlink(path(target_str.clone()), path(link_str.clone()));
        assert!(result.is_ok());
        let link_target = read_link(path(link_str.clone())).unwrap();
        assert!(link_target.0.ends_with("mvl_test_symlink_target.txt"));
        std::fs::remove_file(&link_str).ok();
        std::fs::remove_file(&target_str).ok();
    }

    // ── TempFile / TempDir (#1042) ────────────────────────────────────────

    #[test]
    fn temp_file_creates_and_deletes() {
        let tf = temp_file().unwrap();
        assert!(std::path::Path::new(tf.path.as_str()).exists());
        let path_copy = tf.path.inner.clone();
        delete_temp(tf).unwrap();
        assert!(!std::path::Path::new(&path_copy).exists());
    }

    #[test]
    fn temp_file_write_read_roundtrip() {
        let tf = temp_file().unwrap();
        temp_write(&tf, "hello temp".to_string()).unwrap();
        let data = temp_read(&tf).unwrap();
        assert_eq!(data.0, "hello temp");
        delete_temp(tf).unwrap();
    }

    #[test]
    fn temp_file_in_creates_in_specified_dir() {
        let dir_str = tmp("mvl_test_temp_in_dir");
        std::fs::create_dir_all(&dir_str).unwrap();
        let tf = temp_file_in(path(dir_str.clone())).unwrap();
        assert!(tf.path.as_str().starts_with(&dir_str));
        delete_temp(tf).unwrap();
        std::fs::remove_dir_all(&dir_str).ok();
    }

    #[test]
    fn temp_dir_creates_and_deletes() {
        let td = temp_dir().unwrap();
        assert!(std::path::Path::new(td.path.as_str()).is_dir());
        let path_copy = td.path.inner.clone();
        delete_temp_dir(td).unwrap();
        assert!(!std::path::Path::new(&path_copy).exists());
    }

    #[test]
    fn persist_moves_temp_to_permanent() {
        let tf = temp_file().unwrap();
        temp_write(&tf, "persist me".to_string()).unwrap();
        let temp_path_str = tf.path.inner.clone();
        let dest_str = tmp("mvl_test_persist_dest.txt");
        persist(tf, path(dest_str.clone())).unwrap();
        assert!(!std::path::Path::new(&temp_path_str).exists());
        let content = std::fs::read_to_string(&dest_str).unwrap();
        assert_eq!(content, "persist me");
        std::fs::remove_file(&dest_str).ok();
    }
}
