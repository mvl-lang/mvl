// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.net` stdlib functions.
//!
//! TcpListener and TcpStream are opaque i64 handle wrappers backed by
//! per-entry `Arc<Mutex<…>>` tables.  The global HashMap is only locked
//! briefly to look up or insert a handle; the actual I/O happens under a
//! per-stream/listener mutex so that concurrent reads, writes, and accepts
//! on *different* handles never block each other.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::ifc::Tainted;

// ── Handle types ──────────────────────────────────────────────────────────────

/// Opaque handle to a bound TCP listener — mirrors `TcpListener` in `std/net.mvl`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpListener(pub i64);

/// Opaque handle to a connected TCP stream — mirrors `TcpStream` in `std/net.mvl`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpStream(pub i64);

// ── Global handle tables ──────────────────────────────────────────────────────

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

type ArcListener = Arc<Mutex<std::net::TcpListener>>;
type ArcStream = Arc<Mutex<std::net::TcpStream>>;

fn listeners() -> &'static Mutex<HashMap<i64, ArcListener>> {
    static L: OnceLock<Mutex<HashMap<i64, ArcListener>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

fn streams() -> &'static Mutex<HashMap<i64, ArcStream>> {
    static S: OnceLock<Mutex<HashMap<i64, ArcStream>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

fn lookup_listener(h: i64) -> Result<ArcListener, NetError> {
    match listeners().lock().unwrap().get(&h) {
        Some(arc) => Ok(Arc::clone(arc)),
        None => Err(NetError::Other("invalid listener handle".to_string())),
    }
}

fn lookup_stream(h: i64) -> Result<ArcStream, NetError> {
    match streams().lock().unwrap().get(&h) {
        Some(arc) => Ok(Arc::clone(arc)),
        None => Err(NetError::Other("invalid stream handle".to_string())),
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Mirrors the `NetError` enum declared in `std/net.mvl`.
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
    let l = std::net::TcpListener::bind(&addr).map_err(|e| sanitize_net_error(&e))?;
    let h = next_handle();
    listeners()
        .lock()
        .unwrap()
        .insert(h, Arc::new(Mutex::new(l)));
    Ok(TcpListener(h))
}

/// Connect to a remote TCP server at `host:port`.
pub fn tcp_connect(host: String, port: i64) -> Result<TcpStream, NetError> {
    let addr = format!("{}:{}", host, port);
    let s = std::net::TcpStream::connect(&addr).map_err(|e| sanitize_net_error(&e))?;
    let h = next_handle();
    streams().lock().unwrap().insert(h, Arc::new(Mutex::new(s)));
    Ok(TcpStream(h))
}

/// Accept the next incoming connection on `listener`.
///
/// The global listeners lock is released before blocking on `accept()` so
/// that other threads can concurrently look up or remove listeners/streams.
pub fn tcp_accept(listener: TcpListener) -> Result<TcpStream, NetError> {
    let arc = lookup_listener(listener.0)?;
    let guard = arc.lock().unwrap();
    let (stream, _addr) = guard.accept().map_err(|e| sanitize_net_error(&e))?;
    drop(guard);
    let h = next_handle();
    streams()
        .lock()
        .unwrap()
        .insert(h, Arc::new(Mutex::new(stream)));
    Ok(TcpStream(h))
}

/// Read all available bytes from `stream` (blocks until peer closes write half).
///
/// The global streams lock is released before blocking so concurrent
/// `tcp_write` calls on other streams are not blocked.
pub fn tcp_read(stream: TcpStream) -> Result<Tainted<String>, NetError> {
    let arc = lookup_stream(stream.0)?;
    let mut buf = Vec::new();
    let mut guard = arc.lock().unwrap();
    let r = guard
        .read_to_end(&mut buf)
        .map_err(|e| sanitize_net_error(&e));
    drop(guard);
    r?;
    Ok(Tainted(String::from_utf8_lossy(&buf).into_owned()))
}

/// Read one HTTP request from `stream` — returns after the blank-line terminator.
///
/// Unlike `tcp_read`, this does NOT wait for the peer to close the connection.
/// Caps at 8 KiB; returns `Tainted[String]`.
pub fn tcp_read_request(stream: TcpStream) -> Result<Tainted<String>, NetError> {
    let arc = lookup_stream(stream.0)?;
    let mut guard = arc.lock().unwrap();
    let mut buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        match guard.read(&mut one) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(one[0]);
                if buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n") {
                    break;
                }
                if buf.len() >= 8192 {
                    break;
                }
            }
            Err(e) => return Err(sanitize_net_error(&e)),
        }
    }
    drop(guard);
    Ok(Tainted(String::from_utf8_lossy(&buf).into_owned()))
}

/// Write `data` to `stream`.
pub fn tcp_write(stream: TcpStream, data: String) -> Result<(), NetError> {
    let arc = lookup_stream(stream.0)?;
    let mut guard = arc.lock().unwrap();
    let r = guard
        .write_all(data.as_bytes())
        .map_err(|e| sanitize_net_error(&e));
    drop(guard);
    r
}

/// Return the local port the listener is bound to.
pub fn tcp_listener_port(listener: TcpListener) -> Result<i64, NetError> {
    let arc = lookup_listener(listener.0)?;
    let guard = arc.lock().unwrap();
    let addr = guard.local_addr().map_err(|e| sanitize_net_error(&e))?;
    drop(guard);
    Ok(addr.port() as i64)
}

/// Close a listener and release its port.
pub fn tcp_close_listener(listener: TcpListener) {
    listeners().lock().unwrap().remove(&listener.0);
}

/// Close a stream and release its resources.
pub fn tcp_close_stream(stream: TcpStream) {
    streams().lock().unwrap().remove(&stream.0);
}

/// Parse the URL path from a raw HTTP/1.x request.
///
/// Extracts the path from `"METHOD /path HTTP/..."`.
/// Falls back to `"/"` on malformed input.
/// IFC trust boundary: converts `Tainted<String>` network input to a `String`
/// path safe for routing decisions only — not for SQL, file paths, or shell commands.
pub fn http_request_path(raw: Tainted<String>) -> String {
    let mut parts = raw.0.splitn(3, ' ');
    parts.nth(1).unwrap_or("/").to_string()
}

/// Format a NetError as a human-readable string (useful for logging).
pub fn net_error_msg(e: NetError) -> String {
    match e {
        NetError::ConnectionRefused => "connection refused".to_string(),
        NetError::ConnectionReset => "connection reset".to_string(),
        NetError::Timeout => "timeout".to_string(),
        NetError::AddressInUse => "address already in use".to_string(),
        NetError::HostUnreachable => "host unreachable".to_string(),
        NetError::Other(msg) => msg,
    }
}
