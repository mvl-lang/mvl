//! C-ABI exports for `std.process` — wraps `mvl_runtime::stdlib::process` (#432).
//!
//! # Handle representation
//!
//! The Rust `Child`, `ChildStdin`, `ChildStdout`, `ChildStderr` types are boxed
//! and exposed as opaque `*mut c_void` pointers. The caller must not inspect or
//! free them directly; use the corresponding `_mvl_process_*` functions.
//!
//! # LLVM codegen integration status
//!
//! All process symbols are pending LLVM codegen wiring. The C-ABI surface is
//! defined here so the declare helpers in `codegen/runtime_c.rs` can reference
//! them, but no `emit_fn_call` dispatch exists yet.

use libc::c_char;
use mvl_runtime::{
    ifc::Clean,
    stdlib::process::{self, Child, Stdio},
};
use std::ffi::CStr;

unsafe fn cstr_to_string(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    CStr::from_ptr(s).to_string_lossy().into_owned()
}

fn i8_to_stdio(mode: i8) -> Stdio {
    match mode {
        0 => Stdio::Pipe,
        1 => Stdio::Capture,
        2 => Stdio::Inherit,
        _ => Stdio::Devnull,
    }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Spawn a child process. Returns an opaque `Child*` on success, null on error.
///
/// `stdin_mode`, `stdout_mode`, `stderr_mode`:
///   0 = Pipe, 1 = Capture, 2 = Inherit, 3 = Devnull
#[no_mangle]
pub unsafe extern "C" fn _mvl_process_spawn(
    cmd: *const c_char,
    stdin_mode: i8,
    stdout_mode: i8,
    stderr_mode: i8,
) -> *mut libc::c_void {
    let cmd_str = cstr_to_string(cmd);
    match process::spawn(
        Clean(cmd_str),
        vec![],
        i8_to_stdio(stdin_mode),
        i8_to_stdio(stdout_mode),
        i8_to_stdio(stderr_mode),
    ) {
        Ok(child) => Box::into_raw(Box::new(child)) as *mut libc::c_void,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Wait for the child to exit. Consumes the `Child*` — must not be used after.
/// Returns 0 on success exit, the exit code on failure, -1 on internal error.
#[no_mangle]
pub unsafe extern "C" fn _mvl_process_wait(child_ptr: *mut libc::c_void) -> i64 {
    if child_ptr.is_null() {
        return -1;
    }
    let child = *Box::from_raw(child_ptr as *mut Child);
    match process::wait(child) {
        Ok(out) => process::exit_code(out.status),
        Err(_) => -1,
    }
}

/// Send SIGKILL to the child. Returns 0 on success, -1 on error.
/// Ownership of the `Child*` is retained — pass it to `_mvl_process_wait` next.
#[no_mangle]
pub unsafe extern "C" fn _mvl_process_kill(child_ptr: *mut libc::c_void) -> i64 {
    if child_ptr.is_null() {
        return -1;
    }
    let child = std::ptr::read(child_ptr as *mut Child);
    match process::kill(child) {
        Ok(new_child) => {
            std::ptr::write(child_ptr as *mut Child, new_child);
            0
        }
        Err(_) => -1,
    }
}

/// Free a `Child*` without waiting (use only after kill+wait or on error paths).
#[no_mangle]
pub unsafe extern "C" fn _mvl_process_drop_child(child_ptr: *mut libc::c_void) {
    if !child_ptr.is_null() {
        drop(Box::from_raw(child_ptr as *mut Child));
    }
}
