//! C-ABI runtime declarations for the LLVM backend (ADR-0018).
//!
//! Lazy-declares symbols from `libmvl_runtime_c.{so,dylib}`, using the same
//! `get_or_declare_fn` pattern as `memory.rs`.  Add one `get_*` method per
//! exported symbol; the LLVM backend calls these on first use.

use inkwell::{values::FunctionValue, AddressSpace};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    /// `_mvl_runtime_version() -> ptr`  (null-terminated C string)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_runtime_version(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_runtime_version",
            &[],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }
}
