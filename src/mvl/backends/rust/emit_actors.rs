// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust actor infrastructure from MVL [`ActorDecl`] nodes.
//!
//! Each `actor Foo { fields; pub fn behaviors; fn helpers }` compiles to:
//!
//! ```text
//! // Private mutable state
//! struct FooState { field: ty, ... }
//!
//! // Message discriminant — one variant per public behavior
//! enum FooMsg { Behavior { params }, UnitBehavior, ... }
//!
//! // State machine — all method bodies run on the actor thread
//! impl FooState {
//!     fn behavior(&mut self, params) { body }
//!     fn helper(&mut self) -> ret { body }
//! }
//!
//! // Tag-capability handle — cheap to clone, safe to send across threads
//! #[derive(Clone)]
//! pub struct Foo { _sender: std::sync::mpsc::SyncSender<FooMsg> }
//!
//! impl Foo {
//!     pub fn behavior(&self, params) {
//!         let _ = self._sender.try_send(FooMsg::Behavior { params });
//!     }
//! }
//!
//! // Start function — called by `actor Foo { field: val, ... }` expressions
//! fn _start_foo(state: FooState) -> Foo {
//!     let (tx, rx) = std::sync::mpsc::sync_channel(256);
//!     std::thread::spawn(move || {
//!         let mut actor = state;
//!         while let Ok(msg) = rx.recv() {
//!             match msg {
//!                 FooMsg::Behavior { params } => actor.behavior(params),
//!                 FooMsg::UnitBehavior => actor.unit_behavior(),
//!             }
//!         }
//!     });
//!     Foo { _sender: tx }
//! }
//! ```
//!
//! Runtime: `std::sync::mpsc::sync_channel` + `std::thread::spawn` (no tokio).
//! Behaviors are fire-and-forget — `try_send` drops the message on a full queue
//! rather than blocking the caller.  Queue capacity defaults to 256.
//!
//! See Spec 015 (actors), ADR-0029. Phase 8, #695.

use crate::mvl::backends::rust::emit_exprs::emit_block_stmts;
use crate::mvl::backends::rust::emit_types::emit_type_expr;
use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::rust::last_use::compute_last_uses;
use crate::mvl::parser::ast::{ActorDecl, Stmt};

