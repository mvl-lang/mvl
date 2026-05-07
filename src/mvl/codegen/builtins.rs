//! Built-in function emission for the MVL LLVM backend.
//!
//! Covers libc wrappers (printf, snprintf, strlen), collection literals
//! (List, Map, Set), and the `println` / `print` / `format` built-ins (L5-17).

use inkwell::{
    module::Linkage,
    types::BasicMetadataTypeEnum,
    values::{BasicValueEnum, FunctionValue},
    AddressSpace,
};

use crate::mvl::parser::ast::{Expr, Literal};

use super::LlvmBackend;

impl<'ctx> LlvmBackend<'ctx> {
    // ── Printf declaration (L5-17) ───────────────────────────────────────────

    /// Get (or lazily declare) the external `printf` function.
    pub(crate) fn get_printf(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("printf") {
            return f;
        }
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let printf_ty = self.context.i32_type().fn_type(&[ptr_ty], true);
        self.module
            .add_function("printf", printf_ty, Some(Linkage::External))
    }

    #[allow(dead_code)]
    pub(crate) fn get_strlen(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("strlen") {
            return f;
        }
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let strlen_ty = self.context.i64_type().fn_type(&[ptr_ty], false);
        self.module
            .add_function("strlen", strlen_ty, Some(Linkage::External))
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

    // ── format() built-in ────────────────────────────────────────────────────

    /// Emit `format("template {}", a, b)` → `snprintf` into a stack-allocated 256-byte buffer,
    /// returning a `ptr` (char*) to that buffer.
    ///
    /// Uses a per-call stack allocation (not a global) so multiple format() calls in the same
    /// function each get an independent buffer.
    pub(crate) fn emit_format(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let Some(Expr::Literal(Literal::Str(fmt_template), _)) = args.first() else {
            return None;
        };
        let fmt_template = fmt_template.clone();
        let value_args = &args[1..];

        // Emit all value expressions — fail fast if any argument cannot be emitted.
        let values: Vec<BasicValueEnum<'ctx>> = value_args
            .iter()
            .map(|e| self.emit_expr(e))
            .collect::<Option<Vec<_>>>()?;

        // Build snprintf format string (same specifier logic as emit_printf_format).
        let mut fmt = String::new();
        let mut arg_idx = 0usize;
        let mut chars = fmt_template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'}') {
                chars.next();
                let spec = values
                    .get(arg_idx)
                    .map(|val| match val {
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
                    })
                    .unwrap_or("%d");
                fmt.push_str(spec);
                arg_idx += 1;
            } else {
                fmt.push(c);
            }
        }

        // Stack-allocate a 256-byte buffer for this call (not a shared global).
        let buf_ty = self.context.i8_type().array_type(256);
        let buf_alloca = self.builder.build_alloca(buf_ty, "format_buf").unwrap();

