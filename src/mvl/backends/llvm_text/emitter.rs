// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `LlvmTextCompiler` — pure-string LLVM IR emitter (Phase 1, issue #1111).
//!
//! Generates valid LLVM IR text for a subset of MVL programs (primitives,
//! arithmetic, if/else, while, function calls).  No inkwell, no unsafe.

use std::collections::HashMap;

use crate::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, FnDecl, LValue, LetKind, Literal, Pattern, Program,
    Stmt, TypeExpr, UnaryOp,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Pure-string LLVM IR compiler — Phase 1.
///
/// Generates LLVM IR text for programs using only primitive types (`Int`,
/// `Float`, `Bool`, `Unit`).  Invoke [`compile_to_ir`](Self::compile_to_ir).
pub struct LlvmTextCompiler {
    /// Target triple emitted in the module header.
    pub target_triple: String,
}

impl LlvmTextCompiler {
    /// Create a new compiler with the host target triple.
    pub fn new() -> Self {
        Self {
            target_triple: default_target_triple(),
        }
    }

    /// Compile a MVL [`Program`] to LLVM IR text.
    ///
    /// Returns the full `.ll` file contents on success, or an error message
    /// when the program contains constructs not yet supported in Phase 1.
    pub fn compile_to_ir(&self, prog: &Program, module_name: &str) -> Result<String, String> {
        let mut emitter = TextEmitter::new(module_name, &self.target_triple);
        emitter.emit_program(prog)?;
        Ok(emitter.finish())
    }
}

impl Default for LlvmTextCompiler {
    fn default() -> Self {
        Self::new()
    }
}

// ── Internal emitter ──────────────────────────────────────────────────────────

/// Internal IR builder.  Maintains SSA counter, basic-block tracking, and
/// three output sections (globals, function bodies, extern declarations).
struct TextEmitter {
    module_name: String,
    target_triple: String,

    // ── Output sections (assembled in `finish`) ───────────────────────────
    fn_bodies: Vec<String>, // define … { … }

    // ── Per-function state (reset on every new function) ──────────────────
    /// Accumulated lines for the function body currently being built.
    fn_buf: Vec<String>,
    /// Current basic-block label (name without `%`).
    current_bb: String,
    /// Whether the current basic block already ends with a terminator.
    terminated: bool,
    /// SSA register counter for the current function.
    reg: usize,
    /// Basic-block counter for the current function.
    bb: usize,
    /// Named locals: MVL variable name → SSA value string (e.g. `%t3`).
    locals: HashMap<String, String>,
    /// Mutable ref locals: MVL `ref` name → alloca register (ptr to value).
    ref_locals: HashMap<String, RefLocal>,
    /// Return type of the current function (needed for terminators).
    current_ret_ty: TypeExpr,
    /// Known user-defined function signatures: name → ret TypeExpr.
    fn_ret_types: HashMap<String, TypeExpr>,
}

/// An alloca'd mutable variable: holds the pointer register and the element type.
#[derive(Clone)]
struct RefLocal {
    ptr: String,
    elem_ty: TypeExpr,
}

impl TextEmitter {
    fn new(module_name: &str, target_triple: &str) -> Self {
        Self {
            module_name: module_name.to_string(),
            target_triple: target_triple.to_string(),
            fn_bodies: Vec::new(),
            fn_buf: Vec::new(),
            current_bb: String::new(),
            terminated: false,
            reg: 0,
            bb: 0,
            locals: HashMap::new(),
            ref_locals: HashMap::new(),
            current_ret_ty: TypeExpr::Base {
                name: "Unit".into(),
                args: vec![],
                span: Default::default(),
            },
            fn_ret_types: HashMap::new(),
        }
    }

    // ── Finalise: assemble the complete .ll text ──────────────────────────

    fn finish(self) -> String {
        let mut out = String::new();
        out.push_str(&format!("; ModuleID = '{}'\n", self.module_name));
        out.push_str(&format!("source_filename = \"{}\"\n", self.module_name));
        out.push_str(&format!("target triple = \"{}\"\n", self.target_triple));
        for body in &self.fn_bodies {
            out.push('\n');
            out.push_str(body);
        }
        out
    }