/// Capitalize the first character of a string, leaving the rest unchanged.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Convert a `PascalCase` name to `snake_case` for function names.
///
/// `Counter` → `counter`, `MyActor` → `my_actor`.
pub fn actor_name_to_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Emit the complete Rust runtime infrastructure for an MVL actor declaration.
///
/// Emits six items in order (items 4–6 omitted when no public behaviors exist):
/// 1. `{Name}State` struct — private mutable state
/// 2. `{Name}Msg` enum — message discriminants for each public behavior
/// 3. `impl {Name}State` — state-machine method bodies
/// 4. `struct {Name}` — tag-capability actor handle (with `#[derive(Clone)]`)
/// 5. `impl {Name}` — fire-and-forget dispatch wrappers
/// 6. `fn _start_{name}` — spawns the actor thread, returns the handle
pub fn emit_actor_decl(cg: &mut RustEmitter, ad: &ActorDecl) {
    let name = &ad.name;
    let state_name = format!("{name}State");
    // Mailbox enum is named `{Name}Mailbox` (not `{Name}Msg`) to avoid
    // colliding with user-defined message struct types like `PingMsg`.
    let msg_name = format!("{name}Mailbox");
    let start_fn = format!("_start_{}", actor_name_to_snake(name));

    let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).collect();

    // ── 1. State struct ────────────────────────────────────────────────────
    cg.line(&format!("struct {state_name} {{"));
    cg.push_indent();
    for field in &ad.fields {
        let ty_str = emit_type_expr(&field.ty);
        cg.line(&format!("{}: {ty_str},", field.name));
    }
    if !pub_methods.is_empty() {
        // `_self_ref` holds the actor's own handle so that behaviors can pass
        // `self` as a `tag` argument to other actors.  Initialised to `None`
        // at construction; set by `_start_<name>` before the thread is spawned.
        cg.line(&format!("_self_ref: Option<{name}>,"));
    }
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // ── 2. Message enum (one variant per public behavior) ──────────────────
    if !pub_methods.is_empty() {
        cg.line(&format!("enum {msg_name} {{"));
        cg.push_indent();
        for m in &pub_methods {
            let variant = capitalize(&m.name);
            if m.params.is_empty() {
                cg.line(&format!("{variant},"));
            } else {
                let field_strs: Vec<String> = m
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, emit_type_expr(&p.ty)))
                    .collect();
                cg.line(&format!("{variant} {{ {} }},", field_strs.join(", ")));
            }
        }
        cg.pop_indent();
        cg.line("}");
        cg.blank();
    }

    // ── 3. State impl (all methods — behaviors and helpers run on actor thread) ──
    if !ad.methods.is_empty() {
        // Expose actor method names and handle type so emit_exprs can:
        //   - prefix free calls to these names with `self.` (e.g. log → self.log)
        //   - replace `Expr::Ident("self")` arguments with `self._self_ref.as_ref().unwrap().clone()`
        cg.actor_methods = ad.methods.iter().map(|m| m.name.clone()).collect();
        cg.actor_self_type = name.clone();
        cg.line(&format!("impl {state_name} {{"));
        cg.push_indent();
        for m in &ad.methods {
            let param_strs: Vec<String> = m
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, emit_type_expr(&p.ty)))
                .collect();
            let params_sig = if param_strs.is_empty() {
                String::new()
            } else {
                format!(", {}", param_strs.join(", "))
            };
            let ret_str = emit_type_expr(&m.return_type);
            let ret_part = if ret_str == "()" {
                String::new()
            } else {
                format!(" -> {ret_str}")
            };
            cg.line(&format!(
                "fn {}(&mut self{params_sig}){ret_part} {{",
                m.name
            ));
            cg.push_indent();
            cg.last_uses = compute_last_uses(&m.body);
            let stmts = &m.body.stmts;
            if stmts.is_empty() {
                // empty body — unit return, Rust implicit ()
            } else if ret_str == "()" {
                emit_block_stmts(cg, stmts);
            } else {
                // Non-unit return: emit all but the tail, then the tail as an expression.
                let (head, tail) = stmts.split_at(stmts.len() - 1);
                emit_block_stmts(cg, head);
                match &tail[0] {
                    Stmt::Expr { expr, .. } => {
                        cg.indent();
                        // Inline the expression without a trailing semicolon so Rust
                        // treats it as the implicit return value.
                        use crate::mvl::backends::rust::emit_exprs::emit_expr;
                        emit_expr(cg, expr);
                        cg.nl();
                    }
                    other => emit_block_stmts(cg, std::slice::from_ref(other)),
                }
            }
            cg.pop_indent();
            cg.line("}");
        }
        cg.pop_indent();
        cg.line("}");
        cg.blank();
        // Clear actor context after the impl block.
        cg.actor_methods.clear();
        cg.actor_self_type.clear();
    }

    // ── 4, 5, 6: actor handle, dispatch impl, start fn ────────────────────
    // Only emitted when there are public behaviors (otherwise there is nothing
    // to send and no point in spawning a thread).
    if pub_methods.is_empty() {
        cg.line(&format!(
            "// actor {name}: no public behaviors — actor handle omitted"
        ));
        return;
    }

    // 4. Actor handle struct (tag capability: only the sender channel)
    let vis = if ad.visible { "pub " } else { "" };
    cg.line("#[derive(Clone)]");
    cg.line(&format!("{vis}struct {name} {{"));
    cg.push_indent();
    cg.line(&format!(
        "_sender: std::sync::mpsc::SyncSender<{msg_name}>,"
    ));
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // 5. Handle impl: one fire-and-forget wrapper per public behavior
    cg.line(&format!("impl {name} {{"));
    cg.push_indent();
    for m in &pub_methods {
        let variant = capitalize(&m.name);
        let param_strs: Vec<String> = m
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, emit_type_expr(&p.ty)))
            .collect();
        let params_sig = if param_strs.is_empty() {
            String::new()
        } else {
            format!(", {}", param_strs.join(", "))
        };
        cg.line(&format!("pub fn {}(&self{params_sig}) {{", m.name));
        cg.push_indent();
        let msg_expr = if m.params.is_empty() {
            format!("{msg_name}::{variant}")
        } else {
            let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
            format!("{msg_name}::{variant} {{ {} }}", fields.join(", "))
        };
        cg.line(&format!("let _ = self._sender.try_send({msg_expr});"));
        cg.pop_indent();
        cg.line("}");
    }
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // 6. Start function: spawn actor thread, return handle.
    //    Takes `mut state` so we can inject `_self_ref` after creating the
    //    channel — the actor needs a clone of its own sender to pass `self`
    //    as a `tag` argument inside behaviors.
    cg.line(&format!(
        "fn {start_fn}(mut state: {state_name}) -> {name} {{"
    ));
    cg.push_indent();
    cg.line("let (tx, rx) = std::sync::mpsc::sync_channel(256);");
    cg.line(&format!(
        "state._self_ref = Some({name} {{ _sender: tx.clone() }});"
    ));
    cg.line("std::thread::spawn(move || {");
    cg.push_indent();
    cg.line("let mut actor = state;");
    cg.line("while let Ok(msg) = rx.recv() {");
    cg.push_indent();
    cg.line("match msg {");
    cg.push_indent();
    for m in &pub_methods {
        let variant = capitalize(&m.name);
        if m.params.is_empty() {
            cg.line(&format!("{msg_name}::{variant} => actor.{}(),", m.name));
        } else {
            let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
            let args = fields.join(", ");
            cg.line(&format!(
                "{msg_name}::{variant} {{ {args} }} => actor.{}({args}),",
                m.name
            ));
        }
    }
    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("}");
    cg.pop_indent();
    cg.line("});");
    cg.line(&format!("{name} {{ _sender: tx }}"));
    cg.pop_indent();
    cg.line("}");
}
