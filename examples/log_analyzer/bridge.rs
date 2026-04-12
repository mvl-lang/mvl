//! bridge.rs — Rust implementations of the `extern "rust"` trust boundary
//! declared in main.mvl.
//!
//! These three functions cross the trust boundary between verified MVL code
//! and the outside world (CLI args, filesystem, JSON pipeline).  They are
//! responsible for tainting incoming data with `Tainted<T>` and only
//! producing `Clean<T>` values after explicit validation.
//!
//! Compile with: `mvl build examples/log_analyzer/main.mvl`
//! (bridge.rs is detected automatically and linked in)

use mvl_runtime::prelude::*;

// Types generated from main.mvl — accessible as `crate::*` in a binary crate.
use crate::{IOError, PipelineError};

// ── Trust boundary implementations ────────────────────────────────────────

/// Scan command-line args for `--<name> <value>` and return the value as
/// Tainted (it has not been validated yet).
#[no_mangle]
pub extern "Rust" fn clap_get_arg(name: Clean<String>) -> Option<Tainted<String>> {
    let flag = format!("--{}", *name);
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == flag {
            return Some(Tainted(args[i + 1].clone()));
        }
    }
    None
}

/// Read a file from disk, returning the raw bytes as Tainted (external input).
#[no_mangle]
pub extern "Rust" fn fs_read_file(path: Clean<String>) -> Result<Tainted<String>, IOError> {
    std::fs::read_to_string(&*path)
        .map(Tainted)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => IOError::FileNotFound,
            std::io::ErrorKind::PermissionDenied => IOError::PermissionDenied,
            _ => IOError::ReadError,
        })
}

/// Parse JSONL content line-by-line, optionally filter by log level, and
/// return a compact JSON report string.
///
/// Each line is expected to be:
///   {"level": "info", "message": "...", "timestamp": 1234567890}
#[no_mangle]
pub extern "Rust" fn analyze_and_format(
    content: Tainted<String>,
    level: Option<Clean<String>>,
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
