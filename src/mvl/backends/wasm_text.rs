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
//! - `stdout()` / `stderr()` / `stdin()` — heap-allocated `Fd { inner }`
//!   (1 / 2 / 0), `write(fd, msg)` — dynamic `fd_write` on the runtime `Fd`
//!   value, no trailing newline. `now()` / `_instant_epoch_seconds(t)` —
//!   WASI `clock_time_get` (real wall-clock reads, not faked). Together
//!   these unblock `std.log` (#2056) for the common stdout/stderr case.
//!   Arbitrary-fd `write`/`read` against `open()`-returned file descriptors
//!   is still unsupported (no WASI preopen wiring).
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
//! Also supported since this list was last accurate: structs and their fields,
//! payload enums / `Option` / `Result`, `List` / `Set` and their methods, string
//! equality, `MvlString` refcount + drops, and generic monomorphization.
//!
//! Deliberately not supported (later phases of #1817):
//! - Closures / higher-order fns
//! - `Map` beyond `Map[String, Int]`
//! - String concat (`_mvl_string_concat` wiring is incomplete)
//! - Arbitrary-fd `write`/`read`/`open` (real files, WASI preopens),
//!   `extern "wasm"` ABI — separate ticket
//!
//! Actors (#2012, ADR-0059): supported for spawn, behaviour sends, and
//! `pub test fn` synchronous reads. Single-threaded run-to-completion — the
//! mailbox and drain loop are emitted into the module (the `--preload` runtime
//! cannot call back into it), dispatch is a static switch on an actor type tag,
//! and `send` drains at the outermost call so per-actor FIFO holds and a
//! self-send queues instead of recursing. No parallelism, and no
//! `link`/`monitor`/`select`/`on_exit` supervision yet.

use std::cell::Cell;
use std::collections::HashMap;

use super::{AssertMode, Backend};
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{
    ArithOp, BinaryOp, CmpOp, GenericParam, LValue, Literal, LogicOp, Pattern, RefExpr,
    TirActorDecl, TirActorMethod, TirBlock, TirElseBranch, TirExpr, TirExprKind, TirFn,
    TirMatchArm, TirMatchBody, TirParam, TirProgram, TirStmt, TirTypeBody, TirTypeDecl,
    TirVariantFields, UnaryOp,
};
use crate::mvl::parser::lexer::Span;

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
    /// Declared field names, in the same order as `fields`, for struct-shaped
    /// variants (`Variant { field: Ty, .. }`) — empty for tuple/unit variants,
    /// where a `Pattern::Struct` field-name lookup never applies.
    field_names: Vec<String>,
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

/// Declaration-level `relabel name: A -> B audit` transitions, keyed by
/// transition name, from `TirProgram.relabel_decls` — mirrors the Rust/LLVM
/// backends' `audit_relabels` (#896). A call site whose `relabel name(...)`
/// omits the `audit` keyword still needs an audit event emitted if the
/// transition itself was declared `audit`.
type AuditRelabels = HashMap<String, (Option<String>, Option<String>)>;

/// One actor's emission metadata (#2012, ADR-0059).
///
/// The handle value on the WASM stack is just the actor's state pointer — the
/// same `i32` a struct of the same shape would be — so the state layout is
/// registered in `struct_layouts` under the actor's own name and field
/// reads/writes go through the ordinary struct path unchanged.
///
/// `tag` identifies the actor type inside a queued message, which is what lets
/// one drain loop dispatch a heterogeneous queue with direct calls instead of a
/// funcref table (ADR-0059 §2).
#[derive(Debug, Clone)]
struct ActorInfo {
    name: String,
    snake: String,
    tag: i32,
    /// Public non-test behaviours in declaration order; index = discriminant.
    behaviors: Vec<TirActorMethod>,
    /// `pub test fn` synchronous reads, dispatched as direct calls.
    test_methods: Vec<TirActorMethod>,
    /// Every method, including private helpers — needed to resolve an
    /// intra-actor call, which may target a non-public method (#2012).
    methods: Vec<TirActorMethod>,
}

/// Byte layout of one queued actor message. Mirrors the LLVM runtime's
/// `MvlMsg` (8 argument slots), plus the receiver and type tag that the
/// in-module drain loop needs (there is no runtime-side actor cell here).
const ACTOR_MSG_STATE: u32 = 0; // i32 — receiver state pointer
const ACTOR_MSG_TAG: u32 = 4; // i32 — actor type tag
const ACTOR_MSG_DISC: u32 = 8; // i32 — behaviour discriminant
const ACTOR_MSG_ARGS: u32 = 16; // 8 × i64 argument slots
const ACTOR_MAX_ARGS: u32 = 8;
const ACTOR_MSG_SIZE: u32 = ACTOR_MSG_ARGS + ACTOR_MAX_ARGS * 8;
/// Queue depth. Drain-at-send empties the queue at every statement boundary, so
/// this only has to cover the messages one behaviour chain enqueues. Overflow
/// traps rather than silently dropping (ADR-0059 §5).
const ACTOR_QUEUE_SLOTS: u32 = 256;

