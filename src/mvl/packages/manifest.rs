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

/// A dependency specification.
#[derive(Debug, Clone)]
pub enum DepSpec {
    /// Version constraint string: `">=1.0.0, <2.0.0"`.
    Version(String),
    /// Git dependency with a tag: `{ git = "...", tag = "v1.2.0" }`.
    Git { git: String, tag: String },
}

impl DepSpec {
    /// Return the declared version/tag string for display.
    pub fn version_str(&self) -> &str {
        match self {
            DepSpec::Version(v) => v,
            DepSpec::Git { tag, .. } => tag,
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
        let native = parse_native(table.get("native"))?;

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
                    DepSpec::Git { git, tag } => {
                        out.push_str(&format!(
                            "\"{}\" = {{ git = \"{}\", tag = \"{}\" }}\n",
                            name,
                            toml_escape(git),
                            toml_escape(tag)
                        ));
                    }
                }
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
        }
    }
}

// ── Minimal TOML parser ────────────────────────────────────────────────────

/// A minimal TOML value sufficient for mvl.toml parsing.
#[derive(Debug, Clone)]
enum TomlValue {
    String(String),
    Table(TomlTable),
    // Inline table: { git = "...", tag = "..." }
}

type TomlTable = HashMap<String, TomlValue>;

impl TomlValue {
    fn as_str(&self) -> Option<&str> {
        if let TomlValue::String(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }

    fn as_table(&self) -> Option<&TomlTable> {
        if let TomlValue::Table(t) = self {
            Some(t)
        } else {
            None
        }
    }
}

/// Parse a TOML document into a nested table structure.
/// This is a minimal parser that handles:
/// - `[section]` headers
/// - `key = "value"` string assignments
/// - `key = { key2 = "value" }` inline tables
fn parse_toml_table(content: &str) -> Result<TomlTable, String> {
    let mut root: TomlTable = HashMap::new();
    let mut current_section: Option<String> = None;

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = strip_comment(raw_line).trim().to_string();

        if line.is_empty() {
            continue;
        }

        // Section header: [section] or [[section]]
        if line.starts_with('[') && !line.starts_with("[[") {
            let inner = line.trim_start_matches('[').trim_end_matches(']').trim();
            current_section = Some(inner.to_string());
            // Ensure section exists as a Table
            let tbl = root
                .entry(inner.to_string())
                .or_insert_with(|| TomlValue::Table(HashMap::new()));
            if tbl.as_table().is_none() {
                return Err(format!(
                    "line {}: section '{inner}' conflicts with scalar",
                    line_num + 1
                ));
            }
            continue;
        }

        // Key = value assignment
        if let Some(eq_pos) = line.find('=') {
            let raw_key = line[..eq_pos].trim();
            let key = unquote_key(raw_key);
            let raw_val = line[eq_pos + 1..].trim();
            let value = parse_value(raw_val, line_num + 1)?;

            if let Some(ref section) = current_section {
                let tbl = root
                    .entry(section.clone())
                    .or_insert_with(|| TomlValue::Table(HashMap::new()));
                if let TomlValue::Table(ref mut t) = tbl {
                    t.insert(key, value);
                }
            } else {
                root.insert(key, value);
            }
        }
    }

    Ok(root)
}

fn strip_comment(s: &str) -> &str {
    // Strip # comments but not inside strings
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_str {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if c == '#' && !in_str {
            return &s[..i];
        }
    }
    s
}

fn unquote_key(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn parse_value(s: &str, line: usize) -> Result<TomlValue, String> {
    let s = s.trim();
    // Quoted string
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return Ok(TomlValue::String(unescape_string(&s[1..s.len() - 1])));
    }
    // Inline table: { key = "val", ... }
    if s.starts_with('{') && s.ends_with('}') {
        let inner = s[1..s.len() - 1].trim();
        let mut tbl: TomlTable = HashMap::new();
        // Split on commas that are not inside strings
        for part in split_on_comma(inner) {
            let part = part.trim();
            if let Some(eq) = part.find('=') {
                let k = unquote_key(part[..eq].trim());
                let v_str = part[eq + 1..].trim();
                tbl.insert(k, parse_value(v_str, line)?);
            }
        }
        return Ok(TomlValue::Table(tbl));
    }
    Err(format!("line {line}: unsupported TOML value: {s:?}"))
}

fn split_on_comma(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_str = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && in_str {
            current.push(c);
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            current.push(c);
            continue;
        }
        if c == ',' && !in_str {
            parts.push(current.clone());
            current.clear();
            continue;
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_dependencies(
    value: Option<&TomlValue>,
) -> Result<HashMap<String, DepSpec>, ManifestError> {
    let mut deps = HashMap::new();
    let tbl = match value {
        None => return Ok(deps),
        Some(v) => v.as_table().ok_or_else(|| {
            ManifestError::ParseError("[dependencies] must be a table".to_string())
        })?,
    };
    for (name, val) in tbl {
        let spec = match val {
            TomlValue::String(s) => DepSpec::Version(s.clone()),
            TomlValue::Table(t) => {
                let git = t
                    .get("git")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ManifestError::ParseError(format!("dep '{name}': missing 'git'"))
                    })?
                    .to_string();
                let tag = t
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ManifestError::ParseError(format!("dep '{name}': missing 'tag'"))
                    })?
                    .to_string();
                DepSpec::Git { git, tag }
            }
        };
        deps.insert(name.clone(), spec);
    }
    Ok(deps)
}

fn parse_native(value: Option<&TomlValue>) -> Result<HashMap<String, String>, ManifestError> {
    let mut native = HashMap::new();
    let tbl = match value {
        None => return Ok(native),
        Some(v) => v
            .as_table()
            .ok_or_else(|| ManifestError::ParseError("[native] must be a table".to_string()))?,
    };
    for (name, val) in tbl {
        let version = val
            .as_str()
            .ok_or_else(|| {
                ManifestError::ParseError(format!("native dep '{name}' must be a string"))
            })?
            .to_string();
        native.insert(name.clone(), version);
    }
    Ok(native)
}

#[cfg(test)]
mod tests {
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
            DepSpec::Git { git, tag } => {
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
}
