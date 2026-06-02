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
//! pub struct Foo { _sender: MvlSender<FooMailbox> }
//!
//! impl Foo {
//!     pub fn behavior(&self, params) {
//!         self._sender.send(FooMailbox::Behavior { params });
//!     }
//! }
//!
//! // Start function — called by `actor Foo { field: val, ... }` expressions
//! fn _start_foo(state: FooState) -> Foo {
//!     let (tx, rx) = mvl_channel(256_i64, 0_i64);
//!     let __handle = mvl_spawn(move || {
//!         let mut actor = state;
//!         while let Some(msg) = rx.recv() {
//!             match msg {
//!                 FooMailbox::Behavior { params } => actor.behavior(params),
//!                 FooMailbox::UnitBehavior => actor.unit_behavior(),
//!             }
//!         }
//!     });
//!     mvl_register_actor(__handle);
//!     Foo { _sender: tx }
//! }
//! ```
//!
//! Runtime symbols come from `mvl_runtime::actors` — the emitter calls only
//! the named interface; the runtime crate provides the implementation.
//! Swapping `--target` replaces the crate without touching this emitter.
//!
//! See Spec 015 (actors), ADR-0027. Phase 8, #695, #1014.

use crate::mvl::backends::rust::emit_exprs::emit_block_stmts;
use crate::mvl::backends::rust::emit_types::emit_type_expr;
use crate::mvl::backends::rust::emitter::RustEmitter;
use crate::mvl::backends::rust::last_use::compute_last_uses;
use crate::mvl::parser::ast::{ActorDecl, MailboxConfig, MailboxPolicy, Stmt};

/// Convert a `snake_case` name to `PascalCase` for Rust enum variant names.
///
/// `query_done` → `QueryDone`, `handle` → `Handle`.
fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
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

/// Emit the actor runtime import for programs that contain at least one actor.
///
/// Pulls in the named interface from `mvl_runtime::actors` — the emitter
/// calls only these symbols; the runtime crate provides the implementation.
/// Swapping `--target` replaces the crate without changing emitter output.
/// ADR-0027 §"Actor runtime interface".
pub fn emit_actor_runtime_preamble(cg: &mut RustEmitter) {
    cg.line("use mvl_runtime::actors::*;");
}

