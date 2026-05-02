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

    // ── std.env — wired (#432) ────────────────────────────────────────────────

    /// `_mvl_env_getuid() -> i64`
    pub(crate) fn get_mvl_env_getuid(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_env_getuid",
            &[],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `_mvl_env_getgid() -> i64`
    pub(crate) fn get_mvl_env_getgid(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_env_getgid",
            &[],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `_mvl_env_exit(code: i64) -> void`  (diverging — caller emits `unreachable` after)
    pub(crate) fn get_mvl_env_exit(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_env_exit",
            &[self.context.i64_type().into()],
            None,
            false,
        )
    }

    // ── std.env — pending LLVM codegen wiring ────────────────────────────────

    /// `_mvl_env_get(key: ptr) -> ptr`  (heap C string, null = None)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_get(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_get", &[ptr], Some(ptr.try_into().unwrap()), false)
    }

    /// `_mvl_env_set_var(key: ptr, val: ptr) -> i32`  (0=ok, 1=err)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_set_var(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i32_ty = self.context.i32_type().into();
        self.get_or_declare_fn("_mvl_env_set_var", &[ptr, ptr], Some(i32_ty), false)
    }

    /// `_mvl_env_remove_var(key: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_remove_var(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_remove_var", &[ptr], None, false)
    }

    /// `_mvl_env_current_dir() -> ptr`  (heap C string, null = error)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_current_dir(&self) -> FunctionValue<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.get_or_declare_fn("_mvl_env_current_dir", &[], Some(ptr.into()), false)
    }

    /// `_mvl_env_args_count() -> i64`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_args_count(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "_mvl_env_args_count",
            &[],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `_mvl_env_args_get(i: i64) -> ptr`  (heap C string, null = out of bounds)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_args_get(&self) -> FunctionValue<'ctx> {
        let ptr = self.context.ptr_type(AddressSpace::default());
        self.get_or_declare_fn(
            "_mvl_env_args_get",
            &[self.context.i64_type().into()],
            Some(ptr.into()),
            false,
        )
    }

    /// `_mvl_env_free_cstr(s: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_env_free_cstr(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_env_free_cstr", &[ptr], None, false)
    }

    // ── std.process — pending LLVM codegen wiring ─────────────────────────────

    /// `_mvl_process_spawn(cmd: ptr, stdin: i8, stdout: i8, stderr: i8) -> ptr`  (Child*)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_spawn(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i8_ty: BasicMetadataTypeEnum = self.context.i8_type().into();
        self.get_or_declare_fn(
            "_mvl_process_spawn",
            &[ptr, i8_ty, i8_ty, i8_ty],
            Some(ptr.try_into().unwrap()),
            false,
        )
    }

    /// `_mvl_process_wait(child: ptr) -> i64`  (exit code; -1 on error)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_wait(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn(
            "_mvl_process_wait",
            &[ptr],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `_mvl_process_kill(child: ptr) -> i64`  (0=ok, -1=error)
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_kill(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn(
            "_mvl_process_kill",
            &[ptr],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `_mvl_process_drop_child(child: ptr)`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_process_drop_child(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        self.get_or_declare_fn("_mvl_process_drop_child", &[ptr], None, false)
    }
}
