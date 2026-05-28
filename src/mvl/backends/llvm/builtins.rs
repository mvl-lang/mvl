// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Built-in function emission for the MVL LLVM backend.
//!
//! Covers libc wrappers (dprintf, snprintf, strlen), collection literals
//! (List, Map, Set), and the `format` built-in (L5-17).

use inkwell::{
    module::Linkage,
    types::BasicMetadataTypeEnum,
    values::{BasicValueEnum, FunctionValue},
    AddressSpace,
};

use crate::mvl::parser::ast::{Expr, Literal};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── dprintf declaration ───────────────────────────────────────────────────

    /// Get (or lazily declare) the POSIX `dprintf(fd, fmt, ...)` function.
    ///
    /// Used by `emit_eprintln` to write to stderr (fd = 2)
    /// without requiring a FILE* pointer.
    pub(crate) fn get_dprintf(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("dprintf") {
            return f;
        }
        let i32_ty: BasicMetadataTypeEnum = self.context.i32_type().into();
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let dprintf_ty = self.context.i32_type().fn_type(&[i32_ty, ptr_ty], true);
        self.module
            .add_function("dprintf", dprintf_ty, Some(Linkage::External))
    }

    pub(crate) fn get_snprintf(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("snprintf") {
            return f;
        }
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let i64_ty: BasicMetadataTypeEnum = self.context.i64_type().into();
        let snprintf_ty = self
            .context
            .i32_type()
            .fn_type(&[ptr_ty, i64_ty, ptr_ty], true);
        self.module
            .add_function("snprintf", snprintf_ty, Some(Linkage::External))
    }

    // ── String conversion helpers ─────────────────────────────────────────────

    /// Emit `snprintf` into a stack buffer, then wrap the result in a heap
    /// `MvlString` via `mvl_string_new`.  Returns an `MvlString*` so that:
    /// - `mvl_string_ptr()` in the printf path works correctly (no more
    ///   "treat char[] as MvlString*" crash in `range_pipeline` / `to_string`).
    /// - The pointer stays valid after the caller's stack frame is torn down
    ///   (fixes the dangling-stack-ptr crash in `generic_fns` / `struct_value_semantics`).
    fn snprintf_to_mvl_string(
        &mut self,
        alloca: inkwell::values::PointerValue<'ctx>,
        snprintf_call: inkwell::values::CallSiteValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        // snprintf returns i32 = bytes written (not including null terminator).
        use inkwell::values::AnyValue;
        let written = BasicValueEnum::try_from(snprintf_call.as_any_value_enum())
            .ok()
            .and_then(|v| {
                if let BasicValueEnum::IntValue(n) = v {
                    Some(n)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| self.context.i32_type().const_int(0, false));
        let len = self
            .builder
            .build_int_z_extend(written, self.context.i64_type(), "str_len")
            .unwrap();
        let new_fn = self.get_mvl_string_new();
        let call = self
            .builder
            .build_call(new_fn, &[alloca.into(), len.into()], "str_new")
            .unwrap();
        BasicValueEnum::try_from(call.as_any_value_enum()).unwrap_or_else(|_| {
            self.context
                .ptr_type(AddressSpace::default())
                .const_null()
                .into()
        })
    }

    /// Emit `Bool.to_string()` → heap `MvlString*` ("true" or "false").
    pub(crate) fn emit_bool_to_string(
        &mut self,
        v: inkwell::values::IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let true_str = self
            .builder
            .build_global_string_ptr("true", "bool_true")
            .unwrap();
        let false_str = self
            .builder
            .build_global_string_ptr("false", "bool_false")
            .unwrap();
        let ptr = self
            .builder
            .build_select(
                v,
                true_str.as_pointer_value(),
                false_str.as_pointer_value(),
                "bool_str_ptr",
            )
            .unwrap()
            .into_pointer_value();
        let true_len = self.context.i64_type().const_int(4, false);
        let false_len = self.context.i64_type().const_int(5, false);
        let len = self
            .builder
            .build_select(v, true_len, false_len, "bool_str_len")
            .unwrap()
            .into_int_value();
        let mvl_string_new = self.get_mvl_string_new();
        let call = self
            .builder
            .build_call(mvl_string_new, &[ptr.into(), len.into()], "bool_str")
            .unwrap();
        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).unwrap_or_else(|_| {
            self.context
                .ptr_type(inkwell::AddressSpace::default())
                .const_null()
                .into()
        })
    }

    /// Emit `Int.to_string()` → heap `MvlString*`.
    pub(crate) fn emit_int_to_string(
        &mut self,
        v: inkwell::values::IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let buf_ty = self.context.i8_type().array_type(32);
        let alloca = self.builder.build_alloca(buf_ty, "int_str_buf").unwrap();
        let snprintf = self.get_snprintf();
        let fmt = self
            .builder
            .build_global_string_ptr("%lld", "int_fmt")
            .unwrap();
        let size = self.context.i64_type().const_int(32, false);
        let call = self
            .builder
            .build_call(
                snprintf,
                &[
                    alloca.into(),
                    size.into(),
                    fmt.as_pointer_value().into(),
                    v.into(),
                ],
                "snprintf_int",
            )
            .unwrap();
        self.snprintf_to_mvl_string(alloca, call)
    }

    /// Emit `Float.to_string()` → heap `MvlString*`.
    pub(crate) fn emit_float_to_string(
        &mut self,
        v: inkwell::values::FloatValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let buf_ty = self.context.i8_type().array_type(32);
        let alloca = self.builder.build_alloca(buf_ty, "flt_str_buf").unwrap();
        let snprintf = self.get_snprintf();
        let fmt = self
            .builder
            .build_global_string_ptr("%g", "flt_fmt")
            .unwrap();
        let size = self.context.i64_type().const_int(32, false);
        let call = self
            .builder
            .build_call(
                snprintf,
                &[
                    alloca.into(),
                    size.into(),
                    fmt.as_pointer_value().into(),
                    v.into(),
                ],
                "snprintf_flt",
            )
            .unwrap();
        self.snprintf_to_mvl_string(alloca, call)
    }

    /// Select "true" or "false" and return a heap `MvlString*`.
    ///
    /// Previously returned a raw `char*` to a global string literal.  That
    /// broke the printf path which calls `mvl_string_ptr()` on every
    /// `PointerValue` argument (treating the raw char* as MvlString* caused
    /// a crash in `core_types_demo`'s Bool section).
    pub(crate) fn emit_bool_to_str_ptr(
        &mut self,
        v: inkwell::values::IntValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let t = self
            .builder
            .build_global_string_ptr("true", "true_str")
            .unwrap();
        let f = self
            .builder
            .build_global_string_ptr("false", "false_str")
            .unwrap();
        let i64_ty = self.context.i64_type();
        // "true" = 4 bytes, "false" = 5 bytes.
        let true_len = i64_ty.const_int(4, false);
        let false_len = i64_ty.const_int(5, false);
        let selected_ptr = self
            .builder
            .build_select(v, t.as_pointer_value(), f.as_pointer_value(), "bool_cstr")
            .unwrap();
        let selected_len = self
            .builder
            .build_select(v, true_len, false_len, "bool_len")
            .unwrap();
        let (BasicValueEnum::PointerValue(char_ptr), BasicValueEnum::IntValue(len)) =
            (selected_ptr, selected_len)
        else {
            return self
                .context
                .ptr_type(AddressSpace::default())
                .const_null()
                .into();
        };
        let new_fn = self.get_mvl_string_new();
        let call = self
            .builder
            .build_call(new_fn, &[char_ptr.into(), len.into()], "bool_str_new")
            .unwrap();
        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).unwrap_or_else(|_| {
            self.context
                .ptr_type(AddressSpace::default())
                .const_null()
                .into()
        })
    }

    // ── format() built-in removed (#901) — now a regular 2-arg MVL function ──

    // ── eprintln / eprint (stderr output, used by panic handler) ────────────

    /// Emit `eprintln(arg)` → `dprintf(2, "<arg>\n")`.
    pub(crate) fn emit_eprintln(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        self.emit_dprintf(args, true)
    }

    /// Core of `emit_eprintln`: write `args` to fd 2 via dprintf.
    fn emit_dprintf(&mut self, args: &[Expr], newline: bool) -> Option<BasicValueEnum<'ctx>> {
        let dprintf = self.get_dprintf();
        let fd2 = self.context.i32_type().const_int(2, false);
        let suffix = if newline { "\n" } else { "" };

        // Single string literal: embed directly in the format string.
        if let Some(Expr::Literal(Literal::Str(s), _)) = args.first() {
            if args.len() == 1 && !s.contains("{}") {
                let fmt = format!("{s}{suffix}");
                let global = self
                    .builder
                    .build_global_string_ptr(&fmt, "dprintf_fmt")
                    .unwrap();
                self.builder
                    .build_call(
                        dprintf,
                        &[fd2.into(), global.as_pointer_value().into()],
                        "dprintf",
                    )
                    .unwrap();
                return None;
            }
        }

        // Format string with `{}` placeholders: delegate to emit_printf_format
        // but re-route the output to stderr by swapping printf → dprintf(2, ...).
        if let Some(Expr::Literal(Literal::Str(fmt_str), _)) = args.first() {
            if args.len() > 1 || fmt_str.contains("{}") {
                let fmt_str = fmt_str.clone();
                return self.emit_dprintf_format(&fmt_str, &args[1..], newline);
            }
        }

        // Single non-string expression: choose specifier by LLVM type.
        let dprintf_args = self.build_dprintf_args(args, suffix);
        self.builder
            .build_call(dprintf, &dprintf_args, "dprintf")
            .unwrap();
        None
    }

    /// Build the dprintf argument list (fd=2 prepended, then format string + value).
    fn build_dprintf_args(
        &mut self,
        args: &[Expr],
        suffix: &str,
    ) -> Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> {
        let dprintf = self.get_dprintf();
        let fd2: inkwell::values::BasicMetadataValueEnum =
            self.context.i32_type().const_int(2, false).into();
        let _ = dprintf; // ensure dprintf is declared

        if let Some(expr) = args.first() {
            if let Some(val) = self.emit_expr(expr) {
                let val = if let BasicValueEnum::IntValue(iv) = val {
                    if iv.get_type().get_bit_width() == 1 {
                        self.emit_bool_to_str_ptr(iv)
                    } else {
                        val
                    }
                } else {
                    val
                };
                let (fmt_str, extra): (String, Option<inkwell::values::BasicMetadataValueEnum>) =
                    match val {
                        BasicValueEnum::IntValue(v) => {
                            let bits = v.get_type().get_bit_width();
                            let spec = if bits <= 32 { "%d" } else { "%lld" };
                            (format!("{spec}{suffix}"), Some(v.into()))
                        }
                        BasicValueEnum::FloatValue(v) => (format!("%g{suffix}"), Some(v.into())),
                        BasicValueEnum::PointerValue(v) => {
                            let sp = self.get_mvl_string_ptr();
                            let cstr_call = self
                                .builder
                                .build_call(sp, &[v.into()], "str_cptr_dp")
                                .unwrap();
                            use inkwell::values::AnyValue;
                            let cstr = BasicValueEnum::try_from(cstr_call.as_any_value_enum())
                                .unwrap_or(v.into());
                            (format!("%s{suffix}"), Some(cstr.into()))
                        }
                        _ => (suffix.to_string(), None),
                    };
                let fmt_global = self
                    .builder
                    .build_global_string_ptr(&fmt_str, "dprintf_fmt")
                    .unwrap();
                let mut result: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![fd2, fmt_global.as_pointer_value().into()];
                if let Some(arg) = extra {
                    result.push(arg);
                }
                return result;
            }
        }

        let fmt_global = self
            .builder
            .build_global_string_ptr(suffix, "dprintf_fmt_empty")
            .unwrap();
        vec![fd2, fmt_global.as_pointer_value().into()]
    }

    /// Emit a format-string dprintf call (stderr), substituting `{}` with specifiers.
    fn emit_dprintf_format(
        &mut self,
        fmt_template: &str,
        value_args: &[Expr],
        newline: bool,
    ) -> Option<BasicValueEnum<'ctx>> {
        let mut values: Vec<BasicValueEnum<'ctx>> = Vec::new();
        for e in value_args {
            let v = self.emit_expr(e)?;
            let v = if let BasicValueEnum::IntValue(iv) = v {
                if iv.get_type().get_bit_width() == 1 {
                    self.emit_bool_to_str_ptr(iv)
                } else {
                    v
                }
            } else {
                v
            };
            values.push(v);
        }

        let mut result_fmt = String::new();
        let mut arg_idx = 0usize;
        let mut chars = fmt_template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'}') {
                chars.next();
                let spec = if let Some(val) = values.get(arg_idx) {
                    match val {
                        BasicValueEnum::IntValue(v) => {
                            if v.get_type().get_bit_width() <= 32 {
                                "%d"
                            } else {
                                "%lld"
                            }
                        }
                        BasicValueEnum::FloatValue(_) => "%g",
                        BasicValueEnum::PointerValue(_) => "%s",
                        _ => "%d",
                    }
                } else {
                    "%d"
                };
                result_fmt.push_str(spec);
                arg_idx += 1;
            } else {
                result_fmt.push(c);
            }
        }
        if newline {
            result_fmt.push('\n');
        }

        let dprintf = self.get_dprintf();
        let fd2 = self.context.i32_type().const_int(2, false);
        let fmt_global = self
            .builder
            .build_global_string_ptr(&result_fmt, "dprintf_fmt")
            .unwrap();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![fd2.into(), fmt_global.as_pointer_value().into()];
        for val in values {
            let printf_val = match val {
                BasicValueEnum::PointerValue(p) => {
                    let sp = self.get_mvl_string_ptr();
                    let cstr_call = self
                        .builder
                        .build_call(sp, &[p.into()], "str_cptr_dpf")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(cstr_call.as_any_value_enum()).unwrap_or(val)
                }
                other => other,
            };
            call_args.push(printf_val.into());
        }
        self.builder
            .build_call(dprintf, &call_args, "dprintf_fmt_call")
            .unwrap();
        None
    }
}
