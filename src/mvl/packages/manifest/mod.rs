// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl.toml` manifest parsing and writing.
//!
//! Implements Spec 008 Requirement 1: Package Manifest.

use std::collections::HashMap;
use std::path::Path;

/// The `[package]` table in `mvl.toml`.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub license: String,
    /// MVL compiler version constraint: `">=0.24.0"`.
    pub requires_mvl: String,
    /// Required when any `extern "rust"` block exists (Spec 008 Req 1).
    pub extern_rationale: Option<String>,
}

/// A C-native dependency specification with optional license.
#[derive(Debug, Clone)]
pub struct CNativeSpec {
    pub version: String,
    pub license: Option<String>,
}

/// License policy mode (#635).
#[derive(Debug, Clone, PartialEq)]
pub enum LicensePolicyMode {
    /// Allow standard permissive licenses; reject copyleft. Default.
    Permissive,
    /// Allow both permissive and copyleft licenses.
    CopyleftOk,
    /// Allow everything — no enforcement.
    Any,
    /// Use explicit allow/deny lists.
    Custom,
}

/// License policy configuration from `[license-policy]` (#635).
#[derive(Debug, Clone)]
pub struct LicensePolicy {
    pub mode: LicensePolicyMode,
    /// Explicit allow list (used in `Custom` mode, but also extends other modes).
    pub allow: Vec<String>,
    /// Explicit deny list (used in `Custom` mode, but also applies in all modes).
    pub deny: Vec<String>,
}

/// Standard permissive SPDX license IDs accepted by the `Permissive` policy.
const PERMISSIVE_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "0BSD",
    "Unlicense",
    "CC0-1.0",
    "BSL-1.0",
];

/// Additional copyleft licenses accepted by the `CopyleftOk` policy.
const COPYLEFT_LICENSES: &[&str] = &[
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MPL-2.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
];

impl Default for LicensePolicy {
    fn default() -> Self {
        LicensePolicy {
            mode: LicensePolicyMode::Permissive,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
}

impl LicensePolicy {
    /// Check whether an SPDX license expression is compatible with this policy.
    ///
    /// For `OR` expressions (e.g. "MIT OR Apache-2.0"), the license is
    /// compatible if *any* alternative is allowed.
    ///
    /// Returns `Ok(())` if compatible, or `Err(reason)` if rejected.
    pub fn check(&self, license_expr: &str) -> Result<(), String> {
        if self.mode == LicensePolicyMode::Any {
            return Ok(());
        }

        // Split on " OR " to handle SPDX disjunctions
        let alternatives: Vec<&str> = license_expr.split(" OR ").map(|s| s.trim()).collect();

        // Check deny list first — any denied alternative taints the expression
        // unless another alternative is allowed
        let mut any_allowed = false;
        let mut all_denied_reason = String::new();

        for alt in &alternatives {
            // Explicit deny list always wins
            if self.deny.iter().any(|d| d == alt) {
                all_denied_reason = format!("{alt} is in the deny list");
                continue;
            }

            // Explicit allow list always passes
            if self.allow.iter().any(|a| a == alt) {
                any_allowed = true;
                break;
            }

            match self.mode {
                LicensePolicyMode::Permissive => {
                    if PERMISSIVE_LICENSES.iter().any(|p| p == alt) {
                        any_allowed = true;
                        break;
                    }
                    all_denied_reason = format!("{alt} is not a recognized permissive license");
                }
                LicensePolicyMode::CopyleftOk => {
                    if PERMISSIVE_LICENSES.iter().any(|p| p == alt)
                        || COPYLEFT_LICENSES.iter().any(|c| c == alt)
                    {
                        any_allowed = true;
                        break;
                    }
                    all_denied_reason =
                        format!("{alt} is not a recognized permissive or copyleft license");
                }
                LicensePolicyMode::Custom => {
                    // In custom mode, only explicitly allowed licenses pass
                    all_denied_reason = format!("{alt} is not in the allow list");
                }
                LicensePolicyMode::Any => {
                    // Any-mode is handled by the early return at the top of
                    // this fn. Defensive arm: if that branch is ever removed,
                    // accept here too instead of panicking.
                    any_allowed = true;
                    break;
                }
            }
        }

        if any_allowed {
            Ok(())
        } else {
            Err(all_denied_reason)
        }
    }

    /// Human-readable policy name for display.
    pub fn mode_str(&self) -> &'static str {
        match self.mode {
            LicensePolicyMode::Permissive => "permissive",
            LicensePolicyMode::CopyleftOk => "copyleft-ok",
            LicensePolicyMode::Any => "any",
            LicensePolicyMode::Custom => "custom",
        }
    }
}

/// A dependency specification.
#[derive(Debug, Clone)]
pub enum DepSpec {
    /// Version constraint string: `">=1.0.0, <2.0.0"`.
    Version(String),
    /// Git dependency with a tag: `{ git = "...", tag = "v1.2.0" }`.
    Git {
        git: String,
        tag: String,
        rationale: Option<String>,
        /// Versions to skip during `mvl update`, e.g. known-CVE releases.
        exclude: Vec<String>,
    },
}

impl DepSpec {
    /// Return the declared version/tag string for display.
    pub fn version_str(&self) -> &str {
        match self {
            DepSpec::Version(v) => v,
            DepSpec::Git { tag, .. } => tag,
        }
    }

    /// Return the dependency rationale, if any.
    pub fn rationale(&self) -> Option<&str> {
        match self {
            DepSpec::Git { rationale, .. } => rationale.as_deref(),
            DepSpec::Version(_) => None,
        }
    }

