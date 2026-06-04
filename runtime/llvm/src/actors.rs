// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime for the LLVM backend — work-stealing scheduler (#1226).
//!
//! Replaces the Phase 1 thread-per-actor model with a task-per-message
//! work-stealing scheduler:
//!
//! - N worker threads (one per hardware thread) share a global injector queue.
//! - Each actor is an `ActorCell` holding its mailbox, state, and scheduling flag.
//! - Sending a message schedules the actor on the injector (if not already queued).
//! - Workers pop or steal `Arc<ActorCell>` tasks, dispatch one message per slot,
//!   and re-schedule the actor if its mailbox still has messages.
//!
//! This enables ~100K actors with no OS-thread-per-actor overhead.
//!
//! # Message protocol (unchanged from Phase 1)
//!
//! - `disc` (discriminant): which public behavior to invoke (0-based index)
//! - `args`: packed `i64` array — all MVL scalar types map to `i64` at the C ABI
//!
//! # System discriminants (Phase 9, #1177)
//!
//! Negative discriminants are reserved for system messages:
//! - `-1` = Shutdown — actor drains remaining messages then exits
//! - `-2` = ExitSignal — linked actor died (args[0]=from_id, args[1]=reason)
//! - `-3` = DownSignal — monitored actor died (args[0]=from_id, args[1]=reason, args[2]=monitor_id)
//!
//! # Mailbox configuration (#1127)
//!
//! - `capacity > 0`: bounded (drop-newest when full)
//! - `capacity <= 0`: unbounded
//! - `policy = 1` (Block) degrades to DropNewest in Phase 2; backpressure is Phase 3
//!
//! # Shutdown protocol
//!
//! `mvl_actor_join_all()` is emitted at end of `fn main()` by the LLVM backend:
//! 1. Clears the link/monitor registry (releases all `Arc<ActorCell>` refs there)
//! 2. Sends `DISC_SHUTDOWN` to every live actor cell
//! 3. Spin-waits until every cell is idle (`scheduled = false`)
//! 4. Signals worker threads to exit and joins them
//!
//! Phase 8, #696. Shutdown: #1124. Mailbox config: #1127. Links: #1177.
//! Yield points: #1181. Work-stealing scheduler: #1226.

use std::cell::Cell;
use std::cell::UnsafeCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use crossbeam_deque::{Injector, Steal, Stealer, Worker};

// ── Constants ─────────────────────────────────────────────────────────────

/// Maximum number of `i64` arguments a single behavior call can carry.
const MAX_ARGS: usize = 8;

const DISC_SHUTDOWN: i64 = -1;
const DISC_EXIT_SIGNAL: i64 = -2;
const DISC_DOWN_SIGNAL: i64 = -3;

/// Erlang-default reduction budget per scheduling slot (#1181).
const REDUCTION_LIMIT: u64 = 4000;

// ── Message ────────────────────────────────────────────────────────────────

/// An in-flight actor message: behavior discriminant + packed i64 arguments.
struct MvlMsg {
    disc: i64,
    args: [i64; MAX_ARGS],
}

// ── C-ABI dispatch function ────────────────────────────────────────────────

/// ```c
/// void dispatch(void *state, int64_t disc, int64_t const *args);
/// ```
type DispatchFn = unsafe extern "C" fn(*mut u8, i64, *const i64);

// ── Actor cell ─────────────────────────────────────────────────────────────

/// Per-actor execution unit shared between the handle, link registry, and scheduler.
///
/// Only one worker processes an actor at a time — enforced by the `scheduled`
/// `AtomicBool` used as a single-owner token.  This makes accessing `state`
/// without a lock safe: the worker holding `scheduled = true` is the exclusive
/// reader/writer of the state bytes.
struct ActorCell {
    dispatch: DispatchFn,
    /// Actor state bytes.  Protected by the `scheduled` single-owner invariant:
    /// only the worker currently holding `scheduled = true` may touch this.
    state: UnsafeCell<Vec<u8>>,
    /// Pending messages (multiple producers → `Mutex`).
    mailbox: Mutex<VecDeque<MvlMsg>>,
    /// True while this cell is queued in a worker or the injector.
    scheduled: AtomicBool,
    actor_id: u64,
    /// Bounded mailbox capacity (0 = unbounded).  Messages are silently dropped
    /// when the mailbox is full (DropNewest policy; Block degrades here in Phase 2).
    capacity: usize,
    /// Raw pointer to the owning `MvlActorHandle` — for `mvl_actor_self()`.
    /// Cleared to 0 by `mvl_actor_drop` / `mvl_actor_join_all`.
    handle_ptr: AtomicUsize,
}

