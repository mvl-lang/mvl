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

/// An open file handle — mirrors the `File` struct declared in `std/io.mvl`.
///
/// Returned by [`open`]. Pass to [`buf_reader`] or [`buf_writer`] for
/// buffered I/O. Dropped automatically when it goes out of scope (Rust Drop).
pub struct File {
    inner: std::fs::File,
}

/// A buffered reader wrapping a [`File`] handle.
///
/// Single-use in Phase 2 (move semantics). Pass to [`read_line`] to read one
/// line; after the call the reader is consumed. Phase 3 will add loop-friendly
/// iteration via borrow inference.
pub struct BufReader {
    inner: std::io::BufReader<std::fs::File>,
}

/// A buffered writer wrapping a [`File`] handle.
///
/// Single-use in Phase 2 (move semantics). Pass to [`write_line`] to write one
/// line; after the call the writer is consumed and the buffer is flushed.
pub struct BufWriter {
    inner: std::io::BufWriter<std::fs::File>,
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

/// Standard input handle — mirrors `Stdin` in `std/io.mvl`.
///
/// Obtained via [`stdin`]. Single-use in Phase 2 (move semantics).
pub struct Stdin {
    inner: std::io::Stdin,
}

/// Standard output handle — mirrors `Stdout` in `std/io.mvl`.
///
/// Obtained via [`stdout`]. Pass to [`stdout_write`] to write to stdout.
pub struct Stdout {
    inner: std::io::Stdout,
}

/// Standard error handle — mirrors `Stderr` in `std/io.mvl`.
///
/// Obtained via [`stderr`]. Pass to [`stderr_write`] to write to stderr.
pub struct Stderr {
    inner: std::io::Stderr,
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

// ── File handle functions ──────────────────────────────────────────────────

/// Open a file for reading and writing, creating it if it does not exist.
///
/// Returns a [`File`] handle on success, or an `IoError` on failure.
pub fn open(p: Path) -> Result<File, IoError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&p.inner)
        .map(|f| File { inner: f })
        .map_err(|e| sanitize_io_error(&e))
}

/// Close a file handle and release the file descriptor.
///
/// Takes [`File`] by value; the file descriptor is released when `f` is dropped.
pub fn close(_f: File) -> () {
    // Drop is called on `_f` here, which closes the underlying std::fs::File.
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
pub(crate) fn _read_file(p: String) -> Result<String, IoError> {
    std::fs::read_to_string(&p).map_err(|e| sanitize_io_error(&e))
}

/// Read the entire contents of a file given a path string.
///
/// Returns `Tainted[String]` — file contents are external (untrusted) data.
pub fn read_file(p: String) -> Result<Tainted<String>, IoError> {
    _read_file(p).map(Tainted)
}

/// Write a string to a file, truncating it if it already exists.
pub fn write(p: Path, content: Tainted<String>) -> Result<(), IoError> {
    std::fs::write(&p.inner, content.0.as_bytes()).map_err(|e| sanitize_io_error(&e))
}

/// Append a string to a file, creating it if it does not exist.
pub fn append(p: Path, content: Tainted<String>) -> Result<(), IoError> {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.inner)
        .and_then(|mut f| f.write_all(content.0.as_bytes()))
        .map_err(|e| sanitize_io_error(&e))
}

/// Wrap a [`File`] handle in a [`BufReader`] for line-oriented reading.
pub fn buf_reader(f: File) -> BufReader {
    BufReader {
        inner: std::io::BufReader::new(f.inner),
    }
}

/// Wrap a [`File`] handle in a [`BufWriter`] for line-oriented writing.
pub fn buf_writer(f: File) -> BufWriter {
    BufWriter {
        inner: std::io::BufWriter::new(f.inner),
    }
}

/// Read the next line from a [`BufReader`] (up to and including `\n`).
///
/// Returns `Ok(Tainted<String>)` on success.
/// Returns `Ok(Tainted(""))` at end-of-file.
/// Returns `Err(IoError)` on I/O failure.
///
/// Single-use in Phase 2 — the reader is consumed after this call.
pub fn read_line(r: BufReader) -> Result<Tainted<String>, IoError> {
    use std::io::BufRead as _;
    let mut inner = r.inner;
    let mut line = String::new();
    match inner.read_line(&mut line) {
        Ok(_) => Ok(Tainted(line)),
        Err(e) => Err(sanitize_io_error(&e)),
    }
}

