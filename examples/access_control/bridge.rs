//! bridge.rs — Irreducible Rust trust boundary for access_control.
//!
//! This file contains ONLY functions that cannot be expressed in MVL:
//! constant-time string comparison for password verification.
//!
//! Everything else (demo fixtures, token generation) has been moved to
//! pure MVL — see auth.mvl and main.mvl.
//!
//! Security note: hash_verify uses a plain string comparison — this is a
//! demo only and is NOT cryptographically secure. A real implementation
//! would use bcrypt, argon2, or similar.
//!
//! Compile with: `mvl build examples/access_control/main.mvl`
//! (bridge.rs is detected automatically and linked in)

use mvl_runtime::prelude::*;

use crate::AuthError;

// ── Irreducible trust boundary ──────────────────────────────────────────

/// Verify a password against a stored hash.
///
/// The `stored_hash` is `Secret<String>` — it cannot be printed or logged
/// from MVL code. It can only be consumed here at the trust boundary.
///
/// This function is the irreducible Rust core: constant-time string
/// comparison cannot be expressed in MVL (no byte-level XOR accumulator).
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