// Safety: Only one worker touches `state` at a time (scheduled AtomicBool token).
// `dispatch` is a C function pointer — always `Send + Sync`.
unsafe impl Send for ActorCell {}
unsafe impl Sync for ActorCell {}

impl ActorCell {
    /// Push a message respecting capacity.  Returns `false` if dropped.
    fn push_msg(&self, msg: MvlMsg) -> bool {
        let mut mb = self.mailbox.lock().unwrap_or_else(|p| p.into_inner());
        if self.capacity > 0 && mb.len() >= self.capacity {
            return false; // DropNewest
        }
        mb.push_back(msg);
        true
    }

    /// Schedule this cell on the global injector if not already queued.
    fn schedule(self: &Arc<Self>) {
        if !self.scheduled.swap(true, Ordering::AcqRel) {
            global_scheduler().injector.push(Arc::clone(self));
        }
    }
}

// ── Work-stealing scheduler ────────────────────────────────────────────────

/// Global N-thread work-stealing scheduler.
struct GlobalScheduler {
    injector: Injector<Arc<ActorCell>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    /// Set to `true` by `mvl_actor_join_all` to signal workers to exit.
    shutdown: AtomicBool,
}

static GLOBAL_SCHED: OnceLock<GlobalScheduler> = OnceLock::new();

fn global_scheduler() -> &'static GlobalScheduler {
    GLOBAL_SCHED.get_or_init(|| {
        let n = thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let local_workers: Vec<Worker<Arc<ActorCell>>> =
            (0..n).map(|_| Worker::new_fifo()).collect();
        let stealers: Vec<Stealer<Arc<ActorCell>>> =
            local_workers.iter().map(|w| w.stealer()).collect();

        let join_handles: Vec<JoinHandle<()>> = local_workers
            .into_iter()
            .map(|local| {
                let my_stealers = stealers.clone();
                thread::spawn(move || worker_loop(local, my_stealers))
            })
            .collect();

        GlobalScheduler {
            injector: Injector::new(),
            workers: Mutex::new(join_handles),
            shutdown: AtomicBool::new(false),
        }
    })
}

// ── Worker thread ──────────────────────────────────────────────────────────

fn worker_loop(local: Worker<Arc<ActorCell>>, stealers: Vec<Stealer<Arc<ActorCell>>>) {
    let sched = global_scheduler();
    loop {
        match pop_task(&local, sched, &stealers) {
            Some(cell) => process_one_message(cell, &local),
            None => {
                if sched.shutdown.load(Ordering::Acquire) {
                    break;
                }
                thread::yield_now();
            }
        }
    }
}

/// Try to obtain a task: local queue → injector (batch) → sibling steal (batch).
fn pop_task(
    local: &Worker<Arc<ActorCell>>,
    sched: &GlobalScheduler,
    stealers: &[Stealer<Arc<ActorCell>>],
) -> Option<Arc<ActorCell>> {
    // 1. Local queue (cheapest).
    if let Some(t) = local.pop() {
        return Some(t);
    }
    // 2. Global injector — batch-steal for cache efficiency.
    loop {
        match sched.injector.steal_batch_and_pop(local) {
            Steal::Success(t) => return Some(t),
            Steal::Empty => break,
            Steal::Retry => {}
        }
    }
    // 3. Steal from sibling workers.
    for stealer in stealers {
        loop {
            match stealer.steal_batch_and_pop(local) {
                Steal::Success(t) => return Some(t),
                Steal::Empty => break,
                Steal::Retry => {}
            }
        }
    }
    None
}

