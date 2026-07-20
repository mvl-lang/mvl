// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared low-level emit helpers — type mapping, heap-drop tracking,
//! string globals, and value-conversion routines used by the TIR walker.
//!
//! Consolidated from the AST emit_*.rs modules (which #1612 Phase 3b PR 2
//! deleted). All helpers here are AST-shape-agnostic and live above the
//! TIR vs AST boundary.

use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{BinaryOp, Literal, TypeExpr};
use crate::mvl::parser::lexer::Span;

use super::{HeapKind, TextEmitter, RESULT_LLVM_TY};

/// Synthesize a `TypeExpr` from a checker-resolved `Ty` so loop variables
/// land in `local_mvl_types` and method dispatch can find the right kind.
///
/// Returns `None` for compound types we don't yet need to round-trip
/// (e.g. `Ty::Session`).
pub(super) fn ty_to_type_expr(ty: &Ty) -> Option<TypeExpr> {
    let base = |name: &str, args: Vec<TypeExpr>| TypeExpr::Base {
        name: name.into(),
        args,
        span: Span::default(),
    };
    Some(match ty {
        Ty::Int => base("Int", vec![]),
        Ty::UInt => base("UInt", vec![]),
        Ty::Float => base("Float", vec![]),
        Ty::Bool => base("Bool", vec![]),
        Ty::Byte => base("Byte", vec![]),
        Ty::UByte => base("UByte", vec![]),
        Ty::Char => base("Char", vec![]),
        Ty::Unit => base("Unit", vec![]),
        Ty::String => base("String", vec![]),
        Ty::List(inner) => base("List", vec![ty_to_type_expr(inner)?]),
        Ty::Array(inner, _) => base("Array", vec![ty_to_type_expr(inner)?]),
        Ty::Set(inner) => base("Set", vec![ty_to_type_expr(inner)?]),
        Ty::Map(k, v) => base("Map", vec![ty_to_type_expr(k)?, ty_to_type_expr(v)?]),
        Ty::Option(inner) => TypeExpr::Option {
            inner: Box::new(ty_to_type_expr(inner)?),
            span: Span::default(),
        },
        Ty::Result(ok, err) => TypeExpr::Result {
            ok: Box::new(ty_to_type_expr(ok)?),
            err: Box::new(ty_to_type_expr(err)?),
            span: Span::default(),
        },
        Ty::Ref(mutable, inner) => TypeExpr::Ref {
            mutable: *mutable,
            inner: Box::new(ty_to_type_expr(inner)?),
            span: Span::default(),
        },
        Ty::Named(name, args) => base(name, args.iter().filter_map(ty_to_type_expr).collect()),
        // IFC labels (`Secret[T]`, `Tainted[T]`, user-declared labels). The label
        // is erased at codegen; only the underlying `T` matters for LLVM
        // emission.
        Ty::Labeled(label, inner) => TypeExpr::Labeled {
            label: label.clone(),
            inner: Box::new(ty_to_type_expr(inner)?),
            span: Span::default(),
        },
        // Refinements are spec-only and erased before codegen — drop the
        // predicate and surface the underlying type.
        Ty::Refined(inner, _pred) => ty_to_type_expr(inner)?,
        Ty::Ptr(inner) => base("Ptr", vec![ty_to_type_expr(inner)?]),
        // Fn types — required for `type Dispatcher = fn(...) -> ...` aliases
        // to be registered in `module.fn_aliases`, which is how the emitter
        // dispatches `d(req)` to an indirect call through a fn-pointer local
        // instead of a direct named call (#1467 / #1612 task 2d).
        Ty::Fn(params, ret, effects, _totality) => TypeExpr::Fn {
            params: params.iter().filter_map(ty_to_type_expr).collect(),
            ret: Box::new(ty_to_type_expr(ret)?),
            effects: effects.clone(),
            span: Span::default(),
        },
        _ => return None,
    })
}

impl TextEmitter {
    // ── Heap drop emission (#1185) ────────────────────────────────────────

    /// Drop heap locals registered after `snapshot_len` (i.e. inside the
    /// current loop body / branch) and truncate `heap_locals` back to that
    /// length, skipping `escape` if provided (#1617).
    ///
    /// Loops pass `escape = None` — every per-iteration temporary must be
    /// dropped. Branches of an if-expression pass `escape = Some(<return ssa>)`
    /// so the branch's return value isn't freed before the phi consumes it.
    /// The escape ssa is removed from heap_locals (the phi result that
    /// dominates the join becomes the new owner via the surrounding let).
    pub(super) fn drop_loop_body_locals(&mut self, snapshot_len: usize) {
        self.drop_scope_locals(snapshot_len, None);
    }