    /// Return the per-dependency exclusion list, if any.
    pub fn exclude(&self) -> &[String] {
        match self {
            DepSpec::Git { exclude, .. } => exclude,
            DepSpec::Version(_) => &[],
        }
    }
}

/// Security policy from `[security]` in `mvl.toml` or XDG global config.
#[derive(Debug, Clone, Default)]
pub struct SecurityPolicy {
    /// Minimum number of days a git tag must be published before `mvl update`
    /// will select it. `0` means no restriction (default).
    pub min_age_days: u64,
}

/// Dependency policy configuration from `[dependency-policy]`.
#[derive(Debug, Clone)]
pub struct DependencyPolicy {
    /// LOC threshold below which a rationale warning fires. Default: 1000.
    pub complexity_threshold: u64,
    /// Whether rationale is required for all dependencies. Default: true.
    pub rationale_required: bool,
}

impl Default for DependencyPolicy {
    fn default() -> Self {
        DependencyPolicy {
            complexity_threshold: 1000,
            rationale_required: true,
        }
    }
}

/// Parsed `mvl.toml` manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub package: PackageInfo,
    /// `[dependencies]` — MVL package dependencies.
    pub dependencies: HashMap<String, DepSpec>,
    /// `[native]` — Rust crates used in `bridge.rs` (for SBOM).
    pub native: HashMap<String, String>,
    /// `[c-native]` — C libraries linked via `extern "c"` blocks (#633/#635).
    pub c_native: HashMap<String, CNativeSpec>,
    /// `[dependency-policy]` — Dependency Paradox enforcement settings.
    pub dependency_policy: DependencyPolicy,
    /// `[license-policy]` — License enforcement settings (#635).
    pub license_policy: LicensePolicy,
    /// `[security]` — Supply-chain security settings (lockout period, etc.).
    pub security: SecurityPolicy,
}

/// Load and parse a cached package's `mvl.toml`.
///
/// Resolves the global package cache directory for `name`/`version` and
/// returns the parsed `Manifest`, or `None` if the directory or file is
/// missing or unparseable. Use this when you need to inspect metadata of a
/// transitively cached package (license, deps, etc.) without caring about
/// the specific I/O or parse failure mode — those callers all collapse the
/// error to "skip this entry" anyway. See #1537.
pub fn load_cached_manifest(name: &str, version: &str) -> Option<Manifest> {
    let dir = super::fetch::pkg_cache_dir(name, version);
    Manifest::load(&dir).ok()
}

