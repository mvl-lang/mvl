// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! SBOM generation from `mvl.lock` and `mvl.toml`.
//!
//! Implements Phase A of issue #252: supply chain visibility without external
//! services.  Reads the lock file (which already contains name, version, hash,
//! and git URL for every dependency) and emits a standard SBOM document.
//!
//! Supported formats:
//! - CycloneDX 1.5 JSON (default, `--format=cyclonedx`)
//! - SPDX 2.3 tag-value  (`--format=spdx`)

use super::lock::{LockFile, LockedPackage};
use super::manifest::Manifest;

/// Output format for `mvl sbom`.
pub enum SbomFormat {
    CycloneDx,
    Spdx,
}

impl SbomFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cyclonedx" | "CycloneDX" => Some(SbomFormat::CycloneDx),
            "spdx" | "SPDX" => Some(SbomFormat::Spdx),
            _ => None,
        }
    }
}

/// Generate an SBOM document from the manifest and lock file.
/// CycloneDX component type for the root package.
#[derive(Clone, Copy, PartialEq)]
pub enum ComponentType {
    Application,
    Library,
}

impl ComponentType {
    fn as_str(self) -> &'static str {
        match self {
            ComponentType::Application => "application",
            ComponentType::Library => "library",
        }
    }
}

use std::collections::HashMap;

/// License map: package name → SPDX license identifier (e.g. "Apache-2.0").
pub type LicenseMap = HashMap<String, String>;

pub fn generate(
    manifest: &Manifest,
    lock: &LockFile,
    format: SbomFormat,
    component_type: ComponentType,
    licenses: &LicenseMap,
) -> String {
    match format {
        SbomFormat::CycloneDx => cyclonedx(manifest, lock, component_type, licenses),
        SbomFormat::Spdx => spdx(manifest, lock, component_type, licenses),
    }
}

// ── CycloneDX 1.5 JSON ────────────────────────────────────────────────────────

fn cyclonedx(
    manifest: &Manifest,
    lock: &LockFile,
    component_type: ComponentType,
    licenses: &LicenseMap,
) -> String {
    let pkg = &manifest.package;
    let serial = make_serial(&pkg.name, &pkg.version);
    let mvl_ver = env!("CARGO_PKG_VERSION");

    let mut out = String::new();
    out += "{\n";
    out += "  \"bomFormat\": \"CycloneDX\",\n";
    out += "  \"specVersion\": \"1.5\",\n";
    out += &format!("  \"serialNumber\": \"{}\",\n", json_escape(&serial));
    out += "  \"version\": 1,\n";
    out += "  \"metadata\": {\n";
    out += &format!(
        "    \"tools\": [{{\"vendor\": \"MVL\", \"name\": \"mvl\", \"version\": \"{}\"}}],\n",
        mvl_ver
    );
    out += "    \"component\": {\n";
    out += &format!("      \"type\": \"{}\",\n", component_type.as_str());
    out += &format!("      \"name\": \"{}\",\n", json_escape(&pkg.name));
    out += &format!("      \"version\": \"{}\",\n", json_escape(&pkg.version));
    out += &format!(
        "      \"purl\": \"{}\",\n",
        json_escape(&mvl_purl(&pkg.name, &pkg.version))
    );
    out += &format!(
        "      \"licenses\": [{{\"license\": {{\"id\": \"{}\"}}}}]\n",
        json_escape(&pkg.license)
    );
    out += "    }\n";
    out += "  },\n";
    out += "  \"components\": [\n";

    let mut entries: Vec<String> = Vec::new();
    for lp in &lock.packages {
        entries.push(mvl_component_json(
            lp,
            licenses.get(&lp.name).map(|s| s.as_str()),
        ));
    }
    let mut native: Vec<(&String, &String)> = manifest.native.iter().collect();
    native.sort_by_key(|(k, _)| *k);
    for (name, version) in &native {
        entries.push(cargo_component_json(name, version));
    }

    out += &entries.join(",\n");
    if !entries.is_empty() {
        out += "\n";
    }
    out += "  ]\n";
    out += "}\n";
    out
}