/// Dispatch one message from `cell`, then re-schedule or go idle.
fn process_one_message(cell: Arc<ActorCell>, local: &Worker<Arc<ActorCell>>) {
    // Pop one message (brief lock).
    let msg = cell
        .mailbox
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .pop_front();
    let msg = match msg {
        Some(m) => m,
        None => {
            // Spurious schedule: mailbox empty, release the scheduled token.
            cell.scheduled.store(false, Ordering::Release);
            return;
        }
    };

    // Set per-task thread-locals (for mvl_actor_self / mvl_yield_check).
    CURRENT_ACTOR_PTR.with(|c| c.set(cell.handle_ptr.load(Ordering::Relaxed)));
    CURRENT_ACTOR_ID.with(|c| c.set(cell.actor_id));
    ACTOR_REDUCTIONS.with(|c| c.set(REDUCTION_LIMIT));

    // ── System discriminants ──────────────────────────────────────────────

    if msg.disc == DISC_SHUTDOWN {
        // Drain mailbox, release token, notify links/monitors.
        cell.mailbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        cell.scheduled.store(false, Ordering::Release);
        process_actor_exit(cell.actor_id, 2); // Killed
        return;
    }

    // ExitSignal / DownSignal: silently discarded for now (#1177 TODO — LLVM
    // backend has no user-visible signal-handling mechanism yet).

    if msg.disc >= 0 {
        // ── User message: call the actor's dispatch function ──────────────
        // Safety: `state` is only accessed here, and only one worker holds the
        // `scheduled = true` token, so there is no concurrent access.
        let state_ptr = unsafe { (*cell.state.get()).as_mut_ptr() };
        let result = catch_unwind(AssertUnwindSafe(|| {
            unsafe { (cell.dispatch)(state_ptr, msg.disc, msg.args.as_ptr()) };
        }));
        if result.is_err() {
            // Actor panicked: drain and exit.
            cell.mailbox
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear();
            cell.scheduled.store(false, Ordering::Release);
            process_actor_exit(cell.actor_id, 1); // Panic
            return;
        }
    }

    // ── Re-schedule or go idle ────────────────────────────────────────────

    let has_more = !cell
        .mailbox
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty();
    if has_more {
        // Stay on the local queue for immediate re-processing.
        local.push(cell);
    } else {
        // Release the scheduled token.
        cell.scheduled.store(false, Ordering::Release);
        // Guard against the producer-race window: a producer may have pushed a
        // message between our `is_empty` check and `scheduled.store(false)`.
        // If they saw `scheduled = false` they did NOT schedule the cell, so we
        // must re-schedule it ourselves.
        if !cell
            .mailbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
            && !cell.scheduled.swap(true, Ordering::AcqRel)
        {
            local.push(cell);
        }
    }
}

// ── Link/monitor registry (Phase 9, #1177) ───────────────────────────────

type ActorId = u64;
type MonitorId = u64;

static NEXT_ACTOR_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MONITOR_ID: AtomicU64 = AtomicU64::new(1);

struct ActorEntry {
    /// Cell reference for injecting system messages without going through a handle.
    cell: Arc<ActorCell>,
    traps_exit: bool,
}

struct LinkRegistry {
    actors: HashMap<ActorId, ActorEntry>,
    links: HashMap<ActorId, HashSet<ActorId>>,
    monitors: HashMap<ActorId, Vec<(MonitorId, ActorId)>>,
    monitor_targets: HashMap<MonitorId, ActorId>,
}

impl LinkRegistry {
    fn new() -> Self {
        Self {
            actors: HashMap::new(),
            links: HashMap::new(),
            monitors: HashMap::new(),
            monitor_targets: HashMap::new(),
        }
    }
}

fn global_link_registry() -> &'static Mutex<LinkRegistry> {
    static REGISTRY: OnceLock<Mutex<LinkRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(LinkRegistry::new()))
}

/// Process an actor's death: notify linked actors and monitors.
///
/// Collects target cells under the lock, then injects messages outside the lock
/// to avoid deadlock with bounded-mailbox actors.
fn process_actor_exit(dead_id: ActorId, reason: i64) {
    // Phase 1: collect notification targets under the lock.
    let (kill_cells, exit_cells, down_cells) = {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let linked = reg.links.remove(&dead_id).unwrap_or_default();
        for &peer in &linked {
            if let Some(set) = reg.links.get_mut(&peer) {
                set.remove(&dead_id);
            }
        }

        let mut kill_cells: Vec<Arc<ActorCell>> = Vec::new();
        let mut exit_cells: Vec<(Arc<ActorCell>, i64)> = Vec::new();
        for peer in linked {
            if let Some(entry) = reg.actors.get(&peer) {
                if entry.traps_exit {
                    exit_cells.push((Arc::clone(&entry.cell), reason));
                } else {
                    kill_cells.push(Arc::clone(&entry.cell));
                }
            }
        }

        let monitored = reg.monitors.remove(&dead_id).unwrap_or_default();
        let mut down_cells: Vec<(Arc<ActorCell>, MonitorId)> = Vec::new();
        for (mid, watcher) in monitored {
            reg.monitor_targets.remove(&mid);
            if let Some(entry) = reg.actors.get(&watcher) {
                down_cells.push((Arc::clone(&entry.cell), mid));
            }
        }

        reg.actors.remove(&dead_id);
        (kill_cells, exit_cells, down_cells)
    };

    // Phase 2: inject messages outside the lock.
    for cell in kill_cells {
        if cell.push_msg(MvlMsg {
            disc: DISC_SHUTDOWN,
            args: [0; MAX_ARGS],
        }) {
            cell.schedule();
        }
    }
    for (cell, exit_reason) in exit_cells {
        let mut args = [0i64; MAX_ARGS];
        args[0] = dead_id as i64;
        args[1] = exit_reason;
        if cell.push_msg(MvlMsg {
            disc: DISC_EXIT_SIGNAL,
            args,
        }) {
            cell.schedule();
        }
    }
    for (cell, mid) in down_cells {
        let mut args = [0i64; MAX_ARGS];
        args[0] = dead_id as i64;
        args[1] = reason;
        args[2] = mid as i64;
        if cell.push_msg(MvlMsg {
            disc: DISC_DOWN_SIGNAL,
            args,
        }) {
            cell.schedule();
        }
    }
}

