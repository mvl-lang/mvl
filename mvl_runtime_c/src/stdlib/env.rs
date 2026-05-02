//! C-ABI exports for `std.env` — mirrors `mvl_runtime::stdlib::env` (#432).
//!
//! # Status
//!
//! Scaffolded. Requires `mvl_runtime::stdlib::env` from #414 to land before
//! the wrappers can be filled in.  The function signatures below define the
//! C-ABI contract; implementations are marked TODO.
//!
//! # Planned exports
//!
//! | C symbol               | MVL stdlib fn         | Return type       |
//! |------------------------|-----------------------|-------------------|
//! | `_mvl_env_get`         | `env.get(key)`        | `MvlOption`       |
//! | `_mvl_env_set_var`     | `env.set_var(k, v)`   | `()`              |
//! | `_mvl_env_remove_var`  | `env.remove_var(key)` | `()`              |
//! | `_mvl_env_vars`        | `env.vars()`          | `MvlArray*`       |
//! | `_mvl_env_args`        | `env.args()`          | `MvlArray*`       |
//! | `_mvl_env_current_dir` | `env.current_dir()`   | `MvlOption`       |
//! | `_mvl_env_home_dir`    | `env.home_dir()`      | `MvlOption`       |
//! | `_mvl_process_id`      | `env.process_id()`    | `i64`             |
//!
//! All pointer arguments and returns use types from `crate::abi`.
//! String arguments arrive as `*const libc::c_char` (null-terminated).
//! String return values are `*mut MvlString` (from `mvl_memory`).

// TODO(#432): implement once mvl_runtime::stdlib::env is merged from #414.
// Each wrapper should follow this pattern:
//
//   use crate::mvl_c_export;
//   use crate::abi::MvlOption;
//
//   mvl_c_export! {
//       pub fn _mvl_env_get(key: *const libc::c_char) -> *mut MvlOption {
//           // marshal key → &str, call mvl_runtime::stdlib::env::get(),
//           // convert Option<String> → MvlOption on the heap, return ptr.
//           todo!()
//       }
//   }
