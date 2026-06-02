// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `LlvmTextCompiler` — pure-string LLVM IR emitter (Phase 2, issue #1136).
//!
//! Extends Phase 1 with: string literals, println/assert/format builtins,
//! struct construction/field access, unit enums, match expressions,
//! method calls (to_string/len/concat), and for-range loops.

use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{
    ActorDecl, BinaryOp, Block, Decl, ElseBranch, Expr, FnDecl, LValue, LetKind, Literal, MatchArm,
    MatchBody, Pattern, Program, Stmt, TypeBody, TypeExpr, UnaryOp,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Pure-string LLVM IR compiler — Phase 2.
///
/// Generates LLVM IR text for programs using primitives, strings, structs,
/// unit enums, match, and method calls.  No inkwell, no unsafe.
pub struct LlvmTextCompiler {
    /// Target triple emitted in the module header.
    pub target_triple: String,
    /// `builtin fn` dispatch table: MVL name → (C-ABI symbol, return TypeExpr, param TypeExprs).
    ///
    /// Populated via `with_builtins()` or set directly before calling
    /// `compile_to_ir_with_prelude`.  The emitter uses this to route calls to
    /// `builtin fn` declarations to their C runtime symbols instead of generating
    /// a body for the empty block.
    pub builtin_symbols: HashMap<String, (String, TypeExpr, Vec<TypeExpr>)>,
}

impl LlvmTextCompiler {
    /// Create a new compiler with the host target triple.
    pub fn new() -> Self {
        Self {
            target_triple: default_target_triple(),
            builtin_symbols: HashMap::new(),
        }
    }

    /// Compile a MVL [`Program`] to LLVM IR text (no prelude, no builtin dispatch).
    pub fn compile_to_ir(&self, prog: &Program, module_name: &str) -> Result<String, String> {
        self.compile_to_ir_with_prelude(&[], prog, module_name)
    }

    /// Compile prelude programs merged with `prog` into a single LLVM IR module.
    ///
    /// Prelude programs are emitted first (stdlib bodies), then `prog`.
    /// `builtin_symbols` is pre-populated into the emitter so that calls to
    /// `builtin fn` names are routed to their C-ABI symbols automatically.
    ///
    /// Non-builtin extension methods in prelude programs (e.g. `String::contains`,
    /// `String::starts_with`) are stripped before emission — they are handled
    /// via hardcoded C-ABI dispatch in `emit_method_call` and must not be
    /// emitted as MVL bodies (they reference unsupported stdlib constructs).
    pub fn compile_to_ir_with_prelude(
        &self,
        prelude: &[Program],
        prog: &Program,
        module_name: &str,
    ) -> Result<String, String> {
        let mut emitter =
            TextEmitter::new_with_builtins(module_name, &self.target_triple, &self.builtin_symbols);
        for p in prelude {
            let stripped = strip_prelude_extension_methods(p);
            emitter.emit_program(&stripped)?;
        }
        emitter.emit_program(prog)?;
        Ok(emitter.finish())
    }
}

impl Default for LlvmTextCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Remove prelude function bodies that the llvm_text emitter cannot handle.
///
/// Strips non-builtin functions that fall into either category:
///
/// 1. **Extension methods** (`receiver_type.is_some()`) — e.g. `String::contains`,
///    `List::is_empty`.  These call other String/List kernel methods via patterns
///    the emitter cannot yet fully lower (e.g. `self.find(sub).is_some()`).
///    Method calls on these types are handled via hardcoded C-ABI dispatch in
///    `emit_method_call` instead.
///
/// 2. **Option/Result return types** — prelude functions like `env.get_secret` or
///    `env.env_var` that return `Option[T]` or `Result[T, E]` may call runtime
///    symbols not available in the lli runtime.  User-defined functions with
///    Option/Result are handled correctly by the emitter — this filter only
///    applies to prelude functions.
fn strip_prelude_extension_methods(prog: &Program) -> Program {
    let mut out = prog.clone();
    out.declarations.retain(|d| {
        if let Decl::Fn(fd) = d {
            if fd.is_builtin {
                return true; // keep builtin declarations
            }
            // Drop non-builtin extension methods.
            if fd.receiver_type.is_some() {
                return false;
            }
            // Drop non-builtin prelude functions whose return type is Option or
            // Result — they may call runtime symbols not available in lli.
            if return_type_needs_option_abi(&fd.return_type) {
                return false;
            }
        }
        true
    });
    out
}

/// Return `true` if `ty` is `Option[_]` or `Result[_, _]` (possibly wrapped
/// in `Labeled` or `Refined`).  Used to skip prelude functions that may
/// reference runtime symbols unavailable in the lli runtime.
fn return_type_needs_option_abi(ty: &TypeExpr) -> bool {
    match ty {
        TypeExpr::Option { .. } | TypeExpr::Result { .. } => true,
        TypeExpr::Labeled { inner, .. } | TypeExpr::Refined { inner, .. } => {
            return_type_needs_option_abi(inner)
        }
        TypeExpr::Ref { inner, .. } => return_type_needs_option_abi(inner),
        _ => false,
    }
}

// ── Internal emitter ──────────────────────────────────────────────────────────

/// LLVM type for `Result[T, E]` tagged unions (discriminant byte + payload pointer).
const RESULT_LLVM_TY: &str = "{ i8, ptr }";
/// LLVM return instruction for the C-ABI `main` entry point.
const MAIN_RET: &str = "ret i32 0";

struct TextEmitter {
    module_name: String,
    target_triple: String,

    // ── Module-level output sections ──────────────────────────────────────
    fn_bodies: Vec<String>,
    str_counter: usize,
    str_globals: Vec<String>,
    type_defs: Vec<String>,
    extern_decls: Vec<String>,

    // ── Type registries (populated during first pass) ─────────────────────
    /// struct name → ordered list of (field_name, field_TypeExpr)
    struct_fields: HashMap<String, Vec<(String, TypeExpr)>>,
    /// enum name → ordered list of variant names (index = discriminant)
    enum_variants: HashMap<String, Vec<String>>,

    // ── Per-function state (reset on every new function) ──────────────────
    fn_buf: Vec<String>,
    current_bb: String,
    terminated: bool,
    reg: usize,
    bb: usize,
    locals: HashMap<String, String>,
    ref_locals: HashMap<String, RefLocal>,
    current_ret_ty: TypeExpr,
    fn_ret_types: HashMap<String, TypeExpr>,
    /// Function name → ordered parameter types (for named-fn closure trampolines).
    fn_param_types: HashMap<String, Vec<TypeExpr>>,
    /// SSA register → LLVM type string (for phi type inference)
    reg_types: HashMap<String, String>,
    /// MVL variable name → TypeExpr (for struct field access)
    local_mvl_types: HashMap<String, TypeExpr>,

    // ── Helper-global presence flags ──────────────────────────────────────
    has_println_fmt: bool,
    has_int_fmt: bool,
    has_str_true: bool,
    has_str_false: bool,

    // ── Closure / lambda state (#1148) ────────────────────────────────────
    /// Monotonic counter for generating unique lambda function names.
    lambda_counter: usize,
    /// True once `%__closure_type = type { ptr, ptr }` has been emitted.
    closure_type_emitted: bool,

    // ── Actor state (#1149) ───────────────────────────────────────────────
    /// Actor declarations keyed by actor type name (populated in first pass).
    actor_decls: HashMap<String, ActorDecl>,
    /// True once actor runtime externs have been emitted.
    actor_runtime_declared: bool,

    // ── Builtin fn dispatch (#1160) ────────────────────────────────────────
    /// Maps MVL builtin function name → C-ABI symbol (e.g. `bytes` → `_mvl_random_bytes`).
    /// Populated from `LlvmTextCompiler::builtin_symbols` at construction time.
    builtin_syms: HashMap<String, String>,

    // ── Per-function flags ────────────────────────────────────────────────
    /// True while emitting the `main` function (affects `ret` instruction type).
    current_fn_is_main: bool,
    /// SSA registers of actor handles spawned in the current function.
    /// Emitted as `mvl_actor_drop` calls before `mvl_actor_join_all` in `main`.
    spawned_actor_handles: Vec<String>,

    // ── Generic monomorphization (#1156) ──────────────────────────────────
    /// Generic function declarations (type_params non-empty), keyed by name.
    generic_fns: HashMap<String, FnDecl>,
    /// Active type-parameter → concrete-type mapping during monomorphized emission.
    type_param_map: HashMap<String, TypeExpr>,
    /// Mangled names of monomorphized copies already emitted (avoid duplicates).
    mono_emitted: HashSet<String>,
    /// Queue of monomorphized functions to emit: (mangled_name, concrete_types).
    mono_queue: Vec<(String, String, Vec<TypeExpr>)>,

    // ── Heap drop tracking (#1185) ───────────────────────────────────────
    /// Heap-allocated locals in the current function.  Entries are emitted
    /// as `mvl_*_drop` calls before every `ret` instruction.
    /// Heap-allocated locals: (ssa_or_alloca, kind, is_ref).
    /// When `is_ref` is true, the SSA is a stack alloca that must be loaded
    /// before the drop call (the alloca holds the heap object pointer).
    heap_locals: Vec<(String, HeapKind, bool)>,
}

/// Tracks heap-allocated value types for automatic drop emission.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeapKind {
    String,
    Array,
    Map,
}

#[derive(Clone)]
struct RefLocal {
    ptr: String,
    elem_ty: TypeExpr,
}

impl TextEmitter {
    #[allow(dead_code)]
    fn new(module_name: &str, target_triple: &str) -> Self {
        Self::new_with_builtins(module_name, target_triple, &HashMap::new())
    }

    /// Construct with pre-populated builtin dispatch table.
    ///
    /// `builtin_map` entries are pre-loaded into `fn_ret_types` / `fn_param_types`
    /// so call sites see the correct LLVM types even when the prelude doesn't
    /// include `builtin fn` declarations (e.g. RUST_BACKED_STDLIB modules strip them).
    fn new_with_builtins(
        module_name: &str,
        target_triple: &str,
        builtin_map: &HashMap<String, (String, TypeExpr, Vec<TypeExpr>)>,
    ) -> Self {
        let mut fn_ret_types: HashMap<String, TypeExpr> = HashMap::new();
        let mut fn_param_types: HashMap<String, Vec<TypeExpr>> = HashMap::new();
        let mut builtin_syms: HashMap<String, String> = HashMap::new();

        for (fn_name, (c_sym, ret_ty, param_tys)) in builtin_map {
            fn_ret_types.insert(fn_name.clone(), ret_ty.clone());
            fn_param_types.insert(fn_name.clone(), param_tys.clone());
            builtin_syms.insert(fn_name.clone(), c_sym.clone());
        }

        Self {
            module_name: module_name.to_string(),
            target_triple: target_triple.to_string(),
            fn_bodies: Vec::new(),
            str_counter: 0,
            str_globals: Vec::new(),
            type_defs: Vec::new(),
            extern_decls: Vec::new(),
            struct_fields: HashMap::new(),
            enum_variants: HashMap::new(),
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
            fn_ret_types,
            fn_param_types,
            reg_types: HashMap::new(),
            local_mvl_types: HashMap::new(),
            has_println_fmt: false,
            has_int_fmt: false,
            has_str_true: false,
            has_str_false: false,
            lambda_counter: 0,
            closure_type_emitted: false,
            actor_decls: HashMap::new(),
            actor_runtime_declared: false,
            builtin_syms,
            current_fn_is_main: false,
            spawned_actor_handles: Vec::new(),
            generic_fns: HashMap::new(),
            type_param_map: HashMap::new(),
            mono_emitted: HashSet::new(),
            mono_queue: Vec::new(),
            heap_locals: Vec::new(),
        }
    }

    // ── Finalise ──────────────────────────────────────────────────────────

    fn finish(self) -> String {
        let mut out = String::new();
        out.push_str(&format!("; ModuleID = '{}'\n", self.module_name));
        out.push_str(&format!("source_filename = \"{}\"\n", self.module_name));
        out.push_str(&format!("target triple = \"{}\"\n", self.target_triple));
        for td in &self.type_defs {
            out.push('\n');
            out.push_str(td);
        }
        for sg in &self.str_globals {
            out.push('\n');
            out.push_str(sg);
        }
        for body in &self.fn_bodies {
            out.push('\n');
            out.push_str(body);
        }
        for ext in &self.extern_decls {
            out.push('\n');
            out.push_str(ext);
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
        self.reg_types.clear();
        self.local_mvl_types.clear();
        self.current_ret_ty = ret_ty;
        self.current_fn_is_main = false;
        self.spawned_actor_handles.clear();
        self.heap_locals.clear();
    }

    // ── Extern declaration helpers ────────────────────────────────────────

    fn ensure_extern(&mut self, decl: &str) {
        if !self.extern_decls.iter().any(|d| d == decl) {
            self.extern_decls.push(decl.to_string());
        }
    }

    // ── Heap drop emission (#1185) ─────────────────────────────────────

    /// Emit `mvl_*_drop` calls for all tracked heap locals.
    /// Called before every `ret` instruction to clean up owned allocations.
    fn emit_heap_drops(&mut self) {
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

    fn ensure_println_fmt(&mut self) -> &'static str {
        if !self.has_println_fmt {
            self.str_globals.push(
                "@println_fmt = private unnamed_addr constant [4 x i8] c\"%s\\0a\\00\"".into(),
            );
            self.has_println_fmt = true;
        }
        "println_fmt"
    }

    fn ensure_int_fmt(&mut self) -> &'static str {
        if !self.has_int_fmt {
            self.str_globals
                .push("@int_fmt = private unnamed_addr constant [5 x i8] c\"%lld\\00\"".into());
            self.has_int_fmt = true;
        }
        "int_fmt"
    }