// ── Global actor registry ─────────────────────────────────────────────────

/// Tracks every spawned actor: `(Option<handle_raw_ptr>, Arc<ActorCell>)`.
/// `Option` is `None` when the handle has been explicitly dropped via `mvl_actor_drop`.
fn global_actor_registry() -> &'static Mutex<Vec<(Option<usize>, Arc<ActorCell>)>> {
    static REGISTRY: OnceLock<Mutex<Vec<(Option<usize>, Arc<ActorCell>)>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

// ── Thread-local context ──────────────────────────────────────────────────

thread_local! {
    /// Raw pointer to the current task's `MvlActorHandle` (for `mvl_actor_self()`).
    /// Set by the worker before each dispatch; 0 outside actor context.
    static CURRENT_ACTOR_PTR: Cell<usize> = const { Cell::new(0) };
    static CURRENT_ACTOR_ID: Cell<u64> = const { Cell::new(0) };
    /// Remaining reductions for cooperative scheduling (#1181).
    static ACTOR_REDUCTIONS: Cell<u64> = const { Cell::new(REDUCTION_LIMIT) };
}

// ── Public handle type ────────────────────────────────────────────────────

/// Opaque actor handle allocated on the heap.
///
/// The LLVM backend stores this as a `ptr` (opaque pointer) in LLVM IR.
/// Only pass to [`mvl_actor_send`], [`mvl_actor_drop`], [`mvl_actor_get_id`],
/// [`mvl_link`], [`mvl_unlink`], [`mvl_monitor`], [`mvl_set_trap_exit`].
pub struct MvlActorHandle {
    cell: Arc<ActorCell>,
    actor_id: u64,
}

// ── Spawn ──────────────────────────────────────────────────────────────────

/// Spawn a new actor and return an opaque handle.
///
/// # Parameters
///
/// - `dispatch`: behavior dispatch function (generated by the MVL LLVM backend)
/// - `state_ptr`: pointer to the initial actor state struct
/// - `state_size`: byte size of the state struct (deep-copied)
/// - `capacity`: mailbox bound — `> 0` for bounded, `<= 0` for unbounded (#1127)
/// - `policy`: overflow policy — `0` = DropNewest, `1` = Block (degrades to
///   DropNewest in Phase 2; backpressure scheduling is Phase 3)
///
/// # Safety
///
/// `state_ptr` must point to at least `state_size` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_spawn(
    dispatch: DispatchFn,
    state_ptr: *mut u8,
    state_size: i64,
    capacity: i64,
    policy: i64,
) -> *mut u8 {
    if state_size < 0 || state_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let size = state_size as usize;
    let state: Vec<u8> = std::slice::from_raw_parts(state_ptr, size).to_vec();
    let actor_id = NEXT_ACTOR_ID.fetch_add(1, Ordering::Relaxed);
    let cap = if capacity > 0 { capacity as usize } else { 0 };
    let _ = policy; // Block degrades to DropNewest in Phase 2

    let cell = Arc::new(ActorCell {
        dispatch,
        state: UnsafeCell::new(state),
        mailbox: Mutex::new(VecDeque::new()),
        scheduled: AtomicBool::new(false),
        actor_id,
        capacity: cap,
        handle_ptr: AtomicUsize::new(0), // filled after handle is allocated below
    });

    let handle = Box::new(MvlActorHandle {
        cell: Arc::clone(&cell),
        actor_id,
    });
    let handle_ptr = Box::into_raw(handle);

    // Store the handle's address back into the cell for mvl_actor_self().
    cell.handle_ptr
        .store(handle_ptr as usize, Ordering::Release);

    // Register in link/monitor registry.
    {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reg.actors.insert(
            actor_id,
            ActorEntry {
                cell: Arc::clone(&cell),
                traps_exit: false,
            },
        );
    }

    // Register in actor registry.
    global_actor_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((Some(handle_ptr as usize), cell));

    // Ensure the scheduler (and its worker threads) is running.
    let _ = global_scheduler();

    handle_ptr as *mut u8
}

