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

mod builtins;
mod exprs;
mod memory;
mod stmts;
mod types;

pub(crate) use memory::HeapKind;

use inkwell::{
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType},
    values::{BasicValueEnum, FunctionValue, PointerValue},
    AddressSpace,
};
use std::collections::{HashMap, HashSet};

use crate::mvl::parser::ast::{
    Block, Decl, Expr, ExternDecl, ExternFnDecl, FnDecl, Param, Program, Stmt, TypeExpr,
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

/// Locate a cdylib by env-var override then by proximity to the current executable.
///
/// Search order:
/// 1. `env_var` environment variable (explicit override)
/// 2. `target/{profile}/{lib_name}.{dylib,so}` — sibling of the current executable
/// 3. `target/{profile}/deps/{lib_name}.{dylib,so}` — Cargo cdylib output location
/// 4. Returns `None` if not found
fn find_cdylib(env_var: &str, lib_name: &str) -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var(env_var) {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for ext in &["dylib", "so"] {
                let lib = dir.join(format!("{lib_name}.{ext}"));
                if lib.exists() {
                    return Some(lib);
                }
                let lib = dir.join(format!("deps/{lib_name}.{ext}"));
                if lib.exists() {
                    return Some(lib);
                }
            }
        }
    }
    None
}

/// Find the `libmvl_memory` shared library for the `lli --load` flag (ADR-0016).
pub fn find_mvl_memory_lib() -> Option<std::path::PathBuf> {
    find_cdylib("MVL_MEMORY_LIB", "libmvl_memory")
}

