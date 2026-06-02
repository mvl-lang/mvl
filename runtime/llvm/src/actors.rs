// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Actor runtime for the LLVM backend.
//!
//! Provides C-ABI functions for actor lifecycle: spawn, send, drop, join_all.
//!
//! Each actor is a std::thread running a message-dispatch loop. The actor state
//! is a heap-allocated raw byte array; the dispatch function is a function pointer
//! with the signature `fn(state_ptr: *mut u8, disc: i64, args: *const i64)`.
//!
//! # Message protocol
//!
//! - `disc` (discriminant): which public behavior to invoke (0-based index in
//!   declaration order)
//! - `args`: array of `i64` values, one per behavior parameter (all MVL scalar
//!   types map to `i64` at the LLVM ABI level; pointer types pass as pointer-width
//!   integers)
//!
//! # System discriminants (Phase 9, #1177)
//!
//! Negative discriminants are reserved for system messages:
//! - `-1` = Shutdown — actor thread exits the dispatch loop
//! - `-2` = ExitSignal — linked actor died (args[0]=from_id, args[1]=reason)
//! - `-3` = DownSignal — monitored actor died (args[0]=from_id, args[1]=reason, args[2]=monitor_id)
//!
//! These are handled in the dispatch loop before calling the user dispatch function.
//!
//! # Mailbox configuration (#1127)
//!
//! `mvl_actor_spawn` accepts `capacity` and `policy` parameters:
//! - `capacity > 0`: bounded sync_channel(capacity)
//! - `capacity <= 0`: unbounded channel (grows without limit)
//! - `policy = 0`: DropNewest — `try_send`, drop new message when full
//! - `policy = 1`: Block — `send`, block the sender until space is available
//!
//! # Shutdown protocol (#1124)
//!
//! `mvl_actor_join_all()` is emitted at the end of `fn main()` by the LLVM
//! backend. It:
//! 1. Drains the global actor registry (all spawned handles + join handles)
//! 2. Drops each live `MvlActorHandle` — closing the sender
//! 3. Joins all actor threads (which drain buffered messages then exit)
//!
//! `mvl_actor_drop` marks the handle as consumed in the registry (setting its
//! slot to `None`) to prevent double-free when `mvl_actor_join_all` runs later.
//!
//! Phase 8, #696. Shutdown fix: #1124. Mailbox config: #1127. Links: #1177.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

/// Maximum number of `i64` arguments a single behavior call can carry.
/// Behaviors with more parameters than this are not yet supported.
const MAX_ARGS: usize = 8;

/// System discriminant: shut down the actor.
const DISC_SHUTDOWN: i64 = -1;
/// System discriminant: exit signal from a linked actor.
const DISC_EXIT_SIGNAL: i64 = -2;
/// System discriminant: down signal from a monitored actor.
const DISC_DOWN_SIGNAL: i64 = -3;

/// An in-flight actor message: behavior discriminant + packed i64 arguments.
struct MvlMsg {
    disc: i64,
    args: [i64; MAX_ARGS],
}

/// Typed mailbox sender — captures both channel type and overflow policy (#1127).
enum MvlSender {
    /// Bounded channel, drop-newest policy: `try_send` (fire-and-forget).
    BoundedDrop(mpsc::SyncSender<MvlMsg>),
    /// Bounded channel, blocking policy: `send` (blocks sender when full).
    BoundedBlock(mpsc::SyncSender<MvlMsg>),
    /// Unbounded channel: `send` never blocks (grows without limit).
    Unbounded(mpsc::Sender<MvlMsg>),
}

impl Clone for MvlSender {
    fn clone(&self) -> Self {
        match self {
            MvlSender::BoundedDrop(tx) => MvlSender::BoundedDrop(tx.clone()),
            MvlSender::BoundedBlock(tx) => MvlSender::BoundedBlock(tx.clone()),
            MvlSender::Unbounded(tx) => MvlSender::Unbounded(tx.clone()),
        }
    }
}

