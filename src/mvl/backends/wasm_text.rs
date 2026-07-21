// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `WasmTextCompiler` — TIR → WebAssembly Text emitter (#1818, epic #1817).
//!
//! Runs against `tests/corpus/` via `make test-rust-wasm` (delegated to
//! mvlr). Scope: everything that can be lowered without a `runtime/wasm/`
//! crate. Phase 2 of #1817 stands that up and unlocks strings, collections,
//! and tagged-union payloads.
//!
//! Supported today:
//! - Primitives: `Int → i64`, `Float → f64`, `Bool` / `Byte → i32`,
//!   unit-variant enum types → `i32` discriminant
//! - All literal kinds (`Integer`, `Float`, `Bool`, `Str`)
//! - Arithmetic, comparison, bitwise, and short-circuit boolean ops
//! - Unary `Neg`, `Not`, `BitNot`
//! - `Int.to_string()` (inline bump-allocated i64 → decimal helper)
//! - `Bool.to_string()` (branch between interned `"true"` / `"false"`)
//! - String literals — interned up front, emitted as `(data …)` sections
//! - `println(s)` / `eprintln(s)` — WASI `fd_write` fd 1 / fd 2 + newline
//! - `assert(cond)` / `assert_eq[T](a, b)` / `assert_ne[T](a, b)` — trap
//!   via `unreachable` on failure. Type-directed equality.
//! - `let` and `let ref` bindings — WASM locals, declared in a fn prelude
//!   from a pre-scan of the body
//! - `x = value;` assignment for `ref` locals — `local.set`
//! - `if` / `else if` / `else` — statement + expression form; statement
//!   form auto-detects a matching non-Unit return type from both branches
//! - `while cond { body }` — canonical WASM `block/loop/br_if` shape
//! - Early `return` (both `return expr` and bare `return`)
//! - `match` on Int / Bool / unit-variant enum patterns with wildcard —
//!   both statement and expression form
//! - Unit-variant enums (`type Direction = enum { North, South, ... }`) —
//!   variants lower to i32 discriminants, referenced by qualified name
//! - `fn main() -> Unit ! Console` → WASI `_start` export
//! - Bodies containing unsupported constructs stub to `unreachable` so
//!   sibling fns in the same file can still assemble and run
//!
//! Deliberately not supported (later phases of #1817):
//! - Structs and their fields — needs linear-memory layout
//! - Enum variants with payloads, `Option`, `Result` — tagged unions +
//!   memory layout, plus the `?` operator
//! - Collections (`List`, `Map`, `Set`) — phase 3 with `runtime/wasm/`
//! - Higher-order fns / closures / generic monomorphization
//! - String equality / concat / `MvlString` refcount — phase 2 runtime
//! - Other WASI hostcalls, `extern "wasm"` ABI — separate ticket
//! - Actors — phase 6+

use std::cell::Cell;
use std::collections::HashMap;

use super::{AssertMode, Backend};
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{
    ArithOp, BinaryOp, CmpOp, LValue, Literal, LogicOp, Pattern, RefExpr, TirBlock, TirElseBranch,
    TirExpr, TirExprKind, TirFn, TirMatchArm, TirMatchBody, TirParam, TirProgram, TirStmt,
    TirTypeBody, TirTypeDecl, TirVariantFields, UnaryOp,
};

pub struct WasmTextCompiler {
    pub assert_mode: AssertMode,
}

impl WasmTextCompiler {
    pub fn new() -> Self {
        Self {
            assert_mode: AssertMode::Always,
        }
    }
}

impl Default for WasmTextCompiler {
    fn default() -> Self {
        Self::new()
    }
}

/// One field slot in a heap-allocated struct: name, byte offset in the
/// allocated block, and resolved MVL type (used to choose the WASM load/store
/// opcode and to unpack `*MvlString` fields into `(ptr, len)` on reads).
#[derive(Debug, Clone)]
struct FieldSlot {
    name: String,
    offset: u32,
    ty: Ty,
}

/// Pre-computed memory layout for a single struct type.
#[derive(Debug, Clone)]
struct StructLayout {
    total_size: u32,
    fields: Vec<FieldSlot>,
}

/// One variant within a payload-carrying enum.
#[derive(Debug, Clone)]
struct PayloadVariant {
    name: String,
    disc: i32,
    /// Payload field types in declaration order. Empty = unit variant.
    fields: Vec<Ty>,
    /// Byte size of the payload region (sum of field sizes, 8-byte granules).
    payload_size: u32,
}

/// Pre-computed info for an enum that has at least one non-Unit variant.
/// The enum value on the WASM stack is an `i32` pointer to
/// `{ disc: i32, payload_ptr: i32 }` (8 bytes) in the bump-allocated heap.
#[derive(Debug, Clone)]
struct PayloadEnumInfo {
    variants: Vec<PayloadVariant>,
}

/// Shared per-emission context. Bundles the flags/tables threaded through
/// every emit_*  free function so their signatures stay stable as the
/// spike grows (or shrinks). Uses `Cell` for the label counter so the
/// context stays behind a `&`-reference — labels are module-wide unique,
/// which is stricter than WASM requires but simpler to bookkeep.
struct Ctx<'a> {
    needs_wasi: bool,
    /// Interned string literals: content → (linear-memory offset, byte length).
    literals: &'a HashMap<String, (u32, u32)>,
    /// Enum types whose variants are all `Unit` — lower to `i32` discriminant.
    /// Enums with tuple/struct payloads are excluded here (they use the
    /// tagged-union heap layout in `payload_enums`).
    enum_types: &'a std::collections::HashSet<String>,
    /// Qualified unit-variant name (e.g. `"Direction::North"`) → i32 discriminant.
    /// Used both when a variant appears as a `Var` value and as a match
    /// pattern (`Pattern::Ident`).
    enum_variants: &'a HashMap<String, i32>,
    /// Heap-layout info for struct types (#1821). Key = struct name.
    struct_layouts: &'a HashMap<String, StructLayout>,
    /// Heap-layout info for payload-carrying enums (#1821). Key = enum type name.
    payload_enums: &'a HashMap<String, PayloadEnumInfo>,
    /// Monotonic counter for fresh WAT labels (`$while_0`, `$while_1`, …).
    label_counter: Cell<usize>,
    /// Set by emitters that reach for `runtime/wasm/` symbols (#1819).
    /// When true, `emit_program` swaps its own `(memory 1)` for
    /// `(import "runtime" "memory" (memory 0))` and appends the needed
    /// `(import "runtime" "_mvl_*" ...)` declarations.
    needs_runtime: Cell<bool>,
    /// Names of the current function's `String`-typed parameters. These are
    /// split into two WASM params (`$name_ptr i32, $name_len i32`) in the
    /// function signature and must be read back as two local.gets. Updated
    /// at the start of each function in `emit_fn`. String locals that are
    /// NOT in this set (e.g. match-arm bindings) emit `;; unsupported`.
    string_params: std::cell::RefCell<std::collections::HashSet<String>>,
    assert_mode: AssertMode,
}

impl Ctx<'_> {
    fn fresh_label(&self, prefix: &str) -> String {
        let n = self.label_counter.get();
        self.label_counter.set(n + 1);
        format!("{prefix}_{n}")
    }
}

/// First offset available for string-literal data after the fixed WASI
/// scratch region (iovec pair + nwritten slot + newline byte).
const LITERAL_BASE: u32 = 32;

/// Runtime symbols the emitter can dispatch to. Every entry produces one
/// `(import "runtime" ...)` declaration when `Ctx::needs_runtime` is set.
/// Symbol names match `runtime/wasm/src/lib.rs`; signatures are WAT
/// param/result clauses.
///
/// Not all imports are used by every module — WASM is fine with unused
/// imports, so listing them all up front is simpler than tracking which
/// symbols were touched during emission.
const RUNTIME_IMPORTS: &[(&str, &str)] = &[
    ("_mvl_string_eq", "(param i32 i32 i32 i32) (result i32)"),
    ("_mvl_string_len", "(param i32 i32) (result i64)"),
    ("_mvl_string_is_empty", "(param i32 i32) (result i32)"),
    (
        "_mvl_string_contains",
        "(param i32 i32 i32 i32) (result i32)",
    ),
    (
        "_mvl_string_starts_with",
        "(param i32 i32 i32 i32) (result i32)",
    ),
    (
        "_mvl_string_ends_with",
        "(param i32 i32 i32 i32) (result i32)",
    ),
    ("_mvl_string_find", "(param i32 i32 i32 i32) (result i64)"),
    // Group B — allocation, returns `*MvlString` (pointer as i32). The
    // emitter unpacks `.ptr` / `.len` via `i32.load` at offsets 0 / 4 so
    // downstream code keeps the same `(ptr, len)` stack shape as literals.
    ("_mvl_string_new", "(param i32 i32) (result i32)"),
    ("_mvl_string_clone", "(param i32) (result i32)"),
    ("_mvl_string_drop", "(param i32)"),
    ("_mvl_string_concat", "(param i32 i32 i32 i32) (result i32)"),
    // `.substring(start, end)` — MVL `Int` args are i64 on the WASM side.
    (
        "_mvl_string_substring",
        "(param i32 i32 i64 i64) (result i32)",
    ),
    // Group B commit 3 — case fold + trim. Unary transforms: receiver
    // (ptr, len) → `*MvlString`. Same unpack shape as concat/substring.
    ("_mvl_string_to_upper", "(param i32 i32) (result i32)"),
    ("_mvl_string_to_lower", "(param i32 i32) (result i32)"),
    ("_mvl_string_trim", "(param i32 i32) (result i32)"),
    // `.replace(from, to)` — three (ptr, len) pairs in, `*MvlString` out.
    (
        "_mvl_string_replace",
        "(param i32 i32 i32 i32 i32 i32) (result i32)",
    ),
    // Group C — MvlArray (List[T] / Array[T, N] / Set[T] backing storage,
    // #1820). Pointer-typed as i32; elements accessed by byte offset with
    // `i32.load` / `i64.load` / `f64.load` on the pointer returned by
    // `_mvl_array_get`. Typed push variants exist so the emitter can pass
    // the value directly on the WASM stack (no scratch alloc needed).
    ("_mvl_array_new", "(param i32 i32) (result i32)"),
    ("_mvl_array_len", "(param i32) (result i64)"),
    ("_mvl_array_is_empty", "(param i32) (result i32)"),
    ("_mvl_array_push", "(param i32 i32)"),
    ("_mvl_array_push_i32", "(param i32 i32)"),
    ("_mvl_array_push_i64", "(param i32 i64)"),
    ("_mvl_array_push_f64", "(param i32 f64)"),
    ("_mvl_array_get", "(param i32 i64) (result i32)"),
    ("_mvl_array_clone", "(param i32) (result i32)"),
    ("_mvl_array_drop", "(param i32)"),
    // Group D — MvlOption (#1821 partial, Phase 4 prelude). Heap-allocated
    // `Option[T]`; emitter treats the pointer as opaque i32 and calls the
    // typed accessors below. Corpus scope: `Option[Int]` (i64 payload) and
    // `Option[Bool]` / enum discriminants (i32 payload).
    ("_mvl_option_some_i64", "(param i64) (result i32)"),
    ("_mvl_option_some_i32", "(param i32) (result i32)"),
    ("_mvl_option_none", "(result i32)"),
    ("_mvl_option_tag", "(param i32) (result i32)"),
    ("_mvl_option_value_i64", "(param i32) (result i64)"),
    ("_mvl_option_value_i32", "(param i32) (result i32)"),
    ("_mvl_option_drop", "(param i32)"),
    // `xs.get(i)` on `List[T]` — dispatches to one of these based on T.
    // Returns *MvlOption (Some(value) in bounds, None otherwise).
    ("_mvl_array_get_option_i64", "(param i32 i64) (result i32)"),
    ("_mvl_array_get_option_i32", "(param i32 i64) (result i32)"),
    // Group G — struct heap allocation (#1821). `_mvl_struct_alloc(size)`
    // bump-allocates `size` bytes and returns the pointer as i32. Used for
    // both struct construction and payload-enum header + payload blocks.
    ("_mvl_struct_alloc", "(param i32) (result i32)"),
    // Group E — Set ops (#1820). Sort+dedup at construction; linear-scan
    // contains / insert for `Set[T].contains` / `Set[T].insert`.
    ("_mvl_array_dedup_i64", "(param i32)"),
    ("_mvl_array_dedup_i32", "(param i32)"),
    ("_mvl_array_contains_i64", "(param i32 i64) (result i32)"),
    ("_mvl_array_contains_i32", "(param i32 i32) (result i32)"),
    ("_mvl_array_insert_i64", "(param i32 i64)"),
    ("_mvl_array_insert_i32", "(param i32 i32)"),
    // Group F — Map[String, Int] ops (#1820). Linear-scan map backed by
    // `MvlMap` on the Rust heap. `si64` suffix = String key, i64 value.
    ("_mvl_map_new_si64", "(result i32)"),
    ("_mvl_map_len", "(param i32) (result i64)"),
    ("_mvl_map_insert_si64", "(param i32 i32 i32 i64)"),
    ("_mvl_map_get_si64", "(param i32 i32 i32) (result i32)"),
    (
        "_mvl_map_contains_key_si64",
        "(param i32 i32 i32) (result i32)",
    ),
    ("_mvl_map_drop_si64", "(param i32)"),
    // Group G — Result ops (#1821 extension). i32 pointer to heap-allocated
    // MvlResult. Ok = tag 0, Err = tag 1. Corpus scope: Result[Int, String].
    ("_mvl_result_ok_i64", "(param i64) (result i32)"),
    ("_mvl_result_ok_i32", "(param i32) (result i32)"),
    ("_mvl_result_err_str", "(param i32 i32) (result i32)"),
    ("_mvl_result_tag", "(param i32) (result i32)"),
    ("_mvl_result_value_i64", "(param i32) (result i64)"),
    ("_mvl_result_value_i32", "(param i32) (result i32)"),
    ("_mvl_result_drop", "(param i32)"),
    // Group H — String parse ops. Take raw (ptr, len) byte slice; return
    // heap-allocated MvlResult pointer.
    ("_mvl_string_parse_int", "(param i32 i32) (result i32)"),
];

/// Layout offsets on `MvlString` — mirrors `runtime/wasm/src/lib.rs` /
/// `runtime/llvm/src/memory.rs`. Only `.ptr` and `.len` are read by the
/// emitter today; `.cap` and `.rc` land when drop / clone wire up.
const MVL_STRING_OFFSET_PTR: u32 = 0;
const MVL_STRING_OFFSET_LEN: u32 = 4;

impl Backend for WasmTextCompiler {
    fn name(&self) -> &'static str {
        "wasm"
    }

    fn file_extension(&self) -> &'static str {
        "wat"
    }

    fn emit_program(&self, tir: &TirProgram, _crate_name: &str) -> String {
        let fns: Vec<&TirFn> = tir
            .fns
            .iter()
            .filter(|f| !f.is_builtin && f.receiver_type.is_none() && f.type_params.is_empty())
            .collect();

        // A Unit-returning `main` becomes the WASI `_start` entry point.
        // When present we emit the WASI runtime blob (memory, fd_write import,
        // bump allocator, int-to-string, println).
        let needs_wasi = fns
            .iter()
            .any(|f| f.name == "main" && matches!(f.ret_ty, Ty::Unit));

        let (literals, heap_start) = collect_literals(&fns, needs_wasi);
        let (enum_types, enum_variants) = collect_enums(&tir.types);
        let struct_layouts = collect_structs(&tir.types);
        let payload_enums = collect_payload_enums(&tir.types);
        let ctx = Ctx {
            needs_wasi,
            literals: &literals,
            enum_types: &enum_types,
            enum_variants: &enum_variants,
            struct_layouts: &struct_layouts,
            payload_enums: &payload_enums,
            label_counter: Cell::new(0),
            needs_runtime: Cell::new(false),
            string_params: std::cell::RefCell::new(std::collections::HashSet::new()),
            assert_mode: self.assert_mode,
        };

        // Emit fns into a scratch buffer first — `emit_assert_eq` on
        // String flips `ctx.needs_runtime`, and we only know whether to
        // import `runtime` memory + symbols after the whole body has been
        // walked. Fn bodies are self-contained, so buffering is cheap.
        let mut fns_out = String::new();
        for f in &fns {
            emit_fn(&mut fns_out, f, &ctx);
        }

        let mut out = String::from("(module\n");
        if ctx.needs_runtime.get() {
            // runtime.wasm exports its memory and the `_mvl_string_*` ops;
            // the user module imports both. Runtime data lives at 1 MB+,
            // ours at low offsets, so no address conflicts. We re-export
            // memory under the same name because WASI command modules
            // must have a `memory` export (wasmtime enforces this).
            out.push_str("  (import \"runtime\" \"memory\" (memory 0))\n");
            out.push_str("  (export \"memory\" (memory 0))\n");
            for (name, signature) in RUNTIME_IMPORTS {
                out.push_str(&format!(
                    "  (import \"runtime\" \"{name}\"\n    (func ${name} {signature}))\n"
                ));
            }
            if needs_wasi {
                // WASI blob but without its own `(memory 1) (export "memory")`
                // — memory is imported above.
                out.push_str(&emit_wasi_runtime_shared_memory(heap_start, &literals));
            }
        } else if needs_wasi {
            // Standalone WASI module — own memory, no runtime preload
            // needed. Matches the pre-#1819 behaviour for simple programs.
            out.push_str(&emit_wasi_runtime(heap_start, &literals));
        }

        out.push_str(&fns_out);

        for f in &fns {
            let (wasm_name, export_name) = effective_name(f, needs_wasi);
            out.push_str(&format!(
                "  (export \"{export_name}\" (func ${wasm_name}))\n"
            ));
        }

        out.push(')');
        out.push('\n');
        out
    }
}