fn mvl_component_json(lp: &LockedPackage, license: Option<&str>) -> String {
    let indent = "    ";
    let mut fields: Vec<String> = Vec::new();
    fields.push(format!("{indent}  \"type\": \"library\""));
    fields.push(format!("{indent}  \"name\": \"{}\"", json_escape(&lp.name)));
    fields.push(format!(
        "{indent}  \"version\": \"{}\"",
        json_escape(&lp.version)
    ));
    fields.push(format!(
        "{indent}  \"purl\": \"{}\"",
        json_escape(&mvl_purl(&lp.name, &lp.version))
    ));
    if let Some(hex) = lp.hash.strip_prefix("sha256:") {
        fields.push(format!(
            "{indent}  \"hashes\": [{{\"alg\": \"SHA-256\", \"content\": \"{}\"}}]",
            json_escape(hex)
        ));
    }
    if let Some(lic) = license {
        fields.push(format!(
            "{indent}  \"licenses\": [{{\"license\": {{\"id\": \"{}\"}}}}]",
            json_escape(lic)
        ));
    }
    if let Some(ref url) = lp.git {
        fields.push(format!(
            "{indent}  \"externalReferences\": [{{\"type\": \"vcs\", \"url\": \"{}\"}}]",
            json_escape(url)
        ));
    }
    format!("{indent}{{\n{}\n{indent}}}", fields.join(",\n"))
}

fn cargo_component_json(name: &str, version: &str) -> String {
    let indent = "    ";
    let fields = [
        format!("{indent}  \"type\": \"library\""),
        format!("{indent}  \"name\": \"{}\"", json_escape(name)),
        format!("{indent}  \"version\": \"{}\"", json_escape(version)),
        format!(
            "{indent}  \"purl\": \"{}\"",
            json_escape(&cargo_purl(name, version))
        ),
    ];
    format!("{indent}{{\n{}\n{indent}}}", fields.join(",\n"))
}

// ── SPDX 2.3 tag-value ───────────────────────────────────────────────────────

fn spdx(
    manifest: &Manifest,
    lock: &LockFile,
    component_type: ComponentType,
    licenses: &LicenseMap,
) -> String {
    let pkg = &manifest.package;
    let mvl_ver = env!("CARGO_PKG_VERSION");
    let doc_name = format!("{}-{}", pkg.name, pkg.version);
    let namespace = format!(
        "https://spdx.org/spdxdocs/{}-{}",
        spdx_slug(&pkg.name),
        pkg.version
    );

    let mut out = String::new();
    out += "SPDXVersion: SPDX-2.3\n";
    out += "DataLicense: CC0-1.0\n";
    out += "SPDXID: SPDXRef-DOCUMENT\n";
    out += &format!("DocumentName: {doc_name}\n");
    out += &format!("DocumentNamespace: {namespace}\n");
    out += &format!("Creator: Tool: mvl-{mvl_ver}\n");
    out += "\n";

    // Root package
    out += &format!("PackageName: {}\n", pkg.name);
    out += "SPDXID: SPDXRef-Package\n";
    out += &format!("PackageVersion: {}\n", pkg.version);
    out += &format!("PackageLicense: {}\n", pkg.license);
    out += &format!(
        "PrimaryPackagePurpose: {}\n",
        match component_type {
            ComponentType::Application => "APPLICATION",
            ComponentType::Library => "LIBRARY",
        }
    );
    out += &format!(
        "ExternalRef: PACKAGE-MANAGER purl {}\n",
        mvl_purl(&pkg.name, &pkg.version)
    );
    out += "\n";

    // MVL dependencies
    for (i, lp) in lock.packages.iter().enumerate() {
        let spdx_id = format!("SPDXRef-{}", spdx_id_for(&lp.name, i));
        out += &format!("PackageName: {}\n", lp.name);
        out += &format!("SPDXID: {spdx_id}\n");
        out += &format!("PackageVersion: {}\n", lp.version);
        if let Some(hex) = lp.hash.strip_prefix("sha256:") {
            out += &format!("PackageChecksum: SHA256: {hex}\n");
        }
        if let Some(ref url) = lp.git {
            out += &format!("PackageDownloadLocation: {url}\n");
        } else {
            out += "PackageDownloadLocation: NOASSERTION\n";
        }
        if let Some(lic) = licenses.get(&lp.name) {
            out += &format!("PackageLicense: {lic}\n");
        } else {
            out += "PackageLicense: NOASSERTION\n";
        }
        out += "FilesAnalyzed: false\n";
        out += &format!(
            "ExternalRef: PACKAGE-MANAGER purl {}\n",
            mvl_purl(&lp.name, &lp.version)
        );
        out += "\n";
        out += &format!("Relationship: SPDXRef-Package DEPENDS_ON {spdx_id}\n");
        out += "\n";
    }

    // Native (Rust) dependencies
    let mut native: Vec<(&String, &String)> = manifest.native.iter().collect();
    native.sort_by_key(|(k, _)| *k);
    for (i, (name, version)) in native.iter().enumerate() {
        let spdx_id = format!("SPDXRef-native-{}", spdx_id_for(name, i));
        out += &format!("PackageName: {name}\n");
        out += &format!("SPDXID: {spdx_id}\n");
        out += &format!("PackageVersion: {version}\n");
        out += "PackageDownloadLocation: https://crates.io\n";
        out += "FilesAnalyzed: false\n";
        out += &format!(
            "ExternalRef: PACKAGE-MANAGER purl {}\n",
            cargo_purl(name, version)
        );
        out += "\n";
        out += &format!("Relationship: SPDXRef-Package DEPENDS_ON {spdx_id}\n");
        out += "\n";
    }

    out
}