impl Manifest {
    /// Load and parse `mvl.toml` from the given directory.
    pub fn load(dir: &Path) -> Result<Self, ManifestError> {
        let path = dir.join("mvl.toml");
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ManifestError::Io(path.display().to_string(), e.to_string()))?;
        Self::parse(&content)
    }

    /// Parse a manifest from TOML source text.
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        let table = parse_toml_table(content).map_err(ManifestError::ParseError)?;

        let pkg = table
            .get("package")
            .and_then(|v| v.as_table())
            .ok_or_else(|| ManifestError::MissingSection("[package]".to_string()))?;

        let name = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField("name".to_string()))?
            .to_string();
        let version = pkg
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField("version".to_string()))?
            .to_string();
        let license = pkg
            .get("license")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField("license".to_string()))?
            .to_string();
        let requires_mvl = pkg
            .get("requires-mvl")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ManifestError::MissingField("requires-mvl".to_string()))?
            .to_string();
        let extern_rationale = pkg
            .get("extern-rationale")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let dependencies = parse_dependencies(table.get("dependencies"))?;
        let native = parse_native(table.get("native"), "native")?;
        let c_native = parse_c_native_section(table.get("c-native"))?;
        let dependency_policy = parse_dependency_policy(table.get("dependency-policy"))?;
        let license_policy = parse_license_policy(table.get("license-policy"))?;
        let security = parse_security_policy(table.get("security"))?;

        Ok(Manifest {
            package: PackageInfo {
                name,
                version,
                license,
                requires_mvl,
                extern_rationale,
            },
            dependencies,
            native,
            c_native,
            dependency_policy,
            license_policy,
            security,
        })
    }

    /// Check that `extern-rationale` is present if `has_extern` is true.
    ///
    /// Returns `Err` with error code `E700` if validation fails.
    pub fn validate_extern(&self, has_extern: bool) -> Result<(), ManifestError> {
        if has_extern && self.package.extern_rationale.is_none() {
            return Err(ManifestError::MissingExternRationale(
                self.package.name.clone(),
            ));
        }
        Ok(())
    }

    /// Validate that the `license` field in `mvl.toml` matches the LICENSE file.
    ///
    /// Checks two things:
    /// 1. A LICENSE file exists in the project directory
    /// 2. The LICENSE file content is consistent with the declared SPDX identifier
    ///
    /// Returns `Ok(())` on success, `Err(LicenseMismatch)` on failure.
    pub fn validate_license(&self, dir: &Path) -> Result<(), ManifestError> {
        let license_path = dir.join("LICENSE");
        if !license_path.exists() {
            return Err(ManifestError::LicenseMismatch(format!(
                "license = \"{}\" but no LICENSE file found in {}",
                self.package.license,
                dir.display()
            )));
        }
        let content = std::fs::read_to_string(&license_path)
            .map_err(|e| ManifestError::Io(license_path.display().to_string(), e.to_string()))?;
        let expected = &self.package.license;
        let matches = match expected.as_str() {
            "Apache-2.0" => content.contains("Apache License") && content.contains("Version 2.0"),
            "MIT" => {
                content.contains("MIT License") || content.contains("Permission is hereby granted")
            }
            "BSD-2-Clause" => {
                content.contains("BSD 2-Clause") || content.contains("Redistribution and use")
            }
            "BSD-3-Clause" => {
                content.contains("BSD 3-Clause") || content.contains("neither the name")
            }
            "ISC" => {
                content.contains("ISC License")
                    || content.contains("Permission to use, copy, modify")
            }
            _ => true, // unknown SPDX id — skip content check, file existence is enough
        };
        if !matches {
            return Err(ManifestError::LicenseMismatch(format!(
                "license = \"{}\" but LICENSE file content does not match",
                expected
            )));
        }
        Ok(())
    }

    /// Validate dependency rationale policy (#637).
    ///
    /// When `rationale-required` is true, returns a list of dependency names
    /// that are missing a rationale string. Returns an empty vec when all
    /// deps are compliant or when enforcement is disabled.
    pub fn audit_dep_rationale(&self) -> Vec<String> {
        if !self.dependency_policy.rationale_required {
            return Vec::new();
        }
        let mut missing = Vec::new();
        let mut deps: Vec<(&String, &DepSpec)> = self.dependencies.iter().collect();
        deps.sort_by_key(|(k, _)| k.as_str());
        for (name, spec) in deps {
            if spec.rationale().is_none() {
                missing.push(name.clone());
            }
        }
        missing
    }

    /// Serialize the manifest back to TOML text.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("[package]\n");
        out.push_str(&format!("name = \"{}\"\n", self.package.name));
        out.push_str(&format!("version = \"{}\"\n", self.package.version));
        out.push_str(&format!("license = \"{}\"\n", self.package.license));
        out.push_str(&format!(
            "requires-mvl = \"{}\"\n",
            self.package.requires_mvl
        ));
        if let Some(ref r) = self.package.extern_rationale {
            out.push_str(&format!("extern-rationale = \"{}\"\n", toml_escape(r)));
        }

        if !self.dependencies.is_empty() {
            out.push_str("\n[dependencies]\n");
            let mut deps: Vec<(&String, &DepSpec)> = self.dependencies.iter().collect();
            deps.sort_by_key(|(k, _)| *k);
            for (name, spec) in deps {
                match spec {
                    DepSpec::Version(v) => {
                        out.push_str(&format!("\"{}\" = \"{}\"\n", name, toml_escape(v)));
                    }
                    DepSpec::Git {
                        git,
                        tag,
                        rationale,
                        exclude,
                    } => {
                        let mut parts = format!(
                            "git = \"{}\", tag = \"{}\"",
                            toml_escape(git),
                            toml_escape(tag)
                        );
                        if let Some(ref r) = rationale {
                            parts.push_str(&format!(", rationale = \"{}\"", toml_escape(r)));
                        }
                        if !exclude.is_empty() {
                            let list = exclude
                                .iter()
                                .map(|v| format!("\"{}\"", toml_escape(v)))
                                .collect::<Vec<_>>()
                                .join(", ");
                            parts.push_str(&format!(", exclude = [{list}]"));
                        }
                        out.push_str(&format!("\"{}\" = {{ {} }}\n", name, parts));
                    }
                }
            }
        }

        // Write [dependency-policy] only when non-default
        let default_policy = DependencyPolicy::default();
        if self.dependency_policy.complexity_threshold != default_policy.complexity_threshold
            || self.dependency_policy.rationale_required != default_policy.rationale_required
        {
            out.push_str("\n[dependency-policy]\n");
            if self.dependency_policy.complexity_threshold != default_policy.complexity_threshold {
                out.push_str(&format!(
                    "complexity-threshold = {}\n",
                    self.dependency_policy.complexity_threshold
                ));
            }
            if self.dependency_policy.rationale_required != default_policy.rationale_required {
                out.push_str(&format!(
                    "rationale-required = {}\n",
                    self.dependency_policy.rationale_required
                ));
            }
        }

        if !self.native.is_empty() {
            out.push_str("\n[native]\n");
            let mut native: Vec<(&String, &String)> = self.native.iter().collect();
            native.sort_by_key(|(k, _)| *k);
            for (name, version) in native {
                out.push_str(&format!("{} = \"{}\"\n", name, toml_escape(version)));
            }
        }

        if !self.c_native.is_empty() {
            out.push_str("\n[c-native]\n");
            let mut c_native: Vec<(&String, &CNativeSpec)> = self.c_native.iter().collect();
            c_native.sort_by_key(|(k, _)| *k);
            for (name, spec) in c_native {
                if let Some(ref lic) = spec.license {
                    out.push_str(&format!(
                        "{} = {{ version = \"{}\", license = \"{}\" }}\n",
                        name,
                        toml_escape(&spec.version),
                        toml_escape(lic)
                    ));
                } else {
                    out.push_str(&format!("{} = \"{}\"\n", name, toml_escape(&spec.version)));
                }
            }
        }

        // Write [license-policy] only when non-default
        let default_lp = LicensePolicy::default();
        if self.license_policy.mode != default_lp.mode
            || !self.license_policy.allow.is_empty()
            || !self.license_policy.deny.is_empty()
        {
            out.push_str("\n[license-policy]\n");
            if self.license_policy.mode != default_lp.mode {
                out.push_str(&format!("mode = \"{}\"\n", self.license_policy.mode_str()));
            }
            if !self.license_policy.allow.is_empty() {
                let items: Vec<String> = self
                    .license_policy
                    .allow
                    .iter()
                    .map(|s| format!("\"{}\"", toml_escape(s)))
                    .collect();
                out.push_str(&format!("allow = [{}]\n", items.join(", ")));
            }
            if !self.license_policy.deny.is_empty() {
                let items: Vec<String> = self
                    .license_policy
                    .deny
                    .iter()
                    .map(|s| format!("\"{}\"", toml_escape(s)))
                    .collect();
                out.push_str(&format!("deny = [{}]\n", items.join(", ")));
            }
        }

        if self.security.min_age_days > 0 {
            out.push_str("\n[security]\n");
            out.push_str(&format!("min-age-days = {}\n", self.security.min_age_days));
        }

        out
    }

    /// Create a minimal manifest for a new project.
    pub fn new_project(name: &str, mvl_version: &str) -> Self {
        Manifest {
            package: PackageInfo {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                license: "MIT".to_string(),
                requires_mvl: format!(">={}", mvl_version),
                extern_rationale: None,
            },
            dependencies: HashMap::new(),
            native: HashMap::new(),
            c_native: HashMap::new(),
            dependency_policy: DependencyPolicy::default(),
            license_policy: LicensePolicy::default(),
            security: SecurityPolicy::default(),
        }
    }
}