    pub(super) fn drop_scope_locals(&mut self, snapshot_len: usize, escape: Option<&str>) {
        // A terminated sibling branch may have called `retain` (via
        // `exclude_returned_value_tir`) and removed a pre-snapshot item,
        // leaving `heap_locals.len() < snapshot_len`. In that case there are
        // no post-snapshot items to drain — clamp to avoid a panic.
        let start = snapshot_len.min(self.fn_ctx.heap_locals.len());
        let extras: Vec<_> = self.fn_ctx.heap_locals.drain(start..).collect();
        for (ssa, kind, is_ref) in extras {
            if escape.map(|e| e == ssa).unwrap_or(false) {
                // Branch return value — consumed by the surrounding phi.
                continue;
            }
            let sym = match kind {
                HeapKind::String => "_mvl_string_drop",
                HeapKind::Array => "_mvl_array_drop",
                HeapKind::Map => "_mvl_map_drop",
            };
            self.ensure_extern(&format!("declare void @{sym}(ptr)"));
            if is_ref {
                let loaded = self.next_reg();
                self.push_instr(&format!("{loaded} = load ptr, ptr {ssa}"));
                self.push_instr(&format!("call void @{sym}(ptr {loaded})"));
            } else {
                self.push_instr(&format!("call void @{sym}(ptr {ssa})"));
            }
        }
    }

    /// Emit `mvl_*_drop` calls for all tracked heap locals.
    /// Called before every `ret` instruction to clean up owned allocations.
    pub(super) fn emit_heap_drops(&mut self) {
        for (ssa, kind, is_ref) in self.fn_ctx.heap_locals.clone() {
            let sym = match kind {
                HeapKind::String => "_mvl_string_drop",
                HeapKind::Array => "_mvl_array_drop",
                HeapKind::Map => "_mvl_map_drop",
            };
            self.ensure_extern(&format!("declare void @{sym}(ptr)"));
            if is_ref {
                // For ref locals, the SSA is a stack alloca — load the heap
                // object pointer before dropping it.
                let loaded = self.next_reg();
                self.push_instr(&format!("{loaded} = load ptr, ptr {ssa}"));
                self.push_instr(&format!("call void @{sym}(ptr {loaded})"));
            } else {
                self.push_instr(&format!("call void @{sym}(ptr {ssa})"));
            }
        }
    }

    // ── Helper-global emitters ────────────────────────────────────────────

