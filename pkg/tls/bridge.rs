//! bridge.rs — pkg.tls Rust backend
//!
//! Implements the `extern "rust"` functions declared in src/internal/ffi.mvl.
//! Uses rustls (pure Rust, no OpenSSL) with webpki-roots for certificate
//! validation against the Mozilla root store.
//!
//! # Handle table
//!
//!   CONNECTIONS  i64 → TlsConn   (active TLS connections)
//!   ERRORS       i64 → (errno, msg)  (last error per handle; -1 = last connect failure)
//!
//! # Why rustls, not OpenSSL
//!
//! - Pure Rust: no C dependencies, no system libssl required
//! - Memory safe: no buffer overflows in the TLS stack
//! - Works identically for Rust and LLVM backends (same crate, different ABI)
//! - Bundled root certificates via webpki-roots (no system cert store dependency)
//! - Smaller attack surface than OpenSSL (TLS 1.2+ only, no legacy protocols)
//!
//! See #1017.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

// ── Connection type ──────────────────────────────────────────────────────────

type TlsConn = StreamOwned<ClientConnection, TcpStream>;

// ── Global handle tables ─────────────────────────────────────────────────────

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_handle() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::SeqCst)
}

fn connections() -> &'static Mutex<HashMap<i64, TlsConn>> {
    static C: OnceLock<Mutex<HashMap<i64, TlsConn>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn errors() -> &'static Mutex<HashMap<i64, (i64, String)>> {
    static E: OnceLock<Mutex<HashMap<i64, (i64, String)>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Shared TLS config (singleton) ────────────────────────────────────────────

fn tls_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Arc::new(config)
    })
    .clone()
}

// ── Error classification ─────────────────────────────────────────────────────

fn classify_error(e: &dyn std::fmt::Display, is_cert: bool) -> (i64, String) {
    let msg = e.to_string();
    if is_cert || msg.contains("certificate") || msg.contains("CertificateRequired") {
        (1, msg) // CertificateInvalid
    } else if msg.contains("handshake") || msg.contains("AlertReceived") {
        (0, msg) // HandshakeFailed
    } else if msg.contains("closed") || msg.contains("CloseNotify") || msg.contains("EOF") {
        (2, msg) // ConnectionClosed
    } else if msg.contains("io error") || msg.contains("Connection refused") {
        (3, msg) // IoError
    } else {
        (4, msg) // Other
    }
}

fn store_err(handle: i64, errno: i64, msg: String) {
    errors().lock().unwrap().insert(handle, (errno, msg));
}

fn clear_err(handle: i64) {
    errors().lock().unwrap().remove(&handle);
}

// ── Connection ───────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn tls_connect(host: String, port: i64) -> i64 {
    // Parse server name for SNI
    let server_name = match ServerName::try_from(host.clone()) {
        Ok(sn) => sn,
        Err(e) => {
            store_err(-1, 0, format!("invalid hostname '{}': {}", host, e));
            return -1;
        }
    };

    // TCP connect
    let addr = format!("{}:{}", host, port);
    let tcp = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            store_err(-1, 3, format!("TCP connect to {}: {}", addr, e));
            return -1;
        }
    };

    // TLS handshake
    let tls_conn = match ClientConnection::new(tls_config(), server_name) {
        Ok(c) => c,
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(-1, errno, msg);
            return -1;
        }
    };

    let mut stream = StreamOwned::new(tls_conn, tcp);

    // Force the handshake to complete now (rustls is lazy)
    if let Err(e) = stream.flush() {
        let (errno, msg) = classify_error(&e, false);
        store_err(-1, errno, msg);
        return -1;
    }

    let h = next_handle();
    connections().lock().unwrap().insert(h, stream);
    clear_err(h);
    h
}

#[no_mangle]
pub extern "Rust" fn tls_close(handle: i64) {
    if let Some(mut conn) = connections().lock().unwrap().remove(&handle) {
        // Send close_notify (best-effort)
        let _ = conn.conn.send_close_notify();
        let _ = conn.flush();
    }
    errors().lock().unwrap().remove(&handle);
}

#[no_mangle]
pub extern "Rust" fn tls_errmsg(handle: i64) -> String {
    errors()
        .lock()
        .unwrap()
        .get(&handle)
        .map(|(_, m)| m.clone())
        .unwrap_or_default()
}

#[no_mangle]
pub extern "Rust" fn tls_errno(handle: i64) -> i64 {
    errors()
        .lock()
        .unwrap()
        .get(&handle)
        .map(|(c, _)| *c)
        .unwrap_or(-1)
}

// ── I/O ──────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "Rust" fn tls_read(handle: i64) -> String {
    let mut conns = connections().lock().unwrap();
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 4, "invalid TLS handle".to_string());
        return String::new();
    };
    let mut buf = Vec::new();
    match stream.read_to_end(&mut buf) {
        Ok(_) => {
            clear_err(handle);
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(handle, errno, msg);
            String::new()
        }
    }
}

#[no_mangle]
pub extern "Rust" fn tls_read_response(handle: i64) -> String {
    let mut conns = connections().lock().unwrap();
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 4, "invalid TLS handle".to_string());
        return String::new();
    };
    // Read byte-by-byte until we get the full response.
    // For HTTP/1.1 with Connection: close, the server closes after the response.
    let mut buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        match stream.read(&mut one) {
            Ok(0) => break,                  // EOF — server closed connection
            Ok(_) => {
                buf.push(one[0]);
                // Safety cap at 1 MiB
                if buf.len() >= 1_048_576 {
                    break;
                }
            }
            Err(e) => {
                // ConnectionAborted / EOF during read is normal for close-delimited responses
                if !buf.is_empty() {
                    break;
                }
                let (errno, msg) = classify_error(&e, false);
                store_err(handle, errno, msg);
                return String::new();
            }
        }
    }
    clear_err(handle);
    String::from_utf8_lossy(&buf).into_owned()
}

#[no_mangle]
pub extern "Rust" fn tls_write(handle: i64, data: String) -> i64 {
    let mut conns = connections().lock().unwrap();
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 4, "invalid TLS handle".to_string());
        return -1;
    };
    match stream.write_all(data.as_bytes()) {
        Ok(()) => {
            let _ = stream.flush();
            clear_err(handle);
            data.len() as i64
        }
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(handle, errno, msg);
            -1
        }
    }
}
