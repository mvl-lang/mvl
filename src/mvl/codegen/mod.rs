//! LLVM backend for MVL — Phase A (issues #353–#359 / epic #352).
//!
//! Compiles a checked MVL `Program` AST directly to LLVM IR via inkwell.
//! Enable with `--features llvm`; requires LLVM 22 installed.
//!
//! Phase A scope:
//!   L5-02: module setup, target triple, main() returns 0
//!   L5-04: primitive types (Int→i64, Float→f64, Bool→i1, Byte→i8, Char→i32)
//!   L5-07: function declarations, parameters, return values, basic calls
//!   L5-10: arithmetic, comparison, logical operators
//!   L5-17: print/println → libc printf

use inkwell::{
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum},
    values::{BasicValueEnum, FunctionValue, PointerValue},
    AddressSpace, IntPredicate,
};
use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, Expr, FnDecl, Literal, Pattern, Program, Stmt, TypeExpr, UnaryOp,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Compile a MVL program AST to LLVM IR text.
///
/// Returns the IR as a string on success, or an error message on failure.
pub fn compile_to_ir(prog: &Program, module_name: &str) -> Result<String, String> {
    let context = Context::create();
    let mut backend = LlvmBackend::new(&context, module_name);
    backend.emit_program(prog);
    backend.verify()?;
    Ok(backend.to_ir_string())
}

/// Find the `lli` interpreter binary.
///
/// Checks `PATH` first, then the well-known Homebrew keg-only location on macOS.
pub fn find_lli() -> Option<std::path::PathBuf> {
    // 1. Check PATH
    if let Ok(path) = which_lli() {
        return Some(path);
    }
    // 2. Homebrew keg-only (macOS)
    let brew = std::path::PathBuf::from("/opt/homebrew/opt/llvm/bin/lli");
    if brew.exists() {
        return Some(brew);
    }
    // 3. Intel Homebrew path
    let brew_intel = std::path::PathBuf::from("/usr/local/opt/llvm/bin/lli");
    if brew_intel.exists() {
        return Some(brew_intel);
    }
    None
}

fn which_lli() -> Result<std::path::PathBuf, ()> {
    let output = std::process::Command::new("which")
        .arg("lli")
        .output()
        .map_err(|_| ())?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Ok(std::path::PathBuf::from(s));
        }
    }
    Err(())
}

/// Parse `// expect: <line>` or `// Expected stdout:` block annotations from MVL source.
///
/// Returns the expected stdout lines joined with newlines, or `None` if no annotation found.
pub fn parse_expect_annotation(source: &str) -> Option<String> {
    // Format 1: one or more `// expect: <line>` annotations
    let single_lines: Vec<String> = source
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            t.strip_prefix("// expect:").map(|s| s.trim().to_string())
        })
        .collect();
    if !single_lines.is_empty() {
        return Some(single_lines.join("\n"));
    }

    // Format 2: `// Expected stdout:\n//   <line>\n//   ...`
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "// Expected stdout:" {
            let mut collected: Vec<String> = Vec::new();
            for following in lines.by_ref() {
                let t = following.trim();
                if let Some(rest) = t.strip_prefix("//") {
                    collected.push(rest.trim_start_matches(' ').to_string());
                } else if t.is_empty() || t.starts_with("//") {
                    // empty comment line — stop
                    break;
                } else {
                    break;
                }
            }
            if !collected.is_empty() {
                return Some(collected.join("\n"));
            }
        }
    }
    None
}

// ── Backend struct ────────────────────────────────────────────────────────────

/// Tracks alloca pointer + element type for each local variable.
type LocalEntry<'ctx> = (PointerValue<'ctx>, BasicTypeEnum<'ctx>);

struct LlvmBackend<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    /// Named local variables: name → (alloca, element_type).
    locals: HashMap<String, LocalEntry<'ctx>>,
    /// Whether the current basic block already has a terminator.
    terminated: bool,
}

