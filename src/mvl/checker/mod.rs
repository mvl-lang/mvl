//! MVL type checker — verifies requirements 1, 3, 4, 5, 6, 10.
//!
//! The checker runs after parsing and before transpilation.  It reports
//! [`CheckError`] values for every violation found; unlike the parser, it
//! does not short-circuit on the first error.
//!
//! # Architecture
//!
//! ```text
//! Program
//!   └─ pass 1: collect_declarations  (populate type/function tables)
//!   └─ pass 2: check_declarations    (verify each decl)
//!              └─ check_fn_decl      (type-check function body)
//!                 └─ check_block / check_stmt / infer_expr
//! ```

pub mod context;
pub mod errors;
pub mod types;

use crate::mvl::checker::context::{
    field_infos, variant_infos, FnInfo, TypeBodyInfo, TypeEnv, TypeInfo, VarInfo,
};
use crate::mvl::checker::errors::CheckError;
use crate::mvl::checker::types::{resolve, types_compatible, Ty};
use crate::mvl::parser::ast::{
    BinaryOp, Block, ConstDecl, Decl, ElseBranch, Expr, FnDecl, LValue, Literal, MatchArm,
    MatchBody, ModuleDecl, Pattern, Program, Stmt, TypeBody, TypeDecl, UnaryOp,
};
use crate::mvl::parser::lexer::Span;

// ── Public API ───────────────────────────────────────────────────────────────

/// Result of running the type checker over a [`Program`].
#[derive(Debug, Default)]
pub struct CheckResult {
    pub errors: Vec<CheckError>,
}

impl CheckResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Entry point: type-check a parsed [`Program`].
pub fn check(prog: &Program) -> CheckResult {
    let mut checker = TypeChecker::new();
    checker.check_program(prog);
    CheckResult {
        errors: checker.errors,
    }
}

// ── TypeChecker ──────────────────────────────────────────────────────────────

struct TypeChecker {
    errors: Vec<CheckError>,
    env: TypeEnv,
    /// Return type of the function currently being checked (for `?` and `return`).
    current_return_ty: Option<Ty>,
}

impl TypeChecker {
    fn new() -> Self {
        TypeChecker {
            errors: Vec::new(),
            env: TypeEnv::new(),
            current_return_ty: None,
        }
    }

    fn emit(&mut self, err: CheckError) {
        self.errors.push(err);
    }

    // ── Program ──────────────────────────────────────────────────────────

    fn check_program(&mut self, prog: &Program) {
        self.collect_declarations(&prog.declarations);
        for decl in &prog.declarations {
            self.check_decl(decl);
        }
    }

