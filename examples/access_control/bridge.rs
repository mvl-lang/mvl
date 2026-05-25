//! bridge.rs — Rust implementations of the `extern "rust"` trust boundary
//! declared in main.mvl.
//!
//! These functions cross the trust boundary between verified MVL code and
//! the outside world (crypto, session tokens, demo fixtures).
//!
//! Security note: hash_verify uses a plain string comparison — this is a
//! demo only and is NOT cryptographically secure. A real implementation
//! would use bcrypt, argon2, or similar.
//!
//! Compile with: `mvl build examples/access_control/main.mvl`
//! (bridge.rs is detected automatically and linked in)

use mvl_runtime::prelude::*;

// Types generated from main.mvl — accessible as `crate::*` in a binary crate.
use crate::{AuthError, Role};

// ── Trust boundary implementations ────────────────────────────────────────

/// Verify a password against a stored hash.
///
/// The `stored_hash` is `Secret<String>` — it cannot be printed or logged
/// from MVL code. It can only be consumed here at the trust boundary.
///
/// DEMO ONLY: compares strings directly. Use argon2/bcrypt in production.
#[no_mangle]
pub extern "Rust" fn hash_verify(
    input: Tainted<String>,
    stored_hash: Secret<String>,
) -> Result<(), AuthError> {
    if input.trim() == stored_hash.trim() {
        Ok(())
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

/// Generate a session token for a validated (Clean) username.
///
/// In production this would be a cryptographically random JWT or opaque token.
#[no_mangle]
pub extern "Rust" fn generate_token(username: Clean<String>) -> Clean<String> {
    Clean(format!("tok-{}", username.as_str()))
}

/// Return the demo password hash for a known username.
///
/// Returns `Secret<String>` — the hash is confidential and cannot be
/// passed to MVL's `println` without a compile/runtime error.
///
/// In production, this would query a database using a prepared statement.
#[no_mangle]
pub extern "Rust" fn get_demo_hash(username: Clean<String>) -> Option<Secret<String>> {
    match username.as_str() {
        "alice" => Some(Secret("hunter2".to_string())),
        "bob"   => Some(Secret("passw0rd".to_string())),
        _       => None,
    }
}

/// Return the demo role for a known username.
///
/// In production, this would be a database or directory lookup.
#[no_mangle]
pub extern "Rust" fn get_demo_role(username: Clean<String>) -> Option<Role> {
    match username.as_str() {
        "alice" => Some(Role::Admin),
        "bob"   => Some(Role::User),
        "mod1"  => Some(Role::Moderator),
        _       => Some(Role::Guest),
    }
}
