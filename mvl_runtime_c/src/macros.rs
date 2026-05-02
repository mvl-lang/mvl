//! `mvl_c_export!` — declarative macro for generating C-ABI wrapper functions.
//!
//! Reduces boilerplate: each new stdlib export is ~5-10 lines instead of ~50.
//!
//! # Usage
//!
//! ```rust,ignore
//! mvl_c_export! {
//!     /// Optional doc comment.
//!     fn _mvl_env_getuid() -> i64 {
//!         mvl_runtime::stdlib::env::getuid()
//!     }
//! }
//! ```
//!
//! The macro wraps the body in `#[no_mangle] pub extern "C"` and marks the
//! function `#[allow(unsafe_code)]` so that callers in the body can use unsafe
//! blocks when needed.
//!
//! For functions whose body uses unsafe blocks, write the `unsafe` inside the
//! body expression rather than on the `fn` signature.

#[macro_export]
macro_rules! mvl_c_export {
    // One or more function items with optional doc attrs.
    ($($(#[$attr:meta])* fn $name:ident($($param:ident : $pty:ty),*) -> $ret:ty $body:block)*) => {
        $(
            $(#[$attr])*
            #[no_mangle]
            #[allow(unsafe_code)]
            pub extern "C" fn $name($($param: $pty),*) -> $ret {
                $body
            }
        )*
    };
}
