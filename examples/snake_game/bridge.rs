//! bridge.rs — extern "rust" implementations for examples/snake_game.
//!
//! Provides:
//!   - Terminal I/O: raw mode via POSIX `stty`, output via ANSI escape sequences.
//!   - Input:        blocking/timeout byte reads from stdin decoded to key strings.
//!   - Clock:        monotonic millisecond timestamp via std::time::SystemTime.
//!   - Random:       LCG seeded from clock for food position generation.
//!
//! Design: std-only, no external crate dependencies.
//! All functions declared in the MVL `extern "rust"` blocks of main.mvl and
//! render.mvl are implemented here.
//!
//! Phase note: bridge.rs mirrors the approach in pkg/tui/bridge.rs (stty + ANSI).
//! A future Phase 4 bridge could replace stty with crossterm for Windows support.

use std::fs::File;
use std::io::{Read, Write, stdout};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static RAW_MODE: AtomicBool = AtomicBool::new(false);

// Run stty on /dev/tty using the macOS -f flag.
// Using -f lets stty open the device itself (O_RDWR), which is more reliable
// than passing an inherited or opened fd as stdin across a process chain.
fn run_stty(args: &[&str]) -> bool {
    let mut cmd_args = vec!["-f", "/dev/tty"];
    cmd_args.extend_from_slice(args);
    let out = Command::new("/usr/bin/stty")
        .args(&cmd_args)
        .output();
    match out {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!(
                "snake_game bridge: stty {:?} failed: {}",
                args,
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            eprintln!("snake_game bridge: cannot spawn stty: {e}");
            false
        }
    }
}

// Read up to `buf.len()` bytes from /dev/tty (the controlling terminal).
fn read_tty(buf: &mut [u8]) -> usize {
    match File::open("/dev/tty") {
        Ok(mut f) => f.read(buf).unwrap_or(0),
        Err(e) => {
            eprintln!("snake_game bridge: open /dev/tty for read: {e}");
            0
        }
    }
}

// ── Terminal lifecycle ─────────────────────────────────────────────────────────

/// Enter raw mode (no echo, single-keypress) and switch to the alternate screen.
/// Returns 1 on success, 0 on failure (e.g. stdout is not a tty).
#[no_mangle]
pub extern "Rust" fn tui_init() -> i64 {
    let ok = run_stty(&["-echo", "-icanon", "min", "1", "time", "0"]);
    if !ok {
        return 0;
    }
    RAW_MODE.store(true, Ordering::SeqCst);
    // Hide cursor; switch to alternate screen buffer.
    print!("\x1b[?25l\x1b[?1049h");
    stdout().flush().ok();
    // TODO: install a panic/SIGINT hook here to call tui_drop() so the terminal
    // is restored even when the game crashes or the user presses Ctrl-C.
    // Requires either a signal handler (libc) or a panic hook — deferred to Phase 4.
    1
}

/// Leave alternate screen, show cursor, restore cooked mode.
#[no_mangle]
pub extern "Rust" fn tui_drop() {
    if RAW_MODE.swap(false, Ordering::SeqCst) {
        print!("\x1b[?1049l\x1b[?25h");
        stdout().flush().ok();
        run_stty(&["sane"]);
    }
}

// ── Output ────────────────────────────────────────────────────────────────────

/// Clear the screen and move the cursor to (1,1).
#[no_mangle]
pub extern "Rust" fn tui_clear() {
    print!("\x1b[2J\x1b[H");
    stdout().flush().ok();
}

/// Move the cursor to row, col (1-indexed, clamped to ≥ 1).
#[no_mangle]
pub extern "Rust" fn tui_set_cursor(row: i64, col: i64) {
    let row = row.max(1);
    let col = col.max(1);
    print!("\x1b[{};{}H", row, col);
    stdout().flush().ok();
}

