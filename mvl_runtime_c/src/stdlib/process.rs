//! C-ABI exports for `std.process` — mirrors `mvl_runtime::stdlib::process` (#432).
//!
//! # Status
//!
//! Scaffolded. Requires `mvl_runtime::stdlib::process` from #414 to land before
//! the wrappers can be filled in.
//!
//! # Planned exports
//!
//! | C symbol                      | MVL stdlib fn              | Return type   |
//! |-------------------------------|----------------------------|---------------|
//! | `_mvl_process_command_new`    | `process.command(prog)`    | `*mut Handle` |
//! | `_mvl_process_command_arg`    | `cmd.arg(a)`               | `()`          |
//! | `_mvl_process_command_spawn`  | `cmd.spawn()`              | `MvlResult`   |
//! | `_mvl_process_handle_wait`    | `handle.wait()`            | `MvlResult`   |
//! | `_mvl_process_handle_output`  | `handle.output()`          | `MvlResult`   |
//! | `_mvl_process_handle_kill`    | `handle.kill()`            | `MvlResult`   |

// TODO(#432): implement once mvl_runtime::stdlib::process is merged from #414.
