// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! C-ABI dispatch helpers shared by all method-call emit sites.
//!
//! Extracted from the pre-TIR `emit_method_call.rs` during #1612 Phase 3b so
//! they could be reused by the TIR-walking emitter. Each `emit_c_call_*` helper
//! consults the [`LLVM_DISPATCH`] table for the C-ABI symbol + signature; see
//! `dispatch.rs` for the table itself.

use crate::mvl::backends::llvm_text::dispatch::{self, Dispatch};

use super::TextEmitter;

/// Look up a dispatch entry, panicking with a drift-detection message on miss.
fn lookup_dispatch(method: &str) -> &'static Dispatch {
    dispatch::lookup(method).unwrap_or_else(|| {
        // AUDIT: drift detector — dispatch.rs ↔ c_call.rs (see #1549)
        panic!(
            "LLVM_DISPATCH missing entry for '{method}' — drift between dispatch.rs and c_call.rs"
        )
    })
}

/// Build the LLVM argument list for a C-ABI call with an opaque-pointer receiver.
fn build_arg_list(recv_val: &str, extra_args: &[(&'static str, &str)]) -> String {
    let mut s = format!("ptr {recv_val}");
    for (ty, v) in extra_args {
        s.push_str(&format!(", {ty} {v}"));
    }
    s
}

impl TextEmitter {
    /// Emit a Shape A builtin call: simple C-ABI runtime function with a
    /// single return register.  Reads `sym`, `signature`, and `ret_ty` from
    /// the `LLVM_DISPATCH` row for `method` (must be [`Dispatch::CCall`]).
    ///
    /// Emits:
    /// ```text
    /// declare {signature}             // via ensure_extern (deduped)
    /// {reg} = call {ret_ty} @{sym}(ptr {recv_val}{, extra_args})
    /// ```
    /// and inserts `{reg} -> ret_ty` into `reg_types`.  Returns the result
    /// register name; call sites typically wrap with `Ok(Some(reg))`.
    ///
    /// Panics on a dispatch-table miss — same drift-detection contract used
    /// across all helpers in this file.
    pub(super) fn emit_c_call_simple(
        &mut self,
        method: &str,
        recv_val: &str,
        extra_args: &[(&'static str, &str)],
    ) -> String {
        let Dispatch::CCall {
            sym,
            signature,
            ret_ty,
        } = lookup_dispatch(method)
        else {
            // AUDIT: drift detector — dispatch.rs ↔ c_call.rs (see #1549)
            panic!(
                "LLVM_DISPATCH entry for '{method}' is not Dispatch::CCall — use a different emit_c_call_* helper"
            );
        };
        self.ensure_extern(&format!("declare {signature}"));
        let arg_list = build_arg_list(recv_val, extra_args);
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = call {ret_ty} @{sym}({arg_list})"));
        self.fn_ctx
            .reg_types
            .insert(reg.clone(), ret_ty.to_string());
        reg
    }

    /// Emit a Shape B builtin call: C call returns `i64`, result is coerced
    /// to `i1` via `icmp ne i64 X, 0`.  Reads sym + signature from the
    /// `LLVM_DISPATCH` row for `method` (must be
    /// [`Dispatch::CCallBoolFromI64`]).
    ///
    /// Emits:
    /// ```text
    /// declare {signature}
    /// {raw} = call i64 @{sym}(ptr {recv_val}{, extra_args})
    /// {reg} = icmp ne i64 {raw}, 0
    /// ```
    /// and inserts `{reg} -> i1` into `reg_types`.  Returns the result
    /// register (the i1, not the raw i64).
    pub(super) fn emit_c_call_bool_from_i64(
        &mut self,
        method: &str,
        recv_val: &str,
        extra_args: &[(&'static str, &str)],
    ) -> String {
        let Dispatch::CCallBoolFromI64 { sym, signature } = lookup_dispatch(method) else {
            // AUDIT: drift detector — dispatch.rs ↔ c_call.rs (see #1549)
            panic!(
                "LLVM_DISPATCH entry for '{method}' is not Dispatch::CCallBoolFromI64 — use a different emit_c_call_* helper"
            );
        };
        self.ensure_extern(&format!("declare {signature}"));
        let arg_list = build_arg_list(recv_val, extra_args);
        let raw = self.next_reg();
        self.push_instr(&format!("{raw} = call i64 @{sym}({arg_list})"));
        let reg = self.next_reg();
        self.push_instr(&format!("{reg} = icmp ne i64 {raw}, 0"));
        self.fn_ctx.reg_types.insert(reg.clone(), "i1".to_string());
        reg
    }

    /// Emit a Shape D builtin call: C call returns a pointer to an N-slot
    /// array of pointers; emitter loads each slot and assembles a named
    /// LLVM struct via `insertvalue`.
    ///
    /// Reads sym + signature + struct_name + slot_tys from the
    /// `LLVM_DISPATCH` row for `method` (must be
    /// [`Dispatch::CCallStructFromSlots`]).
    ///
    /// Returns the register holding the assembled struct value.
    pub(super) fn emit_c_call_struct_from_slots(
        &mut self,
        method: &str,
        recv_val: &str,
        extra_args: &[(&'static str, &str)],
    ) -> String {
        let Dispatch::CCallStructFromSlots {
            sym,
            signature,
            struct_name,
            slot_tys,
        } = lookup_dispatch(method)
        else {
            // AUDIT: drift detector — dispatch.rs ↔ c_call.rs (see #1549)
            panic!(
                "LLVM_DISPATCH entry for '{method}' is not Dispatch::CCallStructFromSlots — use a different emit_c_call_* helper"
            );
        };
        assert!(
            !slot_tys.is_empty(),
            "LLVM_DISPATCH entry for '{method}' has empty slot_tys — CCallStructFromSlots requires at least one slot"
        );
        self.ensure_extern(&format!("declare {signature}"));
        let arg_list = build_arg_list(recv_val, extra_args);
        let raw = self.next_reg();
        self.push_instr(&format!("{raw} = call ptr @{sym}({arg_list})"));

        // Load each slot.
        let mut slot_vals: Vec<String> = Vec::with_capacity(slot_tys.len());
        for (i, slot_ty) in slot_tys.iter().enumerate() {
            let slot_ptr = self.next_reg();
            self.push_instr(&format!(
                "{slot_ptr} = getelementptr {slot_ty}, ptr {raw}, i64 {i}"
            ));
            let val = self.next_reg();
            self.push_instr(&format!("{val} = load {slot_ty}, ptr {slot_ptr}"));
            slot_vals.push(val);
        }

        // Assemble the struct: undef → insertvalue chain.
        let mut prev = format!("{struct_name} undef");
        let mut last_reg = String::new();
        for (i, (slot_ty, val)) in slot_tys.iter().zip(slot_vals.iter()).enumerate() {
            let next = self.next_reg();
            self.push_instr(&format!(
                "{next} = insertvalue {prev}, {slot_ty} {val}, {i}"
            ));
            prev = format!("{struct_name} {next}");
            last_reg = next;
        }
        self.fn_ctx
            .reg_types
            .insert(last_reg.clone(), struct_name.to_string());
        last_reg
    }

    /// Emit a Shape C builtin call: C call returns an `i8` discriminant and
    /// fills an out-pointer with the payload.  Result is wrapped as
    /// `Option[T]` via [`wrap_result_pair`].  Reads sym + signature +
    /// payload_ty from the `LLVM_DISPATCH` row for `method` (must be
    /// [`Dispatch::CCallOptionOutPtr`]).
    ///
    /// The out-pointer is alloca'd by the helper; `extra_args` should NOT
    /// include it.  The helper appends `, ptr {out}` to the call's argument
    /// list automatically.
    pub(super) fn emit_c_call_option_out_ptr(
        &mut self,
        method: &str,
        recv_val: &str,
        extra_args: &[(&'static str, &str)],
    ) -> String {
        let Dispatch::CCallOptionOutPtr {
            sym,
            signature,
            payload_ty,
        } = lookup_dispatch(method)
        else {
            // AUDIT: drift detector — dispatch.rs ↔ c_call.rs (see #1549)
            panic!(
                "LLVM_DISPATCH entry for '{method}' is not Dispatch::CCallOptionOutPtr — use a different emit_c_call_* helper"
            );
        };
        self.ensure_extern(&format!("declare {signature}"));
        let out = self.next_reg();
        self.push_instr(&format!("{out} = alloca {payload_ty}"));
        let mut arg_list = build_arg_list(recv_val, extra_args);
        arg_list.push_str(&format!(", ptr {out}"));
        let tag = self.next_reg();
        self.push_instr(&format!("{tag} = call i8 @{sym}({arg_list})"));
        let payload = self.next_reg();
        self.push_instr(&format!("{payload} = load {payload_ty}, ptr {out}"));
        let slot = self.next_reg();
        self.push_instr(&format!("{slot} = alloca {payload_ty}"));
        self.push_instr(&format!("store {payload_ty} {payload}, ptr {slot}"));
        self.wrap_result_pair(&tag, &slot)
    }
}
