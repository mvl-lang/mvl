//! bridge.rs — Rust implementations of the `extern "rust"` trust boundary
//! declared in main.mvl.
//!
//! The game loop requires one capability not provided by std.tui's Phase 2
//! stubs: a non-blocking keypress read with a millisecond timeout. This
//! function is declared in main.mvl as:
//!
//!   extern "rust" {
//!       fn read_key_nonblocking(timeout_ms: Int) -> Option<Key>;
//!   }
//!
//! Phase 2 plan: this bridge implements `read_key_nonblocking` using
//! crossterm's `event::poll` + `event::read`. The returned `Key` value
//! is the MVL type generated from std/tui.mvl.
//!
//! Phase 3: std.tui will be backed by a full crossterm integration, at which
//! point read_key_nonblocking moves into the stdlib and this bridge is removed.
//!
//! Compile: `mvl build examples/snake_game/main.mvl`
//! (bridge.rs is detected automatically and linked in as a sibling Rust file.)

use mvl_runtime::prelude::*;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use std::time::Duration;

// Types generated from std/tui.mvl — accessible as `crate::tui::*`.
use crate::tui::{Key, Direction};

// ── Trust boundary: non-blocking input ────────────────────────────────────

/// Poll for a key event for up to `timeout_ms` milliseconds.
///
/// Returns Some(key) if a keypress arrived within the timeout, None otherwise.
/// Crossterm's `poll` is non-blocking: it parks the thread for at most
/// `timeout_ms` ms while waiting for an event, then returns.
///
/// The MVL game loop calls this once per tick (100 ms) to read player input
/// without stalling. The game advances each tick regardless of input.
#[no_mangle]
pub extern "Rust" fn read_key_nonblocking(timeout_ms: i64) -> Option<Key> {
    let duration = Duration::from_millis(timeout_ms.max(0) as u64);
    match event::poll(duration) {
        Ok(true) => match event::read() {
            Ok(Event::Key(KeyEvent { code, .. })) => Some(crossterm_key_to_mvl(code)),
            _ => None,
        },
        _ => None,
    }
}

// ── Internal helper ────────────────────────────────────────────────────────

/// Map a crossterm KeyCode to the MVL Key enum.
fn crossterm_key_to_mvl(code: KeyCode) -> Key {
    match code {
        KeyCode::Up        => Key::Arrow(Direction::Up),
        KeyCode::Down      => Key::Arrow(Direction::Down),
        KeyCode::Left      => Key::Arrow(Direction::Left),
        KeyCode::Right     => Key::Arrow(Direction::Right),
        KeyCode::Char('w') => Key::Arrow(Direction::Up),
        KeyCode::Char('s') => Key::Arrow(Direction::Down),
        KeyCode::Char('a') => Key::Arrow(Direction::Left),
        KeyCode::Char('d') => Key::Arrow(Direction::Right),
        KeyCode::Enter     => Key::Enter,
        KeyCode::Esc       => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete    => Key::Delete,
        KeyCode::Tab       => Key::Tab,
        KeyCode::F(n)      => Key::F(n as i64),
        KeyCode::Char(c)   => Key::Char(c),
        _                  => Key::Escape,  // treat unknown keys as quit-safe default
    }
}
