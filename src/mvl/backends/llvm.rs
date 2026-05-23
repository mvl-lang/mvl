// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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

mod actors;
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
    ActorDecl, Block, Decl, Expr, ExternDecl, ExternFnDecl, FnDecl, Program, Stmt, TypeExpr,
};

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns true for prelude functions whose call sites are inlined by the LLVM
/// backend (e.g. `println` → `printf`). Their empty prelude stubs must not be
/// emitted, as they would produce dead code that shadows the inline paths.
fn is_inlined_builtin(name: &str) -> bool {
    // #839: println/print/eprintln/eprint removed — they are now pure-MVL wrappers
    // in std/core.mvl that call stdout_write(stdout(), ...) / stderr_write(stderr(), ...).
    // Their bodies are compiled in the second pass like any other user-defined function.
    matches!(
        name,
        "format" | "assert" | "assert_eq" | "assert_ne" | "panic"
    )
}

/// A reusable LLVM compiler that owns its `Context`.
///
/// Construct once and call [`LlvmCompiler::compile_to_ir`] repeatedly to avoid
/// the overhead of allocating a new LLVM context on every compilation.
pub struct LlvmCompiler {
    context: Context,
    /// Controls invariant enforcement level in emitted IR (issue #662).
    pub assert_mode: crate::mvl::backends::AssertMode,
}

impl LlvmCompiler {
    /// Create a new compiler with a fresh LLVM context.
    pub fn new() -> Self {
        Self {
            context: Context::create(),
            assert_mode: crate::mvl::backends::AssertMode::Always,
        }
    }

    /// Compile a MVL program AST to LLVM IR text.
    ///
    /// Returns the IR as a string on success, or an error message on failure.
    pub fn compile_to_ir(&self, prog: &Program, module_name: &str) -> Result<String, String> {
        self.compile_to_ir_with_prelude(&[], prog, module_name)
    }

    /// Compile prelude programs merged with `prog` into a single LLVM IR module.
    ///
    /// This is the LLVM equivalent of `load_implicit_prelude` + `load_mvl_native_stdlib_extras`
    /// on the transpiler path — all declarations are merged before codegen so that
    /// functions defined in the prelude (str_chars, list_get, etc.) are available.
    pub fn compile_to_ir_with_prelude(
        &self,
        prelude: &[Program],
        prog: &Program,
        module_name: &str,
    ) -> Result<String, String> {
        use crate::mvl::parser::lexer::Span;
        let zero = Span {
            line: 0,
            col: 0,
            offset: 0,
            len: 0,
        };
        let mut all_decls = Vec::new();
        for p in prelude {
            all_decls.extend(p.declarations.iter().cloned());
        }
        all_decls.extend(prog.declarations.iter().cloned());
        let merged = Program {
            declarations: all_decls,
            span: zero,
        };
        // #583: run the checker on the merged program so expr_types is available
        // for generic builtin call sites (choice[T], shuffle[T]) during emission.
        let check_result = crate::mvl::checker::check(&merged);
        // ADR-0034: compute the monomorphization plan before emission so all
        // generic instantiations are pre-emitted in emit_program's mono pass.
        let mono = {
            use crate::mvl::passes::mono::{collect_fns, monomorphize};
            let all_fns = collect_fns(std::iter::once(&merged));
            monomorphize(&merged, &all_fns, &check_result.expr_types)
        };
        let mut backend = LlvmBackend::new(&self.context, module_name);
        backend.expr_types = check_result.expr_types;
        backend.mono = Some(mono);
        backend.assert_mode = self.assert_mode;
        backend.emit_program(&merged);
        backend.verify()?;
        Ok(backend.to_ir_string())
    }
}

