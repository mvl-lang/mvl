// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementations of `std.net` stdlib functions.
//!
//! TcpListener and TcpStream are opaque i64 handle wrappers backed by
//! `Arc<std::net::TcpStream/TcpListener>` handle tables.  The global HashMap
//! is locked only briefly to look up or insert a handle.  Actual I/O uses
//! `Read/Write for &TcpStream` so no per-stream lock is held during blocking
//! calls — this prevents deadlocks when `tcp_close_stream` needs to shut down
//! a stream while a concurrent `tcp_read` is waiting for EOF (#826).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::capability::ApiEndpoint;
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

// No inner Mutex: TcpStream/TcpListener implement Read/Write/shutdown for &Self,
// so I/O can happen via a shared Arc reference without exclusive access.
// This avoids holding a lock across blocking I/O calls, which caused a deadlock
// where tcp_read held Arc<Mutex<TcpStream>> during read_to_end while tcp_close_stream
// tried to remove the entry — the Arc refcount never reached zero, so EOF was never sent.
type ArcListener = Arc<std::net::TcpListener>;
type ArcStream = Arc<std::net::TcpStream>;

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
        _ => NetError::Other(e.kind().to_string()),
    }
}

// ── Stdlib functions ──────────────────────────────────────────────────────────

/// Bind a TCP listener on `host:port`.
pub fn tcp_listen(host: String, port: i64) -> Result<TcpListener, NetError> {
    let addr = format!("{}:{}", host, port);
    let l = std::net::TcpListener::bind(&addr).map_err(|e| sanitize_net_error(&e))?;
    let h = next_handle();
    listeners().lock().unwrap().insert(h, Arc::new(l));
    Ok(TcpListener(h))
}

/// Connect to a remote TCP server at `host:port`.
pub fn tcp_connect(host: String, port: i64) -> Result<TcpStream, NetError> {
    let addr = format!("{}:{}", host, port);
    let s = std::net::TcpStream::connect(&addr).map_err(|e| sanitize_net_error(&e))?;
    let h = next_handle();
    streams().lock().unwrap().insert(h, Arc::new(s));
    Ok(TcpStream(h))
}

/// Accept the next incoming connection on `listener`.
///
/// No lock is held during the blocking `accept()` call — `TcpListener::accept`
/// takes `&self` so multiple threads can call it concurrently on different handles.
pub fn tcp_accept(listener: TcpListener) -> Result<TcpStream, NetError> {
    let arc = lookup_listener(listener.0)?;
    let (stream, _addr) = arc.accept().map_err(|e| sanitize_net_error(&e))?;
    let h = next_handle();
    streams().lock().unwrap().insert(h, Arc::new(stream));
    Ok(TcpStream(h))
}

/// Raw private builtin: read all bytes from `stream`, return bare `String` (#894 Pattern 002).
///
/// Module-private in MVL (`builtin fn _tcp_read`) — callers use `tcp_read`.
/// Uses Latin-1 encoding: each byte maps to the Unicode codepoint with the
/// same value (0–255).  This preserves binary data for protocols like ZMTP
/// while being identical to UTF-8 for ASCII text.
pub(crate) fn _tcp_read(stream: TcpStream) -> Result<String, NetError> {
    let arc = lookup_stream(stream.0)?;
    let mut buf = Vec::new();
    (&*arc)
        .read_to_end(&mut buf)
        .map_err(|e| sanitize_net_error(&e))?;
    Ok(buf.iter().map(|&b| b as char).collect())
}

/// Read all available bytes from `stream` (blocks until peer closes write half).
///
/// Returns `Tainted[String]` — network data is always untrusted.
/// Uses `Read for &TcpStream` — no exclusive lock held during the blocking read,
/// so `tcp_close_stream` on a concurrent thread can call `shutdown()` to send EOF.
pub fn tcp_read(stream: TcpStream) -> Result<Tainted<String>, NetError> {
    _tcp_read(stream).map(Tainted)
}

/// Raw private builtin: read one HTTP request, return bare `String` (#894 Pattern 002).
///
/// Module-private in MVL — callers use `tcp_read_request`.
/// Uses Latin-1 encoding for consistency with `_tcp_read`.
///
/// Reads headers until `\r\n\r\n`, then reads the body if `Content-Length` is present.
pub fn _tcp_read_request(stream: TcpStream) -> Result<String, NetError> {
    let arc = lookup_stream(stream.0)?;
    let mut header_buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        match (&*arc).read(&mut one) {
            Ok(0) => break,
            Ok(_) => {
                header_buf.push(one[0]);
                if header_buf.ends_with(b"\r\n\r\n") || header_buf.ends_with(b"\n\n") {
                    break;
                }
                if header_buf.len() >= 8192 {
                    break;
                }
            }
            Err(e) => return Err(sanitize_net_error(&e)),
        }
    }
    // Parse Content-Length header to read the body.
    let header_str: String = header_buf.iter().map(|&b| b as char).collect();
    let content_length: Option<usize> = header_str
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            let rest = lower.strip_prefix("content-length:")?;
            rest.trim().parse::<usize>().ok()
        });
    if let Some(body_len) = content_length {
        if body_len > 0 {
            let mut body_buf = vec![0u8; body_len];
            let mut filled = 0;
            while filled < body_len {
                match (&*arc).read(&mut body_buf[filled..]) {
                    Ok(0) => break,
                    Ok(k) => filled += k,
                    Err(e) => return Err(sanitize_net_error(&e)),
                }
            }
            let body_str: String = body_buf[..filled].iter().map(|&b| b as char).collect();
            return Ok(header_str + &body_str);
        }
    }
    Ok(header_str)
}

