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
