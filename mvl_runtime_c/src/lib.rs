//! MVL C-ABI runtime — cdylib for the LLVM backend (ADR-0018).
//!
//! Loaded by `lli` at runtime via `--load=libmvl_runtime_c.{so,dylib}`.
//! Wraps `mvl_runtime` Rust APIs behind C-ABI symbols so LLVM IR can call
//! them with `declare` + `Linkage::External`.
//!
//! # Two-path architecture
//!
//! ```text
//! Rust transpiler:  MVL → Rust source → cargo
//!                   stdlib via `use mvl_runtime::prelude::*`  (Rust API)
//!
//! LLVM backend:     MVL → LLVM IR → lli
//!                   stdlib via libmvl_runtime_c.so (C-ABI exports, this crate)
//! ```
//!
//! # Adding a new export
//!
//! Use the `mvl_c_export!` macro for mechanical wrappers:
//!
//! ```rust,ignore
//! mvl_c_export! {
//!     pub fn _mvl_my_fn(x: i64) -> i64 {
//!         mvl_runtime::my_module::my_fn(x)
//!     }
//! }
//! ```
//!
//! For functions that take or return heap types, use the marshalling types
//! from [`abi`] (`MvlOption`, `MvlResult`) and the pointer types from
//! `mvl_memory` (`MvlString*`, `MvlArray*`, `MvlMap*`).

pub mod abi;
pub mod stdlib;
pub mod version;

/// Generate a `#[no_mangle] pub unsafe extern "C"` wrapper from a plain
/// function definition.  The function body may call into `mvl_runtime` APIs.
///
/// # Example
///
/// ```rust,ignore
/// mvl_c_export! {
///     pub fn _mvl_env_get(key: *const libc::c_char) -> *mut abi::MvlOption {
///         // ... marshal and call mvl_runtime::stdlib::env::get(...)
///         std::ptr::null_mut()
///     }
/// }
/// ```
#[macro_export]
macro_rules! mvl_c_export {
    (
        pub fn $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty $body:block
    ) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name( $($arg: $ty),* ) -> $ret $body
    };
    (
        pub fn $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) $body:block
    ) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name( $($arg: $ty),* ) $body
    };
}
