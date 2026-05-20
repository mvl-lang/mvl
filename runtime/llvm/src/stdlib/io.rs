// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.io` stdlib functions — LLVM backend path (#435).
//!
//! Each function operates directly on the filesystem via `std::fs` (bypassing
//! the `mvl_runtime::stdlib::io::Path` wrapper, which enforces IFC at the Rust
//! type level; IFC is not enforced in the LLVM backend).
//!
//! # Return layout
//!
//! Functions returning `Result[T, E]` use `LlvmResult { tag: u8, payload: *mut c_void }`:
//! - `tag = 0` (Ok):  `payload = null` for `Result[Unit,_]`;
//!                    `payload = *mut MvlString` for `Result[String,_]`.
//! - `tag = 1` (Err): `payload = *mut MvlString` (error string).
//!
//! The LLVM emission helper (`wrap_c_result_with_slot`) stores the payload in a
//! stack alloca before passing it to `emit_propagate`/`bind_pattern_vars`, so
//! the direct-pointer convention here is safe: the payload is never freed by the
//! C side.  For `Ok(MvlString*)` results, ownership of the MvlString transfers
//! to the LLVM heap-drop tracking system via the caller's `heap_locals` map.
//! For `Err(MvlString*)` results, the error string leaks (acceptable for MVP).
//!
//! # `path` identity
//!
//! `_mvl_io_path` simply returns its input pointer unchanged.  At the LLVM IR
//! level both `String` and `Path` are represented as `*mut MvlString`, so no
//! wrapping is needed.

use std::slice;

use crate::abi::LlvmEnumError;
use crate::memory::{mvl_string_new, MvlString};
use libc::c_void;

// ── IoError discriminants (must match variant order in std/io.mvl) ────────────
const IO_ERR_NOT_FOUND: u8 = 0;
const IO_ERR_PERMISSION_DENIED: u8 = 1;
const IO_ERR_ALREADY_EXISTS: u8 = 2;
const IO_ERR_OTHER: u8 = 3;