/// Write a line (followed by `\n`) to a [`BufWriter`].
///
/// Flushes the buffer before returning.
/// Single-use in Phase 2 — the writer is consumed after this call.
pub fn write_line(w: BufWriter, line: String) -> Result<(), IoError> {
    use std::io::Write as _;
    let mut inner = w.inner;
    writeln!(inner, "{}", line)
        .and_then(|_| inner.flush())
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

// ── Standard input (! Console) ─────────────────────────────────────────────

/// Return the standard input handle.
///
/// Single-use in Phase 2 — pass to [`stdin_read_line`] or [`stdin_read_to_string`].
pub fn stdin() -> Stdin {
    Stdin {
        inner: std::io::stdin(),
    }
}

/// Read one line from stdin (up to and including `\n`).
///
/// Returns `Ok(Tainted<String>)` on success.
/// Returns `Ok(Tainted(""))` at end-of-file.
pub fn stdin_read_line(s: Stdin) -> Result<Tainted<String>, IoError> {
    use std::io::BufRead as _;
    let mut line = String::new();
    match s.inner.lock().read_line(&mut line) {
        Ok(_) => Ok(Tainted(line)),
        Err(e) => Err(sanitize_io_error(&e)),
    }
}

/// Read all of stdin into a string.
///
/// Returns `Ok(Tainted<String>)` on success.
pub fn stdin_read_to_string(s: Stdin) -> Result<Tainted<String>, IoError> {
    use std::io::Read as _;
    let mut buf = String::new();
    s.inner
        .lock()
        .read_to_string(&mut buf)
        .map(|_| Tainted(buf))
        .map_err(|e| sanitize_io_error(&e))
}

// ── Standard output / error (! Console) ───────────────────────────────────

/// Return the standard output handle.
pub fn stdout() -> Stdout {
    Stdout {
        inner: std::io::stdout(),
    }
}

/// Return the standard error handle.
pub fn stderr() -> Stderr {
    Stderr {
        inner: std::io::stderr(),
    }
}

/// Write a string to stdout without a trailing newline.
pub fn stdout_write(s: Stdout, line: String) {
    use std::io::Write as _;
    let _ = s.inner.lock().write_all(line.as_bytes());
}

/// Write a string to stderr without a trailing newline.
pub fn stderr_write(s: Stderr, line: String) {
    use std::io::Write as _;
    let _ = s.inner.lock().write_all(line.as_bytes());
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

    // ── write / append / read roundtrip ───────────────────────────────────

    #[test]
    fn write_creates_file_with_content() {
        let path_str = tmp("mvl_test_write.txt");
        let p = path(path_str.clone());
        write(p.clone(), Tainted("hello".to_string())).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "hello");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn write_truncates_existing_file() {
        let path_str = tmp("mvl_test_write_trunc.txt");
        std::fs::write(&path_str, "old content with more bytes").unwrap();
        let p = path(path_str.clone());
        write(p, Tainted("new".to_string())).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "new");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn append_adds_to_existing_content() {
        let path_str = tmp("mvl_test_append.txt");
        std::fs::write(&path_str, "line1\n").unwrap();
        let p = path(path_str.clone());
        append(p, Tainted("line2\n".to_string())).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "line1\nline2\n");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn write_read_roundtrip() {
        let path_str = tmp("mvl_test_roundtrip.txt");
        let p = path(path_str.clone());
        write(p.clone(), Tainted("roundtrip content".to_string())).unwrap();
        let result = read_to_string(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "roundtrip content");
        std::fs::remove_file(&path_str).ok();
    }

    // ── open / close / buf_reader / read_line ─────────────────────────────

    #[test]
    fn open_missing_file_returns_err() {
        let p = path("/tmp/mvl_no_such_file_to_open_xyz".to_string());
        // open() creates the file if it doesn't exist, so use a dir path to force error
        let p_dir = path("/tmp/mvl_no_such_dir_xyz/file.txt".to_string());
        assert!(open(p_dir).is_err());
        // Clean up if open() created the file
        std::fs::remove_file("/tmp/mvl_no_such_file_to_open_xyz").ok();
        let _ = p;
    }

    #[test]
    fn buf_reader_read_line_returns_content() {
        let path_str = tmp("mvl_test_buf_reader.txt");
        std::fs::write(&path_str, "first line\nsecond line\n").unwrap();
        let p = path(path_str.clone());
        let f = open(p).unwrap();
        let r = buf_reader(f);
        let line = read_line(r).unwrap();
        assert_eq!(line.0, "first line\n");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_line_eof_returns_empty_string() {
        let path_str = tmp("mvl_test_read_line_eof.txt");
        std::fs::write(&path_str, "").unwrap();
        let p = path(path_str.clone());
        let f = open(p).unwrap();
        let r = buf_reader(f);
        let line = read_line(r).unwrap();
        assert_eq!(line.0, "", "EOF should return empty string");
        std::fs::remove_file(&path_str).ok();
    }

    // ── write_line ─────────────────────────────────────────────────────────

    #[test]
    fn write_line_creates_file_with_newline() {
        let path_str = tmp("mvl_test_write_line.txt");
        // Remove if exists from prior run
        std::fs::remove_file(&path_str).ok();
        let p = path(path_str.clone());
        let f = open(p).unwrap();
        let w = buf_writer(f);
        write_line(w, "header".to_string()).unwrap();
        let got = std::fs::read_to_string(&path_str).unwrap();
        assert_eq!(got, "header\n");
        std::fs::remove_file(&path_str).ok();
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
}
