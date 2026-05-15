// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI exports for `std.net` stdlib functions — LLVM backend path (#779).
//!
//! TcpListener and TcpStream are heap-allocated Rust structs returned as raw
//! `*mut c_void` pointers.  The LLVM IR treats them as opaque pointers and
//! passes them through to the next C-ABI call without dereferencing.
//!
//! # Return layout
//!
//! Same `LlvmResult { tag: u8, payload: *mut c_void }` convention as io.rs:
//! - `tag = 0` (Ok):  `payload = *mut c_void` (boxed handle or null for Unit).
//! - `tag = 1` (Err): `payload = *mut MvlString` (error message).
//!
//! # Ownership
//!
//! The Box created by `_mvl_net_tcp_listen` / `_mvl_net_tcp_accept` is owned
//! by the caller.  It must be freed by calling `_mvl_net_tcp_close_listener` /
//! `_mvl_net_tcp_close_stream`, which drop the Box and close the socket.

use std::io::{Read, Write};

use crate::memory::{mvl_string_new, MvlString};
use libc::c_void;

use super::io::LlvmResult;

// ── helpers ───────────────────────────────────────────────────────────────────

#[allow(unsafe_code)]
unsafe fn read_mvl_string(s: *const MvlString) -> String {
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

fn new_mvl_str(s: &str) -> *mut c_void {
    let bytes = s.as_bytes();
    #[allow(unsafe_code)]
    unsafe {
        mvl_string_new(bytes.as_ptr(), bytes.len()) as *mut c_void
    }
}

fn sanitize_net_error(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => "address already in use",
        std::io::ErrorKind::ConnectionRefused => "connection refused",
        std::io::ErrorKind::ConnectionReset => "connection reset",
        std::io::ErrorKind::TimedOut => "connection timed out",
        std::io::ErrorKind::BrokenPipe => "broken pipe",
        _ => "network error",
    }
}

// ── C-ABI exports ─────────────────────────────────────────────────────────────

/// `tcp_listen(host: String, port: Int) → Result[TcpListener, String]`
///
/// Binds a TCP listener on `host:port`.  On success the Ok payload is a
/// heap-allocated `Box<std::net::TcpListener>` cast to `*mut c_void`.
///
/// # Safety
/// `host` must be a valid `MvlString*`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_listen(host: *const MvlString, port: i64) -> LlvmResult {
    let host_str = read_mvl_string(host);
    let addr = format!("{}:{}", host_str, port);
    match std::net::TcpListener::bind(&addr) {
        Ok(listener) => LlvmResult {
            tag: 0,
            payload: Box::into_raw(Box::new(listener)) as *mut c_void,
        },
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_connect(host: String, port: Int) → Result[TcpStream, String]`
///
/// Connects to a remote TCP server at `host:port`.
///
/// # Safety
/// `host` must be a valid `MvlString*`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_connect(host: *const MvlString, port: i64) -> LlvmResult {
    let host_str = read_mvl_string(host);
    let addr = format!("{}:{}", host_str, port);
    match std::net::TcpStream::connect(&addr) {
        Ok(stream) => LlvmResult {
            tag: 0,
            payload: Box::into_raw(Box::new(stream)) as *mut c_void,
        },
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_accept(listener: TcpListener) → Result[TcpStream, String]`
///
/// Accepts the next connection on the listener.  `listener_ptr` is the raw
/// pointer returned by `_mvl_net_tcp_listen`.  The listener stays open.
///
/// # Safety
/// `listener_ptr` must be a valid `*mut std::net::TcpListener` for the
/// duration of the call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_accept(listener_ptr: *mut c_void) -> LlvmResult {
    let listener = &*(listener_ptr as *mut std::net::TcpListener);
    match listener.accept() {
        Ok((stream, _addr)) => LlvmResult {
            tag: 0,
            payload: Box::into_raw(Box::new(stream)) as *mut c_void,
        },
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_read(stream: TcpStream) → Result[Tainted[String], String]`
///
/// Reads all available bytes from the stream.  Blocks until the remote side
/// closes the write half.  Returns `Tainted[String]` (at the C level: an
/// `MvlString*` Ok payload).
///
/// # Safety
/// `stream_ptr` must be a valid `*mut std::net::TcpStream`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_read(stream_ptr: *mut c_void) -> LlvmResult {
    let stream = &mut *(stream_ptr as *mut std::net::TcpStream);
    let mut buf = Vec::new();
    match stream.read_to_end(&mut buf) {
        Ok(_) => {
            let s = String::from_utf8_lossy(&buf);
            LlvmResult {
                tag: 0,
                payload: new_mvl_str(&s),
            }
        }
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_write(stream: TcpStream, data: String) → Result[Unit, String]`
///
/// Writes all bytes of `data` to the stream.
///
/// # Safety
/// `stream_ptr` must be a valid `*mut std::net::TcpStream`.
/// `data` must be a valid `MvlString*`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_write(
    stream_ptr: *mut c_void,
    data: *const MvlString,
) -> LlvmResult {
    let stream = &mut *(stream_ptr as *mut std::net::TcpStream);
    let s = read_mvl_string(data);
    match stream.write_all(s.as_bytes()) {
        Ok(()) => LlvmResult {
            tag: 0,
            payload: std::ptr::null_mut(),
        },
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_listener_port(listener: TcpListener) → Result[Int, String]`
///
/// Returns the local port as an i64 wrapped in LlvmResult.
///
/// # Safety
/// `listener_ptr` must be a valid `*mut std::net::TcpListener`.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_listener_port(listener_ptr: *mut c_void) -> LlvmResult {
    if listener_ptr.is_null() {
        return LlvmResult {
            tag: 1,
            payload: new_mvl_str("invalid listener handle"),
        };
    }
    let listener = &*(listener_ptr as *mut std::net::TcpListener);
    match listener.local_addr() {
        Ok(addr) => {
            let port = addr.port() as i64;
            LlvmResult {
                tag: 0,
                payload: port as *mut c_void,
            }
        }
        Err(e) => LlvmResult {
            tag: 1,
            payload: new_mvl_str(sanitize_net_error(&e)),
        },
    }
}

/// `tcp_close_listener(listener: TcpListener) → Unit`
///
/// Drops the boxed listener, closing the socket and releasing the port.
///
/// # Safety
/// `listener_ptr` must be a valid `*mut std::net::TcpListener` previously
/// returned by `_mvl_net_tcp_listen`.  Must not be used after this call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_close_listener(listener_ptr: *mut c_void) {
    if !listener_ptr.is_null() {
        drop(Box::from_raw(listener_ptr as *mut std::net::TcpListener));
    }
}

/// `tcp_close_stream(stream: TcpStream) → Unit`
///
/// Drops the boxed stream, closing the connection.
///
/// # Safety
/// `stream_ptr` must be a valid `*mut std::net::TcpStream` previously
/// returned by `_mvl_net_tcp_accept`.  Must not be used after this call.
#[no_mangle]
#[allow(unsafe_code)]
pub unsafe extern "C" fn _mvl_net_tcp_close_stream(stream_ptr: *mut c_void) {
    if !stream_ptr.is_null() {
        drop(Box::from_raw(stream_ptr as *mut std::net::TcpStream));
    }
}
