// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust type declarations from MVL type declarations.
//!
//! Mappings:
//! - `type Foo = struct { … }` → `pub struct Foo { … }`
//! - `type Bar = enum { … }` → `pub enum Bar { … }`
//! - `type Alias = T` → `pub type Alias = <rust_type>;`
//! - `type Refined = T where pred` → newtype with constructor validation
//! - Security labels (Public<T> etc.) → module-level preamble structs
//! - Refinement field predicates → `assert!` in constructors (always enforced)

use super::emitter::RustEmitter;
use crate::mvl::checker::types::ARRAY_SIZE_UNKNOWN;
use crate::mvl::ir::{
    ArithOp, CmpOp, GenericParam, LogicOp, RefExpr, TirExternDecl, TirFieldDecl, TirTypeBody,
    TirTypeDecl, TirVariant, TirVariantFields, Ty, TypeExpr,
};

// ── Security label preamble ───────────────────────────────────────────────

impl RustEmitter {
    /// Emit the security-label newtype wrappers that every MVL program needs.
    ///
    /// ```rust
    /// #[derive(Debug, Clone, PartialEq)]
    /// pub struct Public<T>(pub T);
    /// // … etc.
    /// ```
    ///
    /// Phase 1: only `From`/`Into` for upward lattice flows are generated.
    /// The lattice is: Tainted → Clean → Public (least trusted to most trusted).
    /// Secret is a separate confidentiality label; only explicit declassify/sanitize
    /// can convert between them.
    pub fn emit_security_preamble(&mut self) {
        self.line("// ── Security label newtypes (MVL Req 11) ─────────────────────────────────");
        self.blank();

        for label in ["Public", "Tainted", "Secret", "Clean"] {
            self.emit_label_newtype(label);
            self.blank();
        }

        // Lattice flows: Tainted → Clean (after sanitize)
        // We express this as a From impl: Clean<T>: From<Tainted<T>> is NOT emitted
        // because sanitize() is an explicit conversion; the Rust type system enforces
        // that you must call sanitize() / declassify() explicitly.
        //
        // Phase 1: conversion functions emitted as standalone fns.
        self.line("/// Sanitize a tainted value — cleans external input.");
        self.line("/// MVL: `sanitize(x)` where x: Tainted<T>");
        self.line("pub fn sanitize<T>(v: Tainted<T>) -> Clean<T> { Clean(v.0) }");
        self.blank();
        self.line("/// Declassify a secret value — makes it public.");
        self.line("/// MVL: `declassify(x)` where x: Secret<T>");
        self.line("pub fn declassify<T>(v: Secret<T>) -> Public<T> { Public(v.0) }");
        self.blank();
        // Numeric conversion helpers for labeled integer/float types
        self.line("impl Public<i64> {");
        self.push_indent();
        self.line("/// Convert labeled integer to raw f64 (for use with Float-typed functions).");
        self.line("pub fn to_float(&self) -> f64 { self.0 as f64 }");
        self.pop_indent();
        self.line("}");
        self.blank();
    }

