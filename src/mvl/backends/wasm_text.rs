// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `WasmTextCompiler` — TIR → WebAssembly Text emitter (#1818, epic #1817).
//!
//! Runs against `tests/corpus/` via `make test-rust-wasm` (delegated to
//! mvlr). Phase 1 scope: cover chapters 00_smoke, 01_expressions, and
//! 02_control_flow (except `match`).
//!
//! Supported today:
//! - Primitives: `Int → i64`, `Float → f64`, `Bool` / `Byte → i32`
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
//! - `if` / `else if` / `else` — both statement and expression forms
//! - `while cond { body }` — canonical WASM `block/loop/br_if` shape
//! - Early `return` (both `return expr` and bare `return`)
//! - `fn main() -> Unit ! Console` → WASI `_start` export
//!
//! Deliberately not supported (later phases of #1817):
//! - `match` — pattern compilation, punted to phase 4
//! - Structs, enums, `Option`, `Result` — phase 4
//! - Collections (`List`, `Map`, `Set`) — phase 3
//! - Higher-order fns / closures / generics — later
//! - String equality, indexing, concat — phase 2 with `runtime/wasm/`
//! - `MvlString` refcount layout, drop emission — phase 2
//! - Other WASI hostcalls, `extern "wasm"` ABI — separate ticket
//! - Actors, refinements, contracts — phase 4/5

use std::cell::Cell;
use std::collections::HashMap;

use super::Backend;
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{TirBlock, TirElseBranch, TirExpr, TirExprKind, TirFn, TirProgram, TirStmt};
use crate::mvl::parser::ast::{BinaryOp, LValue, Literal, Pattern, UnaryOp};

pub struct WasmTextCompiler;

impl WasmTextCompiler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WasmTextCompiler {
    fn default() -> Self {
        Self::new()
    }
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
    /// Monotonic counter for fresh WAT labels (`$while_0`, `$while_1`, …).
    label_counter: Cell<usize>,
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
        let ctx = Ctx {
            needs_wasi,
            literals: &literals,
            label_counter: Cell::new(0),
        };

        let mut out = String::from("(module\n");
        if needs_wasi {
            out.push_str(&emit_wasi_runtime(heap_start, &literals));
        }

        for f in &fns {
            emit_fn(&mut out, f, &ctx);
        }

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

/// Map a MVL function to its WAT symbol / export name. Unit-returning `main`
/// becomes `_start` (WASI command convention) when the WASI runtime is enabled.
fn effective_name(f: &TirFn, needs_wasi: bool) -> (&str, &str) {
    if needs_wasi && f.name == "main" && matches!(f.ret_ty, Ty::Unit) {
        ("_start", "_start")
    } else {
        (f.name.as_str(), f.name.as_str())
    }
}

fn emit_fn(out: &mut String, f: &TirFn, ctx: &Ctx) {
    let (wasm_name, _) = effective_name(f, ctx.needs_wasi);
    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty)));
    }
    if !matches!(f.ret_ty, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&f.ret_ty)));
    }
    out.push('\n');

    // WASM locals must be declared before any instruction. Pre-scan the
    // body for `let` bindings and emit `(local $name ty)` declarations
    // up front. Nested blocks (if / while) don't get their own scope in
    // WASM — corpus tests happen not to reuse names, so a flat collection
    // is fine for phase 1.
    let mut locals: Vec<(String, Ty)> = Vec::new();
    collect_locals_block(&f.body, &mut locals);
    for (name, ty) in &locals {
        out.push_str(&format!("    (local ${} {})\n", name, wasm_ty(ty)));
    }

    emit_block(out, &f.body, ctx);
    out.push_str("  )\n");
}

// ── Local collection ─────────────────────────────────────────────────────

fn collect_locals_block(block: &TirBlock, locals: &mut Vec<(String, Ty)>) {
    for s in &block.stmts {
        collect_locals_stmt(s, locals);
    }
}

