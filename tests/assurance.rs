//! E2E tests for `mvl assurance` — Spec 023.
//!
//! Requirement 1: Per-module verdicts (9+ proven on hello_world)
//! Requirement 2: Aggregate summary (functions, types, counts)
//! Requirement 3: JSON output (--json produces valid JSON)
//! Requirement 4: Verbose mode (--verbose shows per-requirement detail)
//! Requirement 5: Verdict caching (tested at unit level in passes.rs)
//!
//! Issue: #1233

use std::process::Command;

fn mvl_bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_mvl"))
}

fn corpus(name: &str) -> String {
    format!("{}/examples/programs/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn run_assurance(file: &str, extra_args: &[&str]) -> std::process::Output {
    Command::new(mvl_bin())
        .arg("assurance")
        .arg(file)
        .args(extra_args)
        .output()
        .expect("failed to run mvl assurance")
}

// ── Requirement 1: Per-Module Requirement Verdicts ───────────────────────────

#[test]
fn assurance_shows_requirement_verdicts() {
    let out = run_assurance(&corpus("hello_world.mvl"), &[]);
    assert!(out.status.success(), "mvl assurance failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Requirements verified:"),
        "expected requirement verdicts in output:\n{stdout}"
    );
    assert!(
        stdout.contains("proven"),
        "expected 'proven' in output:\n{stdout}"
    );
}

// ── Requirement 2: Aggregate Summary ─────────────────────────────────────────

#[test]
fn assurance_shows_aggregate_summary() {
    let out = run_assurance(&corpus("hello_world.mvl"), &[]);
    assert!(out.status.success(), "mvl assurance failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Functions:"),
        "expected function count in output:\n{stdout}"
    );
    assert!(
        stdout.contains("Files checked:"),
        "expected file count in output:\n{stdout}"
    );
}

// ── Requirement 3: JSON Output ───────────────────────────────────────────────

#[test]
fn assurance_json_is_valid() {
    let out = run_assurance(&corpus("hello_world.mvl"), &["--json"]);
    assert!(out.status.success(), "mvl assurance --json failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let trimmed = stdout.trim();
    assert!(
        trimmed.starts_with('{') && trimmed.ends_with('}'),
        "JSON output must be a top-level object: {stdout}"
    );
    // Validate structural correctness: balanced braces
    let open = trimmed.chars().filter(|&c| c == '{').count();
    let close = trimmed.chars().filter(|&c| c == '}').count();
    assert_eq!(open, close, "unbalanced braces in JSON output:\n{stdout}");
    assert!(
        stdout.contains("\"functions\""),
        "JSON must contain functions field:\n{stdout}"
    );
    assert!(
        stdout.contains("\"requirements\""),
        "JSON must contain requirements field:\n{stdout}"
    );
}

// ── Requirement 4: Verbose Mode ──────────────────────────────────────────────

#[test]
fn assurance_verbose_shows_per_requirement_detail() {
    let out = run_assurance(&corpus("hello_world.mvl"), &["--verbose"]);
    assert!(out.status.success(), "mvl assurance --verbose failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Req  1"),
        "verbose must show Req 1 detail:\n{stdout}"
    );
    assert!(
        stdout.contains("Req 11") || stdout.contains("Req 10"),
        "verbose must show later requirements:\n{stdout}"
    );
}

// ── Cross-requirement: programs with errors ──────────────────────────────────

#[test]
fn assurance_multi_file_programs() {
    let out = run_assurance(&corpus("calculator.mvl"), &[]);
    assert!(
        out.status.success(),
        "mvl assurance on calculator.mvl failed"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Functions:"),
        "expected function summary:\n{stdout}"
    );
}

#[test]
fn assurance_json_requirements_array() {
    let out = run_assurance(&corpus("calculator.mvl"), &["--json"]);
    assert!(out.status.success(), "mvl assurance --json failed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Find the requirements object and verify all 11 keys are present
    assert!(
        stdout.contains("\"requirements\""),
        "JSON must contain requirements object:\n{stdout}"
    );
    let req_start = stdout.find("\"requirements\"").unwrap();
    let req_section = &stdout[req_start..];
    for i in 1..=11 {
        let key = format!("\"{i}\":");
        assert!(
            req_section.contains(&key),
            "requirements object missing key {i}:\n{req_section}"
        );
    }
}