        // Call snprintf(buf, 256, fmt, args...).
        let snprintf = self.get_snprintf();
        let fmt_global = self
            .builder
            .build_global_string_ptr(&fmt, "format_fmt")
            .unwrap();
        let size = self.context.i64_type().const_int(256, false);
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            buf_alloca.into(),
            size.into(),
            fmt_global.as_pointer_value().into(),
        ];
        for val in values {
            call_args.push(val.into());
        }
        let snprintf_call = self
            .builder
            .build_call(snprintf, &call_args, "snprintf_fmt_call")
            .unwrap();

        // Wrap the stack buffer in a heap MvlString* so the pointer stays valid
        // after this stack frame exits (fixes dangling-ptr crash when format()
        // result is returned from a function like `show` in struct_value_semantics).
        Some(self.snprintf_to_mvl_string(buf_alloca, snprintf_call))
    }

    // ── Printf / println (L5-17, enhanced for Phase B) ───────────────────────

    /// Emit `println(arg)` → `printf("<arg>\n")` (L5-17).
    pub(crate) fn emit_println(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        // Single string literal: use directly.
        if args.len() == 1 {
            if let Some(Expr::Literal(Literal::Str(s), _)) = args.first() {
                let printf = self.get_printf();
                let fmt = format!("{s}\n");
                let global = self
                    .builder
                    .build_global_string_ptr(&fmt, "println_fmt")
                    .unwrap();
                self.builder
                    .build_call(printf, &[global.as_pointer_value().into()], "println")
                    .unwrap();
                return None;
            }
        }

        // Format string + value args: `println("template {}", a, b)`
        if let Some(Expr::Literal(Literal::Str(fmt_str), _)) = args.first() {
            if args.len() > 1 || fmt_str.contains("{}") {
                let fmt_str = fmt_str.clone();
                return self.emit_printf_format(&fmt_str, &args[1..], true);
            }
        }

        // Multi-arg non-string: synthesize implicit "{} {} ..." template.
        if args.len() > 1 {
            let template = vec!["{}"; args.len()].join(" ");
            return self.emit_printf_format(&template, args, true);
        }

        // Single non-string expression.
        let fmt_args = self.build_printf_args(args, true);
        let printf = self.get_printf();
        self.builder
            .build_call(printf, &fmt_args, "println")
            .unwrap();
        None
    }

    /// Emit `print(arg)` → `printf("<arg>")` (L5-17).
    pub(crate) fn emit_print(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        // Single string literal: use directly.
        if args.len() == 1 {
            if let Some(Expr::Literal(Literal::Str(s), _)) = args.first() {
                let printf = self.get_printf();
                let global = self
                    .builder
                    .build_global_string_ptr(s, "print_fmt")
                    .unwrap();
                self.builder
                    .build_call(printf, &[global.as_pointer_value().into()], "print")
                    .unwrap();
                return None;
            }
        }
        if let Some(Expr::Literal(Literal::Str(fmt_str), _)) = args.first() {
            if args.len() > 1 || fmt_str.contains("{}") {
                let fmt_str = fmt_str.clone();
                return self.emit_printf_format(&fmt_str, &args[1..], false);
            }
        }

        // Multi-arg non-string: synthesize implicit "{} {} ..." template.
        if args.len() > 1 {
            let template = vec!["{}"; args.len()].join(" ");
            return self.emit_printf_format(&template, args, false);
        }

        let fmt_args = self.build_printf_args(args, false);
        let printf = self.get_printf();
        self.builder.build_call(printf, &fmt_args, "print").unwrap();
        None
    }

    /// Emit a format-string printf call, substituting `{}` with type-appropriate specifiers.
    ///
    /// `println("value is {}", x)` → `printf("value is %lld\n", x)`
    pub(crate) fn emit_printf_format(
        &mut self,
        fmt_template: &str,
        value_args: &[Expr],
        newline: bool,
    ) -> Option<BasicValueEnum<'ctx>> {
        // Emit all value expressions — fail fast if any argument cannot be emitted.
        // Convert i1 (Bool) → "true"/"false" ptr as we go.
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

        // Build the printf format string by replacing `{}` with specifiers.
        let mut result_fmt = String::new();
        let mut arg_idx = 0usize;
        let mut chars = fmt_template.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' && chars.peek() == Some(&'}') {
                chars.next(); // consume '}'
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

        let printf = self.get_printf();
        let fmt_global = self
            .builder
            .build_global_string_ptr(&result_fmt, "printf_fmt")
            .unwrap();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![fmt_global.as_pointer_value().into()];
        for val in values {
            // L5-14: MvlString* values must be converted to char* for printf.
            let printf_val = match val {
                BasicValueEnum::PointerValue(p) => {
                    let sp = self.get_mvl_string_ptr();
                    let cstr_call = self
                        .builder
                        .build_call(sp, &[p.into()], "str_cptr_fmt")
                        .unwrap();
                    use inkwell::values::AnyValue;
                    BasicValueEnum::try_from(cstr_call.as_any_value_enum()).unwrap_or(val)
                }
                other => other,
            };
            call_args.push(printf_val.into());
        }
        self.builder
            .build_call(printf, &call_args, "printf_fmt_call")
            .unwrap();
        None
    }

    /// Build the argument list for a printf call (single-arg, non-format-string path).
    pub(crate) fn build_printf_args(
        &mut self,
        args: &[Expr],
        newline: bool,
    ) -> Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> {
        let suffix = if newline { "\n" } else { "" };

        // Single string literal → use directly as format string.
        if let Some(Expr::Literal(Literal::Str(s), _)) = args.first() {
            let fmt = format!("{s}{suffix}");
            let global = self
                .builder
                .build_global_string_ptr(&fmt, "println_fmt")
                .unwrap();
            return vec![global.as_pointer_value().into()];
        }

        // Single expression → emit value and choose format specifier.
        if let Some(expr) = args.first() {
            if let Some(val) = self.emit_expr(expr) {
                // Convert i1 Bool → "true"/"false" ptr.
                let val = if let BasicValueEnum::IntValue(iv) = val {
                    if iv.get_type().get_bit_width() == 1 {
                        self.emit_bool_to_str_ptr(iv)
                    } else {
                        val
                    }
                } else {
                    val
                };
                let (fmt_str, extra_arg): (
                    String,
                    Option<inkwell::values::BasicMetadataValueEnum>,
                ) = match val {
                    BasicValueEnum::IntValue(v) => {
                        let bits = v.get_type().get_bit_width();
                        let spec = if bits <= 32 { "%d" } else { "%lld" };
                        (format!("{spec}{suffix}"), Some(v.into()))
                    }
                    BasicValueEnum::FloatValue(v) => (format!("%g{suffix}"), Some(v.into())),
                    BasicValueEnum::PointerValue(v) => {
                        // L5-14: in Phase C, pointer values are MvlString*.
                        // Call mvl_string_ptr to extract the null-terminated char* for printf.
                        let sp = self.get_mvl_string_ptr();
                        let cstr_call = self
                            .builder
                            .build_call(sp, &[v.into()], "str_cptr")
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
                    .build_global_string_ptr(&fmt_str, "printf_fmt")
                    .unwrap();
                let mut result: Vec<inkwell::values::BasicMetadataValueEnum> =
                    vec![fmt_global.as_pointer_value().into()];
                if let Some(arg) = extra_arg {
                    result.push(arg);
                }
                return result;
            }
        }

        // No args: just print the newline/nothing.
        let fmt_global = self
            .builder
            .build_global_string_ptr(suffix, "printf_fmt_empty")
            .unwrap();
        vec![fmt_global.as_pointer_value().into()]
    }
}
