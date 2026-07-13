// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `WasmTextCompiler` — minimal TIR → WebAssembly Text emitter (#1571).
//!
//! Spike scope: emit WAT for the two programs in
//! `tests/spikes/006-wasm-backend/` (`add.mvl` and `hello.mvl`). Enough to
//! validate the end-to-end pipeline; deliberately not enough for anything
//! larger. Extending beyond this requires the ADR decisions listed in the
//! epic (string ABI, allocator, effect → import table).
//!
//! Supported today:
//! - `Int → i64` (all other primitives punted to i64)
//! - integer literals, parameters, direct calls, arithmetic
//! - trailing block expression as return value
//! - `Int.to_string()` (via inline bump-allocated i64 → decimal helper)
//! - **String literals** — collected up front, placed in a data section,
//!   pushed on the WASM stack as `(ptr, len)` i32 pairs.
//! - `println(s)` (via WASI `fd_write` + newline iovec)
//! - `fn main() -> Unit ! Console` becomes the WASI `_start` export
//!
//! Everything else (control flow, structs, actors, refcounting, other
//! effects, other host imports) is deliberately out of scope.

use std::collections::HashMap;

use super::Backend;
use crate::mvl::checker::types::Ty;
use crate::mvl::ir::{TirBlock, TirExpr, TirExprKind, TirFn, TirProgram, TirStmt};
use crate::mvl::parser::ast::{BinaryOp, Literal};

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
/// spike grows (or shrinks).
struct Ctx<'a> {
    needs_wasi: bool,
    /// Interned string literals: content → (linear-memory offset, byte length).
    literals: &'a HashMap<String, (u32, u32)>,
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

        let (literals, heap_start) = collect_literals(&fns);
        let ctx = Ctx {
            needs_wasi,
            literals: &literals,
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
fn effective_name<'a>(f: &'a TirFn, needs_wasi: bool) -> (&'a str, &'a str) {
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
    emit_block(out, &f.body, ctx);
    out.push_str("  )\n");
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
        TirExprKind::Literal(Literal::Str(s)) => {
            // String literals are placed in the module's data section during
            // collect_literals; here we just push (offset, len) as i32s.
            // Missing entries are a bug — every Str seen at emit time should
            // have been recorded during the pre-scan.
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
        TirExprKind::Binary { op, left, right } => {
            emit_expr(out, left, ctx);
            emit_expr(out, right, ctx);
            let opcode = match op {
                BinaryOp::Add => "i64.add",
                BinaryOp::Sub => "i64.sub",
                BinaryOp::Mul => "i64.mul",
                BinaryOp::Div => "i64.div_s",
                BinaryOp::Rem => "i64.rem_s",
                other => {
                    out.push_str(&format!("    ;; unsupported binop: {other:?}\n"));
                    return;
                }
            };
            out.push_str(&format!("    {opcode}\n"));
        }
        TirExprKind::FnCall { name, args, .. } => {
            // Route `println` through the WASI helper. `$mvl_println` takes
            // (ptr, len) which is exactly what both string literals and
            // `Int.to_string()` leave on the stack.
            if name == "println" {
                for a in args {
                    emit_expr(out, a, ctx);
                }
                out.push_str("    call $mvl_println\n");
                return;
            }
            for a in args {
                emit_expr(out, a, ctx);
            }
            out.push_str(&format!("    call ${name}\n"));
        }
        TirExprKind::MethodCall {
            receiver, method, ..
        } if method == "to_string" && matches!(receiver.ty, Ty::Int) => {
            emit_expr(out, receiver, ctx);
            out.push_str("    call $mvl_int_to_string\n");
        }
        TirExprKind::Block(block) => emit_block(out, block, ctx),
        other => {
            out.push_str(&format!("    ;; unsupported expr: {other:?}\n"));
        }
    }
}

fn wasm_ty(ty: &Ty) -> &'static str {
    match ty {
        Ty::Int => "i64",
        Ty::Bool => "i32",
        Ty::UInt => "i64",
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => wasm_ty(inner),
        _ => "i64",
    }
}

// ── String-literal collection ────────────────────────────────────────────

/// Walk every user function and intern each distinct string literal at a
/// unique linear-memory offset starting at [`LITERAL_BASE`]. Returns the
/// interning table and the first free offset after all literals — used as
/// the initial value of the runtime's `$heap` global so bump allocations
/// don't overwrite the data section.
fn collect_literals(fns: &[&TirFn]) -> (HashMap<String, (u32, u32)>, u32) {
    let mut map = HashMap::new();
    let mut next = LITERAL_BASE;
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
        TirStmt::Expr { expr, .. }
        | TirStmt::Return {
            value: Some(expr), ..
        } => collect_expr(expr, map, next),
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
"#;
