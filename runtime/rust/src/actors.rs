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
//!
//! ## Process links and monitors (Phase 9, #1177)
//!
//! - `mvl_link(a, b)` — bidirectional: if either dies, the other is killed or notified
//! - `mvl_monitor(watcher, target)` — one-way: watcher notified when target dies
//! - `mvl_set_trap_exit(id)` — actor receives exit signals as messages instead of dying
//! - Death detection via `catch_unwind` in the dispatch loop
//! - Exit processing via global `LINK_REGISTRY`

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;

// ── Actor identity ────────────────────────────────────────────────────────

/// Unique identifier for an actor instance, assigned at spawn time.
pub type ActorId = u64;

/// Unique identifier for a monitor relationship.
pub type MonitorId = u64;

/// Exit reason codes passed through the link/monitor infrastructure.
///
/// - `0` = Normal (actor exited because its channel closed)
/// - `1` = Panic (actor thread panicked)
/// - `2` = Killed (actor was killed via a link cascade)
pub type ExitReason = i64;

static NEXT_ACTOR_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MONITOR_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate the next unique actor ID.
pub fn mvl_next_actor_id() -> ActorId {
    NEXT_ACTOR_ID.fetch_add(1, Ordering::Relaxed)
}

fn next_monitor_id() -> MonitorId {
    NEXT_MONITOR_ID.fetch_add(1, Ordering::Relaxed)
}

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
/// is closed (all senders have been dropped) or dispatch returns `false`.
///
/// The dispatch function returns `bool`: `true` to continue, `false` to shut
/// down (used by the `_Shutdown` system message variant, Phase 9 #1177).
///
/// If `actor_id > 0`, the dispatch loop is wrapped in `catch_unwind` and
/// [`mvl_process_exit`] is called when the actor dies (normal or panic).
///
/// The Rust backend emitter calls this instead of inlining the dispatch loop
/// inside a `mvl_spawn` closure, keeping generated code free of the loop
/// pattern and independent of runtime primitives.  ADR-0027 §"Actor runtime
/// interface".
pub fn mvl_actor_run<S, M>(
    rx: MvlReceiver<M>,
    state: S,
    dispatch: fn(&mut S, M) -> bool,
    actor_id: ActorId,
) -> MvlJoinHandle
where
    S: Send + 'static,
    M: Send + 'static,
{
    mvl_spawn(move || {
        let mut actor = state;
        if actor_id > 0 {
            // Linked actor: catch panics, process exit signals.
            let result = catch_unwind(AssertUnwindSafe(|| {
                while let Some(msg) = rx.recv() {
                    if !dispatch(&mut actor, msg) {
                        return false; // shutdown requested
                    }
                }
                true // normal exit (channel closed)
            }));
            let reason: ExitReason = match result {
                Ok(true) => 0,  // Normal — channel closed
                Ok(false) => 2, // Killed — shutdown requested
                Err(_) => 1,    // Panic
            };
            mvl_process_exit(actor_id, reason);
        } else {
            // Legacy path (actor_id == 0): no link/monitor support.
            while let Some(msg) = rx.recv() {
                if !dispatch(&mut actor, msg) {
                    break;
                }
            }
        }
    })
}

// ── Link/monitor registry (Phase 9, #1177) ───────────────────────────────

/// Type-erased actor control interface.
///
/// Stored in the global registry so the exit-processing code can interact
/// with actors of any mailbox type.
struct ActorControls {
    /// Send a `_Shutdown` poison pill into the actor's typed mailbox.
    /// The dispatch loop receives it and returns `false`, stopping the actor.
    kill_fn: Box<dyn Fn() + Send + Sync>,

    /// Send a `_ExitSignal { from_id, reason }` into the actor's typed mailbox.
    /// Only called when the actor has `traps_exit` set.
    exit_notify_fn: Box<dyn Fn(ActorId, ExitReason) + Send + Sync>,

    /// Send a `_DownSignal { from_id, reason, monitor_id }` into the actor's mailbox.
    /// Called when a monitored actor dies.
    down_notify_fn: Box<dyn Fn(ActorId, ExitReason, MonitorId) + Send + Sync>,

    /// Whether this actor traps exit signals from linked actors.
    traps_exit: bool,
}

struct LinkRegistry {
    /// Per-actor controls (type-erased senders and flags).
    controls: HashMap<ActorId, ActorControls>,

    /// Bidirectional links: if `links[a]` contains `b`, then `links[b]` contains `a`.
    links: HashMap<ActorId, HashSet<ActorId>>,

    /// Monitors: target_id → list of (monitor_id, watcher_id).
    monitors: HashMap<ActorId, Vec<(MonitorId, ActorId)>>,

    /// Reverse index: monitor_id → target_id (for demonitor).
    monitor_targets: HashMap<MonitorId, ActorId>,
}

impl LinkRegistry {
    fn new() -> Self {
        Self {
            controls: HashMap::new(),
            links: HashMap::new(),
            monitors: HashMap::new(),
            monitor_targets: HashMap::new(),
        }
    }
}

fn global_registry() -> &'static Mutex<LinkRegistry> {
    static REGISTRY: OnceLock<Mutex<LinkRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(LinkRegistry::new()))
}

