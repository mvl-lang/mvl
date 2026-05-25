//! bridge.rs — Rust implementations of the `extern "rust"` trust boundary
//! declared in main.mvl, handler.mvl, and storage.mvl.
//!
//! This bridge provides:
//!   1. **Config loader** — parses sample_config.json, validates refinements
//!   2. **Auth store**    — stores api_key (from env via get_secret) as Secret, constant-time verify
//!   3. **Config store**  — in-memory HashMap for non-secret config values
//!   4. **Server sim**    — pre-scripted demo request sequence (3 requests)
//!
//! Demo sequence (see `server_recv`):
//!   0: GET  /config/timeout     (valid key)  → 200 with current timeout
//!   1: PUT  /config/timeout     (valid key)  → 200 with updated value "45"
//!      body "45" enters as Tainted<String>; sanitize() required before storage
//!   2: GET  /config/timeout     (wrong key)  → 401 Unauthorized
//!      demonstrates: the stored Secret key cannot be guessed or leaked from MVL
//!
//! Security notes:
//!   - api_key is stored behind a Mutex and NEVER returned to MVL code.
//!   - Constant-time comparison in `verify_request_auth` prevents timing attacks.
//!   - In production, replace server_recv/server_send with a real HTTP crate
//!     (e.g. hyper 1.0 + tokio) keeping the same MVL-visible signatures.
//!
//! Compile with: `mvl build examples/config_server/main.mvl`
//! (bridge.rs is detected automatically and linked in)

use mvl_runtime::prelude::*;

use crate::{Config, ConfigError, HandlerError, MaxConns, Method, Port, Request, Response, Timeout};

use std::collections::HashMap;
use std::sync::Mutex;

// ── Global state ──────────────────────────────────────────────────────────

/// Stores the api_key as a Secret. Never returned to MVL — only verified.
static AUTH_KEY: Mutex<Option<Secret<String>>> = Mutex::new(None);

/// In-memory config store (non-secret values only).
static CONFIG_STORE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn default_store() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("port".to_string(), "8080".to_string());
    m.insert("timeout".to_string(), "30".to_string());
    m.insert("max_connections".to_string(), "100".to_string());
    m.insert("debug_mode".to_string(), "false".to_string());
    m
}

// ── Config loader ─────────────────────────────────────────────────────────

/// Parse sample_config.json and return a Config with validated refinements.
///
/// Returns Err if the file is missing, unparseable, or any value falls outside
/// its refinement constraint (e.g. port 0 or port > 65535).
#[no_mangle]
pub extern "Rust" fn load_config(path: String) -> Result<Config, ConfigError> {
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Err(ConfigError::FileNotFound),
    };

    let port = extract_int_field(&content, "port")
        .ok_or(ConfigError::ParseError)?;
    let timeout = extract_int_field(&content, "timeout")
        .ok_or(ConfigError::ParseError)?;
    let max_connections = extract_int_field(&content, "max_connections")
        .ok_or(ConfigError::ParseError)?;
    let debug_mode = extract_bool_field(&content, "debug_mode").unwrap_or(false);

    // Validate refinement constraints.
    if port <= 0 || port > 65535 {
        return Err(ConfigError::InvalidPort);
    }
    if !(0..=300).contains(&timeout) {
        return Err(ConfigError::InvalidTimeout);
    }
    if max_connections <= 0 || max_connections > 10000 {
        return Err(ConfigError::InvalidMaxConns);
    }

    // Seed the config store with the loaded values.
    {
        let mut store = CONFIG_STORE.lock().unwrap();
        let mut m = default_store();
        m.insert("port".to_string(), port.to_string());
        m.insert("timeout".to_string(), timeout.to_string());
        m.insert("max_connections".to_string(), max_connections.to_string());
        m.insert("debug_mode".to_string(), debug_mode.to_string());
        *store = Some(m);
    }

    Ok(Config {
        port: Port(port),
        timeout: Timeout(timeout),
        max_connections: MaxConns(max_connections),
        debug_mode,
    })
}

// ── Auth store ────────────────────────────────────────────────────────────

/// Consume the api_key and store it securely. It can never be retrieved — only verified.
///
/// MVL IFC invariant: after this call the Secret<String> is gone from MVL code.
/// The type system prevents it from appearing in any Response body or log argument.
#[no_mangle]
pub extern "Rust" fn init_auth_store(api_key: Secret<String>) {
    let mut guard = AUTH_KEY.lock().unwrap();
    *guard = Some(api_key);
}

/// Verify an Authorization header against the stored api_key (constant-time).
///
/// Returns Ok(()) on match, Err(Unauthorized) on mismatch or missing key.
/// The stored Secret<String> is never returned — it stays inside Rust state.
#[no_mangle]
pub extern "Rust" fn verify_request_auth(provided: Tainted<String>) -> Result<(), HandlerError> {
    let guard = AUTH_KEY.lock().unwrap();
    match guard.as_ref() {
        None => Err(HandlerError::Unauthorized),
        Some(stored) => {
            let ok = constant_time_eq(provided.trim(), stored.trim());
            if ok {
                Ok(())
            } else {
                Err(HandlerError::Unauthorized)
            }
        }
    }
}

/// Constant-time string comparison — prevents timing side-channel attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── Config store ──────────────────────────────────────────────────────────

