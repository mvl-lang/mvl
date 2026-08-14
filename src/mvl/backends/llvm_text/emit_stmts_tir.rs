// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Statement and block emission for the TIR-walking path (#1612, Phase 3b PR 1).
//!
//! Parallel to `emit_stmts.rs`. Walks [`TirBlock`] and [`TirStmt`].
//!
//! TIR statements always carry their `span` inline; some variants (e.g. `Let`)
//! also carry the fully-resolved declared `Ty` so the emitter doesn't need to
//! re-infer types from initializers.

use crate::mvl::ir::{
    LValue, LetKind, Pattern, TirBlock, TirElseBranch, TirExpr, TirExprKind, TirMatchBody, TirStmt,
    Ty,
};

use super::emit_helpers::ty_to_type_expr;
use super::{HeapKind, RefLocal, TextEmitter, MAIN_RET};

impl TextEmitter {
    /// TIR variant of [`Self::exclude_returned_value`] — walks a `TirExpr`.
    ///
    /// Removes the heap-local entry for a value about to be returned (moved
    /// out of the function), so the subsequent `emit_heap_drops` does not
    /// free what now belongs to the caller. Matches the AST-side rules in
    /// `emit_types.rs::exclude_returned_value`: only `Var` is the canonical
    /// owning expression; `Consume` / `Relabel` are transparent wrappers.
    ///
    /// Blanks the matching entry's SSA name in place rather than physically
    /// removing it (`Vec::retain`) — a physical removal shifts every later
    /// index down, which silently invalidates any `heap_snapshot` a scope
    /// above captured as a plain `heap_locals.len()` before this call ran:
    /// an entry pushed *after* that snapshot could shift below it, so
    /// `drop_scope_locals`'s `drain(snapshot..)` would then skip it entirely
    /// and it would leak all the way to the function's `emit_heap_drops` —
    /// an SSA register that only dominates one arm being read from a block
    /// that doesn't dominate it ("Instruction does not dominate all uses",
    /// #2169). Blanking preserves every other entry's index.
    pub(super) fn exclude_returned_value_tir(&mut self, expr: &TirExpr) {
        match &expr.kind {
            TirExprKind::Var(name) => {
                if let Some(loc) = self.fn_ctx.ref_locals.get(name) {
                    let ptr = loc.ptr.clone();
                    for entry in self.fn_ctx.heap_locals.iter_mut() {
                        if entry.0 == ptr {
                            entry.0.clear();
                        }
                    }
                    return;
                }
                if let Some(ssa) = self.fn_ctx.locals.get(name) {
                    let ssa = ssa.clone();
                    for entry in self.fn_ctx.heap_locals.iter_mut() {
                        if entry.0 == ssa {
                            entry.0.clear();
                        }
                    }
                }
            }
            TirExprKind::Consume(inner) | TirExprKind::Relabel { expr: inner, .. } => {
                self.exclude_returned_value_tir(inner);
            }
            // #2264: a struct/enum-payload literal returned as a function's
            // tail expression (e.g. `BwtResult { data: last, primary: primary }`)
            // moves each field's value into the constructed value — but this
            // case wasn't handled at all, so none of those locals were
            // excluded from the blanket `emit_heap_drops()` sweep that runs
            // right after. `examples/bzip/bwt.mvl::bwt_encode` returns
            // `BwtResult { data: last, .. }`: `last` got returned as the
            // struct's `data` field *and* dropped by the same sweep — a
            // use-after-free on the return value itself, corrupting it
            // before the caller ever reads it. Same shape as the
            // already-handled push/Some/Ok/Err/list-literal "moved into a
            // container" sites — a struct/enum constructor is just another
            // container.
            TirExprKind::Construct { fields, .. } | TirExprKind::Spawn { fields, .. } => {
                for (_, value) in fields {
                    self.exclude_returned_value_tir(value);
                }
            }
            // #2286: a `match`/`if` used *as* the returned value yields one of
            // its arms' values through a phi, so each arm's tail is an escape
            // candidate — but neither shape was handled here, so an arm that
            // returns an owned local had that local dropped by the very sweep
            // this function exists to suppress:
            //
            //   fn opt_or(opt: Option[String], default: String) -> String {
            //       let d: String = consume(default);
            //       match opt { Some(s) => s, None => d }
            //   }
            //
            // emitted `phi ptr [ %t2, some ], [ %default, none ]` followed by
            // `_mvl_string_drop(ptr %default)` before the `ret` — on the None
            // path the returned string was freed on the way out, and the
            // caller's first use of it (`"x".concat(cat)`) read freed memory.
            //
            // Excluding *every* arm's value is deliberate: which arm runs is
            // a runtime property. The arms not taken were never separately
            // dropped anyway (their values are unreachable), so the cost of
            // over-excluding is at worst a leak, never a double free — the
            // same trade `Construct` above already makes for its fields.
            TirExprKind::Match { arms, .. } => {
                for arm in arms {
                    match &arm.body {
                        TirMatchBody::Expr(e) => self.exclude_returned_value_tir(e),
                        TirMatchBody::Block(b) => self.exclude_block_tail_value_tir(b),
                    }
                }
            }
            TirExprKind::If { then, else_, .. } => {
                self.exclude_block_tail_value_tir(then);
                if let Some(e) = else_ {
                    self.exclude_returned_value_tir(e);
                }
            }
            _ => {}
        }
    }