fn io_error_enum(e: &std::io::Error) -> *mut c_void {
    match e.kind() {
        std::io::ErrorKind::NotFound => LlvmEnumError::unit(IO_ERR_NOT_FOUND),
        std::io::ErrorKind::PermissionDenied => LlvmEnumError::unit(IO_ERR_PERMISSION_DENIED),
        std::io::ErrorKind::AlreadyExists => LlvmEnumError::unit(IO_ERR_ALREADY_EXISTS),
        _ => LlvmEnumError::with_str(IO_ERR_OTHER, &e.to_string()),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Read a `MvlString*` as a Rust `String`.  Null / empty are handled gracefully.
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

/// Allocate a new heap `MvlString` from a Rust `&str`.
/// Returns a `*mut c_void` cast of `*mut MvlString`.
#[allow(unsafe_code)]
fn new_mvl_str(s: &str) -> *mut c_void {
    let bytes = s.as_bytes();
    unsafe { mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void }
}

// ── C-ABI Result type ─────────────────────────────────────────────────────────

/// `{i8, ptr}` — matches the LLVM internal `Result[T,E]` layout.
///
/// `tag = 0` → Ok  — `payload` is null (Unit) or `*mut MvlString` (String).
/// `tag = 1` → Err — `payload` is `*mut LlvmEnumError` (enum error value).
#[repr(C)]
pub struct LlvmResult {
    pub tag: u8,
    pub payload: *mut c_void,
}

impl LlvmResult {
    fn ok_unit() -> Self {
        LlvmResult {
            tag: 0,
            payload: std::ptr::null_mut(),
        }
    }
    fn ok_str(s: &str) -> Self {
        LlvmResult {
            tag: 0,
            payload: new_mvl_str(s),
        }
    }
    fn err(e: &std::io::Error) -> Self {
        LlvmResult {
            tag: 1,
            payload: io_error_enum(e),
        }
    }
}

// ── C-ABI exports ─────────────────────────────────────────────────────────────

/// `path(s: String) → Path` — identity at the LLVM level (both are `*mut MvlString`).
///
/// # Safety
/// `s` must be a valid `*mut MvlString` or null.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_path(s: *const MvlString) -> *const MvlString {
    s
}

/// `write(p: Path, content: Tainted[String]) → Result[Unit, String]`
///
/// # Safety
/// Both pointers must be valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_write(
    path: *const MvlString,
    content: *const MvlString,
) -> LlvmResult {
    let p = read_mvl_string(path);
    let c = read_mvl_string(content);
    match std::fs::write(&p, c.as_bytes()) {
        Ok(()) => LlvmResult::ok_unit(),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `append(p: Path, content: Tainted[String]) → Result[Unit, String]`
///
/// # Safety
/// Both pointers must be valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_append(
    path: *const MvlString,
    content: *const MvlString,
) -> LlvmResult {
    use std::io::Write as _;
    let p = read_mvl_string(path);
    let c = read_mvl_string(content);
    let result = std::fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .and_then(|mut f| f.write_all(c.as_bytes()));
    match result {
        Ok(()) => LlvmResult::ok_unit(),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `read_to_string(p: Path) → Result[Tainted[String], String]`
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_read_to_string(path: *const MvlString) -> LlvmResult {
    let p = read_mvl_string(path);
    match std::fs::read_to_string(&p) {
        Ok(contents) => LlvmResult::ok_str(&contents),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `create_dir_all(p: Path) → Result[Unit, String]`
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_create_dir_all(path: *const MvlString) -> LlvmResult {
    let p = read_mvl_string(path);
    match std::fs::create_dir_all(&p) {
        Ok(()) => LlvmResult::ok_unit(),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `remove(p: Path) → Result[Unit, String]`
///
/// Removes a file or directory (uses `symlink_metadata` to distinguish them).
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_remove(path: *const MvlString) -> LlvmResult {
    let p = read_mvl_string(path);
    let result = std::fs::symlink_metadata(&p).and_then(|meta| {
        if meta.is_dir() {
            std::fs::remove_dir(&p)
        } else {
            std::fs::remove_file(&p)
        }
    });
    match result {
        Ok(()) => LlvmResult::ok_unit(),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `exists(p: Path) → Bool` — return 1 if the path exists, 0 otherwise.
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_exists(path: *const MvlString) -> i64 {
    let p = read_mvl_string(path);
    std::path::Path::new(&p).exists() as i64
}

/// `is_file(p: Path) → Bool` — return 1 if the path is a regular file, 0 otherwise.
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_is_file(path: *const MvlString) -> i64 {
    let p = read_mvl_string(path);
    std::path::Path::new(&p).is_file() as i64
}

/// `is_dir(p: Path) → Bool` — return 1 if the path is a directory, 0 otherwise.
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_is_dir(path: *const MvlString) -> i64 {
    let p = read_mvl_string(path);
    std::path::Path::new(&p).is_dir() as i64
}

/// `read_file(p: Clean[String]) → Result[Tainted[String], String]`
///
/// Convenience variant of `read_to_string` accepting a raw string path directly.
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_read_file(path: *const MvlString) -> LlvmResult {
    let p = read_mvl_string(path);
    match std::fs::read_to_string(&p) {
        Ok(contents) => LlvmResult::ok_str(&contents),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `create_symlink(target: Path, link: Path) → Result[Unit, String]`
///
/// # Safety
/// Both pointers must be valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_create_symlink(
    target: *const MvlString,
    link: *const MvlString,
) -> LlvmResult {
    let t = read_mvl_string(target);
    let l = read_mvl_string(link);
    #[cfg(unix)]
    {
        match std::os::unix::fs::symlink(&t, &l) {
            Ok(()) => LlvmResult::ok_unit(),
            Err(e) => LlvmResult::err(&e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (t, l);
        LlvmResult {
            tag: 1,
            payload: LlvmEnumError::with_str(
                IO_ERR_OTHER,
                "symlinks not supported on this platform",
            ),
        }
    }
}

/// `read_link(p: Path) → Result[Tainted[String], String]`
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_read_link(path: *const MvlString) -> LlvmResult {
    let p = read_mvl_string(path);
    match std::fs::read_link(&p) {
        Ok(target) => LlvmResult::ok_str(&target.to_string_lossy()),
        Err(e) => LlvmResult::err(&e),
    }
}

/// `chmod(p: Path, mode: Int) → Result[Unit, String]`
///
/// Sets Unix permission bits.  No-op on non-Unix platforms.
///
/// # Safety
/// `path` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_chmod(path: *const MvlString, mode: i64) -> LlvmResult {
    let p = read_mvl_string(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        match std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode as u32)) {
            Ok(()) => LlvmResult::ok_unit(),
            Err(e) => LlvmResult::err(&e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (p, mode);
        LlvmResult::ok_unit()
    }
}

// ── Standard output / error ───────────────────────────────────────────────────

/// `stdout() → Stdout` — return a unit struct (null ptr sentinel at LLVM level).
///
/// At the LLVM IR level `Stdout` is represented as an opaque pointer; we return
/// a null pointer since the actual I/O uses libc directly in `_mvl_io_stdout_write`.
#[no_mangle]
pub extern "C" fn _mvl_io_stdout() -> *const c_void {
    std::ptr::null()
}

/// `stderr() → Stderr` — return a unit struct (null ptr sentinel at LLVM level).
#[no_mangle]
pub extern "C" fn _mvl_io_stderr() -> *const c_void {
    std::ptr::null()
}

/// `stdout_write(s: Stdout, line: String) → Unit`
///
/// # Safety
/// `line` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_stdout_write(_s: *const c_void, line: *const MvlString) {
    use std::io::Write as _;
    let s = read_mvl_string(line);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// `stderr_write(s: Stderr, line: String) → Unit`
///
/// # Safety
/// `line` must be a valid `MvlString*` for the duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_io_stderr_write(_s: *const c_void, line: *const MvlString) {
    use std::io::Write as _;
    let s = read_mvl_string(line);
    let _ = std::io::stderr().lock().write_all(s.as_bytes());
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{mvl_string_drop, mvl_string_new};

    unsafe fn make_str(s: &str) -> *mut MvlString {
        mvl_string_new(s.as_bytes().as_ptr(), s.len())
    }

    #[test]
    fn path_identity() {
        unsafe {
            let ms = make_str("/tmp/test");
            let out = _mvl_io_path(ms);
            assert_eq!(out, ms as *const MvlString);
            mvl_string_drop(ms);
        }
    }

    #[test]
    fn write_read_roundtrip() {
        let tmp = std::env::temp_dir().join("mvl_runtime_c_io_test_write.txt");
        let path_str = tmp.to_string_lossy().to_string();
        unsafe {
            let path_ms = make_str(&path_str);
            let content_ms = make_str("hello llvm io");
            let wr = _mvl_io_write(path_ms, content_ms);
            assert_eq!(wr.tag, 0, "write should succeed");
            let rd = _mvl_io_read_to_string(path_ms);
            assert_eq!(rd.tag, 0, "read_to_string should succeed");
            let read_back = read_mvl_string(rd.payload as *const MvlString);
            assert_eq!(read_back, "hello llvm io");
            // cleanup
            let _ = std::fs::remove_file(&path_str);
            mvl_string_drop(path_ms);
            mvl_string_drop(content_ms);
        }
    }

    #[test]
    fn append_accumulates() {
        let tmp = std::env::temp_dir().join("mvl_runtime_c_io_test_append.txt");
        let path_str = tmp.to_string_lossy().to_string();
        unsafe {
            let path_ms = make_str(&path_str);
            let c1 = make_str("hello");
            let c2 = make_str(" world");
            let _ = _mvl_io_write(path_ms, c1);
            let ar = _mvl_io_append(path_ms, c2);
            assert_eq!(ar.tag, 0, "append should succeed");
            let rd = _mvl_io_read_to_string(path_ms);
            let s = read_mvl_string(rd.payload as *const MvlString);
            assert_eq!(s, "hello world");
            let _ = std::fs::remove_file(&path_str);
            mvl_string_drop(path_ms);
            mvl_string_drop(c1);
            mvl_string_drop(c2);
        }
    }

    #[test]
    fn create_dir_and_remove() {
        let tmp = std::env::temp_dir().join("mvl_runtime_c_io_test_dir_435");
        let path_str = tmp.to_string_lossy().to_string();
        unsafe {
            let path_ms = make_str(&path_str);
            let cr = _mvl_io_create_dir_all(path_ms);
            assert_eq!(cr.tag, 0, "create_dir_all should succeed");
            let rm = _mvl_io_remove(path_ms);
            assert_eq!(rm.tag, 0, "remove dir should succeed");
            mvl_string_drop(path_ms);
        }
    }

    #[test]
    fn write_err_on_bad_path() {
        unsafe {
            let path_ms = make_str("/nonexistent_dir_mvl/file.txt");
            let content_ms = make_str("data");
            let wr = _mvl_io_write(path_ms, content_ms);
            assert_eq!(wr.tag, 1, "write to bad path should fail");
            mvl_string_drop(path_ms);
            mvl_string_drop(content_ms);
        }
    }
}