/// Errors that can occur when reading or validating a manifest.
#[derive(Debug)]
pub enum ManifestError {
    Io(String, String),
    ParseError(String),
    MissingSection(String),
    MissingField(String),
    /// E700: extern-rationale required when extern blocks are present.
    MissingExternRationale(String),
    /// E701: dependency rationale required by policy (#637).
    MissingDepRationale(Vec<String>),
    /// E702: license field does not match LICENSE file.
    LicenseMismatch(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(path, e) => write!(f, "cannot read {path}: {e}"),
            ManifestError::ParseError(e) => write!(f, "TOML parse error: {e}"),
            ManifestError::MissingSection(s) => write!(f, "mvl.toml: missing {s} section"),
            ManifestError::MissingField(n) => write!(f, "mvl.toml: missing required field '{n}'"),
            ManifestError::MissingExternRationale(pkg) => write!(
                f,
                "E700: extern-rationale required when extern blocks are present in '{pkg}'"
            ),
            ManifestError::MissingDepRationale(deps) => {
                write!(
                    f,
                    "E701: dependency rationale required for: {}",
                    deps.join(", ")
                )
            }
            ManifestError::LicenseMismatch(msg) => {
                write!(f, "E702: {msg}")
            }
        }
    }
}

// ── Internal sub-modules ─────────────────────────────────────────────────────

mod sections;
mod toml;

use sections::{
    parse_c_native_section, parse_dependencies, parse_dependency_policy, parse_license_policy,
    parse_native, parse_security_policy,
};
use toml::{parse_toml_table, toml_escape};

// ── In-place tag sync (#1459) ────────────────────────────────────────────────

/// Rewrite the `tag = "..."` field for each updated dep in `mvl.toml` (#1459).
///
/// Uses a line-surgical edit (not a full to_toml roundtrip) so comments and
/// formatting in the user's manifest are preserved.
pub(super) fn sync_manifest_tags(
    project_root: &Path,
    updates: &[(String, String)],
) -> Result<(), super::error::PackageError> {
    use super::error::PackageError;
    let path = project_root.join("mvl.toml");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| PackageError::Io(path.display().to_string(), e.to_string()))?;

    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        let mut replaced = line.to_string();
        let trimmed = line.trim_start();
        // Match lines that begin with `"<dep-name>" = { ... }` and contain a
        // `tag = "..."` entry.
        if let Some(after_quote) = trimmed.strip_prefix('"') {
            if let Some(close_quote) = after_quote.find('"') {
                let dep_name = &after_quote[..close_quote];
                if let Some((_, new_ver)) = updates.iter().find(|(n, _)| n == dep_name) {
                    replaced = rewrite_tag_in_line(line, new_ver);
                }
            }
        }
        out.push_str(&replaced);
        out.push('\n');
    }
    // Preserve original trailing newline behavior — only strip the one we added
    // if the original didn't end with a newline.
    if !content.ends_with('\n') {
        out.pop();
    }
    std::fs::write(&path, out)
        .map_err(|e| PackageError::Io(path.display().to_string(), e.to_string()))?;
    Ok(())
}

/// Replace a `tag = "vX.Y.Z"` substring in `line` with the new version,
/// preserving an existing `v` prefix if present.
pub(super) fn rewrite_tag_in_line(line: &str, new_version: &str) -> String {
    let needle = "tag = \"";
    let start = match line.find(needle) {
        Some(i) => i + needle.len(),
        None => return line.to_string(),
    };
    let rest = &line[start..];
    let end_off = match rest.find('"') {
        Some(i) => i,
        None => return line.to_string(),
    };
    let current_tag = &rest[..end_off];
    let new_tag = if current_tag.starts_with('v') {
        format!("v{new_version}")
    } else {
        new_version.to_string()
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..start]);
    out.push_str(&new_tag);
    out.push_str(&line[start + end_off..]);
    out
}

#[cfg(test)]
mod tests {
    use super::toml::{strip_comment, unescape_string};
    use super::*;

    const MINIMAL: &str = r#"
[package]
name = "mvl-json"
version = "1.0.0"
license = "MIT"
requires-mvl = ">=0.6.0"
"#;

    const WITH_DEPS: &str = r#"
[package]
name = "http"
version = "1.2.0"
license = "MIT"
requires-mvl = ">=0.24.0"
extern-rationale = "wraps hyper for async HTTP"

[dependencies]
"github.com/lab271/mvl-stdlib" = ">=1.0.0, <2.0.0"
tls = { git = "https://github.com/lab271/mvl_tls", tag = "v0.4.0" }

[native]
hyper = "1.0"
"#;

    const WITH_C_NATIVE: &str = r#"
[package]
name = "crypto-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.40.0"
extern-rationale = "links openssl and zlib"

[c-native]
libz = "1.3"
openssl = "3.0"
"#;

    // ── Existing tests ────────────────────────────────────────────────────────

    #[test]
    fn parse_minimal_manifest() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert_eq!(m.package.name, "mvl-json");
        assert_eq!(m.package.version, "1.0.0");
        assert_eq!(m.package.license, "MIT");
        assert_eq!(m.package.requires_mvl, ">=0.6.0");
        assert!(m.package.extern_rationale.is_none());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn parse_manifest_with_dependencies() {
        let m = Manifest::parse(WITH_DEPS).unwrap();
        assert_eq!(m.package.name, "http");
        assert_eq!(
            m.package.extern_rationale.as_deref(),
            Some("wraps hyper for async HTTP")
        );
        assert!(m.dependencies.contains_key("github.com/lab271/mvl-stdlib"));
        assert!(m.dependencies.contains_key("tls"));
        match m.dependencies.get("tls").unwrap() {
            DepSpec::Git { git, tag, .. } => {
                assert!(git.contains("mvl_tls"));
                assert_eq!(tag, "v0.4.0");
            }
            _ => panic!("expected git dep"),
        }
        assert_eq!(m.native.get("hyper").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn missing_required_field_returns_error() {
        let bad = "[package]\nname = \"foo\"\nversion = \"1.0.0\"\n";
        let err = Manifest::parse(bad).unwrap_err();
        assert!(matches!(err, ManifestError::MissingField(_)));
    }

    #[test]
    fn validate_extern_rationale_required() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert!(m.validate_extern(false).is_ok());
        let err = m.validate_extern(true).unwrap_err();
        assert!(matches!(err, ManifestError::MissingExternRationale(_)));
    }

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest::parse(WITH_DEPS).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(m2.package.name, m.package.name);
        assert_eq!(m2.package.version, m.package.version);
        assert_eq!(m2.dependencies.len(), m.dependencies.len());
    }