    /// [`Self::exclude_returned_value_tir`] applied to a block's own trailing
    /// expression — the value a match arm's or `if` branch's block yields
    /// into the enclosing phi (#2286).
    fn exclude_block_tail_value_tir(&mut self, block: &TirBlock) {
        if let Some(TirStmt::Expr { expr, .. }) = block.stmts.last() {
            self.exclude_returned_value_tir(expr);
        }
    }

    /// Walk a [`TirBlock`] and emit the trailing expression's SSA register
    /// (mirrors `emit_block(&Block)` semantics).
    pub(super) fn emit_block_tir(&mut self, block: &TirBlock) -> Result<Option<String>, String> {
        self.emit_block_tir_typed(block, None)
    }

    /// [`Self::emit_block_tir`], additionally threading the *expected* type
    /// of the block's trailing value (when the caller knows it) down into a
    /// bare tail-position `if`/`else` statement's phi construction.
    ///
    /// Needed because a bare literal branch value (e.g. a `Char` literal,
    /// emitted as a raw numeric string with no `%` register prefix) gives
    /// `infer_val_type` no way to tell an `i32`-shaped `Char` from an
    /// `i64`-shaped `Int` — it silently defaults to `i64`, producing a
    /// `phi i64` that gets returned into an `i32`-declared slot: invalid IR
    /// that crashes `lli` at module load (#2146). `TirStmt`/`TirBlock` carry
    /// no `.ty` of their own (unlike `TirExpr`), so this has to be threaded
    /// in from a caller that *does* know the expected type — e.g. a
    /// function's own `ret_ty` when this is the function's body block.
    pub(super) fn emit_block_tir_typed(
        &mut self,
        block: &TirBlock,
        expected_ty: Option<&Ty>,
    ) -> Result<Option<String>, String> {
        let stmts = &block.stmts;
        if stmts.is_empty() {
            return Ok(None);
        }
        let (head, tail) = stmts.split_at(stmts.len() - 1);
        for s in head {
            self.emit_stmt_tir(s)?;
        }
        match &tail[0] {
            // #2264: this block's tail value may become a *caller's* return
            // value (a function body, an if/else branch, a match-arm block —
            // every use of `emit_block_tir_typed`) — if it's a struct-field
            // read (`owned.bytes`), it needs the same clone-not-exclude
            // treatment as a `FieldAccess` used as a call argument or
            // explicit `return` value (see `resolve_owned_call_arg` and the
            // `TirStmt::Return` handling below): there's no single tracked
            // local `exclude_returned_value_tir` can blank out for a field
            // read, since the struct it came from keeps existing and will
            // deep-drop all its own heap-typed fields regardless.
            // `examples/bzip/bitstream.mvl::flush_writer`'s
            // `if owned.bit_pos == 0 { owned.bytes } else { .. }` hits this
            // exact shape — the FieldAccess is the `then`-branch's tail,
            // not the function's own outermost tail statement.
            TirStmt::Expr { expr, .. } => {
                let val = self.emit_expr_tir(expr)?;
                if let (Some(v), true) = (
                    val.as_ref(),
                    matches!(expr.kind, TirExprKind::FieldAccess { .. }),
                ) {
                    if let Some(cloned) = self.clone_heap_value_for_ty(v, &expr.ty) {
                        return Ok(Some(cloned));
                    }
                }
                // #2264: this block's tail value escapes into the caller's
                // phi/return — exclude the owning local (`Var`/`Consume`/
                // `Relabel`/`Construct`) from *this* block's own scope-exit
                // drop sweep the same way a function body's tail statement
                // and an explicit `return` already do (see those call
                // sites). Without this, a `ref`-qualified local returned as
                // an if/else branch's bare tail `Var` is dropped by
                // `drop_scope_locals` right before its value is used as the
                // branch's phi input — `drop_scope_locals`'s `escape`
                // parameter only string-matches a *non-ref* local's own SSA
                // value, never a `ref` local's alloca, so it can't catch
                // this on its own. `examples/bzip/bitstream.mvl::
                // flush_writer`'s `else` branch (`out.push(..); out`) hits
                // this exact shape.
                self.exclude_returned_value_tir(expr);
                Ok(val)
            }
            // #2286: a tail-position `if`/`match` *statement* yields this
            // block's value through a phi, exactly like the tail-expression
            // arm above — so each branch's escaping value needs the same
            // exclusion, or the scope-exit sweep frees whichever local the
            // taken branch is about to return. `std/csv.mvl`-style
            // `match opt { Some(s) => s, None => d }` as a whole function
            // body is the common shape; before this, the `None` arm's `d`
            // was dropped immediately before the `ret` that returned it.
            TirStmt::If {
                cond, then, else_, ..
            } => {
                self.exclude_block_tail_value_tir(then);
                if let Some(TirElseBranch::Block(b)) = else_.as_ref() {
                    self.exclude_block_tail_value_tir(b);
                }
                self.emit_if_stmt_chain_tir(cond, then, else_.as_ref(), expected_ty)
            }
            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                for arm in arms {
                    match &arm.body {
                        TirMatchBody::Expr(e) => self.exclude_returned_value_tir(e),
                        TirMatchBody::Block(b) => self.exclude_block_tail_value_tir(b),
                    }
                }
                self.emit_match_expr_tir(scrutinee, arms)
            }
            other => {
                self.emit_stmt_tir(other)?;
                Ok(None)
            }
        }
    }

    /// TIR variant of [`Self::emit_if_stmt_chain`].
    ///
    /// Emits an `if`-statement that, at block-tail position, returns a phi value.
    /// Recursively follows `TirElseBranch::If` chains so deep `else if` trees
    /// emit correct IR. `expected_ty` is threaded through from
    /// [`Self::emit_block_tir_typed`] — see its doc comment (#2146).
    fn emit_if_stmt_chain_tir(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirElseBranch>,
        expected_ty: Option<&Ty>,
    ) -> Result<Option<String>, String> {
        match else_ {
            None => self.emit_if_phi_tir_from_blocks(cond, then, None, expected_ty),
            Some(TirElseBranch::Block(b)) => {
                self.emit_if_phi_tir_from_blocks(cond, then, Some(b), expected_ty)
            }
            Some(TirElseBranch::If(nested)) => {
                if let TirStmt::If {
                    cond: ncond,
                    then: nthen,
                    else_: nelse,
                    ..
                } = nested.as_ref()
                {
                    let cond_val = match self.emit_expr_tir(cond)? {
                        Some(v) => v,
                        None => return Ok(None),
                    };
                    let then_bb = self.next_bb("then");
                    let else_bb = self.next_bb("else");
                    let merge_bb = self.next_bb("merge");
                    self.push_instr(&format!(
                        "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
                    ));
                    // Branch heap_locals must not leak past merge_bb (#1617).
                    let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

                    self.start_bb(&then_bb);
                    let then_val = self.emit_block_tir_typed(then, expected_ty)?;
                    let then_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.drop_scope_locals(heap_locals_snapshot, then_val.as_deref());
                        self.push_instr(&format!("br label %{merge_bb}"));
                    } else {
                        self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
                    }

                    self.start_bb(&else_bb);
                    let else_val =
                        self.emit_if_stmt_chain_tir(ncond, nthen, nelse.as_ref(), expected_ty)?;
                    let else_end = self.fn_ctx.current_bb.clone();
                    if !self.fn_ctx.terminated {
                        self.drop_scope_locals(heap_locals_snapshot, else_val.as_deref());
                        self.push_instr(&format!("br label %{merge_bb}"));
                    } else {
                        self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
                    }

                    self.start_bb(&merge_bb);
                    match (then_val, else_val) {
                        (Some(tv), Some(ev)) => {
                            let phi_ty = expected_ty
                                .map(|ty| self.ty_to_llvm_ctx(ty))
                                .unwrap_or_else(|| self.infer_val_type(&tv));
                            let result = self.next_reg();
                            self.push_instr(&format!(
                                "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                            ));
                            self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
                            Ok(Some(result))
                        }
                        _ => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Shared helper: emit if/else with phi merging when both branches yield a
    /// value. Used by both block-tail If statements and If expressions.
    /// `result_ty_hint` — the merge point's expected type, when a caller
    /// knows it (e.g. `expr.ty` for an if-*expression*, or a function's
    /// `ret_ty` when this if is the function body's tail statement) — types
    /// the `phi` directly instead of guessing from one branch's emitted
    /// value text, which can't tell an `i32`-shaped `Char` literal from an
    /// `i64`-shaped `Int` literal (#2146). Threaded into both branch blocks
    /// too, so a *nested* tail-position if/else picks up the same hint.
    pub(super) fn emit_if_phi_tir_from_blocks(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirBlock>,
        result_ty_hint: Option<&Ty>,
    ) -> Result<Option<String>, String> {
        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(None),
        };

        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        // Branch heap_locals must not leak past merge_bb (#1617).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.start_bb(&then_bb);
        let then_val = self.emit_block_tir_typed(then, result_ty_hint)?;
        let then_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, then_val.as_deref());
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&else_bb);
        let else_val = if let Some(b) = else_ {
            self.emit_block_tir_typed(b, result_ty_hint)?
        } else {
            None
        };
        let else_end = self.fn_ctx.current_bb.clone();
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, else_val.as_deref());
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&merge_bb);

        match (then_val, else_val) {
            (Some(tv), Some(ev)) => {
                let phi_ty = result_ty_hint
                    .map(|ty| self.ty_to_llvm_ctx(ty))
                    .unwrap_or_else(|| self.infer_val_type(&tv));
                let result = self.next_reg();
                self.push_instr(&format!(
                    "{result} = phi {phi_ty} [ {tv}, %{then_end} ], [ {ev}, %{else_end} ]"
                ));
                self.fn_ctx.reg_types.insert(result.clone(), phi_ty);
                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    /// Walk a [`TirStmt`] for side effects (no value returned).
    ///
    /// Mirror of `emit_stmt(&Stmt)`. Unimplemented variants return an error;
    /// the `cross_backend_tir` test target tolerates these while the walker is
    /// being built out.
    pub(super) fn emit_stmt_tir(&mut self, stmt: &TirStmt) -> Result<(), String> {
        match stmt {
            TirStmt::Expr { expr, .. } => {
                self.emit_expr_tir(expr)?;
                Ok(())
            }

            TirStmt::Let {
                kind,
                pattern,
                ty,
                init,
                ..
            } => {
                if *kind == LetKind::Ghost {
                    return Ok(());
                }
                // #2264: an *empty* list/set literal's own `.ty` is never
                // resolved by the checker (`List[Unknown]`, not e.g.
                // `List[Byte]`) — there's no element to infer it from. The
                // generic `emit_expr_tir(init)` path then falls back to a
                // hardcoded "ptr" (8-byte) element size regardless of the
                // declared type, so `let result: ref List[Byte] = [];`
                // built an array with `elem_size == 8` instead of `1`.
                // Every later `.push()` copied 8 bytes from a 1-byte source
                // per that wrong metadata (UB, though accidentally
                // byte-correct at read time), and the array's `elem_size`
                // silently disagreed with any same-typed `List[Byte]` built
                // from a non-empty literal — breaking content equality
                // (`_mvl_array_eq`'s `elem_size` check) despite identical
                // bytes. This `let` statement's own declared `ty` (unlike
                // the literal's) IS fully resolved, so use it directly for
                // the empty case instead of going through `emit_expr_tir`.
                let mut declared_ty = ty;
                while let Ty::Ref(_, inner) = declared_ty {
                    declared_ty = inner;
                }
                let empty_hint = match declared_ty {
                    Ty::List(e) | Ty::Array(e, _) | Ty::Set(e) => Some(self.ty_to_llvm_ctx(e)),
                    _ => None,
                };
                let val = match &init.kind {
                    crate::mvl::ir::TirExprKind::List { elems } if elems.is_empty() => {
                        self.emit_list_literal_tir(elems, empty_hint.as_deref())?
                    }
                    crate::mvl::ir::TirExprKind::Set { elems } if elems.is_empty() => {
                        // Dedup is a no-op on zero elements — safe to skip.
                        self.emit_list_literal_tir(elems, empty_hint.as_deref())?
                    }
                    _ => self.emit_expr_tir(init)?,
                };
                // Convert TIR `Ty` once at the boundary; the rest reuses the
                // existing AST-shaped helpers (deref_ty, is_mutable_ref, …).
                let ty_te = ty_to_type_expr(ty).unwrap_or_else(|| {
                    // Fallback — shouldn't happen for any user-facing Ty variants.
                    crate::mvl::ir::TypeExpr::Base {
                        name: "Unit".into(),
                        args: Vec::new(),
                        span: crate::mvl::parser::lexer::Span::default(),
                    }
                });
                let elem_ty = Self::deref_ty(&ty_te).clone();

                if Self::is_mutable_ref(&ty_te) {
                    let ty_str = self.llvm_ty_ctx(&elem_ty);
                    if ty_str == "void" {
                        return Ok(());
                    }
                    let ptr = self.next_reg();
                    // Hoist to entry block when the binding is inside a branch BB
                    // so the alloca dominates all uses including cross-arm drops (#1645).
                    // Loop bodies manage their own heap scope via heap_locals snapshots
                    // so their allocas don't need hoisting — emit inline instead.
                    let bb = &self.fn_ctx.current_bb;
                    let in_loop_body = bb.starts_with("loop_body")
                        || bb.starts_with("for_body")
                        || bb.starts_with("for_list_body");
                    if bb == "entry" || in_loop_body {
                        self.push_instr(&format!("{ptr} = alloca {ty_str}"));
                    } else {
                        self.fn_ctx
                            .pre_allocas
                            .push(format!("  {ptr} = alloca {ty_str}"));
                    }
                    if let Some(v) = val {
                        // `let alias: ref List/Set/Array[T] = <existing var/expr>;`
                        // (T scalar — `HeapKind::Array`) — a bare pointer store
                        // aliases the same heap `MvlArray` as the source. Two
                        // owning locals then both decrement its refcount at
                        // scope exit: a double-free. Worse, since neither copy
                        // observes the other's mutations as independent, this
                        // also violates MVL's array value semantics. A fresh
                        // literal init (`List { .. }`/`Set { .. }`) already
                        // owns a unique buffer, so only non-literal inits need
                        // the deep copy (#2124).
                        let needs_deep_copy =
                            matches!(Self::heap_kind(&elem_ty), Some(HeapKind::Array))
                                && !matches!(
                                    init.kind,
                                    TirExprKind::List { .. }
                                        | TirExprKind::Set { .. }
                                        | TirExprKind::Map { .. }
                                );
                        if needs_deep_copy {
                            self.ensure_extern("declare ptr @_mvl_array_deep_clone(ptr)");
                            let cloned = self.next_reg();
                            self.push_instr(&format!(
                                "{cloned} = call ptr @_mvl_array_deep_clone(ptr {v})"
                            ));
                            self.push_instr(&format!("store {ty_str} {cloned}, ptr {ptr}"));
                        } else {
                            self.push_instr(&format!("store {ty_str} {v}, ptr {ptr}"));
                            // #2265: no deep copy was made, so this binding
                            // now *aliases* the initializer's heap object —
                            // and the alloca below gets its own
                            // `heap_locals` entry. Without releasing the
                            // initializer's own entry, the same allocation
                            // is tracked twice and the scope-exit sweep
                            // drops it twice; worse, when the binding is
                            // the function's return value, the source's
                            // stale entry frees it *before* the caller ever
                            // reads it. `examples/bzip/huffman.mvl::
                            // remove_at_ll` is exactly this shape:
                            //
                            //   let before: List[List[Int]] = list.slice(..);
                            //   let result: ref List[List[Int]] = before;
                            //   result.extend(after);
                            //   result            // returns freed memory
                            //
                            // Ownership moves into the new binding, same as
                            // the `Assign` arm below already does for
                            // `result = out` (#2260) and as push/Some/Ok/
                            // Err/list-literal/struct-literal do for a
                            // value moved into a container (#1991/#2264).
                            // The `needs_deep_copy` branch above must NOT
                            // do this: it built an independent copy, so the
                            // source keeps its own drop.
                            self.exclude_returned_value_tir(init);
                        }
                    }
                    if let Pattern::Ident(name, _) = pattern {
                        if let Some(hk) = Self::heap_kind(&elem_ty) {
                            self.fn_ctx.heap_locals.push((ptr.clone(), hk, true));
                        }
                        // Shadow any same-named plain binding — see the
                        // mirrored `ref_locals.remove` in the non-ref arm
                        // below for why both maps have to be kept in sync
                        // (#2265).
                        self.fn_ctx.locals.remove(name);
                        self.fn_ctx.ref_locals.insert(
                            name.clone(),
                            RefLocal {
                                ptr,
                                elem_ty: elem_ty.clone(),
                            },
                        );
                    }
                } else if let (Some(v), Pattern::Ident(name, _)) = (val, pattern) {
                    if !self.fn_ctx.reg_types.contains_key(&v) {
                        let ty_str = self.llvm_ty_ctx(&elem_ty);
                        self.fn_ctx.reg_types.insert(v.clone(), ty_str);
                    }
                    if let Some(old_ssa) = self.fn_ctx.locals.get(name) {
                        let old_ssa = old_ssa.clone();
                        // Blank in place, don't physically remove — see the
                        // comment on `exclude_returned_value_tir` (#2169):
                        // a positional removal here would shift every later
                        // heap_locals index down, invalidating any
                        // `heap_snapshot` an enclosing scope already took.
                        for entry in self.fn_ctx.heap_locals.iter_mut() {
                            if entry.0 == old_ssa {
                                entry.0.clear();
                            }
                        }
                    }
                    // A same-named `ref` binding must stop resolving here
                    // (#2265). `emit_expr_tir`'s `Var` arm consults
                    // `ref_locals` *before* `locals`, and neither map is
                    // scoped to the block that introduced its entry — so a
                    // `ref` binding left over from an already-finished
                    // sibling branch silently captured every later mention
                    // of that name, including in branches that declared
                    // their own plain local. `examples/bzip/huffman.mvl::
                    // build_tree` has exactly this shape:
                    //
                    //   } else if init.queue.len() == 1 {
                    //       let codes: ref List[List[Int]] = ...;   // alloca
                    //   } else {
                    //       let codes: List[List[Int]] = ...;       // %ssa
                    //       BuildState { .., codes: codes }         // read the *alloca*
                    //
                    // The else branch's `codes` compiled to a load from the
                    // then-branch's alloca — never stored to on this path —
                    // so `BuildState.codes` got uninitialized stack memory,
                    // and its own freshly-mapped list was dropped unused.
                    // The garbage pointer then reached `_mvl_array_clone`/
                    // `_mvl_array_len` as a misaligned dereference.
                    self.fn_ctx.ref_locals.remove(name);
                    self.fn_ctx.locals.insert(name.clone(), v.clone());
                    if let Some(hk) = Self::heap_kind(&elem_ty) {
                        if !self.fn_ctx.heap_locals.iter().any(|(s, _, _)| s == &v) {
                            self.fn_ctx.heap_locals.push((v, hk, false));
                        }
                    }
                    self.fn_ctx.local_mvl_types.insert(name.clone(), elem_ty);
                }
                Ok(())
            }

            TirStmt::Assign { target, value, .. } => {
                let val = self.emit_expr_tir(value)?;
                match target {
                    LValue::Ident(name, _) => {
                        if let Some(loc) = self.fn_ctx.ref_locals.get(name).cloned() {
                            if let Some(v) = val {
                                let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                                self.push_instr(&format!("store {ty_str} {v}, ptr {}", loc.ptr));
                                // #2260: `result = out` moves `out`'s value
                                // into `result` — exclude `out` from
                                // `heap_locals` the same way `push`/`Some`/
                                // `Ok`/`Err`/`return` already do (#1994),
                                // otherwise `out`'s own end-of-scope drop
                                // frees the allocation `result` now points
                                // to too, and the next loop iteration reads
                                // freed memory through `result`.
                                self.exclude_returned_value_tir(value);
                            }
                        }
                    }
                    // `self.field = value` in an actor behavior body. The actor
                    // emitter binds each state field as a ref_local GEP into
                    // `%self`, so the store goes straight through that pointer —
                    // the mirror of the read path in `emit_field_access_tir`.
                    // Without this the assignment was silently dropped and actor
                    // state never changed (#2012).
                    LValue::Field { base, field, .. } => {
                        if let LValue::Ident(base_name, _) = base.as_ref() {
                            if base_name == "self" {
                                if let Some(loc) = self.fn_ctx.ref_locals.get(field).cloned() {
                                    if let Some(v) = val {
                                        let ty_str = self.llvm_ty_ctx(&loc.elem_ty);
                                        self.push_instr(&format!(
                                            "store {ty_str} {v}, ptr {}",
                                            loc.ptr
                                        ));
                                    }
                                    return Ok(());
                                }
                            }
                        }
                        return Err(format!(
                            "unsupported assignment target: field '{field}' on a non-self base"
                        ));
                    }
                }
                Ok(())
            }

            TirStmt::Return { value, .. } => {
                let ret_ty = self.fn_ctx.current_ret_ty.clone();
                let mut ret_val = if let Some(expr) = value {
                    self.emit_expr_tir(expr)?
                } else {
                    None
                };
                if let Some(expr) = value {
                    // #2264: an early `return owned.bytes`-shaped FieldAccess
                    // needs the same clone-not-exclude fix as the
                    // tail-position case in emit_program_tir.rs — see that
                    // call site's comment for why.
                    if let (Some(v), true) = (
                        ret_val.as_ref(),
                        matches!(expr.kind, TirExprKind::FieldAccess { .. }),
                    ) {
                        if let Some(cloned) = self.clone_heap_value_for_ty(v, &expr.ty) {
                            ret_val = Some(cloned);
                        }
                    }
                    self.exclude_returned_value_tir(expr);
                }
                self.emit_heap_drops();
                if Self::is_void(&ret_ty) {
                    if self.fn_ctx.current_fn_is_main {
                        self.push_instr(MAIN_RET);
                    } else {
                        self.push_instr("ret void");
                    }
                } else if let Some(v) = ret_val {
                    let ty = self.llvm_ty_ctx(&ret_ty);
                    self.push_instr(&format!("ret {ty} {v}"));
                } else if self.fn_ctx.current_fn_is_main {
                    self.push_instr(MAIN_RET);
                } else {
                    self.push_instr("ret void");
                }
                self.fn_ctx.terminated = true;
                Ok(())
            }

            TirStmt::If {
                cond, then, else_, ..
            } => {
                self.emit_if_stmt_void_tir(cond, then, else_.as_ref())?;
                Ok(())
            }

            TirStmt::While { cond, body, .. } => self.emit_while_tir(cond, body),

            TirStmt::For {
                pattern,
                iter,
                body,
                ..
            } => self.emit_for_stmt_tir(pattern, iter, body),

            TirStmt::Match {
                scrutinee, arms, ..
            } => {
                self.emit_match_expr_tir(scrutinee, arms)?;
                Ok(())
            }
        }
    }

    /// TIR variant of [`Self::emit_if_stmt`] — if-as-statement at non-tail
    /// position (no value returned, no phi).
    fn emit_if_stmt_void_tir(
        &mut self,
        cond: &TirExpr,
        then: &TirBlock,
        else_: Option<&TirElseBranch>,
    ) -> Result<(), String> {
        let then_bb = self.next_bb("then");
        let else_bb = self.next_bb("else");
        let merge_bb = self.next_bb("merge");

        let cond_val = match self.emit_expr_tir(cond)? {
            Some(v) => v,
            None => return Ok(()),
        };
        self.push_instr(&format!(
            "br i1 {cond_val}, label %{then_bb}, label %{else_bb}"
        ));

        // Branch heap_locals must not leak past merge_bb — see emit_stmts.rs
        // (`emit_if_stmt`) and #1617. Without the snapshot/drop discipline the
        // function-end drop pass would emit `_mvl_string_drop(%v)` against an
        // SSA value that is only defined in the then-block, violating LLVM
        // dominance when the else-branch reaches the merge.
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.start_bb(&then_bb);
        self.emit_block_tir(then)?;
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&else_bb);
        if let Some(e) = else_ {
            match e {
                TirElseBranch::Block(b) => {
                    self.emit_block_tir(b)?;
                }
                TirElseBranch::If(nested) => {
                    self.emit_stmt_tir(nested)?;
                }
            }
        }
        if !self.fn_ctx.terminated {
            self.drop_scope_locals(heap_locals_snapshot, None);
            self.push_instr(&format!("br label %{merge_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&merge_bb);
        Ok(())
    }

    /// Emit a `for` loop.
    ///
    /// Dispatches to [`Self::emit_for_range_tir`] when the iterator is a
    /// `range(lo, hi)` FnCall; otherwise delegates to [`Self::emit_for_list_tir`].
    /// Receiver/iter types come from `iter.ty` directly.
    fn emit_for_stmt_tir(
        &mut self,
        pattern: &Pattern,
        iter: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        // `for var in range(lo, hi)` — integer range loop.
        if let crate::mvl::ir::TirExprKind::FnCall { name, args, .. } = &iter.kind {
            if name == "range" && args.len() == 2 {
                let var_name = match pattern {
                    Pattern::Ident(n, _) => n.clone(),
                    _ => "_".into(),
                };
                return self.emit_for_range_tir(&var_name, &args[0], &args[1], body);
            }
        }
        // `for var in <list-expr>` — list / array / set iteration (#1546).
        let var_name = match pattern {
            Pattern::Ident(n, _) => n.clone(),
            _ => "_".into(),
        };
        self.emit_for_list_tir(&var_name, iter, body)
    }

    /// TIR variant of [`Self::emit_for_range`].
    fn emit_for_range_tir(
        &mut self,
        var_name: &str,
        lo: &TirExpr,
        hi: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        let lo_val = match self.emit_expr_tir(lo)? {
            Some(v) => v,
            None => return Ok(()),
        };
        let hi_val = match self.emit_expr_tir(hi)? {
            Some(v) => v,
            None => return Ok(()),
        };

        let i_ptr = self.next_reg();
        self.push_instr(&format!("{i_ptr} = alloca i64"));
        self.push_instr(&format!("store i64 {lo_val}, ptr {i_ptr}"));

        let cond_bb = self.next_bb("for_cond");
        let body_bb = self.next_bb("for_body");
        let end_bb = self.next_bb("for_end");

        self.push_instr(&format!("br label %{cond_bb}"));
        self.start_bb(&cond_bb);

        let cur_i = self.next_reg();
        self.push_instr(&format!("{cur_i} = load i64, ptr {i_ptr}"));

        let cond_reg = self.next_reg();
        self.push_instr(&format!("{cond_reg} = icmp slt i64 {cur_i}, {hi_val}"));
        self.push_instr(&format!(
            "br i1 {cond_reg}, label %{body_bb}, label %{end_bb}"
        ));

        self.start_bb(&body_bb);

        let old = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), cur_i.clone());
        self.fn_ctx.reg_types.insert(cur_i.clone(), "i64".into());
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.emit_block_tir(body)?;

        if let Some(prev) = old {
            self.fn_ctx.locals.insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.locals.remove(var_name);
        }

        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{cond_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    /// Emit an over-list `for` loop.
    ///
    /// Element type comes from `iter.ty` (unwrapping `Ref` / `Labeled` /
    /// `Refined` then matching `Ty::List(e)` / `Array(e, _)` / `Set(e)`).
    fn emit_for_list_tir(
        &mut self,
        var_name: &str,
        iter: &TirExpr,
        body: &TirBlock,
    ) -> Result<(), String> {
        use crate::mvl::ir::Ty;

        let arr = match self.emit_expr_tir(iter)? {
            Some(v) => v,
            None => return Ok(()),
        };

        // Unwrap label/refinement/ref wrappers, then match List/Array/Set.
        let mut cur = &iter.ty;
        while let Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) = cur {
            cur = inner;
        }
        let (elem_ty_opt, elem_llvm_ty): (Option<Ty>, String) = match cur {
            Ty::List(e) | Ty::Array(e, _) | Ty::Set(e) => {
                ((**e).clone().into(), self.ty_to_llvm_ctx(e))
            }
            _ => (None, "i64".into()),
        };

        self.ensure_extern("declare i64 @_mvl_array_len(ptr)");
        self.ensure_extern("declare ptr @_mvl_array_get(ptr, i64)");

        let len_reg = self.next_reg();
        self.push_instr(&format!("{len_reg} = call i64 @_mvl_array_len(ptr {arr})"));

        let i_ptr = self.next_reg();
        self.push_instr(&format!("{i_ptr} = alloca i64"));
        self.push_instr(&format!("store i64 0, ptr {i_ptr}"));

        let cond_bb = self.next_bb("for_list_cond");
        let body_bb = self.next_bb("for_list_body");
        let end_bb = self.next_bb("for_list_end");

        self.push_instr(&format!("br label %{cond_bb}"));
        self.start_bb(&cond_bb);

        let cur_i = self.next_reg();
        self.push_instr(&format!("{cur_i} = load i64, ptr {i_ptr}"));
        let cond_reg = self.next_reg();
        self.push_instr(&format!("{cond_reg} = icmp slt i64 {cur_i}, {len_reg}"));
        self.push_instr(&format!(
            "br i1 {cond_reg}, label %{body_bb}, label %{end_bb}"
        ));

        self.start_bb(&body_bb);

        let elem_ptr = self.next_reg();
        self.push_instr(&format!(
            "{elem_ptr} = call ptr @_mvl_array_get(ptr {arr}, i64 {cur_i})"
        ));
        let elem_val = self.next_reg();
        self.push_instr(&format!("{elem_val} = load {elem_llvm_ty}, ptr {elem_ptr}"));
        self.fn_ctx
            .reg_types
            .insert(elem_val.clone(), elem_llvm_ty.clone());

        let old_local = self
            .fn_ctx
            .locals
            .insert(var_name.to_string(), elem_val.clone());
        let old_mvl_ty = elem_ty_opt
            .as_ref()
            .and_then(ty_to_type_expr)
            .and_then(|te| self.fn_ctx.local_mvl_types.insert(var_name.to_string(), te));

        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();

        self.emit_block_tir(body)?;

        if let Some(prev) = old_local {
            self.fn_ctx.locals.insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.locals.remove(var_name);
        }
        if let Some(prev) = old_mvl_ty {
            self.fn_ctx
                .local_mvl_types
                .insert(var_name.to_string(), prev);
        } else {
            self.fn_ctx.local_mvl_types.remove(var_name);
        }

        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            let next_i = self.next_reg();
            self.push_instr(&format!("{next_i} = add i64 {cur_i}, 1"));
            self.push_instr(&format!("store i64 {next_i}, ptr {i_ptr}"));
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{cond_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
        Ok(())
    }

    /// TIR variant of [`Self::emit_while`].
    fn emit_while_tir(&mut self, cond: &TirExpr, body: &TirBlock) -> Result<(), String> {
        let loop_bb = self.next_bb("loop");
        let body_bb = self.next_bb("loop_body");
        let end_bb = self.next_bb("loop_end");

        self.push_instr(&format!("br label %{loop_bb}"));
        self.start_bb(&loop_bb);

        let cond_val = self.emit_expr_tir(cond)?;
        if let Some(cv) = cond_val {
            self.push_instr(&format!("br i1 {cv}, label %{body_bb}, label %{end_bb}"));
        } else {
            self.push_instr(&format!("br label %{end_bb}"));
        }

        // Snapshot heap_locals before the body so any lets inside the loop are
        // dropped at the back-edge, matching the AST fix for #1617 (#1645).
        let heap_locals_snapshot = self.fn_ctx.heap_locals.len();
        self.start_bb(&body_bb);
        self.emit_block_tir(body)?;
        if !self.fn_ctx.terminated {
            self.drop_loop_body_locals(heap_locals_snapshot);
            self.ensure_yield_check_extern();
            self.push_instr("call void @_mvl_yield_check()");
            self.push_instr(&format!("br label %{loop_bb}"));
        } else {
            self.fn_ctx.heap_locals.truncate(heap_locals_snapshot);
        }

        self.start_bb(&end_bb);
        Ok(())
    }
}