/// Shared per-emission context. Bundles the flags/tables threaded through
/// every emit_*  free function so their signatures stay stable as the
/// spike grows (or shrinks). Uses `Cell` for the label counter so the
/// context stays behind a `&`-reference — labels are module-wide unique,
/// which is stricter than WASM requires but simpler to bookkeep.
struct Ctx<'a> {
    needs_wasi: bool,
    /// Interned string literals: content → (linear-memory offset, byte length).
    literals: &'a HashMap<String, (u32, u32)>,
    /// Declaration-level mandatory-audit relabel transitions (#2013). See
    /// [`AuditRelabels`].
    audit_relabels: &'a AuditRelabels,
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
    /// Type alias targets: `type Foo = Bar where ...` → `Foo → Ty::Refined(Bar, ...)`.
    /// Used by `wasm_ty` / `is_float` to resolve named aliases to their base WASM type.
    type_aliases: &'a HashMap<String, Ty>,
    /// Type parameter substitution for generic function monomorphization.
    /// E.g. when emitting `identity[T=Int]`, contains `{"T": Ty::Int}`.
    /// Empty map for non-generic functions.
    type_subst: &'a HashMap<String, Ty>,
    /// Generic function name → (type_params, fn_params) for call-site name mangling.
    generic_fn_map: &'a HashMap<String, (Vec<GenericParam>, Vec<TirParam>)>,
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
    /// All locals collected for the current function body.
    /// Set by `emit_fn` before body emission; read by `emit_stmt(Return)`
    /// to emit drops on explicit-return paths and by loop back-edges.
    fn_locals: std::cell::RefCell<Vec<(String, Ty)>>,
    /// `let NAME = INIT` bindings for the current function body, name →
    /// initializer expression. Set by `emit_fn`/`emit_extension_method`
    /// before body emission; read by `exclude_returned_locals` so a
    /// `return name` can trace back to the heap-owning temp `name`'s
    /// initializer actually materialized (#2023/#2052's one-`let`-removed
    /// case).
    fn_let_inits: std::cell::RefCell<HashMap<String, TirExpr>>,
    /// Actor metadata by actor type name (#2012). Empty for programs with no
    /// actors, which is what keeps the actor scheduler out of every module.
    actors: &'a HashMap<String, ActorInfo>,
    /// Type name bound to `self` while emitting an actor method body — the only
    /// way `self.field = …` can find its layout, since an `LValue` carries no
    /// type (#2012).
    self_type: std::cell::RefCell<Option<String>>,
    /// `(receiver_type, method)` pairs with a user-defined, non-generic extension
    /// method emitted by `emit_extension_method` (#2054). Consulted by the
    /// `MethodCall` dispatch chain after every builtin-type special case has
    /// missed, so a call to a method on the user's own struct routes to
    /// `${receiver_type}_${method}` instead of falling to `;; unsupported`.
    struct_methods: &'a std::collections::HashSet<(String, String)>,
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
    // `.split(sep)` — two (ptr, len) pairs in, `*MvlArray` out (#2014). The
    // odd one out in Group B: the result is an array of `*MvlString`, not a
    // `*MvlString`, so the call site does *not* run `emit_unpack_mvl_string`
    // and the value stays a bare pointer. Ownership falls out of
    // `local_drop_fn`, which already maps a `List[String]` local to
    // `_mvl_string_ptr_array_drop`.
    ("_mvl_string_split", "(param i32 i32 i32 i32) (result i32)"),
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
    ("_mvl_string_ptr_array_drop", "(param i32)"),
    ("_mvl_string_ptr_array_dedup", "(param i32)"),
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
    // Map[String, String] get — clones the returned `*MvlString` handle so
    // the caller can drop it independently of the map's own copy (#2047).
    ("_mvl_map_get_str", "(param i32 i32 i32) (result i32)"),
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
    // Non-String Err payloads (#2066) — mirrors the Ok i64/i32 pair above.
    ("_mvl_result_err_i64", "(param i64) (result i32)"),
    ("_mvl_result_err_i32", "(param i32) (result i32)"),
    ("_mvl_result_tag", "(param i32) (result i32)"),
    ("_mvl_result_value_i64", "(param i32) (result i64)"),
    ("_mvl_result_value_i32", "(param i32) (result i32)"),
    ("_mvl_result_drop", "(param i32)"),
    // Group H — String parse ops. Take raw (ptr, len) byte slice; return
    // heap-allocated MvlResult pointer.
    ("_mvl_string_parse_int", "(param i32 i32) (result i32)"),
    // std.io::read_file / _read_file (#2076) — WASI file read via
    // wasm32-wasip1's `std::fs`, no hand-rolled `path_open`/`fd_read`
    // imports needed (unlike #2056's write-side `fd_write`). Takes the
    // path as a raw (ptr, len) byte slice; returns a heap-allocated
    // MvlResult (Ok(*MvlString) / Err(*IoError header)).
    ("_mvl_io_read_file", "(param i32 i32) (result i32)"),
    // Group I — IFC audit event (#2013). Five (ptr, len) string pairs:
    // transition, from_label, to_label, tag, location. No return value.
    (
        "_mvl_audit_emit_relabel",
        "(param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)",
    ),
    // Group J — Float.to_string() and the format() builtin (#2039). Both
    // return `*MvlString`; the emitter unpacks `.ptr`/`.len` immediately
    // after the call, same as `_mvl_string_new` (Group B).
    ("_mvl_float_to_string", "(param f64) (result i32)"),
    ("_mvl_format", "(param i32 i32 i32) (result i32)"),
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

        // All TirFn entries, including generics — needed for monomorphization lookup.
        let all_fns: Vec<&TirFn> = tir
            .fns
            .iter()
            .filter(|f| !f.is_builtin && f.receiver_type.is_none())
            .collect();

        // User-defined, non-generic extension methods (`fn Type::method(self, ...)`)
        // (#2054). These carry a `receiver_type` and were previously dropped from
        // every emission path — neither `fns`/`all_fns` above (which require
        // `receiver_type.is_none()`) nor the generic-instantiation path (which
        // requires `type_params` non-empty) ever saw them.
        let ext_methods: Vec<&TirFn> = tir
            .fns
            .iter()
            .filter(|f| !f.is_builtin && f.type_params.is_empty() && f.receiver_type.is_some())
            .collect();
        let struct_methods: std::collections::HashSet<(String, String)> = ext_methods
            .iter()
            .map(|f| {
                (
                    f.receiver_type.clone().expect("filtered above"),
                    f.name.clone(),
                )
            })
            .collect();

        // A Unit-returning `main` becomes the WASI `_start` entry point.
        // When present we emit the WASI runtime blob (memory, fd_write import,
        // bump allocator, int-to-string, println).
        let needs_wasi = fns
            .iter()
            .any(|f| f.name == "main" && matches!(f.ret_ty, Ty::Unit));

        // Declaration-level mandatory-audit relabel transitions (#2013) —
        // mirrors Rust/LLVM's `audit_relabels` (#896). A call site's own
        // `relabel name(...) audit` keyword is OR'd with this at the
        // Relabel emit site, so a transition declared `audit` gets an audit
        // event even when call sites omit the keyword.
        let audit_relabels: AuditRelabels = tir
            .relabel_decls
            .iter()
            .filter(|rd| rd.audit)
            .map(|rd| (rd.name.clone(), (rd.from.clone(), rd.to.clone())))
            .collect();

        // Extension-method bodies are emitted separately from `fns` (see
        // `ext_methods` above) but still need their string literals interned
        // here — otherwise a literal inside one emits as `;; missing literal`
        // with nothing pushed to the stack (#2058 follow-up).
        let literal_scan_fns: Vec<&TirFn> = fns.iter().chain(ext_methods.iter()).copied().collect();
        let (literals, heap_start) =
            collect_literals(&literal_scan_fns, &tir.actors, needs_wasi, &audit_relabels);
        let (enum_types, enum_variants) = collect_enums(&tir.types);
        let mut struct_layouts = collect_structs(&tir.types);
        // Actor state layouts land in `struct_layouts` so the handle behaves
        // like a struct pointer everywhere (#2012).
        let actors = collect_actors(&tir.actors, &mut struct_layouts);
        let struct_layouts = struct_layouts;
        let payload_enums = collect_payload_enums(&tir.types);
        let type_aliases = collect_type_aliases(&tir.types);
        let empty_subst: HashMap<String, Ty> = HashMap::new();
        let generic_fn_map: HashMap<String, (Vec<GenericParam>, Vec<TirParam>)> = all_fns
            .iter()
            .filter(|f| !f.type_params.is_empty())
            .map(|f| (f.name.clone(), (f.type_params.clone(), f.params.clone())))
            .collect();
        let ctx = Ctx {
            needs_wasi,
            literals: &literals,
            audit_relabels: &audit_relabels,
            enum_types: &enum_types,
            enum_variants: &enum_variants,
            struct_layouts: &struct_layouts,
            payload_enums: &payload_enums,
            type_aliases: &type_aliases,
            type_subst: &empty_subst,
            generic_fn_map: &generic_fn_map,
            label_counter: Cell::new(0),
            needs_runtime: Cell::new(false),
            string_params: std::cell::RefCell::new(std::collections::HashSet::new()),
            assert_mode: self.assert_mode,
            fn_locals: std::cell::RefCell::new(Vec::new()),
            fn_let_inits: std::cell::RefCell::new(HashMap::new()),
            actors: &actors,
            self_type: std::cell::RefCell::new(None),
            struct_methods: &struct_methods,
        };

        // Collect unique generic-function instantiations needed by the corpus fns.
        let instantiations = collect_generic_instantiations(&fns, &all_fns, &tir.actors, &ctx);

        // Emit fns into a scratch buffer first — `emit_assert_eq` on
        // String flips `ctx.needs_runtime`, and we only know whether to
        // import `runtime` memory + symbols after the whole body has been
        // walked. Fn bodies are self-contained, so buffering is cheap.
        let mut fns_out = String::new();

        // Emit monomorphized copies of generic functions before the regular fns.
        for (generic_fn, type_subst, mangled) in &instantiations {
            emit_generic_fn(&mut fns_out, generic_fn, type_subst, mangled, &ctx);
        }

        // Actor behaviours, dispatch, and the in-module scheduler (#2012).
        if !actors.is_empty() {
            emit_actor_decls(&mut fns_out, &tir.actors, &ctx);
            emit_actor_scheduler(&mut fns_out, &ctx);
        }

        for f in &fns {
            emit_fn(&mut fns_out, f, &ctx);
        }

        // User-defined extension methods on custom structs (#2054) — emitted
        // alongside plain fns, not exported (mirrors actor methods above).
        for f in &ext_methods {
            emit_extension_method(&mut fns_out, f, &ctx);
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
    let t = block_trailing_ty(then, ctx)?;
    let e = match else_ {
        Some(TirElseBranch::Block(b)) => block_trailing_ty(b, ctx)?,
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

/// Type of a block's trailing statement, if it leaves a non-Unit value on
/// the stack. Used to decide if a `TirStmt::If`'s branches (or a match
/// arm's block body) leave a value behind that the enclosing block-type
/// needs to declare.
///
/// Recurses into a trailing `TirStmt::If`/`TirStmt::Match` — not just a bare
/// `TirStmt::Expr` — because the TIR lowerer emits those for what reads as a
/// trailing expression (e.g. a match arm body `{ if c { A } else { B } }`).
/// Without this, a block-bodied arm whose tail is itself an if/else chain or
/// nested match silently reports no result type, and the *enclosing* match's
/// `if`s all end up emitted without `(result ...)` even though every arm
/// still leaves a value on the stack — a WASM validator stack-imbalance
/// (#2053), not a MVL-level bug.
fn block_trailing_ty(block: &TirBlock, ctx: &Ctx) -> Option<Ty> {
    let last = block.stmts.last()?;
    match last {
        TirStmt::Expr { expr, .. } if !matches!(expr.ty, Ty::Unit) => Some(expr.ty.clone()),
        TirStmt::If { then, else_, .. } => if_stmt_result_ty(then, else_, ctx),
        TirStmt::Match { arms, .. } => match_arms_result_ty(arms, ctx),
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
        RefExpr::BoundedForall { .. }
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
        RefExpr::ArithOp {
            op, left, right, ..
        } => {
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
        RefExpr::Compare {
            op, left, right, ..
        } => {
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
        RefExpr::LogicOp {
            op, left, right, ..
        } => {
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

/// Returns the WAT drop-function name for a heap-owning local, or `None`
/// if the local does not hold an allocation that requires a manual drop call.
/// Mirrors the logic in the implicit-return drop loop inside `emit_fn`.
fn local_drop_fn(name: &str, ty: &Ty) -> Option<&'static str> {
    if name.starts_with("__ms_") {
        Some("_mvl_string_drop")
    } else if name.starts_with("__mo_") || name.starts_with("__mr_") {
        // `.unwrap_or(default)`'s own temp (Option or Result) is already
        // dropped inline, immediately after the if/else that extracts its
        // payload — see the `unwrap_or` emitters above. Matching it here
        // too double-drops the box: harmless-looking UB for `Option`
        // (`_mvl_option_drop`'s second `Box::from_raw` doesn't trip any
        // check), but for `Result` the second drop reads freed memory that
        // can look like a stale `Err` and fires a spurious
        // `_mvl_string_drop` on a garbage pointer — a real crash (#2024).
        None
    } else if name.starts_with("__pr_") {
        // `expr?`'s temp has no inline drop on the Ok path (the Err path
        // returns the box to the caller instead) — this catch-all is its
        // only cleanup.
        Some("_mvl_result_drop")
    } else if name.starts_with("__match_") && option_inner_ty(ty).is_some() {
        Some("_mvl_option_drop")
    } else if name.starts_with("__match_") && result_ok_ty(ty).is_some() {
        Some("_mvl_result_drop")
    } else if !name.starts_with("__") && option_inner_ty(ty).is_some() {
        Some("_mvl_option_drop")
    } else if !name.starts_with("__") && result_ok_ty(ty).is_some() {
        Some("_mvl_result_drop")
    } else if !name.starts_with("__")
        && collection_elem_ty(ty).map(peels_to_string).unwrap_or(false)
    {
        Some("_mvl_string_ptr_array_drop")
    } else if !name.starts_with("__")
        && collection_elem_ty(ty).is_some()
        && collection_elem_ty(ty)
            .map(|e| !peels_to_string(e))
            .unwrap_or(true)
    {
        Some("_mvl_array_drop")
    } else if !name.starts_with("__") && matches!(map_key_val_ty(ty), Some((Ty::String, _))) {
        Some("_mvl_map_drop_si64")
    } else {
        None
    }
}

/// Emit `local.get $name; call $drop_fn` for every heap-owning local,
/// skipping any named in `excludes` (the value(s) being returned — plural
/// because `return name` may chase back through one or more `let` bindings
/// to the temp that actually owns the heap allocation, see
/// `exclude_returned_locals`). All drop functions are null-safe, so
/// uninitialized locals (value = 0) are harmless no-ops.
fn emit_fn_heap_drops(out: &mut String, locals: &[(String, Ty)], excludes: &[String]) {
    for (name, ty) in locals {
        if excludes.iter().any(|ex| ex == name) {
            continue;
        }
        if let Some(drop_fn) = local_drop_fn(name, ty) {
            out.push_str(&format!("    local.get ${name}\n"));
            out.push_str(&format!("    call ${drop_fn}\n"));
        }
    }
}

/// (from_label, to_label) display strings for a relabel transition (#2013).
/// Mirrors LLVM's `relabel_label_strings_tir` fallback table — the WASM
/// emitter has no per-module `audit_relabels` declaration registry, so this
/// covers the built-in relabel names used by the corpus. Unknown names fall
/// back to `("_", "_")`, same as LLVM's default arm.
/// `audit_relabels` (declared transitions) takes precedence over the
/// built-in table — mirrors LLVM's `relabel_label_strings_tir`, which
/// prefers `self.module.audit_relabels.get(name)` before falling back to
/// its own hardcoded match.
fn relabel_label_strings(name: &str, audit_relabels: &AuditRelabels) -> (String, String) {
    if let Some((from, to)) = audit_relabels.get(name) {
        return (
            from.clone().unwrap_or_else(|| "_".to_string()),
            to.clone().unwrap_or_else(|| "_".to_string()),
        );
    }
    let (f, t) = match name {
        "classify" => ("_", "Secret"),
        "taint" => ("_", "Tainted"),
        "trust" => ("Tainted", "_"),
        "release" => ("Secret", "_"),
        "config_path" => ("_", "ConfigPath"),
        "unconfig_path" => ("ConfigPath", "_"),
        "db_url" => ("_", "DbUrl"),
        "undb_url" => ("DbUrl", "_"),
        "api_endpoint" => ("_", "ApiEndpoint"),
        "unapi_endpoint" => ("ApiEndpoint", "_"),
        "audit_target" => ("_", "AuditTarget"),
        "unaudit_target" => ("AuditTarget", "_"),
        _ => ("_", "_"),
    };
    (f.to_string(), t.to_string())
}

/// Push a literal string's `(offset, len)` as two `i32.const` operands, for
/// strings that aren't `TirExprKind::Literal(Literal::Str)` nodes but were
/// still registered in `ctx.literals` by `collect_expr` (e.g. relabel audit
/// metadata strings, #2013). Empty strings need no data-section entry —
/// `(ptr=0, len=0)` mirrors `slice_or_empty`'s null-pointer handling.
fn emit_literal_str_operands(out: &mut String, s: &str, ctx: &Ctx) {
    if s.is_empty() {
        out.push_str("    i32.const 0\n");
        out.push_str("    i32.const 0\n");
    } else if let Some(&(offset, len)) = ctx.literals.get(s) {
        out.push_str(&format!("    i32.const {offset}\n"));
        out.push_str(&format!("    i32.const {len}\n"));
    } else {
        out.push_str(&format!("    ;; missing literal: {s:?}\n"));
    }
}

/// True if `ty` is a `String`, possibly wrapped in `Ref`/`Labeled`/`Refined`
/// layers — e.g. `Tainted[String]`, `Secret[String]`, `ref String` (#2013).
/// IFC labels are compile-time-only wrappers; a `Secret[String]` fn param or
/// let-binding needs the same split `(ptr, len)` WASM representation as a
/// bare `String`. Ctx-free mirror of `is_string_ty`'s `Ref | Labeled |
/// Refined` peel, usable from `collect_locals_stmt` (runs before `Ctx`
/// exists) and from `emit_fn`'s param/return-type checks.
fn peels_to_string(ty: &Ty) -> bool {
    match ty {
        Ty::String => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => peels_to_string(inner),
        _ => false,
    }
}

/// Extract the fn-local name(s) holding a `return expr` expression's value
/// so they can be excluded from heap drops (they must survive for the
/// caller). Mirrors LLVM's `exclude_returned_value_tir` for the
/// `Var`/`Consume`/`Relabel` cases, but also has to cover WASM-specific
/// temps: allocation-returning String methods, `Float.to_string()`, and
/// `format(...)` materialize their `*MvlString` result into a span-keyed
/// `__ms_*` local (`collect_locals_expr`, `mvl_string_temp_name`) rather
/// than a named binding. When the return expression *is* one of these
/// calls directly, that temp — not any named `Var` — is the value being
/// returned, and the drop sweep must skip it instead of freeing it out
/// from under the caller (#2023, #2052).
///
/// A `Var(name)` alone is not enough: `let s: String = a.concat(b); return
/// s;` returns `Var("s")`, but `s` (split into `s_ptr`/`s_len` locals) is
/// never itself drop-tracked — the actual heap owner is the `__ms_*` temp
/// created when `a.concat(b)` was unpacked into `(ptr, len)` during the
/// `let`. Excluding only `"s"` protects nothing, and the blanket drop sweep
/// still frees that temp out from under the return value (same UAF class as
/// #2023/#2052, one `let` removed). So `Var(name)` also looks `name` up in
/// `ctx.fn_let_inits` — the whole function's `let`-bindings, collected once
/// up front — and recurses into the initializer, chasing `let a = ...; let
/// b = a; return b;` chains the same way.
fn exclude_returned_locals(expr: &TirExpr, ctx: &Ctx) -> Vec<String> {
    let mut out = Vec::new();
    exclude_returned_locals_into(expr, ctx, &mut out);
    out
}

fn exclude_returned_locals_into(expr: &TirExpr, ctx: &Ctx, out: &mut Vec<String>) {
    match &expr.kind {
        TirExprKind::Var(name) => {
            out.push(name.clone());
            if let Some(init) = ctx.fn_let_inits.borrow().get(name).cloned() {
                exclude_returned_locals_into(&init, ctx, out);
            }
        }
        TirExprKind::Consume(inner) => exclude_returned_locals_into(inner, ctx, out),
        TirExprKind::Relabel { expr: inner, .. } => exclude_returned_locals_into(inner, ctx, out),
        TirExprKind::MethodCall {
            receiver, method, ..
        } if peels_to_string(&receiver.ty)
            && matches!(
                method.as_str(),
                "concat" | "substring" | "to_upper" | "to_lower" | "trim" | "replace"
            ) =>
        {
            out.push(mvl_string_temp_name(expr));
        }
        TirExprKind::MethodCall {
            receiver, method, ..
        } if matches!(receiver.ty, Ty::Float) && method == "to_string" => {
            out.push(mvl_string_temp_name(expr));
        }
        TirExprKind::FnCall { name, args, .. } if name == "format" && args.len() == 2 => {
            out.push(mvl_string_temp_name(expr));
        }
        _ => {}
    }
}

/// Populate `ctx.fn_let_inits` with every `let NAME = INIT` binding in
/// `body`, recursing into nested blocks (`if`/`else`, `while`, `match` arms)
/// so a `let` above a conditional `return` is still found. Best-effort by
/// name, like the rest of this module's span/name-keyed lookups — a
/// shadowed name across sibling scopes can resolve to the wrong initializer,
/// but the failure mode is a missed exclusion (leak) or a spurious one
/// (also just a leak), never a new UAF, since this only ever adds
/// exclusions on top of the existing direct-expression cases above.
fn collect_let_inits_block(block: &TirBlock, map: &mut HashMap<String, TirExpr>) {
    for stmt in &block.stmts {
        collect_let_inits_stmt(stmt, map);
    }
}

fn collect_let_inits_stmt(stmt: &TirStmt, map: &mut HashMap<String, TirExpr>) {
    match stmt {
        TirStmt::Let {
            pattern: Pattern::Ident(name, _),
            init,
            ..
        } => {
            map.insert(name.clone(), init.clone());
        }
        TirStmt::If { then, else_, .. } => {
            collect_let_inits_block(then, map);
            match else_ {
                Some(TirElseBranch::Block(b)) => collect_let_inits_block(b, map),
                Some(TirElseBranch::If(nested)) => collect_let_inits_stmt(nested, map),
                None => {}
            }
        }
        TirStmt::While { body, .. } => collect_let_inits_block(body, map),
        TirStmt::Match { arms, .. } => {
            for arm in arms {
                if let TirMatchBody::Block(b) = &arm.body {
                    collect_let_inits_block(b, map);
                }
            }
        }
        _ => {}
    }
}

fn emit_fn(out: &mut String, f: &TirFn, ctx: &Ctx) {
    // Update the per-function String-param registry so that `Var` accesses
    // to these params emit two `local.get` ops instead of one.
    *ctx.string_params.borrow_mut() = f
        .params
        .iter()
        .filter(|p| peels_to_string(&p.ty))
        .map(|p| p.name.clone())
        .collect();

    let (wasm_name, _) = effective_name(f, ctx.needs_wasi);
    // Populate the per-function String-param set so the Var emitter knows
    // which String locals are split (ptr, len) params vs unsupported locals.
    {
        let mut sp = ctx.string_params.borrow_mut();
        sp.clear();
        for p in &f.params {
            if peels_to_string(&p.ty) {
                sp.insert(p.name.clone());
            }
        }
    }

    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        if peels_to_string(&p.ty) {
            // String (or Secret[String]/Tainted[String] — #2013, labels are
            // compile-time only) params split into two i32 WASM params: (ptr, len).
            out.push_str(&format!(
                " (param ${}_ptr i32) (param ${}_len i32)",
                p.name, p.name
            ));
        } else {
            out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty, ctx)));
        }
    }
    if peels_to_string(&f.ret_ty) {
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
        && !matches!(f.ret_ty, Ty::Unit)
        && !peels_to_string(&f.ret_ty)
        && (f.ensures.iter().any(is_runtime_checkable)
            || f.return_refinement
                .as_ref()
                .is_some_and(is_runtime_checkable));
    if has_checkable_ensures {
        locals.push(("__result_CONTRACT".to_string(), f.ret_ty.clone()));
    }

    // Deduplicate (collect passes may register the same name from nested
    // expressions or speculative String locals; WAT rejects duplicates).
    dedup_locals_keep_last(&mut locals);
    for (name, ty) in &locals {
        body.push_str(&format!("    (local ${} {})\n", name, wasm_ty(ty, ctx)));
    }

    // Emit `requires` precondition checks at function entry (#1822).
    if ctx.assert_mode != AssertMode::Assume {
        for pred in &f.requires {
            emit_contract_check(&mut body, pred, "", "i64", &f.params, ctx.assert_mode);
        }
    }

    // Publish the locals list so emit_stmt(Return) can emit drops on
    // explicit-return paths without threading locals through every call.
    *ctx.fn_locals.borrow_mut() = locals.clone();
    // Publish this function's `let` bindings so `exclude_returned_locals`
    // can chase a returned `Var(name)` back to its initializer (#2023,
    // #2052's one-`let`-removed case).
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&f.body, &mut let_inits);
    *ctx.fn_let_inits.borrow_mut() = let_inits;

    emit_block(&mut body, &f.body, ctx);

    // Emit `ensures` / return_refinement checks before implicit return (#1822).
    // We save the implicit-return expression into $__result_CONTRACT, run the
    // checks, then push it back. Explicit `return` mid-function bypasses these
    // checks — acceptable for the corpus tests, all of which use implicit return.
    if has_checkable_ensures {
        let ret_wasm = wasm_ty(&f.ret_ty, ctx);
        body.push_str("    local.set $__result_CONTRACT\n");
        for pred in f.ensures.iter().chain(f.return_refinement.as_ref()) {
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
        // Emit heap drops for the implicit-return path. All drop functions
        // are null-safe, so locals that were never initialised (value = 0)
        // are harmless no-ops. Explicit-return paths emit their own drops
        // via `emit_stmt(Return)` before the `return` instruction.
        //
        // Note: `__ma_*` (MvlArray literal temps) are intentionally absent
        // from `local_drop_fn` — they alias the same pointer as the
        // user-bound list local. Dropping both would double-free.
        //
        // A trailing bare-expression statement (no `return` keyword) is the
        // fn's implicit return value — same exclusion rule as the explicit
        // path, needed so e.g. `fn f(a, b) -> String { a.concat(b) }` doesn't
        // free its own `*MvlString` result before the caller reads it
        // (#2023, #2052).
        let implicit_excludes = match f.body.stmts.last() {
            Some(TirStmt::Expr { expr, .. }) => exclude_returned_locals(expr, ctx),
            _ => Vec::new(),
        };
        emit_fn_heap_drops(out, &locals, &implicit_excludes);
    }
    out.push_str("  )\n");
}

// ── Local collection ─────────────────────────────────────────────────────

/// Deduplicate a locals list, keeping each name's LAST-pushed entry — the
/// ctx-aware second pass (`collect_locals_ctx`) may push a corrected type
/// for a name the ctx-unaware first pass already declared with a wrong
/// placeholder type (`correct_payload_pattern_locals`, #2073); its pushes
/// must win, not the earlier ones. Genuine duplicates from speculative
/// String-local scaffolding are idempotent either way, so "keep last" never
/// regresses those.
fn dedup_locals_keep_last(locals: &mut Vec<(String, Ty)>) {
    let mut seen = std::collections::HashSet::new();
    let mut rev: Vec<(String, Ty)> = locals
        .drain(..)
        .rev()
        .filter(|(name, _)| seen.insert(name.clone()))
        .collect();
    rev.reverse();
    *locals = rev;
}

fn collect_locals_block(block: &TirBlock, locals: &mut Vec<(String, Ty)>) {
    for s in &block.stmts {
        collect_locals_stmt(s, locals);
    }
}

/// Correct a match arm's payload-pattern-bound local types using
/// `ctx.payload_enums` field metadata. The ctx-unaware first pass
/// (`collect_match_arm_locals`) can't resolve a `TupleStruct`/`Struct`
/// field's real type by slot — no `ctx` in scope there — so it declares
/// every such binding as `Ty::Int` regardless of its actual shape (#2073).
/// This runs in the ctx-aware second pass below; its pushes land after the
/// first pass's in `locals`, so the "keep last occurrence" dedup in
/// `emit_fn` lets them win and replace the wrong entries. String fields are
/// skipped — those already declare correctly-shaped `_ptr`/`_len`/`__sv_*`
/// placeholder locals from the first pass, keyed by name, not by type.
fn correct_payload_pattern_locals(
    pattern: &Pattern,
    scrutinee_ty: &Ty,
    locals: &mut Vec<(String, Ty)>,
    ctx: &Ctx,
) {
    let (target, enum_ty): (&Pattern, Option<&Ty>) = match pattern {
        Pattern::TupleStruct { .. } | Pattern::Struct { .. } => (pattern, Some(scrutinee_ty)),
        Pattern::Err { inner, .. } => (inner.as_ref(), result_err_ty(scrutinee_ty)),
        Pattern::Ok { inner, .. } => (inner.as_ref(), result_ok_ty(scrutinee_ty)),
        _ => return,
    };
    let Some(enum_ty) = enum_ty else { return };
    let Some(type_name) = underlying_named_ty(enum_ty, ctx) else {
        return;
    };
    let Some(info) = ctx.payload_enums.get(&type_name) else {
        return;
    };

    match target {
        Pattern::TupleStruct { name, fields, .. } => {
            let Some(pv) = info.variants.iter().find(|v| v.name == *name) else {
                return;
            };
            for (slot, pat) in fields.iter().enumerate() {
                if let Pattern::Ident(n, _) = pat {
                    if n != "_" && !n.contains("::") {
                        if let Some(field_ty) = pv.fields.get(slot) {
                            if !peels_to_string(field_ty) {
                                locals.push((n.clone(), field_ty.clone()));
                            }
                        }
                    }
                }
            }
        }
        Pattern::Struct {
            name,
            fields: named_fields,
            ..
        } => {
            let Some(pv) = info.variants.iter().find(|v| v.name == *name) else {
                return;
            };
            for (slot, fname) in pv.field_names.iter().enumerate() {
                let Some((_, pat)) = named_fields.iter().find(|(n, _)| n == fname) else {
                    continue;
                };
                if let Pattern::Ident(bound, _) = pat {
                    if bound != "_" && !bound.contains("::") {
                        if let Some(field_ty) = pv.fields.get(slot) {
                            if !peels_to_string(field_ty) {
                                locals.push((bound.clone(), field_ty.clone()));
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

// ── ctx-aware local scan (#1821) ─────────────────────────────────────────
//
// A second pass over the function body that requires `ctx` to identify:
//  - Payload-enum unit-variant `Var` expressions → `__ev_<off>` (i32)
//  - String-field `FieldAccess` reads → `__sf_<off>_<len>` (i32)
//  - Match-arm payload-pattern field locals whose real type needs a
//    payload_enums lookup (#2073) — see `correct_payload_pattern_locals`.
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
        TirStmt::Assign { target, value, .. } => {
            // `base.field = …` on a String field needs a scratch local to hold
            // the new handle while the old one is dropped (see
            // `emit_field_assign`). Registered unconditionally — an unused
            // local is free, and resolving the field's type here would mean
            // duplicating the base-type lookup.
            if matches!(target, LValue::Field { .. }) {
                locals.push((field_assign_temp_name(value), Ty::Bool)); // i32 placeholder
            }
            collect_locals_ctx_expr(value, locals, ctx)
        }
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
                correct_payload_pattern_locals(&arm.pattern, &scrutinee.ty, locals, ctx);
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
                        if peels_to_string(&slot.ty) {
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
        // `actor Name { … }` — state-pointer temp, same shape as a struct
        // construct's `__st_*` (#2012).
        TirExprKind::Spawn { fields, .. } => {
            locals.push((struct_temp_name(expr), Ty::Bool)); // i32 placeholder
            for (_, e) in fields {
                collect_locals_ctx_expr(e, locals, ctx);
            }
        }
        // Wrappers that emit their inner expression unchanged. Without these
        // arms, a temp-needing expression nested inside one (e.g. a String
        // `self.field` read inside `relabel trust(…)`) never registers its
        // local and the module fails to assemble (#2012 × #2013 seam).
        TirExprKind::Propagate(inner)
        | TirExprKind::Consume(inner)
        | TirExprKind::Relabel { expr: inner, .. }
        | TirExprKind::Borrow { expr: inner, .. } => collect_locals_ctx_expr(inner, locals, ctx),
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
                correct_payload_pattern_locals(&arm.pattern, &scrutinee.ty, locals, ctx);
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
            // `stdout()` / `stderr()` / `stdin()` (#2056) heap-allocate an
            // `Fd` the same way `emit_struct_construct` does — same __st_*
            // temp scheme so the tee/reload in the emitter above has a
            // declared local to target.
            if (name == "stdout" || name == "stderr" || name == "stdin") && args.is_empty() {
                locals.push((struct_temp_name(expr), Ty::Bool)); // i32 placeholder
            }
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
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } => {
            // Behaviour sends need a message-slot temp (#2012). Sync `pub test
            // fn` reads are plain calls and need nothing.
            if let Some(info) = actor_name_of(&receiver.ty, ctx) {
                if info.behaviors.iter().any(|m| m.name == *method) {
                    locals.push((actor_msg_temp_name(expr), Ty::Bool)); // i32 placeholder
                }
            }
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
                if peels_to_string(ty) {
                    // String (or Secret[String]/Tainted[String] — #2013)
                    // variables use split (ptr, len) locals.
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
            if peels_to_string(&var_ty) {
                // List[String] element — split into ptr/len locals (i32×2),
                // plus a *MvlString unpack temp for the loop-body load.
                // Ty::Bool → i32 WASM local (convention for opaque pointer/int
                // slots; see __for_arr_* below).
                locals.push((format!("{var_name}_ptr"), Ty::Bool));
                locals.push((format!("{var_name}_len"), Ty::Bool));
                locals.push((format!("__for_ms_{}", span.offset), Ty::Bool));
            } else {
                locals.push((var_name, var_ty));
            }
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
        TirExprKind::FnCall { name, args, .. } => {
            for a in args {
                collect_locals_expr(a, locals);
            }
            // `format(...)` returns `*MvlString` (via `_mvl_format`), unpacked
            // through the same temp-local convention as the String methods below.
            if name == "format" && args.len() == 2 {
                locals.push((mvl_string_temp_name(expr), Ty::Bool));
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
            if peels_to_string(&receiver.ty)
                && matches!(
                    method.as_str(),
                    "concat" | "substring" | "to_upper" | "to_lower" | "trim" | "replace"
                )
            {
                // Ty::Bool → i32 in `wasm_ty` — reuse for the pointer
                // temp so we don't need a dedicated "raw i32" ty.
                locals.push((mvl_string_temp_name(expr), Ty::Bool));
            }
            // `Float.to_string()` also returns `*MvlString` (via
            // `_mvl_float_to_string`, #2039) and gets the same unpack treatment.
            if matches!(receiver.ty, Ty::Float) && method == "to_string" {
                locals.push((mvl_string_temp_name(expr), Ty::Bool));
            }
            // `.unwrap_or(default)` on Option stashes the option pointer
            // in a temp so it can be dropped after the if-else selects
            // a value.
            if let Some(inner) = option_inner_ty(&receiver.ty) {
                if method == "unwrap_or" {
                    locals.push((mvl_option_temp_name(expr), Ty::Bool));
                    // `Option[String]` unwraps to a `*MvlString` that the
                    // then-branch unpacks into (ptr, len) via the same
                    // temp scheme as the Group B string methods (#2024).
                    if peels_to_string(inner) {
                        locals.push((mvl_string_temp_name(expr), Ty::Bool));
                    }
                }
            }
            // Same for Result.unwrap_or — stashes the Result pointer in __mr_*.
            if let Some(ok_ty) = result_ok_ty(&receiver.ty) {
                if method == "unwrap_or" {
                    locals.push((mvl_result_temp_name(expr), Ty::Bool));
                    if peels_to_string(ok_ty) {
                        locals.push((mvl_string_temp_name(expr), Ty::Bool));
                    }
                }
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
            // Drop all heap locals except the one being returned.
            // The exclude keeps the return value alive for the caller;
            // all other live heap locals are freed here (not at fn exit).
            let excluded = exclude_returned_locals(e, ctx);
            emit_fn_heap_drops(out, &ctx.fn_locals.borrow(), &excluded);
            out.push_str("    return\n");
        }
        TirStmt::Return { value: None, .. } => {
            emit_fn_heap_drops(out, &ctx.fn_locals.borrow(), &[]);
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
                if is_string_ty(ty, ctx) {
                    // Init leaves (ptr, len) on stack — store into split locals.
                    out.push_str(&format!("    local.set ${name}_len\n"));
                    out.push_str(&format!("    local.set ${name}_ptr\n"));
                } else {
                    out.push_str(&format!("    local.set ${name}\n"));
                }
            } else if matches!(pattern, Pattern::Wildcard(_)) {
                // `let _ = expr` — evaluate for side effects, discard result.
                emit_expr(out, init, ctx);
                if is_string_ty(ty, ctx) {
                    // String init leaves two i32s (ptr, len) on the stack.
                    out.push_str("    drop\n    drop\n");
                } else {
                    out.push_str("    drop\n");
                }
            } else {
                out.push_str(&format!("    ;; unsupported let pattern: {pattern:?}\n"));
            }
        }
        // `x = value;` for `ref` locals, and `base.field = value;` for
        // heap-allocated struct / actor-state pointers (#2012). The latter is
        // a plain typed store — WASM struct values *are* pointers, so unlike
        // the LLVM backend there is no SSA-aggregate obstacle here.
        TirStmt::Assign { target, value, .. } => match target {
            LValue::Ident(name, _) => {
                emit_expr(out, value, ctx);
                out.push_str(&format!("    local.set ${name}\n"));
            }
            LValue::Field { base, field, .. } => {
                emit_field_assign(out, base, field, value, ctx);
            }
        },
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
                Some(ty) if peels_to_string(&ty) => {
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
            // Drop heap locals introduced in this loop body before the
            // back-edge so per-iteration allocations don't accumulate.
            // Zero each out afterward — the function-exit drops re-visit
            // all fn locals; null-safe drops on zeroed values are no-ops.
            {
                let mut loop_locals: Vec<(String, Ty)> = Vec::new();
                collect_locals_block(body, &mut loop_locals);
                collect_locals_ctx(body, &mut loop_locals, ctx);
                let mut seen = std::collections::HashSet::new();
                loop_locals.retain(|(n, _)| seen.insert(n.clone()));
                for (name, ty) in &loop_locals {
                    if let Some(drop_fn) = local_drop_fn(name, ty) {
                        out.push_str(&format!("    local.get ${name}\n"));
                        out.push_str(&format!("    call ${drop_fn}\n"));
                        out.push_str("    i32.const 0\n");
                        out.push_str(&format!("    local.set ${name}\n"));
                    }
                }
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
            // Also handles generic type params (e.g. T=String) by resolving
            // Named("T") through ctx.type_subst.
            if is_string_ty(&expr.ty, ctx) {
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
            if peels_to_string(&left.ty) && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
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
            // `stdout()` / `stderr()` / `stdin()` (std.io, #2056) — pure
            // constructors for the standard-stream `Fd` values. Heap-allocate
            // an `Fd { inner: <fd number> }` exactly like a user struct
            // literal so `logger.fd` field reads and dynamic `write(fd, …)`
            // dispatch (below) see an ordinary Fd pointer.
            if (name == "stdout" || name == "stderr" || name == "stdin") && args.is_empty() {
                if let Some(layout) = ctx.struct_layouts.get("Fd") {
                    if let Some(slot) = layout.fields.iter().find(|s| s.name == "inner") {
                        ctx.needs_runtime.set(true);
                        let fd_num: i64 = match name.as_str() {
                            "stdout" => 1,
                            "stderr" => 2,
                            _ => 0,
                        };
                        let temp = struct_temp_name(expr);
                        out.push_str(&format!("    i32.const {}\n", layout.total_size));
                        out.push_str("    call $_mvl_struct_alloc\n");
                        out.push_str(&format!("    local.tee ${temp}\n"));
                        out.push_str(&format!("    i64.const {fd_num}\n"));
                        out.push_str(&format!("    i64.store offset={}\n", slot.offset));
                        out.push_str(&format!("    local.get ${temp}\n"));
                        return;
                    }
                }
            }
            // `write(fd, msg)` (std.io, #2056) — dynamic dispatch on the
            // runtime `Fd.inner` value via WASI `fd_write`. No trailing
            // newline (unlike println/eprintln) — `std.log`'s `log_write`
            // appends its own. A non-zero WASI errno traps rather than
            // constructing a real `IoError` payload — same tradeoff
            // println/eprintln already make by ignoring `fd_write`'s result.
            // Arbitrary file fds from `open()` remain unsupported: `open`
            // itself has no WASM body, so any caller stubs to `unreachable`.
            if name == "write" && args.len() == 2 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx); // i64 fd number
                out.push_str("    i32.wrap_i64\n");
                emit_expr(out, &args[1], ctx); // (ptr, len)
                out.push_str("    call $mvl_write\n"); // -> i32 errno
                out.push_str("    if (result i32)\n");
                out.push_str("      unreachable\n");
                out.push_str("    else\n");
                out.push_str("      i64.const 0\n");
                out.push_str("      call $_mvl_result_ok_i64\n");
                out.push_str("    end\n");
                return;
            }
            // `read_file(path)` / `_read_file(path)` (std.io, #2076) — file
            // read via the preloaded `runtime/wasm/` crate's
            // `_mvl_io_read_file`, not hand-rolled WASI imports (reads need
            // path/rights marshalling `fd_write` didn't; wasm32-wasip1's
            // `std::fs` already does it correctly inside the crate).
            // `Tainted[String]`/`Path` erase to the same (ptr, len)
            // representation as `String` at this layer, so both builtin
            // names route through the same runtime call unconditionally.
            if (name == "read_file" || name == "_read_file") && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx); // (ptr, len)
                out.push_str("    call $_mvl_io_read_file\n");
                return;
            }
            // `now()` (std.time, #2056) — real wall-clock read via WASI
            // `clock_time_get`, heap-boxed as an opaque nanoseconds handle.
            // `Instant` is MVL-visible as `struct {}` (no fields) — the
            // nanosecond payload is a WASM-backend-only representation
            // detail, same trick the Rust/LLVM backends already use.
            if name == "now" && args.is_empty() {
                out.push_str("    call $mvl_now\n");
                return;
            }
            // `_instant_epoch_seconds(t)` (std.time, module-private) — reads
            // the nanoseconds `$mvl_now` boxed and converts to whole seconds.
            if name == "_instant_epoch_seconds" && args.len() == 1 {
                emit_expr(out, &args[0], ctx);
                out.push_str("    i64.load\n");
                out.push_str("    i64.const 1000000000\n");
                out.push_str("    i64.div_s\n");
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
            // `format(template, values)` — positional `{}` interpolation
            // (std/core.mvl builtin, #2039). `template` leaves (ptr, len)
            // on the stack like any String expr; `values` (a
            // `List[String]`) leaves its `*MvlArray` pointer. `_mvl_format`
            // returns `*MvlString`, unpacked back to (ptr, len) like the
            // other Group B allocation-returning calls.
            if name == "format" && args.len() == 2 {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                emit_expr(out, &args[1], ctx);
                out.push_str("    call $_mvl_format\n");
                emit_unpack_mvl_string(out, expr);
                return;
            }
            // `Some(x)` constructor — the TIR lowerer represents it as a
            // FnCall on the bare name "Some". Dispatch to the runtime's
            // typed constructor based on the payload's WASM lowering.
            if name == "Some" && args.len() == 1 && matches!(&expr.ty, Ty::Option(_)) {
                ctx.needs_runtime.set(true);
                let inner = option_inner_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
                emit_expr(out, &args[0], ctx);
                if is_string_ty(&inner, ctx) {
                    // String payload arrives as (ptr, len); box it into a
                    // `*MvlString` before handing it to the i32-payload
                    // Option constructor — same blind spot as unwrap_or
                    // (#2024), just on the construction side.
                    out.push_str("    call $_mvl_string_new\n");
                    out.push_str("    call $_mvl_option_some_i32\n");
                } else {
                    // The runtime `Some` ctor is i64-typed; Float payloads are
                    // stored bit-for-bit via reinterpret rather than a
                    // dedicated f64 ctor (#2038).
                    if is_float_ctx(&inner, ctx) {
                        out.push_str("    i64.reinterpret_f64\n");
                    }
                    let (some_ctor, _) = option_ops_for(&inner, ctx);
                    out.push_str(&format!("    call ${some_ctor}\n"));
                }
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
                let ok_ty = result_ok_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
                emit_expr(out, &args[0], ctx);
                if is_string_ty(&ok_ty, ctx) {
                    // Same String blind spot as `Some(x)` above (#2024).
                    out.push_str("    call $_mvl_string_new\n");
                    out.push_str("    call $_mvl_result_ok_i32\n");
                } else {
                    // The runtime `Ok` ctor is i64-typed; Float payloads are
                    // stored bit-for-bit via reinterpret rather than a
                    // dedicated f64 ctor (#2038).
                    if is_float_ctx(&ok_ty, ctx) {
                        out.push_str("    i64.reinterpret_f64\n");
                    }
                    let (ok_ctor, _) = result_ops_for_ok(&ok_ty, ctx);
                    out.push_str(&format!("    call ${ok_ctor}\n"));
                }
                return;
            }
            // `Err(x)` constructor — dispatches by the Result's actual
            // Err-payload type, mirroring `Ok` above (#2066; previously
            // any non-String Err type stubbed the whole enclosing function
            // to `unreachable`).
            if name == "Err" && args.len() == 1 && matches!(&expr.ty, Ty::Result(_, _)) {
                ctx.needs_runtime.set(true);
                let err_ty = result_err_ty(&expr.ty).cloned().unwrap_or(Ty::String);
                emit_expr(out, &args[0], ctx);
                if peels_to_string(&err_ty) {
                    out.push_str("    call $_mvl_result_err_str\n");
                } else {
                    if is_float_ctx(&err_ty, ctx) {
                        out.push_str("    i64.reinterpret_f64\n");
                    }
                    let (err_ctor, _) = result_ops_for_err(&err_ty, ctx);
                    out.push_str(&format!("    call ${err_ctor}\n"));
                }
                return;
            }
            // A bare call inside an actor body may name one of the actor's own
            // methods — that is how a private helper is invoked, since
            // `self.helper()` is not accepted for non-public methods. Route it
            // to the emitted `$<actor>_<method>` with the state pointer; without
            // this it emitted `call $helper`, a symbol that does not exist (#2012).
            if let Some(actor) = ctx.self_type.borrow().clone() {
                if let Some(info) = ctx.actors.get(actor.as_str()) {
                    if info.methods.iter().any(|m| m.name == *name) {
                        emit_actor_self_call(out, info, name, args, ctx);
                        return;
                    }
                }
            }
            for a in args {
                emit_expr(out, a, ctx);
            }
            // If the callee is a generic function, use the mangled monomorphized name.
            if let Some((type_params, fn_params)) = ctx.generic_fn_map.get(name.as_str()) {
                let subst = infer_type_subst_from_args(type_params, fn_params, args);
                let mangled = mangle_generic_name(name, type_params, &subst);
                out.push_str(&format!("    call ${mangled}\n"));
            } else {
                out.push_str(&format!("    call ${name}\n"));
            }
        }
        // Actor behaviour send / `pub test fn` read. Checked before the builtin
        // method table so a behaviour never collides with a stdlib method name
        // (mirrors the LLVM backend's actor fast path).
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if actor_name_of(&receiver.ty, ctx).is_some() => {
            let info = actor_name_of(&receiver.ty, ctx)
                .expect("guarded above")
                .clone();
            // `self.method(…)` inside the actor's own body is a direct
            // synchronous call, never a queued send. Routing it through the
            // mailbox deferred the call past the rest of the caller's body, so
            // a self-send followed by more `self.field` writes produced a
            // different answer than the Rust/LLVM backends (#2012).
            if matches!(&receiver.kind, TirExprKind::Var(n) if n == "self") {
                emit_actor_self_call(out, &info, method, args, ctx);
                return;
            }
            if !emit_actor_method_call(out, &info, receiver, method, args, expr, ctx) {
                out.push_str(&format!(
                    "    ;; unsupported actor method: {}.{method}\n",
                    info.name
                ));
            }
        }
        // Guarded off a user-defined extension method of the same name
        // (#2058 follow-up) — a struct's own `to_string` must win over this
        // generic by-name fallback, which never checked the receiver's type.
        TirExprKind::MethodCall {
            receiver, method, ..
        } if method == "to_string" && !is_struct_method_call(receiver, method, ctx) => {
            emit_expr(out, receiver, ctx);
            match &receiver.ty {
                Ty::Int => out.push_str("    call $mvl_int_to_string\n"),
                Ty::Float => {
                    ctx.needs_runtime.set(true);
                    out.push_str("    call $_mvl_float_to_string\n");
                    emit_unpack_mvl_string(out, expr);
                }
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
        } if peels_to_string(&receiver.ty)
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
        } if peels_to_string(&receiver.ty)
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
        // `String.split(sep)` — two (ptr, len) pairs in, `*MvlArray` of
        // `*MvlString` out (#2014). Unlike its Group B neighbours above there
        // is no `emit_unpack_mvl_string`: the result is already the array
        // pointer every `List[T]` operation expects on the stack.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if peels_to_string(&receiver.ty) && method == "split" && args.len() == 1 => {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str("    call $_mvl_string_split\n");
        }
        // `String.parse_int()` — returns a heap-allocated MvlResult pointer
        // (Group H import). Receiver is the raw (ptr, len) string on the stack.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if peels_to_string(&receiver.ty) && method == "parse_int" && args.is_empty() => {
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
            let temp = mvl_result_temp_name(expr);
            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    local.tee ${temp}\n"));
            out.push_str("    call $_mvl_result_tag\n");
            // tag == 0 → Ok. i32.eqz maps 0→1, non-zero→0.
            out.push_str("    i32.eqz\n");
            if is_string_ty(&ok_ty, ctx) {
                // Same String blind spot as Option.unwrap_or above (#2024) —
                // the Ok payload is a `*MvlString` i32 pointer, unpacked to
                // (ptr, len) to match the `else` branch's shape.
                out.push_str("    if (result i32 i32)\n");
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_result_value_i32\n");
                emit_unpack_mvl_string(out, expr);
            } else {
                let (_, getter) = result_ops_for_ok(&ok_ty, ctx);
                let result_wasm_ty = wasm_ty(&ok_ty, ctx);
                out.push_str(&format!("    if (result {result_wasm_ty})\n"));
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str(&format!("    call ${getter}\n"));
                // The runtime getter returns i64; Float payloads are
                // bit-cast back via reinterpret (#2038).
                if is_float_ctx(&ok_ty, ctx) {
                    out.push_str("    f64.reinterpret_i64\n");
                }
            }
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
            // String values are `*MvlString` handles still owned by the
            // map's own entry — `_mvl_map_get_str` clones before wrapping
            // in the Option so the caller's eventual drop (e.g.
            // `unwrap_or`'s cleanup) doesn't free memory the map still
            // references (#2047). Non-string values are plain scalars with
            // no ownership to share, so the untyped getter is fine as-is.
            let val_ty = map_key_val_ty(&receiver.ty).map(|(_, v)| v.clone());
            let getter = if val_ty.as_ref().is_some_and(|v| is_string_ty(v, ctx)) {
                "_mvl_map_get_str"
            } else {
                "_mvl_map_get_si64"
            };
            emit_expr(out, receiver, ctx); // map ptr
            emit_expr(out, &args[0], ctx); // key → (ptr, len)
            out.push_str(&format!("    call ${getter}\n"));
        }
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if map_key_val_ty(&receiver.ty).is_some() && method == "insert" && args.len() == 2 => {
            ctx.needs_runtime.set(true);
            let val_ty = map_key_val_ty(&receiver.ty)
                .map(|(_, v)| v.clone())
                .unwrap_or(Ty::Int);
            emit_expr(out, receiver, ctx); // map ptr
            emit_expr(out, &args[0], ctx); // key → (ptr, len)
            emit_map_value_push(out, &args[1], &val_ty, ctx);
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
            // String elements are stored as *MvlString (i32); Bool/enum/struct are
            // i32 too. Everything else (Int, Float) is i64.
            let getter = if is_i32(&elem_ty, ctx) || is_string_ty(&elem_ty, ctx) {
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
            let temp = mvl_option_temp_name(expr);
            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    local.tee ${temp}\n"));
            out.push_str("    call $_mvl_option_tag\n");
            // tag == 0 → Some. `i32.eqz` maps 0→1, non-zero→0.
            out.push_str("    i32.eqz\n");
            if is_string_ty(&inner, ctx) {
                // `Option[String]`'s payload slot stores the `*MvlString`
                // pointer as i32 (same convention `.get()` uses via
                // `_mvl_array_get_option_i32`, wasm_text.rs:2152) — not the
                // generic i32-or-i64 shape `option_ops_for` picks between.
                // The `then` branch must yield the ordinary (ptr, len)
                // String shape to match the `else` branch (#2024).
                out.push_str("    if (result i32 i32)\n");
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_option_value_i32\n");
                emit_unpack_mvl_string(out, expr);
            } else {
                let (_, getter) = option_ops_for(&inner, ctx);
                let result_ty = wasm_ty(&inner, ctx);
                out.push_str(&format!("    if (result {result_ty})\n"));
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str(&format!("    call ${getter}\n"));
                // The runtime getter returns i64; Float payloads are
                // bit-cast back via reinterpret (#2038).
                if is_float_ctx(&inner, ctx) {
                    out.push_str("    f64.reinterpret_i64\n");
                }
            }
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
            let elem_size = elem_size_bytes(&elem_ty, ctx);
            let cap = elems.len().max(4) as i32;
            let temp = mvl_array_temp_name(expr);
            out.push_str(&format!("    i32.const {elem_size}\n"));
            out.push_str(&format!("    i32.const {cap}\n"));
            out.push_str("    call $_mvl_array_new\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            if is_string_ty(&elem_ty, ctx) {
                // Each String element arrives on the stack as (ptr, len);
                // wrap it in a *MvlString allocation before pushing (i32).
                for e in elems {
                    out.push_str(&format!("    local.get ${temp}\n"));
                    emit_expr(out, e, ctx);
                    out.push_str("    call $_mvl_string_new\n");
                    out.push_str("    call $_mvl_array_push_i32\n");
                }
            } else {
                let push_op = push_op_for(&elem_ty, ctx);
                for e in elems {
                    out.push_str(&format!("    local.get ${temp}\n"));
                    emit_expr(out, e, ctx);
                    out.push_str(&format!("    call {push_op}\n"));
                }
            }
            out.push_str(&format!("    local.get ${temp}\n"));
        }
        // Set literal — `{e1, e2, ...}` (unique values). Same array
        // construction as List, then a dedup call (sort + remove adjacent
        // duplicates) to enforce Set semantics.
        TirExprKind::Set { elems } => {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&expr.ty).cloned().unwrap_or(Ty::Int);
            let elem_size = elem_size_bytes(&elem_ty, ctx);
            let cap = elems.len().max(4) as i32;
            let temp = mvl_array_temp_name(expr);
            out.push_str(&format!("    i32.const {elem_size}\n"));
            out.push_str(&format!("    i32.const {cap}\n"));
            out.push_str("    call $_mvl_array_new\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            if is_string_ty(&elem_ty, ctx) {
                for e in elems {
                    out.push_str(&format!("    local.get ${temp}\n"));
                    emit_expr(out, e, ctx);
                    out.push_str("    call $_mvl_string_new\n");
                    out.push_str("    call $_mvl_array_push_i32\n");
                }
                // Dedup by content — pointer-address dedup misses equal strings
                // from distinct allocations; use the content-aware variant.
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_string_ptr_array_dedup\n");
            } else {
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
            }
            out.push_str(&format!("    local.get ${temp}\n"));
        }
        // Map literal — `{"k1": v1, "k2": v2, ...}`. String keys only;
        // any value type that fits the i64 map-value slot is supported
        // (#2024) — see `emit_map_value_push`. Emits `_mvl_map_new_si64()`,
        // stashes the pointer, then inserts each pair via `_mvl_map_insert_si64`.
        TirExprKind::Map { pairs } => {
            ctx.needs_runtime.set(true);
            let kv = map_key_val_ty(&expr.ty);
            let val_ty = kv.map(|(_, v)| v.clone());
            let supported = matches!(kv, Some((Ty::String, _)))
                && val_ty.as_ref().is_some_and(|v| map_value_supported(v, ctx));
            if !supported {
                out.push_str(
                    "    ;; unsupported: Map literal (String keys only; Float values not wired, #2024)\n",
                );
                return;
            }
            let val_ty = val_ty.unwrap();
            let temp = mvl_map_temp_name(expr);
            out.push_str("    call $_mvl_map_new_si64\n");
            out.push_str(&format!("    local.set ${temp}\n"));
            for (k, v) in pairs {
                out.push_str(&format!("    local.get ${temp}\n"));
                emit_expr(out, k, ctx); // key → (ptr, len) two i32s
                emit_map_value_push(out, v, &val_ty, ctx);
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
            } else if peels_to_string(&expr.ty) {
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
        // `consume(x)` — ownership transfer is compile-time only; at runtime
        // just emit the inner value unchanged.
        TirExprKind::Consume(inner) => {
            emit_expr(out, inner, ctx);
        }
        // `&x` / `&mut x` — borrows are compile-time only for the WASM
        // backend; the underlying value is passed by its WASM representation.
        TirExprKind::Borrow { expr: inner, .. } => {
            emit_expr(out, inner, ctx);
        }
        // `relabel name(expr, "tag")` — IFC relabel transition (#2013).
        // Labels are compile-time only; the runtime value passes through
        // unchanged (mirrors LLVM's `emit_relabel_tir`). An audit event is
        // emitted via `_mvl_audit_emit_relabel` when the call site carries
        // `audit` OR the transition itself was declared `audit` (#896) —
        // same needs_audit rule as the Rust/LLVM backends, so a mandatory
        // audit declaration can't be silently skipped by omitting the
        // call-site keyword on this backend alone.
        TirExprKind::Relabel {
            name,
            expr: inner,
            tag,
            audit,
        } => {
            emit_expr(out, inner, ctx);
            let needs_audit = *audit || ctx.audit_relabels.contains_key(name);
            if needs_audit {
                ctx.needs_runtime.set(true);
                let (from_lbl, to_lbl) = relabel_label_strings(name, ctx.audit_relabels);
                for s in [
                    name.as_str(),
                    from_lbl.as_str(),
                    to_lbl.as_str(),
                    tag.as_str(),
                    "",
                ] {
                    emit_literal_str_operands(out, s, ctx);
                }
                out.push_str("    call $_mvl_audit_emit_relabel\n");
            }
        }
        // `actor Name { field: value }` (#2012).
        TirExprKind::Spawn { actor_type, fields } => {
            emit_actor_spawn(out, actor_type, fields, expr, ctx);
        }
        // User-defined extension method on a custom struct (#2054) — checked
        // last so it never shadows a builtin-type special case above (e.g. a
        // `List`/`String` method). Routes to `${receiver_type}_${method}`,
        // emitted by `emit_extension_method`.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if is_struct_method_call(receiver, method, ctx) => {
            let receiver_type = named_type_name(&receiver.ty).expect("guarded above");
            emit_expr(out, receiver, ctx);
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call ${receiver_type}_{method}\n"));
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
    // load element into loop variable
    out.push_str(&format!("    local.get ${arr_local}\n"));
    out.push_str(&format!("    local.get ${idx_local}\n"));
    out.push_str("    call $_mvl_array_get\n");
    if is_string_ty(&elem_ty, ctx) {
        // Each slot holds a *MvlString (i32 pointer). Load it, then unpack
        // .ptr (offset 0) and .len (offset 4) into the split locals.
        let ms_temp = format!("__for_ms_{span_offset}");
        out.push_str("    i32.load offset=0\n");
        out.push_str(&format!("    local.tee ${ms_temp}\n"));
        out.push_str("    i32.load offset=0\n");
        out.push_str(&format!("    local.set ${var_name}_ptr\n"));
        out.push_str(&format!("    local.get ${ms_temp}\n"));
        out.push_str("    i32.load offset=4\n");
        out.push_str(&format!("    local.set ${var_name}_len\n"));
    } else {
        let (load_op, _) = list_elem_load_op(&elem_ty, ctx);
        out.push_str(&format!("    {load_op}\n"));
        out.push_str(&format!("    local.set ${var_name}\n"));
    }
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
            if peels_to_string(t) {
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
            Pattern::Some { inner, span } => {
                ctx.needs_runtime.set(true);
                let inner_ty = option_inner_ty(&scrutinee.ty).cloned().unwrap_or(Ty::Int);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_option_tag\n");
                out.push_str("    i32.eqz\n"); // 1 when tag was 0 (Some)
                out.push_str(&if_open);
                if let Pattern::Ident(name, _) = inner.as_ref() {
                    if name != "_" {
                        if is_string_ty(&inner_ty, ctx) {
                            // `Option[String]`'s payload slot stores the
                            // `*MvlString` pointer as i32 (same convention as
                            // `.unwrap_or`, wasm_text.rs ~2420) — bind the
                            // split (ptr, len) locals every other String
                            // variable uses, not a single generic local
                            // (#2056; `field_or_empty` in std/log.mvl hit this
                            // via `match fields.get(k) { Some(v) => v, ... }`).
                            let scratch = mvl_some_string_temp_name(span);
                            out.push_str(&format!("    local.get ${temp}\n"));
                            out.push_str("    call $_mvl_option_value_i32\n");
                            out.push_str(&format!("    local.tee ${scratch}\n"));
                            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_PTR}\n"));
                            out.push_str(&format!("    local.set ${name}_ptr\n"));
                            out.push_str(&format!("    local.get ${scratch}\n"));
                            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_LEN}\n"));
                            out.push_str(&format!("    local.set ${name}_len\n"));
                        } else {
                            let (_, getter) = option_ops_for(&inner_ty, ctx);
                            out.push_str(&format!("    local.get ${temp}\n"));
                            out.push_str(&format!("    call ${getter}\n"));
                            if is_float_ctx(&inner_ty, ctx) {
                                out.push_str("    f64.reinterpret_i64\n");
                            }
                            out.push_str(&format!("    local.set ${name}\n"));
                        }
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
            Pattern::Ok { inner, span } => {
                ctx.needs_runtime.set(true);
                let ok_ty = result_ok_ty(&scrutinee.ty).cloned().unwrap_or(Ty::Int);
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_result_tag\n");
                out.push_str("    i32.eqz\n"); // 1 when tag == 0 (Ok)
                out.push_str(&if_open);
                if let Pattern::Ident(name, _) = inner.as_ref() {
                    if name != "_" {
                        if is_string_ty(&ok_ty, ctx) {
                            // `Result[String, E]`'s Ok slot stores the
                            // `*MvlString` pointer as i32 (same convention as
                            // `Option[String]`'s `Some`, #2056) — bind the
                            // split (ptr, len) locals every other String
                            // variable uses, not a single generic local
                            // (#2076; `read_file`'s Ok payload is the first
                            // WASM-backend builtin with Ok=String).
                            let scratch = mvl_ok_string_temp_name(span);
                            out.push_str(&format!("    local.get ${temp}\n"));
                            out.push_str("    call $_mvl_result_value_i32\n");
                            out.push_str(&format!("    local.tee ${scratch}\n"));
                            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_PTR}\n"));
                            out.push_str(&format!("    local.set ${name}_ptr\n"));
                            out.push_str(&format!("    local.get ${scratch}\n"));
                            out.push_str(&format!("    i32.load offset={MVL_STRING_OFFSET_LEN}\n"));
                            out.push_str(&format!("    local.set ${name}_len\n"));
                        } else {
                            let (_, getter) = result_ops_for_ok(&ok_ty, ctx);
                            out.push_str(&format!("    local.get ${temp}\n"));
                            out.push_str(&format!("    call ${getter}\n"));
                            if is_float_ctx(&ok_ty, ctx) {
                                out.push_str("    f64.reinterpret_i64\n");
                            }
                            out.push_str(&format!("    local.set ${name}\n"));
                        }
                    }
                }
                emit_match_body(out, &arm.body, ctx);
                out.push_str("    else\n");
                open_ifs += 1;
            }
            // `Err(inner)` pattern on Result[T, E]. Check tag == 1, and —
            // when `inner` names a specific qualified enum variant — also
            // check that variant's own discriminant. Without this, multiple
            // qualified Err arms in the same match (e.g. `Err(AuthError::A)`,
            // `Err(AuthError::B)`) all lower to the same "tag == 1" condition,
            // so only the first one listed is ever reachable.
            Pattern::Err { inner, .. } => {
                ctx.needs_runtime.set(true);
                let err_ty = result_err_ty(&scrutinee.ty).cloned().unwrap_or(Ty::String);
                let qualified = wasm_qualified_variant_name(inner);
                let variant_info =
                    qualified.and_then(|qname| payload_variant_for(qname, ctx).cloned());

                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    call $_mvl_result_tag\n");
                if let Some(pv) = &variant_info {
                    // Nested if, not an unconditional AND — the wrapped enum
                    // pointer is only valid to dereference when the Result is
                    // actually Err; on Ok its slot may hold unrelated data.
                    out.push_str("    if (result i32)\n");
                    out.push_str(&format!("    local.get ${temp}\n"));
                    out.push_str("    call $_mvl_result_value_i32\n");
                    out.push_str("    i32.load offset=0\n");
                    out.push_str(&format!("    i32.const {}\n", pv.disc));
                    out.push_str("    i32.eq\n");
                    out.push_str("    else\n");
                    out.push_str("    i32.const 0\n");
                    out.push_str("    end\n");
                }
                out.push_str(&if_open);
                // Bind inner only if named and non-wildcard. Dispatches by
                // the Result's actual Err-payload type (#2066) — String is
                // extracted as *MvlString i32 (not unpacked to (ptr, len);
                // no corpus/example Err arm uses the bound string as a
                // String yet), a genuinely i64-shaped payload (Int, Float)
                // keeps the full i64 getter instead of the old unconditional
                // `i32.wrap_i64`, which silently truncated it.
                match inner.as_ref() {
                    Pattern::Ident(name, _) if name != "_" && !name.contains("::") => {
                        out.push_str(&format!("    local.get ${temp}\n"));
                        if peels_to_string(&err_ty) {
                            out.push_str("    call $_mvl_result_value_i32\n");
                        } else {
                            let (_, getter) = result_ops_for_err(&err_ty, ctx);
                            out.push_str(&format!("    call ${getter}\n"));
                            if is_float_ctx(&err_ty, ctx) {
                                out.push_str("    f64.reinterpret_i64\n");
                            }
                        }
                        out.push_str(&format!("    local.set ${name}\n"));
                    }
                    // Struct-shaped variant pattern with bound fields (e.g.
                    // `Err(AuthError::AccountLocked { attempts })`) — the
                    // discriminant check above already narrowed the variant;
                    // here just extract and bind the named payload fields
                    // from the wrapped enum's own `{disc,payload_ptr}` header
                    // (same layout `emit_payload_load` uses for a bare match).
                    Pattern::Struct {
                        fields: named_fields,
                        ..
                    } => {
                        if let Some(pv) = &variant_info {
                            let inner_off = inner.span().offset;
                            let err_val_local = format!("__ev_{span_offset}_{inner_off}");
                            let payload_ptr_local = format!("__epp_{span_offset}_{inner_off}");
                            out.push_str(&format!("    local.get ${temp}\n"));
                            out.push_str("    call $_mvl_result_value_i32\n");
                            out.push_str(&format!("    local.set ${err_val_local}\n"));
                            out.push_str(&format!("    local.get ${err_val_local}\n"));
                            out.push_str("    i32.load offset=4\n");
                            out.push_str(&format!("    local.set ${payload_ptr_local}\n"));
                            for (slot, fname) in pv.field_names.clone().iter().enumerate() {
                                let Some((_, pat)) = named_fields.iter().find(|(n, _)| n == fname)
                                else {
                                    continue;
                                };
                                if let Pattern::Ident(bname, _) = pat {
                                    if bname != "_" && !bname.contains("::") {
                                        let field_ty =
                                            pv.fields.get(slot).cloned().unwrap_or(Ty::Int);
                                        let byte_off = (slot as u32) * 8;
                                        out.push_str(&format!(
                                            "    local.get ${payload_ptr_local}\n"
                                        ));
                                        if peels_to_string(&field_ty) {
                                            out.push_str(&format!(
                                                "    i64.load offset={byte_off}\n"
                                            ));
                                            out.push_str("    i32.wrap_i64\n");
                                            let sv_tmp = format!("__svs_{inner_off}_{bname}");
                                            out.push_str(&format!("    local.tee ${sv_tmp}\n"));
                                            out.push_str(&format!(
                                                "    i32.load offset={MVL_STRING_OFFSET_PTR}\n"
                                            ));
                                            out.push_str(&format!("    local.set ${bname}_ptr\n"));
                                            out.push_str(&format!("    local.get ${sv_tmp}\n"));
                                            out.push_str(&format!(
                                                "    i32.load offset={MVL_STRING_OFFSET_LEN}\n"
                                            ));
                                            out.push_str(&format!("    local.set ${bname}_len\n"));
                                        } else {
                                            emit_payload_load(out, &field_ty, byte_off, ctx);
                                            out.push_str(&format!("    local.set ${bname}\n"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
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
                // A nested unit-variant enum field (e.g. `Wrapped(Inner::A)`)
                // narrows further than the outer tag — AND in a comparison
                // against the payload slot's inline i32 discriminant so arms
                // sharing an outer discriminant are actually disambiguated
                // (#2029). Any field-pattern shape this loop can't safely
                // discriminate (an unresolved/mistyped qualifier, a literal,
                // a doubly-nested payload-carrying variant, …) falls through
                // to the `;; unsupported nested pattern` marker instead of
                // silently matching every arm — the whole-body `unreachable`
                // stub (see the `;; unsupported` scan at fn-emission time)
                // turns that into a loud trap rather than a wrong answer.
                // Note: this AND unconditionally dereferences the payload
                // pointer even when the outer tag check above is false —
                // safe only because unit variants store `payload_ptr = 0`
                // (see `emit_enum_variant_construct`), so the speculative
                // load stays in bounds and its result is simply discarded.
                for (slot, pat) in pats.iter().enumerate() {
                    match pat {
                        Pattern::Wildcard(_) => {}
                        Pattern::Ident(name, _) if name == "_" => {}
                        Pattern::Ident(name, _) if name.contains("::") => {
                            let field_ty = pv.fields.get(slot).cloned().unwrap_or(Ty::Int);
                            let qualifier_enum = name.split_once("::").map(|(t, _)| t);
                            let type_matches = qualifier_enum
                                .zip(underlying_named_ty(&field_ty, ctx))
                                .is_some_and(|(q, actual)| q == actual.as_str());
                            match (ctx.enum_variants.get(name), type_matches) {
                                (Some(&inner_disc), true) => {
                                    let byte_off = (slot as u32) * 8;
                                    out.push_str(&format!("    local.get ${temp}\n"));
                                    out.push_str("    i32.load offset=4\n");
                                    // `type_matches` confirms the field's declared
                                    // type resolves to this qualifier's own
                                    // (all-unit, and therefore i32-represented)
                                    // enum — pass a clean `Ty::Named` for it rather
                                    // than the raw (possibly-aliased) `field_ty` so
                                    // `emit_payload_load` always takes the i32 path.
                                    emit_payload_load(
                                        out,
                                        &Ty::Named(qualifier_enum.unwrap().to_string(), vec![]),
                                        byte_off,
                                        ctx,
                                    );
                                    out.push_str(&format!("    i32.const {inner_disc}\n"));
                                    out.push_str("    i32.eq\n");
                                    out.push_str("    i32.and\n");
                                }
                                _ => {
                                    out.push_str(&format!(
                                        "    ;; unsupported nested pattern: {name}\n"
                                    ));
                                }
                            }
                        }
                        Pattern::Ident(_, _) => {}
                        other => {
                            out.push_str(&format!(
                                "    ;; unsupported nested pattern: {other:?}\n"
                            ));
                        }
                    }
                }
                out.push_str(&if_open);
                // Load payload_ptr from header offset 4.
                let payload_ptr_local = format!("__pp_{span_offset}_{pat_off}");
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    i32.load offset=4\n");
                out.push_str(&format!("    local.set ${payload_ptr_local}\n"));
                // Bind each named pattern field from the payload at slot × 8.
                // Qualified variant references (e.g. `Inner::A`) are guards,
                // not bindings — the discriminant check above already
                // consumed them.
                for (slot, pat) in pats.iter().enumerate() {
                    if let Pattern::Ident(name, _) = pat {
                        if name != "_" && !name.contains("::") {
                            let field_ty = pv.fields.get(slot).cloned().unwrap_or(Ty::Int);
                            let byte_off = (slot as u32) * 8;
                            out.push_str(&format!("    local.get ${payload_ptr_local}\n"));
                            if peels_to_string(&field_ty) {
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
            // `Variant { field: pat, .. }` — struct-shaped payload-enum pattern.
            // Same payload layout as TupleStruct (positional 8-byte slots) —
            // only the pattern syntax differs — so this reorders `named_fields`
            // to the variant's declared slot order (`pv.field_names`) and then
            // reuses the identical discriminant-check + payload-load sequence.
            Pattern::Struct {
                name: variant_name,
                fields: named_fields,
                ..
            } => {
                let type_name = variant_name.split_once("::").map(|(t, _)| t).unwrap_or("");
                let pv_opt = ctx
                    .payload_enums
                    .get(type_name)
                    .and_then(|info| info.variants.iter().find(|v| v.name == *variant_name));
                let Some(pv) = pv_opt else {
                    out.push_str(&format!(
                        "    ;; unsupported Struct pattern (unknown variant): {variant_name}\n"
                    ));
                    for _ in 0..open_ifs {
                        out.push_str("    end\n");
                    }
                    return;
                };
                ctx.needs_runtime.set(true);
                let disc = pv.disc;
                let pat_off = arm.pattern.span().offset;
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    i32.load offset=0\n");
                out.push_str(&format!("    i32.const {disc}\n"));
                out.push_str("    i32.eq\n");
                out.push_str(&if_open);
                let payload_ptr_local = format!("__pp_{span_offset}_{pat_off}");
                out.push_str(&format!("    local.get ${temp}\n"));
                out.push_str("    i32.load offset=4\n");
                out.push_str(&format!("    local.set ${payload_ptr_local}\n"));
                // Bind each named field in its declared slot order — a struct
                // pattern need not mention every field (partial destructure,
                // `..`), so look each declared name up in the pattern rather
                // than iterating the pattern's own (unordered) field list.
                for (slot, fname) in pv.field_names.clone().iter().enumerate() {
                    let Some((_, pat)) = named_fields.iter().find(|(n, _)| n == fname) else {
                        continue;
                    };
                    if let Pattern::Ident(name, _) = pat {
                        if name != "_" && !name.contains("::") {
                            let field_ty = pv.fields.get(slot).cloned().unwrap_or(Ty::Int);
                            let byte_off = (slot as u32) * 8;
                            out.push_str(&format!("    local.get ${payload_ptr_local}\n"));
                            if peels_to_string(&field_ty) {
                                out.push_str(&format!("    i64.load offset={byte_off}\n"));
                                out.push_str("    i32.wrap_i64\n");
                                // Named unlike the TupleStruct arm's `__sv_<byte_off>_*`
                                // (keyed on positional slot, trivially known there):
                                // `collect_match_arm_locals` can't compute a struct
                                // pattern's declared slot order without a
                                // payload_enums lookup it doesn't have access to
                                // (#2073's still-open TupleStruct byte_off gap), so
                                // this temp is keyed on the pattern's own span + field
                                // name instead — both collect and emit sides can
                                // derive that without any registry lookup.
                                let sv_tmp = format!("__svs_{pat_off}_{name}");
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
        _ if peels_to_string(field_ty) => {
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
        _ if peels_to_string(field_ty) => {
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
        _ if peels_to_string(field_ty) => {
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
        _ if peels_to_string(&slot.ty) => {
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
    scrutinee_ty: &Ty,
    option_inner: Option<&Ty>,
    span_offset: u32,
    locals: &mut Vec<(String, Ty)>,
) {
    match &arm.pattern {
        Pattern::Some { inner, span } => {
            if let Pattern::Ident(name, _) = inner.as_ref() {
                if name != "_" {
                    let ty = option_inner.cloned().unwrap_or(Ty::Int);
                    if peels_to_string(&ty) {
                        // Split (ptr, len) locals — matches every other
                        // String variable's representation (#2056). Plus a
                        // scratch local for the *MvlString pointer itself
                        // while it's being unpacked (see the emit-side
                        // `Pattern::Some` arm).
                        locals.push((format!("{name}_ptr"), Ty::Bool)); // i32
                        locals.push((format!("{name}_len"), Ty::Bool)); // i32
                        locals.push((mvl_some_string_temp_name(span), Ty::Bool));
                    // i32
                    } else {
                        locals.push((name.clone(), ty));
                    }
                }
            }
        }
        Pattern::Ok { inner, span } => {
            if let Pattern::Ident(name, _) = inner.as_ref() {
                if name != "_" {
                    // Bind at the Result's actual Ok-payload type (#2038) — a
                    // hardcoded `Ty::Int` here declares e.g. a Float
                    // payload's local as `i64`, then `local.set` on the
                    // `f64.reinterpret_i64`'d value fails validation.
                    let ty = result_ok_ty(scrutinee_ty).cloned().unwrap_or(Ty::Int);
                    if peels_to_string(&ty) {
                        // Split (ptr, len) locals — mirrors `Pattern::Some`'s
                        // String handling above (#2056) and #2076's
                        // `read_file`, the first Ok=String builtin. Plus a
                        // scratch local for the `*MvlString` pointer itself
                        // while it's being unpacked (see the emit-side
                        // `Pattern::Ok` arm).
                        locals.push((format!("{name}_ptr"), Ty::Bool)); // i32
                        locals.push((format!("{name}_len"), Ty::Bool)); // i32
                        locals.push((mvl_ok_string_temp_name(span), Ty::Bool)); // i32
                    } else {
                        locals.push((name.clone(), ty));
                    }
                }
            }
        }
        Pattern::Err { inner, .. } => match inner.as_ref() {
            Pattern::Ident(name, _) if name != "_" => {
                // Bind at the Result's actual Err-payload type (#2066) —
                // mirrors the Ok arm's #2038 fix above. A hardcoded
                // Ty::Bool (i32) here declared e.g. an Int/Float
                // payload's local as i32, then `local.set` on the
                // i64/f64-reinterpreted extraction value failed
                // validation. String keeps the i32 placeholder — it's
                // extracted as a raw *MvlString pointer, not unpacked
                // to (ptr, len).
                let err_ty = result_err_ty(scrutinee_ty).cloned().unwrap_or(Ty::String);
                let ty = if peels_to_string(&err_ty) {
                    Ty::Bool
                } else {
                    err_ty
                };
                locals.push((name.clone(), ty));
            }
            // Struct-shaped variant pattern with bound fields (e.g.
            // `Err(AuthError::AccountLocked { attempts })`) — names must
            // match what the emit-side `Pattern::Err` arm computes exactly:
            // (match span_offset, inner pattern's own span offset).
            Pattern::Struct {
                fields: named_fields,
                span: inner_span,
                ..
            } => {
                locals.push((
                    format!("__ev_{}_{}", span_offset, inner_span.offset),
                    Ty::Bool, // i32 placeholder
                ));
                locals.push((
                    format!("__epp_{}_{}", span_offset, inner_span.offset),
                    Ty::Bool, // i32 placeholder
                ));
                for (_, pat) in named_fields {
                    if let Pattern::Ident(bound, _) = pat {
                        if bound != "_" && !bound.contains("::") {
                            // Real field type isn't resolvable here without a
                            // payload_enums lookup this pre-pass doesn't have
                            // access to (same #2073-class limitation as the
                            // TupleStruct/Struct arms below) — Int/Bool-shaped
                            // fields (the common case) still declare correctly.
                            locals.push((bound.clone(), Ty::Int));
                            locals.push((format!("{bound}_ptr"), Ty::Bool));
                            locals.push((format!("{bound}_len"), Ty::Bool));
                            locals
                                .push((format!("__svs_{}_{}", inner_span.offset, bound), Ty::Bool));
                        }
                    }
                }
            }
            _ => {}
        },
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
                    // Qualified names (e.g. `Inner::A`) are nested-variant
                    // guards, not bindings — no local is declared for them
                    // (#2029).
                    if n != "_" && !n.contains("::") {
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
        Pattern::Struct {
            name: vname,
            fields: named_fields,
            span,
            ..
        } => {
            locals.push((
                format!("__pp_{}_{}", span_offset, span.offset),
                Ty::Bool, // i32 placeholder
            ));
            let _ = vname;
            for (n, pat) in named_fields {
                if let Pattern::Ident(bound, _) = pat {
                    if bound != "_" && !bound.contains("::") {
                        // Real field type isn't resolvable here without a
                        // payload_enums lookup this pre-pass doesn't have
                        // access to (same limitation as #2073's TupleStruct
                        // gap above) — Int/Bool-shaped fields (the common
                        // case) still declare correctly; a struct/i32-shaped
                        // field would need the same ctx-threading fix as #2073.
                        let _ = n;
                        locals.push((bound.clone(), Ty::Int));
                        locals.push((format!("{bound}_ptr"), Ty::Bool));
                        locals.push((format!("{bound}_len"), Ty::Bool));
                        locals.push((format!("__svs_{}_{}", span.offset, bound), Ty::Bool));
                    }
                }
            }
        }
        _ => {}
    }
}

/// If `pattern` names a qualified enum variant (e.g. `Weekday::Mon`, bare or
/// as an empty `TupleStruct`, or a struct-shaped `Variant { field: pat }`),
/// return that name. `None` for wildcards, plain bindings, and payload
/// sub-patterns. Mirrors the LLVM backend's `qualified_variant_name`.
fn wasm_qualified_variant_name(pattern: &Pattern) -> Option<&str> {
    match pattern {
        Pattern::TupleStruct { name, .. } => Some(name.as_str()),
        Pattern::Struct { name, .. } => Some(name.as_str()),
        Pattern::Ident(name, _) if name.contains("::") => Some(name.as_str()),
        _ => None,
    }
}

/// Resolve a qualified enum-variant name (e.g. `AuthError::AccountLocked`) to
/// its `PayloadVariant`, if the owning type is a payload-carrying enum.
/// `None` for pure-unit enums (their discriminant lives in `ctx.enum_variants`
/// instead) or unresolvable names.
fn payload_variant_for<'a>(qname: &str, ctx: &'a Ctx) -> Option<&'a PayloadVariant> {
    let type_name = qname.split_once("::").map(|(t, _)| t).unwrap_or("");
    ctx.payload_enums
        .get(type_name)
        .and_then(|info| info.variants.iter().find(|v| v.name == qname))
}

/// WASM equality opcode for a scrutinee type. Types beyond scalar defaults
/// (String, structs, etc.) fall back to `i64.eq` which is wrong for them —
/// but the pattern arm would have hit the unsupported case before this
/// runs, so nothing hits the wrong branch in practice.
fn eq_op_for(ty: &Ty, ctx: &Ctx) -> &'static str {
    if is_float_ctx(ty, ctx) {
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
            TirMatchBody::Block(b) => block_trailing_ty(b, ctx)?,
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

/// Temp local holding the `*MvlString` pointer while a `Some(v)` match arm
/// on `Option[String]` unpacks it into the `v_ptr`/`v_len` locals every
/// other String variable uses (#2056). Keyed by the `Some(...)` pattern's
/// own span, not the arm body's — the same pattern can't collide with
/// itself across arms in one match.
fn mvl_some_string_temp_name(pattern_span: &Span) -> String {
    format!("__mvs_{}_{}", pattern_span.offset, pattern_span.len)
}

/// Temp local holding the `*MvlString` pointer while an `Ok(v)` match arm
/// on `Result[String, E]` unpacks it into the `v_ptr`/`v_len` locals every
/// other String variable uses (#2076, mirrors `mvl_some_string_temp_name`
/// for #2056). Keyed by the `Ok(...)` pattern's own span.
fn mvl_ok_string_temp_name(pattern_span: &Span) -> String {
    format!("__mvo_{}_{}", pattern_span.offset, pattern_span.len)
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

/// True if `val_ty` can be stored in a `Map[String, V]`'s i64 value slot —
/// everything [`emit_map_value_push`] knows how to encode. `Float` is the
/// one gap: reading it back out would need `f64.reinterpret_i64` wiring
/// that hasn't landed, and no corpus fixture exercises it yet (#2024).
fn map_value_supported(val_ty: &Ty, ctx: &Ctx) -> bool {
    !is_float_ctx(val_ty, ctx)
}

/// Push a `Map[String, V]` value onto the stack in the i64 shape
/// `_mvl_map_insert_si64` expects. The map's value slot is a single i64 —
/// same "one wide slot for everything" convention `MvlOption`/`MvlResult`
/// use (`option_ops_for` doc comment above) — so i32-shaped payloads
/// (collection/Option/Result pointers, Bool/Byte, enum discriminants) are
/// zero-extended, and `String` is boxed into a `*MvlString` first, then
/// extended. `Int`/`UInt` are already i64 and pass through unchanged.
/// Callers must reject `Float` via [`map_value_supported`] first (#2024).
fn emit_map_value_push(out: &mut String, val_expr: &TirExpr, val_ty: &Ty, ctx: &Ctx) {
    if is_string_ty(val_ty, ctx) {
        emit_expr(out, val_expr, ctx); // (ptr, len)
        out.push_str("    call $_mvl_string_new\n");
        out.push_str("    i64.extend_i32_u\n");
    } else if is_i32(val_ty, ctx) {
        emit_expr(out, val_expr, ctx);
        out.push_str("    i64.extend_i32_u\n");
    } else {
        emit_expr(out, val_expr, ctx);
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
    // String elements are stored as *MvlString (i32 pointer); is_i32 does not
    // match Ty::String, so the two conditions are checked together here.
    if is_string_ty(elem_ty, ctx) || is_i32(elem_ty, ctx) {
        4
    } else {
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
/// back to i64 (Int, UInt). Float payloads also use the i64 variants —
/// callers must bit-cast with `i64.reinterpret_f64` / `f64.reinterpret_i64`
/// around the ctor/getter call, since there is no dedicated f64 runtime
/// helper (#2038).
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
/// names (no `$`). Float payloads reuse the i64 variants; callers must
/// bit-cast with `i64.reinterpret_f64` / `f64.reinterpret_i64` around the
/// ctor/getter call (#2038).
fn result_ops_for_ok(ok_ty: &Ty, ctx: &Ctx) -> (&'static str, &'static str) {
    if is_i32(ok_ty, ctx) {
        ("_mvl_result_ok_i32", "_mvl_result_value_i32")
    } else {
        ("_mvl_result_ok_i64", "_mvl_result_value_i64")
    }
}

/// Constructor and value-getter names for a `Result[T, E]` Err payload of
/// `err_ty`, for the non-String case — `peels_to_string(err_ty)` callers
/// route through `_mvl_result_err_str` instead, same split `Ok` already has
/// between this function and its own String special-case (#2066). The
/// getter is deliberately the *same* `_mvl_result_value_i32`/`_i64` pair
/// `Ok` uses: both constructors now store their payload in the shared
/// `ok_value` slot (`err_ptr` stays reserved for `_mvl_result_drop`'s
/// String-ownership marker).
fn result_ops_for_err(err_ty: &Ty, ctx: &Ctx) -> (&'static str, &'static str) {
    if is_i32(err_ty, ctx) {
        ("_mvl_result_err_i32", "_mvl_result_value_i32")
    } else {
        ("_mvl_result_err_i64", "_mvl_result_value_i64")
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
    if peels_to_string(&left.ty) {
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
    let eq_op = if is_float_ctx(&left.ty, ctx) {
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
            if is_float_ctx(&inner.ty, ctx) {
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
    if peels_to_string(&left.ty) && matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
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
    let (family, is_cmp_operand_float) = if is_float_ctx(&left.ty, ctx) {
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
        Ty::Named(name, args) if args.is_empty() => {
            // Generic type parameter substitution (e.g. T → Int in identity[T]).
            if let Some(concrete) = ctx.type_subst.get(name.as_str()) {
                return wasm_ty(concrete, ctx);
            }
            // Unit-variant enum.
            if ctx.enum_types.contains(name) {
                return "i32";
            }
            // Heap-allocated struct pointer (#1821).
            if ctx.struct_layouts.contains_key(name.as_str()) {
                return "i32";
            }
            // Heap-allocated payload-enum pointer (#1821).
            if ctx.payload_enums.contains_key(name.as_str()) {
                return "i32";
            }
            // Type alias (e.g. `type Probability = Float where ...`).
            if ctx.type_aliases.contains_key(name.as_str()) {
                return wasm_ty(&ctx.type_aliases[name.as_str()].clone(), ctx);
            }
            "i64"
        }
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
fn is_float_ctx(ty: &Ty, ctx: &Ctx) -> bool {
    match ty {
        Ty::Float => true,
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(concrete) = ctx.type_subst.get(name.as_str()) {
                return is_float_ctx(concrete, ctx);
            }
            if ctx.type_aliases.contains_key(name.as_str()) {
                return is_float_ctx(&ctx.type_aliases[name.as_str()].clone(), ctx);
            }
            false
        }
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            is_float_ctx(inner, ctx)
        }
        _ => false,
    }
}

/// True if this MVL type is a String (possibly via generic type param
/// resolution). Layers `ctx.type_subst` resolution on top of the ctx-free
/// [`peels_to_string`] peel so the two helpers share one definition of
/// "String, possibly wrapped in Ref/Labeled/Refined" instead of duplicating
/// it — `peels_to_string` stays the ctx-free version for call sites (like
/// `collect_locals_stmt`) that run without a `Ctx` in scope.
fn is_string_ty(ty: &Ty, ctx: &Ctx) -> bool {
    match ty {
        Ty::Named(name, args) if args.is_empty() => ctx
            .type_subst
            .get(name.as_str())
            .is_some_and(|concrete| is_string_ty(concrete, ctx)),
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            is_string_ty(inner, ctx)
        }
        _ => peels_to_string(ty),
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
        // Type alias (e.g. `type Alias = Inner`) — peel to the underlying
        // type, mirroring `wasm_ty`/`is_float_ctx`. Without this, a payload
        // field declared via an alias to a unit-variant enum falls through
        // to the `_ => false` default below, and `emit_payload_store` skips
        // the `i64.extend_i32_u` widen it needs for the 8-byte slot.
        Ty::Named(name, _) if ctx.type_aliases.contains_key(name.as_str()) => {
            is_i32(&ctx.type_aliases[name.as_str()].clone(), ctx)
        }
        Ty::List(_) | Ty::Array(_, _) | Ty::Set(_) | Ty::Map(_, _) => true,
        Ty::Option(_) | Ty::Result(_, _) => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => is_i32(inner, ctx),
        _ => false,
    }
}

/// Peels `Ref`/`Labeled`/`Refined` wrappers and registered type aliases down
/// to the underlying `Ty::Named` name, if any. Used to verify a qualified
/// nested-variant pattern's enum (e.g. `Inner` in `Inner::A`) actually
/// matches a payload field's declared type — including when that field is
/// declared via a type alias — before treating it as a valid discriminant
/// guard (#2029 follow-up: an unrelated enum with a colliding ordinal must
/// not be able to satisfy the guard).
fn underlying_named_ty(ty: &Ty, ctx: &Ctx) -> Option<String> {
    match ty {
        Ty::Named(name, _) => match ctx.type_aliases.get(name.as_str()) {
            Some(aliased) => underlying_named_ty(aliased, ctx),
            None => Some(name.clone()),
        },
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            underlying_named_ty(inner, ctx)
        }
        _ => None,
    }
}

// ── Type alias registry ─────────────────────────────────────────────────
//
// Pre-scan `TirProgram.types` for `TirTypeBody::Alias` declarations.
// These are transparent type aliases — `type Probability = Float where ...`.
// `wasm_ty` looks them up to resolve the underlying WASM primitive type so
// that e.g. a refined Float alias emits `f64` locals, not `i64`.

fn collect_type_aliases(types: &[TirTypeDecl]) -> HashMap<String, Ty> {
    let mut aliases = HashMap::new();
    for td in types {
        if let TirTypeBody::Alias(target) = &td.body {
            aliases.insert(td.name.clone(), target.clone());
        }
    }
    aliases
}

// ── Generic function monomorphization ───────────────────────────────────
//
// The TIR preserves generic functions in un-substituted form (type params
// like `T`, `A`, `B` remain as `Ty::Named`). The WASM backend must emit a
// specialized copy per unique concrete instantiation seen at call sites.
//
// Algorithm:
//   1. Scan all non-generic function bodies for FnCall nodes whose callee
//      has type_params.
//   2. Infer the type substitution by matching each generic param type
//      (e.g. `x: T`) against the corresponding call-site arg expression type.
//   3. Emit one WASM function per unique (fn_name, type_subst) pair, using
//      a mangled name (e.g. `identity__Int`, `pair_first__Int__Str`).
//   4. At call sites, replace `call $fn_name` with `call $mangled_name`.

/// A short tag for a MVL type used in generic name mangling.
fn mangle_ty_tag(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::UInt => "Int".to_string(),
        Ty::Float => "Float".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Byte => "Byte".to_string(),
        Ty::String => "Str".to_string(),
        Ty::Named(name, _) => name.clone(),
        Ty::Option(inner) => format!("Opt_{}", mangle_ty_tag(inner)),
        Ty::List(inner) => format!("List_{}", mangle_ty_tag(inner)),
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => mangle_ty_tag(inner),
        _ => "Unknown".to_string(),
    }
}

/// Produce the mangled WASM function name for a generic instantiation.
/// E.g. ("identity", ["T"], {"T": Int}) → "identity__Int"
fn mangle_generic_name(
    fn_name: &str,
    type_params: &[GenericParam],
    subst: &HashMap<String, Ty>,
) -> String {
    let mut name = fn_name.to_string();
    for gp in type_params {
        let tag = subst
            .get(gp.name())
            .map(mangle_ty_tag)
            .unwrap_or_else(|| "Unknown".to_string());
        name.push_str("__");
        name.push_str(&tag);
    }
    name
}

/// Infer a type substitution for a generic function call from the arg types.
/// Matches each param type of the form `Ty::Named(param_name)` against the
/// corresponding arg expression type to build the substitution.
fn infer_type_subst(generic_fn: &TirFn, args: &[TirExpr]) -> HashMap<String, Ty> {
    let param_names: std::collections::HashSet<String> = generic_fn
        .type_params
        .iter()
        .map(|gp| gp.name().to_string())
        .collect();

    let mut subst = HashMap::new();
    for (param, arg) in generic_fn.params.iter().zip(args.iter()) {
        if let Ty::Named(ref tname, ref targs) = param.ty {
            if targs.is_empty() && param_names.contains(tname.as_str()) {
                subst.entry(tname.clone()).or_insert_with(|| arg.ty.clone());
            }
        }
    }
    subst
}

/// Infer a type substitution at a call site by matching param types against arg types.
/// `fn_params` are the generic function's formal parameters (with generic type names).
fn infer_type_subst_from_args(
    type_params: &[GenericParam],
    fn_params: &[TirParam],
    args: &[TirExpr],
) -> HashMap<String, Ty> {
    let param_names: std::collections::HashSet<String> =
        type_params.iter().map(|gp| gp.name().to_string()).collect();
    let mut subst = HashMap::new();
    for (param, arg) in fn_params.iter().zip(args.iter()) {
        if let Ty::Named(ref tname, ref targs) = param.ty {
            if targs.is_empty() && param_names.contains(tname.as_str()) {
                subst.entry(tname.clone()).or_insert_with(|| arg.ty.clone());
            }
        }
    }
    subst
}

/// Scan all non-generic function bodies for calls to generic functions.
/// Returns unique (generic_fn_ref, type_subst, mangled_name) triples.
fn collect_generic_instantiations<'a>(
    fns: &[&'a TirFn],
    all_fns: &[&'a TirFn],
    actors: &[TirActorDecl],
    _ctx: &Ctx,
) -> Vec<(&'a TirFn, HashMap<String, Ty>, String)> {
    // Build lookup: fn_name → TirFn for generic fns
    let generic_fns: HashMap<&str, &TirFn> = all_fns
        .iter()
        .filter(|f| !f.type_params.is_empty())
        .map(|f| (f.name.as_str(), *f))
        .collect();

    if generic_fns.is_empty() {
        return vec![];
    }

    let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut result = vec![];

    for f in fns {
        collect_instantiations_in_block(&f.body, &generic_fns, &mut seen, &mut result);
    }
    // Actor method bodies are emitted as functions but are not in `tir.fns`, so
    // a generic called only from a behaviour would never be instantiated and the
    // module referenced a symbol that was never emitted (#2012). Same gap the
    // literal walker had.
    for ad in actors {
        for m in &ad.methods {
            collect_instantiations_in_block(&m.body, &generic_fns, &mut seen, &mut result);
        }
    }
    result
}

fn collect_instantiations_in_block<'a>(
    block: &TirBlock,
    generic_fns: &HashMap<&str, &'a TirFn>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    for stmt in &block.stmts {
        collect_instantiations_in_stmt(stmt, generic_fns, seen, result);
    }
}

fn collect_instantiations_in_stmt<'a>(
    stmt: &TirStmt,
    generic_fns: &HashMap<&str, &'a TirFn>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    match stmt {
        TirStmt::Expr { expr, .. }
        | TirStmt::Return {
            value: Some(expr), ..
        } => {
            collect_instantiations_in_expr(expr, generic_fns, seen, result);
        }
        TirStmt::Let { init, .. } | TirStmt::Assign { value: init, .. } => {
            collect_instantiations_in_expr(init, generic_fns, seen, result);
        }
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_instantiations_in_expr(cond, generic_fns, seen, result);
            collect_instantiations_in_block(then, generic_fns, seen, result);
            match else_ {
                Some(TirElseBranch::Block(b)) => {
                    collect_instantiations_in_block(b, generic_fns, seen, result);
                }
                Some(TirElseBranch::If(s)) => {
                    collect_instantiations_in_stmt(s, generic_fns, seen, result);
                }
                None => {}
            }
        }
        TirStmt::While { cond, body, .. }
        | TirStmt::For {
            iter: cond, body, ..
        } => {
            collect_instantiations_in_expr(cond, generic_fns, seen, result);
            collect_instantiations_in_block(body, generic_fns, seen, result);
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_instantiations_in_expr(scrutinee, generic_fns, seen, result);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => {
                        collect_instantiations_in_expr(e, generic_fns, seen, result);
                    }
                    TirMatchBody::Block(b) => {
                        collect_instantiations_in_block(b, generic_fns, seen, result);
                    }
                }
            }
        }
        _ => {}
    }
}

fn collect_instantiations_in_expr<'a>(
    expr: &TirExpr,
    generic_fns: &HashMap<&str, &'a TirFn>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    if let TirExprKind::FnCall { name, args, .. } = &expr.kind {
        if let Some(gf) = generic_fns.get(name.as_str()) {
            let subst = infer_type_subst(gf, args);
            if subst.len() == gf.type_params.len() {
                let mangled = mangle_generic_name(&gf.name, &gf.type_params, &subst);
                if seen.insert(mangled.clone(), ()).is_none() {
                    result.push((gf, subst, mangled));
                }
            }
        }
        for a in args {
            collect_instantiations_in_expr(a, generic_fns, seen, result);
        }
    }
    // Recurse into sub-expressions.
    match &expr.kind {
        TirExprKind::Unary { expr: inner, .. }
        | TirExprKind::Consume(inner)
        | TirExprKind::Borrow { expr: inner, .. } => {
            collect_instantiations_in_expr(inner, generic_fns, seen, result);
        }
        TirExprKind::Binary { left, right, .. } => {
            collect_instantiations_in_expr(left, generic_fns, seen, result);
            collect_instantiations_in_expr(right, generic_fns, seen, result);
        }
        TirExprKind::If { cond, then, else_ } => {
            collect_instantiations_in_expr(cond, generic_fns, seen, result);
            collect_instantiations_in_block(then, generic_fns, seen, result);
            if let Some(e) = else_ {
                collect_instantiations_in_expr(e, generic_fns, seen, result);
            }
        }
        TirExprKind::Block(b) => {
            collect_instantiations_in_block(b, generic_fns, seen, result);
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_instantiations_in_expr(scrutinee, generic_fns, seen, result);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => {
                        collect_instantiations_in_expr(e, generic_fns, seen, result);
                    }
                    TirMatchBody::Block(b) => {
                        collect_instantiations_in_block(b, generic_fns, seen, result);
                    }
                }
            }
        }
        _ => {} // FnCall handled above; others don't need recursion for generics
    }
}

/// Emit a monomorphized copy of a generic function.
/// `type_subst` maps type param names to concrete types.
/// `mangled_name` is the WASM function name to emit (e.g. "identity__Int").
fn emit_generic_fn(
    out: &mut String,
    f: &TirFn,
    type_subst: &HashMap<String, Ty>,
    mangled_name: &str,
    ctx: &Ctx,
) {
    // Build a temporary Ctx with the type_subst active so wasm_ty resolves
    // type params to their concrete types.
    let mono_ctx = Ctx {
        needs_wasi: ctx.needs_wasi,
        literals: ctx.literals,
        audit_relabels: ctx.audit_relabels,
        enum_types: ctx.enum_types,
        enum_variants: ctx.enum_variants,
        struct_layouts: ctx.struct_layouts,
        payload_enums: ctx.payload_enums,
        type_aliases: ctx.type_aliases,
        type_subst,
        generic_fn_map: ctx.generic_fn_map,
        label_counter: Cell::new(ctx.label_counter.get()),
        needs_runtime: Cell::new(ctx.needs_runtime.get()),
        string_params: std::cell::RefCell::new(std::collections::HashSet::new()),
        assert_mode: ctx.assert_mode,
        fn_locals: std::cell::RefCell::new(Vec::new()),
        fn_let_inits: std::cell::RefCell::new(HashMap::new()),
        actors: ctx.actors,
        // A monomorphized instantiation is a different function, not a
        // continuation of whatever triggered it — reset like `string_params`
        // and `fn_locals` above, or `self.field` inside the generic body would
        // resolve against the caller's actor layout (#2012).
        self_type: std::cell::RefCell::new(None),
        struct_methods: ctx.struct_methods,
    };

    // Set up string_params for params whose concrete type is String.
    {
        let mut sp = mono_ctx.string_params.borrow_mut();
        for p in &f.params {
            let concrete = resolve_ty_param(&p.ty, type_subst);
            if peels_to_string(&concrete) {
                sp.insert(p.name.clone());
            }
        }
    }

    // Emit function signature with concrete param/return types.
    out.push_str(&format!("  (func ${mangled_name}"));
    for p in &f.params {
        let concrete = resolve_ty_param(&p.ty, type_subst);
        if peels_to_string(&concrete) {
            out.push_str(&format!(
                " (param ${}_ptr i32) (param ${}_len i32)",
                p.name, p.name
            ));
        } else {
            out.push_str(&format!(
                " (param ${} {})",
                p.name,
                wasm_ty(&concrete, &mono_ctx)
            ));
        }
    }
    // Return type.
    let concrete_ret = resolve_ty_param(&f.ret_ty, type_subst);
    if !matches!(concrete_ret, Ty::Unit) {
        if peels_to_string(&concrete_ret) {
            out.push_str(" (result i32 i32)");
        } else {
            out.push_str(&format!(" (result {})", wasm_ty(&concrete_ret, &mono_ctx)));
        }
    }
    out.push('\n');

    // Emit locals.
    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_block(&f.body, &mut locals);
    collect_locals_ctx(&f.body, &mut locals, &mono_ctx);
    dedup_locals_keep_last(&mut locals);
    for (name, ty) in &locals {
        let concrete = resolve_ty_param(ty, type_subst);
        if peels_to_string(&concrete) {
            out.push_str(&format!("    (local ${name}_ptr i32)\n"));
            out.push_str(&format!("    (local ${name}_len i32)\n"));
        } else {
            out.push_str(&format!(
                "    (local ${name} {})\n",
                wasm_ty(&concrete, &mono_ctx)
            ));
        }
    }

    // Emit body.
    let mut body_buf = String::new();
    emit_block(&mut body_buf, &f.body, &mono_ctx);

    if body_buf.contains(";; unsupported") {
        out.push_str("    ;; body stubbed — contained unsupported constructs\n");
        out.push_str("    unreachable\n");
    } else {
        out.push_str(&body_buf);
    }
    out.push_str("  )\n");

    // Propagate needs_runtime back.
    if mono_ctx.needs_runtime.get() {
        ctx.needs_runtime.set(true);
    }
}

/// Resolve a type that may be a generic type param name.
fn resolve_ty_param(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(concrete) = subst.get(name.as_str()) {
                concrete.clone()
            } else {
                ty.clone()
            }
        }
        Ty::Ref(m, inner) => Ty::Ref(*m, Box::new(resolve_ty_param(inner, subst))),
        Ty::Refined(inner, pred) => {
            Ty::Refined(Box::new(resolve_ty_param(inner, subst)), pred.clone())
        }
        _ => ty.clone(),
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

// ── Actor collection and emission (#2012, ADR-0059) ─────────────────────

/// Build the actor registry and register each actor's state layout in
/// `layouts` under the actor's own name.
///
/// Registering the layout there is deliberate: an actor handle is represented
/// as its state pointer, so `wasm_ty`, `emit_field_access`, and `is_i32` all
/// treat `Counter` exactly as they treat a struct — no actor-specific cases in
/// any of them.
fn collect_actors(
    actors: &[TirActorDecl],
    layouts: &mut HashMap<String, StructLayout>,
) -> HashMap<String, ActorInfo> {
    let mut map = HashMap::new();
    for (idx, ad) in actors.iter().enumerate() {
        let mut offset = 0u32;
        let mut slots = Vec::new();
        for f in &ad.fields {
            let size = field_byte_size(&f.ty);
            let align = field_alignment(&f.ty);
            offset = (offset + align - 1) & !(align - 1);
            slots.push(FieldSlot {
                name: f.name.clone(),
                offset,
                ty: f.ty.clone(),
            });
            offset += size;
        }
        let total = (offset + 7) & !7;
        layouts.insert(
            ad.name.clone(),
            StructLayout {
                total_size: total.max(8),
                fields: slots,
            },
        );

        let behaviors: Vec<TirActorMethod> = ad
            .methods
            .iter()
            .filter(|m| m.is_public && !m.is_test)
            .cloned()
            .collect();
        let test_methods: Vec<TirActorMethod> = ad
            .methods
            .iter()
            .filter(|m| m.is_public && m.is_test)
            .cloned()
            .collect();

        map.insert(
            ad.name.clone(),
            ActorInfo {
                name: ad.name.clone(),
                snake: actor_name_to_snake(&ad.name),
                tag: idx as i32,
                behaviors,
                test_methods,
                methods: ad.methods.clone(),
            },
        );
    }
    map
}

/// Emit every actor's method bodies plus its behaviour-discriminant dispatch.
fn emit_actor_decls(out: &mut String, actors: &[TirActorDecl], ctx: &Ctx) {
    // Deterministic order — the emitted WAT must not depend on HashMap
    // iteration.
    let mut sorted: Vec<&TirActorDecl> = actors.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for ad in sorted {
        let Some(info) = ctx.actors.get(&ad.name) else {
            continue;
        };
        for m in &ad.methods {
            emit_actor_method(out, info, m, ctx);
        }
        emit_actor_dispatch(out, info, ctx);
    }
}

/// Emit one actor method as `$<snake>_<method>(self, params…)`.
///
/// `self` is the state pointer, so `self.field` reads and writes reuse the
/// struct field path — see [`collect_actors`].
fn emit_actor_method(out: &mut String, info: &ActorInfo, m: &TirActorMethod, ctx: &Ctx) {
    *ctx.self_type.borrow_mut() = Some(info.name.clone());
    // `peels_to_string`, not `matches!(Ty::String)` — an IFC-labeled or refined
    // String (`Tainted[String]`, `String where …`) is still a split (ptr, len)
    // param. Using the bare match emitted a single `$raw` param while the body
    // referenced `$raw_ptr`, producing a module that would not assemble. Must
    // stay in step with `emit_fn` (#2012, #2013).
    *ctx.string_params.borrow_mut() = m
        .params
        .iter()
        .filter(|p| peels_to_string(&p.ty))
        .map(|p| p.name.clone())
        .collect();

    let fn_name = format!("{}_{}", info.snake, m.name);
    out.push_str(&format!("  (func ${fn_name} (param $self i32)"));
    for p in &m.params {
        if peels_to_string(&p.ty) {
            out.push_str(&format!(
                " (param ${}_ptr i32) (param ${}_len i32)",
                p.name, p.name
            ));
        } else {
            out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty, ctx)));
        }
    }
    if peels_to_string(&m.ret_ty) {
        out.push_str(" (result i32 i32)");
    } else if !matches!(m.ret_ty, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&m.ret_ty, ctx)));
    }
    out.push('\n');

    let mut body = String::new();
    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_block(&m.body, &mut locals);
    collect_locals_ctx(&m.body, &mut locals, ctx);
    dedup_locals_keep_last(&mut locals);
    for (name, ty) in &locals {
        body.push_str(&format!("    (local ${} {})\n", name, wasm_ty(ty, ctx)));
    }
    *ctx.fn_locals.borrow_mut() = locals.clone();
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&m.body, &mut let_inits);
    *ctx.fn_let_inits.borrow_mut() = let_inits;
    emit_block(&mut body, &m.body, ctx);

    if body.contains(";; unsupported") {
        out.push_str("    ;; body stubbed — contained unsupported constructs\n");
        out.push_str("    unreachable\n");
    } else {
        out.push_str(&body);
    }
    out.push_str("  )\n");
    *ctx.self_type.borrow_mut() = None;
}

/// Emit a user-defined extension method (`fn Type::method(self, ...) { ... }`)
/// as `${receiver_type}_${method}` (#2054).
///
/// Unlike actor methods, `self` is already an ordinary entry in `f.params`
/// (the parser/checker treat it as `params[0]` with `ty = Ty::Named(receiver_type,
/// [])`), so the param-emission loop below is the same as `emit_fn`'s — no
/// synthesized `(param $self i32)` needed. `ctx.self_type` is still bound for
/// the body so `self.field = …` writes can resolve their layout (see
/// `emit_field_assign`), same as actor methods.
///
/// The mangled name is receiver-qualified rather than the bare `f.name`
/// because two different structs may declare a method with the same name
/// (the checker's `method_table` is keyed per-receiver-type) — a bare-name
/// symbol would collide.
fn emit_extension_method(out: &mut String, f: &TirFn, ctx: &Ctx) {
    let receiver_type = f
        .receiver_type
        .as_deref()
        .expect("emit_extension_method called on a fn without a receiver_type");
    *ctx.self_type.borrow_mut() = Some(receiver_type.to_string());
    *ctx.string_params.borrow_mut() = f
        .params
        .iter()
        .filter(|p| peels_to_string(&p.ty))
        .map(|p| p.name.clone())
        .collect();

    let wasm_name = format!("{receiver_type}_{}", f.name);
    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        if peels_to_string(&p.ty) {
            out.push_str(&format!(
                " (param ${}_ptr i32) (param ${}_len i32)",
                p.name, p.name
            ));
        } else {
            out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty, ctx)));
        }
    }
    if peels_to_string(&f.ret_ty) {
        out.push_str(" (result i32 i32)");
    } else if !matches!(f.ret_ty, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&f.ret_ty, ctx)));
    }
    out.push('\n');

    let mut body = String::new();
    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_block(&f.body, &mut locals);
    collect_locals_ctx(&f.body, &mut locals, ctx);
    dedup_locals_keep_last(&mut locals);
    for (name, ty) in &locals {
        body.push_str(&format!("    (local ${} {})\n", name, wasm_ty(ty, ctx)));
    }
    *ctx.fn_locals.borrow_mut() = locals.clone();
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&f.body, &mut let_inits);
    *ctx.fn_let_inits.borrow_mut() = let_inits;
    emit_block(&mut body, &f.body, ctx);

    if body.contains(";; unsupported") {
        out.push_str("    ;; body stubbed — contained unsupported constructs\n");
        out.push_str("    unreachable\n");
    } else {
        out.push_str(&body);
        // Same implicit-return exclusion as emit_fn (#2023, #2052): a trailing
        // bare-expression method body (e.g. `fn Type::to_string(self) -> String
        // { "...".concat(...) }`, no `return` keyword) must not have its own
        // *MvlString result freed by the blanket drop sweep before the caller
        // reads it.
        let implicit_excludes = match f.body.stmts.last() {
            Some(TirStmt::Expr { expr, .. }) => exclude_returned_locals(expr, ctx),
            _ => Vec::new(),
        };
        emit_fn_heap_drops(out, &locals, &implicit_excludes);
    }
    out.push_str("  )\n");
    *ctx.self_type.borrow_mut() = None;
}

/// Emit `base.field = value` as a typed store through the base pointer.
///
/// `LValue` carries no type, so the base's struct name comes from
/// [`Ctx::self_type`] for `self` and from the collected locals otherwise.
fn emit_field_assign(out: &mut String, base: &LValue, field: &str, value: &TirExpr, ctx: &Ctx) {
    let LValue::Ident(base_name, _) = base else {
        out.push_str("    ;; unsupported nested field assignment target\n");
        return;
    };

    let type_name = if base_name == "self" {
        ctx.self_type.borrow().clone()
    } else {
        ctx.fn_locals
            .borrow()
            .iter()
            .find(|(n, _)| n == base_name)
            .and_then(|(_, ty)| named_type_name(ty))
    };
    let Some(type_name) = type_name else {
        out.push_str(&format!(
            "    ;; unsupported assign target: unresolved base type for {base_name}.{field}\n"
        ));
        return;
    };
    let Some(layout) = ctx.struct_layouts.get(&type_name) else {
        out.push_str(&format!(
            "    ;; unsupported assign target: unknown struct {type_name}\n"
        ));
        return;
    };
    let Some(slot) = layout.fields.iter().find(|s| s.name == field).cloned() else {
        out.push_str(&format!(
            "    ;; unsupported assign target: unknown field {type_name}.{field}\n"
        ));
        return;
    };

    // Overwriting a *MvlString handle leaks the old allocation, so it has to be
    // released — but only AFTER the new value exists. The right-hand side may
    // read the very field being overwritten (`self.s = f(self.s)`), and dropping
    // first frees the string the RHS then reads: a use-after-free that shows up
    // as `copy_nonoverlapping requires ... non-null` inside the runtime.
    if peels_to_string(&slot.ty) {
        ctx.needs_runtime.set(true);
        let tmp = field_assign_temp_name(value);
        emit_expr(out, value, ctx); // leaves (ptr, len)
        out.push_str("    call $_mvl_string_new\n");
        out.push_str(&format!("    local.set ${tmp}\n"));
        out.push_str(&format!("    local.get ${base_name}\n"));
        out.push_str(&format!("    i32.load offset={}\n", slot.offset));
        out.push_str("    call $_mvl_string_drop\n");
        out.push_str(&format!("    local.get ${base_name}\n"));
        out.push_str(&format!("    local.get ${tmp}\n"));
        out.push_str(&format!("    i32.store offset={}\n", slot.offset));
        return;
    }

    out.push_str(&format!("    local.get ${base_name}\n"));
    emit_struct_store(out, value, &slot.ty, slot.offset, ctx);
}

/// Scratch local holding the new `*MvlString` while the previous handle in a
/// `base.field = …` assignment is released. Span-keyed so the locals-collection
/// pass and the emit path agree.
fn field_assign_temp_name(value: &TirExpr) -> String {
    format!("__fa_{}_{}", value.span.offset, value.span.len)
}

/// Strip wrappers and return the underlying `Ty::Named` name, if any.
fn named_type_name(ty: &Ty) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) => cur = inner,
            Ty::Named(n, _) => return Some(n.clone()),
            _ => return None,
        }
    }
}

/// True if `method` is a user-defined extension method registered on
/// `receiver`'s named type (#2054, #2058 follow-up). Consulted by any
/// dispatch arm that would otherwise match purely on method name (e.g.
/// `to_string`, checked below) so a struct's own `to_string`/etc. extension
/// method isn't shadowed by a builtin-type special case that never
/// considered the receiver's type.
fn is_struct_method_call(receiver: &TirExpr, method: &str, ctx: &Ctx) -> bool {
    named_type_name(&receiver.ty)
        .is_some_and(|t| ctx.struct_methods.contains(&(t, method.to_string())))
}

/// Emit `$<snake>_dispatch(state, disc, args)` — a discriminant switch that
/// unpacks each behaviour's arguments from the message slot and calls it.
fn emit_actor_dispatch(out: &mut String, info: &ActorInfo, ctx: &Ctx) {
    out.push_str(&format!(
        "  (func ${}_dispatch (param $state i32) (param $disc i32) (param $args i32)\n",
        info.snake
    ));
    // Scratch for unpacking a *MvlString argument into (ptr, len). Declared
    // unconditionally — an unused local is free.
    out.push_str("    (local $__amstr i32)\n");
    for (disc, m) in info.behaviors.iter().enumerate() {
        out.push_str("    local.get $disc\n");
        out.push_str(&format!("    i32.const {disc}\n"));
        out.push_str("    i32.eq\n");
        out.push_str("    if\n");
        out.push_str("      local.get $state\n");
        for (j, p) in m.params.iter().enumerate() {
            let off = ACTOR_MSG_ARGS + (j as u32) * 8;
            emit_actor_arg_load(out, &p.ty, off, ctx);
        }
        out.push_str(&format!("      call ${}_{}\n", info.snake, m.name));
        // Behaviours are `Unit`; a non-Unit body value would be left dangling.
        if !matches!(m.ret_ty, Ty::Unit) {
            if peels_to_string(&m.ret_ty) {
                out.push_str("      drop\n      drop\n");
            } else {
                out.push_str("      drop\n");
            }
        }
        out.push_str("      return\n");
        out.push_str("    end\n");
    }
    out.push_str("  )\n");
}

/// Load one behaviour argument out of a message slot, widening back to the
/// parameter's WASM representation. Slots are uniformly 8 bytes.
fn emit_actor_arg_load(out: &mut String, ty: &Ty, off: u32, ctx: &Ctx) {
    match ty {
        _ if peels_to_string(ty) => {
            // Stored as a *MvlString handle; unpack to (ptr, len).
            ctx.needs_runtime.set(true);
            out.push_str("      local.get $args\n");
            out.push_str(&format!("      i32.load offset={off}\n"));
            out.push_str("      local.set $__amstr\n");
            out.push_str("      local.get $__amstr\n");
            out.push_str(&format!("      i32.load offset={MVL_STRING_OFFSET_PTR}\n"));
            out.push_str("      local.get $__amstr\n");
            out.push_str(&format!("      i32.load offset={MVL_STRING_OFFSET_LEN}\n"));
        }
        Ty::Float => {
            out.push_str("      local.get $args\n");
            out.push_str(&format!("      f64.load offset={off}\n"));
        }
        _ if is_i32(ty, ctx) => {
            out.push_str("      local.get $args\n");
            out.push_str(&format!("      i32.load offset={off}\n"));
        }
        _ => {
            out.push_str("      local.get $args\n");
            out.push_str(&format!("      i64.load offset={off}\n"));
        }
    }
}

/// Emit the module-wide actor scheduler: message queue globals, the enqueue
/// helper, and the drain loop with its re-entrancy guard.
///
/// Single-threaded run-to-completion (ADR-0059): `send` appends and then drains
/// unless a drain is already running. The guard is what makes a self-send queue
/// instead of recursing into dispatch until the stack traps.
fn emit_actor_scheduler(out: &mut String, ctx: &Ctx) {
    let queue_bytes = ACTOR_QUEUE_SLOTS * ACTOR_MSG_SIZE;

    out.push_str("  (global $__actor_q (mut i32) (i32.const 0))\n");
    out.push_str("  (global $__actor_head (mut i32) (i32.const 0))\n");
    out.push_str("  (global $__actor_tail (mut i32) (i32.const 0))\n");
    out.push_str("  (global $__actor_draining (mut i32) (i32.const 0))\n");

    // Reserve the next free message slot, allocating the queue on first use.
    out.push_str("  (func $__mvl_actor_slot (result i32)\n");
    out.push_str("    (local $slot i32)\n");
    out.push_str("    global.get $__actor_q\n");
    out.push_str("    i32.eqz\n");
    out.push_str("    if\n");
    out.push_str(&format!("      i32.const {queue_bytes}\n"));
    out.push_str("      call $_mvl_struct_alloc\n");
    out.push_str("      global.set $__actor_q\n");
    out.push_str("    end\n");
    // Overflow traps — a silent drop on a single-threaded target would be a
    // compiler bug, not backpressure (ADR-0059 §5).
    out.push_str("    global.get $__actor_tail\n");
    out.push_str(&format!("    i32.const {ACTOR_QUEUE_SLOTS}\n"));
    out.push_str("    i32.ge_u\n");
    out.push_str("    if\n");
    out.push_str("      unreachable\n");
    out.push_str("    end\n");
    out.push_str("    global.get $__actor_q\n");
    out.push_str("    global.get $__actor_tail\n");
    out.push_str(&format!("    i32.const {ACTOR_MSG_SIZE}\n"));
    out.push_str("    i32.mul\n");
    out.push_str("    i32.add\n");
    out.push_str("    local.set $slot\n");
    out.push_str("    global.get $__actor_tail\n");
    out.push_str("    i32.const 1\n");
    out.push_str("    i32.add\n");
    out.push_str("    global.set $__actor_tail\n");
    out.push_str("    local.get $slot\n");
    out.push_str("  )\n");

    // Route one message to its actor type's dispatch. Static switch on the
    // type tag — no funcref table (ADR-0059 §2).
    out.push_str("  (func $__mvl_actor_route (param $slot i32)\n");
    let mut sorted: Vec<&ActorInfo> = ctx.actors.values().collect();
    sorted.sort_by_key(|a| a.tag);
    for info in sorted {
        out.push_str("    local.get $slot\n");
        out.push_str(&format!("    i32.load offset={ACTOR_MSG_TAG}\n"));
        out.push_str(&format!("    i32.const {}\n", info.tag));
        out.push_str("    i32.eq\n");
        out.push_str("    if\n");
        out.push_str("      local.get $slot\n");
        out.push_str(&format!("      i32.load offset={ACTOR_MSG_STATE}\n"));
        out.push_str("      local.get $slot\n");
        out.push_str(&format!("      i32.load offset={ACTOR_MSG_DISC}\n"));
        out.push_str("      local.get $slot\n");
        out.push_str(&format!("      call ${}_dispatch\n", info.snake));
        out.push_str("      return\n");
        out.push_str("    end\n");
    }
    out.push_str("  )\n");

    // Drain to exhaustion, unless we are already inside a drain.
    out.push_str("  (func $__mvl_actor_pump\n");
    out.push_str("    global.get $__actor_draining\n");
    out.push_str("    if\n");
    out.push_str("      return\n");
    out.push_str("    end\n");
    out.push_str("    i32.const 1\n");
    out.push_str("    global.set $__actor_draining\n");
    out.push_str("    block $done\n");
    out.push_str("      loop $next\n");
    out.push_str("        global.get $__actor_head\n");
    out.push_str("        global.get $__actor_tail\n");
    out.push_str("        i32.ge_u\n");
    out.push_str("        br_if $done\n");
    out.push_str("        global.get $__actor_q\n");
    out.push_str("        global.get $__actor_head\n");
    out.push_str(&format!("        i32.const {ACTOR_MSG_SIZE}\n"));
    out.push_str("        i32.mul\n");
    out.push_str("        i32.add\n");
    out.push_str("        global.get $__actor_head\n");
    out.push_str("        i32.const 1\n");
    out.push_str("        i32.add\n");
    out.push_str("        global.set $__actor_head\n");
    out.push_str("        call $__mvl_actor_route\n");
    out.push_str("        br $next\n");
    out.push_str("      end\n");
    out.push_str("    end\n");
    // Queue is empty again — reset so the fixed slot region is reusable.
    out.push_str("    i32.const 0\n");
    out.push_str("    global.set $__actor_head\n");
    out.push_str("    i32.const 0\n");
    out.push_str("    global.set $__actor_tail\n");
    out.push_str("    i32.const 0\n");
    out.push_str("    global.set $__actor_draining\n");
    out.push_str("  )\n");
}

/// Emit `actor Name { field: value, … }` — allocate state, initialise fields,
/// leave the state pointer (the handle) on the stack.
fn emit_actor_spawn(
    out: &mut String,
    actor_type: &str,
    fields: &[(String, TirExpr)],
    expr: &TirExpr,
    ctx: &Ctx,
) {
    let Some(layout) = ctx.struct_layouts.get(actor_type) else {
        out.push_str(&format!("    ;; unsupported actor spawn: {actor_type}\n"));
        return;
    };
    ctx.needs_runtime.set(true);
    let temp = struct_temp_name(expr);
    out.push_str(&format!("    i32.const {}\n", layout.total_size));
    out.push_str("    call $_mvl_struct_alloc\n");
    out.push_str(&format!("    local.set ${temp}\n"));
    for slot in &layout.fields {
        let Some(val) = fields.iter().find(|(n, _)| n == &slot.name).map(|(_, e)| e) else {
            continue;
        };
        out.push_str(&format!("    local.get ${temp}\n"));
        emit_struct_store(out, val, &slot.ty, slot.offset, ctx);
    }
    out.push_str(&format!("    local.get ${temp}\n"));
}

/// Emit a call on an actor handle: either a fire-and-forget behaviour send or a
/// synchronous `pub test fn` read.
///
/// Returns `false` when `method` is not a public method of `actor_type`, so the
/// caller can fall through to the ordinary method-dispatch table.
fn emit_actor_method_call(
    out: &mut String,
    info: &ActorInfo,
    receiver: &TirExpr,
    method: &str,
    args: &[TirExpr],
    expr: &TirExpr,
    ctx: &Ctx,
) -> bool {
    // Synchronous read. The queue is empty at every statement boundary
    // (drain-at-send), so this is a plain direct call — no reply cell needed,
    // unlike the LLVM backend's `_mvl_actor_sync_call` (ADR-0059 §4).
    if let Some(m) = info.test_methods.iter().find(|m| m.name == method) {
        emit_expr(out, receiver, ctx);
        for a in args {
            emit_expr(out, a, ctx);
        }
        out.push_str(&format!("    call ${}_{}\n", info.snake, m.name));
        return true;
    }

    let Some(disc) = info.behaviors.iter().position(|m| m.name == method) else {
        return false;
    };
    let m = &info.behaviors[disc];
    if m.params.len() as u32 > ACTOR_MAX_ARGS {
        out.push_str(&format!(
            "    ;; unsupported actor behavior arity: {}.{method}\n",
            info.name
        ));
        return true;
    }
    ctx.needs_runtime.set(true);

    let slot = actor_msg_temp_name(expr);
    out.push_str("    call $__mvl_actor_slot\n");
    out.push_str(&format!("    local.set ${slot}\n"));
    // Receiver, type tag, discriminant.
    out.push_str(&format!("    local.get ${slot}\n"));
    emit_expr(out, receiver, ctx);
    out.push_str(&format!("    i32.store offset={ACTOR_MSG_STATE}\n"));
    out.push_str(&format!("    local.get ${slot}\n"));
    out.push_str(&format!("    i32.const {}\n", info.tag));
    out.push_str(&format!("    i32.store offset={ACTOR_MSG_TAG}\n"));
    out.push_str(&format!("    local.get ${slot}\n"));
    out.push_str(&format!("    i32.const {disc}\n"));
    out.push_str(&format!("    i32.store offset={ACTOR_MSG_DISC}\n"));
    // Arguments, one 8-byte slot each.
    for (j, (param, arg)) in m.params.iter().zip(args.iter()).enumerate() {
        let off = ACTOR_MSG_ARGS + (j as u32) * 8;
        out.push_str(&format!("    local.get ${slot}\n"));
        emit_struct_store(out, arg, &param.ty, off, ctx);
    }
    out.push_str("    call $__mvl_actor_pump\n");
    true
}

/// Emit `self.method(args…)` inside an actor body as a direct call on `$self`.
///
/// Intra-actor calls are synchronous on every backend: the actor already holds
/// exclusive access to its own state while dispatching, so there is nothing to
/// serialise, and queueing would defer the call past the rest of the caller's
/// body. Resolves against every method, so private helpers work too (#2012).
fn emit_actor_self_call(
    out: &mut String,
    info: &ActorInfo,
    method: &str,
    args: &[TirExpr],
    ctx: &Ctx,
) {
    let Some(m) = info.methods.iter().find(|m| m.name == method) else {
        out.push_str(&format!(
            "    ;; unsupported intra-actor call: {}.{method}\n",
            info.name
        ));
        return;
    };
    out.push_str("    local.get $self\n");
    for a in args {
        emit_expr(out, a, ctx);
    }
    out.push_str(&format!("    call ${}_{}\n", info.snake, m.name));
}

/// Resolve `ty` to an actor name, stripping label/refinement/ref wrappers.
fn actor_name_of<'c>(ty: &Ty, ctx: &'c Ctx) -> Option<&'c ActorInfo> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) => cur = inner,
            Ty::Named(n, _) => return ctx.actors.get(n.as_str()),
            _ => return None,
        }
    }
}

/// `actor Name` → `name` (snake_case). Mirrors the Rust backend's
/// `actor_name_to_snake` so all three backends agree on emitted symbol names.
fn actor_name_to_snake(name: &str) -> String {
    crate::mvl::backends::rust::emit_actors::actor_name_to_snake(name)
}

/// Per-send message-slot local name. Span-keyed so the local-collection pass
/// and the emit path agree.
fn actor_msg_temp_name(expr: &TirExpr) -> String {
    format!("__am_{}_{}", expr.span.offset, expr.span.len)
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
                let (fields, field_names): (Vec<Ty>, Vec<String>) = match &v.fields {
                    TirVariantFields::Unit => (vec![], vec![]),
                    TirVariantFields::Tuple(tys) => (tys.clone(), vec![]),
                    TirVariantFields::Struct(fs) => (
                        fs.iter().map(|f| f.ty.clone()).collect(),
                        fs.iter().map(|f| f.name.clone()).collect(),
                    ),
                };
                let payload_size = fields.iter().map(|_| 8u32).sum::<u32>();
                pvs.push(PayloadVariant {
                    name: format!("{}::{}", td.name, v.name),
                    disc: disc as i32,
                    fields,
                    field_names,
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
fn collect_literals(
    fns: &[&TirFn],
    actors: &[TirActorDecl],
    needs_wasi: bool,
    audit_relabels: &AuditRelabels,
) -> (HashMap<String, (u32, u32)>, u32) {
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
        collect_block(&f.body, &mut map, &mut next, audit_relabels);
    }
    // Actor method bodies are emitted as functions but are not in `tir.fns`, so
    // their literals need interning here too (#2012).
    for ad in actors {
        for m in &ad.methods {
            collect_block(&m.body, &mut map, &mut next, audit_relabels);
        }
    }
    (map, next)
}

fn collect_block(
    block: &TirBlock,
    map: &mut HashMap<String, (u32, u32)>,
    next: &mut u32,
    audit_relabels: &AuditRelabels,
) {
    for stmt in &block.stmts {
        collect_stmt(stmt, map, next, audit_relabels);
    }
}

fn collect_stmt(
    stmt: &TirStmt,
    map: &mut HashMap<String, (u32, u32)>,
    next: &mut u32,
    audit_relabels: &AuditRelabels,
) {
    match stmt {
        TirStmt::Expr { expr, .. } => collect_expr(expr, map, next, audit_relabels),
        TirStmt::Return { value: Some(v), .. } => collect_expr(v, map, next, audit_relabels),
        TirStmt::Let { init, .. } => collect_expr(init, map, next, audit_relabels),
        TirStmt::Assign { value, .. } => collect_expr(value, map, next, audit_relabels),
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_expr(cond, map, next, audit_relabels);
            collect_block(then, map, next, audit_relabels);
            match else_ {
                Some(TirElseBranch::Block(b)) => collect_block(b, map, next, audit_relabels),
                Some(TirElseBranch::If(s)) => collect_stmt(s, map, next, audit_relabels),
                None => {}
            }
        }
        TirStmt::While { cond, body, .. } => {
            collect_expr(cond, map, next, audit_relabels);
            collect_block(body, map, next, audit_relabels);
        }
        TirStmt::For { iter, body, .. } => {
            collect_expr(iter, map, next, audit_relabels);
            collect_block(body, map, next, audit_relabels);
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_expr(scrutinee, map, next, audit_relabels);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => collect_expr(e, map, next, audit_relabels),
                    TirMatchBody::Block(b) => collect_block(b, map, next, audit_relabels),
                }
            }
        }
        _ => {}
    }
}

fn collect_expr(
    expr: &TirExpr,
    map: &mut HashMap<String, (u32, u32)>,
    next: &mut u32,
    audit_relabels: &AuditRelabels,
) {
    match &expr.kind {
        TirExprKind::Literal(Literal::Str(s)) => {
            if !map.contains_key(s) {
                let len = s.len() as u32;
                map.insert(s.clone(), (*next, len));
                *next += len;
            }
        }
        TirExprKind::Unary { expr, .. } => collect_expr(expr, map, next, audit_relabels),
        TirExprKind::Binary { left, right, .. } => {
            collect_expr(left, map, next, audit_relabels);
            collect_expr(right, map, next, audit_relabels);
        }
        TirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_expr(a, map, next, audit_relabels);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            collect_expr(receiver, map, next, audit_relabels);
            for a in args {
                collect_expr(a, map, next, audit_relabels);
            }
        }
        TirExprKind::If { cond, then, else_ } => {
            collect_expr(cond, map, next, audit_relabels);
            collect_block(then, map, next, audit_relabels);
            if let Some(e) = else_ {
                collect_expr(e, map, next, audit_relabels);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_expr(scrutinee, map, next, audit_relabels);
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
                    TirMatchBody::Expr(e) => collect_expr(e, map, next, audit_relabels),
                    TirMatchBody::Block(b) => collect_block(b, map, next, audit_relabels),
                }
            }
        }
        TirExprKind::Block(block) => collect_block(block, map, next, audit_relabels),
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_expr(e, map, next, audit_relabels);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_expr(k, map, next, audit_relabels);
                collect_expr(v, map, next, audit_relabels);
            }
        }
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            for (_, v) in fields {
                collect_expr(v, map, next, audit_relabels);
            }
        }
        // `relabel name(expr, "tag")` (#2013) — recurse into the wrapped
        // value (may itself be a string literal, e.g. `relabel classify("x", tag)`),
        // and when the call site's `audit` flag OR a declaration-level
        // `audit` (#896, via `audit_relabels`) applies, register the audit
        // event's own literal strings (transition name, from/to labels,
        // tag) for emit_expr — same `needs_audit` rule as the emit side, so
        // the two passes agree on which strings must be interned.
        TirExprKind::Relabel {
            name,
            expr,
            tag,
            audit,
        } => {
            collect_expr(expr, map, next, audit_relabels);
            let needs_audit = *audit || audit_relabels.contains_key(name);
            if needs_audit {
                let (from_lbl, to_lbl) = relabel_label_strings(name, audit_relabels);
                for s in [
                    name.as_str(),
                    from_lbl.as_str(),
                    to_lbl.as_str(),
                    tag.as_str(),
                ] {
                    if !s.is_empty() && !map.contains_key(s) {
                        let len = s.len() as u32;
                        map.insert(s.to_string(), (*next, len));
                        *next += len;
                    }
                }
            }
        }
        TirExprKind::Propagate(inner)
        | TirExprKind::Consume(inner)
        | TirExprKind::Borrow { expr: inner, .. } => collect_expr(inner, map, next, audit_relabels),
        TirExprKind::FieldAccess { expr: inner, .. } => {
            collect_expr(inner, map, next, audit_relabels)
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
    // Used by `$mvl_now` (#2056) — real wall-clock read for `std.time.now()`.
    // Imported unconditionally alongside `fd_write`, same as the existing
    // convention: cheap to declare, no downside if the program never calls it.
    out.push_str(
        "  (import \"wasi_snapshot_preview1\" \"clock_time_get\"\n    \
         (func $clock_time_get (param i32 i64 i32) (result i32)))\n",
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
  ;; std.io write(fd, msg) (#2056) — dynamic fd_write on a runtime fd number,
  ;; no trailing newline (unlike println/eprintln). Returns the raw WASI
  ;; errno; the caller (emit_expr's `write` special case) traps on nonzero.
  (func $mvl_write (param $fd i32) (param $ptr i32) (param $len i32) (result i32)
    (i32.store (i32.const 0) (local.get $ptr))
    (i32.store (i32.const 4) (local.get $len))
    (call $fd_write (local.get $fd) (i32.const 0) (i32.const 1) (i32.const 16)))
  ;; std.time now() (#2056) — boxes a real WASI clock_time_get read (clock id
  ;; 0 = realtime) as an opaque nanoseconds handle. `_mvl_alloc` is the
  ;; WASI-local bump allocator above, not the `runtime` crate's
  ;; `_mvl_struct_alloc` — `now()`/`_instant_epoch_seconds` need no runtime
  ;; import at all. `clock_time_get` requires its out-pointer 8-byte
  ;; aligned; `$mvl_alloc`'s bump pointer isn't guaranteed aligned (it can
  ;; land right after an odd-length string literal), so over-allocate by 8
  ;; and round up.
  (func $mvl_now (result i32)
    (local $p i32)
    (local.set $p (call $mvl_alloc (i32.const 16)))
    (local.set $p (i32.and (i32.add (local.get $p) (i32.const 7)) (i32.const -8)))
    (drop (call $clock_time_get (i32.const 0) (i64.const 0) (local.get $p)))
    (local.get $p))
"#;

// ── Emitter tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    /// Compile `src` straight to WAT, mirroring the LLVM emitter's test helper.
    fn compile(src: &str) -> String {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
        expr_types.extend(crate::mvl::checker::check(&prog).expr_types);
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        WasmTextCompiler::new().emit_program(&tir, "test")
    }

    const COUNTER: &str = "actor Counter {\n\
           count: Int\n\
           pub fn increment(val n: Int) { self.count = self.count + n }\n\
           pub fn reset() { self.count = 0 }\n\
           pub test fn get_count() -> Int { self.count }\n\
         }\n";

    #[test]
    fn actor_emits_behavior_and_dispatch_fns() {
        let wat = compile(COUNTER);
        assert!(
            wat.contains("(func $counter_increment (param $self i32) (param $n i64)"),
            "{wat}"
        );
        assert!(
            wat.contains("(func $counter_get_count (param $self i32) (result i64)"),
            "{wat}"
        );
        assert!(
            wat.contains(
                "(func $counter_dispatch (param $state i32) (param $disc i32) (param $args i32)"
            ),
            "{wat}"
        );
    }

    /// `pub test fn` is a synchronous read, so it must NOT occupy a behaviour
    /// discriminant — only `increment` (0) and `reset` (1) do.
    #[test]
    fn actor_test_fn_is_not_a_behavior_discriminant() {
        let wat = compile(COUNTER);
        let dispatch = wat
            .split("(func $counter_dispatch")
            .nth(1)
            .expect("dispatch fn emitted");
        let dispatch = dispatch.split("\n  (func").next().unwrap();
        assert!(dispatch.contains("call $counter_increment"), "{dispatch}");
        assert!(dispatch.contains("call $counter_reset"), "{dispatch}");
        assert!(
            !dispatch.contains("call $counter_get_count"),
            "sync read must not be dispatched as a behaviour\n{dispatch}"
        );
    }

    /// `self.field = value` must emit a real store. It used to fall through as
    /// an unsupported assign target, which stubbed the whole body (#2012).
    #[test]
    fn actor_field_assignment_emits_store() {
        let wat = compile(COUNTER);
        assert!(
            !wat.contains(";; unsupported"),
            "actor bodies must not contain unsupported markers\n{wat}"
        );
        assert!(
            !wat.contains("body stubbed"),
            "actor bodies must not be stubbed\n{wat}"
        );
        assert!(wat.contains("i64.store offset=0"), "{wat}");
    }

    #[test]
    fn actor_scheduler_emitted_with_reentrancy_guard() {
        let wat = compile(COUNTER);
        assert!(wat.contains("(global $__actor_draining"), "{wat}");
        assert!(wat.contains("(func $__mvl_actor_slot"), "{wat}");
        assert!(wat.contains("(func $__mvl_actor_pump"), "{wat}");
        assert!(wat.contains("(func $__mvl_actor_route"), "{wat}");
        // The guard is what stops a self-send from recursing into dispatch.
        let pump = wat.split("(func $__mvl_actor_pump").nth(1).unwrap();
        let pump = pump.split("\n  (func").next().unwrap();
        assert!(
            pump.contains("global.get $__actor_draining"),
            "pump must bail out when a drain is already running\n{pump}"
        );
    }

    /// Dispatch is a static switch on the actor type tag — no funcref table
    /// and no `call_indirect`, because the runtime cannot call back into the
    /// emitted module (ADR-0059 §2).
    #[test]
    fn actor_dispatch_is_static_no_call_indirect() {
        let wat = compile(
            "actor A { x: Int  pub fn set(val n: Int) { self.x = n } }\n\
             actor B { y: Int  pub fn set(val n: Int) { self.y = n } }\n",
        );
        assert!(!wat.contains("call_indirect"), "{wat}");
        assert!(!wat.contains("(table "), "{wat}");
        let route = wat.split("(func $__mvl_actor_route").nth(1).unwrap();
        let route = route.split("\n  (func").next().unwrap();
        assert!(route.contains("call $a_dispatch"), "{route}");
        assert!(route.contains("call $b_dispatch"), "{route}");
    }

    /// Programs without actors must not pay for the scheduler.
    #[test]
    fn no_actors_means_no_scheduler() {
        let wat = compile("fn add(a: Int, b: Int) -> Int { a + b }");
        assert!(!wat.contains("__mvl_actor"), "{wat}");
        assert!(!wat.contains("__actor_q"), "{wat}");
    }

    /// String-typed actor state releases the old handle before overwriting, or
    /// every write leaks an `MvlString`.
    #[test]
    fn actor_string_field_reassignment_drops_old_handle() {
        let wat = compile(
            "actor Label {\n\
               text: String\n\
               pub fn write(val s: String) { self.text = s }\n\
               pub test fn read() -> String { self.text }\n\
             }\n",
        );
        let write = wat.split("(func $label_write").nth(1).unwrap();
        let write = write.split("\n  (func").next().unwrap();
        assert!(
            write.contains("call $_mvl_string_drop"),
            "overwriting a String field must drop the old handle\n{write}"
        );
        assert!(write.contains("call $_mvl_string_new"), "{write}");
    }

    /// The direct guard against the vacuous-assertion class of bug: `assert_eq`
    /// on an actor read must emit a real comparison. Before #2012 the actor
    /// method call typed as `Unknown` and the assertion emitted nothing at all,
    /// so `assert_eq(c.get_count(), 99999)` passed.
    #[test]
    fn actor_read_assertion_emits_real_comparison() {
        let wat = compile(
            "actor Counter {\n\
               count: Int\n\
               pub fn increment(val n: Int) { self.count = self.count + n }\n\
               pub test fn get_count() -> Int { self.count }\n\
             }\n\
             test fn t() -> Unit ! Spawn + Send {\n\
                 let c: Counter = actor Counter { count: 0 };\n\
                 c.increment(5);\n\
                 assert_eq(c.get_count(), 5);\n\
             }",
        );
        assert!(
            wat.contains("i64.eq"),
            "assert_eq on an actor read must compare, not no-op\n{wat}"
        );
        assert!(
            wat.contains("unreachable"),
            "and must trap on mismatch\n{wat}"
        );
    }

    /// `self.behaviour()` must be a direct call, not a queued send: queueing
    /// defers it past the rest of the caller's body, which diverges from the
    /// Rust and LLVM backends (#2012).
    #[test]
    fn intra_actor_call_is_direct_not_queued() {
        let wat = compile(
            "actor Probe {\n\
               n: Int\n\
               pub fn mark() { self.n = self.n * 2 }\n\
               pub fn step() { self.n = self.n + 1; self.mark() }\n\
             }\n",
        );
        let step = wat.split("(func $probe_step").nth(1).expect("step emitted");
        let step = step.split("\n  (func").next().unwrap();
        assert!(
            step.contains("call $probe_mark"),
            "self.mark() must call directly\n{step}"
        );
        assert!(
            !step.contains("$__mvl_actor_slot"),
            "self-call must not enqueue a message\n{step}"
        );
    }

    /// A private helper is invoked by bare name (`helper()`), since
    /// `self.helper()` is not accepted for non-public methods. It must route to
    /// the emitted symbol with the state pointer, not `call $helper` (#2012).
    #[test]
    fn private_actor_helper_routes_to_emitted_symbol() {
        let wat = compile(
            "actor Probe {\n\
               n: Int\n\
               fn helper() { self.n = self.n * 2 }\n\
               pub fn step() { self.n = self.n + 1; helper() }\n\
             }\n",
        );
        assert!(wat.contains("call $probe_helper"), "{wat}");
        assert!(
            !wat.contains("call $helper\n"),
            "must not emit the bare source name\n{wat}"
        );
    }

    /// A behaviour arity above the fixed message-slot count must be reported,
    /// not silently truncated.
    #[test]
    fn actor_arity_over_slot_limit_is_reported() {
        let wat = compile(
            "actor Wide {\n\
               v: Int\n\
               pub fn many(val a: Int, val b: Int, val c: Int, val d: Int, \
                           val e: Int, val f: Int, val g: Int, val h: Int, val i: Int) \
                 { self.v = a }\n\
             }\n\
             fn main() -> Unit ! Spawn + Send {\n\
               let w: Wide = actor Wide { v: 0 };\n\
               w.many(1, 2, 3, 4, 5, 6, 7, 8, 9);\n\
             }\n",
        );
        // The caller stubs rather than emitting a message that overruns its slot.
        assert!(wat.contains("body stubbed"), "{wat}");
    }

    /// A unit-variant enum nested inside a payload-carrying variant (#2029)
    /// must be independently discriminated per arm, not just the outer tag —
    /// and must not fabricate a mismatched-type local for the qualified
    /// variant name.
    #[test]
    fn nested_unit_variant_in_payload_gets_distinct_inner_guards() {
        let wat = compile(
            "type Inner = enum { A, B, C, D }\n\
             type Outer = enum { Plain, Wrapped(Inner) }\n\
             total fn describe(o: Outer) -> String {\n\
                 match o {\n\
                     Outer::Plain             => \"PLAIN\",\n\
                     Outer::Wrapped(Inner::A) => \"WRAPPED A\",\n\
                     Outer::Wrapped(Inner::B) => \"WRAPPED B\",\n\
                     Outer::Wrapped(Inner::C) => \"WRAPPED C\",\n\
                     Outer::Wrapped(Inner::D) => \"WRAPPED D\",\n\
                 }\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        // Four distinct `Wrapped(Inner::X)` arms must each AND in their own
        // inner-discriminant comparison, not share one outer-only guard.
        // A bare count would pass even if two arms' discriminant constants
        // were swapped, so assert the exact guard sequence (including the
        // expected `inner_disc` value) for each arm instead.
        for inner_disc in 0..4 {
            let guard = format!(
                "i32.load offset=4\n    i64.load offset=0\n    i32.wrap_i64\n    i32.const {inner_disc}\n    i32.eq\n    i32.and\n"
            );
            assert!(
                wat.contains(&guard),
                "missing inner-discriminant guard for disc {inner_disc}\n{wat}"
            );
        }
        // Qualified variant names are guards, not bindings — no local should
        // ever be declared for them.
        assert!(!wat.contains("(local $Inner::A"), "{wat}");
        assert!(!wat.contains("(local $Inner::B"), "{wat}");
        assert!(!wat.contains("(local $Inner::C"), "{wat}");
        assert!(!wat.contains("(local $Inner::D"), "{wat}");
    }

    /// A qualified reference to an unrelated enum whose variant happens to
    /// share an ordinal with the field's actual enum (e.g. `ColorB::Y` == 1,
    /// `ColorA::Green` == 1) must not satisfy the guard — the type mismatch
    /// has to route to the safe `;; unsupported`/`unreachable` stub instead
    /// of silently matching the wrong arm.
    #[test]
    fn cross_enum_ordinal_collision_is_unsupported_not_silently_matched() {
        let wat = compile(
            "type ColorA = enum { Red, Green, Blue }\n\
             type ColorB = enum { X, Y, Blue2 }\n\
             type Outer = enum { Plain, Wrapped(ColorA) }\n\
             fn describe(o: Outer) -> Int {\n\
                 match o {\n\
                     Outer::Plain              => 0,\n\
                     Outer::Wrapped(ColorB::Y) => 111,\n\
                     Outer::Wrapped(_)         => 999,\n\
                 }\n\
             }\n",
        );
        // The `;; unsupported nested pattern` marker itself lives in the
        // scratch body buffer that gets discarded once a whole-body stub is
        // triggered — so the *emitted* module only shows the stub, not the
        // marker text. Asserting the stub confirms the safe-trap path fired.
        assert!(wat.contains("body stubbed"), "{wat}");
        assert!(wat.contains("unreachable"), "{wat}");
    }

    /// A nested variant reference whose own variant carries a payload (e.g.
    /// `Inner::B(n)` where `Inner::B` is `B(Int)`) parses as a `TupleStruct`
    /// field pattern, not `Ident` — the guard loop must recognize this shape
    /// as unsupported rather than silently emitting only the outer-tag
    /// guard (the same bug class as #2029, one level deeper).
    #[test]
    fn doubly_nested_payload_carrying_variant_is_unsupported_not_silently_matched() {
        let wat = compile(
            "type Inner = enum { B(Int), C(Int) }\n\
             type Outer = enum { Plain, Wrapped(Inner) }\n\
             fn describe(o: Outer) -> String {\n\
                 match o {\n\
                     Outer::Plain                => \"PLAIN\",\n\
                     Outer::Wrapped(Inner::B(n)) => \"WRAPPED B\",\n\
                     Outer::Wrapped(Inner::C(n)) => \"WRAPPED C\",\n\
                 }\n\
             }\n",
        );
        assert!(wat.contains("body stubbed"), "{wat}");
        assert!(wat.contains("unreachable"), "{wat}");
    }

    /// A literal field pattern (`Code(0)` vs `Code(n)`) gets no discriminant
    /// check from either the guard loop or the binding loop today — it must
    /// route to the safe stub rather than silently sharing the outer-only
    /// guard with the catch-all `Code(n)` arm.
    #[test]
    fn literal_field_pattern_is_unsupported_not_silently_matched() {
        let wat = compile(
            "type Msg2 = enum { Quit, Code(Int) }\n\
             fn classify(m: Msg2) -> Int {\n\
                 match m {\n\
                     Msg2::Quit    => -1,\n\
                     Msg2::Code(0) => 100,\n\
                     Msg2::Code(n) => n,\n\
                 }\n\
             }\n",
        );
        assert!(wat.contains("body stubbed"), "{wat}");
        assert!(wat.contains("unreachable"), "{wat}");
    }

    /// A nested-variant guard whose payload field is declared via a type
    /// alias (`type Alias = Inner`) must still resolve and emit a real
    /// guard — not stub to unreachable — since the alias peels down to the
    /// same all-unit enum the qualifier names.
    #[test]
    fn aliased_enum_field_type_still_gets_a_real_guard() {
        let wat = compile(
            "type Inner = enum { A, B }\n\
             type Alias = Inner\n\
             type Outer = enum { Plain, Wrapped(Alias) }\n\
             fn describe(o: Outer) -> Int {\n\
                 match o {\n\
                     Outer::Plain             => 0,\n\
                     Outer::Wrapped(Inner::A) => 1,\n\
                     Outer::Wrapped(Inner::B) => 2,\n\
                 }\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        for inner_disc in 0..2 {
            let guard = format!(
                "i32.load offset=4\n    i64.load offset=0\n    i32.wrap_i64\n    i32.const {inner_disc}\n    i32.eq\n    i32.and\n"
            );
            assert!(
                wat.contains(&guard),
                "missing inner-discriminant guard for disc {inner_disc}\n{wat}"
            );
        }
    }

    /// Two simultaneous nested-enum guard slots in one payload variant, one
    /// of them wildcarded — each live guard slot must AND in its own check
    /// independently (#2029 follow-up). This is a WASM-backend-only test:
    /// the equivalent corpus case tripped a pre-existing, unrelated LLVM
    /// backend bug ("duplicate case value in switch" in its match-arm
    /// lowering) that isn't part of this fix's scope.
    #[test]
    fn multiple_simultaneous_nested_guards_in_one_variant() {
        let wat = compile(
            "type Weekday = enum { Mon, Tue, Wed }\n\
             type Season = enum { Spring, Summer, Fall, Winter }\n\
             type Combo = enum { Solo(Weekday), Duo(Weekday, Season) }\n\
             fn describe(c: Combo) -> String {\n\
                 match c {\n\
                     Combo::Solo(Weekday::Mon)               => \"SOLO MON\",\n\
                     Combo::Solo(Weekday::Tue)               => \"SOLO TUE\",\n\
                     Combo::Solo(Weekday::Wed)               => \"SOLO WED\",\n\
                     Combo::Duo(Weekday::Mon, Season::Spring) => \"MON SPRING\",\n\
                     Combo::Duo(_, Season::Fall)              => \"ANY FALL\",\n\
                     Combo::Duo(_, _)                         => \"OTHER DUO\",\n\
                 }\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        // The three `Solo(Weekday::X)` arms get one guard each; the
        // `Duo(Weekday::Mon, Season::Spring)` arm gets two ANDed together
        // (both slots live); `Duo(_, Season::Fall)` gets one (first slot is
        // wildcarded); `Duo(_, _)` gets none. Total: 3 + 2 + 1 = 6.
        assert_eq!(
            wat.matches("i32.and").count(),
            6,
            "expected 6 total inner-discriminant guards across all arms\n{wat}"
        );
    }

    /// `write(fd, msg)` on a dynamic `Fd` value routes through `$mvl_write`
    /// (dynamic `fd_write`, no forced newline) rather than the generic
    /// `call $write` — a dangling reference, since `write` is `builtin`
    /// (#2056).
    #[test]
    fn write_on_dynamic_fd_uses_mvl_write_runtime_shim() {
        let wat = compile(
            "type Fd = struct { inner: Int }\n\
             pub builtin fn stdout() -> Fd\n\
             pub builtin fn stderr() -> Fd\n\
             pub builtin fn write(fd: Fd, msg: String) -> Result[Unit, String] ! Console\n\
             fn log_line(fd: Fd, line: String) -> Unit ! Console {\n\
                 let _: Result[Unit, String] = write(fd, line);\n\
             }\n\
             fn main() -> Unit ! Console {\n\
                 log_line(stderr(), \"hello\")\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(!wat.contains("call $write\n"), "{wat}");
        assert!(!wat.contains("call $stdout\n"), "{wat}");
        assert!(!wat.contains("call $stderr\n"), "{wat}");
        assert!(wat.contains("call $mvl_write"), "{wat}");
        // `stderr()`/`stdout()` heap-allocate an `Fd { inner }` like any
        // other struct literal.
        assert!(wat.contains("call $_mvl_struct_alloc"), "{wat}");
    }

    /// `read_file(path)` routes through the preloaded runtime's
    /// `_mvl_io_read_file`, not a dangling `call $read_file` (#2076). The
    /// `Ok(contents)` binding must split into `contents_ptr`/`contents_len`
    /// locals like any other String — a Result[String, E] Ok payload had
    /// no test coverage before this (#2038/#2066 only ever covered
    /// Ok=Int/Float and Err=String).
    #[test]
    fn read_file_uses_mvl_io_read_file_runtime_shim() {
        let wat = compile(
            "type IoError = enum { NotFound, PermissionDenied, AlreadyExists, Other(String) }\n\
             pub builtin fn read_file(p: String) -> Result[String, IoError] ! Console\n\
             fn main() -> Unit ! Console {\n\
                 match read_file(\"x\") {\n\
                     Ok(contents) => println(contents),\n\
                     Err(_) => println(\"error\"),\n\
                 }\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(!wat.contains("call $read_file\n"), "{wat}");
        assert!(wat.contains("call $_mvl_io_read_file"), "{wat}");
        // Split (ptr, len) locals for `contents`, not a single generic one.
        assert!(wat.contains("(local $contents_ptr i32)"), "{wat}");
        assert!(wat.contains("(local $contents_len i32)"), "{wat}");
        assert!(!wat.contains("(local $contents i64)"), "{wat}");
        assert!(wat.contains("call $_mvl_result_value_i32"), "{wat}");
    }

    /// `now()` / a module-private `_instant_epoch_seconds(t)` builtin route
    /// through the WASI `clock_time_get` shim, not a dangling `call $now`
    /// (#2056).
    #[test]
    fn now_and_epoch_seconds_use_wasi_clock_shim() {
        let wat = compile(
            "pub type Instant = struct {}\n\
             pub builtin fn now() -> Instant ! Clock\n\
             builtin fn _instant_epoch_seconds(t: Instant) -> Int\n\
             fn main() -> Unit ! Clock + Console {\n\
                 let t: Instant = now();\n\
                 let secs: Int = _instant_epoch_seconds(t);\n\
                 println(secs.to_string())\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(!wat.contains("call $now\n"), "{wat}");
        assert!(!wat.contains("call $_instant_epoch_seconds\n"), "{wat}");
        assert!(wat.contains("call $mvl_now"), "{wat}");
        assert!(wat.contains("call $clock_time_get"), "{wat}");
    }

    /// `Some(v) => v` on `Option[String]` binds `v` as the split (ptr, len)
    /// locals every other String variable uses — not a single generic
    /// local, which desyncs from how a bare `Var("v")` reference is always
    /// emitted for a String-typed name (#2056; this is exactly the shape
    /// `std/log.mvl`'s `field_or_empty` hit via `match fields.get(k) {
    /// Some(v) => v, None => "" }`).
    #[test]
    fn some_string_match_binding_uses_split_locals() {
        let wat = compile(
            "fn first_or_empty(xs: List[String]) -> String {\n\
                 match xs.get(0) {\n\
                     Some(v) => v,\n\
                     None => \"\",\n\
                 }\n\
             }\n",
        );
        assert!(!wat.contains(";; unsupported"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(wat.contains("(local $v_ptr i32)"), "{wat}");
        assert!(wat.contains("(local $v_len i32)"), "{wat}");
        assert!(wat.contains("local.set $v_ptr"), "{wat}");
        assert!(wat.contains("local.set $v_len"), "{wat}");
    }
}
