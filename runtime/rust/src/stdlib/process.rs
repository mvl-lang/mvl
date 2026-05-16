// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.process` stdlib functions.
//!
//! Provides real process spawning and lifecycle backing for the stubs declared
//! in `std/process.mvl`. Re-exported via `mvl_runtime::prelude::*`.

use crate::ifc::{Clean, Tainted};
use std::io::{Read, Write};

// ── Error type ────────────────────────────────────────────────────────────

/// Mirrors the `ProcessError` enum declared in `std/process.mvl`.
/// Variant order and names must stay in sync with the MVL definition.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessError {
    NotFound,
    PermissionDenied,
    Other(String),
}

// ── Stdio configuration ────────────────────────────────────────────────────

/// Configures how a child process's stdio stream is connected.
/// Mirrors the `Stdio` enum declared in `std/process.mvl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdio {
    /// Connect to a pipe for streaming reads/writes via `stdout_read` / `stdin_write`.
    Pipe,
    /// Collect all output into a buffer; available in `ProcessOutput` after `wait`.
    Capture,
    /// Inherit the parent's stdio stream.
    Inherit,
    /// Redirect to `/dev/null` (discard).
    Devnull,
}

fn to_std_stdio(mode: &Stdio) -> std::process::Stdio {
    match mode {
        // Both Pipe and Capture use an OS pipe; the difference is whether `wait`
        // buffers the output (Capture) or the caller reads it via stdout_read (Pipe).
        Stdio::Pipe | Stdio::Capture => std::process::Stdio::piped(),
        Stdio::Inherit => std::process::Stdio::inherit(),
        Stdio::Devnull => std::process::Stdio::null(),
    }
}

// ── Handle types ───────────────────────────────────────────────────────────

/// Handle to the stdin pipe of a child process.
pub struct ChildStdin(std::process::ChildStdin);

/// Handle to the stdout pipe of a child process.
pub struct ChildStdout(std::process::ChildStdout);

/// Handle to the stderr pipe of a child process.
pub struct ChildStderr(std::process::ChildStderr);

// ── Child process ──────────────────────────────────────────────────────────

/// A running child process.
/// Mirrors the `Child` struct declared in `std/process.mvl`.
pub struct Child {
    /// Stdin pipe handle; `Some` when spawned with `Stdio::Pipe` on stdin.
    pub stdin: Option<ChildStdin>,
    /// Stdout pipe handle; `Some` when spawned with `Stdio::Pipe` on stdout.
    pub stdout: Option<ChildStdout>,
    /// Stderr pipe handle; `Some` when spawned with `Stdio::Pipe` on stderr.
    pub stderr: Option<ChildStderr>,
    inner: std::process::Child,
    // Track whether stdout/stderr should be buffered into ProcessOutput by `wait`.
    stdout_capture: bool,
    stderr_capture: bool,
}

// ── Exit status and output ─────────────────────────────────────────────────

/// The exit status of a finished child process.
/// Mirrors the `ExitStatus` enum declared in `std/process.mvl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus {
    /// Process exited with code 0.
    Success,
    /// Process exited with a non-zero code.
    Failed(i64),
}

/// The result of waiting for a child process.
/// Mirrors the `ProcessOutput` struct declared in `std/process.mvl`.
pub struct ProcessOutput {
    /// How the process terminated.
    pub status: ExitStatus,
    /// Captured stdout; `Some` when the corresponding mode was `Stdio::Capture`.
    pub stdout: Option<Tainted<String>>,
    /// Captured stderr; `Some` when the corresponding mode was `Stdio::Capture`.
    pub stderr: Option<Tainted<String>>,
}

// ── Process spawning ────────────────────────────────────────────────────────

/// Spawn a child process.
///
/// Both `cmd` and each element of `args` must be `Clean<String>` to prevent
/// command injection from tainted sources.
pub fn spawn(
    cmd: Clean<String>,
    args: Vec<Clean<String>>,
    stdin_mode: Stdio,
    stdout_mode: Stdio,
    stderr_mode: Stdio,
) -> Result<Child, ProcessError> {
    let stdout_capture = stdout_mode == Stdio::Capture;
    let stderr_capture = stderr_mode == Stdio::Capture;

    let mut builder = std::process::Command::new(&**cmd);
    for arg in &args {
        builder.arg(&**arg);
    }
    builder
        .stdin(to_std_stdio(&stdin_mode))
        .stdout(to_std_stdio(&stdout_mode))
        .stderr(to_std_stdio(&stderr_mode));

    let mut child = builder
        .spawn()
        .map_err(|e| sanitize_spawn_error(e.kind()))?;

    let stdin = child.stdin.take().map(ChildStdin);
    let stdout = child.stdout.take().map(ChildStdout);
    let stderr = child.stderr.take().map(ChildStderr);

    Ok(Child {
        stdin,
        stdout,
        stderr,
        inner: child,
        stdout_capture,
        stderr_capture,
    })
}