impl MvlSender {
    fn send_msg(&self, msg: MvlMsg) {
        match self {
            MvlSender::BoundedDrop(tx) => {
                let _ = tx.try_send(msg);
            }
            MvlSender::BoundedBlock(tx) => {
                let _ = tx.send(msg);
            }
            MvlSender::Unbounded(tx) => {
                let _ = tx.send(msg);
            }
        }
    }
}

/// Opaque actor handle allocated on the heap.
///
/// The LLVM backend stores this as a `ptr` (opaque pointer) in LLVM IR.
/// Callers must treat the returned pointer as opaque and only pass it to
/// [`mvl_actor_send`] and [`mvl_actor_drop`].
pub struct MvlActorHandle {
    sender: MvlSender,
    actor_id: u64,
}

/// C-ABI dispatch function pointer type.
///
/// ```c
/// void dispatch(void *state, int64_t disc, int64_t const *args);
/// ```
type DispatchFn = unsafe extern "C" fn(*mut u8, i64, *const i64);

// Global registry of spawned actors.
//
// Each entry is `(Option<handle_ptr as usize>, JoinHandle<()>)`.
// The `Option` is set to `None` when the handle has been explicitly freed
// via `mvl_actor_drop`, preventing double-free in `mvl_actor_join_all`.
//
// Uses `thread_local!` because LLVM-generated programs spawn actors only from
// the main thread. `mvl_actor_join_all` is also called from the main thread,
// so both access the same TLS slot.
thread_local! {
    static ACTOR_REGISTRY: RefCell<Vec<(Option<usize>, JoinHandle<()>)>> =
        const { RefCell::new(Vec::new()) };
}

// ── Link/monitor registry (Phase 9, #1177) ───────────────────────────────

/// Unique actor identifier, assigned at spawn time.
type ActorId = u64;
/// Unique monitor identifier.
type MonitorId = u64;

static NEXT_ACTOR_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MONITOR_ID: AtomicU64 = AtomicU64::new(1);

struct ActorEntry {
    /// Cloned sender for injecting system messages.
    sender: MvlSender,
    /// Whether this actor traps exit signals.
    traps_exit: bool,
}

struct LinkRegistry {
    actors: HashMap<ActorId, ActorEntry>,
    /// Bidirectional links.
    links: HashMap<ActorId, HashSet<ActorId>>,
    /// Monitors: target → list of (monitor_id, watcher_id).
    monitors: HashMap<ActorId, Vec<(MonitorId, ActorId)>>,
    /// Reverse index: monitor_id → target.
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
/// Collects all notification targets and clones their senders under the lock,
/// then sends all messages outside the lock to avoid deadlock with `BoundedBlock`
/// senders that may block when the target's mailbox is full.
fn process_actor_exit(dead_id: ActorId, reason: i64) {
    // Phase 1: collect targets and clone their senders under the lock.
    let (kill_senders, exit_senders, down_senders) = {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let linked = reg.links.remove(&dead_id).unwrap_or_default();
        for &peer in &linked {
            if let Some(set) = reg.links.get_mut(&peer) {
                set.remove(&dead_id);
            }
        }

        let mut kill_senders: Vec<MvlSender> = Vec::new();
        let mut exit_senders: Vec<(MvlSender, i64)> = Vec::new();
        for peer in linked {
            if let Some(entry) = reg.actors.get(&peer) {
                if entry.traps_exit {
                    exit_senders.push((entry.sender.clone(), reason));
                } else {
                    kill_senders.push(entry.sender.clone());
                }
            }
        }

        let monitored = reg.monitors.remove(&dead_id).unwrap_or_default();
        let mut down_senders: Vec<(MvlSender, MonitorId)> = Vec::new();
        for (mid, watcher) in monitored {
            reg.monitor_targets.remove(&mid);
            if let Some(entry) = reg.actors.get(&watcher) {
                down_senders.push((entry.sender.clone(), mid));
            }
        }

        reg.actors.remove(&dead_id);

        (kill_senders, exit_senders, down_senders)
    };

    // Phase 2: send all messages outside the lock — avoids deadlock with
    // BoundedBlock senders that may block when the target's mailbox is full.
    for sender in kill_senders {
        sender.send_msg(MvlMsg {
            disc: DISC_SHUTDOWN,
            args: [0i64; MAX_ARGS],
        });
    }

    for (sender, exit_reason) in exit_senders {
        let mut args = [0i64; MAX_ARGS];
        args[0] = dead_id as i64;
        args[1] = exit_reason;
        sender.send_msg(MvlMsg {
            disc: DISC_EXIT_SIGNAL,
            args,
        });
    }

    for (sender, mid) in down_senders {
        let mut args = [0i64; MAX_ARGS];
        args[0] = dead_id as i64;
        args[1] = reason;
        args[2] = mid as i64;
        sender.send_msg(MvlMsg {
            disc: DISC_DOWN_SIGNAL,
            args,
        });
    }
}

// ── Spawn ──────────────────────────────────────────────────────────────────

/// Spawn a new actor thread and return an opaque actor handle.
///
/// # Parameters
///
/// - `dispatch`: the actor's behavior dispatch function
///   (generated by the MVL LLVM backend from `actor` declarations)
/// - `state_ptr`: pointer to the initial actor state struct
/// - `state_size`: byte size of the state struct (used to heap-copy the state)
/// - `capacity`: mailbox capacity — `> 0` for bounded, `<= 0` for unbounded (#1127)
/// - `policy`: overflow policy — `0` = DropNewest (`try_send`), `1` = Block (`send`) (#1127)
///
/// # Returns
///
/// An opaque `*mut MvlActorHandle`, or null on spawn failure.
/// The caller owns the returned pointer and must eventually call [`mvl_actor_drop`]
/// or [`mvl_actor_join_all`] to release it.
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
    // Guard: reject negative sizes and null state pointer.
    if state_size < 0 || state_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let size = state_size as usize;
    // Deep-copy the initial state so the actor thread fully owns it.
    let mut state: Vec<u8> = std::slice::from_raw_parts(state_ptr, size).to_vec();

