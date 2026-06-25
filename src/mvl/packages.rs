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
//!
//! # Module layout
//!
//! This module is a facade. The implementations live in topic sub-modules:
//! - [`cmd_add`], [`cmd_install`], [`cmd_update`] — the basic verbs
//! - [`cmd_sbom`] — SBOM generation, snapshot, diff (#636)
//! - [`cmd_audit`] — supply-chain, license, paradox audits (#633, #635, #637)
//! - [`resolver`] — pre-build [`ensure_dependencies`] entry point
//! - [`error`] — the unified [`PackageError`] type
//! - [`config`] — internal global-config + semver-tag picker
//! - [`audit`], [`fetch`], [`hash`], [`lock`], [`manifest`], [`mvs`],
//!   [`sbom`], [`sbom_diff`], [`version`] — supporting libraries

pub mod audit;
pub(crate) mod cmd_add;
pub(crate) mod cmd_audit;
pub(crate) mod cmd_install;
pub(crate) mod cmd_sbom;
pub(crate) mod cmd_update;
pub(crate) mod config;
pub(crate) mod error;
pub mod fetch;
pub mod hash;
pub mod lock;
pub mod manifest;
pub mod mvs;
pub(crate) mod resolver;
pub mod sbom;
pub mod sbom_diff;
pub mod version;

// ── Public re-exports — the stable API consumed by `cli/` and `loader.rs` ─────

pub use cmd_add::cmd_add;
pub use cmd_audit::{
    cmd_audit_license, cmd_audit_paradox, cmd_audit_supply_chain, LicenseAudit, LicenseEntry,
    LicenseStatus, ParadoxAudit, ParadoxEntry,
};
pub use cmd_install::cmd_install;
pub use cmd_sbom::{cmd_sbom, cmd_sbom_diff, cmd_sbom_snapshot};
pub use cmd_update::{cmd_update, UpdateOptions};
pub use error::PackageError;
pub use fetch::{local_override_dir, pkg_cache_root};
pub use resolver::ensure_dependencies;
