// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Runtime backing for `std.audit` IFC relabel audit events (#896).
//!
//! `emit_relabel_event` is called by transpiled code whenever a `relabel`
//! expression or declaration is marked with the `audit` keyword.  Events
//! are written as JSONL lines to the path in `MVL_AUDIT_SINK` (env var),
//! or to stderr if the variable is not set.

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// Emit a structured JSONL audit event for an IFC relabel transition.
///
/// Called from transpiled Rust output when the `audit` keyword is present on
/// a `relabel` declaration or expression.  Parameters:
///
/// - `transition` — relabel name, e.g. `"trust"` or `"release"`
/// - `from_label` — source label name, e.g. `"Tainted"`, or `"_"` for bare
/// - `to_label`   — destination label name, e.g. `"Secret"`, or `"_"` for bare
/// - `tag`        — audit tag string supplied at the call site, e.g. `"XSS-001"`
/// - `location`   — source location hint, e.g. `"src/api.mvl"`
pub fn emit_relabel_event(
    transition: String,
    from_label: String,
    to_label: String,
    tag: String,
    location: String,
) {
    let ts = {
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        // ISO-8601 UTC timestamp (seconds precision).
        let secs = dur.as_secs();
        let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(secs);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    };
    let line = format!(
        "{{\"timestamp\":\"{ts}\",\"kind\":\"relabel\",\"transition\":\"{}\",\"from\":\"{}\",\"to\":\"{}\",\"tag\":\"{}\",\"location\":\"{}\"}}",
        json_escape(&transition),
        json_escape(&from_label),
        json_escape(&to_label),
        json_escape(&tag),
        json_escape(&location),
    );
    if let Ok(sink) = std::env::var("MVL_AUDIT_SINK") {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sink)
            .and_then(|mut f| writeln!(f, "{line}"));
    } else {
        eprintln!("[mvl-audit] {line}");
    }
}

fn json_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c => vec![c],
        })
        .collect()
}

/// Convert Unix epoch seconds to (year, month, day, hour, min, sec) UTC.
fn epoch_to_ymd_hms(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    secs /= 60;
    let mi = secs % 60;
    secs /= 60;
    let h = secs % 24;
    let mut days = secs / 24;
    // Epoch is 1970-01-01 (Thursday).
    let mut y = 1970u64;
    loop {
        let leap = is_leap(y);
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }
    let leap = is_leap(y);
    let month_days = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        mo += 1;
    }
    (y, mo, days + 1, h, mi, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_ymd_known_date() {
        // 2026-06-04T00:00:00Z = 1780531200
        let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(1780531200);
        assert_eq!((y, mo, d, h, mi, s), (2026, 6, 4, 0, 0, 0));
    }

    #[test]
    fn json_escape_quotes() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
    }
}
