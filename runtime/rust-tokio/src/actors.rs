// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime interface — `tokio` target implementation.
//!
//! Provides the same named interface as `runtime/rust/src/actors.rs` but uses
//! tokio channels for mailboxes.  Actor closures are still spawned on OS threads
//! via `std::thread::spawn` (async actor bodies are out of scope for Phase 9
//! Stage 2); tokio is used only for the channel layer.
//!
//! The receiver wraps the tokio receiver in a `Mutex` so that `recv(&self)` has
//! the same signature as the default runtime (which uses `mpsc::Receiver::recv`
//! taking `&self`).  This keeps the emitter output identical across targets.
//!
//! ADR-0027 §"Actor runtime interface".

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc;

// ── Internal channel variant ────────────────────────────────────────────────

enum SenderInner<M: Send + 'static> {
    /// Bounded queue — drop newest message when full (`try_send`).
    BoundedDrop(mpsc::Sender<M>),
    /// Bounded queue — block sender when full (`blocking_send`).
    BoundedBlock(mpsc::Sender<M>),
    /// Unbounded queue — never drops or blocks.
    Unbounded(mpsc::UnboundedSender<M>),
}

enum ReceiverInner<M: Send + 'static> {
    Bounded(mpsc::Receiver<M>),
    Unbounded(mpsc::UnboundedReceiver<M>),
}

// ── Public types ────────────────────────────────────────────────────────────

/// Cloneable actor handle — cheap to share across threads.
///
/// Created by [`mvl_channel`]; stored as the `_sender` field of every actor
/// handle struct emitted by the Rust backend.
pub struct MvlSender<M: Send + 'static>(Arc<SenderInner<M>>);

/// Receiving end of an actor mailbox — owned by the actor thread.
///
/// Wraps the tokio receiver in a `Mutex` so that `recv` takes `&self`,
/// matching the signature of the default runtime.
pub struct MvlReceiver<M: Send + 'static>(Mutex<ReceiverInner<M>>);

/// Opaque join handle — returned by [`mvl_spawn`], consumed by [`mvl_register_actor`].
pub struct MvlJoinHandle(thread::JoinHandle<()>);

// ── MvlSender impl ─────────────────────────────────────────────────────────

impl<M: Send + 'static> MvlSender<M> {
    /// Send a message to the actor, respecting the mailbox policy.
    ///
    /// - `BoundedDrop`: drops the message silently when the queue is full.
    /// - `BoundedBlock`: blocks the caller until space is available.
    /// - `Unbounded`: never blocks; queue grows without bound.
    pub fn send(&self, msg: M) {
        match self.0.as_ref() {
            SenderInner::BoundedDrop(tx) => {
                let _ = tx.try_send(msg);
            }
            SenderInner::BoundedBlock(tx) => {
                let _ = tx.blocking_send(msg);
            }
            SenderInner::Unbounded(tx) => {
                let _ = tx.send(msg);
            }
        }
    }
}

impl<M: Send + 'static> Clone for MvlSender<M> {
    fn clone(&self) -> Self {
        MvlSender(Arc::clone(&self.0))
    }
}

// ── MvlReceiver impl ───────────────────────────────────────────────────────

impl<M: Send + 'static> MvlReceiver<M> {
    /// Block until the next message arrives, or return `None` when all senders
    /// have been dropped (actor shutdown signal).
    ///
    /// Uses `blocking_recv` which parks the calling thread without requiring a
    /// tokio runtime context — safe to call from `std::thread::spawn` threads.
    pub fn recv(&self) -> Option<M> {
        let mut inner = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match &mut *inner {
            ReceiverInner::Bounded(rx) => rx.blocking_recv(),
            ReceiverInner::Unbounded(rx) => rx.blocking_recv(),
        }
    }
}

// ── Channel factory ────────────────────────────────────────────────────────

/// Create a linked sender/receiver pair backed by tokio channels.
///
/// | `capacity` | `policy` | behaviour |
/// |------------|----------|-----------|
/// | `<= 0`     | any      | unbounded queue |
/// | `> 0`      | `0`      | bounded, drop newest when full |
/// | `> 0`      | `1`      | bounded, block sender when full |
pub fn mvl_channel<M: Send + 'static>(
    capacity: i64,
    policy: i64,
) -> (MvlSender<M>, MvlReceiver<M>) {
    if capacity <= 0 {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            MvlSender(Arc::new(SenderInner::Unbounded(tx))),
            MvlReceiver(Mutex::new(ReceiverInner::Unbounded(rx))),
        )
    } else {
        let (tx, rx) = mpsc::channel(capacity as usize);
        let inner = if policy == 1 {
            SenderInner::BoundedBlock(tx)
        } else {
            SenderInner::BoundedDrop(tx)
        };
        (
            MvlSender(Arc::new(inner)),
            MvlReceiver(Mutex::new(ReceiverInner::Bounded(rx))),
        )
    }
}

// ── Spawn and lifecycle ────────────────────────────────────────────────────

/// Spawn an actor thread.  Returns an opaque handle for [`mvl_register_actor`].
///
/// Actor closures run on OS threads (async actor bodies are out of scope for
/// Stage 2; see ADR-0027).  The tokio runtime is used only for channels.
pub fn mvl_spawn<F: FnOnce() + Send + 'static>(f: F) -> MvlJoinHandle {
    MvlJoinHandle(thread::spawn(f))
}

thread_local! {
    static MVL_ACTOR_HANDLES: RefCell<Vec<MvlJoinHandle>> = RefCell::new(Vec::new());
}

/// Register a spawned actor so [`mvl_join_actors`] can await it.
pub fn mvl_register_actor(h: MvlJoinHandle) {
    MVL_ACTOR_HANDLES.with(|v| v.borrow_mut().push(h));
}

/// Block until every registered actor has exited.
///
/// Called once at the end of `fn main()`.  All actor handles (`MvlSender`s)
/// must have been dropped before this call so that `MvlReceiver::recv()`
/// returns `None` and each actor thread exits naturally.
pub fn mvl_join_actors() {
    MVL_ACTOR_HANDLES.with(|v| {
        for h in v.borrow_mut().drain(..) {
            if h.0.join().is_err() {
                eprintln!("[mvl runtime] actor thread panicked");
            }
        }
    });
}

/// Spawn an actor thread that owns `state` and runs the dispatch loop.
///
/// Calls `dispatch(state, msg)` for each incoming message until the receiver
/// is closed (all senders have been dropped).  Returns an opaque join handle
/// for [`mvl_register_actor`].
///
/// Mirrors the default runtime implementation — generated code is identical
/// across `--target` variants.  ADR-0027 §"Actor runtime interface".
pub fn mvl_actor_run<S, M>(rx: MvlReceiver<M>, state: S, dispatch: fn(&mut S, M)) -> MvlJoinHandle
where
    S: Send + 'static,
    M: Send + 'static,
{
    mvl_spawn(move || {
        let mut actor = state;
        while let Some(msg) = rx.recv() {
            dispatch(&mut actor, msg);
        }
    })
}
