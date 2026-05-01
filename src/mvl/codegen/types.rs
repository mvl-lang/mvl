//! Type registration, LLVM type building, and MVL→LLVM type mapping.
//!
//! Phase B (L5-05, L5-06): struct and enum type knowledge used by the rest of
//! the backend.  Separated here so expression and statement emitters can share
//! type-lookup helpers without depending on each other.

use inkwell::{types::BasicTypeEnum, AddressSpace};

use crate::mvl::parser::ast::{Expr, TypeBody, TypeDecl, TypeExpr, VariantFields};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── Phase B: type registration ───────────────────────────────────────────

    pub(crate) fn register_type_decl(&mut self, td: &TypeDecl) {
        match &td.body {
            TypeBody::Struct(fields) => {
                self.struct_fields.insert(
                    td.name.clone(),
                    fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect(),
                );
            }
            TypeBody::Enum(variants) => {
                self.enum_variants.insert(
                    td.name.clone(),
                    variants
                        .iter()
                        .map(|v| (v.name.clone(), v.fields.clone()))
                        .collect(),
                );
            }
            TypeBody::Alias(_) => {}
        }
    }

    /// Create LLVM types for all registered MVL struct and enum types.
    ///
    /// Two passes: first create all opaque named types, then set their bodies.
    /// This handles forward references between types.
    pub(crate) fn build_llvm_types(&mut self) {
        // Pass 1: create all opaque struct types.
        let struct_names: Vec<String> = self.struct_fields.keys().cloned().collect();
        for name in &struct_names {
            let ty = self.context.opaque_struct_type(name);
            self.llvm_struct_types.insert(name.clone(), ty);
        }
        let enum_names: Vec<String> = self.enum_variants.keys().cloned().collect();
        for name in &enum_names {
            let variants = self.enum_variants[name].clone();
            if !Self::is_unit_enum_variants(&variants) {
                let ty = self.context.opaque_struct_type(name);
                self.llvm_struct_types.insert(name.clone(), ty);
            }
        }

        // Pass 2: set struct bodies.
        for name in &struct_names {
            let fields: Vec<(String, TypeExpr)> = self.struct_fields[name].clone();
            let field_types: Vec<BasicTypeEnum<'ctx>> = fields
                .iter()
                .filter_map(|(_, ty)| self.mvl_type_to_llvm(ty))
                .collect();
            let st = self.llvm_struct_types[name];
            st.set_body(&field_types, false);
        }

        // Pass 2: set enum bodies (tagged unions).
        for name in &enum_names {
            let variants = self.enum_variants[name].clone();
            if !Self::is_unit_enum_variants(&variants) {
                let max_size = Self::max_variant_payload_size_static(&variants);
                let st = self.llvm_struct_types[name];
                let disc_ty: BasicTypeEnum = self.context.i8_type().into();
                // Payload: [max_size × i8], minimum 1 byte.
                let payload_len = max_size.max(1) as u32;
                let payload_ty: BasicTypeEnum =
                    self.context.i8_type().array_type(payload_len).into();
                st.set_body(&[disc_ty, payload_ty], false);
            }
        }
    }

    pub(crate) fn is_unit_enum_variants(variants: &[(String, VariantFields)]) -> bool {
        variants
            .iter()
            .all(|(_, f)| matches!(f, VariantFields::Unit))
    }

    pub(crate) fn max_variant_payload_size_static(variants: &[(String, VariantFields)]) -> usize {
        variants
            .iter()
            .map(|(_, f)| Self::variant_payload_size_static(f))
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn variant_payload_size_static(fields: &VariantFields) -> usize {
        match fields {
            VariantFields::Unit => 0,
            VariantFields::Tuple(types) => types.iter().map(Self::type_size_bytes_static).sum(),
            VariantFields::Struct(fields) => fields
                .iter()
                .map(|f| Self::type_size_bytes_static(&f.ty))
                .sum(),
        }
    }

    pub(crate) fn type_size_bytes_static(ty: &TypeExpr) -> usize {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "Bool" | "Byte" => 1,
                "Char" => 4,
                _ => 8, // Int, Float, String (ptr), unknown
            },
            _ => 8,
        }
    }

    // ── Type mapping (L5-04 + L5-05 + L5-06) ────────────────────────────────

    /// Map a MVL TypeExpr to an LLVM BasicTypeEnum.
    /// Returns None for the `Unit` / void type.
    pub(crate) fn mvl_type_to_llvm(&self, ty: &TypeExpr) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            TypeExpr::Base { name, args, .. } => {
                // L5-08: if this bare name is an active type-parameter substitution, use it.
                if args.is_empty() {
                    if let Some(&sub_ty) = self.type_subs.get(name.as_str()) {
                        return Some(sub_ty);
                    }
                }
                match name.as_str() {
                    "Int" => Some(self.context.i64_type().into()),
                    "Float" => Some(self.context.f64_type().into()),
                    "Bool" => Some(self.context.bool_type().into()),
                    "Byte" => Some(self.context.i8_type().into()),
                    "Char" => Some(self.context.i32_type().into()),
                    "Unit" => None,
                    "String" => Some(self.context.ptr_type(AddressSpace::default()).into()),
                    _ => {
                        // Known struct type → %StructName
                        if let Some(&st) = self.llvm_struct_types.get(name.as_str()) {
                            return Some(st.into());
                        }
                        // Known unit enum → i8 discriminant
                        if let Some(variants) = self.enum_variants.get(name.as_str()) {
                            if Self::is_unit_enum_variants(variants) {
                                return Some(self.context.i8_type().into());
                            }
                        }
                        // Known payload enum → %EnumName
                        if let Some(&st) = self.llvm_struct_types.get(name.as_str()) {
                            return Some(st.into());
                        }
                        // Unknown: fall back to i64
                        Some(self.context.i64_type().into())
                    }
                }
            }
            // Ref types fall back to ptr
            TypeExpr::Ref { .. } => Some(self.context.ptr_type(AddressSpace::default()).into()),
            // Security labels (Public[T], Secret[T], Tainted[T]) and refinements
            // (T where pred) are transparent: use the inner type.
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                self.mvl_type_to_llvm(inner)
            }
            // Result[T, E] and Option[T]: pointer-based tagged union { i8, ptr }.
            // The payload is stored by pointer so any T size is supported (L5-08).
            TypeExpr::Result { .. } | TypeExpr::Option { .. } => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                Some(
                    self.context
                        .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false)
                        .into(),
                )
            }
            // Generic / compound types: i64 placeholder for Phase B
            _ => Some(self.context.i64_type().into()),
        }
    }

    pub(crate) fn is_unit_type(&self, ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Base { name, .. } if name == "Unit")
    }

    /// Peel `Labeled { inner }` and `Refined { inner }` wrappers recursively.
    pub(crate) fn strip_type_wrappers(ty: &TypeExpr) -> &TypeExpr {
        match ty {
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                Self::strip_type_wrappers(inner)
            }
            other => other,
        }
    }

    /// Given an expression whose value is a `Result[T,E]` or `Option[T]`, return
    /// the LLVM type of the Ok/Some payload.  Falls back to `i64` if unknown.
    pub(crate) fn infer_result_ok_llvm_ty(&self, expr: &Expr) -> BasicTypeEnum<'ctx> {
        // Look up the MVL return/annotation type for this expression.
        let mvl_ty: Option<&TypeExpr> = match expr {
            Expr::FnCall { name, .. } => self.fn_return_types.get(name.as_str()),
            // L5-08: for local variable scrutinees, use the annotation stored at let-binding.
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()),
            _ => None,
        };
        if let Some(ret_ty) = mvl_ty {
            let inner = Self::strip_type_wrappers(ret_ty);
            let payload_ty = match inner {
                TypeExpr::Result { ok, .. } => Some(ok.as_ref()),
                TypeExpr::Option {
                    inner: opt_inner, ..
                } => Some(opt_inner.as_ref()),
                _ => None,
            };
            if let Some(pt) = payload_ty {
                if let Some(llvm_ty) = self.mvl_type_to_llvm(Self::strip_type_wrappers(pt)) {
                    return llvm_ty;
                }
            }
        }
        self.context.i64_type().into()
    }
}