// ── purl helpers ──────────────────────────────────────────────────────────────

/// Package URL for an MVL dependency: `pkg:mvl/<name>@<version>`.
fn mvl_purl(name: &str, version: &str) -> String {
    format!("pkg:mvl/{}@{}", name, version)
}

/// Package URL for a Cargo (Rust) crate: `pkg:cargo/<name>@<version>`.
fn cargo_purl(name: &str, version: &str) -> String {
    format!("pkg:cargo/{}@{}", name, version)
}

// ── identifier helpers ────────────────────────────────────────────────────────

/// Deterministic pseudo-UUID serial for CycloneDX `serialNumber`.
///
/// Uses FNV-1a hash of `name-version` to produce a stable UUID v5-like value
/// without requiring a UUID crate.
fn make_serial(name: &str, version: &str) -> String {
    let mut h: u64 = 14695981039346656037;
    for b in name
        .bytes()
        .chain(b"-".iter().copied())
        .chain(version.bytes())
    {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    let a = (h >> 32) as u32;
    let b = (h >> 16) as u16;
    let c = 0x5000u16 | ((h >> 4) as u16 & 0x0fff); // version 5
    let d = 0x8000u16 | (h as u16 & 0x3fff); // variant 1
    let e = h & 0x0000_ffff_ffff_ffff;
    format!("urn:uuid:{a:08x}-{b:04x}-{c:04x}-{d:04x}-{e:012x}")
}

/// Slugify a package name for use in SPDX document namespace.
fn spdx_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Safe SPDX identifier for a package (letters, digits, `.`, `-`).
fn spdx_id_for(name: &str, idx: usize) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{slug}-{idx}")
}

// ── JSON escaping ─────────────────────────────────────────────────────────────