// ── Public link/monitor API ───────────────────────────────────────────────

/// Register an actor's type-erased controls in the global registry.
///
/// Called from generated `_start_{actor}` functions. The closures capture
/// the actor's typed `MvlSender<{Actor}Mailbox>` to send system messages.
pub fn mvl_register_actor_controls(
    id: ActorId,
    kill_fn: Box<dyn Fn() + Send + Sync>,
    exit_notify_fn: Box<dyn Fn(ActorId, ExitReason) + Send + Sync>,
    down_notify_fn: Box<dyn Fn(ActorId, ExitReason, MonitorId) + Send + Sync>,
    traps_exit: bool,
) {
    let mut reg = global_registry().lock().unwrap();
    reg.controls.insert(
        id,
        ActorControls {
            kill_fn,
            exit_notify_fn,
            down_notify_fn,
            traps_exit,
        },
    );
}

/// Create a bidirectional link between two actors.
///
/// If either actor dies, the other is either killed (default) or receives
/// an exit signal (if it has `traps_exit` set).
pub fn mvl_link(a: ActorId, b: ActorId) {
    let mut reg = global_registry().lock().unwrap();
    reg.links.entry(a).or_default().insert(b);
    reg.links.entry(b).or_default().insert(a);
}

/// Remove a bidirectional link between two actors.
pub fn mvl_unlink(a: ActorId, b: ActorId) {
    let mut reg = global_registry().lock().unwrap();
    if let Some(set) = reg.links.get_mut(&a) {
        set.remove(&b);
    }
    if let Some(set) = reg.links.get_mut(&b) {
        set.remove(&a);
    }
}

/// Create a one-way monitor: `watcher` is notified when `target` dies.
///
/// Returns a `MonitorId` that can be passed to [`mvl_demonitor`].
pub fn mvl_monitor(watcher: ActorId, target: ActorId) -> MonitorId {
    let mid = next_monitor_id();
    let mut reg = global_registry().lock().unwrap();
    reg.monitors.entry(target).or_default().push((mid, watcher));
    reg.monitor_targets.insert(mid, target);
    mid
}

/// Remove a monitor.
pub fn mvl_demonitor(id: MonitorId) {
    let mut reg = global_registry().lock().unwrap();
    if let Some(target) = reg.monitor_targets.remove(&id) {
        if let Some(list) = reg.monitors.get_mut(&target) {
            list.retain(|(mid, _)| *mid != id);
        }
    }
}

/// Mark an actor as trapping exit signals.
///
/// When a linked actor dies, this actor receives an exit notification
/// instead of being killed.
pub fn mvl_set_trap_exit(id: ActorId) {
    let mut reg = global_registry().lock().unwrap();
    if let Some(ctrl) = reg.controls.get_mut(&id) {
        ctrl.traps_exit = true;
    }
}

/// Process an actor's death: notify linked actors and monitors.
///
/// Called automatically when an actor's dispatch loop exits (normal, panic,
/// or shutdown). Runs on the dying actor's thread.
///
/// For each linked actor:
/// - If it has `traps_exit`: send an `_ExitSignal` message
/// - Otherwise: send a `_Shutdown` to kill it (cascade)
///
/// For each monitor watcher: send a `_DownSignal` message.
pub fn mvl_process_exit(dead_id: ActorId, reason: ExitReason) {
    // Collect actions under the lock, then execute outside to avoid deadlock.
    let (kill_targets, exit_targets, down_targets) = {
        let mut reg = global_registry().lock().unwrap();

        // Gather linked actors.
        let linked = reg.links.remove(&dead_id).unwrap_or_default();

        // Clean up reverse links.
        for &peer in &linked {
            if let Some(set) = reg.links.get_mut(&peer) {
                set.remove(&dead_id);
            }
        }

        // Partition linked actors by trap_exit flag.
        let mut kill_targets: Vec<ActorId> = Vec::new();
        let mut exit_targets: Vec<(ActorId, ExitReason)> = Vec::new();
        for peer in linked {
            if reg.controls.get(&peer).map_or(false, |c| c.traps_exit) {
                exit_targets.push((peer, reason));
            } else {
                kill_targets.push(peer);
            }
        }

        // Gather monitors.
        let monitored = reg.monitors.remove(&dead_id).unwrap_or_default();
        let down_targets: Vec<(MonitorId, ActorId)> = monitored;

        // Clean up monitor reverse index.
        for &(mid, _) in &down_targets {
            reg.monitor_targets.remove(&mid);
        }

        // Remove dead actor's controls.
        reg.controls.remove(&dead_id);

        (kill_targets, exit_targets, down_targets)
    };

    // Execute notifications outside the lock.
    let reg = global_registry().lock().unwrap();

    for target in kill_targets {
        if let Some(ctrl) = reg.controls.get(&target) {
            (ctrl.kill_fn)();
        }
    }

    for (target, exit_reason) in exit_targets {
        if let Some(ctrl) = reg.controls.get(&target) {
            (ctrl.exit_notify_fn)(dead_id, exit_reason);
        }
    }

    for (mid, watcher) in down_targets {
        if let Some(ctrl) = reg.controls.get(&watcher) {
            (ctrl.down_notify_fn)(dead_id, reason, mid);
        }
    }
}
