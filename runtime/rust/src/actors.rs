// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime interface — `default` target implementation.
//!
//! This module provides the **named interface** between the MVL emitter and the
//! actor runtime.  The emitter calls only the symbols defined here; swapping
//! `--target` replaces this module with an alternative implementation
//! (tokio, freertos, …) without changing any emitter output.
//!
//! ADR-0027 §"Actor runtime interface".

use std::cell::RefCell;
use std::sync::{mpsc, Arc};
use std::thread;

// ── Internal channel variant ────────────────────────────────────────────────

enum SenderInner<M: Send + 'static> {
    /// Bounded queue — drop newest message when full (`try_send`).
    BoundedDrop(mpsc::SyncSender<M>),
    /// Bounded queue — block sender when full (`send`).
    BoundedBlock(mpsc::SyncSender<M>),
    /// Unbounded queue — never drops or blocks.
    Unbounded(mpsc::Sender<M>),
}

// ── Public types ────────────────────────────────────────────────────────────

/// Cloneable actor handle — cheap to share across threads.
///
/// Created by [`mvl_channel`]; stored as the `_sender` field of every actor
/// handle struct emitted by the Rust backend.
pub struct MvlSender<M: Send + 'static>(Arc<SenderInner<M>>);

/// Receiving end of an actor mailbox — owned by the actor thread.
pub struct MvlReceiver<M: Send + 'static>(mpsc::Receiver<M>);

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
                let _ = tx.send(msg);
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

impl<M: Send + 'static> MvlSender<M> {
    /// Create a weak reference to this sender.  The weak ref does not prevent
    /// the channel from closing when all strong (`MvlSender`) clones are dropped.
    pub fn downgrade(&self) -> MvlWeakSender<M> {
        MvlWeakSender(Arc::downgrade(&self.0))
    }
}

/// Non-owning actor handle.  Held by the actor thread in `_self_ref` so that
/// behaviors can pass `self` as a tag argument without keeping the channel open.
/// When all external `MvlSender` handles are dropped the channel disconnects
/// and `MvlReceiver::recv()` returns `None` regardless of any live weak refs.
pub struct MvlWeakSender<M: Send + 'static>(std::sync::Weak<SenderInner<M>>);

impl<M: Send + 'static> Clone for MvlWeakSender<M> {
    fn clone(&self) -> Self {
        MvlWeakSender(self.0.clone())
    }
}

impl<M: Send + 'static> MvlWeakSender<M> {
    /// Upgrade to a strong sender, or `None` if all external handles are gone.
    pub fn upgrade(&self) -> Option<MvlSender<M>> {
        self.0.upgrade().map(MvlSender)
    }
}

// ── MvlReceiver impl ───────────────────────────────────────────────────────

impl<M: Send + 'static> MvlReceiver<M> {
    /// Block until the next message arrives, or return `None` when all senders
    /// have been dropped (actor shutdown signal).
    pub fn recv(&self) -> Option<M> {
        self.0.recv().ok()
    }
}

// ── Channel factory ────────────────────────────────────────────────────────

/// Create a linked sender/receiver pair.
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
        let (tx, rx) = mpsc::channel();
        (
            MvlSender(Arc::new(SenderInner::Unbounded(tx))),
            MvlReceiver(rx),
        )
    } else {
        let (tx, rx) = mpsc::sync_channel(capacity as usize);
        let inner = if policy == 1 {
            SenderInner::BoundedBlock(tx)
        } else {
            SenderInner::BoundedDrop(tx)
        };
        (MvlSender(Arc::new(inner)), MvlReceiver(rx))
    }
}

// ── Spawn and lifecycle ────────────────────────────────────────────────────

/// Spawn an actor thread.  Returns an opaque handle for [`mvl_register_actor`].
pub fn mvl_spawn<F: FnOnce() + Send + 'static>(f: F) -> MvlJoinHandle {
    MvlJoinHandle(thread::spawn(f))
}

thread_local! {
    static MVL_ACTOR_HANDLES: RefCell<Vec<MvlJoinHandle>> = const { RefCell::new(Vec::new()) };
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
/// The Rust backend emitter calls this instead of inlining the dispatch loop
/// inside a `mvl_spawn` closure, keeping generated code free of the loop
/// pattern and independent of runtime primitives.  ADR-0027 §"Actor runtime
/// interface".
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
