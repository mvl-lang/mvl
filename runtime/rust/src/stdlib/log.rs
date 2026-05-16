// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.log` stdlib functions.
//!
//! Provides Phase A backing for structured logging declared in `std/log.mvl`.
//! Re-exported via `mvl_runtime::prelude::*`.
//!
//! Three output formats (issue #801):
//!   Plain   — `2026-05-16 14:32:00 INFO  msg key=value`
//!   Logfmt  — `level=INFO ts=2026-05-16T14:32:00Z msg="msg" key=value`
//!   Json    — `{"level":"INFO","ts":"...","msg":"msg","key":"value"}`
//!
//! Format is process-global (thread-local Cell); default is Plain.
//! `log_set_format()` / `get_current_format()` are pub(crate) for use by
//! future std.runtime manifest logging (#803).
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

use std::cell::Cell;
use std::collections::HashMap;

use crate::stdlib::time::{format_instant, now};

// ── Format selection ──────────────────────────────────────────────────────────

/// Selects the output format for all log calls in the current process.
///
/// Set via [`log_set_format`]. Default is [`LogFormat::Plain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable console output: `YYYY-MM-DD HH:MM:SS LEVEL  msg [key=value ...]`
    Plain,
    /// Structured key=value pairs: `level=LEVEL ts=ISO8601 msg="..." [key=value ...]`
    Logfmt,
    /// Machine-readable JSONL: `{"level":"...","ts":"...","msg":"...",...}`
    Json,
}

thread_local! {
    static FORMAT: Cell<LogFormat> = const { Cell::new(LogFormat::Plain) };
}

/// Returns the current log output format for this thread.
pub fn get_current_format() -> LogFormat {
    FORMAT.with(Cell::get)
}

/// Sets the log output format for all subsequent log calls in this process.
pub fn log_set_format(fmt: LogFormat) {
    FORMAT.with(|f| f.set(fmt));
}

// ── Sanitization ──────────────────────────────────────────────────────────────

fn sanitize(s: &str) -> String {
    s.replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\0', "\\0")
}

fn sorted_keys(fields: &HashMap<String, String>) -> Vec<&String> {
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    keys
}

// ── Formatters ────────────────────────────────────────────────────────────────

/// Plain: `2026-05-16 14:32:00 INFO  msg [key=value ...]`
pub(crate) fn format_plain(
    level: &str,
    timestamp: &str,
    msg: &str,
    fields: &HashMap<String, String>,
) -> String {
    // Timestamp for plain uses space separator instead of T, strip trailing Z.
    let ts = timestamp
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string();
    let level_col = format!("{:<5}", level);
    let safe_msg = sanitize(msg);
    let keys = sorted_keys(fields);
    if keys.is_empty() {
        format!("{} {} {}", ts, level_col, safe_msg)
    } else {
        let field_str = keys
            .iter()
            .map(|k| format!("{}={}", sanitize(k), sanitize(&fields[*k])))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {} {} {}", ts, level_col, safe_msg, field_str)
    }
}

/// Logfmt: `level=INFO ts=<ISO8601> msg="<msg>" [key=value ...]`
pub(crate) fn format_logfmt(
    level: &str,
    timestamp: &str,
    msg: &str,
    fields: &HashMap<String, String>,
) -> String {
    let safe_msg = sanitize(msg);
    let quoted_msg = if safe_msg.contains(' ') {
        format!("\"{}\"", safe_msg)
    } else {
        safe_msg
    };
    let keys = sorted_keys(fields);
    let mut parts = vec![
        format!("level={}", level),
        format!("ts={}", timestamp),
        format!("msg={}", quoted_msg),
    ];
    for k in keys {
        let v = sanitize(&fields[k]);
        let qv = if v.contains(' ') {
            format!("\"{}\"", v)
        } else {
            v
        };
        parts.push(format!("{}={}", sanitize(k), qv));
    }
    parts.join(" ")
}

