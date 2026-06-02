// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime interface — `tokio` target implementation (Phase 9 Stage 2).
//!
//! Provides the same named interface as `runtime/rust/src/actors.rs` but runs
//! actor dispatch loops as tokio tasks instead of OS threads.  A global
//! multi-thread tokio [`Runtime`] is created lazily on the first actor spawn;
//! `fn main()` does **not** need `#[tokio::main]` — the emitter is unchanged.
//!
//! # Scaling improvement over the default runtime
//!
//! The default runtime spawns one OS thread per actor (~8 KB stack, ~10 K
//! actors before hitting OS limits).  This runtime spawns tokio tasks (~few
//! hundred bytes, M:N scheduled), targeting 1 M+ concurrent actors on a
//! fixed-size thread pool.
//!
//! # Limitation: synchronous behaviors
//!
//! Behavior bodies are still emitted as synchronous Rust — a behavior that
//! calls a blocking syscall will stall a tokio worker thread.  Full async
//! behavior emission is deferred to Phase 9 Stage 3.  Until then, avoid
//! long-blocking calls in actor behaviors when using `--target=tokio`.
//!
//! ADR-0027 §"Actor runtime interface".

use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

// ── Global tokio runtime ────────────────────────────────────────────────────

static MVL_TOKIO_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    MVL_TOKIO_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("[mvl runtime] failed to initialize tokio runtime")
    })
}

// ── Internal channel variants ───────────────────────────────────────────────

enum SenderInner<M: Send + 'static> {
    /// Bounded queue — drop newest message when full (`try_send`).
    BoundedDrop(mpsc::Sender<M>),
    /// Bounded queue — block sender when full.
    ///
    /// Uses `block_in_place` + `send().await` so callers inside tokio tasks
    /// do not panic (unlike `blocking_send` which panics in async context).
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

/// Receiving end of an actor mailbox — consumed by the actor task.
pub struct MvlReceiver<M: Send + 'static>(ReceiverInner<M>);

/// Opaque join handle — returned by [`mvl_spawn`] / [`mvl_actor_run`].
pub struct MvlJoinHandle(tokio::task::JoinHandle<()>);

// ── MvlSender impl ─────────────────────────────────────────────────────────

impl<M: Send + 'static> MvlSender<M> {
    /// Send a message to the actor, respecting the mailbox policy.
    ///
    /// - `BoundedDrop`: silently drops when the queue is full.
    /// - `BoundedBlock`: blocks (cooperatively via `block_in_place`) until space is available.
    /// - `Unbounded`: never blocks; queue grows without bound.
    pub fn send(&self, msg: M) {
        match self.0.as_ref() {
            SenderInner::BoundedDrop(tx) => {
                let _ = tx.try_send(msg);
            }
            SenderInner::BoundedBlock(tx) => {
                // `blocking_send` panics inside async contexts.  Use
                // `block_in_place` which is safe from tokio worker threads and
                // yields the scheduler while waiting for capacity.
                let tx = tx.clone();
                let _ = tokio::task::block_in_place(move || {
                    tokio::runtime::Handle::current().block_on(tx.send(msg))
                });
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

// ── MvlWeakSender ──────────────────────────────────────────────────────────

/// Non-owning sender — used by actors to hold a `self` tag capability.
pub struct MvlWeakSender<M: Send + 'static>(std::sync::Weak<SenderInner<M>>);

impl<M: Send + 'static> Clone for MvlWeakSender<M> {
    fn clone(&self) -> Self {
        MvlWeakSender(self.0.clone())
    }
}

impl<M: Send + 'static> MvlWeakSender<M> {
    pub fn upgrade(&self) -> Option<MvlSender<M>> {
        self.0.upgrade().map(MvlSender)
    }
}

impl<M: Send + 'static> MvlSender<M> {
    pub fn downgrade(&self) -> MvlWeakSender<M> {
        MvlWeakSender(Arc::downgrade(&self.0))
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
            MvlReceiver(ReceiverInner::Unbounded(rx)),
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
            MvlReceiver(ReceiverInner::Bounded(rx)),
        )
    }
}

