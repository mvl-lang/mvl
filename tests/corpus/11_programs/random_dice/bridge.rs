// bridge.rs — Rust implementation of roll_dice declared in main.mvl.
//
// Uses std::time for pseudo-random seeding — no external crate dependencies.
// Returns a pseudo-random integer in [1, 6].

#[no_mangle]
pub extern "Rust" fn roll_dice() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(42);
    // Simple LCG to spread the bits
    let mixed = nanos.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (mixed % 6 + 1) as i64
}
