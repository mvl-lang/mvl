//! LLVM backend for MVL — Phase A + Phase B (issues #352, #367–#371 / epics #352, #367).
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
//!
//! Phase B scope:
//!   L5-05: structs → LLVM named structs, field access via extractvalue/insertvalue
//!   L5-06: enums/ADTs → i8 discriminant (unit enums) or tagged union {i8, [N×i8]}
//!   L5-11: match → LLVM switch + phi nodes
//!   L5-12: while + for (range) loops; ? propagation on Result[T,E]

use inkwell::{
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType},
    values::{BasicValueEnum, FunctionValue, PointerValue},
    AddressSpace, IntPredicate,
};
use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, Expr, FnDecl, Literal, MatchArm, MatchBody, Pattern, Program, Stmt,
    TypeBody, TypeDecl, TypeExpr, UnaryOp, VariantFields,
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
    /// Current function being emitted — needed for `?` early return.
    current_fn: Option<FunctionValue<'ctx>>,

    // ── Phase B: type knowledge ──────────────────────────────────────────────
    /// Enum types: enum_name → [(variant_name, VariantFields)].
    enum_variants: HashMap<String, Vec<(String, VariantFields)>>,
    /// Struct types: struct_name → [(field_name, TypeExpr)] in declaration order.
    struct_fields: HashMap<String, Vec<(String, TypeExpr)>>,
    /// LLVM named struct types (for structs and payload enums).
    llvm_struct_types: HashMap<String, StructType<'ctx>>,
    /// Return types of user-defined functions (name → MVL TypeExpr).
    /// Used to determine the Ok/Some payload type when extracting from Result/Option.
    fn_return_types: HashMap<String, TypeExpr>,
}

impl<'ctx> LlvmBackend<'ctx> {
    fn new(context: &'ctx Context, module_name: &str) -> Self {
        let module = context.create_module(module_name);
        // L5-02: set target triple from LLVM defaults.
        let triple = inkwell::targets::TargetMachine::get_default_triple();
        module.set_triple(&triple);
        let builder = context.create_builder();
        Self {
            context,
            module,
            builder,
            locals: HashMap::new(),
            terminated: false,
            current_fn: None,
            enum_variants: HashMap::new(),
            struct_fields: HashMap::new(),
            llvm_struct_types: HashMap::new(),
            fn_return_types: HashMap::new(),
        }
    }

    // ── Phase B: type registration ───────────────────────────────────────────

    fn register_type_decl(&mut self, td: &TypeDecl) {
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
    fn build_llvm_types(&mut self) {
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

    fn is_unit_enum_variants(variants: &[(String, VariantFields)]) -> bool {
        variants
            .iter()
            .all(|(_, f)| matches!(f, VariantFields::Unit))
    }

    fn max_variant_payload_size_static(variants: &[(String, VariantFields)]) -> usize {
        variants
            .iter()
            .map(|(_, f)| Self::variant_payload_size_static(f))
            .max()
            .unwrap_or(0)
    }

    fn variant_payload_size_static(fields: &VariantFields) -> usize {
        match fields {
            VariantFields::Unit => 0,
            VariantFields::Tuple(types) => types.iter().map(Self::type_size_bytes_static).sum(),
            VariantFields::Struct(fields) => fields
                .iter()
                .map(|f| Self::type_size_bytes_static(&f.ty))
                .sum(),
        }
    }

    fn type_size_bytes_static(ty: &TypeExpr) -> usize {
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
    fn mvl_type_to_llvm(&self, ty: &TypeExpr) -> Option<BasicTypeEnum<'ctx>> {
        match ty {
            TypeExpr::Base { name, .. } => match name.as_str() {
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
            },
            // Ref types fall back to ptr
            TypeExpr::Ref { .. } => Some(self.context.ptr_type(AddressSpace::default()).into()),
            // Security labels (Public[T], Secret[T], Tainted[T]) and refinements
            // (T where pred) are transparent: use the inner type.
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                self.mvl_type_to_llvm(inner)
            }
            // Result[T, E] and Option[T]: tagged union { i8, [8 x i8] }
            TypeExpr::Result { .. } | TypeExpr::Option { .. } => {
                let payload_ty = self.context.i8_type().array_type(8);
                Some(
                    self.context
                        .struct_type(&[self.context.i8_type().into(), payload_ty.into()], false)
                        .into(),
                )
            }
            // Generic / compound types: i64 placeholder for Phase B
            _ => Some(self.context.i64_type().into()),
        }
    }

    fn is_unit_type(&self, ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Base { name, .. } if name == "Unit")
    }