/// Read one HTTP request from `stream` — returns after the blank-line terminator.
///
/// Returns `Tainted[String]` — network data is always untrusted.
/// Unlike `tcp_read`, this does NOT wait for the peer to close the connection.
/// Caps at 8 KiB.
pub fn tcp_read_request(stream: TcpStream) -> Result<Tainted<String>, NetError> {
    _tcp_read_request(stream).map(Tainted)
}

/// Raw private builtin: read exactly `n` bytes, return bare `String` (Pattern 002).
/// Module-private in MVL — callers use `tcp_read_exact`.
/// Uses Latin-1 encoding for consistency with `_tcp_read`.
pub fn _tcp_read_exact(stream: TcpStream, n: i64) -> Result<String, NetError> {
    if n < 0 {
        return Err(NetError::Other("negative read size".to_string()));
    }
    let arc = lookup_stream(stream.0)?;
    let n = n as usize;
    let mut buf = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        match (&*arc).read(&mut buf[filled..]) {
            Ok(0) => {
                return Err(NetError::Other(format!(
                    "unexpected EOF after {} of {} bytes",
                    filled, n
                )));
            }
            Ok(k) => filled += k,
            Err(e) => return Err(sanitize_net_error(&e)),
        }
    }
    Ok(buf.iter().map(|&b| b as char).collect())
}

/// Read exactly `n` bytes from `stream`.
///
/// Returns `Tainted[String]` — network data is always untrusted.
/// Unlike `tcp_read`, this does NOT wait for EOF — it returns as soon as `n`
/// bytes have been read.  Returns `Err` if the peer closes before `n` bytes
/// are available.
pub fn tcp_read_exact(stream: TcpStream, n: i64) -> Result<Tainted<String>, NetError> {
    _tcp_read_exact(stream, n).map(Tainted)
}

/// Shut down the write half of `stream`, signaling EOF to the peer.
///
/// The read half remains open — the peer can still send data.
/// This is the standard way to signal "I'm done writing" without closing
/// the entire connection.
pub fn tcp_shutdown_write(stream: TcpStream) -> Result<(), NetError> {
    let arc = lookup_stream(stream.0)?;
    arc.shutdown(std::net::Shutdown::Write)
        .map_err(|e| sanitize_net_error(&e))
}

/// Write `data` to `stream`.
///
/// Uses Latin-1 decoding: each character's Unicode codepoint (0–255) is sent
/// as one byte on the wire.  This is the inverse of `_tcp_read` / `_tcp_read_exact`,
/// which use Latin-1 encoding.  For ASCII strings, identical to the previous
/// `as_bytes()` behavior.
///
/// Uses `Write for &TcpStream` — no exclusive lock held.
pub fn tcp_write(stream: TcpStream, data: String) -> Result<(), NetError> {
    let arc = lookup_stream(stream.0)?;
    let bytes: Vec<u8> = data.chars().map(|c| c as u32 as u8).collect();
    (&*arc)
        .write_all(&bytes)
        .map_err(|e| sanitize_net_error(&e))
}

/// Return the local port the listener is bound to.
pub fn tcp_listener_port(listener: TcpListener) -> Result<i64, NetError> {
    let arc = lookup_listener(listener.0)?;
    let addr = arc.local_addr().map_err(|e| sanitize_net_error(&e))?;
    Ok(addr.port() as i64)
}

/// Close a listener and release its port.
pub fn tcp_close_listener(listener: TcpListener) {
    listeners().lock().unwrap().remove(&listener.0);
}

/// Close a stream and release its resources.
///
/// Calls `shutdown(Both)` before removing the handle so that any concurrent
/// `tcp_read` (which holds an Arc clone and calls `read_to_end`) sees EOF
/// immediately.  Without the explicit shutdown the Arc refcount in `tcp_read`
/// would keep the socket alive and `read_to_end` would block forever (#826).
pub fn tcp_close_stream(stream: TcpStream) {
    if let Some(arc) = streams().lock().unwrap().remove(&stream.0) {
        let _ = arc.shutdown(std::net::Shutdown::Both);
    }
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

// ── ApiEndpoint capability helpers ───────────────────────────────────────────
// Pure-MVL functions from std/net.mvl, mirrored here so that `use
// mvl_runtime::stdlib::net::*` provides them without requiring hybrid
// module loading.

/// Wrap a compile-time default endpoint as `ApiEndpoint[String]`.
pub fn default_endpoint(fallback: String) -> ApiEndpoint<String> {
    ApiEndpoint(fallback)
}

/// Load an API endpoint from an environment variable.
///
/// Returns `Some(ApiEndpoint[String])` if the env var is set, `None` otherwise.
pub fn load_endpoint(key: String) -> Option<ApiEndpoint<String>> {
    std::env::var(&key).ok().map(ApiEndpoint)
}

/// Connect to an `ApiEndpoint` host on the given port.
///
/// Unwraps the capability label and delegates to `tcp_connect`.
pub fn endpoint_connect(endpoint: ApiEndpoint<String>, port: i64) -> Result<TcpStream, NetError> {
    tcp_connect(endpoint.0, port)
}

/// Listen on an `ApiEndpoint` host on the given port.
///
/// Unwraps the capability label and delegates to `tcp_listen`.
pub fn endpoint_listen(endpoint: ApiEndpoint<String>, port: i64) -> Result<TcpListener, NetError> {
    tcp_listen(endpoint.0, port)
}
