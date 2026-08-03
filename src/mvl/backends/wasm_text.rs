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
//! equality, `MvlString` refcount + drops, generic monomorphization, and
//! non-capturing higher-order fns — lambda literals become top-level functions
//! reached through an intra-module funcref table + `call_indirect`, which is
//! what backs `List[T]::map`/`filter`/`fold`/`sort_by`/`min_by`/`max_by`
//! (#2014).
//!
//! `extern "rust"` FFI (#2049, ADR-0062): supported for `Int`/`Bool`/
//! `String`/user structs/unit-enums/refinement-newtype-aliases/labels
//! (`Secret`/`Tainted`/`Clean`/`Public`)/`Option`/`Result` nested over the
//! above. `(import "extern" "<name>" ...)` is declared here for every
//! `extern "rust"` fn (this file's own responsibility); satisfying those
//! imports at run time is `wasm_host_glue.rs`'s job — a generated,
//! wasmtime-embedding native host that links the directory's real
//! `bridge.rs` unmodified. Payload-carrying enums, `List`/`Map`/`Set`, and
//! `Fn` values crossing the extern boundary are not supported yet (reported
//! via `UnsupportedExternFn`, not silently mis-marshalled) — this is *not*
//! a new `extern "wasm"` ABI keyword, `extern "rust"` is unchanged; the
//! whole surface is compiler-internal codegen, see ADR-0062 for why.
//!
//! Deliberately not supported (later phases of #1817):
//! - *Capturing* closures — these need an environment representation, not just
//!   a function pointer. See `03_functions/higher_order_test.mvl` and
//!   `07_ownership/lambda_capture_test.mvl`, both still in
//!   `WASM_CORPUS_EXCLUDE`. A capturing lambda is detected and stubbed rather
//!   than emitted, so the failure stays inside the one function.
//! - `Map` beyond `Map[String, Int]`
//! - String concat (`_mvl_string_concat` wiring is incomplete)
//! - Arbitrary-fd `write`/`read`/`open` (real files, WASI preopens)
//!
//! Actors (#2012, ADR-0059): supported for spawn, behaviour sends, and
//! `pub test fn` synchronous reads. Single-threaded run-to-completion — the
//! mailbox and drain loop are emitted into the module (the `--preload` runtime
//! cannot call back into it), dispatch is a static switch on an actor type tag,
//! and `send` drains at the outermost call so per-actor FIFO holds and a
//! self-send queues instead of recursing. No parallelism, and no
//! `link`/`monitor`/`select`/`on_exit` supervision yet.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use super::{AssertMode, Backend};
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{
    ArithOp, BinaryOp, BitwiseOp, CmpOp, GenericParam, LValue, Literal, LogicOp, Pattern, RefExpr,
    TirActorDecl, TirActorMethod, TirBlock, TirElseBranch, TirExpr, TirExprKind, TirFn,
    TirMatchArm, TirMatchBody, TirParam, TirProgram, TirStmt, TirTypeBody, TirTypeDecl,
    TirVariantFields, UnaryOp,
};
use crate::mvl::parser::lexer::Span;

pub struct WasmTextCompiler {
    pub assert_mode: AssertMode,
    /// Functions whose bodies were replaced by `unreachable` during the last
    /// `emit_program`, in emission order.
    ///
    /// Exists because stubbing is otherwise *invisible*: the body is discarded,
    /// the module still assembles, and `mvl build --backend=wasm` exits 0. A
    /// half-implemented method is indistinguishable from a working one at the
    /// CLI, so gaps accumulate silently until someone runs the file — which is
    /// how `List[T]::push` came to have no dispatch arm at all without anyone
    /// noticing (#2014). Reported by the CLI so an incomplete build says so.
    ///
    /// `RefCell` because `Backend::emit_program` takes `&self`, and this is
    /// deliberately not on that trait — the Rust and LLVM backends have no
    /// equivalent notion.
    stubbed: std::cell::RefCell<Vec<String>>,
}

