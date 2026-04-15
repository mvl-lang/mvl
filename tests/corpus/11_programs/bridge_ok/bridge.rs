// bridge.rs — Rust implementation of extern "rust" fns declared in main.mvl.
//
// Follows the bridge convention: one `#[no_mangle] pub extern "Rust" fn`
// per function declared in the MVL `extern "rust"` block.

#[no_mangle]
pub extern "Rust" fn bridge_add(n: i64) -> i64 {
    n + 1
}