    fn emit_label_newtype(&mut self, label: &str) {
        self.line("#[derive(Debug, Clone, PartialEq)]");
        self.line(&format!("pub struct {label}<T>(pub T);"));
        self.blank();
        // Copy impl: labels over Copy types are themselves Copy (e.g. Public<i64>)
        self.line(&format!("impl<T: Copy> Copy for {label}<T> {{}}"));
        self.blank();
        self.line(&format!("impl<T> {label}<T> {{"));
        self.push_indent();
        self.line("pub fn new(v: T) -> Self { Self(v) }");
        self.line("pub fn into_inner(self) -> T { self.0 }");
        self.line("pub fn as_inner(&self) -> &T { &self.0 }");
        self.pop_indent();
        self.line("}");
        self.blank();
        // as_str(): enables `match labeled_string.as_str() { "foo" => ... }` in generated code
        self.line(&format!(
            "impl {label}<String> {{ pub fn as_str(&self) -> &str {{ self.0.as_str() }} }}"
        ));
        self.blank();
        // Display: label<T> displays as T when T: Display
        self.line(&format!(
            "impl<T: std::fmt::Display> std::fmt::Display for {label}<T> {{"
        ));
        self.push_indent();
        self.line(
            "fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }",
        );
        self.pop_indent();
        self.line("}");
        self.blank();
        // Arithmetic: delegate ops to the inner value, preserving the label
        for (trait_name, method, op) in [
            ("std::ops::Add", "add", "+"),
            ("std::ops::Sub", "sub", "-"),
            ("std::ops::Mul", "mul", "*"),
            ("std::ops::Div", "div", "/"),
            ("std::ops::Rem", "rem", "%"),
        ] {
            self.line(&format!(
                "impl<T: {trait_name}<Output=T>> {trait_name} for {label}<T> {{"
            ));
            self.push_indent();
            self.line(&format!("type Output = {label}<T>;"));
            self.line(&format!(
                "fn {method}(self, rhs: Self) -> Self {{ {label}(self.0 {op} rhs.0) }}"
            ));
            self.pop_indent();
            self.line("}");
            self.blank();
        }
        self.line(&format!(
            "impl<T: std::ops::Neg<Output=T>> std::ops::Neg for {label}<T> {{"
        ));
        self.push_indent();
        self.line(&format!("type Output = {label}<T>;"));
        self.line(&format!("fn neg(self) -> Self {{ {label}(-self.0) }}"));
        self.pop_indent();
        self.line("}");
    }
}

/// Assert that `name` is a safe Rust identifier before interpolating it into
/// generated source code.  Panics at transpile time (not user runtime) if
/// violated, turning a potential codegen-injection into a loud compiler error.
///
/// Valid: `[a-zA-Z_][a-zA-Z0-9_]*`.  The MVL lexer enforces this for all
/// identifiers produced by parsing, but this assertion also guards AST nodes
/// produced by test helpers, fuzzing harnesses, or future parser extensions.
fn assert_safe_identifier(name: &str) {
    assert!(
        !name.is_empty()
            && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "unsafe identifier in codegen: {name:?}"
    );
}

/// Strip a `Refined { inner, .. }` wrapper, returning the inner type.
/// Returns the type unchanged for all other variants.
fn unwrap_refined_ty(ty: &TypeExpr) -> &TypeExpr {
    match ty {
        TypeExpr::Refined { inner, .. } => unwrap_refined_ty(inner),
        other => other,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

impl RustEmitter {
    fn emit_derive(&mut self, traits: &[&str]) {
        self.line(&format!("#[derive({})]", traits.join(", ")));
    }
}

fn generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = params
        .iter()
        .map(|p| match p {
            GenericParam::Type(name) => name.clone(),
            GenericParam::Const(name, _ty) => format!("const {name}: usize"),
        })
        .collect();
    format!("<{}>", parts.join(", "))
}

// ── Ty → Rust type string ─────────────────────────────────────────────────

/// Convert a resolved MVL [`Ty`] to its Rust representation.
pub fn emit_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "i64".to_string(),
        Ty::Float => "f64".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::String => "String".to_string(),
        Ty::Char => "char".to_string(),
        Ty::Byte | Ty::UByte => "u8".to_string(),
        Ty::UInt => "u64".to_string(),
        Ty::Unit => "()".to_string(),
        Ty::Never => "!".to_string(),
        Ty::Named(name, args) => {
            let rust_name = map_base_type(name);
            if args.is_empty() {
                rust_name.to_string()
            } else {
                let args_str: Vec<String> = args.iter().map(emit_ty).collect();
                format!("{}<{}>", rust_name, args_str.join(", "))
            }
        }
        Ty::Option(inner) => format!("Option<{}>", emit_ty(inner)),
        Ty::Result(ok, err) => format!("Result<{}, {}>", emit_ty(ok), emit_ty(err)),
        Ty::Ref(true, inner) => format!("&mut {}", emit_ty(inner)),
        Ty::Ref(false, inner) => format!("&{}", emit_ty(inner)),
        Ty::Fn(params, ret, _, _) => {
            let params_str: Vec<String> = params.iter().map(emit_ty).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_ty(ret))
        }
        Ty::List(inner) => format!("Vec<{}>", emit_ty(inner)),
        Ty::Array(inner, size) => {
            if *size == ARRAY_SIZE_UNKNOWN {
                format!("Vec<{}>", emit_ty(inner))
            } else {
                format!("[{}; {}]", emit_ty(inner), size)
            }
        }
        Ty::Map(k, v) => format!("std::collections::HashMap<{}, {}>", emit_ty(k), emit_ty(v)),
        Ty::Set(t) => format!("std::collections::HashSet<{}>", emit_ty(t)),
        Ty::Ptr(inner) => match inner.as_ref() {
            Ty::Unit => "*mut std::ffi::c_void".to_string(),
            other => format!("*mut {}", emit_ty(other)),
        },
        Ty::Refined(inner, _) => emit_ty(inner),
        Ty::Labeled(label, inner) => format!("{}<{}>", label, emit_ty(inner)),
        Ty::Session(_) => "/* session type */()".to_string(),
        Ty::Unknown => "()".to_string(),
    }
}

