//! L5-14: Heap allocation helpers for the MVL LLVM backend.
//!
//! Provides lazy-declaration helpers for every `mvl_memory` runtime function
//! (ADR-0016) and the per-function drop-tracking infrastructure.
//!
//! Pattern mirrors `builtins.rs` (get_printf etc.): each `get_*` method
//! returns the `FunctionValue`, declaring it on first use with External linkage
//! so lli can resolve it via `--load=libmvl_memory.{dylib,so}`.

use inkwell::{module::Linkage, types::BasicMetadataTypeEnum, values::FunctionValue, AddressSpace};

use super::LlvmBackend;

/// Which kind of heap collection a local variable holds.
/// Used to select the correct `_drop` call at function exit.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HeapKind {
    String,
    Array,
    Map,
    /// Set is backed by MvlArray (ADR-0016). If Set ever gets its own layout,
    /// update `heap_kind_of` in stmts.rs and the dispatch arms below.
    Set,
}

impl<'ctx> LlvmBackend<'ctx> {
    // ── String runtime declarations ───────────────────────────────────────────

    /// `mvl_string_new(ptr, i64) -> ptr`
    pub(crate) fn get_mvl_string_new(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_new",
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_string_clone(ptr) -> ptr`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_string_clone(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_clone",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_string_drop(ptr)`
    pub(crate) fn get_mvl_string_drop(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_drop",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            None,
            false,
        )
    }

    /// `mvl_string_len(ptr) -> i64`
    pub(crate) fn get_mvl_string_len(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_len",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `mvl_string_ptr(ptr) -> ptr`  — returns the null-terminated char* for printf.
    pub(crate) fn get_mvl_string_ptr(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_ptr",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_string_concat(ptr, ptr) -> ptr`
    pub(crate) fn get_mvl_string_concat(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_concat",
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.ptr_type(AddressSpace::default()).into(),
            ],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_string_eq(ptr, ptr) -> i32`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_string_eq(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_eq",
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.ptr_type(AddressSpace::default()).into(),
            ],
            Some(self.context.i32_type().into()),
            false,
        )
    }

    // ── Array runtime declarations ────────────────────────────────────────────

    /// `mvl_array_new(i64 elem_size, i64 initial_cap) -> ptr`
    pub(crate) fn get_mvl_array_new(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_new",
            &[
                self.context.i64_type().into(),
                self.context.i64_type().into(),
            ],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_array_clone(ptr) -> ptr`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_array_clone(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_clone",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_array_drop(ptr)`
    pub(crate) fn get_mvl_array_drop(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_drop",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            None,
            false,
        )
    }

    /// `mvl_array_push(ptr arr, ptr elem)`
    pub(crate) fn get_mvl_array_push(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_push",
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.ptr_type(AddressSpace::default()).into(),
            ],
            None,
            false,
        )
    }