/// Find the `libmvl_runtime_c` shared library for the `lli --load` flag (ADR-0018).
pub fn find_mvl_runtime_c_lib() -> Option<std::path::PathBuf> {
    find_cdylib("MVL_RUNTIME_C_LIB", "libmvl_runtime_c")
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
/// Parse `// expect-pattern: <glob>` annotation — used for non-deterministic output.
/// Supports `?` (any single char) and `*` (any sequence of chars).
pub fn parse_expect_pattern_annotation(source: &str) -> Option<String> {
    source.lines().find_map(|l| {
        l.trim()
            .strip_prefix("// expect-pattern:")
            .map(|s| s.trim().to_string())
    })
}

/// Simple glob-style pattern match: `?` = any char, `*` = any sequence.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    fn inner(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (None, _) => false,
            (Some('*'), _) => {
                // Try consuming 0..=n chars with the star.
                inner(&p[1..], t) || (!t.is_empty() && inner(p, &t[1..]))
            }
            (_, None) => false,
            (Some('?'), _) => inner(&p[1..], &t[1..]),
            (Some(pc), Some(tc)) => pc == tc && inner(&p[1..], &t[1..]),
        }
    }
    inner(&p, &t)
}

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
    enum_variants: HashMap<String, Vec<(String, crate::mvl::parser::ast::VariantFields)>>,
    /// Struct types: struct_name → [(field_name, TypeExpr)] in declaration order.
    struct_fields: HashMap<String, Vec<(String, TypeExpr)>>,
    /// LLVM named struct types (for structs and payload enums).
    llvm_struct_types: HashMap<String, StructType<'ctx>>,
    /// Return types of user-defined functions (name → MVL TypeExpr).
    /// Used to determine the Ok/Some payload type when extracting from Result/Option.
    fn_return_types: HashMap<String, TypeExpr>,

    // ── L5-08: generic monomorphization ─────────────────────────────────────
    /// All user function declarations (cloned), keyed by name.
    /// Needed to emit monomorphized bodies on demand at call sites.
    fn_decls: HashMap<String, FnDecl>,
    /// Active type-parameter substitutions during monomorphized function emission.
    /// Maps type-param name (e.g. "T") → concrete LLVM type.
    type_subs: HashMap<String, BasicTypeEnum<'ctx>>,
    /// Mangled names of already-emitted monomorphized functions (prevents duplicate emission).
    emitted_monomorphs: HashSet<String>,
    /// MVL TypeExpr for each local variable that has an explicit type annotation.
    /// Used to infer the Ok/Some payload type when the scrutinee is a local variable.
    local_mvl_types: HashMap<String, TypeExpr>,

    // ── L5-14: heap drop tracking ────────────────────────────────────────────
    /// Locals that hold heap-allocated collection values (String, Array, Map).
    /// Keyed by variable name → HeapKind.  Cleared at function entry.
    /// Used to emit `_drop` calls before `return` and at function end.
    pub(crate) heap_locals: HashMap<String, HeapKind>,

    // ── ADR-0019: stdlib import tracking ─────────────────────────────────────
    /// Maps a MVL function name (imported via `use std.*`) to its C-ABI symbol
    /// in `libmvl_runtime_c`.  Populated from `Decl::Use` nodes in emit_program.
    /// Used by emit_fn_call to dispatch to the correct `_mvl_*` extern.
    stdlib_imports: HashMap<String, String>,
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
            fn_decls: HashMap::new(),
            type_subs: HashMap::new(),
            emitted_monomorphs: HashSet::new(),
            local_mvl_types: HashMap::new(),
            heap_locals: HashMap::new(),
            stdlib_imports: HashMap::new(),
        }
    }

    // ── Program emission ─────────────────────────────────────────────────────

    fn emit_program(&mut self, prog: &Program) {
        // ADR-0019: scan `use std.*` imports and build the stdlib dispatch table.
        // Maps each imported MVL function name to its `_mvl_*` C-ABI symbol.
        self.collect_stdlib_imports(prog);

        // Phase B: collect type declarations first.
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                self.register_type_decl(td);
            }
        }
        self.build_llvm_types();

        // First pass: record return types and declarations; pre-declare non-generic functions
        // so forward calls resolve.  Generic functions are emitted on-demand at call sites.
        // Also pre-declare extern fn signatures so calls from fn bodies resolve correctly.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.fn_return_types
                        .insert(fd.name.clone(), *fd.return_type.clone());
                    self.fn_decls.insert(fd.name.clone(), fd.clone());
                    if fd.type_params.is_empty() {
                        self.declare_fn(fd);
                    }
                }
            }
            if let Decl::Extern(ext) = decl {
                for efn in &ext.fns {
                    if self.module.get_function(&efn.name).is_none() {
                        let param_tys: Vec<BasicMetadataTypeEnum> = efn
                            .params
                            .iter()
                            .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
                            .map(Into::into)
                            .collect();
                        let fn_ty = if self.is_unit_type(&efn.return_type) {
                            self.context.void_type().fn_type(&param_tys, false)
                        } else if let Some(ret) = self.mvl_type_to_llvm(&efn.return_type) {
                            ret.fn_type(&param_tys, false)
                        } else {
                            self.context.void_type().fn_type(&param_tys, false)
                        };
                        self.module
                            .add_function(&efn.name, fn_ty, Some(Linkage::External));
                    }
                }
            }
        }
        // Second pass: emit bodies for non-generic functions only.
        // Generic functions are emitted on-demand when their call sites are reached.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test && fd.type_params.is_empty() {
                    self.emit_fn(fd);
                }
            }
        }
        // Third pass: wire extern blocks — emit LLVM IR bodies for `extern "rust"` functions.
        for decl in &prog.declarations {
            if let Decl::Extern(ext) = decl {
                self.emit_extern_decl(ext);
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

    /// Emit LLVM IR for `extern` blocks (issue #381).
    ///
    /// For `extern "c"`: emit `declare` with external linkage (C ABI).
    /// For `extern "rust"`: emit an LLVM IR function body that provides the
    /// implementation via libc. Each function name is matched against a set of
    /// known bridge functions; unknowns get a zero-returning stub.
    fn emit_extern_decl(&mut self, ext: &ExternDecl) {
        for efn in &ext.fns {
            let i64_ty = self.context.i64_type();
            let ptr_ty = self.context.ptr_type(AddressSpace::default());

            match ext.abi.as_str() {
                "c" => {
                    // Skip if already declared.
                    if self.module.get_function(&efn.name).is_some() {
                        continue;
                    }
                    // Just declare — the linker supplies the body.
                    let param_tys: Vec<BasicMetadataTypeEnum> = efn
                        .params
                        .iter()
                        .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
                        .map(Into::into)
                        .collect();
                    let fn_ty = if self.is_unit_type(&efn.return_type) {
                        self.context.void_type().fn_type(&param_tys, false)
                    } else if let Some(ret) = self.mvl_type_to_llvm(&efn.return_type) {
                        ret.fn_type(&param_tys, false)
                    } else {
                        self.context.void_type().fn_type(&param_tys, false)
                    };
                    self.module
                        .add_function(&efn.name, fn_ty, Some(Linkage::External));
                }
                _ => {
                    // For `extern "rust"` and any other ABI, emit an LLVM IR body that
                    // provides a real implementation using libc.
                    self.emit_extern_rust_fn_body(efn, i64_ty, ptr_ty);
                }
            }
        }
    }

    // ── ADR-0019: stdlib import dispatch ─────────────────────────────────────

    /// Scan `Decl::Use` nodes for `use std.*` imports and populate `stdlib_imports`.
    ///
    /// The MVL parser discards individual items from brace imports
    /// (`use std.env.{getuid, getgid}` → `path = ["std", "env"]`), so we register
    /// *all* known symbols for a module when a brace import is detected.
    /// Single-item imports (`use std.env.getuid`) produce `path = ["std", "env", "getuid"]`
    /// and register only that symbol.
    fn collect_stdlib_imports(&mut self, prog: &Program) {
        // Table of all C-ABI dispatching symbols: (module, mvl_name) → symbol.
        // Only no-arg, i64-returning functions are listed here — dispatched via
        // emit_stdlib_call_i64.
        //
        // Excluded (pending follow-up with non-i64 / argument-passing dispatch):
        //   - sigint/sigterm/sighup/sigusr1/sigusr2: return i8, not i64
        //   - signal_reset / signal_ignore: take an i8 argument
        //   - process.is_success: takes an i8 argument
        let known: &[(&str, &str, &str)] = &[
            ("env", "getuid", "_mvl_env_getuid"),
            ("env", "getgid", "_mvl_env_getgid"),
            ("env", "args_len", "_mvl_env_args_len"),
        ];

        for decl in &prog.declarations {
            let Decl::Use(ud) = decl else { continue };
            if ud.path.is_empty() || ud.path[0] != "std" {
                continue;
            }
            if ud.path.len() < 2 {
                continue;
            }
            let module = &ud.path[1];

            if ud.path.len() == 2 {
                // Brace import: `use std.env.{getuid, getgid}` — the parser discards the
                // item list and stores only ["std", "env"] (parser limitation).
                // We register all known symbols for the module as a conservative approximation.
                // Single-item imports always have path.len() == 3 (e.g. ["std", "env", "getuid"]).
                for (m, fn_name, symbol) in known {
                    if *m == module.as_str() {
                        self.stdlib_imports
                            .insert((*fn_name).to_string(), (*symbol).to_string());
                    }
                }
            } else {
                // Single import: `use std.env.getuid` → path = ["std", "env", "getuid"].
                let fn_name = &ud.path[ud.path.len() - 1];
                for (m, kfn, symbol) in known {
                    if *m == module.as_str() && *kfn == fn_name.as_str() {
                        self.stdlib_imports
                            .insert(fn_name.clone(), (*symbol).to_string());
                        break;
                    }
                }
            }
        }
    }

    /// Emit a call to a stdlib C-ABI function with no arguments, returning i64.
    /// Returns `None` if the symbol is unknown.
    pub(crate) fn emit_stdlib_call_i64(
        &mut self,
        symbol: &str,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = self.context.i64_type().fn_type(&[], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self.builder.build_call(fn_val, &[], "stdlib_i64").ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit an LLVM IR function body for a single `extern "rust"` declaration.
    ///
    /// Known bridges:
    ///   `roll_dice() -> Int`  →  `rand() % 6 + 1`  (libc rand, seeded by OS)
    ///
    /// All other functions get a stub that returns 0 / null.
    fn emit_extern_rust_fn_body(
        &mut self,
        efn: &ExternFnDecl,
        i64_ty: inkwell::types::IntType<'ctx>,
        _ptr_ty: inkwell::types::PointerType<'ctx>,
    ) {
        let ret_llvm = self.mvl_type_to_llvm(&efn.return_type);
        let param_tys: Vec<BasicMetadataTypeEnum> = efn
            .params
            .iter()
            .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
            .map(Into::into)
            .collect();

        // Reuse existing declaration (pre-declared in first pass) or create new.
        let fn_val = self.module.get_function(&efn.name).unwrap_or_else(|| {
            let fn_ty = match ret_llvm {
                Some(rt) => rt.fn_type(&param_tys, false),
                None => self.context.void_type().fn_type(&param_tys, false),
            };
            self.module.add_function(&efn.name, fn_ty, None)
        });
        let entry = self.context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);

        match efn.name.as_str() {
            // roll_dice() -> Int: return rand() % 6 + 1
            "roll_dice" => {
                // declare i32 @rand()
                let rand_fn = self.module.get_function("rand").unwrap_or_else(|| {
                    let rand_ty = self.context.i32_type().fn_type(&[], false);
                    self.module
                        .add_function("rand", rand_ty, Some(Linkage::External))
                });
                let rand_call = self.builder.build_call(rand_fn, &[], "rand_call").unwrap();
                use inkwell::values::AnyValue;
                let r32 = BasicValueEnum::try_from(rand_call.as_any_value_enum())
                    .expect("rand() must return i32");
                // sext i32 to i64
                let r64 = self
                    .builder
                    .build_int_s_extend(r32.into_int_value(), i64_ty, "rand64")
                    .unwrap();
                // abs: ensure non-negative (rand() is ≥ 0 but defensive)
                let six = i64_ty.const_int(6, false);
                let one = i64_ty.const_int(1, false);
                let rem = self.builder.build_int_signed_rem(r64, six, "rem").unwrap();
                let result = self.builder.build_int_add(rem, one, "dice").unwrap();
                self.builder.build_return(Some(&result)).unwrap();
            }
            // Generic stub: return zero / null.
            _ => match ret_llvm {
                Some(BasicTypeEnum::IntType(it)) => {
                    let zero = it.const_zero();
                    self.builder.build_return(Some(&zero)).unwrap();
                }
                Some(BasicTypeEnum::FloatType(ft)) => {
                    let zero = ft.const_zero();
                    self.builder.build_return(Some(&zero)).unwrap();
                }
                Some(BasicTypeEnum::PointerType(_)) => {
                    let null = self.context.ptr_type(AddressSpace::default()).const_null();
                    self.builder.build_return(Some(&null)).unwrap();
                }
                _ => {
                    self.builder.build_return(None).unwrap();
                }
            },
        }
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
        self.local_mvl_types.clear();
        self.heap_locals.clear();
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
                    self.maybe_register_heap_param(param, ty);
                }
                // L5-08: record MVL type for Ok/Some payload inference in match arms.
                self.local_mvl_types
                    .insert(param.name.clone(), param.ty.clone());
            }
        }

        let body_val = self.emit_block(&fd.body);

        // Emit return terminator if the block didn't already terminate.
        if !self.terminated {
            // L5-14: drop heap locals; exclude the implicit return value if it is a heap
            // collection (ownership transfers to the caller — dropping it here is a UAF).
            let ret_name = self.heap_return_ident(&fd.body);
            self.emit_heap_drops_except(ret_name);
            if is_c_main {
                let zero = self.context.i32_type().const_int(0, false);
                self.builder.build_return(Some(&zero)).unwrap();
            } else if self.is_unit_type(&fd.return_type) {
                self.builder.build_return(None).unwrap();
            } else if let Some(val) = body_val {
                self.builder.build_return(Some(&val)).unwrap();
            } else {
                // Fallback: void return for non-unit functions whose body failed to emit.
                // LLVM verification will catch the type mismatch and surface an error.
                // TODO(#385): surface a user-visible "unsupported construct" diagnostic here
                //   instead of relying on the IR verifier's opaque error message.
                self.builder.build_return(None).unwrap();
            }
        }
    }

    // ── L5-08: generic monomorphization ──────────────────────────────────────

    /// Emit a function body using an explicit LLVM name (used for monomorphized instances).
    ///
    /// Identical to `emit_fn` but never treats the function as C `main`.
    fn emit_fn_named(&mut self, fd: &FnDecl, name: &str) {
        let fn_val = match self.module.get_function(name) {
            Some(f) => f,
            None => {
                let param_types: Vec<BasicMetadataTypeEnum<'ctx>> = fd
                    .params
                    .iter()
                    .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
                    .map(|t| t.into())
                    .collect();
                let fn_ty = if self.is_unit_type(&fd.return_type) {
                    self.context.void_type().fn_type(&param_types, false)
                } else if let Some(ret) = self.mvl_type_to_llvm(&fd.return_type) {
                    ret.fn_type(&param_types, false)
                } else {
                    self.context.void_type().fn_type(&param_types, false)
                };
                self.module.add_function(name, fn_ty, None)
            }
        };

        let entry = self.context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);
        self.locals.clear();
        self.local_mvl_types.clear();
        self.heap_locals.clear();
        self.terminated = false;
        self.current_fn = Some(fn_val);

        for (i, param) in fd.params.iter().enumerate() {
            if let Some(param_val) = fn_val.get_nth_param(i as u32) {
                param_val.set_name(&param.name);
                if let Some(ty) = self.mvl_type_to_llvm(&param.ty) {
                    let alloca = self.builder.build_alloca(ty, &param.name).unwrap();
                    self.builder.build_store(alloca, param_val).unwrap();
                    self.locals.insert(param.name.clone(), (alloca, ty));
                    self.maybe_register_heap_param(param, ty);
                }
                // L5-08: record MVL type for Ok/Some payload inference in match arms.
                self.local_mvl_types
                    .insert(param.name.clone(), param.ty.clone());
            }
        }

        let body_val = self.emit_block(&fd.body);

        if !self.terminated {
            // L5-14: drop heap locals; exclude the implicit return value if it is a heap
            // collection (ownership transfers to the caller — dropping it here is a UAF).
            let ret_name = self.heap_return_ident(&fd.body);
            self.emit_heap_drops_except(ret_name);
            if self.is_unit_type(&fd.return_type) {
                self.builder.build_return(None).unwrap();
            } else if let Some(val) = body_val {
                self.builder.build_return(Some(&val)).unwrap();
            } else {
                self.builder.build_return(None).unwrap();
            }
        }
    }

    /// Return the name of the last expression in `block` if it is a bare identifier
    /// tracked as a heap local.  Used to exclude the implicit return value from drops
    /// (returning a heap pointer transfers ownership to the caller).
    /// L5-15: register a value parameter in `heap_locals` if it is a heap type.
    /// Borrow params (`&T`) are skipped — the caller retains ownership.
    fn maybe_register_heap_param(&mut self, param: &Param, llvm_ty: BasicTypeEnum) {
        if !matches!(&param.ty, TypeExpr::Ref { .. }) {
            if let Some(kind) = stmts::heap_kind_of(&param.ty) {
                if matches!(llvm_ty, BasicTypeEnum::PointerType(_)) {
                    self.heap_locals.insert(param.name.clone(), kind);
                }
            }
        }
    }

    fn heap_return_ident<'b>(&self, block: &'b Block) -> Option<&'b str> {
        let last = block.stmts.last()?;
        let Stmt::Expr { expr, .. } = last else {
            return None;
        };
        let Expr::Ident(name, _) = expr else {
            return None;
        };
        self.heap_locals
            .contains_key(name.as_str())
            .then_some(name.as_str())
    }

    /// Emit a monomorphized copy of `fd` with the given type-parameter substitutions,
    /// using `mangled_name` as the LLVM symbol.  No-ops if already emitted.
    fn ensure_monomorphized(
        &mut self,
        fd: FnDecl,
        type_subs: HashMap<String, BasicTypeEnum<'ctx>>,
        mangled_name: &str,
    ) {
        if self.emitted_monomorphs.contains(mangled_name) {
            return;
        }
        self.emitted_monomorphs.insert(mangled_name.to_string());

        // Save builder insert point and per-function state.
        let saved_block = self.builder.get_insert_block();
        let saved_subs = std::mem::replace(&mut self.type_subs, type_subs);
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_mvl_types = std::mem::take(&mut self.local_mvl_types);
        let saved_terminated = self.terminated;
        let saved_fn = self.current_fn;

        self.emit_fn_named(&fd, mangled_name);

        // Restore state.
        self.type_subs = saved_subs;
        self.locals = saved_locals;
        self.local_mvl_types = saved_mvl_types;
        self.terminated = saved_terminated;
        self.current_fn = saved_fn;
        if let Some(block) = saved_block {
            self.builder.position_at_end(block);
        }
    }

    /// Infer type-parameter substitutions for `fd` from the LLVM types of `arg_vals`.
    ///
    /// For each parameter whose MVL type is a bare type-parameter name (e.g. `T`),
    /// records the concrete LLVM type of the corresponding argument.
    pub(crate) fn infer_type_subs(
        &self,
        fd: &FnDecl,
        arg_vals: &[BasicValueEnum<'ctx>],
    ) -> HashMap<String, BasicTypeEnum<'ctx>> {
        let mut subs = HashMap::new();
        for (param, val) in fd.params.iter().zip(arg_vals.iter()) {
            if let TypeExpr::Base { name, args, .. } = &param.ty {
                if args.is_empty() && fd.type_params.iter().any(|tp| tp.name() == name.as_str()) {
                    subs.insert(name.clone(), val.get_type());
                }
            }
        }
        subs
    }

    /// Produce a mangled LLVM name for a generic function given its type substitutions.
    ///
    /// Example: `identity` with `T=i64` → `identity_Int`.
    pub(crate) fn mangle_fn_name(
        &self,
        fd: &FnDecl,
        type_subs: &HashMap<String, BasicTypeEnum<'ctx>>,
    ) -> String {
        let suffix: Vec<String> = fd
            .type_params
            .iter()
            .filter_map(|tp| {
                type_subs
                    .get(tp.name())
                    .map(|ty| self.llvm_type_mvl_name(*ty))
            })
            .collect();
        if suffix.is_empty() {
            fd.name.clone()
        } else {
            format!("{}_{}", fd.name, suffix.join("_"))
        }
    }

    /// Human-readable MVL type name for an LLVM type, used in name mangling.
    pub(crate) fn llvm_type_mvl_name(&self, ty: BasicTypeEnum<'ctx>) -> String {
        match ty {
            BasicTypeEnum::IntType(it) => match it.get_bit_width() {
                1 => "Bool".into(),
                8 => "Byte".into(),
                32 => "Char".into(),
                64 => "Int".into(),
                _ => format!("i{}", it.get_bit_width()),
            },
            BasicTypeEnum::FloatType(_) => "Float".into(),
            BasicTypeEnum::PointerType(_) => "Ptr".into(),
            BasicTypeEnum::StructType(st) => st
                .get_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Struct".into()),
            _ => "Unknown".into(),
        }
    }

    // ── Verification and IR output ───────────────────────────────────────────

    fn verify(&self) -> Result<(), String> {
        self.module.verify().map_err(|e| e.to_string())
    }

    fn to_ir_string(&self) -> String {
        self.module.print_to_string().to_string()
    }
}