// ── Send ───────────────────────────────────────────────────────────────────

/// Send a behavior message to an actor (fire-and-forget).
///
/// # Safety
///
/// `handle` must be a valid pointer returned by [`mvl_actor_spawn`] and not yet
/// dropped. `args` must point to at least `argc` valid `i64` values.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_send(handle: *mut u8, disc: i64, argc: i64, args: *const i64) {
    if handle.is_null() {
        return;
    }
    let actor = &*(handle as *const MvlActorHandle);
    let argc = (argc as usize).min(MAX_ARGS);
    let mut msg = MvlMsg {
        disc,
        args: [0i64; MAX_ARGS],
    };
    if argc > 0 {
        let src = std::slice::from_raw_parts(args, argc);
        msg.args[..argc].copy_from_slice(src);
    }
    if actor.cell.push_msg(msg) {
        actor.cell.schedule();
    }
}

// ── Self / ID ──────────────────────────────────────────────────────────────

/// Return the current actor's own handle pointer.
///
/// Must only be called from within a behavior dispatch.
/// Returns null if called from a non-actor context.
#[no_mangle]
pub extern "C" fn mvl_actor_self() -> *mut u8 {
    CURRENT_ACTOR_PTR.with(|c| c.get() as *mut u8)
}

/// Get the actor ID from a handle.  Returns 0 for a null handle.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_get_id(handle: *mut u8) -> i64 {
    if handle.is_null() {
        return 0;
    }
    let actor = &*(handle as *const MvlActorHandle);
    actor.actor_id as i64
}

// ── Drop ───────────────────────────────────────────────────────────────────

/// Drop an actor handle.
///
/// Clears the back-pointer in the cell (so `mvl_actor_self()` returns null
/// from within this actor) and marks the registry entry as consumed so that
/// `mvl_actor_join_all` does not double-free it.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by [`mvl_actor_spawn`] and must
/// not be used after this call.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_drop(handle: *mut u8) {
    if handle.is_null() {
        return;
    }
    {
        let mut reg = global_actor_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = reg.iter_mut().find(|(p, _)| *p == Some(handle as usize)) {
            entry.0 = None;
        }
    }
    let actor = &*(handle as *const MvlActorHandle);
    actor.cell.handle_ptr.store(0, Ordering::Release);
    drop(Box::from_raw(handle as *mut MvlActorHandle));
}

// ── Join all ───────────────────────────────────────────────────────────────

/// Drain and shut down all actors, then join worker threads.
///
/// Emitted at the end of `fn main()` by the LLVM backend (#1124).
///
/// 1. Clear the link/monitor registry — drops `Arc<ActorCell>` refs held there.
/// 2. Drain the actor registry — send `DISC_SHUTDOWN` to every live cell.
/// 3. Spin-wait until every cell is idle (`scheduled = false`).
/// 4. Set `shutdown = true` and join all worker threads.
#[no_mangle]
pub extern "C" fn mvl_actor_join_all() {
    // 1. Clear link/monitor registry.
    {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reg.actors.clear();
        reg.links.clear();
        reg.monitors.clear();
        reg.monitor_targets.clear();
    }

    // 2. Drain actor registry; collect all cells and drop live handles.
    let cells: Vec<Arc<ActorCell>> = global_actor_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain(..)
        .map(|(ptr_opt, cell)| {
            if let Some(ptr) = ptr_opt {
                cell.handle_ptr.store(0, Ordering::Release);
                unsafe { drop(Box::from_raw(ptr as *mut MvlActorHandle)) };
            }
            cell
        })
        .collect();

    // 3. Push DISC_SHUTDOWN to every live actor cell.
    for cell in &cells {
        cell.mailbox
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(MvlMsg {
                disc: DISC_SHUTDOWN,
                args: [0; MAX_ARGS],
            });
        cell.schedule();
    }

    // 4. Spin-wait until all cells are idle (DISC_SHUTDOWN has been processed).
    loop {
        let all_idle = cells.iter().all(|c| !c.scheduled.load(Ordering::Acquire));
        if all_idle {
            break;
        }
        thread::yield_now();
    }

    // 5. Signal workers to stop, then join them.
    if let Some(sched) = GLOBAL_SCHED.get() {
        sched.shutdown.store(true, Ordering::Release);
        let mut handles = sched.workers.lock().unwrap_or_else(|p| p.into_inner());
        for jh in handles.drain(..) {
            let _ = jh.join();
        }
    }
}

