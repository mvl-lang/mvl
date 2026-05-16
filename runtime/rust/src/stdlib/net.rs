// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.net` stdlib functions.
//!
//! TcpListener and TcpStream are opaque i64 handle wrappers backed by a
//! thread-local handle table.  The `Copy` impl means MVL value semantics
//! work naturally: passing a handle to a function does not invalidate it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::ifc::Tainted;

// ── Handle types ──────────────────────────────────────────────────────────────

/// Opaque handle to a bound TCP listener — mirrors `TcpListener` in `std/net.mvl`.
///
/// Copy + Clone: passing the handle to `tcp_accept` does not invalidate it;
/// the listener stays open until `tcp_close_listener` is called.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpListener(pub i64);

/// Opaque handle to a connected TCP stream — mirrors `TcpStream` in `std/net.mvl`.
///
/// Copy + Clone: the stream is shared by handle, not by ownership.
/// Callers must call `tcp_close_stream` when finished.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpStream(pub i64);

// ── Global handle tables ──────────────────────────────────────────────────────

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn listeners() -> &'static Mutex<HashMap<i64, std::net::TcpListener>> {
    static LISTENERS: OnceLock<Mutex<HashMap<i64, std::net::TcpListener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn streams() -> &'static Mutex<HashMap<i64, std::net::TcpStream>> {
    static STREAMS: OnceLock<Mutex<HashMap<i64, std::net::TcpStream>>> = OnceLock::new();
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Mirrors the `NetError` enum declared in `std/net.mvl`.
/// Variant order and names must stay in sync with the MVL definition.
#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    /// The remote end refused the connection.
    ConnectionRefused,
    /// The connection was reset by the peer.
    ConnectionReset,
    /// The operation timed out before completing.
    Timeout,
    /// The local address is already in use.
    AddressInUse,
    /// The remote host could not be reached.
    HostUnreachable,
    /// An unclassified network error with a description.
    Other(String),
}

fn sanitize_net_error(e: &std::io::Error) -> NetError {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => NetError::ConnectionRefused,
        std::io::ErrorKind::ConnectionReset => NetError::ConnectionReset,
        std::io::ErrorKind::TimedOut => NetError::Timeout,
        std::io::ErrorKind::AddrInUse => NetError::AddressInUse,
        _ => NetError::Other(e.to_string()),
    }
}

// ── Stdlib functions ──────────────────────────────────────────────────────────

/// Bind a TCP listener on `host:port`.
pub fn tcp_listen(host: String, port: i64) -> Result<TcpListener, NetError> {
    let addr = format!("{}:{}", host, port);
    std::net::TcpListener::bind(&addr)
        .map_err(|e| sanitize_net_error(&e))
        .map(|l| {
            let h = next_handle();
            listeners().lock().unwrap().insert(h, l);
            TcpListener(h)
        })
}

/// Connect to a remote TCP server at `host:port`.
pub fn tcp_connect(host: String, port: i64) -> Result<TcpStream, NetError> {
    let addr = format!("{}:{}", host, port);
    std::net::TcpStream::connect(&addr)
        .map_err(|e| sanitize_net_error(&e))
        .map(|stream| {
            let h = next_handle();
            streams().lock().unwrap().insert(h, stream);
            TcpStream(h)
        })
}

/// Accept the next incoming connection on `listener`.
///
/// The listener remains open — call `tcp_accept` again for the next connection.
pub fn tcp_accept(listener: TcpListener) -> Result<TcpStream, NetError> {
    let guard = listeners().lock().unwrap();
    guard
        .get(&listener.0)
        .ok_or_else(|| NetError::Other("invalid listener handle".to_string()))
        .and_then(|l| l.accept().map_err(|e| sanitize_net_error(&e)))
        .map(|(stream, _addr)| {
            let h = next_handle();
            drop(guard);
            streams().lock().unwrap().insert(h, stream);
            TcpStream(h)
        })
}

/// Read all available bytes from `stream`.
pub fn tcp_read(stream: TcpStream) -> Result<Tainted<String>, NetError> {
    let mut guard = streams().lock().unwrap();
    match guard.get_mut(&stream.0) {
        None => Err(NetError::Other("invalid stream handle".to_string())),
        Some(s) => {
            let mut buf = Vec::new();
            s.read_to_end(&mut buf)
                .map_err(|e| sanitize_net_error(&e))
                .map(|_| Tainted(String::from_utf8_lossy(&buf).into_owned()))
        }
    }
}

/// Write `data` to `stream`.
pub fn tcp_write(stream: TcpStream, data: String) -> Result<(), NetError> {
    let mut guard = streams().lock().unwrap();
    match guard.get_mut(&stream.0) {
        None => Err(NetError::Other("invalid stream handle".to_string())),
        Some(s) => s
            .write_all(data.as_bytes())
            .map_err(|e| sanitize_net_error(&e)),
    }
}

/// Return the port the listener is bound to.
pub fn tcp_listener_port(listener: TcpListener) -> Result<i64, NetError> {
    let guard = listeners().lock().unwrap();
    guard
        .get(&listener.0)
        .ok_or_else(|| NetError::Other("invalid listener handle".to_string()))
        .and_then(|l| {
            l.local_addr()
                .map(|a| a.port() as i64)
                .map_err(|e| sanitize_net_error(&e))
        })
}

/// Close a listener and release its port.
pub fn tcp_close_listener(listener: TcpListener) {
    listeners().lock().unwrap().remove(&listener.0);
}

/// Close a stream and release its resources.
pub fn tcp_close_stream(stream: TcpStream) {
    streams().lock().unwrap().remove(&stream.0);
}
