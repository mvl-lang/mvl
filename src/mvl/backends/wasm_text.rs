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
//! - `println(s)` (via WASI `fd_write` + newline iovec)
//! - `fn main() -> Unit ! Console` becomes the WASI `_start` export
//!
//! Everything else (strings beyond `Int.to_string`, control flow, structs,
//! actors, refcounting, other effects, other host imports) is deliberately
//! out of scope.

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
        // When present, we also emit the WASI runtime blob (memory, fd_write
        // import, bump allocator, int-to-string, println).
        let needs_wasi = fns
            .iter()
            .any(|f| f.name == "main" && matches!(f.ret_ty, Ty::Unit));

        let mut out = String::from("(module\n");
        if needs_wasi {
            out.push_str(WASI_RUNTIME);
        }

        for f in &fns {
            emit_fn(&mut out, f, needs_wasi);
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

fn emit_fn(out: &mut String, f: &TirFn, needs_wasi: bool) {
    let (wasm_name, _) = effective_name(f, needs_wasi);
    out.push_str(&format!("  (func ${wasm_name}"));
    for p in &f.params {
        out.push_str(&format!(" (param ${} {})", p.name, wasm_ty(&p.ty)));
    }
    if !matches!(f.ret_ty, Ty::Unit) {
        out.push_str(&format!(" (result {})", wasm_ty(&f.ret_ty)));
    }
    out.push('\n');
    emit_block(out, &f.body);
    out.push_str("  )\n");
}

fn emit_block(out: &mut String, block: &TirBlock) {
    for stmt in &block.stmts {
        emit_stmt(out, stmt);
    }
}

fn emit_stmt(out: &mut String, stmt: &TirStmt) {
    match stmt {
        TirStmt::Expr { expr, .. } => emit_expr(out, expr),
        TirStmt::Return { value: Some(e), .. } => {
            emit_expr(out, e);
            out.push_str("    return\n");
        }
        _ => {
            out.push_str(&format!("    ;; unsupported stmt: {stmt:?}\n"));
        }
    }
}

fn emit_expr(out: &mut String, expr: &TirExpr) {
    match &expr.kind {
        TirExprKind::Literal(Literal::Integer(n)) => {
            out.push_str(&format!("    i64.const {n}\n"));
        }
        TirExprKind::Var(name) => {
            out.push_str(&format!("    local.get ${name}\n"));
        }
        TirExprKind::Binary { op, left, right } => {
            emit_expr(out, left);
            emit_expr(out, right);
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
            // Route `println` through the WASI helper. The runtime blob's
            // `$mvl_println` takes (ptr, len) which is exactly what
            // `Int.to_string()` leaves on the stack.
            if name == "println" {
                for a in args {
                    emit_expr(out, a);
                }
                out.push_str("    call $mvl_println\n");
                return;
            }
            for a in args {
                emit_expr(out, a);
            }
            out.push_str(&format!("    call ${name}\n"));
        }
        TirExprKind::MethodCall {
            receiver, method, ..
        } if method == "to_string" && matches!(receiver.ty, Ty::Int) => {
            emit_expr(out, receiver);
            out.push_str("    call $mvl_int_to_string\n");
        }
        TirExprKind::Block(block) => emit_block(out, block),
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

/// WASI preview 1 runtime shims. Emitted verbatim when a Unit-returning
/// `main` is present. Provides:
///
/// - `fd_write` import from `wasi_snapshot_preview1`
/// - linear memory + export
/// - bump allocator (`$mvl_alloc`)
/// - `$mvl_int_to_string` — i64 → decimal ASCII, returns (ptr, len)
/// - `$mvl_println` — takes (ptr, len), writes with trailing newline
///
/// Memory layout:
/// - offset 0..8: iovec[0] {ptr, len}
/// - offset 8..16: iovec[1] {ptr, len} (points at the newline)
/// - offset 16..20: nwritten output slot
/// - offset 20: static "\n" byte
/// - offset 32..: bump-allocated string storage
const WASI_RUNTIME: &str = r#"  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory 1)
  (export "memory" (memory 0))
  (data (i32.const 20) "\n")
  (global $heap (mut i32) (i32.const 32))
  (func $mvl_alloc (param $n i32) (result i32)
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
