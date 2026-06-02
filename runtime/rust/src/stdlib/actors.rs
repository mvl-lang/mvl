// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Bridge functions for `std.actors` builtins.
//!
//! Called from MVL programs that `use std.actors.{link, unlink, monitor, demonitor}`.
//! Wraps the runtime-internal `mvl_link` / `mvl_monitor` API (Phase 9, #1128).

use crate::actors::{mvl_demonitor, mvl_link, mvl_monitor, mvl_unlink, ActorId, MonitorId};

/// Create a bidirectional link between two actors by ID.
///
/// If either actor dies, the other is killed — unless it has `traps_exit`.
pub fn link(a: i64, b: i64) {
    debug_assert!(a > 0 && b > 0, "actor IDs must be positive (got {a}, {b})");
    mvl_link(a as ActorId, b as ActorId);
}

/// Remove a bidirectional link between two actors.
pub fn unlink(a: i64, b: i64) {
    debug_assert!(a > 0 && b > 0, "actor IDs must be positive (got {a}, {b})");
    mvl_unlink(a as ActorId, b as ActorId);
}

/// Create a one-way monitor: `watcher` is notified when `target` dies.
///
/// Returns a monitor reference (opaque `Int`) that can be passed to `demonitor`.
pub fn monitor(watcher: i64, target: i64) -> i64 {
    debug_assert!(
        watcher > 0 && target > 0,
        "actor IDs must be positive (got {watcher}, {target})"
    );
    let id = mvl_monitor(watcher as ActorId, target as ActorId);
    i64::try_from(id).expect("monitor ID overflowed i64")
}

/// Remove a previously created monitor.
pub fn demonitor(monitor_ref: i64) {
    debug_assert!(
        monitor_ref > 0,
        "monitor ref must be positive (got {monitor_ref})"
    );
    mvl_demonitor(monitor_ref as MonitorId);
}