    #[test]
    fn new_project_manifest() {
        let m = Manifest::new_project("my-app", "0.42.0");
        assert_eq!(m.package.name, "my-app");
        assert_eq!(m.package.version, "0.1.0");
        assert_eq!(m.package.requires_mvl, ">=0.42.0");
    }

    // ── New tests ─────────────────────────────────────────────────────────────

    // --- missing section ---

    #[test]
    fn parse_missing_package_section_returns_error() {
        let content = "name = \"foo\"\nversion = \"1.0.0\"\n";
        let err = Manifest::parse(content).unwrap_err();
        assert!(matches!(err, ManifestError::MissingSection(_)));
    }

    // --- dependency inline-table edge cases ---

    #[test]
    fn dep_with_inline_table_missing_git_field_returns_error() {
        let content = r#"
[package]
name = "foo"
version = "1.0.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
bar = { tag = "v1.0.0" }
"#;
        let err = Manifest::parse(content).unwrap_err();
        assert!(
            matches!(err, ManifestError::ParseError(ref s) if s.contains("missing 'git'")),
            "got: {err}"
        );
    }

    #[test]
    fn dep_with_inline_table_missing_tag_field_returns_error() {
        let content = r#"
[package]
name = "foo"
version = "1.0.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
bar = { git = "https://example.com/bar" }
"#;
        let err = Manifest::parse(content).unwrap_err();
        assert!(
            matches!(err, ManifestError::ParseError(ref s) if s.contains("missing 'tag'")),
            "got: {err}"
        );
    }

    // --- validate_extern ---

    #[test]
    fn validate_extern_passes_when_rationale_present() {
        let m = Manifest::parse(WITH_DEPS).unwrap();
        // WITH_DEPS has extern-rationale set
        assert!(m.validate_extern(true).is_ok());
        assert!(m.validate_extern(false).is_ok());
    }

    // --- DepSpec::version_str ---

    #[test]
    fn dep_version_str_for_version_spec() {
        let spec = DepSpec::Version(">=1.0.0, <2.0.0".to_string());
        assert_eq!(spec.version_str(), ">=1.0.0, <2.0.0");
    }

    #[test]
    fn dep_version_str_for_git_spec() {
        let spec = DepSpec::Git {
            git: "https://example.com/pkg".to_string(),
            tag: "v1.2.3".to_string(),
            rationale: None,
            exclude: vec![],
        };
        assert_eq!(spec.version_str(), "v1.2.3");
    }

    // --- toml_escape / unescape roundtrip ---

    #[test]
    fn toml_escape_backslash_and_quote() {
        let original = r#"has "quotes" and \backslash"#;
        let escaped = toml_escape(original);
        let unescaped = unescape_string(&escaped);
        assert_eq!(unescaped, original);
    }

    #[test]
    fn toml_escape_plain_string_unchanged() {
        let s = "plain string with no special chars";
        assert_eq!(toml_escape(s), s);
    }

    // --- strip_comment ---

    #[test]
    fn strip_comment_ignores_hash_in_string() {
        // Hash inside a quoted string must not be treated as a comment
        let line = r#"key = "value # not a comment""#;
        let stripped = strip_comment(line);
        assert_eq!(stripped, line);
    }

    #[test]
    fn strip_comment_strips_trailing_hash() {
        let line = r#"key = "value" # this is a comment"#;
        let stripped = strip_comment(line).trim();
        assert_eq!(stripped, r#"key = "value""#);
    }

    // --- new_project ---

    #[test]
    fn new_project_has_empty_deps_and_native() {
        let m = Manifest::new_project("app", "1.0.0");
        assert!(m.dependencies.is_empty());
        assert!(m.native.is_empty());
        assert!(m.package.extern_rationale.is_none());
    }

    // --- ManifestError Display ---

    #[test]
    fn manifest_error_display_io() {
        let e = ManifestError::Io("/path".to_string(), "not found".to_string());
        assert!(e.to_string().contains("/path"));
    }

    #[test]
    fn manifest_error_display_missing_section() {
        let e = ManifestError::MissingSection("[package]".to_string());
        assert!(e.to_string().contains("[package]"));
    }

    #[test]
    fn manifest_error_display_missing_field() {
        let e = ManifestError::MissingField("license".to_string());
        assert!(e.to_string().contains("license"));
    }

    #[test]
    fn manifest_error_display_extern_rationale() {
        let e = ManifestError::MissingExternRationale("my-pkg".to_string());
        let s = e.to_string();
        assert!(s.contains("E700"));
        assert!(s.contains("my-pkg"));
    }

    // --- load from file ---

    #[test]
    fn load_parses_file_from_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mvl.toml"), MINIMAL).unwrap();
        let m = Manifest::load(tmp.path()).unwrap();
        assert_eq!(m.package.name, "mvl-json");
    }

    #[test]
    fn load_returns_io_error_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Manifest::load(tmp.path()).unwrap_err();
        assert!(matches!(err, ManifestError::Io(_, _)));
    }

    // --- package name with dots and slashes ---

    #[test]
    fn parse_package_name_with_dots_and_slashes() {
        let content = r#"
[package]
name = "github.com/lab271/mvl-stdlib"
version = "2.0.0"
license = "Apache-2.0"
requires-mvl = ">=0.40.0"
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.package.name, "github.com/lab271/mvl-stdlib");
    }

    // --- dependency rationale (#637) ---

    #[test]
    fn parse_dep_with_rationale() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/briansmith/ring", tag = "v0.17.8", rationale = "Crypto — constant-time guarantees" }
