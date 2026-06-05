// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.process` stdlib functions.
//!
//! Mirrors `mvl_runtime::stdlib::process`. Process handles cannot cross the
//! C-ABI boundary directly — they are heap-allocated and passed as opaque
//! `*mut c_void` pointers. The LLVM backend holds these pointers and passes
//! them back to subsequent calls.
//!
//! # Handle lifetime
//!
//! - `_mvl_process_spawn` returns a `*mut Child` (opaque handle).
//! - `_mvl_process_wait` / `_mvl_process_kill` consume the handle — callers
//!   must NOT use the pointer after passing it to these functions.
//! - `_mvl_process_output_free` releases a `ProcessOutput` handle.

use mvl_runtime::ifc::Clean;
use mvl_runtime::stdlib::process::{self, Child, ProcessOutput, Stdio};

use crate::abi::{string_to_c, LlvmResult, MvlOption};
use crate::memory::{MvlArray, MvlString};

/// Convert a `ProcessError` into an `LlvmResult::err_mvl` (heap-allocated MvlString).
#[allow(unsafe_code)]
fn process_err_result(e: &process::ProcessError) -> LlvmResult {
    let msg = match e {
        process::ProcessError::NotFound => "command not found",
        process::ProcessError::PermissionDenied => "permission denied",
        process::ProcessError::Other(msg) => msg.as_str(),
    };
    let str_ptr = unsafe { crate::memory::mvl_string_new(msg.as_ptr(), msg.len()) };
    LlvmResult::err_mvl(str_ptr as *mut libc::c_void)
}

// ── Stdio mode encoding ─────────────────────────────────────────────────────
// Encoded as i64 at the LLVM boundary (unit enum → i64 discriminant):
//   0 = Pipe, 1 = Capture, 2 = Inherit, 3 = Devnull

fn decode_stdio(tag: i64) -> Stdio {
    match tag {
        0 => Stdio::Pipe,
        1 => Stdio::Capture,
        2 => Stdio::Inherit,
        _ => Stdio::Devnull,
    }
}

/// Extract a Rust `String` from a `*const MvlString`.
///
/// # Safety
/// `s` must be a valid `MvlString` pointer or null.
#[inline]
#[allow(unsafe_code)]
unsafe fn mvl_str_to_string(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = (*s).len as usize;
    if len == 0 || (*s).ptr.is_null() {
        return String::new();
    }
    let bytes = std::slice::from_raw_parts((*s).ptr, len);
    std::str::from_utf8(bytes).unwrap_or("").to_string()
}

// ── Spawn ───────────────────────────────────────────────────────────────────

/// Spawn a child process.
///
/// `cmd` — `*const MvlString` command path.
/// `argv` — `*const MvlArray` of `MvlString` pointers (the argument list).
/// `stdin_mode`, `stdout_mode`, `stderr_mode` — Stdio encoding as i64
///   (unit enum discriminant: 0=Pipe, 1=Capture, 2=Inherit, 3=Devnull).
///
/// Returns an opaque `*mut c_void` handle on success; `LlvmResult::err_mvl`
/// on failure. The handle must eventually be passed to `_mvl_process_wait` or
/// `_mvl_process_kill` to avoid a resource leak.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_spawn(
    cmd: *const MvlString,
    argv: *const MvlArray,
    stdin_mode: i64,
    stdout_mode: i64,
    stderr_mode: i64,
) -> LlvmResult {
    let cmd_s = unsafe { mvl_str_to_string(cmd) };

    // Extract argument strings from the MvlArray of MvlString pointers.
    let mut args: Vec<Clean<String>> = Vec::new();
    if !argv.is_null() {
        let arr = unsafe { &*argv };
        let len = arr.len as usize;
        let elem_size = arr.elem_size as usize;
        for i in 0..len {
            // Each element is a pointer-sized slot containing a *const MvlString.
            let slot_ptr = unsafe { arr.ptr.add(i * elem_size) } as *const *const MvlString;
            let str_ptr = unsafe { *slot_ptr };
            args.push(Clean(unsafe { mvl_str_to_string(str_ptr) }));
        }
    }

    match process::spawn(
        Clean(cmd_s),
        args,
        decode_stdio(stdin_mode),
        decode_stdio(stdout_mode),
        decode_stdio(stderr_mode),
    ) {
        Ok(child) => {
            let boxed = Box::new(child);
            LlvmResult::ok_ptr(Box::into_raw(boxed) as *mut libc::c_void)
        }
        Err(e) => process_err_result(&e),
    }
}

// ── Wait ────────────────────────────────────────────────────────────────────