fn sanitize_spawn_error(kind: std::io::ErrorKind) -> ProcessError {
    match kind {
        std::io::ErrorKind::NotFound => ProcessError::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessError::PermissionDenied,
        _ => ProcessError::Other("spawn failed".to_string()),
    }
}

// ── Pipe I/O ────────────────────────────────────────────────────────────────

/// Write bytes to the child's stdin pipe.
pub fn stdin_write(handle: ChildStdin, data: String) -> Result<(), ProcessError> {
    let mut h = handle.0;
    h.write_all(data.as_bytes())
        .map_err(|_| ProcessError::Other("stdin write failed".to_string()))
}

/// Read all available output from the child's stdout pipe.
pub fn stdout_read(handle: ChildStdout) -> Result<Tainted<String>, ProcessError> {
    let mut h = handle.0;
    let mut buf = String::new();
    h.read_to_string(&mut buf)
        .map_err(|_| ProcessError::Other("stdout read failed".to_string()))?;
    Ok(Tainted(buf))
}

/// Read all available output from the child's stderr pipe.
pub fn stderr_read(handle: ChildStderr) -> Result<Tainted<String>, ProcessError> {
    let mut h = handle.0;
    let mut buf = String::new();
    h.read_to_string(&mut buf)
        .map_err(|_| ProcessError::Other("stderr read failed".to_string()))?;
    Ok(Tainted(buf))
}

// ── Process lifecycle ───────────────────────────────────────────────────────

/// Wait for the child to exit. Consumes the `Child`.
///
/// Closes stdin before waiting. Buffers stdout/stderr into `ProcessOutput`
/// when the corresponding mode was `Stdio::Capture`; otherwise drops the pipe.
pub fn wait(mut child: Child) -> Result<ProcessOutput, ProcessError> {
    drop(child.stdin.take());

    let stdout_data = if child.stdout_capture {
        child.stdout.take().map(|h| {
            let mut buf = String::new();
            let mut pipe = h.0;
            pipe.read_to_string(&mut buf).unwrap_or(0);
            Tainted(buf)
        })
    } else {
        drop(child.stdout.take());
        None
    };

    let stderr_data = if child.stderr_capture {
        child.stderr.take().map(|h| {
            let mut buf = String::new();
            let mut pipe = h.0;
            pipe.read_to_string(&mut buf).unwrap_or(0);
            Tainted(buf)
        })
    } else {
        drop(child.stderr.take());
        None
    };

    let status = child
        .inner
        .wait()
        .map_err(|_| ProcessError::Other("wait failed".to_string()))?;
    let exit_status = if status.success() {
        ExitStatus::Success
    } else {
        ExitStatus::Failed(status.code().unwrap_or(-1) as i64)
    };

    Ok(ProcessOutput {
        status: exit_status,
        stdout: stdout_data,
        stderr: stderr_data,
    })
}

/// Send SIGKILL (Unix) or TerminateProcess (Windows) to the child.
///
/// Returns the `Child` handle so it can be passed to `wait` for reaping.
pub fn kill(mut child: Child) -> Result<Child, ProcessError> {
    child
        .inner
        .kill()
        .map_err(|_| ProcessError::Other("kill failed".to_string()))?;
    Ok(child)
}

// ── ExitStatus helpers (pure) ───────────────────────────────────────────────

/// Return true if the exit status represents success (code 0).
pub fn is_success(status: ExitStatus) -> bool {
    matches!(status, ExitStatus::Success)
}

