//! Generic C-ABI stdlib dispatch for the LLVM backend (ADR-0018).
//!
//! Replaces per-function getter boilerplate with a single `emit_stdlib_call`
//! that derives the C symbol name and LLVM types from the typed stdlib `FnDecl`
//! metadata passed into `compile_to_ir`.
//!
//! # How to add a new stdlib function
//!
//! 1. Export the C-ABI wrapper in `mvl_runtime_c/src/stdlib/<module>.rs`
//! 2. No changes needed here — `emit_stdlib_call` derives everything from the
//!    `StdlibFnInfo` map built in `main.rs`.

use inkwell::{
    types::{BasicMetadataTypeEnum, BasicTypeEnum},
    values::{BasicMetadataValueEnum, BasicValueEnum},
    AddressSpace,
};

use crate::mvl::parser::ast::{Expr, TypeExpr};

use super::{LlvmBackend, StdlibFnInfo};

impl<'ctx> LlvmBackend<'ctx> {
    /// Emit a call to a stdlib function via the C-ABI runtime (`libmvl_runtime_c`).
    ///
    /// Derives the C symbol name as `_mvl_{module}_{fn_name}`, looks up or
    /// declares the function with types derived from the `StdlibFnInfo`, and
    /// emits the call.
    ///
    /// For functions that return `Never` (e.g. `exit`), also emits `unreachable`
    /// and sets `self.terminated = true`.
    ///
    /// Returns `None` for void/Never returns or if a type cannot be lowered.
    pub(crate) fn emit_stdlib_call(
        &mut self,
        fn_name: &str,
        info: &StdlibFnInfo,
        args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let c_symbol = format!("_mvl_{}_{}", info.module, fn_name);

        let param_tys: Vec<BasicMetadataTypeEnum<'ctx>> = info
            .params
            .iter()
            .filter_map(|ty| self.stdlib_type_to_llvm_meta(ty))
            .collect();

        let ret_llvm = self.stdlib_return_type(&info.return_type);
        let is_never = Self::is_never_type(&info.return_type);

        let f = self.get_or_declare_fn(&c_symbol, &param_tys, ret_llvm, false);

        let arg_vals: Vec<BasicValueEnum<'ctx>> =
            args.iter().filter_map(|a| self.emit_expr(a)).collect();

        let call_args: Vec<BasicMetadataValueEnum<'ctx>> =
            arg_vals.iter().map(|v| (*v).into()).collect();

        let call = self
            .builder
            .build_call(f, &call_args, &format!("{fn_name}_result"))
            .ok()?;

        if is_never {
            let _ = self.builder.build_unreachable();
            self.terminated = true;
            return None;
        }

        ret_llvm?;

        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Map a MVL `TypeExpr` to an LLVM metadata type for use as a C-ABI parameter.
    ///
    /// Only the primitive types that map cleanly to C scalar types are supported.
    /// `Labeled`/`Refined` wrappers are unwrapped to their inner type.
    /// Unsupported types (Option, Result, complex generics) return `None`.
    fn stdlib_type_to_llvm_meta(&self, ty: &TypeExpr) -> Option<BasicMetadataTypeEnum<'ctx>> {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "Int" => Some(self.context.i64_type().into()),
                "Float" => Some(self.context.f64_type().into()),
                "Bool" => Some(self.context.bool_type().into()),
                "Byte" => Some(self.context.i8_type().into()),
                "Char" => Some(self.context.i32_type().into()),
                "String" => Some(self.context.ptr_type(AddressSpace::default()).into()),
                _ => None,
            },
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                self.stdlib_type_to_llvm_meta(inner)
            }
            TypeExpr::Ref { inner, .. } => self.stdlib_type_to_llvm_meta(inner),
            _ => None,
        }
    }

    /// Map a MVL `TypeExpr` return type to an LLVM basic type.
    ///
    /// Returns `None` for `Unit`, `Never`, and unsupported types (void return).
    fn stdlib_return_type(&self, ty: &TypeExpr) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "Int" => Some(self.context.i64_type().into()),
                "Float" => Some(self.context.f64_type().into()),
                "Bool" => Some(self.context.bool_type().into()),
                "Byte" => Some(self.context.i8_type().into()),
                "Char" => Some(self.context.i32_type().into()),
                "String" => Some(self.context.ptr_type(AddressSpace::default()).into()),
                "Unit" | "Never" => None,
                _ => None,
            },
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                self.stdlib_return_type(inner)
            }
            TypeExpr::Ref { inner, .. } => self.stdlib_return_type(inner),
            _ => None,
        }
    }

    /// Check whether a MVL type is `Never` (diverging — no return value).
    fn is_never_type(ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Base { name, .. } if name == "Never")
    }
}
