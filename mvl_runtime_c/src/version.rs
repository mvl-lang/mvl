//! Pilot C-ABI export: `_mvl_runtime_version`.
//!
//! Returns the crate version as a null-terminated C string.  Used as a smoke
//! test that `libmvl_runtime_c` loads and resolves correctly under lli.

/// Version string with null terminator, compiled in at build time.
static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();

/// Returns the MVL runtime version as a null-terminated C string.
///
/// The pointer is valid for the lifetime of the process (static storage).
#[no_mangle]
pub extern "C" fn _mvl_runtime_version() -> *const libc::c_char {
    VERSION.as_ptr().cast()
}
