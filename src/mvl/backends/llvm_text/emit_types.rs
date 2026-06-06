// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Type mapping, classification, and helper emitters for the `llvm_text` backend.

use crate::mvl::parser::ast::{BinaryOp, Expr, Literal, Stmt, TypeExpr, UnaryOp};

use super::{HeapKind, TextEmitter, RESULT_LLVM_TY};

impl TextEmitter {
    // ── Heap drop emission (#1185) ─────────────────────────────────────

    /// Remove the heap-local entry for a value that is about to be returned
    /// (moved out of the function), preventing `emit_heap_drops` from freeing it.
    pub(super) fn exclude_returned_value(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name, _) => {
                // Check ref locals first — the alloca ptr is tracked in heap_locals.
                if let Some(loc) = self.ref_locals.get(name) {
                    let ptr = loc.ptr.clone();
                    self.heap_locals.retain(|(s, _, _)| *s != ptr);
                    return;
                }
                // Check regular (non-ref) locals — the SSA itself is tracked.
                if let Some(ssa) = self.locals.get(name) {
                    let ssa = ssa.clone();
                    self.heap_locals.retain(|(s, _, _)| *s != ssa);
                }
            }
            // Consume / Relabel are transparent wrappers — recurse.
            Expr::Consume { expr: inner, .. } | Expr::Relabel { expr: inner, .. } => {
                self.exclude_returned_value(inner);
            }
            _ => {}
        }
    }

    /// Emit `mvl_*_drop` calls for all tracked heap locals.
    /// Called before every `ret` instruction to clean up owned allocations.
    pub(super) fn emit_heap_drops(&mut self) {
        for (ssa, kind, is_ref) in self.heap_locals.clone() {
            let sym = match kind {
                HeapKind::String => "mvl_string_drop",
                HeapKind::Array => "mvl_array_drop",
                HeapKind::Map => "mvl_map_drop",
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
        if !self.has_println_fmt {
            self.str_globals.push(
                "@println_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0a\\00\"".into(),
            );
            self.has_println_fmt = true;
        }
        "println_fmt"
    }

    pub(super) fn ensure_int_fmt(&mut self) -> &'static str {
        if !self.has_int_fmt {
            self.str_globals
                .push("@int_fmt = private unnamed_addr constant [5 x i8] c\"%lld\\00\"".into());
            self.has_int_fmt = true;
        }
        "int_fmt"
    }

    pub(super) fn ensure_bool_str_globals(&mut self) -> (&'static str, &'static str) {
        if !self.has_str_true {
            self.str_globals
                .push("@str_true = private unnamed_addr constant [5 x i8] c\"true\\00\"".into());
            self.has_str_true = true;
        }
        if !self.has_str_false {
            self.str_globals
                .push("@str_false = private unnamed_addr constant [6 x i8] c\"false\\00\"".into());
            self.has_str_false = true;
        }
        ("str_true", "str_false")
    }

    /// Create a module-level string constant from raw bytes.
    /// Returns the global name (without `@`).
    pub(super) fn emit_str_global(&mut self, s: &str) -> String {
        let name = format!("str.{}", self.str_counter);
        self.str_counter += 1;
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
        self.str_globals.push(format!(
            "@{name} = private unnamed_addr constant [{total_len} x i8] c\"{escaped}\\00\""
        ));
        name
    }

    /// Emit instructions to create a heap `MvlString*` from a Rust string literal.
    /// Returns the SSA register (type: ptr).
    pub(super) fn emit_string_literal(&mut self, s: &str) -> String {
        let global = self.emit_str_global(s);
        let len = s.len();
        self.ensure_extern("declare ptr @mvl_string_new(ptr, i64)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @mvl_string_new(ptr @{global}, i64 {len})"
        ));
        self.reg_types.insert(reg.clone(), "ptr".into());
        reg
    }

    // ── Type helpers ──────────────────────────────────────────────────────

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
            TypeExpr::Ref {
                mutable: true,
                inner,
                ..
            } => Self::llvm_ty(inner),
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
                if let Some(concrete) = self.type_param_map.get(name.as_str()) {
                    return self.llvm_ty_ctx(concrete);
                }
                if self.struct_fields.contains_key(name) {
                    // Actor state structs are always accessed via pointer — the
                    // actor handle is an opaque ptr, not an inline struct value.
                    if self.actor_decls.contains_key(name.as_str()) {
                        return "ptr".to_string();
                    }
                    return format!("%{name}");
                }
                if self.enum_variants.contains_key(name) {
                    // Payload enums lower to `{ i8, ptr }`; pure unit enums stay i64 (#1200).
                    if self.enum_has_payloads(name) {
                        return RESULT_LLVM_TY.to_string();
                    }
                    return "i64".to_string();
                }
                // Actor type without registered state struct (e.g. handle as field).
                if self.actor_decls.contains_key(name.as_str()) {
                    return "ptr".to_string();
                }
                Self::llvm_ty(ty)
            }
            TypeExpr::Ref {
                mutable: true,
                inner,
                ..
            } => self.llvm_ty_ctx(inner),
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

    /// Infer the LLVM type of an expression without emitting instructions.
    pub(super) fn type_of_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Literal(Literal::Integer(_), _) => "i64".into(),
            Expr::Literal(Literal::Float(_), _) => "double".into(),
            Expr::Literal(Literal::Bool(_), _) => "i1".into(),
            Expr::Literal(Literal::Str(_), _) => "ptr".into(),
            Expr::Literal(Literal::Unit, _) => "void".into(),
            Expr::Ident(name, _) => {
                if name == "None" {
                    return RESULT_LLVM_TY.into();
                }
                // Qualified enum variant "Type::Variant"
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = &name[..pos];
                        if self.enum_variants.contains_key(type_name) {
                            if self.enum_has_payloads(type_name) {
                                return RESULT_LLVM_TY.into();
                            }
                            return "i64".into();
                        }
                    }
                }
                if let Some(loc) = self.ref_locals.get(name) {
                    return self.llvm_ty_ctx(&loc.elem_ty);
                }
                if let Some(TypeExpr::Base { name: tn, .. }) = self.local_mvl_types.get(name) {
                    let tn = tn.clone();
                    if self.struct_fields.contains_key(&tn) {
                        return format!("%{tn}");
                    }
                    if self.enum_variants.contains_key(&tn) {
                        if self.enum_has_payloads(&tn) {
                            return RESULT_LLVM_TY.into();
                        }
                        return "i64".into();
                    }
                }
                if let Some(mvl_ty) = self.local_mvl_types.get(name) {
                    return Self::llvm_ty(mvl_ty);
                }
                if let Some(ssa) = self.locals.get(name) {
                    if let Some(ty) = self.reg_types.get(ssa) {
                        return ty.clone();
                    }
                }
                "i64".into()
            }
            Expr::Binary {
                op:
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Gt
                    | BinaryOp::Le
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or,
                ..
            } => "i1".into(),
            Expr::Binary { .. } => "i64".into(),
            Expr::Unary {
                op: UnaryOp::Not, ..
            } => "i1".into(),
            Expr::FnCall { name, .. } => {
                // Enum variant constructor
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = &name[..pos];
                        if self.enum_variants.contains_key(type_name) {
                            if self.enum_has_payloads(type_name) {
                                return RESULT_LLVM_TY.into();
                            }
                            return "i64".into();
                        }
                    }
                }
                match name.as_str() {
                    "assert" | "println" | "print" | "eprintln" => "void".into(),
                    "format" => "ptr".into(),
                    "Some" | "None" | "Ok" | "Err" => RESULT_LLVM_TY.into(),
                    // Stdlib C-ABI dispatch functions whose return types are
                    // stripped from fn_ret_types by the prelude filter (#1202).
                    "path" | "format_datetime" | "format_instant" | "find_all" | "replace" => {
                        "ptr".into()
                    }
                    _ => {
                        if let Some(ret) = self.fn_ret_types.get(name) {
                            self.llvm_ty_ctx(ret)
                        } else {
                            "i64".into()
                        }
                    }
                }
            }
            Expr::MethodCall {
                method,
                receiver,
                args: margs,
                ..
            } => match method.as_str() {
                "to_string" | "concat" | "to_lower" | "to_upper" | "trim" => "ptr".into(),
                "len" => "i64".into(),
                "is_some" | "is_none" => "i1".into(),
                "unwrap_or" => {
                    if let Some(a) = margs.first() {
                        self.type_of_expr(a)
                    } else {
                        "i64".into()
                    }
                }
                "get" if matches!(self.mvl_receiver_kind(receiver), Some("List") | Some("Map")) => {
                    "{ i8, ptr }".into()
                }
                "first" | "last" => "{ i8, ptr }".into(),
                _ => "ptr".into(),
            },
            Expr::Construct { name, .. } => {
                if self.struct_fields.contains_key(name) {
                    format!("%{name}")
                } else {
                    "ptr".into()
                }
            }
            Expr::FieldAccess { .. } => "i64".into(), // default; refined in emit_field_access
            Expr::List { .. } | Expr::Map { .. } | Expr::Set { .. } => "ptr".into(),
            Expr::Consume { expr, .. } | Expr::Relabel { expr, .. } => self.type_of_expr(expr),
            Expr::Unary {
                op: UnaryOp::Deref,
                expr: inner,
                ..
            } => self
                .box_inner_llvm_ty(inner)
                .unwrap_or_else(|| "i64".into()),
            Expr::If { then, .. } => {
                // Use the type of the last expression in `then`
                if let Some(Stmt::Expr { expr, .. }) = then.stmts.last() {
                    return self.type_of_expr(expr);
                }
                "i64".into()
            }
            // A lambda expression is a closure pointer.
            Expr::Lambda { .. } => "ptr".into(),
            // A spawn expression produces an opaque actor handle pointer.
            Expr::Spawn { .. } => "ptr".into(),
            _ => "i64".into(),
        }
    }

    /// Infer the LLVM type from an already-emitted SSA value string.
    pub(super) fn infer_val_type(&self, val: &str) -> String {
        if val.starts_with('%') {
            self.reg_types
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

    /// Look up the struct type name (e.g. "Point") of an expression, if known.
    pub(super) fn struct_name_of_expr(&self, expr: &Expr) -> Option<String> {
        if let Expr::Ident(name, _) = expr {
            if let Some(TypeExpr::Base { name: tn, .. }) = self.local_mvl_types.get(name) {
                if self.struct_fields.contains_key(tn) {
                    return Some(tn.clone());
                }
            }
        }
        None
    }

    /// Return the MVL base type name of a receiver expression when it can be
    /// determined statically from `local_mvl_types`.
    ///
    /// Returns `"String"`, `"List"`, `"Map"`, `"Set"`, etc.  Returns `None`
    /// when the type is unknown (e.g. an SSA value with no MVL annotation).
    pub(super) fn mvl_receiver_kind(&self, expr: &Expr) -> Option<&str> {
        match expr {
            Expr::Literal(Literal::Str(_), _) => Some("String"),
            Expr::Ident(name, _) => {
                let mvl_ty = self.local_mvl_types.get(name.as_str())?;
                match mvl_ty {
                    TypeExpr::Base { name: tn, .. } => Some(tn.as_str()),
                    TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                        if let TypeExpr::Base { name: tn, .. } = inner.as_ref() {
                            Some(tn.as_str())
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            // Relabel/Consume strip the IFC label — recurse into the inner expr.
            Expr::Relabel { expr: inner, .. } | Expr::Consume { expr: inner, .. } => {
                self.mvl_receiver_kind(inner)
            }
            _ => None,
        }
    }

    /// True if `receiver`'s MVL type is `List`, `Array`, or `Set` — used to
    /// guard dispatch arms that lower to the shared `_mvl_list_slice` runtime.
    pub(super) fn is_list_array_set(&self, receiver: &Expr) -> bool {
        matches!(
            self.mvl_receiver_kind(receiver),
            Some("List") | Some("Array") | Some("Set")
        )
    }

    /// Emit `call ptr @_mvl_list_slice(ptr val, i64 start, i64 end)` and
    /// return the result register. Shared by slice/take/skip dispatch.
    pub(super) fn emit_list_slice_call(&mut self, val: &str, start: &str, end: &str) -> String {
        self.ensure_extern("declare ptr @_mvl_list_slice(ptr, i64, i64)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @_mvl_list_slice(ptr {val}, i64 {start}, i64 {end})"
        ));
        self.reg_types.insert(reg.clone(), "ptr".into());
        reg
    }

    /// If `expr` has MVL type `Box[T]`, return the LLVM IR type of `T`.
    /// Used to emit `load T, ptr %box` when emitting `*box` deref (#1154).
    pub(super) fn box_inner_llvm_ty(&self, expr: &Expr) -> Option<String> {
        let mvl_ty: TypeExpr = match expr {
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned()?,
            Expr::FieldAccess {
                expr: receiver,
                field,
                ..
            } => {
                if let Expr::Ident(name, _) = receiver.as_ref() {
                    let recv_ty = self.local_mvl_types.get(name.as_str())?;
                    if let TypeExpr::Base { name: tn, .. } = recv_ty {
                        let fields = self.struct_fields.get(tn)?;
                        fields
                            .iter()
                            .find(|(fname, _)| fname == field)
                            .map(|(_, ty)| ty.clone())?
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        if let TypeExpr::Base { name, args, .. } = mvl_ty {
            if name == "Box" && args.len() == 1 {
                return Some(self.llvm_ty_ctx(&args[0]));
            }
        }
        None
    }

    // ── Int/Bool → String helpers ─────────────────────────────────────────

    pub(super) fn emit_int_to_string(&mut self, val: &str) -> String {
        let int_fmt = self.ensure_int_fmt();
        self.ensure_extern("declare i32 @snprintf(ptr, i64, ptr, ...)");
        self.ensure_extern("declare ptr @mvl_string_new(ptr, i64)");
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
            "{str_reg} = call ptr @mvl_string_new(ptr {buf}, i64 {len})"
        ));
        self.reg_types.insert(str_reg.clone(), "ptr".into());
        str_reg
    }

    pub(super) fn emit_bool_to_string(&mut self, val: &str) -> String {
        let (t, f) = self.ensure_bool_str_globals();
        self.ensure_extern("declare ptr @mvl_string_new(ptr, i64)");
        let cptr = self.next_reg();
        self.push_instr(&format!("{cptr} = select i1 {val}, ptr @{t}, ptr @{f}"));
        let clen = self.next_reg();
        self.push_instr(&format!("{clen} = select i1 {val}, i64 4, i64 5"));
        let str_reg = self.next_reg();
        self.push_instr(&format!(
            "{str_reg} = call ptr @mvl_string_new(ptr {cptr}, i64 {clen})"
        ));
        self.reg_types.insert(str_reg.clone(), "ptr".into());
        str_reg
    }
}
