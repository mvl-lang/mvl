//! C-ABI runtime declarations for the LLVM backend (ADR-0018).
//!
//! Lazy-declares symbols from `libmvl_runtime_c.{so,dylib}`, using the same
//! `get_or_declare_fn` pattern as `memory.rs`.  Add one `get_*` method per
//! exported symbol; the LLVM backend calls these on first use.
//!
//! # Adding a new symbol (#432)
//!
//! 1. Export the symbol in `mvl_runtime_c/src/stdlib/<module>.rs`
//! 2. Add a `get_mvl_<name>()` method here mirroring the pattern below
//! 3. Emit the `declare` + `call` in the relevant codegen file

use inkwell::{types::BasicMetadataTypeEnum, values::FunctionValue, AddressSpace};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── Bootstrap / version ───────────────────────────────────────────────────

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

    // ── std.env (#432 — declare helpers ready; implementations pending #414) ──

    /// `_mvl_env_get(key: ptr) -> ptr`  (MvlOption*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_get(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_get", &[ptr], Some(ptr.try_into().unwrap()), false)
    }

    /// `_mvl_env_set_var(key: ptr, val: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_set_var(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_set_var", &[ptr, ptr], None, false)
    }

    /// `_mvl_env_remove_var(key: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_remove_var(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_remove_var", &[ptr], None, false)
    }

    /// `_mvl_env_args() -> ptr`  (MvlArray* of MvlString*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_args(&self) -> FunctionValue<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.get_or_declare_fn("_mvl_env_args", &[], Some(ptr.into()), false)
    }

    /// `_mvl_env_current_dir() -> ptr`  (MvlOption*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_current_dir(&self) -> FunctionValue<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.get_or_declare_fn("_mvl_env_current_dir", &[], Some(ptr.into()), false)
    }

    /// `_mvl_process_id() -> i64`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_id(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_process_id",
            &[],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    // ── std.process (#432 — declare helpers ready; implementations pending #414) ─

    /// `_mvl_process_command_new(prog: ptr) -> ptr`  (command handle)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_command_new(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn(
            "_mvl_process_command_new",
            &[ptr],
            Some(ptr.try_into().unwrap()),
            false,
        )
    }

    /// `_mvl_process_command_arg(cmd: ptr, arg: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_command_arg(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_process_command_arg", &[ptr, ptr], None, false)
    }

    /// `_mvl_process_command_spawn(cmd: ptr) -> ptr`  (MvlResult*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_command_spawn(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn(
            "_mvl_process_command_spawn",
            &[ptr],
            Some(ptr.try_into().unwrap()),
            false,
        )
    }

    /// `_mvl_process_handle_wait(handle: ptr) -> ptr`  (MvlResult*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_handle_wait(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn(
            "_mvl_process_handle_wait",
            &[ptr],
            Some(ptr.try_into().unwrap()),
            false,
        )
    }
}