/// Wait for the child to exit. Consumes the handle.
///
/// `child_ptr` — opaque handle returned by `_mvl_process_spawn`.
/// Returns a `*mut ProcessOutput` handle on success. Caller must eventually
/// call `_mvl_process_output_free` to release it.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_wait(child_ptr: *mut libc::c_void) -> LlvmResult {
    if child_ptr.is_null() {
        let msg = "null child handle";
        let str_ptr = unsafe { crate::memory::mvl_string_new(msg.as_ptr(), msg.len()) };
        return LlvmResult::err_mvl(str_ptr as *mut libc::c_void);
    }
    // Safety: child_ptr was allocated by _mvl_process_spawn via Box::into_raw.
    let child: Child = unsafe { *Box::from_raw(child_ptr as *mut Child) };
    match process::wait(child) {
        Ok(out) => {
            let boxed = Box::new(out);
            LlvmResult::ok_ptr(Box::into_raw(boxed) as *mut libc::c_void)
        }
        Err(e) => process_err_result(&e),
    }
}

// ── Kill ─────────────────────────────────────────────────────────────────────

/// Send kill signal to the child.
///
/// The child handle (`child_ptr`) is **unconditionally consumed** by this call
/// regardless of success or failure.  The caller must NOT use `child_ptr` again
/// after calling this function.
///
/// On success (`tag=0`): `payload` is a new `*mut Child` handle — pass it to
/// `_mvl_process_wait` to reap the process.
///
/// On error (`tag=1`): the child has been dropped (no recovered handle exists),
/// `err` holds an error message (caller frees with `libc::free`).
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_kill(child_ptr: *mut libc::c_void) -> LlvmResult {
    if child_ptr.is_null() {
        let msg = "null child handle";
        let str_ptr = unsafe { crate::memory::mvl_string_new(msg.as_ptr(), msg.len()) };
        return LlvmResult::err_mvl(str_ptr as *mut libc::c_void);
    }
    // Safety: child_ptr was allocated by _mvl_process_spawn or a prior _mvl_process_kill.
    // The child is unconditionally moved out here; child_ptr must not be used after this call.
    let child: Child = unsafe { *Box::from_raw(child_ptr as *mut Child) };
    match process::kill(child) {
        Ok(child) => {
            let boxed = Box::new(child);
            LlvmResult::ok_ptr(Box::into_raw(boxed) as *mut libc::c_void)
        }
        // child was moved into process::kill and dropped on error — no handle to return.
        Err(e) => process_err_result(&e),
    }
}

// ── ProcessOutput accessors ──────────────────────────────────────────────────

/// Return the exit status tag from a `ProcessOutput` handle.
/// `0` = Success, `1` = Failed.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_output_status(out_ptr: *const libc::c_void) -> i8 {
    if out_ptr.is_null() {
        return 1;
    }
    // Safety: out_ptr was returned by _mvl_process_wait.
    let out = unsafe { &*(out_ptr as *const ProcessOutput) };
    match out.status {
        mvl_runtime::stdlib::process::ExitStatus::Success => 0,
        mvl_runtime::stdlib::process::ExitStatus::Failed(_) => 1,
    }
}

/// Return the exit code from a `ProcessOutput` handle.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_output_exit_code(out_ptr: *const libc::c_void) -> i64 {
    if out_ptr.is_null() {
        return -1;
    }
    // Safety: out_ptr was returned by _mvl_process_wait.
    let out = unsafe { &*(out_ptr as *const ProcessOutput) };
    process::exit_code(out.status.clone())
}

/// Return captured stdout from a `ProcessOutput` handle.
///
/// Returns `MvlOption::some_str(ptr)` if stdout was captured (caller frees),
/// or `MvlOption::none()` if stdout was not captured.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_output_stdout(out_ptr: *const libc::c_void) -> MvlOption {
    if out_ptr.is_null() {
        return MvlOption::none();
    }
    // Safety: out_ptr was returned by _mvl_process_wait.
    let out = unsafe { &*(out_ptr as *const ProcessOutput) };
    match &out.stdout {
        Some(mvl_runtime::ifc::Tainted(s)) => MvlOption::some_str(string_to_c(s)),
        None => MvlOption::none(),
    }
}

/// Return captured stderr from a `ProcessOutput` handle.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_output_stderr(out_ptr: *const libc::c_void) -> MvlOption {
    if out_ptr.is_null() {
        return MvlOption::none();
    }
    // Safety: out_ptr was returned by _mvl_process_wait.
    let out = unsafe { &*(out_ptr as *const ProcessOutput) };
    match &out.stderr {
        Some(mvl_runtime::ifc::Tainted(s)) => MvlOption::some_str(string_to_c(s)),
        None => MvlOption::none(),
    }
}

/// Free a `ProcessOutput` handle returned by `_mvl_process_wait`.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_output_free(out_ptr: *mut libc::c_void) {
    if !out_ptr.is_null() {
        // Safety: out_ptr was allocated by _mvl_process_wait via Box::into_raw.
        drop(unsafe { Box::from_raw(out_ptr as *mut ProcessOutput) });
    }
}