impl Default for LlvmCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile a MVL program AST to LLVM IR text.
///
/// Convenience one-shot wrapper — creates an [`LlvmCompiler`], compiles, then
/// drops it. For hot loops prefer constructing [`LlvmCompiler`] once and
/// reusing it across calls.
pub fn compile_to_ir(prog: &Program, module_name: &str) -> Result<String, String> {
    LlvmCompiler::new().compile_to_ir(prog, module_name)
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
/// 1. `env_var` environment variable (explicit override, must end in `.dylib` or `.so`)
/// 2. `target/{profile}/{lib_name}.{dylib,so}` — sibling of the current executable
/// 3. `target/{profile}/deps/{lib_name}.{dylib,so}` — Cargo cdylib output location
/// 4. Returns `None` if not found
///
/// **Security note:** `env_var` is a trusted-operator override. It must not be
/// derived from user-controlled input. The extension check guards against
/// obvious misconfiguration but is not a sandbox boundary.
fn find_cdylib(env_var: &str, lib_name: &str) -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var(env_var) {
        let p = std::path::PathBuf::from(&path);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "dylib" | "so") {
            eprintln!("warning: {env_var} ignored — must end in .dylib or .so: {path}");
        } else if p.exists() {
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

/// Find the `libmvl_runtime_c` shared library for the `lli --load` flag (ADR-0018).
///
/// Since `mvl_memory` is now merged into `mvl_runtime_c` (runtime/llvm), only this
/// single library needs to be loaded by lli.
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

// ── ADR-0019: stdlib call signature ──────────────────────────────────────────

/// Describes the calling convention of a C-ABI stdlib symbol.
/// Used by the dispatch table so the codegen emitter can select the right LLVM IR pattern.
#[derive(Clone, Debug)]
pub(crate) enum StdlibSig {
    /// No arguments, returns i64.  e.g. `_mvl_env_getuid`.
    I64NoArg(String),
    /// No arguments, returns f64.  e.g. `_mvl_random_float`.
    F64NoArg(String),
    /// Two i64 arguments, returns i64.  e.g. `_mvl_random_int(min, max)`.
    I64TwoI64Args(String),
    /// One Duration struct argument `{secs: i64, nanos: i64}`, returns void.
    /// The struct is flattened to two i64 parameters at the C-ABI boundary.
    /// e.g. `sleep(d: Duration)` → `_mvl_time_thread_sleep(secs, nanos)`.
    VoidDurationArg(String),
    /// `(MvlString*, MvlMap*) → void` — for log functions.
    /// Both arguments are opaque pointer-typed at the LLVM IR level.
    /// e.g. `log_debug(msg, fields)` → `_mvl_log_debug(ptr, ptr)`.
    VoidStringMapArg(String),

    // ── #435: io stdlib (ptr-based Result returns) ────────────────────────────
    /// `ptr → ptr` — identity pass-through (e.g. `path(s)`: String → Path).
    /// Both types are represented as `MvlString*` at the LLVM IR level.
    PtrIdentArg(String),
    /// `(ptr) → {i8, ptr}` — one-ptr-arg C function returning Result[Unit, String].
    /// The C function returns `{tag, direct_payload}` where direct_payload is a
    /// direct value (null for Ok(Unit), MvlString* for Err).  The emission helper
    /// wraps the payload with a stack alloca to produce the internal `{i8, ptr}`
    /// double-indirected format that `emit_propagate` and `bind_pattern_vars` expect.
    ResultUnitOnePtrArg(String),
    /// `(ptr, ptr) → {i8, ptr}` — two-ptr-arg variant of `ResultUnitOnePtrArg`.
    /// e.g. `write(path, content)` and `append(path, content)`.
    ResultUnitTwoPtrArgs(String),
    /// `(ptr) → {i8, ptr}` — one-ptr-arg C function returning Result[String, String].
    /// The C function returns `{tag, direct_payload}` where direct_payload is a
    /// MvlString* for Ok and Err.  Wrapping is the same as ResultUnitOnePtrArg.
    ResultStringOnePtrArg(String),

    // ── #420/#439: regex stdlib ───────────────────────────────────────────────
    /// `(ptr, ptr, ptr) → *mut c_char` — three-ptr-arg C function returning a heap string.
    /// Caller frees the returned pointer with `libc::free`.
    /// e.g. `_mvl_regex_replace(handle, input, replacement)`.
    StringThreePtrArgs(String),
    /// `(ptr, ptr) → {i8, ptr}` — two-ptr-arg C function returning `Option[Match]`.
    ///
    /// The C function returns `{i8, ptr}` where:
    ///   tag=0 (Some) — payload is a heap-allocated `*mut MvlMatch { text: ptr, start: i64, end: i64 }`
    ///   tag=1 (None) — payload is null
    ///
    /// The emission helper conditionally loads the `%Match` struct from the heap pointer into a
    /// stack slot, then wraps in the double-indirected `{i8, ptr}` format that
    /// `bind_pattern_vars` expects for `Some(m)` pattern binding.
    /// e.g. `_mvl_regex_find(handle, input)`.
    OptionMatchTwoPtrArgs(String),
    /// `i64 → ptr` — one i64 argument, returns an opaque pointer (e.g. `MvlArray*`).
    /// Used for `_mvl_crypto_random_bytes(n)` → `*mut MvlArray` (#507).
    I64ReturnsPtrArg(String),

    // ── #557: parity quick wins ───────────────────────────────────────────────
    /// No arguments, returns an opaque pointer (e.g. `*mut MvlArray`).
    /// Used for `env.args()`, `args.get_args()`.
    PtrNoArg(String),

    // ── #584: regex.find_all ─────────────────────────────────────────────────
    /// `(ptr, ptr) → ptr` — two ptr args, returns an opaque pointer (MvlArray*).
    /// Used for `_mvl_regex_find_all(handle, input)`.
    PtrTwoPtrArgs(String),

    // ── #536: parity additions ────────────────────────────────────────────────
    /// `(ptr) → i64` — one-ptr-arg C function returning i64 (e.g. Bool 0/1).
    /// Used for `exists`, `is_file`, `is_dir`.
    I64OnePtrArg(String),
    /// `(ptr, i64) → {i8, ptr}` — ptr+int args returning Result[Unit, String].
    /// Used for `chmod(path, mode)`.
    ResultUnitPtrI64Args(String),
    /// `(i64) → void / noreturn` — one i64 arg, no return.
    /// Used for `exit(code)`.
    VoidI64Arg(String),

    // ── #586: signal handling ────────────────────────────────────────────────
    /// `(i8) → void` — signal_reset(sig), signal_ignore(sig).
    /// Signal is a unit enum encoded as i8 at the C-ABI boundary.
    VoidI8Arg(String),
    /// `(i8, ptr) → void` — signal_on(sig, handler).
    /// Second arg is a function pointer (non-capturing named fn).
    VoidI8FnPtrArg(String),

    // ── #779: std.net ─────────────────────────────────────────────────────────
    /// `(ptr, i64) → {i8, ptr}` — one ptr + one i64 arg, returns Result[OpaqueHandle, String].
    /// Used for `tcp_listen(host: String, port: Int) → Result[TcpListener, String]`.
    ResultPtrPtrI64Args(String),
    /// `(ptr) → void` — one opaque-pointer arg, no return.
    /// Used for `tcp_close_listener` and `tcp_close_stream`.
    VoidOnePtrArg(String),
    /// `(ptr) → {i8, i64}` — one ptr arg, Ok payload is an i64 value (not a ptr-to-i64).
    /// The C function encodes the i64 directly in the `payload` field of `LlvmResult`
    /// via pointer-sized cast.  Used for `tcp_listener_port`.
    ResultI64OnePtrArg(String),

    // ── #839: std.io stdout/stderr writes ─────────────────────────────────────
    /// `(ptr, ptr) → void` — two opaque-pointer args, no return.
    /// Used for `stdout_write(s: Stdout, line: String)` and
    /// `stderr_write(s: Stderr, line: String)`.
    VoidTwoPtrArgs(String),
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
    /// Struct invariant predicates: struct_name → RefExpr (Phase 6, #670).
    struct_invariants: HashMap<String, crate::mvl::parser::ast::RefExpr>,
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
    /// Type annotation of the let-binding currently being initialised, if any.
    /// Set by Stmt::Let before calling emit_expr so that emit_list_literal can
    /// derive the correct element size for empty list literals (fixes #520).
    pending_let_ty: Option<TypeExpr>,

    // ── L5-14: heap drop tracking ────────────────────────────────────────────
    /// Locals that hold heap-allocated collection values (String, Array, Map).
    /// Keyed by variable name → HeapKind.  Cleared at function entry.
    /// Used to emit `_drop` calls before `return` and at function end.
    pub(crate) heap_locals: HashMap<String, HeapKind>,

    // ── ADR-0019: stdlib import tracking ─────────────────────────────────────
    /// Maps a MVL function name (imported via `use std.*`) to its C-ABI symbol
    /// and calling convention in `libmvl_runtime_c`.  Populated from `Decl::Use`
    /// nodes in emit_program.  Used by emit_fn_call to dispatch to the correct
    /// `_mvl_*` extern via the appropriate emission helper.
    stdlib_imports: HashMap<String, StdlibSig>,

    // ── #583: checker type information ───────────────────────────────────────
    /// Inferred type for every expression, keyed by span.  Populated from
    /// `CheckResult::expr_types` in `compile_to_ir_with_prelude` so that
    /// generic builtin call sites (e.g. `choice[T]`, `shuffle[T]`) can emit
    /// type-specific inline IR without going through the C-ABI dispatch table.
    expr_types: HashMap<crate::mvl::parser::lexer::Span, crate::mvl::checker::types::Ty>,

    // ── ADR-0034: pre-computed monomorphization plan ──────────────────────────
    /// All generic instantiations discovered by the mono pass before emission.
    /// Populated in `compile_to_ir_with_prelude`; consumed by the mono pre-emit
    /// pass in `emit_program` so every call site finds the symbol already defined.
    mono: Option<crate::mvl::passes::mono::MonoProgram>,

    // ── #588: lambda lowering ─────────────────────────────────────────────────
    /// Counter for generating unique names for lambda functions (`__lambda_N`).
    lambda_counter: u32,
    /// Controls how struct invariants are enforced in emitted IR (issue #662).
    assert_mode: crate::mvl::backends::AssertMode,

    // ── Phase 8 / #696: actor declarations ───────────────────────────────────
    /// Actor declarations keyed by actor type name (e.g. `"Counter"`).
    /// Used to detect actor method calls and to emit dispatch functions.
    actor_decls: HashMap<String, ActorDecl>,
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
            struct_invariants: HashMap::new(),
            llvm_struct_types: HashMap::new(),
            fn_return_types: HashMap::new(),
            fn_decls: HashMap::new(),
            type_subs: HashMap::new(),
            emitted_monomorphs: HashSet::new(),
            local_mvl_types: HashMap::new(),
            pending_let_ty: None,
            heap_locals: HashMap::new(),
            stdlib_imports: HashMap::new(),
            expr_types: HashMap::new(),
            mono: None,
            lambda_counter: 0,
            assert_mode: crate::mvl::backends::AssertMode::Always,
            actor_decls: HashMap::new(),
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
            // Phase 8 / #696: register actor state structs alongside regular struct types.
            if let Decl::Actor(ad) = decl {
                self.actor_decls.insert(ad.name.clone(), ad.clone());
                let state_name = format!("{}State", ad.name);
                self.struct_fields.insert(
                    state_name,
                    ad.fields
                        .iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect(),
                );
            }
        }
        self.build_llvm_types();

        // First pass: record return types and declarations; pre-declare non-generic functions
        // so forward calls resolve.  Generic functions are emitted on-demand at call sites.
        // Also pre-declare extern fn signatures so calls from fn bodies resolve correctly.
        // Builtin generic functions (e.g. list_len[T], list_get[T]) are also pre-declared
        // here because they are NOT monomorphized at call sites — the fourth pass supplies
        // a single pointer-typed body that works for all element types.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    // Type-attached methods use a mangled name for LLVM lookup (#868).
                    let llvm_name: String = if let Some(recv_ty) = &fd.receiver_type {
                        format!("{}_{}", recv_ty, fd.name)
                    } else {
                        fd.name.clone()
                    };
                    // Note: duplicate fn_decls entries (same llvm_name) silently overwrite
                    // here — prelude + program re-declarations are expected.  A free function
                    // named `Logger_info` would shadow the mangled symbol for `fn Logger::info`;
                    // this is caught at the MVL checker level (DuplicateFnDecl / UndefinedType)
                    // before LLVM is reached (#875 review).
                    // Don't overwrite stdlib-registered return types: when a stdlib
                    // function shadows a prelude function (e.g. regex.find vs strings.find),
                    // the C-ABI dispatch always wins, so the return type must match.
                    if self.stdlib_imports.contains_key(&fd.name) {
                        self.fn_return_types
                            .entry(llvm_name.clone())
                            .or_insert(*fd.return_type.clone());
                    } else {
                        self.fn_return_types
                            .insert(llvm_name.clone(), *fd.return_type.clone());
                    }
                    self.fn_decls.insert(llvm_name, fd.clone());
                    if fd.type_params.is_empty() || fd.is_builtin {
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
        // Phase 8 / #696: actor pass — emit behavior functions + dispatch functions.
        // Must run before the second pass so that dispatch function symbols are declared
        // when `emit_actor_spawn` is called inside user function bodies (e.g. main).
        if !self.actor_decls.is_empty() {
            self.declare_actor_runtime_fns();
            let actors: Vec<ActorDecl> = self.actor_decls.values().cloned().collect();
            for ad in actors {
                self.emit_actor_decl(&ad);
            }
        }

        // ADR-0034 mono pass: pre-emit all monomorphized generic function copies so
        // call sites find the symbol already defined instead of triggering JIT emission.
        // Must run before the second pass so that user function bodies can call them.
        if let Some(mono) = &self.mono {
            struct MonoEntry {
                mangled: String,
                type_sub_exprs: Vec<(String, TypeExpr)>,
                decl: crate::mvl::parser::ast::FnDecl,
            }
            let entries: Vec<MonoEntry> = mono
                .fns
                .iter()
                .filter(|mf| !mf.type_subs.is_empty())
                .map(|mf| MonoEntry {
                    mangled: mf.mangled_name.clone(),
                    type_sub_exprs: mf
                        .type_subs
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    decl: mf.decl.clone(),
                })
                .collect();
            for entry in entries {
                let llvm_subs: HashMap<String, BasicTypeEnum<'ctx>> = entry
                    .type_sub_exprs
                    .iter()
                    .filter_map(|(k, v)| self.mvl_type_to_llvm(v).map(|t| (k.clone(), t)))
                    .collect();
                self.ensure_monomorphized(entry.decl, llvm_subs, &entry.mangled);
            }
        }

        // Second pass: emit bodies for non-generic non-builtin functions.
        // Generic functions are emitted on-demand when their call sites are reached.
        // Prelude stubs for inlined builtins are skipped — their call sites emit
        // inline IR directly (e.g. printf for println), so the stub would be dead code.
        // Last-definition wins: RUST_BACKED_STDLIB pure-MVL bodies (regex, time) are
        // appended to the prelude after implicit-prelude modules (strings, lists) by
        // load_rust_backed_stdlib_fns, so same-named wrappers (e.g. regex::replace
        // over strings::replace) automatically win without special ordering.
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test
                    && !fd.is_builtin
                    && fd.type_params.is_empty()
                    && !is_inlined_builtin(&fd.name)
                {
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

        // Fourth pass: emit LLVM IR bodies for `pub builtin fn` declarations.
        // These are the type-operation kernel (string/list primitives) declared in the
        // implicit prelude; treated identically to `extern "rust"` functions.
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        for decl in &prog.declarations.clone() {
            if let Decl::Fn(fd) = decl {
                if fd.is_builtin && !is_inlined_builtin(&fd.name) {
                    // #928: Extension methods on builtin types use mangled names in the
                    // LLVM module (e.g. `String_chars`). The bridge body match arms use
                    // the old-style prefixed names (e.g. `str_chars`), so we translate.
                    let bridge_name = if let Some(recv_ty) = &fd.receiver_type {
                        let prefix = match recv_ty.as_str() {
                            "String" => "str",
                            "List" => "list",
                            "Map" => "map",
                            "Set" => "set",
                            "Option" => "option",
                            "Result" => "result",
                            _ => recv_ty.as_str(),
                        };
                        format!("{}_{}", prefix, fd.name)
                    } else {
                        fd.name.clone()
                    };
                    // Use the mangled LLVM name so the bridge body is attached to the
                    // function that call sites actually reference.
                    let llvm_name = if let Some(recv) = &fd.receiver_type {
                        format!("{}_{}", recv, fd.name)
                    } else {
                        fd.name.clone()
                    };
                    let efn = ExternFnDecl {
                        name: bridge_name,
                        params: fd.params.clone(),
                        return_type: fd.return_type.clone(),
                        effects: fd.effects.clone(),
                        totality: fd.totality.clone(),
                        span: fd.span,
                    };
                    // Override: attach bridge body to the mangled LLVM name.
                    self.emit_extern_rust_fn_body_named(&efn, i64_ty, ptr_ty, &llvm_name);
                }
            }
        }
    }

    /// Declare a function signature without emitting its body.
    fn declare_fn(&self, fd: &FnDecl) {
        let llvm_name: String = if let Some(recv_ty) = &fd.receiver_type {
            format!("{}_{}", recv_ty, fd.name)
        } else {
            fd.name.clone()
        };
        if self.module.get_function(&llvm_name).is_some() {
            return; // already declared
        }
        let (fn_ty, _) = self.build_fn_type(fd);
        self.module.add_function(&llvm_name, fn_ty, None);
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

    // ── ADR-0019: stdlib sig derivation (#557) ───────────────────────────────

    /// Derive the C-ABI symbol name for a stdlib function from its module and MVL name.
    ///
    /// Most symbols follow `_mvl_{module}_{fn_name}`, with three exceptions:
    /// - `time.sleep` uses `_mvl_time_thread_sleep` (POSIX thread-sleep naming).
    /// - `log.*` functions already carry the `log_` prefix in their MVL name,
    ///   so the symbol is `_mvl_{fn_name}` to avoid `_mvl_log_log_debug`.
    /// - `crypto.crypto_random_bytes` would double the prefix — use `_mvl_{fn_name}`.
    fn derive_c_abi_symbol(module: &str, fn_name: &str) -> String {
        match (module, fn_name) {
            ("time", "sleep") => "_mvl_time_thread_sleep".into(),
            ("log", _) => format!("_mvl_{fn_name}"),
            ("crypto", _) if fn_name.starts_with("crypto_") => format!("_mvl_{fn_name}"),
            _ => format!("_mvl_{module}_{fn_name}"),
        }
    }

    /// Strip IFC labels (`Tainted<T>`, `Secret<T>`, etc.) and refinements from a type.
    fn unlabel_type(te: &TypeExpr) -> &TypeExpr {
        match te {
            TypeExpr::Labeled { inner, .. } => Self::unlabel_type(inner),
            TypeExpr::Refined { inner, .. } => Self::unlabel_type(inner),
            _ => te,
        }
    }

    /// Return the outermost type name of `te` after stripping labels/refinements.
    /// Returns `""` for non-named types (tuples, fn types, etc.).
    fn base_type_name(te: &TypeExpr) -> &str {
        match Self::unlabel_type(te) {
            TypeExpr::Base { name, .. } => name.as_str(),
            TypeExpr::Option { .. } => "Option",
            TypeExpr::Result { .. } => "Result",
            _ => "",
        }
    }

    /// True if a type name is heap-pointer-sized at the C-ABI level.
    ///
    /// Everything except primitive scalars and `Duration` (flattened to two i64s)
    /// is passed as an opaque pointer (`MvlString*`, `MvlArray*`, struct ptr, etc.).
    fn is_ptr_type(name: &str) -> bool {
        !matches!(
            name,
            "Int" | "Float" | "Bool" | "Unit" | "Never" | "Duration" | ""
        )
    }

    /// Infer a [`StdlibSig`] from a `pub builtin fn` declaration's return and parameter types.
    ///
    /// Returns `None` for functions whose type shapes aren't supported by the LLVM
    /// backend (complex generics, signal handlers, IFC-typed env vars, etc.).
    /// Callers silently skip `None` — no error is emitted.
    fn stdlib_sig_from_decl(
        module: &str,
        fn_name: &str,
        ret_ty: &TypeExpr,
        params: &[crate::mvl::parser::ast::Param],
    ) -> Option<StdlibSig> {
        let symbol = Self::derive_c_abi_symbol(module, fn_name);
        let unlabeled_ret = Self::unlabel_type(ret_ty);
        let ret_base = Self::base_type_name(unlabeled_ret);
        let n = params.len();
        let p0 = params
            .first()
            .map(|p| Self::base_type_name(&p.ty))
            .unwrap_or("");
        let p1 = params
            .get(1)
            .map(|p| Self::base_type_name(&p.ty))
            .unwrap_or("");
        let p2 = params
            .get(2)
            .map(|p| Self::base_type_name(&p.ty))
            .unwrap_or("");

        match (ret_base, n) {
            // ── Scalar returns ────────────────────────────────────────────────
            ("Int", 0) => Some(StdlibSig::I64NoArg(symbol)),
            ("Float", 0) => Some(StdlibSig::F64NoArg(symbol)),
            // ── Ptr return, no args (e.g. env.args, args.get_args) ────────────
            (ret, 0) if Self::is_ptr_type(ret) => Some(StdlibSig::PtrNoArg(symbol)),
            ("Int", 2) if p0 == "Int" && p1 == "Int" => Some(StdlibSig::I64TwoI64Args(symbol)),
            // ── Void/Never returns ────────────────────────────────────────────
            ("Unit" | "Never", 1) if p0 == "Int" => Some(StdlibSig::VoidI64Arg(symbol)),
            // ── #586: Signal (unit enum, i8 at C-ABI boundary) — must precede VoidOnePtrArg ─
            ("Unit", 1) if p0 == "Signal" => Some(StdlibSig::VoidI8Arg(symbol)),
            // ── #779: opaque ptr → void (tcp_close_listener, tcp_close_stream) ─
            ("Unit", 1) if Self::is_ptr_type(p0) => Some(StdlibSig::VoidOnePtrArg(symbol)),
            ("Unit", 2) if p0 == "Signal" => Some(StdlibSig::VoidI8FnPtrArg(symbol)),
            ("Unit", 1) if p0 == "Duration" => Some(StdlibSig::VoidDurationArg(symbol)),
            ("Unit", 2) if p0 == "String" && p1 == "Map" => {
                Some(StdlibSig::VoidStringMapArg(symbol))
            }
            // ── #839: (ptr, ptr) → void — stdout_write / stderr_write ─────────
            ("Unit", 2) if Self::is_ptr_type(p0) && Self::is_ptr_type(p1) => {
                Some(StdlibSig::VoidTwoPtrArgs(symbol))
            }
            // ── Bool predicates: ptr → i64 ────────────────────────────────────
            ("Bool", 1) if Self::is_ptr_type(p0) => Some(StdlibSig::I64OnePtrArg(symbol)),
            // ── Int from opaque handle: ptr → i64 (e.g. _instant_epoch_seconds) ──
            ("Int", 1) if Self::is_ptr_type(p0) => Some(StdlibSig::I64OnePtrArg(symbol)),
            // ── Result returns (must precede generic ptr patterns) ────────────
            ("Result", 1) => {
                let ok_base = if let TypeExpr::Result { ok, .. } = unlabeled_ret {
                    Self::base_type_name(ok)
                } else {
                    ""
                };
                if ok_base == "Unit" && Self::is_ptr_type(p0) {
                    Some(StdlibSig::ResultUnitOnePtrArg(symbol))
                } else if ok_base == "Int" && Self::is_ptr_type(p0) {
                    // e.g. tcp_listener_port(listener) → Result[Int, String]
                    Some(StdlibSig::ResultI64OnePtrArg(symbol))
                } else if Self::is_ptr_type(ok_base) && Self::is_ptr_type(p0) {
                    Some(StdlibSig::ResultStringOnePtrArg(symbol))
                } else {
                    None
                }
            }
            ("Result", 2) if p1 == "Int" => {
                let ok_base = if let TypeExpr::Result { ok, .. } = unlabeled_ret {
                    Self::base_type_name(ok)
                } else {
                    ""
                };
                if ok_base == "Unit" && Self::is_ptr_type(p0) {
                    Some(StdlibSig::ResultUnitPtrI64Args(symbol))
                } else if Self::is_ptr_type(ok_base) && Self::is_ptr_type(p0) {
                    // e.g. tcp_listen(host: String, port: Int) → Result[TcpListener, String]
                    Some(StdlibSig::ResultPtrPtrI64Args(symbol))
                } else {
                    None
                }
            }
            ("Result", 2) => {
                let ok_base = if let TypeExpr::Result { ok, .. } = unlabeled_ret {
                    Self::base_type_name(ok)
                } else {
                    ""
                };
                if ok_base == "Unit" && Self::is_ptr_type(p0) && Self::is_ptr_type(p1) {
                    Some(StdlibSig::ResultUnitTwoPtrArgs(symbol))
                } else {
                    None
                }
            }
            // ── Option[Match], two ptr args (must precede generic ptr patterns) ─
            ("Option", 2) if Self::is_ptr_type(p0) && Self::is_ptr_type(p1) => {
                let inner = if let TypeExpr::Option { inner, .. } = unlabeled_ret {
                    Self::base_type_name(inner)
                } else {
                    ""
                };
                if inner == "Match" {
                    Some(StdlibSig::OptionMatchTwoPtrArgs(symbol))
                } else {
                    None
                }
            }
            // ── Ptr return, int arg (crypto_random_bytes, random.bytes) ───────
            (ret, 1) if Self::is_ptr_type(ret) && p0 == "Int" => {
                Some(StdlibSig::I64ReturnsPtrArg(symbol))
            }
            // ── Ptr → ptr identity (sha256, sha512, path) ────────────────────
            (ret, 1) if Self::is_ptr_type(ret) && Self::is_ptr_type(p0) => {
                Some(StdlibSig::PtrIdentArg(symbol))
            }
            // ── Ptr return, two ptr args (e.g. regex.find_all) ────────────────
            (ret, 2)
                if Self::is_ptr_type(ret) && Self::is_ptr_type(p0) && Self::is_ptr_type(p1) =>
            {
                Some(StdlibSig::PtrTwoPtrArgs(symbol))
            }
            // ── String/ptr return, three ptr args (regex.replace) ─────────────
            (ret, 3)
                if Self::is_ptr_type(ret)
                    && Self::is_ptr_type(p0)
                    && Self::is_ptr_type(p1)
                    && Self::is_ptr_type(p2) =>
            {
                Some(StdlibSig::StringThreePtrArgs(symbol))
            }
            // ── Unrecognized: complex generics, signal handlers, etc. ─────────
            _ => None,
        }
    }

    /// Parse `{module}.mvl` from the embedded stdlib and return all non-generic
    /// `pub builtin fn` declarations.
    ///
    /// Returns an empty vec if the module file isn't embedded (pure-MVL modules).
    /// Generic builtins (e.g. `choice[T]`, `shuffle[T]`) are excluded — they have
    /// no single concrete C-ABI calling convention.
    fn load_module_builtins(module: &str) -> Vec<crate::mvl::parser::ast::FnDecl> {
        use crate::mvl::parser::ast::Decl;
        use crate::mvl::parser::Parser;
        use crate::mvl::stdlib::stdlib_content;

        // Modules that have `_`-prefixed builtins with C-ABI backing in mvl_runtime_c.
        // Only these modules expose private builtins the LLVM JIT must resolve (#899).
        const MODULES_WITH_PRIVATE_BUILTINS: &[&str] = &["time"];

        let filename = format!("{module}.mvl");
        let Some(content) = stdlib_content(&filename) else {
            return Vec::new();
        };
        let (mut parser, _) = Parser::new(content);
        let prog = parser.parse_program();
        let include_private = MODULES_WITH_PRIVATE_BUILTINS.contains(&module);
        prog.declarations
            .into_iter()
            .filter_map(|decl| {
                if let Decl::Fn(fd) = decl {
                    if fd.is_builtin
                        && fd.type_params.is_empty()
                        && (!fd.name.starts_with('_') || include_private)
                    {
                        Some(fd)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    // ── ADR-0019: stdlib import dispatch ─────────────────────────────────────

    /// Scan `Decl::Use` nodes for `use std.*` imports and populate `stdlib_imports`.
    ///
    /// C-ABI symbol names and calling conventions are derived from the corresponding
    /// `pub builtin fn` declarations in each stdlib `.mvl` file — no hardcoded
    /// dispatch table required.  Adding a new `pub builtin fn` automatically works
    /// in both backends as long as its type shape is recognised by
    /// [`Self::stdlib_sig_from_decl`].
    ///
    /// Brace imports (`use std.env.{getuid, getgid}` → path `["std", "env"]`) register
    /// all supported builtins for that module; single-item imports register only the
    /// named symbol.
    fn collect_stdlib_imports(&mut self, prog: &Program) {
        // Pre-load builtins for every module referenced in `use std.*` declarations,
        // plus `crypto` which is always registered regardless of explicit imports.
        let mut module_cache: HashMap<String, Vec<crate::mvl::parser::ast::FnDecl>> =
            HashMap::new();
        for decl in &prog.declarations {
            let Decl::Use(ud) = decl else { continue };
            if ud.path.len() >= 2 && ud.path[0] == "std" {
                let m = ud.path[1].as_str();
                module_cache
                    .entry(m.to_string())
                    .or_insert_with(|| Self::load_module_builtins(m));
            }
        }
        module_cache
            .entry("crypto".to_string())
            .or_insert_with(|| Self::load_module_builtins("crypto"));

        // Register stdlib imports from `use std.*` declarations.
        for decl in &prog.declarations {
            let Decl::Use(ud) = decl else { continue };
            if ud.path.is_empty() || ud.path[0] != "std" || ud.path.len() < 2 {
                continue;
            }
            let module = &ud.path[1];
            let Some(builtins) = module_cache.get(module.as_str()) else {
                continue;
            };

            if ud.path.len() == 2 {
                // Brace import: `use std.env.{getuid, getgid}` — the parser discards the
                // item list and stores only `["std", "env"]` (parser limitation).
                // Register all supported builtins for this module.
                for fd in builtins {
                    if let Some(sig) =
                        Self::stdlib_sig_from_decl(module, &fd.name, &fd.return_type, &fd.params)
                    {
                        self.stdlib_imports.entry(fd.name.clone()).or_insert(sig);
                    }
                }
            } else {
                // Single import: `use std.env.getuid` → path = ["std", "env", "getuid"].
                let fn_name = ud.path.last().unwrap();
                if let Some(fd) = builtins.iter().find(|fd| &fd.name == fn_name) {
                    if let Some(sig) =
                        Self::stdlib_sig_from_decl(module, fn_name, &fd.return_type, &fd.params)
                    {
                        self.stdlib_imports.insert(fn_name.clone(), sig);
                    }
                }
            }
        }

        // #839: io handle builtins — always available, no `use std.io` required.
        // core.mvl's println/print/eprintln/eprint call these; they must be reachable
        // without an explicit import so that the implicit prelude compiles correctly.
        module_cache
            .entry("io".to_string())
            .or_insert_with(|| Self::load_module_builtins("io"));
        for fn_name in &["stdout", "stderr", "stdout_write", "stderr_write"] {
            if let Some(fd) = module_cache
                .get("io")
                .and_then(|fns| fns.iter().find(|fd| fd.name.as_str() == *fn_name))
            {
                if let Some(sig) =
                    Self::stdlib_sig_from_decl("io", &fd.name, &fd.return_type, &fd.params)
                {
                    self.stdlib_imports.entry(fd.name.clone()).or_insert(sig);
                }
            }
        }

        // #180/#438/#507: crypto tier-1 builtins — always available, no `use` import required.
        // Derived from pub builtin fn declarations in crypto.mvl like all other stdlib symbols.
        // Use `entry().or_insert()` so an explicit `use std.crypto.*` import doesn't conflict.
        for fn_name in &["sha256", "sha512", "crypto_random_bytes"] {
            if let Some(fd) = module_cache
                .get("crypto")
                .and_then(|fns| fns.iter().find(|fd| fd.name.as_str() == *fn_name))
            {
                if let Some(sig) =
                    Self::stdlib_sig_from_decl("crypto", &fd.name, &fd.return_type, &fd.params)
                {
                    self.stdlib_imports.entry(fd.name.clone()).or_insert(sig);
                }
            }
        }

        // #508: Register return type for crypto_random_bytes so local_mvl_types tracks
        // it as Secret[List[Int]] when used in let-bindings — enables the codegen-level
        // assert that guards public-sink emitters against Secret leaks.
        {
            use crate::mvl::parser::lexer::Span;
            let s = Span::default();
            let int_ty = TypeExpr::Base {
                name: "Int".into(),
                args: vec![],
                span: s,
            };
            let list_int = TypeExpr::Base {
                name: "List".into(),
                args: vec![int_ty],
                span: s,
            };
            let secret_list_int = TypeExpr::Labeled {
                label: "Secret".to_string(),
                inner: Box::new(list_int),
                span: s,
            };
            self.fn_return_types
                .entry("crypto_random_bytes".into())
                .or_insert(secret_list_int);
        }

        // #435: Register return types for io stdlib functions so that
        // `infer_result_ok_llvm_ty` can distinguish `Result[Unit,String]` (ok=None)
        // from `Result[String,String]` (ok=Some(ptr)) at emit_propagate time.
        // Only registered when the program imports from std.io.
        let imports_io = prog.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.len() >= 2 && ud.path[0] == "std" && ud.path[1] == "io"
            } else {
                false
            }
        });
        if imports_io {
            use crate::mvl::parser::lexer::Span;
            let s = Span::default();
            let unit = TypeExpr::Base {
                name: "Unit".into(),
                args: vec![],
                span: s,
            };
            let string = TypeExpr::Base {
                name: "String".into(),
                args: vec![],
                span: s,
            };
            let path = TypeExpr::Base {
                name: "Path".into(),
                args: vec![],
                span: s,
            };
            let result_unit_str = || TypeExpr::Result {
                ok: Box::new(unit.clone()),
                err: Box::new(string.clone()),
                span: s,
            };
            let result_str_str = || TypeExpr::Result {
                ok: Box::new(string.clone()),
                err: Box::new(string.clone()),
                span: s,
            };
            // path(s: String) → Path
            self.fn_return_types.entry("path".into()).or_insert(path);
            // Result[Unit, String] functions
            for name in &[
                "write",
                "append",
                "create_dir_all",
                "remove",
                "create_symlink",
                "chmod",
            ] {
                self.fn_return_types
                    .entry((*name).to_string())
                    .or_insert_with(result_unit_str);
            }
            // Result[String, String] functions
            for name in &["read_to_string", "read_file", "read_link"] {
                self.fn_return_types
                    .entry((*name).to_string())
                    .or_insert_with(result_str_str);
            }
            // Bool (i64) functions — exists, is_file, is_dir have plain i64 return
            // (no fn_return_types entry needed; emitter uses i64 directly)
        }

        // #585: Register `DateTime` struct fields for programs that import `std.time`.
        // `time.mvl` is a Rust-backed stdlib module and is never loaded into the LLVM
        // prelude, so register_type_decl never sees its type declarations.
        let imports_time = prog.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.len() >= 2 && ud.path[0] == "std" && ud.path[1] == "time"
            } else {
                false
            }
        });
        if imports_time {
            use crate::mvl::parser::lexer::Span;
            let s = Span {
                line: 0,
                col: 0,
                offset: 0,
                len: 0,
            };
            let mk_base = |name: &str| TypeExpr::Base {
                name: name.to_string(),
                args: vec![],
                span: s,
            };
            let int_ty = mk_base("Int");
            // Register `DateTime = struct { year: Int, month: Int, day: Int, hour: Int, minute: Int, second: Int }`
            self.struct_fields
                .entry("DateTime".into())
                .or_insert_with(|| {
                    vec![
                        ("year".into(), int_ty.clone()),
                        ("month".into(), int_ty.clone()),
                        ("day".into(), int_ty.clone()),
                        ("hour".into(), int_ty.clone()),
                        ("minute".into(), int_ty.clone()),
                        ("second".into(), int_ty.clone()),
                    ]
                });
        }

        // #779: Register return types for net stdlib functions so that `?` propagation
        // and match arms can infer the Ok payload type (TcpListener/TcpStream = opaque ptr).
        // net.mvl is Rust-backed and never loaded into the LLVM prelude.
        let imports_net = prog.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.len() >= 2 && ud.path[0] == "std" && ud.path[1] == "net"
            } else {
                false
            }
        });
        if imports_net {
            use crate::mvl::parser::lexer::Span;
            let s = Span::default();
            let mk_base = |name: &str| TypeExpr::Base {
                name: name.to_string(),
                args: vec![],
                span: s,
            };
            let string_ty = mk_base("String");
            let listener_ty = mk_base("TcpListener");
            let stream_ty = mk_base("TcpStream");
            let int_ty = mk_base("Int");
            let mk_result = |ok: TypeExpr| TypeExpr::Result {
                ok: Box::new(ok),
                err: Box::new(string_ty.clone()),
                span: s,
            };
            // tcp_listen(host, port) → Result[TcpListener, String]
            self.fn_return_types
                .entry("tcp_listen".into())
                .or_insert_with(|| mk_result(listener_ty.clone()));
            // tcp_connect(host, port) → Result[TcpStream, String]
            self.fn_return_types
                .entry("tcp_connect".into())
                .or_insert_with(|| mk_result(stream_ty.clone()));
            // tcp_accept(listener) → Result[TcpStream, String]
            self.fn_return_types
                .entry("tcp_accept".into())
                .or_insert_with(|| mk_result(stream_ty.clone()));
            // tcp_read(stream) → Result[Tainted[String], String]
            self.fn_return_types
                .entry("tcp_read".into())
                .or_insert_with(|| mk_result(string_ty.clone()));
            // tcp_listener_port(listener) → Result[Int, String]
            self.fn_return_types
                .entry("tcp_listener_port".into())
                .or_insert_with(|| mk_result(int_ty));
        }

        // #586: Register Signal enum variants for programs that import std.env.
        // env.mvl is a Rust-backed stdlib module (skipped by load_mvl_native_stdlib_extras),
        // so register_type_decl never sees its Signal type declaration.
        let imports_env = prog.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.len() >= 2 && ud.path[0] == "std" && ud.path[1] == "env"
            } else {
                false
            }
        });
        if imports_env {
            use crate::mvl::parser::ast::VariantFields;
            self.enum_variants
                .entry("Signal".into())
                .or_insert_with(|| {
                    vec![
                        ("SIGINT".into(), VariantFields::Unit),
                        ("SIGTERM".into(), VariantFields::Unit),
                        ("SIGHUP".into(), VariantFields::Unit),
                        ("SIGUSR1".into(), VariantFields::Unit),
                        ("SIGUSR2".into(), VariantFields::Unit),
                    ]
                });
        }

        // #420: Register return type for regex.compile so that `?` propagation
        // can infer the Ok payload type (Regex = opaque ptr).
        let imports_regex = prog.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.len() >= 2 && ud.path[0] == "std" && ud.path[1] == "regex"
            } else {
                false
            }
        });
        if imports_regex {
            use crate::mvl::parser::lexer::Span;
            let s = Span {
                line: 0,
                col: 0,
                offset: 0,
                len: 0,
            };
            let mk_base = |name: &str| TypeExpr::Base {
                name: name.to_string(),
                args: vec![],
                span: s,
            };
            let string_ty = mk_base("String");
            let int_ty = mk_base("Int");

            // Register `Match = struct { text: String, start: Int, end: Int }` into
            // struct_fields so build_llvm_types can create the LLVM struct type.
            // (std/regex.mvl type declarations are not in the user program's AST so
            //  register_type_decl never sees them.)
            self.struct_fields.entry("Match".into()).or_insert_with(|| {
                vec![
                    ("text".into(), string_ty.clone()),
                    ("start".into(), int_ty.clone()),
                    ("end".into(), int_ty.clone()),
                ]
            });

            let regex_ty = mk_base("Regex");
            let match_ty = mk_base("Match");

            // compile(pattern: String) → Result[Regex, String]
            self.fn_return_types
                .entry("compile".into())
                .or_insert(TypeExpr::Result {
                    ok: Box::new(regex_ty),
                    err: Box::new(string_ty),
                    span: s,
                });
            // find(re: Regex, s: String) → Option[Match]
            self.fn_return_types
                .entry("find".into())
                .or_insert(TypeExpr::Option {
                    inner: Box::new(match_ty),
                    span: s,
                });
        }
    }

    // ── #583/#907: generic builtin inline emitters ─────────────────────────

    /// Emit inline IR for `choice[T](list) -> Option[T]`.
    ///
    /// Delegates random index selection to `_mvl_random_choice_index` (C-ABI),
    /// then does the type-dependent element load inline (Int/Float vs ptr) and
    /// wraps in `Some` / `None`.
    pub(crate) fn emit_random_choice(
        &mut self,
        list_expr: &crate::mvl::parser::ast::Expr,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use crate::mvl::checker::types::Ty;
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());

        let list_ptr = self.emit_expr(list_expr)?.into_pointer_value();

        // Call _mvl_random_choice_index(arr) → i64 (index or -1).
        let idx = self
            .emit_stdlib_call_i64_one_ptr_arg("_mvl_random_choice_index", list_ptr.into())?
            .into_int_value();

        let parent_fn = self.current_fn?;
        let is_empty = self
            .builder
            .build_int_compare(
                inkwell::IntPredicate::SLT,
                idx,
                i64_ty.const_zero(),
                "ch_empty",
            )
            .unwrap();

        let some_bb = self.context.append_basic_block(parent_fn, "ch_some");
        let none_bb = self.context.append_basic_block(parent_fn, "ch_none");
        let merge_bb = self.context.append_basic_block(parent_fn, "ch_merge");
        self.builder
            .build_conditional_branch(is_empty, none_bb, some_bb)
            .unwrap();

        // Some branch: load element at idx, wrap in Some.
        self.builder.position_at_end(some_bb);
        self.terminated = false;
        use inkwell::values::AnyValue;
        let get_fn = self.get_mvl_array_get();
        let elem_ptr = inkwell::values::BasicValueEnum::try_from(
            self.builder
                .build_call(get_fn, &[list_ptr.into(), idx.into()], "ch_eptr")
                .unwrap()
                .as_any_value_enum(),
        )
        .ok()?
        .into_pointer_value();

        let elem_ty = self.expr_types.get(&list_expr.span()).cloned();
        let loaded: inkwell::values::BasicValueEnum<'ctx> = match elem_ty.as_ref().and_then(|t| {
            if let Ty::List(inner) = t {
                Some(inner.as_ref())
            } else {
                None
            }
        }) {
            Some(Ty::Int) | Some(Ty::Bool) => {
                self.builder.build_load(i64_ty, elem_ptr, "ch_val").unwrap()
            }
            Some(Ty::Float) => self
                .builder
                .build_load(self.context.f64_type(), elem_ptr, "ch_val")
                .unwrap(),
            _ => self.builder.build_load(ptr_ty, elem_ptr, "ch_val").unwrap(),
        };

        let some_val = self.emit_some_from_val(loaded)?;
        self.builder.build_unconditional_branch(merge_bb).unwrap();
        let some_end = self.builder.get_insert_block()?;

        // None branch.
        self.builder.position_at_end(none_bb);
        self.terminated = false;
        let none_val = self.emit_none_val()?;
        self.builder.build_unconditional_branch(merge_bb).unwrap();
        let none_end = self.builder.get_insert_block()?;

        // Merge with phi.
        self.builder.position_at_end(merge_bb);
        self.terminated = false;
        let phi = self
            .builder
            .build_phi(some_val.get_type(), "ch_result")
            .unwrap();
        phi.add_incoming(&[(&some_val, some_end), (&none_val, none_end)]);
        Some(phi.as_basic_value())
    }

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

    /// Emit a call to a stdlib C-ABI function with no arguments, returning f64.
    pub(crate) fn emit_stdlib_call_f64(
        &mut self,
        symbol: &str,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = self.context.f64_type().fn_type(&[], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self.builder.build_call(fn_val, &[], "stdlib_f64").ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit a call to a stdlib C-ABI function with no arguments, returning a pointer.
    pub(crate) fn emit_stdlib_call_ptr_no_arg(
        &mut self,
        symbol: &str,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = ptr_ty.fn_type(&[], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self.builder.build_call(fn_val, &[], "stdlib_ptr").ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit a call to a stdlib C-ABI function `(i64, i64) → i64`.
    pub(crate) fn emit_stdlib_call_i64_two_args(
        &mut self,
        symbol: &str,
        a: inkwell::values::BasicValueEnum<'ctx>,
        b: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let i64_ty = self.context.i64_type();
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = i64_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(i64_ty),
                    BasicMetadataTypeEnum::from(i64_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(a),
                    BasicMetadataValueEnum::from(b),
                ],
                "stdlib_i64_2a",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit a call to a stdlib C-ABI function `(i64, i64) → void`.
    /// Returns a constant i64 zero as a stand-in Unit value for the expression result.
    pub(crate) fn emit_stdlib_call_void_two_args(
        &mut self,
        symbol: &str,
        a: inkwell::values::BasicValueEnum<'ctx>,
        b: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let i64_ty = self.context.i64_type();
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = self.context.void_type().fn_type(
                &[
                    BasicMetadataTypeEnum::from(i64_ty),
                    BasicMetadataTypeEnum::from(i64_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        self.builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(a),
                    BasicMetadataValueEnum::from(b),
                ],
                "",
            )
            .ok()?;
        // Return i64 0 as the Unit value.
        Some(i64_ty.const_zero().into())
    }

    /// Emit a call to a stdlib C-ABI function `(ptr, ptr) → void`.
    /// Used for log functions: `_mvl_log_*(MvlString*, MvlMap*)`.
    /// Returns i64 zero as the Unit value.
    pub(crate) fn emit_stdlib_call_void_string_map(
        &mut self,
        symbol: &str,
        msg: inkwell::values::BasicValueEnum<'ctx>,
        fields: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = self.context.void_type().fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        self.builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(msg),
                    BasicMetadataValueEnum::from(fields),
                ],
                "",
            )
            .ok()?;
        Some(self.context.i64_type().const_zero().into())
    }

    /// Emit a call to `_mvl_time_thread_sleep` from a `Duration` struct argument.
    /// Flattens `Duration {secs: i64, nanos: i64}` into two i64 parameters.
    /// Returns i64 zero as the Unit value.
    pub(crate) fn emit_stdlib_call_void_duration_arg(
        &mut self,
        symbol: &str,
        duration: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let dur_struct = duration.into_struct_value();
        let secs = self
            .builder
            .build_extract_value(dur_struct, 0, "dur_secs")
            .ok()?;
        let nanos = self
            .builder
            .build_extract_value(dur_struct, 1, "dur_nanos")
            .ok()?;
        self.emit_stdlib_call_void_two_args(symbol, secs, nanos)
    }

    // ── #435: io C-ABI emission helpers ──────────────────────────────────────

    /// Emit a call to a C-ABI io function `(ptr) → {i8, ptr}`.
    ///
    /// The C function returns `{ tag: i8, direct_payload: ptr }` where `direct_payload`
    /// is the raw value (null for Ok(Unit), MvlString* for Ok(String) or Err).
    /// This helper wraps the payload with a stack alloca to produce the internal
    /// double-indirected `{i8, ptr}` format expected by `emit_propagate`.
    pub(crate) fn emit_stdlib_call_result_one_ptr_arg(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);

        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = result_ty.fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(fn_val, &[arg.into()], "io_c_call")
            .ok()?;
        use inkwell::values::AnyValue;
        let c_val = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        self.wrap_c_result_with_slot(c_val, result_ty)
    }

    /// Emit a call to a C-ABI io function `(ptr, ptr) → {i8, ptr}`.
    /// Same wrapping as `emit_stdlib_call_result_one_ptr_arg`.
    pub(crate) fn emit_stdlib_call_result_two_ptr_args(
        &mut self,
        symbol: &str,
        a: inkwell::values::BasicValueEnum<'ctx>,
        b: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);

        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = result_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(a),
                    BasicMetadataValueEnum::from(b),
                ],
                "io_c_call",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        let c_val = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        self.wrap_c_result_with_slot(c_val, result_ty)
    }

    /// Emit a call to a C-ABI `ptr → ptr` function (e.g. `_mvl_io_path`).
    pub(crate) fn emit_stdlib_call_ptr_identity(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = ptr_ty.fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(fn_val, &[arg.into()], "io_path")
            .ok()?;
        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit a call to a C-ABI `(ptr, ptr) → ptr` function (e.g. `_mvl_regex_find_all`).
    ///
    /// Struct-typed arguments (e.g. `DateTime`) are automatically boxed on the stack so
    /// they can be passed as opaque pointers to the C function.  This enables
    /// `format_datetime(dt, fmt)` where `dt` is an LLVM struct value (#585).
    pub(crate) fn emit_stdlib_call_ptr_two_ptr_args(
        &mut self,
        symbol: &str,
        a: inkwell::values::BasicValueEnum<'ctx>,
        b: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;

        // Coerce struct values to stack pointers so they can be passed as `ptr` to C.
        let a = self.coerce_struct_to_stack_ptr(a);
        let b = self.coerce_struct_to_stack_ptr(b);

        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = ptr_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(a),
                    BasicMetadataValueEnum::from(b),
                ],
                "ptr2_c",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Coerce a `StructValue` to a stack-allocated pointer so it can be passed to a
    /// C-ABI function expecting `*const T`.  All other value kinds are returned as-is.
    fn coerce_struct_to_stack_ptr(
        &mut self,
        val: inkwell::values::BasicValueEnum<'ctx>,
    ) -> inkwell::values::BasicValueEnum<'ctx> {
        if let BasicValueEnum::StructValue(sv) = val {
            let slot = self
                .builder
                .build_alloca(sv.get_type(), "struct_slot")
                .unwrap();
            self.builder.build_store(slot, sv).unwrap();
            BasicValueEnum::PointerValue(slot)
        } else {
            val
        }
    }

    // ── #420/#439: regex emission helpers ────────────────────────────────────────

    /// Emit a call to `_mvl_regex_replace`: `(ptr, ptr, ptr) → *mut c_char`.
    ///
    /// Returns the heap-allocated result string as a ptr value.
    pub(crate) fn emit_stdlib_call_string_three_ptr_args(
        &mut self,
        symbol: &str,
        a: inkwell::values::BasicValueEnum<'ctx>,
        b: inkwell::values::BasicValueEnum<'ctx>,
        c: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = ptr_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(a),
                    BasicMetadataValueEnum::from(b),
                    BasicMetadataValueEnum::from(c),
                ],
                "regex_replace",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// Emit a call to `_mvl_regex_find`: `(ptr, ptr) → {i8, ptr}` returning `Option[Match]`.
    ///
    /// The C function returns `{i8, ptr}` where tag=0 (Some) carries a heap `*mut MvlMatch`
    /// and tag=1 (None) carries null.  This helper:
    /// 1. Calls the C function.
    /// 2. Conditionally (tag=0) loads the `%Match` struct from the heap pointer into a
    ///    stack-allocated `%Match` slot so that `bind_pattern_vars` can load it by value.
    /// 3. Returns a wrapped `{i8, ptr}` where field 1 is a pointer to the `%Match` slot.
    pub(crate) fn emit_stdlib_call_option_match_two_ptr_args(
        &mut self,
        symbol: &str,
        handle: inkwell::values::BasicValueEnum<'ctx>,
        input: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::{AnyValue, BasicMetadataValueEnum};
        use inkwell::IntPredicate;

        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let c_result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);

        // Get/declare `_mvl_regex_find(ptr, ptr) → {i8, ptr}`.
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = c_result_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(ptr_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };

        // Call the C function.
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(handle),
                    BasicMetadataValueEnum::from(input),
                ],
                "regex_find_c",
            )
            .ok()?;
        let c_val = BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        let BasicValueEnum::StructValue(c_sv) = c_val else {
            return None;
        };

        // Extract tag and the heap MvlMatch* from the C return value.
        let tag = self.builder.build_extract_value(c_sv, 0, "find_tag").ok()?;
        let match_ptr = self.builder.build_extract_value(c_sv, 1, "find_ptr").ok()?;
        let match_ptr_pv = match_ptr.into_pointer_value();

        // Look up the %Match LLVM struct type (registered when std/regex.mvl is parsed).
        let match_llvm_ty = self.llvm_struct_types.get("Match").copied()?;

        // Allocate a stack slot for the Match struct — sized for the full struct.
        let match_slot = self
            .builder
            .build_alloca(match_llvm_ty, "find_match_slot")
            .unwrap();

        // Emit a conditional branch: only dereference match_ptr when tag == 0 (Some).
        let parent_fn = self.builder.get_insert_block()?.get_parent()?;
        let then_bb = self.context.append_basic_block(parent_fn, "find_some");
        let merge_bb = self.context.append_basic_block(parent_fn, "find_merge");

        let tag_i8 = tag.into_int_value();
        let is_some = self
            .builder
            .build_int_compare(
                IntPredicate::EQ,
                tag_i8,
                self.context.i8_type().const_zero(),
                "find_is_some",
            )
            .ok()?;
        self.builder
            .build_conditional_branch(is_some, then_bb, merge_bb)
            .ok()?;

        // then_bb: dereference the heap MvlMatch* and store the struct into our slot.
        self.builder.position_at_end(then_bb);
        let match_val = self
            .builder
            .build_load(match_llvm_ty, match_ptr_pv, "find_match_val")
            .ok()?;
        self.builder.build_store(match_slot, match_val).ok()?;
        self.builder.build_unconditional_branch(merge_bb).ok()?;

        // merge_bb: build the wrapped {tag, &match_slot} in the internal format.
        self.builder.position_at_end(merge_bb);
        let opt_ty = c_result_ty; // same {i8, ptr} layout
        let wrapped = self.builder.build_alloca(opt_ty, "find_wrapped").unwrap();
        let tag_gep = self
            .builder
            .build_struct_gep(opt_ty, wrapped, 0, "find_tag_ptr")
            .unwrap();
        self.builder.build_store(tag_gep, tag_i8).unwrap();
        let payload_gep = self
            .builder
            .build_struct_gep(opt_ty, wrapped, 1, "find_payload_ptr")
            .unwrap();
        self.builder.build_store(payload_gep, match_slot).unwrap();

        Some(
            self.builder
                .build_load(opt_ty, wrapped, "find_result")
                .unwrap(),
        )
    }

    /// Emit a call to a C-ABI `i64 → ptr` function (e.g. `_mvl_crypto_random_bytes`).
    ///
    /// The C function takes one i64 argument and returns an opaque pointer (e.g. `*mut MvlArray`).
    pub(crate) fn emit_stdlib_call_i64_returns_ptr(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let i64_val = match arg {
            inkwell::values::BasicValueEnum::IntValue(v) => v,
            _ => return None,
        };
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let fn_val = if let Some(f) = self.module.get_function(symbol) {
            f
        } else {
            let fn_ty = ptr_ty.fn_type(&[i64_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        };
        let call = self
            .builder
            .build_call(fn_val, &[i64_val.into()], symbol)
            .ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// `#536`: `(ptr) → i64` — e.g. `exists(path)`, `is_file(path)`, `is_dir(path)`.
    pub(crate) fn emit_stdlib_call_i64_one_ptr_arg(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = i64_ty.fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(fn_val, &[arg.into()], symbol)
            .ok()?;
        use inkwell::values::AnyValue;
        inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()
    }

    /// `#536`: `(ptr, i64) → {i8, ptr}` Result[Unit, String] — e.g. `chmod(path, mode)`.
    pub(crate) fn emit_stdlib_call_result_unit_ptr_i64_args(
        &mut self,
        symbol: &str,
        path: inkwell::values::BasicValueEnum<'ctx>,
        mode: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let i8_ty = self.context.i8_type();
        let result_ty = self
            .context
            .struct_type(&[i8_ty.into(), ptr_ty.into()], false);
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = result_ty.fn_type(&[ptr_ty.into(), i64_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(fn_val, &[path.into(), mode.into()], symbol)
            .ok()?;
        use inkwell::values::AnyValue;
        let c_val = inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        self.wrap_c_result_with_slot(c_val, result_ty)
    }

    /// `#536`: `(i64) → void / noreturn` — `exit(code)`.
    ///
    /// The call never returns; we follow it with `unreachable` to satisfy the LLVM
    /// verifier without emitting a second terminator.
    pub(crate) fn emit_stdlib_call_void_i64_arg(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let i64_ty = self.context.i64_type();
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = self.context.void_type().fn_type(&[i64_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        self.builder
            .build_call(fn_val, &[arg.into()], symbol)
            .ok()?;
        // exit() never returns — emit unreachable and mark block terminated.
        self.builder.build_unreachable().ok()?;
        self.terminated = true;
        None // caller sees None → no value produced, which is correct for Never
    }

    // ── #586: signal helpers ──────────────────────────────────────────────────

    /// `#586`: `(i8) → void` — signal_ignore(sig), signal_reset(sig).
    ///
    /// Signal is a unit enum encoded as `i8` at the C-ABI boundary.
    /// Returns `Some(i64 0)` as the Unit value.
    pub(crate) fn emit_stdlib_call_void_i8_arg(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let i8_ty = self.context.i8_type();
        let i64_ty = self.context.i64_type();
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = self.context.void_type().fn_type(&[i8_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        self.builder
            .build_call(fn_val, &[arg.into()], symbol)
            .ok()?;
        Some(i64_ty.const_zero().into())
    }

    /// `#586`: `(i8, ptr) → void` — signal_on(sig, handler).
    ///
    /// First arg is a Signal (i8), second is a function pointer cast to `*mut c_void`.
    /// Returns `Some(i64 0)` as the Unit value.
    pub(crate) fn emit_stdlib_call_void_i8_fn_ptr_arg(
        &mut self,
        symbol: &str,
        sig_arg: inkwell::values::BasicValueEnum<'ctx>,
        fn_ptr: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let i8_ty = self.context.i8_type();
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = self
                .context
                .void_type()
                .fn_type(&[i8_ty.into(), ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        self.builder
            .build_call(fn_val, &[sig_arg.into(), fn_ptr.into()], symbol)
            .ok()?;
        Some(i64_ty.const_zero().into())
    }

    /// Truncate an i64 IntValue to the target LLVM type if narrower (e.g. i64→i1 for Bool).
    /// Used by `extern "rust"` bridges where the C helper always returns i64.
    #[allow(dead_code)]
    fn trunc_int_to_ret(
        &self,
        raw: inkwell::values::IntValue<'ctx>,
        ret_llvm: Option<BasicTypeEnum<'ctx>>,
    ) -> BasicValueEnum<'ctx> {
        match ret_llvm {
            Some(BasicTypeEnum::IntType(it))
                if it.get_bit_width() < raw.get_type().get_bit_width() =>
            {
                self.builder
                    .build_int_truncate(raw, it, "trunc")
                    .unwrap()
                    .into()
            }
            _ => raw.into(),
        }
    }

    /// Emit `(ptr, i64) → {i8, ptr}` — used for `tcp_listen(host, port)`.
    ///
    /// The C function signature is `LlvmResult fn(*const MvlString, i64)`.
    /// Same wrapping as `emit_stdlib_call_result_one_ptr_arg`.
    pub(crate) fn emit_stdlib_call_result_ptr_i64_args(
        &mut self,
        symbol: &str,
        ptr_arg: inkwell::values::BasicValueEnum<'ctx>,
        i64_arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::types::BasicMetadataTypeEnum;
        use inkwell::values::BasicMetadataValueEnum;
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i64_ty = self.context.i64_type();
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = result_ty.fn_type(
                &[
                    BasicMetadataTypeEnum::from(ptr_ty),
                    BasicMetadataTypeEnum::from(i64_ty),
                ],
                false,
            );
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(
                fn_val,
                &[
                    BasicMetadataValueEnum::from(ptr_arg),
                    BasicMetadataValueEnum::from(i64_arg),
                ],
                "net_c_call",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        let c_val = inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        self.wrap_c_result_with_slot(c_val, result_ty)
    }

    /// Emit `(ptr) → {i8, ptr}` for `tcp_listener_port` — Result[Int, String].
    ///
    /// The C function encodes the i64 port as a raw `*mut c_void` integer cast.
    /// We ptrtoint the payload ptr to i64, store it in a stack slot, then return
    /// `{i8, ptr→i64}` so `bind_pattern_vars` can load the port with `build_load(i64, slot)`.
    pub(crate) fn emit_stdlib_call_result_i64_one_ptr_arg(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let i8_ty = self.context.i8_type();
        let i64_ty = self.context.i64_type();
        let c_result_ty = self
            .context
            .struct_type(&[i8_ty.into(), ptr_ty.into()], false);
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = c_result_ty.fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(fn_val, &[arg.into()], "net_port_call")
            .ok()?;
        use inkwell::values::AnyValue;
        let c_val = inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        let disc = self
            .builder
            .build_extract_value(c_val.into_struct_value(), 0, "port_disc")
            .ok()?;
        let payload_ptr = self
            .builder
            .build_extract_value(c_val.into_struct_value(), 1, "port_payload_ptr")
            .ok()?;
        // ptrtoint: convert the pointer-sized integer back to i64.
        let port_i64 = self
            .builder
            .build_ptr_to_int(payload_ptr.into_pointer_value(), i64_ty, "port_i64")
            .ok()?;
        // Store the i64 in a stack slot so bind_pattern_vars can load it.
        let slot = self.builder.build_alloca(i64_ty, "port_slot").unwrap();
        self.builder.build_store(slot, port_i64).unwrap();
        // Build {i8, ptr} where field 1 points to the i64 slot.
        let result_ty = self
            .context
            .struct_type(&[i8_ty.into(), ptr_ty.into()], false);
        let wrapped = self.builder.build_alloca(result_ty, "port_result").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, wrapped, 0, "port_disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc).unwrap();
        let payload_slot_ptr = self
            .builder
            .build_struct_gep(result_ty, wrapped, 1, "port_payload_slot_ptr")
            .unwrap();
        self.builder.build_store(payload_slot_ptr, slot).unwrap();
        Some(
            self.builder
                .build_load(result_ty, wrapped, "port_wrapped")
                .unwrap(),
        )
    }

    /// Emit `(ptr) → void` — used for `tcp_close_listener` / `tcp_close_stream`.
    pub(crate) fn emit_stdlib_call_void_one_ptr(
        &mut self,
        symbol: &str,
        arg: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let void_ty = self.context.void_type();
        let fn_val = self.module.get_function(symbol).unwrap_or_else(|| {
            let fn_ty = void_ty.fn_type(&[ptr_ty.into()], false);
            self.module
                .add_function(symbol, fn_ty, Some(Linkage::External))
        });
        self.builder
            .build_call(fn_val, &[arg.into()], "net_close_call")
            .ok()?;
        // Unit return — emit the unit value the LLVM backend convention expects.
        Some(self.context.i8_type().const_zero().into())
    }

    /// 3. Returns a new `{i8, ptr}` struct where field 1 = `slot`.
    ///
    /// This matches the internal format produced by `emit_result_variant` where
    /// field 1 is always a pointer TO the payload value, not the value itself.
    fn wrap_c_result_with_slot(
        &mut self,
        c_val: inkwell::values::BasicValueEnum<'ctx>,
        result_ty: inkwell::types::StructType<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        let BasicValueEnum::StructValue(sv) = c_val else {
            return None;
        };
        let disc = self.builder.build_extract_value(sv, 0, "c_disc").ok()?;
        let direct = self.builder.build_extract_value(sv, 1, "c_direct").ok()?;

        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        // Stack alloca to hold the direct payload — this becomes the pointer
        // that `emit_propagate`/`bind_pattern_vars` will dereference.
        let slot = self.builder.build_alloca(ptr_ty, "c_slot").unwrap();
        self.builder.build_store(slot, direct).unwrap();

        // Build wrapped {disc, slot} using GEP + store (mirrors emit_result_variant).
        let wrapped_alloca = self.builder.build_alloca(result_ty, "c_wrapped").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, wrapped_alloca, 0, "c_disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc).unwrap();
        let payload_ptr = self
            .builder
            .build_struct_gep(result_ty, wrapped_alloca, 1, "c_payload_ptr")
            .unwrap();
        self.builder.build_store(payload_ptr, slot).unwrap();
        Some(
            self.builder
                .build_load(result_ty, wrapped_alloca, "c_result")
                .unwrap(),
        )
    }

    /// Emit `s.parse_int()` → `Result[Int, String]` via `_mvl_str_parse_int`.
    ///
    /// The C function uses out-pointer parameters to avoid the sret calling
    /// convention on ARM64 (which `lli` mishandles for structs > 16 bytes):
    ///   `i8 _mvl_str_parse_int(ptr s, ptr ok_out, ptr err_out)`
    ///
    /// We pre-allocate i64_slot and ptr_slot, call the function, then build
    /// the LLVM `{i8, ptr}` Result pointing at the appropriate slot.
    pub(crate) fn emit_parse_int(
        &mut self,
        input_ptr: inkwell::values::PointerValue<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::IntPredicate;
        let i64_ty = self.context.i64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);

        // Pre-allocate output slots.
        let ok_slot = self.builder.build_alloca(i64_ty, "pi_ok_slot").unwrap();
        let err_slot = self.builder.build_alloca(ptr_ty, "pi_err_slot").unwrap();

        let parse_fn = self.get_mvl_str_parse_int();
        let call = self
            .builder
            .build_call(
                parse_fn,
                &[input_ptr.into(), ok_slot.into(), err_slot.into()],
                "pi_tag",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        let tag_val = inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        let inkwell::values::BasicValueEnum::IntValue(disc_i) = tag_val else {
            return None;
        };

        // select(disc == 0, ok_slot, err_slot) — both are ptr-typed allocas.
        let zero = self.context.i8_type().const_int(0, false);
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, disc_i, zero, "pi_is_ok")
            .unwrap();
        let payload_ptr = self
            .builder
            .build_select(is_ok, ok_slot, err_slot, "pi_payload")
            .unwrap()
            .into_pointer_value();

        // Build {i8, ptr} LLVM Result.
        let res_alloca = self.builder.build_alloca(result_ty, "pi_res").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, res_alloca, 0, "pi_disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_i).unwrap();
        let payload_field = self
            .builder
            .build_struct_gep(result_ty, res_alloca, 1, "pi_payload_field")
            .unwrap();
        self.builder
            .build_store(payload_field, payload_ptr)
            .unwrap();
        Some(
            self.builder
                .build_load(result_ty, res_alloca, "pi_result")
                .unwrap(),
        )
    }

    /// Emit `s.parse_float()` → `Result[Float, String]` via `_mvl_str_parse_float`.
    ///
    /// Mirrors `emit_parse_int` with f64 instead of i64.
    pub(crate) fn emit_parse_float(
        &mut self,
        input_ptr: inkwell::values::PointerValue<'ctx>,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        use inkwell::IntPredicate;
        let f64_ty = self.context.f64_type();
        let ptr_ty = self.context.ptr_type(inkwell::AddressSpace::default());
        let result_ty = self
            .context
            .struct_type(&[self.context.i8_type().into(), ptr_ty.into()], false);

        let ok_slot = self.builder.build_alloca(f64_ty, "pf_ok_slot").unwrap();
        let err_slot = self.builder.build_alloca(ptr_ty, "pf_err_slot").unwrap();

        let parse_fn = self.get_mvl_str_parse_float();
        let call = self
            .builder
            .build_call(
                parse_fn,
                &[input_ptr.into(), ok_slot.into(), err_slot.into()],
                "pf_tag",
            )
            .ok()?;
        use inkwell::values::AnyValue;
        let tag_val = inkwell::values::BasicValueEnum::try_from(call.as_any_value_enum()).ok()?;
        let inkwell::values::BasicValueEnum::IntValue(disc_i) = tag_val else {
            return None;
        };

        let zero = self.context.i8_type().const_int(0, false);
        let is_ok = self
            .builder
            .build_int_compare(IntPredicate::EQ, disc_i, zero, "pf_is_ok")
            .unwrap();
        let payload_ptr = self
            .builder
            .build_select(is_ok, ok_slot, err_slot, "pf_payload")
            .unwrap()
            .into_pointer_value();

        let res_alloca = self.builder.build_alloca(result_ty, "pf_res").unwrap();
        let disc_ptr = self
            .builder
            .build_struct_gep(result_ty, res_alloca, 0, "pf_disc_ptr")
            .unwrap();
        self.builder.build_store(disc_ptr, disc_i).unwrap();
        let payload_field = self
            .builder
            .build_struct_gep(result_ty, res_alloca, 1, "pf_payload_field")
            .unwrap();
        self.builder
            .build_store(payload_field, payload_ptr)
            .unwrap();
        Some(
            self.builder
                .build_load(result_ty, res_alloca, "pf_result")
                .unwrap(),
        )
    }

    /// Emit an LLVM IR function body for a single `extern "rust"` declaration.
    ///
    /// Known bridges delegate to C-ABI runtime functions (str_chars, str_concat,
    /// etc.).  Unrecognized functions get a stub that returns 0 / null.
    fn emit_extern_rust_fn_body(
        &mut self,
        efn: &ExternFnDecl,
        i64_ty: inkwell::types::IntType<'ctx>,
        ptr_ty: inkwell::types::PointerType<'ctx>,
    ) {
        self.emit_extern_rust_fn_body_named(efn, i64_ty, ptr_ty, &efn.name.clone());
    }

    /// Like `emit_extern_rust_fn_body` but attaches the body to `llvm_name` in the module
    /// instead of `efn.name`. Used by the fourth pass for builtin extension methods whose
    /// LLVM name is mangled (e.g. `String_chars`) but whose bridge match key is the old
    /// prefixed name (e.g. `str_chars`). (#928)
    #[allow(clippy::too_many_lines)]
    fn emit_extern_rust_fn_body_named(
        &mut self,
        efn: &ExternFnDecl,
        i64_ty: inkwell::types::IntType<'ctx>,
        _ptr_ty: inkwell::types::PointerType<'ctx>,
        llvm_name: &str,
    ) {
        let ret_llvm = self.mvl_type_to_llvm(&efn.return_type);
        let param_tys: Vec<BasicMetadataTypeEnum> = efn
            .params
            .iter()
            .filter_map(|p| self.mvl_type_to_llvm(&p.ty))
            .map(Into::into)
            .collect();

        // Reuse existing declaration (pre-declared in first pass) or create new.
        let fn_val = self.module.get_function(llvm_name).unwrap_or_else(|| {
            let fn_ty = match ret_llvm {
                Some(rt) => rt.fn_type(&param_tys, false),
                None => self.context.void_type().fn_type(&param_tys, false),
            };
            self.module.add_function(llvm_name, fn_ty, None)
        });
        let entry = self.context.append_basic_block(fn_val, "entry");
        self.builder.position_at_end(entry);

        match efn.name.as_str() {
            // str_chars(s: String) -> List[String]: delegate to mvl_string_chars
            "str_chars" => {
                let arg0 = fn_val.get_first_param().expect("str_chars: missing arg");
                let chars_fn = self.get_mvl_string_chars();
                let result = self
                    .builder
                    .build_call(chars_fn, &[arg0.into()], "chars")
                    .unwrap();
                use inkwell::values::AnyValue;
                let arr_ptr = BasicValueEnum::try_from(result.as_any_value_enum())
                    .expect("mvl_string_chars must return ptr");
                self.builder.build_return(Some(&arr_ptr)).unwrap();
            }
            // str_concat(a: String, b: String) -> String: delegate to mvl_string_concat
            "str_concat" => {
                let params: Vec<_> = fn_val.get_param_iter().collect();
                let a = params[0];
                let b = params[1];
                let concat_fn = self.get_mvl_string_concat();
                let result = self
                    .builder
                    .build_call(concat_fn, &[a.into(), b.into()], "concat")
                    .unwrap();
                use inkwell::values::AnyValue;
                let s_ptr = BasicValueEnum::try_from(result.as_any_value_enum())
                    .expect("mvl_string_concat must return ptr");
                self.builder.build_return(Some(&s_ptr)).unwrap();
            }
            // str_len(s: String) -> Int: delegate to _mvl_str_len
            "str_len" => {
                self.emit_builtin_bridge_1_to_1(fn_val, "_mvl_str_len", i64_ty.into());
            }
            // str_char_at(s: String, i: Int) -> String: delegate to _mvl_str_char_at
            "str_char_at" => {
                self.emit_builtin_bridge_2_to_ptr(fn_val, "_mvl_str_char_at");
            }
            // str_substring(s: String, start: Int, end: Int) -> String
            "str_substring" => {
                self.emit_builtin_bridge_3_to_ptr(fn_val, "_mvl_str_substring");
            }
            // str_replace(s: String, from: String, to: String) -> String
            "str_replace" => {
                self.emit_builtin_bridge_3_to_ptr(fn_val, "_mvl_str_replace");
            }
            // str_split(s: String, sep: String) -> List[String]
            "str_split" => {
                self.emit_builtin_bridge_2_to_ptr(fn_val, "_mvl_str_split");
            }
            // String → String (1-arg): trim, to_lower, to_upper, from_chars, from_bytes
            "str_trim" | "str_to_lower" | "str_to_upper" | "str_from_chars" | "str_from_bytes" => {
                self.emit_builtin_bridge_1_to_1(
                    fn_val,
                    &format!("_mvl_{}", efn.name),
                    self.context.ptr_type(AddressSpace::default()).into(),
                );
            }
            // list_len(list: List[T]) -> Int: delegate to mvl_array_len
            "list_len" => {
                let arg0 = fn_val.get_first_param().expect("list_len: missing arg");
                let len_fn = self.get_mvl_array_len();
                let result = self
                    .builder
                    .build_call(len_fn, &[arg0.into()], "list_len")
                    .unwrap();
                use inkwell::values::AnyValue;
                let len_val = BasicValueEnum::try_from(result.as_any_value_enum())
                    .expect("mvl_array_len must return i64");
                self.builder.build_return(Some(&len_val)).unwrap();
            }
            // list_get(list: List[T], idx: Int) -> Option[T]: delegate to mvl_array_get
            "list_get" => {
                let params: Vec<_> = fn_val.get_param_iter().collect();
                let arr = params[0];
                let idx = params[1];
                let get_fn = self.get_mvl_array_get();
                let raw = self
                    .builder
                    .build_call(get_fn, &[arr.into(), idx.into()], "raw")
                    .unwrap();
                use inkwell::values::AnyValue;
                let raw_ptr = BasicValueEnum::try_from(raw.as_any_value_enum())
                    .expect("mvl_array_get must return ptr")
                    .into_pointer_value();
                // Build Option{i8, ptr}: disc=0 (Some) with the slot pointer, or disc=1 (None)
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let i8_ty = self.context.i8_type();
                let opt_ty = self
                    .context
                    .struct_type(&[i8_ty.into(), ptr_ty.into()], false);
                let null = ptr_ty.const_null();
                let is_null = self.builder.build_is_null(raw_ptr, "is_null").unwrap();
                let some_val = opt_ty.const_zero();
                let some_val = self
                    .builder
                    .build_insert_value(some_val, i8_ty.const_int(0, false), 0, "some_disc")
                    .unwrap()
                    .into_struct_value();
                let some_val = self
                    .builder
                    .build_insert_value(some_val, raw_ptr, 1, "some_ptr")
                    .unwrap()
                    .into_struct_value();
                let none_val = opt_ty.const_zero();
                let none_val = self
                    .builder
                    .build_insert_value(none_val, i8_ty.const_int(1, false), 0, "none_disc")
                    .unwrap()
                    .into_struct_value();
                let none_val = self
                    .builder
                    .build_insert_value(none_val, null, 1, "none_ptr")
                    .unwrap()
                    .into_struct_value();
                let result = self
                    .builder
                    .build_select(is_null, none_val, some_val, "opt")
                    .unwrap();
                self.builder.build_return(Some(&result)).unwrap();
            }
            // Generic stub: return zero / null / None-struct.
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
                Some(BasicTypeEnum::StructType(st)) => {
                    // Return a zeroed struct (acts as None / default for Option/Result).
                    let zero = st.const_zero();
                    // Set discriminant byte 0 to 1 (None/Err convention).
                    let none_like: BasicValueEnum = if st.count_fields() > 0 {
                        self.builder
                            .build_insert_value(
                                zero,
                                self.context.i8_type().const_int(1, false),
                                0,
                                "none_disc",
                            )
                            .map(|v| v.into_struct_value().into())
                            .unwrap_or_else(|_| zero.into())
                    } else {
                        zero.into()
                    };
                    self.builder.build_return(Some(&none_like)).unwrap();
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
        // Type-attached methods use a mangled LLVM name: `TypeName_method` (#868).
        let llvm_name: String = if let Some(recv_ty) = &fd.receiver_type {
            format!("{}_{}", recv_ty, fd.name)
        } else {
            fd.name.clone()
        };
        let fn_val = match self.module.get_function(&llvm_name) {
            Some(f) => f,
            None => {
                let (fn_ty, _) = self.build_fn_type(fd);
                self.module.add_function(&llvm_name, fn_ty, None)
            }
        };
        let is_c_main = fd.name == "main";

        // If the function already has a body (duplicate name from an earlier prelude
        // module), delete it so the later declaration wins.  This handles name collisions
        // between implicit-prelude functions and RUST_BACKED_STDLIB functions (e.g.
        // strings.replace vs regex.replace).
        if fn_val.count_basic_blocks() > 0 {
            while let Some(bb) = fn_val.get_last_basic_block() {
                unsafe { bb.delete().unwrap() };
            }
        }

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
                // val/ref params arrive as ptr. Dereference immediately into a struct-typed
                // alloca so field access in the body works without special-casing.
                if let TypeExpr::Ref { inner, .. } = &param.ty {
                    if let Some(inner_ty) = self.mvl_type_to_llvm(inner) {
                        let loaded = self
                            .builder
                            .build_load(inner_ty, param_val.into_pointer_value(), &param.name)
                            .unwrap();
                        let alloca = self.builder.build_alloca(inner_ty, &param.name).unwrap();
                        self.builder.build_store(alloca, loaded).unwrap();
                        self.locals.insert(param.name.clone(), (alloca, inner_ty));
                        self.local_mvl_types
                            .insert(param.name.clone(), *inner.clone());
                        continue;
                    }
                }
                if let Some(ty) = self.mvl_type_to_llvm(&param.ty) {
                    let alloca = self.builder.build_alloca(ty, &param.name).unwrap();
                    self.builder.build_store(alloca, param_val).unwrap();
                    self.locals.insert(param.name.clone(), (alloca, ty));
                }
                // L5-08: record MVL type for Ok/Some payload inference in match arms.
                self.local_mvl_types
                    .insert(param.name.clone(), param.ty.clone());
            }
        }

        // Req 10 / Phase 4 (#627): emit runtime requires-clause guards.
        // Mirrors the Rust backend's `assert!(pred, "requires: ...")`.
        if !fd.requires.is_empty() {
            // Build a name → loaded-i64 map for all Int parameters.
            let mut param_vals: HashMap<String, inkwell::values::IntValue> = HashMap::new();
            for (i, param) in fd.params.iter().enumerate() {
                if let Some(inkwell::values::BasicValueEnum::IntValue(iv)) =
                    fn_val.get_nth_param(i as u32)
                {
                    param_vals.insert(param.name.clone(), iv);
                    // Also bind "self" → single-param shortcut used in normalised preds.
                    if fd.params.len() == 1 {
                        param_vals.insert("self".to_string(), iv);
                    }
                }
            }
            for req_pred in &fd.requires {
                if let Some(cond) = self.emit_requires_pred_bool(req_pred, &param_vals) {
                    match self.assert_mode {
                        // Note: LLVM IR has no concept of `debug_assertions`, so
                        // DebugOnly emits the same unconditional trap as Always.
                        // TODO(#627): Distinguish at link time via a separate IR
                        // module compiled under a debug-flavoured target triple.
                        crate::mvl::backends::AssertMode::Always
                        | crate::mvl::backends::AssertMode::DebugOnly => {
                            let cur_block = self.builder.get_insert_block().unwrap();
                            let cur_fn = cur_block.get_parent().unwrap();
                            let trap_bb = self.context.append_basic_block(cur_fn, "req_fail");
                            let ok_bb = self.context.append_basic_block(cur_fn, "req_ok");
                            self.builder
                                .build_conditional_branch(cond, ok_bb, trap_bb)
                                .unwrap();
                            self.builder.position_at_end(trap_bb);
                            let trap_ty = self.context.void_type().fn_type(&[], false);
                            let trap_fn =
                                self.module.get_function("llvm.trap").unwrap_or_else(|| {
                                    self.module.add_function("llvm.trap", trap_ty, None)
                                });
                            self.builder.build_call(trap_fn, &[], "trap").unwrap();
                            self.builder.build_unreachable().unwrap();
                            self.builder.position_at_end(ok_bb);
                        }
                        crate::mvl::backends::AssertMode::Assume => {
                            let assume_ty = self
                                .context
                                .void_type()
                                .fn_type(&[self.context.bool_type().into()], false);
                            let assume_fn =
                                self.module.get_function("llvm.assume").unwrap_or_else(|| {
                                    self.module.add_function("llvm.assume", assume_ty, None)
                                });
                            self.builder
                                .build_call(assume_fn, &[cond.into()], "req_assume")
                                .unwrap();
                        }
                    }
                }
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
                // Fallback: body emitted no value. Return a zeroed value of the correct
                // type (or unreachable) so the IR is well-formed even for unsupported constructs.
                let fallback = self.mvl_type_to_llvm(&fd.return_type);
                match fallback {
                    Some(BasicTypeEnum::IntType(it)) => {
                        self.builder.build_return(Some(&it.const_zero())).unwrap();
                    }
                    Some(BasicTypeEnum::FloatType(ft)) => {
                        self.builder.build_return(Some(&ft.const_zero())).unwrap();
                    }
                    Some(BasicTypeEnum::PointerType(pt)) => {
                        self.builder.build_return(Some(&pt.const_null())).unwrap();
                    }
                    Some(BasicTypeEnum::StructType(st)) => {
                        self.builder.build_return(Some(&st.const_zero())).unwrap();
                    }
                    _ => {
                        self.builder.build_unreachable().unwrap();
                    }
                }
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
                // Fallback: body emitted no value (e.g. unsupported method call in body).
                // Emit a zeroed return to keep IR well-formed.
                let fallback = self.mvl_type_to_llvm(&fd.return_type);
                match fallback {
                    Some(BasicTypeEnum::IntType(it)) => {
                        self.builder.build_return(Some(&it.const_zero())).unwrap();
                    }
                    Some(BasicTypeEnum::FloatType(ft)) => {
                        self.builder.build_return(Some(&ft.const_zero())).unwrap();
                    }
                    Some(BasicTypeEnum::PointerType(pt)) => {
                        self.builder.build_return(Some(&pt.const_null())).unwrap();
                    }
                    Some(BasicTypeEnum::StructType(st)) => {
                        self.builder.build_return(Some(&st.const_zero())).unwrap();
                    }
                    _ => {
                        self.builder.build_unreachable().unwrap();
                    }
                }
            }
        }
    }

    /// Return the name of the last expression in `block` if it is a bare identifier
    /// tracked as a heap local.  Used to exclude the implicit return value from drops
    /// (returning a heap pointer transfers ownership to the caller).
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

    /// Enhance type substitutions by resolving type params inside compound types
    /// (e.g. `Map[K, V]`) using MVL-level type information from call-site arguments.
    ///
    /// `infer_type_subs` only handles bare type params like `key: K`.  For params
    /// like `m: Map[K, V]`, K and V cannot be inferred from the LLVM type (opaque
    /// ptr).  This method matches each formal param's type args against the actual
    /// MVL type of the argument expression to extract the missing substitutions.
    pub(crate) fn infer_type_subs_from_args(
        &self,
        fd: &FnDecl,
        call_args: &[crate::mvl::parser::ast::Expr],
        subs: &mut HashMap<String, BasicTypeEnum<'ctx>>,
    ) {
        for (param, arg_expr) in fd.params.iter().zip(call_args.iter()) {
            if let TypeExpr::Base {
                args: formal_args, ..
            } = &param.ty
            {
                if formal_args.is_empty() {
                    continue;
                }
                // Look up the actual MVL type of the argument expression.
                let actual_ty = match arg_expr {
                    crate::mvl::parser::ast::Expr::Ident(name, _) => {
                        self.local_mvl_types.get(name.as_str())
                    }
                    _ => None,
                };
                let actual_ty = match actual_ty {
                    Some(t) => Self::strip_type_wrappers(t),
                    None => continue,
                };
                if let TypeExpr::Base {
                    args: actual_args, ..
                } = actual_ty
                {
                    for (formal_tp, actual_tp) in formal_args.iter().zip(actual_args.iter()) {
                        if let TypeExpr::Base {
                            name: tp_name,
                            args: tp_args,
                            ..
                        } = formal_tp
                        {
                            if tp_args.is_empty()
                                && fd
                                    .type_params
                                    .iter()
                                    .any(|tp| tp.name() == tp_name.as_str())
                                && !subs.contains_key(tp_name.as_str())
                            {
                                if let Some(llvm_ty) = self.mvl_type_to_llvm(actual_tp) {
                                    subs.insert(tp_name.clone(), llvm_ty);
                                }
                            }
                        }
                    }
                }
            }
        }
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

    // ── Builtin bridge helpers ─────────────────────────────────────────────

    /// Bridge a 1-arg builtin to a C-ABI function with the same signature shape.
    fn emit_builtin_bridge_1_to_1(
        &mut self,
        fn_val: FunctionValue<'ctx>,
        c_name: &str,
        ret_ty: BasicTypeEnum<'ctx>,
    ) {
        use inkwell::values::AnyValue;
        let arg0 = fn_val.get_first_param().expect("bridge_1: missing arg");
        let c_fn = self.module.get_function(c_name).unwrap_or_else(|| {
            let ft = ret_ty.fn_type(&[arg0.get_type().into()], false);
            self.module
                .add_function(c_name, ft, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(c_fn, &[arg0.into()], "bridge")
            .unwrap();
        let rv = BasicValueEnum::try_from(call.as_any_value_enum()).expect("bridge_1 return");
        self.builder.build_return(Some(&rv)).unwrap();
    }

    /// Bridge a 2-arg builtin to a C-ABI function returning ptr.
    fn emit_builtin_bridge_2_to_ptr(&mut self, fn_val: FunctionValue<'ctx>, c_name: &str) {
        use inkwell::values::AnyValue;
        let params: Vec<_> = fn_val.get_param_iter().collect();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let c_fn = self.module.get_function(c_name).unwrap_or_else(|| {
            let ft = ptr_ty.fn_type(
                &[params[0].get_type().into(), params[1].get_type().into()],
                false,
            );
            self.module
                .add_function(c_name, ft, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(c_fn, &[params[0].into(), params[1].into()], "bridge")
            .unwrap();
        let rv = BasicValueEnum::try_from(call.as_any_value_enum()).expect("bridge_2p return");
        self.builder.build_return(Some(&rv)).unwrap();
    }

    /// Bridge a 3-arg builtin to a C-ABI function returning ptr.
    fn emit_builtin_bridge_3_to_ptr(&mut self, fn_val: FunctionValue<'ctx>, c_name: &str) {
        use inkwell::values::AnyValue;
        let params: Vec<_> = fn_val.get_param_iter().collect();
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let c_fn = self.module.get_function(c_name).unwrap_or_else(|| {
            let ft = ptr_ty.fn_type(
                &[
                    params[0].get_type().into(),
                    params[1].get_type().into(),
                    params[2].get_type().into(),
                ],
                false,
            );
            self.module
                .add_function(c_name, ft, Some(Linkage::External))
        });
        let call = self
            .builder
            .build_call(
                c_fn,
                &[params[0].into(), params[1].into(), params[2].into()],
                "bridge",
            )
            .unwrap();
        let rv = BasicValueEnum::try_from(call.as_any_value_enum()).expect("bridge_3p return");
        self.builder.build_return(Some(&rv)).unwrap();
    }

    // ── Verification and IR output ───────────────────────────────────────────

    fn verify(&self) -> Result<(), String> {
        self.module.verify().map_err(|e| e.to_string())
    }

    fn to_ir_string(&self) -> String {
        self.module.print_to_string().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::find_cdylib;
    use std::io::Write;

    /// Extension validation: a path with a wrong extension must be rejected even
    /// if the file exists on disk.
    #[test]
    fn find_cdylib_rejects_bad_extension() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // Rename via a path with .txt extension
        let bad_path = tmp.path().with_extension("txt");
        std::fs::copy(tmp.path(), &bad_path).unwrap();

        let var = "MVL_TEST_BAD_EXT_LIB";
        std::env::set_var(var, &bad_path);
        let result = find_cdylib(var, "dummy");
        std::env::remove_var(var);
        std::fs::remove_file(&bad_path).ok();

        assert!(result.is_none(), "bad extension must be rejected");
    }

    /// Extension validation: a path with a `.so` extension is accepted when the
    /// file exists.
    #[test]
    fn find_cdylib_accepts_so_extension() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("libfake.so");
        std::fs::File::create(&lib).unwrap().write_all(&[]).unwrap();

        let var = "MVL_TEST_SO_LIB";
        std::env::set_var(var, &lib);
        let result = find_cdylib(var, "dummy");
        std::env::remove_var(var);

        assert_eq!(result, Some(lib));
    }

    /// Extension validation: a path with a `.dylib` extension is accepted when
    /// the file exists.
    #[test]
    fn find_cdylib_accepts_dylib_extension() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("libfake.dylib");
        std::fs::File::create(&lib).unwrap().write_all(&[]).unwrap();

        let var = "MVL_TEST_DYLIB_LIB";
        std::env::set_var(var, &lib);
        let result = find_cdylib(var, "dummy");
        std::env::remove_var(var);

        assert_eq!(result, Some(lib));
    }

    /// When the env var is absent the function falls through to the filesystem
    /// search (which returns None in a test context since the binary dir won't
    /// have the lib). Crucially it must not panic.
    #[test]
    fn find_cdylib_no_env_var_returns_none_or_path() {
        let var = "MVL_TEST_ABSENT_LIB_XYZ";
        std::env::remove_var(var);
        // Just must not panic; result is environment-dependent.
        let _ = find_cdylib(var, "libnonexistent_xyz");
    }
}

// ── Backend trait implementation ─────────────────────────────────────────────

/// Unit struct implementing the [`Backend`] trait for the LLVM backend.
pub struct LlvmBackendImpl;

impl crate::mvl::backends::Backend for LlvmBackendImpl {
    fn name(&self) -> &'static str {
        "llvm"
    }

    fn file_extension(&self) -> &'static str {
        "ll"
    }

    fn emit_program(&self, prog: &crate::mvl::parser::ast::Program, module_name: &str) -> String {
        let compiler = LlvmCompiler::new();
        compiler
            .compile_to_ir(prog, module_name)
            .unwrap_or_default()
    }
}