// ── TypeExpr → Rust type string ───────────────────────────────────────────

/// Convert an MVL [`TypeExpr`] to its Rust representation.
pub fn emit_type_expr(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::IntConst { value, .. } => {
            if *value < 0 {
                // Negative array sizes are invalid — emit a sentinel that will produce a clear
                // Rust compile error rather than silently emitting a negative literal.
                "__mvl_invalid_negative_array_size__".to_string()
            } else {
                value.to_string()
            }
        }
        TypeExpr::Base { name, args, .. } => {
            // Array<T, N> → [T; N]
            if name == "Array" && args.len() == 2 {
                let elem = emit_type_expr(&args[0]);
                let size = emit_type_expr(&args[1]);
                return format!("[{elem}; {size}]");
            }
            // Positional<T> is a CLI annotation — transparent at runtime, emit just T
            if name == "Positional" && args.len() == 1 {
                return emit_type_expr(&args[0]);
            }
            // Ptr[T] → *mut T; Ptr[Unit] / Ptr[Void] → *mut std::ffi::c_void.
            // C pointers are mutable by default; *const would prevent write-through (malloc, memcpy).
            if name == "Ptr" && args.len() == 1 {
                let inner = &args[0];
                let is_void =
                    matches!(inner, TypeExpr::Base { name: n, .. } if n == "Unit" || n == "Void");
                return if is_void {
                    "*mut std::ffi::c_void".to_string()
                } else {
                    format!("*mut {}", emit_type_expr(inner))
                };
            }
            let rust_name = map_base_type(name);
            if args.is_empty() {
                rust_name.to_string()
            } else {
                let args_str: Vec<String> = args.iter().map(emit_type_expr).collect();
                format!("{}<{}>", rust_name, args_str.join(", "))
            }
        }
        TypeExpr::Option { inner, .. } => {
            // Option[Positional[T]] → Option<T> (Positional is transparent at runtime)
            let inner_base = unwrap_refined_ty(inner);
            if let TypeExpr::Base { name, args, .. } = inner_base {
                if name == "Positional" && args.len() == 1 {
                    return format!("Option<{}>", emit_type_expr(&args[0]));
                }
            }
            format!("Option<{}>", emit_type_expr(inner))
        }
        TypeExpr::Result { ok, err, .. } => {
            format!("Result<{}, {}>", emit_type_expr(ok), emit_type_expr(err))
        }
        TypeExpr::Ref { mutable, inner, .. } => {
            if *mutable {
                format!("&mut {}", emit_type_expr(inner))
            } else {
                format!("&{}", emit_type_expr(inner))
            }
        }
        TypeExpr::Labeled { label, inner, .. } => {
            format!("{}<{}>", label, emit_type_expr(inner))
        }
        TypeExpr::Refined { inner, .. } => {
            // Erase refinement at the type level; the newtype constructor handles it
            emit_type_expr(inner)
        }
        TypeExpr::Fn { params, ret, .. } => {
            let params_str: Vec<String> = params.iter().map(emit_type_expr).collect();
            format!("fn({}) -> {}", params_str.join(", "), emit_type_expr(ret))
        }
        // Session types are compile-time protocol descriptors with no runtime representation.
        TypeExpr::Session { .. } => "/* session type */()".to_string(),
    }
}