// ── ExitStatus pure helpers ──────────────────────────────────────────────────

/// Return 1 if `status_tag` represents success (0), else 0.
#[no_mangle]
pub extern "C" fn _mvl_process_is_success(status_tag: i8) -> i8 {
    if status_tag == 0 {
        1
    } else {
        0
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::mvl_string_new;
    use crate::memory_ops::mvl_array_push;

    /// Create a `*const MvlString` from a Rust `&str`.
    #[allow(unsafe_code)]
    fn make_mvl_str(s: &str) -> *const MvlString {
        unsafe { mvl_string_new(s.as_ptr(), s.len()) as *const MvlString }
    }

    /// Create a `*const MvlArray` containing the given `MvlString` pointers.
    #[allow(unsafe_code)]
    fn make_mvl_argv(strs: &[*const MvlString]) -> *const MvlArray {
        let arr = unsafe { crate::memory::mvl_array_new(8, strs.len().max(4)) };
        for s in strs {
            let mut slot = *s as usize;
            let slot_ptr = &mut slot as *mut usize as *const u8;
            unsafe { mvl_array_push(arr, slot_ptr) };
        }
        arr as *const MvlArray
    }

    /// Create an empty `MvlArray` (no arguments).
    #[allow(unsafe_code)]
    fn empty_argv() -> *const MvlArray {
        unsafe { crate::memory::mvl_array_new(8, 4) as *const MvlArray }
    }

    #[test]
    fn spawn_echo_and_wait() {
        let cmd = make_mvl_str("echo");
        let arg1 = make_mvl_str("hello_mvl_c");
        let argv = make_mvl_argv(&[arg1]);

        let spawn_r = _mvl_process_spawn(
            cmd, argv, 3, // stdin=Devnull
            1, // stdout=Capture
            3, // stderr=Devnull
        );
        assert_eq!(spawn_r.tag, 0, "spawn must succeed");

        let wait_r = _mvl_process_wait(spawn_r.payload);
        assert_eq!(wait_r.tag, 0, "wait must succeed");

        let status = _mvl_process_output_status(wait_r.payload);
        assert_eq!(status, 0, "echo must exit successfully");

        let stdout_opt = _mvl_process_output_stdout(wait_r.payload);
        assert_eq!(stdout_opt.tag, 1, "stdout must be captured");
        #[allow(unsafe_code)]
        let s = unsafe {
            std::ffi::CStr::from_ptr(stdout_opt.payload as *const libc::c_char)
                .to_str()
                .unwrap()
                .to_owned()
        };
        assert!(s.contains("hello_mvl_c"), "got: {s}");

        // Free captured stdout string and output handle.
        #[allow(unsafe_code)]
        unsafe {
            libc::free(stdout_opt.payload)
        };
        _mvl_process_output_free(wait_r.payload);
    }

    #[test]
    fn spawn_nonexistent_command_returns_err() {
        let cmd = make_mvl_str("mvl_nonexistent_xyz_12345");
        let r = _mvl_process_spawn(cmd, empty_argv(), 3, 3, 3);
        assert_eq!(r.tag, 1, "spawn of nonexistent command must return Err");
        // LlvmResult stores error message in payload (MvlString*)
        assert!(!r.payload.is_null(), "error payload must not be null");
    }

    #[test]
    fn is_success_helper() {
        assert_eq!(_mvl_process_is_success(0), 1);
        assert_eq!(_mvl_process_is_success(1), 0);
    }

    #[test]
    fn kill_and_wait_terminates() {
        let cmd = make_mvl_str("sleep");
        let arg = make_mvl_str("60");
        let argv = make_mvl_argv(&[arg]);
        let r = _mvl_process_spawn(cmd, argv, 3, 3, 3);
        assert_eq!(r.tag, 0);
        let kill_r = _mvl_process_kill(r.payload);
        assert_eq!(kill_r.tag, 0, "kill must succeed");
        let wait_r = _mvl_process_wait(kill_r.payload);
        assert_eq!(wait_r.tag, 0, "wait after kill must succeed");
        let status = _mvl_process_output_status(wait_r.payload);
        assert_ne!(status, 0, "killed process must not exit successfully");
        _mvl_process_output_free(wait_r.payload);
    }

    #[test]
    fn wait_null_returns_err() {
        let r = _mvl_process_wait(std::ptr::null_mut());
        assert_eq!(r.tag, 1, "wait(null) must return Err");
        assert!(!r.payload.is_null(), "error payload must not be null");
    }

    #[test]
    fn kill_null_returns_err() {
        let r = _mvl_process_kill(std::ptr::null_mut());
        assert_eq!(r.tag, 1, "kill(null) must return Err");
        assert!(!r.payload.is_null(), "error payload must not be null");
    }

    #[test]
    fn output_free_null_is_safe() {
        // Must not crash or panic.
        _mvl_process_output_free(std::ptr::null_mut());
    }
}