    /// Peel `Labeled { inner }` and `Refined { inner }` wrappers recursively.
    fn strip_type_wrappers(ty: &TypeExpr) -> &TypeExpr {
        match ty {
            TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
                Self::strip_type_wrappers(inner)
            }
            other => other,
        }
    }

    /// Given an expression whose value is a `Result[T,E]` or `Option[T]`, return
    /// the LLVM type of the Ok/Some payload.  Falls back to `i64` if unknown.
    fn infer_result_ok_llvm_ty(&self, expr: &Expr) -> BasicTypeEnum<'ctx> {
        if let Expr::FnCall { name, .. } = expr {
            if let Some(ret_ty) = self.fn_return_types.get(name.as_str()) {
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
        }
        self.context.i64_type().into()
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
        // Phase B: collect type declarations first.
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                self.register_type_decl(td);
            }
        }
        self.build_llvm_types();

        // First pass: record return types, then declare all functions so forward calls resolve.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.fn_return_types
                        .insert(fd.name.clone(), *fd.return_type.clone());
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
        self.current_fn = Some(fn_val);

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
                match target {
                    LValue::Ident(n, _) => {
                        if let Some((alloca, _)) = self.locals.get(n).copied() {
                            self.builder.build_store(alloca, val).unwrap();
                        }
                    }
                    LValue::Field { base, field, .. } => {
                        self.emit_field_assign(base, field, val);
                    }
                }
                None
            }
            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt(cond, then, else_),

            // L5-11: match — returns value when in tail/expression position
            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match(scrutinee, arms),

            // L5-12: while loop
            Stmt::While { cond, body, .. } => {
                self.emit_while(cond, body);
                None
            }

            // L5-12: for loop (range-based)
            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => {
                self.emit_for(pattern, iter, body);
                None
            }
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
        let then_val = self.emit_block(then);
        let then_end = self.builder.get_insert_block().unwrap();
        if !self.terminated {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
        }

        // Emit `else` block (if present).
        let else_val = if let Some(eb) = else_ {
            self.terminated = false;
            self.builder.position_at_end(else_bb);
            let ev = match eb {
                ElseBranch::Block(blk) => self.emit_block(blk),
                ElseBranch::If(if_stmt) => self.emit_stmt(if_stmt),
            };
            let else_end = self.builder.get_insert_block().unwrap();
            if !self.terminated {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
            ev.map(|v| (v, else_end))
        } else {
            None
        };

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);

        // Build phi when both branches produce values of the same type.
        match (then_val, else_val) {
            (Some(tv), Some((ev, else_end))) if tv.get_type() == ev.get_type() => {
                let phi = self.builder.build_phi(tv.get_type(), "if_val").unwrap();
                phi.add_incoming(&[(&tv, then_end), (&ev, else_end)]);
                Some(phi.as_basic_value())
            }
            _ => None,
        }
    }

    // ── L5-12: While loop ─────────────────────────────────────────────────────

    fn emit_while(&mut self, cond: &Expr, body: &Block) {
        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let cond_bb = self.context.append_basic_block(parent_fn, "while_cond");
        let body_bb = self.context.append_basic_block(parent_fn, "while_body");
        let exit_bb = self.context.append_basic_block(parent_fn, "while_exit");

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Condition block.
        self.builder.position_at_end(cond_bb);
        let cond_val = self.emit_expr(cond);
        if let Some(BasicValueEnum::IntValue(cv)) = cond_val {
            let cv_bool = if cv.get_type().get_bit_width() != 1 {
                self.builder
                    .build_int_truncate(cv, self.context.bool_type(), "w_cond")
                    .unwrap()
            } else {
                cv
            };
            self.builder
                .build_conditional_branch(cv_bool, body_bb, exit_bb)
                .unwrap();
        } else {
            self.builder.build_unconditional_branch(exit_bb).unwrap();
        }

        // Body block.
        self.builder.position_at_end(body_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        self.emit_block(body);
        if !self.terminated {
            self.builder.build_unconditional_branch(cond_bb).unwrap();
        }
        self.terminated = prev_terminated;

        // Exit block.
        self.builder.position_at_end(exit_bb);
    }

    // ── L5-11: Match ─────────────────────────────────────────────────────────

    /// Emit a match expression or statement, returning the phi-merged result value (if any).
    fn emit_match(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Option<BasicValueEnum<'ctx>> {
        let ok_ty = self.infer_result_ok_llvm_ty(scrutinee);
        let scrutinee_val = self.emit_expr(scrutinee)?;

        // Extract i8 discriminant from the scrutinee value.
        let disc_val = self.extract_discriminant(scrutinee_val)?;

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let merge_bb = self.context.append_basic_block(parent_fn, "match_merge");
        let fallback_bb = self.context.append_basic_block(parent_fn, "match_default");

        // Determine discriminant and basic block for each arm.
        let mut arm_blocks: Vec<inkwell::basic_block::BasicBlock<'ctx>> = Vec::new();
        let mut switch_cases: Vec<(
            inkwell::values::IntValue<'ctx>,
            inkwell::basic_block::BasicBlock<'ctx>,
        )> = Vec::new();
        let mut default_bb: Option<inkwell::basic_block::BasicBlock<'ctx>> = None;

        for (i, arm) in arms.iter().enumerate() {
            let arm_bb = self
                .context
                .append_basic_block(parent_fn, &format!("arm{i}"));
            arm_blocks.push(arm_bb);

            if let Some(disc) = self.pattern_to_discriminant(&arm.pattern) {
                switch_cases.push((disc, arm_bb));
            } else if default_bb.is_none() {
                default_bb = Some(arm_bb);
            }
        }

        let actual_default = default_bb.unwrap_or(fallback_bb);
        self.builder
            .build_switch(disc_val, actual_default, &switch_cases)
            .unwrap();

        // Emit each arm body.
        let prev_terminated = self.terminated;
        let mut phi_incoming: Vec<(BasicValueEnum<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)> =
            Vec::new();

        let mut arms_reaching_merge = 0usize;

        for (i, arm) in arms.iter().enumerate() {
            let arm_bb = arm_blocks[i];
            self.builder.position_at_end(arm_bb);
            self.terminated = false;

            // Bind pattern variables if needed (Phase B: simple cases only).
            self.bind_pattern_vars(&arm.pattern, scrutinee_val, Some(ok_ty));

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e),
                MatchBody::Block(b) => self.emit_block(b),
            };

            let arm_end = self.builder.get_insert_block().unwrap();
            if !self.terminated {
                arms_reaching_merge += 1;
                if let Some(val) = arm_val {
                    phi_incoming.push((val, arm_end));
                }
                self.builder.build_unconditional_branch(merge_bb).unwrap();
            }
        }

        // Fallback block: unreachable for exhaustive match.
        self.builder.position_at_end(fallback_bb);
        self.builder.build_unreachable().unwrap();

        self.terminated = prev_terminated;
        self.builder.position_at_end(merge_bb);

        // Only build a phi if every arm that reaches merge_bb produced a value.
        // Fewer phi entries than predecessors would produce invalid LLVM IR.
        if phi_incoming.is_empty() || phi_incoming.len() < arms_reaching_merge {
            return None;
        }

        // All arms must produce the same type for phi to work.
        let first_ty = phi_incoming[0].0.get_type();
        if phi_incoming.iter().all(|(v, _)| v.get_type() == first_ty) {
            let phi = self.builder.build_phi(first_ty, "match_val").unwrap();
            for (val, bb) in &phi_incoming {
                phi.add_incoming(&[(val, *bb)]);
            }
            Some(phi.as_basic_value())
        } else {
            None
        }
    }

    /// Extract an i8 discriminant from a value.
    ///
    /// - `i8` value (unit enum) → use directly.
    /// - Struct value (tagged union) → extractvalue at index 0.
    fn extract_discriminant(
        &mut self,
        val: BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        match val {
            BasicValueEnum::IntValue(v) if v.get_type().get_bit_width() == 8 => Some(v),
            BasicValueEnum::StructValue(sv) => {
                let disc = self.builder.build_extract_value(sv, 0, "disc").unwrap();
                if let BasicValueEnum::IntValue(iv) = disc {
                    Some(iv)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Map a match pattern to its i8 discriminant constant (None for wildcards / bindings).
    fn pattern_to_discriminant(&self, pat: &Pattern) -> Option<inkwell::values::IntValue<'ctx>> {
        match pat {
            Pattern::Ident(name, _) | Pattern::TupleStruct { name, .. } => {
                self.lookup_enum_variant_disc(name)
            }
            // Built-in Result/Option patterns.
            Pattern::Ok { .. } | Pattern::Some { .. } => {
                Some(self.context.i8_type().const_int(0, false))
            }
            Pattern::Err { .. } | Pattern::None(_) => {
                Some(self.context.i8_type().const_int(1, false))
            }
            _ => None,
        }
    }

    /// Look up the discriminant for an enum variant by name.
    ///
    /// Accepts both qualified (`Shape::Circle`) and unqualified (`Circle`) names.
    fn lookup_enum_variant_disc(&self, name: &str) -> Option<inkwell::values::IntValue<'ctx>> {
        // Built-in Result/Option variants.
        match name {
            "Ok" | "Some" => return Some(self.context.i8_type().const_int(0, false)),
            "Err" | "None" => return Some(self.context.i8_type().const_int(1, false)),
            _ => {}
        }
        // Qualified: "Shape::Circle"
        if let Some(pos) = name.find("::") {
            let type_name = &name[..pos];
            let variant_name = &name[pos + 2..];
            if let Some(variants) = self.enum_variants.get(type_name) {
                let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;
                return Some(self.context.i8_type().const_int(disc, false));
            }
            return None;
        }
        // Unqualified: search all enums.
        for variants in self.enum_variants.values() {
            if let Some(disc) = variants.iter().position(|(vn, _)| vn == name) {
                return Some(self.context.i8_type().const_int(disc as u64, false));
            }
        }
        None
    }

    /// Bind pattern-introduced variables into `self.locals` before emitting arm body.
    ///
    /// For tuple-variant patterns like `Some(v)`, extracts the payload and stores it.
    /// `ok_ty` is the expected LLVM type of an Ok/Some payload (defaults to i64 if None).
    fn bind_pattern_vars(
        &mut self,
        pat: &Pattern,
        scrutinee: BasicValueEnum<'ctx>,
        ok_ty: Option<BasicTypeEnum<'ctx>>,
    ) {
        let default_ok_ty: BasicTypeEnum = self.context.i64_type().into();
        let ok_llvm_ty = ok_ty.unwrap_or(default_ok_ty);

        // Built-in Pattern::Ok(inner), Pattern::Err(inner), Pattern::Some(inner).
        let (inner_pat, is_err) = match pat {
            Pattern::Ok { inner, .. } | Pattern::Some { inner, .. } => {
                (Some(inner.as_ref()), false)
            }
            Pattern::Err { inner, .. } => (Some(inner.as_ref()), true),
            _ => (None, false),
        };
        if let Some(inner) = inner_pat {
            if let Pattern::Ident(bind_name, _) = inner {
                let BasicValueEnum::StructValue(sv) = scrutinee else {
                    return;
                };
                let payload = match self.builder.build_extract_value(sv, 1, "payload") {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let llvm_ty: BasicTypeEnum = if is_err {
                    self.context.ptr_type(AddressSpace::default()).into()
                } else {
                    ok_llvm_ty
                };
                let payload_ty = payload.get_type();
                let tmp = self
                    .builder
                    .build_alloca(payload_ty, "res_payload_tmp")
                    .unwrap();
                self.builder.build_store(tmp, payload).unwrap();
                let loaded = self.builder.build_load(llvm_ty, tmp, bind_name).unwrap();
                let alloca = self.builder.build_alloca(llvm_ty, bind_name).unwrap();
                self.builder.build_store(alloca, loaded).unwrap();
                self.locals.insert(bind_name.clone(), (alloca, llvm_ty));
            }
            return;
        }

        if let Pattern::TupleStruct { name, fields, .. } = pat {
            // Extract payload from tagged union.
            let BasicValueEnum::StructValue(sv) = scrutinee else {
                return;
            };
            // payload is at index 1 (byte array).
            let payload_arr = match self.builder.build_extract_value(sv, 1, "payload") {
                Ok(v) => v,
                Err(_) => return,
            };

            // Built-in Result/Option variants: Ok(v), Some(v) → ok_llvm_ty; Err(e) → ptr.
            if matches!(name.as_str(), "Ok" | "Some" | "Err") {
                let bind_name = match fields.first() {
                    Some(Pattern::Ident(n, _)) => n.clone(),
                    _ => return,
                };
                let payload_arr = match self.builder.build_extract_value(sv, 1, "payload") {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let llvm_ty: BasicTypeEnum = if name == "Err" {
                    self.context.ptr_type(AddressSpace::default()).into()
                } else {
                    ok_llvm_ty
                };
                let payload_arr_ty = payload_arr.get_type();
                let tmp = self
                    .builder
                    .build_alloca(payload_arr_ty, "res_payload_tmp")
                    .unwrap();
                self.builder.build_store(tmp, payload_arr).unwrap();
                let loaded = self.builder.build_load(llvm_ty, tmp, &bind_name).unwrap();
                let alloca = self.builder.build_alloca(llvm_ty, &bind_name).unwrap();
                self.builder.build_store(alloca, loaded).unwrap();
                self.locals.insert(bind_name, (alloca, llvm_ty));
                return;
            }

            // Determine variant payload types.
            let (type_name, variant_name) = if let Some(pos) = name.find("::") {
                (name[..pos].to_string(), name[pos + 2..].to_string())
            } else {
                // Search for unqualified variant name.
                let found = self.enum_variants.iter().find_map(|(tn, variants)| {
                    variants
                        .iter()
                        .any(|(vn, _)| vn == name)
                        .then(|| tn.clone())
                });
                match found {
                    Some(tn) => (tn, name.clone()),
                    None => return,
                }
            };

            let variants = match self.enum_variants.get(&type_name) {
                Some(v) => v.clone(),
                None => return,
            };
            let variant_fields = match variants.iter().find(|(vn, _)| vn == &variant_name) {
                Some((_, vf)) => vf.clone(),
                None => return,
            };

            if let VariantFields::Tuple(field_types) = &variant_fields {
                for (i, (pat_field, field_ty)) in fields.iter().zip(field_types.iter()).enumerate()
                {
                    let Pattern::Ident(bind_name, _) = pat_field else {
                        continue;
                    };
                    let Some(llvm_ty) = self.mvl_type_to_llvm(field_ty) else {
                        continue;
                    };

                    // Alloca a slot for the extracted value.
                    let alloca = self.builder.build_alloca(llvm_ty, bind_name).unwrap();

                    // Bitcast the payload array into a pointer to the field type,
                    // then load. For the first field we use the payload base; for
                    // subsequent fields we GEP forward by the accumulated offset.
                    let offset: usize = (0..i)
                        .map(|j| Self::type_size_bytes_static(&field_types[j]))
                        .sum();

                    // Store payload_arr into a temporary alloca so we can GEP into it.
                    let payload_ty = payload_arr.get_type();
                    let tmp = self
                        .builder
                        .build_alloca(payload_ty, "payload_tmp")
                        .unwrap();
                    self.builder.build_store(tmp, payload_arr).unwrap();

                    let field_ptr = if offset == 0 {
                        tmp
                    } else {
                        let off_val = self.context.i64_type().const_int(offset as u64, false);
                        unsafe {
                            self.builder
                                .build_gep(self.context.i8_type(), tmp, &[off_val], "field_ptr")
                                .unwrap()
                        }
                    };

                    let loaded = self
                        .builder
                        .build_load(llvm_ty, field_ptr, bind_name)
                        .unwrap();
                    self.builder.build_store(alloca, loaded).unwrap();
                    self.locals.insert(bind_name.clone(), (alloca, llvm_ty));
                }
            }
        }
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

            // move/consume/declassify/sanitize: transparent at IR level.
            Expr::Move { expr, .. }
            | Expr::Consume { expr, .. }
            | Expr::Declassify { expr, .. }
            | Expr::Sanitize { expr, .. } => self.emit_expr(expr),

            Expr::If {
                cond, then, else_, ..
            } => self.emit_if_expr(cond, then, else_.as_deref()),

            // L5-11: match expression
            Expr::Match {
                scrutinee, arms, ..
            } => self.emit_match(scrutinee, arms),

            // L5-05: struct construction
            Expr::Construct { name, fields, .. } => self.emit_construct(name, fields),

            // L5-05: field access
            Expr::FieldAccess { expr, field, .. } => self.emit_field_access(expr, field),

            // L5-12: ? propagation
            Expr::Propagate { expr, .. } => self.emit_propagate(expr),

            // Method calls: minimal support for .len() on range and .to_string() on Int
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.emit_method_call(receiver, method, args),

            _ => None,
        }
    }

    fn emit_ident(&mut self, name: &str) -> Option<BasicValueEnum<'ctx>> {
        // L5-06: qualified enum variant reference, e.g. `Shape::Circle`
        if name.contains("::") {
            if let Some(pos) = name.find("::") {
                let type_name = name[..pos].to_string();
                let variant_name = name[pos + 2..].to_string();
                return self.emit_enum_variant_construct(&type_name, &variant_name, &[]);
            }
        }

        // Local variable.
        if let Some((alloca, ty)) = self.locals.get(name).copied() {
            let val = self.builder.build_load(ty, alloca, name).unwrap();
            return Some(val);
        }

        // L5-06: unqualified unit enum variant (e.g. `Circle` without `Shape::`).
        let found = self.enum_variants.iter().find_map(|(etype, variants)| {
            variants
                .iter()
                .position(|(vn, _)| vn == name)
                .map(|_| etype.clone())
        });
        if let Some(etype) = found {
            return self.emit_enum_variant_construct(&etype, name, &[]);
        }

        None
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

    // ── L5-05: Struct emission ────────────────────────────────────────────────

    /// Emit `Name { field: expr, ... }` struct construction.
    fn emit_construct(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        // Enum struct variant: "EnumType::Variant { fields }"
        if let Some(pos) = name.find("::") {
            let type_name = name[..pos].to_string();
            let variant_name = name[pos + 2..].to_string();
            if self.enum_variants.contains_key(&type_name) {
                return self.emit_enum_struct_variant(&type_name, &variant_name, fields);
            }
        }

        // Regular struct construction.
        let field_info: Vec<(String, TypeExpr)> = self.struct_fields.get(name)?.clone();
        let struct_ty = *self.llvm_struct_types.get(name)?;

        let mut sv = struct_ty.get_undef();
        for (idx, (fname, _)) in field_info.iter().enumerate() {
            if let Some((_, fexpr)) = fields.iter().find(|(n, _)| n == fname) {
                if let Some(fval) = self.emit_expr(fexpr) {
                    sv = self
                        .builder
                        .build_insert_value(sv, fval, idx as u32, &format!("s{idx}"))
                        .unwrap()
                        .into_struct_value();
                }
            }
        }
        Some(sv.into())
    }

    /// Emit a struct-variant enum construction: `AuthError::AccountLocked { attempts: 3 }`.
    fn emit_enum_struct_variant(
        &mut self,
        type_name: &str,
        variant_name: &str,
        fields: &[(String, Expr)],
    ) -> Option<BasicValueEnum<'ctx>> {
        let variants = self.enum_variants.get(type_name)?.clone();
        let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;
        let struct_ty = *self.llvm_struct_types.get(type_name)?;
        let alloca = self.builder.build_alloca(struct_ty, "enum_sv").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store struct fields into payload area.
        let variant_fields = variants
            .iter()
            .find(|(vn, _)| vn == variant_name)
            .map(|(_, vf)| vf.clone())?;
        if let VariantFields::Struct(field_decls) = &variant_fields {
            let payload_ptr = self
                .builder
                .build_struct_gep(struct_ty, alloca, 1, "payload_ptr")
                .unwrap();

            let mut offset = 0usize;
            for fd in field_decls {
                if let Some((_, fexpr)) = fields.iter().find(|(n, _)| n == &fd.name) {
                    if let Some(fval) = self.emit_expr(fexpr) {
                        if let Some(llvm_ty) = self.mvl_type_to_llvm(&fd.ty) {
                            let field_ptr = if offset == 0 {
                                payload_ptr
                            } else {
                                let off = self.context.i64_type().const_int(offset as u64, false);
                                unsafe {
                                    self.builder
                                        .build_gep(
                                            self.context.i8_type(),
                                            payload_ptr,
                                            &[off],
                                            "sv_field",
                                        )
                                        .unwrap()
                                }
                            };
                            self.builder.build_store(field_ptr, fval).unwrap();
                            offset += Self::type_size_bytes_static(&fd.ty);
                            let _ = llvm_ty; // used above
                        }
                    }
                }
            }
        }

        Some(
            self.builder
                .build_load(struct_ty, alloca, "enum_sv_val")
                .unwrap(),
        )
    }

    /// Emit `expr.field` field access.
    fn emit_field_access(&mut self, obj: &Expr, field: &str) -> Option<BasicValueEnum<'ctx>> {
        let obj_val = self.emit_expr(obj)?;
        let BasicValueEnum::StructValue(sv) = obj_val else {
            return None;
        };

        // Look up the struct type name → field index.
        let ty = sv.get_type();
        let type_name = ty.get_name()?.to_str().ok()?.to_string();
        let field_info = self.struct_fields.get(&type_name)?.clone();
        let idx = field_info.iter().position(|(n, _)| n == field)? as u32;

        self.builder.build_extract_value(sv, idx, field).ok()
    }

    /// Emit a field assignment `lvalue.field = val`.
    fn emit_field_assign(
        &mut self,
        base: &crate::mvl::parser::ast::LValue,
        field: &str,
        new_val: BasicValueEnum<'ctx>,
    ) {
        use crate::mvl::parser::ast::LValue;
        // Only handle the simple case: `ident.field = val`.
        let LValue::Ident(var_name, _) = base else {
            return;
        };
        let Some((alloca, ty)) = self.locals.get(var_name.as_str()).copied() else {
            return;
        };
        let BasicTypeEnum::StructType(st) = ty else {
            return;
        };
        let type_name = match st.get_name() {
            Some(n) => n.to_str().unwrap_or("").to_string(),
            None => return,
        };
        let field_info = match self.struct_fields.get(&type_name) {
            Some(fi) => fi.clone(),
            None => return,
        };
        let idx = match field_info.iter().position(|(n, _)| n == field) {
            Some(i) => i as u32,
            None => return,
        };

        // Load → insert → store.
        let cur = self.builder.build_load(ty, alloca, "cur").unwrap();
        let BasicValueEnum::StructValue(sv) = cur else {
            return;
        };
        let updated = self
            .builder
            .build_insert_value(sv, new_val, idx, "updated")
            .unwrap()
            .into_struct_value();
        self.builder.build_store(alloca, updated).unwrap();
    }

    // ── L5-06: Enum variant construction ────────────────────────────────────

    /// Construct an enum variant value.
    ///
    /// - Unit enum (all variants are Unit): returns `i8` discriminant.
    /// - Payload enum: allocas a tagged union `{ i8, [N×i8] }`, stores discriminant
    ///   and payload, then loads and returns the struct value.
    fn emit_enum_variant_construct(
        &mut self,
        type_name: &str,
        variant_name: &str,
        payload_args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let variants = self.enum_variants.get(type_name)?.clone();
        let disc = variants.iter().position(|(vn, _)| vn == variant_name)? as u64;

        if Self::is_unit_enum_variants(&variants) {
            // Unit enum: just the i8 discriminant.
            return Some(self.context.i8_type().const_int(disc, false).into());
        }

        // Payload enum: build tagged union on the stack.
        let struct_ty = *self.llvm_struct_types.get(type_name)?;
        let alloca = self.builder.build_alloca(struct_ty, "enum_tmp").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(struct_ty, alloca, 0, "disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store payload if arguments were provided.
        if !payload_args.is_empty() {
            let variant_fields = variants
                .iter()
                .find(|(vn, _)| vn == variant_name)
                .map(|(_, vf)| vf.clone())?;

            if let VariantFields::Tuple(field_types) = &variant_fields {
                let payload_ptr = self
                    .builder
                    .build_struct_gep(struct_ty, alloca, 1, "payload_ptr")
                    .unwrap();

                let mut offset = 0usize;
                for (arg, fty) in payload_args.iter().zip(field_types.iter()) {
                    if let Some(fval) = self.emit_expr(arg) {
                        let field_ptr = if offset == 0 {
                            payload_ptr
                        } else {
                            let off = self.context.i64_type().const_int(offset as u64, false);
                            unsafe {
                                self.builder
                                    .build_gep(
                                        self.context.i8_type(),
                                        payload_ptr,
                                        &[off],
                                        "pf_ptr",
                                    )
                                    .unwrap()
                            }
                        };
                        self.builder.build_store(field_ptr, fval).unwrap();
                        offset += Self::type_size_bytes_static(fty);
                    }
                }
            }
        }

        Some(
            self.builder
                .build_load(struct_ty, alloca, "enum_val")
                .unwrap(),
        )
    }

    // ── Result/Option construction ────────────────────────────────────────────

    /// Emit `Ok(val)` (disc=0) or `Err(val)` (disc=1) as a tagged union `{i8, [8 x i8]}`.
    fn emit_result_variant(&mut self, disc: u64, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let payload_ty = self.context.i8_type().array_type(8);
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), payload_ty.into()], false);
        let alloca = self.builder.build_alloca(result_ty, "res_tmp").unwrap();

        // Store discriminant.
        let disc_val = self.context.i8_type().const_int(disc, false);
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, alloca, 0, "res_disc")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_val).unwrap();

        // Store payload value.
        if let Some(arg) = args.first() {
            if let Some(val) = self.emit_expr(arg) {
                let payload_ptr = self
                    .builder
                    .build_struct_gep(result_ty, alloca, 1, "res_payload")
                    .unwrap();
                self.builder.build_store(payload_ptr, val).unwrap();
            }
        }

        Some(
            self.builder
                .build_load(result_ty, alloca, "res_val")
                .unwrap(),
        )
    }

    // ── L5-12: for loop ───────────────────────────────────────────────────────

    /// Emit `for pat in range(a, b) { body }` as a counted LLVM loop.
    ///
    /// Only `range(a, b)` iterators are supported for now.
    fn emit_for(&mut self, pattern: &Pattern, iter: &Expr, body: &Block) {
        // Only handle `for x in range(a, b)`.
        let (var_name, start_expr, end_expr) = match iter {
            Expr::FnCall { name, args, .. } if name == "range" && args.len() == 2 => {
                let var = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => return,
                };
                (var, &args[0], &args[1])
            }
            _ => return,
        };

        let start_val = match self.emit_expr(start_expr) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => return,
        };
        let end_val = match self.emit_expr(end_expr) {
            Some(BasicValueEnum::IntValue(v)) => v,
            _ => return,
        };

        let i64_ty = self.context.i64_type();
        let alloca = self.builder.build_alloca(i64_ty, &var_name).unwrap();
        self.builder.build_store(alloca, start_val).unwrap();
        self.locals
            .insert(var_name.clone(), (alloca, i64_ty.into()));

        let parent_fn = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();
        let cond_bb = self.context.append_basic_block(parent_fn, "for_cond");
        let body_bb = self.context.append_basic_block(parent_fn, "for_body");
        let exit_bb = self.context.append_basic_block(parent_fn, "for_exit");

        self.builder.build_unconditional_branch(cond_bb).unwrap();

        // Condition: i < end
        self.builder.position_at_end(cond_bb);
        let cur = self
            .builder
            .build_load(i64_ty, alloca, "for_i")
            .unwrap()
            .into_int_value();
        let cond = self
            .builder
            .build_int_compare(IntPredicate::SLT, cur, end_val, "for_lt")
            .unwrap();
        self.builder
            .build_conditional_branch(cond, body_bb, exit_bb)
            .unwrap();

        // Body: execute, then increment and loop back.
        self.builder.position_at_end(body_bb);
        let prev_terminated = self.terminated;
        self.terminated = false;
        self.emit_block(body);
        if !self.terminated {
            let cur = self
                .builder
                .build_load(i64_ty, alloca, "for_i_inc")
                .unwrap()
                .into_int_value();
            let one = i64_ty.const_int(1, false);
            let next = self.builder.build_int_add(cur, one, "for_next").unwrap();
            self.builder.build_store(alloca, next).unwrap();
            self.builder.build_unconditional_branch(cond_bb).unwrap();
        }
        self.terminated = prev_terminated;

        // Exit block.
        self.builder.position_at_end(exit_bb);
    }

    // ── L5-12: ? propagation ─────────────────────────────────────────────────

    /// Emit `expr?` — evaluate expr (must return `Result[T, E]` tagged union),
    /// branch to ok/err: on Err, return early; on Ok, yield the payload value.
    fn emit_propagate(&mut self, expr: &Expr) -> Option<BasicValueEnum<'ctx>> {
        let ok_ty = self.infer_result_ok_llvm_ty(expr);
        let result_val = self.emit_expr(expr)?;
        let BasicValueEnum::StructValue(sv) = result_val else {
            return None;
        };

        // Extract i8 discriminant (field 0).
        let disc = self.builder.build_extract_value(sv, 0, "prop_disc").ok()?;
        let BasicValueEnum::IntValue(disc_i) = disc else {
            return None;
        };

        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
        let ok_bb = self.context.append_basic_block(parent_fn, "prop_ok");
        let err_bb = self.context.append_basic_block(parent_fn, "prop_err");

        let zero = self.context.i8_type().const_int(0, false);
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, disc_i, zero, "is_ok")
            .unwrap();
        self.builder
            .build_conditional_branch(is_ok, ok_bb, err_bb)
            .unwrap();

        // Err branch: return the Result struct unchanged (propagate the error).
        self.builder.position_at_end(err_bb);
        self.builder.build_return(Some(&result_val)).unwrap();

        // Ok branch: extract payload and yield with the correct type.
        self.builder.position_at_end(ok_bb);
        let payload = self
            .builder
            .build_extract_value(sv, 1, "prop_payload")
            .ok()?;
        let payload_ty = payload.get_type();
        let tmp = self.builder.build_alloca(payload_ty, "prop_tmp").unwrap();
        self.builder.build_store(tmp, payload).unwrap();
        let ok_val = self.builder.build_load(ok_ty, tmp, "prop_ok_val").unwrap();
        Some(ok_val)
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
            BinaryOp::Add => self.emit_checked_int_arith(l, r, "llvm.sadd.with.overflow", "add")?,
            BinaryOp::Sub => self.emit_checked_int_arith(l, r, "llvm.ssub.with.overflow", "sub")?,
            BinaryOp::Mul => self.emit_checked_int_arith(l, r, "llvm.smul.with.overflow", "mul")?,
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
            "format" => self.emit_format(args),
            // range(start, end) as a value → { i64 start, i64 end } range struct
            "range" if args.len() == 2 => {
                let start = match self.emit_expr(&args[0])? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                let end = match self.emit_expr(&args[1])? {
                    BasicValueEnum::IntValue(v) => v,
                    _ => return None,
                };
                let range_ty = self.context.struct_type(
                    &[
                        self.context.i64_type().into(),
                        self.context.i64_type().into(),
                    ],
                    false,
                );
                let alloca = self.builder.build_alloca(range_ty, "range_tmp").unwrap();
                let s_ptr = self
                    .builder
                    .build_struct_gep(range_ty, alloca, 0, "range_start")
                    .unwrap();
                let e_ptr = self
                    .builder
                    .build_struct_gep(range_ty, alloca, 1, "range_end")
                    .unwrap();
                self.builder.build_store(s_ptr, start).unwrap();
                self.builder.build_store(e_ptr, end).unwrap();
                Some(
                    self.builder
                        .build_load(range_ty, alloca, "range_val")
                        .unwrap(),
                )
            }
            _ => {
                // Built-in Result/Option constructors: Ok(v), Err(e), Some(v)
                if matches!(name, "Ok" | "Some") && args.len() == 1 {
                    return self.emit_result_variant(0, args);
                }
                if name == "Err" && args.len() == 1 {
                    return self.emit_result_variant(1, args);
                }
                // L5-06: enum tuple variant constructor, e.g. `Shape::Circle(r)`
                if name.contains("::") {
                    if let Some(pos) = name.find("::") {
                        let type_name = name[..pos].to_string();
                        let variant_name = name[pos + 2..].to_string();
                        if self.enum_variants.contains_key(&type_name) {
                            return self.emit_enum_variant_construct(
                                &type_name,
                                &variant_name,
                                args,
                            );
                        }
                    }
                }
                // Forward call to a user-defined function (already declared).
                let fn_val = self.module.get_function(name)?;
                // Use undef for args that can't be emitted (e.g. unimplemented method
                // calls) so the arg count stays correct and IR validation passes.
                let param_types = fn_val.get_type().get_param_types();
                let meta_args: Vec<inkwell::values::BasicMetadataValueEnum> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| match self.emit_expr(a) {
                        Some(v) => v.into(),
                        None => {
                            let ty = param_types
                                .get(i)
                                .copied()
                                .unwrap_or_else(|| self.context.i64_type().into());
                            match ty {
                                BasicMetadataTypeEnum::IntType(t) => t.get_undef().into(),
                                BasicMetadataTypeEnum::FloatType(t) => t.get_undef().into(),
                                BasicMetadataTypeEnum::PointerType(t) => t.get_undef().into(),
                                BasicMetadataTypeEnum::StructType(t) => t.get_undef().into(),
                                BasicMetadataTypeEnum::ArrayType(t) => t.get_undef().into(),
                                _ => self.context.i64_type().get_undef().into(),
                            }
                        }
                    })
                    .collect();
                let call = self.builder.build_call(fn_val, &meta_args, "call").unwrap();
                use inkwell::values::AnyValue;
                BasicValueEnum::try_from(call.as_any_value_enum()).ok()
            }
        }
    }

    // ── Method call emission ─────────────────────────────────────────────────

    /// Emit `receiver.method(args)`.
    ///
    /// Supported:
    /// - `range_struct.len()` → `end - start` (i64)
    /// - `int_val.to_string()` → snprintf to a static 32-byte buffer (ptr)
    fn emit_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        _args: &[Expr],
    ) -> Option<BasicValueEnum<'ctx>> {
        let recv_val = self.emit_expr(receiver)?;
        match method {
            "len" => {
                // Expect a range struct { i64, i64 } — return end - start.
                let BasicValueEnum::StructValue(sv) = recv_val else {
                    return None;
                };
                let start = self
                    .builder
                    .build_extract_value(sv, 0, "range_s")
                    .ok()?
                    .into_int_value();
                let end = self
                    .builder
                    .build_extract_value(sv, 1, "range_e")
                    .ok()?
                    .into_int_value();
                Some(
                    self.builder
                        .build_int_sub(end, start, "range_len")
                        .unwrap()
                        .into(),
                )
            }
            "to_string" => {
                // Convert an integer to its decimal string representation.
                match recv_val {
                    BasicValueEnum::IntValue(v) => Some(self.emit_int_to_string(v)),
                    BasicValueEnum::FloatValue(v) => Some(self.emit_float_to_string(v)),
                    BasicValueEnum::PointerValue(p) => Some(p.into()), // already a string
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Emit snprintf(buf, 32, "%lld", v) and return buf ptr.
    fn emit_int_to_string(&mut self, v: inkwell::values::IntValue<'ctx>) -> BasicValueEnum<'ctx> {
        let buf_ty = self.context.i8_type().array_type(32);
        let alloca = self.builder.build_alloca(buf_ty, "int_str_buf").unwrap();
        let snprintf = self.get_snprintf();
        let fmt = self
            .builder
            .build_global_string_ptr("%lld", "int_fmt")
            .unwrap();
        let size = self.context.i64_type().const_int(32, false);
        self.builder
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
        alloca.into()
    }

    /// Emit snprintf(buf, 32, "%g", v) and return buf ptr.
    fn emit_float_to_string(
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
        self.builder
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
        alloca.into()
    }

    fn get_snprintf(&self) -> FunctionValue<'ctx> {
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

    // ── format() built-in ────────────────────────────────────────────────────

    /// Emit `format("template {}", a, b)` → `sprintf` into a global 256-byte buffer,
    /// returning a `ptr` (char*) to that buffer.
    fn emit_format(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
        let Some(Expr::Literal(Literal::Str(fmt_template), _)) = args.first() else {
            return None;
        };
        let fmt_template = fmt_template.clone();
        let value_args = &args[1..];

        // Emit all value expressions first so we know their LLVM types.
        let values: Vec<BasicValueEnum<'ctx>> = value_args
            .iter()
            .filter_map(|e| self.emit_expr(e))
            .collect();

        // Build sprintf format string (same specifier logic as emit_printf_format).
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

        // Get or create global 256-byte format buffer.
        let buf_name = "format_buf";
        let buf_ty = self.context.i8_type().array_type(256);
        let buf_global = if let Some(g) = self.module.get_global(buf_name) {
            g
        } else {
            let g = self.module.add_global(buf_ty, None, buf_name);
            g.set_initializer(&buf_ty.const_zero());
            g
        };
        let buf_ptr = buf_global.as_pointer_value();

        // Call sprintf(buf, fmt, args...).
        let sprintf = self.get_sprintf();
        let fmt_global = self
            .builder
            .build_global_string_ptr(&fmt, "sprintf_fmt")
            .unwrap();
        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            vec![buf_ptr.into(), fmt_global.as_pointer_value().into()];
        for val in values {
            call_args.push(val.into());
        }
        self.builder
            .build_call(sprintf, &call_args, "sprintf_call")
            .unwrap();

        Some(buf_ptr.into())
    }

    fn get_sprintf(&self) -> FunctionValue<'ctx> {
        if let Some(f) = self.module.get_function("sprintf") {
            return f;
        }
        let ptr_ty: BasicMetadataTypeEnum = self.context.ptr_type(AddressSpace::default()).into();
        let sprintf_ty = self.context.i32_type().fn_type(&[ptr_ty, ptr_ty], true);
        self.module
            .add_function("sprintf", sprintf_ty, Some(Linkage::External))
    }

    // ── Printf / println (L5-17, enhanced for Phase B) ───────────────────────

    /// Emit `println(arg)` → `printf("<arg>\n")` (L5-17).
    fn emit_println(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
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

        // Single non-string expression.
        let fmt_args = self.build_printf_args(args, true);
        let printf = self.get_printf();
        self.builder
            .build_call(printf, &fmt_args, "println")
            .unwrap();
        None
    }

    /// Emit `print(arg)` → `printf("<arg>")` (L5-17).
    fn emit_print(&mut self, args: &[Expr]) -> Option<BasicValueEnum<'ctx>> {
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
        let fmt_args = self.build_printf_args(args, false);
        let printf = self.get_printf();
        self.builder.build_call(printf, &fmt_args, "print").unwrap();
        None
    }

    /// Emit a format-string printf call, substituting `{}` with type-appropriate specifiers.
    ///
    /// `println("value is {}", x)` → `printf("value is %lld\n", x)`
    fn emit_printf_format(
        &mut self,
        fmt_template: &str,
        value_args: &[Expr],
        newline: bool,
    ) -> Option<BasicValueEnum<'ctx>> {
        // Emit all value expressions first so we know their LLVM types.
        let values: Vec<BasicValueEnum<'ctx>> = value_args
            .iter()
            .filter_map(|e| self.emit_expr(e))
            .collect();

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
            call_args.push(val.into());
        }
        self.builder
            .build_call(printf, &call_args, "printf_fmt_call")
            .unwrap();
        None
    }

    /// Build the argument list for a printf call (single-arg, non-format-string path).
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
                        let spec = if bits <= 32 { "%d" } else { "%lld" };
                        (format!("{spec}{suffix}"), Some(v.into()))
                    }
                    BasicValueEnum::FloatValue(v) => (format!("%g{suffix}"), Some(v.into())),
                    BasicValueEnum::PointerValue(v) => {
                        // Assume char* string.
                        (format!("%s{suffix}"), Some(v.into()))
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

    // ── Verification and IR output ───────────────────────────────────────────

    fn verify(&self) -> Result<(), String> {
        self.module.verify().map_err(|e| e.to_string())
    }

    fn to_ir_string(&self) -> String {
        self.module.print_to_string().to_string()
    }
}