// ── Spawn and lifecycle ────────────────────────────────────────────────────

/// Spawn a blocking closure as a tokio `spawn_blocking` task.
///
/// Kept for API parity with the default runtime.  Actor code uses
/// [`mvl_actor_run`] instead, which runs an async dispatch loop.
pub fn mvl_spawn<F: FnOnce() + Send + 'static>(f: F) -> MvlJoinHandle {
    MvlJoinHandle(runtime().spawn_blocking(f))
}

// Use a global Mutex instead of thread_local so handles registered from inside
// tokio tasks (e.g. actors spawning child actors) are visible to the main
// thread when mvl_join_actors() drains the list.
static MVL_ACTOR_HANDLES: std::sync::Mutex<Vec<MvlJoinHandle>> = std::sync::Mutex::new(Vec::new());

/// Register a spawned actor so [`mvl_join_actors`] can await it.
pub fn mvl_register_actor(h: MvlJoinHandle) {
    MVL_ACTOR_HANDLES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(h);
}

/// Await every registered actor task, then return.
///
/// Called once at the end of `fn main()`.  All actor handles (`MvlSender`s)
/// must have been dropped before this call so that the async recv loop exits
/// and each task completes naturally.
pub fn mvl_join_actors() {
    let handles: Vec<_> = MVL_ACTOR_HANDLES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain(..)
        .collect();
    runtime().block_on(async move {
        for h in handles {
            if h.0.await.is_err() {
                eprintln!("[mvl runtime] actor task panicked");
            }
        }
    });
}

/// Spawn a tokio task that owns `state` and runs the async dispatch loop.
///
/// Awaits `rx.recv()` asynchronously so the tokio worker thread is free to
/// schedule other tasks while an actor is idle.  Exits when all senders are
/// dropped (`recv()` returns `None`).
///
/// Mirrors the default runtime API — generated code is identical across
/// `--target` variants.  ADR-0027 §"Actor runtime interface".
pub fn mvl_actor_run<S, M>(rx: MvlReceiver<M>, state: S, dispatch: fn(&mut S, M)) -> MvlJoinHandle
where
    S: Send + 'static,
    M: Send + 'static,
{
    MvlJoinHandle(runtime().spawn(async move {
        let mut actor = state;
        let mut rx = rx;
        loop {
            let msg = match &mut rx.0 {
                ReceiverInner::Bounded(r) => r.recv().await,
                ReceiverInner::Unbounded(r) => r.recv().await,
            };
            match msg {
                Some(msg) => dispatch(&mut actor, msg),
                None => break,
            }
        }
    }))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify round-trip: channel → actor task → join.
    ///
    /// Spawns a counter actor via `mvl_actor_run`, sends N increment messages,
    /// drops the sender so the task exits, then joins via `mvl_join_actors`.
    #[test]
    fn actor_run_processes_messages_and_exits() {
        struct State {
            count: i64,
        }
        enum Msg {
            Increment(i64),
        }
        fn dispatch(s: &mut State, msg: Msg) {
            match msg {
                Msg::Increment(n) => s.count += n,
            }
        }

        let (tx, rx) = mvl_channel::<Msg>(10, 0);
        let handle = mvl_actor_run(rx, State { count: 0 }, dispatch);
        mvl_register_actor(handle);

        tx.send(Msg::Increment(1));
        tx.send(Msg::Increment(2));
        tx.send(Msg::Increment(3));
        drop(tx); // signal shutdown

        mvl_join_actors(); // waits for the actor task to drain and exit
    }

    /// Two actors on the same tokio runtime do not deadlock each other.
    #[test]
    fn multiple_actors_run_concurrently() {
        enum Msg {
            Noop,
        }
        fn dispatch(_s: &mut (), _msg: Msg) {}

        for _ in 0..4 {
            let (tx, rx) = mvl_channel::<Msg>(16, 0);
            let h = mvl_actor_run(rx, (), dispatch);
            mvl_register_actor(h);
            tx.send(Msg::Noop);
            drop(tx);
        }

        mvl_join_actors();
    }
}