impl<'ctx> LlvmBackend<'ctx> {
    fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        // L5-02: set target triple and data layout from LLVM defaults.
        let triple = inkwell::targets::TargetMachine::get_default_triple();
        module.set_triple(&triple);
        let builder = context.create_builder();
        Self {
            context,
            module,
            builder,
            locals: HashMap::new(),
            terminated: false,
        }
    }

    // ── Type mapping (L5-04) ─────────────────────────────────────────────────

    /// Map a MVL TypeExpr to an LLVM BasicTypeEnum.
    /// Returns None for the `Unit` / void type.
    fn mvl_type_to_llvm(&self, ty: &TypeExpr) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "Int" => Some(self.context.i64_type().into()),
                "Float" => Some(self.context.f64_type().into()),
                "Bool" => Some(self.context.bool_type().into()),
                "Byte" => Some(self.context.i8_type().into()),
                "Char" => Some(self.context.i32_type().into()),
                "Unit" => None,
                // String is a pointer to i8 (C char*).
                "String" => Some(self.context.ptr_type(AddressSpace::default()).into()),
                // Unknown types fall back to i64 — good enough for Phase A scalar work.
                _ => Some(self.context.i64_type().into()),
            },
            // Option<T>, Result<T,E>, List<T>, etc. → i64 placeholder for Phase A.
            _ => Some(self.context.i64_type().into()),
        }
    }

    fn is_unit_type(&self, ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Base { name, .. } if name == "Unit")
    }

    // ── Printf declaration (L5-17) ───────────────────────────────────────────

    /// Get (or lazily declare) the external `printf` function.
    fn get_printf(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("printf") {
            return f;
        }
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let printf_ty = self.context.i32_type().fn_type(&[ptr_ty], true);
        self.module
            .add_function("printf", printf_ty, Some(Linkage::External))
    }

    // ── Program emission ─────────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) {
        // First pass: declare all functions so forward calls resolve.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.declare_fn(fd);
                }
            }
        }
        // Second pass: emit bodies.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.emit_fn(fd);
                }
            }
        }
    }

    /// Declare a function signature without emitting its body.
    fn declare_fn(&self, fd: &FnDecl) {
        if self.module.get_function(&fd.name).is_some() {
            return; // already declared
        }
        let (fn_ty, _) = self.build_fn_type(fd);
        self.module.add_function(&fd.name, fn_ty, None);
    }

    fn build_fn_type(&self, fd: &FnDecl) -> (inkwell::types::FunctionType<'ctx>, bool) {
        // Special case: `fn main` uses C ABI i32 return regardless of MVL type.
        let is_c_main = fd.name == "main";
        let param_types: Vec<BasicMetadataTypeEnum<'ctx>> = fd
            .params
            .iter()
            .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
            .map(|t| t.into())
            .collect();
        let fn_ty = if is_c_main {
            self.context.i32_type().fn_type(&[], false)
        } else if self.is_unit_type(&fd.return_type) {
            self.context.void_type().fn_type(&param_types, false)
        } else if let Some(ret) = self.mvl_type_to_llvm(&fd.return_type) {
            ret.fn_type(&param_types, false)
        } else {
            self.context.void_type().fn_type(&param_types, false)
        };
        (fn_ty, is_c_main)
    }

    // ── Function emission (L5-07) ────────────────────────────────────────────

    fn emit_fn(&mut self, fd: &FnDecl) {
        let fn_val = match self.module.get_function(&fd.name) {
            Some(f) => f,
            None => {
                let (fn_ty, _) = self.build_fn_type(fd);
                self.module.add_function(&fd.name, fn_ty, None)
            }
        };
        let is_c_main = fd.name == "main";

        let entry = self.context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);
        self.locals.clear();
        self.terminated = false;

        // Alloca each parameter so they can be loaded by name as variables.
        for (i, param) in fd.params.iter().enumerate() {
            if let Some(param_val) = fn_val.get_nth_param(i as u32) {
                param_val.set_name(&param.name);
                if let Some(ty) = self.mvl_type_to_llvm(&param.ty) {
                    let alloca = self.builder.build_alloca(ty, &param.name).unwrap();
                    self.builder.build_store(alloca, param_val).unwrap();
                    self.locals.insert(param.name.clone(), (alloca, ty));
                }
            }
        }

        let body_val = self.emit_block(&fd.body);

        // Emit return terminator if the block didn't already terminate.
        if !self.terminated {
            if is_c_main {
                let zero = self.context.i32_type().const_int(0, false);
                self.builder.build_return(Some(&zero)).unwrap();
            } else if self.is_unit_type(&fd.return_type) {
                self.builder.build_return(None).unwrap();
            } else if let Some(val) = body_val {
                self.builder.build_return(Some(&val)).unwrap();
            } else {
                // Fallback: void return.
                self.builder.build_return(None).unwrap();
            }
        }
    }

    // ── Block / statement emission ───────────────────────────────────────────

    fn emit_block(&mut self, block: &Block) -> Option<BasicValueEnum<'ctx>> {
        let mut last: Option<BasicValueEnum<'ctx>> = None;
        for stmt in &block.stmts {
            if self.terminated {
                break;
            }
            last = self.emit_stmt(stmt);
        }
        last
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Option<BasicValueEnum<'ctx>> {
        match stmt {
            Stmt::Let {
                pattern, init, ty, ..
            } => {
                let val = self.emit_expr(init)?;
                // Determine the LLVM type: prefer the declared type, fall back to inferred.
                let llvm_ty = ty
                    .as_ref()
                    .and_then(|t| self.mvl_type_to_llvm(t))
                    .unwrap_or_else(|| val.get_type());
                let name = match pattern {
                    Pattern::Ident(name, _) => name.clone(),
                    Pattern::Wildcard(_) => "_".to_string(),
                    _ => "_".to_string(),
                };
                let alloca = self.builder.build_alloca(llvm_ty, &name).unwrap();
                self.builder.build_store(alloca, val).unwrap();
                self.locals.insert(name, (alloca, llvm_ty));
                None
            }
            Stmt::Return { value, .. } => {
                let ret_val = value.as_ref().and_then(|e| self.emit_expr(e));
                if let Some(v) = ret_val {
                    self.builder.build_return(Some(&v)).unwrap();
                } else {
                    self.builder.build_return(None).unwrap();
                }
                self.terminated = true;
                None
            }
            Stmt::Expr { expr, .. } => self.emit_expr(expr),
            Stmt::Assign { target, value, .. } => {
                use crate::mvl::parser::ast::LValue;
                let val = self.emit_expr(value)?;
                let target_name = match target {
                    LValue::Ident(n, _) => n.clone(),
                    LValue::Field { .. } => return None, // Phase B
                };
                if let Some((alloca, _)) = self.locals.get(&target_name).copied() {
                    self.builder.build_store(alloca, val).unwrap();
                }
                None
            }
            // If / While / For / Match — minimal stubs for Phase A; skip body.
            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt(cond, then, else_),
            _ => None,
        }
    }

    fn emit_if_stmt(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: &Option<crate::mvl::parser::ast::ElseBranch>,
    ) -> Option<BasicValueEnum<'ctx>> {
        use crate::mvl::parser::ast::ElseBranch;
        let cond_val = self.emit_expr(cond)?;
        let cond_int = match cond_val {
            BasicValueEnum::IntValue(v) => {
                // Truncate to i1 if wider (e.g. comparing i64 booleans).
                if v.get_type().get_bit_width() != 1 {
                    self.builder
                        .build_int_truncate(v, self.context.bool_type(), "cond_trunc")
                        .unwrap()
                } else {
                    v
                }
            }
            _ => return None,
        };

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let then_bb = self.context.append_basic_block(parent_fn, "then");
        let merge_bb = self.context.append_basic_block(parent_fn, "merge");
        let else_bb = if else_.is_some() {
            self.context.append_basic_block(parent_fn, "else")
        } else {
            merge_bb
        };

        self.builder
            .build_conditional_branch(cond_int, then_bb, else_bb)
            .unwrap();

        // Emit `then` block.
        self.builder.position_at_end(then_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        self.emit_block(then);
        if !self.terminated {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // Emit `else` block (if present).
        if let Some(eb) = else_ {
            self.terminated = false;
            self.builder.position_at_end(else_bb);
            match eb {
                ElseBranch::Block(blk) => {
                    self.emit_block(blk);
                }
                ElseBranch::If(if_stmt) => {
                    self.emit_stmt(if_stmt);
                }
            }
            if !self.terminated {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);
        None
    }

    // ── Expression emission ──────────────────────────────────────────────────

    fn emit_expr(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        match expr {
            Expr::Literal(lit, _) => self.emit_literal(lit),

            Expr::Ident(name, _) => self.emit_ident(name),

            Expr::Binary {
                op, left, right, ..
            } => self.emit_binary(op, left, right),

            Expr::Unary { op, expr, .. } => self.emit_unary(op, expr),

            Expr::FnCall { name, args, .. } => self.emit_fn_call(name, args),

            Expr::Block(block) => self.emit_block(block),

            // move/consume/declassify/sanitize are MVL-level concepts; the underlying
            // value is what matters at IR level.
            Expr::Move { expr, .. }
            | Expr::Consume { expr, .. }
            | Expr::Declassify { expr, .. }
            | Expr::Sanitize { expr, .. } => self.emit_expr(expr),

            Expr::If {
                cond, then, else_, ..
            } => self.emit_if_expr(cond, then, else_.as_deref()),

            _ => None,
        }
    }

    fn emit_ident(&mut self, name: &str) -> Option<BasicValueEnum<'ctx>> {
        if let Some((alloca, ty)) = self.locals.get(name).copied() {
            let val = self.builder.build_load(ty, alloca, name).unwrap();
            Some(val)
        } else {
            None
        }
    }

    fn emit_if_expr(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Expr>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let cond_val = self.emit_expr(cond)?;
        let cond_int = match cond_val {
            BasicValueEnum::IntValue(v) => {
                if v.get_type().get_bit_width() != 1 {
                    self.builder
                        .build_int_truncate(v, self.context.bool_type(), "cond_trunc")
                        .unwrap()
                } else {
                    v
                }
            }
            _ => return None,
        };

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let then_bb = self.context.append_basic_block(parent_fn, "if_then");
        let else_bb = self.context.append_basic_block(parent_fn, "if_else");
        let merge_bb = self.context.append_basic_block(parent_fn, "if_merge");

        self.builder
            .build_conditional_branch(cond_int, then_bb, else_bb)
            .unwrap();

        // then block
        self.builder.position_at_end(then_bb);
        let then_val = self.emit_block(then);
        let then_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        // else block
        self.builder.position_at_end(else_bb);
        let else_val = else_.and_then(|e| self.emit_expr(e));
        let else_end = self.builder.get_insert_block().unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);

        // Build phi if both branches produce a value of the same type.
        match (then_val, else_val) {
            (Some(tv), Some(ev)) if tv.get_type() == ev.get_type() => {
                let phi = self.builder.build_phi(tv.get_type(), "if_result").unwrap();
                phi.add_incoming(&[(&tv, then_end), (&ev, else_end)]);
                Some(phi.as_basic_value())
            }
            _ => None,
        }
    }

    // ── Literal emission ─────────────────────────────────────────────────────

    fn emit_literal(&self, lit: &Literal) -> Option<BasicValueEnum<'ctx>> {
        match lit {
            Literal::Integer(n) => {
                let v = self.context.i64_type().const_int(*n as u64, *n < 0);
                Some(v.into())
            }
            Literal::Float(f) => {
                let v = self.context.f64_type().const_float(*f);
                Some(v.into())
            }
            Literal::Bool(b) => {
                let v = self.context.bool_type().const_int(u64::from(*b), false);
                Some(v.into())
            }
            Literal::Str(s) => {
                // Create a global null-terminated string constant and return its pointer.
                let global = self.builder.build_global_string_ptr(s, "str_lit").unwrap();
                Some(global.as_pointer_value().into())
            }
            Literal::Char(c) => {
                let v = self.context.i32_type().const_int(*c as u64, false);
                Some(v.into())
            }
            Literal::Unit => None,
        }
    }

    // ── Binary operators (L5-10) ─────────────────────────────────────────────

    fn emit_binary(
        &mut self,
        op: &BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Option<BasicValueEnum<'ctx>> {
        let lhs = self.emit_expr(left)?;
        let rhs = self.emit_expr(right)?;

        match (lhs, rhs) {
            (BasicValueEnum::IntValue(l), BasicValueEnum::IntValue(r)) => {
                self.emit_int_binop(op, l, r)
            }
            (BasicValueEnum::FloatValue(l), BasicValueEnum::FloatValue(r)) => {
                self.emit_float_binop(op, l, r)
            }
            _ => None,
        }
    }

    fn emit_int_binop(
        &mut self,
        op: &BinaryOp,
        l: inkwell::values::IntValue<'ctx>,
        r: inkwell::values::IntValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        // Use checked arithmetic intrinsics for Add/Sub/Mul (L5-10: overflow detection).
        let result = match op {
            BinaryOp::Add => {
                let res = self.emit_checked_int_arith(l, r, "llvm.sadd.with.overflow", "add")?;
                res
            }
            BinaryOp::Sub => {
                let res = self.emit_checked_int_arith(l, r, "llvm.ssub.with.overflow", "sub")?;
                res
            }
            BinaryOp::Mul => {
                let res = self.emit_checked_int_arith(l, r, "llvm.smul.with.overflow", "mul")?;
                res
            }
            BinaryOp::Div => self
                .builder
                .build_int_signed_div(l, r, "div")
                .unwrap()
                .into(),
            BinaryOp::Rem => self
                .builder
                .build_int_signed_rem(l, r, "rem")
                .unwrap()
                .into(),
            BinaryOp::Eq => self
                .builder
                .build_int_compare(IntPredicate::EQ, l, r, "eq")
                .unwrap()
                .into(),
            BinaryOp::Ne => self
                .builder
                .build_int_compare(IntPredicate::NE, l, r, "ne")
                .unwrap()
                .into(),
            BinaryOp::Lt => self
                .builder
                .build_int_compare(IntPredicate::SLT, l, r, "lt")
                .unwrap()
                .into(),
            BinaryOp::Gt => self
                .builder
                .build_int_compare(IntPredicate::SGT, l, r, "gt")
                .unwrap()
                .into(),
            BinaryOp::Le => self
                .builder
                .build_int_compare(IntPredicate::SLE, l, r, "le")
                .unwrap()
                .into(),
            BinaryOp::Ge => self
                .builder
                .build_int_compare(IntPredicate::SGE, l, r, "ge")
                .unwrap()
                .into(),
            BinaryOp::And => self.builder.build_and(l, r, "and").unwrap().into(),
            BinaryOp::Or => self.builder.build_or(l, r, "or").unwrap().into(),
        };
        Some(result)
    }

    /// Emit a checked arithmetic intrinsic (`llvm.sadd.with.overflow.i64`, etc.).
    ///
    /// Extracts the result value and traps (unreachable) on overflow.
    fn emit_checked_int_arith(
        &mut self,
        l: inkwell::values::IntValue<'ctx>,
        r: inkwell::values::IntValue<'ctx>,
        intrinsic_name: &str,
        result_name: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let i1_ty = self.context.bool_type();
        // LLVM intrinsic names use dots: e.g. "llvm.sadd.with.overflow.i64".
        let full_name = format!("{intrinsic_name}.i64");
        let intrinsic_fn = self.module.get_function(&full_name).unwrap_or_else(|| {
            let struct_ty = self
                .context
                .struct_type(&[i64_ty.into(), i1_ty.into()], false);
            let fn_ty = struct_ty.fn_type(&[i64_ty.into(), i64_ty.into()], false);
            // Declare with no explicit linkage so LLVM recognises this as a built-in intrinsic.
            self.module.add_function(&full_name, fn_ty, None)
        });

        let call = self
            .builder
            .build_call(intrinsic_fn, &[l.into(), r.into()], result_name)
            .unwrap();
        use inkwell::values::AnyValue;
        let result_struct = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;

        let val = self
            .builder
            .build_extract_value(
                result_struct.into_struct_value(),
                0,
                &format!("{result_name}_val"),
            )
            .unwrap();
        let overflow = self
            .builder
            .build_extract_value(
                result_struct.into_struct_value(),
                1,
                &format!("{result_name}_ovf"),
            )
            .unwrap();

        // On overflow: trap via llvm.trap and unreachable.
        let trap_fn = self.module.get_function("llvm.trap").unwrap_or_else(|| {
            let trap_ty = self.context.void_type().fn_type(&[], false);
            self.module.add_function("llvm.trap", trap_ty, None)
        });
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let overflow_bb = self.context.append_basic_block(parent_fn, "overflow");
        let ok_bb = self.context.append_basic_block(parent_fn, "ok");
        self.builder
            .build_conditional_branch(overflow.into_int_value(), overflow_bb, ok_bb)
            .unwrap();
        self.builder.position_at_end(overflow_bb);
        self.builder.build_call(trap_fn, &[], "trap").unwrap();
        self.builder.build_unreachable().unwrap();
        self.builder.position_at_end(ok_bb);

        Some(val)
    }

    fn emit_float_binop(
        &mut self,
        op: &BinaryOp,
        l: inkwell::values::FloatValue<'ctx>,
        r: inkwell::values::FloatValue<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        use inkwell::FloatPredicate;
        let v = match op {
            BinaryOp::Add => self.builder.build_float_add(l, r, "fadd").unwrap().into(),
            BinaryOp::Sub => self.builder.build_float_sub(l, r, "fsub").unwrap().into(),
            BinaryOp::Mul => self.builder.build_float_mul(l, r, "fmul").unwrap().into(),
            BinaryOp::Div => self.builder.build_float_div(l, r, "fdiv").unwrap().into(),
            BinaryOp::Rem => self.builder.build_float_rem(l, r, "frem").unwrap().into(),
            BinaryOp::Eq => self
                .builder
                .build_float_compare(FloatPredicate::OEQ, l, r, "feq")
                .unwrap()
                .into(),
            BinaryOp::Ne => self
                .builder
                .build_float_compare(FloatPredicate::ONE, l, r, "fne")
                .unwrap()
                .into(),
            BinaryOp::Lt => self
                .builder
                .build_float_compare(FloatPredicate::OLT, l, r, "flt")
                .unwrap()
                .into(),
            BinaryOp::Gt => self
                .builder
                .build_float_compare(FloatPredicate::OGT, l, r, "fgt")
                .unwrap()
                .into(),
            BinaryOp::Le => self
                .builder
                .build_float_compare(FloatPredicate::OLE, l, r, "fle")
                .unwrap()
                .into(),
            BinaryOp::Ge => self
                .builder
                .build_float_compare(FloatPredicate::OGE, l, r, "fge")
                .unwrap()
                .into(),
            _ => return None,
        };
        Some(v)
    }

    // ── Unary operators ──────────────────────────────────────────────────────

    fn emit_unary(&mut self, op: &UnaryOp, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let val = self.emit_expr(expr)?;
        match op {
            UnaryOp::Neg => match val {
                BasicValueEnum::IntValue(v) => {
                    Some(self.builder.build_int_neg(v, "neg").unwrap().into())
                }
                BasicValueEnum::FloatValue(v) => {
                    Some(self.builder.build_float_neg(v, "fneg").unwrap().into())
                }
                _ => None,
            },
            UnaryOp::Not => match val {
                BasicValueEnum::IntValue(v) => {
                    Some(self.builder.build_not(v, "not").unwrap().into())
                }
                _ => None,
            },
            UnaryOp::Deref => Some(val),
        }
    }

    // ── Function call emission (L5-07 + L5-17) ──────────────────────────────

    fn emit_fn_call(&mut self, name: &str, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        match name {
            "println" => self.emit_println(args),
            "print" => self.emit_print(args),
            _ => {
                // Forward call to a user-defined function (already declared).
                let fn_val = self.module.get_function(name)?;
                let meta_args: Vec<inkwell::values::BasicMetadataValueEnum> = args
                    .iter()
                    .filter_map(|a| self.emit_expr(a))
                    .map(|v| v.into())
                    .collect();
                let call = self.builder.build_call(fn_val, &meta_args, "call").unwrap();
                use inkwell::values::AnyValue;
                BasicValueEnum::try_from(call.as_any_value_enum()).ok()
            }
        }
    }

    /// Emit `println(arg)` → `printf("<arg>\n")` (L5-17).
    fn emit_println(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let printf = self.get_printf();
        let fmt_args = self.build_printf_args(args, true);
        self.builder
            .build_call(printf, &fmt_args, "println")
            .unwrap();
        None
    }

    /// Emit `print(arg)` → `printf("<arg>")` (L5-17).
    fn emit_print(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let printf = self.get_printf();
        let fmt_args = self.build_printf_args(args, false);
        self.builder.build_call(printf, &fmt_args, "print").unwrap();
        None
    }

    /// Build the argument list for a printf call.
    ///
    /// For a single string-literal argument: use the literal as the format string
    /// (appending `\n` if `newline` is true).
    ///
    /// For a single non-string argument: pick a printf format specifier based on
    /// the inferred LLVM type (%lld for i64, %f for f64, %d for i1/i8/i32).
    fn build_printf_args(
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
                let (fmt_str, extra_arg): (
                    String,
                    Option<inkwell::values::BasicMetadataValueEnum>,
                ) = match val {
                    BasicValueEnum::IntValue(v) => {
                        let bits = v.get_type().get_bit_width();
                        let spec = if bits == 1 {
                            // Bool: print as 0/1 for now.
                            "%d"
                        } else if bits <= 32 {
                            "%d"
                        } else {
                            "%lld"
                        };
                        (format!("{spec}{suffix}"), Some(v.into()))
                    }
                    BasicValueEnum::FloatValue(v) => (format!("%f{suffix}"), Some(v.into())),
                    BasicValueEnum::PointerValue(v) => {
                        // Assume char* string.
                        (format!("%s{suffix}"), Some(v.into()))
                    }
                    _ => (format!("{suffix}"), None),
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

    // ── Verification and IR output ───────────────────────────────────────────

    fn verify(&self) -> Result<(), String> {
        self.module.verify().map_err(|e| e.to_string())
    }

    fn to_ir_string(&self) -> String {
        self.module.print_to_string().to_string()
    }
}