    // Build the typed sender based on capacity and policy (#1127).
    let (sender, rx): (MvlSender, _) = if capacity > 0 {
        let cap = capacity as usize;
        let (tx, rx) = mpsc::sync_channel::<MvlMsg>(cap);
        let sender = if policy == 1 {
            MvlSender::BoundedBlock(tx)
        } else {
            MvlSender::BoundedDrop(tx)
        };
        (sender, rx)
    } else {
        let (tx, rx) = mpsc::channel::<MvlMsg>();
        (MvlSender::Unbounded(tx), rx)
    };

    // Assign unique actor ID (#1177).
    let actor_id = NEXT_ACTOR_ID.fetch_add(1, Ordering::Relaxed);

    // Register in link/monitor registry.
    {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reg.actors.insert(
            actor_id,
            ActorEntry {
                sender: sender.clone(),
                traps_exit: false,
            },
        );
    }

    let handle = Box::new(MvlActorHandle { sender, actor_id });
    let handle_ptr = Box::into_raw(handle);

    let handle_addr = handle_ptr as usize;
    let join_handle = thread::spawn(move || {
        CURRENT_ACTOR_HANDLE.with(|cell| cell.set(handle_addr));
        CURRENT_ACTOR_ID.with(|cell| cell.set(actor_id));
        let state_ptr = state.as_mut_ptr();

        let result = catch_unwind(AssertUnwindSafe(|| {
            while let Ok(msg) = rx.recv() {
                // System discriminants are handled here, not in user dispatch.
                if msg.disc == DISC_SHUTDOWN {
                    return false; // shutdown requested
                }
                // TODO(#1177): ExitSignal and DownSignal are silently discarded
                // here regardless of the `traps_exit` flag. The LLVM backend does
                // not yet have a mechanism for user code to handle these signals
                // (unlike the Rust backend which has typed enum variants). The
                // actor survives, but the signal data (from_id, reason) is lost.
                if msg.disc < 0 {
                    continue;
                }
                dispatch(state_ptr, msg.disc, msg.args.as_ptr());
            }
            true // normal exit
        }));

        let reason: i64 = match result {
            Ok(true) => 0,  // Normal
            Ok(false) => 2, // Killed
            Err(_) => 1,    // Panic
        };
        process_actor_exit(actor_id, reason);
    });

