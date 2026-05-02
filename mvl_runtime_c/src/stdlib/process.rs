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

use libc::c_char;
use mvl_runtime::ifc::Clean;
use mvl_runtime::stdlib::process::{self, Child, ProcessOutput, Stdio};

use crate::abi::{c_to_string, string_to_c, MvlOption, MvlResult};

// ── Stdio mode encoding ─────────────────────────────────────────────────────
// Encoded as i8 at the C boundary:
//   0 = Pipe, 1 = Capture, 2 = Inherit, 3 = Devnull

fn decode_stdio(tag: i8) -> Stdio {
    match tag {
        0 => Stdio::Pipe,
        1 => Stdio::Capture,
        2 => Stdio::Inherit,
        _ => Stdio::Devnull,
    }
}

// ── Spawn ───────────────────────────────────────────────────────────────────

/// Spawn a child process.
///
/// `cmd` — NUL-terminated command path (must be `Clean`).
/// `argv` — null-terminated array of `*const c_char` argument pointers.
/// `stdin_mode`, `stdout_mode`, `stderr_mode` — Stdio encoding (0-3).
///
/// Returns an opaque `*mut c_void` handle on success; `MvlResult::err_str`
/// on failure. The handle must eventually be passed to `_mvl_process_wait` or
/// `_mvl_process_kill` to avoid a resource leak.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_spawn(
    cmd: *const c_char,
    argv: *const *const c_char,
    stdin_mode: i8,
    stdout_mode: i8,
    stderr_mode: i8,
) -> MvlResult {
    let cmd_s = unsafe { c_to_string(cmd) };

    // Collect null-terminated argv array.
    let mut args: Vec<Clean<String>> = Vec::new();
    if !argv.is_null() {
        let mut i = 0;
        loop {
            let p = unsafe { *argv.add(i) };
            if p.is_null() {
                break;
            }
            args.push(Clean(unsafe { c_to_string(p) }));
            i += 1;
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
            MvlResult {
                tag: 0,
                payload: Box::into_raw(boxed) as *mut libc::c_void,
                err: std::ptr::null_mut(),
            }
        }
        Err(e) => MvlResult::err_str(&e),
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
pub extern "C" fn _mvl_process_wait(child_ptr: *mut libc::c_void) -> MvlResult {
    if child_ptr.is_null() {
        return MvlResult::err_str("null child handle");
    }
    // Safety: child_ptr was allocated by _mvl_process_spawn via Box::into_raw.
    let child: Child = unsafe { *Box::from_raw(child_ptr as *mut Child) };
    match process::wait(child) {
        Ok(out) => {
            let boxed = Box::new(out);
            MvlResult {
                tag: 0,
                payload: Box::into_raw(boxed) as *mut libc::c_void,
                err: std::ptr::null_mut(),
            }
        }
        Err(e) => MvlResult::err_str(&e),
    }
}

// ── Kill ─────────────────────────────────────────────────────────────────────

/// Send kill signal to the child. Returns the child handle so it can be passed
/// to `_mvl_process_wait` for reaping.
#[no_mangle]
#[allow(unsafe_code)]
pub extern "C" fn _mvl_process_kill(child_ptr: *mut libc::c_void) -> MvlResult {
    if child_ptr.is_null() {
        return MvlResult::err_str("null child handle");
    }
    // Safety: child_ptr was allocated by _mvl_process_spawn or _mvl_process_kill.
    let child: Child = unsafe { *Box::from_raw(child_ptr as *mut Child) };
    match process::kill(child) {
        Ok(child) => {
            let boxed = Box::new(child);
            MvlResult {
                tag: 0,
                payload: Box::into_raw(boxed) as *mut libc::c_void,
                err: std::ptr::null_mut(),
            }
        }
        Err(e) => MvlResult::err_str(&e),
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
    use std::ffi::CString;

    fn null_argv() -> *const *const c_char {
        // A single null pointer — empty argv.
        static NULL_PTR: libc::uintptr_t = 0;
        &NULL_PTR as *const libc::uintptr_t as *const *const c_char
    }

    #[test]
    fn spawn_echo_and_wait() {
        let cmd = CString::new("echo").unwrap();
        let arg1 = CString::new("hello_mvl_c").unwrap();
        // Two-element argv: arg1 + null terminator.
        let argv: [*const c_char; 2] = [arg1.as_ptr(), std::ptr::null()];

        let spawn_r = _mvl_process_spawn(
            cmd.as_ptr(),
            argv.as_ptr(),
            3, // stdin=Devnull
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
            std::ffi::CStr::from_ptr(stdout_opt.payload as *const c_char)
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
        let cmd = CString::new("mvl_nonexistent_xyz_12345").unwrap();
        let r = _mvl_process_spawn(cmd.as_ptr(), null_argv(), 3, 3, 3);
        assert_eq!(r.tag, 1, "spawn of nonexistent command must return Err");
        #[allow(unsafe_code)]
        unsafe {
            libc::free(r.err as *mut libc::c_void)
        };
    }

    #[test]
    fn is_success_helper() {
        assert_eq!(_mvl_process_is_success(0), 1);
        assert_eq!(_mvl_process_is_success(1), 0);
    }

    #[test]
    fn kill_and_wait_terminates() {
        let cmd = CString::new("sleep").unwrap();
        let arg = CString::new("60").unwrap();
        let argv: [*const c_char; 2] = [arg.as_ptr(), std::ptr::null()];
        let r = _mvl_process_spawn(cmd.as_ptr(), argv.as_ptr(), 3, 3, 3);
        assert_eq!(r.tag, 0);
        let kill_r = _mvl_process_kill(r.payload);
        assert_eq!(kill_r.tag, 0, "kill must succeed");
        let wait_r = _mvl_process_wait(kill_r.payload);
        assert_eq!(wait_r.tag, 0, "wait after kill must succeed");
        let status = _mvl_process_output_status(wait_r.payload);
        assert_ne!(status, 0, "killed process must not exit successfully");
        _mvl_process_output_free(wait_r.payload);
    }
}