/// Compute the block-type that a statement-form `if` should carry.
///
/// The TIR lowerer sometimes emits `TirStmt::If` for what a reader would
/// consider an expression, e.g. `fn f() -> Int { if c { 1 } else { 2 } }`.
/// If both branches leave a matching non-Unit value on the stack, we need
/// `if (result T)` — otherwise the WASM validator rejects the fn (values
/// left over inside a bare `if` block don't propagate to the enclosing
/// function's return slot).
///
/// Compares WASM types (not MVL types) so that e.g. `Ok(1)` with type
/// `Result[Int, Unknown]` and `Err("x")` with type `Result[Unknown, String]`
/// both lower to i32 and are recognised as compatible block types.
fn if_stmt_result_ty(then: &TirBlock, else_: &Option<TirElseBranch>, ctx: &Ctx) -> Option<Ty> {
    let t = block_trailing_ty(then)?;
    let e = match else_ {
        Some(TirElseBranch::Block(b)) => block_trailing_ty(b)?,
        Some(TirElseBranch::If(nested)) => match nested.as_ref() {
            TirStmt::If {
                then: t2,
                else_: e2,
                ..
            } => if_stmt_result_ty(t2, e2, ctx)?,
            _ => return None,
        },
        None => return None,
    };
    if matches!(t, Ty::Unit) {
        return None;
    }
    // Exact MVL-type match or same WASM type — either is fine for block-typing.
    if t == e || wasm_ty(&t, ctx) == wasm_ty(&e, ctx) {
        Some(t)
    } else {
        None
    }
}

/// Type of a block's trailing expression, if the block ends in one and
/// that expression is non-Unit.  Used to decide if a `TirStmt::If`'s
/// branches leave a value on the WASM stack.
fn block_trailing_ty(block: &TirBlock) -> Option<Ty> {
    let last = block.stmts.last()?;
    match last {
        TirStmt::Expr { expr, .. } if !matches!(expr.ty, Ty::Unit) => Some(expr.ty.clone()),
        _ => None,
    }
}

/// Map a MVL function to its WAT symbol / export name. Unit-returning `main`
/// becomes `_start` (WASI command convention) when the WASI runtime is enabled.
fn effective_name(f: &TirFn, needs_wasi: bool) -> (&str, &str) {
    if needs_wasi && f.name == "main" && matches!(f.ret_ty, Ty::Unit) {
        ("_start", "_start")
    } else {
        (f.name.as_str(), f.name.as_str())
    }
}

// ── Refinement / contract emission (#1822) ──────────────────────────────────

/// Returns true if `pred` can be checked at WASM runtime. Mirrors
/// `backends::rust::emit_types::is_runtime_checkable`: quantifiers and
/// ArrayGet are static-only; everything else emits.
fn is_runtime_checkable(pred: &RefExpr) -> bool {
    match pred {
        RefExpr::Forall { .. }
        | RefExpr::Exists { .. }
        | RefExpr::BoundedForall { .. }
        | RefExpr::BoundedExists { .. }
        | RefExpr::ArrayGet { .. } => false,
        RefExpr::LogicOp { left, right, .. }
        | RefExpr::Compare { left, right, .. }
        | RefExpr::ArithOp { left, right, .. }
        | RefExpr::BitwiseOp { left, right, .. }
        | RefExpr::Min { left, right, .. }
        | RefExpr::Max { left, right, .. } => {
            is_runtime_checkable(left) && is_runtime_checkable(right)
        }
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. }
        | RefExpr::BitwiseNot { inner, .. }
        | RefExpr::Abs { inner, .. } => is_runtime_checkable(inner),
        RefExpr::FieldAccess { object, .. } => is_runtime_checkable(object),
        RefExpr::StringOp { receiver, .. } => is_runtime_checkable(receiver),
        RefExpr::RegexMatch { receiver, .. } => is_runtime_checkable(receiver),
        RefExpr::Ident { .. }
        | RefExpr::Integer { .. }
        | RefExpr::Float { .. }
        | RefExpr::Bool { .. }
        | RefExpr::Len { .. } => true,
    }
}

/// Infer the WASM value type of a `RefExpr` leaf or arithmetic node.
/// Used to pick the right comparison opcode (`i64.eq` vs `f64.lt` etc.).
/// Returns `"i64"` for integers/unknown, `"f64"` for floats, `"i32"` for bools.
fn ref_expr_wasm_ty(pred: &RefExpr, binding_ty: &str, params: &[TirParam]) -> &'static str {
    match pred {
        RefExpr::Float { .. } => "f64",
        RefExpr::Bool { .. } => "i32",
        RefExpr::Integer { .. } => "i64",
        RefExpr::Ident { name, .. } => {
            if name == "self" || name == "result" {
                // Leak to 'static: binding_ty comes from wasm_ty which returns &'static str.
                // We need to return &'static str — match on the known variants.
                match binding_ty {
                    "f64" => "f64",
                    "i32" => "i32",
                    _ => "i64",
                }
            } else {
                params
                    .iter()
                    .find(|p| p.name == *name)
                    .map(|p| match p.ty.base() {
                        Ty::Float => "f64",
                        Ty::Bool | Ty::Byte => "i32",
                        _ => "i64",
                    })
                    .unwrap_or("i64")
            }
        }
        RefExpr::ArithOp { left, .. } => ref_expr_wasm_ty(left, binding_ty, params),
        RefExpr::Len { .. } => "i64",
        // Compare / LogicOp / Not always yield i32 (boolean)
        _ => "i32",
    }
}

/// Emit WASM instructions that push the raw *value* of `pred` onto the stack.
/// The result type is `ref_expr_wasm_ty(pred, …)`. Used as operands in Compare.
fn emit_ref_val_wasm(
    out: &mut String,
    pred: &RefExpr,
    binding: &str,
    binding_ty: &str,
    params: &[TirParam],
) {
    match pred {
        RefExpr::Integer { value, .. } => {
            out.push_str(&format!("    i64.const {value}\n"));
        }
        RefExpr::Float { value, .. } => {
            out.push_str(&format!("    f64.const {value}\n"));
        }
        RefExpr::Bool { value, .. } => {
            out.push_str(&format!("    i32.const {}\n", if *value { 1 } else { 0 }));
        }
        RefExpr::Ident { name, .. } => {
            let local = if name == "self" || name == "result" {
                binding.to_string()
            } else {
                format!("${name}")
            };
            out.push_str(&format!("    local.get {local}\n"));
        }
        RefExpr::ArithOp { op, left, right, .. } => {
            emit_ref_val_wasm(out, left, binding, binding_ty, params);
            emit_ref_val_wasm(out, right, binding, binding_ty, params);
            let ty = ref_expr_wasm_ty(left, binding_ty, params);
            let instr = match (ty, op) {
                ("f64", ArithOp::Add) => "f64.add",
                ("f64", ArithOp::Sub) => "f64.sub",
                ("f64", ArithOp::Mul) => "f64.mul",
                ("f64", ArithOp::Div) => "f64.div",
                (_, ArithOp::Add) => "i64.add",
                (_, ArithOp::Sub) => "i64.sub",
                (_, ArithOp::Mul) => "i64.mul",
                (_, ArithOp::Div) => "i64.div_s",
                (_, ArithOp::Rem) => "i64.rem_s",
            };
            out.push_str(&format!("    {instr}\n"));
        }
        RefExpr::Grouped { inner, .. } => {
            emit_ref_val_wasm(out, inner, binding, binding_ty, params);
        }
        // Abs(-x) = if x < 0 { -x } else { x } — emit inline for i64
        RefExpr::Abs { inner, .. } => {
            emit_ref_val_wasm(out, inner, binding, binding_ty, params);
            out.push_str("    i64.abs\n");
        }
        // Fallback: try to emit as boolean i32 (shouldn't be used as a value operand)
        _ => {
            emit_ref_expr_wasm(out, pred, binding, binding_ty, params);
        }
    }
}

/// Emit WASM instructions that push an `i32` boolean (0=false, 1=true) for `pred`.
/// Caller must ensure `is_runtime_checkable(pred)` is true.
fn emit_ref_expr_wasm(
    out: &mut String,
    pred: &RefExpr,
    binding: &str,
    binding_ty: &str,
    params: &[TirParam],
) {
    match pred {
        RefExpr::Compare { op, left, right, .. } => {
            let ty = ref_expr_wasm_ty(left, binding_ty, params);
            emit_ref_val_wasm(out, left, binding, binding_ty, params);
            emit_ref_val_wasm(out, right, binding, binding_ty, params);
            let instr = match (ty, op) {
                ("i64", CmpOp::Eq) => "i64.eq",
                ("i64", CmpOp::Ne) => "i64.ne",
                ("i64", CmpOp::Lt) => "i64.lt_s",
                ("i64", CmpOp::Gt) => "i64.gt_s",
                ("i64", CmpOp::Le) => "i64.le_s",
                ("i64", CmpOp::Ge) => "i64.ge_s",
                ("f64", CmpOp::Eq) => "f64.eq",
                ("f64", CmpOp::Ne) => "f64.ne",
                ("f64", CmpOp::Lt) => "f64.lt",
                ("f64", CmpOp::Gt) => "f64.gt",
                ("f64", CmpOp::Le) => "f64.le",
                ("f64", CmpOp::Ge) => "f64.ge",
                ("i32", CmpOp::Eq) => "i32.eq",
                ("i32", CmpOp::Ne) => "i32.ne",
                ("i32", CmpOp::Lt) => "i32.lt_s",
                ("i32", CmpOp::Gt) => "i32.gt_s",
                ("i32", CmpOp::Le) => "i32.le_s",
                ("i32", CmpOp::Ge) => "i32.ge_s",
                // Fallback — shouldn't occur with well-typed predicates
                (_, CmpOp::Eq) => "i64.eq",
                (_, CmpOp::Ne) => "i64.ne",
                (_, CmpOp::Lt) => "i64.lt_s",
                (_, CmpOp::Gt) => "i64.gt_s",
                (_, CmpOp::Le) => "i64.le_s",
                (_, CmpOp::Ge) => "i64.ge_s",
            };
            out.push_str(&format!("    {instr}\n"));
        }
        RefExpr::LogicOp { op, left, right, .. } => {
            // Short-circuit semantics would require blocks; emit eager and/or instead.
            // Sufficient for corpus predicates which have no side effects.
            emit_ref_expr_wasm(out, left, binding, binding_ty, params);
            emit_ref_expr_wasm(out, right, binding, binding_ty, params);
            match op {
                LogicOp::And => out.push_str("    i32.and\n"),
                LogicOp::Or => out.push_str("    i32.or\n"),
            }
        }
        RefExpr::Not { inner, .. } => {
            emit_ref_expr_wasm(out, inner, binding, binding_ty, params);
            out.push_str("    i32.eqz\n");
        }
        RefExpr::Grouped { inner, .. } => {
            emit_ref_expr_wasm(out, inner, binding, binding_ty, params);
        }
        RefExpr::Bool { value, .. } => {
            out.push_str(&format!("    i32.const {}\n", if *value { 1 } else { 0 }));
        }
        RefExpr::Ident { name, .. } => {
            // Boolean ident used as predicate directly
            let local = if name == "self" || name == "result" {
                binding.to_string()
            } else {
                format!("${name}")
            };
            out.push_str(&format!("    local.get {local}\n"));
        }
        // Other nodes are not boolean — emit as value and wrap with i32.ne 0
        _ => {
            emit_ref_val_wasm(out, pred, binding, binding_ty, params);
            let ty = ref_expr_wasm_ty(pred, binding_ty, params);
            match ty {
                "i64" => out.push_str("    i64.const 0\n    i64.ne\n"),
                "f64" => out.push_str("    f64.const 0\n    f64.ne\n"),
                _ => out.push_str("    i32.const 0\n    i32.ne\n"),
            }
        }
    }
}

/// Emit a runtime contract check for `pred`. Traps via `unreachable` if the
/// predicate evaluates to false.
///
/// `binding` is the WASM local name (e.g. `$b`, `$__result`) that replaces
/// `"self"` / `"result"` in the predicate; `binding_ty` is its WASM type.
///
/// Respects `AssertMode`: `Assume` skips entirely; `DebugOnly` is treated as
/// `Always` because WASM has no build-time configuration equivalent.
fn emit_contract_check(
    out: &mut String,
    pred: &RefExpr,
    binding: &str,
    binding_ty: &str,
    params: &[TirParam],
    assert_mode: AssertMode,
) {
    if assert_mode == AssertMode::Assume {
        return;
    }
    if !is_runtime_checkable(pred) {
        return;
    }
    emit_ref_expr_wasm(out, pred, binding, binding_ty, params);
    out.push_str("    i32.eqz\n");
    out.push_str("    if\n      unreachable\n    end\n");
}

fn emit_fn(out: &mut String, f: &TirFn, ctx: &Ctx) {
    // Update the per-function String-param registry so that `Var` accesses
    // to these params emit two `local.get` ops instead of one.
    *ctx.string_params.borrow_mut() = f
        .params
        .iter()
        .filter(|p| matches!(&p.ty, Ty::String))
        .map(|p| p.name.clone())
        .collect();

    let (wasm_name, _) = effective_name(f, ctx.needs_wasi);
    // Populate the per-function String-param set so the Var emitter knows
    // which String locals are split (ptr, len) params vs unsupported locals.
    {
        let mut sp = ctx.string_params.borrow_mut();
        sp.clear();
        for p in &f.params {
            if matches!(&p.ty, Ty::String) {
                sp.insert(p.name.clone());
            }
        }
    }

    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        if matches!(&p.ty, Ty::String) {
            // String params split into two i32 WASM params: (ptr, len).
            out.push_str(&format!(
                " (param ${}_ptr i32) (param ${}_len i32)",
                p.name, p.name
            ));
        } else {
            out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty, ctx)));
        }
    }
    if matches!(f.ret_ty, Ty::String) {
        // String returns as two i32s (ptr, len) — WASM multi-value return.
        out.push_str(" (result i32 i32)");
    } else if !matches!(f.ret_ty, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&f.ret_ty, ctx)));
    }
    out.push('\n');

    // Emit the body into a scratch buffer first. If it hits anything the
    // emitter doesn't support (leaves a `;; unsupported` marker), stub the
    // whole body with `unreachable` — a polymorphic trap that satisfies
    // the WASM validator regardless of the fn's signature. Callers hit a
    // clean runtime trap instead of the whole module failing to assemble,
    // which lets sibling fns in the same file still run.
    let mut body = String::new();
    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_block(&f.body, &mut locals);
    // Second pass: ctx-aware scan for temps that can only be discovered with
    // type-registry info (payload-enum unit-variant Var temps, string-field
    // unpack temps from FieldAccess). These use span-based names that the
    // emit path and collect path must agree on.
    collect_locals_ctx(&f.body, &mut locals, ctx);

    // Determine whether we need a $__result_CONTRACT local to check ensures /
    // return_refinement (#1822). Skip for Unit and String returns (String is
    // multi-value i32×2 — deferred) and when AssertMode is Assume.
    let has_checkable_ensures = ctx.assert_mode != AssertMode::Assume
        && !matches!(f.ret_ty, Ty::Unit | Ty::String)
        && (f.ensures.iter().any(is_runtime_checkable)
            || f.return_refinement
                .as_ref()
                .is_some_and(is_runtime_checkable));
    if has_checkable_ensures {
        locals.push(("__result_CONTRACT".to_string(), f.ret_ty.clone()));
    }

    // Deduplicate (collect passes may register the same name from nested
    // expressions or speculative String locals; WAT rejects duplicates).
    {
        let mut seen = std::collections::HashSet::new();
        locals.retain(|(name, _)| seen.insert(name.clone()));
    }
    for (name, ty) in &locals {
        body.push_str(&format!("    (local ${} {})\n", name, wasm_ty(ty, ctx)));
    }

    // Emit `requires` precondition checks at function entry (#1822).
    if ctx.assert_mode != AssertMode::Assume {
        for pred in &f.requires {
            emit_contract_check(&mut body, pred, "", "i64", &f.params, ctx.assert_mode);
        }
    }

    emit_block(&mut body, &f.body, ctx);

    // Emit `ensures` / return_refinement checks before implicit return (#1822).
    // We save the implicit-return expression into $__result_CONTRACT, run the
    // checks, then push it back. Explicit `return` mid-function bypasses these
    // checks — acceptable for the corpus tests, all of which use implicit return.
    if has_checkable_ensures {
        let ret_wasm = wasm_ty(&f.ret_ty, ctx);
        body.push_str("    local.set $__result_CONTRACT\n");
        for pred in f
            .ensures
            .iter()
            .chain(f.return_refinement.as_ref().into_iter())
        {
            emit_contract_check(
                &mut body,
                pred,
                "$__result_CONTRACT",
                ret_wasm,
                &f.params,
                ctx.assert_mode,
            );
        }
        body.push_str("    local.get $__result_CONTRACT\n");
    }

    if body.contains(";; unsupported") {
        out.push_str("    ;; body stubbed — contained unsupported constructs\n");
        out.push_str("    unreachable\n");
    } else {
        out.push_str(&body);
        // Drop each `__ms_*` temp local at the implicit-return path.
        // Every allocation from `.concat` / `.substring` / … was stashed
        // in a temp; freeing at fn exit reclaims the byte buffer + struct.
        // `_mvl_string_drop` is null-safe, so temps on code paths that
        // never allocated are harmless.
        //
        // For collections (`__ma_*` — MvlArray literal temps) we do NOT
        // drop, because that temp holds the *same pointer* as the value
        // that flowed out to the outer expression (typically bound to a
        // `let xs: List[T] = ...` local). Dropping both would double-free.
        // Instead, we drop user-bound list locals below.
        //
        // Limitation: this only catches implicit-return paths. Explicit
        // `return` in the middle of the fn skips cleanup and leaks. Fine
        // for phase-2/3 corpus tests, which all end via implicit return.
        for (name, ty) in &locals {
            if name.starts_with("__ms_") {
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_string_drop\n");
            } else if name.starts_with("__mo_") {
                // `.unwrap_or` already drops the box inline (see emit_expr).
                // The __mo_* local's value is 0 after that drop — a second
                // _mvl_option_drop(0) is a no-op (null-safe), so re-dropping
                // here is defense-in-depth without double-free risk.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_option_drop\n");
            } else if name.starts_with("__mr_") {
                // Same as __mo_*: Result.unwrap_or drops inline; re-drop is
                // null-safe defense-in-depth.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_result_drop\n");
            } else if name.starts_with("__match_") && option_inner_ty(ty).is_some() {
                // Match scrutinee that was an Option — drop the box the
                // arms consumed by value.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_option_drop\n");
            } else if name.starts_with("__match_") && result_ok_ty(ty).is_some() {
                // Match scrutinee that was a Result — drop the box.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_result_drop\n");
            } else if name.starts_with("__pr_") {
                // `?`-operator temp — the Result was already propagated (Ok
                // or Err) so drop its box here at fn exit. Null-safe.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_result_drop\n");
            } else if !name.starts_with("__") && option_inner_ty(ty).is_some() {
                // User-bound `let opt: Option[T] = …`. Rare in corpus.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_option_drop\n");
            } else if !name.starts_with("__") && result_ok_ty(ty).is_some() {
                // User-bound `let r: Result[T, E] = …`.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_result_drop\n");
            } else if !name.starts_with("__")
                && collection_elem_ty(ty).is_some()
                && collection_elem_ty(ty)
                    .map(|e| !matches!(e, Ty::String))
                    .unwrap_or(true)
            {
                // User-bound list / array / set. Drops the array header +
                // element buffer. Element-level drops (e.g. inner strings)
                // aren't emitted yet — deferred with List[String].
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_array_drop\n");
            } else if !name.starts_with("__")
                && matches!(map_key_val_ty(ty), Some((Ty::String, Ty::Int)))
            {
                // User-bound Map[String, Int]. Frees the MvlMap + copied key bytes.
                // Gate on the concrete type so future Map variants don't emit a
                // mismatched ABI call before their own drop function exists.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    call $_mvl_map_drop_si64\n");
            }
        }
    }
    out.push_str("  )\n");
}

