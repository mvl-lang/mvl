// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Unified error type for package operations.

use super::{fetch, lock, manifest};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
