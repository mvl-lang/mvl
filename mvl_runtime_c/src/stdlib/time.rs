//! C-ABI exports for `std.time` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::time`. The C boundary uses:
//!   - `_mvl_time_now_systemtime` → seconds since Unix epoch (i64)
//!   - `_mvl_time_now_instant`    → nanoseconds since Unix epoch (i64)
//!   - `_mvl_time_thread_sleep`   → (secs: i64, nanos: i64) → void
//!   - `_mvl_time_iso8601_format` → (secs: i64) → *mut c_char (caller frees)
//!   - `_mvl_time_now`            → *mut c_void (boxed epoch-seconds handle; #585)
//!   - `_mvl_time_format_instant` → (handle: ptr, fmt: *MvlString) → *mut MvlString (#585)
//!   - `_mvl_time_format_datetime`→ (dt_struct: ptr, fmt: *MvlString) → *mut MvlString (#585)
//!
//! `Duration` is split into `(secs: i64, nanos: i64)` to avoid struct-layout
//! complexity at the C boundary.
//!
//! # Instant handle pattern (#585)
//!
//! `Instant` is an opaque Rust type that cannot be C-ABI encoded directly.
//! `_mvl_time_now()` boxes the current epoch-second count as a `Box<i64>` on
//! the heap and returns the raw pointer as `*mut c_void`.  Subsequent calls
//! (`_mvl_time_format_instant`) receive that pointer, dereference the i64, and
//! free it (caller is responsible — or the process exits, which is equivalent
//! for short-lived programs).
//!
//! # String ownership (MvlString variant)
//!
//! `_mvl_time_format_instant` and `_mvl_time_format_datetime` return a
//! heap-allocated `*mut MvlString` whose lifetime is owned by the LLVM caller.

use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use libc::{c_char, c_void};

use crate::abi::string_to_c;
use mvl_memory::{mvl_string_new, MvlString};
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

// ── #585: Instant handle + format functions ────────────────────────────────────

/// Read a `MvlString*` as a Rust `String`.
#[allow(unsafe_code)]
unsafe fn read_mvl_str(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (*s).len as usize;
    if len == 0 || (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = std::slice::from_raw_parts((*s).ptr as *const u8, len);
    String::from_utf8_lossy(bytes).into_owned()
}

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

/// Format an `Instant` handle as a string using the given pattern.
///
/// `handle` must be a non-null pointer returned by `_mvl_time_now`.
/// Returns a heap-allocated `*mut MvlString`; caller owns it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_time_format_instant(
    handle: *const c_void,
    fmt: *const MvlString,
) -> *mut MvlString {
    if handle.is_null() {
        let empty = "";
        return mvl_string_new(empty.as_ptr(), 0);
    }
    let secs: i64 = *(handle as *const i64);
    let systime = UNIX_EPOCH + StdDuration::from_secs(secs.max(0) as u64);
    let instant = Instant(systime);
    let pattern = read_mvl_str(fmt);
    let result = rt::format_instant(instant, pattern);
    mvl_string_new(result.as_bytes().as_ptr(), result.len())
}

/// Format a `DateTime` struct as a string using the given pattern.
///
/// `dt` points to an LLVM-stack-allocated `{i64, i64, i64, i64, i64, i64}`
/// in field order: `year, month, day, hour, minute, second`.
/// Returns a heap-allocated `*mut MvlString`; caller owns it.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_time_format_datetime(
    dt: *const c_void,
    fmt: *const MvlString,
) -> *mut MvlString {
    if dt.is_null() {
        let empty = "";
        return mvl_string_new(empty.as_ptr(), 0);
    }
    let f = dt as *const i64;
    let dt_val = rt::DateTime {
        year: *f,
        month: *f.add(1),
        day: *f.add(2),
        hour: *f.add(3),
        minute: *f.add(4),
        second: *f.add(5),
    };
    let pattern = read_mvl_str(fmt);
    let result = rt::format_datetime(dt_val, pattern);
    mvl_string_new(result.as_bytes().as_ptr(), result.len())
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