    /// Pass 1: register all type and function signatures so forward references work.
    fn collect_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match decl {
                Decl::Type(td) => self.register_type(td),
                Decl::Fn(fd) => self.register_fn(fd),
                Decl::Const(_) => {}
                Decl::Module(md) => self.collect_declarations(&md.declarations),
            }
        }
    }

    fn register_type(&mut self, td: &TypeDecl) {
        let body_info = match &td.body {
            TypeBody::Struct(fields) => TypeBodyInfo::Struct(field_infos(fields)),
            TypeBody::Enum(variants) => TypeBodyInfo::Enum(variant_infos(variants)),
            TypeBody::Alias(ty_expr) => TypeBodyInfo::Alias(resolve(ty_expr)),
        };
        self.env.define_type(
            td.name.clone(),
            TypeInfo {
                params: td.params.clone(),
                body: body_info,
            },
        );
    }

    fn register_fn(&mut self, fd: &FnDecl) {
        let params: Vec<Ty> = fd.params.iter().map(|p| resolve(&p.ty)).collect();
        let ret = resolve(&fd.return_type);
        self.env.define_fn(fd.name.clone(), FnInfo { params, ret });
    }

    // ── Declarations ─────────────────────────────────────────────────────

    fn check_decl(&mut self, decl: &Decl) {
        match decl {
            Decl::Type(_) => {} // type declarations are structurally valid if parsed
            Decl::Fn(fd) => self.check_fn_decl(fd),
            Decl::Const(cd) => self.check_const_decl(cd),
            Decl::Module(md) => self.check_module_decl(md),
        }
    }

    fn check_fn_decl(&mut self, fd: &FnDecl) {
        let ret_ty = resolve(&fd.return_type);
        let prev_ret = self.current_return_ty.replace(ret_ty.clone());

        self.env.push_scope();
        for param in &fd.params {
            let ty = resolve(&param.ty);
            self.env
                .define(param.name.clone(), VarInfo::new(ty, param.mutable));
        }

        self.check_block(&fd.body, Some(&ret_ty));
        self.env.pop_scope();
        self.current_return_ty = prev_ret;
    }

    fn check_const_decl(&mut self, cd: &ConstDecl) {
        let expected = resolve(&cd.ty);
        let found = self.infer_expr(&cd.value);
        if !types_compatible(&expected, &found) {
            self.emit(CheckError::TypeMismatch {
                expected: expected.display(),
                found: found.display(),
                span: cd.value.span(),
            });
        }
    }

    fn check_module_decl(&mut self, md: &ModuleDecl) {
        self.collect_declarations(&md.declarations);
        for decl in &md.declarations {
            self.check_decl(decl);
        }
    }

    // ── Blocks and statements ─────────────────────────────────────────────

    fn check_block(&mut self, block: &Block, expected_ty: Option<&Ty>) {
        self.env.push_scope();
        for stmt in &block.stmts {
            self.check_stmt(stmt, expected_ty);
        }
        self.env.pop_scope();
    }

    fn check_stmt(&mut self, stmt: &Stmt, return_ty: Option<&Ty>) {
        match stmt {
            Stmt::Let {
                mutable,
                pattern,
                ty,
                init,
                span,
            } => {
                let init_ty = self.infer_expr(init);
                if let Some(ann) = ty {
                    let ann_ty = resolve(ann);
                    if !types_compatible(&ann_ty, &init_ty) {
                        self.emit(CheckError::TypeMismatch {
                            expected: ann_ty.display(),
                            found: init_ty.display(),
                            span: init.span(),
                        });
                    }
                    self.bind_pattern(pattern, &ann_ty, *mutable);
                } else {
                    self.bind_pattern(pattern, &init_ty, *mutable);
                }
                // #14: ResultIgnored — if the init expression is a Result and
                // it's not being used at all, that would be caught at Stmt::Expr.
                // Here the Result is being bound, which is acceptable.
                let _ = span;
            }

            // #17: immutability enforcement
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let val_ty = self.infer_expr(value);
                self.check_assignment(target, &val_ty, *span);
            }

            Stmt::Return { value, span } => {
                if let Some(expr) = value {
                    let found = self.infer_expr(expr);
                    if let Some(ret) = return_ty {
                        if !types_compatible(ret, &found) {
                            self.emit(CheckError::TypeMismatch {
                                expected: ret.display(),
                                found: found.display(),
                                span: *span,
                            });
                        }
                    }
                }
            }

            Stmt::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                self.check_block(then, return_ty);
                if let Some(else_branch) = else_ {
                    match else_branch {
                        ElseBranch::Block(b) => self.check_block(b, return_ty),
                        ElseBranch::If(s) => self.check_stmt(s, return_ty),
                    }
                }
                let _ = span;
            }

            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                self.check_match_arms(arms, &scrutinee_ty, *span, return_ty);
            }

            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => {
                let iter_ty = self.infer_expr(iter);
                let elem_ty = match iter_ty.base() {
                    Ty::List(inner) => *inner.clone(),
                    _ => Ty::Unknown,
                };
                self.env.push_scope();
                self.bind_pattern(pattern, &elem_ty, false);
                self.check_block(body, return_ty);
                self.env.pop_scope();
            }

            Stmt::While { cond, body, .. } => {
                let cond_ty = self.infer_expr(cond);
                if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                self.check_block(body, return_ty);
            }

            // #14: Reject bare Result expressions (ResultIgnored)
            Stmt::Expr { expr, .. } => {
                let ty = self.infer_expr(expr);
                if ty.is_result() {
                    self.emit(CheckError::ResultIgnored { span: expr.span() });
                }
            }
        }
    }

    // ── Assignment target (#17 immutability) ─────────────────────────────

    fn check_assignment(&mut self, target: &LValue, _val_ty: &Ty, span: Span) {
        match target {
            LValue::Ident(name, _) => {
                if let Some(info) = self.env.lookup(name) {
                    if !info.mutable {
                        self.emit(CheckError::AssignToImmutable {
                            name: name.clone(),
                            span,
                        });
                    }
                } else {
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span,
                    });
                }
            }
            LValue::Field {
                base,
                field,
                span: field_span,
            } => {
                // Check that the base is accessible
                let base_ty = self.infer_lvalue(base);
                self.check_field_mutation(&base_ty, field, *field_span);
                self.check_assignment(base, _val_ty, span);
            }
        }
    }

    fn infer_lvalue(&self, target: &LValue) -> Ty {
        match target {
            LValue::Ident(name, _) => self
                .env
                .lookup(name)
                .map(|i| i.ty.clone())
                .unwrap_or(Ty::Unknown),
            LValue::Field { base, field, .. } => {
                let base_ty = self.infer_lvalue(base);
                self.field_type(&base_ty, field).unwrap_or(Ty::Unknown)
            }
        }
    }

    fn check_field_mutation(&mut self, ty: &Ty, field: &str, span: Span) {
        let base = ty.base();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name).cloned() {
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
                    if let Some(fi) = fields.iter().find(|f| f.name == field) {
                        if !fi.mutable {
                            self.emit(CheckError::MutateImmutableField {
                                ty: name.clone(),
                                field: field.to_string(),
                                span,
                            });
                        }
                    }
                }
            }
        }
    }

    // ── Expression type inference ─────────────────────────────────────────

    pub fn infer_expr(&mut self, expr: &Expr) -> Ty {
        match expr {
            // #11: Literals
            Expr::Literal(lit, _) => self.infer_literal(lit),

            // #11/#15: Variable reference
            Expr::Ident(name, span) => {
                if let Some(info) = self.env.lookup(name).cloned() {
                    // #15: ownership — reject use after move
                    if info.moved {
                        self.emit(CheckError::UseAfterMove {
                            name: name.clone(),
                            span: *span,
                        });
                        return Ty::Unknown;
                    }
                    info.ty.clone()
                } else {
                    self.emit(CheckError::UndefinedVariable {
                        name: name.clone(),
                        span: *span,
                    });
                    Ty::Unknown
                }
            }

            // #11: Binary operations
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.infer_binary(*op, left, right, *span),

            Expr::Unary { op, expr, span } => self.infer_unary(*op, expr, *span),

            // #12: Field access — reject direct field access on enum or Option
            Expr::FieldAccess { expr, field, span } => {
                let ty = self.infer_expr(expr);
                // #14: Option direct access
                if ty.is_option() {
                    self.emit(CheckError::OptionDirectAccess { span: *span });
                    return Ty::Unknown;
                }
                self.field_type_checked(&ty, field, *span)
            }

            // #11: Function call
            Expr::FnCall {
                name, args, span, ..
            } => self.infer_fn_call(name, args, *span),

            Expr::MethodCall {
                receiver,
                method,
                args,
                span,
            } => {
                let _recv_ty = self.infer_expr(receiver);
                for arg in args {
                    self.infer_expr(arg);
                }
                let _ = (method, span);
                Ty::Unknown // method resolution not yet implemented
            }

            // #13: Match expressions
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                self.infer_match_expr(arms, &scrutinee_ty, *span)
            }

            Expr::If {
                cond,
                then,
                else_,
                span,
            } => {
                let cond_ty = self.infer_expr(cond);
                if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: cond_ty.display(),
                        span: cond.span(),
                    });
                }
                self.check_block(then, None);
                if let Some(else_expr) = else_ {
                    self.infer_expr(else_expr)
                } else {
                    let _ = span;
                    Ty::Unit
                }
            }

            Expr::Block(block) => {
                self.check_block(block, None);
                Ty::Unit
            }

            // #12: Struct construction
            Expr::Construct { name, fields, span } => self.check_construction(name, fields, *span),

            Expr::List { elems, .. } => {
                let elem_ty = elems
                    .first()
                    .map(|e| self.infer_expr(e))
                    .unwrap_or(Ty::Unknown);
                for e in elems.iter().skip(1) {
                    self.infer_expr(e);
                }
                Ty::List(Box::new(elem_ty))
            }

            // #14: `?` propagation
            Expr::Propagate { expr, span } => {
                let ty = self.infer_expr(expr);
                if !ty.is_propagatable() && !matches!(ty, Ty::Unknown) {
                    self.emit(CheckError::PropagateNotResult {
                        ty: ty.display(),
                        span: *span,
                    });
                    return Ty::Unknown;
                }
                ty.propagate_inner()
            }

            // #15: explicit move — infer first, then mark as moved so
            // subsequent references to the same binding are caught.
            Expr::Move { expr, .. } => {
                let ty = self.infer_expr(expr);
                if let Expr::Ident(name, _) = expr.as_ref() {
                    self.env.mark_moved(name);
                }
                ty
            }

            Expr::Consume { expr, .. } => self.infer_expr(expr),
            Expr::Declassify { expr, .. } => self.infer_expr(expr),
            Expr::Sanitize { expr, .. } => self.infer_expr(expr),

            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => {
                self.env.push_scope();
                let param_tys: Vec<Ty> = params
                    .iter()
                    .map(|p| {
                        let ty = resolve(&p.ty);
                        self.env
                            .define(p.name.clone(), VarInfo::new(ty.clone(), p.mutable));
                        ty
                    })
                    .collect();
                let ret_ty = ret_type.as_ref().map(|t| resolve(t)).unwrap_or(Ty::Unknown);
                self.infer_expr(body);
                self.env.pop_scope();
                Ty::Fn(param_tys, Box::new(ret_ty))
            }
        }
    }

    // ── Literal types (#11) ───────────────────────────────────────────────

    fn infer_literal(&self, lit: &Literal) -> Ty {
        match lit {
            Literal::Integer(_) => Ty::Int,
            Literal::Float(_) => Ty::Float,
            Literal::Str(_) => Ty::String,
            Literal::Char(_) => Ty::Char,
            Literal::Bool(_) => Ty::Bool,
        }
    }

    // ── Binary operations (#11) ───────────────────────────────────────────

    fn infer_binary(&mut self, op: BinaryOp, left: &Expr, right: &Expr, span: Span) -> Ty {
        let lt = self.infer_expr(left);
        let rt = self.infer_expr(right);

        match op {
            // Arithmetic: both operands must be numeric and the same type
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                if !matches!(lt, Ty::Unknown) && !lt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: lt.display(),
                        span: left.span(),
                    });
                    return Ty::Unknown;
                }
                if !matches!(rt, Ty::Unknown) && !rt.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: rt.display(),
                        span: right.span(),
                    });
                    return Ty::Unknown;
                }
                if !matches!(lt, Ty::Unknown) && !matches!(rt, Ty::Unknown) && lt != rt {
                    self.emit(CheckError::ArithmeticTypeMismatch {
                        op: format!("{op:?}").to_lowercase(),
                        left: lt.display(),
                        right: rt.display(),
                        span,
                    });
                    return Ty::Unknown;
                }
                if matches!(lt, Ty::Unknown) {
                    rt
                } else {
                    lt
                }
            }

            // Comparison: both sides same type → Bool
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => {
                if !matches!(lt, Ty::Unknown)
                    && !matches!(rt, Ty::Unknown)
                    && !types_compatible(&lt, &rt)
                {
                    self.emit(CheckError::TypeMismatch {
                        expected: lt.display(),
                        found: rt.display(),
                        span,
                    });
                }
                Ty::Bool
            }

            // Logic: both must be Bool
            BinaryOp::And | BinaryOp::Or => {
                let op_str = format!("{op:?}").to_lowercase();
                if !matches!(lt, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str.clone(),
                        ty: lt.display(),
                        span: left.span(),
                    });
                }
                if !matches!(rt, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::LogicTypeMismatch {
                        op: op_str,
                        ty: rt.display(),
                        span: right.span(),
                    });
                }
                Ty::Bool
            }
        }
    }

    fn infer_unary(&mut self, op: UnaryOp, expr: &Expr, span: Span) -> Ty {
        let ty = self.infer_expr(expr);
        match op {
            UnaryOp::Neg => {
                if !matches!(ty, Ty::Unknown) && !ty.is_numeric() {
                    self.emit(CheckError::NonNumericArithmetic {
                        ty: ty.display(),
                        span,
                    });
                    Ty::Unknown
                } else {
                    ty
                }
            }
            UnaryOp::Not => {
                if !matches!(ty, Ty::Bool | Ty::Unknown) {
                    self.emit(CheckError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: ty.display(),
                        span,
                    });
                }
                Ty::Bool
            }
        }
    }

    // ── Function calls (#11) ──────────────────────────────────────────────

    fn infer_fn_call(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
        // Infer all argument types (for side-effect error collection)
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.infer_expr(a)).collect();

        if let Some(fn_info) = self.env.lookup_fn(name).cloned() {
            if fn_info.params.len() != arg_tys.len() {
                self.emit(CheckError::WrongArgCount {
                    name: name.to_string(),
                    expected: fn_info.params.len(),
                    found: arg_tys.len(),
                    span,
                });
                return fn_info.ret.clone();
            }
            for (i, (expected, found)) in fn_info.params.iter().zip(arg_tys.iter()).enumerate() {
                if !types_compatible(expected, found) {
                    self.emit(CheckError::TypeMismatch {
                        expected: expected.display(),
                        found: found.display(),
                        span: args[i].span(),
                    });
                }
            }
            fn_info.ret.clone()
        } else {
            // Not in function table — could be builtin or foreign; emit Unknown
            self.emit(CheckError::UndefinedFunction {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }

    // ── Field access (#12) ────────────────────────────────────────────────

    /// Look up a field type without emitting errors.
    fn field_type(&self, ty: &Ty, field: &str) -> Option<Ty> {
        let base = ty.base();
        if let Ty::Named(name, _) = base {
            if let Some(type_info) = self.env.lookup_type(name) {
                if let TypeBodyInfo::Struct(fields) = &type_info.body {
                    return fields
                        .iter()
                        .find(|f| f.name == field)
                        .map(|f| f.ty.clone());
                }
            }
        }
        None
    }

    /// Look up a field type, emitting errors for violations.
    fn field_type_checked(&mut self, ty: &Ty, field: &str, span: Span) -> Ty {
        let base = ty.base().clone();
        match &base {
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    match &type_info.body {
                        TypeBodyInfo::Struct(fields) => {
                            if let Some(fi) = fields.iter().find(|f| f.name == field) {
                                fi.ty.clone()
                            } else {
                                self.emit(CheckError::FieldNotFound {
                                    ty: name.clone(),
                                    field: field.to_string(),
                                    span,
                                });
                                Ty::Unknown
                            }
                        }
                        TypeBodyInfo::Enum(_) => {
                            self.emit(CheckError::FieldAccessOnEnum {
                                ty: name.clone(),
                                span,
                            });
                            Ty::Unknown
                        }
                        TypeBodyInfo::Alias(inner) => {
                            self.field_type_checked(&inner.clone(), field, span)
                        }
                    }
                } else {
                    // Unknown named type — already reported elsewhere
                    Ty::Unknown
                }
            }
            Ty::Unknown => Ty::Unknown,
            other => {
                self.emit(CheckError::FieldNotFound {
                    ty: other.display(),
                    field: field.to_string(),
                    span,
                });
                Ty::Unknown
            }
        }
    }

    // ── Struct construction (#12) ─────────────────────────────────────────

    fn check_construction(&mut self, name: &str, fields: &[(String, Expr)], span: Span) -> Ty {
        // Infer all provided field values
        let provided: Vec<(String, Ty)> = fields
            .iter()
            .map(|(fname, fexpr)| (fname.clone(), self.infer_expr(fexpr)))
            .collect();

        if let Some(type_info) = self.env.lookup_type(name).cloned() {
            match &type_info.body {
                TypeBodyInfo::Struct(declared_fields) => {
                    // Check that all declared fields are provided
                    for df in declared_fields.iter() {
                        if !provided.iter().any(|(pname, _)| pname == &df.name) {
                            self.emit(CheckError::MissingField {
                                ty: name.to_string(),
                                field: df.name.clone(),
                                span,
                            });
                        }
                    }
                    // Check no extra fields are provided
                    for (pname, pty) in &provided {
                        if let Some(df) = declared_fields.iter().find(|f| &f.name == pname) {
                            if !types_compatible(&df.ty, pty) {
                                self.emit(CheckError::TypeMismatch {
                                    expected: df.ty.display(),
                                    found: pty.display(),
                                    span,
                                });
                            }
                        } else {
                            self.emit(CheckError::UnknownField {
                                ty: name.to_string(),
                                field: pname.clone(),
                                span,
                            });
                        }
                    }
                    Ty::Named(name.to_string(), vec![])
                }
                TypeBodyInfo::Enum(_) => {
                    // Enum variant construction — name might be "EnumType::Variant"
                    // For now just return the type
                    Ty::Named(name.to_string(), vec![])
                }
                TypeBodyInfo::Alias(inner) => inner.clone(),
            }
        } else {
            // Unknown type
            self.emit(CheckError::UndefinedType {
                name: name.to_string(),
                span,
            });
            Ty::Unknown
        }
    }

    // ── Match exhaustiveness (#13) ────────────────────────────────────────

    fn infer_match_expr(&mut self, arms: &[MatchArm], scrutinee_ty: &Ty, span: Span) -> Ty {
        self.check_match_arms(arms, scrutinee_ty, span, None)
    }

    /// Check match arms for exhaustiveness and return the result type.
    fn check_match_arms(
        &mut self,
        arms: &[MatchArm],
        scrutinee_ty: &Ty,
        span: Span,
        return_ty: Option<&Ty>,
    ) -> Ty {
        // Check each arm body
        let mut arm_tys: Vec<Ty> = Vec::new();
        for arm in arms {
            self.env.push_scope();
            self.bind_match_pattern(&arm.pattern, scrutinee_ty);
            let body_ty = match &arm.body {
                MatchBody::Expr(e) => self.infer_expr(e),
                MatchBody::Block(b) => {
                    self.check_block(b, return_ty);
                    Ty::Unit
                }
            };
            self.env.pop_scope();
            arm_tys.push(body_ty);
        }

        // Exhaustiveness check
        self.check_exhaustiveness(arms, scrutinee_ty, span);

        arm_tys
            .into_iter()
            .find(|t| !matches!(t, Ty::Unknown))
            .unwrap_or(Ty::Unknown)
    }

    fn check_exhaustiveness(&mut self, arms: &[MatchArm], scrutinee_ty: &Ty, span: Span) {
        let base = scrutinee_ty.base().clone();

        match &base {
            // Option<T>: must cover Some(_) and None
            Ty::Option(_) => {
                // A bare `_` or non-Option-variant ident is a wildcard → exhaustive
                if arms.iter().any(|a| is_wildcard_pattern(&a.pattern, &[])) {
                    return;
                }
                let has_some = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Some { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Some")
                });
                let has_none = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::None(_))
                        || matches!(&a.pattern, Pattern::Ident(n, _) if n == "None")
                });
                let mut missing = Vec::new();
                if !has_some {
                    missing.push("Some(_)".to_string());
                }
                if !has_none {
                    missing.push("None".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Result<T,E>: must cover Ok(_) and Err(_)
            Ty::Result(_, _) => {
                if arms.iter().any(|a| is_wildcard_pattern(&a.pattern, &[])) {
                    return;
                }
                let has_ok = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Ok { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Ok")
                });
                let has_err = arms.iter().any(|a| {
                    matches!(a.pattern, Pattern::Err { .. })
                        || matches!(&a.pattern, Pattern::TupleStruct { name, .. } if name == "Err")
                });
                let mut missing = Vec::new();
                if !has_ok {
                    missing.push("Ok(_)".to_string());
                }
                if !has_err {
                    missing.push("Err(_)".to_string());
                }
                if !missing.is_empty() {
                    self.emit(CheckError::NonExhaustiveMatch { missing, span });
                }
            }

            // Named enum: collect which variants are covered
            Ty::Named(name, _) => {
                if let Some(type_info) = self.env.lookup_type(name).cloned() {
                    if let TypeBodyInfo::Enum(variants) = &type_info.body {
                        let variant_names: Vec<String> =
                            variants.iter().map(|v| v.name.clone()).collect();

                        // A wildcard is any Pattern::Wildcard OR a bare ident not in the enum's variants
                        if arms
                            .iter()
                            .any(|a| is_wildcard_pattern(&a.pattern, &variant_names))
                        {
                            return;
                        }

                        // Collect which variant names are explicitly covered
                        let covered: Vec<String> = arms
                            .iter()
                            .filter_map(|arm| covered_variant_name(&arm.pattern, &variant_names))
                            .collect();

                        let missing: Vec<String> = variant_names
                            .iter()
                            .filter(|v| !covered.contains(v))
                            .cloned()
                            .collect();
                        if !missing.is_empty() {
                            self.emit(CheckError::NonExhaustiveMatch { missing, span });
                        }
                    }
                }
                // Unknown type or non-enum → no exhaustiveness check
            }

            _ => {} // literals, bools, tuples — skip exhaustiveness
        }
    }

    // ── Pattern binding ───────────────────────────────────────────────────

    fn bind_pattern(&mut self, pattern: &Pattern, ty: &Ty, mutable: bool) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(ty.clone(), mutable));
            }
            Pattern::Wildcard(_) => {}
            Pattern::Tuple { elems, .. } => {
                if let Ty::Tuple(elem_tys) = ty.base() {
                    for (p, t) in elems.iter().zip(elem_tys.iter()) {
                        self.bind_pattern(p, t, mutable);
                    }
                } else {
                    for p in elems {
                        self.bind_pattern(p, &Ty::Unknown, mutable);
                    }
                }
            }
            Pattern::Literal(_, _) => {}
            _ => {
                // For struct/tuple-struct patterns, just bind sub-patterns as Unknown
                self.bind_sub_patterns(pattern, mutable);
            }
        }
    }

    fn bind_match_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Ty) {
        match pattern {
            Pattern::Ident(name, _) => {
                self.env
                    .define(name.clone(), VarInfo::new(scrutinee_ty.clone(), false));
            }
            Pattern::Wildcard(_) | Pattern::Literal(_, _) | Pattern::None(_) => {}
            Pattern::Some { inner, .. } => {
                let inner_ty = match scrutinee_ty.base() {
                    Ty::Option(t) => *t.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Ok { inner, .. } => {
                let inner_ty = match scrutinee_ty.base() {
                    Ty::Result(ok, _) => *ok.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::Err { inner, .. } => {
                let inner_ty = match scrutinee_ty.base() {
                    Ty::Result(_, err) => *err.clone(),
                    _ => Ty::Unknown,
                };
                self.bind_match_pattern(inner, &inner_ty);
            }
            Pattern::TupleStruct { fields, .. } => {
                for p in fields {
                    self.bind_match_pattern(p, &Ty::Unknown);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_match_pattern(p, &Ty::Unknown);
                }
            }
            Pattern::Tuple { elems, .. } => {
                let elem_tys = match scrutinee_ty.base() {
                    Ty::Tuple(ts) => ts.clone(),
                    _ => vec![],
                };
                for (i, p) in elems.iter().enumerate() {
                    let ty = elem_tys.get(i).cloned().unwrap_or(Ty::Unknown);
                    self.bind_match_pattern(p, &ty);
                }
            }
        }
    }

    fn bind_sub_patterns(&mut self, pattern: &Pattern, mutable: bool) {
        match pattern {
            Pattern::TupleStruct { fields, .. } => {
                for p in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Struct { fields, .. } => {
                for (_, p) in fields {
                    self.bind_pattern(p, &Ty::Unknown, mutable);
                }
            }
            Pattern::Some { inner, .. }
            | Pattern::Ok { inner, .. }
            | Pattern::Err { inner, .. } => {
                self.bind_pattern(inner, &Ty::Unknown, mutable);
            }
            _ => {}
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// True if `pattern` acts as a catch-all / wildcard in the context of an enum
/// whose variants are listed in `variant_names`.
///
/// - `Pattern::Wildcard` is always a wildcard.
/// - `Pattern::Ident(name)` is a wildcard when `name` is NOT a known variant
///   (it's a variable binding, not a variant tag).
fn is_wildcard_pattern(pattern: &Pattern, variant_names: &[String]) -> bool {
    match pattern {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => !variant_names.contains(name),
        _ => false,
    }
}

/// Extract the variant name that a pattern explicitly covers, given the set of
/// known variant names.  Returns `None` for non-variant or wildcard patterns.
fn covered_variant_name(pattern: &Pattern, variant_names: &[String]) -> Option<String> {
    match pattern {
        Pattern::TupleStruct { name, .. } | Pattern::Struct { name, .. } => Some(name.clone()),
        // A bare ident that IS a known variant name counts as that variant
        Pattern::Ident(name, _) if variant_names.contains(name) => Some(name.clone()),
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn check_src(src: &str) -> CheckResult {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        check(&prog)
    }

    fn errors_for(src: &str) -> Vec<CheckError> {
        check_src(src).errors
    }

    // ── Requirement 1 / Scenario: Basic type inference (#11) ─────────────

    #[test]
    fn literal_int_inferred() {
        let result = check_src("fn f() -> Int { 42 }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn literal_bool_inferred() {
        let result = check_src("fn f() -> Bool { true }");
        assert!(result.is_ok(), "errors: {:?}", result.errors);
    }

    #[test]
    fn arithmetic_requires_numeric() {
        let errors = errors_for("fn f() -> Int { true + 1 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::NonNumericArithmetic { .. })),
            "expected NonNumericArithmetic, got: {errors:?}"
        );
    }

    #[test]
    fn arithmetic_mixed_types_rejected() {
        let errors = errors_for("fn f() -> Float { 1 + 2.0 }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ArithmeticTypeMismatch { .. })),
            "expected ArithmeticTypeMismatch, got: {errors:?}"
        );
    }

    #[test]
    fn logic_requires_bool() {
        let errors = errors_for("fn f() -> Bool { 1 && true }");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::LogicTypeMismatch { .. })),
            "expected LogicTypeMismatch, got: {errors:?}"
        );
    }

    // ── Requirement 1 / Scenario: ADT checking (#12) ─────────────────────

    #[test]
    fn struct_construction_valid() {
        let src =
            "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1, y: 2 } }";
        let result = check_src(src);
        // UndefinedFunction for `Point { x: 1, y: 2 }` should not appear;
        // struct construction goes through Construct not FnCall
        let serious: Vec<_> = result
            .errors
            .iter()
            .filter(|e| !matches!(e, CheckError::TypeMismatch { .. }))
            .collect();
        assert!(
            serious.iter().all(|e| !matches!(
                e,
                CheckError::MissingField { .. } | CheckError::UnknownField { .. }
            )),
            "unexpected errors: {serious:?}"
        );
    }

    #[test]
    fn struct_missing_field_rejected() {
        let src = "type Point = struct { x: Int, y: Int }\nfn make() -> Point { Point { x: 1 } }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::MissingField { field, .. } if field == "y")),
            "expected MissingField(y), got: {errors:?}"
        );
    }

    #[test]
    fn field_access_on_enum_rejected() {
        let src = "type Color = enum { Red, Green, Blue }\nfn f(c: Color) -> Int { c.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::FieldAccessOnEnum { .. })),
            "expected FieldAccessOnEnum, got: {errors:?}"
        );
    }

    // ── Requirement 3 / Scenario: Exhaustive match (#13) ─────────────────

    #[test]
    fn option_match_exhaustive() {
        let src = "fn f(x: Option<Int>) -> Int { match x { Some(v) => v, None => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn option_match_missing_none_rejected() {
        let src = "fn f(x: Option<Int>) -> Int { match x { Some(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"None".to_string())
            )),
            "expected NonExhaustiveMatch(None), got: {errors:?}"
        );
    }

    #[test]
    fn result_match_exhaustive() {
        let src = "fn f(x: Result<Int, String>) -> Int { match x { Ok(v) => v, Err(_) => 0 } }";
        let result = check_src(src);
        let exhaustive_errors: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::NonExhaustiveMatch { .. }))
            .collect();
        assert!(
            exhaustive_errors.is_empty(),
            "should be exhaustive, got: {exhaustive_errors:?}"
        );
    }

    #[test]
    fn result_match_missing_err_rejected() {
        let src = "fn f(x: Result<Int, String>) -> Int { match x { Ok(v) => v } }";
        let errors = errors_for(src);
        assert!(
            errors.iter().any(|e| matches!(
                e,
                CheckError::NonExhaustiveMatch { missing, .. } if missing.contains(&"Err(_)".to_string())
            )),
            "expected NonExhaustiveMatch(Err(_)), got: {errors:?}"
        );
    }

    // ── Requirement 4/5 / Scenario: Option/Result enforcement (#14) ───────

    #[test]
    fn option_direct_access_rejected() {
        let src = "fn f(x: Option<Int>) -> Int { x.value }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::OptionDirectAccess { .. })),
            "expected OptionDirectAccess, got: {errors:?}"
        );
    }

    #[test]
    fn result_ignored_rejected() {
        let src = "fn produce() -> Result<Int, String> { Ok(1) }\nfn f() -> Unit { produce() }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::ResultIgnored { .. })),
            "expected ResultIgnored, got: {errors:?}"
        );
    }

    // ── Requirement 6 / Scenario: Immutability enforcement (#17) ──────────

    #[test]
    fn assign_to_immutable_rejected() {
        let src = "fn f() -> Unit { let x = 1; x = 2; }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::AssignToImmutable { name, .. } if name == "x")),
            "expected AssignToImmutable(x), got: {errors:?}"
        );
    }

    #[test]
    fn assign_to_mutable_allowed() {
        let src = "fn f() -> Unit { let mut x = 1; x = 2; }";
        let errors = errors_for(src);
        let assign_errors: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, CheckError::AssignToImmutable { .. }))
            .collect();
        assert!(
            assign_errors.is_empty(),
            "should allow mut assignment, got: {assign_errors:?}"
        );
    }

    // ── Requirement 2 / Scenario: Ownership / use-after-move (#15) ────────

    #[test]
    fn use_after_move_rejected() {
        // move(x) is the MVL syntax for explicit move
        let src = "fn f() -> Int { let x = 1; let y = move(x); x }";
        let errors = errors_for(src);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, CheckError::UseAfterMove { name, .. } if name == "x")),
            "expected UseAfterMove(x), got: {errors:?}"
        );
    }
}