/// Map MVL primitive names to Rust equivalents.
fn map_base_type(name: &str) -> &str {
    match name {
        "Int" => "i64",
        "Float" => "f64",
        "Bool" => "bool",
        "String" => "String",
        "Char" => "char",
        // Both Byte and UByte map to u8. Rust's u8 is unsigned; the signed/unsigned
        // distinction is tracked at the checker level (Ty::Byte vs Ty::UByte) and
        // affects method dispatch (e.g. Byte has `abs`, UByte does not). At the Rust
        // emission level both compile to u8 with wrapping semantics. If Byte needs i8
        // semantics, this mapping would need revisiting (tracked as a follow-up).
        "Byte" => "u8",
        "UByte" => "u8",
        "UInt" => "u64",
        "Unit" => "()",
        "Never" => "!",
        "List" => "Vec",
        "Map" => "std::collections::HashMap",
        "Set" => "std::collections::HashSet",
        // Phase 1: unknown types are passed through as-is (user-defined or external)
        other => other,
    }
}

pub fn emit_label(label: &str) -> &str {
    label
}

// ── TIR TypeDecl emission ─────────────────────────────────────────────────

impl RustEmitter {
    pub fn emit_tir_type_decl(&mut self, td: &TirTypeDecl) {
        match &td.body {
            TirTypeBody::Struct { fields, invariant } => {
                self.emit_tir_struct(&td.name, &td.params, fields, invariant.as_ref());
            }
            TirTypeBody::Enum(variants) => self.emit_tir_enum(&td.name, &td.params, variants),
            TirTypeBody::Alias(ty) => self.emit_tir_alias(&td.name, &td.params, ty),
        }
    }