// ── Reduction-based cooperative yield (#1181) ─────────────────────────────

/// Cooperative yield check — inserted at loop back-edges by the LLVM compiler.
///
/// Decrements the per-task reduction budget.  When exhausted, resets to
/// `REDUCTION_LIMIT`.  The work-stealing scheduler takes over naturally: when
/// the worker finishes dispatching this message it pops the next ready actor.
#[no_mangle]
pub extern "C" fn mvl_yield_check() {
    ACTOR_REDUCTIONS.with(|cell| {
        let remaining = cell.get();
        if remaining <= 1 {
            cell.set(REDUCTION_LIMIT);
        } else {
            cell.set(remaining - 1);
        }
    });
}

// ── Link/monitor C-ABI functions (Phase 9, #1177) ────────────────────────

/// Create a bidirectional link between two actors (#1177).
///
/// # Safety
///
/// Both handles must be valid pointers returned by [`mvl_actor_spawn`].
#[no_mangle]
pub unsafe extern "C" fn mvl_link(handle_a: *mut u8, handle_b: *mut u8) {
    if handle_a.is_null() || handle_b.is_null() {
        return;
    }
    let a = &*(handle_a as *const MvlActorHandle);
    let b = &*(handle_b as *const MvlActorHandle);
    let mut reg = global_link_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reg.links.entry(a.actor_id).or_default().insert(b.actor_id);
    reg.links.entry(b.actor_id).or_default().insert(a.actor_id);
}

/// Remove a bidirectional link between two actors.
///
/// # Safety
///
/// Both handles must be valid pointers returned by [`mvl_actor_spawn`].
#[no_mangle]
pub unsafe extern "C" fn mvl_unlink(handle_a: *mut u8, handle_b: *mut u8) {
    if handle_a.is_null() || handle_b.is_null() {
        return;
    }
    let a = &*(handle_a as *const MvlActorHandle);
    let b = &*(handle_b as *const MvlActorHandle);
    let mut reg = global_link_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(set) = reg.links.get_mut(&a.actor_id) {
        set.remove(&b.actor_id);
    }
    if let Some(set) = reg.links.get_mut(&b.actor_id) {
        set.remove(&a.actor_id);
    }
}

/// Create a one-way monitor: `watcher` is notified when `target` dies.
///
/// Returns a monitor ID for use with [`mvl_demonitor`].
///
/// # Safety
///
/// Both handles must be valid pointers returned by [`mvl_actor_spawn`].
#[no_mangle]
pub unsafe extern "C" fn mvl_monitor(watcher: *mut u8, target: *mut u8) -> i64 {
    if watcher.is_null() || target.is_null() {
        return 0;
    }
    let w = &*(watcher as *const MvlActorHandle);
    let t = &*(target as *const MvlActorHandle);
    let mid = NEXT_MONITOR_ID.fetch_add(1, Ordering::Relaxed);
    let mut reg = global_link_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    reg.monitors
        .entry(t.actor_id)
        .or_default()
        .push((mid, w.actor_id));
    reg.monitor_targets.insert(mid, t.actor_id);
    mid as i64
}

/// Remove a monitor by ID.
#[no_mangle]
pub extern "C" fn mvl_demonitor(monitor_id: i64) {
    let mid = monitor_id as u64;
    let mut reg = global_link_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(target) = reg.monitor_targets.remove(&mid) {
        if let Some(list) = reg.monitors.get_mut(&target) {
            list.retain(|(m, _)| *m != mid);
        }
    }
}

/// Enable exit-signal trapping for an actor.
///
/// When a linked actor dies, this actor receives an `ExitSignal` message
/// instead of being killed.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by [`mvl_actor_spawn`].
#[no_mangle]
pub unsafe extern "C" fn mvl_set_trap_exit(handle: *mut u8) {
    if handle.is_null() {
        return;
    }
    let actor = &*(handle as *const MvlActorHandle);
    let mut reg = global_link_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(entry) = reg.actors.get_mut(&actor.actor_id) {
        entry.traps_exit = true;
    }
}
