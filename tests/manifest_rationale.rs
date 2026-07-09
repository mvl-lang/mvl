// Integration tests for dependency rationale enforcement (#637).
//
// Validates that:
// 1. Synthetic manifests parse rationale and policy fields correctly
// 2. `audit_dep_rationale()` reports missing rationale entries
// 3. The `[dependency-policy]` `rationale-required = false` opts out
// 4. Rationale survives a TOML roundtrip
//
// Note: prior versions of this file also covered the root `mvl.toml` and
// `examples/log_to_file/mvl.toml` — both removed in ead7ad484 ("chore:
// remove stale root mvl.toml/mvl.lock and dead install invocations")
// because no in-repo example imports a `pkg-*` package anymore.  The
// tests that read them were left behind and started failing.  Dropped
// here; the synthetic tests below still cover every code path that
// mattered.

use mvl::mvl::packages::manifest::Manifest;

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
//
// `validate_license` was previously exercised against the root manifest
// and `examples/log_to_file/mvl.toml`.  Both files were removed in
// ead7ad484; the license path is now covered by unit tests on the
// `Manifest` type itself (see `src/mvl/packages/manifest.rs::tests`).