    pub(super) fn ensure_println_fmt(&mut self) -> &'static str {
        if !self.module.has_println_fmt {
            self.module.str_globals.push(
                "@println_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0a\\00\"".into(),
            );
            self.module.has_println_fmt = true;
        }
        "println_fmt"
    }

    pub(super) fn ensure_int_fmt(&mut self) -> &'static str {
        if !self.module.has_int_fmt {
            self.module
                .str_globals
                .push("@int_fmt = private unnamed_addr constant [5 x i8] c\"%lld\\00\"".into());
            self.module.has_int_fmt = true;
        }
        "int_fmt"
    }

    pub(super) fn ensure_bool_str_globals(&mut self) -> (&'static str, &'static str) {
        if !self.module.has_str_true {
            self.module
                .str_globals
                .push("@str_true = private unnamed_addr constant [5 x i8] c\"true\\00\"".into());
            self.module.has_str_true = true;
        }
        if !self.module.has_str_false {
            self.module
                .str_globals
                .push("@str_false = private unnamed_addr constant [6 x i8] c\"false\\00\"".into());
            self.module.has_str_false = true;
        }
        ("str_true", "str_false")
    }

    /// Create a module-level string constant from raw bytes.
    /// Returns the global name (without `@`).
    pub(super) fn emit_str_global(&mut self, s: &str) -> String {
        let name = format!("str.{}", self.module.str_counter);
        self.module.str_counter += 1;
        let mut escaped = String::new();
        for byte in s.bytes() {
            match byte {
                b'\\' => escaped.push_str("\\5c"),
                b'"' => escaped.push_str("\\22"),
                b'\n' => escaped.push_str("\\0a"),
                b'\r' => escaped.push_str("\\0d"),
                b'\t' => escaped.push_str("\\09"),
                b if !(0x20..0x7f).contains(&b) => escaped.push_str(&format!("\\{b:02x}")),
                b => escaped.push(b as char),
            }
        }
        let total_len = s.len() + 1; // +1 for null terminator
        self.module.str_globals.push(format!(
            "@{name} = private unnamed_addr constant [{total_len} x i8] c\"{escaped}\\00\""
        ));
        name
    }

    /// Emit instructions to create a heap `MvlString*` from a Rust string literal.
    /// Returns the SSA register (type: ptr).
    pub(super) fn emit_string_literal(&mut self, s: &str) -> String {
        let global = self.emit_str_global(s);
        let len = s.len();
        self.ensure_extern("declare ptr @_mvl_string_new(ptr, i64)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @_mvl_string_new(ptr @{global}, i64 {len})"
        ));
        self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
        reg
    }

    // ── Checked-type helpers (Ty → LLVM) (#1302) ──────────────────────────

    /// Map a checker-resolved [`Ty`] to its LLVM IR type string (static).
    pub(super) fn ty_to_llvm(ty: &Ty) -> String {
        match ty {
            Ty::Int | Ty::UInt => "i64".into(),
            Ty::Float => "double".into(),
            Ty::Bool => "i1".into(),
            Ty::Byte | Ty::UByte => "i8".into(),
            Ty::Char => "i32".into(),
            Ty::Unit => "void".into(),
            Ty::String => "ptr".into(),
            Ty::Option(_) | Ty::Result(_, _) => RESULT_LLVM_TY.into(),
            Ty::List(_) | Ty::Array(_, _) | Ty::Set(_) | Ty::Map(_, _) | Ty::Ptr(_) => "ptr".into(),
            Ty::Ref(_, inner) => Self::ty_to_llvm(inner),
            Ty::Labeled(_, inner) => Self::ty_to_llvm(inner),
            Ty::Refined(inner, _) => Self::ty_to_llvm(inner),
            Ty::Named(_, _) | Ty::Fn(_, _, _, _) => "ptr".into(),
            // Never (bottom type) maps to void — expressions of this type diverge
            // and should never produce a value.
            Ty::Never => "void".into(),
            Ty::Session(_) | Ty::Unknown => "ptr".into(),
        }
    }

    /// Map a checker-resolved [`Ty`] to its LLVM IR type, consulting registries.
    pub(super) fn ty_to_llvm_ctx(&self, ty: &Ty) -> String {
        match ty {
            Ty::Named(name, _) => {
                if self.module.struct_fields.contains_key(name) {
                    if self.module.tir_actor_decls.contains_key(name.as_str()) {
                        return "ptr".into();
                    }
                    return format!("%{name}");
                }
                if self.module.enum_variants.contains_key(name) {
                    if self.enum_has_payloads(name) {
                        return RESULT_LLVM_TY.into();
                    }
                    return "i64".into();
                }
                if self.module.tir_actor_decls.contains_key(name.as_str()) {
                    return "ptr".into();
                }
                // #1851: named type aliases resolve to their underlying Ty.
                if let Some(inner) = self.module.type_aliases.get(name).cloned() {
                    return self.ty_to_llvm_ctx(&inner);
                }
                Self::ty_to_llvm(ty)
            }
            Ty::Ref(_, inner) => self.ty_to_llvm_ctx(inner),
            Ty::Labeled(_, inner) => self.ty_to_llvm_ctx(inner),
            Ty::Refined(inner, _) => self.ty_to_llvm_ctx(inner),
            _ => Self::ty_to_llvm(ty),
        }
    }

    // ── Type helpers (TypeExpr → LLVM) ────────────────────────────────────

    /// Map a MVL `TypeExpr` to its LLVM IR type string (static, no context).
    pub(super) fn llvm_ty(ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "Int" | "UInt" => "i64".to_string(),
                "Float" => "double".to_string(),
                "Bool" => "i1".to_string(),
                "Byte" | "UByte" => "i8".to_string(),
                "Char" => "i32".to_string(),
                "Unit" | "Never" => "void".to_string(),
                _ => "ptr".to_string(),
            },
            // Both `val T` and `ref T` lower to the underlying type's IR
            // representation — capability is enforced at the checker, not in codegen.
            TypeExpr::Ref { inner, .. } => Self::llvm_ty(inner),
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                Self::llvm_ty(inner)
            }
            // Option[T] / Result[T, E] → { i8, ptr } tagged-union (disc byte + payload ptr)
            TypeExpr::Option { .. } | TypeExpr::Result { .. } => "{ i8, ptr }".to_string(),
            _ => "ptr".to_string(),
        }
    }

    /// Map a MVL `TypeExpr` to its LLVM IR type, consulting struct/enum registries.
    pub(super) fn llvm_ty_ctx(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, .. } => {
                // Resolve generic type parameters (active during monomorphized emission).
                if let Some(concrete) = self.mono.type_param_map.get(name.as_str()) {
                    return self.llvm_ty_ctx(concrete);
                }
                if self.module.struct_fields.contains_key(name) {
                    // Actor state structs are always accessed via pointer — the
                    // actor handle is an opaque ptr, not an inline struct value.
                    if self.module.tir_actor_decls.contains_key(name.as_str()) {
                        return "ptr".to_string();
                    }
                    return format!("%{name}");
                }
                if self.module.enum_variants.contains_key(name) {
                    // Payload enums lower to `{ i8, ptr }`; pure unit enums stay i64 (#1200).
                    if self.enum_has_payloads(name) {
                        return RESULT_LLVM_TY.to_string();
                    }
                    return "i64".to_string();
                }
                // Actor type without registered state struct (e.g. handle as field).
                if self.module.tir_actor_decls.contains_key(name.as_str()) {
                    return "ptr".to_string();
                }
                // #1851: named type aliases (`type Port = Int where ...`).
                // Resolve to their underlying representation before falling
                // through to the raw base-name matcher (which defaults
                // unknown names to `ptr`).
                if let Some(inner) = self.module.type_aliases.get(name).cloned() {
                    return self.ty_to_llvm_ctx(&inner);
                }
                Self::llvm_ty(ty)
            }
            // Both `val T` and `ref T` lower to T's IR representation.
            TypeExpr::Ref { inner, .. } => self.llvm_ty_ctx(inner),
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                self.llvm_ty_ctx(inner)
            }
            _ => Self::llvm_ty(ty),
        }
    }

    pub(super) fn is_void(ty: &TypeExpr) -> bool {
        Self::llvm_ty(ty) == "void"
    }

    /// Return the byte size of an LLVM IR type string on a 64-bit target.
    ///
    /// Used to compute `elem_size` for `mvl_array_new`.
    pub(super) fn llvm_type_size(ty: &str) -> usize {
        match ty {
            "i1" | "i8" => 1,
            "i16" => 2,
            "i32" => 4,
            "i64" | "double" | "ptr" => 8,
            // Tagged unions: { i8, ptr } → 16 bytes (8-byte aligned)
            s if s.starts_with("{ i8, ptr }") => 16,
            // Named struct types (%Foo) — conservatively use pointer size
            s if s.starts_with('%') => 8,
            _ => 8,
        }
    }

    /// Classify a type as heap-allocated for drop tracking.
    pub(super) fn heap_kind(ty: &TypeExpr) -> Option<HeapKind> {
        let base = match ty {
            TypeExpr::Ref { inner, .. }
            | TypeExpr::Labeled { inner, .. }
            | TypeExpr::Refined { inner, .. } => inner.as_ref(),
            other => other,
        };
        match base {
            TypeExpr::Base { name, args, .. } => match name.as_str() {
                "String" => Some(HeapKind::String),
                "List" | "Array" | "Set" => {
                    // Lists whose element type is not a known primitive heap type
                    // (e.g. List[Match]) require per-element cleanup that the LLVM
                    // emitter cannot generate.  Skip heap tracking for these to
                    // avoid SSA dominance violations from out-of-scope drops (#1202).
                    let elem_is_known = args.first().is_none_or(|a| {
                        matches!(
                            a,
                            TypeExpr::Base { name, .. }
                            if matches!(name.as_str(), "Int" | "Float" | "Bool" | "String"
                                | "UInt" | "Byte" | "UByte" | "Char")
                        )
                    });
                    if elem_is_known {
                        Some(HeapKind::Array)
                    } else {
                        None
                    }
                }
                "Map" => Some(HeapKind::Map),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn is_mutable_ref(ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Ref { mutable: true, .. })
    }

    pub(super) fn deref_ty(ty: &TypeExpr) -> &TypeExpr {
        match ty {
            TypeExpr::Ref { inner, .. } => inner.as_ref(),
            other => other,
        }
    }

    /// Infer the LLVM type from an already-emitted SSA value string.
    pub(super) fn infer_val_type(&self, val: &str) -> String {
        if val.starts_with('%') {
            self.fn_ctx
                .reg_types
                .get(val)
                .cloned()
                .unwrap_or_else(|| "i64".into())
        } else if val == "true" || val == "false" {
            "i1".into()
        } else if val.contains('.') {
            "double".into()
        } else {
            "i64".into()
        }
    }

    /// Resolve a possibly-aliased fn type to its underlying `TypeExpr::Fn`.
    ///
    /// Peels `val`/`ref`/`Labeled`/`Refined` wrappers and follows named
    /// aliases (e.g. `type Dispatcher = fn(val Request) -> Response`).
    /// Returns `None` if the type isn't a fn-alias or direct fn type.
    pub(super) fn resolve_fn_alias(&self, ty: &TypeExpr) -> Option<TypeExpr> {
        let mut cur = ty.clone();
        loop {
            match cur {
                TypeExpr::Fn { .. } => return Some(cur),
                TypeExpr::Ref { inner, .. }
                | TypeExpr::Labeled { inner, .. }
                | TypeExpr::Refined { inner, .. } => cur = *inner,
                TypeExpr::Base { ref name, .. } => {
                    let aliased = self.module.fn_aliases.get(name)?;
                    cur = aliased.clone();
                }
                _ => return None,
            }
        }
    }

    /// Emit `call ptr @_mvl_list_slice(ptr val, i64 start, i64 end)` and
    /// return the result register. Shared by slice/take/skip dispatch.
    pub(super) fn emit_list_slice_call(&mut self, val: &str, start: &str, end: &str) -> String {
        self.ensure_extern("declare ptr @_mvl_list_slice(ptr, i64, i64)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @_mvl_list_slice(ptr {val}, i64 {start}, i64 {end})"
        ));
        self.fn_ctx.reg_types.insert(reg.clone(), "ptr".into());
        reg
    }

    // ── Int/Bool → String helpers ─────────────────────────────────────────

    pub(super) fn emit_int_to_string(&mut self, val: &str) -> String {
        let int_fmt = self.ensure_int_fmt();
        self.ensure_extern("declare i32 @snprintf(ptr, i64, ptr, ...)");
        self.ensure_extern("declare ptr @_mvl_string_new(ptr, i64)");
        let buf = self.next_reg();
        self.push_instr(&format!("{buf} = alloca [32 x i8]"));
        let len32 = self.next_reg();
        self.push_instr(&format!(
            "{len32} = call i32 (ptr, i64, ptr, ...) @snprintf(ptr {buf}, i64 32, ptr @{int_fmt}, i64 {val})"
        ));
        let len = self.next_reg();
        self.push_instr(&format!("{len} = sext i32 {len32} to i64"));
        let str_reg = self.next_reg();
        self.push_instr(&format!(
            "{str_reg} = call ptr @_mvl_string_new(ptr {buf}, i64 {len})"
        ));
        self.fn_ctx.reg_types.insert(str_reg.clone(), "ptr".into());
        str_reg
    }

    pub(super) fn emit_bool_to_string(&mut self, val: &str) -> String {
        let (t, f) = self.ensure_bool_str_globals();
        self.ensure_extern("declare ptr @_mvl_string_new(ptr, i64)");
        let cptr = self.next_reg();
        self.push_instr(&format!("{cptr} = select i1 {val}, ptr @{t}, ptr @{f}"));
        let clen = self.next_reg();
        self.push_instr(&format!("{clen} = select i1 {val}, i64 4, i64 5"));
        let str_reg = self.next_reg();
        self.push_instr(&format!(
            "{str_reg} = call ptr @_mvl_string_new(ptr {cptr}, i64 {clen})"
        ));
        self.fn_ctx.reg_types.insert(str_reg.clone(), "ptr".into());
        str_reg
    }

    // ── Result/Option aggregate builders ──────────────────────────────────

    /// Compute the heap-allocation size (in bytes) for an LLVM type string.
    ///
    /// Used by Result/Option constructors to replace stack `alloca` with
    /// `_mvl_alloc`, so that payload pointers remain valid after the
    /// constructor function returns.
    pub(super) fn alloc_size_for_llvm_ty(&self, ty: &str) -> u64 {
        match ty {
            "i1" | "i8" => 1,
            "i32" => 4,
            "i64" | "double" | "ptr" => 8,
            super::RESULT_LLVM_TY => 16, // { i8, ptr } — 1-byte disc + 7 pad + 8-byte ptr
            _ if ty.starts_with('%') => {
                // Named struct: compute size using natural alignment.
                let struct_name = &ty[1..];
                if let Some(fields) = self.module.struct_fields.get(struct_name) {
                    let mut offset: u64 = 0;
                    let mut max_align: u64 = 1;
                    for (_, fte) in fields {
                        let ft = self.llvm_ty_ctx(fte);
                        let (fsz, falign) = self.llvm_type_size_align(&ft);
                        offset = (offset + falign - 1) & !(falign - 1);
                        offset += fsz;
                        max_align = max_align.max(falign);
                    }
                    (offset + max_align - 1) & !(max_align - 1)
                } else {
                    8 // unknown struct — conservative fallback
                }
            }
            _ => 8,
        }
    }

    /// Returns (size_bytes, align_bytes) for a primitive or well-known
    /// composite LLVM type.  Used by [`alloc_size_for_llvm_ty`].
    fn llvm_type_size_align(&self, ty: &str) -> (u64, u64) {
        match ty {
            "i1" | "i8" => (1, 1),
            "i32" => (4, 4),
            "i64" | "double" | "ptr" => (8, 8),
            super::RESULT_LLVM_TY => (16, 8),
            _ if ty.starts_with('%') => (self.alloc_size_for_llvm_ty(ty), 8),
            _ => (8, 8),
        }
    }

    /// Emit `_mvl_alloc(size)` for a heap-allocated payload slot of the given
    /// LLVM type, then store `value` into it.  Returns the allocated pointer
    /// register.  This replaces the old `alloca T; store T value, ptr alloca`
    /// pattern, which produced dangling pointers when the Result/Option
    /// escaped the constructing function's stack frame.
    pub(super) fn emit_heap_slot(&mut self, ty: &str, value: &str) -> String {
        let sz = self.alloc_size_for_llvm_ty(ty);
        self.ensure_extern("declare ptr @_mvl_alloc(i64)");
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = call ptr @_mvl_alloc(i64 {sz})"));
        self.push_instr(&format!("store {ty} {value}, ptr {slot}"));
        slot
    }

    /// Build a `{ i8, ptr }` Result aggregate from a discriminant byte and a
    /// payload slot pointer.
    ///
    /// Both fields are immediately overwritten, so `zeroinitializer` is used
    /// as the base (safe if the struct ever gains padding fields, unlike
    /// `undef`).
    pub(super) fn wrap_result_pair(&mut self, disc: &str, slot: &str) -> String {
        let r0 = self.next_reg();
        self.push_instr(&format!(
            "{r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 {disc}, 0"
        ));
        self.fn_ctx
            .reg_types
            .insert(r0.clone(), RESULT_LLVM_TY.into());
        let r1 = self.next_reg();
        self.push_instr(&format!(
            "{r1} = insertvalue {RESULT_LLVM_TY} {r0}, ptr {slot}, 1"
        ));
        self.fn_ctx
            .reg_types
            .insert(r1.clone(), RESULT_LLVM_TY.into());
        r1
    }

    /// Emit `None` — builds a `{ i8, ptr }` tagged union with disc=1 and
    /// null payload.  No allocation needed: the None match arm never
    /// dereferences the payload pointer.
    pub(super) fn emit_none_constructor(&mut self) -> Result<Option<String>, String> {
        let r1 = self.wrap_result_pair("1", "null");
        Ok(Some(r1))
    }

    // ── Closure infrastructure (#1148) ────────────────────────────────────

    /// Emit `%__closure_type = type { ptr, ptr }` exactly once.
    pub(super) fn ensure_closure_type(&mut self) {
        if !self.module.closure_type_emitted {
            self.module
                .type_defs
                .push("%__closure_type = type { ptr, ptr }".into());
            self.module.closure_type_emitted = true;
        }
    }

    /// Wrap a named module-level function in a `{ wrapper_ptr, null }` closure struct.
    ///
    /// Lazily generates `__closure_wrap_NAME(ptr env, params…) → ret` that ignores
    /// `env` and forwards to the original function.
    pub(super) fn make_named_fn_closure_hof(
        &mut self,
        name: &str,
        ptr_param_indices: &[usize],
    ) -> Result<Option<String>, String> {
        // Include ptr_param_indices in the wrapper name so we get distinct
        // wrappers for HOF vs non-HOF uses of the same named function.
        let suffix = if ptr_param_indices.is_empty() {
            String::new()
        } else {
            format!(
                "_hof{}",
                ptr_param_indices
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join("_")
            )
        };
        let wrapper_name = format!("__closure_wrap_{name}{suffix}");
        self.ensure_closure_type();

        // Emit the wrapper function once.
        if !self.module.fn_ret_types.contains_key(&wrapper_name) {
            let orig_ret = match self.module.fn_ret_types.get(name).cloned() {
                Some(t) => t,
                None => return Ok(None),
            };

            let llvm_ret = self.llvm_ty_ctx(&orig_ret);
            let is_void = Self::is_void(&orig_ret);
            let define_ret = if is_void {
                "void".into()
            } else {
                llvm_ret.clone()
            };

            // Build typed trampoline: (ptr %__env, ty0 %__arg0, ty1 %__arg1, …)
            // For HOF params (in ptr_param_indices), accept ptr and load inside.
            let orig_params = self
                .module
                .fn_param_types
                .get(name)
                .cloned()
                .unwrap_or_default();
            let mut wrapper_param_parts = vec!["ptr %__env".to_string()];
            let mut forward_arg_parts: Vec<String> = Vec::new();
            let mut loads: Vec<String> = Vec::new();
            for (i, p_ty) in orig_params.iter().enumerate() {
                let ty_str = self.llvm_ty_ctx(p_ty);
                if ty_str != "void" {
                    if ptr_param_indices.contains(&i) {
                        // Runtime passes element by pointer.
                        wrapper_param_parts.push(format!("ptr %__raw_arg{i}"));
                        let loaded = format!("%__loaded_arg{i}");
                        loads.push(format!("  {loaded} = load {ty_str}, ptr %__raw_arg{i}"));
                        forward_arg_parts.push(format!("{ty_str} {loaded}"));
                    } else {
                        wrapper_param_parts.push(format!("{ty_str} %__arg{i}"));
                        forward_arg_parts.push(format!("{ty_str} %__arg{i}"));
                    }
                }
            }
            let wrapper_params_str = wrapper_param_parts.join(", ");
            let forward_args_str = forward_arg_parts.join(", ");

            // Emit the trampoline as a separate top-level function with a
            // fresh FnCtx (#1535).
            self.with_fresh_fn_ctx(orig_ret.clone(), |this| -> Result<(), String> {
                this.fn_ctx.fn_buf.push(format!(
                    "define {define_ret} @{wrapper_name}({wrapper_params_str})"
                ));
                this.fn_ctx.fn_buf.push("{".into());
                this.fn_ctx.fn_buf.push("entry:".into());

                // Emit loads for by-pointer HOF params.
                for load in &loads {
                    this.fn_ctx.fn_buf.push(load.clone());
                }

                if is_void {
                    this.push_instr(&format!("call void @{name}({forward_args_str})"));
                    this.push_instr("ret void");
                } else {
                    let reg = this.next_reg();
                    this.push_instr(&format!(
                        "{reg} = call {llvm_ret} @{name}({forward_args_str})"
                    ));
                    this.push_instr(&format!("ret {llvm_ret} {reg}"));
                }

                this.fn_ctx.fn_buf.push("}".into());
                let wrapper_body = this.fn_ctx.fn_buf.join("\n");
                this.module.fn_bodies.push(wrapper_body);
                Ok(())
            })?;

            // Record wrapper so we don't emit it twice.
            self.module
                .fn_ret_types
                .insert(wrapper_name.clone(), orig_ret);
        }

        // Build `{ &wrapper, null }` closure struct.
        let closure_alloca = self.next_reg();
        self.push_instr(&format!("{closure_alloca} = alloca %__closure_type"));
        self.fn_ctx
            .reg_types
            .insert(closure_alloca.clone(), "ptr".into());

        let fn_field = self.next_reg();
        self.push_instr(&format!(
            "{fn_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 0"
        ));
        self.push_instr(&format!("store ptr @{wrapper_name}, ptr {fn_field}"));

        let env_field = self.next_reg();
        self.push_instr(&format!(
            "{env_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 1"
        ));
        self.push_instr(&format!("store ptr null, ptr {env_field}"));

        Ok(Some(closure_alloca))
    }

    // ── Enum variant lookup helpers (#1200) ────────────────────────────────

    /// Returns true if any variant of `enum_name` has tuple payload fields.
    ///
    /// Payload enums lower to `{ i8, ptr }`; pure unit enums stay as `i64`
    /// discriminants.
    pub(super) fn enum_has_payloads(&self, enum_name: &str) -> bool {
        self.module
            .enum_variant_fields
            .get(enum_name)
            .is_some_and(|vs| vs.iter().any(|f| !f.is_empty()))
    }

    /// Split a qualified variant name `"Type::Variant"` into `(type, variant)`.
    pub(super) fn split_qualified(name: &str) -> Option<(&str, &str)> {
        let pos = name.find("::")?;
        Some((&name[..pos], &name[pos + 2..]))
    }

    /// Look up the tuple payload types for `Type::Variant` (#1200).
    pub(super) fn variant_payload_types(&self, qualified_name: &str) -> Option<&[TypeExpr]> {
        let (type_name, variant_name) = Self::split_qualified(qualified_name)?;
        let names = self.module.enum_variants.get(type_name)?;
        let idx = names.iter().position(|n| n == variant_name)?;
        let fields = self.module.enum_variant_fields.get(type_name)?;
        fields.get(idx).map(|v| v.as_slice())
    }

    /// Resolve a pattern name like "Shape::Circle" to its discriminant i64.
    pub(super) fn pattern_discriminant(&self, name: &str) -> Option<i64> {
        if let Some(pos) = name.find("::") {
            let type_name = &name[..pos];
            let variant_name = &name[pos + 2..];
            if let Some(variants) = self.module.enum_variants.get(type_name) {
                if let Some(idx) = variants.iter().position(|v| v == variant_name) {
                    return Some(idx as i64);
                }
            }
        }
        None
    }

    // ── Literal emission ───────────────────────────────────────────────────

    /// Emit an MVL literal — returns the SSA value (or `None` for `Unit`).
    /// `Literal` is shared between AST and TIR.
    pub(super) fn emit_literal(&mut self, lit: &Literal) -> Result<Option<String>, String> {
        match lit {
            Literal::Integer(n) => Ok(Some(format!("{n}"))),
            Literal::Float(f) => Ok(Some(if f.fract() == 0.0 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            })),
            Literal::Bool(b) => Ok(Some(if *b {
                "true".to_string()
            } else {
                "false".to_string()
            })),
            Literal::Str(s) => Ok(Some(self.emit_string_literal(s))),
            Literal::Unit => Ok(None),
            Literal::Char(c) => Ok(Some(format!("{}", *c as u32))),
        }
    }

    // ── Mangling helpers ───────────────────────────────────────────────────

    /// Sanitize a string segment for use in LLVM IR identifiers.
    pub(super) fn mangle_segment(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Mangle a generic function name with concrete types: `identity` + [Int] → `identity__Int`.
    pub(super) fn mangle_generic(name: &str, concrete: &[TypeExpr]) -> String {
        let suffix: Vec<String> = concrete
            .iter()
            .map(|ty| match ty {
                TypeExpr::Base { name, .. } => Self::mangle_segment(name),
                TypeExpr::Option { inner, .. } => {
                    format!(
                        "Option_{}",
                        Self::mangle_segment(&Self::mangle_type_name(inner))
                    )
                }
                TypeExpr::Result { ok, err, .. } => {
                    format!(
                        "Result_{}_{}",
                        Self::mangle_segment(&Self::mangle_type_name(ok)),
                        Self::mangle_segment(&Self::mangle_type_name(err))
                    )
                }
                _ => "T".into(),
            })
            .collect();
        format!("{}__{}", Self::mangle_segment(name), suffix.join("_"))
    }

    /// Extract a human-readable type name for mangling purposes.
    pub(super) fn mangle_type_name(ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, .. } => name.clone(),
            TypeExpr::Option { inner, .. } => format!("Option_{}", Self::mangle_type_name(inner)),
            TypeExpr::Result { ok, err, .. } => {
                format!(
                    "Result_{}_{}",
                    Self::mangle_type_name(ok),
                    Self::mangle_type_name(err)
                )
            }
            _ => "T".into(),
        }
    }

    // ── Binary operator → LLVM instruction string ─────────────────────────

    /// Emit the LLVM IR for a binary op given operand types and SSA values.
    /// `BinaryOp` is shared between AST and TIR via `crate::mvl::ir`.
    pub(super) fn binary_instr(
        op: &BinaryOp,
        is_float: bool,
        lhs_ty: &str,
        lv: &str,
        rv: &str,
    ) -> String {
        let is_bool = lhs_ty == "i1";
        match op {
            BinaryOp::Add if is_float => format!("fadd double {lv}, {rv}"),
            BinaryOp::Sub if is_float => format!("fsub double {lv}, {rv}"),
            BinaryOp::Mul if is_float => format!("fmul double {lv}, {rv}"),
            BinaryOp::Div if is_float => format!("fdiv double {lv}, {rv}"),
            BinaryOp::Add => format!("add i64 {lv}, {rv}"),
            BinaryOp::Sub => format!("sub i64 {lv}, {rv}"),
            BinaryOp::Mul => format!("mul i64 {lv}, {rv}"),
            BinaryOp::Div => format!("sdiv i64 {lv}, {rv}"),
            BinaryOp::Rem => format!("srem i64 {lv}, {rv}"),
            BinaryOp::Eq if is_float => format!("fcmp oeq double {lv}, {rv}"),
            BinaryOp::Ne if is_float => format!("fcmp one double {lv}, {rv}"),
            BinaryOp::Lt if is_float => format!("fcmp olt double {lv}, {rv}"),
            BinaryOp::Gt if is_float => format!("fcmp ogt double {lv}, {rv}"),
            BinaryOp::Le if is_float => format!("fcmp ole double {lv}, {rv}"),
            BinaryOp::Ge if is_float => format!("fcmp oge double {lv}, {rv}"),
            BinaryOp::Eq if is_bool => format!("icmp eq i1 {lv}, {rv}"),
            BinaryOp::Ne if is_bool => format!("icmp ne i1 {lv}, {rv}"),
            // Use the actual lhs type for comparisons so Byte (i8) and Char (i32)
            // operands don't produce type mismatches (e.g. `icmp eq i64 %i8_val`).
            BinaryOp::Eq => format!("icmp eq {lhs_ty} {lv}, {rv}"),
            BinaryOp::Ne => format!("icmp ne {lhs_ty} {lv}, {rv}"),
            BinaryOp::Lt => format!("icmp slt {lhs_ty} {lv}, {rv}"),
            BinaryOp::Gt => format!("icmp sgt {lhs_ty} {lv}, {rv}"),
            BinaryOp::Le => format!("icmp sle {lhs_ty} {lv}, {rv}"),
            BinaryOp::Ge => format!("icmp sge {lhs_ty} {lv}, {rv}"),
            BinaryOp::BitAnd => format!("and i64 {lv}, {rv}"),
            BinaryOp::BitOr => format!("or i64 {lv}, {rv}"),
            BinaryOp::BitXor => format!("xor i64 {lv}, {rv}"),
            BinaryOp::Shl => format!("shl i64 {lv}, {rv}"),
            BinaryOp::Shr => format!("ashr i64 {lv}, {rv}"),
            BinaryOp::And | BinaryOp::Or => unreachable!("handled before binary_instr"),
        }
    }

    // ── Actor runtime externs (#1149) ──────────────────────────────────────

    /// Emit `declare` statements for actor runtime C-ABI functions, once per
    /// module. Idempotent via `module.actor_runtime_declared`.
    pub(super) fn ensure_actor_runtime_externs(&mut self) {
        if self.module.actor_runtime_declared {
            return;
        }
        self.ensure_extern("declare ptr @_mvl_actor_spawn(ptr, ptr, i64, i64, i64)");
        self.ensure_extern("declare void @_mvl_actor_send(ptr, i64, i64, ptr)");
        self.ensure_extern("declare void @_mvl_actor_drop(ptr)");
        self.ensure_extern("declare ptr @_mvl_actor_self()");
        self.ensure_extern("declare void @_mvl_actor_join_all()");
        self.ensure_extern("declare i64 @_mvl_actor_get_id(ptr)");
        // Link/monitor externs (#1599): ID-based C-ABI matching the MVL surface
        // (`std.actors.{link, unlink, monitor, demonitor}`). Declared via the
        // standard `c_symbols` builtin path; nothing else to declare here.
        self.module.actor_runtime_declared = true;
    }

    // ── String → numeric parse (Result-wrapped) ────────────────────────────

    /// Emit `s.parse_int()` or `s.parse_float()` — calls the C-ABI parser and
    /// wraps the result in a `{ i8, ptr }` Result.
    ///
    /// `ok_llvm_ty` is the LLVM type of the success value (`"i64"` or `"double"`).
    pub(super) fn emit_str_parse(
        &mut self,
        val: &str,
        ok_llvm_ty: &str,
        c_sym: &str,
    ) -> Result<Option<String>, String> {
        let ok_slot = self.next_reg();
        self.push_instr(&format!("{ok_slot} = alloca {ok_llvm_ty}"));
        let err_slot = self.next_reg();
        self.push_instr(&format!("{err_slot} = alloca ptr"));
        self.ensure_extern(&format!("declare i8 @{c_sym}(ptr, ptr, ptr)"));
        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = call i8 @{c_sym}(ptr {val}, ptr {ok_slot}, ptr {err_slot})"
        ));
        self.fn_ctx.reg_types.insert(disc.clone(), "i8".into());
        // Select the correct payload pointer based on discriminant.
        let disc_is_ok = self.next_reg();
        self.push_instr(&format!("{disc_is_ok} = icmp eq i8 {disc}, 0"));
        self.fn_ctx
            .reg_types
            .insert(disc_is_ok.clone(), "i1".into());
        let payload = self.next_reg();
        self.push_instr(&format!(
            "{payload} = select i1 {disc_is_ok}, ptr {ok_slot}, ptr {err_slot}"
        ));
        self.fn_ctx.reg_types.insert(payload.clone(), "ptr".into());
        let r1 = self.wrap_result_pair(&disc, &payload);
        Ok(Some(r1))
    }
}
