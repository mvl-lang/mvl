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
mod stmts;
mod types;

use inkwell::{
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, StructType},
    values::{BasicValueEnum, FunctionValue, PointerValue},
    AddressSpace,
};
use std::collections::HashMap;

use crate::mvl::parser::ast::{Decl, ExternDecl, ExternFnDecl, FnDecl, Program, TypeExpr};

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
        // Also pre-declare extern fn signatures so calls from fn bodies resolve correctly.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    self.fn_return_types
                        .insert(fd.name.clone(), *fd.return_type.clone());
                    self.declare_fn(fd);
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
        // Second pass: emit bodies.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
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
                // Fallback: void return for non-unit functions whose body failed to emit.
                // LLVM verification will catch the type mismatch and surface an error.
                // TODO(#385): surface a user-visible "unsupported construct" diagnostic here
                //   instead of relying on the IR verifier's opaque error message.
                self.builder.build_return(None).unwrap();
            }
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