    // Register (handle_ptr, JoinHandle) so mvl_actor_join_all can drain and join.
    ACTOR_REGISTRY.with(|r| {
        r.borrow_mut()
            .push((Some(handle_ptr as usize), join_handle));
    });

    handle_ptr as *mut u8
}

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
    actor.sender.send_msg(msg);
}

// Thread-local that each actor thread sets to its own handle before processing messages.
thread_local! {
    static CURRENT_ACTOR_HANDLE: Cell<usize> = const { Cell::new(0) };
    static CURRENT_ACTOR_ID: Cell<u64> = const { Cell::new(0) };
}

/// Return the current actor's own handle (for passing `self` to other actors).
///
/// Must only be called from within a behavior dispatch. Returns null if called
/// from a non-actor thread.
#[no_mangle]
pub extern "C" fn mvl_actor_self() -> *mut u8 {
    CURRENT_ACTOR_HANDLE.with(|cell| cell.get() as *mut u8)
}

/// Get the actor ID from an actor handle.
///
/// Returns 0 if the handle is null.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_get_id(handle: *mut u8) -> i64 {
    if handle.is_null() {
        return 0;
    }
    let actor = &*(handle as *const MvlActorHandle);
    actor.actor_id as i64
}

/// Drop an actor handle, disconnecting the sender.
///
/// After this call the actor thread will drain any remaining messages and then exit.
/// The handle is marked as consumed in the global registry so that
/// [`mvl_actor_join_all`] does not double-free it.
///
/// # Safety
///
/// `handle` must be a valid pointer returned by [`mvl_actor_spawn`] and must not
/// be used after this call.
#[no_mangle]
pub unsafe extern "C" fn mvl_actor_drop(handle: *mut u8) {
    if handle.is_null() {
        return;
    }
    // Mark the registry entry as consumed so mvl_actor_join_all skips it.
    ACTOR_REGISTRY.with(|r| {
        let mut reg = r.borrow_mut();
        if let Some(entry) = reg.iter_mut().find(|(p, _)| *p == Some(handle as usize)) {
            entry.0 = None;
        }
    });
    drop(Box::from_raw(handle as *mut MvlActorHandle));
}

/// Join all spawned actor threads, waiting for them to drain and exit.
///
/// Emitted at the end of `fn main()` by the LLVM backend (#1124).
///
/// For each registered actor handle that has not been explicitly dropped:
/// 1. Drops the `MvlActorHandle`, closing the `SyncSender`
/// 2. The actor thread drains any buffered messages and exits
///
/// After dropping all handles, joins all actor threads.
///
/// The link/monitor registry (#1177) holds cloned senders for each actor
/// (kill/exit/down notification entries). These must be dropped before
/// joining, otherwise the channels remain open and actor threads block
/// forever on `recv()`.
#[no_mangle]
pub extern "C" fn mvl_actor_join_all() {
    // Clear all actor entries from the link registry — this drops the
    // cloned senders held by ActorEntry structs, allowing channels to
    // close so actor threads can exit.
    {
        let mut reg = global_link_registry()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reg.actors.clear();
        reg.links.clear();
        reg.monitors.clear();
        reg.monitor_targets.clear();
    }
    let entries = ACTOR_REGISTRY.with(|r| r.borrow_mut().drain(..).collect::<Vec<_>>());
    // Drop all live handles first — closes senders so actor threads will exit.
    for (ptr_opt, _) in &entries {
        if let Some(ptr) = ptr_opt {
            unsafe {
                drop(Box::from_raw(*ptr as *mut MvlActorHandle));
            }
        }
    }
    // Join all actor threads.
    for (_, jh) in entries {
        let _ = jh.join();
    }
}

// ── Link/monitor C-ABI functions (Phase 9, #1177) ────────────────────────

/// Create a bidirectional link between two actors.
///
/// If either actor dies, the other is killed (or receives an exit signal
/// if it has `traps_exit` set).
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
/// Returns a monitor ID that can be passed to [`mvl_demonitor`].
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

/// Remove a monitor.
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

/// Set the `traps_exit` flag for an actor.
///
/// When a linked actor dies, this actor receives an exit signal message
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