/// Print `text` at the current cursor position with optional bold/underline/colour.
/// ESC bytes are stripped to prevent ANSI injection from caller-supplied text.
///
/// fg_color encoding: 0=default 1=black 2=red 3=green 4=yellow 5=blue 6=magenta 7=cyan 8=white
#[no_mangle]
pub extern "Rust" fn tui_print(text: String, bold: bool, underline: bool, fg_color: i64) {
    let text         = text.replace('\x1b', "");
    let bold_on      = if bold      { "\x1b[1m" } else { "" };
    let underline_on = if underline { "\x1b[4m" } else { "" };
    let color_on     = ansi_fg(fg_color);
    print!("{}{}{}{}\x1b[0m", bold_on, underline_on, color_on, text);
    stdout().flush().ok();
}

fn ansi_fg(n: i64) -> &'static str {
    match n {
        1 => "\x1b[30m",
        2 => "\x1b[31m",
        3 => "\x1b[32m",
        4 => "\x1b[33m",
        5 => "\x1b[34m",
        6 => "\x1b[35m",
        7 => "\x1b[36m",
        8 => "\x1b[37m",
        _ => "",
    }
}

// ── Input ─────────────────────────────────────────────────────────────────────

/// Wait up to `millis` milliseconds for a keypress.
/// Returns the key string on success or "" on timeout.
///
/// Key encoding (mirrors pkg/tui/src/internal/ffi.mvl):
///   "Up" | "Down" | "Left" | "Right" | "Escape" | "Enter" | "Backspace"
///   "<char>"  — printable character (e.g. "q", " ")
///   ""        — timeout, no key pressed
#[no_mangle]
pub extern "Rust" fn tui_read_key_timeout(millis: i64) -> String {
    // Use stty VTIME (1/10-second units) for timeout; granularity ~100 ms.
    let tenths = (millis / 100).clamp(0, 255) as u64;
    run_stty(&["min", "0", "time", &tenths.to_string()]);
    let mut b = [0u8; 1];
    let n = read_tty(&mut b);
    // Restore blocking raw mode.
    run_stty(&["-echo", "-icanon", "min", "1", "time", "0"]);
    if n == 0 {
        return String::new(); // timeout
    }
    match b[0] {
        b'\r' | b'\n' => "Enter".to_string(),
        127            => "Backspace".to_string(),
        27             => read_escape_sequence(),
        c if c >= 0x20 => String::from_utf8(vec![c]).unwrap_or_default(),
        _              => String::new(),
    }
}

// Decode an ANSI escape sequence (ESC already consumed).
fn read_escape_sequence() -> String {
    let mut seq = [0u8; 2];
    // Allow zero-wait reads for the sequence bytes (1/10 s).
    run_stty(&["min", "0", "time", "1"]);
    let n = read_tty(&mut seq);
    // Restore raw mode.
    run_stty(&["-echo", "-icanon", "min", "1", "time", "0"]);
    if n >= 2 && seq[0] == b'[' {
        return match seq[1] {
            b'A' => "Up".to_string(),
            b'B' => "Down".to_string(),
            b'C' => "Right".to_string(),
            b'D' => "Left".to_string(),
            _    => "Escape".to_string(),
        };
    }
    "Escape".to_string()
}

// ── Clock ─────────────────────────────────────────────────────────────────────

/// Return the current monotonic time in milliseconds.
/// Used by main.mvl to seed the random food position generator each tick.
#[no_mangle]
pub extern "Rust" fn clock_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Pseudo-random number generator ────────────────────────────────────────────

/// LCG step: returns a non-negative value in [0, 999_999].
/// Used by main.mvl to place food; % board.width and % board.height are safe
/// because rand_next always returns a value in [0, 999_999].
#[no_mangle]
pub extern "Rust" fn rand_next(seed: i64) -> i64 {
    let mixed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    // Map to [0, 999_999]: Rust signed % truncates toward zero, so negative
    // seeds yield a negative remainder. Adding 1_000_000 guarantees the
    // intermediate is non-negative before the final modulo.
    (mixed % 1_000_000 + 1_000_000) % 1_000_000
}
