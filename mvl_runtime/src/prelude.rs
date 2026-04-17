//! Prelude — everything a generated MVL file needs in one `use` line.
//!
//! Every file emitted by the MVL transpiler starts with:
//! ```rust
//! use mvl_runtime::prelude::*;
//! ```

pub use crate::effects::{
    Alloc, Concurrent, Console, Db, FileRead, FileWrite, Net, Panic, Terminal,
};
pub use crate::ifc::{declassify, sanitize, Clean, Public, Secret, Tainted};
pub use crate::mvl_refine;

// ── Standard library implementations ──────────────────────────────────────
//
// These re-exports provide the Rust backing for stdlib functions declared as
// stubs in `std/*.mvl`. Programs that import `use std.io.*` or `use std.args.*`
// call these directly — no per-program `bridge.rs` is needed for generic I/O.

/// `std.io` — file I/O operations.
pub use crate::stdlib::io::{path, read_file, read_to_string, Path};

/// `std.args` — CLI argument and environment access.
pub use crate::stdlib::args::{get_arg, get_args, get_env, parse, ParseFromArgs};