    /// `mvl_array_get(ptr arr, i64 idx) -> ptr`
    pub(crate) fn get_mvl_array_get(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_get",
            &[
                self.context.ptr_type(AddressSpace::default()).into(),
                self.context.i64_type().into(),
            ],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_array_len(ptr) -> i64`
    pub(crate) fn get_mvl_array_len(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_array_len",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    // ── Map runtime declarations ──────────────────────────────────────────────

    /// `mvl_map_new(i64 initial_cap) -> ptr`
    pub(crate) fn get_mvl_map_new(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_map_new",
            &[self.context.i64_type().into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_map_clone(ptr) -> ptr`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_map_clone(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_map_clone",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_map_drop(ptr)`
    pub(crate) fn get_mvl_map_drop(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_map_drop",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            None,
            false,
        )
    }

    /// `mvl_map_insert(ptr map, ptr key, i64 key_len, ptr val, i64 val_len)`
    pub(crate) fn get_mvl_map_insert(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i64: BasicMetadataTypeEnum = self.context.i64_type().into();
        self.get_or_declare_fn("mvl_map_insert", &[ptr, ptr, i64, ptr, i64], None, false)
    }

    /// `mvl_map_get(ptr map, ptr key, i64 key_len) -> ptr`
    #[allow(dead_code)]
    pub(crate) fn get_mvl_map_get(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i64: BasicMetadataTypeEnum = self.context.i64_type().into();
        self.get_or_declare_fn(
            "mvl_map_get",
            &[ptr, ptr, i64],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_map_len(ptr) -> i64`
    pub(crate) fn get_mvl_map_len(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_map_len",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.i64_type().into()),
            false,
        )
    }

    /// `mvl_string_chars(ptr) -> ptr`  — returns MvlArray* of MvlString* per char
    pub(crate) fn get_mvl_string_chars(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_string_chars",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_map_keys(ptr) -> ptr`  — returns MvlArray* of MvlString* keys
    pub(crate) fn get_mvl_map_keys(&self) -> FunctionValue<'ctx> {
        self.get_or_declare_fn(
            "mvl_map_keys",
            &[self.context.ptr_type(AddressSpace::default()).into()],
            Some(self.context.ptr_type(AddressSpace::default()).into()),
            false,
        )
    }

    /// `mvl_map_remove(ptr map, ptr key, i64 key_len)`
    pub(crate) fn get_mvl_map_remove(&self) -> FunctionValue<'ctx> {
        let ptr: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i64: BasicMetadataTypeEnum = self.context.i64_type().into();
        self.get_or_declare_fn("mvl_map_remove", &[ptr, ptr, i64], None, false)
    }

    fn get_or_declare_fn(
        &self,
        name: &str,
        param_tys: &[BasicMetadataTypeEnum<'ctx>],
        ret_ty: Option<inkwell::types::BasicTypeEnum<'ctx>>,
        variadic: bool,
    ) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function(name) {
            return f;
        }
        let fn_ty = match ret_ty {
            Some(r) => {
                use inkwell::types::BasicType;
                r.fn_type(param_tys, variadic)
            }
            None => self.context.void_type().fn_type(param_tys, variadic),
        };
        self.module
            .add_function(name, fn_ty, Some(Linkage::External))
    }

    // ── Drop emission (per-function heap cleanup) ─────────────────────────────

    /// Emit `_drop` calls for all tracked heap locals in the current function.
    /// Drop all heap locals except `exclude` (if `Some`).
    ///
    /// Pass `Some(name)` when emitting a `return <name>` for a heap-allocated
    /// variable: ownership of the returned pointer transfers to the caller, so
    /// dropping it here would produce a use-after-free.
    pub(crate) fn emit_heap_drops_except(&self, exclude: Option<&str>) {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        for (name, kind) in &self.heap_locals {
            if exclude == Some(name.as_str()) {
                continue;
            }
            let Some(&(alloca, _)) = self.locals.get(name.as_str()) else {
                continue;
            };
            let heap_ptr = match self
                .builder
                .build_load(ptr_ty, alloca, &format!("drop_{name}"))
            {
                Ok(v) => v,
                Err(_) => continue,
            };
            let drop_fn = match kind {
                HeapKind::String => self.get_mvl_string_drop(),
                HeapKind::Array | HeapKind::Set => self.get_mvl_array_drop(),
                HeapKind::Map => self.get_mvl_map_drop(),
            };
            let _ = self
                .builder
                .build_call(drop_fn, &[heap_ptr.into()], "drop_call");
        }
    }

    /// Return true if the builder is currently positioned in the entry block of the
    /// current function.  Used to decide whether a heap local's alloca will dominate
    /// the function exit (where drop calls are emitted).
    pub(crate) fn in_entry_block(&self) -> bool {
        let Some(fn_val) = self.current_fn else {
            return false;
        };
        let Some(entry) = fn_val.get_first_basic_block() else {
            return false;
        };
        self.builder.get_insert_block() == Some(entry)
    }
}