/// Emit the complete Rust runtime infrastructure for an MVL actor declaration.
///
/// Emits seven items in order (items 4–7 omitted when no public behaviors exist):
/// 1. `{Name}State` struct — private mutable state
/// 2. `{Name}Msg` enum — message discriminants for each public behavior
/// 3. `impl {Name}State` — state-machine method bodies
/// 4. `struct {Name}` — tag-capability actor handle (with `#[derive(Clone)]`)
/// 5. `impl {Name}` — fire-and-forget dispatch wrappers
/// 6. `fn {name}_dispatch` — dispatch free function passed to `mvl_actor_run`
/// 7. `fn _start_{name}` — spawns the actor thread via `mvl_actor_run`, returns the handle
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
        // `_self_ref` holds a *weak* sender so that behaviors can pass `self`
        // as a `tag` argument without keeping the mailbox channel alive.
        // When all external handles are dropped the channel disconnects and
        // `rx.recv()` returns `None` even though this weak ref still exists.
        cg.line(&format!("_self_ref: Option<MvlWeakSender<{msg_name}>>,"));
    }
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // ── 2. Message enum (one variant per public behavior + system variants) ─
    if !pub_methods.is_empty() {
        cg.line(&format!("enum {msg_name} {{"));
        cg.push_indent();
        for m in &pub_methods {
            let variant = snake_to_pascal(&m.name);
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
        // System variants for link/monitor infrastructure (Phase 9, #1177).
        cg.line("_Shutdown,");
        cg.line("_ExitSignal { _from_id: ActorId, _reason: ExitReason },");
        cg.line("_DownSignal { _from_id: ActorId, _reason: ExitReason, _monitor_id: MonitorId },");
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

    // ── 4, 5, 6, 7: actor handle, dispatch impl, dispatch fn, start fn ────
    // Only emitted when there are public behaviors (otherwise there is nothing
    // to send and no point in spawning a thread).
    if pub_methods.is_empty() {
        cg.line(&format!(
            "// actor {name}: no public behaviors — actor handle omitted"
        ));
        return;
    }

    // 4. Actor handle struct (tag capability: sender channel + unique actor ID).
    //    `_id` enables `actor_id()` — used by link/monitor callers (#1128).
    let vis = if ad.visible { "pub " } else { "" };
    cg.line("#[derive(Clone)]");
    cg.line(&format!("{vis}struct {name} {{"));
    cg.push_indent();
    cg.line(&format!("_sender: MvlSender<{msg_name}>,"));
    cg.line("_id: ActorId,");
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // 5. Handle impl: actor_id() accessor + one fire-and-forget wrapper per public behavior.
    cg.line(&format!("impl {name} {{"));
    cg.push_indent();
    // Pure sync accessor — no mailbox send, no Send effect required.
    cg.line("pub fn actor_id(&self) -> i64 { self._id as i64 }");
    for m in &pub_methods {
        let variant = snake_to_pascal(&m.name);
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
        cg.line(&format!("self._sender.send({msg_expr});"));
        cg.pop_indent();
        cg.line("}");
    }
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // 6. Dispatch function: a named free function passed to `mvl_actor_run`.
    //    Returns `bool`: `true` to continue, `false` to shut down.
    //    Handles system variants for link/monitor (#1177).  ADR-0027.
    let dispatch_fn = format!("{}_dispatch", actor_name_to_snake(name));
    cg.line(&format!(
        "fn {dispatch_fn}(actor: &mut {state_name}, msg: {msg_name}) -> bool {{"
    ));
    cg.push_indent();
    cg.line("match msg {");
    cg.push_indent();
    // System variants (#1177, #1128).
    cg.line(&format!("{msg_name}::_Shutdown => return false,"));
    // Wire _ExitSignal → on_exit(from_id, reason) if the actor defines that method.
    let on_exit_method = ad
        .methods
        .iter()
        .find(|m| !m.is_public && m.name == "on_exit");
    if let Some(m) = on_exit_method {
        assert!(
            m.params.len() == 2,
            "actor `{}`: on_exit must have exactly 2 parameters (from_id: Int, reason: Int), found {}",
            ad.name,
            m.params.len()
        );
        cg.line(&format!(
            "{msg_name}::_ExitSignal {{ _from_id, _reason }} => actor.on_exit(_from_id as i64, _reason as i64),"
        ));
    } else {
        cg.line(&format!(
            "{msg_name}::_ExitSignal {{ _from_id: _, _reason: _ }} => {{}}"
        ));
    }
    // Wire _DownSignal → on_down(from_id, reason, monitor_ref) if defined.
    let on_down_method = ad
        .methods
        .iter()
        .find(|m| !m.is_public && m.name == "on_down");
    if let Some(m) = on_down_method {
        assert!(
            m.params.len() == 3,
            "actor `{}`: on_down must have exactly 3 parameters (from_id: Int, reason: Int, monitor_ref: Int), found {}",
            ad.name,
            m.params.len()
        );
        cg.line(&format!(
            "{msg_name}::_DownSignal {{ _from_id, _reason, _monitor_id }} => actor.on_down(_from_id as i64, _reason as i64, _monitor_id as i64),"
        ));
    } else {
        cg.line(&format!(
            "{msg_name}::_DownSignal {{ _from_id: _, _reason: _, _monitor_id: _ }} => {{}}"
        ));
    }
    // User behavior variants.
    for m in &pub_methods {
        let variant = snake_to_pascal(&m.name);
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
    cg.line("true");
    cg.pop_indent();
    cg.line("}");
    cg.blank();

    // 7. Start function: spawn actor thread via `mvl_actor_run`, return handle.
    //    Takes `mut state` so we can inject `_self_ref` after creating the
    //    channel — the actor needs a weak sender to pass `self`
    //    as a `tag` argument inside behaviors.
    //
    //    Assigns a unique ActorId and registers type-erased controls in the
    //    global link/monitor registry (#1177).
    //
    //    Shutdown protocol (#1048, #1125): the main body scope drops all actor
    //    handles before `mvl_join_actors()` runs.  `MvlReceiver::recv()` drains
    //    buffered messages then returns `None` once every sender is gone.
    cg.line(&format!(
        "fn {start_fn}(mut state: {state_name}) -> {name} {{"
    ));
    cg.push_indent();
    // Assign unique actor ID (#1177).
    cg.line("let __actor_id = mvl_next_actor_id();");
    let channel_line = match &ad.mailbox {
        Some(MailboxConfig::Unbounded) => "let (tx, rx) = mvl_channel(-1_i64, 0_i64);".to_string(),
        Some(MailboxConfig::Bounded { capacity, policy }) => {
            let pol: i64 = match policy {
                MailboxPolicy::Block => 1,
                MailboxPolicy::DropNewest => 0,
            };
            format!("let (tx, rx) = mvl_channel({capacity}_i64, {pol}_i64);")
        }
        None => "let (tx, rx) = mvl_channel(256_i64, 0_i64);".to_string(),
    };
    cg.line(&channel_line);
    cg.line("state._self_ref = Some(tx.downgrade());");
    // Register type-erased actor controls for link/monitor (#1177).
    // Each closure captures a sender clone and constructs the typed system message.
    cg.line("{");
    cg.push_indent();
    cg.line("let tx_kill = tx.clone();");
    cg.line("let tx_exit = tx.clone();");
    cg.line("let tx_down = tx.clone();");
    cg.line("mvl_register_actor_controls(");
    cg.push_indent();
    cg.line("__actor_id,");
    cg.line(&format!(
        "Box::new(move || {{ tx_kill.send({msg_name}::_Shutdown); }}),"
    ));
    cg.line(&format!(
        "Box::new(move |from, reason| {{ tx_exit.send({msg_name}::_ExitSignal {{ _from_id: from, _reason: reason }}); }}),"
    ));
    cg.line(&format!(
        "Box::new(move |from, reason, mid| {{ tx_down.send({msg_name}::_DownSignal {{ _from_id: from, _reason: reason, _monitor_id: mid }}); }}),"
    ));
    cg.line(&format!("{},", ad.traps_exit));
    cg.pop_indent();
    cg.line(");");
    cg.pop_indent();
    cg.line("}");
    cg.line(&format!(
        "let __handle = mvl_actor_run(rx, state, {dispatch_fn}, __actor_id);"
    ));
    cg.line("mvl_register_actor(__handle);");
    cg.line(&format!("{name} {{ _sender: tx, _id: __actor_id }}"));
    cg.pop_indent();
    cg.line("}");
}
