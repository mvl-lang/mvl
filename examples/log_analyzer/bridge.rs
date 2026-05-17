//! bridge.rs — Rust implementation of the `extern "rust"` trust boundary
//! declared in main.mvl.
//!
//! This bridge contains only the domain-specific pipeline logic that cannot
//! yet be expressed in MVL (blocked by lambda/iterator support; no tracking issue yet).
//!
//! Generic infrastructure (file I/O, CLI argument parsing) has been moved to
//! the MVL standard library (`std.io`, `std.args`) and no longer requires a
//! per-program bridge.
//!
//! Compile with: `mvl build examples/log_analyzer/main.mvl`
//! (bridge.rs is detected automatically and linked in)

use mvl_runtime::prelude::*;

// Types generated from main.mvl — accessible as `crate::*` in a binary crate.
use crate::PipelineError;

// ── Trust boundary implementation ─────────────────────────────────────────

/// Parse JSONL content line-by-line, optionally filter by log level, and
/// return a compact JSON report string.
///
/// Each line is expected to be:
///   {"level": "info", "message": "...", "timestamp": 1234567890}
#[no_mangle]
pub extern "Rust" fn analyze_and_format(
    content: Tainted<String>,
    level: Option<String>,
) -> Result<String, PipelineError> {
    let filter = level.as_ref().map(|l| l.to_lowercase());
    let (mut count, mut errors, mut warnings, mut infos) = (0i64, 0i64, 0i64, 0i64);

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lvl = extract_str_field(line, "level").unwrap_or_default();
        if let Some(ref f) = filter {
            if &lvl != f {
                continue;
            }
        }
        count += 1;
        match lvl.as_str() {
            "error" => errors += 1,
            "warn" => warnings += 1,
            "info" => infos += 1,
            _ => {}
        }
    }

    Ok(format!(
        r#"{{"count":{count},"errors":{errors},"warnings":{warnings},"infos":{infos}}}"#
    ))
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Extract a JSON string-field value from a flat JSON object without serde.
///
/// Only handles the simple `"field": "value"` case produced by log_generator.py.
fn extract_str_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let rest = &json[pos + key.len()..];
    let rest = &rest[rest.find(':')? + 1..].trim_start();
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}