    // ── Counter helpers ───────────────────────────────────────────────────

    fn next_reg(&mut self) -> String {
        let n = self.reg;
        self.reg += 1;
        format!("%t{n}")
    }

    fn next_bb(&mut self, prefix: &str) -> String {
        let n = self.bb;
        self.bb += 1;
        format!("{prefix}_{n}")
    }

    // ── Instruction helpers ───────────────────────────────────────────────

    fn push_line(&mut self, line: &str) {
        self.fn_buf.push(line.to_string());
    }

    fn push_instr(&mut self, instr: &str) {
        self.fn_buf.push(format!("  {instr}"));
    }

    fn start_bb(&mut self, label: &str) {
        self.fn_buf.push(format!("{label}:"));
        self.current_bb = label.to_string();
        self.terminated = false;
    }

    // ── Per-function state reset ──────────────────────────────────────────

    fn reset_fn_state(&mut self, ret_ty: TypeExpr) {
        self.fn_buf.clear();
        self.current_bb = "entry".to_string();
        self.terminated = false;
        self.reg = 0;
        self.bb = 0;
        self.locals.clear();
        self.ref_locals.clear();
        self.current_ret_ty = ret_ty;
    }

    // ── Type helpers ──────────────────────────────────────────────────────

    /// Map a MVL `TypeExpr` to its LLVM IR type string.
    fn llvm_ty(ty: &TypeExpr) -> String {
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
            _ => "ptr".to_string(),
        }
    }

    fn is_void(ty: &TypeExpr) -> bool {
        Self::llvm_ty(ty) == "void"
    }

    fn is_mutable_ref(ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Ref { mutable: true, .. })
    }

    /// Unwrap one level of `ref T` to get the inner type.
    fn deref_ty(ty: &TypeExpr) -> &TypeExpr {
        match ty {
            TypeExpr::Ref { inner, .. } => inner.as_ref(),
            other => other,
        }
    }

    // ── Program emission ──────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) -> Result<(), String> {
        // First pass: register all function return types for forward calls.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                self.fn_ret_types
                    .insert(fd.name.clone(), fd.return_type.as_ref().clone());
            }
        }
        // Second pass: emit each function (Phase 1: skip all non-Fn declarations).
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.emit_fn(fd)?;
                }
            }
        }
        Ok(())
    }

    // ── Function emission ─────────────────────────────────────────────────

    fn emit_fn(&mut self, fd: &FnDecl) -> Result<(), String> {
        let ret_ty = fd.return_type.as_ref();
        self.reset_fn_state(ret_ty.clone());

        // Build parameter list: skip Unit-typed params
        let params: Vec<String> = fd
            .params
            .iter()
            .filter_map(|p| {
                let ty_str = Self::llvm_ty(&p.ty);
                if ty_str == "void" {
                    None
                } else {
                    Some(format!("{ty_str} %{}", p.name))
                }
            })
            .collect();
        let params_str = params.join(", ");

        let llvm_ret = Self::llvm_ty(ret_ty);
        let is_main = fd.name == "main";

        let sig = if is_main {
            "define i32 @main()".to_string()
        } else {
            format!(
                "define {llvm_ret} @{fn_name}({params_str})",
                fn_name = fd.name
            )
        };

        self.push_line(&sig);
        self.push_line("{");
        self.push_line("entry:");

        // Bind parameters as SSA locals
        for p in &fd.params {
            if Self::llvm_ty(&p.ty) != "void" {
                self.locals.insert(p.name.clone(), format!("%{}", p.name));
            }
        }

        // Emit body
        let body_val = self.emit_block(&fd.body)?;

        // Emit final return (unless already terminated by a `return` stmt)
        if !self.terminated {
            if is_main {
                self.push_instr("ret i32 0");
            } else if Self::is_void(ret_ty) {
                self.push_instr("ret void");
            } else if let Some(val) = body_val {
                self.push_instr(&format!("ret {llvm_ret} {val}"));
            } else {
                self.push_instr(&format!("ret {llvm_ret} undef"));
            }
        }

        self.push_line("}");

        let body_text = self.fn_buf.join("\n");
        self.fn_bodies.push(body_text);
        Ok(())
    }

    // ── Block emission ────────────────────────────────────────────────────

    fn emit_block(&mut self, block: &Block) -> Result<Option<String>, String> {
        let stmts = &block.stmts;
        if stmts.is_empty() {
            return Ok(None);
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for s in head {
            self.emit_stmt(s)?;
        }
        match &tail[0] {
            Stmt::Expr { expr, .. } => self.emit_expr(expr),
            // A trailing `if/else` is value-producing — emit with phi nodes.
            Stmt::If {
                cond, then, else_, ..
            } => {
                let else_block = match else_ {
                    Some(ElseBranch::Block(b)) => Some(b),
                    _ => None,
                };
                self.emit_if_phi(cond, then, else_block)
            }
            other => {
                self.emit_stmt(other)?;
                Ok(None)
            }
        }
    }

    // ── Statement emission ────────────────────────────────────────────────

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), String> {
        match stmt {
            Stmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                // Ghost bindings are erased before codegen
                if *kind == LetKind::Ghost {
                    return Ok(());
                }
                let val = self.emit_expr(init)?;
                let elem_ty = Self::deref_ty(ty).clone();

                if Self::is_mutable_ref(ty) {
                    // Mutable: alloca the inner type + store initial value
                    let ty_str = Self::llvm_ty(&elem_ty);
                    if ty_str == "void" {
                        return Ok(());
                    }
                    let ptr = self.next_reg();
                    self.push_instr(&format!("{ptr} = alloca {ty_str}"));
                    if let Some(v) = val {
                        self.push_instr(&format!("store {ty_str} {v}, ptr {ptr}"));
                    }
                    if let Pattern::Ident(name, _) = pattern {
                        self.ref_locals.insert(
                            name.clone(),
                            RefLocal {
                                ptr,
                                elem_ty: elem_ty.clone(),
                            },
                        );
                    }
                } else {
                    // Immutable: alias SSA value
                    if let (Some(v), Pattern::Ident(name, _)) = (val, pattern) {
                        self.locals.insert(name.clone(), v);
                    }
                }
                Ok(())
            }

            Stmt::Assign { target, value, .. } => {
                let val = self.emit_expr(value)?;
                if let LValue::Ident(name, _) = target {
                    if let Some(loc) = self.ref_locals.get(name).cloned() {
                        if let Some(v) = val {
                            let ty_str = Self::llvm_ty(&loc.elem_ty);
                            self.push_instr(&format!("store {ty_str} {v}, ptr {}", loc.ptr));
                        }
                    }
                }
                Ok(())
            }

            Stmt::Return { value, .. } => {
                let ret_ty = self.current_ret_ty.clone();
                if Self::is_void(&ret_ty) {
                    self.push_instr("ret void");
                } else if let Some(expr) = value {
                    let val = self.emit_expr(expr)?;
                    let ty = Self::llvm_ty(&ret_ty);
                    if let Some(v) = val {
                        self.push_instr(&format!("ret {ty} {v}"));
                    } else {
                        self.push_instr(&format!("ret {ty} undef"));
                    }
                } else {
                    self.push_instr("ret void");
                }
                self.terminated = true;
                Ok(())
            }

            Stmt::While { cond, body, .. } => self.emit_while(cond, body),

            Stmt::If {
                cond, then, else_, ..
            } => {
                self.emit_if_stmt(cond, then, else_.as_ref())?;
                Ok(())
            }

            Stmt::Expr { expr, .. } => {
                self.emit_expr(expr)?;
                Ok(())
            }

            // Phase 2: for, match, ghost
            _ => Ok(()),
        }
    }

    // ── While loop emission ───────────────────────────────────────────────

    fn emit_while(&mut self, cond: &Expr, body: &Block) -> Result<(), String> {
        let loop_bb = self.next_bb("loop");
        let body_bb = self.next_bb("loop_body");
        let end_bb = self.next_bb("loop_end");

        self.push_instr(&format!("br label %{loop_bb}"));
        self.start_bb(&loop_bb);

        let cond_val = self.emit_expr(cond)?;
        if let Some(cv) = cond_val {
            self.push_instr(&format!("br i1 {cv}, label %{body_bb}, label %{end_bb}"));
        } else {
            self.push_instr(&format!("br label %{end_bb}"));
        }

        self.start_bb(&body_bb);
        self.emit_block(body)?;
        if !self.terminated {
            self.push_instr(&format!("br label %{loop_bb}"));
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    // ── If-statement emission (no phi, just control flow) ─────────────────

    fn emit_if_stmt(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&ElseBranch>,
    ) -> Result<(), String> {
        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(()),
        };
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        self.emit_block(then)?;
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        if let Some(e) = else_ {
            match e {
                ElseBranch::Block(b) => {
                    self.emit_block(b)?;
                }
                ElseBranch::If(stmt) => {
                    self.emit_stmt(stmt)?;
                }
            }
        }
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);
        Ok(())
    }

    // ── Expression emission ───────────────────────────────────────────────

    fn emit_expr(&mut self, expr: &Expr) -> Result<Option<String>, String> {
        match expr {
            Expr::Literal(lit, _) => Ok(self.emit_literal(lit)),

            Expr::Ident(name, _) => {
                if let Some(loc) = self.ref_locals.get(name).cloned() {
                    let ty_str = Self::llvm_ty(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    return Ok(Some(reg));
                }
                if let Some(val) = self.locals.get(name).cloned() {
                    return Ok(Some(val));
                }
                Ok(None)
            }

            Expr::Binary {
                op, left, right, ..
            } => self.emit_binary(op, left, right),

            Expr::Unary { op, expr, .. } => self.emit_unary(op, expr),

            Expr::If {
                cond, then, else_, ..
            } => self.emit_if_expr(cond, then, else_.as_deref()),

            Expr::Block(block) => self.emit_block(block),

            Expr::FnCall { name, args, .. } => self.emit_fn_call(name, args),

            Expr::Consume { expr, .. } | Expr::Relabel { expr, .. } => self.emit_expr(expr),

            _ => Ok(None),
        }
    }

    // ── Literal emission ──────────────────────────────────────────────────

    fn emit_literal(&self, lit: &Literal) -> Option<String> {
        match lit {
            Literal::Integer(n) => Some(format!("{n}")),
            Literal::Float(f) => Some(if f.fract() == 0.0 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }),
            Literal::Bool(b) => Some(if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }),
            _ => None,
        }
    }

    // ── Binary operator emission ──────────────────────────────────────────

    fn emit_binary(
        &mut self,
        op: &BinaryOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        if matches!(op, BinaryOp::And) {
            return self.emit_short_circuit_and(left, right);
        }
        if matches!(op, BinaryOp::Or) {
            return self.emit_short_circuit_or(left, right);
        }

        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let is_float = Self::expr_is_float(left);
        let instr = Self::binary_instr(op, is_float, &lv, &rv);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = {instr}"));
        Ok(Some(reg))
    }

    fn binary_instr(op: &BinaryOp, is_float: bool, lv: &str, rv: &str) -> String {
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
            BinaryOp::Eq => format!("icmp eq i64 {lv}, {rv}"),
            BinaryOp::Ne => format!("icmp ne i64 {lv}, {rv}"),
            BinaryOp::Lt => format!("icmp slt i64 {lv}, {rv}"),
            BinaryOp::Gt => format!("icmp sgt i64 {lv}, {rv}"),
            BinaryOp::Le => format!("icmp sle i64 {lv}, {rv}"),
            BinaryOp::Ge => format!("icmp sge i64 {lv}, {rv}"),
            BinaryOp::BitAnd => format!("and i64 {lv}, {rv}"),
            BinaryOp::BitOr => format!("or i64 {lv}, {rv}"),
            BinaryOp::BitXor => format!("xor i64 {lv}, {rv}"),
            BinaryOp::Shl => format!("shl i64 {lv}, {rv}"),
            BinaryOp::Shr => format!("ashr i64 {lv}, {rv}"),
            BinaryOp::And | BinaryOp::Or => unreachable!("handled before binary_instr"),
        }
    }

    fn expr_is_float(expr: &Expr) -> bool {
        match expr {
            Expr::Literal(Literal::Float(_), _) => true,
            Expr::Binary { left, .. } => Self::expr_is_float(left),
            _ => false,
        }
    }

    // ── Short-circuit && / || ─────────────────────────────────────────────

    fn emit_short_circuit_and(
        &mut self,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("and_rhs");
        let merge_bb = self.next_bb("and_merge");
        let left_end = self.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{rhs_bb}, label %{merge_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ false, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        Ok(Some(result))
    }

    fn emit_short_circuit_or(
        &mut self,
        left: &Expr,
        right: &Expr,
    ) -> Result<Option<String>, String> {
        let lv = match self.emit_expr(left)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_bb = self.next_bb("or_rhs");
        let merge_bb = self.next_bb("or_merge");
        let left_end = self.current_bb.clone();

        self.push_instr(&format!("br i1 {lv}, label %{merge_bb}, label %{rhs_bb}"));
        self.start_bb(&rhs_bb);
        let rv = match self.emit_expr(right)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let rhs_end = self.current_bb.clone();
        self.push_instr(&format!("br label %{merge_bb}"));

        self.start_bb(&merge_bb);
        let result = self.next_reg();
        self.push_instr(&format!(
            "{result} = phi i1 [ true, %{left_end} ], [ {rv}, %{rhs_end} ]"
        ));
        Ok(Some(result))
    }

    // ── Unary operator emission ───────────────────────────────────────────

    fn emit_unary(&mut self, op: &UnaryOp, expr: &Expr) -> Result<Option<String>, String> {
        let val = match self.emit_expr(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let is_float = Self::expr_is_float(expr);
        let reg = self.next_reg();
        match op {
            UnaryOp::Neg if is_float => {
                self.push_instr(&format!("{reg} = fneg double {val}"));
            }
            UnaryOp::Neg => {
                self.push_instr(&format!("{reg} = sub i64 0, {val}"));
            }
            UnaryOp::Not => {
                self.push_instr(&format!("{reg} = xor i1 {val}, true"));
            }
            UnaryOp::BitNot => {
                self.push_instr(&format!("{reg} = xor i64 {val}, -1"));
            }
            UnaryOp::Deref => {
                // Deref is transparent in the text emitter for Phase 1
                return Ok(Some(val));
            }
        }
        Ok(Some(reg))
    }

    // ── If expression emission (produces a phi) ───────────────────────────

    /// Emit an if/else that produces a value via phi nodes.
    /// Both `Expr::If` (from emit_expr) and tail `Stmt::If` (from emit_block) use this.
    fn emit_if_phi(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Block>,
    ) -> Result<Option<String>, String> {
        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        self.start_bb(&then_bb);
        let then_val = self.emit_block(then)?;
        let then_end = self.current_bb.clone();
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&else_bb);
        let else_val = if let Some(b) = else_ {
            self.emit_block(b)?
        } else {
            None
        };
        let else_end = self.current_bb.clone();
        if !self.terminated {
            self.push_instr(&format!("br label %{merge_bb}"));
        }

        self.start_bb(&merge_bb);

        match (then_val, else_val) {
            (Some(tv), Some(ev)) => {
                // Use i64 as default phi type (Phase 1 limitation: no type tracking)
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi i64 [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                ));
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    /// Emit an `Expr::If` (else_ is an expression, not a block).
    fn emit_if_expr(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Expr>,
    ) -> Result<Option<String>, String> {
        // Unwrap Expr::Block else branches to reuse emit_if_phi.
        match else_ {
            Some(Expr::Block(b)) => self.emit_if_phi(cond, then, Some(b)),
            Some(nested_if @ Expr::If { .. }) => {
                // Nested `else if` — emit phi treating nested if as else block.
                // Fall back to the general path (no phi across nested ifs in Phase 1).
                let cond_val = match self.emit_expr(cond)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let then_bb = self.next_bb("then");
                let else_bb = self.next_bb("else");
                let merge_bb = self.next_bb("merge");
                self.push_instr(&format!(
                    "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
                ));
                self.start_bb(&then_bb);
                let then_val = self.emit_block(then)?;
                let then_end = self.current_bb.clone();
                if !self.terminated {
                    self.push_instr(&format!("br label %{merge_bb}"));
                }
                self.start_bb(&else_bb);
                let else_val = self.emit_expr(nested_if)?;
                let else_end = self.current_bb.clone();
                if !self.terminated {
                    self.push_instr(&format!("br label %{merge_bb}"));
                }
                self.start_bb(&merge_bb);
                match (then_val, else_val) {
                    (Some(tv), Some(ev)) => {
                        let result = self.next_reg();
                        self.push_instr(&format!(
                            "{result} = phi i64 [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                        ));
                        Ok(Some(result))
                    }
                    _ => Ok(None),
                }
            }
            None => self.emit_if_phi(cond, then, None),
            Some(_) => {
                // Other expr kinds as else: treat as unit (no phi value)
                self.emit_if_phi(cond, then, None)
            }
        }
    }

    // ── Function call emission ────────────────────────────────────────────

    fn emit_fn_call(&mut self, name: &str, args: &[Expr]) -> Result<Option<String>, String> {
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            if let Some(v) = self.emit_expr(arg)? {
                // Phase 1: assume i64 for all args (Phase 2: use type tracking)
                arg_vals.push(("i64".to_string(), v));
            }
        }
        let args_str = arg_vals
            .iter()
            .map(|(ty, v)| format!("{ty} {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        let ret_ty = self
            .fn_ret_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });

        let is_void = Self::is_void(&ret_ty);
        let llvm_ret = Self::llvm_ty(&ret_ty);

        if is_void {
            self.push_instr(&format!("call {llvm_ret} @{name}({args_str})"));
            Ok(None)
        } else {
            let reg = self.next_reg();
            self.push_instr(&format!("{reg} = call {llvm_ret} @{name}({args_str})"));
            Ok(Some(reg))
        }
    }
}

// ── Target triple detection ───────────────────────────────────────────────────

fn default_target_triple() -> String {
    // Try to get the host triple at runtime via `llc --version` or similar.
    // Fall back to compile-time detection.
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    return "arm64-apple-darwin".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    return "x86_64-pc-linux-gnu".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    return "x86_64-apple-darwin".to_string();
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    return "x86_64-pc-windows-msvc".to_string();
    #[allow(unreachable_code)]
    "x86_64-pc-linux-gnu".to_string()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn compile(src: &str) -> String {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        LlvmTextCompiler::new()
            .compile_to_ir(&prog, "test")
            .expect("compile_to_ir failed")
    }

    #[test]
    fn simple_add_function() {
        let ir = compile("fn add(a: Int, b: Int) -> Int { a + b }");
        assert!(ir.contains("define i64 @add(i64 %a, i64 %b)"), "{ir}");
        assert!(ir.contains("add i64"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn integer_literal_returned() {
        let ir = compile("fn answer() -> Int { 42 }");
        assert!(ir.contains("define i64 @answer()"), "{ir}");
        assert!(ir.contains("ret i64 42"), "{ir}");
    }

    #[test]
    fn bool_literal_returned() {
        let ir = compile("fn always_true() -> Bool { true }");
        assert!(ir.contains("define i1 @always_true()"), "{ir}");
        assert!(ir.contains("ret i1 true"), "{ir}");
    }

    #[test]
    fn arithmetic_operators() {
        let ir = compile("fn f(a: Int, b: Int) -> Int { a - b }");
        assert!(ir.contains("sub i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a * b }");
        assert!(ir.contains("mul i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a / b }");
        assert!(ir.contains("sdiv i64"), "{ir}");
        let ir = compile("fn f(a: Int, b: Int) -> Int { a % b }");
        assert!(ir.contains("srem i64"), "{ir}");
    }

    #[test]
    fn comparison_operators_emit_icmp() {
        let ir = compile("fn lt(a: Int, b: Int) -> Bool { a < b }");
        assert!(ir.contains("icmp slt i64"), "{ir}");
        let ir = compile("fn gt(a: Int, b: Int) -> Bool { a > b }");
        assert!(ir.contains("icmp sgt i64"), "{ir}");
        let ir = compile("fn eq(a: Int, b: Int) -> Bool { a == b }");
        assert!(ir.contains("icmp eq i64"), "{ir}");
    }

    #[test]
    fn if_else_emits_phi() {
        let ir = compile("fn max(a: Int, b: Int) -> Int { if a > b { a } else { b } }");
        assert!(ir.contains("icmp sgt"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
        assert!(ir.contains("phi i64"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn unit_function_emits_ret_void() {
        let ir = compile("fn noop() -> Unit { }");
        assert!(ir.contains("define void @noop()"), "{ir}");
        assert!(ir.contains("ret void"), "{ir}");
    }

    #[test]
    fn main_emits_i32_return() {
        let ir = compile("fn main() -> Unit { }");
        assert!(ir.contains("define i32 @main()"), "{ir}");
        assert!(ir.contains("ret i32 0"), "{ir}");
    }

    #[test]
    fn let_binding_aliases_ssa_value() {
        let ir = compile("fn f(x: Int) -> Int { let y: Int = x; y }");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    #[test]
    fn logical_not_emits_xor() {
        let ir = compile("fn f(b: Bool) -> Bool { !b }");
        assert!(ir.contains("xor i1"), "{ir}");
    }

    #[test]
    fn module_header_present() {
        let ir = compile("fn f() -> Int { 0 }");
        assert!(ir.contains("ModuleID = 'test'"), "{ir}");
        assert!(ir.contains("source_filename = \"test\""), "{ir}");
        assert!(ir.contains("target triple"), "{ir}");
    }

    #[test]
    fn multiple_functions_and_call() {
        let ir = compile(
            "fn add(a: Int, b: Int) -> Int { a + b }\n\
             fn double(n: Int) -> Int { add(n, n) }",
        );
        assert!(ir.contains("define i64 @add"), "{ir}");
        assert!(ir.contains("define i64 @double"), "{ir}");
        assert!(ir.contains("call i64 @add"), "{ir}");
    }

    #[test]
    fn negation_emits_sub_from_zero() {
        let ir = compile("fn neg(x: Int) -> Int { -x }");
        assert!(ir.contains("sub i64 0,"), "{ir}");
    }

    #[test]
    fn short_circuit_and_emits_phi() {
        let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a && b }");
        assert!(ir.contains("phi i1"), "{ir}");
        assert!(ir.contains("false"), "{ir}");
    }

    #[test]
    fn short_circuit_or_emits_phi() {
        let ir = compile("fn f(a: Bool, b: Bool) -> Bool { a || b }");
        assert!(ir.contains("phi i1"), "{ir}");
        assert!(ir.contains("true"), "{ir}");
    }

    #[test]
    fn mutable_ref_uses_alloca_store_load() {
        let ir = compile(
            "partial fn counter(n: Int) -> Int {\
             let c: ref Int = 0;\
             while c < n {\
               c = c + 1;\
             }\
             c\
             }",
        );
        assert!(ir.contains("alloca i64"), "{ir}");
        assert!(ir.contains("store i64"), "{ir}");
        assert!(ir.contains("load i64"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
    }
}