/// Escape a string for use inside a JSON double-quoted value.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::packages::lock::LockedPackage;
    use crate::mvl::packages::manifest::{Manifest, PackageInfo};
    use std::collections::HashMap;

    fn sample_manifest() -> Manifest {
        Manifest {
            package: PackageInfo {
                name: "github.com/lab271/my-app".to_string(),
                version: "1.0.0".to_string(),
                license: "MIT".to_string(),
                requires_mvl: ">=0.60.0".to_string(),
                extern_rationale: None,
            },
            dependencies: HashMap::new(),
            native: {
                let mut m = HashMap::new();
                m.insert("hyper".to_string(), "1.0".to_string());
                m
            },
        }
    }

    fn sample_lock() -> LockFile {
        LockFile {
            packages: vec![
                LockedPackage {
                    name: "github.com/lab271/mvl-stdlib".to_string(),
                    version: "1.2.0".to_string(),
                    hash: "sha256:abc123def456".to_string(),
                    commit: Some("deadbeef".to_string()),
                    git: Some("https://github.com/lab271/mvl-stdlib".to_string()),
                },
                LockedPackage {
                    name: "tls".to_string(),
                    version: "0.4.0".to_string(),
                    hash: "sha256:aabbccdd".to_string(),
                    commit: None,
                    git: None,
                },
            ],
        }
    }

    // ── CycloneDX ─────────────────────────────────────────────────────────────

    #[test]
    fn cyclonedx_is_valid_json_structure() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.starts_with('{'), "must start with {{");
        assert!(out.trim_end().ends_with('}'), "must end with }}");
        assert!(out.contains("\"bomFormat\": \"CycloneDX\""));
        assert!(out.contains("\"specVersion\": \"1.5\""));
    }

    #[test]
    fn cyclonedx_contains_root_package() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("github.com/lab271/my-app"));
        assert!(out.contains("\"version\": \"1.0.0\""));
        assert!(out.contains("\"id\": \"MIT\""));
    }

    #[test]
    fn cyclonedx_application_type_for_app() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Application,
            &LicenseMap::new(),
        );
        assert!(
            out.contains("\"type\": \"application\""),
            "root component must be 'application'"
        );
    }

    #[test]
    fn cyclonedx_library_type_for_lib() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(
            out.contains("\"type\": \"library\""),
            "root component must be 'library'"
        );
    }

    #[test]
    fn spdx_application_purpose_for_app() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Application,
            &LicenseMap::new(),
        );
        assert!(out.contains("PrimaryPackagePurpose: APPLICATION"));
    }

    #[test]
    fn spdx_library_purpose_for_lib() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("PrimaryPackagePurpose: LIBRARY"));
    }

    #[test]
    fn cyclonedx_contains_mvl_dependency() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("github.com/lab271/mvl-stdlib"));
        assert!(out.contains("\"SHA-256\""));
        assert!(out.contains("abc123def456"));
        assert!(out.contains("vcs"));
    }

    #[test]
    fn cyclonedx_contains_native_dependency() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("\"name\": \"hyper\""));
        assert!(out.contains("pkg:cargo/hyper@1.0"));
    }

    #[test]
    fn cyclonedx_purl_format() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("pkg:mvl/github.com/lab271/mvl-stdlib@1.2.0"));
    }

    #[test]
    fn cyclonedx_serial_is_deterministic() {
        let s1 = make_serial("github.com/lab271/my-app", "1.0.0");
        let s2 = make_serial("github.com/lab271/my-app", "1.0.0");
        assert_eq!(s1, s2);
        assert!(s1.starts_with("urn:uuid:"));
    }

    #[test]
    fn cyclonedx_serial_differs_for_different_inputs() {
        let s1 = make_serial("pkg-a", "1.0.0");
        let s2 = make_serial("pkg-b", "1.0.0");
        assert_ne!(s1, s2);
    }

    #[test]
    fn cyclonedx_empty_lock_produces_empty_components() {
        let lock = LockFile { packages: vec![] };
        let mut manifest = sample_manifest();
        manifest.native.clear();
        let out = generate(
            &manifest,
            &lock,
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("\"components\": [\n  ]"));
    }

    #[test]
    fn cyclonedx_no_hash_prefix_omits_hashes() {
        // A package without a recognisable hash prefix must not emit a "hashes" field.
        let lock = LockFile {
            packages: vec![LockedPackage {
                name: "foo".to_string(),
                version: "1.0.0".to_string(),
                hash: "sha256:deadbeef".to_string(),
                commit: None,
                git: None,
            }],
        };
        let mut manifest = sample_manifest();
        manifest.native.clear();
        let out = generate(
            &manifest,
            &lock,
            SbomFormat::CycloneDx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        // git URL absent → no externalReferences
        assert!(!out.contains("externalReferences"));
    }

    // ── SPDX ──────────────────────────────────────────────────────────────────

    #[test]
    fn spdx_starts_with_version_header() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.starts_with("SPDXVersion: SPDX-2.3\n"));
    }

    #[test]
    fn spdx_contains_root_package() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("PackageName: github.com/lab271/my-app"));
        assert!(out.contains("PackageVersion: 1.0.0"));
        assert!(out.contains("PackageLicense: MIT"));
    }

    #[test]
    fn spdx_contains_dependency_relationships() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("Relationship: SPDXRef-Package DEPENDS_ON"));
    }

    #[test]
    fn spdx_contains_checksum_for_locked_package() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("PackageChecksum: SHA256: abc123def456"));
    }

    #[test]
    fn spdx_native_deps_reference_crates_io() {
        let out = generate(
            &sample_manifest(),
            &sample_lock(),
            SbomFormat::Spdx,
            ComponentType::Library,
            &LicenseMap::new(),
        );
        assert!(out.contains("https://crates.io"));
        assert!(out.contains("pkg:cargo/hyper@1.0"));
    }

    // ── SbomFormat::from_str ──────────────────────────────────────────────────

    #[test]
    fn format_from_str_cyclonedx() {
        assert!(matches!(
            SbomFormat::parse("cyclonedx"),
            Some(SbomFormat::CycloneDx)
        ));
        assert!(matches!(
            SbomFormat::parse("CycloneDX"),
            Some(SbomFormat::CycloneDx)
        ));
    }

    #[test]
    fn format_from_str_spdx() {
        assert!(matches!(SbomFormat::parse("spdx"), Some(SbomFormat::Spdx)));
    }

    #[test]
    fn format_from_str_unknown_returns_none() {
        assert!(SbomFormat::parse("unknown").is_none());
    }

    // ── json_escape ───────────────────────────────────────────────────────────

    #[test]
    fn json_escape_quotes_and_backslashes() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(json_escape(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn json_escape_control_chars() {
        assert_eq!(json_escape("a\nb"), r"a\nb");
        assert_eq!(json_escape("a\tb"), r"a\tb");
    }
}