// ── Local collection ─────────────────────────────────────────────────────

fn collect_locals_block(block: &TirBlock, locals: &mut Vec<(String, Ty)>) {
    for s in &block.stmts {
        collect_locals_stmt(s, locals);
    }
}

// ── ctx-aware local scan (#1821) ─────────────────────────────────────────
//
// A second pass over the function body that requires `ctx` to identify:
//  - Payload-enum unit-variant `Var` expressions → `__ev_<off>` (i32)
//  - String-field `FieldAccess` reads → `__sf_<off>_<len>` (i32)
//
// The main `collect_locals_*` functions can't see these because they don't
// carry `ctx`. This pass is run after the main scan in `emit_fn`.

fn collect_locals_ctx(block: &TirBlock, locals: &mut Vec<(String, Ty)>, ctx: &Ctx) {
    for s in &block.stmts {
        collect_locals_ctx_stmt(s, locals, ctx);
    }
}

fn collect_locals_ctx_stmt(stmt: &TirStmt, locals: &mut Vec<(String, Ty)>, ctx: &Ctx) {
    match stmt {
        TirStmt::Let { init, .. } => collect_locals_ctx_expr(init, locals, ctx),
        TirStmt::Assign { value, .. } => collect_locals_ctx_expr(value, locals, ctx),
        TirStmt::Return { value: Some(v), .. } => collect_locals_ctx_expr(v, locals, ctx),
        TirStmt::Expr { expr, .. } => collect_locals_ctx_expr(expr, locals, ctx),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_locals_ctx_expr(cond, locals, ctx);
            collect_locals_ctx(then, locals, ctx);
            match else_ {
                Some(TirElseBranch::Block(b)) => collect_locals_ctx(b, locals, ctx),
                Some(TirElseBranch::If(s)) => collect_locals_ctx_stmt(s, locals, ctx),
                None => {}
            }
        }
        TirStmt::While { cond, body, .. } => {
            collect_locals_ctx_expr(cond, locals, ctx);
            collect_locals_ctx(body, locals, ctx);
        }
        TirStmt::For { iter, body, .. } => {
            collect_locals_ctx_expr(iter, locals, ctx);
            collect_locals_ctx(body, locals, ctx);
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_locals_ctx_expr(scrutinee, locals, ctx);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_locals_ctx_expr(e, locals, ctx),
                    TirMatchBody::Block(b) => collect_locals_ctx(b, locals, ctx),
                }
            }
        }
        _ => {}
    }
}

fn collect_locals_ctx_expr(expr: &TirExpr, locals: &mut Vec<(String, Ty)>, ctx: &Ctx) {
    match &expr.kind {
        TirExprKind::Var(name) => {
            // Payload-enum unit-variant used as a value: `Shape::Point`.
            if let Some((type_name, _)) = name.split_once("::") {
                if let Some(info) = ctx.payload_enums.get(type_name) {
                    if info
                        .variants
                        .iter()
                        .any(|v| v.name == *name && v.fields.is_empty())
                    {
                        // __ev_<off>: i32 pointer from _mvl_struct_alloc.
                        locals.push((format!("__ev_{}", expr.span.offset), Ty::Bool));
                    }
                }
            }
        }
        TirExprKind::FieldAccess { expr: recv, field } => {
            collect_locals_ctx_expr(recv, locals, ctx);
            // String-field reads unpack via a tee temp.
            let struct_name = match &recv.ty {
                Ty::Named(n, _) => Some(n.clone()),
                Ty::Ref(_, inner) => {
                    if let Ty::Named(n, _) = inner.as_ref() {
                        Some(n.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(sname) = struct_name {
                if let Some(layout) = ctx.struct_layouts.get(&sname) {
                    if let Some(slot) = layout.fields.iter().find(|s| s.name == *field) {
                        if matches!(slot.ty, Ty::String) {
                            locals.push((
                                format!("__sf_{}_{}", slot.offset, field.len()),
                                Ty::Bool, // i32 placeholder
                            ));
                        }
                    }
                }
            }
        }
        TirExprKind::Construct { name, fields } => {
            // __st_* is registered by the ctx-unaware pass (it always applies).
            // __ep_* (payload area pointer for enum variants) needs ctx.
            if name.contains("::") {
                if let Some((type_name, _)) = name.split_once("::") {
                    if let Some(info) = ctx.payload_enums.get(type_name) {
                        if let Some(pv) = info.variants.iter().find(|v| v.name == *name) {
                            if pv.payload_size > 0 {
                                locals.push((
                                    format!("__ep_{}_{}", expr.span.offset, expr.span.len),
                                    Ty::Bool, // i32 placeholder
                                ));
                            }
                        }
                    }
                }
            }
            for (_, e) in fields {
                collect_locals_ctx_expr(e, locals, ctx);
            }
        }
        TirExprKind::Propagate(inner) => collect_locals_ctx_expr(inner, locals, ctx),
        TirExprKind::If { cond, then, else_ } => {
            collect_locals_ctx_expr(cond, locals, ctx);
            collect_locals_ctx(then, locals, ctx);
            if let Some(e) = else_ {
                collect_locals_ctx_expr(e, locals, ctx);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_locals_ctx_expr(scrutinee, locals, ctx);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_locals_ctx_expr(e, locals, ctx),
                    TirMatchBody::Block(b) => collect_locals_ctx(b, locals, ctx),
                }
            }
        }
        TirExprKind::Block(b) => collect_locals_ctx(b, locals, ctx),
        TirExprKind::Binary { left, right, .. } => {
            collect_locals_ctx_expr(left, locals, ctx);
            collect_locals_ctx_expr(right, locals, ctx);
        }
        TirExprKind::Unary { expr: inner, .. } => collect_locals_ctx_expr(inner, locals, ctx),
        TirExprKind::FnCall { name, args, .. } => {
            // Enum-variant FnCall (`Shape::Circle(5)`) routed to emit_construct
            // needs the same __st_* and __ep_* temps as TirExprKind::Construct.
            if let Some((type_name, _)) = name.split_once("::") {
                if let Some(info) = ctx.payload_enums.get(type_name) {
                    locals.push((struct_temp_name(expr), Ty::Bool));
                    if let Some(pv) = info.variants.iter().find(|v| v.name == *name) {
                        if pv.payload_size > 0 {
                            locals.push((
                                format!("__ep_{}_{}", expr.span.offset, expr.span.len),
                                Ty::Bool,
                            ));
                        }
                    }
                }
            }
            for a in args {
                collect_locals_ctx_expr(a, locals, ctx);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            collect_locals_ctx_expr(receiver, locals, ctx);
            for a in args {
                collect_locals_ctx_expr(a, locals, ctx);
            }
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_locals_ctx_expr(e, locals, ctx);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_locals_ctx_expr(k, locals, ctx);
                collect_locals_ctx_expr(v, locals, ctx);
            }
        }
        _ => {}
    }
}

fn collect_locals_stmt(stmt: &TirStmt, locals: &mut Vec<(String, Ty)>) {
    match stmt {
        TirStmt::Let {
            pattern, ty, init, ..
        } => {
            if let Pattern::Ident(name, _) = pattern {
                if matches!(ty, Ty::String) {
                    // String variables use split (ptr, len) locals.
                    locals.push((format!("{name}_ptr"), Ty::Bool)); // i32
                    locals.push((format!("{name}_len"), Ty::Bool)); // i32
                } else {
                    locals.push((name.clone(), ty.clone()));
                }
            }
            collect_locals_expr(init, locals);
        }
        TirStmt::Assign { value, .. } => collect_locals_expr(value, locals),
        TirStmt::Return { value: Some(v), .. } => collect_locals_expr(v, locals),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_locals_expr(cond, locals);
            collect_locals_block(then, locals);
            match else_ {
                Some(TirElseBranch::Block(b)) => collect_locals_block(b, locals),
                Some(TirElseBranch::If(s)) => collect_locals_stmt(s, locals),
                None => {}
            }
        }
        TirStmt::While {
            cond,
            body,
            decreases,
            span,
            ..
        } => {
            collect_locals_expr(cond, locals);
            // Declare the decreases-measure save slot (#1822). Use the span
            // offset as a stable per-loop unique suffix so collect and emit agree.
            if decreases.is_some() {
                locals.push((format!("__dec_{}", span.offset), Ty::Int));
            }
            collect_locals_block(body, locals);
        }
        TirStmt::For {
            pattern,
            iter,
            body,
            span,
            ..
        } => {
            collect_locals_expr(iter, locals);
            // Loop variable — `for x in xs { ... }` binds `x` to each element.
            // `Pattern::Wildcard` (`for _ in xs`) gets a synthesized name so
            // the local is still declared (some `for _` code still increments
            // an outer counter — the local itself is unused but needs to
            // exist for wasm-tools to accept `local.set`).
            let (var_name, var_ty) = match pattern {
                Pattern::Ident(n, _) => (
                    n.clone(),
                    collection_elem_ty(&iter.ty).cloned().unwrap_or(Ty::Int),
                ),
                _ => (
                    format!("__for_wild_{}", span.offset),
                    collection_elem_ty(&iter.ty).cloned().unwrap_or(Ty::Int),
                ),
            };
            locals.push((var_name, var_ty));
            // Range form uses only `__for_hi_<off>` (i64); list form uses
            // `__for_arr_<off>` (i32), `__for_idx_<off>` (i64),
            // `__for_len_<off>` (i64). Declaring all four for both shapes is
            // cheap and lets `emit_for_stmt` dispatch without pre-scan sync.
            locals.push((format!("__for_hi_{}", span.offset), Ty::Int));
            locals.push((format!("__for_arr_{}", span.offset), Ty::Bool));
            locals.push((format!("__for_idx_{}", span.offset), Ty::Int));
            locals.push((format!("__for_len_{}", span.offset), Ty::Int));
            collect_locals_block(body, locals);
        }
        TirStmt::Match {
            scrutinee,
            arms,
            span,
        } => {
            // Stmt-form match needs the same scrutinee temp as expr-form.
            locals.push((format!("__match_{}", span.offset), scrutinee.ty.clone()));
            collect_locals_expr(scrutinee, locals);
            let inner_ty = option_inner_ty(&scrutinee.ty).cloned();
            for arm in arms {
                collect_match_arm_locals(
                    arm,
                    &scrutinee.ty,
                    inner_ty.as_ref(),
                    span.offset,
                    locals,
                );
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_locals_expr(e, locals),
                    TirMatchBody::Block(b) => collect_locals_block(b, locals),
                }
            }
        }
        TirStmt::Expr { expr, .. } => collect_locals_expr(expr, locals),
        _ => {}
    }
}

fn collect_locals_expr(expr: &TirExpr, locals: &mut Vec<(String, Ty)>) {
    match &expr.kind {
        TirExprKind::If { cond, then, else_ } => {
            collect_locals_expr(cond, locals);
            collect_locals_block(then, locals);
            if let Some(e) = else_ {
                collect_locals_expr(e, locals);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            // Fresh temp for the scrutinee value — `emit_match` stashes
            // the scrutinee here so it doesn't re-evaluate per arm.
            locals.push((match_temp_name(expr), scrutinee.ty.clone()));
            collect_locals_expr(scrutinee, locals);
            let inner_ty = option_inner_ty(&scrutinee.ty).cloned();
            let span_off = expr.span.offset;
            for arm in arms {
                collect_match_arm_locals(arm, &scrutinee.ty, inner_ty.as_ref(), span_off, locals);
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_locals_expr(e, locals),
                    TirMatchBody::Block(b) => collect_locals_block(b, locals),
                }
            }
        }
        TirExprKind::Construct { fields, .. } => {
            // Struct/enum-variant construction needs a temp i32 local for the
            // allocated pointer so `local.tee` during field stores works.
            locals.push((struct_temp_name(expr), Ty::Bool)); // Bool → i32 placeholder
            for (_, e) in fields {
                collect_locals_expr(e, locals);
            }
        }
        TirExprKind::FieldAccess { expr: recv, .. } => {
            collect_locals_expr(recv, locals);
        }
        TirExprKind::Propagate(inner) => {
            collect_locals_expr(inner, locals);
            // Temp i32 to stash the Result pointer for tag check.
            locals.push((propagate_temp_name(expr), Ty::Bool)); // i32 placeholder
        }
        TirExprKind::Block(b) => collect_locals_block(b, locals),
        TirExprKind::Binary { left, right, .. } => {
            collect_locals_expr(left, locals);
            collect_locals_expr(right, locals);
        }
        TirExprKind::Unary { expr, .. } => collect_locals_expr(expr, locals),
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_locals_expr(a, locals);
            }
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            collect_locals_expr(receiver, locals);
            for a in args {
                collect_locals_expr(a, locals);
            }
            // Allocation-returning String methods leave a `*MvlString` on
            // the stack that the emitter unpacks via a temp i32 local.
            // Register it here so the fn prelude declares it.
            if matches!(&receiver.ty, Ty::String)
                && matches!(
                    method.as_str(),
                    "concat" | "substring" | "to_upper" | "to_lower" | "trim" | "replace"
                )
            {
                // Ty::Bool → i32 in `wasm_ty` — reuse for the pointer
                // temp so we don't need a dedicated "raw i32" ty.
                locals.push((mvl_string_temp_name(expr), Ty::Bool));
            }
            // `.unwrap_or(default)` on Option stashes the option pointer
            // in a temp so it can be dropped after the if-else selects
            // a value.
            if option_inner_ty(&receiver.ty).is_some() && method == "unwrap_or" {
                locals.push((mvl_option_temp_name(expr), Ty::Bool));
            }
            // Same for Result.unwrap_or — stashes the Result pointer in __mr_*.
            if result_ok_ty(&receiver.ty).is_some() && method == "unwrap_or" {
                locals.push((mvl_result_temp_name(expr), Ty::Bool));
            }
        }
        // List / Set literals stash their `*MvlArray` pointer in a temp
        // during the per-element push sequence. Declare it here.
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_locals_expr(e, locals);
            }
            locals.push((mvl_array_temp_name(expr), Ty::Bool));
        }
        // Map literals stash their `*MvlMap` pointer in a `__mm_*` temp
        // during the per-pair insert sequence.
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_locals_expr(k, locals);
                collect_locals_expr(v, locals);
            }
            locals.push((mvl_map_temp_name(expr), Ty::Bool));
        }
        _ => {}
    }
}

fn emit_block(out: &mut String, block: &TirBlock, ctx: &Ctx) {
    for stmt in &block.stmts {
        emit_stmt(out, stmt, ctx);
    }
}