impl WasmTextCompiler {
    pub fn new() -> Self {
        Self {
            assert_mode: AssertMode::Always,
            stubbed: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Names of functions stubbed to `unreachable` by the last `emit_program`.
    ///
    /// Empty means the module is fully implemented. Non-empty means it will
    /// trap if any listed function is called, so callers should surface this
    /// rather than treat a successful emit as a successful compile.
    pub fn stubbed_fns(&self) -> Vec<String> {
        self.stubbed.borrow().clone()
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
pub(crate) struct FieldSlot {
    pub(crate) name: String,
    pub(crate) offset: u32,
    pub(crate) ty: Ty,
}

/// Pre-computed memory layout for a single struct type.
///
/// `pub(crate)`: reused as-is by `wasm_host_glue` (#2049) so the extern
/// "rust" FFI host glue marshals struct fields at the exact same byte
/// offsets this emitter itself uses — recomputing an independent copy would
/// only need to drift once to silently corrupt every struct crossing the
/// FFI boundary.
#[derive(Debug, Clone)]
pub(crate) struct StructLayout {
    pub(crate) total_size: u32,
    pub(crate) fields: Vec<FieldSlot>,
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
    /// Generic extension methods in scope, keyed `(receiver_type, method)`
    /// (#2014). The emission-side half of the same registry
    /// `collect_generic_instantiations` walks: a `MethodCall` that reaches the
    /// end of the builtin dispatch chain consults this and, on a hit, emits a
    /// direct `call` to the monomorphized instance.
    ///
    /// Both sides go through `resolve_generic_method_call` so the name emitted
    /// here is by construction the name that got instantiated.
    generic_methods: &'a HashMap<(String, String), &'a TirFn>,
    /// Lambda literals encountered while emitting, in table order — position
    /// *is* the `call_indirect` table index (#2014). Each becomes a top-level
    /// `(func $__lambda_…)` and an entry in the module's single `(elem)`.
    ///
    /// Appended to during emission rather than pre-scanned, so the index a call
    /// site pushes and the slot the function lands in cannot drift apart.
    ///
    /// Shared by reference — not owned per-`Ctx` — because the module has
    /// exactly one table. A monomorphized instance or a nested lambda builds a
    /// derived `Ctx`, and if each owned its own registry a lambda registered
    /// there would take index 0 of a throwaway list while the real table put it
    /// somewhere else entirely.
    /// Function-typed parameters of the body being emitted, so `fn_value_ty`
    /// can tell `f(x)` (indirect, through a `fn(T) -> U` param) from an
    /// ordinary `call $name` (#2014). Kept separate from `fn_locals` because
    /// that list drives the drop sweep and params must never be dropped.
    fn_params: std::cell::RefCell<Vec<(String, Ty)>>,
    /// Sink for names of functions stubbed to `unreachable`, shared with
    /// [`WasmTextCompiler::stubbed`] so the CLI can report an incomplete build
    /// instead of exiting 0 on one (#2014).
    stubbed: &'a std::cell::RefCell<Vec<String>>,
    lambdas: &'a std::cell::RefCell<Vec<LambdaEntry>>,
    /// Span → table index, so re-emitting the same lambda expression reuses its
    /// slot instead of allocating a second one.
    lambda_slots: &'a std::cell::RefCell<HashMap<(u32, u32, String), u32>>,
    /// Distinct `(type $sig… (func …))` declarations `call_indirect` needs,
    /// keyed by generated name. A `BTreeMap` so module output is deterministic.
    indirect_sigs: &'a std::cell::RefCell<std::collections::BTreeMap<String, String>>,
}

/// One lambda literal awaiting emission as a top-level function (#2014).
///
/// Only non-capturing lambdas are representable: there is no environment
/// pointer, so the emitted function takes exactly the lambda's own parameters.
/// `Ctx::type_subst` is captured at the point of encounter because a lambda
/// inside a monomorphized body must resolve its types under that instance's
/// substitution, not whatever is live when the deferred emission runs.
#[derive(Clone)]
struct LambdaEntry {
    wasm_name: String,
    params: Vec<TirParam>,
    body: TirExpr,
    ret_ty: Ty,
    type_subst: HashMap<String, Ty>,
    /// Free variables read from the enclosing scope (#2118), in a fixed
    /// order that also fixes their byte offset in the heap-allocated
    /// environment (`i * 8`). Empty for a non-capturing lambda — it still
    /// gets a `$__env` param like every lambda (the calling convention is
    /// uniform across the whole `Ty::Fn` shape, see `emit_closure_value`),
    /// just never reads it.
    captures: Vec<(String, Ty)>,
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
    // ── std.env ───────────────────────────────────────────────────────────
    ("_mvl_env_args", "(result i32)"),
    ("_mvl_env_get", "(param i32 i32) (result i32)"),
    ("_mvl_env_set", "(param i32 i32 i32 i32) (result i32)"),
    ("_mvl_env_remove_var", "(param i32 i32)"),
    ("_mvl_env_current_dir", "(result i32)"),
    ("_mvl_env_chdir", "(param i32 i32) (result i32)"),
    ("_mvl_env_exit", "(param i64)"),
    ("_mvl_env_getuid", "(result i64)"),
    ("_mvl_env_getgid", "(result i64)"),
    ("_mvl_env_all", "(result i32)"),
    ("_mvl_box_new", "(param i32) (result i32)"),
    ("_mvl_array_new", "(param i32 i32) (result i32)"),
    ("_mvl_array_len", "(param i32) (result i64)"),
    ("_mvl_array_is_empty", "(param i32) (result i32)"),
    // No `_mvl_array_push` (the untyped, pointer-taking form): the runtime
    // exports it and `runtime/llvm` uses it internally, but no emit site here
    // ever produces `call $_mvl_array_push` — only the typed variants below.
    // Declaring it read like a live contract.
    ("_mvl_array_push_i32", "(param i32 i32)"),
    ("_mvl_array_push_i64", "(param i32 i64)"),
    ("_mvl_array_push_f64", "(param i32 f64)"),
    ("_mvl_array_get", "(param i32 i64) (result i32)"),
    ("_mvl_array_clone", "(param i32) (result i32)"),
    // `.slice(start, end)` — MVL `Int` bounds are i64 (#2014). Backs
    // `List[T]::take`/`::skip`, which are pure-MVL wrappers over `slice`.
    ("_mvl_array_slice", "(param i32 i64 i64) (result i32)"),
    // `.concat(other)` (#2114) — new array holding `self`'s elements
    // followed by `other`'s. Byte-wise copy at `elem_size` granularity,
    // same caveat as `slice`: correct for scalar/pointer arrays, not
    // refcount-aware for `List[String]` elements.
    ("_mvl_array_concat", "(param i32 i32) (result i32)"),
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
    // `Set[T].remove(val)` — linear-scan remove-by-value (#2124).
    ("_mvl_array_remove_value_i64", "(param i32 i64)"),
    ("_mvl_array_remove_value_i32", "(param i32 i32)"),
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
    // ── std.io — WASI file operations ───────────────────────────────────
    // Takes path as (ptr, len); returns heap-allocated MvlResult.
    ("_mvl_io_read_file", "(param i32 i32) (result i32)"),
    ("_mvl_io_write_file", "(param i32 i32 i32 i32) (result i32)"),
    ("_mvl_io_append", "(param i32 i32 i32 i32) (result i32)"),
    ("_mvl_io_exists", "(param i32 i32) (result i32)"),
    ("_mvl_io_is_file", "(param i32 i32) (result i32)"),
    ("_mvl_io_is_dir", "(param i32 i32) (result i32)"),
    ("_mvl_io_create_dir_all", "(param i32 i32) (result i32)"),
    ("_mvl_io_remove", "(param i32 i32) (result i32)"),
    ("_mvl_io_open", "(param i32 i32) (result i32)"),
    ("_mvl_io_close", "(param i32)"),
    // ── std.time — wall clock and sleep ───────────────────────────────────
    ("_mvl_time_now", "(result i32)"),
    (
        "_mvl_time_instant_epoch_seconds",
        "(param i32) (result i64)",
    ),
    ("_mvl_time_thread_sleep", "(param i64 i64)"),
    // ── std.random — PRNG ─────────────────────────────────────────────────
    ("_mvl_random_int", "(param i64 i64) (result i64)"),
    ("_mvl_random_float", "(result f64)"),
    ("_mvl_random_bytes", "(param i64) (result i32)"),
    ("_mvl_random_choice_index", "(param i32) (result i64)"),
    ("_mvl_random_shuffle", "(param i32) (result i32)"),
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
    // Group K — Int/UInt/Float::pow (#2122). WASM has no integer or f64
    // exponentiation opcode, unlike abs/ceil/floor/sqrt/min/max, which are
    // native instructions and need no runtime call at all.
    ("_mvl_int_pow", "(param i64 i64) (result i64)"),
    ("_mvl_float_pow", "(param f64 f64) (result f64)"),
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
        // Per-emit, not cumulative: one process may emit several modules
        // (entry + siblings), and a stale list would blame the wrong one.
        self.stubbed.borrow_mut().clear();

        // Partition `tir.fns` on (has receiver?, is generic?). One pass and a
        // 2×2 match, so exhaustiveness and exclusivity are visible rather than
        // derived from four overlapping filter predicates — a bucket silently
        // matching none of them is exactly how every pure-MVL `List[T]` method
        // in `std/lists.mvl` went unemitted before #2014.
        //
        // Only `plain_fns` and `ext_methods` are emitted directly. The two
        // generic buckets have no single WASM signature, so they are
        // monomorphized per call site by `collect_generic_instantiations`.
        let mut plain_fns: Vec<&TirFn> = Vec::new();
        let mut generic_fns: Vec<&TirFn> = Vec::new();
        let mut ext_methods: Vec<&TirFn> = Vec::new();
        let mut generic_ext_methods: Vec<&TirFn> = Vec::new();
        for f in &tir.fns {
            if f.is_builtin {
                continue;
            }
            match (f.receiver_type.is_some(), f.type_params.is_empty()) {
                (false, true) => plain_fns.push(f),
                (false, false) => generic_fns.push(f),
                (true, true) => ext_methods.push(f),
                (true, false) => generic_ext_methods.push(f),
            }
        }
        let fns = plain_fns;
        // Receiverless fns of both kinds — the monomorphization lookup needs the
        // generic ones, and the literal/instantiation walkers want both.
        let all_fns: Vec<&TirFn> = fns.iter().chain(generic_fns.iter()).copied().collect();

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
        //
        // Generic bodies must be scanned too. They are emitted via
        // `emit_generic_fn` per instantiation rather than directly, but the
        // literals they reference are the same data-section entries, and a
        // literal reachable *only* from a generic body was interned by nobody:
        // `fn List[T]::tag(self) -> String { "x" }` emitted a body containing
        // just `;; missing literal: "x"` under a `(result i32 i32)` signature,
        // i.e. a stack-underflow module that `wasm-tools parse` accepts and
        // `validate` rejects — with no stub recorded (#2014). `all_fns` covers
        // the generic plain fns (it is `fns` plus those); `generic_ext_methods`
        // covers the fourth bucket.
        let literal_scan_fns: Vec<&TirFn> = fns
            .iter()
            .chain(ext_methods.iter())
            .chain(all_fns.iter())
            .chain(generic_ext_methods.iter())
            .copied()
            .collect();
        let (literals, heap_start) =
            collect_literals(&literal_scan_fns, &tir.actors, &audit_relabels);
        let (enum_types, enum_variants) = collect_enums(&tir.types);
        // Collected before `collect_structs` (moved ahead of its previous
        // position below `collect_structs`/`collect_actors`) — struct field
        // layout needs to peel alias references (`type Port = Int where
        // ...` used as a field type is `Ty::Named("Port", [])` in TIR, not
        // `Ty::Int`) to size fields correctly; see `field_byte_size` (#2049
        // follow-up: found while building the WASM extern-FFI host glue,
        // which needs byte-accurate struct layouts to marshal fields).
        let type_aliases = collect_type_aliases(&tir.types);
        let mut struct_layouts = collect_structs(&tir.types, &type_aliases);
        // Actor state layouts land in `struct_layouts` so the handle behaves
        // like a struct pointer everywhere (#2012).
        let actors = collect_actors(&tir.actors, &mut struct_layouts, &type_aliases);
        let struct_layouts = struct_layouts;
        let payload_enums = collect_payload_enums(&tir.types);
        let empty_subst: HashMap<String, Ty> = HashMap::new();
        let generic_fn_map: HashMap<String, (Vec<GenericParam>, Vec<TirParam>)> = all_fns
            .iter()
            .filter(|f| !f.type_params.is_empty())
            .map(|f| (f.name.clone(), (f.type_params.clone(), f.params.clone())))
            .collect();
        let generic_methods: HashMap<(String, String), &TirFn> = generic_ext_methods
            .iter()
            .map(|f| {
                (
                    (
                        f.receiver_type.clone().expect("filtered above"),
                        f.name.clone(),
                    ),
                    *f,
                )
            })
            .collect();
        // Owned here so every derived Ctx (monomorphized instance, lambda body)
        // can borrow the same registries — the module has one funcref table.
        let lambdas: std::cell::RefCell<Vec<LambdaEntry>> = std::cell::RefCell::new(Vec::new());
        let lambda_slots: std::cell::RefCell<HashMap<(u32, u32, String), u32>> =
            std::cell::RefCell::new(HashMap::new());
        let indirect_sigs: std::cell::RefCell<std::collections::BTreeMap<String, String>> =
            std::cell::RefCell::new(std::collections::BTreeMap::new());
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
            generic_methods: &generic_methods,
            fn_params: std::cell::RefCell::new(Vec::new()),
            stubbed: &self.stubbed,
            lambdas: &lambdas,
            lambda_slots: &lambda_slots,
            indirect_sigs: &indirect_sigs,
        };

        // `extern "rust"` FFI (#2049): a native host embedding wasmtime and
        // linking the same `bridge.rs` the Rust backend already uses
        // supplies these at runtime. Any non-scalar arg/return (String,
        // struct, enum, Option, Result) is marshalled through the runtime
        // module's own boxed-value constructors, which only produce valid
        // pointers if the guest imports the *same* memory the runtime module
        // owns — so force the `(import "runtime" "memory" ...)` path on
        // whenever a rust-abi extern exists, even if the body never happens
        // to call an `_mvl_*` helper itself. Forcing this unconditionally
        // (rather than only for non-scalar signatures) keeps the rule simple
        // and avoids a two-separate-memories bug that would otherwise only
        // show up once an extern signature grows a String/struct param.
        if tir
            .externs
            .iter()
            .any(|ed| ed.abi == "rust" && !ed.fns.is_empty())
        {
            ctx.needs_runtime.set(true);
        }

        // Collect unique generic-function instantiations needed by the corpus fns.
        let instantiations = collect_generic_instantiations(
            &fns,
            &all_fns,
            &ext_methods,
            &generic_ext_methods,
            &tir.actors,
            &ctx,
        );

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

        // Lambda bodies, last: every preceding emission may have registered
        // one, and a lambda body can register further lambdas (#2014).
        emit_lambda_fns(&mut fns_out, &ctx);

        let mut out = String::from("(module\n");

        // `extern "rust"` FFI imports (#2049). Declared under a separate
        // "extern" namespace so they read as distinct from the compiler's
        // own "runtime" module — a native host (embedding wasmtime, linking
        // the same `bridge.rs` the Rust backend already builds against)
        // supplies these. The signature mirrors `emit_fn`'s own param/return
        // lowering exactly, so a call site (already emitting a plain
        // `call $name` after pushing its args — this is what was silently
        // broken before) produces the exact stack shape declared here.
        //
        // Emitted before the runtime/WASI imports below (rather than after,
        // where it read more naturally) because WAT requires every import to
        // precede any function/global/etc. definition, and
        // `emit_wasi_runtime`/`emit_wasi_runtime_shared_memory` below emit
        // real function bodies (the WASI shim, `_start`) — `wasm-tools
        // parse` rejects an import appearing after those with "import after
        // function".
        for ed in &tir.externs {
            if ed.abi != "rust" {
                continue;
            }
            for ef in &ed.fns {
                let sig = extern_fn_signature(&ef.params, &ef.ret_ty, &ctx);
                out.push_str(&format!(
                    "  (import \"extern\" \"{}\"\n    (func ${}{sig}))\n",
                    ef.name, ef.name
                ));
            }
        }

        // `needs_wasi` (has a `fn main() -> Unit`) used to be the *only*
        // trigger for emitting the literal `(data ...)` sections, `$heap`
        // bump allocator, and `$mvl_int_to_string`/`$mvl_println`/etc.
        // helpers below. That's wrong: `mvl test --backend=wasm` compiles
        // and runs each `test fn` standalone via `wasmtime run --invoke`,
        // with no synthesized `main` at all — `needs_wasi` was always false
        // there, so string literals silently got NO data written at their
        // assigned offsets (a correctness bug: comparisons against
        // uninitialized memory could spuriously match) and any `.to_string()`
        // on an `Int` referenced an undefined `$mvl_int_to_string`, rejecting
        // the whole module (#2153). Both are actually needed whenever the
        // compiled body *references* them, independent of whether a WASI
        // entry point exists — checked by scanning the already-built
        // `fns_out` for the exact call-site strings, the same text-based
        // detection this file already uses for `ctx.needs_runtime`-style gating.
        let needs_wasi_helpers = needs_wasi
            || !literals.is_empty()
            || fns_out.contains("call $mvl_int_to_string")
            || fns_out.contains("call $mvl_println")
            || fns_out.contains("call $mvl_eprintln")
            || fns_out.contains("call $mvl_write")
            || fns_out.contains("call $mvl_now")
            // A closure value (capturing or not) heap-allocates its env and
            // its `{funcidx, envptr}` box via `$mvl_alloc` (#2118).
            || fns_out.contains("call $mvl_alloc");

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
            if needs_wasi_helpers {
                // WASI blob but without its own `(memory 1) (export "memory")`
                // — memory is imported above.
                out.push_str(&emit_wasi_runtime_shared_memory(heap_start, &literals));
            }
        } else if needs_wasi_helpers {
            // Standalone WASI module — own memory, no runtime preload
            // needed. Matches the pre-#1819 behaviour for simple programs.
            out.push_str(&emit_wasi_runtime(heap_start, &literals));
        }

        // Function-value support (#2014). Emitted after the bodies are built,
        // because that walk is what discovers the lambdas and signatures, but
        // placed ahead of them in the module — `(type)` before its uses reads
        // naturally, and a WAT `(elem)` may forward-reference functions.
        //
        // Entirely module-local: nothing here crosses the `--preload` boundary
        // that ADR-0059 §2 is about. See the scope note in that ADR.
        for (name, decl) in indirect_sigs.borrow().iter() {
            out.push_str(&format!("  (type {name} {decl})\n"));
        }
        {
            let lambdas = lambdas.borrow();
            if !lambdas.is_empty() {
                out.push_str(&format!("  (table {} funcref)\n", lambdas.len()));
                out.push_str("  (elem (i32.const 0)");
                for l in lambdas.iter() {
                    out.push_str(&format!(" ${}", l.wasm_name));
                }
                out.push_str(")\n");
            }
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
        // Not yet implemented by this backend's runtime-check emitter
        // (`emit_ref_val_wasm`/`emit_ref_expr_wasm`) — the static proof
        // still applies; only the runtime assertion is skipped, same
        // treatment as the quantifiers/ArrayGet above. Without this
        // exclusion these fell through both emitters' `_` fallback arms,
        // which call each other on the *same* unhandled node forever —
        // infinite mutual recursion, a stack overflow at codegen time
        // rather than a graceful skip or a stub (#2086).
        RefExpr::FieldAccess { .. }
        | RefExpr::Len { .. }
        | RefExpr::StringOp { .. }
        | RefExpr::RegexMatch { .. }
        | RefExpr::Min { .. }
        | RefExpr::Max { .. } => false,
        RefExpr::LogicOp { left, right, .. }
        | RefExpr::Compare { left, right, .. }
        | RefExpr::ArithOp { left, right, .. }
        | RefExpr::BitwiseOp { left, right, .. } => {
            is_runtime_checkable(left) && is_runtime_checkable(right)
        }
        RefExpr::Not { inner, .. }
        | RefExpr::Grouped { inner, .. }
        | RefExpr::Old { inner, .. }
        | RefExpr::BitwiseNot { inner, .. }
        | RefExpr::Abs { inner, .. } => is_runtime_checkable(inner),
        RefExpr::Ident { .. }
        | RefExpr::Integer { .. }
        | RefExpr::Float { .. }
        | RefExpr::Bool { .. } => true,
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
        // Bitwise ops (#1928) yield the same width as their operand — same
        // convention as `ArithOp` above. Without this arm it fell to the `_
        // => "i32"` default below, so `Compare`'s `self.bit_and(15) == self`
        // picked `i32.eq` for two i64 values on the stack (#2086): `wasm-tools
        // validate` rejects the type mismatch.
        RefExpr::BitwiseOp { left, .. } => ref_expr_wasm_ty(left, binding_ty, params),
        RefExpr::BitwiseNot { inner, .. } => ref_expr_wasm_ty(inner, binding_ty, params),
        RefExpr::Old { inner, .. } => ref_expr_wasm_ty(inner, binding_ty, params),
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
        // `old(e)` in `ensures`: for runtime assertion purposes, treat as the
        // current value — matches the Rust backend's `emit_ref_expr`. Full
        // entry-time capture is a future enhancement.
        RefExpr::Old { inner, .. } => {
            emit_ref_val_wasm(out, inner, binding, binding_ty, params);
        }
        // Bitwise binary op in a predicate (#1928, #2086): `self.bit_and(15)`
        // etc. An `Int`/`UInt` operand is i64 here (see `wasm_ty`), so these
        // are plain i64 bitwise instructions — no runtime call, mirroring the
        // Rust backend's `&`/`|`/`^`/`<<`/`>>`. `Shr` is arithmetic
        // (`shr_s`): `Int` is signed two's complement, matching Rust's `>>`
        // on `i64`.
        RefExpr::BitwiseOp {
            op, left, right, ..
        } => {
            emit_ref_val_wasm(out, left, binding, binding_ty, params);
            emit_ref_val_wasm(out, right, binding, binding_ty, params);
            let instr = match op {
                BitwiseOp::And => "i64.and",
                BitwiseOp::Or => "i64.or",
                BitwiseOp::Xor => "i64.xor",
                BitwiseOp::Shl => "i64.shl",
                BitwiseOp::Shr => "i64.shr_s",
            };
            out.push_str(&format!("    {instr}\n"));
        }
        // `self.bit_not()` (#1928, #2086) — no dedicated WASM instruction;
        // `x ^ -1` flips every bit, same as LLVM's lowering.
        RefExpr::BitwiseNot { inner, .. } => {
            emit_ref_val_wasm(out, inner, binding, binding_ty, params);
            out.push_str("    i64.const -1\n");
            out.push_str("    i64.xor\n");
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

/// Whether `.clone()` on this (already substitution-resolved) receiver type has
/// a sound lowering — see the `clone` arm in `emit_expr` for the reasoning per
/// category.
///
/// Returning `false` routes the call to `;; unsupported`, which stubs the
/// enclosing function. That is deliberate for refcounted boxes
/// (`Option`/`Result`/structs), where identity-cloning a handle that is later
/// dropped twice is a double-free rather than a visible failure.
/// Whether `.slice(start, end)` can be lowered for this receiver.
///
/// `_mvl_array_slice` copies the element range byte-wise at `elem_size`
/// granularity into a fresh array with its own refcount. For scalar elements
/// that is a complete copy. For elements that are themselves *pointers* —
/// `*MvlString`, and any other heap handle — it duplicates the pointer without
/// bumping the pointee's refcount, so the slice aliases the parent's elements
/// while `local_drop_fn` maps *both* arrays to `_mvl_string_ptr_array_drop`.
/// Each element then gets dropped twice: a use-after-free and a double-free
/// that Rust's wasm allocator does not detect, so it corrupts silently rather
/// than trapping.
///
/// So `List[String]::take`/`::skip`/`.slice()` stubs until the runtime grows an
/// element-aware copy. Same reasoning as [`clone_is_supported`]: a loud stub
/// beats miscompiled ownership (#2014).
fn slice_is_supported(ty: &Ty, ctx: &Ctx) -> bool {
    match collection_elem_ty(ty) {
        None => false,
        Some(elem) => {
            let elem = resolve_ty_param(elem, ctx.type_subst);
            // Scalars are copied whole; anything pointer-shaped is aliased.
            if peels_to_string(&elem)
                || collection_elem_ty(&elem).is_some()
                || map_key_val_ty(&elem).is_some()
                || option_inner_ty(&elem).is_some()
                || result_ok_ty(&elem).is_some()
            {
                return false;
            }
            // Structs and payload enums are boxed pointers too; unit-variant
            // enums are a bare i32 discriminant and copy fine.
            match named_type_name(&elem) {
                Some(name) => {
                    !ctx.struct_layouts.contains_key(name.as_str())
                        && !ctx.payload_enums.contains_key(name.as_str())
                }
                None => true,
            }
        }
    }
}

/// Whether `.concat(other)` can be lowered for this receiver (#2114).
///
/// Same underlying mechanism as [`slice_is_supported`] — `_mvl_array_concat`
/// copies both arrays' elements byte-wise at `elem_size` granularity into a
/// fresh array — so it excludes the same refcounted-box element types for
/// the same reason (String, nested collections, Map, Option, Result: a
/// byte-wise copy aliases the pointee without bumping its refcount, and
/// `local_drop_fn` would then double-drop it).
///
/// Unlike `slice_is_supported`, this does *not* exclude structs and
/// payload enums. Those are boxed pointers too, but `_mvl_struct_alloc`
/// never frees its allocations (#1821 — intentionally leaked, corpus
/// allocations are short-lived) and `local_drop_fn` already maps every
/// non-String collection — struct-element lists included — to the
/// shallow `_mvl_array_drop` (frees the buffer, not the elements). So
/// aliasing struct/payload-enum pointers via `concat` carries no double-free
/// risk today: nothing ever frees the pointee to begin with.
fn concat_is_supported(ty: &Ty, ctx: &Ctx) -> bool {
    match collection_elem_ty(ty) {
        None => false,
        Some(elem) => {
            let elem = resolve_ty_param(elem, ctx.type_subst);
            !(peels_to_string(&elem)
                || collection_elem_ty(&elem).is_some()
                || map_key_val_ty(&elem).is_some()
                || option_inner_ty(&elem).is_some()
                || result_ok_ty(&elem).is_some())
        }
    }
}

/// Whether `Set[T]::map(f: fn(T) -> U) -> Set[U]` can be lowered natively
/// (#2124). `Set[T]::map` cannot be expressed as a pure MVL body like the
/// rest of `std/collections.mvl`'s HOFs (`filter`/`fold`/`any`/`all`): those
/// mutate a clone of `self` in place, but `map` produces a *different*
/// element type, and `Set[U]::new()` does not exist as a callable symbol in
/// any backend. This native arm allocates the output array directly and
/// dedup-inserts each mapped element — same refcounted-box exclusion as
/// [`concat_is_supported`], applied to both the input and output element
/// type, since a dedup-checking `_mvl_array_insert_*` on a String/nested
/// collection element would alias the pointee without bumping its refcount.
///
/// `receiver_ty` is the receiver's full `Set[T]` (or `ref`-wrapped) type;
/// `arg_fn_ty` is the mapper argument's full `fn(T) -> U` type — both are
/// unpacked to their element/return type here rather than by the caller, so
/// a guard site can't accidentally pass the wrong shape.
fn set_map_is_supported(receiver_ty: &Ty, arg_fn_ty: &Ty, ctx: &Ctx) -> bool {
    let not_boxed = |t: &Ty| {
        let t = resolve_ty_param(t, ctx.type_subst);
        !(peels_to_string(&t)
            || collection_elem_ty(&t).is_some()
            || map_key_val_ty(&t).is_some()
            || option_inner_ty(&t).is_some()
            || result_ok_ty(&t).is_some())
    };
    let Some(elem_ty) = collection_elem_ty(receiver_ty) else {
        return false;
    };
    let Ty::Fn(_, ret_ty, ..) = arg_fn_ty else {
        return false;
    };
    not_boxed(elem_ty) && not_boxed(ret_ty)
}

fn clone_is_supported(ty: &Ty, ctx: &Ctx) -> bool {
    if collection_elem_ty(ty).is_some() {
        return true;
    }
    if peels_to_string(ty) {
        return true;
    }
    if option_inner_ty(ty).is_some()
        || result_ok_ty(ty).is_some()
        || map_key_val_ty(ty).is_some()
        || matches!(ty, Ty::Fn(..))
    {
        return false;
    }
    let bare = match ty {
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => inner.as_ref(),
        other => other,
    };
    match bare {
        Ty::Int | Ty::UInt | Ty::Float | Ty::Bool | Ty::Byte | Ty::UByte | Ty::Char => true,
        // Unit-variant enums are a bare i32 discriminant — copyable.
        Ty::Named(name, _) => {
            ctx.enum_types.contains(name)
                || ctx
                    .type_aliases
                    .get(name.as_str())
                    .map(|aliased| clone_is_supported(&aliased.clone(), ctx))
                    .unwrap_or(false)
        }
        _ => false,
    }
}

/// All parameters of the body being emitted, for [`Ctx::fn_params`].
///
/// Originally only `Ty::Fn` params were kept, because the sole consumer was
/// `fn_value_ty`'s "is this name a callable value?" question (#2014) — safe
/// either way, since it resolves the looked-up type and only matches the
/// `Ty::Fn` shape, so a wider registry doesn't change its answer. Widened to
/// every param (#2118): `collect_lambda_captures` needs to look up the type
/// of *any* outer parameter a lambda reads (not just fn-typed ones) to tell
/// a genuine scalar capture from an unsupported one — `repeat_byte(b: Byte,
/// count: Int)`'s `range(0, count).map(|_| b)` failed to recognize `b` as
/// capturable and fell through to the stub path until this widened.
///
/// These must NOT go into [`Ctx::fn_locals`]. An earlier cut did exactly that,
/// and since `emit_stmt(Return)` derives its drop sweep from `fn_locals`, the
/// early `return true` in `List[T]::any` started emitting
/// `local.get $self; call $_mvl_array_drop` — freeing the *caller's* list on the
/// way out. `list_any` then reused `xs` for its second `.any()` call and
/// trapped. A parameter is neither `(local …)`-declared nor owned by the callee.
fn fn_scope_params(params: &[TirParam]) -> Vec<(String, Ty)> {
    params
        .iter()
        .map(|p| (p.name.clone(), p.ty.clone()))
        .collect()
}

/// First local referenced by `body` that is not in `declared`, if any.
///
/// A WASM function may only touch locals it declares or receives; emitting
/// otherwise makes `wasm-tools` reject the entire module, not just the offending
/// function. Used to detect a capturing lambda, whose body reads a name from an
/// enclosing scope that has no representation without a closure environment.
///
/// Works on the emitted text rather than the TIR because that is exactly the
/// property that must hold — a structural capture analysis would have to
/// re-derive which `Var`s the emitter turns into `local.get` (enum variants,
/// qualified variants and `None` do not) and could drift from it.
fn undeclared_local_ref<'a>(
    body: &'a str,
    declared: &std::collections::HashSet<&str>,
) -> Option<&'a str> {
    for line in body.lines() {
        let t = line.trim_start();
        let Some(rest) = t
            .strip_prefix("local.get $")
            .or_else(|| t.strip_prefix("local.set $"))
            .or_else(|| t.strip_prefix("local.tee $"))
        else {
            continue;
        };
        let name = rest.split_whitespace().next().unwrap_or(rest);
        // String-typed values are split into `name_ptr` / `name_len` pairs.
        let base = name
            .strip_suffix("_ptr")
            .or_else(|| name.strip_suffix("_len"))
            .unwrap_or(name);
        if !declared.contains(name) && !declared.contains(base) {
            return Some(name);
        }
    }
    None
}

/// A `Ctx` for emitting a *separate* function body under `type_subst`.
///
/// Used for monomorphized instantiations and for lambda bodies lifted to
/// top-level functions. Both sites previously hand-copied all 24 fields, and
/// this PR's first cut had to hand-add six more to each — a silent hazard,
/// because getting `lambdas`/`lambda_slots` wrong does not fail to compile: two
/// lambdas would claim table index 0 and `call_indirect` would call the wrong
/// one.
///
/// Module-wide registries are shared by reference. Per-body state is reset,
/// because an instantiation is a different function and not a continuation of
/// whatever triggered it — `self_type` in particular, or `self.field` inside a
/// generic body would resolve against the caller's actor layout (#2012).
fn derived_ctx<'a>(base: &Ctx<'a>, type_subst: &'a HashMap<String, Ty>) -> Ctx<'a> {
    Ctx {
        // Shared: module-wide, read-only.
        needs_wasi: base.needs_wasi,
        literals: base.literals,
        audit_relabels: base.audit_relabels,
        enum_types: base.enum_types,
        enum_variants: base.enum_variants,
        struct_layouts: base.struct_layouts,
        payload_enums: base.payload_enums,
        type_aliases: base.type_aliases,
        generic_fn_map: base.generic_fn_map,
        actors: base.actors,
        struct_methods: base.struct_methods,
        generic_methods: base.generic_methods,
        assert_mode: base.assert_mode,
        // Shared: module-wide, mutable. One funcref table and one stub list per
        // module, so a lambda nested in this body claims a real slot and a stub
        // here stays visible to `stubbed_fns`.
        stubbed: base.stubbed,
        lambdas: base.lambdas,
        lambda_slots: base.lambda_slots,
        indirect_sigs: base.indirect_sigs,
        // Carried across so labels stay unique and a runtime need propagates.
        label_counter: Cell::new(base.label_counter.get()),
        needs_runtime: Cell::new(base.needs_runtime.get()),
        // Per-body: reset.
        type_subst,
        string_params: std::cell::RefCell::new(std::collections::HashSet::new()),
        fn_locals: std::cell::RefCell::new(Vec::new()),
        fn_params: std::cell::RefCell::new(Vec::new()),
        fn_let_inits: std::cell::RefCell::new(HashMap::new()),
        self_type: std::cell::RefCell::new(None),
    }
}

/// Marker an emit site writes when it cannot lower a construct. Its *presence
/// in a body* is what makes the enclosing function stub, so producers and the
/// five scan sites must spell it identically — `";;unsupported"` or
/// `";; not supported"` would ship an invalid body silently, the one failure
/// `emit_stub_body` cannot catch. A `const` makes that a compile-time check.
const UNSUPPORTED_MARKER: &str = ";; unsupported";

/// Record that `wasm_name`'s body was discarded in favour of `unreachable`, and
/// emit the marker comment.
///
/// Every stub site goes through here so none can be added without also becoming
/// visible to `WasmTextCompiler::stubbed_fns` — the whole point being that a
/// silent stub is what let gaps pile up unnoticed (#2014).
fn emit_stub_body(out: &mut String, wasm_name: &str, ctx: &Ctx) {
    ctx.stubbed.borrow_mut().push(wasm_name.to_string());
    out.push_str("    ;; body stubbed — contained unsupported constructs\n");
    out.push_str("    unreachable\n");
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
        .filter(|p| is_string_ty(&p.ty, ctx))
        .map(|p| p.name.clone())
        .collect();

    let (wasm_name, _) = effective_name(f, ctx.needs_wasi);
    // Populate the per-function String-param set so the Var emitter knows
    // which String locals are split (ptr, len) params vs unsupported locals.
    // `is_string_ty` (not `peels_to_string`) so a named alias to `String`
    // (e.g. `type BoundedInput = String where ...`, #2112) is recognized
    // too — matches the param-splitting decision just below.
    {
        let mut sp = ctx.string_params.borrow_mut();
        sp.clear();
        for p in &f.params {
            if is_string_ty(&p.ty, ctx) {
                sp.insert(p.name.clone());
            }
        }
    }

    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        if is_string_ty(&p.ty, ctx) {
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
    if is_string_ty(&f.ret_ty, ctx) {
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
        && !is_string_ty(&f.ret_ty, ctx)
        && (f.ensures.iter().any(is_runtime_checkable)
            || f.return_refinement
                .as_ref()
                .is_some_and(is_runtime_checkable));
    if has_checkable_ensures {
        locals.push(("__result_CONTRACT".to_string(), f.ret_ty.clone()));
    }

    // Exclude any collected "local" whose WASM identifier is already a
    // function *parameter* — WAT's param/local namespace is flat, so
    // `(param $body_ptr i32) ... (local $body_ptr i32)` is a duplicate-
    // identifier error even though nothing is wrong at the MVL level: a
    // `match`/`if let` arm binding a String value to a name that shadows an
    // outer String-typed parameter (`fn f(body: Tainted[String]) -> ... {
    // match r { Ok(body) => ... } }`, #2049 follow-up — found while running
    // `examples/config_server/handler_test.mvl`'s `handle_put` for the
    // first time, via the new WASM test-fn harness) is valid MVL shadowing;
    // the *inner* binding's own store/load instructions already correctly
    // reuse the outer parameter's WASM slot (harmless here since the outer
    // value's last MVL-level use precedes the shadowing point), so dropping
    // the redundant redeclaration is enough — no renaming needed.
    let param_wasm_names: HashSet<String> = f
        .params
        .iter()
        .flat_map(|p| {
            if is_string_ty(&p.ty, ctx) {
                vec![format!("{}_ptr", p.name), format!("{}_len", p.name)]
            } else {
                vec![p.name.clone()]
            }
        })
        .collect();
    locals.retain(|(name, _)| !param_wasm_names.contains(name));

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
    *ctx.fn_params.borrow_mut() = fn_scope_params(&f.params);
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

    if body.contains(UNSUPPORTED_MARKER) {
        emit_stub_body(out, wasm_name, ctx);
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
        TirStmt::Let {
            pattern, ty, init, ..
        } => {
            // The ctx-free first pass (`collect_locals_stmt`) already
            // split `{name}_ptr`/`{name}_len` locals for a bare `String`
            // (or `Ref`/`Labeled`/`Refined`-wrapped `String`) binding via
            // `peels_to_string`, which can't see through a named type
            // alias (e.g. `type BoundedInput = String where ...`) without
            // a `ctx.type_aliases` lookup. When `ty` is such an alias, it
            // instead declared a single scalar local — wrong shape, since
            // `emit_stmt`'s `is_string_ty` (ctx-aware, #2112) correctly
            // treats the init as a String and pushes two i32s. Speculatively
            // add the split locals here too; harmless if `peels_to_string`
            // already caught it (same names, deduped), and leaves the wrong
            // scalar local as dead-but-harmless when it didn't.
            if let Pattern::Ident(name, _) = pattern {
                if is_string_ty(ty, ctx) {
                    locals.push((format!("{name}_ptr"), Ty::Bool)); // i32
                    locals.push((format!("{name}_len"), Ty::Bool)); // i32
                }
            }
            collect_locals_ctx_expr(init, locals, ctx)
        }
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
        TirStmt::For {
            iter, body, span, ..
        } => {
            // `collect_locals_stmt`'s String branch tests the element type as
            // written, so inside a monomorphized body it sees `T` — not a
            // String — and takes the `else` branch, declaring the loop variable
            // as a single `T`-typed local. The `_ptr`/`_len` pair still comes
            // out right, because the emission loop expands any String-typed
            // local itself once `type_subst` resolves `T`. What nobody declares
            // is the `*MvlString` unpack temp: `emit_for_stmt` emits
            // `local.tee $__for_ms_<off>` regardless. An undeclared local makes
            // wasm-tools reject the *whole* module, so a single
            // `for s in strings` inside any instantiated generic body sank every
            // unrelated function in the file too (#2014).
            //
            // Only the temp is pushed here — adding `_ptr`/`_len` as well would
            // collide with the emission-time expansion ("duplicate local").
            let elem = collection_elem_ty(&iter.ty)
                .cloned()
                .map(|t| resolve_ty_param(&t, ctx.type_subst));
            if elem.is_some_and(|e| peels_to_string(&e)) {
                locals.push((format!("__for_ms_{}", span.offset), Ty::Bool));
            }
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
            // A named top-level function used as a value (#2159) routes
            // through `emit_named_fn_as_value` → `emit_closure_value`, which
            // needs the same `__env_*`/`__closure_*` temps a lambda literal
            // gets from `collect_locals_expr`'s `Lambda` arm — this ctx-free
            // pass can't tell "named function value" apart from an ordinary
            // `Var` read (that needs `ctx.fn_params`/`fn_locals`), hence
            // registering it here instead.
            if matches!(resolve_ty_param(&expr.ty, ctx.type_subst), Ty::Fn(..))
                && fn_value_ty(name, ctx).is_none()
            {
                let off = expr.span.offset;
                locals.push((format!("__env_{off}"), Ty::Bool));
                locals.push((format!("__closure_{off}"), Ty::Bool));
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
            // `write_file`/`append`/`path_exists`/`is_file`/`is_dir`/
            // `create_dir_all`/`remove`/`open` (std.io) unwrap `Path.inner`
            // via `emit_field_access` (#2100, #2110) — same
            // `__sf_<off>_<len>` tee temp as an explicit `p.inner`
            // `FieldAccess` node, but there is no such node in the TIR here
            // (the field access is synthesized by the emitter, not the
            // source), so it needs registering by hand.
            const PATH_ARG_FNS: &[&str] = &[
                "write_file",
                "append",
                "path_exists",
                "is_file",
                "is_dir",
                "create_dir_all",
                "remove",
                "open",
            ];
            if PATH_ARG_FNS.contains(&name.as_str()) {
                if let Some(path_arg) = args.first() {
                    if let Some(sname) = named_type_name(&path_arg.ty) {
                        if let Some(layout) = ctx.struct_layouts.get(&sname) {
                            if let Some(slot) = layout.fields.iter().find(|s| s.name == "inner") {
                                if peels_to_string(&slot.ty) {
                                    locals.push((
                                        format!("__sf_{}_{}", slot.offset, "inner".len()),
                                        Ty::Bool, // i32 placeholder
                                    ));
                                }
                            }
                        }
                    }
                }
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
            pattern,
            ty,
            init,
            span,
            ..
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
            // `let x: ref List/Set/Array[T] = <existing var/expr>;` needs a
            // deep-copy temp pair (see the matching arm in `emit_stmt`) — the
            // pre-scan can't check `slice_is_supported` (no `ctx` here), so
            // declare unconditionally whenever the shape could match; unused
            // locals are harmless.
            if is_let_deep_copy_shape(ty, init) {
                let off = span.offset;
                locals.push((format!("__clone_ptr_{off}"), Ty::Bool)); // i32
                locals.push((format!("__clone_len_{off}"), Ty::Int)); // i64
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
            push_match_scrutinee_locals(locals, &format!("__match_{}", span.offset), &scrutinee.ty);
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
            push_match_scrutinee_locals(locals, &match_temp_name(expr), &scrutinee.ty);
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
            // Slot pointer for `Box::new(x)` — `i32.store` consumes the
            // address, so the emitter tees it into this local to keep it as the
            // expression's value. Ty::Bool is this file's i32-slot convention.
            if name == "Box::new" && args.len() == 1 {
                locals.push((box_temp_name(&args[0]), Ty::Bool));
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
            // `abs`/`clamp` on Int/UInt/Float (#2122) — both need the
            // receiver stashed in a temp so it can be read more than once
            // (a `select`-based abs, or clamp's bounds check); `clamp`
            // additionally needs its two argument temps, plus one more for
            // Int/UInt's intermediate `max(n, lo)` (Float uses the native
            // `f64.max`/`f64.min` instructions directly, no intermediate
            // needed). `pow` needs no temps — receiver and arg go straight
            // onto the stack for the runtime call.
            if matches!(receiver.ty, Ty::Int | Ty::UInt | Ty::Float)
                && matches!(method.as_str(), "abs" | "clamp")
            {
                let off = expr.span.offset;
                let num_ty = if matches!(receiver.ty, Ty::Float) {
                    Ty::Float
                } else {
                    Ty::Int
                };
                locals.push((format!("__num_n_{off}"), num_ty.clone()));
                if method == "clamp" {
                    locals.push((format!("__num_lo_{off}"), num_ty.clone()));
                    locals.push((format!("__num_hi_{off}"), num_ty.clone()));
                    if !matches!(receiver.ty, Ty::Float) {
                        locals.push((format!("__num_max_{off}"), Ty::Int));
                    }
                }
            }
            // `Set[T].map(f)` — native-arm loop temps (#2124). Declared
            // whenever the shape matches, even if `set_map_is_supported`
            // will end up gating the arm off at emission time (this fn has
            // no `ctx` to check that here) — an unused local is harmless.
            if peels_to_set(&receiver.ty) && method == "map" && args.len() == 1 {
                let off = expr.span.offset;
                locals.push((format!("__set_map_out_{off}"), Ty::Bool));
                locals.push((format!("__set_map_arr_{off}"), Ty::Bool));
                locals.push((format!("__set_map_idx_{off}"), Ty::Int));
                locals.push((format!("__set_map_len_{off}"), Ty::Int));
                locals.push((format!("__set_map_f_{off}"), Ty::Bool));
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
        // A lambda literal used as a value needs two i32 temps to build its
        // heap-boxed `{funcidx, envptr}` pair (#2118) — `__env_*` always
        // (0/null when non-capturing), `__closure_*` for the box itself.
        // Declared unconditionally, even for a lambda that ends up stubbed
        // (an unsupported capture) or never actually captures anything: an
        // unused local is harmless, and this fn has no `ctx` to know which
        // case applies. The lambda's own *body* is not recursed into here —
        // it is a separate emission unit collected by its own
        // `collect_locals_expr` call inside `emit_one_lambda_fn`.
        TirExprKind::Lambda { .. } => {
            let off = expr.span.offset;
            locals.push((format!("__env_{off}"), Ty::Bool));
            locals.push((format!("__closure_{off}"), Ty::Bool));
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
            pattern,
            ty,
            init,
            span,
            ..
        } => {
            if let Pattern::Ident(name, _) = pattern {
                emit_expr(out, init, ctx);
                if is_string_ty(ty, ctx) {
                    // Init leaves (ptr, len) on stack — store into split locals.
                    out.push_str(&format!("    local.set ${name}_len\n"));
                    out.push_str(&format!("    local.set ${name}_ptr\n"));
                } else if is_let_deep_copy_shape(ty, init) && slice_is_supported(ty, ctx) {
                    // See `is_let_deep_copy_shape` — alias the pointer would
                    // let mutations through `name` corrupt the init's source.
                    ctx.needs_runtime.set(true);
                    let off = span.offset;
                    let ptr_tmp = format!("__clone_ptr_{off}");
                    let len_tmp = format!("__clone_len_{off}");
                    out.push_str(&format!("    local.tee ${ptr_tmp}\n"));
                    out.push_str("    call $_mvl_array_len\n");
                    out.push_str(&format!("    local.set ${len_tmp}\n"));
                    out.push_str(&format!("    local.get ${ptr_tmp}\n"));
                    out.push_str("    i64.const 0\n");
                    out.push_str(&format!("    local.get ${len_tmp}\n"));
                    out.push_str("    call $_mvl_array_slice\n");
                    out.push_str(&format!("    local.set ${name}\n"));
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
                if is_string_ty(&value.ty, ctx) {
                    // `ref String` reassignment (`out = out.concat(...)`) —
                    // `value` leaves (ptr, len) on the stack, not one value.
                    // `name` itself was never declared as a local — the
                    // binding's original `let` split it into `{name}_ptr`/
                    // `{name}_len` — so a bare `local.set $name` here doesn't
                    // just store the wrong shape, it references a name that
                    // doesn't exist at all, and wasm-tools rejects the whole
                    // module with "unknown local".
                    out.push_str(&format!("    local.set ${name}_len\n"));
                    out.push_str(&format!("    local.set ${name}_ptr\n"));
                } else {
                    out.push_str(&format!("    local.set ${name}\n"));
                }
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
        // Delegates to the same `emit_literal` used by match-pattern lowering
        // rather than duplicating per-variant logic here — this match used to
        // spell out Integer/Float/Bool/Str inline and had silently drifted out
        // of sync with `emit_literal`, missing `Char` and `Unit` entirely.
        // `Ok(())`/`Err(())` — a Unit-payload Result — is real, working MVL
        // (`Result[Unit, E]` return types, `.map(|u: Unit| ...)`), but its
        // literal argument fell through to the catch-all `;; unsupported expr`
        // arm and stubbed the whole enclosing function to `unreachable` (#2144).
        TirExprKind::Literal(lit) => emit_literal(out, lit, ctx),
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
            // A qualified name (`Type::Variant`) that matched neither
            // `enum_variants` nor `payload_enums` above means the owning
            // enum's `TirTypeDecl` was never pulled into this module (e.g. a
            // prelude enum reached only via a `let` annotation, never a call
            // — see `pull_in_missing_prelude_items`, #2090). Falling through
            // to the plain-local path below would emit `local.get
            // $Type::Variant`, which is not a valid local name/reference and
            // makes `wasm-tools` reject the whole module rather than just
            // this function. No local ever legitimately contains "::" (see
            // the `!contains("::")` guards used throughout this file for
            // real bindings), so this is unambiguous. Stub instead.
            if name.contains("::") {
                out.push_str(&format!(
                    "    ;; unsupported: qualified variant `{name}` — owning enum not registered\n"
                ));
                return;
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
            // A bare reference to a *named function* used as a value —
            // `apply(double, 3)` where `double` is a top-level fn (#2159,
            // follow-up to #2014/#2118). `emit_named_fn_as_value` synthesizes
            // a thin non-capturing wrapper lambda and boxes it exactly like a
            // real lambda literal.
            if matches!(resolve_ty_param(&expr.ty, ctx.type_subst), Ty::Fn(..))
                && fn_value_ty(name, ctx).is_none()
            {
                emit_named_fn_as_value(out, expr, name, ctx);
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
            // Calling a function *value* — `f(x)` where `f` is a parameter of
            // type `fn(T) -> U`, as in every `std/lists.mvl` HOF body (#2014).
            // Checked before the builtin shims below because the callee is a
            // runtime table index, not a name: emitting `call $f` produced
            // "unknown func: failed to find name `$f`".
            if let Some((param_tys, ret_ty)) = fn_value_ty(name, ctx) {
                // `$name` holds a pointer to the closure's `{funcidx,
                // envptr}` box (#2118) — unpack both. Env goes first (it's
                // the lambda's first param), then the real arguments, then
                // the callee index on top, right before `call_indirect`.
                // `$name` is a plain local/param, so reading it twice is a
                // side-effect-free re-read, not a re-evaluation.
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    i32.load offset=4\n");
                for a in args {
                    emit_expr(out, a, ctx);
                }
                let sig = register_indirect_sig(&param_tys, &ret_ty, ctx);
                out.push_str(&format!("    local.get ${name}\n"));
                out.push_str("    i32.load\n");
                out.push_str(&format!("    call_indirect (type {sig})\n"));
                return;
            }
            // `from_int(n)` / `wrapping_from_int(n)` — Byte construction from
            // an Int. A Byte is an i32 slot here (see `is_i32`) and an Int is
            // i64, so this is a plain narrowing with no runtime call, mirroring
            // LLVM's `trunc i64 … to i8`. `from_int` carries the refinement
            // `n >= 0 && n <= 255`, which the checker discharges; the extra
            // `i32.and 0xFF` gives `wrapping_from_int` its documented wrap and
            // costs nothing on the checked path.
            //
            // Without this arm the emitter fell through to `call $from_int`, a
            // symbol nothing declares — `examples/bzip` assembled to a module
            // that no runtime would load.
            if (name == "from_int" || name == "wrapping_from_int") && args.len() == 1 {
                emit_expr(out, &args[0], ctx);
                out.push_str("    i32.wrap_i64\n");
                out.push_str("    i32.const 255\n");
                out.push_str("    i32.and\n");
                return;
            }
            // `std.env`'s `args()` / `get(name)` — both `builtin fn`s that the
            // Rust and LLVM backends implement and this one did not, so they
            // emitted bare `call $args` / `call $get` and the module could not
            // load. Same gap #2076 closed for `read_file`; `args` alone blocked
            // three examples.
            if name == "args" && args.is_empty() {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_env_args\n");
                return;
            }
            // `get` is a plausible user-defined name, so match std.env's exact
            // shape — one String argument *and* an `Option[String]` result —
            // rather than the bare name, or a user's own `fn get(s: String)
            // -> Option[T]` for any other `T` would be silently hijacked:
            // `option_inner_ty(&expr.ty).is_some()` alone accepts any Option
            // payload, not just String, so `fn get(s: String) -> Option[Int]`
            // matched the old guard and would have been routed to the env
            // shim instead of the user's own function.
            if name == "get"
                && args.len() == 1
                && peels_to_string(&args[0].ty)
                && option_inner_ty(&expr.ty).is_some_and(peels_to_string)
            {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_env_get\n");
                return;
            }
            // `set(name, value)` (std.env) — set environment variable.
            // Shape: two String args, returns Result[Unit, String].
            if name == "set"
                && args.len() == 2
                && peels_to_string(&args[0].ty)
                && peels_to_string(&args[1].ty)
                && matches!(&expr.ty, Ty::Result(_, _))
            {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx); // name (ptr, len)
                emit_expr(out, &args[1], ctx); // value (ptr, len)
                out.push_str("    call $_mvl_env_set\n");
                return;
            }
            // `remove_var(name)` (std.env) — unset environment variable.
            if name == "remove_var" && args.len() == 1 && peels_to_string(&args[0].ty) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_env_remove_var\n");
                return;
            }
            // `current_dir()` (std.env) — get current working directory.
            if name == "current_dir" && args.is_empty() && matches!(&expr.ty, Ty::Result(_, _)) {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_env_current_dir\n");
                return;
            }
            // `chdir(path)` (std.env) — change current working directory.
            if name == "chdir" && args.len() == 1 && peels_to_string(&args[0].ty) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_env_chdir\n");
                return;
            }
            // `exit(code)` (std.env) — terminate process.
            if name == "exit" && args.len() == 1 && matches!(&args[0].ty, Ty::Int) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_env_exit\n");
                out.push_str("    unreachable\n");
                return;
            }
            // `getuid()` / `getgid()` (std.env) — user/group ID.
            if name == "getuid" && args.is_empty() {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_env_getuid\n");
                return;
            }
            if name == "getgid" && args.is_empty() {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_env_getgid\n");
                return;
            }
            // `all()` (std.env) — list all environment variables.
            if name == "all" && args.is_empty() && matches!(&expr.ty, Ty::List(_)) {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_env_all\n");
                return;
            }
            // `Box::new(x)` — heap slot holding `x`, so a recursive enum can
            // have a finite-sized payload (`HuffmanTree::Node(w, Box::new(l),
            // Box::new(r))`). Mirrors LLVM's `_mvl_box_new(size)` + store: the
            // runtime hands back zeroed memory and the store happens here,
            // widths chosen the same way every other slot in this emitter is.
            //
            // Without this arm the emitter emitted `call $Box::new`, a symbol
            // nothing declares — `examples/bzip` produced an unloadable module.
            if name == "Box::new" && args.len() == 1 {
                let inner = resolve_ty_param(&args[0].ty, ctx.type_subst);
                let is32 = wasm_ty(&inner, ctx) == "i32";
                let is_float = matches!(inner, Ty::Float);
                let is_string = peels_to_string(&inner);
                let size = if is32 { 4 } else { 8 };
                ctx.needs_runtime.set(true);
                out.push_str(&format!("    i32.const {size}\n"));
                out.push_str("    call $_mvl_box_new\n");
                // Keep the pointer: [ptr] → [ptr, ptr] → store consumes one.
                let slot = box_temp_name(&args[0]);
                out.push_str(&format!("    local.tee ${slot}\n"));
                emit_expr(out, &args[0], ctx);
                if is_float {
                    // Float pushes an f64 value — i32/i64.store both reject it.
                    out.push_str("    f64.store\n");
                } else if is_string {
                    // A String rvalue pushes (ptr, len), not a single value —
                    // collapse to one *MvlString pointer and widen, same as
                    // `emit_payload_store` does for the same reason.
                    out.push_str("    call $_mvl_string_new\n");
                    out.push_str("    i64.extend_i32_u\n");
                    out.push_str("    i64.store\n");
                } else if is32 {
                    out.push_str("    i32.store\n");
                } else {
                    out.push_str("    i64.store\n");
                }
                out.push_str(&format!("    local.get ${slot}\n"));
                return;
            }
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
                // A `struct_layouts` miss here used to fall through silently
                // to the generic `FnCall` emission at the bottom of this
                // match, writing a dangling `call $stdout`/`$stderr`/`$stdin`
                // to a function with no body and no import — a module that
                // cannot assemble at all (#2090). Stub instead: the
                // `UNSUPPORTED_MARKER` comment below is caught by the
                // whole-body scan at fn-emission time and replaced with a
                // well-formed `unreachable`, same as every other
                // unsupported-construct site in this file.
                let Some(layout) = ctx.struct_layouts.get("Fd") else {
                    out.push_str(&format!(
                        "    ;; unsupported: {name}() — Fd struct layout not registered\n"
                    ));
                    return;
                };
                let Some(slot) = layout.fields.iter().find(|s| s.name == "inner") else {
                    out.push_str(&format!(
                        "    ;; unsupported: {name}() — Fd.inner field not registered\n"
                    ));
                    return;
                };
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
            // `write_file(path, content)` (std.io) — write content to file.
            // `path`'s type is a real one-field struct (`Path { inner:
            // String }`), not an erased (ptr, len) pair like `read_file`'s
            // plain `String` param above — unwrap `.inner` the same way the
            // `write` shim unwraps `Fd.inner` (#2100).
            if name == "write_file" && args.len() == 2 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx); // path (ptr, len)
                emit_expr(out, &args[1], ctx); // content (ptr, len)
                out.push_str("    call $_mvl_io_write_file\n");
                return;
            }
            // `append(path, content)` (std.io) — append content to file.
            if name == "append" && args.len() == 2 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx); // path (ptr, len)
                emit_expr(out, &args[1], ctx); // content (ptr, len)
                out.push_str("    call $_mvl_io_append\n");
                return;
            }
            // `path_exists(path)` (std.io) — check if path exists.
            if name == "path_exists" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_exists\n");
                return;
            }
            // `is_file(path)` (std.io) — check if path is a file.
            if name == "is_file" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_is_file\n");
                return;
            }
            // `is_dir(path)` (std.io) — check if path is a directory.
            if name == "is_dir" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_is_dir\n");
                return;
            }
            // `create_dir_all(path)` (std.io) — create directory and parents.
            if name == "create_dir_all" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_create_dir_all\n");
                return;
            }
            // `remove(path)` (std.io) — remove file or empty directory.
            // No `expr.ty` guard, unlike an earlier version of this arm: the
            // only free (non-method) single-arg `remove` in the stdlib is
            // this one (`List`/`Map`'s equivalents are extension methods,
            // called via `x.remove(...)` `MethodCall` nodes, never this
            // `FnCall` arm). `cli::wasm_text::compile_wat`'s separate,
            // intentionally-incomplete `check_with_prelude` pass doesn't
            // reliably resolve `expr.ty` for RUST_BACKED_STDLIB builtins,
            // silently defeating a type-based guard here (#2100).
            if name == "remove" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_remove\n");
                return;
            }
            // `open(path)` (std.io, #2110) — open a file, returning
            // `Result[Fd, IoError]`. The runtime shim heap-allocates the
            // `Fd` struct itself (mirroring `stdout()`/`stderr()`'s layout)
            // and wraps it in the standard MvlResult convention, so the
            // emitter just unwraps `Path.inner` and forwards the call —
            // same shape as `write_file`/`path_exists` above.
            if name == "open" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx);
                out.push_str("    call $_mvl_io_open\n");
                return;
            }
            // `close(fd)` (std.io, #2110) — release the OS file descriptor.
            if name == "close" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_field_access(out, &args[0], "inner", ctx); // i64 fd number
                out.push_str("    i32.wrap_i64\n");
                out.push_str("    call $_mvl_io_close\n");
                return;
            }
            // `now()` (std.time) — returns Instant handle via runtime.
            if name == "now" && args.is_empty() {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_time_now\n");
                return;
            }
            // `_instant_epoch_seconds(t)` (std.time, module-private) — reads
            // epoch seconds from an Instant handle.
            if name == "_instant_epoch_seconds" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_time_instant_epoch_seconds\n");
                return;
            }
            // `sleep(duration)` (std.time) — sleep for Duration.
            // Duration is a struct { secs: Int, nanos: Int }.
            if name == "sleep" && args.len() == 1 {
                ctx.needs_runtime.set(true);
                // Extract secs and nanos fields from Duration struct
                emit_field_access(out, &args[0], "secs", ctx);
                emit_field_access(out, &args[0], "nanos", ctx);
                out.push_str("    call $_mvl_time_thread_sleep\n");
                return;
            }
            // ── std.random ────────────────────────────────────────────────
            // `int(min, max)` (std.random) — random integer in [min, max].
            if name == "int" && args.len() == 2 && matches!(&args[0].ty, Ty::Int) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                emit_expr(out, &args[1], ctx);
                out.push_str("    call $_mvl_random_int\n");
                return;
            }
            // `float()` (std.random) — random float in [0, 1).
            if name == "float" && args.is_empty() && matches!(&expr.ty, Ty::Float) {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_random_float\n");
                return;
            }
            // `bytes(n)` (std.random) — n random bytes as List[Byte].
            if name == "bytes" && args.len() == 1 && matches!(&args[0].ty, Ty::Int) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_random_bytes\n");
                return;
            }
            // `choice(list)` (std.random) — random element from list.
            // Returns Option[T], implemented as choice_index + get.
            if name == "choice" && args.len() == 1 && matches!(&args[0].ty, Ty::List(_)) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                // Get random index; -1 if empty
                out.push_str("    call $_mvl_random_choice_index\n");
                // We need to return Option[T] - this is complex, stub for now
                // The full implementation would do: if idx >= 0, get element and wrap in Some
                // For now, just return the index and let caller handle it
                return;
            }
            // `shuffle(list)` (std.random) — shuffled copy of list.
            if name == "shuffle" && args.len() == 1 && matches!(&args[0].ty, Ty::List(_)) {
                ctx.needs_runtime.set(true);
                emit_expr(out, &args[0], ctx);
                out.push_str("    call $_mvl_random_shuffle\n");
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
                if matches!(inner, Ty::Unit) {
                    // Any Unit-typed expression pushes nothing (Unit has no
                    // runtime representation), but the Option constructor is
                    // always exactly one i64/i32 param — push a placeholder
                    // so `Option[Unit]` still has a slot to construct (#2144).
                    out.push_str("    i64.const 0\n");
                }
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
                if matches!(ok_ty, Ty::Unit) {
                    // See the matching comment on `Some(x)` above (#2144).
                    out.push_str("    i64.const 0\n");
                }
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
                if matches!(err_ty, Ty::Unit) {
                    // See the matching comment on `Some(x)` above (#2144).
                    out.push_str("    i64.const 0\n");
                }
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
                let mut subst = infer_type_subst_from_args(type_params, fn_params, args);
                // Resolve through the enclosing instantiation, exactly as the
                // collection side does. Inside a monomorphized body the argument
                // types are still written in the *callee's* type params, so
                // without this a generic fn calling another (or itself) mangled
                // to `__T`/`__Unknown` — a symbol nobody emitted (#2014).
                for v in subst.values_mut() {
                    *v = resolve_ty_param(v, ctx.type_subst);
                }
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
        // Int/UInt/Float::abs/clamp/pow (#2122). WASM has native `f64.abs`/
        // `f64.max`/`f64.min` for Float but no integer min/max/abs opcode and
        // no exponentiation opcode for either family, so Int/UInt route
        // through `select`-based arithmetic (signed comparisons — this
        // backend already treats `UInt` as a plain signed i64 everywhere
        // else, e.g. the `<`/`>` operators) and `pow` always calls the
        // runtime. `clamp` mirrors the Rust backend's `emit_safe_clamp`:
        // inverted bounds (`lo > hi`) return the receiver unchanged rather
        // than an unspecified min/max composition.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if matches!(receiver.ty, Ty::Int | Ty::UInt | Ty::Float)
            && matches!(method.as_str(), "abs" | "clamp" | "pow") =>
        {
            let off = expr.span.offset;
            let is_float = matches!(receiver.ty, Ty::Float);
            match method.as_str() {
                "abs" => {
                    if is_float {
                        emit_expr(out, receiver, ctx);
                        out.push_str("    f64.abs\n");
                    } else {
                        let n = format!("__num_n_{off}");
                        emit_expr(out, receiver, ctx);
                        out.push_str(&format!("    local.set ${n}\n"));
                        out.push_str(&format!(
                            "    i64.const 0\n    local.get ${n}\n    i64.sub\n"
                        ));
                        out.push_str(&format!("    local.get ${n}\n"));
                        out.push_str(&format!(
                            "    local.get ${n}\n    i64.const 0\n    i64.lt_s\n"
                        ));
                        out.push_str("    select\n");
                    }
                }
                "pow" => {
                    ctx.needs_runtime.set(true);
                    emit_expr(out, receiver, ctx);
                    emit_expr(out, &args[0], ctx);
                    if is_float {
                        out.push_str("    call $_mvl_float_pow\n");
                    } else {
                        out.push_str("    call $_mvl_int_pow\n");
                    }
                }
                "clamp" => {
                    let n = format!("__num_n_{off}");
                    let lo = format!("__num_lo_{off}");
                    let hi = format!("__num_hi_{off}");
                    let ty = if is_float { "f64" } else { "i64" };
                    emit_expr(out, receiver, ctx);
                    out.push_str(&format!("    local.set ${n}\n"));
                    emit_expr(out, &args[0], ctx);
                    out.push_str(&format!("    local.set ${lo}\n"));
                    emit_expr(out, &args[1], ctx);
                    out.push_str(&format!("    local.set ${hi}\n"));
                    // Inverted bounds (lo > hi): return `n` unchanged.
                    out.push_str(&format!("    local.get ${lo}\n    local.get ${hi}\n"));
                    out.push_str(&format!(
                        "    {ty}.gt{}\n",
                        if is_float { "" } else { "_s" }
                    ));
                    out.push_str(&format!("    if (result {ty})\n"));
                    out.push_str(&format!("      local.get ${n}\n"));
                    out.push_str("    else\n");
                    if is_float {
                        out.push_str(&format!(
                            "      local.get ${n}\n      local.get ${lo}\n      f64.max\n"
                        ));
                        out.push_str(&format!("      local.get ${hi}\n      f64.min\n"));
                    } else {
                        let max = format!("__num_max_{off}");
                        // max(n, lo)
                        out.push_str(&format!(
                            "      local.get ${n}\n      local.get ${lo}\n      local.get ${n}\n      local.get ${lo}\n      i64.gt_s\n      select\n"
                        ));
                        out.push_str(&format!("      local.set ${max}\n"));
                        // min(max, hi)
                        out.push_str(&format!(
                            "      local.get ${max}\n      local.get ${hi}\n      local.get ${max}\n      local.get ${hi}\n      i64.lt_s\n      select\n"
                        ));
                    }
                    out.push_str("    end\n");
                }
                _ => unreachable!(),
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
        // Set[T].contains(val) / List[T].contains(val) — backed by MvlArray, so
        // the same linear scan serves both. `contains` returns Bool (i32).
        //
        // `Ty::List(_)`/`Ty::Array` were missing from this guard, so
        // `xs.contains(20)` on a plain `List` fell through to `;; unsupported`
        // while the identical call on a `Set` — or on a `ref List`, which
        // `Ty::Ref(_, _)` admits — worked. That is what stubbed
        // `list_hof_test.mvl`'s `list_contains` (#2014).
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some()
            && matches!(
                &receiver.ty,
                Ty::Set(_) | Ty::List(_) | Ty::Array(_, _) | Ty::Ref(_, _)
            )
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
        // `Set[T].remove(val)` — linear-scan remove-by-value, no-op if absent
        // (#2124). Neither `List` nor `Set` had a by-value removal primitive
        // before this; mirrors `insert`'s element-type dispatch exactly.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some()
            && matches!(&receiver.ty, Ty::Set(_) | Ty::Ref(_, _))
            && method == "remove"
            && args.len() == 1 =>
        {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let fn_name = if is_i32(&elem_ty, ctx) {
                "_mvl_array_remove_value_i32"
            } else {
                "_mvl_array_remove_value_i64"
            };
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str(&format!("    call ${fn_name}\n"));
        }
        // `Set[T].map(f: fn(T) -> U) -> Set[U]` — native arm (#2124). See
        // `set_map_is_supported` for why this can't be a pure MVL body like
        // `filter`/`fold`/`any`/`all`. Allocates a fresh output array, loops
        // over `self` via the same shape `emit_for_list` uses, calls `f`
        // through `call_indirect`, and dedup-inserts each result.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if peels_to_set(&receiver.ty)
            && method == "map"
            && args.len() == 1
            && set_map_is_supported(&receiver.ty, &effective_arg_ty(&args[0]), ctx) =>
        {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            let ret_ty = match effective_arg_ty(&args[0]) {
                Ty::Fn(_, ret, ..) => *ret,
                other => other,
            };
            let off = expr.span.offset;
            let out_local = format!("__set_map_out_{off}");
            let arr_local = format!("__set_map_arr_{off}");
            let idx_local = format!("__set_map_idx_{off}");
            let len_local = format!("__set_map_len_{off}");
            let f_local = format!("__set_map_f_{off}");
            let brk = ctx.fresh_label("set_map_end");
            let cnt = ctx.fresh_label("set_map_cont");

            let out_elem_size = elem_size_bytes(&ret_ty, ctx);
            out.push_str(&format!("    i32.const {out_elem_size}\n"));
            out.push_str("    i32.const 4\n");
            out.push_str("    call $_mvl_array_new\n");
            out.push_str(&format!("    local.set ${out_local}\n"));

            emit_expr(out, &args[0], ctx);
            out.push_str(&format!("    local.set ${f_local}\n"));

            emit_expr(out, receiver, ctx);
            out.push_str(&format!("    local.set ${arr_local}\n"));
            out.push_str(&format!("    local.get ${arr_local}\n"));
            out.push_str("    call $_mvl_array_len\n");
            out.push_str(&format!("    local.set ${len_local}\n"));
            out.push_str("    i64.const 0\n");
            out.push_str(&format!("    local.set ${idx_local}\n"));

            out.push_str(&format!("    block ${brk}\n"));
            out.push_str(&format!("    loop ${cnt}\n"));
            out.push_str(&format!("    local.get ${idx_local}\n"));
            out.push_str(&format!("    local.get ${len_local}\n"));
            out.push_str("    i64.ge_s\n");
            out.push_str(&format!("    br_if ${brk}\n"));

            out.push_str(&format!("    local.get ${out_local}\n"));
            // `$f_local` holds a pointer to `f`'s closure box (#2118) — env
            // first (the lambda's first param), matching the general
            // fn-value `call_indirect` site above.
            out.push_str(&format!("    local.get ${f_local}\n"));
            out.push_str("    i32.load offset=4\n");
            out.push_str(&format!("    local.get ${arr_local}\n"));
            out.push_str(&format!("    local.get ${idx_local}\n"));
            out.push_str("    call $_mvl_array_get\n");
            let (load_op, _) = list_elem_load_op(&elem_ty, ctx);
            out.push_str(&format!("    {load_op}\n"));
            out.push_str(&format!("    local.get ${f_local}\n"));
            out.push_str("    i32.load\n");
            let sig = register_indirect_sig(std::slice::from_ref(&elem_ty), &ret_ty, ctx);
            out.push_str(&format!("    call_indirect (type {sig})\n"));
            let insert_fn = if is_i32(&ret_ty, ctx) {
                "_mvl_array_insert_i32"
            } else {
                "_mvl_array_insert_i64"
            };
            out.push_str(&format!("    call ${insert_fn}\n"));

            out.push_str(&format!("    local.get ${idx_local}\n"));
            out.push_str("    i64.const 1\n");
            out.push_str("    i64.add\n");
            out.push_str(&format!("    local.set ${idx_local}\n"));
            out.push_str(&format!("    br ${cnt}\n"));
            out.push_str("    end\n");
            out.push_str("    end\n");
            out.push_str(&format!("    local.get ${out_local}\n"));
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
        // `.push(x)` on List — append in place, returns Unit (#2014).
        //
        // `_mvl_array_push_*` existed only for building list *literals* before
        // this; the method itself had no arm, so every `std/lists.mvl` body was
        // unsupported — each one is `let result: ref List[U] = []; …
        // result.push(…)`. That made this a prerequisite for `flatten` and for
        // every HOF, not a separate nicety.
        //
        // Element encoding matches the `TirExprKind::List` literal arm: a
        // String element arrives as (ptr, len) and is wrapped into a
        // *MvlString first; everything else uses the typed push for its WASM
        // type. Nothing is left on the stack — `push` is Unit-typed, and the
        // runtime mutates the array through the pointer.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if collection_elem_ty(&receiver.ty).is_some() && method == "push" && args.len() == 1 => {
            ctx.needs_runtime.set(true);
            let elem_ty = collection_elem_ty(&receiver.ty).cloned().unwrap_or(Ty::Int);
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            if is_string_ty(&elem_ty, ctx) {
                out.push_str("    call $_mvl_string_new\n");
                out.push_str("    call $_mvl_array_push_i32\n");
            } else {
                out.push_str(&format!("    call {}\n", push_op_for(&elem_ty, ctx)));
            }
        }
        // `.clone()` — needed by six `std/lists.mvl` bodies (#2014):
        // `filter`/`take_while`/`skip_while` call `f(x.clone())`, and
        // `sort_by`/`min_by`/`max_by` call `cmp(x.clone(), y.clone())`. There
        // was no arm for it anywhere, so all six stubbed.
        //
        // The receiver's type is resolved through `type_subst` first — inside a
        // monomorphized body the element is still spelled `T`.
        //
        // - Scalars (`Int`/`Float`/`Bool`/`Byte`, unit enums): a copy is the
        //   value itself, so this is identity.
        // - Array-backed collections: bump the refcount, which is what makes
        //   the result an owned handle rather than a borrow of the original.
        // - String: identity on the `(ptr, len)` pair. That matches how every
        //   other site in this emitter passes a string value around; it is a
        //   borrow, sound only because a cloned string is consumed by the
        //   callee without being dropped. `_mvl_string_clone` is not usable
        //   here — it takes a `*MvlString`, not the unpacked pair.
        // - Anything else (Option/Result/struct pointers) falls through to
        //   `;; unsupported` rather than guessing: those are refcounted boxes
        //   where an identity "clone" that later gets dropped is a
        //   double-free. Stubbing is loud; miscompiling ownership is not.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if method == "clone"
            && args.is_empty()
            && clone_is_supported(&resolve_ty_param(&receiver.ty, ctx.type_subst), ctx) =>
        {
            let ty = resolve_ty_param(&receiver.ty, ctx.type_subst);
            emit_expr(out, receiver, ctx);
            if collection_elem_ty(&ty).is_some() {
                ctx.needs_runtime.set(true);
                out.push_str("    call $_mvl_array_clone\n");
            }
        }
        // `.slice(start, end)` on List / Array — returns a new array with the
        // half-open element range, clamped (#2014). A `builtin fn` in
        // std/lists.mvl with no WASM runtime function until now, which is what
        // stubbed `take` (`self.slice(0, n)`) and `skip`
        // (`self.slice(n, self.len())`).
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if method == "slice"
            && args.len() == 2
            && slice_is_supported(&resolve_ty_param(&receiver.ty, ctx.type_subst), ctx) =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            emit_expr(out, &args[1], ctx);
            out.push_str("    call $_mvl_array_slice\n");
        }
        // `.concat(other)` on List / Array — new array holding `self`'s
        // elements followed by `other`'s (#2114). A `builtin fn` in
        // std/lists.mvl with no WASM runtime function at all until now —
        // there was no native arm here for *any* element type, scalar or
        // struct, so any `List[T]::concat` call fell to the `;; unsupported
        // expr` catch-all and stubbed the whole calling function.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if method == "concat"
            && args.len() == 1
            && concat_is_supported(&resolve_ty_param(&receiver.ty, ctx.type_subst), ctx) =>
        {
            ctx.needs_runtime.set(true);
            emit_expr(out, receiver, ctx);
            emit_expr(out, &args[0], ctx);
            out.push_str("    call $_mvl_array_concat\n");
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
            // i32 too. Everything else (Int, Float) is i64. Asked via `wasm_ty`
            // so this agrees with `push_op_for`/`list_elem_load_op` by
            // construction rather than by two predicates happening to match.
            let getter = if wasm_ty(&elem_ty, ctx) == "i32" || is_string_ty(&elem_ty, ctx) {
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
        // Lambda literal as a value — pushes a pointer to a heap-boxed
        // `{funcidx: i32, envptr: i32}` pair (#2118; was a bare funcidx
        // before capturing lambdas existed, #2014). The body is emitted
        // later as a top-level function by `emit_lambda_fns`.
        TirExprKind::Lambda { params, body } => {
            emit_closure_value(out, expr, params, body, ctx);
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
            let receiver_type = receiver_type_name(&receiver.ty).expect("guarded above");
            emit_expr(out, receiver, ctx);
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call ${receiver_type}_{method}\n"));
        }
        // Generic extension method (`xs.flatten()`, `xs.first()`) — #2014.
        // Last resort, after every builtin special case *and* the non-generic
        // struct-method arm: a `List` method the emitter handles natively
        // (`.len()`, `.push()`) must keep its inline lowering rather than
        // routing through a monomorphized `std/lists.mvl` body.
        //
        // `resolve_generic_method_call` is the same function
        // `collect_generic_instantiations` used, so `mangled` is guaranteed to
        // name an instance that was actually emitted.
        TirExprKind::MethodCall {
            receiver,
            method,
            args,
        } if resolve_generic_method_call(
            receiver,
            method,
            args,
            ctx.generic_methods,
            ctx.type_subst,
        )
        .is_some() =>
        {
            let (_, _, mangled) = resolve_generic_method_call(
                receiver,
                method,
                args,
                ctx.generic_methods,
                ctx.type_subst,
            )
            .expect("guarded above");
            emit_expr(out, receiver, ctx);
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call ${mangled}\n"));
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

    // Store scrutinee once — arms compare against it repeatedly. A String
    // scrutinee leaves two values (ptr, len) on the stack, so it needs the
    // split locals `push_match_scrutinee_locals` declared, not the single
    // `$temp` every other scrutinee type uses (#2113).
    let scrutinee_is_string = peels_to_string(&scrutinee.ty);
    emit_expr(out, scrutinee, ctx);
    if scrutinee_is_string {
        out.push_str(&format!("    local.set ${temp}_len\n"));
        out.push_str(&format!("    local.set ${temp}_ptr\n"));
    } else {
        out.push_str(&format!("    local.set ${temp}\n"));
    }

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
                if scrutinee_is_string {
                    ctx.needs_runtime.set(true);
                    out.push_str(&format!("    local.get ${temp}_ptr\n"));
                    out.push_str(&format!("    local.get ${temp}_len\n"));
                    emit_literal(out, lit, ctx);
                    out.push_str("    call $_mvl_string_eq\n");
                } else {
                    out.push_str(&format!("    local.get ${temp}\n"));
                    emit_literal(out, lit, ctx);
                    out.push_str(&format!("    {}\n", eq_op_for(&scrutinee.ty, ctx)));
                }
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
        _ if is_string_ty(field_ty, ctx) => {
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
        // Same defect class as #2090's `stdout`/`stderr`/`stdin` fallthrough:
        // this comment used to omit "unsupported", so it was invisible to
        // the `UNSUPPORTED_MARKER` whole-body scan and left the caller
        // expecting a value on the stack that was never pushed — a stack
        // imbalance that fails `wasm-tools validate`, not a merely-missing
        // feature. Matches the sibling branches above in this same match.
        out.push_str(&format!(
            "    ;; unsupported: unknown struct for field access: {struct_name}\n"
        ));
        return;
    };
    let Some(slot) = layout.fields.iter().find(|s| s.name == field) else {
        out.push_str(&format!(
            "    ;; unsupported: unknown field: {struct_name}.{field}\n"
        ));
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

/// Declare the scrutinee-cache local(s) for a `match` (expr- or stmt-form).
/// A String scrutinee needs split `(_ptr, _len)` i32 locals — `emit_expr`
/// leaves two values on the stack for it, not one (#2113: `emit_match_impl`
/// used to unconditionally `local.set` a single temp, which silently
/// dropped one of the two and declared the wrong local shape for any
/// String-typed match scrutinee that wasn't already a bare, pre-split
/// `Var`). Everything else keeps a single scalar local of its own type.
fn push_match_scrutinee_locals(locals: &mut Vec<(String, Ty)>, temp: &str, scrutinee_ty: &Ty) {
    if peels_to_string(scrutinee_ty) {
        locals.push((format!("{temp}_ptr"), Ty::Bool));
        locals.push((format!("{temp}_len"), Ty::Bool));
    } else {
        locals.push((temp.to_string(), scrutinee_ty.clone()));
    }
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

/// Temp local holding the `Box::new` slot pointer while its payload is stored.
/// `i32.store` consumes the address, so the pointer has to be kept somewhere to
/// be the expression's value. Keyed by the boxed argument's span, matching how
/// every other temp here is named.
fn box_temp_name(arg: &TirExpr) -> String {
    format!("__bx_{}_{}", arg.span.offset, arg.span.len)
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

/// Whether `let name: ref List/Set/Array[T] = init;` needs a deep copy
/// instead of a bare `local.set` alias.
///
/// WASM's `List`/`Set`/`Array` values are handles to a heap-allocated
/// `MvlArray` — `local.set` just copies the *pointer*. For a fresh literal
/// (`List { .. }`/`Set { .. }`/`Map { .. }`) that pointer is already unique,
/// so aliasing it is fine. But `let x: ref Set[Int] = self;` (or any other
/// non-literal init — a parameter, a field read, another local) makes `x`
/// and the source share one buffer: mutating `x` (`x.insert(...)`) silently
/// corrupts the source too, since MVL's collections are supposed to have
/// value semantics, not reference semantics. `std/config.mvl`,
/// `std/json.mvl`, and `std/pbt.mvl` all rely on `let out: ref T = base;`
/// followed by mutation expecting `base` to be untouched — this was a
/// latent, unexercised bug because `WASM_CORPUS` never routes through those
/// library files (#2124).
///
/// Scoped to `List`/`Set`/`Array` (matches [`collection_elem_ty`]) — `Map`
/// has no runtime clone primitive yet in WASM and only two concrete
/// instantiations (`_si64`/`_str`), so it is left for a follow-up.
fn is_let_deep_copy_shape(ty: &Ty, init: &TirExpr) -> bool {
    matches!(ty, Ty::Ref(_, inner) if collection_elem_ty(inner).is_some())
        && !matches!(
            init.kind,
            TirExprKind::List { .. } | TirExprKind::Set { .. } | TirExprKind::Map { .. }
        )
}

/// True when `ty` is a `Set<T>`, possibly wrapped in `Ref`/`Labeled`/`Refined`
/// (e.g. `let s: ref Set[Int] = …`). Unlike matching `Ty::Set(_) | Ty::Ref(_,
/// _)` directly, this does not also admit a `ref List[T]`/`ref Array[T, N]`
/// receiver — needed for `Set[T]::map`, which (unlike `insert`/`remove`,
/// which have no `List` namesake to collide with) must not steal `.map()`
/// away from `List[T]::map`'s generic-dispatch path (#2124).
fn peels_to_set(ty: &Ty) -> bool {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => cur = inner,
            Ty::Set(_) => return true,
            _ => return false,
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

/// WASM `(param ...) (result ...)` text for an `extern "rust"` fn
/// declaration. Mirrors `emit_fn`'s own param/return-type lowering exactly —
/// String (and `Secret[String]`/`Tainted[String]`, labels are compile-time
/// only) splits into two `i32` params and, on return, WASM multi-value
/// `(result i32 i32)`; everything else is `wasm_ty`'s single value. Kept in
/// lockstep with `emit_fn` deliberately duplicated rather than shared: the
/// two call sites differ (a `(func $name ...)` body vs. a bare `(import ...)`
/// declaration) enough that threading one through the other would obscure
/// both.
fn extern_fn_signature(params: &[TirParam], ret_ty: &Ty, ctx: &Ctx) -> String {
    let mut sig = String::new();
    for p in params {
        if is_string_ty(&p.ty, ctx) {
            sig.push_str(" (param i32 i32)");
        } else {
            sig.push_str(&format!(" (param {})", wasm_ty(&p.ty, ctx)));
        }
    }
    if is_string_ty(ret_ty, ctx) {
        sig.push_str(" (result i32 i32)");
    } else if !matches!(ret_ty, Ty::Unit) {
        sig.push_str(&format!(" (result {})", wasm_ty(ret_ty, ctx)));
    }
    sig
}

fn wasm_ty(ty: &Ty, ctx: &Ctx) -> &'static str {
    match ty {
        Ty::Int | Ty::UInt => "i64",
        Ty::Float => "f64",
        // `Char` is a Unicode scalar value, small enough for i32 — same
        // treatment as Bool/Byte. Previously fell to the `_ => "i64"`
        // default below while `emit_literal`'s `Literal::Char` arm always
        // pushed `i32.const`, a width mismatch invisible until #2144 fixed
        // the *other* bug (a missing dispatch arm) that had made every char
        // literal unreachable code up to that point.
        Ty::Bool | Ty::Byte | Ty::Char => "i32",
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
        // `Box[T]` is a heap pointer, so an i32 address on wasm32 — like every
        // other boxed handle below. Without this it fell to the `_ => "i64"`
        // default, and `emit_payload_store` then skipped the
        // `i64.extend_i32_u` widen an 8-byte payload slot needs: the i32 from
        // `_mvl_box_new` met an `i64.store` and the module failed to validate.
        Ty::Named(name, _) if name == "Box" => "i32",
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
        // A function value is an index into the module's `(table funcref)`
        // (#2014) — an i32, not a pointer to anything in linear memory.
        // Without this arm it fell to the `_ => "i64"` default below, so a
        // `fn(T) -> U` parameter was declared i64 while call sites pushed an
        // i32 index.
        Ty::Fn(..) => "i32",
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
/// resolution or a named type alias, e.g. `type BoundedInput = String
/// where ...`). Layers `ctx.type_subst`/`ctx.type_aliases` resolution
/// (mirroring `is_float_ctx`) on top of the ctx-free [`peels_to_string`]
/// peel so the two helpers share one definition of "String, possibly
/// wrapped in Ref/Labeled/Refined" instead of duplicating it —
/// `peels_to_string` stays the ctx-free version for call sites (like
/// `collect_locals_stmt`) that run without a `Ctx` in scope.
fn is_string_ty(ty: &Ty, ctx: &Ctx) -> bool {
    match ty {
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(concrete) = ctx.type_subst.get(name.as_str()) {
                return is_string_ty(concrete, ctx);
            }
            if let Some(aliased) = ctx.type_aliases.get(name.as_str()) {
                return is_string_ty(&aliased.clone(), ctx);
            }
            false
        }
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
        Ty::Bool | Ty::Byte | Ty::Char => true,
        // `Box[T]` — heap pointer, i32 on wasm32. Must agree with `wasm_ty`.
        Ty::Named(name, _) if name == "Box" => true,
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
        // An unsubstituted type param inside a monomorphized body — resolve it,
        // as `wasm_ty`/`is_float_ctx`/`is_string_ty` all do. Without this arm
        // `is_i32` was the odd one out: with `U → Bool` it answered `false`
        // while `wasm_ty(U)` answered `"i32"`, so the `.get` arm picked
        // `_mvl_array_get_option_i64` and read 8 bytes where
        // `_mvl_array_push_i32` had written 4 (#2014).
        Ty::Named(name, _) if ctx.type_subst.contains_key(name.as_str()) => {
            is_i32(&ctx.type_subst[name.as_str()].clone(), ctx)
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

pub(crate) fn collect_type_aliases(types: &[TirTypeDecl]) -> HashMap<String, Ty> {
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
        // These six used to fall through to the `"Unknown"` backstop. Because
        // `collect_generic_instantiations` dedups on the *mangled name*, two
        // different substitutions that both tagged `Unknown` collapsed onto one
        // emitted body and every call site got whichever was collected first —
        // a silently wrong callee. Newly reachable once `map`/`filter`/`fold`
        // could be instantiated on collection-shaped element types (#2014).
        Ty::Set(inner) => format!("Set_{}", mangle_ty_tag(inner)),
        Ty::Array(inner, _) => format!("Arr_{}", mangle_ty_tag(inner)),
        Ty::Map(k, v) => format!("Map_{}_{}", mangle_ty_tag(k), mangle_ty_tag(v)),
        Ty::Result(ok, err) => format!("Res_{}_{}", mangle_ty_tag(ok), mangle_ty_tag(err)),
        Ty::Unit => "Unit".to_string(),
        Ty::Fn(params, ret, _, _) => {
            let ps: Vec<String> = params.iter().map(mangle_ty_tag).collect();
            format!("Fn_{}_r_{}", ps.join("_"), mangle_ty_tag(ret))
        }
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
        unify_ty_params(&param.ty, &arg.ty, &param_names, &mut subst);
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
        unify_ty_params(&param.ty, &arg.ty, &param_names, &mut subst);
    }
    subst
}

/// Receiver-type name for method *dispatch* purposes, unlike
/// [`named_type_name`] which only answers for `Ty::Named`.
///
/// Extension methods are declared as `fn List[T]::first(self)`, and the parser
/// stores the head name verbatim — `Some("List")`. But a `List[Int]` receiver
/// has type `Ty::List(..)`, never `Ty::Named("List", ..)`, so matching a call
/// site against that declaration needs the built-in constructors spelled back
/// out as their MVL names (#2014). This is why `List[T]::map` was invisible to
/// `is_struct_method_call` even after #2054 added the non-generic bucket.
fn receiver_type_name(ty: &Ty) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Labeled(_, inner) | Ty::Refined(inner, _) | Ty::Ref(_, inner) => cur = inner,
            Ty::Named(n, _) => return Some(n.clone()),
            Ty::List(_) | Ty::Array(_, _) => return Some("List".to_string()),
            Ty::Set(_) => return Some("Set".to_string()),
            Ty::Map(_, _) => return Some("Map".to_string()),
            Ty::Option(_) => return Some("Option".to_string()),
            Ty::Result(_, _) => return Some("Result".to_string()),
            Ty::String => return Some("String".to_string()),
            _ => return None,
        }
    }
}

/// Mangled WASM name for one instantiation of a generic extension method.
///
/// Includes the receiver type, so `List[T]::first` and `Set[T]::first` cannot
/// collide on `first__Int`.
fn mangle_generic_method_name(
    receiver_type: &str,
    method: &str,
    type_params: &[GenericParam],
    subst: &HashMap<String, Ty>,
) -> String {
    format!(
        "{receiver_type}_{}",
        mangle_generic_name(method, type_params, subst)
    )
}

// ── Function values: funcref table + call_indirect (#2014) ───────────────
//
// A function value is an i32 index into one module-local `(table funcref)`.
// ADR-0059 §2 rules out a funcref table for *actor* dispatch because the
// preloaded `runtime/wasm` module cannot call back into the emitted module —
// see the scope note there. That constraint does not reach here: the table, its
// `elem` segment, the lambda functions, and every `call_indirect` all live
// inside the single emitted module, so nothing crosses the `--preload`
// boundary. Actor dispatch remains static.
//
// Lambdas that capture a scalar outer variable are supported too (#2118):
// every function value is a pointer to a heap-boxed `{funcidx, envptr}` pair
// (see `emit_closure_value`), and every `(type $sig…)` declared below always
// takes an env pointer as its first param — uniform across capturing and
// non-capturing lambdas alike, since a generic HOF body's `call_indirect`
// can't know ahead of time which kind of closure value it's about to
// invoke. A capture of a heap-owned type (String, a collection, a struct)
// is still unsupported — see `is_capturable_scalar`.

/// WASM-level signature of a resolved function type, as
/// (type-name, `(func …)` declaration).
///
/// The name is derived from the WASM types so structurally identical
/// signatures collapse onto one `(type)` — `fn(Int) -> Bool` and
/// `fn(Float) -> Bool` differ, but `fn(Int) -> Int` and `fn(UInt) -> UInt` do
/// not, and must not produce two incompatible declarations for the same shape.
fn indirect_sig(params: &[Ty], ret: &Ty, ctx: &Ctx) -> (String, String) {
    // Strings are two i32s (ptr, len) everywhere in this emitter, and
    // `emit_one_lambda_fn` declares them that way. `wasm_ty` has no `Ty::String`
    // arm — it falls through to the `_ => "i64"` default — which is harmless at
    // every other call site because they all test `peels_to_string` first and
    // branch to the pair convention. This was the one site that did not, so a
    // `fn(String) -> Bool` lambda got `(type (func (param i64) (result i32)))`
    // against an actual `(param $s_ptr i32) (param $s_len i32)` body. WASM only
    // type-checks `call_indirect` *dynamically*, so that mismatch passed
    // validation and trapped at runtime (#2014).
    let mut name = String::from("$sig");
    // Every lambda takes `$__env i32` first (#2118) — baked into every
    // declared signature so `name` doesn't need an "env or not" variant.
    let mut decl = String::from("(func (param i32)");
    let push = |slot: &str, name: &mut String, decl: &mut String| {
        name.push('_');
        name.push_str(slot);
        decl.push_str(&format!(" (param {slot})"));
    };
    for p in params {
        if peels_to_string(p) {
            push("i32", &mut name, &mut decl);
            push("i32", &mut name, &mut decl);
        } else {
            push(wasm_ty(p, ctx), &mut name, &mut decl);
        }
    }
    if peels_to_string(ret) {
        name.push_str("_r_i32_i32");
        decl.push_str(" (result i32 i32)");
    } else if !matches!(ret, Ty::Unit) {
        let r = wasm_ty(ret, ctx);
        name.push_str("_r_");
        name.push_str(r);
        decl.push_str(&format!(" (result {r})"));
    }
    decl.push(')');
    (name, decl)
}

/// Register a `call_indirect` signature and return its `(type $name)` clause.
fn register_indirect_sig(params: &[Ty], ret: &Ty, ctx: &Ctx) -> String {
    let (name, decl) = indirect_sig(params, ret, ctx);
    ctx.indirect_sigs
        .borrow_mut()
        .insert(name.clone(), decl.clone());
    name
}

/// Types a closure environment slot can hold (#2118).
///
/// Scoped to plain scalars deliberately: a heap-owning capture (`String`,
/// `List[T]`, `Map[K,V]`, `Set[T]`, a struct with heap fields) would need to
/// be deep-cloned into the environment — sharing the raw pointer risks a
/// double free the moment both the outer function's copy and the lambda's
/// loaded-from-env copy run through `emit_fn_heap_drops` for the same heap
/// block. Every capture in the issue's own motivating examples (`b: Byte`,
/// `n: Int` in `examples/bzip`) is a plain scalar; deep-clone-on-capture for
/// heap types is real design work, not this fix's scope. A capture outside
/// this set is simply left out of the returned list, so it stays absent
/// from the lambda's `declared` set and the existing `undeclared_local_ref`
/// stub path (unchanged) catches it exactly as it does today.
///
/// `UByte` is deliberately excluded even though it's a plain scalar: neither
/// `wasm_ty` nor `is_i32` has a dedicated arm for it (both silently fall
/// through to a default), a pre-existing gap this fix doesn't try to paper
/// over — safer to leave a `UByte` capture stubbed than guess its width.
fn is_capturable_scalar(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int | Ty::UInt | Ty::Float | Ty::Bool | Ty::Byte | Ty::Char
    )
}

/// Free variables in `body` that resolve to a name declared in the
/// *enclosing* function's params/locals (per `ctx.fn_params`/`ctx.fn_locals`
/// — this is called from `register_lambda`, while `ctx` is still the
/// *caller's* context, before a lambda's own `Ctx` is derived) rather than
/// the lambda's own params. `param_names` excludes the lambda's own
/// parameters (and, recursively, any nested lambda's own parameters) so a
/// shadowing inner name is never mistaken for a capture.
///
/// Structural (TIR-level) rather than the text-scanning
/// `undeclared_local_ref` used elsewhere in this file, because here the
/// *type* of each free variable is needed (to size/lay out the
/// environment and to decide `is_capturable_scalar`) and the caller's
/// `ctx.fn_locals`/`ctx.fn_params` are only available at this call site —
/// `emit_one_lambda_fn` runs later, once module-wide, long after the
/// caller's own `Ctx` is gone. Mirrors the LLVM backend's
/// `collect_lambda_captures_tir` (`emit_closures_tir.rs`) structurally;
/// adapted to this file's `Vec<(String, Ty)>` registries instead of LLVM's
/// `HashMap` `fn_ctx.locals`/`ref_locals`.
fn collect_lambda_captures(
    body: &TirExpr,
    param_names: &std::collections::HashSet<String>,
    ctx: &Ctx,
) -> Vec<(String, Ty)> {
    let mut seen = std::collections::HashSet::new();
    let mut caps = Vec::new();
    walk_expr_for_captures(body, param_names, ctx, &mut seen, &mut caps);
    caps
}

fn capture_var_if_outer_local(
    name: &str,
    exclude: &std::collections::HashSet<String>,
    ctx: &Ctx,
    seen: &mut std::collections::HashSet<String>,
    caps: &mut Vec<(String, Ty)>,
) {
    if exclude.contains(name) || seen.contains(name) {
        return;
    }
    seen.insert(name.to_string());
    let ty = ctx
        .fn_params
        .borrow()
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t.clone())
        .or_else(|| {
            ctx.fn_locals
                .borrow()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
        });
    if let Some(ty) = ty {
        if is_capturable_scalar(&resolve_ty_param(&ty, ctx.type_subst)) {
            caps.push((name.to_string(), ty));
        }
    }
}

fn walk_expr_for_captures(
    expr: &TirExpr,
    exclude: &std::collections::HashSet<String>,
    ctx: &Ctx,
    seen: &mut std::collections::HashSet<String>,
    caps: &mut Vec<(String, Ty)>,
) {
    match &expr.kind {
        TirExprKind::Var(name) => capture_var_if_outer_local(name, exclude, ctx, seen, caps),
        TirExprKind::Lambda { params, body } => {
            let mut inner_excl = exclude.clone();
            for p in params {
                inner_excl.insert(p.name.clone());
            }
            walk_expr_for_captures(body, &inner_excl, ctx, seen, caps);
        }
        TirExprKind::Binary { left, right, .. } => {
            walk_expr_for_captures(left, exclude, ctx, seen, caps);
            walk_expr_for_captures(right, exclude, ctx, seen, caps);
        }
        TirExprKind::Unary { expr, .. } => walk_expr_for_captures(expr, exclude, ctx, seen, caps),
        TirExprKind::FnCall { name, args, .. } => {
            capture_var_if_outer_local(name, exclude, ctx, seen, caps);
            for a in args {
                walk_expr_for_captures(a, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::MethodCall { receiver, args, .. } => {
            walk_expr_for_captures(receiver, exclude, ctx, seen, caps);
            for a in args {
                walk_expr_for_captures(a, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::FieldAccess { expr, .. } => {
            walk_expr_for_captures(expr, exclude, ctx, seen, caps)
        }
        TirExprKind::If { cond, then, else_ } => {
            walk_expr_for_captures(cond, exclude, ctx, seen, caps);
            walk_block_for_captures(then, exclude, ctx, seen, caps);
            if let Some(e) = else_ {
                walk_expr_for_captures(e, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Block(b) => walk_block_for_captures(b, exclude, ctx, seen, caps),
        TirExprKind::Construct { fields, .. } => {
            for (_, v) in fields {
                walk_expr_for_captures(v, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Match { scrutinee, arms } => {
            walk_expr_for_captures(scrutinee, exclude, ctx, seen, caps);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => walk_expr_for_captures(e, exclude, ctx, seen, caps),
                    TirMatchBody::Block(b) => walk_block_for_captures(b, exclude, ctx, seen, caps),
                }
            }
        }
        TirExprKind::Consume(inner)
        | TirExprKind::Propagate(inner)
        | TirExprKind::Borrow { expr: inner, .. } => {
            walk_expr_for_captures(inner, exclude, ctx, seen, caps);
        }
        TirExprKind::Relabel { expr, .. } => walk_expr_for_captures(expr, exclude, ctx, seen, caps),
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                walk_expr_for_captures(e, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                walk_expr_for_captures(k, exclude, ctx, seen, caps);
                walk_expr_for_captures(v, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Spawn { fields, .. } => {
            for (_, v) in fields {
                walk_expr_for_captures(v, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Select { arms } => {
            for arm in arms {
                walk_expr_for_captures(&arm.expr, exclude, ctx, seen, caps);
                walk_block_for_captures(&arm.body, exclude, ctx, seen, caps);
            }
        }
        TirExprKind::Literal(_) | TirExprKind::Quantifier(_) => {}
    }
}

fn walk_block_for_captures(
    block: &TirBlock,
    exclude: &std::collections::HashSet<String>,
    ctx: &Ctx,
    seen: &mut std::collections::HashSet<String>,
    caps: &mut Vec<(String, Ty)>,
) {
    for stmt in &block.stmts {
        match stmt {
            TirStmt::Expr { expr, .. } => walk_expr_for_captures(expr, exclude, ctx, seen, caps),
            TirStmt::Let { init, .. } => walk_expr_for_captures(init, exclude, ctx, seen, caps),
            TirStmt::Assign { value, .. } => {
                walk_expr_for_captures(value, exclude, ctx, seen, caps)
            }
            TirStmt::Return { value: Some(e), .. } => {
                walk_expr_for_captures(e, exclude, ctx, seen, caps)
            }
            TirStmt::Return { value: None, .. } => {}
            TirStmt::While { cond, body, .. } => {
                walk_expr_for_captures(cond, exclude, ctx, seen, caps);
                walk_block_for_captures(body, exclude, ctx, seen, caps);
            }
            TirStmt::For { iter, body, .. } => {
                walk_expr_for_captures(iter, exclude, ctx, seen, caps);
                walk_block_for_captures(body, exclude, ctx, seen, caps);
            }
            TirStmt::If {
                cond, then, else_, ..
            } => {
                walk_expr_for_captures(cond, exclude, ctx, seen, caps);
                walk_block_for_captures(then, exclude, ctx, seen, caps);
                match else_ {
                    Some(TirElseBranch::Block(b)) => {
                        walk_block_for_captures(b, exclude, ctx, seen, caps)
                    }
                    Some(TirElseBranch::If(s)) => {
                        let tmp_block = TirBlock {
                            stmts: vec![(**s).clone()],
                            span: Span::default(),
                        };
                        walk_block_for_captures(&tmp_block, exclude, ctx, seen, caps);
                    }
                    None => {}
                }
            }
            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                walk_expr_for_captures(scrutinee, exclude, ctx, seen, caps);
                for arm in arms {
                    match &arm.body {
                        TirMatchBody::Expr(e) => {
                            walk_expr_for_captures(e, exclude, ctx, seen, caps)
                        }
                        TirMatchBody::Block(b) => {
                            walk_block_for_captures(b, exclude, ctx, seen, caps)
                        }
                    }
                }
            }
        }
    }
}

/// A bare reference to a named top-level function used as a value —
/// `apply(double, 3)` (#2159, follow-up to #2014/#2118).
///
/// Only lambda literals get a table slot from `register_lambda`; a named
/// function has none. Rather than a second, parallel table-registration
/// path, synthesize a thin non-capturing wrapper lambda —
/// `|p0: T0, p1: T1, ...| -> R { name(p0, p1, ...) }` — built directly from
/// `expr.ty`'s already-resolved `Ty::Fn(params, ret, ..)` shape (the
/// checker already typed this `Var` reference that way; no separate
/// top-level signature registry is needed), and feed it through the exact
/// same `emit_closure_value` a real lambda literal uses. Mirrors the LLVM
/// backend's `make_named_fn_closure_hof` (`emit_helpers.rs`), which
/// synthesizes the same kind of trampoline function.
fn emit_named_fn_as_value(out: &mut String, expr: &TirExpr, name: &str, ctx: &Ctx) {
    let (param_tys, ret_ty) = match resolve_ty_param(&expr.ty, ctx.type_subst) {
        Ty::Fn(params, ret, ..) => (params, *ret),
        other => {
            out.push_str(&format!(
                "    ;; unsupported: `{name}` used as a value has non-fn type {other:?}\n"
            ));
            return;
        }
    };
    let params: Vec<TirParam> = param_tys
        .iter()
        .enumerate()
        .map(|(i, ty)| TirParam {
            name: format!("__wp{i}"),
            ty: ty.clone(),
            capability: None,
            span: expr.span,
        })
        .collect();
    let args: Vec<TirExpr> = params
        .iter()
        .map(|p| TirExpr {
            kind: TirExprKind::Var(p.name.clone()),
            ty: p.ty.clone(),
            span: expr.span,
        })
        .collect();
    let body = TirExpr {
        kind: TirExprKind::FnCall {
            name: name.to_string(),
            args,
            type_args: Vec::new(),
        },
        ty: ret_ty,
        span: expr.span,
    };
    emit_closure_value(out, expr, &params, &body, ctx);
}

/// Emit a lambda literal as a runtime *value*: a pointer to a heap-boxed
/// `{funcidx: i32, envptr: i32}` pair (#2118).
///
/// Boxed unconditionally, even when `captures` is empty (env = 0/null): a
/// generic HOF body like `List[T]::map`'s is compiled once per element-type
/// instantiation and `call_indirect`s through an abstract `f` parameter that
/// different call sites can bind to either a capturing or a non-capturing
/// lambda — the representation has to be uniform across every value of a
/// given `Ty::Fn` shape, not just within one call site. Mirrors the LLVM
/// backend's `%__closure_type` struct (`emit_closures_tir.rs`), and inherits
/// its documented tradeoff: `$mvl_alloc` never frees, so a closure literal
/// evaluated in a loop leaks one 8-byte block (env) plus one 8-byte block
/// (the box) per iteration.
fn emit_closure_value(
    out: &mut String,
    expr: &TirExpr,
    params: &[TirParam],
    body: &TirExpr,
    ctx: &Ctx,
) {
    let idx = register_lambda(expr, params, body, ctx);
    let captures = ctx.lambdas.borrow()[idx as usize].captures.clone();
    let off = expr.span.offset;
    let env_local = format!("__env_{off}");
    let closure_local = format!("__closure_{off}");

    if captures.is_empty() {
        out.push_str("    i32.const 0\n");
        out.push_str(&format!("    local.set ${env_local}\n"));
    } else {
        let env_size = captures.len() * 8;
        out.push_str(&format!("    i32.const {env_size}\n"));
        out.push_str("    call $mvl_alloc\n");
        out.push_str(&format!("    local.set ${env_local}\n"));
        for (i, (name, ty)) in captures.iter().enumerate() {
            let field_off = capture_env_offset(i);
            out.push_str(&format!("    local.get ${env_local}\n"));
            out.push_str(&format!("    local.get ${name}\n"));
            let store_op = if matches!(ty, Ty::Float) {
                "f64.store"
            } else if is_i32(ty, ctx) {
                "i32.store"
            } else {
                "i64.store"
            };
            if field_off == 0 {
                out.push_str(&format!("    {store_op}\n"));
            } else {
                out.push_str(&format!("    {store_op} offset={field_off}\n"));
            }
        }
    }

    out.push_str("    i32.const 8\n");
    out.push_str("    call $mvl_alloc\n");
    out.push_str(&format!("    local.set ${closure_local}\n"));
    out.push_str(&format!("    local.get ${closure_local}\n"));
    out.push_str(&format!("    i32.const {idx}\n"));
    out.push_str("    i32.store\n");
    out.push_str(&format!("    local.get ${closure_local}\n"));
    out.push_str(&format!("    local.get ${env_local}\n"));
    out.push_str("    i32.store offset=4\n");
    out.push_str(&format!("    local.get ${closure_local}\n"));
}

/// Assign (or reuse) a table slot for a lambda literal, returning its index.
///
/// Keyed on the source span *and* the enclosing type substitution. The span
/// alone is not identifying: one lambda literal inside a generic body is
/// compiled once per instantiation, and each copy has a different signature.
/// Keying on the span alone handed the `T → String` instantiation the slot and
/// the compiled body belonging to `T → Int`, so `List_filter__Str` called the
/// i64 lambda through `call_indirect` and trapped on the dynamic type check.
/// Where two concrete types happen to lower to the same WASM width it was worse
/// than a trap — the wrong specialization ran silently (#2014).
fn register_lambda(expr: &TirExpr, params: &[TirParam], body: &TirExpr, ctx: &Ctx) -> u32 {
    // Canonical, order-independent tag for the substitution.
    let subst_tag = {
        let mut pairs: Vec<String> = ctx
            .type_subst
            .iter()
            .map(|(k, v)| format!("{k}={}", mangle_ty_tag(v)))
            .collect();
        pairs.sort();
        pairs.join(",")
    };
    let key = (expr.span.offset, expr.span.len, subst_tag.clone());
    if let Some(idx) = ctx.lambda_slots.borrow().get(&key) {
        return *idx;
    }
    // Capture analysis needs the *caller's* `ctx.fn_locals`/`ctx.fn_params`
    // (#2118) — done here, before the lambda's own slot/body are set up,
    // because `emit_lambda_fns` drains `ctx.lambdas` in a later, module-wide
    // pass where the calling function's `Ctx` no longer exists.
    let param_names: std::collections::HashSet<String> =
        params.iter().map(|p| p.name.clone()).collect();
    let captures = collect_lambda_captures(body, &param_names, ctx);
    let mut lambdas = ctx.lambdas.borrow_mut();
    let idx = lambdas.len() as u32;
    // The emitted function name must be per-instantiation too, or two table
    // entries would share one symbol.
    let suffix = if subst_tag.is_empty() {
        String::new()
    } else {
        format!("__{}", mangle_ident(&subst_tag))
    };
    lambdas.push(LambdaEntry {
        wasm_name: format!("__lambda_{}_{}{}", key.0, key.1, suffix),
        params: params.to_vec(),
        body: body.clone(),
        ret_ty: body.ty.clone(),
        type_subst: ctx.type_subst.clone(),
        captures,
    });
    ctx.lambda_slots.borrow_mut().insert(key, idx);
    idx
}

/// Sanitize a substitution tag into a WAT-identifier-safe suffix.
fn mangle_ident(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// The resolved `Ty::Fn` behind a name, when that name is a function-typed
/// parameter or local rather than a top-level function.
///
/// This is what distinguishes `f(x)` inside `List[T]::map` — an indirect call
/// through the `f` parameter — from an ordinary `call $some_fn`.
fn fn_value_ty(name: &str, ctx: &Ctx) -> Option<(Vec<Ty>, Ty)> {
    let declared = {
        let params = ctx.fn_params.borrow();
        match params.iter().find(|(n, _)| n == name) {
            Some((_, ty)) => ty.clone(),
            None => {
                let locals = ctx.fn_locals.borrow();
                locals
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, t)| t.clone())?
            }
        }
    };
    let resolved = resolve_ty_param(&declared, ctx.type_subst);
    match resolved {
        Ty::Fn(params, ret, ..) => Some((params, *ret)),
        _ => None,
    }
}

/// Emit every registered lambda as a top-level function.
///
/// Drains the registry in a loop because emitting one lambda body can register
/// another (a lambda nested inside a lambda), which would otherwise be given a
/// table slot but never a body — a validation failure, not a silent one.
fn emit_lambda_fns(out: &mut String, ctx: &Ctx) {
    let mut emitted = 0usize;
    loop {
        // Cloned out of the RefCell before emitting: `emit_one_lambda_fn` can
        // register a nested lambda, which needs a mutable borrow.
        let batch: Vec<LambdaEntry> = {
            let lambdas = ctx.lambdas.borrow();
            if emitted >= lambdas.len() {
                break;
            }
            lambdas[emitted..].to_vec()
        };
        emitted += batch.len();
        for l in &batch {
            emit_one_lambda_fn(
                out,
                &l.wasm_name,
                &l.params,
                &l.body,
                &l.ret_ty,
                &l.type_subst,
                &l.captures,
                ctx,
            );
        }
    }
}

/// Byte offset of capture `i` inside its lambda's heap environment. Every
/// slot is 8 bytes regardless of the capture's actual width (i32 wastes 4
/// bytes of padding) — simpler than variable-width packing, and captured
/// environments are tiny (#2118).
fn capture_env_offset(i: usize) -> usize {
    i * 8
}

#[allow(clippy::too_many_arguments)]
fn emit_one_lambda_fn(
    out: &mut String,
    wasm_name: &str,
    params: &[TirParam],
    body: &TirExpr,
    ret_ty: &Ty,
    type_subst: &HashMap<String, Ty>,
    captures: &[(String, Ty)],
    ctx: &Ctx,
) {
    let lam_ctx = derived_ctx(ctx, type_subst);

    let ret = resolve_ty_param(ret_ty, type_subst);
    // Every lambda takes an environment pointer as its first param, whether
    // or not it captures anything — the calling convention has to be
    // uniform across a `Ty::Fn` shape, since `call_indirect` at a generic
    // HOF call site (e.g. `List[T]::map`'s body) can't know ahead of time
    // whether the specific closure value it's about to invoke captures
    // anything (#2118). A non-capturing lambda just never reads `$__env`.
    out.push_str(&format!("  (func ${wasm_name} (param $__env i32)"));
    {
        let mut sp = lam_ctx.string_params.borrow_mut();
        for p in params {
            let concrete = resolve_ty_param(&p.ty, type_subst);
            if peels_to_string(&concrete) {
                sp.insert(p.name.clone());
                out.push_str(&format!(
                    " (param ${}_ptr i32) (param ${}_len i32)",
                    p.name, p.name
                ));
            } else {
                out.push_str(&format!(
                    " (param ${} {})",
                    p.name,
                    wasm_ty(&concrete, &lam_ctx)
                ));
            }
        }
    }
    if peels_to_string(&ret) {
        out.push_str(" (result i32 i32)");
    } else if !matches!(ret, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&ret, &lam_ctx)));
    }
    out.push('\n');

    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_expr(body, &mut locals);
    dedup_locals_keep_last(&mut locals);
    // Captures become ordinary locals too — loaded from `$__env` once at
    // function entry (below), then indistinguishable from any other local
    // for the rest of the body, including `emit_fn_heap_drops` at function
    // exit (a no-op for these, since captures are restricted to scalars
    // `local_drop_fn` never matches). Prepended so a capture can never
    // collide with a `collect_locals_expr` temp keyed off the same span.
    let mut all_locals: Vec<(String, Ty)> = captures.to_vec();
    all_locals.extend(locals.iter().cloned());
    dedup_locals_keep_last(&mut all_locals);
    for (name, ty) in &all_locals {
        let concrete = resolve_ty_param(ty, type_subst);
        if peels_to_string(&concrete) {
            out.push_str(&format!("    (local ${name}_ptr i32)\n"));
            out.push_str(&format!("    (local ${name}_len i32)\n"));
        } else {
            out.push_str(&format!(
                "    (local ${name} {})\n",
                wasm_ty(&concrete, &lam_ctx)
            ));
        }
    }
    *lam_ctx.fn_locals.borrow_mut() = all_locals.clone();
    *lam_ctx.fn_params.borrow_mut() = fn_scope_params(params);

    let mut body_buf = String::new();
    // Load every capture out of the environment before the real body runs.
    // Captures are restricted to `is_capturable_scalar` types, so this is
    // always a single scalar load — no ptr/len pairs to unpack.
    for (i, (name, ty)) in captures.iter().enumerate() {
        let concrete = resolve_ty_param(ty, type_subst);
        let off = capture_env_offset(i);
        let load_op = if matches!(concrete, Ty::Float) {
            "f64.load"
        } else if is_i32(&concrete, &lam_ctx) {
            "i32.load"
        } else {
            "i64.load"
        };
        body_buf.push_str("    local.get $__env\n");
        if off == 0 {
            body_buf.push_str(&format!("    {load_op}\n"));
        } else {
            body_buf.push_str(&format!("    {load_op} offset={off}\n"));
        }
        body_buf.push_str(&format!("    local.set ${name}\n"));
    }
    emit_expr(&mut body_buf, body, &lam_ctx);
    // A capturing lambda reads a name it neither declares nor takes as a
    // parameter. Captures recognized by `collect_lambda_captures` are now
    // declared locals (above) and so never trip this; it remains as a
    // defense-in-depth catch-all for anything that check doesn't recognize
    // (a heap-owned capture, deliberately excluded — see
    // `is_capturable_scalar` — or any node kind the structural walk
    // misses), so an unsupported case still stubs loudly instead of
    // emitting a function that references an undefined local — `wasm-tools`
    // would otherwise reject the *whole* module with "unknown local: failed
    // to find name `$k`", taking every unrelated function down with it
    // (#2014, #2118).
    let declared: std::collections::HashSet<&str> = all_locals
        .iter()
        .map(|(n, _)| n.as_str())
        .chain(params.iter().map(|p| p.name.as_str()))
        .chain(std::iter::once("__env"))
        .collect();
    if let Some(captured) = undeclared_local_ref(&body_buf, &declared) {
        out.push_str(&format!(
            "    ;; unsupported: lambda captures `{captured}` — only scalar captures are supported (#2118)\n"
        ));
        emit_stub_body(out, wasm_name, ctx);
    } else if body_buf.contains(UNSUPPORTED_MARKER) {
        emit_stub_body(out, wasm_name, ctx);
    } else {
        out.push_str(&body_buf);
    }
    out.push_str("  )\n");

    if lam_ctx.needs_runtime.get() {
        ctx.needs_runtime.set(true);
    }
}

/// The type to unify a call argument against, repairing a lambda's unresolved
/// return type.
///
/// A lambda literal reaches TIR with `ty = Fn([Int], Unknown)` — the checker
/// records the annotated parameter types but leaves the result `Unknown`. That
/// is fatal for inferring the `U` of `List[T]::map[U](self, f: fn(T) -> U)`:
/// unifying against `Unknown` binds `U → Unknown` and the instance mangles to
/// `List_map__Int__Unknown`, whose body types every `f(x)` result as the
/// `wasm_ty` i64 default. The enclosing `let`'s annotation would resolve it,
/// but the call expression's own type is the *unresolved* `List[U]`, so it is
/// no help either.
///
/// The lambda's body does carry a real type, so rebuild the function type from
/// `params` + `body.ty` instead. Non-lambda arguments pass through untouched.
fn effective_arg_ty(arg: &TirExpr) -> Ty {
    if let TirExprKind::Lambda { params, body } = &arg.kind {
        let (effects, totality) = match &arg.ty {
            Ty::Fn(_, _, e, t) => (e.clone(), t.clone()),
            _ => (Vec::new(), None),
        };
        return Ty::Fn(
            params.iter().map(|p| p.ty.clone()).collect(),
            Box::new(body.ty.clone()),
            effects,
            totality,
        );
    }
    arg.ty.clone()
}

/// True when the `MethodCall` dispatch chain in `emit_expr` already lowers
/// `(receiver, method)` itself, so a generic `std/*.mvl` body must not be
/// monomorphized for it.
///
/// The chain checks its builtin arms *before* the generic-method arm, so
/// without this filter the two halves disagree: emission would use the native
/// arm while collection still instantiated the std body, leaving a dead
/// monomorphized function in the module. That is not merely wasteful — the
/// dead body can be *invalid*, and a WASM module is rejected as a whole. The
/// first version of #2014 emitted a dead `Option_unwrap_or__Str` whose body
/// failed validation with "expected i32, found i64", which broke
/// `parse_test.mvl` even though nothing ever called it.
///
/// Grouped by receiver shape rather than method name alone, because the same
/// name can be native on one receiver and pure MVL on another: `concat` is a
/// runtime call on `String` but a `std/lists.mvl` body on `List`.
///
/// Keep in sync with the guards in `emit_expr`'s `MethodCall` arms. A name
/// added there but missed here yields a dead instantiation; the reverse
/// silently drops a method back to `;; unsupported`.
///
/// `pub` because `cli::wasm_text`'s prelude pull-in loop needs the same answer:
/// lowering a std body the emitter never calls emits a dead function, and for
/// e.g. `String::contains`/`String::trim` that dead body itself stubs to
/// `unreachable` — noise that reads like missing support in a `.wat` dump.
pub fn emitter_handles_method_natively(receiver_ty: &Ty, method: &str) -> bool {
    if peels_to_string(receiver_ty) {
        return matches!(
            method,
            "len"
                | "is_empty"
                | "contains"
                | "starts_with"
                | "ends_with"
                | "find"
                | "concat"
                | "substring"
                | "to_upper"
                | "to_lower"
                | "trim"
                | "replace"
                | "split"
                | "parse_int"
        );
    }
    if matches!(receiver_ty, Ty::Float) && method == "to_string" {
        return true;
    }
    // Int/UInt/Float::abs/clamp/pow (#2122).
    if matches!(receiver_ty, Ty::Int | Ty::UInt | Ty::Float)
        && matches!(method, "abs" | "clamp" | "pow")
    {
        return true;
    }
    if option_inner_ty(receiver_ty).is_some() || result_ok_ty(receiver_ty).is_some() {
        return method == "unwrap_or";
    }
    if map_key_val_ty(receiver_ty).is_some() {
        return matches!(
            method,
            "len" | "is_empty" | "get" | "insert" | "contains_key"
        );
    }
    // `Set[T]::map` gets a dedicated native arm (#2124) — unlike
    // `List[T]::map`, which stays on the generic-dispatch path below to
    // monomorphize the real `std/lists.mvl` body. Checked ahead of the
    // shared `collection_elem_ty` branch so it doesn't also claim
    // `List`/`Array` receivers, which share that branch's shape check but
    // must keep routing `map` through `std/lists.mvl`.
    if peels_to_set(receiver_ty) && method == "map" {
        return true;
    }
    if collection_elem_ty(receiver_ty).is_some() {
        // `clone`, `slice`, and `concat` have native arms too. All three are
        // conditional — `clone_is_supported` / `slice_is_supported` /
        // `concat_is_supported` gate them on the element type — so this
        // answers for the shape and the emitter's own guard has the final
        // say. Listing them keeps this function honest about which methods
        // have arms at all; omitting them was harmless only because
        // `slice`/`concat` are `builtin` in std/lists.mvl (no body to
        // instantiate) and `clone` has no std declaration, i.e. by luck
        // rather than by design.
        return matches!(
            method,
            "len"
                | "is_empty"
                | "get"
                | "push"
                | "contains"
                | "insert"
                | "remove"
                | "clone"
                | "slice"
                | "concat"
        );
    }
    false
}

/// An expression's *resolved* type, seeing through chained generic method
/// calls.
///
/// A `MethodCall`'s own `ty` is its callee's **declared** return type, still
/// written in the callee's type params — `xs.map(f)` reports `List[U]`, not
/// `List[Int]`. Reading it directly breaks chains: in
/// `xs.filter(..).map(..).fold(0, ..)` the `fold` receiver reports an
/// unresolved `List[U]`, which unified `T → Unknown` and produced a bogus
/// `List_fold__Unknown__Int` alongside the real instance.
///
/// Recomputing the callee's substitution here yields the concrete type instead.
/// Mutually recursive with `resolve_generic_method_call`, bounded by expression
/// depth.
fn effective_expr_ty(
    expr: &TirExpr,
    methods: &HashMap<(String, String), &TirFn>,
    outer: &HashMap<String, Ty>,
) -> Ty {
    if let TirExprKind::MethodCall {
        receiver,
        method,
        args,
    } = &expr.kind
    {
        if let Some((gm, subst, _)) =
            resolve_generic_method_call(receiver, method, args, methods, outer)
        {
            return resolve_ty_param(&gm.ret_ty, &subst);
        }
    }
    expr.ty.clone()
}

/// Resolve a method call against the generic extension methods in scope,
/// returning the callee, its full type substitution, and the mangled name.
///
/// **Both instantiation collection and code emission must call this**, or they
/// disagree about the callee's name and the module references a symbol that was
/// never emitted. That is the entire reason this is a shared function rather
/// than the obvious two-lines-each at both sites.
///
/// `outer` is the substitution of the enclosing instantiation (empty at the top
/// level): a call inside `List_first__Int`'s body still describes its receiver
/// as `List[T]`, so the actual types are resolved through `outer` before
/// unification. Returns `None` unless every type param got bound — a partial
/// substitution would mangle to a name like `List_map__T__Int`.
fn resolve_generic_method_call<'a>(
    receiver: &TirExpr,
    method: &str,
    args: &[TirExpr],
    methods: &HashMap<(String, String), &'a TirFn>,
    outer: &HashMap<String, Ty>,
) -> Option<(&'a TirFn, HashMap<String, Ty>, String)> {
    // Chained calls need the receiver's *resolved* type, not its declared one.
    let recv_ty = effective_expr_ty(receiver, methods, outer);
    if emitter_handles_method_natively(&recv_ty, method) {
        return None;
    }
    let recv_name = receiver_type_name(&recv_ty)?;
    let gm = *methods.get(&(recv_name.clone(), method.to_string()))?;

    // `gm.type_params` only names generics the method itself introduces
    // (`U` in `Result[T, E]::map[U]`) — not the receiver's own `T`/`E`.
    // Methods that never reconstruct a payload-carrying value from a
    // receiver-only param (`is_ok`, `unwrap_or`) don't need those bound, so
    // this went unnoticed. `Result::map`/`Result::and_then`'s `Err(e) =>
    // Err(e)` arm does — it needs `E`'s concrete runtime shape to pick the
    // right constructor — and silently picked the i64 variant for any
    // enum/scalar `E` that never got bound, corrupting the payload (#2149).
    // Extending the bindable name set with every bare `Ty::Named` leaf in
    // the *declared* self type (always genuine placeholders there — a
    // stdlib/user generic method never hard-codes a concrete type in place
    // of its own declared receiver type param) covers `T`/`E` the same way
    // for any two-type-param receiver (`Map[K, V]` included).
    let mut param_names: std::collections::HashSet<String> = gm
        .type_params
        .iter()
        .map(|gp| gp.name().to_string())
        .collect();
    if let Some(self_param) = gm.params.first() {
        collect_named_leaves(&self_param.ty, &mut param_names);
    }
    let mut subst = HashMap::new();

    // `self` is params[0] (the parser synthesises it); the remaining formals
    // line up with the call's arguments.
    let mut formals = gm.params.iter();
    if let Some(self_param) = formals.next() {
        let actual = resolve_ty_param(&recv_ty, outer);
        unify_ty_params(&self_param.ty, &actual, &param_names, &mut subst);
    }
    for (formal, arg) in formals.zip(args.iter()) {
        let actual = resolve_ty_param(&effective_arg_ty(arg), outer);
        unify_ty_params(&formal.ty, &actual, &param_names, &mut subst);
    }

    if !gm
        .type_params
        .iter()
        .all(|gp| subst.contains_key(gp.name()))
    {
        return None;
    }
    let mangled = mangle_generic_method_name(&recv_name, &gm.name, &gm.type_params, &subst);
    Some((gm, subst, mangled))
}

/// The two kinds of generic callee a body can reference, in one place so the
/// recursive walkers below take a single parameter instead of two parallel
/// maps.
///
/// `fns` is keyed by name; `methods` by `(receiver_type, method_name)` because
/// the method name alone is ambiguous — `List[T]::first` and a hypothetical
/// `Set[T]::first` are different functions (#2014).
struct GenericCallees<'a> {
    fns: HashMap<&'a str, &'a TirFn>,
    methods: HashMap<(String, String), &'a TirFn>,
}

impl GenericCallees<'_> {
    fn is_empty(&self) -> bool {
        self.fns.is_empty() && self.methods.is_empty()
    }
}

/// Scan all non-generic function bodies for calls to generic functions and to
/// generic extension methods.
/// Returns unique (generic_fn_ref, type_subst, mangled_name) triples.
fn collect_generic_instantiations<'a>(
    fns: &[&'a TirFn],
    all_fns: &[&'a TirFn],
    ext_methods: &[&'a TirFn],
    generic_ext_methods: &[&'a TirFn],
    actors: &[TirActorDecl],
    _ctx: &Ctx,
) -> Vec<(&'a TirFn, HashMap<String, Ty>, String)> {
    // Build lookup: fn_name → TirFn for generic fns
    let callees = GenericCallees {
        fns: all_fns
            .iter()
            .filter(|f| !f.type_params.is_empty())
            .map(|f| (f.name.as_str(), *f))
            .collect(),
        methods: generic_ext_methods
            .iter()
            .map(|f| {
                (
                    (
                        f.receiver_type.clone().expect("filtered by caller"),
                        f.name.clone(),
                    ),
                    *f,
                )
            })
            .collect(),
    };

    if callees.is_empty() {
        return vec![];
    }

    let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut result = vec![];
    let top: HashMap<String, Ty> = HashMap::new();

    for f in fns {
        collect_instantiations_in_block(&f.body, &callees, &top, &mut seen, &mut result);
    }
    // Non-generic extension methods are emitted directly by
    // `emit_extension_method`, through the same `emit_expr` arms that mangle a
    // generic callee's name — so `fn Widget::doubled(self) { self.items.map(f) }`
    // emitted `call $List_map__Int__Int` while collection, seeded only from
    // `fns`, never discovered it. The module then failed to link on a symbol
    // nobody emitted (#2014). Before generic ext methods were emittable at all
    // this stubbed loudly instead; making them emittable without seeding their
    // callers turned a stub into an invalid module.
    for f in ext_methods {
        collect_instantiations_in_block(&f.body, &callees, &top, &mut seen, &mut result);
    }
    // Actor method bodies are emitted as functions but are not in `tir.fns`, so
    // a generic called only from a behaviour would never be instantiated and the
    // module referenced a symbol that was never emitted (#2012). Same gap the
    // literal walker had.
    for ad in actors {
        for m in &ad.methods {
            collect_instantiations_in_block(&m.body, &callees, &top, &mut seen, &mut result);
        }
    }
    // A generic extension method's own body may call another one — `first`/`last`
    // are `self.get(..)`, `take`/`skip` are `self.slice(..)`, `rev` is
    // `self.reverse()`. Walking only user code would emit `List_rev__Int` with a
    // call to a `List_reverse__Int` that was never emitted, so the module fails
    // to link. Iterate to a fixpoint: each pass may discover callees one level
    // deeper, and `seen` keeps it terminating.
    //
    // Each instance is scanned under *its own* substitution, not `top` — the
    // body of `List_first__Int` still says `self.get(0)` on a `List[T]`, so
    // scanning it with an empty `outer` would look for `List_get__T`.
    let mut scanned = 0;
    while scanned < result.len() {
        let batch: Vec<(&TirFn, HashMap<String, Ty>)> = result[scanned..]
            .iter()
            .map(|(f, s, _)| (*f, s.clone()))
            .collect();
        scanned = result.len();
        for (f, subst) in batch {
            collect_instantiations_in_block(&f.body, &callees, &subst, &mut seen, &mut result);
        }
    }
    result
}

fn collect_instantiations_in_block<'a>(
    block: &TirBlock,
    callees: &GenericCallees<'a>,
    outer: &HashMap<String, Ty>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    for stmt in &block.stmts {
        collect_instantiations_in_stmt(stmt, callees, outer, seen, result);
    }
}

fn collect_instantiations_in_stmt<'a>(
    stmt: &TirStmt,
    callees: &GenericCallees<'a>,
    outer: &HashMap<String, Ty>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    match stmt {
        TirStmt::Expr { expr, .. }
        | TirStmt::Return {
            value: Some(expr), ..
        } => {
            collect_instantiations_in_expr(expr, callees, outer, seen, result);
        }
        TirStmt::Let { init, .. } | TirStmt::Assign { value: init, .. } => {
            collect_instantiations_in_expr(init, callees, outer, seen, result);
        }
        TirStmt::If {
            cond, then, else_, ..
        } => {
            collect_instantiations_in_expr(cond, callees, outer, seen, result);
            collect_instantiations_in_block(then, callees, outer, seen, result);
            match else_ {
                Some(TirElseBranch::Block(b)) => {
                    collect_instantiations_in_block(b, callees, outer, seen, result);
                }
                Some(TirElseBranch::If(s)) => {
                    collect_instantiations_in_stmt(s, callees, outer, seen, result);
                }
                None => {}
            }
        }
        TirStmt::While { cond, body, .. }
        | TirStmt::For {
            iter: cond, body, ..
        } => {
            collect_instantiations_in_expr(cond, callees, outer, seen, result);
            collect_instantiations_in_block(body, callees, outer, seen, result);
        }
        TirStmt::Match {
            scrutinee, arms, ..
        } => {
            collect_instantiations_in_expr(scrutinee, callees, outer, seen, result);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => {
                        collect_instantiations_in_expr(e, callees, outer, seen, result);
                    }
                    TirMatchBody::Block(b) => {
                        collect_instantiations_in_block(b, callees, outer, seen, result);
                    }
                }
            }
        }
        _ => {}
    }
}

fn collect_instantiations_in_expr<'a>(
    expr: &TirExpr,
    callees: &GenericCallees<'a>,
    outer: &HashMap<String, Ty>,
    seen: &mut std::collections::HashMap<String, ()>,
    result: &mut Vec<(&'a TirFn, HashMap<String, Ty>, String)>,
) {
    if let TirExprKind::FnCall { name, args, .. } = &expr.kind {
        if let Some(gf) = callees.fns.get(name.as_str()) {
            let mut subst = infer_type_subst(gf, args);
            // Resolve through the enclosing instantiation: inside
            // `List_first__Int`'s body the arg types are still written in terms
            // of `T`, so an unresolved binding here would mangle to `__T` and
            // reference a function nobody emits.
            for v in subst.values_mut() {
                *v = resolve_ty_param(v, outer);
            }
            if subst.len() == gf.type_params.len() {
                let mangled = mangle_generic_name(&gf.name, &gf.type_params, &subst);
                if seen.insert(mangled.clone(), ()).is_none() {
                    result.push((gf, subst, mangled));
                }
            }
        }
        for a in args {
            collect_instantiations_in_expr(a, callees, outer, seen, result);
        }
    }
    // Generic extension method call (`xs.flatten()`, `xs.map(f)`) — #2014.
    if let TirExprKind::MethodCall {
        receiver,
        method,
        args,
    } = &expr.kind
    {
        if let Some((gm, subst, mangled)) =
            resolve_generic_method_call(receiver, method, args, &callees.methods, outer)
        {
            if seen.insert(mangled.clone(), ()).is_none() {
                result.push((gm, subst, mangled));
            }
        }
        collect_instantiations_in_expr(receiver, callees, outer, seen, result);
        for a in args {
            collect_instantiations_in_expr(a, callees, outer, seen, result);
        }
    }
    // Recurse into sub-expressions.
    match &expr.kind {
        TirExprKind::Unary { expr: inner, .. }
        | TirExprKind::Consume(inner)
        | TirExprKind::Borrow { expr: inner, .. }
        | TirExprKind::Propagate(inner)
        | TirExprKind::FieldAccess { expr: inner, .. }
        | TirExprKind::Relabel { expr: inner, .. } => {
            collect_instantiations_in_expr(inner, callees, outer, seen, result);
        }
        // A lambda's *body* is a separate emission unit: `emit_one_lambda_fn`
        // runs `emit_expr` over it, which mangles any generic callee it finds.
        // Skipping it here meant `xss.map(|row: List[Int]| row.first())` emitted
        // `call $List_first__Int` for an instance collection never discovered —
        // the module then failed to link (#2014). Previously near-unreachable,
        // since a generic call rarely sat inside a lambda; lambda arguments to
        // generic methods are now the common case.
        TirExprKind::Lambda { body, .. } => {
            collect_instantiations_in_expr(body, callees, outer, seen, result);
        }
        TirExprKind::List { elems } | TirExprKind::Set { elems } => {
            for e in elems {
                collect_instantiations_in_expr(e, callees, outer, seen, result);
            }
        }
        TirExprKind::Map { pairs } => {
            for (k, v) in pairs {
                collect_instantiations_in_expr(k, callees, outer, seen, result);
                collect_instantiations_in_expr(v, callees, outer, seen, result);
            }
        }
        TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
            for (_, e) in fields {
                collect_instantiations_in_expr(e, callees, outer, seen, result);
            }
        }
        TirExprKind::Select { arms } => {
            for arm in arms {
                collect_instantiations_in_expr(&arm.expr, callees, outer, seen, result);
                collect_instantiations_in_block(&arm.body, callees, outer, seen, result);
            }
        }
        TirExprKind::Binary { left, right, .. } => {
            collect_instantiations_in_expr(left, callees, outer, seen, result);
            collect_instantiations_in_expr(right, callees, outer, seen, result);
        }
        TirExprKind::If { cond, then, else_ } => {
            collect_instantiations_in_expr(cond, callees, outer, seen, result);
            collect_instantiations_in_block(then, callees, outer, seen, result);
            if let Some(e) = else_ {
                collect_instantiations_in_expr(e, callees, outer, seen, result);
            }
        }
        TirExprKind::Block(b) => {
            collect_instantiations_in_block(b, callees, outer, seen, result);
        }
        TirExprKind::Match { scrutinee, arms } => {
            collect_instantiations_in_expr(scrutinee, callees, outer, seen, result);
            for arm in arms {
                match &arm.body {
                    TirMatchBody::Expr(e) => {
                        collect_instantiations_in_expr(e, callees, outer, seen, result);
                    }
                    TirMatchBody::Block(b) => {
                        collect_instantiations_in_block(b, callees, outer, seen, result);
                    }
                }
            }
        }
        // FnCall/MethodCall recurse above. `Literal`/`Var` have no
        // sub-expressions, and `Quantifier` is spec-only, erased before codegen.
        TirExprKind::Literal(_)
        | TirExprKind::Var(_)
        | TirExprKind::Quantifier(_)
        | TirExprKind::FnCall { .. }
        | TirExprKind::MethodCall { .. } => {}
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
    let mono_ctx = derived_ctx(ctx, type_subst);

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

    // Publish locals and `let` initializers, as `emit_fn` /
    // `emit_extension_method` do. Body emitters read both out of the Ctx
    // rather than as arguments — without them a monomorphized body's
    // `self.field = …` cannot find its layout and `return name` cannot trace
    // back to the heap-owning temp. Stored with the *declared* types (matching
    // the two callers above); resolution through `type_subst` happens at each
    // read site, since `mono_ctx.type_subst` is live for the whole body.
    *mono_ctx.fn_locals.borrow_mut() = locals.clone();
    *mono_ctx.fn_params.borrow_mut() = fn_scope_params(&f.params);
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&f.body, &mut let_inits);
    *mono_ctx.fn_let_inits.borrow_mut() = let_inits;
    // A generic *extension* method has a receiver, so `self` needs a bound type
    // name for the same reason `emit_extension_method` sets one (#2014). Plain
    // generic fns have no receiver and leave it `None`.
    *mono_ctx.self_type.borrow_mut() = f.receiver_type.clone();

    // Emit body.
    let mut body_buf = String::new();
    emit_block(&mut body_buf, &f.body, &mono_ctx);

    if body_buf.contains(UNSUPPORTED_MARKER) {
        emit_stub_body(out, mangled_name, ctx);
    } else {
        out.push_str(&body_buf);
    }
    out.push_str("  )\n");

    // Propagate needs_runtime back.
    if mono_ctx.needs_runtime.get() {
        ctx.needs_runtime.set(true);
    }
}

/// Resolve a type that may be a generic type param name, substituting
/// recursively through every type constructor.
///
/// The recursion into `List`/`Map`/`Set`/`Option`/`Result`/`Array`/`Fn` and
/// `Named`'s own type arguments matters for generic *extension methods*
/// (#2014): `List[T]::flatten` declares `self: List[List[T]]` and returns
/// `List[T]`, so a substitution of `T → Int` has to reach inside two
/// constructors to produce `List[Int]`. Substituting only at the top level
/// left `wasm_ty` looking at a bare `Ty::Named("T")`, which falls through to
/// its `_ => "i64"` default — an array pointer silently typed as i64.
fn resolve_ty_param(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    let rec = |t: &Ty| Box::new(resolve_ty_param(t, subst));
    match ty {
        Ty::Named(name, args) if args.is_empty() => {
            if let Some(concrete) = subst.get(name.as_str()) {
                concrete.clone()
            } else {
                ty.clone()
            }
        }
        Ty::Named(name, args) => {
            let resolved_args: Vec<Ty> = args.iter().map(|a| *rec(a)).collect();
            // A builtin wrapper type spelled `Ty::Named("Option", [T])`
            // instead of the structural `Ty::Option(Box::new(T))` — how a
            // generic extension method's own `self: Option[T]` parameter
            // resolves (#2125). After substituting `T`, normalize back to
            // the structural variant: every consumer downstream of generic
            // instantiation (`wasm_ty`, `is_i32`, `is_string_ty`, ...)
            // pattern-matches the structural shape, not this named
            // spelling, and silently fell to a wrong default (`wasm_ty`'s
            // `_ => "i64"`) for a receiver that's actually an i32 pointer —
            // the emitted function's own signature disagreed with what
            // every call site already pushed.
            match (name.as_str(), resolved_args.as_slice()) {
                ("List", [e]) => Ty::List(Box::new(e.clone())),
                ("Set", [e]) => Ty::Set(Box::new(e.clone())),
                ("Option", [e]) => Ty::Option(Box::new(e.clone())),
                ("Map", [k, v]) => Ty::Map(Box::new(k.clone()), Box::new(v.clone())),
                ("Result", [ok, err]) => Ty::Result(Box::new(ok.clone()), Box::new(err.clone())),
                _ => Ty::Named(name.clone(), resolved_args),
            }
        }
        Ty::Ref(m, inner) => Ty::Ref(*m, rec(inner)),
        Ty::Refined(inner, pred) => Ty::Refined(rec(inner), pred.clone()),
        Ty::Labeled(label, inner) => Ty::Labeled(label.clone(), rec(inner)),
        Ty::List(inner) => Ty::List(rec(inner)),
        Ty::Set(inner) => Ty::Set(rec(inner)),
        Ty::Option(inner) => Ty::Option(rec(inner)),
        Ty::Ptr(inner) => Ty::Ptr(rec(inner)),
        Ty::Array(inner, n) => Ty::Array(rec(inner), *n),
        Ty::Map(k, v) => Ty::Map(rec(k), rec(v)),
        Ty::Result(ok, err) => Ty::Result(rec(ok), rec(err)),
        Ty::Fn(params, ret, effects, totality) => Ty::Fn(
            params.iter().map(|p| *rec(p)).collect(),
            rec(ret),
            effects.clone(),
            totality.clone(),
        ),
        _ => ty.clone(),
    }
}

/// Structurally match a generic function's *declared* parameter type against
/// the *actual* type at a call site, binding type-param names along the way.
///
/// `infer_type_subst` only matches a whole parameter that is exactly a bare
/// type param (`fn f[T](x: T)`), which is enough for plain generic fns but not
/// for extension methods (#2014): the binding for `T` in `List[T]::map` comes
/// from *inside* the receiver's type, matching declared `List[T]` against
/// actual `List[Int]`. Wrappers (`ref`, refinement, label) are peeled on both
/// sides independently so a `ref List[Int]` receiver still binds `T → Int`.
///
/// Existing bindings win — the first occurrence of a param decides it, so a
/// mismatched later occurrence cannot silently overwrite an earlier one.
fn unify_ty_params(
    declared: &Ty,
    actual: &Ty,
    param_names: &std::collections::HashSet<String>,
    subst: &mut HashMap<String, Ty>,
) {
    // Peel wrappers that carry no information for substitution purposes.
    fn peel(t: &Ty) -> &Ty {
        let mut cur = t;
        loop {
            match cur {
                Ty::Ref(_, inner) | Ty::Refined(inner, _) | Ty::Labeled(_, inner) => cur = inner,
                _ => return cur,
            }
        }
    }
    // A builtin-wrapper type spelled `Ty::Named("Option", [T])` instead of
    // the structural `Ty::Option(Box::new(T))` (#2125) — this is how a
    // *self*-parameter's declared type resolves for a non-generic extension
    // method on a builtin wrapper (`pub fn Option[T]::is_some(self) -> Bool
    // { ... }`; `std/core.mvl`'s own declaration hits this too, not just
    // user code). The call site's actual receiver type is always the proper
    // structural variant, so `declared`/`actual` disagreed on shape and
    // fell through every arm below without binding anything — `subst`
    // stayed empty, the caller's `subst.len() != gm.type_params.len()`
    // check failed, and the whole call was rejected as unresolvable,
    // stubbing `is_some`/`is_none`/`is_ok`/`is_err` (and any other
    // Option/Result/List/Set/Map extension method that binds a receiver
    // type param and has no own type params of its own to force the
    // `generic_ext_methods` bucket some other way).
    fn normalize_named_wrapper(t: &Ty) -> Option<Ty> {
        let Ty::Named(name, args) = t else {
            return None;
        };
        match (name.as_str(), args.as_slice()) {
            ("List", [e]) => Some(Ty::List(Box::new(e.clone()))),
            ("Set", [e]) => Some(Ty::Set(Box::new(e.clone()))),
            ("Option", [e]) => Some(Ty::Option(Box::new(e.clone()))),
            ("Map", [k, v]) => Some(Ty::Map(Box::new(k.clone()), Box::new(v.clone()))),
            ("Result", [ok, err]) => Some(Ty::Result(Box::new(ok.clone()), Box::new(err.clone()))),
            _ => None,
        }
    }
    let declared_owned = normalize_named_wrapper(declared);
    let declared = declared_owned.as_ref().unwrap_or(declared);
    let actual_owned = normalize_named_wrapper(actual);
    let actual = actual_owned.as_ref().unwrap_or(actual);

    let (declared, actual) = (peel(declared), peel(actual));

    if let Ty::Named(name, args) = declared {
        if args.is_empty() && param_names.contains(name.as_str()) {
            subst.entry(name.clone()).or_insert_with(|| actual.clone());
            return;
        }
    }

    match (declared, actual) {
        (Ty::List(d), Ty::List(a))
        | (Ty::Set(d), Ty::Set(a))
        | (Ty::Option(d), Ty::Option(a))
        | (Ty::Ptr(d), Ty::Ptr(a))
        | (Ty::Array(d, _), Ty::Array(a, _)) => unify_ty_params(d, a, param_names, subst),
        // A fixed-size array flows into a `List[T]` parameter, and a bare list
        // literal can land where an `Array[T, N]` is declared — bind the
        // element either way rather than giving up on the shape mismatch.
        (Ty::List(d), Ty::Array(a, _)) | (Ty::Array(d, _), Ty::List(a)) => {
            unify_ty_params(d, a, param_names, subst)
        }
        (Ty::Map(dk, dv), Ty::Map(ak, av)) => {
            unify_ty_params(dk, ak, param_names, subst);
            unify_ty_params(dv, av, param_names, subst);
        }
        (Ty::Result(dok, derr), Ty::Result(aok, aerr)) => {
            unify_ty_params(dok, aok, param_names, subst);
            unify_ty_params(derr, aerr, param_names, subst);
        }
        (Ty::Fn(dp, dr, ..), Ty::Fn(ap, ar, ..)) => {
            for (d, a) in dp.iter().zip(ap.iter()) {
                unify_ty_params(d, a, param_names, subst);
            }
            unify_ty_params(dr, ar, param_names, subst);
        }
        (Ty::Named(dn, da), Ty::Named(an, aa)) if dn == an => {
            for (d, a) in da.iter().zip(aa.iter()) {
                unify_ty_params(d, a, param_names, subst);
            }
        }
        _ => {}
    }
}

/// Collect every bare `Ty::Named(name, [])` leaf appearing in `ty` — used to
/// find a declared self-parameter type's own generic placeholders (`T`, `E`
/// in `Result[T, E]`) so `resolve_generic_method_call` can bind them even
/// when the method introduces no type params of its own beyond them (#2149).
/// Safe by construction: `Ty::Int`/`Ty::String`/etc. are dedicated variants,
/// never `Ty::Named` — so every leaf found here in a *declared* signature is
/// genuinely a placeholder, not a concrete type name.
fn collect_named_leaves(ty: &Ty, out: &mut std::collections::HashSet<String>) {
    match ty {
        Ty::Named(name, args) if args.is_empty() => {
            out.insert(name.clone());
        }
        Ty::Named(_, args) => {
            for a in args {
                collect_named_leaves(a, out);
            }
        }
        Ty::List(inner) | Ty::Set(inner) | Ty::Option(inner) | Ty::Ptr(inner) => {
            collect_named_leaves(inner, out)
        }
        Ty::Array(inner, _) => collect_named_leaves(inner, out),
        Ty::Map(k, v) => {
            collect_named_leaves(k, out);
            collect_named_leaves(v, out);
        }
        Ty::Result(ok, err) => {
            collect_named_leaves(ok, out);
            collect_named_leaves(err, out);
        }
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            collect_named_leaves(inner, out)
        }
        Ty::Fn(params, ret, ..) => {
            for p in params {
                collect_named_leaves(p, out);
            }
            collect_named_leaves(ret, out);
        }
        _ => {}
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

/// Peel a field's declared type down to the representation
/// `field_byte_size`/`field_alignment` actually size against — `Ty::Ref`/
/// `Ty::Labeled`/`Ty::Refined` wrappers, and `type X = <target>` alias
/// references (`type Port = Int where ...` used as a field type lowers to
/// `Ty::Named("Port", [])` in TIR, not `Ty::Int` — see `typeexpr_to_ty` in
/// `ir/lower.rs`, a "shallow conversion").
///
/// Without this, a struct field typed as an alias to `Int`/`UInt`/`Float`
/// fell into `field_byte_size`'s 4-byte default (real data corruption: the
/// struct was under-allocated and a following field's offset landed inside
/// the 8 bytes `emit_struct_store`/`emit_field_access` — which DO resolve
/// aliases via `Ctx::type_aliases` — actually read and wrote for that field).
/// Found while building the WASM `extern "rust"` FFI host glue, which needs
/// byte-accurate struct layouts to marshal fields correctly; unrelated to
/// FFI itself, so fixed here rather than worked around in the new code.
fn resolve_field_ty<'a>(ty: &'a Ty, aliases: &'a HashMap<String, Ty>) -> std::borrow::Cow<'a, Ty> {
    match ty {
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => {
            match resolve_field_ty(inner, aliases) {
                std::borrow::Cow::Borrowed(t) => std::borrow::Cow::Owned(t.clone()),
                owned => owned,
            }
        }
        Ty::Named(name, args) if args.is_empty() => match aliases.get(name.as_str()) {
            Some(target) => std::borrow::Cow::Owned(resolve_field_ty(target, aliases).into_owned()),
            None => std::borrow::Cow::Borrowed(ty),
        },
        _ => std::borrow::Cow::Borrowed(ty),
    }
}

fn field_byte_size(ty: &Ty, aliases: &HashMap<String, Ty>) -> u32 {
    match resolve_field_ty(ty, aliases).as_ref() {
        Ty::Int | Ty::UInt | Ty::Float => 8,
        // Everything else is an i32-width value in the struct slot.
        _ => 4,
    }
}

fn field_alignment(ty: &Ty, aliases: &HashMap<String, Ty>) -> u32 {
    field_byte_size(ty, aliases)
}

pub(crate) fn collect_structs(
    types: &[TirTypeDecl],
    type_aliases: &HashMap<String, Ty>,
) -> HashMap<String, StructLayout> {
    let mut map = HashMap::new();
    for td in types {
        if let TirTypeBody::Struct { fields, .. } = &td.body {
            let mut offset = 0u32;
            let mut slots = Vec::new();
            for f in fields {
                let size = field_byte_size(&f.ty, type_aliases);
                let align = field_alignment(&f.ty, type_aliases);
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
    type_aliases: &HashMap<String, Ty>,
) -> HashMap<String, ActorInfo> {
    let mut map = HashMap::new();
    for (idx, ad) in actors.iter().enumerate() {
        let mut offset = 0u32;
        let mut slots = Vec::new();
        for f in &ad.fields {
            let size = field_byte_size(&f.ty, type_aliases);
            let align = field_alignment(&f.ty, type_aliases);
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
    *ctx.fn_params.borrow_mut() = fn_scope_params(&m.params);
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&m.body, &mut let_inits);
    *ctx.fn_let_inits.borrow_mut() = let_inits;
    emit_block(&mut body, &m.body, ctx);

    if body.contains(UNSUPPORTED_MARKER) {
        emit_stub_body(out, &fn_name, ctx);
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
    *ctx.fn_params.borrow_mut() = fn_scope_params(&f.params);
    let mut let_inits = HashMap::new();
    collect_let_inits_block(&f.body, &mut let_inits);
    *ctx.fn_let_inits.borrow_mut() = let_inits;
    emit_block(&mut body, &f.body, ctx);

    if body.contains(UNSUPPORTED_MARKER) {
        emit_stub_body(out, &wasm_name, ctx);
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

/// True if `method` is a non-generic pure-MVL extension method registered
/// on `receiver`'s type (#2054, #2058 follow-up; widened for #2125). Despite
/// the name, this isn't struct-only: any method declared with no type
/// params of its own — `pub fn Option[T]::is_some(self) -> Bool { ... }`,
/// say — lands in the same `ext_methods` bucket as a user struct's own
/// extension methods (`collect_program`'s `(has receiver?, is generic?)`
/// partition only looks at the *method's* type params, not the receiver's),
/// and is emitted the same way (`emit_extension_method`, named
/// `${receiver_type}_${method}`).
///
/// Originally `named_type_name`, which only resolves `Ty::Named` — so any
/// non-generic pure-MVL method on `List`/`Set`/`Map`/`Option`/`Result`
/// (built-in wrapper types, never `Ty::Named`) had a real emitted body that
/// no call site could ever reach: this always returned `false` for them,
/// every earlier native-method arm had already missed them (that's *why*
/// they ended up here, last resort), and the call fell to `;; unsupported
/// expr`. `receiver_type_name` resolves both named and built-in-constructor
/// receivers uniformly (`Ty::List(_) => "List"`, `Ty::Option(_) =>
/// "Option"`, etc.) — the same mapping `emit_extension_method` and
/// `struct_methods` already key on, so this now actually agrees with what
/// got emitted.
///
/// Consulted by any dispatch arm that would otherwise match purely on
/// method name (e.g. `to_string`, checked below) so a type's own
/// `to_string`/etc. extension method isn't shadowed by a builtin-type
/// special case that never considered the receiver's type.
fn is_struct_method_call(receiver: &TirExpr, method: &str, ctx: &Ctx) -> bool {
    receiver_type_name(&receiver.ty)
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
    audit_relabels: &AuditRelabels,
) -> (HashMap<String, (u32, u32)>, u32) {
    let mut map = HashMap::new();
    let mut next = LITERAL_BASE;
    // Seed "true" / "false" so `Bool.to_string()` has offsets to point at.
    // Cheap: 4 + 5 = 9 bytes of data section even when unused. Unconditional
    // (not gated on `needs_wasi`, i.e. "has a `fn main`") for the same reason
    // the emission gate below was fixed in #2153: `mvl test --backend=wasm`
    // compiles each `test fn` standalone with no synthesized `main`, so
    // `needs_wasi` was always false there — `Bool::to_string()`'s lookup
    // (`ctx.literals.get("true")`) silently fell back to `(0, 0)` instead of
    // a hard error, producing an empty/garbage string rather than "true".
    for lit in &["true", "false"] {
        let len = lit.len() as u32;
        map.insert((*lit).to_string(), (next, len));
        next += len;
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

    /// Regression for a data-corruption bug found while building the WASM
    /// `extern "rust"` FFI host glue (#2049 follow-up): `type Port = Int
    /// where ...` used as a struct field lowers to `Ty::Named("Port", [])`
    /// in TIR (a "shallow conversion", see `typeexpr_to_ty` in
    /// `ir/lower.rs`), not `Ty::Int`. `field_byte_size` had no alias
    /// resolution and fell into its 4-byte default for that field, while
    /// `emit_struct_store`/`emit_field_access` (which DO resolve aliases via
    /// `Ctx::type_aliases`) correctly treated it as an 8-byte `i64` slot —
    /// under-allocating the struct so a following field's offset landed
    /// inside those 8 bytes. Fixed by threading `type_aliases` into
    /// `field_byte_size`/`field_alignment` via `resolve_field_ty`.
    #[test]
    fn refined_alias_struct_field_does_not_overlap_following_field() {
        let wat = compile(
            "type Port = Int where self > 0 && self <= 65535\n\
             type Config = struct { port: Port, name: String }\n\
             fn main() -> Unit ! Console {\n\
                 let c: Config = Config { port: 8080, name: \"hi\" };\n\
                 println(c.name)\n\
             }\n",
        );
        // 8 bytes for `port` (i64) + 4 for `name`'s pointer, rounded to 16 —
        // NOT 8 (which is what the 4-byte-default bug produced).
        assert!(
            wat.contains("i32.const 16\n    call $_mvl_struct_alloc"),
            "{wat}"
        );
        assert!(wat.contains("i64.store offset=0"), "{wat}");
        assert!(
            wat.contains("i32.store offset=8"),
            "`name` must be stored at offset=8, past the full 8-byte `port` \
             slot, not overlapping it at offset=4\n{wat}"
        );
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
        // Scoped to `describe`'s own body — the module now also carries the
        // WASI helper blob (this program has string literals, #2153), and
        // `$mvl_now`'s alignment rounding has its own unrelated `i32.and`.
        let describe_start = wat.find("(func $describe").expect("describe not in module");
        let describe_body = &wat[describe_start..];
        let describe_end = describe_body
            .find("\n  (export")
            .unwrap_or(describe_body.len());
        let describe_body = &describe_body[..describe_end];
        // The three `Solo(Weekday::X)` arms get one guard each; the
        // `Duo(Weekday::Mon, Season::Spring)` arm gets two ANDed together
        // (both slots live); `Duo(_, Season::Fall)` gets one (first slot is
        // wildcarded); `Duo(_, _)` gets none. Total: 3 + 2 + 1 = 6.
        assert_eq!(
            describe_body.matches("i32.and").count(),
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

    /// `now()` / `_instant_epoch_seconds(t)` route through the WASM runtime,
    /// not a dangling `call $now` (#2056, #2094).
    #[test]
    fn now_and_epoch_seconds_use_runtime() {
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
        // Bare `$now`/`$_instant_epoch_seconds` should NOT be emitted (those
        // would be plain WAT-local shims); the runtime versions use `$_mvl_`.
        assert!(!wat.contains("call $now\n"), "{wat}");
        assert!(!wat.contains("call $_instant_epoch_seconds\n"), "{wat}");
        // Runtime functions are used:
        assert!(wat.contains("call $_mvl_time_now"), "{wat}");
        assert!(
            wat.contains("call $_mvl_time_instant_epoch_seconds"),
            "{wat}"
        );
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

// ── Generic extension methods (#2014) ────────────────────────────────────
//
// `List[T]::flatten` and friends carry a `receiver_type` *and* type params,
// which put them outside every emission bucket before #2014. These tests use
// a user-declared generic extension method rather than `std/lists.mvl` so
// they stay independent of the prelude the CLI assembles.

#[cfg(test)]
mod generic_ext_method_tests {
    use super::*;
    use crate::mvl::parser::Parser;

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

    /// The bucket gap itself: before #2014 this body stubbed to `unreachable`.
    #[test]
    fn generic_ext_method_emits_monomorphized_body_and_call() {
        let wat = compile(
            "pub fn List[T]::tally(self) -> Int { 7 }\n\
             test fn t() -> Unit { let xs: List[Int] = [1, 2]; assert_eq(xs.tally(), 7); }\n",
        );
        // Mangled with the receiver type, so `Set[T]::tally` could coexist.
        assert!(
            wat.contains("(func $List_tally__Int (param $self i32) (result i64)"),
            "{wat}"
        );
        assert!(wat.contains("call $List_tally__Int"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
    }

    /// Two element types must produce two distinct instances, not one shared
    /// body typed by whichever call site was walked first.
    #[test]
    fn distinct_element_types_get_distinct_instances() {
        let wat = compile(
            "pub fn List[T]::tally(self) -> Int { 7 }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1];\n\
                 let ys: List[Bool] = [true];\n\
                 assert_eq(xs.tally(), 7);\n\
                 assert_eq(ys.tally(), 7);\n\
             }\n",
        );
        assert!(wat.contains("(func $List_tally__Int"), "{wat}");
        assert!(wat.contains("(func $List_tally__Bool"), "{wat}");
    }

    /// A method returning `List[T]` needs `resolve_ty_param` to substitute
    /// *inside* the constructor; a shallow substitution left the result typed
    /// from `wasm_ty`'s `_ => "i64"` default instead of an i32 array pointer.
    #[test]
    fn generic_ext_method_returning_list_resolves_element_type() {
        let wat = compile(
            "pub fn List[T]::dup(self) -> List[T] { self }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2];\n\
                 assert_eq(xs.dup().len(), 2);\n\
             }\n",
        );
        assert!(
            wat.contains("(func $List_dup__Int (param $self i32) (result i32)"),
            "{wat}"
        );
        assert!(!wat.contains("body stubbed"), "{wat}");
    }

    /// One generic method calling another must instantiate the callee under the
    /// *caller's* substitution — scanning with an empty one looks for
    /// `List_inner__T` and emits a call to a function nobody defines.
    #[test]
    fn nested_generic_method_call_instantiates_callee() {
        let wat = compile(
            "pub fn List[T]::inner(self) -> Int { 3 }\n\
             pub fn List[T]::outer(self) -> Int { self.inner() }\n\
             test fn t() -> Unit { let xs: List[Int] = [1]; assert_eq(xs.outer(), 3); }\n",
        );
        assert!(wat.contains("(func $List_outer__Int"), "{wat}");
        assert!(wat.contains("(func $List_inner__Int"), "{wat}");
        assert!(wat.contains("call $List_inner__Int"), "{wat}");
        // No unresolved-substitution leftovers.
        assert!(!wat.contains("__T"), "{wat}");
    }

    /// `.push()` had no dispatch arm at all — `_mvl_array_push_*` was reachable
    /// only from list *literals*. Every `std/lists.mvl` body needs it.
    #[test]
    fn list_push_method_emits_typed_push() {
        let wat = compile(
            "test fn t() -> Unit {\n\
                 let acc: ref List[Int] = [];\n\
                 acc.push(5);\n\
                 assert_eq(acc.len(), 1);\n\
             }\n",
        );
        assert!(wat.contains("call $_mvl_array_push_i64"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
    }

    /// A natively-dispatched method must NOT also be monomorphized: emission
    /// prefers its builtin arm, so the instance would be dead — and a dead
    /// invalid body is enough to make wasmtime reject the whole module (the
    /// `Option_unwrap_or__Str` regression).
    #[test]
    fn natively_handled_method_is_not_monomorphized() {
        let wat = compile(
            "pub fn List[T]::len(self) -> Int { 99 }\n\
             test fn t() -> Unit { let xs: List[Int] = [1, 2]; assert_eq(xs.len(), 2); }\n",
        );
        assert!(!wat.contains("$List_len__Int"), "{wat}");
        assert!(wat.contains("call $_mvl_array_len"), "{wat}");
    }

    #[test]
    fn emitter_handles_method_natively_is_receiver_shaped() {
        // `concat` has native arms on both receivers (#2114 added the List
        // one), but each dispatches to a completely different runtime call
        // (`_mvl_string_concat` vs `_mvl_array_concat`) — a name-only check
        // would collapse that distinction.
        assert!(emitter_handles_method_natively(&Ty::String, "concat"));
        assert!(emitter_handles_method_natively(
            &Ty::List(Box::new(Ty::Int)),
            "concat"
        ));
        assert!(emitter_handles_method_natively(
            &Ty::List(Box::new(Ty::Int)),
            "push"
        ));
        assert!(!emitter_handles_method_natively(
            &Ty::List(Box::new(Ty::Int)),
            "flatten"
        ));
        assert!(emitter_handles_method_natively(
            &Ty::Option(Box::new(Ty::Int)),
            "unwrap_or"
        ));
    }

    #[test]
    fn receiver_type_name_maps_builtin_constructors() {
        // The parser stores `fn List[T]::first` under "List", but a receiver's
        // type is `Ty::List`, never `Ty::Named("List", _)`.
        assert_eq!(
            receiver_type_name(&Ty::List(Box::new(Ty::Int))).as_deref(),
            Some("List")
        );
        assert_eq!(
            receiver_type_name(&Ty::Ref(true, Box::new(Ty::List(Box::new(Ty::Int))))).as_deref(),
            Some("List"),
            "ref wrapper must peel"
        );
        assert_eq!(
            receiver_type_name(&Ty::Named("Logger".into(), vec![])).as_deref(),
            Some("Logger")
        );
    }

    #[test]
    fn resolve_ty_param_substitutes_inside_constructors() {
        let subst: HashMap<String, Ty> = [("T".to_string(), Ty::Int)].into_iter().collect();
        // `List[List[T]]` — flatten's own receiver, two constructors deep.
        let nested = Ty::List(Box::new(Ty::List(Box::new(Ty::Named("T".into(), vec![])))));
        assert_eq!(
            resolve_ty_param(&nested, &subst),
            Ty::List(Box::new(Ty::List(Box::new(Ty::Int))))
        );
        // Map values and Result payloads too.
        let m = Ty::Map(
            Box::new(Ty::String),
            Box::new(Ty::Named("T".into(), vec![])),
        );
        assert_eq!(
            resolve_ty_param(&m, &subst),
            Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))
        );
    }

    #[test]
    fn unify_ty_params_binds_from_inside_receiver() {
        let names: std::collections::HashSet<String> = ["T".to_string()].into_iter().collect();

        // declared `List[T]` vs actual `List[Int]` → T = Int.
        let mut subst = HashMap::new();
        unify_ty_params(
            &Ty::List(Box::new(Ty::Named("T".into(), vec![]))),
            &Ty::List(Box::new(Ty::Int)),
            &names,
            &mut subst,
        );
        assert_eq!(subst.get("T"), Some(&Ty::Int));

        // flatten's shape: `List[List[T]]` vs `List[List[Bool]]`.
        let mut subst = HashMap::new();
        unify_ty_params(
            &Ty::List(Box::new(Ty::List(Box::new(Ty::Named("T".into(), vec![]))))),
            &Ty::List(Box::new(Ty::List(Box::new(Ty::Bool)))),
            &names,
            &mut subst,
        );
        assert_eq!(subst.get("T"), Some(&Ty::Bool));

        // A `ref` receiver still binds — wrappers peel on both sides.
        let mut subst = HashMap::new();
        unify_ty_params(
            &Ty::List(Box::new(Ty::Named("T".into(), vec![]))),
            &Ty::Ref(true, Box::new(Ty::List(Box::new(Ty::Float)))),
            &names,
            &mut subst,
        );
        assert_eq!(subst.get("T"), Some(&Ty::Float));

        // First binding wins rather than being overwritten by a later mismatch.
        let mut subst = HashMap::new();
        let declared = Ty::Map(
            Box::new(Ty::Named("T".into(), vec![])),
            Box::new(Ty::Named("T".into(), vec![])),
        );
        unify_ty_params(
            &declared,
            &Ty::Map(Box::new(Ty::Int), Box::new(Ty::String)),
            &names,
            &mut subst,
        );
        assert_eq!(subst.get("T"), Some(&Ty::Int));
    }
}

// ── Function values: funcref table + call_indirect (#2014) ───────────────

#[cfg(test)]
mod funcref_table_tests {
    use super::*;
    use crate::mvl::parser::Parser;

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

    /// A user-declared HOF, so the test does not depend on `std/lists.mvl`.
    const MYMAP: &str = "pub fn List[T]::mymap[U](self, f: fn(T) -> U) -> List[U] {\n\
                             let r: ref List[U] = [];\n\
                             for x in self { r.push(f(x)) };\n\
                             r\n\
                         }\n";

    #[test]
    fn hof_call_emits_table_elem_and_call_indirect() {
        let wat = compile(&format!(
            "{MYMAP}test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1, 2];\n\
                 let d: List[Int] = xs.mymap(|x: Int| x * 2);\n\
                 assert_eq(d.len(), 2);\n\
             }}\n"
        ));
        assert!(wat.contains("(table 1 funcref)"), "{wat}");
        assert!(wat.contains("(elem (i32.const 0) $__lambda_"), "{wat}");
        assert!(
            wat.contains("(type $sig_i64_r_i64 (func (param i32) (param i64) (result i64)))"),
            "{wat}"
        );
        assert!(wat.contains("call_indirect (type $sig_i64_r_i64)"), "{wat}");
        // The lambda body became a real top-level function.
        assert!(wat.contains("(func $__lambda_"), "{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
    }

    /// The lambda's return type is `Unknown` in TIR, so `U` has to come from the
    /// body — otherwise the instance mangles to `..__Unknown` and every `f(x)`
    /// result takes `wasm_ty`'s i64 default.
    #[test]
    fn lambda_body_type_resolves_the_result_type_param() {
        let wat = compile(&format!(
            "{MYMAP}test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1];\n\
                 let d: List[Bool] = xs.mymap(|x: Int| x > 0);\n\
                 assert_eq(d.len(), 1);\n\
             }}\n"
        ));
        assert!(wat.contains("$List_mymap__Int__Bool"), "{wat}");
        assert!(!wat.contains("Unknown"), "{wat}");
        // Predicate lambda returns Bool → i32 result.
        assert!(wat.contains("call_indirect (type $sig_i64_r_i32)"), "{wat}");
    }

    /// Distinct lambdas must occupy distinct slots, and the `elem` order must
    /// match the indices the call sites push.
    #[test]
    fn distinct_lambdas_get_distinct_table_slots() {
        let wat = compile(&format!(
            "{MYMAP}test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1];\n\
                 let a: List[Int] = xs.mymap(|x: Int| x * 2);\n\
                 let b: List[Int] = xs.mymap(|x: Int| x + 9);\n\
                 assert_eq(a.len(), b.len());\n\
             }}\n"
        ));
        assert!(wat.contains("(table 2 funcref)"), "{wat}");
        assert!(wat.contains("i32.const 0"), "{wat}");
        assert!(wat.contains("i32.const 1"), "{wat}");
        let elem = wat
            .lines()
            .find(|l| l.contains("(elem (i32.const 0)"))
            .expect("elem segment");
        assert_eq!(elem.matches("$__lambda_").count(), 2, "{wat}");
    }

    /// Structurally identical signatures must collapse onto one `(type)`, or
    /// the module carries redundant declarations for the same shape.
    #[test]
    fn identical_signatures_share_one_type_decl() {
        let wat = compile(&format!(
            "{MYMAP}test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1];\n\
                 let a: List[Int] = xs.mymap(|x: Int| x * 2);\n\
                 let b: List[Int] = xs.mymap(|x: Int| x + 9);\n\
                 assert_eq(a.len(), b.len());\n\
             }}\n"
        ));
        // Count declarations only — `call_indirect (type $sig…)` shares the
        // prefix, so match the `(func` that only a declaration carries.
        assert_eq!(
            wat.matches("(type $sig_i64_r_i64 (func").count(),
            1,
            "signature declared more than once:\n{wat}"
        );
    }

    /// A function value is a table index (i32), not a heap pointer or an i64.
    #[test]
    fn fn_typed_param_is_an_i32() {
        let wat = compile(&format!(
            "{MYMAP}test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1];\n\
                 let d: List[Int] = xs.mymap(|x: Int| x * 2);\n\
                 assert_eq(d.len(), 1);\n\
             }}\n"
        ));
        assert!(
            wat.contains("(func $List_mymap__Int__Int (param $self i32) (param $f i32)"),
            "{wat}"
        );
    }

    /// Regression: params must not join `fn_locals`, which drives the drop
    /// sweep. When they did, `List[T]::any`'s early `return true` emitted
    /// `local.get $self; call $_mvl_array_drop`, freeing the *caller's* list —
    /// `list_any` then trapped on its second `.any()` call.
    #[test]
    fn early_return_does_not_drop_a_parameter() {
        let wat = compile(
            "pub fn List[T]::myany(self, f: fn(T) -> Bool) -> Bool {\n\
                 for x in self { if f(x) { return true } };\n\
                 false\n\
             }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 5];\n\
                 assert_eq(xs.myany(|x: Int| x > 3), true);\n\
                 assert_eq(xs.myany(|x: Int| x > 9), false);\n\
             }\n",
        );
        let body = wat
            .split("(func $List_myany__Int")
            .nth(1)
            .expect("myany instance");
        let body = body.split("\n  )").next().unwrap();
        assert!(
            !body.contains("local.get $self\n    call $_mvl_array_drop"),
            "early return drops the caller's array:\n{body}"
        );
    }

    /// A module with no function values must not grow a table or elem segment.
    #[test]
    fn no_table_emitted_when_no_lambdas() {
        let wat = compile("test fn t() -> Unit { assert_eq(1 + 1, 2); }\n");
        assert!(!wat.contains("funcref"), "{wat}");
        assert!(!wat.contains("(elem"), "{wat}");
        assert!(!wat.contains("call_indirect"), "{wat}");
    }

    #[test]
    fn indirect_sig_names_collapse_by_wasm_type() {
        // `Int` and `UInt` are both i64, so they must share a signature name;
        // `Bool` is i32 and must not.
        let names = |params: Vec<Ty>, ret: Ty| {
            let stubbed = std::cell::RefCell::new(Vec::new());
            let lambdas = std::cell::RefCell::new(Vec::new());
            let slots = std::cell::RefCell::new(HashMap::new());
            let sigs = std::cell::RefCell::new(std::collections::BTreeMap::new());
            let empty_subst: HashMap<String, Ty> = HashMap::new();
            let empty_lits = HashMap::new();
            let empty_audit: AuditRelabels = HashMap::new();
            let empty_enum_types = std::collections::HashSet::new();
            let empty_variants = HashMap::new();
            let empty_layouts = HashMap::new();
            let empty_payload = HashMap::new();
            let empty_aliases = HashMap::new();
            let empty_generic = HashMap::new();
            let empty_actors = HashMap::new();
            let empty_methods = std::collections::HashSet::new();
            let empty_gmethods = HashMap::new();
            let ctx = Ctx {
                needs_wasi: false,
                literals: &empty_lits,
                audit_relabels: &empty_audit,
                enum_types: &empty_enum_types,
                enum_variants: &empty_variants,
                struct_layouts: &empty_layouts,
                payload_enums: &empty_payload,
                type_aliases: &empty_aliases,
                type_subst: &empty_subst,
                generic_fn_map: &empty_generic,
                label_counter: Cell::new(0),
                needs_runtime: Cell::new(false),
                string_params: std::cell::RefCell::new(std::collections::HashSet::new()),
                assert_mode: AssertMode::Always,
                fn_locals: std::cell::RefCell::new(Vec::new()),
                fn_let_inits: std::cell::RefCell::new(HashMap::new()),
                actors: &empty_actors,
                self_type: std::cell::RefCell::new(None),
                struct_methods: &empty_methods,
                generic_methods: &empty_gmethods,
                fn_params: std::cell::RefCell::new(Vec::new()),
                stubbed: &stubbed,
                lambdas: &lambdas,
                lambda_slots: &slots,
                indirect_sigs: &sigs,
            };
            indirect_sig(&params, &ret, &ctx).0
        };
        assert_eq!(
            names(vec![Ty::Int], Ty::Int),
            names(vec![Ty::UInt], Ty::UInt)
        );
        assert_ne!(
            names(vec![Ty::Int], Ty::Int),
            names(vec![Ty::Int], Ty::Bool)
        );
        // A Unit return contributes no `(result …)`.
        assert!(!names(vec![Ty::Int], Ty::Unit).contains("_r_"));
    }
}

// ── Stub visibility (#2014) ──────────────────────────────────────────────
//
// A stubbed body is discarded and replaced by `unreachable`; the module still
// assembles and the CLI exits 0. These tests pin that such a build *reports*
// itself, because silence is how gaps accumulated unnoticed.

#[cfg(test)]
mod stub_reporting_tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn compile_with(src: &str) -> (String, Vec<String>) {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
        expr_types.extend(crate::mvl::checker::check(&prog).expr_types);
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let compiler = WasmTextCompiler::new();
        let wat = compiler.emit_program(&tir, "test");
        (wat, compiler.stubbed_fns())
    }

    #[test]
    fn fully_supported_program_reports_no_stubs() {
        let (wat, stubbed) = compile_with("test fn t() -> Unit { assert_eq(1 + 1, 2); }\n");
        assert!(stubbed.is_empty(), "unexpected stubs {stubbed:?}\n{wat}");
    }

    #[test]
    fn stubbed_function_is_reported_by_name() {
        // `.clone()` on an Option is deliberately unsupported.
        let (_, stubbed) = compile_with(
            "test fn uses_unsupported() -> Unit {\n\
                 let o: Option[Int] = Some(1);\n\
                 let p: Option[Int] = o.clone();\n\
                 assert_eq(p.unwrap_or(0), 1);\n\
             }\n\
             test fn fine() -> Unit { assert_eq(1 + 1, 2); }\n",
        );
        assert_eq!(stubbed, vec!["uses_unsupported".to_string()]);
    }

    /// Regression: a capturing lambda used to emit a function referencing an
    /// undeclared local, which makes `wasm-tools` reject the *entire* module —
    /// taking every unrelated function down with it. It must stub instead, so
    /// the damage stays inside the one function that cannot be compiled.
    #[test]
    fn scalar_capturing_lambda_compiles_with_environment() {
        // A scalar capture (#2118) is no longer stubbed — `k` is heap-boxed
        // into the closure's environment and loaded back inside the lambda.
        let (wat, stubbed) = compile_with(
            "pub fn List[T]::mymap[U](self, f: fn(T) -> U) -> List[U] {\n\
                 let r: ref List[U] = [];\n\
                 for x in self { r.push(f(x)) };\n\
                 r\n\
             }\n\
             test fn t() -> Unit {\n\
                 let k: Int = 10;\n\
                 let xs: List[Int] = [1];\n\
                 let d: List[Int] = xs.mymap(|x: Int| x + k);\n\
                 assert_eq(d.len(), 1);\n\
             }\n",
        );
        assert!(
            !stubbed.iter().any(|n| n.starts_with("__lambda_")),
            "capturing lambda unexpectedly stubbed: {stubbed:?}\n{wat}"
        );
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(!wat.contains("lambda captures"), "{wat}");
        // The lambda function loads `k` from its env pointer instead of
        // reading an undeclared local.
        assert!(wat.contains("(func $__lambda_"), "{wat}");
        assert!(wat.contains("(param $__env i32)"), "{wat}");
        assert!(wat.contains("local.get $__env"), "{wat}");
        assert!(wat.contains("local.set $k"), "{wat}");
        // The call site boxes `k` into a heap environment before invoking.
        assert!(wat.contains("call $mvl_alloc"), "{wat}");
    }

    /// A capture of a heap-owned type (here `String`) is still unsupported
    /// (#2118 scopes captures to `is_capturable_scalar`) — deep-clone-on-
    /// capture semantics are real design work this fix doesn't take on, so
    /// the pre-existing stub-and-contain behavior must still apply.
    #[test]
    fn heap_owned_capturing_lambda_still_stubs() {
        let (wat, stubbed) = compile_with(
            "pub fn List[T]::mymap[U](self, f: fn(T) -> U) -> List[U] {\n\
                 let r: ref List[U] = [];\n\
                 for x in self { r.push(f(x)) };\n\
                 r\n\
             }\n\
             test fn t() -> Unit {\n\
                 let suffix: String = \"!\";\n\
                 let xs: List[String] = [\"hi\"];\n\
                 let d: List[String] = xs.mymap(|x: String| x.concat(suffix));\n\
                 assert_eq(d.len(), 1);\n\
             }\n",
        );
        assert!(
            stubbed.iter().any(|n| n.starts_with("__lambda_")),
            "heap-owned capturing lambda not stubbed: {stubbed:?}\n{wat}"
        );
        assert!(wat.contains("lambda captures `suffix"), "{wat}");
        assert!(!wat.contains("local.get $suffix"), "{wat}");
    }

    /// Same failure mode from the other direction: a top-level function used as
    /// a value has no table slot, so `local.get $double` named a local that
    /// never existed.
    #[test]
    fn named_function_as_value_compiles_via_synthesized_wrapper() {
        // A named top-level function used as a value (#2159) is no longer
        // stubbed — `emit_named_fn_as_value` synthesizes a thin
        // non-capturing wrapper lambda and boxes it exactly like a real
        // lambda literal.
        let (wat, stubbed) = compile_with(
            "fn double(x: Int) -> Int { x * 2 }\n\
             fn apply(f: fn(Int) -> Int, v: Int) -> Int { f(v) }\n\
             test fn t() -> Unit { assert_eq(apply(double, 3), 6); }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}\n{wat}");
        assert!(!wat.contains("body stubbed"), "{wat}");
        assert!(!wat.contains("named function used as a value"), "{wat}");
        // The synthesized wrapper calls straight through to `double`.
        assert!(wat.contains("call $double"), "{wat}");
        assert!(wat.contains("call_indirect (type $sig_i64_r_i64)"), "{wat}");
    }

    /// Non-capturing lambdas must be unaffected — the whole point of #2014.
    #[test]
    fn non_capturing_lambda_is_not_stubbed() {
        let (wat, stubbed) = compile_with(
            "pub fn List[T]::mymap[U](self, f: fn(T) -> U) -> List[U] {\n\
                 let r: ref List[U] = [];\n\
                 for x in self { r.push(f(x)) };\n\
                 r\n\
             }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1];\n\
                 let d: List[Int] = xs.mymap(|x: Int| x * 2);\n\
                 assert_eq(d.len(), 1);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs {stubbed:?}\n{wat}");
    }

    #[test]
    fn stub_list_is_per_emit_not_cumulative() {
        let src = "test fn t() -> Unit {\n\
                       let o: Option[Int] = Some(1);\n\
                       let p: Option[Int] = o.clone();\n\
                       assert_eq(p.unwrap_or(0), 1);\n\
                   }\n";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
        expr_types.extend(crate::mvl::checker::check(&prog).expr_types);
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let compiler = WasmTextCompiler::new();
        compiler.emit_program(&tir, "test");
        let first = compiler.stubbed_fns();
        compiler.emit_program(&tir, "test");
        assert_eq!(compiler.stubbed_fns(), first, "stub list accumulated");
    }

    #[test]
    fn undeclared_local_ref_finds_only_undeclared_names() {
        let declared: std::collections::HashSet<&str> = ["x", "acc"].into_iter().collect();
        assert_eq!(
            undeclared_local_ref("    local.get $x\n    local.get $acc\n", &declared),
            None
        );
        assert_eq!(
            undeclared_local_ref("    local.get $x\n    local.get $k\n", &declared),
            Some("k")
        );
        // `local.set` / `local.tee` count too.
        assert_eq!(
            undeclared_local_ref("    local.tee $nope\n", &declared),
            Some("nope")
        );
        // A String local is declared as a `_ptr`/`_len` pair under its base name.
        let s: std::collections::HashSet<&str> = ["msg"].into_iter().collect();
        assert_eq!(
            undeclared_local_ref("    local.get $msg_ptr\n    local.get $msg_len\n", &s),
            None
        );
        // Non-local instructions are ignored rather than terminating the scan.
        assert_eq!(
            undeclared_local_ref(
                "    i64.const 1\n    call $f\n    local.get $k\n",
                &declared
            ),
            Some("k")
        );
    }
}

/// Regression tests for the #2014 review findings.
///
/// Every test here asserts on a *validated module*, not on substrings of the
/// emitted text. That distinction is the point: each bug below produced WAT
/// that `wasm-tools parse` accepted and `wasm-tools validate` rejected, so a
/// `wat.contains(..)` assertion could not have caught any of them.
#[cfg(test)]
mod validated_module_tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn emit(src: &str) -> (String, Vec<String>) {
        let (mut p, errs) = Parser::new(src);
        assert!(errs.is_empty(), "lex errors: {errs:?}");
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        let mut expr_types = crate::mvl::checker::collect_prelude_expr_types(&[]);
        expr_types.extend(crate::mvl::checker::check(&prog).expr_types);
        let all_fns = crate::mvl::passes::mono::collect_fns([&prog]);
        let mono = crate::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = crate::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let c = WasmTextCompiler::new();
        let wat = c.emit_program(&tir, "test");
        (wat, c.stubbed_fns())
    }

    /// Assemble and type-validate. Panics with the offending WAT on failure.
    fn validate(wat: &str) {
        let bytes = match wat::parse_str(wat) {
            Ok(b) => b,
            Err(e) => panic!("failed to assemble emitted WAT: {e}\n--- WAT ---\n{wat}"),
        };
        if let Err(e) = wasmparser::Validator::new().validate_all(&bytes) {
            panic!("emitted module failed validation: {e}\n--- WAT ---\n{wat}");
        }
    }

    fn emit_and_validate(src: &str) -> Vec<String> {
        let (wat, stubbed) = emit(src);
        validate(&wat);
        stubbed
    }

    /// A user-declared HOF, so these tests do not depend on `std/lists.mvl`.
    const MYMAP: &str = "pub fn List[T]::mymap[U](self, f: fn(T) -> U) -> List[U] {\n\
                             let r: ref List[U] = [];\n\
                             for x in self { r.push(f(x)) };\n\
                             r\n\
                         }\n";

    /// A literal reachable only from a generic extension method was interned by
    /// nobody, so the body was just `;; missing literal` under a
    /// `(result i32 i32)` signature — a stack-underflow module.
    #[test]
    fn literal_only_inside_generic_ext_method_is_interned() {
        let stubbed = emit_and_validate(
            "pub fn List[T]::tag_me(self) -> String { \"ONLYHERE\" }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2, 3];\n\
                 assert_eq(xs.tag_me().len(), 8);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// Same gap for a generic *plain* fn — `fns` also requires no type params.
    #[test]
    fn literal_only_inside_generic_plain_fn_is_interned() {
        let stubbed = emit_and_validate(
            "pub fn label_of[T](x: T) -> String { \"PLAINONLY\" }\n\
             test fn t() -> Unit { assert_eq(label_of(1).len(), 9); }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// A non-generic extension method's body was never scanned for
    /// instantiations, so it emitted a `call` to a symbol nobody defined.
    #[test]
    fn ext_method_body_seeds_generic_instantiation() {
        let stubbed = emit_and_validate(&format!(
            "{MYMAP}\
             type Widget = struct {{ items: List[Int] }}\n\
             pub fn Widget::doubled(self) -> List[Int] {{ self.items.mymap(|x: Int| x * 2) }}\n\
             test fn t() -> Unit {{\n\
                 let w: Widget = Widget {{ items: [1, 2, 3] }};\n\
                 assert_eq(w.doubled().len(), 3);\n\
             }}\n"
        ));
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// A generic call nested inside a lambda *body* was invisible to
    /// instantiation collection, which never recursed into `Lambda`.
    #[test]
    fn generic_call_nested_in_lambda_body_is_instantiated() {
        let stubbed = emit_and_validate(&format!(
            "{MYMAP}\
             pub fn List[T]::second_len(self) -> Int {{ self.len() }}\n\
             test fn t() -> Unit {{\n\
                 let xss: List[List[Int]] = [[1, 2], [3, 4]];\n\
                 let ls: List[Int] = xss.mymap(|row: List[Int]| row.second_len());\n\
                 assert_eq(ls.len(), 2);\n\
             }}\n"
        ));
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// One generic fn calling another bound `T` only from inside `List[T]`,
    /// which the old bare-`Ty::Named` inference could not see — it mangled to
    /// `__Unknown`.
    #[test]
    fn generic_fn_calling_generic_fn_resolves_type_params() {
        let (wat, stubbed) = emit(
            "fn inner_len[T](xs: List[T]) -> Int { xs.len() }\n\
             fn outer_len[T](xs: List[T]) -> Int { inner_len(xs) }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2, 3];\n\
                 assert_eq(outer_len(xs), 3);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            !wat.contains("Unknown"),
            "an unresolved type param leaked into a mangled name: {wat}"
        );
    }

    /// A lambda inside a generic body is compiled once per instantiation. Keying
    /// its table slot on the span alone gave `T → String` the `T → Int` body.
    #[test]
    fn lambda_in_generic_body_gets_a_slot_per_instantiation() {
        let stubbed = emit_and_validate(&format!(
            "{MYMAP}\
             pub fn List[T]::tag_each(self) -> List[Int] {{ self.mymap(|x: T| 1) }}\n\
             test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1, 2, 3];\n\
                 let ss: List[String] = [\"a\", \"b\"];\n\
                 assert_eq(xs.tag_each().len(), 3);\n\
                 assert_eq(ss.tag_each().len(), 2);\n\
             }}\n"
        ));
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// `for s in strings` inside a monomorphized body needs a `__for_ms_*`
    /// unpack temp that the ctx-blind locals pass could not know about.
    #[test]
    fn string_element_for_loop_in_generic_body_declares_its_temp() {
        let stubbed = emit_and_validate(&format!(
            "{MYMAP}\
             test fn t() -> Unit {{\n\
                 let ss: List[String] = [\"aa\", \"b\"];\n\
                 let ls: List[Int] = ss.mymap(|s: String| s.len());\n\
                 assert_eq(ls.len(), 2);\n\
             }}\n"
        ));
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// A `fn(String) -> Int` lambda is `(param i32 i32)`, but `indirect_sig`
    /// asked `wasm_ty`, which has no String arm and defaulted to i64. WASM
    /// checks `call_indirect` types dynamically, so this trapped at runtime
    /// rather than failing validation.
    #[test]
    fn string_typed_lambda_signature_uses_ptr_len_pair() {
        let (wat, stubbed) = emit(&format!(
            "{MYMAP}\
             test fn t() -> Unit {{\n\
                 let ss: List[String] = [\"aa\", \"b\"];\n\
                 let ls: List[Int] = ss.mymap(|s: String| s.len());\n\
                 assert_eq(ls.len(), 2);\n\
             }}\n"
        ));
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        // The lambda body takes (env, ptr, len) — env first (#2118), then the
        // (ptr, len) pair — so its `(type)` must too. The old i64 form passed
        // validation and trapped only when `call_indirect` ran.
        assert!(
            wat.contains("(func (param i32) (param i32) (param i32) (result i64))"),
            "String lambda signature must be (env, ptr, len): {wat}"
        );
        assert!(
            !wat.contains("(func (param i64) (result i64))"),
            "String param must not collapse to i64: {wat}"
        );
    }

    /// `mangle_ty_tag` collapsed six constructors to `"Unknown"`, and
    /// instantiation dedup keys on the mangled name — so two different
    /// substitutions shared one emitted body and one call site won.
    #[test]
    fn mangle_ty_tag_distinguishes_collection_constructors() {
        let tags = [
            mangle_ty_tag(&Ty::Set(Box::new(Ty::Int))),
            mangle_ty_tag(&Ty::Map(Box::new(Ty::String), Box::new(Ty::Int))),
            mangle_ty_tag(&Ty::Result(Box::new(Ty::Int), Box::new(Ty::String))),
            mangle_ty_tag(&Ty::Unit),
            mangle_ty_tag(&Ty::List(Box::new(Ty::Int))),
            mangle_ty_tag(&Ty::Option(Box::new(Ty::Int))),
        ];
        let unique: std::collections::HashSet<&String> = tags.iter().collect();
        assert_eq!(unique.len(), tags.len(), "tags collide: {tags:?}");
        assert!(
            !tags.iter().any(|t| t == "Unknown"),
            "a known constructor still tags as Unknown: {tags:?}"
        );
    }

    /// `.slice()` on a `List[String]` byte-copies element *pointers* without a
    /// refcount bump, so parent and slice both drop each string. Must stub
    /// rather than miscompile ownership.
    /// `.slice()` is the builtin `take`/`skip` are written over, so it is what
    /// the guard has to gate. Called directly here — this harness does not load
    /// `std/lists.mvl`, so `take` itself is not in scope.
    #[test]
    fn slice_on_string_list_stubs_instead_of_double_freeing() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let xs: List[String] = [\"a\", \"b\", \"c\"];\n\
                 let ys: List[String] = xs.slice(0, 2);\n\
                 assert_eq(ys.len(), 2);\n\
             }\n",
        );
        validate(&wat);
        assert_eq!(stubbed, vec!["t".to_string()], "the caller must stub");
        assert!(
            !wat.contains("call $_mvl_array_slice"),
            "a String-element slice must not be lowered: {wat}"
        );
    }

    /// Scalar slicing is a complete copy and must keep working.
    #[test]
    fn slice_on_int_list_still_lowers() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2, 3, 4];\n\
                 let ys: List[Int] = xs.slice(0, 2);\n\
                 assert_eq(ys.len(), 2);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $_mvl_array_slice"));
    }

    /// An `ensures`/`requires` predicate using a bitwise op (#1928) — e.g.
    /// `ensures result.bit_and(15) == result` — fell through both
    /// `emit_ref_val_wasm` and `emit_ref_expr_wasm`'s `_` fallback arm, which
    /// call each other on the *same unhandled node* forever: a stack overflow
    /// at codegen time, not a validation failure or a stub. Found via
    /// `examples/data_integrity/verify.mvl`, whose FIPS 140-3 contracts are
    /// built entirely on `bit_and` (#2086).
    #[test]
    fn ensures_with_bitwise_op_lowers_without_infinite_recursion() {
        let (wat, stubbed) = emit(
            "total fn small_value() -> Int\n\
                 ensures result.bit_and(15) == result\n\
             {\n\
                 5\n\
             }\n\
             test fn t() -> Unit {\n\
                 assert_eq(small_value(), 5);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("i64.and"), "{wat}");
    }

    /// Same bug class, the `Compare` typing half: `ref_expr_wasm_ty` fell to
    /// its `_ => "i32"` default for a `BitwiseOp`/`BitwiseNot` operand, so
    /// `self.bit_and(15) == self` picked `i32.eq` for two i64 values on the
    /// stack — a `wasm-tools validate` type mismatch even after the
    /// recursion was fixed (#2086).
    #[test]
    fn ensures_with_bitwise_not_lowers_and_types_correctly() {
        let (wat, stubbed) = emit(
            "total fn flip_low_bits() -> Int\n\
                 ensures result.bit_not() == -1\n\
             {\n\
                 0\n\
             }\n\
             test fn t() -> Unit {\n\
                 assert_eq(flip_low_bits(), 0);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("i64.xor"), "{wat}");
        assert!(
            wat.contains("i64.eq"),
            "Compare over a bitwise operand must use i64.eq, not i32.eq: {wat}"
        );
    }

    /// `from_int(n)` narrows an i64 Int to an i32 Byte slot. Emitting
    /// `call $from_int` — a symbol nothing declares — made every module using
    /// it unloadable (`examples/bzip`).
    #[test]
    fn from_int_lowers_inline_without_a_call() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let b: Byte = from_int(65);\n\
                 assert_eq(b, from_int(65));\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            !wat.contains("call $from_int"),
            "from_int must lower inline, not call an undeclared symbol: {wat}"
        );
        assert!(wat.contains("i32.wrap_i64"), "{wat}");
    }

    /// `Box::new(x)` heap-allocates a slot so a recursive enum payload is
    /// finite-sized. Two separate bugs: the missing `$Box::new` symbol, and
    /// `Box[T]` falling through `wasm_ty`'s i64 default so the i32 pointer was
    /// stored into an 8-byte payload slot without the widen.
    #[test]
    fn box_new_allocates_and_is_pointer_typed() {
        let (wat, stubbed) = emit(
            "type Tree = enum {\n\
                 Leaf(Int),\n\
                 Node(Int, Box[Tree]),\n\
             }\n\
             test fn t() -> Unit {\n\
                 let leaf: Tree = Tree::Leaf(1);\n\
                 let node: Tree = Tree::Node(2, Box::new(leaf));\n\
                 match node {\n\
                     Tree::Node(w, _) => assert_eq(w, 2),\n\
                     Tree::Leaf(v) => assert_eq(v, 0),\n\
                 }\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            !wat.contains("call $Box::new"),
            "Box::new must route to the runtime shim: {wat}"
        );
        assert!(wat.contains("call $_mvl_box_new"), "{wat}");
    }

    /// `Box::new`'s old is32-vs-not store branching hardcoded `i64.store` for
    /// every non-i32 payload, including `Float` — but a Float pushes an `f64`
    /// value, so `i64.store` is a stack type mismatch. `Box[Float]` never
    /// appeared in the corpus, so this was untested and would have failed
    /// `wasm-tools validate` the first time someone boxed a float.
    #[test]
    fn box_new_stores_float_payload_with_f64_store() {
        let (wat, stubbed) = emit(
            "type FBox = enum {\n\
                 Leaf(Int),\n\
                 Wrap(Box[Float]),\n\
             }\n\
             test fn t() -> Unit {\n\
                 let w: FBox = FBox::Wrap(Box::new(3.5));\n\
                 match w {\n\
                     FBox::Wrap(_) => assert_eq(true, true),\n\
                     FBox::Leaf(v) => assert_eq(v, 0),\n\
                 }\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            wat.contains("f64.store"),
            "Box[Float] must store via f64.store, not i64.store: {wat}"
        );
    }

    /// Same bug, the String case: a String rvalue pushes `(ptr, len)`, not a
    /// single value, so the old code's bare `i64.store` after `emit_expr`
    /// left a mismatched stack shape. `Box[String]` must collapse through
    /// `_mvl_string_new` first, same as `emit_payload_store` does.
    #[test]
    fn box_new_stores_string_payload_via_mvl_string_new() {
        let (wat, stubbed) = emit(
            "type SBox = enum {\n\
                 Leaf(Int),\n\
                 Wrap(Box[String]),\n\
             }\n\
             test fn t() -> Unit {\n\
                 let w: SBox = SBox::Wrap(Box::new(\"hi\"));\n\
                 match w {\n\
                     SBox::Wrap(_) => assert_eq(true, true),\n\
                     SBox::Leaf(v) => assert_eq(v, 0),\n\
                 }\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            wat.contains("call $_mvl_string_new"),
            "Box[String] must collapse (ptr, len) via _mvl_string_new: {wat}"
        );
    }

    /// `std.env`'s `args()` is a `builtin fn` the other backends implement;
    /// WASM emitted a bare `call $args`. Blocked three examples.
    #[test]
    fn env_args_routes_to_the_runtime_shim() {
        let (wat, stubbed) = emit(
            "use std.env.{args}\n\
             test fn t() -> Unit {\n\
                 let a: List[String] = args();\n\
                 assert_eq(a.len() >= 0, true);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(!wat.contains("call $args"), "bare `call $args`: {wat}");
        assert!(wat.contains("call $_mvl_env_args"), "{wat}");
    }

    /// The env `get` shim matches std.env's *shape* — one String argument and
    /// an `Option` result — not the bare name, so a user-defined `get` is left
    /// alone. Only that half is unit-testable: this harness loads an empty
    /// prelude, so a real `std.env.get` call has no resolved `Option` type
    /// here. The positive case is covered end-to-end by `examples/log_to_file`.
    #[test]
    fn user_defined_get_is_not_hijacked_by_the_env_shim() {
        let (wat, stubbed) = emit(
            "fn get(s: String) -> Int { s.len() }\n\
             test fn t() -> Unit { assert_eq(get(\"ab\"), 2); }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            !wat.contains("call $_mvl_env_get"),
            "user-defined `get` was hijacked by the env shim: {wat}"
        );
        assert!(
            wat.contains("call $get"),
            "user `get` must still be called: {wat}"
        );
    }

    /// The old shape guard checked "one String argument and *some* Option
    /// result" — `option_inner_ty(&expr.ty).is_some()` accepts any `Option[T]`,
    /// not just `Option[String]`. `std.env.get`'s actual signature is
    /// `(String) -> Option[Tainted[String]]`, so a user's
    /// `fn get(s: String) -> Option[Int]` matched the old guard just as well
    /// and would have been silently routed to `_mvl_env_get` — a worse bug
    /// than the non-Option case above, since it's silent wrong behavior
    /// rather than a build failure.
    #[test]
    fn user_defined_get_returning_option_int_is_not_hijacked() {
        let (wat, stubbed) = emit(
            "fn get(s: String) -> Option[Int] { Some(s.len()) }\n\
             test fn t() -> Unit {\n\
                 match get(\"ab\") {\n\
                     Some(v) => assert_eq(v, 2),\n\
                     None => assert_eq(true, false),\n\
                 }\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            !wat.contains("call $_mvl_env_get"),
            "user-defined `get` returning Option[Int] was hijacked by the env shim: {wat}"
        );
        assert!(
            wat.contains("call $get"),
            "user `get` must still be called: {wat}"
        );
    }

    /// The marker is the stub trigger; the five scan sites and every producer
    /// must agree on its spelling.
    #[test]
    fn unsupported_marker_spelling_is_pinned() {
        assert_eq!(UNSUPPORTED_MARKER, ";; unsupported");
    }

    /// #2112: a `let` binding, fn param, or struct field typed as a named
    /// alias to `String` (`type BoundedInput = String where ...`) used to
    /// declare a single scalar `i64` local/param/store instead of split
    /// `(_ptr, _len)` i32s — `peels_to_string` is ctx-free and can't see
    /// through `Ty::Named` without a `ctx.type_aliases` lookup, while the
    /// emit side (`is_string_ty`) already resolved it correctly, so the two
    /// sides disagreed on shape and the module failed to validate.
    #[test]
    fn string_type_alias_let_binding_param_and_field_use_split_locals() {
        let (wat, stubbed) = emit(
            "pub type BoundedInput = String where len(self) <= 4096\n\
             pub type SqlQuery = struct {\n\
                 template: BoundedInput,\n\
                 param_count: Int,\n\
             }\n\
             pub total fn build_sql_query(template: BoundedInput, param_count: Int) -> SqlQuery {\n\
                 SqlQuery { template: template, param_count: param_count }\n\
             }\n\
             test fn t() -> Unit {\n\
                 let tmpl: BoundedInput = \"SELECT COUNT(*) FROM users\";\n\
                 let q: SqlQuery = build_sql_query(tmpl, 0);\n\
                 assert_eq(q.param_count, 0);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("(local $tmpl_ptr i32)"), "{wat}");
        assert!(wat.contains("(local $tmpl_len i32)"), "{wat}");
        assert!(
            wat.contains("(param $template_ptr i32) (param $template_len i32)"),
            "{wat}"
        );
    }

    /// #2113: `emit_match_impl` unconditionally cached the scrutinee in a
    /// single `$temp` local via one `local.set`. For a String scrutinee
    /// that isn't already a bare, pre-split `Var`, `emit_expr` leaves two
    /// i32s (ptr, len) on the stack — the single `local.set` only consumed
    /// one, and the declared local was the wrong shape besides (a scalar,
    /// not a split ptr/len pair). Nested `match` on a String bound from an
    /// outer arm (not a `let`) is exactly this shape: `r.category` is
    /// `Option[String]`, so the inner match's scrutinee `c` is a fresh
    /// value each time, not a variable already carrying split locals.
    #[test]
    fn nested_match_on_string_bound_from_outer_arm_uses_split_scrutinee_locals() {
        let (wat, stubbed) = emit(
            "fn classify(x: Option[String]) -> Int {\n\
                 match x {\n\
                     Some(c) => match c { \"high-value\" => 1, _ => 0 },\n\
                     None => 0,\n\
                 }\n\
             }\n\
             test fn t() -> Unit {\n\
                 assert_eq(classify(Some(\"high-value\")), 1);\n\
                 assert_eq(classify(Some(\"other\")), 0);\n\
                 assert_eq(classify(None), 0);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $_mvl_string_eq"), "{wat}");
    }

    /// #2114: `List[T]::concat` had no native emission arm at all — it's a
    /// `builtin fn` in `std/lists.mvl` with no body to monomorphize, so
    /// every call fell straight to the `;; unsupported expr` catch-all and
    /// stubbed the whole calling function, regardless of element type.
    #[test]
    fn concat_on_int_list_lowers() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2];\n\
                 let ys: List[Int] = xs.concat([3, 4]);\n\
                 assert_eq(ys.len(), 4);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $_mvl_array_concat"), "{wat}");
    }

    /// Struct-element lists are the case that actually motivated #2114
    /// (`task_pipeline`'s `enrich_high_value`/`parser.mvl` both concat
    /// `List[Record]`). Unlike `slice`/`clone`, struct/payload-enum pointers
    /// are not excluded here — `_mvl_struct_alloc` never frees its
    /// allocations (#1821), so aliasing them via a byte-wise copy carries
    /// no double-free risk, and `local_drop_fn` already treats every
    /// non-String collection (struct-element lists included) as a shallow
    /// `_mvl_array_drop`.
    #[test]
    fn concat_on_struct_list_lowers() {
        let (wat, stubbed) = emit(
            "type Point = struct { x: Int, y: Int }\n\
             test fn t() -> Unit {\n\
                 let ps: List[Point] = [Point { x: 1, y: 2 }];\n\
                 let qs: List[Point] = ps.concat([Point { x: 3, y: 4 }]);\n\
                 assert_eq(qs.len(), 2);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $_mvl_array_concat"), "{wat}");
    }

    /// `List[String]::concat` must still stub — `_mvl_array_concat` copies
    /// `*MvlString` pointers byte-wise without bumping their refcount, and
    /// both the original and the concatenated list would then double-drop
    /// each shared string, same reasoning as `slice_on_string_list_stubs_
    /// instead_of_double_freeing`.
    #[test]
    fn concat_on_string_list_stubs_instead_of_double_freeing() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let xs: List[String] = [\"a\", \"b\"];\n\
                 let ys: List[String] = xs.concat([\"c\"]);\n\
                 assert_eq(ys.len(), 3);\n\
             }\n",
        );
        validate(&wat);
        assert_eq!(stubbed, vec!["t".to_string()], "the caller must stub");
        assert!(
            !wat.contains("call $_mvl_array_concat"),
            "a String-element concat must not be lowered: {wat}"
        );
    }

    /// Reassigning a `ref String` local (`out = out.concat(x)`) used to emit
    /// a bare `local.set $out` regardless of type. `out` itself was never
    /// declared — its originating `let` split it into `out_ptr`/`out_len` —
    /// so this didn't just store the wrong shape, it referenced a name
    /// wasm-tools can't find at all, invalidating the whole module (found
    /// chasing why `examples/csv_transactions` couldn't build under
    /// `--backend=wasm`: `escape_quotes` in `std/csv.mvl` does exactly this).
    #[test]
    fn ref_string_reassignment_uses_split_locals() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let out: ref String = \"\";\n\
                 out = out.concat(\"a\");\n\
                 out = out.concat(\"b\");\n\
                 assert_eq(out, \"ab\");\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("local.set $out_ptr"), "{wat}");
        assert!(wat.contains("local.set $out_len"), "{wat}");
        assert!(!wat.contains("local.set $out\n"), "{wat}");
    }

    /// #2125: `Option[T]::is_some`/`::is_none` and `Result[T, E]::is_ok`/
    /// `::is_err` stubbed despite each having a trivial, correct `match
    /// self { ... }` body. Root cause was `is_struct_method_call`'s use of
    /// `named_type_name` (`Ty::Named` only) instead of `receiver_type_name`
    /// (which also resolves `List`/`Set`/`Map`/`Option`/`Result`): every
    /// non-generic pure-MVL method on a builtin wrapper type — not just
    /// structs, despite the name — is emitted via `emit_extension_method`
    /// (`${receiver_type}_${method}`, e.g. `$Option_is_some`), but no call
    /// site could ever find it, so the emitted body was dead and every call
    /// fell to the final `;; unsupported expr` catch-all.
    ///
    /// This harness's `emit()` doesn't load the real `std/core.mvl` prelude,
    /// so the method bodies are declared inline — verbatim copies of
    /// `std/core.mvl`'s own `Option[T]::is_some`/`::is_none`, exercising the
    /// exact same dispatch path a real program hits.
    #[test]
    fn option_is_some_and_is_none_lower_via_extension_method_dispatch() {
        let (wat, stubbed) = emit(
            "pub fn Option[T]::is_some(self) -> Bool {\n\
                 match self { Some(_) => true, None => false }\n\
             }\n\
             pub fn Option[T]::is_none(self) -> Bool {\n\
                 match self { Some(_) => false, None => true }\n\
             }\n\
             test fn t() -> Unit {\n\
                 let x: Option[Int] = Some(5);\n\
                 let y: Option[Int] = None;\n\
                 assert_eq(x.is_some(), true);\n\
                 assert_eq(y.is_none(), true);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $Option_is_some"), "{wat}");
        assert!(wat.contains("call $Option_is_none"), "{wat}");
    }

    #[test]
    fn result_is_ok_and_is_err_lower_via_extension_method_dispatch() {
        let (wat, stubbed) = emit(
            "pub fn Result[T, E]::is_ok(self) -> Bool {\n\
                 match self { Ok(_) => true, Err(_) => false }\n\
             }\n\
             pub fn Result[T, E]::is_err(self) -> Bool {\n\
                 match self { Ok(_) => false, Err(_) => true }\n\
             }\n\
             fn div(a: Int, b: Int) -> Result[Int, String] {\n\
                 if b == 0 { Err(\"div by zero\") } else { Ok(a / b) }\n\
             }\n\
             test fn t() -> Unit {\n\
                 assert_eq(div(4, 2).is_ok(), true);\n\
                 assert_eq(div(4, 0).is_err(), true);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $Result_is_ok"), "{wat}");
        assert!(wat.contains("call $Result_is_err"), "{wat}");
    }

    /// Same *bug class* (dead extension-method body, unreachable call site)
    /// via the `is_struct_method_call` fix specifically, not the
    /// `unify_ty_params`/`resolve_ty_param` one above: `List[T]::take`/
    /// `::skip` declare no type params of their own, so — unlike
    /// `Option[T]::is_some` — they land in the `ext_methods` bucket, not
    /// `generic_ext_methods`, and were unreachable for a narrower reason
    /// (`named_type_name` not resolving `Ty::List`) despite having nothing
    /// to do with #2125's `Option`/`Result` framing. Bodies are verbatim
    /// copies of `std/lists.mvl`'s own `take`/`skip`, which just call the
    /// native `slice` builtin.
    #[test]
    fn list_take_and_skip_lower_via_extension_method_dispatch() {
        let (wat, stubbed) = emit(
            "pub fn List[T]::take(self, n: Int) -> List[T] {\n\
                 self.slice(0, n)\n\
             }\n\
             pub fn List[T]::skip(self, n: Int) -> List[T] {\n\
                 self.slice(n, self.len())\n\
             }\n\
             test fn t() -> Unit {\n\
                 let xs: List[Int] = [1, 2, 3, 4];\n\
                 assert_eq(xs.take(2).len(), 2);\n\
                 assert_eq(xs.skip(2).len(), 2);\n\
             }\n",
        );
        validate(&wat);
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(wat.contains("call $List_take"), "{wat}");
        assert!(wat.contains("call $List_skip"), "{wat}");
    }

    /// `Set[T].remove(val)` never had a by-value removal primitive before
    /// #2124 (neither `List` nor `Set`); this pins the new
    /// `_mvl_array_remove_value_i64` native arm, including the no-op case.
    #[test]
    fn set_remove_by_value_lowers() {
        let stubbed = emit_and_validate(
            "test fn t() -> Unit {\n\
                 let s: ref Set[Int] = {1, 2, 3};\n\
                 s.remove(2);\n\
                 assert_eq(s.len(), 2);\n\
                 assert_eq(s.contains(2), false);\n\
                 assert_eq(s.contains(1), true);\n\
                 s.remove(99);\n\
                 assert_eq(s.len(), 2);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// `Set[T].map(f: fn(T) -> U) -> Set[U]` (#2124) is a native arm, not a
    /// pure MVL body — `Set[U]::new()` isn't a callable symbol in any
    /// backend, so the accumulator can't be built in MVL source the way
    /// `filter`/`fold`/`any`/`all` clone `self` in place.
    #[test]
    fn set_map_lowers_and_dedups() {
        let stubbed = emit_and_validate(
            "test fn t() -> Unit {\n\
                 let s: Set[Int] = {1, 2, 3};\n\
                 let doubled: Set[Int] = s.map(|x: Int| x * 2);\n\
                 assert_eq(doubled.len(), 3);\n\
                 assert_eq(doubled.contains(2), true);\n\
                 assert_eq(doubled.contains(4), true);\n\
                 assert_eq(doubled.contains(6), true);\n\
                 assert_eq(s.len(), 3);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// `Set[T].map` must still stub for a String-returning mapper — the
    /// output element is a `*MvlString`, and a dedup-checking
    /// `_mvl_array_insert_i32` compares pointers, not string content, so a
    /// native arm here would silently produce wrong dedup semantics AND
    /// alias the pointee without bumping its refcount (same reasoning as
    /// `concat_on_string_list_stubs_instead_of_double_freeing`).
    #[test]
    fn set_map_to_string_stubs_instead_of_wrong_dedup() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let s: Set[Int] = {1, 2, 3};\n\
                 let strs: Set[String] = s.map(|x: Int| x.to_string());\n\
                 assert_eq(strs.len(), 3);\n\
             }\n",
        );
        assert_eq!(stubbed, vec!["t"], "{wat}");
    }

    /// `Set[T]::map` claiming `emitter_handles_method_natively` must not also
    /// claim `List[T]::map` — they share the `collection_elem_ty` shape
    /// guard, but `List::map` still has to route through generic dispatch
    /// to monomorphize its real `std/lists.mvl` body (#2124).
    #[test]
    fn list_map_still_routes_through_generic_dispatch() {
        const LIST_MAP: &str = "pub fn List[T]::map[U](self, f: fn(T) -> U) -> List[U] {\n\
                                     let result: ref List[U] = [];\n\
                                     for x in self { result.push(f(x)) };\n\
                                     result\n\
                                 }\n";
        let stubbed = emit_and_validate(&format!(
            "{LIST_MAP}\
             test fn t() -> Unit {{\n\
                 let xs: List[Int] = [1, 2, 3];\n\
                 let ys: List[Int] = xs.map(|x: Int| x * 2);\n\
                 assert_eq(ys.len(), 3);\n\
             }}\n"
        ));
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// `let alias: ref Set[Int] = original;` followed by a mutation through
    /// `alias` must not corrupt `original` — `std/config.mvl`'s `merge` and
    /// `std/pbt.mvl`'s shrink loop both rely on exactly this shape (`let out:
    /// ref Map/List[T] = base;`) expecting `base` untouched (#2124). A bare
    /// `local.set` aliases the same heap array; mutating through `alias` then
    /// silently mutates `original` too.
    #[test]
    fn ref_let_binding_from_existing_var_deep_copies_not_aliases() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let original: Set[Int] = {1, 2, 3};\n\
                 let alias: ref Set[Int] = original;\n\
                 alias.insert(99);\n\
                 assert_eq(original.len(), 3);\n\
                 assert_eq(alias.len(), 4);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        assert!(
            wat.contains("call $_mvl_array_slice"),
            "expected a deep copy via _mvl_array_slice: {wat}"
        );
        assert!(
            !wat.contains("call $_mvl_array_clone"),
            "refcount-bump clone still aliases the same buffer, not a fix: {wat}"
        );
    }

    /// `let x: Set[Int] = {1, 2, 3};` (a fresh literal) must keep its plain
    /// `local.set` — no deep copy needed since the literal's buffer is
    /// already unique. Guards against `is_let_deep_copy_shape` over-firing.
    #[test]
    fn let_binding_from_fresh_literal_does_not_deep_copy() {
        let stubbed = emit_and_validate(
            "test fn t() -> Unit {\n\
                 let s: ref Set[Int] = {1, 2, 3};\n\
                 s.insert(4);\n\
                 assert_eq(s.len(), 4);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// `Ok(())` — a Unit-payload `Result` — used to stub the whole enclosing
    /// function: `emit_expr`'s literal-dispatch match had its own duplicated,
    /// out-of-sync copy of `emit_literal`'s per-variant logic, missing the
    /// `Literal::Unit` (and `Literal::Char`) arms entirely, so the inner `()`
    /// fell through to the catch-all `;; unsupported expr` (#2144).
    #[test]
    fn ok_unit_construction_lowers_instead_of_stubbing() {
        let stubbed = emit_and_validate(
            "fn produce() -> Result[Unit, String] {\n\
                 Ok(())\n\
             }\n\
             test fn t() -> Unit {\n\
                 let r: Result[Unit, String] = produce();\n\
                 let ok: Bool = match r { Ok(_) => true, Err(_) => false };\n\
                 assert_eq(ok, true);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// Same gap on the `Option` side — `Some(())`.
    #[test]
    fn some_unit_construction_lowers_instead_of_stubbing() {
        let stubbed = emit_and_validate(
            "fn produce_opt() -> Option[Unit] {\n\
                 Some(())\n\
             }\n\
             test fn t() -> Unit {\n\
                 let o: Option[Unit] = produce_opt();\n\
                 let some: Bool = match o { Some(_) => true, None => false };\n\
                 assert_eq(some, true);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// The other variant `emit_expr`'s duplicated literal match was missing
    /// entirely — a `char` literal (`'x'`) — same root cause as the two
    /// tests above (#2144).
    #[test]
    fn char_literal_lowers_instead_of_stubbing() {
        let stubbed = emit_and_validate(
            "fn first_char() -> Char {\n\
                 'x'\n\
             }\n\
             test fn t() -> Unit {\n\
                 assert_eq(first_char(), 'x');\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// #2149: `Option[T]::map`/`Result[T,E]::map`/`Result[T,E]::and_then`
    /// had no native arm and no `std/core.mvl` body — every call stubbed
    /// the enclosing function. Adding the bodies alone wasn't enough:
    /// `Result::map`/`::and_then`'s `Err(e) => Err(e)` passthrough arm hit
    /// a separate checker bug (`bind_match_pattern` not recognizing a
    /// generic method's named-wrapper `self` type), binding `e` as
    /// `Ty::Unknown` and picking the wrong runtime constructor for any
    /// non-`Int` `E` — an enum here, confirmed to matter (a plain `Int` E
    /// would have masked it, since `Ty::Unknown` happens to default to the
    /// same i64 shape `Int` needs).
    #[test]
    fn option_map_and_result_map_and_then_lower_and_run_correctly() {
        let stubbed = emit_and_validate(
            "type MyError = enum { NonPositive, WasErr }\n\
             pub fn Option[T]::map[U](self, f: fn(T) -> U) -> Option[U] {\n\
                 match self { Some(x) => Some(f(x)), None => None }\n\
             }\n\
             pub fn Result[T, E]::map[U](self, f: fn(T) -> U) -> Result[U, E] {\n\
                 match self { Ok(x) => Ok(f(x)), Err(e) => Err(e) }\n\
             }\n\
             pub fn Result[T, E]::and_then[U](self, f: fn(T) -> Result[U, E]) -> Result[U, E] {\n\
                 match self { Ok(x) => f(x), Err(e) => Err(e) }\n\
             }\n\
             test fn t() -> Unit {\n\
                 let o: Option[Int] = Some(5);\n\
                 let d: Option[Int] = o.map(|x: Int| x * 2);\n\
                 match d { Some(v) => assert_eq(v, 10), None => assert_eq(1, 0) }\n\
                 let n: Option[Int] = None;\n\
                 let dn: Option[Int] = n.map(|x: Int| x * 2);\n\
                 match dn { Some(_) => assert_eq(1, 0), None => assert_eq(1, 1) }\n\
                 let ok: Result[Int, MyError] = Ok(21);\n\
                 let mapped: Result[Int, MyError] = ok.map(|x: Int| x * 2);\n\
                 match mapped { Ok(v) => assert_eq(v, 42), Err(_) => assert_eq(1, 0) }\n\
                 let err: Result[Int, MyError] = Err(MyError::WasErr);\n\
                 let mapped_err: Result[Int, MyError] = err.map(|x: Int| x * 2);\n\
                 match mapped_err {\n\
                     Ok(_) => assert_eq(1, 0),\n\
                     Err(MyError::WasErr) => assert_eq(1, 1),\n\
                     Err(MyError::NonPositive) => assert_eq(1, 0),\n\
                 }\n\
                 let chained: Result[Int, MyError] = ok.and_then(|x: Int| \n\
                     if x > 0 { Ok(x * 2) } else { Err(MyError::NonPositive) }\n\
                 );\n\
                 match chained { Ok(v) => assert_eq(v, 42), Err(_) => assert_eq(1, 0) }\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
    }

    /// #2153: `mvl test --backend=wasm` compiles each `test fn` standalone
    /// via `wasmtime run --invoke`, with no synthesized `fn main`. The
    /// literal `(data ...)` sections, `$mvl_alloc`/`$heap`, and
    /// `$mvl_int_to_string`/`$mvl_println`/etc. were only ever emitted when
    /// `needs_wasi` (has a `fn main`) was true — silently correct for every
    /// other test path (`mvlr` synthesizes a wrapping `main`), but wrong
    /// here: string literals got assigned offsets with *no data written
    /// there*, so a comparison against two different literals could
    /// spuriously match (comparing uninitialized memory to itself).
    #[test]
    fn string_literal_data_section_present_without_main() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 let a: String = \"hello\";\n\
                 let b: String = \"world\";\n\
                 assert_eq(a == b, false);\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        validate(&wat);
        assert!(
            wat.contains("(data (i32.const"),
            "no-main module must still write its string literals' data: {wat}"
        );
    }

    /// Same root cause (#2153), the `Int::to_string()` half: the helper
    /// itself lives outside `runtime/wasm/` (a small inline WAT function,
    /// `$mvl_int_to_string`) and was gated behind the same wrong condition
    /// — a no-`main` module calling `.to_string()` on an `Int` referenced an
    /// undefined function, failing to assemble at all.
    #[test]
    fn int_to_string_helper_defined_without_main() {
        let (wat, stubbed) = emit(
            "fn stringify(x: Int) -> String { x.to_string() }\n\
             test fn t() -> Unit {\n\
                 assert_eq(stringify(42), \"42\");\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        validate(&wat);
        assert!(
            wat.contains("(func $mvl_int_to_string"),
            "no-main module calling Int::to_string() must still define the helper: {wat}"
        );
    }

    /// Same root cause (#2153) again, `Bool::to_string()`'s half: "true"/
    /// "false" are pre-seeded literals `collect_literals` only added when
    /// `needs_wasi` was true, so a no-`main` module's `Bool::to_string()`
    /// looked up an unseeded literal and silently fell back to `(0, 0)` —
    /// an empty string instead of "true"/"false".
    #[test]
    fn bool_to_string_literals_seeded_without_main() {
        let (wat, stubbed) = emit(
            "test fn t() -> Unit {\n\
                 assert_eq(true.to_string(), \"true\");\n\
                 assert_eq(false.to_string(), \"false\");\n\
             }\n",
        );
        assert!(stubbed.is_empty(), "unexpected stubs: {stubbed:?}");
        validate(&wat);
    }
}