    fn emit_tir_struct(
        &mut self,
        name: &str,
        params: &[GenericParam],
        fields: &[TirFieldDecl],
        invariant: Option<&RefExpr>,
    ) {
        self.emit_derive(&["Debug", "Clone", "PartialEq"]);
        self.line(&format!("pub struct {}{} {{", name, generic_params(params)));
        self.push_indent();
        for field in fields {
            // MVL `field: ref T` on a struct declares the field mutable when
            // the struct is reached through a `ref` binding — it is NOT a
            // reference-typed field.  Emitting `&mut T` (via `emit_ty`) would
            // require a struct lifetime parameter and force every use of the
            // struct to thread it (E0106).  Strip the wrapper so the field
            // emits as plain `T`; mutability at the use site comes from Rust's
            // ownership on the containing binding (#1707 phase 13).
            let field_ty = match &field.ty {
                crate::mvl::ir::Ty::Ref(_, inner) => inner.as_ref(),
                other => other,
            };
            let ty_str = emit_ty(field_ty);
            self.line(&format!("pub {}: {},", field.name, ty_str));
        }
        self.pop_indent();
        self.line("}");

        let refined_fields: Vec<_> = fields.iter().filter(|f| f.refinement.is_some()).collect();
        if !refined_fields.is_empty() || invariant.is_some() {
            self.blank();
            self.line(&format!(
                "impl{} {}{} {{",
                generic_params(params),
                name,
                generic_params(params)
            ));
            self.push_indent();
            self.line(&format!(
                "/// Construct `{}`, validating all refinement predicates.",
                name
            ));
            let param_list: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, emit_ty(&f.ty)))
                .collect();
            self.line(&format!("pub fn new({}) -> Self {{", param_list.join(", ")));
            self.push_indent();
            for field in &refined_fields {
                if let Some(pred) = &field.refinement {
                    let pred_str = emit_ref_expr_for_assert(pred, &field.name);
                    self.line(&format!(
                        "assert!({pred_str}, \"refinement violated: {} {{}}\", {});",
                        field.name, field.name
                    ));
                }
            }
            let field_inits: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
            if let Some(inv) = invariant {
                self.line(&format!(
                    "let _mvl_val = Self {{ {} }};",
                    field_inits.join(", ")
                ));
                let inv_str = emit_ref_expr_for_assert(inv, "_mvl_val");
                let inv_stmt = match self.assert_mode {
                    crate::mvl::backends::AssertMode::Always => {
                        format!("assert!({inv_str}, \"struct invariant violated for `{name}`\");")
                    }
                    crate::mvl::backends::AssertMode::DebugOnly => {
                        format!(
                            "debug_assert!({inv_str}, \"struct invariant violated for `{name}`\");"
                        )
                    }
                    crate::mvl::backends::AssertMode::Assume => String::new(),
                };
                if !inv_stmt.is_empty() {
                    self.line(&inv_stmt);
                }
                self.line("_mvl_val");
            } else {
                self.line(&format!("Self {{ {} }}", field_inits.join(", ")));
            }
            self.pop_indent();
            self.line("}");
            self.pop_indent();
            self.line("}");
        }
    }

    fn emit_tir_enum(&mut self, name: &str, params: &[GenericParam], variants: &[TirVariant]) {
        self.emit_derive(&["Debug", "Clone", "PartialEq"]);
        self.line(&format!("pub enum {}{} {{", name, generic_params(params)));
        self.push_indent();
        for v in variants {
            match &v.fields {
                TirVariantFields::Unit => self.line(&format!("{},", v.name)),
                TirVariantFields::Tuple(tys) => {
                    let tys_str: Vec<String> = tys.iter().map(emit_ty).collect();
                    self.line(&format!("{}({}),", v.name, tys_str.join(", ")));
                }
                TirVariantFields::Struct(fields) => {
                    self.line(&format!("{} {{", v.name));
                    self.push_indent();
                    for f in fields {
                        let ty_str = emit_ty(&f.ty);
                        self.line(&format!("{}: {},", f.name, ty_str));
                    }
                    self.pop_indent();
                    self.line("},");
                }
            }
        }
        self.pop_indent();
        self.line("}");
    }

    fn emit_tir_alias(&mut self, name: &str, params: &[GenericParam], ty: &Ty) {
        match ty {
            Ty::Refined(inner, pred) => {
                let inner_str = emit_ty(inner);
                if is_copy_primitive_ty(inner) {
                    self.emit_derive(&["Debug", "Clone", "Copy", "PartialEq", "PartialOrd"]);
                } else {
                    self.emit_derive(&["Debug", "Clone", "PartialEq", "PartialOrd"]);
                }
                self.line(&format!(
                    "pub struct {}{}(pub {});",
                    name,
                    generic_params(params),
                    inner_str
                ));
                self.blank();
                self.line(&format!("impl {} {{", name));
                self.push_indent();
                self.line(&format!(
                    "/// Construct `{name}` — panics if the refinement is violated."
                ));
                self.line(&format!("pub fn new(v: {inner_str}) -> Self {{"));
                self.push_indent();
                let pred_str = emit_ref_expr_for_assert(pred, "v");
                self.line(&format!(
                    "assert!({pred_str}, \"refinement violated: {name}({{}})\", v);"
                ));
                self.line("Self(v)");
                self.pop_indent();
                self.line("}");
                self.pop_indent();
                self.line("}");
                // Generate From<Alias> for BaseType so `.into()` unwraps correctly (#1328)
                self.blank();
                self.line(&format!("impl From<{name}> for {inner_str} {{"));
                self.push_indent();
                self.line(&format!("fn from(v: {name}) -> {inner_str} {{"));
                self.push_indent();
                self.line("v.0");
                self.pop_indent();
                self.line("}");
                self.pop_indent();
                self.line("}");
            }
            _ => {
                let ty_str = emit_ty(ty);
                if params.is_empty() {
                    self.line(&format!("pub type {name} = {ty_str};"));
                } else {
                    self.line(&format!(
                        "pub type {}{} = {ty_str};",
                        name,
                        generic_params(params)
                    ));
                }
            }
        }
    }
}