/// Read a config value by URL path (e.g. "/config/timeout" → "timeout").
#[no_mangle]
pub extern "Rust" fn get_config_value(path: String) -> Result<Clean<String>, HandlerError> {
    let key = path_to_key(&path)?;
    let guard = CONFIG_STORE.lock().unwrap();
    match guard.as_ref().and_then(|m| m.get(key)) {
        Some(v) => Ok(Clean(format!("{{\"{}\":\"{}\"}}", key, v))),
        None => Err(HandlerError::NotFound),
    }
}

/// Update a config value. `value` is Clean<String> — the caller has already sanitized it.
#[no_mangle]
pub extern "Rust" fn put_config_value(
    path: String,
    value: Clean<String>,
) -> Result<Clean<String>, HandlerError> {
    let key = path_to_key(&path)?;
    let mut guard = CONFIG_STORE.lock().unwrap();
    match guard.as_mut() {
        None => Err(HandlerError::ServerError),
        Some(m) => {
            let raw = value.as_inner().clone();
            m.insert(key.to_string(), raw.clone());
            Ok(Clean(format!("{{\"{}\":\"{}\"}}", key, raw)))
        }
    }
}

/// Reset a config key to its default value.
#[no_mangle]
pub extern "Rust" fn reset_config_value(path: String) -> Result<Clean<String>, HandlerError> {
    let key = path_to_key(&path)?;
    let defaults = default_store();
    let default_val = defaults.get(key).ok_or(HandlerError::NotFound)?.clone();
    let mut guard = CONFIG_STORE.lock().unwrap();
    match guard.as_mut() {
        None => Err(HandlerError::ServerError),
        Some(m) => {
            m.insert(key.to_string(), default_val.clone());
            Ok(Clean(format!("{{\"{}\":\"{}\"}}", key, default_val)))
        }
    }
}

/// Aliases for storage.mvl (same implementation as get/put/reset_config_value).
#[no_mangle]
pub extern "Rust" fn store_get(key: String) -> Result<Clean<String>, HandlerError> {
    get_config_value(format!("/config/{key}"))
}

#[no_mangle]
pub extern "Rust" fn store_put(
    key: String,
    value: Clean<String>,
) -> Result<Clean<String>, HandlerError> {
    put_config_value(format!("/config/{key}"), value)
}

#[no_mangle]
pub extern "Rust" fn store_reset(key: String) -> Result<Clean<String>, HandlerError> {
    reset_config_value(format!("/config/{key}"))
}

/// Map a URL path like "/config/timeout" to a store key like "timeout".
fn path_to_key(path: &str) -> Result<&str, HandlerError> {
    path.strip_prefix("/config/")
        .filter(|k| !k.is_empty() && !k.contains('/'))
        .ok_or(HandlerError::NotFound)
}

// ── Server simulator ──────────────────────────────────────────────────────

/// Return a pre-scripted demo request by index.
///
/// Simulates the network accept + HTTP parse steps. In production, replace
/// with a real TCP listener (e.g. `std::net::TcpListener` or hyper 1.0).
///
/// Returns `None` when the demo sequence is exhausted.
#[no_mangle]
pub extern "Rust" fn server_recv(index: i64) -> Option<Request> {
    let valid_key = Tainted("config-server-demo-key".to_string());
    let wrong_key = Tainted("wrong-key".to_string());
    let empty = Tainted(String::new());

    match index {
        // Request 0: GET /config/timeout — valid key → should return 200
        0 => Some(Request {
            method: Method::Get,
            path: "/config/timeout".to_string(),
            body: empty,
            api_key_header: valid_key,
        }),
        // Request 1: PUT /config/timeout with body "45" — valid key → should return 200
        // IFC: body "45" enters as Tainted<String>; MVL's sanitize() is called before storage.
        1 => Some(Request {
            method: Method::Put,
            path: "/config/timeout".to_string(),
            body: Tainted("45".to_string()),
            api_key_header: Tainted("config-server-demo-key".to_string()),
        }),
        // Request 2: GET /config/timeout — WRONG key → should return 401 Unauthorized.
        // Demonstrates: the stored Secret key cannot be guessed or leaked from MVL code.
        2 => Some(Request {
            method: Method::Get,
            path: "/config/timeout".to_string(),
            body: empty,
            api_key_header: wrong_key,
        }),
        _ => None,
    }
}

/// Emit a response.
///
/// Demo mode: prints status + body to stdout.
/// Production: send an HTTP response over the TCP connection.
#[no_mangle]
pub extern "Rust" fn server_send(status: i64, body: Clean<String>) {
    println!("{} {}", status, body.as_inner());
}

// ── JSON field extraction helpers ─────────────────────────────────────────

fn extract_int_field(json: &str, field: &str) -> Option<i64> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let rest = &json[pos + key.len()..];
    let after_colon = rest[rest.find(':')? + 1..].trim_start();
    let end = after_colon
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(after_colon.len());
    after_colon[..end].parse::<i64>().ok()
}

fn extract_str_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let rest = &json[pos + key.len()..];
    let after_colon = rest[rest.find(':')? + 1..].trim_start();
    let inner = after_colon.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

fn extract_bool_field(json: &str, field: &str) -> Option<bool> {
    let key = format!("\"{}\"", field);
    let pos = json.find(&key)?;
    let rest = &json[pos + key.len()..];
    let after_colon = rest[rest.find(':')? + 1..].trim_start();
    if after_colon.starts_with("true") {
        Some(true)
    } else if after_colon.starts_with("false") {
        Some(false)
    } else {
        None
    }
}
