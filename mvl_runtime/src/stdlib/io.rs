//! Rust implementations of `std.io` stdlib functions.
//!
//! Provides real file I/O backing for the stubs declared in `std/io.mvl`.
//! These are re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::{Clean, Tainted};

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

/// Construct a [`Path`] from a `String`.
///
/// Pure — no filesystem access.
pub fn path(s: String) -> Path {
    Path { inner: s }
}

/// Read the entire contents of a file into a string.
///
/// Returns `Ok(Tainted<String>)` on success (file contents are external input).
/// Returns `Err(String)` with a sanitized error category on failure (no path leaked).
///
/// Implements the Rust backing for `std/io.mvl::read_to_string`.
pub fn read_to_string(p: Path) -> Result<Tainted<String>, String> {
    std::fs::read_to_string(p.as_str())
        .map(Tainted)
        .map_err(|e| sanitize_io_error(e.kind()))
}

/// Read the entire contents of a file given a validated (Clean) path string.
///
/// Convenience variant of [`read_to_string`] that accepts a `Clean<String>`
/// path directly. Use this when the caller already holds a validated path string.
///
/// Returns `Ok(Tainted<String>)` on success.
/// Returns `Err(String)` with a sanitized error category on failure (no path leaked).
///
/// Implements the Rust backing for `std/io.mvl::read_file`.
pub fn read_file(p: Clean<String>) -> Result<Tainted<String>, String> {
    std::fs::read_to_string(&*p)
        .map(Tainted)
        .map_err(|e| sanitize_io_error(e.kind()))
}

/// Convert an I/O error to a sanitized category string.
///
/// Returns a fixed string that does not include the file path or OS-level details,
/// preventing information disclosure when error values are surfaced to callers.
fn sanitize_io_error(kind: std::io::ErrorKind) -> String {
    match kind {
        std::io::ErrorKind::NotFound => "file not found".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => "I/O error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_stores_inner_string() {
        let p = path("foo/bar.txt".to_string());
        assert_eq!(p.as_str(), "foo/bar.txt");
    }

    #[test]
    fn read_to_string_missing_file_returns_err() {
        let p = path("/tmp/mvl_nonexistent_file_xyz_12345".to_string());
        assert!(read_to_string(p).is_err());
    }

    #[test]
    fn read_to_string_real_file() {
        // Write a temp file using only std
        let path_str = std::env::temp_dir().join("mvl_test_read_to_string.txt");
        std::fs::write(&path_str, "hello mvl").unwrap();
        let p = path(path_str.to_string_lossy().into_owned());
        let result = read_to_string(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "hello mvl");
        std::fs::remove_file(&path_str).ok();
    }

    #[test]
    fn read_file_missing_file_returns_err() {
        let p = Clean("/tmp/mvl_nonexistent_file_xyz_12345".to_string());
        assert!(read_file(p).is_err());
    }

    #[test]
    fn read_file_real_file() {
        let path_str = std::env::temp_dir().join("mvl_test_read_file.txt");
        std::fs::write(&path_str, "world mvl").unwrap();
        let p = Clean(path_str.to_string_lossy().into_owned());
        let result = read_file(p);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0, "world mvl");
        std::fs::remove_file(&path_str).ok();
    }
}
