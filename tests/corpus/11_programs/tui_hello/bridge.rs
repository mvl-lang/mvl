// bridge.rs — Rust implementations of tui_* declared in main.mvl.
//
// Uses ANSI escape codes directly — no crossterm dependency required for
// the Phase 2 smoke test. Validates the ! Terminal effect + bridge pipeline.

#[no_mangle]
pub extern "Rust" fn tui_clear() {
    use std::io::Write;
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
}

#[no_mangle]
pub extern "Rust" fn tui_print_at(row: i64, col: i64, text: String) {
    use std::io::Write;
    // ANSI cursor position is 1-indexed.
    print!("\x1B[{};{}H{}", row + 1, col + 1, text);
    let _ = std::io::stdout().flush();
}

#[no_mangle]
pub extern "Rust" fn tui_sleep_ms(ms: i64) {
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}