/// Return the integer exit code, or 0 for success.
pub fn exit_code(status: ExitStatus) -> i64 {
    match status {
        ExitStatus::Success => 0,
        ExitStatus::Failed(code) => code,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_echo_captures_stdout() {
        let child = spawn(
            Clean("echo".to_string()),
            vec![Clean("hello mvl".to_string())],
            Stdio::Devnull,
            Stdio::Capture,
            Stdio::Devnull,
        )
        .expect("echo must spawn");

        let out = wait(child).expect("wait must succeed");
        assert!(is_success(out.status.clone()));
        let stdout = out.stdout.expect("stdout must be captured");
        assert!(stdout.0.contains("hello mvl"), "got: {}", stdout.0);
    }

    #[test]
    fn spawn_true_exits_success() {
        let child = spawn(
            Clean("true".to_string()),
            vec![],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Devnull,
        )
        .expect("true must spawn");

        let out = wait(child).expect("wait must succeed");
        assert_eq!(out.status, ExitStatus::Success);
        assert_eq!(exit_code(out.status), 0);
    }

    #[test]
    fn spawn_false_exits_failed() {
        let child = spawn(
            Clean("false".to_string()),
            vec![],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Devnull,
        )
        .expect("false must spawn");

        let out = wait(child).expect("wait must succeed");
        assert!(!is_success(out.status.clone()));
        assert_ne!(exit_code(out.status), 0);
    }

    #[test]
    fn spawn_nonexistent_command_returns_err() {
        let result = spawn(
            Clean("mvl_nonexistent_cmd_xyz_12345".to_string()),
            vec![],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Devnull,
        );
        assert!(result.is_err());
    }

    #[test]
    fn stdin_write_and_capture_via_cat() {
        let mut child = spawn(
            Clean("cat".to_string()),
            vec![],
            Stdio::Pipe,
            Stdio::Capture,
            Stdio::Devnull,
        )
        .expect("cat must spawn");

        let stdin_handle = child.stdin.take().expect("stdin must be piped");
        stdin_write(stdin_handle, "mvl input\n".to_string()).expect("stdin_write must succeed");

        let out = wait(child).expect("wait must succeed");
        let stdout = out.stdout.expect("stdout captured");
        assert_eq!(stdout.0, "mvl input\n");
    }

    #[test]
    fn spawn_captures_stderr() {
        let child = spawn(
            Clean("sh".to_string()),
            vec![
                Clean("-c".to_string()),
                Clean("echo errout >&2".to_string()),
            ],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Capture,
        )
        .expect("sh must spawn");

        let out = wait(child).expect("wait must succeed");
        let stderr = out.stderr.expect("stderr must be captured");
        assert!(stderr.0.contains("errout"), "got: {:?}", stderr.0);
        assert!(
            out.stdout.is_none(),
            "stdout must be None when mode is Devnull"
        );
    }

    #[test]
    fn stdout_pipe_mode_readable_via_stdout_read() {
        let mut child = spawn(
            Clean("echo".to_string()),
            vec![Clean("piped output".to_string())],
            Stdio::Devnull,
            Stdio::Pipe,
            Stdio::Devnull,
        )
        .expect("echo must spawn");

        let handle = child.stdout.take().expect("stdout must be piped");
        let data = stdout_read(handle).expect("stdout_read must succeed");
        assert!(data.0.contains("piped output"), "got: {:?}", data.0);
        // After reading the pipe, wait must still succeed.
        let out = wait(child).expect("wait after stdout_read must succeed");
        assert!(is_success(out.status));
        // stdout was already consumed — ProcessOutput.stdout must be None.
        assert!(out.stdout.is_none());
    }

    #[test]
    fn kill_terminates_long_running_process() {
        let child = spawn(
            Clean("sleep".to_string()),
            vec![Clean("60".to_string())],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Devnull,
        )
        .expect("sleep must spawn");

        let child = kill(child).expect("kill must succeed");
        let out = wait(child).expect("wait after kill must succeed");
        assert!(
            !is_success(out.status.clone()),
            "killed process must not exit successfully"
        );
    }

    #[test]
    fn wait_stdout_is_none_when_mode_is_devnull() {
        let child = spawn(
            Clean("echo".to_string()),
            vec![Clean("ignored".to_string())],
            Stdio::Devnull,
            Stdio::Devnull,
            Stdio::Devnull,
        )
        .expect("echo must spawn");

        let out = wait(child).expect("wait must succeed");
        assert!(out.stdout.is_none(), "stdout must be None for Devnull mode");
        assert!(out.stderr.is_none(), "stderr must be None for Devnull mode");
    }

    #[test]
    fn spawn_with_multiple_args() {
        let child = spawn(
            Clean("printf".to_string()),
            vec![
                Clean("%s %s".to_string()),
                Clean("hello".to_string()),
                Clean("world".to_string()),
            ],
            Stdio::Devnull,
            Stdio::Capture,
            Stdio::Devnull,
        )
        .expect("printf must spawn");

        let out = wait(child).expect("wait must succeed");
        let stdout = out.stdout.expect("stdout captured");
        assert_eq!(stdout.0, "hello world");
    }

    #[test]
    fn is_success_and_exit_code_pure_helpers() {
        assert!(is_success(ExitStatus::Success));
        assert!(!is_success(ExitStatus::Failed(1)));
        assert_eq!(exit_code(ExitStatus::Success), 0);
        assert_eq!(exit_code(ExitStatus::Failed(42)), 42);
    }
}
