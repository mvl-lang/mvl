// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.log` stdlib functions.
//!
//! Provides Phase A backing for structured logging declared in `std/log.mvl`.
//! Re-exported via `mvl_runtime::prelude::*`.
//!
//! Format: `[{LEVEL} {ISO_8601_TIMESTAMP}] {msg} {field=value ...}`
//!
//! Output goes to stderr via `eprintln!`. Field keys are sorted for
//! deterministic test output. No configurable sink (Phase 3 / issue #54).
//!
//! # Effect note
//!
//! `log_internal` calls `time::now()` (a wall-clock read) even though the MVL
//! functions only declare `! Log`. The timestamp is an internal implementation
//! detail of the log format — it is not separately observable by the caller
//! and is intentionally exempt from the `! Clock` effect declaration for
//! Phase A. Phase 3 may revisit this when a configurable sink is introduced.

use std::collections::HashMap;

use crate::stdlib::time::{format_instant, now};

fn sanitize(s: &str) -> String {
    s.replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\0', "\\0")
}

pub(crate) fn format_log_line(
    level: &str,
    timestamp: &str,
    msg: &str,
    fields: &HashMap<String, String>,
) -> String {
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    let field_str = keys
        .iter()
        .map(|k| format!("{}={}", sanitize(k), sanitize(&fields[*k])))
        .collect::<Vec<_>>()
        .join(" ");
    let safe_msg = sanitize(msg);
    if field_str.is_empty() {
        format!("[{} {}] {}", level, timestamp, safe_msg)
    } else {
        format!("[{} {}] {} {}", level, timestamp, safe_msg, field_str)
    }
}

fn log_internal(level: &str, msg: String, fields: HashMap<String, String>) {
    let timestamp = format_instant(now(), "%Y-%m-%dT%H:%M:%SZ".to_string());
    eprintln!("{}", format_log_line(level, &timestamp, &msg, &fields));
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
    fn format_log_line_no_fields() {
        let line = format_log_line("INFO", "2024-03-15T12:30:45Z", "hello", &HashMap::new());
        assert_eq!(line, "[INFO 2024-03-15T12:30:45Z] hello");
    }

    #[test]
    fn format_log_line_fields_sorted() {
        let mut fields = HashMap::new();
        fields.insert("z".to_string(), "last".to_string());
        fields.insert("a".to_string(), "first".to_string());
        fields.insert("m".to_string(), "mid".to_string());
        let line = format_log_line("DEBUG", "T", "msg", &fields);
        assert_eq!(line, "[DEBUG T] msg a=first m=mid z=last");
    }

    #[test]
    fn format_log_line_sanitizes_newlines_in_msg() {
        let line = format_log_line("WARN", "T", "line1\nline2", &HashMap::new());
        assert_eq!(line, "[WARN T] line1\\nline2");
    }

    #[test]
    fn format_log_line_sanitizes_newlines_in_field_value() {
        let mut fields = HashMap::new();
        fields.insert("k".to_string(), "v1\nv2".to_string());
        let line = format_log_line("ERROR", "T", "msg", &fields);
        assert_eq!(line, "[ERROR T] msg k=v1\\nv2");
    }

    #[test]
    fn format_log_line_sanitizes_carriage_return_in_msg() {
        let line = format_log_line("INFO", "T", "line1\rline2", &HashMap::new());
        assert_eq!(line, "[INFO T] line1\\rline2");
    }

    #[test]
    fn format_log_line_sanitizes_carriage_return_in_field_value() {
        let mut fields = HashMap::new();
        fields.insert("k".to_string(), "v1\rv2".to_string());
        let line = format_log_line("WARN", "T", "msg", &fields);
        assert_eq!(line, "[WARN T] msg k=v1\\rv2");
    }

    #[test]
    fn format_log_line_sanitizes_tab_in_msg() {
        let line = format_log_line("DEBUG", "T", "a\tb", &HashMap::new());
        assert_eq!(line, "[DEBUG T] a\\tb");
    }

    #[test]
    fn format_log_line_sanitizes_nul_in_field_value() {
        let mut fields = HashMap::new();
        fields.insert("k".to_string(), "v\0x".to_string());
        let line = format_log_line("ERROR", "T", "msg", &fields);
        assert_eq!(line, "[ERROR T] msg k=v\\0x");
    }

    #[test]
    fn format_log_line_sanitizes_newlines_in_field_key() {
        let mut fields = HashMap::new();
        fields.insert("k\ney".to_string(), "val".to_string());
        let line = format_log_line("INFO", "T", "msg", &fields);
        assert!(
            !line.contains('\n'),
            "raw newline must not appear: {line:?}"
        );
        assert!(
            line.contains("k\\ney=val"),
            "sanitized key must appear: {line:?}"
        );
    }
}