fn emit_stmt(out: &mut String, stmt: &TirStmt, ctx: &Ctx) {
    match stmt {
        TirStmt::Expr { expr, .. } => emit_expr(out, expr, ctx),
        TirStmt::Return { value: Some(e), .. } => {
            emit_expr(out, e, ctx);
            out.push_str("    return\n");
        }
        TirStmt::Return { value: None, .. } => {
            out.push_str("    return\n");
        }
        // `let x: T = init;`  (or `let x: ref T = init;` — same lowering)
        // The local was already declared in the fn prelude via
        // `collect_locals_block`. Here we just evaluate the init and store.
        TirStmt::Let {
            pattern, ty, init, ..
        } => {
            if let Pattern::Ident(name, _) = pattern {
                emit_expr(out, init, ctx);
                if matches!(ty, Ty::String) {
                    // Init leaves (ptr, len) on stack — store into split locals.
                    out.push_str(&format!("    local.set ${name}_len\n"));
                    out.push_str(&format!("    local.set ${name}_ptr\n"));
                } else {
                    out.push_str(&format!("    local.set ${name}\n"));
                }
            } else {
                out.push_str(&format!("    ;; unsupported let pattern: {pattern:?}\n"));
            }
        }
        // `x = value;` — for `ref` locals. Only bare-identifier targets.
        TirStmt::Assign { target, value, .. } => {
            if let LValue::Ident(name, _) = target {
                emit_expr(out, value, ctx);
                out.push_str(&format!("    local.set ${name}\n"));
            } else {
                out.push_str(&format!("    ;; unsupported assign target: {target:?}\n"));
            }
        }
        // `if cond { then } else { else_ }` — statement form.
        //
        // The TIR lowerer emits `TirStmt::If` (not `Expr(If)`) for trailing
        // `if` expressions in a fn body like `fn f() -> Int { if … { 1 }
        // else { 2 } }`. So a statement-form if still needs a block-type
        // whenever both branches produce a matching non-Unit value, or the
        // fn's return slot ends up empty and the WASM validator rejects.
        TirStmt::If {
            cond, then, else_, ..
        } => {
            emit_expr(out, cond, ctx);
            match if_stmt_result_ty(then, else_, ctx) {
                Some(Ty::String) => {
                    out.push_str("    if (result i32 i32)\n");
                }
                Some(ty) => {
                    out.push_str(&format!("    if (result {})\n", wasm_ty(&ty, ctx)));
                }
                None => out.push_str("    if\n"),
            }
            emit_block(out, then, ctx);
            if let Some(e) = else_ {
                out.push_str("    else\n");
                match e {
                    TirElseBranch::Block(b) => emit_block(out, b, ctx),
                    TirElseBranch::If(nested) => emit_stmt(out, nested, ctx),
                }
            }
            out.push_str("    end\n");
        }
        // Trailing `match` in a fn body — same shape as the expression form
        // but arrives via `TirStmt::Match`. Reuse `emit_match_impl` with a
        // result type computed from the arms' trailing types (mirrors how
        // `TirStmt::If` handles its trailing-branch case above).
        TirStmt::Match {
            scrutinee,
            arms,
            span,
        } => {
            let result_ty = match_arms_result_ty(arms, ctx);
            emit_match_impl(out, scrutinee, arms, result_ty, span.offset, ctx);
        }
        // `while cond { body }` — canonical WASM shape:
        //   block $break_N (loop $cont_N (br_if $break_N (i32.eqz cond)) body (br $cont_N))
        //
        // With `decreases expr` (#1822): save the measure into a local before
        // the body and assert it strictly decreased afterward.  The local is
        // declared by `collect_locals_stmt` via the While arm in the collect pass.
        TirStmt::While {
            cond,
            body,
            decreases,
            span,
            ..
        } => {
            let brk = ctx.fresh_label("wend");
            let cnt = ctx.fresh_label("wcont");
            out.push_str(&format!("    block ${brk}\n"));
            out.push_str(&format!("    loop ${cnt}\n"));
            emit_expr(out, cond, ctx);
            out.push_str("    i32.eqz\n");
            out.push_str(&format!("    br_if ${brk}\n"));
            // Save decreases measure before body; assert strictly decreased after (#1822).
            if let Some(dec_expr) = decreases {
                if ctx.assert_mode != AssertMode::Assume {
                    let dec_local = format!("__dec_{}", span.offset);
                    emit_expr(out, dec_expr, ctx);
                    out.push_str(&format!("    local.set ${dec_local}\n"));
                    emit_block(out, body, ctx);
                    emit_expr(out, dec_expr, ctx);
                    out.push_str(&format!("    local.get ${dec_local}\n"));
                    // Trap if new_measure >= old_measure (must strictly decrease).
                    out.push_str("    i64.ge_s\n");
                    out.push_str("    if\n      unreachable\n    end\n");
                } else {
                    emit_block(out, body, ctx);
                }
            } else {
                emit_block(out, body, ctx);
            }
            out.push_str(&format!("    br ${cnt}\n"));
            out.push_str("    end\n");
            out.push_str("    end\n");
        }
        // `for pat in iter { body }` — two shapes, mirroring the LLVM
        // backend (emit_stmts_tir.rs::emit_for_stmt_tir):
        //   1. `for i in range(lo, hi)` — integer range loop, i64 counter.
        //   2. `for x in xs` — list iteration over MvlArray via
        //      `_mvl_array_len` + `_mvl_array_get` + typed load.
        TirStmt::For {
            pattern,
            iter,
            body,
            span,
            ..
        } => {
            emit_for_stmt(out, pattern, iter, body, span.offset, ctx);
        }
    }
}

fn emit_expr(out: &mut String, expr: &TirExpr, ctx: &Ctx) {
    match &expr.kind {
        TirExprKind::Literal(Literal::Integer(n)) => {
            out.push_str(&format!("    i64.const {n}\n"));
        }
        TirExprKind::Literal(Literal::Float(f)) => {
            // {:?} preserves the `.0` on whole-number floats so WAT parses
            // the literal as f64 rather than integer.
            out.push_str(&format!("    f64.const {f:?}\n"));
        }
        TirExprKind::Literal(Literal::Bool(b)) => {
            out.push_str(&format!("    i32.const {}\n", if *b { 1 } else { 0 }));
        }
        TirExprKind::Literal(Literal::Str(s)) => {
            // Placed in the module data section during collect_literals; here
            // we just push (offset, len) as i32s.
            if let Some(&(offset, len)) = ctx.literals.get(s) {
                out.push_str(&format!("    i32.const {offset}\n"));
                out.push_str(&format!("    i32.const {len}\n"));
            } else {
                out.push_str(&format!("    ;; missing literal: {s:?}\n"));
            }
        }
        TirExprKind::Var(name) => {
            // `None` — bare identifier of type `Option[_]`. Dispatch to the
            // runtime constructor before falling through to local.get.
            if name == "None" && matches!(&expr.ty, Ty::Option(_)) {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_option_none\n");
                return;
            }
            // Unit-variant enum values (e.g. `Direction::North`) appear in
            // TIR as bare `Var`s with a `Named` type. Distinguish them from
            // locals by presence in the enum-variant registry.
            if let Some(&id) = ctx.enum_variants.get(name) {
                out.push_str(&format!("    i32.const {id}\n"));
                return;
            }
            // Unit variants within a payload enum (e.g. `Shape::Point`).
            // These appear as `Var("Shape::Point")` but aren't in
            // `enum_variants` (which only covers all-unit enums). Look them
            // up in `payload_enums` and emit a heap-allocated enum header.
            if let Some((type_name, _)) = name.split_once("::") {
                if let Some(info) = ctx.payload_enums.get(type_name) {
                    if let Some(pv) = info.variants.iter().find(|v| v.name == *name) {
                        ctx.needs_runtime.set(true);
                        let disc = pv.disc;
                        // Alloc 8 bytes: { disc: i32, payload_ptr: i32 }.
                        out.push_str("    i32.const 8\n");
                        out.push_str("    call $_mvl_struct_alloc\n");
                        // dup pointer for two stores.
                        let tmp = format!("__ev_{}", expr.span.offset);
                        out.push_str(&format!("    local.tee ${tmp}\n"));
                        out.push_str(&format!("    i32.const {disc}\n"));
                        out.push_str("    i32.store offset=0\n");
                        // payload_ptr = 0 for unit variant.
                        out.push_str(&format!("    local.get ${tmp}\n"));
                        out.push_str("    i32.const 0\n");
                        out.push_str("    i32.store offset=4\n");
                        out.push_str(&format!("    local.get ${tmp}\n"));
                        return;
                    }
                }
            }
            // All String variables (params, let-bindings, match-arm bindings)
            // use split (ptr, len) locals named $name_ptr / $name_len.
            if matches!(&expr.ty, Ty::String) {
                out.push_str(&format!("    local.get ${name}_ptr\n"));
                out.push_str(&format!("    local.get ${name}_len\n"));
                return;
            }
            out.push_str(&format!("    local.get ${name}\n"));
        }
        TirExprKind::Unary { op, expr: inner } => {
            emit_unary(out, *op, inner, ctx);
        }
        TirExprKind::Binary { op, left, right } => {
            // String equality/inequality — route through runtime, same as
            // assert_eq[String]. Leaves i32 (0 or 1) on the stack.
            if matches!(&left.ty, Ty::String) && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
                ctx.needs_runtime.set(true);
                emit_expr(out, left, ctx); // (ptr1, len1)
                emit_expr(out, right, ctx); // (ptr2, len2)
                out.push_str("    call $_mvl_string_eq\n");
                if matches!(op, BinaryOp::Ne) {
                    out.push_str("    i32.eqz\n");
                }
                return;
            }
            emit_binary(out, *op, left, right, ctx);
        }
        TirExprKind::FnCall { name, args, .. } => {
            // Route builtins that don't have MVL bodies through the runtime
            // shims. `assert` and `println` are the two phase-1 cases.
            if name == "println" {
                for a in args {
                    emit_expr(out, a, ctx);
                }
                out.push_str("    call $mvl_println\n");
                return;
            }
            if name == "eprintln" {
                for a in args {
                    emit_expr(out, a, ctx);
                }
                out.push_str("    call $mvl_eprintln\n");
                return;
            }
            if name == "assert" && args.len() == 1 {
                emit_expr(out, &args[0], ctx);
                out.push_str("    i32.eqz\n");
                out.push_str("    if\n      unreachable\n    end\n");
                return;
            }
            if (name == "assert_eq" || name == "assert_ne") && args.len() == 2 {
                emit_assert_eq(out, &args[0], &args[1], name == "assert_ne", ctx);
                return;
            }
            // `Some(x)` constructor — the TIR lowerer represents it as a
            // FnCall on the bare name "Some". Dispatch to the runtime's
            // typed constructor based on the payload's WASM lowering.
            if name == "Some" && args.len() == 1 && matches!(&expr.ty, Ty::Option(_)) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                let inner = option_inner_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
                let (some_ctor, _) = option_ops_for(&inner, ctx);
                out.push_str(&format!("    call ${some_ctor}\n"));
                return;
            }
            // `Shape::Circle(5)` — positional enum-variant constructor written
            // with call syntax. The parser emits FnCall (not Construct) for
            // `Type::Variant(args)` forms. Route to the same emit path as
            // `TirExprKind::Construct` for `::` names whose type-prefix is a
            // known payload enum.
            if let Some((type_name, _)) = name.split_once("::") {
                if ctx.payload_enums.contains_key(type_name) {
                    let fields: Vec<(String, TirExpr)> = args
                        .iter()
                        .enumerate()
                        .map(|(i, a)| (i.to_string(), a.clone()))
                        .collect();
                    emit_construct(out, name, &fields, expr, ctx);
                    return;
                }
            }
            // `Ok(x)` constructor — dispatch to the typed result constructor.
            if name == "Ok" && args.len() == 1 && matches!(&expr.ty, Ty::Result(_, _)) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                let ok_ty = result_ok_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
                let (ok_ctor, _) = result_ops_for_ok(&ok_ty, ctx);
                out.push_str(&format!("    call ${ok_ctor}\n"));
                return;
            }
            // `Err(x)` constructor — `Err(s: String)` routes to
            // `_mvl_result_err_str`. Other error types not yet supported.
            if name == "Err" && args.len() == 1 && matches!(&expr.ty, Ty::Result(_, _)) {
                let err_ty = result_err_ty(&expr.ty).cloned().unwrap_or(Ty::String);
                if matches!(err_ty, Ty::String) {
                    ctx.needs_runtime.set(true);
                    emit_expr(out, &args[0], ctx);
                    out.push_str("    call $_mvl_result_err_str\n");
                } else {
                    out.push_str("    ;; unsupported Err type (only String errors supported)\n");
                }
                return;
            }
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call ${name}\n"));
        }
        TirExprKind::MethodCall {
            receiver, method, ..
        } if method == "to_string" => {
            emit_expr(out, receiver, ctx);
            match &receiver.ty {
                Ty::Int => out.push_str("    call $mvl_int_to_string\n"),
                Ty::Bool => {
                    let (tp, tl) = ctx.literals.get("true").copied().unwrap_or((0, 0));
                    let (fp, fl) = ctx.literals.get("false").copied().unwrap_or((0, 0));
                    out.push_str("    if (result i32 i32)\n");
                    out.push_str(&format!("      i32.const {tp}\n      i32.const {tl}\n"));
                    out.push_str("    else\n");
                    out.push_str(&format!("      i32.const {fp}\n      i32.const {fl}\n"));
                    out.push_str("    end\n");
                }
                other => {
                    out.push_str(&format!("    ;; unsupported to_string on {other:?}\n"));
                }
            }
        }
        // String query methods — route through `runtime/wasm/` ops. Receiver
        // leaves `(ptr, len)` on the stack; unary methods (`.len`,
        // `.is_empty`) leave that plus nothing else. Binary methods
        // (`.contains`, `.starts_with`, `.ends_with`, `.find`) then eval
        // the arg to append `(np, nl)`. Runtime fn pops all four i32 args
        // and returns the result.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if matches!(&receiver.ty, Ty::String)
            && matches!(
                method.as_str(),
                "len" | "is_empty" | "contains" | "starts_with" | "ends_with" | "find"
            ) =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call $_mvl_string_{method}\n"));
        }
        // String allocation-returning methods (Group B). Runtime returns
        // `*MvlString`; the emitter immediately unpacks `.ptr` / `.len`
        // via `i32.load` at the layout offsets so downstream code sees
        // the same `(ptr, len)` shape as a string literal. Temp local
        // holding the pointer is named after the source span so pre-scan
        // (`collect_locals_expr`) and emit agree without a counter.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if matches!(&receiver.ty, Ty::String)
            && matches!(
                method.as_str(),
                "concat" | "substring" | "to_upper" | "to_lower" | "trim" | "replace"
            ) =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call $_mvl_string_{method}\n"));
            emit_unpack_mvl_string(out, expr);
        }
        // `String.parse_int()` — returns a heap-allocated MvlResult pointer
        // (Group H import). Receiver is the raw (ptr, len) string on the stack.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if matches!(&receiver.ty, Ty::String) && method == "parse_int" && args.is_empty() => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            out.push_str("    call $_mvl_string_parse_int\n");
        }
        // `Result[T, E].unwrap_or(default)` — inline if/else on the tag,
        // then drop the Result box. Mirrors the Option.unwrap_or handler.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if result_ok_ty(&receiver.ty).is_some() && method == "unwrap_or" && args.len() == 1 => {
            ctx.needs_runtime.set(true);
            let ok_ty = result_ok_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let (_, getter) = result_ops_for_ok(&ok_ty, ctx);
            let result_wasm_ty = wasm_ty(&ok_ty, ctx);
            let temp = mvl_result_temp_name(expr);
            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    local.tee ${temp}\n"));
            out.push_str("    call $_mvl_result_tag\n");
            // tag == 0 → Ok. i32.eqz maps 0→1, non-zero→0.
            out.push_str("    i32.eqz\n");
            out.push_str(&format!("    if (result {result_wasm_ty})\n"));
            out.push_str(&format!("    local.get ${temp}\n"));
            out.push_str(&format!("    call ${getter}\n"));
            out.push_str("    else\n");
            emit_expr(out, &args[0], ctx);
            out.push_str("    end\n");
            out.push_str(&format!("    local.get ${temp}\n"));
            out.push_str("    call $_mvl_result_drop\n");
        }
        // Map[String, Int] methods (#1820). Guarded by `map_key_val_ty` so
        // these never fire on List / Set receivers.
        TirExprKind::MethodCall {
            receiver, method, ..
        } if map_key_val_ty(&receiver.ty).is_some() && method == "len" => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            out.push_str("    call $_mvl_map_len\n");
        }
        TirExprKind::MethodCall {
            receiver, method, ..
        } if map_key_val_ty(&receiver.ty).is_some() && method == "is_empty" => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            out.push_str("    call $_mvl_map_len\n");
            out.push_str("    i64.eqz\n");
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if map_key_val_ty(&receiver.ty).is_some() && method == "get" && args.len() == 1 => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx); // map ptr
            emit_expr(out, &args[0], ctx); // key → (ptr, len)
            out.push_str("    call $_mvl_map_get_si64\n");
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if map_key_val_ty(&receiver.ty).is_some() && method == "insert" && args.len() == 2 => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx); // map ptr
            emit_expr(out, &args[0], ctx); // key → (ptr, len)
            emit_expr(out, &args[1], ctx); // value → i64
            out.push_str("    call $_mvl_map_insert_si64\n");
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if map_key_val_ty(&receiver.ty).is_some()
            && method == "contains_key"
            && args.len() == 1 =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx); // map ptr
            emit_expr(out, &args[0], ctx); // key → (ptr, len)
            out.push_str("    call $_mvl_map_contains_key_si64\n");
        }
        // Set[T].contains(val) / Set[T].insert(val) — backed by MvlArray.
        // `contains` returns Bool (i32); `insert` pushes if not present.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some()
            && matches!(&receiver.ty, Ty::Set(_) | Ty::Ref(_, _))
            && method == "contains"
            && args.len() == 1 =>
        {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let fn_name = if is_i32(&elem_ty, ctx) {
                "_mvl_array_contains_i32"
            } else {
                "_mvl_array_contains_i64"
            };
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str(&format!("    call ${fn_name}\n"));
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some()
            && matches!(&receiver.ty, Ty::Set(_) | Ty::Ref(_, _))
            && method == "insert"
            && args.len() == 1 =>
        {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let fn_name = if is_i32(&elem_ty, ctx) {
                "_mvl_array_insert_i32"
            } else {
                "_mvl_array_insert_i64"
            };
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str(&format!("    call ${fn_name}\n"));
        }
        // List query methods — `.len()` / `.is_empty()` on any collection
        // that lowers to `*MvlArray` (List / Array / Set).
        TirExprKind::MethodCall {
            receiver, method, ..
        } if collection_elem_ty(&receiver.ty).is_some()
            && matches!(method.as_str(), "len" | "is_empty") =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    call $_mvl_array_{method}\n"));
        }
        // `.get(i)` on List / Array — returns `Option[T]` (heap-allocated
        // MvlOption). Element type comes from the receiver's collection
        // type. Runtime handles the OOB check + Option wrapping.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some() && method == "get" && args.len() == 1 => {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let getter = if is_i32(&elem_ty, ctx) {
                "_mvl_array_get_option_i32"
            } else {
                "_mvl_array_get_option_i64"
            };
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str(&format!("    call ${getter}\n"));
        }
        // `.unwrap_or(default)` on `Option[T]`. Emits an inline
        // `if tag == 0 (result T) then <value> else <default> end`.
        // Also drops the option box before yielding (both branches evaluate
        // to a T, but the intermediate pointer must be freed).
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if option_inner_ty(&receiver.ty).is_some()
            && method == "unwrap_or"
            && args.len() == 1 =>
        {
            ctx.needs_runtime.set(true);
            let inner = option_inner_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let (_, getter) = option_ops_for(&inner, ctx);
            let result_ty = wasm_ty(&inner, ctx);
            let temp = mvl_option_temp_name(expr);
            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    local.tee ${temp}\n"));
            out.push_str("    call $_mvl_option_tag\n");
            // tag == 0 → Some. `i32.eqz` maps 0→1, non-zero→0.
            out.push_str("    i32.eqz\n");
            out.push_str(&format!("    if (result {result_ty})\n"));
            out.push_str(&format!("    local.get ${temp}\n"));
            out.push_str(&format!("    call ${getter}\n"));
            out.push_str("    else\n");
            emit_expr(out, &args[0], ctx);
            out.push_str("    end\n");
            // Drop the Option box now (both branches produced T, box is
            // orphaned). Emitter also tracks __mo_* temps at fn exit as a
            // defense-in-depth against paths that leave one live.
            out.push_str(&format!("    local.get ${temp}\n"));
            out.push_str("    call $_mvl_option_drop\n");
        }
        // List literal — `[e1, e2, ...]`. Emits `_mvl_array_new(elem_size,
        // cap)`, stashes the pointer in a fn-scoped temp, pushes each
        // element via the typed push op. Leaves the pointer on the stack.
        TirExprKind::List { elems } => {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
            // String elements need a `*MvlString` allocation per element
            // (they arrive on the WASM stack as two i32s, but the array
            // stores fixed-width slots). Deferred until Phase 3.2.
            if matches!(&elem_ty, Ty::String) {
                out.push_str("    ;; unsupported: List[String] literal (Phase 3.2)\n");
                return;
            }
            let elem_size = elem_size_bytes(&elem_ty, ctx);
            let cap = elems.len().max(4) as i32;
            let temp = mvl_array_temp_name(expr);
            out.push_str(&format!("    i32.const {elem_size}\n"));
            out.push_str(&format!("    i32.const {cap}\n"));
            out.push_str("    call $_mvl_array_new\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            let push_op = push_op_for(&elem_ty, ctx);
            for e in elems {
                out.push_str(&format!("    local.get ${temp}\n"));
                emit_expr(out, e, ctx);
                out.push_str(&format!("    call {push_op}\n"));
            }
            out.push_str(&format!("    local.get ${temp}\n"));
        }
        // Set literal — `{e1, e2, ...}` (unique values). Same array
        // construction as List, then a dedup call (sort + remove adjacent
        // duplicates) to enforce Set semantics.
        TirExprKind::Set { elems } => {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
            if matches!(&elem_ty, Ty::String) {
                out.push_str("    ;; unsupported: Set[String] literal (Phase 3.2)\n");
                return;
            }
            let elem_size = elem_size_bytes(&elem_ty, ctx);
            let cap = elems.len().max(4) as i32;
            let temp = mvl_array_temp_name(expr);
            out.push_str(&format!("    i32.const {elem_size}\n"));
            out.push_str(&format!("    i32.const {cap}\n"));
            out.push_str("    call $_mvl_array_new\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            let push_op = push_op_for(&elem_ty, ctx);
            for e in elems {
                out.push_str(&format!("    local.get ${temp}\n"));
                emit_expr(out, e, ctx);
                out.push_str(&format!("    call {push_op}\n"));
            }
            // Dedup: sort and remove adjacent duplicates in-place.
            let dedup_fn = if is_i32(&elem_ty, ctx) {
                "_mvl_array_dedup_i32"
            } else {
                "_mvl_array_dedup_i64"
            };
            out.push_str(&format!("    local.get ${temp}\n"));
            out.push_str(&format!("    call ${dedup_fn}\n"));
            out.push_str(&format!("    local.get ${temp}\n"));
        }
        // Map literal — `{"k1": v1, "k2": v2, ...}`. Only `Map[String, Int]`
        // is supported for now (corpus scope). Emits `_mvl_map_new_si64()`,
        // stashes the pointer, then inserts each pair via `_mvl_map_insert_si64`.
        TirExprKind::Map { pairs } => {
            ctx.needs_runtime.set(true);
            // Check key/value types; only String→Int is wired (#1820).
            let kv = map_key_val_ty(&expr.ty);
            let supported = matches!(kv, Some((Ty::String, Ty::Int)));
            if !supported {
                out.push_str(
                    "    ;; unsupported: Map literal (only Map[String, Int] in Phase 3)\n",
                );
                return;
            }
            let temp = mvl_map_temp_name(expr);
            out.push_str("    call $_mvl_map_new_si64\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            for (k, v) in pairs {
                out.push_str(&format!("    local.get ${temp}\n"));
                emit_expr(out, k, ctx); // key → (ptr, len) two i32s
                emit_expr(out, v, ctx); // value → i64
                out.push_str("    call $_mvl_map_insert_si64\n");
            }
            out.push_str(&format!("    local.get ${temp}\n"));
        }
        TirExprKind::Block(block) => emit_block(out, block, ctx),
        // `match scrutinee { pat1 => arm1, pat2 => arm2, _ => default }` —
        // limited to Int/Bool literal patterns + Wildcard/Ident for now.
        // Enough for `02_control_flow/match_test.mvl`; enum / struct
        // patterns fall through to `;; unsupported`.
        TirExprKind::Match { scrutinee, arms } => {
            emit_match(out, expr, scrutinee, arms, ctx);
        }
        // `if cond { then } else { else_ }` — expression form. Both branches
        // must produce a value of `expr.ty`. WASM's block-typed `if
        // (result T)` handles this directly. `else_ = None` would give the
        // whole expr type `Unit` — treat as a no-op else.
        TirExprKind::If { cond, then, else_ } => {
            emit_expr(out, cond, ctx);
            let is_unit = matches!(expr.ty, Ty::Unit);
            if is_unit {
                out.push_str("    if\n");
            } else if matches!(expr.ty, Ty::String) {
                out.push_str("    if (result i32 i32)\n");
            } else {
                out.push_str(&format!("    if (result {})\n", wasm_ty(&expr.ty, ctx)));
            }
            emit_block(out, then, ctx);
            if let Some(e) = else_ {
                out.push_str("    else\n");
                emit_expr(out, e, ctx);
            } else if !is_unit {
                // Bare `if` used in expression position — should be Unit,
                // handled above. Any other missing else is a checker bug;
                // emit a comment so wasm-tools flags it.
                out.push_str("    ;; if-expr with missing else\n");
            }
            out.push_str("    end\n");
        }
        // `Name { field: val, … }` — struct or enum-variant construction (#1821).
        TirExprKind::Construct { name, fields } => {
            emit_construct(out, name, fields, expr, ctx);
        }
        // `expr.field` — struct field access (#1821).
        TirExprKind::FieldAccess { expr: recv, field } => {
            emit_field_access(out, recv, field, ctx);
        }
        // `expr?` — propagate Result failure (#1821).
        TirExprKind::Propagate(inner) => {
            emit_propagate(out, inner, expr, ctx);
        }
        other => {
            out.push_str(&format!("    ;; unsupported expr: {other:?}\n"));
        }
    }
}