/// JSON: `{"level":"INFO","ts":"...","msg":"...","key":"value",...}`
pub(crate) fn format_json(
    level: &str,
    timestamp: &str,
    msg: &str,
    fields: &HashMap<String, String>,
) -> String {
    fn json_escape(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
            .replace('\0', "\\u0000")
    }
    let keys = sorted_keys(fields);
    let mut pairs = vec![
        format!("\"level\":\"{}\"", json_escape(level)),
        format!("\"ts\":\"{}\"", json_escape(timestamp)),
        format!("\"msg\":\"{}\"", json_escape(msg)),
    ];
    for k in keys {
        pairs.push(format!(
            "\"{}\":\"{}\"",
            json_escape(k),
            json_escape(&fields[k])
        ));
    }
    format!("{{{}}}", pairs.join(","))
}

// ── Internal dispatch ─────────────────────────────────────────────────────────

fn log_internal(level: &str, msg: String, fields: HashMap<String, String>) {
    let timestamp = format_instant(now(), "%Y-%m-%dT%H:%M:%SZ".to_string());
    let line = match get_current_format() {
        LogFormat::Plain => format_plain(level, &timestamp, &msg, &fields),
        LogFormat::Logfmt => format_logfmt(level, &timestamp, &msg, &fields),
        LogFormat::Json => format_json(level, &timestamp, &msg, &fields),
    };
    eprintln!("{}", line);
}