"#;
        let m = Manifest::parse(content).unwrap();
        let dep = m.dependencies.get("ring").unwrap();
        assert_eq!(dep.rationale(), Some("Crypto — constant-time guarantees"));
    }

    #[test]
    fn parse_dep_without_rationale_returns_none() {
        let m = Manifest::parse(WITH_DEPS).unwrap();
        let dep = m.dependencies.get("tls").unwrap();
        assert!(dep.rationale().is_none());
    }

    #[test]
    fn rationale_roundtrip() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/briansmith/ring", tag = "v0.17.8", rationale = "Crypto needs" }
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(
            m2.dependencies.get("ring").unwrap().rationale(),
            Some("Crypto needs")
        );
    }

    #[test]
    fn version_dep_rationale_is_none() {
        let spec = DepSpec::Version(">=1.0.0".to_string());
        assert!(spec.rationale().is_none());
    }

    // --- exclude field (#1416) ---

    #[test]
    fn parse_dep_with_exclude_list() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/briansmith/ring", tag = "v0.17.8", exclude = ["0.16.0", "0.16.1"] }
"#;
        let m = Manifest::parse(content).unwrap();
        let dep = m.dependencies.get("ring").unwrap();
        assert_eq!(dep.exclude(), &["0.16.0", "0.16.1"]);
    }

    #[test]
    fn parse_dep_without_exclude_returns_empty() {
        let m = Manifest::parse(WITH_DEPS).unwrap();
        let dep = m.dependencies.get("tls").unwrap();
        assert!(dep.exclude().is_empty());
    }

    #[test]
    fn exclude_roundtrip() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/briansmith/ring", tag = "v0.17.8", exclude = ["0.16.0", "0.17.0"] }
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(
            m2.dependencies.get("ring").unwrap().exclude(),
            &["0.16.0", "0.17.0"]
        );
    }

    #[test]
    fn version_dep_exclude_is_empty() {
        let spec = DepSpec::Version(">=1.0.0".to_string());
        assert!(spec.exclude().is_empty());
    }

    // --- security policy (#1414) ---

    #[test]
    fn parse_default_security_policy() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert_eq!(m.security.min_age_days, 0);
    }

    #[test]
    fn parse_security_min_age_days() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[security]
min-age-days = 7
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.security.min_age_days, 7);
    }

    #[test]
    fn security_roundtrip() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[security]
min-age-days = 14
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(m2.security.min_age_days, 14);
    }

    #[test]
    fn security_zero_not_serialized() {
        // min_age_days = 0 (default) should not emit a [security] section
        let m = Manifest::parse(MINIMAL).unwrap();
        let toml = m.to_toml();
        assert!(
            !toml.contains("[security]"),
            "default security should not be emitted"
        );
    }

    #[test]
    fn security_negative_value_returns_error() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[security]
min-age-days = -1
"#;
        assert!(Manifest::parse(content).is_err());
    }

    // --- dependency policy (#637) ---

    #[test]
    fn parse_default_dependency_policy() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert_eq!(m.dependency_policy.complexity_threshold, 1000);
        assert!(m.dependency_policy.rationale_required);
    }

    #[test]
    fn parse_custom_dependency_policy() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependency-policy]
complexity-threshold = 500
rationale-required = false
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.dependency_policy.complexity_threshold, 500);
        assert!(!m.dependency_policy.rationale_required);
    }

    #[test]
    fn dependency_policy_roundtrip() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependency-policy]
complexity-threshold = 2000
rationale-required = false
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(m2.dependency_policy.complexity_threshold, 2000);
        assert!(!m2.dependency_policy.rationale_required);
    }

    #[test]
    fn default_policy_not_serialized() {
        let m = Manifest::parse(MINIMAL).unwrap();
        let toml = m.to_toml();
        assert!(!toml.contains("[dependency-policy]"));
    }

    // --- TOML parser: booleans and integers ---

    #[test]
    fn parse_toml_boolean_values() {
        let content = "[section]\nflag = true\nother = false\n";
        let table = parse_toml_table(content).unwrap();
        let sec = table.get("section").unwrap().as_table().unwrap();
        assert_eq!(sec.get("flag").unwrap().as_bool(), Some(true));
        assert_eq!(sec.get("other").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn parse_toml_integer_values() {
        let content = "[section]\ncount = 42\n";
        let table = parse_toml_table(content).unwrap();
        let sec = table.get("section").unwrap().as_table().unwrap();
        assert_eq!(sec.get("count").unwrap().as_integer(), Some(42));
    }

    // --- [c-native] parsing (#633) ---

    #[test]
    fn parse_c_native_section_test() {
        let m = Manifest::parse(WITH_C_NATIVE).unwrap();
        assert_eq!(m.c_native.len(), 2);
        assert_eq!(
            m.c_native.get("libz").map(|s| s.version.as_str()),
            Some("1.3")
        );
        assert_eq!(
            m.c_native.get("openssl").map(|s| s.version.as_str()),
            Some("3.0")
        );
        // Bare strings have no license
        assert!(m.c_native.get("libz").unwrap().license.is_none());
    }

    #[test]
    fn c_native_empty_when_absent() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert!(m.c_native.is_empty());
    }

    #[test]
    fn c_native_roundtrip() {
        let m = Manifest::parse(WITH_C_NATIVE).unwrap();
        let toml = m.to_toml();
        assert!(toml.contains("[c-native]"));
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(m2.c_native.len(), 2);
        assert_eq!(
            m2.c_native.get("libz").map(|s| s.version.as_str()),
            Some("1.3")
        );
        assert_eq!(
            m2.c_native.get("openssl").map(|s| s.version.as_str()),
            Some("3.0")
        );
    }

    #[test]
    fn c_native_with_inline_table_version() {
        let content = r#"
[package]
name = "foo"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[c-native]
libz = { version = "1.3" }
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(
            m.c_native.get("libz").map(|s| s.version.as_str()),
            Some("1.3")
        );
        assert!(m.c_native.get("libz").unwrap().license.is_none());
    }

    #[test]
    fn c_native_with_license_field() {
        let content = r#"
[package]
name = "foo"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[c-native]
libz = { version = "1.3", license = "Zlib" }
openssl = { version = "3.0", license = "Apache-2.0" }
libc = "0.2"
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.c_native.len(), 3);
        assert_eq!(
            m.c_native.get("libz").unwrap().license.as_deref(),
            Some("Zlib")
        );
        assert_eq!(
            m.c_native.get("openssl").unwrap().license.as_deref(),
            Some("Apache-2.0")
        );
        assert!(m.c_native.get("libc").unwrap().license.is_none());
    }

    #[test]
    fn c_native_license_roundtrip() {
        let content = r#"
[package]
name = "foo"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[c-native]
libz = { version = "1.3", license = "Zlib" }
libc = "0.2"
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        assert!(toml.contains("license = \"Zlib\""));
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(
            m2.c_native.get("libz").unwrap().license.as_deref(),
            Some("Zlib")
        );
        assert!(m2.c_native.get("libc").unwrap().license.is_none());
    }

    #[test]
    fn new_project_has_empty_c_native() {
        let m = Manifest::new_project("app", "1.0.0");
        assert!(m.c_native.is_empty());
    }

    // --- license policy (#635) ---

    #[test]
    fn parse_default_license_policy() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert_eq!(m.license_policy.mode, LicensePolicyMode::Permissive);
        assert!(m.license_policy.allow.is_empty());
        assert!(m.license_policy.deny.is_empty());
    }

    #[test]
    fn parse_custom_license_policy() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[license-policy]