/// Emit a `for pat in iter { body }` statement — dispatches on iter shape:
///
/// - `for i in range(lo, hi)` → integer range loop with an i64 counter
/// - `for x in xs` → list iteration via `_mvl_array_len` + `_mvl_array_get`
///   and a typed load
///
/// Loop shape is the same in both cases:
///
///   block $break
///     alloca+init counter/index
///     loop $cont
///       load counter
///       compare against upper bound; br_if $break when done
///       body (with loop var bound)
///       counter += 1
///       br $cont
///     end
///   end
///
/// Mirrors the LLVM backend's `emit_for_stmt_tir` (emit_stmts_tir.rs L354+).
fn emit_for_stmt(
    out: &mut String,
    pattern: &Pattern,
    iter: &TirExpr,
    body: &TirBlock,
    span_offset: u32,
    ctx: &Ctx,
) {
    let var_name: String = match pattern {
        Pattern::Ident(n, _) => n.clone(),
        _ => format!("__for_wild_{span_offset}"),
    };
    // `for i in range(lo, hi)` — spelled as a fn call in TIR.
    if let TirExprKind::FnCall { name, args, .. } = &iter.kind {
        if name == "range" && args.len() == 2 {
            emit_for_range(out, &var_name, &args[0], &args[1], body, span_offset, ctx);
            return;
        }
    }
    emit_for_list(out, &var_name, iter, body, span_offset, ctx);
}

/// Range form: `for i in range(lo, hi)` — pre-declared i64 local `$i` is
/// initialized to `lo`, loop compares `< hi`, increment by 1 each iteration.
fn emit_for_range(
    out: &mut String,
    var_name: &str,
    lo: &TirExpr,
    hi: &TirExpr,
    body: &TirBlock,
    span_offset: u32,
    ctx: &Ctx,
) {
    // Stash `hi` once at loop entry — evaluating it every iteration would
    // change the semantics when `hi` has side effects. LLVM does the same.
    let hi_local = format!("__for_hi_{span_offset}");
    let brk = ctx.fresh_label("for_end");
    let cnt = ctx.fresh_label("for_cont");

    emit_expr(out, lo, ctx);
    out.push_str(&format!("    local.set ${var_name}\n"));
    emit_expr(out, hi, ctx);
    out.push_str(&format!("    local.set ${hi_local}\n"));

    out.push_str(&format!("    block ${brk}\n"));
    out.push_str(&format!("    loop ${cnt}\n"));
    // done? i >= hi → break
    out.push_str(&format!("    local.get ${var_name}\n"));
    out.push_str(&format!("    local.get ${hi_local}\n"));
    out.push_str("    i64.ge_s\n");
    out.push_str(&format!("    br_if ${brk}\n"));
    // body
    emit_block(out, body, ctx);
    // i = i + 1
    out.push_str(&format!("    local.get ${var_name}\n"));
    out.push_str("    i64.const 1\n");
    out.push_str("    i64.add\n");
    out.push_str(&format!("    local.set ${var_name}\n"));
    out.push_str(&format!("    br ${cnt}\n"));
    out.push_str("    end\n");
    out.push_str("    end\n");
}

/// List form: `for x in xs` where `xs: List[T]` / `Array[T, N]` / `Set[T]`.
/// Uses `_mvl_array_len` for the bound and `_mvl_array_get` per iteration,
/// loading the element with the appropriate `i64.load` / `i32.load` /
/// `f64.load` based on `T`.
fn emit_for_list(
    out: &mut String,
    var_name: &str,
    iter: &TirExpr,
    body: &TirBlock,
    span_offset: u32,
    ctx: &Ctx,
) {
    let arr_local = format!("__for_arr_{span_offset}");
    let idx_local = format!("__for_idx_{span_offset}");
    let len_local = format!("__for_len_{span_offset}");
    let brk = ctx.fresh_label("for_end");
    let cnt = ctx.fresh_label("for_cont");

    let elem_ty = collection_elem_ty(&iter.ty).cloned().unwrap_or(Ty::Int);
    let (load_op, _) = list_elem_load_op(&elem_ty, ctx);

    ctx.needs_runtime.set(true);

    // Stash the array pointer + length once at loop entry.
    emit_expr(out, iter, ctx);
    out.push_str(&format!("    local.set ${arr_local}\n"));
    out.push_str(&format!("    local.get ${arr_local}\n"));
    out.push_str("    call $_mvl_array_len\n");
    out.push_str(&format!("    local.set ${len_local}\n"));
    // idx starts at 0.
    out.push_str("    i64.const 0\n");
    out.push_str(&format!("    local.set ${idx_local}\n"));

    out.push_str(&format!("    block ${brk}\n"));
    out.push_str(&format!("    loop ${cnt}\n"));
    // done? idx >= len → break
    out.push_str(&format!("    local.get ${idx_local}\n"));
    out.push_str(&format!("    local.get ${len_local}\n"));
    out.push_str("    i64.ge_s\n");
    out.push_str(&format!("    br_if ${brk}\n"));
    // load element into $var_name
    out.push_str(&format!("    local.get ${arr_local}\n"));
    out.push_str(&format!("    local.get ${idx_local}\n"));
    out.push_str("    call $_mvl_array_get\n");
    out.push_str(&format!("    {load_op}\n"));
    out.push_str(&format!("    local.set ${var_name}\n"));
    // body
    emit_block(out, body, ctx);
    // idx = idx + 1
    out.push_str(&format!("    local.get ${idx_local}\n"));
    out.push_str("    i64.const 1\n");
    out.push_str("    i64.add\n");
    out.push_str(&format!("    local.set ${idx_local}\n"));
    out.push_str(&format!("    br ${cnt}\n"));
    out.push_str("    end\n");
    out.push_str("    end\n");
}

/// Pick the WASM load op for an element type when reading from a pointer
/// returned by `_mvl_array_get`. Returns (op, byte width).
fn list_elem_load_op(elem_ty: &Ty, ctx: &Ctx) -> (&'static str, u32) {
    match wasm_ty(elem_ty, ctx) {
        "i32" => ("i32.load offset=0", 4),
        "f64" => ("f64.load offset=0", 8),
        _ => ("i64.load offset=0", 8),
    }
}

/// Emit a `match` expression as a chain of type-directed `eq` compares
/// wrapped in nested `if (result T) … else …` blocks. The default (no
/// pattern matched) is either the wildcard/ident arm or `unreachable` when
/// the match is exhaustive by structure (the checker's job).
///
/// The scrutinee is stashed in a fn-scoped temp local named after the
/// TirExpr's source-span offset (`__match_<offset>`), which
/// `collect_locals_expr` picks up during the pre-scan pass. Using the
/// span offset means both the pre-scan and the emitter agree on the name
/// without threading a counter through.
///
/// Supported patterns for now: `Pattern::Literal(Integer|Bool|Str)`,
/// `Pattern::Wildcard`, `Pattern::Ident` (used as a wildcard bind — we
/// don't emit the actual bind since none of the current corpus arms
/// reference the bound name). Anything else emits `;; unsupported`.
fn emit_match(
    out: &mut String,
    expr: &TirExpr,
    scrutinee: &TirExpr,
    arms: &[TirMatchArm],
    ctx: &Ctx,
) {
    let result_ty = if matches!(expr.ty, Ty::Unit) {
        None
    } else {
        Some(expr.ty.clone())
    };
    emit_match_impl(out, scrutinee, arms, result_ty, expr.span.offset, ctx);
}

