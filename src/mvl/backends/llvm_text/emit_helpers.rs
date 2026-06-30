// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Shared low-level emit helpers — type mapping, heap-drop tracking,
//! string globals, and value-conversion routines used by both the
//! (soon-to-be-deleted) AST walker and the TIR walker.
//!
//! Extracted from `emit_types.rs` in #1612 Phase 3b PR 2 prep. The
//! AST-walking functions that take `&Expr` / `&Stmt` stay in
//! `emit_types.rs` (which PR 2 deletes wholesale); the helpers here
//! are AST-shape-agnostic and survive the deletion.

use crate::mvl::checker::types::Ty;
use crate::mvl::parser::ast::TypeExpr;

use super::{HeapKind, TextEmitter, RESULT_LLVM_TY};

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
        let extras: Vec<_> = self.fn_ctx.heap_locals.drain(snapshot_len..).collect();
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
                    if self.module.actor_decls.contains_key(name.as_str()) {
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
                if self.module.actor_decls.contains_key(name.as_str()) {
                    return "ptr".into();
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
                "Unit" => "void".to_string(),
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
                    if self.module.actor_decls.contains_key(name.as_str()) {
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
                if self.module.actor_decls.contains_key(name.as_str()) {
                    return "ptr".to_string();
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
    /// null payload.
    pub(super) fn emit_none_constructor(&mut self) -> Result<Option<String>, String> {
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca i8"));
        let r1 = self.wrap_result_pair("1", &slot);
        Ok(Some(r1))
    }
}
