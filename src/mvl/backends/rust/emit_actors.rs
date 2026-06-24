// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Emit Rust actor infrastructure from MVL [`TirActorDecl`] nodes.
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

use super::emitter::RustEmitter;
use crate::mvl::backends::rust::emit_types::emit_ty;
use crate::mvl::backends::rust::last_use::compute_last_uses;
use crate::mvl::ir::{MailboxConfig, MailboxPolicy, TirActorDecl, TirStmt};

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

impl RustEmitter {
    /// Emit the actor runtime import for programs that contain at least one actor.
    ///
    /// Pulls in the named interface from `mvl_runtime::actors` — the emitter
    /// calls only these symbols; the runtime crate provides the implementation.
    /// Swapping `--target` replaces the crate without changing emitter output.
    /// ADR-0027 §"Actor runtime interface".
    pub fn emit_actor_runtime_preamble(&mut self) {
        self.line("use mvl_runtime::actors::*;");
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
    pub fn emit_actor_decl(&mut self, ad: &TirActorDecl) {
        let name = &ad.name;
        let state_name = format!("{name}State");
        // Mailbox enum is named `{Name}Mailbox` (not `{Name}Msg`) to avoid
        // colliding with user-defined message struct types like `PingMsg`.
        let msg_name = format!("{name}Mailbox");
        let start_fn = format!("_start_{}", actor_name_to_snake(name));

        let pub_methods: Vec<_> = ad.methods.iter().filter(|m| m.is_public).collect();
        let test_methods: Vec<_> = ad
            .methods
            .iter()
            .filter(|m| m.is_public && m.is_test)
            .collect();

        // ── 1. State struct ────────────────────────────────────────────────────
        self.line(&format!("struct {state_name} {{"));
        self.push_indent();
        for field in &ad.fields {
            let ty_str = emit_ty(&field.ty);
            self.line(&format!("{}: {ty_str},", field.name));
        }
        if !pub_methods.is_empty() {
            // `_self_ref` holds a *weak* sender so that behaviors can pass `self`
            // as a `tag` argument without keeping the mailbox channel alive.
            // When all external handles are dropped the channel disconnects and
            // `rx.recv()` returns `None` even though this weak ref still exists.
            self.line(&format!("_self_ref: Option<MvlWeakSender<{msg_name}>>,"));
            // `_self_id` mirrors the handle's `_id` so self-ref handle construction
            // can set the `_id` field (#1128).
            self.line("_self_id: ActorId,");
        }
        self.pop_indent();
        self.line("}");
        self.blank();

        // ── 2. Message enum (one variant per public behavior + system variants) ─
        if !pub_methods.is_empty() {
            self.line(&format!("enum {msg_name} {{"));
            self.push_indent();
            for m in pub_methods.iter().filter(|m| !m.is_test) {
                let variant = snake_to_pascal(&m.name);
                if m.params.is_empty() {
                    self.line(&format!("{variant},"));
                } else {
                    let field_strs: Vec<String> = m
                        .params
                        .iter()
                        .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                        .collect();
                    self.line(&format!("{variant} {{ {} }},", field_strs.join(", ")));
                }
            }
            // Test-only variants carry a reply channel so the call can be synchronous (#1506).
            for m in &test_methods {
                let variant = format!("_Test{}", snake_to_pascal(&m.name));
                let ret_str = emit_ty(&m.ret_ty);
                let field_strs: Vec<String> = m
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                    .collect();
                let reply_field = format!("_reply: std::sync::mpsc::Sender<{ret_str}>");
                let fields = if field_strs.is_empty() {
                    reply_field
                } else {
                    format!("{}, {reply_field}", field_strs.join(", "))
                };
                self.line("#[cfg(test)]");
                self.line(&format!("{variant} {{ {fields} }},"));
            }
            // System variants for link/monitor infrastructure (Phase 9, #1177).
            // Use the fully-qualified runtime type for _reason to avoid a shadowing
            // clash: when std/actors.mvl is in scope its compiled `ExitReason` enum
            // would shadow the runtime's `ExitReason` (= i64 alias), causing a type
            // mismatch in the register_actor_controls closures.
            self.line("_Shutdown,");
            self.line(
                "_ExitSignal { _from_id: ActorId, _reason: mvl_runtime::actors::ExitReason },",
            );
            self.line(
                "_DownSignal { _from_id: ActorId, _reason: mvl_runtime::actors::ExitReason, _monitor_id: MonitorId },",
            );
            self.pop_indent();
            self.line("}");
            self.blank();
        }

        // ── 3. State impl (all methods — behaviors and helpers run on actor thread) ──
        if !ad.methods.is_empty() {
            // Expose actor method names and handle type so emit_exprs can:
            //   - prefix free calls to these names with `self.` (e.g. log → self.log)
            //   - replace `Expr::Ident("self")` arguments with `self._self_ref.as_ref().unwrap().clone()`
            self.actor_methods = ad.methods.iter().map(|m| m.name.clone()).collect();
            self.actor_self_type = name.clone();
            self.line(&format!("impl {state_name} {{"));
            self.push_indent();
            for m in &ad.methods {
                let param_strs: Vec<String> = m
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                    .collect();
                let params_sig = if param_strs.is_empty() {
                    String::new()
                } else {
                    format!(", {}", param_strs.join(", "))
                };
                let ret_str = emit_ty(&m.ret_ty);
                let ret_part = if ret_str == "()" {
                    String::new()
                } else {
                    format!(" -> {ret_str}")
                };
                // pub test fn methods are only called from #[cfg(test)] dispatch arms (#1506).
                if m.is_public && m.is_test {
                    self.line("#[cfg(test)]");
                }
                self.line(&format!(
                    "fn {}(&mut self{params_sig}){ret_part} {{",
                    m.name
                ));
                self.push_indent();
                // Update current_fn so coverage probes emitted for this method's body
                // are attributed to the correct function name (#1501).
                self.current_fn = m.name.clone();
                self.current_fn_is_test = m.is_test;
                self.last_uses = compute_last_uses(&m.body);
                let stmts = &m.body.stmts;
                if stmts.is_empty() {
                    // empty body — unit return, Rust implicit ()
                } else if ret_str == "()" {
                    self.emit_block_stmts(stmts);
                } else {
                    // Non-unit return: emit all but the tail, then the tail as an expression.
                    let (head, tail) = stmts.split_at(stmts.len() - 1);
                    self.emit_block_stmts(head);
                    match &tail[0] {
                        TirStmt::Expr { expr, .. } => {
                            self.indent();
                            // Inline the expression without a trailing semicolon so Rust
                            // treats it as the implicit return value.
                            self.emit_expr(expr);
                            self.nl();
                        }
                        other => self.emit_block_stmts(std::slice::from_ref(other)),
                    }
                }
                self.pop_indent();
                self.line("}");
            }
            self.pop_indent();
            self.line("}");
            self.blank();
            // Clear actor context after the impl block.
            self.actor_methods.clear();
            self.actor_self_type.clear();
        }

        // ── 4, 5, 6, 7: actor handle, dispatch impl, dispatch fn, start fn ────
        // Only emitted when there are public behaviors (otherwise there is nothing
        // to send and no point in spawning a thread).
        if pub_methods.is_empty() {
            self.line(&format!(
                "// actor {name}: no public behaviors — actor handle omitted"
            ));
            return;
        }

        // 4. Actor handle struct (tag capability: sender channel + unique actor ID).
        //    `_id` enables `actor_id()` — used by link/monitor callers (#1128).
        let vis = if ad.visible { "pub " } else { "" };
        self.line("#[derive(Clone)]");
        self.line(&format!("{vis}struct {name} {{"));
        self.push_indent();
        self.line(&format!("_sender: MvlSender<{msg_name}>,"));
        self.line("_id: ActorId,");
        self.pop_indent();
        self.line("}");
        self.blank();

        // 5. Handle impl: actor_id() accessor + one fire-and-forget wrapper per public behavior.
        //    #[cfg(test)] synchronous methods for pub test fn (#1506).
        self.line(&format!("impl {name} {{"));
        self.push_indent();
        // Pure sync accessor — no mailbox send, no Send effect required.
        self.line("pub fn actor_id(&self) -> i64 { self._id as i64 }");
        for m in pub_methods.iter().filter(|m| !m.is_test) {
            let variant = snake_to_pascal(&m.name);
            let param_strs: Vec<String> = m
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                .collect();
            let params_sig = if param_strs.is_empty() {
                String::new()
            } else {
                format!(", {}", param_strs.join(", "))
            };
            self.line(&format!("pub fn {}(&self{params_sig}) {{", m.name));
            self.push_indent();
            let msg_expr = if m.params.is_empty() {
                format!("{msg_name}::{variant}")
            } else {
                let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
                format!("{msg_name}::{variant} {{ {} }}", fields.join(", "))
            };
            self.line(&format!("self._sender.send({msg_expr});"));
            self.pop_indent();
            self.line("}");
        }
        // Synchronous test accessors — send a request with a reply channel and block (#1506).
        for m in &test_methods {
            let variant = format!("_Test{}", snake_to_pascal(&m.name));
            let ret_str = emit_ty(&m.ret_ty);
            let param_strs: Vec<String> = m
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, emit_ty(&p.ty)))
                .collect();
            let params_sig = if param_strs.is_empty() {
                String::new()
            } else {
                format!(", {}", param_strs.join(", "))
            };
            self.line("#[cfg(test)]");
            self.line(&format!(
                "pub fn {}(&self{params_sig}) -> {ret_str} {{",
                m.name
            ));
            self.push_indent();
            self.line("let (_tx, _rx) = std::sync::mpsc::channel();");
            let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
            let msg_expr = if fields.is_empty() {
                format!("{msg_name}::{variant} {{ _reply: _tx }}")
            } else {
                format!(
                    "{msg_name}::{variant} {{ {}, _reply: _tx }}",
                    fields.join(", ")
                )
            };
            self.line(&format!("self._sender.send({msg_expr});"));
            self.line("_rx.recv().expect(\"actor thread died\")");
            self.pop_indent();
            self.line("}");
        }
        self.pop_indent();
        self.line("}");
        self.blank();

