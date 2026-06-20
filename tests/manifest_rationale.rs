// Integration tests for dependency rationale enforcement (#637).
//
// Validates that:
// 1. All example mvl.toml files with dependencies parse correctly
// 2. All dependencies have rationale when policy is enabled
// 3. The audit_dep_rationale() API works end-to-end
// 4. The root mvl.toml passes rationale audit

use mvl::mvl::packages::manifest::Manifest;
use std::path::Path;

/// Helper: load manifest from a directory and assert it parses.
fn load_manifest(dir: &str) -> Manifest {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    Manifest::load(&path).unwrap_or_else(|e| panic!("failed to load {dir}/mvl.toml: {e}"))
}

// ── Root manifest ────────────────────────────────────────────────────────────

#[test]
fn root_manifest_has_rationale_on_all_deps() {
    let m = load_manifest(".");
    let missing = m.audit_dep_rationale();
    assert!(
        missing.is_empty(),
        "root mvl.toml: deps missing rationale: {missing:?}"
    );
}

#[test]
fn root_manifest_parses_all_rationale_fields() {
    let m = load_manifest(".");
    for (name, spec) in &m.dependencies {
        assert!(
            spec.rationale().is_some(),
            "root dep '{name}' missing rationale"
        );
        let r = spec.rationale().unwrap();
        assert!(!r.is_empty(), "root dep '{name}' has empty rationale");
    }
}

// ── Example manifests with external packages ─────────────────────────────────

#[test]
fn actor_webserver_manifest_rationale() {
    let m = load_manifest("examples/actor_webserver");
    assert_eq!(m.package.name, "actor_webserver");
    assert_eq!(m.dependencies.len(), 1);
    let missing = m.audit_dep_rationale();
    assert!(
        missing.is_empty(),
        "actor_webserver: deps missing rationale: {missing:?}"
    );
}

#[test]
fn anthropic_chat_manifest_rationale() {
    let m = load_manifest("examples/anthropic_chat");
    assert_eq!(m.package.name, "anthropic_chat");
    assert_eq!(m.dependencies.len(), 1);
    let missing = m.audit_dep_rationale();
    assert!(
        missing.is_empty(),
        "anthropic_chat: deps missing rationale: {missing:?}"
    );
}

// ── Policy enforcement tests (synthetic manifests) ───────────────────────────

#[test]
fn audit_catches_missing_rationale() {
    let content = r#"
[package]
name = "test-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
"pkg-a" = { git = "https://example.com/a", tag = "v1.0.0", rationale = "Needed for X" }
"pkg-b" = { git = "https://example.com/b", tag = "v2.0.0" }
"#;
    let m = Manifest::parse(content).unwrap();
    let missing = m.audit_dep_rationale();
    assert_eq!(missing, vec!["pkg-b"]);
}

#[test]
fn audit_disabled_by_policy() {
    let content = r#"
[package]
name = "legacy-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
"pkg-a" = { git = "https://example.com/a", tag = "v1.0.0" }

[dependency-policy]
rationale-required = false
"#;
    let m = Manifest::parse(content).unwrap();
    assert!(!m.dependency_policy.rationale_required);
    assert!(m.audit_dep_rationale().is_empty());
}

#[test]
fn custom_complexity_threshold_parsed() {
    let content = r#"
[package]
name = "strict-app"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependency-policy]
complexity-threshold = 500
"#;
    let m = Manifest::parse(content).unwrap();
    assert_eq!(m.dependency_policy.complexity_threshold, 500);
    assert!(m.dependency_policy.rationale_required);
}

#[test]
fn rationale_survives_roundtrip() {
    let content = r#"
[package]
name = "roundtrip-test"
version = "0.1.0"
license = "MIT"
requires-mvl = ">=0.1.0"

[dependencies]
"pkg-a" = { git = "https://example.com/a", tag = "v1.0.0", rationale = "Critical crypto lib" }
"#;
    let m1 = Manifest::parse(content).unwrap();
    let toml = m1.to_toml();
    let m2 = Manifest::parse(&toml).unwrap();
    assert_eq!(
        m2.dependencies.get("pkg-a").unwrap().rationale(),
        Some("Critical crypto lib")
    );
    assert!(m2.audit_dep_rationale().is_empty());
}

// ── License validation ───────────────────────────────────────────────────────

fn manifest_dir(dir: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)
}

#[test]
fn root_license_matches_manifest() {
    let dir = manifest_dir(".");
    let m = Manifest::load(&dir).unwrap();
    assert!(m.validate_license(&dir).is_ok());
}

#[test]
fn all_examples_license_matches() {
    for name in &["actor_webserver", "anthropic_chat", "log_to_file"] {
        let dir = manifest_dir(&format!("examples/{name}"));
        let m = Manifest::load(&dir).unwrap();
        assert!(
            m.validate_license(&dir).is_ok(),
            "{name}: {}",
            m.validate_license(&dir).unwrap_err()
        );
    }
}

// ── Regression: examples without deps still pass ─────────────────────────────

#[test]
fn log_to_file_manifest_no_deps() {
    let m = load_manifest("examples/log_to_file");
    assert!(m.dependencies.is_empty());
    assert!(m.audit_dep_rationale().is_empty());
}
