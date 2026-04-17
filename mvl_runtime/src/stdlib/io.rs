//! Rust implementations of `std.io` stdlib functions.
//!
//! Provides real file I/O backing for the stubs declared in `std/io.mvl`.
//! These are re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::{Clean, Tainted};

/// Filesystem path — mirrors the `Path` struct declared in `std/io.mvl`.
///
/// Construct with [`path`]. The `inner` field holds the raw path string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Path {
    /// The raw path string.
    pub inner: String,
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
/// Returns `Err(String)` with the OS error message on failure.
///
/// Implements the Rust backing for `std/io.mvl::read_to_string`.
pub fn read_to_string(p: Path) -> Result<Tainted<String>, String> {
    std::fs::read_to_string(&p.inner)
        .map(Tainted)
        .map_err(|e| e.to_string())
}

/// Read the entire contents of a file given a validated (Clean) path string.
///
/// Convenience variant of [`read_to_string`] that accepts a `Clean<String>`
/// path directly. Use this when migrating from `extern "rust"` bridge functions
/// that accept `Clean<String>` paths (e.g. `fs_read_file`).
///
/// Returns `Ok(Tainted<String>)` on success.
/// Returns `Err(String)` with the OS error message on failure.
///
/// Implements the Rust backing for `std/io.mvl::read_file`.
pub fn read_file(p: Clean<String>) -> Result<Tainted<String>, String> {
    std::fs::read_to_string(&*p)
        .map(Tainted)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn path_stores_inner_string() {
        let p = path("foo/bar.txt".to_string());
        assert_eq!(p.inner, "foo/bar.txt");
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