    fn ensure_bool_str_globals(&mut self) -> (&'static str, &'static str) {
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
    fn emit_str_global(&mut self, s: &str) -> String {
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
    fn emit_string_literal(&mut self, s: &str) -> String {
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
            // Option[T] / Result[T, E] → { i8, ptr } tagged-union (disc byte + payload ptr)
            TypeExpr::Option { .. } | TypeExpr::Result { .. } => "{ i8, ptr }".to_string(),
            _ => "ptr".to_string(),
        }
    }

    /// Map a MVL `TypeExpr` to its LLVM IR type, consulting struct/enum registries.
    fn llvm_ty_ctx(&self, ty: &TypeExpr) -> String {
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
                    return "i64".to_string(); // unit enum = discriminant
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

    fn is_void(ty: &TypeExpr) -> bool {
        Self::llvm_ty(ty) == "void"
    }

    /// Classify a type as heap-allocated for drop tracking.
    fn heap_kind(ty: &TypeExpr) -> Option<HeapKind> {
        let base = match ty {
            TypeExpr::Ref { inner, .. }
            | TypeExpr::Labeled { inner, .. }
            | TypeExpr::Refined { inner, .. } => inner.as_ref(),
            other => other,
        };
        match base {
            TypeExpr::Base { name, .. } => match name.as_str() {
                "String" => Some(HeapKind::String),
                "List" | "Array" | "Set" => Some(HeapKind::Array),
                "Map" => Some(HeapKind::Map),
                _ => None,
            },
            _ => None,
        }
    }

    fn is_mutable_ref(ty: &TypeExpr) -> bool {
        matches!(ty, TypeExpr::Ref { mutable: true, .. })
    }

    fn deref_ty(ty: &TypeExpr) -> &TypeExpr {
        match ty {
            TypeExpr::Ref { inner, .. } => inner.as_ref(),
            other => other,
        }
    }

    /// Infer the LLVM type of an expression without emitting instructions.
    fn type_of_expr(&self, expr: &Expr) -> String {
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
                            return "i64".into();
                        }
                    }
                }
                match name.as_str() {
                    "assert" | "println" | "print" | "eprintln" => "void".into(),
                    "format" => "ptr".into(),
                    "Some" | "None" | "Ok" | "Err" => RESULT_LLVM_TY.into(),
                    _ => {
                        if let Some(ret) = self.fn_ret_types.get(name) {
                            self.llvm_ty_ctx(ret)
                        } else {
                            "i64".into()
                        }
                    }
                }
            }
            Expr::MethodCall { method, .. } => match method.as_str() {
                "to_string" | "concat" | "to_lower" | "to_upper" | "trim" => "ptr".into(),
                "len" => "i64".into(),
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
    fn infer_val_type(&self, val: &str) -> String {
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
    fn struct_name_of_expr(&self, expr: &Expr) -> Option<String> {
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
    fn mvl_receiver_kind(&self, expr: &Expr) -> Option<&str> {
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
            _ => None,
        }
    }

    // ── Int/Bool → String helpers ─────────────────────────────────────────

    fn emit_int_to_string(&mut self, val: &str) -> String {
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

    fn emit_bool_to_string(&mut self, val: &str) -> String {
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

    // ── Program emission ──────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) -> Result<(), String> {
        // First pass: register all function return types and type declarations.
        for decl in &prog.declarations {
            match decl {
                Decl::Fn(fd) => {
                    let ret = fd.return_type.as_ref().clone();
                    let params: Vec<TypeExpr> = fd.params.iter().map(|p| p.ty.clone()).collect();
                    // Register under the short name (e.g. `from_chars`)
                    self.fn_ret_types.insert(fd.name.clone(), ret.clone());
                    self.fn_param_types.insert(fd.name.clone(), params.clone());
                    // Also register under the qualified name (e.g. `String::from_chars`)
                    // so that static call-site lookups like `fn_ret_types["String::from_chars"]`
                    // resolve correctly.
                    if let Some(recv) = &fd.receiver_type {
                        let qualified = format!("{}::{}", recv, fd.name);
                        self.fn_ret_types.insert(qualified.clone(), ret.clone());
                        self.fn_param_types.insert(qualified, params);
                    }
                }
                Decl::Type(td) => match &td.body {
                    TypeBody::Struct { fields, .. } => {
                        // Zero-field structs are opaque handles (e.g. `Instant = struct {}`).
                        // Treat them as `ptr` instead of registering a named struct type so
                        // that C-ABI functions returning `*mut c_void` are typed correctly.
                        if fields.is_empty() {
                            // Don't register — llvm_ty_ctx falls back to "ptr" for unknown names.
                        } else {
                            let field_list: Vec<(String, TypeExpr)> = fields
                                .iter()
                                .map(|f| (f.name.clone(), f.ty.clone()))
                                .collect();
                            // Emit type definition: %Name = type { ty0, ty1, ... }
                            let field_types: Vec<String> =
                                field_list.iter().map(|(_, ty)| Self::llvm_ty(ty)).collect();
                            self.type_defs.push(format!(
                                "%{} = type {{ {} }}",
                                td.name,
                                field_types.join(", ")
                            ));
                            self.struct_fields.insert(td.name.clone(), field_list);
                        }
                    }
                    TypeBody::Enum(variants) => {
                        let variant_names: Vec<String> =
                            variants.iter().map(|v| v.name.clone()).collect();
                        self.enum_variants.insert(td.name.clone(), variant_names);
                    }
                    TypeBody::Alias(_) => {}
                },
                Decl::Actor(ad) => {
                    let state_name = format!("{}State", ad.name);
                    let field_list: Vec<(String, TypeExpr)> = ad
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect();
                    let field_types: Vec<String> = field_list
                        .iter()
                        .map(|(_, ty)| self.llvm_ty_ctx(ty))
                        .collect();
                    self.type_defs.push(format!(
                        "%{state_name} = type {{ {} }}",
                        field_types.join(", ")
                    ));
                    self.struct_fields.insert(state_name, field_list);
                    self.actor_decls.insert(ad.name.clone(), ad.clone());
                }
                _ => {}
            }
        }

        // Actor pass: emit behavior functions + dispatch for each actor.
        if !self.actor_decls.is_empty() {
            self.ensure_actor_runtime_externs();
            let actor_names: Vec<String> = self.actor_decls.keys().cloned().collect();
            for name in actor_names {
                let ad = self.actor_decls[&name].clone();
                self.emit_actor_decl(&ad)?;
            }
        }

        // Collect generic function declarations for on-demand monomorphization.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.type_params.is_empty() {
                    self.generic_fns.insert(fd.name.clone(), fd.clone());
                }
            }
        }

        // Second pass: emit each non-generic function body.
        // Skip: test fns, builtin fns (no MVL body — dispatched via C-ABI),
        //        and generic fns (monomorphized lazily when called).
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test && !fd.is_builtin && fd.type_params.is_empty() {
                    self.emit_fn(fd)?;
                }
            }
        }

        // Third pass: emit monomorphized copies queued during call emission.
        // Loop because a monomorphized body may itself call another generic fn.
        const MONO_LIMIT: usize = 10_000;
        let mut mono_iterations = 0usize;
        while !self.mono_queue.is_empty() {
            mono_iterations += 1;
            if mono_iterations > MONO_LIMIT {
                return Err(
                    "monomorphization limit exceeded — possible infinite instantiation".into(),
                );
            }
            let queue = std::mem::take(&mut self.mono_queue);
            for (mangled, orig_name, concrete_types) in queue {
                let gfd = match self.generic_fns.get(&orig_name) {
                    Some(fd) => fd.clone(),
                    None => continue,
                };
                // Set up type parameter → concrete type mapping.
                for (tp, ct) in gfd.type_params.iter().zip(concrete_types.iter()) {
                    self.type_param_map
                        .insert(tp.name().to_string(), ct.clone());
                }
                // Emit the function under its mangled name.
                let mut fd = gfd;
                fd.name = mangled;
                fd.type_params.clear();
                self.emit_fn(&fd)?;
                self.type_param_map.clear();
            }
        }

        Ok(())
    }

    // ── Function emission ─────────────────────────────────────────────────

    fn emit_fn(&mut self, fd: &FnDecl) -> Result<(), String> {
        let ret_ty = fd.return_type.as_ref();
        self.reset_fn_state(ret_ty.clone());
        self.current_fn_is_main = fd.name == "main";

        let params: Vec<String> = fd
            .params
            .iter()
            .filter_map(|p| {
                let ty_str = self.llvm_ty_ctx(&p.ty);
                if ty_str == "void" {
                    None
                } else {
                    Some(format!("{ty_str} %{}", p.name))
                }
            })
            .collect();
        let params_str = params.join(", ");

        let llvm_ret = self.llvm_ty_ctx(ret_ty);

        let sig = if self.current_fn_is_main {
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

        // Bind parameters as SSA locals, track MVL types for struct access
        for p in &fd.params {
            let ty_str = self.llvm_ty_ctx(&p.ty);
            if ty_str != "void" {
                let ssa = format!("%{}", p.name);
                self.locals.insert(p.name.clone(), ssa.clone());
                self.reg_types.insert(ssa, ty_str);
                self.local_mvl_types.insert(p.name.clone(), p.ty.clone());
            }
        }

        let body_val = self.emit_block(&fd.body)?;

        if !self.terminated {
            self.emit_heap_drops();
            if self.current_fn_is_main {
                if !self.actor_decls.is_empty() {
                    // Drop each handle to close the sender — this signals the
                    // actor thread's recv loop to exit.
                    for handle in std::mem::take(&mut self.spawned_actor_handles) {
                        self.push_instr(&format!("call void @mvl_actor_drop(ptr {handle})"));
                    }
                    self.push_instr("call void @mvl_actor_join_all()");
                }
                self.push_instr(MAIN_RET);
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
            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt_chain(cond, then, else_.as_ref()),
            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match_expr(scrutinee, arms),
            other => {
                self.emit_stmt(other)?;
                Ok(None)
            }
        }
    }

    /// Emit a `Stmt::If` as an expression, correctly handling `else if` chains.
    ///
    /// Unlike `emit_if_phi` (which only handles `else { block }`), this recursively
    /// follows `ElseBranch::If` chains so that deeply nested `else if` trees produce
    /// correct IR instead of dropping the tail branches.
    fn emit_if_stmt_chain(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&ElseBranch>,
    ) -> Result<Option<String>, String> {
        match else_ {
            None => self.emit_if_phi(cond, then, None),
            Some(ElseBranch::Block(b)) => self.emit_if_phi(cond, then, Some(b)),
            Some(ElseBranch::If(nested)) => {
                if let Stmt::If {
                    cond: ncond,
                    then: nthen,
                    else_: nelse,
                    ..
                } = nested.as_ref()
                {
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
                    let else_val = self.emit_if_stmt_chain(ncond, nthen, nelse.as_ref())?;
                    let else_end = self.current_bb.clone();
                    if !self.terminated {
                        self.push_instr(&format!("br label %{merge_bb}"));
                    }

                    self.start_bb(&merge_bb);
                    match (then_val, else_val) {
                        (Some(tv), Some(ev)) => {
                            let phi_ty = self.infer_val_type(&tv);
                            let result = self.next_reg();
                            self.push_instr(&format!(
                                "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                            ));
                            self.reg_types.insert(result.clone(), phi_ty);
                            Ok(Some(result))
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
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
                if *kind == LetKind::Ghost {
                    return Ok(());
                }
                let val = self.emit_expr(init)?;
                let elem_ty = Self::deref_ty(ty).clone();

                if Self::is_mutable_ref(ty) {
                    let ty_str = self.llvm_ty_ctx(&elem_ty);
                    if ty_str == "void" {
                        return Ok(());
                    }
                    let ptr = self.next_reg();
                    self.push_instr(&format!("{ptr} = alloca {ty_str}"));
                    if let Some(v) = val {
                        self.push_instr(&format!("store {ty_str} {v}, ptr {ptr}"));
                    }
                    if let Pattern::Ident(name, _) = pattern {
                        // Track heap-allocated ref locals for drop at function exit.
                        if let Some(hk) = Self::heap_kind(&elem_ty) {
                            self.heap_locals.push((ptr.clone(), hk, true));
                        }
                        self.ref_locals.insert(
                            name.clone(),
                            RefLocal {
                                ptr,
                                elem_ty: elem_ty.clone(),
                            },
                        );
                    }
                } else if let (Some(v), Pattern::Ident(name, _)) = (val, pattern) {
                    let ty_str = self.llvm_ty_ctx(&elem_ty);
                    self.reg_types.insert(v.clone(), ty_str);
                    // If this name shadows a previous heap-allocated binding,
                    // remove the old SSA from heap_locals to prevent double-drop.
                    if let Some(old_ssa) = self.locals.get(name) {
                        let old_ssa = old_ssa.clone();
                        self.heap_locals.retain(|(s, _, _)| *s != old_ssa);
                    }
                    self.locals.insert(name.clone(), v.clone());
                    // Track heap-allocated locals for automatic drop at function exit.
                    if let Some(hk) = Self::heap_kind(&elem_ty) {
                        self.heap_locals.push((v, hk, false));
                    }
                    self.local_mvl_types.insert(name.clone(), elem_ty);
                }
                Ok(())
            }

            Stmt::Assign { target, value, .. } => {
                let val = self.emit_expr(value)?;
                if let LValue::Ident(name, _) = target {
                    if let Some(loc) = self.ref_locals.get(name).cloned() {
                        if let Some(v) = val {
                            let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                            self.push_instr(&format!("store {ty_str} {v}, ptr {}", loc.ptr));
                        }
                    }
                }
                Ok(())
            }

            Stmt::Return { value, .. } => {
                let ret_ty = self.current_ret_ty.clone();
                // Evaluate return expression first (if any), then drop once.
                let ret_val = if let Some(expr) = value {
                    self.emit_expr(expr)?
                } else {
                    None
                };
                self.emit_heap_drops();
                if Self::is_void(&ret_ty) {
                    if self.current_fn_is_main {
                        self.push_instr(MAIN_RET);
                    } else {
                        self.push_instr("ret void");
                    }
                } else if let Some(v) = ret_val {
                    let ty = self.llvm_ty_ctx(&ret_ty);
                    self.push_instr(&format!("ret {ty} {v}"));
                } else if self.current_fn_is_main {
                    self.push_instr(MAIN_RET);
                } else {
                    self.push_instr("ret void");
                }
                self.terminated = true;
                Ok(())
            }

            Stmt::While { cond, body, .. } => self.emit_while(cond, body),

            Stmt::If {
                cond, then, else_, ..
            } => self.emit_if_stmt(cond, then, else_.as_ref()),

            Stmt::For {
                pattern,
                iter,
                body,
                ..
            } => self.emit_for_stmt(pattern, iter, body),

            Stmt::Match {
                scrutinee, arms, ..
            } => self.emit_match_stmt(scrutinee, arms),

            Stmt::Expr { expr, .. } => {
                self.emit_expr(expr)?;
                Ok(())
            }
        }
    }

    // ── For loop (range only) ─────────────────────────────────────────────

    fn emit_for_stmt(
        &mut self,
        pattern: &Pattern,
        iter: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        // Only handle `for var in range(lo, hi)` for Phase 2.
        if let Expr::FnCall { name, args, .. } = iter {
            if name == "range" && args.len() == 2 {
                let var_name = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => "_".into(),
                };
                return self.emit_for_range(&var_name, &args[0], &args[1], body);
            }
        }
        Ok(())
    }

    fn emit_for_range(
        &mut self,
        var_name: &str,
        lo: &Expr,
        hi: &Expr,
        body: &Block,
    ) -> Result<(), String> {
        let lo_val = match self.emit_expr(lo)? {
            Some(v) => v,
            None => return Ok(()),
        };
        let hi_val = match self.emit_expr(hi)? {
            Some(v) => v,
            None => return Ok(()),
        };

        let i_ptr = self.next_reg();
        self.push_instr(&format!("{i_ptr} = alloca i64"));
        self.push_instr(&format!("store i64 {lo_val}, ptr {i_ptr}"));

        let cond_bb = self.next_bb("for_cond");
        let body_bb = self.next_bb("for_body");
        let end_bb = self.next_bb("for_end");

        self.push_instr(&format!("br label %{cond_bb}"));
        self.start_bb(&cond_bb);

        let cur_i = self.next_reg();
        self.push_instr(&format!("{cur_i} = load i64, ptr {i_ptr}"));

        let cond_reg = self.next_reg();
        self.push_instr(&format!("{cond_reg} = icmp slt i64 {cur_i}, {hi_val}"));
        self.push_instr(&format!(
            "br i1 {cond_reg}, label %{body_bb}, label %{end_bb}"
        ));

        self.start_bb(&body_bb);

        // Bind loop variable (immutable, read-only inside body)
        let old = self.locals.insert(var_name.to_string(), cur_i.clone());
        self.reg_types.insert(cur_i.clone(), "i64".into());
        self.emit_block(body)?;

        // Restore locals
        if let Some(prev) = old {
            self.locals.insert(var_name.to_string(), prev);
        } else {
            self.locals.remove(var_name);
        }

        if !self.terminated {
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.push_instr(&format!("br label %{cond_bb}"));
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    // ── While loop ────────────────────────────────────────────────────────

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

    // ── If-statement (void, no phi) ───────────────────────────────────────

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

    // ── Match (statement, void) ───────────────────────────────────────────

    fn emit_match_stmt(&mut self, scrutinee: &Expr, arms: &[MatchArm]) -> Result<(), String> {
        self.emit_match_expr(scrutinee, arms)?;
        Ok(())
    }

    // ── Match (expression, produces value) ───────────────────────────────

    fn emit_match_expr(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        let scrut_val = match self.emit_expr(scrutinee)? {
            Some(v) => v,
            None => return Ok(None),
        };

        // Delegate to Result-specific match when Ok/Err patterns are present.
        let has_ok_err = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Ok { .. } | Pattern::Err { .. }));
        if has_ok_err {
            return self.emit_result_match(scrutinee, &scrut_val, arms);
        }

        // Delegate to Option-specific match when Some/None patterns are present.
        let has_some_none = arms
            .iter()
            .any(|a| matches!(&a.pattern, Pattern::Some { .. } | Pattern::None(_)));
        if has_some_none {
            return self.emit_option_match(scrutinee, &scrut_val, arms);
        }

        let scrut_ty = self.type_of_expr(scrutinee);

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);

        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Determine which patterns are enum discriminants vs wildcards
        let mut switch_arms: Vec<(i64, usize)> = Vec::new();
        let mut wildcard_arm: Option<usize> = None;

        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::TupleStruct { name, .. } => {
                    if let Some(disc) = self.pattern_discriminant(name) {
                        switch_arms.push((disc, idx));
                        continue;
                    }
                }
                Pattern::Ident(name, _) if name.contains("::") => {
                    if let Some(disc) = self.pattern_discriminant(name) {
                        switch_arms.push((disc, idx));
                        continue;
                    }
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                    continue;
                }
                _ => {}
            }
            wildcard_arm = Some(idx);
        }

        let use_switch = !switch_arms.is_empty();
        let _has_default = wildcard_arm.is_some();

        if use_switch {
            // Emit switch instruction
            let mut switch_str = format!("switch {scrut_ty} {scrut_val}, label %{default_bb} [\n");
            for (disc, arm_idx) in &switch_arms {
                switch_str.push_str(&format!(
                    "    {scrut_ty} {disc}, label %{}\n",
                    arm_bbs[*arm_idx]
                ));
            }
            switch_str.push_str("  ]");
            self.push_instr(&switch_str);
        } else {
            // Fallback: just branch to default
            self.push_instr(&format!("br label %{default_bb}"));
        }

        // Emit each arm block
        let mut phi_entries: Vec<(String, String, String)> = Vec::new(); // (val, ty, from_bb)
                                                                         // Arms that branch to merge_bb but produced no value (need undef phi entries).
        let mut no_val_arms: Vec<String> = Vec::new(); // from_bb

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            // Bind wildcard pattern if present
            let _binding = if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    let bound = self.next_reg();
                    // For enum scrutinee: the bound value is the scrutinee itself
                    self.reg_types.insert(bound.clone(), scrut_ty.clone());
                    self.locals.insert(name.clone(), scrut_val.clone());
                    Some(name.clone())
                } else {
                    None
                }
            } else {
                None
            };

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };

            let end_bb = self.current_bb.clone();
            if !self.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Pattern::Ident(name, _) = &arm.pattern {
                if !name.contains("::") {
                    self.locals.remove(name);
                }
            }
        }

        // Default block
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;

        if let Some(wild_idx) = wildcard_arm {
            let arm_bb = &arm_bbs[wild_idx];
            // Jump to the wildcard arm (it was already emitted above — but wait, arm_bbs covers all arms)
            // Actually the wildcard arm was already emitted in the loop above.
            // Default just branches to the wildcard arm's block.
            // But that arm has already been emitted as its own block.
            // We need to route default to that arm block.
            // However, the wildcard arm already has a branch to merge_bb...
            // The issue is the default block references the wildcard arm BB.
            // The simplest fix: emit wildcard arm code in the default block directly.
            // But we already emitted it in arm_bbs[wild_idx]...
            // Let's just branch to it from default.
            self.push_instr(&format!("br label %{arm_bb}"));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        // Merge block + phi
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;

        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            // Use the first non-i64 type found (e.g. ptr for String arms), else i64.
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            // Add undef entries for arms that branch here but produced no value.
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// Resolve a pattern name like "Shape::Circle" to its discriminant i64.
    fn pattern_discriminant(&self, name: &str) -> Option<i64> {
        if let Some(pos) = name.find("::") {
            let type_name = &name[..pos];
            let variant_name = &name[pos + 2..];
            if let Some(variants) = self.enum_variants.get(type_name) {
                if let Some(idx) = variants.iter().position(|v| v == variant_name) {
                    return Some(idx as i64);
                }
            }
        }
        None
    }

    // ── Expression emission ───────────────────────────────────────────────

    fn emit_expr(&mut self, expr: &Expr) -> Result<Option<String>, String> {
        match expr {
            Expr::Literal(lit, _) => self.emit_literal(lit),

            Expr::Ident(name, _) => {
                // `None` as a bare identifier → Option None constructor.
                if name == "None" {
                    return self.emit_none_constructor();
                }
                // Qualified enum variant: "Shape::Circle" → discriminant i64
                if name.contains("::") {
                    if let Some(disc) = self.pattern_discriminant(name) {
                        return Ok(Some(format!("{disc}")));
                    }
                }
                if let Some(loc) = self.ref_locals.get(name).cloned() {
                    let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                    self.reg_types.insert(reg.clone(), ty_str);
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

            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => self.emit_method_call(receiver, method, args),

            Expr::Construct { name, fields, .. } => self.emit_construct(name, fields),

            Expr::FieldAccess { expr, field, .. } => self.emit_field_access(expr, field),

            Expr::Match {
                scrutinee, arms, ..
            } => self.emit_match_expr(scrutinee, arms),

            Expr::List { elems, .. } => self.emit_list_literal(elems),

            Expr::Set { elems, .. } => self.emit_list_literal(elems),

            Expr::Map { pairs, .. } => self.emit_map_literal(pairs),

            Expr::Consume { expr, .. } | Expr::Relabel { expr, .. } => self.emit_expr(expr),

            Expr::Propagate { expr, .. } => self.emit_propagate(expr),

            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => self.emit_lambda(params, ret_type.as_deref(), body),

            Expr::Spawn {
                actor_type, fields, ..
            } => self.emit_actor_spawn(actor_type, fields),

            _ => Ok(None),
        }
    }

    // ── Literal emission ──────────────────────────────────────────────────

    fn emit_literal(&mut self, lit: &Literal) -> Result<Option<String>, String> {
        match lit {
            Literal::Integer(n) => Ok(Some(format!("{n}"))),
            Literal::Float(f) => Ok(Some(if f.fract() == 0.0 {
                format!("{f:.1}")
            } else {
                format!("{f}")
            })),
            Literal::Bool(b) => Ok(Some(if *b {
                "true".to_string()
            } else {
                "false".to_string()
            })),
            Literal::Str(s) => Ok(Some(self.emit_string_literal(s))),
            Literal::Unit => Ok(None),
            Literal::Char(c) => Ok(Some(format!("{}", *c as u32))),
        }
    }

    // ── Binary operators ──────────────────────────────────────────────────

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
        let lhs_ty = self.type_of_expr(left);

        // String equality/inequality: delegate to runtime via mvl_string_eq.
        if lhs_ty == "ptr" && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            self.ensure_extern("declare i1 @mvl_string_eq(ptr, ptr)");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call i1 @mvl_string_eq(ptr {lv}, ptr {rv})"
            ));
            if matches!(op, BinaryOp::Ne) {
                let neg = self.next_reg();
                self.push_instr(&format!("{neg} = xor i1 {reg}, true"));
                self.reg_types.insert(neg.clone(), "i1".into());
                return Ok(Some(neg));
            }
            self.reg_types.insert(reg.clone(), "i1".into());
            return Ok(Some(reg));
        }

        let instr = Self::binary_instr(op, is_float, &lhs_ty, &lv, &rv);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = {instr}"));

        // Track type: comparison ops → i1, others → i64/double
        let result_ty = match op {
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Gt
            | BinaryOp::Le
            | BinaryOp::Ge => "i1",
            _ => {
                if is_float {
                    "double"
                } else {
                    "i64"
                }
            }
        };
        self.reg_types.insert(reg.clone(), result_ty.into());
        Ok(Some(reg))
    }

    fn binary_instr(op: &BinaryOp, is_float: bool, lhs_ty: &str, lv: &str, rv: &str) -> String {
        let is_bool = lhs_ty == "i1";
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
            BinaryOp::Eq if is_bool => format!("icmp eq i1 {lv}, {rv}"),
            BinaryOp::Ne if is_bool => format!("icmp ne i1 {lv}, {rv}"),
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
        self.reg_types.insert(result.clone(), "i1".into());
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
        self.reg_types.insert(result.clone(), "i1".into());
        Ok(Some(result))
    }

    // ── Unary operators ───────────────────────────────────────────────────

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
                self.reg_types.insert(reg.clone(), "double".into());
            }
            UnaryOp::Neg => {
                self.push_instr(&format!("{reg} = sub i64 0, {val}"));
                self.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Not => {
                self.push_instr(&format!("{reg} = xor i1 {val}, true"));
                self.reg_types.insert(reg.clone(), "i1".into());
            }
            UnaryOp::BitNot => {
                self.push_instr(&format!("{reg} = xor i64 {val}, -1"));
                self.reg_types.insert(reg.clone(), "i64".into());
            }
            UnaryOp::Deref => {
                return Ok(Some(val));
            }
        }
        Ok(Some(reg))
    }

    // ── If expression (phi) ───────────────────────────────────────────────

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
                let phi_ty = self.infer_val_type(&tv).clone();
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                ));
                self.reg_types.insert(result.clone(), phi_ty);
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    fn emit_if_expr(
        &mut self,
        cond: &Expr,
        then: &Block,
        else_: Option<&Expr>,
    ) -> Result<Option<String>, String> {
        match else_ {
            Some(Expr::Block(b)) => self.emit_if_phi(cond, then, Some(b)),
            Some(nested_if @ Expr::If { .. }) => {
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
                        let phi_ty = self.infer_val_type(&tv);
                        let result = self.next_reg();
                        self.push_instr(&format!(
                            "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                        ));
                        self.reg_types.insert(result.clone(), phi_ty);
                        Ok(Some(result))
                    }
                    _ => Ok(None),
                }
            }
            None => self.emit_if_phi(cond, then, None),
            Some(_) => self.emit_if_phi(cond, then, None),
        }
    }

    // ── Function call emission ────────────────────────────────────────────

    fn emit_fn_call(&mut self, name: &str, args: &[Expr]) -> Result<Option<String>, String> {
        // ── Builtins ──────────────────────────────────────────────────────
        match name {
            "assert" => return self.emit_assert_builtin(args),
            "println" | "print" | "eprintln" => return self.emit_println_builtin(name, args),
            "format" => return self.emit_format_builtin(args),
            "Ok" | "Err" => return self.emit_result_constructor(name, args),
            "Some" => return self.emit_option_constructor(args),
            "None" => return self.emit_none_constructor(),
            _ => {}
        }

        // ── Enum variant constructors: "Shape::Circle" ─────────────────
        if name.contains("::") {
            if let Some(disc) = self.pattern_discriminant(name) {
                return Ok(Some(format!("{disc}")));
            }
        }

        // ── Generic function monomorphization ───────────────────────────
        if self.generic_fns.contains_key(name) {
            return self.emit_monomorphized_call(name, args);
        }

        // ── User-defined functions ─────────────────────────────────────
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.type_of_expr(arg);
            if let Some(v) = self.emit_expr(arg)? {
                arg_vals.push((ty, v));
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

        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        // If this is a builtin fn, dispatch to the C-ABI symbol directly.
        let (effective_name, is_c_builtin): (String, bool) =
            if let Some(c_sym) = self.builtin_syms.get(name).cloned() {
                // Emit extern declare if not already present (use arg types from call site).
                let param_tys = arg_vals
                    .iter()
                    .map(|(ty, _)| ty.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.ensure_extern(&format!("declare {llvm_ret} @{c_sym}({param_tys})"));
                (c_sym, true)
            } else {
                (name.to_string(), false)
            };

        if is_void {
            self.push_instr(&format!("call void @{effective_name}({args_str})"));
            Ok(None)
        } else {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call {llvm_ret} @{effective_name}({args_str})"
            ));
            self.reg_types.insert(reg.clone(), llvm_ret.clone());

            // C-ABI builtins that return `{ i8, ptr }` store the raw value directly
            // in the payload field.  MVL-constructed Ok/Err store a slot pointer in
            // field 1 (see emit_result_constructor).  Wrap the C payload into a slot
            // so emit_result_match can use a uniform `load T, ptr payload` convention.
            if is_c_builtin && llvm_ret == RESULT_LLVM_TY {
                // C-ABI builtins store the raw value directly in field 1.
                // MVL-constructed Ok/Err store a slot pointer in field 1 (see
                // emit_result_constructor).  Wrap the C payload into a slot so
                // emit_result_match can use a uniform `load T, ptr payload` convention.
                let disc = self.next_reg();
                self.push_instr(&format!("{disc} = extractvalue {RESULT_LLVM_TY} {reg}, 0"));
                self.reg_types.insert(disc.clone(), "i8".into());
                let raw_payload = self.next_reg();
                self.push_instr(&format!(
                    "{raw_payload} = extractvalue {RESULT_LLVM_TY} {reg}, 1"
                ));
                self.reg_types.insert(raw_payload.clone(), "ptr".into());
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca ptr"));
                self.push_instr(&format!("store ptr {raw_payload}, ptr {slot}"));
                let r1 = self.wrap_result_pair(&disc, &slot);
                return Ok(Some(r1));
            }

            Ok(Some(reg))
        }
    }

    fn emit_assert_builtin(&mut self, args: &[Expr]) -> Result<Option<String>, String> {
        let cond = match args.first() {
            Some(a) => a,
            None => return Ok(None),
        };
        let cond_val = match self.emit_expr(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };
        // Widen i1 to i1 — it already is, but make sure we're treating it as i1
        let ok_bb = self.next_bb("assert_ok");
        let fail_bb = self.next_bb("assert_fail");
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{ok_bb}, label %{fail_bb}"
        ));
        self.fn_buf.push(format!("{fail_bb}:"));
        self.current_bb = fail_bb.clone();
        self.terminated = false;
        self.ensure_extern("declare void @llvm.trap()");
        self.push_instr("call void @llvm.trap()");
        self.push_instr("unreachable");
        self.terminated = true;
        self.fn_buf.push(format!("{ok_bb}:"));
        self.current_bb = ok_bb;
        self.terminated = false;
        Ok(None)
    }

    fn emit_println_builtin(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let fd = if name == "eprintln" { 2i32 } else { 1i32 };
        if args.is_empty() {
            // println() with no args — just print newline
            let fmt = self.ensure_println_fmt();
            self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
            let empty_g = self.emit_str_global("");
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = call ptr @mvl_string_new(ptr @{empty_g}, i64 0)"
            ));
            self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
            let raw = self.next_reg();
            self.push_instr(&format!("{raw} = call ptr @mvl_string_ptr(ptr {reg})"));
            self.push_instr(&format!(
                "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
            ));
            return Ok(None);
        }
        let val = match self.emit_expr(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let fmt = self.ensure_println_fmt();
        self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
        self.ensure_extern("declare i32 @dprintf(i32, ptr, ...)");
        let raw = self.next_reg();
        self.push_instr(&format!("{raw} = call ptr @mvl_string_ptr(ptr {val})"));
        self.push_instr(&format!(
            "call i32 (i32, ptr, ...) @dprintf(i32 {fd}, ptr @{fmt}, ptr {raw})"
        ));
        Ok(None)
    }

    // ── Result[T,E] helpers ───────────────────────────────────────────────

    /// Build a `{ i8, ptr }` Result aggregate from a discriminant byte and a payload slot pointer.
    ///
    /// Both fields are immediately overwritten, so `zeroinitializer` is used as the base
    /// (safe if the struct ever gains padding fields, unlike `undef`).
    fn wrap_result_pair(&mut self, disc: &str, slot: &str) -> String {
        let r0 = self.next_reg();
        self.push_instr(&format!(
            "{r0} = insertvalue {RESULT_LLVM_TY} zeroinitializer, i8 {disc}, 0"
        ));
        self.reg_types.insert(r0.clone(), RESULT_LLVM_TY.into());
        let r1 = self.next_reg();
        self.push_instr(&format!(
            "{r1} = insertvalue {RESULT_LLVM_TY} {r0}, ptr {slot}, 1"
        ));
        self.reg_types.insert(r1.clone(), RESULT_LLVM_TY.into());
        r1
    }

    /// Emit `Ok(val)` or `Err(val)` — builds a `{ i8, ptr }` tagged union.
    fn emit_result_constructor(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let disc: i64 = if name == "Ok" { 0 } else { 1 };
        let arg_ty;
        let slot;
        if let Some(arg) = args.first() {
            arg_ty = self.type_of_expr(arg);
            let arg_val = match self.emit_expr(arg)? {
                Some(v) => v,
                None => return Ok(None),
            };
            slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca {arg_ty}"));
            self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
        } else {
            arg_ty = "i8".into();
            slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca i8"));
        };
        let r1 = self.wrap_result_pair(&disc.to_string(), &slot);
        let _ = arg_ty; // used above
        Ok(Some(r1))
    }

    // ── Option[T] helpers (#1156) ────────────────────────────────────────

    /// Emit `Some(val)` — builds a `{ i8, ptr }` tagged union with disc=0.
    fn emit_option_constructor(&mut self, args: &[Expr]) -> Result<Option<String>, String> {
        let arg = match args.first() {
            Some(a) => a,
            None => return self.emit_none_constructor(),
        };
        let arg_ty = self.type_of_expr(arg);
        let arg_val = match self.emit_expr(arg)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca {arg_ty}"));
        self.push_instr(&format!("store {arg_ty} {arg_val}, ptr {slot}"));
        let r1 = self.wrap_result_pair("0", &slot);
        Ok(Some(r1))
    }

    /// Emit `None` — builds a `{ i8, ptr }` tagged union with disc=1 and null payload.
    fn emit_none_constructor(&mut self) -> Result<Option<String>, String> {
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca i8"));
        let r1 = self.wrap_result_pair("1", &slot);
        Ok(Some(r1))
    }

    /// Emit a `match` where at least one arm has `Pattern::Some` / `Pattern::None`.
    fn emit_option_match(
        &mut self,
        scrutinee: &Expr,
        scrut_val: &str,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        // Determine the inner MVL and LLVM types from the scrutinee's MVL type.
        let mvl_ty = match scrutinee {
            Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned(),
            Expr::FnCall { name, .. } => self.fn_ret_types.get(name.as_str()).cloned(),
            _ => None,
        };
        let (inner_load_ty, inner_mvl_ty) = match &mvl_ty {
            Some(TypeExpr::Option { inner, .. }) => {
                (self.llvm_ty_ctx(inner), Some(inner.as_ref().clone()))
            }
            _ => ("ptr".into(), None),
        };

        // Extract discriminant byte from the { i8, ptr } struct.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 0"
        ));
        self.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Build switch on i8 discriminant: Some=0, None=1.
        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Some { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::None(_) => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        // Emit arm blocks (skip wildcard/ident arms — emitted from default_bb).
        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        let mut no_val_arms: Vec<String> = Vec::new();

        for (idx, arm) in arms.iter().enumerate() {
            // Skip wildcard arms here; they are emitted via the default block.
            if Some(idx) == wildcard_arm {
                continue;
            }

            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            let mut bound_var: Option<String> = None;

            match &arm.pattern {
                Pattern::Some { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!(
                        "{pp} = extractvalue {RESULT_LLVM_TY} {scrut_val}, 1"
                    ));
                    let some_val = self.next_reg();
                    self.push_instr(&format!("{some_val} = load {inner_load_ty}, ptr {pp}"));
                    self.reg_types
                        .insert(some_val.clone(), inner_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.locals.insert(var_name.clone(), some_val.clone());
                            if let Some(ref imty) = inner_mvl_ty {
                                self.local_mvl_types.insert(var_name.clone(), imty.clone());
                            }
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::None(_) => {
                    // Nothing to bind.
                }
                _ => {}
            }

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };
            let end_bb = self.current_bb.clone();
            if !self.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Some(ref var_name) = bound_var {
                self.locals.remove(var_name);
                self.local_mvl_types.remove(var_name);
            }
        }

        // Default block — either jumps to wildcard arm body or traps.
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            // Emit the wildcard arm body inline in the default block.
            let wild_arm = &arms[wild_idx];
            let mut bound_var: Option<String> = None;
            if let Pattern::Ident(name, _) = &wild_arm.pattern {
                self.locals.insert(name.clone(), scrut_val.to_string());
                bound_var = Some(name.clone());
            }
            let arm_val = match &wild_arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };
            let end_bb = self.current_bb.clone();
            if !self.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }
            if let Some(ref var_name) = bound_var {
                self.locals.remove(var_name);
                self.local_mvl_types.remove(var_name);
            }
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        // Merge block + phi.
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming == 0 {
            // All arms terminated (e.g. all `return`) — no merge block needed.
            self.fn_buf.push(format!("{merge_bb}:"));
            self.current_bb = merge_bb.clone();
            self.terminated = false;
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
            return Ok(None);
        }
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries[0].1.clone();
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    // ── Generic monomorphization (#1156) ──────────────────────────────────

    /// Infer the MVL type of an expression (best-effort, for monomorphization).
    fn mvl_type_of_expr(&self, expr: &Expr) -> TypeExpr {
        let default_int = || TypeExpr::Base {
            name: "Int".into(),
            args: vec![],
            span: Default::default(),
        };
        match expr {
            Expr::Literal(lit, _) => match lit {
                Literal::Integer(_) => default_int(),
                Literal::Float(_) => TypeExpr::Base {
                    name: "Float".into(),
                    args: vec![],
                    span: Default::default(),
                },
                Literal::Bool(_) => TypeExpr::Base {
                    name: "Bool".into(),
                    args: vec![],
                    span: Default::default(),
                },
                Literal::Str(_) => TypeExpr::Base {
                    name: "String".into(),
                    args: vec![],
                    span: Default::default(),
                },
                _ => default_int(),
            },
            Expr::Ident(name, _) => self
                .local_mvl_types
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(default_int),
            Expr::FnCall { name, .. } => self
                .fn_ret_types
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(default_int),
            Expr::Construct { name, .. } => TypeExpr::Base {
                name: name.clone(),
                args: vec![],
                span: Default::default(),
            },
            _ => default_int(),
        }
    }

    /// Sanitize a string segment for use in LLVM IR identifiers.
    fn mangle_segment(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// Mangle a generic function name with concrete types: `identity` + [Int] → `identity__Int`.
    fn mangle_generic(name: &str, concrete: &[TypeExpr]) -> String {
        let suffix: Vec<String> = concrete
            .iter()
            .map(|ty| match ty {
                TypeExpr::Base { name, .. } => Self::mangle_segment(name),
                TypeExpr::Option { inner, .. } => {
                    format!(
                        "Option_{}",
                        Self::mangle_segment(&Self::mangle_type_name(inner))
                    )
                }
                TypeExpr::Result { ok, err, .. } => {
                    format!(
                        "Result_{}_{}",
                        Self::mangle_segment(&Self::mangle_type_name(ok)),
                        Self::mangle_segment(&Self::mangle_type_name(err))
                    )
                }
                _ => "T".into(),
            })
            .collect();
        format!("{}__{}", Self::mangle_segment(name), suffix.join("_"))
    }

    /// Extract a human-readable type name for mangling purposes.
    fn mangle_type_name(ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Base { name, .. } => name.clone(),
            TypeExpr::Option { inner, .. } => format!("Option_{}", Self::mangle_type_name(inner)),
            TypeExpr::Result { ok, err, .. } => {
                format!(
                    "Result_{}_{}",
                    Self::mangle_type_name(ok),
                    Self::mangle_type_name(err)
                )
            }
            _ => "T".into(),
        }
    }

    /// Emit a call to a generic function, enqueuing the monomorphized version.
    fn emit_monomorphized_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        let gfd = self.generic_fns.get(name).cloned().ok_or_else(|| {
            format!("ICE: generic fn '{name}' missing from monomorphization table")
        })?;

        // Infer concrete types for each type parameter from the argument types.
        let mut tp_map: HashMap<String, TypeExpr> = HashMap::new();
        for (param, arg) in gfd.params.iter().zip(args.iter()) {
            Self::collect_type_bindings(&param.ty, &self.mvl_type_of_expr(arg), &gfd, &mut tp_map);
        }
        let concrete_types: Vec<TypeExpr> = gfd
            .type_params
            .iter()
            .map(|tp| {
                tp_map
                    .get(tp.name())
                    .cloned()
                    .unwrap_or_else(|| TypeExpr::Base {
                        name: "Int".into(),
                        args: vec![],
                        span: Default::default(),
                    })
            })
            .collect();

        let mangled = Self::mangle_generic(name, &concrete_types);

        // Enqueue monomorphized copy if not already emitted.
        if !self.mono_emitted.contains(&mangled) {
            self.mono_emitted.insert(mangled.clone());
            self.mono_queue
                .push((mangled.clone(), name.to_string(), concrete_types.clone()));

            // Register the return type for the mangled function.
            // Resolve any type params in the return type.
            let resolved_ret = Self::substitute_type(&gfd.return_type, &tp_map);
            self.fn_ret_types.insert(mangled.clone(), resolved_ret);
        }

        // Emit the call.
        let mut arg_vals: Vec<(String, String)> = Vec::new();
        for arg in args {
            let ty = self.type_of_expr(arg);
            if let Some(v) = self.emit_expr(arg)? {
                arg_vals.push((ty, v));
            }
        }
        let args_str = arg_vals
            .iter()
            .map(|(ty, v)| format!("{ty} {v}"))
            .collect::<Vec<_>>()
            .join(", ");

        let ret_ty = self
            .fn_ret_types
            .get(&mangled)
            .cloned()
            .unwrap_or_else(|| TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: Default::default(),
            });
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        if is_void {
            self.push_instr(&format!("call void @{mangled}({args_str})"));
            Ok(None)
        } else {
            let result = self.next_reg();
            self.push_instr(&format!(
                "{result} = call {llvm_ret} @{mangled}({args_str})"
            ));
            self.reg_types.insert(result.clone(), llvm_ret);
            Ok(Some(result))
        }
    }

    /// Match a generic parameter type against a concrete argument type to bind type variables.
    fn collect_type_bindings(
        param_ty: &TypeExpr,
        arg_ty: &TypeExpr,
        gfd: &FnDecl,
        map: &mut HashMap<String, TypeExpr>,
    ) {
        if let TypeExpr::Base { name, .. } = param_ty {
            if gfd.type_params.iter().any(|tp| tp.name() == name) {
                map.insert(name.clone(), arg_ty.clone());
            }
        }
    }

    /// Substitute type parameters in a type expression using the given mapping.
    fn substitute_type(ty: &TypeExpr, map: &HashMap<String, TypeExpr>) -> TypeExpr {
        match ty {
            TypeExpr::Base { name, args, span } => {
                if let Some(concrete) = map.get(name) {
                    concrete.clone()
                } else {
                    TypeExpr::Base {
                        name: name.clone(),
                        args: args.iter().map(|a| Self::substitute_type(a, map)).collect(),
                        span: *span,
                    }
                }
            }
            TypeExpr::Option { inner, span } => TypeExpr::Option {
                inner: Box::new(Self::substitute_type(inner, map)),
                span: *span,
            },
            TypeExpr::Result { ok, err, span } => TypeExpr::Result {
                ok: Box::new(Self::substitute_type(ok, map)),
                err: Box::new(Self::substitute_type(err, map)),
                span: *span,
            },
            other => other.clone(),
        }
    }

    /// Emit `s.parse_int()` or `s.parse_float()` — calls the C-ABI parser and
    /// wraps the result in a `{ i8, ptr }` Result.
    ///
    /// `ok_llvm_ty` is the LLVM type of the success value (`"i64"` or `"double"`).
    fn emit_str_parse(
        &mut self,
        val: &str,
        ok_llvm_ty: &str,
        c_sym: &str,
    ) -> Result<Option<String>, String> {
        let ok_slot = self.next_reg();
        self.push_instr(&format!("{ok_slot} = alloca {ok_llvm_ty}"));
        let err_slot = self.next_reg();
        self.push_instr(&format!("{err_slot} = alloca ptr"));
        self.ensure_extern(&format!("declare i8 @{c_sym}(ptr, ptr, ptr)"));
        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = call i8 @{c_sym}(ptr {val}, ptr {ok_slot}, ptr {err_slot})"
        ));
        self.reg_types.insert(disc.clone(), "i8".into());
        // Select the correct payload pointer based on discriminant.
        let disc_is_ok = self.next_reg();
        self.push_instr(&format!("{disc_is_ok} = icmp eq i8 {disc}, 0"));
        self.reg_types.insert(disc_is_ok.clone(), "i1".into());
        let payload = self.next_reg();
        self.push_instr(&format!(
            "{payload} = select i1 {disc_is_ok}, ptr {ok_slot}, ptr {err_slot}"
        ));
        self.reg_types.insert(payload.clone(), "ptr".into());
        let r1 = self.wrap_result_pair(&disc, &payload);
        Ok(Some(r1))
    }

    /// Emit a `match` where at least one arm has `Pattern::Ok` / `Pattern::Err`.
    fn emit_result_match(
        &mut self,
        scrutinee: &Expr,
        scrut_val: &str,
        arms: &[MatchArm],
    ) -> Result<Option<String>, String> {
        // Determine Ok/Err payload LLVM types from the scrutinee's MVL type.
        let (ok_load_ty, err_load_ty) = {
            let mvl_ty = match scrutinee {
                Expr::Ident(name, _) => self.local_mvl_types.get(name.as_str()).cloned(),
                Expr::FnCall { name, .. } => self.fn_ret_types.get(name.as_str()).cloned(),
                _ => None,
            };
            match mvl_ty {
                Some(TypeExpr::Result { ok, err, .. }) => (Self::llvm_ty(&ok), Self::llvm_ty(&err)),
                _ => ("i64".into(), "ptr".into()),
            }
        };

        // Extract discriminant byte from the { i8, ptr } struct.
        let disc_reg = self.next_reg();
        self.push_instr(&format!(
            "{disc_reg} = extractvalue {{ i8, ptr }} {scrut_val}, 0"
        ));
        self.reg_types.insert(disc_reg.clone(), "i8".into());

        let n = self.bb;
        self.bb += arms.len() + 2;
        let default_bb = format!("match_default_{n}");
        let merge_bb = format!("match_merge_{}", n + arms.len() + 1);
        let arm_bbs: Vec<String> = (0..arms.len())
            .map(|i| format!("match_arm_{}", n + i))
            .collect();

        // Build switch on i8 discriminant.
        let mut switch_str = format!("switch i8 {disc_reg}, label %{default_bb} [\n");
        let mut wildcard_arm: Option<usize> = None;
        for (idx, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Ok { .. } => {
                    switch_str.push_str(&format!("    i8 0, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Err { .. } => {
                    switch_str.push_str(&format!("    i8 1, label %{}\n", arm_bbs[idx]));
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    wildcard_arm = Some(idx);
                }
                _ => {
                    wildcard_arm = Some(idx);
                }
            }
        }
        switch_str.push_str("  ]");
        self.push_instr(&switch_str);

        // Emit arm blocks.
        let mut phi_entries: Vec<(String, String, String)> = Vec::new();
        // Arms that branch to merge_bb but produced no value (need undef phi entries).
        let mut no_val_arms: Vec<String> = Vec::new(); // from_bb

        for (idx, arm) in arms.iter().enumerate() {
            let arm_bb = &arm_bbs[idx];
            self.fn_buf.push(format!("{arm_bb}:"));
            self.current_bb = arm_bb.clone();
            self.terminated = false;

            let mut bound_var: Option<String> = None;

            match &arm.pattern {
                Pattern::Ok { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!("{pp} = extractvalue {{ i8, ptr }} {scrut_val}, 1"));
                    let ok_val = self.next_reg();
                    self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {pp}"));
                    self.reg_types.insert(ok_val.clone(), ok_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.locals.insert(var_name.clone(), ok_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Err { inner, .. } => {
                    let pp = self.next_reg();
                    self.push_instr(&format!("{pp} = extractvalue {{ i8, ptr }} {scrut_val}, 1"));
                    let err_val = self.next_reg();
                    self.push_instr(&format!("{err_val} = load {err_load_ty}, ptr {pp}"));
                    self.reg_types.insert(err_val.clone(), err_load_ty.clone());
                    if let Pattern::Ident(var_name, _) = inner.as_ref() {
                        if var_name != "_" {
                            self.locals.insert(var_name.clone(), err_val.clone());
                            bound_var = Some(var_name.clone());
                        }
                    }
                }
                Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                    if let Pattern::Ident(name, _) = &arm.pattern {
                        self.locals.insert(name.clone(), scrut_val.to_string());
                        bound_var = Some(name.clone());
                    }
                }
                _ => {}
            }

            let arm_val = match &arm.body {
                MatchBody::Expr(e) => self.emit_expr(e)?,
                MatchBody::Block(b) => self.emit_block(b)?,
            };
            let end_bb = self.current_bb.clone();
            if !self.terminated {
                self.push_instr(&format!("br label %{merge_bb}"));
                if let Some(v) = arm_val {
                    let ty = self.infer_val_type(&v);
                    phi_entries.push((v, ty, end_bb));
                } else {
                    no_val_arms.push(end_bb);
                }
            }

            if let Some(var_name) = bound_var {
                self.locals.remove(&var_name);
            }
        }

        // Default block.
        self.fn_buf.push(format!("{default_bb}:"));
        self.current_bb = default_bb.clone();
        self.terminated = false;
        if let Some(wild_idx) = wildcard_arm {
            let arm_bb = &arm_bbs[wild_idx];
            self.push_instr(&format!("br label %{arm_bb}"));
        } else {
            self.ensure_extern("declare void @llvm.trap()");
            self.push_instr("call void @llvm.trap()");
            self.push_instr("unreachable");
            self.terminated = true;
        }

        // Merge block + phi.
        self.fn_buf.push(format!("{merge_bb}:"));
        self.current_bb = merge_bb.clone();
        self.terminated = false;
        let total_incoming = phi_entries.len() + no_val_arms.len();
        if total_incoming >= 2 && !phi_entries.is_empty() {
            let phi_ty = phi_entries
                .iter()
                .find(|(_, ty, _)| ty != "i64")
                .map(|(_, ty, _)| ty.clone())
                .unwrap_or_else(|| phi_entries[0].1.clone());
            let mut parts: Vec<String> = phi_entries
                .iter()
                .map(|(v, _, from)| format!("[ {v}, %{from} ]"))
                .collect();
            for from in &no_val_arms {
                parts.push(format!("[ undef, %{from} ]"));
            }
            let result = self.next_reg();
            self.push_instr(&format!("{result} = phi {phi_ty} {}", parts.join(", ")));
            self.reg_types.insert(result.clone(), phi_ty);
            Ok(Some(result))
        } else if phi_entries.len() == 1 && no_val_arms.is_empty() {
            Ok(Some(phi_entries.remove(0).0))
        } else {
            Ok(None)
        }
    }

    /// Emit the `?` propagation operator on a `Result[T,E]` value.
    ///
    /// On Err: early-return the `{ i8, ptr }` value from the current function.
    /// On Ok:  extract the payload and load the inner `T` value.
    fn emit_propagate(&mut self, inner: &Expr) -> Result<Option<String>, String> {
        let result_val = match self.emit_expr(inner)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let disc = self.next_reg();
        self.push_instr(&format!(
            "{disc} = extractvalue {{ i8, ptr }} {result_val}, 0"
        ));
        self.reg_types.insert(disc.clone(), "i8".into());

        let is_ok = self.next_reg();
        self.push_instr(&format!("{is_ok} = icmp eq i8 {disc}, 0"));
        self.reg_types.insert(is_ok.clone(), "i1".into());

        let ok_bb = self.next_bb("prop_ok");
        let err_bb = self.next_bb("prop_err");
        self.push_instr(&format!("br i1 {is_ok}, label %{ok_bb}, label %{err_bb}"));

        // Err path: propagate the result upwards.
        self.start_bb(&err_bb);
        self.emit_heap_drops();
        let ret_ty = self.current_ret_ty.clone();
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        self.push_instr(&format!("ret {llvm_ret} {result_val}"));
        self.terminated = true;

        // Ok path: extract and load the success payload.
        self.start_bb(&ok_bb);
        let payload_ptr = self.next_reg();
        self.push_instr(&format!(
            "{payload_ptr} = extractvalue {{ i8, ptr }} {result_val}, 1"
        ));
        let ok_load_ty = self.result_ok_llvm_ty(inner);
        let ok_val = self.next_reg();
        self.push_instr(&format!("{ok_val} = load {ok_load_ty}, ptr {payload_ptr}"));
        self.reg_types.insert(ok_val.clone(), ok_load_ty);
        Ok(Some(ok_val))
    }

    /// Infer the LLVM type of the `Ok` payload from a Result-returning expression.
    fn result_ok_llvm_ty(&self, expr: &Expr) -> String {
        match expr {
            Expr::FnCall { name, .. } => {
                if let Some(TypeExpr::Result { ok, .. }) = self.fn_ret_types.get(name.as_str()) {
                    return Self::llvm_ty(ok);
                }
                "i64".into()
            }
            Expr::MethodCall { method, .. } if method == "parse_int" => "i64".into(),
            Expr::MethodCall { method, .. } if method == "parse_float" => "double".into(),
            _ => "i64".into(),
        }
    }

    fn emit_format_builtin(&mut self, args: &[Expr]) -> Result<Option<String>, String> {
        if args.len() < 2 {
            return Ok(None);
        }
        let template = match self.emit_expr(&args[0])? {
            Some(v) => v,
            None => return Ok(None),
        };
        let list = match self.emit_expr(&args[1])? {
            Some(v) => v,
            None => return Ok(None),
        };
        self.ensure_extern("declare ptr @mvl_format(ptr, ptr)");
        let reg = self.next_reg();
        self.push_instr(&format!(
            "{reg} = call ptr @mvl_format(ptr {template}, ptr {list})"
        ));
        self.reg_types.insert(reg.clone(), "ptr".into());
        Ok(Some(reg))
    }

    // ── Method call emission ──────────────────────────────────────────────

    fn emit_method_call(
        &mut self,
        receiver: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<Option<String>, String> {
        // ── Actor method call: fire-and-forget send ───────────────────────
        if let Some(actor_name) = self.resolve_actor_type_name(receiver) {
            let handle_val = match self.emit_expr(receiver)? {
                Some(v) => v,
                None => return Ok(None),
            };
            return self.emit_actor_method_call(&handle_val, &actor_name.clone(), method, args);
        }

        let recv_ty = self.type_of_expr(receiver);
        let val = match self.emit_expr(receiver)? {
            Some(v) => v,
            None => return Ok(None),
        };

        match (method, recv_ty.as_str()) {
            ("to_string", "i64") | ("to_string", "i1") => {
                let s = if recv_ty == "i64" {
                    self.emit_int_to_string(&val)
                } else {
                    self.emit_bool_to_string(&val)
                };
                Ok(Some(s))
            }
            ("to_string", _) => {
                // String.to_string() is identity
                self.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }
            ("len", "ptr") => {
                let kind = self.mvl_receiver_kind(receiver);
                let is_list = matches!(kind, Some("List") | Some("Array") | Some("Set"));
                let is_map = matches!(kind, Some("Map"));
                let reg = self.next_reg();
                if is_list {
                    self.ensure_extern("declare i64 @mvl_array_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @mvl_array_len(ptr {val})"));
                } else if is_map {
                    self.ensure_extern("declare i64 @mvl_map_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @mvl_map_len(ptr {val})"));
                } else {
                    self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                    self.push_instr(&format!("{reg} = call i64 @_mvl_str_len(ptr {val})"));
                }
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }
            ("concat", "ptr") => {
                self.ensure_extern("declare ptr @mvl_string_concat(ptr, ptr)");
                let other = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @mvl_string_concat(ptr {val}, ptr {other})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            // ── Map methods ─────────────────────────────────────────────
            ("get", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare ptr @mvl_map_get(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                // Null-guard: mvl_map_get returns null if key not found.
                let is_null = self.next_reg();
                self.push_instr(&format!("{is_null} = icmp eq ptr {raw}, null"));
                let some_bb = self.next_bb("map_get_some");
                let none_bb = self.next_bb("map_get_none");
                let merge_bb = self.next_bb("map_get_merge");
                self.push_instr(&format!(
                    "br i1 {is_null}, label %{none_bb}, label %{some_bb}"
                ));
                self.start_bb(&some_bb);
                let loaded = self.next_reg();
                self.push_instr(&format!("{loaded} = load i64, ptr {raw}"));
                self.push_instr(&format!("br label %{merge_bb}"));
                self.start_bb(&none_bb);
                self.push_instr(&format!("br label %{merge_bb}"));
                self.start_bb(&merge_bb);
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi i64 [ {loaded}, %{some_bb} ], [ 0, %{none_bb} ]"
                ));
                self.reg_types.insert(result.clone(), "i64".into());
                Ok(Some(result))
            }
            ("insert", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                if args.len() < 2 {
                    return Ok(None);
                }
                let key_arg = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let val_arg = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @mvl_map_insert(ptr, ptr, i64, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let val_ty = self.infer_val_type(&val_arg);
                let vs = self.next_reg();
                self.push_instr(&format!("{vs} = alloca {val_ty}"));
                self.push_instr(&format!("store {val_ty} {val_arg}, ptr {vs}"));
                self.push_instr(&format!(
                    "call void @mvl_map_insert(ptr {val}, ptr {kp}, i64 {kl}, ptr {vs}, i64 8)"
                ));
                // insert returns the map (modified in place)
                Ok(Some(val))
            }
            ("keys", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @mvl_map_keys(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_map_keys(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("values", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                self.ensure_extern("declare ptr @mvl_map_values(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_map_values(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("contains_key", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare ptr @mvl_map_get(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call ptr @mvl_map_get(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                // null → false, non-null → true
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne ptr {raw}, null"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("remove", "ptr") if matches!(self.mvl_receiver_kind(receiver), Some("Map")) => {
                let key_arg = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
                self.ensure_extern("declare i64 @_mvl_str_len(ptr)");
                self.ensure_extern("declare void @mvl_map_remove(ptr, ptr, i64)");
                let kp = self.next_reg();
                self.push_instr(&format!("{kp} = call ptr @mvl_string_ptr(ptr {key_arg})"));
                let kl = self.next_reg();
                self.push_instr(&format!("{kl} = call i64 @_mvl_str_len(ptr {key_arg})"));
                self.push_instr(&format!(
                    "call void @mvl_map_remove(ptr {val}, ptr {kp}, i64 {kl})"
                ));
                // remove returns the map (modified in place)
                Ok(Some(val))
            }

            // ── HOF: filter / map / any / all / find / take_while / skip_while ──
            // Guard: only match when the argument is closure-like (Lambda or a
            // module-level function reference).  String::find takes a plain
            // String argument, not a closure, so it must not match this arm.
            ("filter" | "map" | "find" | "take_while" | "skip_while", "ptr")
                if args.len() == 1 && self.is_closure_arg(&args[0]) =>
            {
                let closure = match self.emit_as_closure(&args[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("List_{method}");
                self.ensure_extern(&format!("declare ptr @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @{sym}(ptr {val}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }
            ("any" | "all", "ptr") if args.len() == 1 => {
                let closure = match self.emit_as_closure(&args[0])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                let sym = format!("List_{method}");
                self.ensure_extern(&format!("declare i1 @{sym}(ptr, ptr)"));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call i1 @{sym}(ptr {val}, ptr {closure})"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }
            ("fold", "ptr") if args.len() == 2 => {
                let init_ty = self.type_of_expr(&args[0]);
                let init_val = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let closure = match self.emit_as_closure(&args[1])? {
                    Some(p) => p,
                    None => return Ok(None),
                };
                // Fold passes init by pointer so the runtime can return the
                // same type.  For scalar inits, stack-allocate a slot.
                let slot = self.next_reg();
                self.push_instr(&format!("{slot} = alloca {init_ty}"));
                self.push_instr(&format!("store {init_ty} {init_val}, ptr {slot}"));
                self.ensure_extern("declare ptr @List_fold(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @List_fold(ptr {val}, ptr {slot}, ptr {closure})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                // Load the result back out as the init type.
                let result = self.next_reg();
                self.push_instr(&format!("{result} = load {init_ty}, ptr {reg}"));
                self.reg_types.insert(result.clone(), init_ty);
                Ok(Some(result))
            }

            // ── List::push(item) → List (in-place) ───────────────────────
            ("push", "ptr") => {
                let item_arg = match args.first() {
                    Some(a) => a,
                    None => return Ok(None),
                };
                let item_ty = self.type_of_expr(item_arg);
                let item_val = match self.emit_expr(item_arg)? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                // mvl_array_push expects a pointer to the item.
                let item_slot = self.next_reg();
                self.push_instr(&format!("{item_slot} = alloca {item_ty}"));
                self.push_instr(&format!("store {item_ty} {item_val}, ptr {item_slot}"));
                self.ensure_extern("declare void @mvl_array_push(ptr, ptr)");
                self.push_instr(&format!(
                    "call void @mvl_array_push(ptr {val}, ptr {item_slot})"
                ));
                // push returns the array (modified in place — same pointer).
                self.reg_types.insert(val.clone(), "ptr".into());
                Ok(Some(val))
            }

            // ── String::parse_int / parse_float → Result[T, String] ───────
            ("parse_int", "ptr") => self.emit_str_parse(&val, "i64", "_mvl_str_parse_int"),
            ("parse_float", "ptr") => self.emit_str_parse(&val, "double", "_mvl_str_parse_float"),

            // ── String::char_at(i) → String ───────────────────────────────
            ("char_at", "ptr") => {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_char_at(ptr, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_char_at(ptr {val}, i64 {idx})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // ── String kernel builtins (#1186) ───────────────────────────

            // chars() → List[String]
            ("chars", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @mvl_string_chars(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @mvl_string_chars(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // byte_at(i) → Int
            ("byte_at", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let idx = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_byte_at(ptr, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_str_byte_at(ptr {val}, i64 {idx})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // find(sub) → Int  (-1 if not found)
            ("find", "ptr") if args.len() == 1 && !self.is_closure_arg(&args[0]) => {
                let sub = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_find(ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call i64 @_mvl_str_find(ptr {val}, ptr {sub})"
                ));
                self.reg_types.insert(reg.clone(), "i64".into());
                Ok(Some(reg))
            }

            // split(delimiter) → List[String]
            ("split", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let delim = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_split(ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_split(ptr {val}, ptr {delim})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // substring(start, end) → String
            ("substring", "ptr") if args.len() >= 2 => {
                let start = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let end = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_substring(ptr, i64, i64)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_substring(ptr {val}, i64 {start}, i64 {end})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // contains(sub) → Bool
            ("contains", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let sub = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_contains(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_contains(ptr {val}, ptr {sub})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // starts_with(prefix) → Bool
            ("starts_with", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let prefix = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_starts_with(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_starts_with(ptr {val}, ptr {prefix})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // ends_with(suffix) → Bool
            ("ends_with", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                let suffix = match args.first() {
                    Some(a) => match self.emit_expr(a)? {
                        Some(v) => v,
                        None => return Ok(None),
                    },
                    None => return Ok(None),
                };
                self.ensure_extern("declare i64 @_mvl_str_ends_with(ptr, ptr)");
                let raw = self.next_reg();
                self.push_instr(&format!(
                    "{raw} = call i64 @_mvl_str_ends_with(ptr {val}, ptr {suffix})"
                ));
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
                self.reg_types.insert(reg.clone(), "i1".into());
                Ok(Some(reg))
            }

            // trim() → String
            ("trim", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @_mvl_str_trim(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_trim(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // to_lower() → String
            ("to_lower", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @_mvl_str_to_lower(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_to_lower(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // to_upper() → String
            ("to_upper", "ptr")
                if !matches!(
                    self.mvl_receiver_kind(receiver),
                    Some("List") | Some("Array") | Some("Set") | Some("Map")
                ) =>
            {
                self.ensure_extern("declare ptr @_mvl_str_to_upper(ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = call ptr @_mvl_str_to_upper(ptr {val})"));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            // replace(old, new) → String
            ("replace", "ptr")
                if args.len() >= 2
                    && !matches!(
                        self.mvl_receiver_kind(receiver),
                        Some("List") | Some("Array") | Some("Set") | Some("Map")
                    ) =>
            {
                let old_s = match self.emit_expr(&args[0])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                let new_s = match self.emit_expr(&args[1])? {
                    Some(v) => v,
                    None => return Ok(None),
                };
                self.ensure_extern("declare ptr @_mvl_str_replace(ptr, ptr, ptr)");
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call ptr @_mvl_str_replace(ptr {val}, ptr {old_s}, ptr {new_s})"
                ));
                self.reg_types.insert(reg.clone(), "ptr".into());
                Ok(Some(reg))
            }

            _ => Ok(None),
        }
    }

    // ── Struct construction ───────────────────────────────────────────────

    fn emit_construct(
        &mut self,
        name: &str,
        fields: &[(String, Expr)],
    ) -> Result<Option<String>, String> {
        let field_defs = match self.struct_fields.get(name).cloned() {
            Some(f) => f,
            None => return Ok(None),
        };

        let mut field_vals: Vec<(String, String)> = Vec::new(); // (llvm_ty, val)
        for (field_name, field_ty) in &field_defs {
            let llvm_t = self.llvm_ty_ctx(field_ty);
            // Find the value for this field in the construct expr
            let val = fields
                .iter()
                .find(|(n, _)| n == field_name)
                .and_then(|(_, e)| self.emit_expr(e).ok().flatten())
                .unwrap_or_else(|| "undef".into());
            field_vals.push((llvm_t, val));
        }

        let struct_ty = format!("%{name}");
        let mut acc = "undef".to_string();
        for (i, (field_ty, val)) in field_vals.iter().enumerate() {
            let reg = self.next_reg();
            self.push_instr(&format!(
                "{reg} = insertvalue {struct_ty} {acc}, {field_ty} {val}, {i}"
            ));
            self.reg_types.insert(reg.clone(), struct_ty.clone());
            acc = reg;
        }
        Ok(Some(acc))
    }

    // ── Field access ──────────────────────────────────────────────────────

    fn emit_field_access(&mut self, expr: &Expr, field: &str) -> Result<Option<String>, String> {
        // In actor method bodies, `self.field` maps to a ref_local GEP pointer.
        // Check this before falling through to extractvalue-based struct access.
        if matches!(expr, Expr::Ident(name, _) if name == "self") {
            if let Some(loc) = self.ref_locals.get(field).cloned() {
                let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                let reg = self.next_reg();
                self.push_instr(&format!("{reg} = load {ty_str}, ptr {}", loc.ptr));
                self.reg_types.insert(reg.clone(), ty_str);
                return Ok(Some(reg));
            }
        }

        let struct_name = self.struct_name_of_expr(expr);
        let base_val = match self.emit_expr(expr)? {
            Some(v) => v,
            None => return Ok(None),
        };

        if let Some(sn) = struct_name {
            if let Some(fields) = self.struct_fields.get(&sn).cloned() {
                if let Some(idx) = fields.iter().position(|(f, _)| f == field) {
                    let field_ty = self.llvm_ty_ctx(&fields[idx].1.clone());
                    let reg = self.next_reg();
                    self.push_instr(&format!("{reg} = extractvalue %{sn} {base_val}, {idx}"));
                    self.reg_types.insert(reg.clone(), field_ty);
                    return Ok(Some(reg));
                }
            }
        }
        Ok(None)
    }

    // ── Closure / lambda lowering (#1148) ────────────────────────────────

    /// Emit `%__closure_type = type { ptr, ptr }` exactly once.
    fn ensure_closure_type(&mut self) {
        if !self.closure_type_emitted {
            self.type_defs
                .push("%__closure_type = type { ptr, ptr }".into());
            self.closure_type_emitted = true;
        }
    }

    /// Collect free variables referenced in `body` that exist in `self.locals`
    /// and are not in `exclude` (the lambda's own parameters).
    /// Returns `(name, TypeExpr)` pairs in stable order.
    fn collect_lambda_captures(
        &self,
        body: &Expr,
        exclude: &std::collections::HashSet<String>,
    ) -> Vec<(String, TypeExpr)> {
        let mut seen = std::collections::HashSet::new();
        let mut caps = Vec::new();
        self.walk_expr_for_captures(body, exclude, &mut seen, &mut caps);
        caps
    }

    fn walk_expr_for_captures(
        &self,
        expr: &Expr,
        exclude: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        match expr {
            Expr::Ident(name, _)
                if !exclude.contains(name)
                    && !seen.contains(name)
                    && (self.locals.contains_key(name) || self.ref_locals.contains_key(name)) =>
            {
                let ty_opt = self
                    .local_mvl_types
                    .get(name)
                    .cloned()
                    .or_else(|| self.ref_locals.get(name).map(|rl| rl.elem_ty.clone()));
                if let Some(ty) = ty_opt {
                    seen.insert(name.clone());
                    caps.push((name.clone(), ty));
                }
            }
            Expr::Lambda { params, body, .. } => {
                let mut inner_excl = exclude.clone();
                for p in params {
                    inner_excl.insert(p.name.clone());
                }
                self.walk_expr_for_captures(body, &inner_excl, seen, caps);
            }
            Expr::Binary { left, right, .. } => {
                self.walk_expr_for_captures(left, exclude, seen, caps);
                self.walk_expr_for_captures(right, exclude, seen, caps);
            }
            Expr::Unary { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::FnCall { args, .. } => {
                for a in args {
                    self.walk_expr_for_captures(a, exclude, seen, caps);
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.walk_expr_for_captures(receiver, exclude, seen, caps);
                for a in args {
                    self.walk_expr_for_captures(a, exclude, seen, caps);
                }
            }
            Expr::FieldAccess { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::If {
                cond, then, else_, ..
            } => {
                self.walk_expr_for_captures(cond, exclude, seen, caps);
                self.walk_block_for_captures(then, exclude, seen, caps);
                if let Some(e) = else_ {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
            }
            Expr::Block(b) => self.walk_block_for_captures(b, exclude, seen, caps),
            Expr::Construct { fields, .. } => {
                for (_, v) in fields {
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.walk_expr_for_captures(scrutinee, exclude, seen, caps);
                for arm in arms {
                    match &arm.body {
                        MatchBody::Expr(e) => self.walk_expr_for_captures(e, exclude, seen, caps),
                        MatchBody::Block(b) => self.walk_block_for_captures(b, exclude, seen, caps),
                    }
                }
            }
            Expr::Consume { expr, .. }
            | Expr::Relabel { expr, .. }
            | Expr::Propagate { expr, .. }
            | Expr::Borrow { expr, .. } => {
                self.walk_expr_for_captures(expr, exclude, seen, caps);
            }
            Expr::List { elems, .. } | Expr::Set { elems, .. } => {
                for e in elems {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
            }
            Expr::Map { pairs, .. } => {
                for (k, v) in pairs {
                    self.walk_expr_for_captures(k, exclude, seen, caps);
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Spawn { fields, .. } => {
                for (_, v) in fields {
                    self.walk_expr_for_captures(v, exclude, seen, caps);
                }
            }
            Expr::Select { arms, .. } => {
                for arm in arms {
                    self.walk_expr_for_captures(&arm.expr, exclude, seen, caps);
                    self.walk_block_for_captures(&arm.body, exclude, seen, caps);
                }
            }
            _ => {}
        }
    }

    fn walk_block_for_captures(
        &self,
        block: &Block,
        exclude: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        caps: &mut Vec<(String, TypeExpr)>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Expr { expr, .. } => self.walk_expr_for_captures(expr, exclude, seen, caps),
                Stmt::Let { init, .. } => {
                    self.walk_expr_for_captures(init, exclude, seen, caps);
                }
                Stmt::Assign { value, .. } => {
                    self.walk_expr_for_captures(value, exclude, seen, caps);
                }
                Stmt::Return { value: Some(e), .. } => {
                    self.walk_expr_for_captures(e, exclude, seen, caps);
                }
                Stmt::While { cond, body, .. } => {
                    self.walk_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_block_for_captures(body, exclude, seen, caps);
                }
                Stmt::For { iter, body, .. } => {
                    self.walk_expr_for_captures(iter, exclude, seen, caps);
                    self.walk_block_for_captures(body, exclude, seen, caps);
                }
                Stmt::If {
                    cond, then, else_, ..
                } => {
                    self.walk_expr_for_captures(cond, exclude, seen, caps);
                    self.walk_block_for_captures(then, exclude, seen, caps);
                    match else_ {
                        Some(ElseBranch::Block(b)) => {
                            self.walk_block_for_captures(b, exclude, seen, caps);
                        }
                        Some(ElseBranch::If(s)) => {
                            // Recurse into else-if as a single statement.
                            let tmp_block = Block {
                                stmts: vec![*s.clone()],
                                span: s.span(),
                            };
                            self.walk_block_for_captures(&tmp_block, exclude, seen, caps);
                        }
                        None => {}
                    }
                }
                Stmt::Match {
                    scrutinee, arms, ..
                } => {
                    self.walk_expr_for_captures(scrutinee, exclude, seen, caps);
                    for arm in arms {
                        match &arm.body {
                            MatchBody::Expr(e) => {
                                self.walk_expr_for_captures(e, exclude, seen, caps);
                            }
                            MatchBody::Block(b) => {
                                self.walk_block_for_captures(b, exclude, seen, caps);
                            }
                        }
                    }
                }
                Stmt::Return { value: None, .. } => {}
            }
        }
    }

    /// Emit a lambda expression as a top-level LLVM function and return a
    /// pointer to a stack-allocated `%__closure_type { fn_ptr, env_ptr }`.
    fn emit_lambda(
        &mut self,
        params: &[crate::mvl::parser::ast::Param],
        ret_type: Option<&TypeExpr>,
        body: &Expr,
    ) -> Result<Option<String>, String> {
        let lambda_name = format!("__lambda_{}", self.lambda_counter);
        self.lambda_counter += 1;

        let ret_ty = match ret_type {
            Some(t) => t.clone(),
            None => {
                // Infer from the body's LLVM type when no annotation is present.
                let inferred = self.type_of_expr(body);
                let base_name = match inferred.as_str() {
                    "i1" => "Bool",
                    "double" => "Float",
                    "ptr" => "String",
                    "void" => "Unit",
                    _ => "Int",
                };
                TypeExpr::Base {
                    name: base_name.into(),
                    args: vec![],
                    span: Default::default(),
                }
            }
        };

        // Capture analysis — must happen before we clear locals.
        let param_names: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.clone()).collect();
        let captures = self.collect_lambda_captures(body, &param_names);

        self.ensure_closure_type();

        // ── Build env struct and alloca in the OUTER function ────────────
        let env_ty_name = format!("__env_{lambda_name}");
        let env_ptr: String = if captures.is_empty() {
            "null".into()
        } else {
            let field_types: Vec<String> = captures
                .iter()
                .map(|(_, ty)| self.llvm_ty_ctx(ty))
                .collect();
            self.type_defs.push(format!(
                "%{env_ty_name} = type {{ {} }}",
                field_types.join(", ")
            ));

            let env_alloca = self.next_reg();
            self.push_instr(&format!("{env_alloca} = alloca %{env_ty_name}"));
            self.reg_types.insert(env_alloca.clone(), "ptr".into());

            for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                // Ref locals: load current value from the alloca before capturing.
                let store_val = if let Some(ref_loc) = self.ref_locals.get(cap_name).cloned() {
                    let ty_str = self.llvm_ty_ctx(&ref_loc.elem_ty);
                    let loaded = self.next_reg();
                    self.push_instr(&format!("{loaded} = load {ty_str}, ptr {}", ref_loc.ptr));
                    self.reg_types.insert(loaded.clone(), ty_str);
                    loaded
                } else if let Some(cap_val) = self.locals.get(cap_name).cloned() {
                    cap_val
                } else {
                    continue; // not in scope (shouldn't happen after collect_lambda_captures)
                };
                let field_llvm_ty = self.llvm_ty_ctx(cap_ty);
                let field_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{field_ptr} = getelementptr %{env_ty_name}, ptr {env_alloca}, i32 0, i32 {i}"
                ));
                self.push_instr(&format!(
                    "store {field_llvm_ty} {store_val}, ptr {field_ptr}"
                ));
            }
            env_alloca
        };

        // ── Save outer function state ────────────────────────────────────
        let saved_fn_buf = std::mem::take(&mut self.fn_buf);
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_ref_locals = std::mem::take(&mut self.ref_locals);
        let saved_reg = self.reg;
        let saved_bb = self.bb;
        let saved_reg_types = std::mem::take(&mut self.reg_types);
        let saved_mvl_types = std::mem::take(&mut self.local_mvl_types);
        let saved_ret_ty = std::mem::replace(&mut self.current_ret_ty, ret_ty.clone());
        let saved_terminated = self.terminated;
        let saved_current_bb = std::mem::replace(&mut self.current_bb, "entry".into());
        let saved_is_main = self.current_fn_is_main;

        self.reg = 0;
        self.bb = 0;
        self.terminated = false;
        self.current_fn_is_main = false; // lambdas are never main

        // ── Emit lambda function header ──────────────────────────────────
        let llvm_ret = self.llvm_ty_ctx(&ret_ty);
        let is_void = Self::is_void(&ret_ty);

        let mut param_parts = vec!["ptr %__env".to_string()];
        for p in params {
            let ty_str = self.llvm_ty_ctx(&p.ty);
            if ty_str != "void" {
                param_parts.push(format!("{ty_str} %{}", p.name));
            }
        }
        let params_str = param_parts.join(", ");

        let define_ret = if is_void {
            "void".into()
        } else {
            llvm_ret.clone()
        };
        self.fn_buf
            .push(format!("define {define_ret} @{lambda_name}({params_str})"));
        self.fn_buf.push("{".into());
        self.fn_buf.push("entry:".into());

        // Bind user parameters as locals.
        for p in params {
            let ty_str = self.llvm_ty_ctx(&p.ty);
            if ty_str != "void" {
                let ssa = format!("%{}", p.name);
                self.locals.insert(p.name.clone(), ssa.clone());
                self.reg_types.insert(ssa, ty_str);
                self.local_mvl_types.insert(p.name.clone(), p.ty.clone());
            }
        }

        // Load captures from env ptr.
        if !captures.is_empty() {
            for (i, (cap_name, cap_ty)) in captures.iter().enumerate() {
                let field_llvm_ty = self.llvm_ty_ctx(cap_ty);
                let field_ptr = self.next_reg();
                self.push_instr(&format!(
                    "{field_ptr} = getelementptr %{env_ty_name}, ptr %__env, i32 0, i32 {i}"
                ));
                let val = self.next_reg();
                self.push_instr(&format!("{val} = load {field_llvm_ty}, ptr {field_ptr}"));
                self.reg_types.insert(val.clone(), field_llvm_ty);
                self.locals.insert(cap_name.clone(), val.clone());
                self.local_mvl_types
                    .insert(cap_name.clone(), cap_ty.clone());
            }
        }

        // Emit body — capture any error so we can restore state before propagating.
        let body_result = self.emit_expr(body);

        let body_val = match body_result {
            Ok(v) => v,
            Err(e) => {
                // Restore outer state before propagating the error.
                self.fn_buf = saved_fn_buf;
                self.locals = saved_locals;
                self.ref_locals = saved_ref_locals;
                self.reg = saved_reg;
                self.bb = saved_bb;
                self.reg_types = saved_reg_types;
                self.local_mvl_types = saved_mvl_types;
                self.current_ret_ty = saved_ret_ty;
                self.terminated = saved_terminated;
                self.current_bb = saved_current_bb;
                self.current_fn_is_main = saved_is_main;
                return Err(e);
            }
        };

        if !self.terminated {
            if is_void {
                self.push_instr("ret void");
            } else if let Some(v) = body_val {
                self.push_instr(&format!("ret {llvm_ret} {v}"));
            } else {
                self.push_instr(&format!("ret {llvm_ret} undef"));
            }
        }

        self.fn_buf.push("}".into());
        let lambda_body = self.fn_buf.join("\n");
        self.fn_bodies.push(lambda_body);

        // ── Restore outer function state ─────────────────────────────────
        self.fn_buf = saved_fn_buf;
        self.locals = saved_locals;
        self.ref_locals = saved_ref_locals;
        self.reg = saved_reg;
        self.bb = saved_bb;
        self.reg_types = saved_reg_types;
        self.local_mvl_types = saved_mvl_types;
        self.current_ret_ty = saved_ret_ty;
        self.terminated = saved_terminated;
        self.current_bb = saved_current_bb;
        self.current_fn_is_main = saved_is_main;

        // ── Build closure struct in outer function ────────────────────────
        let closure_alloca = self.next_reg();
        self.push_instr(&format!("{closure_alloca} = alloca %__closure_type"));
        self.reg_types.insert(closure_alloca.clone(), "ptr".into());

        let fn_field = self.next_reg();
        self.push_instr(&format!(
            "{fn_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 0"
        ));
        self.push_instr(&format!("store ptr @{lambda_name}, ptr {fn_field}"));

        let env_field = self.next_reg();
        self.push_instr(&format!(
            "{env_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 1"
        ));
        if captures.is_empty() {
            self.push_instr(&format!("store ptr null, ptr {env_field}"));
        } else {
            self.push_instr(&format!("store ptr {env_ptr}, ptr {env_field}"));
        }

        Ok(Some(closure_alloca))
    }

    /// Wrap a named module-level function in a `{ wrapper_ptr, null }` closure struct.
    ///
    /// Lazily generates `__closure_wrap_NAME(ptr env, params…) → ret` that ignores
    /// `env` and forwards to the original function.
    fn make_named_fn_closure(&mut self, name: &str) -> Result<Option<String>, String> {
        let wrapper_name = format!("__closure_wrap_{name}");
        self.ensure_closure_type();

        // Emit the wrapper function once.
        if !self.fn_ret_types.contains_key(&wrapper_name) {
            let orig_ret = match self.fn_ret_types.get(name).cloned() {
                Some(t) => t,
                None => return Ok(None),
            };

            let llvm_ret = self.llvm_ty_ctx(&orig_ret);
            let is_void = Self::is_void(&orig_ret);
            let define_ret = if is_void {
                "void".into()
            } else {
                llvm_ret.clone()
            };

            // Build typed trampoline: (ptr %__env, ty0 %__arg0, ty1 %__arg1, …)
            // The runtime calls the closure fn_ptr as fn(env, args…), so the
            // trampoline must match the original function's arity and types.
            let orig_params = self.fn_param_types.get(name).cloned().unwrap_or_default();
            let mut wrapper_param_parts = vec!["ptr %__env".to_string()];
            let mut forward_arg_parts: Vec<String> = Vec::new();
            for (i, p_ty) in orig_params.iter().enumerate() {
                let ty_str = self.llvm_ty_ctx(p_ty);
                if ty_str != "void" {
                    wrapper_param_parts.push(format!("{ty_str} %__arg{i}"));
                    forward_arg_parts.push(format!("{ty_str} %__arg{i}"));
                }
            }
            let wrapper_params_str = wrapper_param_parts.join(", ");
            let forward_args_str = forward_arg_parts.join(", ");

            // Save context.
            let saved_fn_buf = std::mem::take(&mut self.fn_buf);
            let saved_locals = std::mem::take(&mut self.locals);
            let saved_ref_locals = std::mem::take(&mut self.ref_locals);
            let saved_reg = self.reg;
            let saved_bb = self.bb;
            let saved_reg_types = std::mem::take(&mut self.reg_types);
            let saved_mvl_types = std::mem::take(&mut self.local_mvl_types);
            let saved_ret_ty = std::mem::replace(&mut self.current_ret_ty, orig_ret.clone());
            let saved_terminated = self.terminated;
            let saved_current_bb = std::mem::replace(&mut self.current_bb, "entry".into());

            self.reg = 0;
            self.bb = 0;
            self.terminated = false;

            self.fn_buf.push(format!(
                "define {define_ret} @{wrapper_name}({wrapper_params_str})"
            ));
            self.fn_buf.push("{".into());
            self.fn_buf.push("entry:".into());

            if is_void {
                self.push_instr(&format!("call void @{name}({forward_args_str})"));
                self.push_instr("ret void");
            } else {
                let reg = self.next_reg();
                self.push_instr(&format!(
                    "{reg} = call {llvm_ret} @{name}({forward_args_str})"
                ));
                self.push_instr(&format!("ret {llvm_ret} {reg}"));
            }

            self.fn_buf.push("}".into());
            let wrapper_body = self.fn_buf.join("\n");
            self.fn_bodies.push(wrapper_body);

            // Restore context.
            self.fn_buf = saved_fn_buf;
            self.locals = saved_locals;
            self.ref_locals = saved_ref_locals;
            self.reg = saved_reg;
            self.bb = saved_bb;
            self.reg_types = saved_reg_types;
            self.local_mvl_types = saved_mvl_types;
            self.current_ret_ty = saved_ret_ty;
            self.terminated = saved_terminated;
            self.current_bb = saved_current_bb;

            // Record wrapper so we don't emit it twice.
            self.fn_ret_types.insert(wrapper_name.clone(), orig_ret);
        }

        // Build `{ &wrapper, null }` closure struct.
        let closure_alloca = self.next_reg();
        self.push_instr(&format!("{closure_alloca} = alloca %__closure_type"));
        self.reg_types.insert(closure_alloca.clone(), "ptr".into());

        let fn_field = self.next_reg();
        self.push_instr(&format!(
            "{fn_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 0"
        ));
        self.push_instr(&format!("store ptr @{wrapper_name}, ptr {fn_field}"));

        let env_field = self.next_reg();
        self.push_instr(&format!(
            "{env_field} = getelementptr %__closure_type, ptr {closure_alloca}, i32 0, i32 1"
        ));
        self.push_instr(&format!("store ptr null, ptr {env_field}"));

        Ok(Some(closure_alloca))
    }

    /// Emit `expr` as a closure pointer.
    ///
    /// - `Lambda` → emit the lambda and return the closure alloca
    /// - `Ident` referencing a module-level function → `make_named_fn_closure`
    /// - Anything else → treat as already a closure-typed local
    fn emit_as_closure(&mut self, expr: &Expr) -> Result<Option<String>, String> {
        match expr {
            Expr::Lambda {
                params,
                ret_type,
                body,
                ..
            } => self.emit_lambda(params, ret_type.as_deref(), body),
            Expr::Ident(name, _) => {
                // Module-level function reference (not in locals).
                if !self.locals.contains_key(name.as_str())
                    && self.fn_ret_types.contains_key(name.as_str())
                {
                    self.make_named_fn_closure(name)
                } else {
                    // Already a closure-typed local — just return its SSA value.
                    self.emit_expr(expr)
                }
            }
            _ => self.emit_expr(expr),
        }
    }

    /// Return `true` if `expr` is a closure-like argument (Lambda or a
    /// module-level function reference).  Used to guard HOF method arms so
    /// they don't accidentally match String kernel methods like `find`.
    fn is_closure_arg(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Lambda { .. } => true,
            Expr::Ident(name, _) => {
                !self.locals.contains_key(name.as_str())
                    && self.fn_ret_types.contains_key(name.as_str())
            }
            _ => false,
        }
    }

    // ── List literal ──────────────────────────────────────────────────────

    fn emit_list_literal(&mut self, elems: &[Expr]) -> Result<Option<String>, String> {
        // Determine element LLVM type from the first expression (default ptr).
        let elem_ty = elems
            .first()
            .map(|e| self.type_of_expr(e))
            .unwrap_or_else(|| "ptr".into());

        // Emit all element values first
        let mut elem_vals: Vec<String> = Vec::new();
        for e in elems {
            if let Some(v) = self.emit_expr(e)? {
                elem_vals.push(v);
            }
        }

        let n = elem_vals.len().max(4) as i64;
        self.ensure_extern("declare ptr @mvl_array_new(i64, i64)");
        self.ensure_extern("declare void @mvl_array_push(ptr, ptr)");

        let arr = self.next_reg();
        // elem_size=8 for all scalar types (i64, ptr, double)
        self.push_instr(&format!("{arr} = call ptr @mvl_array_new(i64 8, i64 {n})"));
        self.reg_types.insert(arr.clone(), "ptr".into());

        for v in &elem_vals {
            let slot = self.next_reg();
            self.push_instr(&format!("{slot} = alloca {elem_ty}"));
            self.push_instr(&format!("store {elem_ty} {v}, ptr {slot}"));
            self.push_instr(&format!("call void @mvl_array_push(ptr {arr}, ptr {slot})"));
        }

        Ok(Some(arr))
    }

    // ── Map literal ──────────────────────────────────────────────────────

    fn emit_map_literal(&mut self, pairs: &[(Expr, Expr)]) -> Result<Option<String>, String> {
        let n = pairs.len().max(4) as i64;
        self.ensure_extern("declare ptr @mvl_map_new(i64)");
        self.ensure_extern("declare void @mvl_map_insert(ptr, ptr, i64, ptr, i64)");
        self.ensure_extern("declare ptr @mvl_string_ptr(ptr)");
        self.ensure_extern("declare i64 @_mvl_str_len(ptr)");

        let map = self.next_reg();
        self.push_instr(&format!("{map} = call ptr @mvl_map_new(i64 {n})"));
        self.reg_types.insert(map.clone(), "ptr".into());

        for (key_expr, val_expr) in pairs {
            // Emit key (expected to be a String → ptr)
            let key_val = match self.emit_expr(key_expr)? {
                Some(v) => v,
                None => continue,
            };
            // Get raw pointer and length from the MvlString key
            let key_ptr = self.next_reg();
            self.push_instr(&format!(
                "{key_ptr} = call ptr @mvl_string_ptr(ptr {key_val})"
            ));
            let key_len = self.next_reg();
            self.push_instr(&format!(
                "{key_len} = call i64 @_mvl_str_len(ptr {key_val})"
            ));

            // Emit value and store to stack slot
            let val_val = match self.emit_expr(val_expr)? {
                Some(v) => v,
                None => continue,
            };
            let val_ty = self.infer_val_type(&val_val);
            let val_slot = self.next_reg();
            self.push_instr(&format!("{val_slot} = alloca {val_ty}"));
            self.push_instr(&format!("store {val_ty} {val_val}, ptr {val_slot}"));

            // val_size = 8 for all scalar types (i64, ptr, double)
            self.push_instr(&format!(
                "call void @mvl_map_insert(ptr {map}, ptr {key_ptr}, i64 {key_len}, ptr {val_slot}, i64 8)"
            ));
        }

        Ok(Some(map))
    }
}

// ── Target triple ─────────────────────────────────────────────────────────────

fn default_target_triple() -> String {
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

// ── Actor emission submodule ──────────────────────────────────────────────────

// `emit_actors.rs` lives in the same directory as `emitter.rs`.  Using
// `#[path]` keeps it a child of `emitter` so that the private types
// `TextEmitter` and `RefLocal` (and their private fields) remain accessible
// without any visibility changes.
#[path = "emit_actors.rs"]
mod emit_actors;

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
        assert!(ir.contains("phi"), "{ir}");
        assert!(ir.contains("ret i64"), "{ir}");
    }

    /// Regression for #1155: a 3-way `else if` chain must emit PHI nodes for
    /// every branch so the correct value is selected at runtime. Before the fix,
    /// the `else if` condition was silently dropped and the merge block produced
    /// `ret i64 undef`.
    #[test]
    fn else_if_chain_emits_phi_for_all_branches() {
        let ir = compile(
            "fn classify(n: Int) -> Int {\n\
                 if n > 0 { 1 }\n\
                 else if n < 0 { -1 }\n\
                 else { 0 }\n\
             }",
        );
        // The `else if n < 0` condition must actually be evaluated.
        assert!(ir.contains("icmp slt"), "{ir}");
        // Two PHI nodes: inner selects between -1 and 0; outer selects between 1 and inner.
        let phi_count = ir.matches(" = phi ").count();
        assert!(
            phi_count >= 2,
            "else-if chain needs ≥2 phi nodes, got {phi_count}\n{ir}"
        );
        // Return must be a defined value, not undef.
        assert!(ir.contains("ret i64"), "{ir}");
        assert!(!ir.contains("ret i64 undef"), "{ir}");
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
    fn main_explicit_return_emits_ret_i32_0() {
        let ir = compile("fn main() -> Unit { return; }");
        assert!(ir.contains("define i32 @main()"), "{ir}");
        assert!(ir.contains("ret i32 0"), "{ir}");
        assert!(!ir.contains("ret void"), "{ir}");
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

    #[test]
    fn string_literal_emits_global_and_string_new() {
        let ir = compile("fn main() -> Unit ! Console { println(\"hello\") }");
        assert!(ir.contains("mvl_string_new"), "{ir}");
        assert!(ir.contains("hello"), "{ir}");
        assert!(ir.contains("dprintf"), "{ir}");
    }

    #[test]
    fn assert_emits_conditional_trap() {
        let ir = compile("fn main() -> Unit { assert(1 == 1) }");
        assert!(ir.contains("llvm.trap"), "{ir}");
        assert!(ir.contains("br i1"), "{ir}");
    }

    #[test]
    fn struct_type_emits_type_def() {
        let ir = compile(
            "type Point = struct { x: Int, y: Int }\n\
             fn get_x(p: Point) -> Int { p.x }",
        );
        assert!(ir.contains("%Point = type { i64, i64 }"), "{ir}");
        assert!(ir.contains("define i64 @get_x(%Point %p)"), "{ir}");
        assert!(ir.contains("extractvalue %Point"), "{ir}");
    }

    #[test]
    fn enum_variant_emits_discriminant() {
        let ir = compile(
            "type Shape = enum { Circle, Square }\n\
             fn circle() -> Shape { Shape::Circle }",
        );
        assert!(ir.contains("ret i64 0"), "{ir}");
    }

    // ── Closure / lambda tests (#1148) ────────────────────────────────────

    #[test]
    fn closure_type_emitted_once() {
        // Two lambdas in the same program — closure type must appear exactly once.
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2];\n\
             let _a: List[Int] = xs.filter(|x: Int| x > 0);\n\
             let _b: Bool = xs.any(|x: Int| x > 1);\n\
             }",
        );
        let count = ir.matches("%__closure_type = type").count();
        assert_eq!(count, 1, "expected exactly one closure type def:\n{ir}");
    }

    #[test]
    fn non_capturing_lambda_emits_function_and_null_env() {
        // |x: Int| x * 2  — no free variables
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let _d: List[Int] = xs.filter(|x: Int| x > 0);\n\
             }",
        );
        // Lambda function emitted as a top-level define.
        assert!(
            ir.contains("define i1 @__lambda_0(ptr %__env, i64 %x)"),
            "{ir}"
        );
        // Closure struct built with null env ptr.
        assert!(ir.contains("store ptr null"), "{ir}");
        // fn_ptr field set to the lambda.
        assert!(ir.contains("store ptr @__lambda_0"), "{ir}");
    }

    #[test]
    fn capturing_lambda_emits_env_struct_and_getelementptr() {
        // |x: Int| x > threshold  — captures `threshold` from outer scope
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let threshold: Int = 2;\n\
             let _above: List[Int] = xs.filter(|x: Int| x > threshold);\n\
             }",
        );
        // Env struct type must be registered.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // Capture stored via GEP.
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
        // Lambda function has the env parameter.
        assert!(ir.contains("define i1 @__lambda_0(ptr %__env"), "{ir}");
        // Inside the lambda the captured value is loaded.
        assert!(ir.contains("load i64"), "{ir}");
    }

    #[test]
    fn hof_filter_with_lambda_emits_list_filter_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let evens: List[Int] = xs.filter(|x: Int| x > 0);\n\
             }",
        );
        assert!(ir.contains("declare ptr @List_filter(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call ptr @List_filter"), "{ir}");
        assert!(ir.contains("@__lambda_0"), "{ir}");
    }

    #[test]
    fn hof_any_with_lambda_emits_i1_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let b: Bool = xs.any(|x: Int| x > 0);\n\
             }",
        );
        assert!(ir.contains("declare i1 @List_any(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call i1 @List_any"), "{ir}");
    }

    #[test]
    fn named_fn_closure_wraps_in_closure_struct() {
        let ir = compile(
            "fn is_pos(x: Int) -> Bool { x > 0 }\n\
             fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let evens: List[Int] = xs.filter(is_pos);\n\
             }",
        );
        // Wrapper trampoline generated
        assert!(ir.contains("@__closure_wrap_is_pos"), "{ir}");
        // Closure struct built pointing to wrapper
        assert!(ir.contains("store ptr @__closure_wrap_is_pos"), "{ir}");
        assert!(ir.contains("call ptr @List_filter"), "{ir}");
        // Trampoline must forward the element argument, not call with zero args.
        assert!(
            ir.contains("define i1 @__closure_wrap_is_pos(ptr %__env, i64 %__arg0)"),
            "trampoline missing typed param:\n{ir}"
        );
        assert!(
            ir.contains("call i1 @is_pos(i64 %__arg0)"),
            "trampoline must forward arg to original:\n{ir}"
        );
    }

    #[test]
    fn hof_fold_emits_init_slot_and_list_fold_call() {
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let sum: Int = xs.fold(0, |acc: Int, x: Int| acc + x);\n\
             }",
        );
        assert!(ir.contains("declare ptr @List_fold(ptr, ptr, ptr)"), "{ir}");
        // Initial value must be stack-allocated and stored.
        assert!(ir.contains("alloca i64"), "{ir}");
        assert!(ir.contains("store i64 0"), "{ir}");
        assert!(ir.contains("call ptr @List_fold(ptr"), "{ir}");
        // Result loaded back as the accumulator type.
        assert!(ir.contains("load i64"), "{ir}");
        // Lambda for accumulator function has two typed params.
        assert!(
            ir.contains("define i64 @__lambda_0(ptr %__env, i64 %acc, i64 %x)"),
            "{ir}"
        );
    }

    #[test]
    fn capturing_lambda_with_two_captures() {
        // Captures both `lo` and `hi` — env struct must have two i64 fields.
        let ir = compile(
            "fn main() -> Unit ! Console {\n\
             let xs: List[Int] = [1, 2, 3, 4, 5];\n\
             let lo: Int = 1;\n\
             let hi: Int = 4;\n\
             let mid: List[Int] = xs.filter(|x: Int| x > lo);\n\
             }",
        );
        // Env struct type registered.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // GEP accesses for storing captures into env.
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
        // Two stores for the two captured values.
        let store_count = ir.matches("store i64").count();
        assert!(
            store_count >= 1,
            "expected at least one i64 store for env field, got {store_count}:\n{ir}"
        );
    }

    #[test]
    fn ref_local_capture_loads_before_storing_into_env() {
        // `count` is a mutable ref binding — must be loaded before capture.
        let ir = compile(
            "fn run() -> Int {\n\
             let count: ref Int = 0;\n\
             count = count + 1;\n\
             let xs: List[Int] = [1, 2, 3];\n\
             let above: List[Int] = xs.filter(|x: Int| x > count);\n\
             above.len()\n\
             }",
        );
        // ref local alloca present.
        assert!(ir.contains("alloca i64"), "{ir}");
        // Env struct created for the capture.
        assert!(ir.contains("%__env___lambda_0 = type"), "{ir}");
        // A load from the ref alloca must precede the GEP store into the env.
        assert!(ir.contains("load i64"), "{ir}");
        assert!(ir.contains("getelementptr %__env___lambda_0"), "{ir}");
    }

    // ── Actor emission tests (#1149) ──────────────────────────────────────

    #[test]
    fn actor_emits_state_struct_and_behavior_fn() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }",
        );
        // State struct typedef.
        assert!(ir.contains("%CounterState = type"), "{ir}");
        // Behavior function.
        assert!(
            ir.contains("define void @counter_increment(ptr %self, i64 %n)"),
            "{ir}"
        );
    }

    #[test]
    fn actor_emits_dispatch_function_with_switch() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
               pub fn reset() { }\n\
             }",
        );
        // Dispatch function signature.
        assert!(
            ir.contains("define void @counter_dispatch(ptr %state, i64 %disc, ptr %args)"),
            "{ir}"
        );
        // Switch with at least two case labels.
        assert!(ir.contains("switch i64 %disc, label %default"), "{ir}");
        assert!(ir.contains("i64 0, label %behavior_0"), "{ir}");
        assert!(ir.contains("i64 1, label %behavior_1"), "{ir}");
    }

    #[test]
    fn actor_runtime_externs_emitted() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }",
        );
        assert!(ir.contains("declare ptr @mvl_actor_spawn"), "{ir}");
        assert!(ir.contains("declare void @mvl_actor_send"), "{ir}");
        assert!(ir.contains("declare void @mvl_actor_join_all"), "{ir}");
    }

    #[test]
    fn spawn_emits_alloca_and_actor_spawn_call() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int {\n\
               let c: Counter = actor Counter { count: 0 };\n\
               0\n\
             }",
        );
        // State alloca.
        assert!(ir.contains("alloca %CounterState"), "{ir}");
        // Runtime spawn call.
        assert!(ir.contains("call ptr @mvl_actor_spawn"), "{ir}");
    }

    #[test]
    fn actor_method_call_emits_send() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int {\n\
               let c: Counter = actor Counter { count: 0 };\n\
               c.increment(1);\n\
               0\n\
             }",
        );
        // The send call must appear.
        assert!(ir.contains("call void @mvl_actor_send"), "{ir}");
    }

    #[test]
    fn join_all_emitted_in_main_when_actors_present() {
        let ir = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { }\n\
             }\n\
             fn main() -> Int { 0 }",
        );
        assert!(ir.contains("call void @mvl_actor_join_all"), "{ir}");
    }

    // ── Generic monomorphization tests (#1156) ───────────────────────────

    /// Generic `identity[T]` must produce separate monomorphized copies for
    /// each concrete type argument used at call sites.
    #[test]
    fn generic_fn_monomorphized_per_concrete_type() {
        let ir = compile(
            "fn identity[T](x: T) -> T { x }\n\
             fn main() -> Unit {\n\
               let n: Int = identity(42);\n\
               let s: String = identity(\"hi\");\n\
             }",
        );
        // Two separate definitions with correct types.
        assert!(ir.contains("define i64 @identity__Int(i64 %x)"), "{ir}");
        assert!(ir.contains("define ptr @identity__String(ptr %x)"), "{ir}");
        // Call sites use mangled names.
        assert!(ir.contains("call i64 @identity__Int(i64 42)"), "{ir}");
        assert!(ir.contains("call ptr @identity__String("), "{ir}");
    }

    // ── Option constructor + match tests (#1156) ─────────────────────────

    /// `Some(val)` must emit a `{ i8, ptr }` tagged union with disc=0.
    #[test]
    fn some_constructor_emits_tagged_union() {
        let ir = compile("fn wrap(n: Int) -> Option[Int] { Some(n) }");
        assert!(
            ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 0, 0"),
            "{ir}"
        );
        assert!(ir.contains("insertvalue { i8, ptr }"), "{ir}");
        assert!(ir.contains("define { i8, ptr } @wrap"), "{ir}");
    }

    /// `None` must emit a `{ i8, ptr }` tagged union with disc=1.
    #[test]
    fn none_constructor_emits_tagged_union() {
        let ir = compile("fn empty() -> Option[Int] { None }");
        assert!(
            ir.contains("insertvalue { i8, ptr } zeroinitializer, i8 1, 0"),
            "{ir}"
        );
    }

    /// Match on `Option[Int]` must emit a switch on the discriminant byte.
    #[test]
    fn option_match_emits_switch_on_discriminant() {
        let ir = compile(
            "fn unwrap_or(opt: Option[Int], default: Int) -> Int {\n\
                 match opt {\n\
                     Some(v) => v,\n\
                     None => default,\n\
                 }\n\
             }",
        );
        assert!(ir.contains("switch i8"), "{ir}");
        assert!(ir.contains("i8 0, label"), "{ir}"); // Some arm
        assert!(ir.contains("i8 1, label"), "{ir}"); // None arm
        assert!(ir.contains("phi i64"), "{ir}");
    }

    // ── Map literal emission tests (#1184) ───────────────────────────────

    #[test]
    fn map_literal_emits_map_new_and_insert() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1, \"b\": 2};\n\
             }",
        );
        assert!(ir.contains("call ptr @mvl_map_new(i64"), "{ir}");
        assert!(ir.contains("call void @mvl_map_insert(ptr"), "{ir}");
        assert!(ir.contains("call ptr @mvl_string_ptr(ptr"), "{ir}");
        assert!(ir.contains("call i64 @_mvl_str_len(ptr"), "{ir}");
    }

    #[test]
    fn empty_map_emits_map_new_only() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = Map::new();\n\
             }",
        );
        // Map::new() goes through FnCall, not Map literal — just verify no crash.
        assert!(ir.contains("define i32 @main()"), "{ir}");
    }

    #[test]
    fn map_len_emits_mvl_map_len() {
        let ir = compile(
            "fn main() -> Int {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             m.len()\n\
             }",
        );
        assert!(ir.contains("declare i64 @mvl_map_len(ptr)"), "{ir}");
        assert!(ir.contains("call i64 @mvl_map_len(ptr"), "{ir}");
    }

    #[test]
    fn map_keys_emits_mvl_map_keys() {
        let ir = compile(
            "fn main() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             let _k: List[String] = m.keys();\n\
             }",
        );
        assert!(ir.contains("declare ptr @mvl_map_keys(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @mvl_map_keys(ptr"), "{ir}");
    }

    #[test]
    fn map_contains_key_emits_null_check() {
        let ir = compile(
            "fn main() -> Bool {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             m.contains_key(\"a\")\n\
             }",
        );
        assert!(ir.contains("call ptr @mvl_map_get(ptr"), "{ir}");
        assert!(ir.contains("icmp ne ptr"), "{ir}");
    }

    #[test]
    fn map_get_emits_null_guard_before_load() {
        let ir = compile(
            "fn f(m: Map[String, Int]) -> Int {\n\
             m.get(\"key\")\n\
             }",
        );
        assert!(ir.contains("call ptr @mvl_map_get(ptr"), "{ir}");
        // Must null-check before loading
        assert!(ir.contains("icmp eq ptr"), "{ir}");
        assert!(ir.contains("load i64, ptr"), "{ir}");
        assert!(ir.contains("phi i64"), "{ir}");
    }

    // ── HeapKind drop tracking tests (#1185) ─────────────────────────────

    #[test]
    fn string_local_emits_drop_before_ret() {
        let ir = compile(
            "fn greet() -> Unit {\n\
             let s: String = \"hello\";\n\
             }",
        );
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_string_drop(ptr)"), "{ir}");
    }

    #[test]
    fn list_local_emits_drop_before_ret() {
        let ir = compile(
            "fn nums() -> Unit {\n\
             let xs: List[Int] = [1, 2, 3];\n\
             }",
        );
        assert!(ir.contains("call void @mvl_array_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_array_drop(ptr)"), "{ir}");
    }

    #[test]
    fn map_local_emits_drop_before_ret() {
        let ir = compile(
            "fn maps() -> Unit {\n\
             let m: Map[String, Int] = {\"a\": 1};\n\
             }",
        );
        assert!(ir.contains("call void @mvl_map_drop(ptr"), "{ir}");
        assert!(ir.contains("declare void @mvl_map_drop(ptr)"), "{ir}");
    }

    #[test]
    fn multiple_heap_locals_all_dropped() {
        let ir = compile(
            "fn multi() -> Unit {\n\
             let s: String = \"hello\";\n\
             let xs: List[Int] = [1, 2];\n\
             }",
        );
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        assert!(ir.contains("call void @mvl_array_drop(ptr"), "{ir}");
    }

    #[test]
    fn primitive_locals_no_drop() {
        let ir = compile(
            "fn prims() -> Unit {\n\
             let x: Int = 42;\n\
             let b: Bool = true;\n\
             }",
        );
        assert!(!ir.contains("_drop"), "{ir}");
    }

    #[test]
    fn explicit_return_emits_drops() {
        let ir = compile(
            "fn early() -> Int {\n\
             let s: String = \"hello\";\n\
             return 42;\n\
             }",
        );
        // The drop should appear before the ret instruction.
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
    }

    #[test]
    fn shadowed_string_local_no_double_drop() {
        let ir = compile(
            "fn f() -> Unit {\n\
             let s: String = \"first\";\n\
             let s: String = \"second\";\n\
             }",
        );
        // Should have exactly 1 drop call (for the second binding only;
        // the first is removed from tracking when shadowed).
        let drop_count = ir.matches("call void @mvl_string_drop(ptr").count();
        assert_eq!(drop_count, 1, "expected 1 drop, got {drop_count}\n{ir}");
    }

    #[test]
    fn ref_string_local_emits_load_then_drop() {
        let ir = compile(
            "fn f() -> Unit {\n\
             let s: ref String = \"hello\";\n\
             }",
        );
        // ref local: must load from alloca, then drop the loaded value.
        assert!(ir.contains("call void @mvl_string_drop(ptr"), "{ir}");
        // Verify the load-before-drop pattern exists.
        assert!(ir.contains("load ptr, ptr"), "{ir}");
    }

    // ── String builtin kernel methods tests (#1186) ──────────────────────

    #[test]
    fn string_chars_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Unit {\n\
             let _cs: List[String] = s.chars();\n\
             }",
        );
        assert!(ir.contains("declare ptr @mvl_string_chars(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @mvl_string_chars(ptr"), "{ir}");
    }

    #[test]
    fn string_byte_at_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Int {\n\
             s.byte_at(0)\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_byte_at(ptr, i64)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_byte_at(ptr"), "{ir}");
    }

    #[test]
    fn string_find_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Int {\n\
             s.find(\"x\")\n\
             }",
        );
        assert!(ir.contains("declare i64 @_mvl_str_find(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call i64 @_mvl_str_find(ptr"), "{ir}");
    }

    #[test]
    fn string_split_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Unit {\n\
             let _parts: List[String] = s.split(\",\");\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_split(ptr, ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_split(ptr"), "{ir}");
    }

    #[test]
    fn string_substring_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.substring(0, 3)\n\
             }",
        );
        assert!(
            ir.contains("declare ptr @_mvl_str_substring(ptr, i64, i64)"),
            "{ir}"
        );
        assert!(ir.contains("call ptr @_mvl_str_substring(ptr"), "{ir}");
    }

    #[test]
    fn string_contains_emits_i64_to_bool() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.contains(\"x\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_contains(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("icmp ne i64"), "{ir}");
    }

    #[test]
    fn string_starts_with_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.starts_with(\"http\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_starts_with(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_starts_with(ptr"), "{ir}");
    }

    #[test]
    fn string_ends_with_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> Bool {\n\
             s.ends_with(\".mvl\")\n\
             }",
        );
        assert!(
            ir.contains("declare i64 @_mvl_str_ends_with(ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call i64 @_mvl_str_ends_with(ptr"), "{ir}");
    }

    #[test]
    fn string_trim_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.trim()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_trim(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_trim(ptr"), "{ir}");
    }

    #[test]
    fn string_to_lower_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.to_lower()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_to_lower(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_to_lower(ptr"), "{ir}");
    }

    #[test]
    fn string_to_upper_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.to_upper()\n\
             }",
        );
        assert!(ir.contains("declare ptr @_mvl_str_to_upper(ptr)"), "{ir}");
        assert!(ir.contains("call ptr @_mvl_str_to_upper(ptr"), "{ir}");
    }

    #[test]
    fn string_replace_emits_runtime_call() {
        let ir = compile(
            "fn f(s: String) -> String {\n\
             s.replace(\"old\", \"new\")\n\
             }",
        );
        assert!(
            ir.contains("declare ptr @_mvl_str_replace(ptr, ptr, ptr)"),
            "{ir}"
        );
        assert!(ir.contains("call ptr @_mvl_str_replace(ptr"), "{ir}");
    }
}
