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

use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use libc::{c_char, c_void};

use crate::abi::string_to_c;
use mvl_runtime::stdlib::time as rt;
use mvl_runtime::stdlib::time::{sleep, Duration};
use rt::Instant;

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