mode = "custom"
allow = ["MIT", "Apache-2.0", "BSD-3-Clause"]
deny = ["GPL-3.0-only"]
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.license_policy.mode, LicensePolicyMode::Custom);
        assert_eq!(
            m.license_policy.allow,
            vec!["MIT", "Apache-2.0", "BSD-3-Clause"]
        );
        assert_eq!(m.license_policy.deny, vec!["GPL-3.0-only"]);
    }

    #[test]
    fn parse_copyleft_ok_license_policy() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[license-policy]
mode = "copyleft-ok"
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.license_policy.mode, LicensePolicyMode::CopyleftOk);
    }

    #[test]
    fn parse_any_license_policy() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[license-policy]
mode = "any"
"#;
        let m = Manifest::parse(content).unwrap();
        assert_eq!(m.license_policy.mode, LicensePolicyMode::Any);
    }

    #[test]
    fn license_policy_invalid_mode_returns_error() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[license-policy]
mode = "strict"
"#;
        let err = Manifest::parse(content).unwrap_err();
        assert!(matches!(err, ManifestError::ParseError(ref s) if s.contains("strict")));
    }

    #[test]
    fn license_policy_roundtrip() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[license-policy]
mode = "custom"
allow = ["MIT", "Apache-2.0"]
deny = ["GPL-3.0-only"]
"#;
        let m = Manifest::parse(content).unwrap();
        let toml = m.to_toml();
        let m2 = Manifest::parse(&toml).unwrap();
        assert_eq!(m2.license_policy.mode, LicensePolicyMode::Custom);
        assert_eq!(m2.license_policy.allow, vec!["MIT", "Apache-2.0"]);
        assert_eq!(m2.license_policy.deny, vec!["GPL-3.0-only"]);
    }

    #[test]
    fn default_license_policy_not_serialized() {
        let m = Manifest::parse(MINIMAL).unwrap();
        let toml = m.to_toml();
        assert!(!toml.contains("[license-policy]"));
    }

    // --- LicensePolicy::check ---

    #[test]
    fn permissive_policy_allows_mit() {
        let policy = LicensePolicy::default();
        assert!(policy.check("MIT").is_ok());
    }

    #[test]
    fn permissive_policy_allows_apache() {
        let policy = LicensePolicy::default();
        assert!(policy.check("Apache-2.0").is_ok());
    }

    #[test]
    fn permissive_policy_rejects_gpl() {
        let policy = LicensePolicy::default();
        assert!(policy.check("GPL-3.0-only").is_err());
    }

    #[test]
    fn permissive_policy_allows_or_expression_with_permissive_alt() {
        let policy = LicensePolicy::default();
        assert!(policy.check("MIT OR Apache-2.0").is_ok());
        assert!(policy.check("GPL-3.0-only OR MIT").is_ok());
    }

    #[test]
    fn permissive_policy_rejects_all_copyleft_or() {
        let policy = LicensePolicy::default();
        assert!(policy.check("GPL-3.0-only OR AGPL-3.0-only").is_err());
    }

    #[test]
    fn copyleft_ok_policy_allows_gpl() {
        let policy = LicensePolicy {
            mode: LicensePolicyMode::CopyleftOk,
            allow: vec![],
            deny: vec![],
        };
        assert!(policy.check("GPL-3.0-only").is_ok());
        assert!(policy.check("MIT").is_ok());
    }

    #[test]
    fn any_policy_allows_everything() {
        let policy = LicensePolicy {
            mode: LicensePolicyMode::Any,
            allow: vec![],
            deny: vec![],
        };
        assert!(policy.check("GPL-3.0-only").is_ok());
        assert!(policy.check("UNKNOWN-LICENSE").is_ok());
    }

    #[test]
    fn custom_policy_only_allows_listed() {
        let policy = LicensePolicy {
            mode: LicensePolicyMode::Custom,
            allow: vec!["MIT".to_string(), "ISC".to_string()],
            deny: vec![],
        };
        assert!(policy.check("MIT").is_ok());
        assert!(policy.check("ISC").is_ok());
        assert!(policy.check("Apache-2.0").is_err());
    }

    #[test]
    fn deny_list_overrides_mode() {
        let policy = LicensePolicy {
            mode: LicensePolicyMode::Permissive,
            allow: vec![],
            deny: vec!["MIT".to_string()],
        };
        // MIT is normally permissive, but explicitly denied
        assert!(policy.check("MIT").is_err());
    }

    #[test]
    fn allow_list_extends_mode() {
        let policy = LicensePolicy {
            mode: LicensePolicyMode::Permissive,
            allow: vec!["CUSTOM-1.0".to_string()],
            deny: vec![],
        };
        // Custom license not in permissive list, but explicitly allowed
        assert!(policy.check("CUSTOM-1.0").is_ok());
    }

    // --- TOML parser: string arrays ---

    #[test]
    fn parse_toml_string_array() {
        let content = "[section]\nitems = [\"a\", \"b\", \"c\"]\n";
        let table = parse_toml_table(content).unwrap();
        let sec = table.get("section").unwrap().as_table().unwrap();
        let arr = sec.get("items").unwrap().as_string_array().unwrap();
        assert_eq!(arr, &["a", "b", "c"]);
    }

    #[test]
    fn parse_toml_empty_array() {
        let content = "[section]\nitems = []\n";
        let table = parse_toml_table(content).unwrap();
        let sec = table.get("section").unwrap().as_table().unwrap();
        let arr = sec.get("items").unwrap().as_string_array().unwrap();
        assert!(arr.is_empty());
    }

    // --- audit_dep_rationale (#637) ---

    #[test]
    fn audit_reports_missing_rationale() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/ring", tag = "v0.17.8", rationale = "Crypto" }