fn collect_locals_stmt(stmt: &TirStmt, locals: &mut Vec<(String, Ty)>) {
    match stmt {
        TirStmt::Let {
            pattern, ty, init, ..
        } => {
            if let Pattern::Ident(name, _) = pattern {
                locals.push((name.clone(), ty.clone()));
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
        TirStmt::While { cond, body, .. } => {
            collect_locals_expr(cond, locals);
            collect_locals_block(body, locals);
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
        TirExprKind::MethodCall { receiver, args, .. } => {
            collect_locals_expr(receiver, locals);
            for a in args {
                collect_locals_expr(a, locals);
            }
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
        TirStmt::Let { pattern, init, .. } => {
            if let Pattern::Ident(name, _) = pattern {
                emit_expr(out, init, ctx);
                out.push_str(&format!("    local.set ${name}\n"));
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
        // `if cond { then } else { else_ }` — statement form (no result value).
        TirStmt::If {
            cond, then, else_, ..
        } => {
            emit_expr(out, cond, ctx);
            out.push_str("    if\n");
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
        // `while cond { body }` — canonical WASM shape:
        //   block $break_N (loop $cont_N (br_if $break_N (i32.eqz cond)) body (br $cont_N))
        TirStmt::While { cond, body, .. } => {
            let brk = ctx.fresh_label("wend");
            let cnt = ctx.fresh_label("wcont");
            out.push_str(&format!("    block ${brk}\n"));
            out.push_str(&format!("    loop ${cnt}\n"));
            emit_expr(out, cond, ctx);
            out.push_str("    i32.eqz\n");
            out.push_str(&format!("    br_if ${brk}\n"));
            emit_block(out, body, ctx);
            out.push_str(&format!("    br ${cnt}\n"));
            out.push_str("    end\n");
            out.push_str("    end\n");
        }
        _ => {
            out.push_str(&format!("    ;; unsupported stmt: {stmt:?}\n"));
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
            out.push_str(&format!("    local.get ${name}\n"));
        }
        TirExprKind::Unary { op, expr: inner } => {
            emit_unary(out, *op, inner, ctx);
        }
        TirExprKind::Binary { op, left, right } => {
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
        TirExprKind::Block(block) => emit_block(out, block, ctx),
        // `if cond { then } else { else_ }` — expression form. Both branches
        // must produce a value of `expr.ty`. WASM's block-typed `if
        // (result T)` handles this directly. `else_ = None` would give the
        // whole expr type `Unit` — treat as a no-op else.
        TirExprKind::If { cond, then, else_ } => {
            emit_expr(out, cond, ctx);
            let is_unit = matches!(expr.ty, Ty::Unit);
            if is_unit {
                out.push_str("    if\n");
            } else {
                out.push_str(&format!("    if (result {})\n", wasm_ty(&expr.ty)));
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
        other => {
            out.push_str(&format!("    ;; unsupported expr: {other:?}\n"));
        }
    }
}

/// Emit `assert_eq(a, b)` or `assert_ne(a, b)` — mirrors the LLVM backend's
/// `emit_assert_eq_builtin_tir` (#1837). Compares the two values with a
/// type-directed equality op, then traps via `unreachable` when the check
/// fails. `negate = true` traps on equality (i.e. `assert_ne`).
///
/// String equality is deliberately not handled yet — the WASM emitter carries
/// strings as bare `(ptr, len)` i32 pairs and has no `mvl_string_eq` shim.
/// Corpus tests that compare strings will fall through to a `;; unsupported`
/// comment and fail to assemble, which is honest.
fn emit_assert_eq(out: &mut String, left: &TirExpr, right: &TirExpr, negate: bool, ctx: &Ctx) {
    emit_expr(out, left, ctx);
    emit_expr(out, right, ctx);
    let eq_op = if is_float(&left.ty) {
        "f64.eq"
    } else if is_i32(&left.ty) {
        "i32.eq"
    } else {
        // Int, UInt, and anything else defaulting to i64. String comparisons
        // would land here today and silently miscompile — flagged via a
        // comment so the assembly step fails loudly.
        if matches!(&left.ty, Ty::String) {
            out.push_str("    ;; unsupported assert_eq/ne on String\n");
        }
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
    } else if is_i32(&left.ty) {
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

fn wasm_ty(ty: &Ty) -> &'static str {
    match ty {
        Ty::Int | Ty::UInt => "i64",
        Ty::Float => "f64",
        Ty::Bool | Ty::Byte => "i32",
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => wasm_ty(inner),
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

/// True if this MVL type lowers to WASM `i32` (Bool, Byte). Used to pick
/// between `i64.*` / `i32.*` / `f64.*` opcode families.
fn is_i32(ty: &Ty) -> bool {
    match ty {
        Ty::Bool | Ty::Byte => true,
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => is_i32(inner),
        _ => false,
    }
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
        TirExprKind::Block(block) => collect_block(block, map, next),
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
    let mut out = String::new();
    out.push_str(
        "  (import \"wasi_snapshot_preview1\" \"fd_write\"\n    \
         (func $fd_write (param i32 i32 i32 i32) (result i32)))\n",
    );
    out.push_str("  (memory 1)\n");
    out.push_str("  (export \"memory\" (memory 0))\n");
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
  (func $mvl_println (param $ptr i32) (param $len i32)
    (i32.store (i32.const 0) (local.get $ptr))
    (i32.store (i32.const 4) (local.get $len))
    (i32.store (i32.const 8) (i32.const 20))
    (i32.store (i32.const 12) (i32.const 1))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 2) (i32.const 16))))
  (func $mvl_eprintln (param $ptr i32) (param $len i32)
    (i32.store (i32.const 0) (local.get $ptr))
    (i32.store (i32.const 4) (local.get $len))
    (i32.store (i32.const 8) (i32.const 20))
    (i32.store (i32.const 12) (i32.const 1))
    (drop (call $fd_write (i32.const 2) (i32.const 0) (i32.const 2) (i32.const 16))))
"#;
