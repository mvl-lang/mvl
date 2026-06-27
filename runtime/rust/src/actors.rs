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

type KillFn = Arc<dyn Fn() + Send + Sync>;
type ExitNotifyFn = Arc<dyn Fn(ActorId, ExitReason) + Send + Sync>;
type DownNotifyFn = Arc<dyn Fn(ActorId, ExitReason, MonitorId) + Send + Sync>;

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
    static MVL_ACTOR_HANDLES: RefCell<Vec<MvlJoinHandle>> = const { RefCell::new(Vec::new()) };
}

/// Register a spawned actor so [`mvl_join_actors`] can await it.
pub fn mvl_register_actor(h: MvlJoinHandle) {
    MVL_ACTOR_HANDLES.with(|v| v.borrow_mut().push(h));
}

/// Block until every registered actor has exited.
///
/// Called once at the end of `fn main()`.  Shutdown protocol:
///
/// 1. Send `_Shutdown` to every registered actor.  The dispatch handler for
///    `_Shutdown` nulls `actor._self_ref` before returning `false`, dropping
///    the actor's own strong sender clone so the channel can close naturally.
/// 2. Clear the registry to release kill/exit/down sender clones.
/// 3. Join every actor thread.
pub fn mvl_join_actors() {
    let kill_fns: Vec<KillFn> = {
        let reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
        reg.controls.values().map(|c| Arc::clone(&c.kill_fn)).collect()
    };
    for kf in &kill_fns {
        kf();
    }
    drop(kill_fns);
    {
        let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
        reg.controls.clear();
        reg.links.clear();
        reg.monitors.clear();
        reg.monitor_targets.clear();
    }
    MVL_ACTOR_HANDLES.with(|v| {
        for h in v.borrow_mut().drain(..) {
            if h.0.join().is_err() {
                eprintln!("mvl: actor thread panicked");
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
                        return false; // shutdown requested (_self_ref nulled by handler)
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
    kill_fn: KillFn,

    /// Send a `_ExitSignal { from_id, reason }` into the actor's typed mailbox.
    /// Only called when the actor has `traps_exit` set.
    exit_notify_fn: ExitNotifyFn,

    /// Send a `_DownSignal { from_id, reason, monitor_id }` into the actor's mailbox.
    /// Called when a monitored actor dies.
    down_notify_fn: DownNotifyFn,

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
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
    reg.controls.insert(
        id,
        ActorControls {
            kill_fn: Arc::from(kill_fn),
            exit_notify_fn: Arc::from(exit_notify_fn),
            down_notify_fn: Arc::from(down_notify_fn),
            traps_exit,
        },
    );
}

/// Create a bidirectional link between two actors.
///
/// If either actor dies, the other is either killed (default) or receives
/// an exit signal (if it has `traps_exit` set).
pub fn mvl_link(a: ActorId, b: ActorId) {
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
    reg.links.entry(a).or_default().insert(b);
    reg.links.entry(b).or_default().insert(a);
}

/// Remove a bidirectional link between two actors.
pub fn mvl_unlink(a: ActorId, b: ActorId) {
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
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
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
    reg.monitors.entry(target).or_default().push((mid, watcher));
    reg.monitor_targets.insert(mid, target);
    mid
}

/// Remove a monitor.
pub fn mvl_demonitor(id: MonitorId) {
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
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
    let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ctrl) = reg.controls.get_mut(&id) {
        ctrl.traps_exit = true;
    }
}

/// Process an actor's death: notify linked actors and monitors.
///
/// Called automatically when an actor's dispatch loop exits (normal, panic,
/// or shutdown). Runs on the dying actor's thread.
///
/// Collects all notification closures under the lock, then executes them
/// outside the lock to avoid deadlock with `BoundedBlock` senders.
///
/// For each linked actor:
/// - If it has `traps_exit`: send an `_ExitSignal` message
/// - Otherwise: send a `_Shutdown` to kill it (cascade)
///
/// For each monitor watcher: send a `_DownSignal` message.
pub fn mvl_process_exit(dead_id: ActorId, reason: ExitReason) {
    // Phase 1: collect Arc-cloned closures under the lock.
    let (kill_fns, exit_fns, down_fns): (
        Vec<KillFn>,
        Vec<(ExitNotifyFn, ActorId, ExitReason)>,
        Vec<(DownNotifyFn, ActorId, ExitReason, MonitorId)>,
    ) = {
        let mut reg = global_registry().lock().unwrap_or_else(|p| p.into_inner());

        // Gather linked actors.
        let linked = reg.links.remove(&dead_id).unwrap_or_default();

        // Clean up reverse links.
        for &peer in &linked {
            if let Some(set) = reg.links.get_mut(&peer) {
                set.remove(&dead_id);
            }
        }

        // Partition linked actors by trap_exit flag, cloning Arc closures.
        let mut kill_fns: Vec<KillFn> = Vec::new();
        let mut exit_fns: Vec<(ExitNotifyFn, ActorId, ExitReason)> = Vec::new();
        for peer in linked {
            if let Some(ctrl) = reg.controls.get(&peer) {
                if ctrl.traps_exit {
                    exit_fns.push((Arc::clone(&ctrl.exit_notify_fn), dead_id, reason));
                } else {
                    kill_fns.push(Arc::clone(&ctrl.kill_fn));
                }
            }
        }

        // Gather monitors, cloning Arc closures.
        let monitored = reg.monitors.remove(&dead_id).unwrap_or_default();
        let mut down_fns: Vec<(DownNotifyFn, ActorId, ExitReason, MonitorId)> = Vec::new();
        for (mid, watcher) in monitored {
            reg.monitor_targets.remove(&mid);
            if let Some(ctrl) = reg.controls.get(&watcher) {
                down_fns.push((Arc::clone(&ctrl.down_notify_fn), dead_id, reason, mid));
            }
        }

        // Remove dead actor's controls.
        reg.controls.remove(&dead_id);

        (kill_fns, exit_fns, down_fns)
    };

    // Phase 2: execute all notifications outside the lock — avoids deadlock
    // with BoundedBlock senders that may block when the target's mailbox is full.
    for kill_fn in &kill_fns {
        kill_fn();
    }

    for (exit_fn, from_id, exit_reason) in &exit_fns {
        exit_fn(*from_id, *exit_reason);
    }

    for (down_fn, from_id, exit_reason, mid) in &down_fns {
        down_fn(*from_id, *exit_reason, *mid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: register an actor with mock closures that log into shared state.
    fn register_mock_actor(
        id: ActorId,
        traps_exit: bool,
        kill_log: Arc<Mutex<Vec<ActorId>>>,
        exit_log: Arc<Mutex<Vec<(ActorId, ActorId, ExitReason)>>>,
        down_log: Arc<Mutex<Vec<(ActorId, ActorId, ExitReason, MonitorId)>>>,
    ) {
        let kill_id = id;
        let kl = Arc::clone(&kill_log);
        let exit_id = id;
        let el = Arc::clone(&exit_log);
        let down_id = id;
        let dl = Arc::clone(&down_log);
        mvl_register_actor_controls(
            id,
            Box::new(move || {
                kl.lock().unwrap().push(kill_id);
            }),
            Box::new(move |from, reason| {
                el.lock().unwrap().push((exit_id, from, reason));
            }),
            Box::new(move |from, reason, mid| {
                dl.lock().unwrap().push((down_id, from, reason, mid));
            }),
            traps_exit,
        );
    }

    #[test]
    fn linked_actor_killed_on_death() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let a = mvl_next_actor_id();
        let b = mvl_next_actor_id();
        register_mock_actor(
            a,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            b,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        mvl_link(a, b);
        mvl_process_exit(a, 1); // actor A panicked

        let kills = kill_log.lock().unwrap();
        assert_eq!(
            *kills,
            vec![b],
            "linked actor B should be killed when A dies"
        );
        assert!(
            exit_log.lock().unwrap().is_empty(),
            "no exit signals for non-trapping actors"
        );
    }

    #[test]
    fn traps_exit_receives_exit_signal_instead_of_kill() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let a = mvl_next_actor_id();
        let b = mvl_next_actor_id();
        register_mock_actor(
            a,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            b,
            true,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        mvl_link(a, b);
        mvl_process_exit(a, 1); // actor A panicked

        assert!(
            kill_log.lock().unwrap().is_empty(),
            "trapping actor should NOT be killed"
        );
        let exits = exit_log.lock().unwrap();
        assert_eq!(exits.len(), 1);
        assert_eq!(
            exits[0],
            (b, a, 1),
            "B should receive exit signal from A with reason=1"
        );
    }

    #[test]
    fn monitor_watcher_receives_down_signal() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let target = mvl_next_actor_id();
        let watcher = mvl_next_actor_id();
        register_mock_actor(
            target,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            watcher,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        let mid = mvl_monitor(watcher, target);
        mvl_process_exit(target, 0); // target exited normally

        let downs = down_log.lock().unwrap();
        assert_eq!(downs.len(), 1);
        assert_eq!(
            downs[0],
            (watcher, target, 0, mid),
            "watcher should receive down signal"
        );
        assert!(
            kill_log.lock().unwrap().is_empty(),
            "monitor does not kill watcher"
        );
    }

    #[test]
    fn demonitor_prevents_down_signal() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let target = mvl_next_actor_id();
        let watcher = mvl_next_actor_id();
        register_mock_actor(
            target,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            watcher,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        let mid = mvl_monitor(watcher, target);
        mvl_demonitor(mid);
        mvl_process_exit(target, 0);

        assert!(
            down_log.lock().unwrap().is_empty(),
            "demonitored target should not send down signal"
        );
    }

    #[test]
    fn unlink_prevents_kill_cascade() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let a = mvl_next_actor_id();
        let b = mvl_next_actor_id();
        register_mock_actor(
            a,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            b,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        mvl_link(a, b);
        mvl_unlink(a, b);
        mvl_process_exit(a, 1);

        assert!(
            kill_log.lock().unwrap().is_empty(),
            "unlinked actor should not be killed"
        );
    }

    #[test]
    fn set_trap_exit_changes_behavior() {
        let kill_log = Arc::new(Mutex::new(Vec::new()));
        let exit_log = Arc::new(Mutex::new(Vec::new()));
        let down_log = Arc::new(Mutex::new(Vec::new()));

        let a = mvl_next_actor_id();
        let b = mvl_next_actor_id();
        register_mock_actor(
            a,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );
        register_mock_actor(
            b,
            false,
            Arc::clone(&kill_log),
            Arc::clone(&exit_log),
            Arc::clone(&down_log),
        );

        mvl_link(a, b);
        mvl_set_trap_exit(b); // dynamically enable trap

        mvl_process_exit(a, 1);

        assert!(
            kill_log.lock().unwrap().is_empty(),
            "dynamically trapping actor should NOT be killed"
        );
        let exits = exit_log.lock().unwrap();
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0], (b, a, 1));
    }
}
