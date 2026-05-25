// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime primitives for the Rust backend.
//!
//! Every MVL actor program depends on this module through the `mvl_runtime`
//! prelude.  The transpiler emits calls to these functions instead of inlining
//! `std::thread::spawn` + `std::sync::mpsc` directly.
//!
//! # Swapping the scheduler
//!
//! To change the actor scheduler (e.g. tokio, crossbeam, FreeRTOS stubs),
//! replace the type aliases and function bodies in this module.  The
//! generated code does not reference `std::thread` or `std::sync::mpsc`
//! directly — only these abstractions.
//!
//! See ADR-0037: Runtime Abstraction Layer.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread;

// ── Type aliases ─────────────────────────────────────────────────────────────

/// Actor mailbox sender — wraps the channel sender used by actor handles.
///
/// Implements `Clone` (via `SyncSender::clone`) so actor handles are cheap
/// to copy and safe to send across threads (tag capability).
pub type MvlSender<M> = mpsc::SyncSender<M>;

/// Actor mailbox receiver — wraps the channel receiver used by actor loops.
pub type MvlReceiver<M> = mpsc::Receiver<M>;

/// Actor join handle — wraps the thread handle for structured concurrency.
pub type MvlJoinHandle = thread::JoinHandle<()>;

// ── Actor lifecycle ──────────────────────────────────────────────────────────

/// Default mailbox capacity for actor channels.
pub const MVL_ACTOR_MAILBOX_CAPACITY: usize = 256;

/// Create an actor mailbox channel with the default capacity (256 messages).
pub fn mvl_channel<M>() -> (MvlSender<M>, MvlReceiver<M>) {
    mpsc::sync_channel(MVL_ACTOR_MAILBOX_CAPACITY)
}

/// Spawn an actor thread.
///
/// The closure runs on a new OS thread and owns the actor state exclusively.
pub fn mvl_spawn<F>(f: F) -> MvlJoinHandle
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(f)
}

/// Fire-and-forget send — drops the message if the mailbox is full.
///
/// This matches MVL's fire-and-forget semantics: callers MUST NOT rely on
/// message delivery under load.  See Spec 015 (actors).
pub fn mvl_send<M>(sender: &MvlSender<M>, msg: M) {
    let _ = sender.try_send(msg);
}

// ── Join-handle registry for `concurrently {}` blocks ────────────────────────

thread_local! {
    static MVL_ACTOR_HANDLES: RefCell<Vec<MvlJoinHandle>> = RefCell::new(Vec::new());
}

/// Register an actor's join handle for later joining.
///
/// Called by each `_start_<actor>()` function after spawning the actor thread.
pub fn mvl_register_actor(h: MvlJoinHandle) {
    MVL_ACTOR_HANDLES.with(|v| v.borrow_mut().push(h));
}

/// Join all registered actor handles, then clear the registry.
///
/// Called at the end of every `concurrently {}` block to implement structured
/// concurrency: all actors spawned in the block must terminate before the
/// block returns.
pub fn mvl_join_actors() {
    MVL_ACTOR_HANDLES.with(|v| {
        for h in v.borrow_mut().drain(..) {
            let _ = h.join();
        }
    });
}