uuid = { git = "https://github.com/uuid", tag = "v1.0.0" }
"#;
        let m = Manifest::parse(content).unwrap();
        let missing = m.audit_dep_rationale();
        assert_eq!(missing, vec!["uuid"]);
    }

    #[test]
    fn audit_passes_when_all_have_rationale() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
ring = { git = "https://github.com/ring", tag = "v0.17.8", rationale = "Crypto" }
"#;
        let m = Manifest::parse(content).unwrap();
        assert!(m.audit_dep_rationale().is_empty());
    }

    #[test]
    fn audit_skipped_when_policy_disabled() {
        let content = r#"
[package]
name = "my-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
uuid = { git = "https://github.com/uuid", tag = "v1.0.0" }

[dependency-policy]
rationale-required = false
"#;
        let m = Manifest::parse(content).unwrap();
        assert!(m.audit_dep_rationale().is_empty());
    }

    #[test]
    fn audit_empty_deps_passes() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert!(m.audit_dep_rationale().is_empty());
    }

    #[test]
    fn missing_dep_rationale_error_display() {
        let e =
            ManifestError::MissingDepRationale(vec!["uuid".to_string(), "left-pad".to_string()]);
        let s = e.to_string();
        assert!(s.contains("E701"));
        assert!(s.contains("uuid"));
        assert!(s.contains("left-pad"));
    }

    // --- validate_license ---

    #[test]
    fn validate_license_passes_when_file_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mvl.toml"), MINIMAL).unwrap();
        std::fs::write(
            tmp.path().join("LICENSE"),
            "MIT License\n\nPermission is hereby granted...",
        )
        .unwrap();
        let m = Manifest::load(tmp.path()).unwrap();
        assert!(m.validate_license(tmp.path()).is_ok());
    }

    #[test]
    fn validate_license_fails_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mvl.toml"), MINIMAL).unwrap();
        let m = Manifest::load(tmp.path()).unwrap();
        let err = m.validate_license(tmp.path()).unwrap_err();
        assert!(matches!(err, ManifestError::LicenseMismatch(_)));
        assert!(err.to_string().contains("E702"));
        assert!(err.to_string().contains("no LICENSE file"));
    }

    #[test]
    fn validate_license_fails_on_content_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("mvl.toml"), MINIMAL).unwrap();
        // MINIMAL says license = "MIT" but we write Apache content
        std::fs::write(
            tmp.path().join("LICENSE"),
            "Apache License\nVersion 2.0, January 2004",
        )
        .unwrap();
        let m = Manifest::load(tmp.path()).unwrap();
        let err = m.validate_license(tmp.path()).unwrap_err();
        assert!(matches!(err, ManifestError::LicenseMismatch(_)));
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn validate_license_apache2_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let toml = MINIMAL.replace("MIT", "Apache-2.0");
        std::fs::write(tmp.path().join("mvl.toml"), &toml).unwrap();
        std::fs::write(
            tmp.path().join("LICENSE"),
            "Apache License\nVersion 2.0, January 2004\n...",
        )
        .unwrap();
        let m = Manifest::load(tmp.path()).unwrap();
        assert!(m.validate_license(tmp.path()).is_ok());
    }

    // ── sync_manifest_tags / rewrite_tag_in_line (#1459) ─────────────────

    #[test]
    fn rewrite_tag_preserves_v_prefix() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"v0.2.3\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("tag = \"v0.3.0\""), "got: {out}");
    }

    #[test]
    fn rewrite_tag_no_v_prefix_kept() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"0.2.3\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("tag = \"0.3.0\""), "got: {out}");
        assert!(!out.contains("v0.3.0"));
    }

    #[test]
    fn rewrite_tag_preserves_rest_of_line() {
        let line = "\"pkg\" = { git = \"https://e.x/p\", tag = \"v0.2.3\", rationale = \"x\" }";
        let out = rewrite_tag_in_line(line, "0.3.0");
        assert!(out.contains("rationale = \"x\""), "got: {out}");
    }

    #[test]
    fn rewrite_tag_no_match_returns_unchanged() {
        let line = "\"pkg\" = \">=1.0.0\"";
        let out = rewrite_tag_in_line(line, "2.0.0");
        assert_eq!(out, line);
    }

    #[test]
    fn sync_manifest_tags_rewrites_only_matching_dep() {
        let tmp = tempfile::tempdir().unwrap();
        let original = "[package]\nname = \"p\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\nrequires-mvl = \">=0.1.0\"\n\n[dependencies]\n\"foo\" = { git = \"https://e.x/foo\", tag = \"v0.1.0\" }\n\"bar\" = { git = \"https://e.x/bar\", tag = \"v0.5.0\" }\n";
        std::fs::write(tmp.path().join("mvl.toml"), original).unwrap();
        let updates = vec![("foo".to_string(), "0.2.0".to_string())];
        sync_manifest_tags(tmp.path(), &updates).unwrap();
        let result = std::fs::read_to_string(tmp.path().join("mvl.toml")).unwrap();
        assert!(result.contains("\"foo\" = { git = \"https://e.x/foo\", tag = \"v0.2.0\" }"));
        assert!(result.contains("\"bar\" = { git = \"https://e.x/bar\", tag = \"v0.5.0\" }"));
    }
}
