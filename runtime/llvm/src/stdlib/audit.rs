// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.audit` IFC relabel audit events (#896, #1554).
//!
//! Mirrors `mvl_runtime::stdlib::audit`. The LLVM backend's `relabel ... audit`
//! emit calls `_mvl_audit_emit_relabel` with five `*const MvlString`
//! arguments; the wrapper reads each as a Rust `String` and delegates to
//! `emit_relabel_event`, which writes a JSONL line to `MVL_AUDIT_SINK` (or
//! stderr if unset).

use std::slice;

use crate::memory::MvlString;
use mvl_runtime::stdlib::audit as rt;

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

/// Emit a structured JSONL audit event for an IFC relabel transition.
///
/// All five arguments are `*const MvlString`. Null and empty strings are
/// accepted (they become empty Rust strings).
///
/// # Safety
/// Each argument must either be null or a valid `*const MvlString` whose
/// `(ptr, len)` describes a live byte buffer.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_audit_emit_relabel(
    transition: *const MvlString,
    from_label: *const MvlString,
    to_label: *const MvlString,
    tag: *const MvlString,
    location: *const MvlString,
) {
    rt::emit_relabel_event(
        read_mvl_string(transition),
        read_mvl_string(from_label),
        read_mvl_string(to_label),
        read_mvl_string(tag),
        read_mvl_string(location),
    );
}
