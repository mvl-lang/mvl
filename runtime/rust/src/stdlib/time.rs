// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.time` stdlib functions.
//!
//! Provides wall-clock time and sleep backing for the stubs declared in
//! `std/time.mvl`. UTC-only for Phase A; timezone support deferred.
//! Re-exported via `mvl_runtime::prelude::*`.

use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

// ── Types ──────────────────────────────────────────────────────────────────

/// An opaque point in time backed by `SystemTime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instant(pub SystemTime);

/// A human-readable calendar date and time (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    /// Full proleptic Gregorian year.
    pub year: i64,
    /// Month 1–12.
    pub month: i64,
    /// Day of month 1–31.
    pub day: i64,
    /// Hour 0–23.
    pub hour: i64,
    /// Minute 0–59.
    pub minute: i64,
    /// Second 0–60 (60 allows leap seconds).
    pub second: i64,
}

/// A span of time with nanosecond precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duration {
    /// Whole seconds component.
    pub secs: i64,
    /// Sub-second nanoseconds component (0–999_999_999).
    pub nanos: i64,
}

// ── Effect-carrying functions ──────────────────────────────────────────────

/// Returns the current wall-clock time. Requires `! Clock`.
pub fn now() -> Instant {
    Instant(SystemTime::now())
}

/// Suspends execution for the given duration. Requires `! Clock`.
pub fn sleep(d: Duration) {
    let secs = d.secs.max(0) as u64;
    let nanos = d.nanos.max(0) as u32;
    std::thread::sleep(StdDuration::new(secs, nanos));
}

/// Returns whole seconds since the Unix epoch for the given `Instant`.
///
/// Module-private MVL builtin (`builtin fn _instant_epoch_seconds`, #899).
/// Drives pure-MVL `format_instant`/`instant_to_datetime` formatting.
pub fn _instant_epoch_seconds(t: Instant) -> i64 {
    t.0.duration_since(UNIX_EPOCH)
        .unwrap_or(StdDuration::ZERO)
        .as_secs() as i64
}

// ── Pure formatting functions ──────────────────────────────────────────────
//
// `format_instant` and `format_datetime` are Rust-internal helpers — no longer
// exposed as MVL builtins (pure-MVL equivalents live in std/time.mvl).
// Retained for log.rs internals and the legacy `_mvl_time_iso8601_format`.

/// Formats an `Instant` as a string using the given format pattern.
pub fn format_instant(t: Instant, pattern: String) -> String {
    apply_format(&instant_to_datetime(t), &pattern)
}

/// Formats a `DateTime` as a string using the given format pattern.
pub fn format_datetime(t: DateTime, pattern: String) -> String {
    apply_format(&t, &pattern)
}

/// Parses a datetime string using the given format pattern.
///
/// Supports the same tokens as `format_instant`. Returns `None` if the
/// string does not match the pattern or contains out-of-range fields.
pub fn parse(s: String, pattern: String) -> Option<DateTime> {
    parse_datetime(&s, &pattern)
}

// ── Pure duration constructors ─────────────────────────────────────────────

/// Constructs a `Duration` from whole seconds.
pub fn seconds(n: i64) -> Duration {
    Duration { secs: n, nanos: 0 }
}