/// Shared match lowering used by both `TirExprKind::Match` and
/// `TirStmt::Match`. `result_ty = Some(T)` when the match leaves a T on the
/// stack; `None` for statement form / Unit-typed matches.
fn emit_match_impl(
    out: &mut String,
    scrutinee: &TirExpr,
    arms: &[TirMatchArm],
    result_ty: Option<Ty>,
    span_offset: u32,
    ctx: &Ctx,
) {
    let temp = format!("__match_{}", span_offset);
    let if_open: String = result_ty
        .as_ref()
        .map(|t| {
            if matches!(t, Ty::String) {
                "    if (result i32 i32)\n".to_string()
            } else {
                format!("    if (result {})\n", wasm_ty(t, ctx))
            }
        })
        .unwrap_or_else(|| "    if\n".to_string());

    // Store scrutinee once — arms compare against it repeatedly.
    emit_expr(out, scrutinee, ctx);
    out.push_str(&format!("    local.set ${temp}\n"));

    // Split arms into checked (literal-pattern) and default (wildcard /
    // ident at any position — first one wins). Guards fall through to
    // "unsupported" because we haven't wired guard evaluation yet.
    let mut open_ifs = 0usize;
    let mut default_body: Option<&TirMatchBody> = None;

    for arm in arms {
        if arm.guard.is_some() {
            out.push_str("    ;; unsupported match guard\n");
            return;
        }
        match &arm.pattern {
            Pattern::Literal(lit, _) => {
                // scrutinee == literal ?
                out.push_str(&format!("    local.get ${temp}\n"));
                emit_literal(out, lit, ctx);
                out.push_str(&format!("    {}\n", eq_op_for(&scrutinee.ty, ctx)));
                out.push_str(&if_open);
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            Pattern::Ident(name, _) if ctx.enum_variants.contains_key(name) => {
                // Enum unit-variant pattern (e.g. `Direction::North`). Lower
                // like a literal comparison against the variant's i32 id.
                let id = ctx.enum_variants[name];
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str(&format!("    i32.const {id}\n"));
                out.push_str("    i32.eq\n");
                out.push_str(&if_open);
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `Some(inner)` pattern on Option[T]. Check tag == 0, then in
            // the arm body bind `inner` to the extracted payload via the
            // typed value getter. `Pattern::Ident("_")` skips the bind.
            Pattern::Some { inner, .. } => {
                ctx.needs_runtime.set(true);
                let inner_ty = option_inner_ty(&scrutinee.ty).cloned().unwrap_or(Ty::Int);
                let (_, getter) = option_ops_for(&inner_ty, ctx);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_option_tag\n");
                out.push_str("    i32.eqz\n"); // 1 when tag was 0 (Some)
                out.push_str(&if_open);
                if let Pattern::Ident(name, _) = inner.as_ref() {
                    if name != "_" {
                        out.push_str(&format!("    local.get ${temp}\n"));
                        out.push_str(&format!("    call ${getter}\n"));
                        out.push_str(&format!("    local.set ${name}\n"));
                    }
                }
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `None` pattern. Check tag == 1.
            Pattern::None(_) => {
                ctx.needs_runtime.set(true);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_option_tag\n");
                // tag directly serves as the i32 truthy value (1 = None).
                out.push_str(&if_open);
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `Ok(inner)` pattern on Result[T, E]. Check tag == 0, bind inner.
            Pattern::Ok { inner, .. } => {
                ctx.needs_runtime.set(true);
                let ok_ty = result_ok_ty(&scrutinee.ty).cloned().unwrap_or(Ty::Int);
                let (_, getter) = result_ops_for_ok(&ok_ty, ctx);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_result_tag\n");
                out.push_str("    i32.eqz\n"); // 1 when tag == 0 (Ok)
                out.push_str(&if_open);
                if let Pattern::Ident(name, _) = inner.as_ref() {
                    if name != "_" {
                        out.push_str(&format!("    local.get ${temp}\n"));
                        out.push_str(&format!("    call ${getter}\n"));
                        out.push_str(&format!("    local.set ${name}\n"));
                    }
                }
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `Err(inner)` pattern on Result[T, E]. Check tag == 1.
            Pattern::Err { inner, .. } => {
                ctx.needs_runtime.set(true);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_result_tag\n");
                // tag == 1 is truthy directly.
                out.push_str(&if_open);
                // Bind inner only if named and non-wildcard. For corpus the
                // error is String — extracted as *MvlString i32; not
                // unpacked to (ptr, len) since corpus Err arms discard it.
                if let Pattern::Ident(name, _) = inner.as_ref() {
                    if name != "_" {
                        out.push_str(&format!("    local.get ${temp}\n"));
                        out.push_str("    call $_mvl_result_value_i64\n");
                        // Narrow to i32 pointer for String payload.
                        out.push_str("    i32.wrap_i64\n");
                        out.push_str(&format!("    local.set ${name}\n"));
                    }
                }
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `Variant(f1, f2, …)` — payload enum pattern (#1821).
            // `Pattern::TupleStruct { name: "Shape::Circle", fields: [pat] }`
            Pattern::TupleStruct {
                name: variant_name,
                fields: pats,
                ..
            } => {
                // Find the variant in the payload-enum registry.
                let type_name = variant_name.split_once("::").map(|(t, _)| t).unwrap_or("");
                let pv_opt = ctx
                    .payload_enums
                    .get(type_name)
                    .and_then(|info| info.variants.iter().find(|v| v.name == *variant_name));
                let Some(pv) = pv_opt else {
                    out.push_str(&format!(
                        "    ;; unsupported TupleStruct pattern (unknown variant): {variant_name}\n"
                    ));
                    for _ in 0..open_ifs {
                        out.push_str("    end\n");
                    }
                    return;
                };
                ctx.needs_runtime.set(true);
                let disc = pv.disc;
                let pat_off = arm.pattern.span().offset;
                // Load discriminant from header offset 0 and compare.
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    i32.load offset=0\n");
                out.push_str(&format!("    i32.const {disc}\n"));
                out.push_str("    i32.eq\n");
                out.push_str(&if_open);
                // Load payload_ptr from header offset 4.
                let payload_ptr_local = format!("__pp_{span_offset}_{pat_off}");
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    i32.load offset=4\n");
                out.push_str(&format!("    local.set ${payload_ptr_local}\n"));
                // Bind each named pattern field from the payload at slot × 8.
                for (slot, pat) in pats.iter().enumerate() {
                    if let Pattern::Ident(name, _) = pat {
                        if name != "_" {
                            let field_ty = pv.fields.get(slot).cloned().unwrap_or(Ty::Int);
                            let byte_off = (slot as u32) * 8;
                            out.push_str(&format!("    local.get ${payload_ptr_local}\n"));
                            if matches!(field_ty, Ty::String) {
                                // String payload: stored as i64-extended *MvlString.
                                // Load, narrow to i32, unpack to (ptr, len) split locals.
                                out.push_str(&format!("    i64.load offset={byte_off}\n"));
                                out.push_str("    i32.wrap_i64\n");
                                let sv_tmp = format!("__sv_{}_{}", byte_off, name.len());
                                out.push_str(&format!("    local.tee ${sv_tmp}\n"));
                                out.push_str(&format!(
                                    "    i32.load offset={MVL_STRING_OFFSET_PTR}\n"
                                ));
                                out.push_str(&format!("    local.set ${name}_ptr\n"));
                                out.push_str(&format!("    local.get ${sv_tmp}\n"));
                                out.push_str(&format!(
                                    "    i32.load offset={MVL_STRING_OFFSET_LEN}\n"
                                ));
                                out.push_str(&format!("    local.set ${name}_len\n"));
                            } else {
                                emit_payload_load(out, &field_ty, byte_off, ctx);
                                out.push_str(&format!("    local.set ${name}\n"));
                            }
                        }
                    }
                }
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            Pattern::Wildcard(_) | Pattern::Ident(_, _) => {
                // For payload-enum unit variants inside a TupleStruct enum,
                // they appear as `Pattern::Ident("Shape::Point", _)`. Check
                // the payload_enums registry first before treating as default.
                let is_payload_unit = if let Pattern::Ident(iname, _) = &arm.pattern {
                    if let Some((tname, _)) = iname.split_once("::") {
                        ctx.payload_enums
                            .get(tname)
                            .and_then(|info| info.variants.iter().find(|v| v.name == *iname))
                            .map(|pv| pv.fields.is_empty())
                            .unwrap_or(false)
                    } else {
                        false
                    }
                } else {
                    false
                };

                if is_payload_unit {
                    if let Pattern::Ident(iname, _) = &arm.pattern {
                        let type_name = iname.split_once("::").map(|(t, _)| t).unwrap_or("");
                        let disc = ctx
                            .payload_enums
                            .get(type_name)
                            .and_then(|info| info.variants.iter().find(|v| v.name == *iname))
                            .map(|pv| pv.disc)
                            .unwrap_or(0);
                        ctx.needs_runtime.set(true);
                        out.push_str(&format!("    local.get ${temp}\n"));
                        out.push_str("    i32.load offset=0\n");
                        out.push_str(&format!("    i32.const {disc}\n"));
                        out.push_str("    i32.eq\n");
                        out.push_str(&if_open);
                        emit_match_body(out, &arm.body, ctx);
                        out.push_str("    else\n");
                        open_ifs += 1;
                    }
                } else {
                    // First wildcard/ident wins as the default; later arms are
                    // unreachable so we can stop looking.
                    default_body = Some(&arm.body);
                    break;
                }
            }
            other => {
                out.push_str(&format!("    ;; unsupported match pattern: {other:?}\n"));
                // Close any if-blocks we opened so the WAT is still balanced —
                // the `;; unsupported` marker will cause the fn to be
                // stubbed by `emit_fn`, so what we emit here doesn't matter.
                for _ in 0..open_ifs {
                    out.push_str("    end\n");
                }
                return;
            }
        }
    }

    if let Some(b) = default_body {
        emit_match_body(out, b, ctx);
    } else {
        // No default arm — exhaustive match. If we reach here, no arm
        // matched, which is a checker bug at compile time; trap at
        // runtime so it's loud rather than silent.
        out.push_str("    unreachable\n");
    }

    for _ in 0..open_ifs {
        out.push_str("    end\n");
    }
}

fn emit_match_body(out: &mut String, body: &TirMatchBody, ctx: &Ctx) {
    match body {
        TirMatchBody::Expr(e) => emit_expr(out, e, ctx),
        TirMatchBody::Block(b) => emit_block(out, b, ctx),
    }
}

// ── Struct and enum-payload construction (#1821) ─────────────────────────

/// Emit `Name { field: val, … }` construction. Dispatches to struct layout
/// or payload-enum variant layout depending on whether the name contains `::`.
fn emit_construct(
    out: &mut String,
    name: &str,
    fields: &[(String, TirExpr)],
    expr: &TirExpr,
    ctx: &Ctx,
) {
    if let Some((type_name, _)) = name.split_once("::") {
        // Enum-variant construction: `Shape::Circle(5)`.
        emit_enum_variant_construct(out, name, type_name, fields, expr, ctx);
    } else {
        // Struct construction: `Point { x: 3, y: 4 }`.
        emit_struct_construct(out, name, fields, expr, ctx);
    }
}

fn emit_struct_construct(
    out: &mut String,
    name: &str,
    fields: &[(String, TirExpr)],
    expr: &TirExpr,
    ctx: &Ctx,
) {
    let Some(layout) = ctx.struct_layouts.get(name) else {
        out.push_str(&format!("    ;; unsupported struct construct: {name}\n"));
        return;
    };
    ctx.needs_runtime.set(true);
    let temp = struct_temp_name(expr);
    // Allocate the struct region.
    out.push_str(&format!("    i32.const {}\n", layout.total_size));
    out.push_str("    call $_mvl_struct_alloc\n");
    out.push_str(&format!("    local.set ${temp}\n"));
    // Store each field at its layout offset.
    for slot in &layout.fields {
        let val_expr = fields.iter().find(|(n, _)| n == &slot.name).map(|(_, e)| e);
        let Some(val) = val_expr else {
            continue;
        };
        out.push_str(&format!("    local.get ${temp}\n"));
        emit_struct_store(out, val, &slot.ty, slot.offset, ctx);
    }
    out.push_str(&format!("    local.get ${temp}\n"));
}

fn emit_enum_variant_construct(
    out: &mut String,
    variant_name: &str,
    type_name: &str,
    fields: &[(String, TirExpr)],
    expr: &TirExpr,
    ctx: &Ctx,
) {
    let Some(info) = ctx.payload_enums.get(type_name) else {
        out.push_str(&format!(
            "    ;; unsupported enum variant construct: {variant_name}\n"
        ));
        return;
    };
    let Some(pv) = info.variants.iter().find(|v| v.name == variant_name) else {
        out.push_str(&format!("    ;; unknown variant: {variant_name}\n"));
        return;
    };
    ctx.needs_runtime.set(true);
    let temp = struct_temp_name(expr);
    let disc = pv.disc;

    // Alloc 8 bytes for the enum header { disc: i32, payload_ptr: i32 }.
    out.push_str("    i32.const 8\n");
    out.push_str("    call $_mvl_struct_alloc\n");
    out.push_str(&format!("    local.set ${temp}\n"));
    // Store discriminant.
    out.push_str(&format!("    local.get ${temp}\n"));
    out.push_str(&format!("    i32.const {disc}\n"));
    out.push_str("    i32.store offset=0\n");

    let field_exprs: Vec<&TirExpr> = fields.iter().map(|(_, e)| e).collect();
    if pv.payload_size > 0 && !field_exprs.is_empty() {
        // Alloc payload area and store fields.
        let payload_temp = format!("__ep_{}_{}", expr.span.offset, expr.span.len);
        out.push_str(&format!("    i32.const {}\n", pv.payload_size));
        out.push_str("    call $_mvl_struct_alloc\n");
        out.push_str(&format!("    local.set ${payload_temp}\n"));
        for (slot_idx, field_expr) in field_exprs.iter().enumerate() {
            let byte_off = (slot_idx as u32) * 8;
            let field_ty = pv.fields.get(slot_idx).cloned().unwrap_or(Ty::Int);
            out.push_str(&format!("    local.get ${payload_temp}\n"));
            emit_payload_store(out, field_expr, &field_ty, byte_off, ctx);
        }
        // Store payload_ptr in the header.
        out.push_str(&format!("    local.get ${temp}\n"));
        out.push_str(&format!("    local.get ${payload_temp}\n"));
        out.push_str("    i32.store offset=4\n");
    } else {
        // Unit variant within a payload enum: payload_ptr = 0.
        out.push_str(&format!("    local.get ${temp}\n"));
        out.push_str("    i32.const 0\n");
        out.push_str("    i32.store offset=4\n");
    }
    out.push_str(&format!("    local.get ${temp}\n"));
}

/// Store a field value into a struct region at `byte_off`. Dispatches on
/// the field type to choose the correct WASM store opcode.
fn emit_struct_store(out: &mut String, val: &TirExpr, field_ty: &Ty, byte_off: u32, ctx: &Ctx) {
    match field_ty {
        Ty::String => {
            // String fields are stored as *MvlString (i32 pointer).
            // val pushes (ptr, len); call _mvl_string_new to heap-allocate.
            ctx.needs_runtime.set(true);
            emit_expr(out, val, ctx);
            out.push_str("    call $_mvl_string_new\n");
            out.push_str(&format!("    i32.store offset={byte_off}\n"));
        }
        Ty::Float => {
            emit_expr(out, val, ctx);
            out.push_str(&format!("    f64.store offset={byte_off}\n"));
        }
        _ if is_i32(field_ty, ctx) => {
            emit_expr(out, val, ctx);
            out.push_str(&format!("    i32.store offset={byte_off}\n"));
        }
        _ => {
            // Default: i64 (Int and other 8-byte types).
            emit_expr(out, val, ctx);
            out.push_str(&format!("    i64.store offset={byte_off}\n"));
        }
    }
}

/// Store a payload-enum field (always 8-byte slots) at `byte_off`.
fn emit_payload_store(out: &mut String, val: &TirExpr, field_ty: &Ty, byte_off: u32, ctx: &Ctx) {
    match field_ty {
        Ty::String => {
            ctx.needs_runtime.set(true);
            emit_expr(out, val, ctx);
            out.push_str("    call $_mvl_string_new\n");
            // Widen *MvlString i32 to i64 for the 8-byte slot.
            out.push_str("    i64.extend_i32_u\n");
            out.push_str(&format!("    i64.store offset={byte_off}\n"));
        }
        Ty::Float => {
            emit_expr(out, val, ctx);
            out.push_str(&format!("    f64.store offset={byte_off}\n"));
        }
        _ if is_i32(field_ty, ctx) => {
            emit_expr(out, val, ctx);
            // Widen i32 to i64 for the uniform 8-byte slot.
            out.push_str("    i64.extend_i32_u\n");
            out.push_str(&format!("    i64.store offset={byte_off}\n"));
        }
        _ => {
            emit_expr(out, val, ctx);
            out.push_str(&format!("    i64.store offset={byte_off}\n"));
        }
    }
}

/// Load a field from a payload area (8-byte slots). Leaves the correct WASM
/// type on the stack for the field's declared type.
fn emit_payload_load(out: &mut String, field_ty: &Ty, byte_off: u32, ctx: &Ctx) {
    match field_ty {
        Ty::Float => {
            out.push_str(&format!("    f64.load offset={byte_off}\n"));
        }
        Ty::String => {
            // Stored as i64-extended *MvlString; narrow back to i32.
            out.push_str(&format!("    i64.load offset={byte_off}\n"));
            out.push_str("    i32.wrap_i64\n");
            // Now we have *MvlString; unpack to (ptr, len).
            // Store in temp, load .ptr @ 0, load .len @ 4.
            // (Caller stores the i32 *MvlString in the named local.)
        }
        _ if is_i32(field_ty, ctx) => {
            out.push_str(&format!("    i64.load offset={byte_off}\n"));
            out.push_str("    i32.wrap_i64\n");
        }
        _ => {
            out.push_str(&format!("    i64.load offset={byte_off}\n"));
        }
    }
}

// ── Field access (#1821) ─────────────────────────────────────────────────

/// Emit `recv.field` — struct field read.
fn emit_field_access(out: &mut String, recv: &TirExpr, field: &str, ctx: &Ctx) {
    let struct_name = match &recv.ty {
        Ty::Named(n, _) => n.clone(),
        Ty::Ref(_, inner) => match inner.as_ref() {
            Ty::Named(n, _) => n.clone(),
            _ => {
                out.push_str(&format!(
                    "    ;; unsupported field access recv ty: {:?}\n",
                    recv.ty
                ));
                return;
            }
        },
        _ => {
            out.push_str(&format!(
                "    ;; unsupported field access recv ty: {:?}\n",
                recv.ty
            ));
            return;
        }
    };
    let Some(layout) = ctx.struct_layouts.get(&struct_name) else {
        out.push_str(&format!(
            "    ;; unknown struct for field access: {struct_name}\n"
        ));
        return;
    };
    let Some(slot) = layout.fields.iter().find(|s| s.name == field) else {
        out.push_str(&format!("    ;; unknown field: {struct_name}.{field}\n"));
        return;
    };
    emit_expr(out, recv, ctx); // leaves *struct on stack
    let byte_off = slot.offset;
    match &slot.ty {
        Ty::String => {
            // Stored as *MvlString. Load the i32 pointer, then unpack
            // to (ptr, len) so downstream code sees the standard repr.
            ctx.needs_runtime.set(true);
            out.push_str(&format!("    i32.load offset={byte_off}\n"));
            // Now *MvlString is on stack. Load .ptr and .len.
            // Use a temp approach: the string field unpack needs a tee.
            // We re-emit the struct load approach:
            // Actually we already consumed the struct ptr via emit_expr.
            // The *MvlString ptr is on stack — unpack inline.
            let tmp_name = format!("__sf_{}_{}", byte_off, field.len());
            out.push_str(&format!("    local.tee ${tmp_name}\n"));
            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_PTR}\n"));
            out.push_str(&format!("    local.get ${tmp_name}\n"));
            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_LEN}\n"));
        }
        Ty::Float => {
            out.push_str(&format!("    f64.load offset={byte_off}\n"));
        }
        _ if is_i32(&slot.ty, ctx) => {
            out.push_str(&format!("    i32.load offset={byte_off}\n"));
        }
        _ => {
            out.push_str(&format!("    i64.load offset={byte_off}\n"));
        }
    }
}

// ── Result propagation (#1821) ───────────────────────────────────────────

/// Emit `inner?` — evaluate `inner`, check the Result tag; if Err return
/// early, if Ok extract and leave the i64 payload on the stack.
fn emit_propagate(out: &mut String, inner: &TirExpr, expr: &TirExpr, ctx: &Ctx) {
    ctx.needs_runtime.set(true);
    let temp = propagate_temp_name(expr);
    emit_expr(out, inner, ctx); // leaves *MvlResult (i32) on stack
    out.push_str(&format!("    local.tee ${temp}\n"));
    out.push_str("    call $_mvl_result_tag\n");
    out.push_str("    i32.eqz\n"); // 1 if Ok
    out.push_str("    if (result i64)\n");
    // Ok path: extract i64 payload.
    out.push_str(&format!("    local.get ${temp}\n"));
    out.push_str("    call $_mvl_result_value_i64\n");
    out.push_str("    else\n");
    // Err path: re-wrap and early-return the Result.
    // Drop the Ok-path temp; return inner's *MvlResult to caller.
    out.push_str(&format!("    local.get ${temp}\n"));
    out.push_str("    return\n");
    // WASM if requires both branches to leave same type. After `return`
    // the else-branch is dead, but the validator still needs the type to
    // match. Push an unreachable i64 as a type placeholder.
    out.push_str("    i64.const 0\n");
    out.push_str("    end\n");
}

// ── Local collection helpers (#1821) ─────────────────────────────────────

/// Declare locals needed by a single match arm pattern. Extracted so both
/// `collect_locals_stmt` (TirStmt::Match) and `collect_locals_expr`
/// (TirExprKind::Match) can share the same logic.
fn collect_match_arm_locals(
    arm: &TirMatchArm,
    _scrutinee_ty: &Ty,
    option_inner: Option<&Ty>,
    span_offset: u32,
    locals: &mut Vec<(String, Ty)>,
) {
    match &arm.pattern {
        Pattern::Some { inner, .. } => {
            if let Pattern::Ident(name, _) = inner.as_ref() {
                if name != "_" {
                    let ty = option_inner.cloned().unwrap_or(Ty::Int);
                    locals.push((name.clone(), ty));
                }
            }
        }
        Pattern::Ok { inner, .. } => {
            if let Pattern::Ident(name, _) = inner.as_ref() {
                if name != "_" {
                    locals.push((name.clone(), Ty::Int));
                }
            }
        }
        Pattern::Err { inner, .. } => {
            if let Pattern::Ident(name, _) = inner.as_ref() {
                if name != "_" {
                    // Err payload is *MvlString (i32). Use Bool as the i32
                    // placeholder type — wasm_ty maps Bool → i32.
                    locals.push((name.clone(), Ty::Bool));
                }
            }
        }
        Pattern::TupleStruct {
            name: vname,
            fields: pats,
            span,
            ..
        } => {
            // Payload pointer temp — uses (match span_offset, pattern span offset)
            // to match the name emitted by emit_match_impl.
            locals.push((
                format!("__pp_{}_{}", span_offset, span.offset),
                Ty::Bool, // i32 placeholder
            ));
            let _ = vname;
            for (slot, pat) in pats.iter().enumerate() {
                if let Pattern::Ident(n, _) = pat {
                    if n != "_" {
                        locals.push((n.clone(), Ty::Int)); // i64 for Int/Bool fields
                                                           // Speculatively add split String locals and __sv_* temp.
                                                           // Redundant for non-String fields but cheap; deduped later.
                        locals.push((format!("{n}_ptr"), Ty::Bool)); // i32
                        locals.push((format!("{n}_len"), Ty::Bool)); // i32
                        let byte_off = (slot as u32) * 8;
                        locals.push((format!("__sv_{}_{}", byte_off, n.len()), Ty::Bool));
                    }
                }
            }
        }
        _ => {}
    }
}

/// WASM equality opcode for a scrutinee type. Types beyond scalar defaults
/// (String, structs, etc.) fall back to `i64.eq` which is wrong for them —
/// but the pattern arm would have hit the unsupported case before this
/// runs, so nothing hits the wrong branch in practice.
fn eq_op_for(ty: &Ty, ctx: &Ctx) -> &'static str {
    if is_float(ty) {
        "f64.eq"
    } else if is_i32(ty, ctx) {
        "i32.eq"
    } else {
        "i64.eq"
    }
}

/// Fn-scoped temp local name for a `match` scrutinee, keyed on the source
/// span offset so `collect_locals_expr` / `collect_locals_stmt` and the
/// emit paths agree.
fn match_temp_name(expr: &TirExpr) -> String {
    format!("__match_{}", expr.span.offset)
}

/// Compute the WASM block-type a statement-form `match` should carry.
/// Every arm's body must produce a matching non-Unit trailing type, or
/// we fall back to statement (no-result) form.
fn match_arms_result_ty(arms: &[TirMatchArm], ctx: &Ctx) -> Option<Ty> {
    let mut ty: Option<Ty> = None;
    for arm in arms {
        let arm_ty = match &arm.body {
            TirMatchBody::Expr(e) if !matches!(e.ty, Ty::Unit) => e.ty.clone(),
            TirMatchBody::Block(b) => block_trailing_ty(b)?,
            _ => return None,
        };
        match &ty {
            None => ty = Some(arm_ty),
            // Exact MVL match or same WASM type (handles Ok vs Err type differences).
            Some(t) if *t == arm_ty || wasm_ty(t, ctx) == wasm_ty(&arm_ty, ctx) => {}
            _ => return None,
        }
    }
    ty
}

/// Emit a single `Literal` — factored out so `emit_match` and the main
/// `emit_expr` share the same literal lowering (integer / float / bool /
/// string all lower identically in match patterns as in ordinary
/// expressions).
fn emit_literal(out: &mut String, lit: &Literal, ctx: &Ctx) {
    match lit {
        Literal::Integer(n) => out.push_str(&format!("    i64.const {n}\n")),
        Literal::Float(f) => out.push_str(&format!("    f64.const {f:?}\n")),
        Literal::Bool(b) => {
            out.push_str(&format!("    i32.const {}\n", if *b { 1 } else { 0 }));
        }
        Literal::Str(s) => {
            if let Some(&(offset, len)) = ctx.literals.get(s) {
                out.push_str(&format!("    i32.const {offset}\n"));
                out.push_str(&format!("    i32.const {len}\n"));
            } else {
                out.push_str(&format!("    ;; missing literal: {s:?}\n"));
            }
        }
        Literal::Char(c) => out.push_str(&format!("    i32.const {}\n", *c as u32)),
        Literal::Unit => {} // no value pushed
    }
}

/// Unpack a `*MvlString` on top of the stack into `(ptr, len)` pushed
/// back on the stack. Uses a fn-scoped temp local named after the source
/// span so `collect_locals_expr` and the emit path agree on the name.
///
///   before:  stack = [..., *MvlString]
///   after:   stack = [..., ptr, len]
fn emit_unpack_mvl_string(out: &mut String, expr: &TirExpr) {
    let local = mvl_string_temp_name(expr);
    out.push_str(&format!("    local.tee ${local}\n"));
    // .ptr @ offset 0
    out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_PTR}\n"));
    out.push_str(&format!("    local.get ${local}\n"));
    // .len @ offset 4
    out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_LEN}\n"));
}

/// Temp local name for a `*MvlString` unpack — keyed by source span so
/// the pre-scan and emit paths agree without threading a counter through.
///
/// Uses both `offset` and `len` because nested method calls share the
/// same starting offset (the receiver's start position). Given
/// `"a".concat(b).substring(0, 1)` the concat and substring TIR nodes
/// both have `span.offset` at `"a"`'s position; only the length
/// disambiguates. Using offset alone would collide → duplicate local
/// declarations → wasm-tools rejects the WAT.
fn mvl_string_temp_name(expr: &TirExpr) -> String {
    format!("__ms_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local name for the `*MvlArray` pointer stashed during a list
/// literal's per-element push sequence. Same span-based scheme as
/// `mvl_string_temp_name`.
fn mvl_array_temp_name(expr: &TirExpr) -> String {
    format!("__ma_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local name for the `*MvlOption` pointer stashed during an
/// `.unwrap_or(default)` invocation (tee → tag test → conditional value
/// extract → drop). Same span-based scheme.
fn mvl_option_temp_name(expr: &TirExpr) -> String {
    format!("__mo_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local name for a `*MvlMap` pointer built from a Map literal.
/// Excluded from fn-exit drops (same reason as `__ma_*`): the pointer
/// flows out to the user-bound `let m` local and must not double-free.
fn mvl_map_temp_name(expr: &TirExpr) -> String {
    format!("__mm_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local for struct / enum-variant construction — holds the allocated
/// pointer during field stores before returning it on the WASM stack.
fn struct_temp_name(expr: &TirExpr) -> String {
    format!("__st_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local for `expr?` propagation — holds the `*MvlResult` pointer for
/// the tag check and branch.
fn propagate_temp_name(expr: &TirExpr) -> String {
    format!("__pr_{}_{}", expr.span.offset, expr.span.len)
}

/// Temp local name for the `*MvlResult` pointer stashed during a
/// `.unwrap_or(default)` invocation on a `Result[T, E]`. Same span-based
/// scheme as `mvl_option_temp_name`.
fn mvl_result_temp_name(expr: &TirExpr) -> String {
    format!("__mr_{}_{}", expr.span.offset, expr.span.len)
}

/// Peel `Ref` / `Labeled` / `Refined` wrappers and return the inner
/// `(key_ty, val_ty)` if `ty` is a `Map[K, V]`, else `None`.
fn map_key_val_ty(ty: &Ty) -> Option<(&Ty, &Ty)> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => cur = inner,
            Ty::Map(k, v) => return Some((k, v)),
            _ => return None,
        }
    }
}

/// Byte size of a WASM value with the given element type — used as the
/// `elem_size` argument to `_mvl_array_new`. Maps 1:1 to the [`wasm_ty`]
/// families:
///
///   `i32` (Bool / Byte / enum / collection ptr) → 4
///   `i64` (Int / UInt / …)                     → 8
///   `f64` (Float)                              → 8
fn elem_size_bytes(elem_ty: &Ty, ctx: &Ctx) -> u32 {
    if is_i32(elem_ty, ctx) {
        4
    } else {
        // Int, Float, and unsupported-so-far element types all end up here.
        // String elements need a `*MvlString` (i32) wrapper allocation per
        // element — deferred until Phase 3.2 / 3.3.
        8
    }
}

/// WASM push op name for an element type — one of
/// `_mvl_array_push_i32` / `_i64` / `_f64`. The typed variants pass the
/// value directly on the stack (no scratch alloc needed).
fn push_op_for(elem_ty: &Ty, ctx: &Ctx) -> &'static str {
    match wasm_ty(elem_ty, ctx) {
        "i32" => "$_mvl_array_push_i32",
        "f64" => "$_mvl_array_push_f64",
        _ => "$_mvl_array_push_i64",
    }
}

/// The inner element type of a `Ty::List/Array/Set`, or `None` if `ty`
/// is not a collection. Peels off `Ref` / `Labeled` / `Refined` wrappers
/// so `let xs: ref List[Int] = …` still resolves.
fn collection_elem_ty(ty: &Ty) -> Option<&Ty> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                cur = inner;
            }
            Ty::List(e) | Ty::Array(e, _) | Ty::Set(e) => return Some(e),
            _ => return None,
        }
    }
}

/// The payload type of a `Ty::Option`, or `None` if `ty` is not an
/// Option. Peels wrappers the same way as [`collection_elem_ty`].
fn option_inner_ty(ty: &Ty) -> Option<&Ty> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                cur = inner;
            }
            Ty::Option(t) => return Some(t),
            _ => return None,
        }
    }
}

/// Runtime accessor + constructor names for an `Option[T]` payload of
/// `inner_ty`. Returns `(some_ctor, value_getter)` where both are the
/// unprefixed runtime symbol names (no `$`).
///
/// The choice comes from [`wasm_ty`]: i32-typed payloads (Bool, Byte,
/// enum, collection ptr) use the i32 variants; everything else falls
/// back to i64 (Int, UInt, Float via bit-cast if needed later).
fn option_ops_for(inner_ty: &Ty, ctx: &Ctx) -> (&'static str, &'static str) {
    if is_i32(inner_ty, ctx) {
        ("_mvl_option_some_i32", "_mvl_option_value_i32")
    } else {
        ("_mvl_option_some_i64", "_mvl_option_value_i64")
    }
}

/// Extract the Ok-payload type from a `Result[T, E]` type, unwrapping
/// through `Ref` / `Labeled` / `Refined` wrappers.
fn result_ok_ty(ty: &Ty) -> Option<&Ty> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                cur = inner;
            }
            Ty::Result(ok, _) => return Some(ok),
            _ => return None,
        }
    }
}

