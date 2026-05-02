//! Pilot export: `_mvl_runtime_version`.
//!
//! Proves the cdylib loads and resolves correctly from the LLVM backend.
//! Used as a smoke test by `tests/cross_backend.rs`.

use std::ffi::CStr;

/// Return the MVL runtime version string as a static NUL-terminated C string.
///
/// The returned pointer is valid for the lifetime of the process; callers
/// must NOT free it.
///
/// Symbol: `_mvl_runtime_version`
#[no_mangle]
pub extern "C" fn _mvl_runtime_version() -> *const libc::c_char {
    // CARGO_PKG_VERSION is set at compile time. We embed it as a C string
    // literal so the pointer is static and never needs to be freed.
    // The concat! + \0 pattern produces a &'static str ending in NUL.
    static VERSION_BYTES: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();
    // Safety: VERSION_BYTES is 'static, NUL-terminated, and valid UTF-8.
    #[allow(unsafe_code)]
    unsafe {
        CStr::from_bytes_with_nul_unchecked(VERSION_BYTES).as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn version_returns_nonnull_nonempty_string() {
        let ptr = _mvl_runtime_version();
        assert!(!ptr.is_null());
        #[allow(unsafe_code)]
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert!(!s.is_empty(), "version string must not be empty");
        // Must look like a semver string: contains at least one dot.
        assert!(s.contains('.'), "version must be semver-like, got: {s}");
    }
}