/// Constructs a `Duration` from whole milliseconds.
pub fn millis(n: i64) -> Duration {
    Duration {
        secs: n / 1_000,
        nanos: (n % 1_000) * 1_000_000,
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

fn instant_to_datetime(t: Instant) -> DateTime {
    let epoch_secs =
        t.0.duration_since(UNIX_EPOCH)
            .unwrap_or(StdDuration::ZERO)
            .as_secs() as i64;
    epoch_secs_to_datetime(epoch_secs)
}

/// Converts a Unix epoch second count to a `DateTime` (UTC, proleptic Gregorian).
fn epoch_secs_to_datetime(mut secs: i64) -> DateTime {
    const SECS_PER_MIN: i64 = 60;

    let second = secs % SECS_PER_MIN;
    secs /= SECS_PER_MIN;
    let minute = secs % 60;
    secs /= 60;
    let hour = secs % 24;
    let mut days = secs / 24; // days since 1970-01-01

    // Shift epoch to 1 Mar 0000 to make leap-day math regular.
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month of year (Mar=0..Feb=11)
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    DateTime {
        year: y,
        month: m,
        day: d,
        hour: hour % 24,
        minute,
        second,
    }
}

fn apply_format(dt: &DateTime, pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            match chars.next() {
                Some('Y') => out.push_str(&format!("{:04}", dt.year)),
                Some('m') => out.push_str(&format!("{:02}", dt.month)),
                Some('d') => out.push_str(&format!("{:02}", dt.day)),
                Some('H') => out.push_str(&format!("{:02}", dt.hour)),
                Some('M') => out.push_str(&format!("{:02}", dt.minute)),
                Some('S') => out.push_str(&format!("{:02}", dt.second)),
                Some(other) => {
                    out.push('%');
                    out.push(other);
                }
                None => out.push('%'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_datetime(s: &str, pattern: &str) -> Option<DateTime> {
    let mut year: Option<i64> = None;
    let mut month: Option<i64> = None;
    let mut day: Option<i64> = None;
    let mut hour: Option<i64> = None;
    let mut minute: Option<i64> = None;
    let mut second: Option<i64> = None;

    let mut si = 0usize; // byte index into s
    let sbytes = s.as_bytes();
    let mut pi = pattern.chars().peekable();

    while let Some(pc) = pi.next() {
        if pc == '%' {
            let token = pi.next()?;
            let width: usize = match token {
                'Y' => 4,
                'm' | 'd' | 'H' | 'M' | 'S' => 2,
                _ => return None,
            };
            if si + width > sbytes.len() {
                return None;
            }
            let slice = std::str::from_utf8(&sbytes[si..si + width]).ok()?;
            let val: i64 = slice.parse().ok()?;
            si += width;
            match token {
                'Y' => year = Some(val),
                'm' => month = Some(val),
                'd' => day = Some(val),
                'H' => hour = Some(val),
                'M' => minute = Some(val),
                'S' => second = Some(val),
                _ => {}
            }
        } else {
            // Literal character must match.
            if si >= sbytes.len() || sbytes[si] != pc as u8 {
                return None;
            }
            si += pc.len_utf8();
        }
    }

    // Remaining input must be consumed.
    if si != s.len() {
        return None;
    }

    let month = month.unwrap_or(1);
    let day = day.unwrap_or(1);
    let hour = hour.unwrap_or(0);
    let minute = minute.unwrap_or(0);
    let second = second.unwrap_or(0);

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }

    Some(DateTime {
        year: year.unwrap_or(1970),
        month,
        day,
        hour,
        minute,
        second,
    })
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_after_epoch() {
        let t = now();
        assert!(
            t.0 >= UNIX_EPOCH,
            "now() must be at or after the Unix epoch"
        );
    }

    #[test]
    fn seconds_constructor() {
        let d = seconds(5);
        assert_eq!(d.secs, 5);
        assert_eq!(d.nanos, 0);
    }

    #[test]
    fn millis_constructor() {
        let d = millis(1500);
        assert_eq!(d.secs, 1);
        assert_eq!(d.nanos, 500_000_000);
    }

    #[test]
    fn millis_sub_second() {
        let d = millis(250);
        assert_eq!(d.secs, 0);
        assert_eq!(d.nanos, 250_000_000);
    }

    #[test]
    fn epoch_to_datetime_unix_epoch() {
        let dt = epoch_secs_to_datetime(0);
        assert_eq!(dt.year, 1970);
        assert_eq!(dt.month, 1);
        assert_eq!(dt.day, 1);
        assert_eq!(dt.hour, 0);
        assert_eq!(dt.minute, 0);
        assert_eq!(dt.second, 0);
    }

    #[test]
    fn epoch_to_datetime_known_timestamp() {
        // 2024-03-15 12:30:45 UTC = 1710505845
        let dt = epoch_secs_to_datetime(1_710_505_845);
        assert_eq!(dt.year, 2024);
        assert_eq!(dt.month, 3);
        assert_eq!(dt.day, 15);
        assert_eq!(dt.hour, 12);
        assert_eq!(dt.minute, 30);
        assert_eq!(dt.second, 45);
    }

    #[test]
    fn epoch_to_datetime_leap_day() {
        // 2000-02-29 00:00:00 UTC = 951782400
        let dt = epoch_secs_to_datetime(951_782_400);
        assert_eq!(dt.year, 2000);
        assert_eq!(dt.month, 2);
        assert_eq!(dt.day, 29);
    }

    #[test]
    fn format_instant_iso8601() {
        let t = Instant(UNIX_EPOCH + StdDuration::from_secs(1_710_505_845));
        let s = format_instant(t, "%Y-%m-%dT%H:%M:%S".to_string());
        assert_eq!(s, "2024-03-15T12:30:45");
    }

    #[test]
    fn format_datetime_basic() {
        let dt = DateTime {
            year: 2024,
            month: 3,
            day: 15,
            hour: 12,
            minute: 30,
            second: 45,
        };
        let s = format_datetime(dt, "%Y-%m-%d %H:%M:%S".to_string());
        assert_eq!(s, "2024-03-15 12:30:45");
    }

    #[test]
    fn parse_full_datetime() {
        let dt = parse(
            "2024-03-15 12:30:45".to_string(),
            "%Y-%m-%d %H:%M:%S".to_string(),
        );
        assert_eq!(
            dt,
            Some(DateTime {
                year: 2024,
                month: 3,
                day: 15,
                hour: 12,
                minute: 30,
                second: 45
            })
        );
    }

    #[test]
    fn parse_date_only() {
        let dt = parse("2024-03-15".to_string(), "%Y-%m-%d".to_string());
        assert_eq!(
            dt,
            Some(DateTime {
                year: 2024,
                month: 3,
                day: 15,
                hour: 0,
                minute: 0,
                second: 0
            })
        );
    }

    #[test]
    fn parse_invalid_month_returns_none() {
        assert!(parse("2024-13-01".to_string(), "%Y-%m-%d".to_string()).is_none());
    }

    #[test]
    fn parse_trailing_garbage_returns_none() {
        assert!(parse("2024-03-15X".to_string(), "%Y-%m-%d".to_string()).is_none());
    }

    #[test]
    fn parse_mismatch_returns_none() {
        assert!(parse("2024/03/15".to_string(), "%Y-%m-%d".to_string()).is_none());
    }

    #[test]
    fn format_unknown_token_passthrough() {
        let dt = DateTime {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        };
        let s = format_datetime(dt, "%Y%z".to_string());
        assert_eq!(s, "2024%z");
    }
}