/// Extract the Err-payload type from a `Result[T, E]` type.
fn result_err_ty(ty: &Ty) -> Option<&Ty> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
                cur = inner;
            }
            Ty::Result(_, err) => return Some(err),
            _ => return None,
        }
    }
}

/// Constructor and value-getter names for a `Result[T, E]` Ok payload of
/// `ok_ty`. Returns `(ok_ctor, value_getter)` — unprefixed runtime symbol
/// names (no `$`).
fn result_ops_for_ok(ok_ty: &Ty, ctx: &Ctx) -> (&'static str, &'static str) {
    if is_i32(ok_ty, ctx) {
        ("_mvl_result_ok_i32", "_mvl_result_value_i32")
    } else {
        ("_mvl_result_ok_i64", "_mvl_result_value_i64")
    }
}

/// Emit `assert_eq(a, b)` or `assert_ne(a, b)` — mirrors the LLVM backend's
/// `emit_assert_eq_builtin_tir` (#1837). Compares the two values with a
/// type-directed equality op, then traps via `unreachable` when the check
/// fails. `negate = true` traps on equality (i.e. `assert_ne`).
///
/// Strings route through `_mvl_string_eq` in `runtime/wasm/` — the emitter
/// imports it via `(import "runtime" ...)` when `Ctx::needs_runtime` is
/// set. Everything else stays inline (i64.eq / f64.eq / i32.eq).
fn emit_assert_eq(out: &mut String, left: &TirExpr, right: &TirExpr, negate: bool, ctx: &Ctx) {
    // String equality: both operands leave (ptr, len) on the stack (four
    // i32s total), then a runtime call reduces it to i32{0,1}. Same trap
    // logic wraps it below.
    if matches!(&left.ty, Ty::String) {
        ctx.needs_runtime.set(true);
        emit_expr(out, left, ctx);
        emit_expr(out, right, ctx);
        out.push_str("    call $_mvl_string_eq\n");
        if !negate {
            out.push_str("    i32.eqz\n");
        }
        out.push_str("    if\n      unreachable\n    end\n");
        return;
    }

    emit_expr(out, left, ctx);
    emit_expr(out, right, ctx);
    let eq_op = if is_float(&left.ty) {
        "f64.eq"
    } else if is_i32(&left.ty, ctx) {
        "i32.eq"
    } else {
        "i64.eq"
    };
    out.push_str(&format!("    {eq_op}\n"));
    // Normal assert_eq: trap when NOT equal. i32.eqz flips 1→0 (equal, skip)
    // and 0→1 (not equal, trap). assert_ne: trap when equal — omit the flip.
    if !negate {
        out.push_str("    i32.eqz\n");
    }
    out.push_str("    if\n      unreachable\n    end\n");
}

/// Emit a unary operator. `Neg` and `BitNot` dispatch on operand type; `Not`
/// is always Bool→Bool.
fn emit_unary(out: &mut String, op: UnaryOp, inner: &TirExpr, ctx: &Ctx) {
    match op {
        UnaryOp::Neg => {
            if is_float(&inner.ty) {
                emit_expr(out, inner, ctx);
                out.push_str("    f64.neg\n");
            } else {
                out.push_str("    i64.const 0\n");
                emit_expr(out, inner, ctx);
                out.push_str("    i64.sub\n");
            }
        }
        UnaryOp::Not => {
            emit_expr(out, inner, ctx);
            out.push_str("    i32.eqz\n");
        }
        UnaryOp::BitNot => {
            emit_expr(out, inner, ctx);
            out.push_str("    i64.const -1\n");
            out.push_str("    i64.xor\n");
        }
        UnaryOp::Deref => {
            emit_expr(out, inner, ctx);
            // No-op in this backend today — `ref` bindings and dereferences
            // are handled via WASM locals directly.
        }
    }
}