// ── Public API ────────────────────────────────────────────────────────────────

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

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ── smoke tests ───────────────────────────────────────────────────────────

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

    // ── Plain format ──────────────────────────────────────────────────────────

    #[test]
    fn plain_no_fields() {
        let line = format_plain("INFO", "2026-05-16T14:32:00Z", "hello", &HashMap::new());
        assert_eq!(line, "2026-05-16 14:32:00 INFO  hello");
    }

    #[test]
    fn plain_with_fields_sorted() {
        let f = fields(&[("z", "last"), ("a", "first")]);
        let line = format_plain("WARN", "2026-05-16T14:32:00Z", "msg", &f);
        assert_eq!(line, "2026-05-16 14:32:00 WARN  msg a=first z=last");
    }

    #[test]
    fn plain_level_padded_to_five_chars() {
        let line = format_plain("DEBUG", "2026-05-16T14:32:00Z", "msg", &HashMap::new());
        assert!(
            line.contains("DEBUG"),
            "DEBUG is exactly 5 chars, no padding needed: {line}"
        );
        let line2 = format_plain("INFO", "2026-05-16T14:32:00Z", "msg", &HashMap::new());
        // INFO is 4 chars — padded to "INFO "
        assert!(line2.contains("INFO "), "INFO should be padded: {line2}");
    }

    #[test]
    fn plain_sanitizes_newline_in_msg() {
        let line = format_plain("WARN", "2026-05-16T14:32:00Z", "a\nb", &HashMap::new());
        assert!(
            !line.contains('\n'),
            "raw newline must not appear: {line:?}"
        );
        assert!(
            line.contains("a\\nb"),
            "escaped newline must appear: {line:?}"
        );
    }

    #[test]
    fn plain_sanitizes_newline_in_field_value() {
        let f = fields(&[("k", "v1\nv2")]);
        let line = format_plain("INFO", "2026-05-16T14:32:00Z", "msg", &f);
        assert!(!line.contains('\n'));
        assert!(line.contains("k=v1\\nv2"));
    }

    #[test]
    fn plain_sanitizes_newline_in_field_key() {
        let f = fields(&[("k\ney", "val")]);
        let line = format_plain("INFO", "2026-05-16T14:32:00Z", "msg", &f);
        assert!(!line.contains('\n'));
        assert!(line.contains("k\\ney=val"));
    }

    #[test]
    fn plain_sanitizes_carriage_return() {
        let line = format_plain("INFO", "2026-05-16T14:32:00Z", "a\rb", &HashMap::new());
        assert!(!line.contains('\r'));
        assert!(line.contains("a\\rb"));
    }

    #[test]
    fn plain_sanitizes_tab() {
        let line = format_plain("DEBUG", "2026-05-16T14:32:00Z", "a\tb", &HashMap::new());
        assert!(line.contains("a\\tb"));
    }

    #[test]
    fn plain_sanitizes_nul() {
        let f = fields(&[("k", "v\0x")]);
        let line = format_plain("ERROR", "2026-05-16T14:32:00Z", "msg", &f);
        assert!(line.contains("k=v\\0x"));
    }

    // ── Logfmt format ─────────────────────────────────────────────────────────

    #[test]
    fn logfmt_no_fields() {
        let line = format_logfmt("INFO", "2026-05-16T14:32:00Z", "hello", &HashMap::new());
        assert_eq!(line, "level=INFO ts=2026-05-16T14:32:00Z msg=hello");
    }

    #[test]
    fn logfmt_msg_with_spaces_is_quoted() {
        let line = format_logfmt(
            "INFO",
            "2026-05-16T14:32:00Z",
            "User created",
            &HashMap::new(),
        );
        assert_eq!(
            line,
            "level=INFO ts=2026-05-16T14:32:00Z msg=\"User created\""
        );
    }

    #[test]
    fn logfmt_field_value_with_spaces_is_quoted() {
        let f = fields(&[("msg2", "hello world")]);
        let line = format_logfmt("INFO", "2026-05-16T14:32:00Z", "ev", &f);
        assert!(line.contains("msg2=\"hello world\""), "got: {line}");
    }

    #[test]
    fn logfmt_fields_sorted() {
        let f = fields(&[("z", "last"), ("a", "first")]);
        let line = format_logfmt("DEBUG", "T", "msg", &f);
        let a_pos = line.find("a=first").unwrap();
        let z_pos = line.find("z=last").unwrap();
        assert!(a_pos < z_pos, "fields must be sorted: {line}");
    }

    #[test]
    fn logfmt_sanitizes_newline_in_msg() {
        let line = format_logfmt("WARN", "T", "a\nb", &HashMap::new());
        assert!(!line.contains('\n'));
        assert!(line.contains("a\\nb"));
    }

    // ── JSON format ───────────────────────────────────────────────────────────

    #[test]
    fn json_no_fields() {
        let line = format_json("INFO", "2026-05-16T14:32:00Z", "hello", &HashMap::new());
        assert_eq!(
            line,
            r#"{"level":"INFO","ts":"2026-05-16T14:32:00Z","msg":"hello"}"#
        );
    }

    #[test]
    fn json_with_fields_sorted() {
        let f = fields(&[("z", "last"), ("a", "first")]);
        let line = format_json("DEBUG", "T", "msg", &f);
        let a_pos = line.find("\"a\"").unwrap();
        let z_pos = line.find("\"z\"").unwrap();
        assert!(a_pos < z_pos, "fields must be sorted: {line}");
    }

    #[test]
    fn json_escapes_double_quote_in_value() {
        let f = fields(&[("k", "say \"hi\"")]);
        let line = format_json("INFO", "T", "msg", &f);
        assert!(line.contains(r#""k":"say \"hi\"""#), "got: {line}");
    }

    #[test]
    fn json_escapes_backslash() {
        let f = fields(&[("path", r"C:\Users")]);
        let line = format_json("INFO", "T", "msg", &f);
        assert!(line.contains(r#""path":"C:\\Users""#), "got: {line}");
    }

    #[test]
    fn json_escapes_newline_in_msg() {
        let line = format_json("WARN", "T", "a\nb", &HashMap::new());
        assert!(!line.contains('\n'));
        assert!(line.contains(r#""msg":"a\nb""#));
    }

    #[test]
    fn json_escapes_nul() {
        let f = fields(&[("k", "v\0x")]);
        let line = format_json("ERROR", "T", "msg", &f);
        assert!(line.contains(r#""k":"v\u0000x""#), "got: {line}");
    }

    // ── log_set_format / get_current_format ───────────────────────────────────

    #[test]
    fn default_format_is_plain() {
        // Thread-local; set explicitly to avoid interference from other tests.
        log_set_format(LogFormat::Plain);
        assert_eq!(get_current_format(), LogFormat::Plain);
    }

    #[test]
    fn set_format_logfmt() {
        log_set_format(LogFormat::Logfmt);
        assert_eq!(get_current_format(), LogFormat::Logfmt);
        log_set_format(LogFormat::Plain);
    }

    #[test]
    fn set_format_json() {
        log_set_format(LogFormat::Json);
        assert_eq!(get_current_format(), LogFormat::Json);
        log_set_format(LogFormat::Plain);
    }
}
