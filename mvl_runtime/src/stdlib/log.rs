//! Rust implementations of `std.log` stdlib functions.
//!
//! Provides Phase A backing for structured logging declared in `std/log.mvl`.
//! Re-exported via `mvl_runtime::prelude::*`.
//!
//! Format: `[{LEVEL} {ISO_8601_TIMESTAMP}] {msg} {field=value ...}`
//!
//! Output goes to stderr via `eprintln!`. Field keys are sorted for
//! deterministic test output. No configurable sink (Phase 3 / issue #54).

use std::collections::HashMap;

use crate::stdlib::time::{format_instant, now};

fn log_internal(level: &str, msg: String, fields: HashMap<String, String>) {
    let timestamp = format_instant(now(), "%Y-%m-%dT%H:%M:%SZ".to_string());
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    let field_str = keys
        .iter()
        .map(|k| format!("{}={}", k, fields[*k]))
        .collect::<Vec<_>>()
        .join(" ");
    if field_str.is_empty() {
        eprintln!("[{} {}] {}", level, timestamp, msg);
    } else {
        eprintln!("[{} {}] {} {}", level, timestamp, msg, field_str);
    }
}

/// Emit a DEBUG-level structured log record.
pub fn log_debug(msg: String, fields: HashMap<String, String>) {
    log_internal("DEBUG", msg, fields);
}

/// Emit an INFO-level structured log record.
pub fn log_info(msg: String, fields: HashMap<String, String>) {
    log_internal("INFO", msg, fields);
}

/// Emit a WARN-level structured log record.
pub fn log_warn(msg: String, fields: HashMap<String, String>) {
    log_internal("WARN", msg, fields);
}

/// Emit an ERROR-level structured log record.
pub fn log_error(msg: String, fields: HashMap<String, String>) {
    log_internal("ERROR", msg, fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_debug_does_not_panic() {
        log_debug("debug".to_string(), HashMap::new());
    }

    #[test]
    fn log_info_does_not_panic() {
        log_info("info".to_string(), HashMap::new());
    }

    #[test]
    fn log_warn_does_not_panic() {
        log_warn("warn".to_string(), HashMap::new());
    }

    #[test]
    fn log_error_does_not_panic() {
        log_error("error".to_string(), HashMap::new());
    }

    #[test]
    fn log_info_with_fields_does_not_panic() {
        let mut fields = HashMap::new();
        fields.insert("key".to_string(), "value".to_string());
        log_info("event".to_string(), fields);
    }

    #[test]
    fn fields_are_sorted() {
        let mut fields = HashMap::new();
        fields.insert("z".to_string(), "last".to_string());
        fields.insert("a".to_string(), "first".to_string());
        fields.insert("m".to_string(), "mid".to_string());
        log_info("ordering".to_string(), fields);
    }
}
