// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.time` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::time`. The C boundary uses:
//!   - `_mvl_time_now_systemtime` → seconds since Unix epoch (i64)
//!   - `_mvl_time_now_instant`    → nanoseconds since Unix epoch (i64)
//!   - `_mvl_time_thread_sleep`   → (secs: i64, nanos: i64) → void
//!   - `_mvl_time_iso8601_format` → (secs: i64) → *mut c_char (caller frees)
//!   - `_mvl_time_now`            → *mut c_void (boxed epoch-seconds handle; #585)
//!   - `_mvl_time__instant_epoch_seconds` → (handle: ptr) → i64 (#899)
//!
//! `Duration` is split into `(secs: i64, nanos: i64)` to avoid struct-layout
//! complexity at the C boundary.
//!
//! # Instant handle pattern (#585)
//!
//! `Instant` is an opaque Rust type that cannot be C-ABI encoded directly.
//! `_mvl_time_now()` boxes the current epoch-second count as a `Box<i64>` on
//! the heap and returns the raw pointer as `*mut c_void`. The pure-MVL
//! `format_instant` / `format_datetime` (in std/time.mvl) call
//! `_instant_epoch_seconds(handle)` to read the i64 and compute formatting
//! entirely in MVL — no `_mvl_time_format_*` C-ABI shims required.

use std::slice;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use libc::{c_char, c_void};

use crate::abi::string_to_c;
use crate::memory::{MvlString, _mvl_string_new};
use mvl_runtime::stdlib::time as rt;
use mvl_runtime::stdlib::time::{sleep, Duration};
use rt::Instant;

// ── MvlString helpers (mirrors regex.rs) ─────────────────────────────────────

#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (*s).len as usize;
    if len == 0 || (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = slice::from_raw_parts((*s).ptr as *const u8, len);
    String::from_utf8_lossy(bytes).into_owned()
}

#[allow(unsafe_code)]
fn new_mvl_str(s: &str) -> *mut c_void {
    let bytes = s.as_bytes();
    unsafe { _mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void }
}

// ── Wall-clock ────────────────────────────────────────────────────────────────

/// Return seconds since the Unix epoch (wall clock). `Int` return — no marshalling.
#[no_mangle]
pub extern "C" fn _mvl_time_now_systemtime() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(StdDuration::ZERO)
        .as_secs() as i64
}

/// Return nanoseconds since the Unix epoch (wall clock). `Int` return — no marshalling.
#[no_mangle]
pub extern "C" fn _mvl_time_now_instant() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(StdDuration::ZERO)
        .as_nanos() as i64
}

// ── Sleep ─────────────────────────────────────────────────────────────────────

/// Suspend execution for `secs` seconds + `nanos` nanoseconds. Returns void.
///
/// Flattened `Duration` — the LLVM caller passes the two struct fields directly.
#[no_mangle]
pub extern "C" fn _mvl_time_thread_sleep(secs: i64, nanos: i64) {
    sleep(Duration { secs, nanos });
}

// ── #585: Instant handle ──────────────────────────────────────────────────────

/// Return the current wall-clock time as a heap-allocated epoch-seconds handle.
///
/// Boxes the current `SystemTime` as an `i64` epoch-second count and returns
/// the raw pointer as `*mut c_void`.  The LLVM caller treats this as an opaque
/// `Instant` handle.  Ownership transfers to the caller; free with `libc::free`.
#[no_mangle]
pub extern "C" fn _mvl_time_now() -> *mut c_void {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(StdDuration::ZERO)
        .as_secs() as i64;
    Box::into_raw(Box::new(secs)) as *mut c_void
}

/// Return whole seconds since the Unix epoch for an `Instant` handle.
///
/// C-ABI backing for `builtin fn _instant_epoch_seconds` (#899).
/// `handle` is the boxed-i64 returned by `_mvl_time_now`. Null is accepted and
/// returns 0.
///
/// # Safety
/// `handle` must be either null or a valid pointer returned by `_mvl_time_now`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_time__instant_epoch_seconds(handle: *const c_void) -> i64 {
    if handle.is_null() {
        return 0;
    }
    *(handle as *const i64)
}

// ── Datetime formatting (#1202) ───────────────────────────────────────────────

/// Format a DateTime (6 flattened i64 fields) as a string using the given pattern.
///
/// Flattening matches `%DateTime = type { i64, i64, i64, i64, i64, i64 }`.
/// Returned `*mut c_void` is a heap-allocated `MvlString`; caller must drop it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_time_format_datetime(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    pattern: *const MvlString,
) -> *mut c_void {
    let pat = read_mvl_string(pattern);
    let dt = rt::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    };
    let s = rt::format_datetime(dt, pat);
    new_mvl_str(&s)
}

/// Format an Instant handle (boxed epoch-seconds i64) as a string using the given pattern.
///
/// `handle` is the opaque `*mut c_void` returned by `_mvl_time_now()`.
/// Returned `*mut c_void` is a heap-allocated `MvlString`; caller must drop it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_time_format_instant(
    handle: *const c_void,
    pattern: *const MvlString,
) -> *mut c_void {
    let secs = if handle.is_null() {
        0i64
    } else {
        *(handle as *const i64)
    };
    let systime = UNIX_EPOCH + StdDuration::from_secs(secs.max(0) as u64);
    let instant = Instant(systime);
    let pat = read_mvl_string(pattern);
    let s = rt::format_instant(instant, pat);
    new_mvl_str(&s)
}

// ── Legacy ISO 8601 formatting ─────────────────────────────────────────────────

/// Format Unix seconds as ISO 8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
///
/// Constructs an `Instant` from the epoch seconds and delegates to
/// `mvl_runtime::stdlib::time::format_instant` with the ISO 8601 pattern.
///
/// Returns a heap-allocated `*mut c_char`; caller frees with `libc::free`.
#[no_mangle]
pub extern "C" fn _mvl_time_iso8601_format(secs: i64) -> *mut c_char {
    let systime = UNIX_EPOCH + StdDuration::from_secs(secs.max(0) as u64);
    let instant = Instant(systime);
    let s = mvl_runtime::stdlib::time::format_instant(instant, "%Y-%m-%dT%H:%M:%SZ".to_string());
    string_to_c(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now_systemtime_positive() {
        assert!(_mvl_time_now_systemtime() > 0);
    }

    #[test]
    fn test_now_instant_positive() {
        assert!(_mvl_time_now_instant() > 0);
    }

    #[test]
    fn test_thread_sleep_zero() {
        _mvl_time_thread_sleep(0, 0);
    }

    #[test]
    #[allow(unsafe_code)]
    fn test_iso8601_epoch() {
        let ptr = _mvl_time_iso8601_format(0);
        assert!(!ptr.is_null());
        let s = unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        assert_eq!(s, "1970-01-01T00:00:00Z");
        unsafe { libc::free(ptr as *mut libc::c_void) };
    }
}
