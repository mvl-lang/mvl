//! Rust implementations of `std.log` stdlib functions.
//!
//! Provides Phase 2 backing for the structured logging stubs declared in
//! `std/log.mvl`. Re-exported via `mvl_runtime::prelude::*`.
//!
//! # Phase 2 behaviour
//!
//! All functions are no-ops: they accept arguments with the correct types and
//! return `()`. No output is produced. The purpose of Phase 2 stubs is to
//! satisfy the type checker and linker so that MVL programs and tests that
//! call `log_*` functions compile and run correctly.
//!
//! # Phase 3
//!
//! The real implementation will wrap `tracing` (see issue #54 and ADR-0006).
//! The log sink (JSON, text, etc.) will be runtime-configurable.

use std::collections::HashMap;

/// Emit a DEBUG-level structured log record.
///
/// Phase 2: no-op stub. Phase 3 will forward to the configured tracing sink.
///
/// Implements the Rust backing for `std/log.mvl::log_debug`.
pub fn log_debug(_msg: String, _fields: HashMap<String, String>) {}

/// Emit an INFO-level structured log record.
///
/// Phase 2: no-op stub. Phase 3 will forward to the configured tracing sink.
///
/// Implements the Rust backing for `std/log.mvl::log_info`.
pub fn log_info(_msg: String, _fields: HashMap<String, String>) {}

/// Emit a WARN-level structured log record.
///
/// Phase 2: no-op stub. Phase 3 will forward to the configured tracing sink.
///
/// Implements the Rust backing for `std/log.mvl::log_warn`.
pub fn log_warn(_msg: String, _fields: HashMap<String, String>) {}

/// Emit an ERROR-level structured log record.
///
/// Phase 2: no-op stub. Phase 3 will forward to the configured tracing sink.
///
/// Implements the Rust backing for `std/log.mvl::log_error`.
pub fn log_error(_msg: String, _fields: HashMap<String, String>) {}

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
}