/// Emit a binary operator, picking i64/f64/i32 opcode family from operand type.
/// Short-circuit `&&` / `||` lower to an inline structured `if` for laziness.
fn emit_binary(out: &mut String, op: BinaryOp, left: &TirExpr, right: &TirExpr, ctx: &Ctx) {
    // String equality / inequality — both operands leave (ptr, len) on the
    // stack; `_mvl_string_eq` consumes all four i32s and returns i32.
    if matches!(&left.ty, Ty::String) && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
        ctx.needs_runtime.set(true);
        emit_expr(out, left, ctx);
        emit_expr(out, right, ctx);
        out.push_str("    call $_mvl_string_eq\n");
        if matches!(op, BinaryOp::Ne) {
            out.push_str("    i32.eqz\n"); // flip: 1 → 0, 0 → 1
        }
        return;
    }

    // Short-circuit boolean ops — need laziness, can't emit both operands up
    // front. `a && b` ≡ `if a then b else false`; `a || b` ≡ `if a then true else b`.
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        emit_expr(out, left, ctx);
        out.push_str("    if (result i32)\n");
        match op {
            BinaryOp::And => {
                emit_expr(out, right, ctx);
                out.push_str("    else\n      i32.const 0\n    end\n");
            }
            BinaryOp::Or => {
                out.push_str("      i32.const 1\n    else\n");
                emit_expr(out, right, ctx);
                out.push_str("    end\n");
            }
            _ => unreachable!(),
        }
        return;
    }

    emit_expr(out, left, ctx);
    emit_expr(out, right, ctx);
    // Pick opcode family from the operand type. Comparisons produce i32
    // regardless of operand type.
    let (family, is_cmp_operand_float) = if is_float(&left.ty) {
        ("f64", true)
    } else if is_i32(&left.ty, ctx) {
        ("i32", false)
    } else {
        ("i64", false)
    };
    let signed_suffix = if family == "i64" { "_s" } else { "" };
    let opcode: String = match op {
        BinaryOp::Add => format!("{family}.add"),
        BinaryOp::Sub => format!("{family}.sub"),
        BinaryOp::Mul => format!("{family}.mul"),
        BinaryOp::Div => {
            if is_cmp_operand_float {
                "f64.div".to_string()
            } else {
                format!("{family}.div{signed_suffix}")
            }
        }
        BinaryOp::Rem => format!("{family}.rem{signed_suffix}"),
        BinaryOp::Eq => format!("{family}.eq"),
        BinaryOp::Ne => format!("{family}.ne"),
        BinaryOp::Lt => format!("{family}.lt{signed_suffix}"),
        BinaryOp::Gt => format!("{family}.gt{signed_suffix}"),
        BinaryOp::Le => format!("{family}.le{signed_suffix}"),
        BinaryOp::Ge => format!("{family}.ge{signed_suffix}"),
        BinaryOp::BitAnd => format!("{family}.and"),
        BinaryOp::BitOr => format!("{family}.or"),
        BinaryOp::BitXor => format!("{family}.xor"),
        BinaryOp::Shl => format!("{family}.shl"),
        BinaryOp::Shr => format!("{family}.shr{signed_suffix}"),
        BinaryOp::And | BinaryOp::Or => unreachable!("short-circuited above"),
    };
    out.push_str(&format!("    {opcode}\n"));
}

fn wasm_ty(ty: &Ty, ctx: &Ctx) -> &'static str {
    match ty {
        Ty::Int | Ty::UInt => "i64",
        Ty::Float => "f64",
        Ty::Bool | Ty::Byte => "i32",
        Ty::Named(name, _) if ctx.enum_types.contains(name) => "i32",
        // Heap-allocated struct pointer (#1821).
        Ty::Named(name, _) if ctx.struct_layouts.contains_key(name.as_str()) => "i32",
        // Heap-allocated payload-enum pointer (#1821).
        Ty::Named(name, _) if ctx.payload_enums.contains_key(name.as_str()) => "i32",
        // Heap-allocated collection pointers: `*MvlArray` / `*MvlMap` are
        // opaque i32 addresses on the WASM stack. Element access is via
        // `_mvl_array_get(a, idx) -> i32` + a typed `i64.load` / `f64.load`.
        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_) | Ty::Map(_, _) => "i32",
        // `Option[T]` / `Result[T, E]` — heap-allocated MvlOption / MvlResult;
        // treated as opaque i32 pointer on the stack (#1821).
        Ty::Option(_) | Ty::Result(_, _) => "i32",
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => wasm_ty(inner, ctx),
        _ => "i64",
    }
}

/// True if this MVL type lowers to WASM `f64`.
fn is_float(ty: &Ty) -> bool {
    match ty {
        Ty::Float => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => is_float(inner),
        _ => false,
    }
}

/// True if this MVL type lowers to WASM `i32` (Bool, Byte, unit-variant
/// enums, heap pointers for structs/payload-enums/collections/Option/Result).
fn is_i32(ty: &Ty, ctx: &Ctx) -> bool {
    match ty {
        Ty::Bool | Ty::Byte => true,
        Ty::Named(name, _)
            if ctx.enum_types.contains(name)
                || ctx.struct_layouts.contains_key(name.as_str())
                || ctx.payload_enums.contains_key(name.as_str()) =>
        {
            true
        }
        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_) | Ty::Map(_, _) => true,
        Ty::Option(_) | Ty::Result(_, _) => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => is_i32(inner, ctx),
        _ => false,
    }
}

// ── Enum registry ───────────────────────────────────────────────────────
//
// Pre-scan `TirProgram.types` for enum declarations whose variants are all
// `Unit`. Those lower to a bare i32 discriminant on the WASM stack. Enums
// with any payload variant are excluded here — they use the heap-allocated
// tagged-union layout registered by `collect_payload_enums`.

fn collect_enums(
    types: &[TirTypeDecl],
) -> (std::collections::HashSet<String>, HashMap<String, i32>) {
    let mut enum_types = std::collections::HashSet::new();
    let mut variants = HashMap::new();
    for td in types {
        if let TirTypeBody::Enum(vs) = &td.body {
            if vs
                .iter()
                .all(|v| matches!(v.fields, TirVariantFields::Unit))
            {
                enum_types.insert(td.name.clone());
                for (idx, v) in vs.iter().enumerate() {
                    variants.insert(format!("{}::{}", td.name, v.name), idx as i32);
                }
            }
        }
    }
    (enum_types, variants)
}

// ── Struct layout collection (#1821) ────────────────────────────────────
//
// Pre-scan struct type declarations and compute a flat field layout for each.
// Layout rules:
//   Int / Float / UInt → 8 bytes (i64 / f64 store)
//   Bool / Byte        → 4 bytes (i32 store)
//   String             → 4 bytes (*MvlString pointer; unpacked on read)
//   all heap ptrs      → 4 bytes (i32: structs, payload enums, Option, Result,
//                                  collections)
// Fields are packed at their natural alignment (4 or 8 bytes). Total size is
// rounded up to 8-byte alignment so adjacent allocations don't share a word.

fn field_byte_size(ty: &Ty) -> u32 {
    match ty {
        Ty::Int | Ty::UInt | Ty::Float => 8,
        // Everything else is an i32-width value in the struct slot.
        _ => 4,
    }
}

fn field_alignment(ty: &Ty) -> u32 {
    field_byte_size(ty)
}

fn collect_structs(types: &[TirTypeDecl]) -> HashMap<String, StructLayout> {
    let mut map = HashMap::new();
    for td in types {
        if let TirTypeBody::Struct { fields, .. } = &td.body {
            let mut offset = 0u32;
            let mut slots = Vec::new();
            for f in fields {
                let size = field_byte_size(&f.ty);
                let align = field_alignment(&f.ty);
                // Align up.
                offset = (offset + align - 1) & !(align - 1);
                slots.push(FieldSlot {
                    name: f.name.clone(),
                    offset,
                    ty: f.ty.clone(),
                });
                offset += size;
            }
            // Round total to 8-byte boundary.
            let total = (offset + 7) & !7;
            map.insert(
                td.name.clone(),
                StructLayout {
                    total_size: total,
                    fields: slots,
                },
            );
        }
    }
    map
}

// ── Payload-enum layout collection (#1821) ──────────────────────────────
//
// Enums with at least one non-Unit variant get a heap-allocated layout:
//   { disc: i32, payload_ptr: i32 }   (8 bytes for the enum header)
//   payload area: N × 8 bytes         (one 8-byte slot per positional field)
//
// Unit variants within a payload enum still get the header layout (disc set,
// payload_ptr = 0). `collect_enums` already skipped these enums from the
// unit-discriminant path, so there's no double-registration.

fn collect_payload_enums(types: &[TirTypeDecl]) -> HashMap<String, PayloadEnumInfo> {
    let mut map = HashMap::new();
    for td in types {
        if let TirTypeBody::Enum(vs) = &td.body {
            // Skip pure-unit enums — those are handled by collect_enums.
            if vs
                .iter()
                .all(|v| matches!(v.fields, TirVariantFields::Unit))
            {
                continue;
            }
            let mut pvs = Vec::new();
            for (disc, v) in vs.iter().enumerate() {
                let fields: Vec<Ty> = match &v.fields {
                    TirVariantFields::Unit => vec![],
                    TirVariantFields::Tuple(tys) => tys.clone(),
                    TirVariantFields::Struct(fs) => fs.iter().map(|f| f.ty.clone()).collect(),
                };
                let payload_size = fields.iter().map(|_| 8u32).sum::<u32>();
                pvs.push(PayloadVariant {
                    name: format!("{}::{}", td.name, v.name),
                    disc: disc as i32,
                    fields,
                    payload_size,
                });
            }
            map.insert(td.name.clone(), PayloadEnumInfo { variants: pvs });
        }
    }
    map
}

// ── String-literal collection ────────────────────────────────────────────

/// Walk every user function and intern each distinct string literal at a
/// unique linear-memory offset starting at [`LITERAL_BASE`]. Returns the
/// interning table and the first free offset after all literals — used as
/// the initial value of the runtime's `$heap` global so bump allocations
/// don't overwrite the data section.
fn collect_literals(fns: &[&TirFn], needs_wasi: bool) -> (HashMap<String, (u32, u32)>, u32) {
    let mut map = HashMap::new();
    let mut next = LITERAL_BASE;
    // Seed "true" / "false" so `Bool.to_string()` has offsets to point at.
    // Cheap: 4 + 5 = 9 bytes of data section even when unused.
    if needs_wasi {
        for lit in &["true", "false"] {
            let len = lit.len() as u32;
            map.insert((*lit).to_string(), (next, len));
            next += len;
        }
    }
    for f in fns {
        collect_block(&f.body, &mut map, &mut next);
    }
    (map, next)
}

fn collect_block(block: &TirBlock, map: &mut HashMap<String, (u32, u32)>, next: &mut u32) {
    for stmt in &block.stmts {
        collect_stmt(stmt, map, next);
    }
}

fn collect_stmt(stmt: &TirStmt, map: &mut HashMap<String, (u32, u32)>, next: &mut u32) {
    match stmt {
        TirStmt::Expr { expr, .. } => collect_expr(expr, map, next),
        TirStmt::Return { value: Some(v), .. } => collect_expr(v, map, next),
        TirStmt::Let { init, .. } => collect_expr(init, map, next),
        TirStmt::Assign { value, .. } => collect_expr(value, map, next),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_expr(cond, map, next);
            collect_block(then, map, next);
            match else_ {
                Some(TirElseBranch::Block(b)) => collect_block(b, map, next),
                Some(TirElseBranch::If(s)) => collect_stmt(s, map, next),
                None => {}
            }
        }
        TirStmt::While { cond, body, .. } => {
            collect_expr(cond, map, next);
            collect_block(body, map, next);
        }
        TirStmt::For { iter, body, .. } => {
            collect_expr(iter, map, next);
            collect_block(body, map, next);
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, map, next);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_expr(e, map, next),
                    TirMatchBody::Block(b) => collect_block(b, map, next),
                }
            }
        }
        _ => {}
    }
}

fn collect_expr(expr: &TirExpr, map: &mut HashMap<String, (u32, u32)>, next: &mut u32) {
    match &expr.kind {
        TirExprKind::Literal(Literal::Str(s)) => {
            if !map.contains_key(s) {
                let len = s.len() as u32;
                map.insert(s.clone(), (*next, len));
                *next += len;
            }
        }
        TirExprKind::Unary { expr, .. } => collect_expr(expr, map, next),
        TirExprKind::Binary { left, right, .. } => {
            collect_expr(left, map, next);
            collect_expr(right, map, next);
        }
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_expr(a, map, next);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr(receiver, map, next);
            for a in args {
                collect_expr(a, map, next);
            }
        }
        TirExprKind::If { cond, then, else_ } => {
            collect_expr(cond, map, next);
            collect_block(then, map, next);
            if let Some(e) = else_ {
                collect_expr(e, map, next);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_expr(scrutinee, map, next);
            for arm in arms {
                // Literal String patterns are compared against the scrutinee
                // and need a data-section entry too.
                if let Pattern::Literal(Literal::Str(s), _) = &arm.pattern {
                    if !map.contains_key(s) {
                        let len = s.len() as u32;
                        map.insert(s.clone(), (*next, len));
                        *next += len;
                    }
                }
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_expr(e, map, next),
                    TirMatchBody::Block(b) => collect_block(b, map, next),
                }
            }
        }
        TirExprKind::Block(block) => collect_block(block, map, next),
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_expr(e, map, next);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_expr(k, map, next);
                collect_expr(v, map, next);
            }
        }
        _ => {}
    }
}

/// Escape a byte string for inclusion in a WAT `(data ...)` string literal.
/// WAT accepts `\n`, `\r`, `\t`, `\"`, `\\`, and `\XX` hex escapes.
fn escape_wat_data(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\{b:02x}")),
        }
    }
    out
}

// ── WASI preview 1 runtime blob ───────────────────────────────────────────

/// Build the WASI runtime prefix: fd_write import, memory + export, static
/// newline byte, string-literal data sections, bump-pointer global, alloc
/// helper, `mvl_int_to_string`, `mvl_println`.
///
/// Memory layout:
/// - `0..8`   iovec[0] {ptr, len}
/// - `8..16`  iovec[1] {ptr, len} (points at the newline byte)
/// - `16..20` `nwritten` output slot
/// - `20`     static `"\n"` byte
/// - `32..heap_start` string-literal contents (one `(data ...)` per literal)
/// - `heap_start..` bump-allocated string storage (used by `$mvl_int_to_string`)
fn emit_wasi_runtime(heap_start: u32, literals: &HashMap<String, (u32, u32)>) -> String {
    emit_wasi_runtime_common(heap_start, literals, /* own_memory */ true)
}

/// Same as [`emit_wasi_runtime`] but skips the `(memory 1) (export "memory")`
/// pair — the caller has already imported memory from `runtime/wasm/`.
fn emit_wasi_runtime_shared_memory(
    heap_start: u32,
    literals: &HashMap<String, (u32, u32)>,
) -> String {
    emit_wasi_runtime_common(heap_start, literals, /* own_memory */ false)
}

fn emit_wasi_runtime_common(
    heap_start: u32,
    literals: &HashMap<String, (u32, u32)>,
    own_memory: bool,
) -> String {
    let mut out = String::new();
    out.push_str(
        "  (import \"wasi_snapshot_preview1\" \"fd_write\"\n    \
         (func $fd_write (param i32 i32 i32 i32) (result i32)))\n",
    );
    if own_memory {
        out.push_str("  (memory 1)\n");
        out.push_str("  (export \"memory\" (memory 0))\n");
    }
    out.push_str("  (data (i32.const 20) \"\\n\")\n");

    // Emit string literals in ascending-offset order so the WAT is stable
    // across runs — HashMap iteration order isn't.
    let mut entries: Vec<(&String, u32, u32)> = literals
        .iter()
        .map(|(s, (off, len))| (s, *off, *len))
        .collect();
    entries.sort_by_key(|(_, off, _)| *off);
    for (content, offset, _len) in entries {
        out.push_str(&format!(
            "  (data (i32.const {offset}) \"{}\")\n",
            escape_wat_data(content)
        ));
    }

    out.push_str(&format!(
        "  (global $heap (mut i32) (i32.const {heap_start}))\n"
    ));
    out.push_str(WASI_HELPERS);
    out
}

/// The fixed part of the WASI runtime (allocator + string helpers). No
/// substitutions — memory layout constants match the diagram in
/// [`emit_wasi_runtime`].
const WASI_HELPERS: &str = r#"  (func $mvl_alloc (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))
  (func $mvl_int_to_string (param $n i64) (result i32 i32)
    (local $buf i32)
    (local $cur i32)
    (local $neg i32)
    (local.set $buf (call $mvl_alloc (i32.const 24)))
    (local.set $cur (i32.add (local.get $buf) (i32.const 24)))
    (if (i64.eqz (local.get $n))
      (then
        (local.set $cur (i32.sub (local.get $cur) (i32.const 1)))
        (i32.store8 (local.get $cur) (i32.const 48))
        (return (local.get $cur) (i32.const 1))))
    (local.set $neg (i32.const 0))
    (if (i64.lt_s (local.get $n) (i64.const 0))
      (then
        (local.set $neg (i32.const 1))
        (local.set $n (i64.sub (i64.const 0) (local.get $n)))))
    (block $done
      (loop $digit
        (br_if $done (i64.eqz (local.get $n)))
        (local.set $cur (i32.sub (local.get $cur) (i32.const 1)))
        (i32.store8
          (local.get $cur)
          (i32.add
            (i32.wrap_i64 (i64.rem_s (local.get $n) (i64.const 10)))
            (i32.const 48)))
        (local.set $n (i64.div_s (local.get $n) (i64.const 10)))
        (br $digit)))
    (if (local.get $neg)
      (then
        (local.set $cur (i32.sub (local.get $cur) (i32.const 1)))
        (i32.store8 (local.get $cur) (i32.const 45))))
    (local.get $cur)
    (i32.sub (i32.add (local.get $buf) (i32.const 24)) (local.get $cur)))
  ;; println / eprintln do TWO fd_write calls (one for the message, one
  ;; for the newline). The intuitive "one call with a 2-entry iovec"
  ;; shape silently drops iovec[1] on wasmtime 43.0.1 (verified against
  ;; the hand-written spike/006 reference too). Two calls are portable
  ;; and add one syscall — cheap tradeoff for a spike runtime.
  (func $mvl_println (param $ptr i32) (param $len i32)
    (i32.store (i32.const 0) (local.get $ptr))
    (i32.store (i32.const 4) (local.get $len))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))
    (i32.store (i32.const 0) (i32.const 20))
    (i32.store (i32.const 4) (i32.const 1))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8))))
  (func $mvl_eprintln (param $ptr i32) (param $len i32)
    (i32.store (i32.const 0) (local.get $ptr))
    (i32.store (i32.const 4) (local.get $len))
    (drop (call $fd_write (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 8)))
    (i32.store (i32.const 0) (i32.const 20))
    (i32.store (i32.const 4) (i32.const 1))
    (drop (call $fd_write (i32.const 2) (i32.const 0) (i32.const 1) (i32.const 8))))
"#;
