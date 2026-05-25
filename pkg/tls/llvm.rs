//! llvm.rs — pkg.tls LLVM backend (C-ABI layer)
//!
//! Mirrors bridge.rs with `extern "C"` symbols so the LLVM backend can link
//! against this file instead of bridge.rs. The LLVM codegen resolves
//! `extern "c"` declarations in ffi.mvl to these symbols via the linker.
//!
//! # Build wiring
//!
//! This file is compiled and linked as part of `mvl build --backend llvm`.
//! The build system discovers it via the #811-A convention: any package
//! containing `llvm.rs` alongside `bridge.rs` gets both compiled, with the
//! correct one selected per backend.
//!
//! # String ABI
//!
//! Input strings  -> `*const MvlString`  (caller owns, not freed here)
//! Output strings -> `*mut MvlString`    (allocated via `mvl_string_new`,
//!                                       caller owns and must drop)
//!
//! MvlString layout matches `runtime/llvm/src/memory.rs`:
//!   { ptr: *mut u8, len: u64, cap: u64, refcount: u64 }
//!
//! See #1017, #811.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

// ── MvlString (mirrored from runtime/llvm/src/memory.rs) ─────────────────────

#[repr(C)]
pub struct MvlString {
    pub ptr: *mut u8,
    pub len: u64,
    pub cap: u64,
    pub refcount: u64,
}

extern "C" {
    fn mvl_string_new(ptr: *const u8, len: usize) -> *mut MvlString;
}

unsafe fn read_str(s: *const MvlString) -> String {
    if s.is_null() {
        return String::new();
    }
    let len = unsafe { (*s).len as usize };
    if len == 0 || unsafe { (*s).ptr.is_null() } {
        return String::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts((*s).ptr as *const u8, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn new_mvl_str(s: &str) -> *mut MvlString {
    let b = s.as_bytes();
    unsafe { mvl_string_new(b.as_ptr(), b.len()) }
}

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
        (2, msg) // CertificateInvalid
    } else if msg.contains("handshake") || msg.contains("AlertReceived") {
        (1, msg) // HandshakeFailed
    } else if msg.contains("closed") || msg.contains("CloseNotify") || msg.contains("EOF") {
        (3, msg) // ConnectionClosed
    } else if msg.contains("io error") || msg.contains("Connection refused") {
        (4, msg) // IoError
    } else {
        (5, msg) // Other
    }
}

fn store_err(handle: i64, errno: i64, msg: String) {
    errors().lock().unwrap_or_else(|e| e.into_inner()).insert(handle, (errno, msg));
}

fn clear_err(handle: i64) {
    errors().lock().unwrap_or_else(|e| e.into_inner()).remove(&handle);
}

// ── C-ABI exports ────────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn tls_connect(host: *const MvlString, port: i64) -> i64 {
    let host_str = unsafe { read_str(host) };

    let server_name = match ServerName::try_from(host_str.clone()) {
        Ok(sn) => sn,
        Err(e) => {
            store_err(-1, 1, format!("invalid hostname '{}': {}", host_str, e));
            return -1;
        }
    };

    let addr = format!("{}:{}", host_str, port);
    let tcp = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            store_err(-1, 4, format!("TCP connect to {}: {}", addr, e));
            return -1;
        }
    };

    // Set socket timeouts to prevent indefinite blocking
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));

    let tls_conn = match ClientConnection::new(tls_config(), server_name) {
        Ok(c) => c,
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(-1, errno, msg);
            return -1;
        }
    };

    let mut stream = StreamOwned::new(tls_conn, tcp);
    if let Err(e) = stream.flush() {
        let (errno, msg) = classify_error(&e, false);
        store_err(-1, errno, msg);
        return -1;
    }

    let h = next_handle();
    connections().lock().unwrap_or_else(|e| e.into_inner()).insert(h, stream);
    clear_err(h);
    h
}

#[no_mangle]
pub unsafe extern "C" fn tls_close(handle: i64) {
    if let Some(mut conn) = connections().lock().unwrap_or_else(|e| e.into_inner()).remove(&handle) {
        let _ = conn.conn.send_close_notify();
        let _ = conn.flush();
    }
    errors().lock().unwrap_or_else(|e| e.into_inner()).remove(&handle);
}

#[no_mangle]
pub unsafe extern "C" fn tls_errmsg(handle: i64) -> *mut MvlString {
    let msg = errors()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .map(|(_, m)| m.clone())
        .unwrap_or_default();
    new_mvl_str(&msg)
}

#[no_mangle]
pub unsafe extern "C" fn tls_errno(handle: i64) -> i64 {
    errors()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .map(|(c, _)| *c)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn tls_read(handle: i64) -> *mut MvlString {
    let mut conns = connections().lock().unwrap_or_else(|e| e.into_inner());
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 5, "invalid TLS handle".to_string());
        return new_mvl_str("");
    };
    let mut buf = Vec::new();
    match stream.read_to_end(&mut buf) {
        Ok(_) => {
            clear_err(handle);
            let s = String::from_utf8_lossy(&buf);
            new_mvl_str(&s)
        }
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(handle, errno, msg);
            new_mvl_str("")
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn tls_read_response(handle: i64) -> *mut MvlString {
    let mut conns = connections().lock().unwrap_or_else(|e| e.into_inner());
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 5, "invalid TLS handle".to_string());
        return new_mvl_str("");
    };
    let mut buf = Vec::new();
    let mut one = [0u8; 1];
    loop {
        match stream.read(&mut one) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(one[0]);
                // Safety cap at 1 MiB — signal error rather than silent truncation
                if buf.len() >= 1_048_576 {
                    store_err(handle, 5, "response truncated at 1 MiB limit".to_string());
                    return new_mvl_str("");
                }
            }
            Err(e) => {
                if !buf.is_empty() {
                    break;
                }
                let (errno, msg) = classify_error(&e, false);
                store_err(handle, errno, msg);
                return new_mvl_str("");
            }
        }
    }
    clear_err(handle);
    let s = String::from_utf8_lossy(&buf);
    new_mvl_str(&s)
}

#[no_mangle]
pub unsafe extern "C" fn tls_write(handle: i64, data: *const MvlString) -> i64 {
    let data_str = unsafe { read_str(data) };
    let mut conns = connections().lock().unwrap_or_else(|e| e.into_inner());
    let Some(stream) = conns.get_mut(&handle) else {
        store_err(handle, 5, "invalid TLS handle".to_string());
        return -1;
    };
    match stream.write_all(data_str.as_bytes()) {
        Ok(()) => {
            let _ = stream.flush();
            clear_err(handle);
            data_str.len() as i64
        }
        Err(e) => {
            let (errno, msg) = classify_error(&e, false);
            store_err(handle, errno, msg);
            -1
        }
    }
}
