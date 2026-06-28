// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime interface — `tokio` target implementation.
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
//! Behavior bodies are emitted as synchronous Rust — a behavior that calls a
//! blocking syscall will stall a tokio worker thread.  Avoid long-blocking
//! calls in actor behaviors when using `--target=tokio`.
//!
//! ADR-0027 §"Actor runtime interface".

use std::sync::atomic::{AtomicU64, Ordering};
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

// ── Supervisor / link types (Phase 9, #1177) ──────────────────────────────
//
// Stubs that make generated code compile today.  Full implementations land
// with the supervisor feature (#1177 – #1180).

/// Unique identifier assigned to every actor at spawn time.
pub type ActorId = u64;

/// Reason an actor terminated (stub — fields added in #1177).
#[derive(Clone, Debug)]
pub struct ExitReason;

/// Opaque monitor registration token (stub — fields added in #1177).
#[derive(Clone, Debug)]
pub struct MonitorId;

static MVL_ACTOR_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocate a fresh [`ActorId`] unique within this process.
pub fn mvl_next_actor_id() -> ActorId {
    MVL_ACTOR_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Kill closures registered by spawned actors — invoked by [`mvl_join_actors`]
/// to send `_Shutdown` to every actor before awaiting handles. Required by the
/// strong-`_self_ref` shutdown protocol (default runtime commit bf6b4e07): each
/// actor holds a strong sender to its own mailbox, so simply dropping the
/// external handle is not enough to close the channel — `_Shutdown` must be
/// delivered explicitly, which nulls `_self_ref` and lets the dispatch loop exit.
static MVL_KILL_FNS: std::sync::Mutex<Vec<Box<dyn FnOnce() + Send>>> =
    std::sync::Mutex::new(Vec::new());

/// Register type-erased actor controls for link/monitor support.
///
/// Currently captures the kill closure so [`mvl_join_actors`] can drive the
/// `_Shutdown` cascade. Exit/down notifications are no-ops until the full
/// supervisor runtime (#1177) is implemented for the tokio target.
pub fn mvl_register_actor_controls(
    _id: ActorId,
    kill: Box<dyn FnOnce() + Send>,
    _exit: Box<dyn Fn(ActorId, ExitReason) + Send>,
    _down: Box<dyn Fn(ActorId, ExitReason, MonitorId) + Send>,
    _traps_exit: bool,
) {
    MVL_KILL_FNS.lock().unwrap().push(kill);
}

// ── Public types ────────────────────────────────────────────────────────────

/// Cloneable actor handle — cheap to share across threads.
///
/// Created by [`mvl_channel`]; stored as the `_sender` field of every actor
/// handle struct emitted by the Rust backend.
pub struct MvlSender<M: Send + 'static>(Arc<SenderInner<M>>);

/// Receiving end of an actor mailbox — consumed by the actor task.
pub struct MvlReceiver<M: Send + 'static>(ReceiverInner<M>);

impl<M: Send + 'static> MvlReceiver<M> {
    async fn recv(&mut self) -> Option<M> {
        match &mut self.0 {
            ReceiverInner::Bounded(r) => r.recv().await,
            ReceiverInner::Unbounded(r) => r.recv().await,
        }
    }
}

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
                // `block_in_place` + `runtime().block_on` which is safe from
                // both tokio worker threads and non-tokio threads.
                // Clone the sender (cheap Arc bump) because block_in_place
                // requires an owned value; we only have a shared ref here.
                let tx = tx.clone();
                if tokio::task::block_in_place(move || runtime().block_on(tx.send(msg))).is_err() {
                    eprintln!("[mvl runtime] send failed: receiver dropped");
                }
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
///
/// NOTE: each call consumes one slot from tokio's blocking-thread pool (capped
/// at 512 by default).  Do not use for long-lived loops — use [`mvl_actor_run`].
pub fn mvl_spawn<F: FnOnce() + Send + 'static>(f: F) -> MvlJoinHandle {
    MvlJoinHandle(runtime().spawn_blocking(f))
}

// Use a global Mutex instead of thread_local so handles registered from inside
// tokio tasks (e.g. actors spawning child actors) are visible to the main
// thread when mvl_join_actors() drains the list.
static MVL_ACTOR_HANDLES: std::sync::Mutex<Vec<MvlJoinHandle>> = std::sync::Mutex::new(Vec::new());

/// Register a spawned actor so [`mvl_join_actors`] can await it.
pub fn mvl_register_actor(h: MvlJoinHandle) {
    MVL_ACTOR_HANDLES.lock().unwrap().push(h);
}

/// Await every registered actor task, then return.
///
/// Called once at the end of `fn main()`. Shutdown protocol:
///
/// 1. Drain the registered kill closures and call each one, sending
///    `_Shutdown` to every actor. The dispatch handler nulls `_self_ref` and
///    returns `false`, dropping the actor's own strong sender clone so the
///    channel can close naturally.
/// 2. Await every registered actor task.
///
/// Actors hold a strong `_self_ref` to their own mailbox (so behaviors can
/// pass `self` as a tag capability), which means "all external handles
/// dropped" no longer closes the channel — the `_Shutdown` cascade is what
/// allows tasks to exit.
pub fn mvl_join_actors() {
    let kills: Vec<_> = MVL_KILL_FNS.lock().unwrap().drain(..).collect();
    for k in kills {
        k();
    }
    let handles: Vec<_> = MVL_ACTOR_HANDLES.lock().unwrap().drain(..).collect();
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
/// `_actor_id` is accepted for API parity with the supervisor runtime; unused
/// until Phase 9 link/monitor support lands (#1177).
pub fn mvl_actor_run<S, M>(
    rx: MvlReceiver<M>,
    state: S,
    dispatch: fn(&mut S, M) -> bool,
    _actor_id: ActorId,
) -> MvlJoinHandle
where
    S: Send + 'static,
    M: Send + 'static,
{
    MvlJoinHandle(runtime().spawn(async move {
        let mut actor = state;
        let mut rx = rx;
        while let Some(msg) = rx.recv().await {
            if !dispatch(&mut actor, msg) {
                break;
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
    /// Joins the handle directly (not via the global registry) so the test is
    /// isolated from parallel test runs that share `MVL_ACTOR_HANDLES`.
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

        tx.send(Msg::Increment(1));
        tx.send(Msg::Increment(2));
        tx.send(Msg::Increment(3));
        drop(tx); // signal shutdown

        runtime().block_on(handle.0).expect("actor task panicked");
    }

    /// Two actors on the same tokio runtime do not deadlock each other.
    ///
    /// Joins handles directly to avoid sharing `MVL_ACTOR_HANDLES` with other
    /// parallel tests.
    #[test]
    fn multiple_actors_run_concurrently() {
        enum Msg {
            Noop,
        }
        fn dispatch(_s: &mut (), _msg: Msg) {}

        let mut handles = Vec::new();
        for _ in 0..4 {
            let (tx, rx) = mvl_channel::<Msg>(16, 0);
            handles.push(mvl_actor_run(rx, (), dispatch));
            tx.send(Msg::Noop);
            drop(tx);
        }

        runtime().block_on(async {
            for h in handles {
                h.0.await.expect("actor task panicked");
            }
        });
    }
}