        // 6. Dispatch function: a named free function passed to `mvl_actor_run`.
        //    Returns `bool`: `true` to continue, `false` to shut down.
        //    Handles system variants for link/monitor (#1177).  ADR-0027.
        let dispatch_fn = format!("{}_dispatch", actor_name_to_snake(name));
        self.line(&format!(
            "fn {dispatch_fn}(actor: &mut {state_name}, msg: {msg_name}) -> bool {{"
        ));
        self.push_indent();
        self.line("match msg {");
        self.push_indent();
        // System variants (#1177, #1128).
        self.line(&format!("{msg_name}::_Shutdown => return false,"));
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
            self.line(&format!(
                "{msg_name}::_ExitSignal {{ _from_id, _reason }} => actor.on_exit(_from_id as i64, _reason),"
            ));
        } else {
            self.line(&format!(
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
            self.line(&format!(
                "{msg_name}::_DownSignal {{ _from_id, _reason, _monitor_id }} => actor.on_down(_from_id as i64, _reason, _monitor_id as i64),"
            ));
        } else {
            self.line(&format!(
                "{msg_name}::_DownSignal {{ _from_id: _, _reason: _, _monitor_id: _ }} => {{}}"
            ));
        }
        // User behavior variants.
        for m in pub_methods.iter().filter(|m| !m.is_test) {
            let variant = snake_to_pascal(&m.name);
            if m.params.is_empty() {
                self.line(&format!("{msg_name}::{variant} => actor.{}(),", m.name));
            } else {
                let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
                let args = fields.join(", ");
                self.line(&format!(
                    "{msg_name}::{variant} {{ {args} }} => actor.{}({args}),",
                    m.name
                ));
            }
        }
        // Test accessor variants — call the method synchronously and send the result back (#1506).
        for m in &test_methods {
            let variant = format!("_Test{}", snake_to_pascal(&m.name));
            let fields: Vec<String> = m.params.iter().map(|p| p.name.clone()).collect();
            let args = fields.join(", ");
            let call = if fields.is_empty() {
                format!("actor.{}()", m.name)
            } else {
                format!("actor.{}({args})", m.name)
            };
            let pattern = if fields.is_empty() {
                format!("{msg_name}::{variant} {{ _reply }}")
            } else {
                format!("{msg_name}::{variant} {{ {args}, _reply }}")
            };
            self.line("#[cfg(test)]");
            self.line(&format!("{pattern} => {{ let _ = _reply.send({call}); }},"));
        }
        self.pop_indent();
        self.line("}");
        self.line("true");
        self.pop_indent();
        self.line("}");
        self.blank();

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
        self.line(&format!(
            "fn {start_fn}(mut state: {state_name}) -> {name} {{"
        ));
        self.push_indent();
        // Assign unique actor ID (#1177).
        self.line("let __actor_id = mvl_next_actor_id();");
        let channel_line = match &ad.mailbox {
            Some(MailboxConfig::Unbounded) => {
                "let (tx, rx) = mvl_channel(-1_i64, 0_i64);".to_string()
            }
            Some(MailboxConfig::Bounded { capacity, policy }) => {
                let pol: i64 = match policy {
                    MailboxPolicy::Block => 1,
                    MailboxPolicy::DropNewest => 0,
                };
                format!("let (tx, rx) = mvl_channel({capacity}_i64, {pol}_i64);")
            }
            None => "let (tx, rx) = mvl_channel(256_i64, 0_i64);".to_string(),
        };
        self.line(&channel_line);
        self.line("state._self_id = __actor_id;");
        self.line("state._self_ref = Some(tx.downgrade());");
        // Register type-erased actor controls for link/monitor (#1177).
        // Each closure captures a sender clone and constructs the typed system message.
        self.line("{");
        self.push_indent();
        self.line("let tx_kill = tx.clone();");
        self.line("let tx_exit = tx.clone();");
        self.line("let tx_down = tx.clone();");
        self.line("mvl_register_actor_controls(");
        self.push_indent();
        self.line("__actor_id,");
        self.line(&format!(
            "Box::new(move || {{ tx_kill.send({msg_name}::_Shutdown); }}),"
        ));
        self.line(&format!(
            "Box::new(move |from, reason| {{ tx_exit.send({msg_name}::_ExitSignal {{ _from_id: from, _reason: reason }}); }}),"
        ));
        self.line(&format!(
            "Box::new(move |from, reason, mid| {{ tx_down.send({msg_name}::_DownSignal {{ _from_id: from, _reason: reason, _monitor_id: mid }}); }}),"
        ));
        self.line(&format!("{},", ad.traps_exit));
        self.pop_indent();
        self.line(");");
        self.pop_indent();
        self.line("}");
        self.line(&format!(
            "let __handle = mvl_actor_run(rx, state, {dispatch_fn}, __actor_id);"
        ));
        self.line("mvl_register_actor(__handle);");
        self.line(&format!("{name} {{ _sender: tx, _id: __actor_id }}"));
        self.pop_indent();
        self.line("}");
    }
}
