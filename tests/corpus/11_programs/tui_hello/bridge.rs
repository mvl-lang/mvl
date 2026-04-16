// bridge.rs — Rust implementations of tui_* declared in main.mvl.
//
// Uses ANSI escape codes directly — no crossterm dependency required for
// the Phase 2 smoke test. Validates the ! Terminal effect + bridge pipeline.
//
// Note: `#[no_mangle]` is required even for `extern "Rust"` bridges because
// the bridge.rs is linked as a separate compilation unit — the transpiler-
// generated code references functions by their C symbol names, so Rust's
// default name mangling must be suppressed.
//
// Safety: `text` in tui_print_at is forwarded verbatim to the terminal.
// Do NOT pass untrusted input — arbitrary escape sequences can be injected.

use std::io::Write;

#[no_mangle]
pub extern "Rust" fn tui_clear() {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
}

#[no_mangle]
pub extern "Rust" fn tui_print_at(row: i64, col: i64, text: String) {
    // ANSI cursor position is 1-indexed.
    print!("\x1B[{};{}H{}", row + 1, col + 1, text);
    let _ = std::io::stdout().flush();
}

#[no_mangle]
pub extern "Rust" fn tui_sleep_ms(ms: i64) {
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
    }
}