fn is_copy_primitive_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Char | Ty::Byte)
}

impl RustEmitter {
    pub fn emit_tir_extern_decl(&mut self, ed: &TirExternDecl) {
        if self.test_extern_stubs {
            self.line(&format!(
                "// ── extern \"{}\" stubs (test mode) ──────────────────────────────────────────",
                ed.abi
            ));
            for f in &ed.fns {
                if !self.emitted_extern_stub_fns.insert(f.name.clone()) {
                    continue;
                }
                let params_str: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                    .collect();
                let ret_str = emit_ty(&f.ret_ty);
                self.line(&format!(
                    "#[allow(dead_code)] pub fn {}({}) -> {} {{ todo!(\"extern stub\") }}",
                    f.name,
                    params_str.join(", "),
                    ret_str
                ));
            }
            return;
        }

        let new_fns: Vec<_> = ed
            .fns
            .iter()
            .filter(|f| self.register_extern_fn(f.name.clone()))
            .collect();
        if new_fns.is_empty() {
            return;
        }

        self.line(&format!(
            "// ── extern \"{}\" trust boundary ({} fn{}) ──────────────────────────────────",
            ed.abi,
            new_fns.len(),
            if new_fns.len() == 1 { "" } else { "s" }
        ));
        for lib in &ed.link_libs {
            self.line(&format!("#[link(name = \"{lib}\")]"));
        }
        let rust_abi = match ed.abi.as_str() {
            "rust" => "Rust",
            "c" => "C",
            other => {
                self.line(&format!(
                    "// extern \"{other}\" block skipped — unsupported ABI (checker error)"
                ));
                return;
            }
        };
        self.line("#[allow(improper_ctypes)]");
        self.line(&format!("extern \"{rust_abi}\" {{"));
        self.push_indent();
        for f in &new_fns {
            if !f.effects.is_empty() {
                self.line(&format!(
                    "// ! {}",
                    f.effects
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let params_str: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                .collect();
            let ret_str = emit_ty(&f.ret_ty);
            self.line(&format!(
                "fn {}({}) -> {};",
                f.name,
                params_str.join(", "),
                ret_str
            ));
        }
        self.pop_indent();
        self.line("}");
    }
}

// ── Refinement predicate → Rust assert expression ─────────────────────────

/// Return `true` iff `pred` can be evaluated at runtime — i.e. contains no
/// `forall`/`exists` quantifiers anywhere in its subtree.  Quantifiers are
/// ghost-only (verification input only) and must be filtered out before any
/// call to `emit_ref_expr_for_assert`.
pub fn is_runtime_checkable(pred: &RefExpr) -> bool {
    match pred {
        RefExpr::Forall { .. }
        | RefExpr::Exists { .. }
        | RefExpr::BoundedForall { .. }
        | RefExpr::BoundedExists { .. } => false,
        RefExpr::LogicOp { left, right, .. }
        | RefExpr::Compare { left, right, .. }
        | RefExpr::ArithOp { left, right, .. } => {
            is_runtime_checkable(left) && is_runtime_checkable(right)
        }
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. } => is_runtime_checkable(inner),
        RefExpr::FieldAccess { object, .. } => is_runtime_checkable(object),
        RefExpr::Ident { .. }
        | RefExpr::Integer { .. }
        | RefExpr::Float { .. }
        | RefExpr::Bool { .. }
        | RefExpr::Len { .. } => true,
        RefExpr::BitwiseOp { left, right, .. } => {
            is_runtime_checkable(left) && is_runtime_checkable(right)
        }
        RefExpr::BitwiseNot { inner, .. } => is_runtime_checkable(inner),
    }
}

/// Emit a refinement predicate as a Rust boolean expression suitable for
/// use inside `assert!(…)`.  The binding name `self` is replaced with
/// `binding`.
///
/// Caller must ensure `pred` is runtime-checkable (see
/// [`is_runtime_checkable`]); passing a quantifier will panic.
pub fn emit_ref_expr_for_assert(pred: &RefExpr, binding: &str) -> String {
    emit_ref_expr(pred, binding)
}

fn emit_ref_expr(pred: &RefExpr, binding: &str) -> String {
    match pred {
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                LogicOp::And => "&&",
                LogicOp::Or => "||",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::Compare {
            op, left, right, ..
        } => {
            let op_str = match op {
                CmpOp::Eq => "==",
                CmpOp::Ne => "!=",
                CmpOp::Lt => "<",
                CmpOp::Gt => ">",
                CmpOp::Le => "<=",
                CmpOp::Ge => ">=",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
            let op_str = match op {
                ArithOp::Add => "+",
                ArithOp::Sub => "-",
                ArithOp::Mul => "*",
                ArithOp::Div => "/",
                ArithOp::Rem => "%",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::Not { inner, .. } => format!("!{}", emit_ref_expr(inner, binding)),
        RefExpr::Ident { name, .. } => {
            if name == "self" || name == "result" {
                binding.to_string()
            } else {
                name.clone()
            }
        }
        RefExpr::FieldAccess { object, field, .. } => {
            assert_safe_identifier(field);
            format!("{}.{}", emit_ref_expr(object, binding), field)
        }
        RefExpr::Integer { value, .. } => value.to_string(),
        RefExpr::Float { value, .. } => {
            let s = format!("{value}");
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        RefExpr::Bool { value, .. } => value.to_string(),
        RefExpr::Len { ident, .. } => {
            if ident == "self" || ident == "result" {
                format!("{binding}.len()")
            } else {
                format!("{ident}.len()")
            }
        }
        RefExpr::Grouped { inner, .. } => format!("({})", emit_ref_expr(inner, binding)),
        // old(e) in ensures: for runtime assertion purposes, treat as the current value.
        // Full entry-time capture is a future enhancement.
        RefExpr::Old { inner, .. } => emit_ref_expr(inner, binding),
        // Quantifiers are ghost-only and erased before codegen; unreachable here.
        RefExpr::Forall { .. }
        | RefExpr::Exists { .. }
        | RefExpr::BoundedForall { .. }
        | RefExpr::BoundedExists { .. } => {
            unreachable!("quantifiers are ghost-only and must not appear in codegen")
        }
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => {
            use crate::mvl::ir::BitwiseOp;
            let op_str = match op {
                BitwiseOp::And => "&",
                BitwiseOp::Or => "|",
                BitwiseOp::Xor => "^",
                BitwiseOp::Shl => "<<",
                BitwiseOp::Shr => ">>",
            };
            format!(
                "({} {op_str} {})",
                emit_ref_expr(left, binding),
                emit_ref_expr(right, binding)
            )
        }
        RefExpr::BitwiseNot { inner, .. } => format!("(!{})", emit_ref_expr(inner, binding)),
    }
}

// ── Float literal in refinement ───────────────────────────────────────────
// Note: RefExpr::Integer covers integer constants; float literals in
// refinements (e.g. `self >= 0.0`) are not yet in the RefExpr grammar —
// they would need to be added as RefExpr::Float. For now, integer literals
// cover most practical predicates.
