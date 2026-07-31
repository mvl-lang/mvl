// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl build --backend=wasm` and `mvl test --backend=wasm` drivers (#1571).
//!
//! Reuses the same prelude/checker/TIR pipeline as the llvm_text backend.
//! The test harness mirrors `cmd_test_llvm_text`: discover `fn main` +
//! `// expect:` files, emit WAT, assemble via `wasm-tools`, run via
//! `wasmtime`, compare stdout to the expected string.

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::wasm_text::{emitter_handles_method_natively, WasmTextCompiler};
use mvl::mvl::backends::{AssertMode, Backend};
use mvl::mvl::checker;
use mvl::mvl::checker::types::Ty;
use mvl::mvl::ir::visit::{walk_tir_expr, Visit};
use mvl::mvl::ir::{
    TirExpr, TirExprKind, TirFn, TirProgram, TirTypeBody, TirTypeDecl, TirVariantFields,
};
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Decl, FnDecl, Program, TypeDecl};
use mvl::mvl::parser::lexer::Span;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use mvl::mvl::stdlib;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// Collects every free-function name referenced via `TirExprKind::FnCall`,
/// every qualified enum-variant reference (`TirExprKind::Var("Type::Variant")`),
/// and every extension-method dot-call (`TirExprKind::MethodCall`) whose
/// receiver resolves to a named type — used to find prelude functions/types
/// a program references but that never got lowered into the emitted module
/// (#2045, #2046, #2056).
#[derive(Default)]
struct RefCollector {
    fn_calls: HashSet<String>,
    variant_refs: HashSet<String>,
    /// `(receiver_type, method, receiver_ty)` — e.g. `("Logger", "info", ..)`
    /// for `logger.info(...)`. The full `Ty` rides along so the pull-in loop can
    /// ask `emitter_handles_method_natively`, whose answer depends on the
    /// receiver's shape (`String` vs `List`) and not just its name.
    ///
    /// A `Vec`, not a `HashSet`, because `Ty` implements neither `Hash` nor
    /// `Eq`. Duplicates are harmless — the pull-in loop dedups on
    /// `known_methods`.
    method_refs: Vec<(String, String, Ty)>,
}

impl<'a> Visit<'a> for RefCollector {
    fn visit_tir_expr(&mut self, e: &'a TirExpr) {
        match &e.kind {
            TirExprKind::FnCall { name, .. } => {
                self.fn_calls.insert(name.clone());
            }
            TirExprKind::Var(name) if name.contains("::") => {
                self.variant_refs.insert(name.clone());
            }
            TirExprKind::MethodCall {
                receiver, method, ..
            } => {
                if let Some(type_name) = named_type_name(&receiver.ty) {
                    self.method_refs
                        .push((type_name, method.clone(), receiver.ty.clone()));
                }
            }
            _ => {}
        }
        walk_tir_expr(self, e);
    }
}

/// Strip `Ref`/`Labeled`/`Refined` wrappers and return the receiver-type name
/// used as the extension-method lookup key.
///
/// Mirrors `wasm_text::receiver_type_name` in the backend, including the
/// built-in constructors: `fn List[T]::flatten(self)` is stored by the parser
/// with `receiver_type = Some("List")`, but a `List[Int]` receiver has type
/// `Ty::List(..)` and never `Ty::Named("List", ..)`. Answering `None` for the
/// built-ins — as this did while it only handled `Ty::Named` — meant
/// `xs.flatten()` produced no entry in `RefCollector::method_refs` at all, so
/// the pull-in loop below never even considered lowering `std/lists.mvl`'s
/// body (#2014).
fn named_type_name(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Named(n, _) => Some(n.clone()),
        Ty::Ref(_, inner) | Ty::Labeled(_, inner) | Ty::Refined(inner, _) => named_type_name(inner),
        Ty::List(_) | Ty::Array(_, _) => Some("List".to_string()),
        Ty::Set(_) => Some("Set".to_string()),
        Ty::Map(_, _) => Some("Map".to_string()),
        Ty::Option(_) => Some("Option".to_string()),
        Ty::Result(_, _) => Some("Result".to_string()),
        Ty::String => Some("String".to_string()),
        _ => None,
    }
}

/// Collect all top-level type declarations from a list of programs, keyed
/// by name — the `Decl::Type` counterpart to `mono::collect_fns`.
fn collect_type_decls<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> HashMap<String, TypeDecl> {
    let mut map = HashMap::new();
    for prog in programs {
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                map.insert(td.name.clone(), td.clone());
            }
        }
    }
    map
}

/// Collect all extension-method declarations (`fn Type::method(self, ...)`)
/// from a list of programs, keyed by `(receiver_type, method_name)`.
///
/// Deliberately not merged into `mono::collect_fns`'s plain-name map: that
/// map is keyed by bare name only, which collides across types that share a
/// common method name (`len`, `get`, `is_empty`, ...). A `MethodCall`'s
/// receiver type disambiguates here.
fn collect_method_decls<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> HashMap<(String, String), FnDecl> {
    let mut map = HashMap::new();
    for prog in programs {
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if let Some(recv) = &fd.receiver_type {
                    map.insert((recv.clone(), fd.name.clone()), fd.clone());
                }
            }
        }
    }
    map
}

/// Recursively collect every `Ty::Named` name reachable from `ty` — through
/// `Option`/`Result`/`List`/`Set`/`Array`/`Map`/`Ptr`/`Ref`/`Labeled`/
/// `Refined`/`Fn` wrappers and generic type arguments.
///
/// Discovers prelude struct/enum types referenced only in *type position* —
/// a param type, a return type, a struct field — which is invisible to
/// `RefCollector`: such a reference is never a qualified `Type::Variant`
/// `Var`, and never any kind of call (#2056). Without this, a type like
/// `std.log`'s `Logger` or `std.io`'s `Fd` never lands in `merged.types`, so
/// `wasm_ty`/`is_i32` never find it in `struct_layouts` and silently fall
/// back to treating it as a plain `i64` — the function still "compiles",
/// just with the wrong ABI shape for every caller.
fn collect_named_types(ty: &Ty, out: &mut HashSet<String>) {
    match ty {
        Ty::Named(name, args) => {
            out.insert(name.clone());
            for a in args {
                collect_named_types(a, out);
            }
        }
        Ty::Option(inner) | Ty::Ptr(inner) | Ty::Set(inner) | Ty::List(inner) => {
            collect_named_types(inner, out)
        }
        Ty::Array(inner, _) => collect_named_types(inner, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            collect_named_types(a, out);
            collect_named_types(b, out);
        }
        Ty::Ref(_, inner) | Ty::Refined(inner, _) | Ty::Labeled(_, inner) => {
            collect_named_types(inner, out)
        }
        Ty::Fn(params, ret, _, _) => {
            for p in params {
                collect_named_types(p, out);
            }
            collect_named_types(ret, out);
        }
        _ => {}
    }
}

/// Named types referenced by a `TirFn`'s params/return type.
fn fn_type_refs(f: &TirFn, out: &mut HashSet<String>) {
    for p in &f.params {
        collect_named_types(&p.ty, out);
    }
    collect_named_types(&f.ret_ty, out);
}

/// Named types referenced by a `TirTypeDecl`'s own fields — struct fields,
/// enum-variant payloads (tuple or struct-shaped), or an alias's target.
/// Drives the transitive part of the type pull-in: once `Logger` is pulled
/// in, its `fd: Fd` field must pull in `Fd` too.
fn type_decl_field_refs(td: &TirTypeDecl, out: &mut HashSet<String>) {
    match &td.body {
        TirTypeBody::Struct { fields, .. } => {
            for f in fields {
                collect_named_types(&f.ty, out);
            }
        }
        TirTypeBody::Enum(variants) => {
            for v in variants {
                match &v.fields {
                    TirVariantFields::Unit => {}
                    TirVariantFields::Tuple(tys) => {
                        for t in tys {
                            collect_named_types(t, out);
                        }
                    }
                    TirVariantFields::Struct(fields) => {
                        for f in fields {
                            collect_named_types(&f.ty, out);
                        }
                    }
                }
            }
        }
        TirTypeBody::Alias(ty) => collect_named_types(ty, out),
    }
}

/// Lower and merge every type reachable from `seed`, transitively following
/// each newly-lowered type's own field types (#2056).
fn pull_in_types(
    merged: &mut TirProgram,
    known_types: &mut HashSet<String>,
    seed: HashSet<String>,
    all_type_decls: &HashMap<String, TypeDecl>,
    expr_types: &HashMap<Span, Ty>,
) {
    let mut worklist: Vec<String> = seed.into_iter().collect();
    while let Some(name) = worklist.pop() {
        if known_types.contains(&name) {
            continue;
        }
        known_types.insert(name.clone());

        let Some(td) = all_type_decls.get(&name) else {
            continue;
        };
        let synthetic = Program {
            declarations: vec![Decl::Type(td.clone())],
            span: td.span,
        };
        let syn_mono =
            mvl::mvl::passes::mono::monomorphize(&synthetic, &HashMap::new(), expr_types);
        let syn_tir = mvl::mvl::ir::lower::lower(&synthetic, &syn_mono, expr_types);

        let mut field_refs = HashSet::new();
        for new_td in &syn_tir.types {
            type_decl_field_refs(new_td, &mut field_refs);
        }
        for r in field_refs {
            if !known_types.contains(&r) {
                worklist.push(r);
            }
        }
        merged.types.extend(syn_tir.types);
    }
}

/// Pull in prelude functions and types that `merged` references by name but
/// that were never lowered (#2045, #2046).
///
/// `entry_tir`/`sibling_tirs` only lower `prog` (and its siblings) — a call
/// like `int_max(a, b)` reached via `use std.math.{int_max}` is resolved by
/// the checker but never gets a `(func $int_max ...)` body in the emitted
/// module, leaving a dangling `call $int_max` that fails `wasm-tools parse`.
/// Likewise, a bare enum-variant value like `ArgType::Int` (`use
/// std.args.{ArgType}`) lowers to `TirExprKind::Var("ArgType::Int")`, and the
/// WASM emitter only recognizes such names when the owning enum's `TirTypeDecl`
/// is present in `merged.types` — otherwise it falls through to treating the
/// name as a plain local (`local.get $ArgType::Int`, "unknown local"). A
/// struct/enum referenced only in type position (never a qualified Var, never
/// a call) needs the same treatment — see `collect_named_types` (#2056).
///
/// The LLVM backend avoids both gaps by lowering *every* prelude module and
/// merging all of it in — too broad here: prelude functions the WASM backend
/// doesn't support yet (e.g. `open()`, `exit()`) would break every program
/// that transitively pulls them in, even ones that never call them. Instead,
/// walk outward from `merged` and lower only the specific missing
/// functions/types, transitively.
///
/// Extern declarations (`extern "rust" { ... }`) are deliberately left
/// unresolved — `all_fn_decls` only contains `Decl::Fn`, never
/// `Decl::Extern`, so a call to an extern function stays dangling here, same
/// as before (#2049 — not supported by `--backend=wasm`).
fn pull_in_missing_prelude_items(
    merged: &mut TirProgram,
    all_fn_decls: &HashMap<String, FnDecl>,
    all_type_decls: &HashMap<String, TypeDecl>,
    all_method_decls: &HashMap<(String, String), FnDecl>,
    expr_types: &HashMap<Span, Ty>,
) {
    let mut known_fns: HashSet<String> = merged.fns.iter().map(|f| f.name.clone()).collect();
    let mut known_types: HashSet<String> = merged.types.iter().map(|t| t.name.clone()).collect();
    let mut known_methods: HashSet<(String, String)> = merged
        .fns
        .iter()
        .filter_map(|f| f.receiver_type.clone().map(|r| (r, f.name.clone())))
        .collect();

    // `merged.types`/`merged.fns` already contain whatever the entry program
    // directly named (e.g. `let logger: Logger = ...` pulls `Logger`'s own
    // `TirTypeDecl` in during the very first `lower()`, before this function
    // ever runs) — `known_types` above already marks those as known, so the
    // main worklist below never re-visits them and never sees their field
    // types. Seed once from the types already present so e.g. `Logger`'s
    // `fd: Fd` field still pulls `Fd` in (#2056).
    let mut initial_type_seed: HashSet<String> = HashSet::new();
    for td in &merged.types {
        type_decl_field_refs(td, &mut initial_type_seed);
    }
    pull_in_types(
        merged,
        &mut known_types,
        initial_type_seed,
        all_type_decls,
        expr_types,
    );

    let mut frontier: Vec<TirFn> = merged.fns.clone();

    while !frontier.is_empty() {
        let mut collector = RefCollector::default();
        for f in &frontier {
            collector.visit_tir_block(&f.body);
        }

        let mut type_seed: HashSet<String> = HashSet::new();
        for f in &frontier {
            fn_type_refs(f, &mut type_seed);
        }
        for name in &collector.variant_refs {
            if let Some((type_name, _)) = name.split_once("::") {
                type_seed.insert(type_name.to_string());
            }
        }
        pull_in_types(
            merged,
            &mut known_types,
            type_seed,
            all_type_decls,
            expr_types,
        );

        let mut newly_added: Vec<TirFn> = Vec::new();
        for name in collector.fn_calls {
            if known_fns.contains(&name) {
                continue;
            }
            known_fns.insert(name.clone());

            // `emit_expr`'s `TirExprKind::FnCall` match hardcodes these two
            // names to WASI runtime shims (`$mvl_println`/`$mvl_eprintln`)
            // rather than ever calling a real `$println`/`$eprintln`
            // function — pulling in their `std/core.mvl` bodies would drag
            // in `write`/`stdout`/`stderr`, which the WASM backend doesn't
            // support standalone, breaking every program that prints.
            if name == "println" || name == "eprintln" {
                continue;
            }

            let Some(fd) = all_fn_decls.get(&name) else {
                continue; // Not a plain fn decl — extern, or unresolved (checker already flagged it).
            };

            let synthetic = Program {
                declarations: vec![Decl::Fn(fd.clone())],
                span: fd.span,
            };
            let syn_fns = mvl::mvl::passes::mono::collect_fns([&synthetic]);
            let syn_mono = mvl::mvl::passes::mono::monomorphize(&synthetic, &syn_fns, expr_types);
            let syn_tir = mvl::mvl::ir::lower::lower(&synthetic, &syn_mono, expr_types);

            // A `builtin fn` (e.g. `write(fd: Fd, ...)`) never gets its body
            // pulled in below — but its *signature* can still name a type
            // (`Fd`) nothing else ever references directly, so pull that in
            // regardless of whether the fn itself is eligible for lowering
            // (#2056).
            let mut sig_types = HashSet::new();
            for f in &syn_tir.fns {
                fn_type_refs(f, &mut sig_types);
            }
            pull_in_types(
                merged,
                &mut known_types,
                sig_types,
                all_type_decls,
                expr_types,
            );

            // Generics/methods/builtins are handled by their own dispatch
            // paths — `emit_program`'s `fns` filter drops them regardless.
            if !fd.type_params.is_empty() || fd.receiver_type.is_some() || fd.is_builtin {
                continue;
            }
            newly_added.extend(syn_tir.fns);
        }

        // Extension-method dot-calls (`logger.info(...)`) — invisible to the
        // `TirExprKind::FnCall` walk above, since a `MethodCall` never sets
        // `name` to the bare method name. Without this, any prelude type
        // (e.g. `std.log`'s `Logger`) whose methods aren't directly `use`d as
        // free functions never gets its method bodies lowered, and the
        // dot-call falls through the WASM backend's `is_struct_method_call`
        // guard to the generic "unsupported method call" stub (#2056).
        for (recv, method, recv_ty) in collector.method_refs {
            let key = (recv.clone(), method.clone());
            if known_methods.contains(&key) {
                continue;
            }
            known_methods.insert(key.clone());

            // The emitter lowers this one itself, so a lowered std body would
            // never be called. Skipping keeps dead functions — some of which
            // stub to `unreachable` and read as missing support — out of the
            // module (#2014).
            if emitter_handles_method_natively(&recv_ty, &method) {
                continue;
            }

            let Some(fd) = all_method_decls.get(&key) else {
                continue; // Not a plain extension-method decl — builtin, or unresolved.
            };

            let synthetic = Program {
                declarations: vec![Decl::Fn(fd.clone())],
                span: fd.span,
            };
            let syn_fns = mvl::mvl::passes::mono::collect_fns([&synthetic]);
            let syn_mono = mvl::mvl::passes::mono::monomorphize(&synthetic, &syn_fns, expr_types);
            let syn_tir = mvl::mvl::ir::lower::lower(&synthetic, &syn_mono, expr_types);

            let mut sig_types = HashSet::new();
            for f in &syn_tir.fns {
                fn_type_refs(f, &mut sig_types);
            }
            pull_in_types(
                merged,
                &mut known_types,
                sig_types,
                all_type_decls,
                expr_types,
            );

            // Generic extension methods ARE lowered (#2014), unlike generic
            // plain fns at the `fn_calls` loop above. `emit_program`'s
            // `generic_ext_methods` bucket monomorphizes them per call site, so
            // the generic TirFn needs to reach `merged.fns` to be found — this
            // is what makes `xs.flatten()` / `xs.map(f)` emit a real body
            // instead of `unreachable`. Their own bodies then join the frontier,
            // so a method calling another method (`first` → `self.get`) pulls
            // its callee in transitively.
            if fd.is_builtin {
                continue;
            }
            newly_added.extend(syn_tir.fns);
        }

        merged.fns.extend(newly_added.iter().cloned());
        frontier = newly_added;
    }
}

/// Types the WASM backend cannot construct/destructure in transpiled MVL,
/// for [`loader::load_rust_backed_stdlib_fns`]. Unlike
/// [`loader::LLVM_OPAQUE_PTR_TYPES`], this excludes `Path` — the WASM
/// backend represents `Path` as a plain `{ inner: String }` struct (see the
/// `write_file`/`path_exists`/etc. call sites in `wasm_text.rs`, which
/// unwrap `.inner` the same way they unwrap `Fd.inner`), so `path`/`join`'s
/// real MVL bodies can and must be pulled in normally (#2100).
const WASM_OPAQUE_PTR_TYPES: &[&str] = &["TcpListener", "TcpStream", "Stdout", "Stderr"];

/// `Fd`/`IoError` (and `stdout`/`stderr`/`write`, etc.) are registered
/// directly in the checker (`register_builtins`, `checker/context.rs`) as
/// always-visible without a `use std.io` import — the same tier as
/// `println`/`eprintln`. Nothing in the prelude-assembly path mirrors that:
/// `load_implicit_prelude` only loads a fixed pure-MVL set (core/strings/
/// lists/effects), and `load_rust_backed_stdlib_fns` only loads modules a
/// program's own `use` statements name. A program that reaches `Fd`
/// transitively — `use std.log` alone, since `Logger.fd: Fd` and `std/log.mvl`
/// itself never writes `use std.io` — never gets `io.mvl`'s type
/// declarations into `all_type_decls`, so `pull_in_types` can never resolve
/// `Fd` even though the checker already resolved it fine (#2056).
///
/// Only `Decl::Type` entries are kept — `stdout`/`stderr`/`write`/etc.'s
/// `Decl::Fn` entries are deliberately left out: the checker already
/// registers those names as builtins, and the WASM backend's codegen
/// (`emit_expr`'s `write`/`stdout`/`stderr`/`now` special cases) matches on
/// the literal call name regardless of whether a separate `FnDecl` exists,
/// so re-adding them here would only risk a duplicate-declaration conflict
/// in `check_with_prelude` for no benefit.
///
/// Scoped to the WASM backend only — `load_implicit_prelude` is shared by
/// every backend and CLI command; broadening it risks shifting behavior
/// (proof obligations, mutation targets, error text) for unrelated commands.
fn load_io_types_prelude() -> Program {
    let content = stdlib::stdlib_content("io.mvl").unwrap_or_else(|| {
        panic!("stdlib file `io.mvl` not found — run `make install` or `mvl self install` to install the stdlib")
    });
    let (mut parser, _) = Parser::new(&content);
    let mut prog = parser.parse_program();
    prog.declarations.retain(|d| matches!(d, Decl::Type(_)));
    prog
}

/// Append `load_io_types_prelude()` unless a program that directly `use`d
/// `std.io` (via `load_rust_backed_stdlib_fns`) already loaded its
/// declarations — avoids handing `check_with_prelude` two `Decl::Type("Fd")`
/// entries across different `Program`s in the same prelude slice.
fn ensure_io_types_prelude(prelude: &mut Vec<Program>) {
    let already_loaded = prelude.iter().any(|p| {
        p.declarations
            .iter()
            .any(|d| matches!(d, Decl::Type(td) if td.name == "Fd"))
    });
    if !already_loaded {
        prelude.push(load_io_types_prelude());
    }
}

/// Top-level name of a `Decl::Fn`/`Decl::Type`, for the dedup check in
/// [`ensure_transitive_rust_backed_stdlib`].
fn decl_name(d: &Decl) -> Option<String> {
    match d {
        Decl::Fn(fd) => Some(fd.name.clone()),
        Decl::Type(td) => Some(td.name.clone()),
        _ => None,
    }
}

/// `load_rust_backed_stdlib_fns` only scans the `use` statements of the
/// programs handed to it directly — it never looks inside a module *those*
/// programs pulled in. `std/log.mvl` (itself loaded into `prelude` via
/// `load_full_prelude` because the entry program did `use std.log`) has its
/// own `use std.time.{now, format_instant}`, but nothing re-scans `log.mvl`
/// for that: the entry program's own `use` list never named `std.time`, so
/// `format_instant`'s real MVL body never lands in `all_fn_decls`, leaving a
/// dangling `call $format_instant` (#2056).
///
/// Re-scans `prog` + the current `prelude` (which now includes whatever the
/// first pass and `load_full_prelude` already loaded) and folds in anything
/// genuinely new, bounded to a few rounds — RUST_BACKED_STDLIB is a short,
/// shallow list (`io`, `net`, `process`, `random`, `regex`, `time`), so this
/// converges in one or two rounds in practice; the bound is just a backstop
/// against surprises, not a real depth requirement.
fn ensure_transitive_rust_backed_stdlib(prog: &Program, prelude: &mut Vec<Program>) {
    for _ in 0..4 {
        let mut scan: Vec<Program> = vec![prog.clone()];
        scan.extend(prelude.iter().cloned());
        let found = loader::load_rust_backed_stdlib_fns(&scan, WASM_OPAQUE_PTR_TYPES);

        let existing_names: HashSet<String> = prelude
            .iter()
            .flat_map(|p| p.declarations.iter())
            .filter_map(decl_name)
            .collect();

        let mut added_any = false;
        for np in found {
            let brings_new = np
                .declarations
                .iter()
                .filter_map(decl_name)
                .any(|n| !existing_names.contains(&n));
            if brings_new {
                prelude.push(np);
                added_any = true;
            }
        }
        if !added_any {
            break;
        }
    }
}

/// Lower `prog` (with prelude) to TIR and emit a WAT string.
fn compile_wat(prog: &Program, module_name: &str, assert_mode: AssertMode) -> String {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(load_full_prelude(
        std::iter::once(prog),
        PreludeMode::Transpile,
    ));
    prelude.extend(loader::load_rust_backed_stdlib_fns(
        std::slice::from_ref(prog),
        WASM_OPAQUE_PTR_TYPES,
    ));
    ensure_io_types_prelude(&mut prelude);
    ensure_transitive_rust_backed_stdlib(prog, &mut prelude);

    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_prelude(&prelude, prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            // Rendered, not `{err:?}` — the Debug dump printed the whole
            // internal variant plus a raw Span, which is unreadable next to the
            // backend error it precedes (#2017). Kept as a warning rather than
            // a hard failure because mvlr drives this path with a synthesized
            // `fn main` whose effect set is deliberately not sound.
            let span = err.span();
            eprintln!(
                "warning: [REQ{}] {} (line {}, col {})",
                err.requirement_number(),
                err.message(),
                span.line,
                span.col
            );
        }
    }
    expr_types.extend(check_result.expr_types);

    let all_fns = mvl::mvl::passes::mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
    let all_types = collect_type_decls(std::iter::once(prog).chain(prelude.iter()));
    let all_methods = collect_method_decls(std::iter::once(prog).chain(prelude.iter()));
    let mono = mvl::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
    let mut entry_tir = mvl::mvl::ir::lower::lower(prog, &mono, &expr_types);
    pull_in_missing_prelude_items(
        &mut entry_tir,
        &all_fns,
        &all_types,
        &all_methods,
        &expr_types,
    );

    let mut compiler = WasmTextCompiler::new();
    compiler.assert_mode = assert_mode;
    let wat = compiler.emit_program(&entry_tir, module_name);
    warn_about_stubs(&compiler, module_name);
    wat
}

/// Warn about functions the emitter stubbed to `unreachable`.
///
/// Without this, an incomplete build is silent: the body is discarded, the
/// module still assembles, `wasm-tools parse` is happy, and the CLI exits 0.
/// The program then traps at runtime only if the stubbed function is actually
/// reached — so a gap can sit unnoticed for as long as nobody exercises that
/// path. `List[T]::push` had no dispatch arm at all and every file using it
/// still "compiled" (#2014).
///
/// A warning rather than an error: stubbing sibling functions on purpose is how
/// partial WASM support has been shipped incrementally (see the emitter's own
/// header notes), so failing here would regress working workflows. Callers that
/// want a hard gate can compare `stubbed_fns()` against an expected set —
/// `make wasm-stub-report` does exactly that over the corpus.
fn warn_about_stubs(compiler: &WasmTextCompiler, source_label: &str) {
    if let Some(msg) = stub_warning_message(&compiler.stubbed_fns(), source_label) {
        eprint!("{msg}");
    }
}

/// The phrase `make wasm-stub-report` greps for in this warning.
///
/// The gate is a `grep` over stderr, so the wording is load-bearing: rewording
/// it without updating the Makefile would turn the gate into a permanent
/// "0 stubs" pass with every unit test still green. `stub_warning_names_each_fn`
/// pins both halves.
pub const STUB_WARNING_PHRASE: &str = "compiled to `unreachable`";

/// Rendered warning for a stubbed module, or `None` when nothing stubbed.
///
/// Split out from `warn_about_stubs` so the exact text can be asserted without
/// capturing stderr.
fn stub_warning_message(stubbed: &[String], source_label: &str) -> Option<String> {
    if stubbed.is_empty() {
        return None;
    }
    let mut msg = format!(
        "warning: {source_label}: {} function(s) {STUB_WARNING_PHRASE} \
         because the WASM backend does not support something in their bodies. \
         Calling any of them traps at runtime:\n",
        stubbed.len()
    );
    for name in stubbed {
        msg.push_str(&format!("  - {name}\n"));
    }
    Some(msg)
}

/// Merge sibling `TirProgram`s into one flat program.
///
/// The WASM emitter (`WasmTextCompiler::emit_program`) derives everything —
/// functions, types, actors — from a single `TirProgram`'s aggregate `Vec`
/// fields rather than tracking per-module boundaries, so cross-file `use`
/// imports resolve simply by concatenating the lowered siblings into the
/// entry program before emission (#2027).
fn merge_tir_programs(programs: &[TirProgram]) -> TirProgram {
    let mut merged = TirProgram::default();
    for p in programs {
        merged.fns.extend(p.fns.iter().cloned());
        merged.types.extend(p.types.iter().cloned());
        merged.externs.extend(p.externs.iter().cloned());
        merged.actors.extend(p.actors.iter().cloned());
        merged.impls.extend(p.impls.iter().cloned());
        merged.consts.extend(p.consts.iter().cloned());
        merged.uses.extend(p.uses.iter().cloned());
        merged.effect_decls.extend(p.effect_decls.iter().cloned());
        merged.label_decls.extend(p.label_decls.iter().cloned());
        merged.relabel_decls.extend(p.relabel_decls.iter().cloned());
    }
    merged
}

/// Lower `prog` and any sibling modules in the same directory to TIR, merge
/// them into a single flat program, and emit WAT (#2027).
///
/// Mirrors `llvm_text.rs`'s `prepare_llvm_text_tir_multi`: siblings are
/// checked with the entry + all *other* siblings as prelude (Go model —
/// same-dir files share declarations without explicit `use` imports), then
/// each is lowered with its own `expr_types`. Falls back to the single-file
/// path when there are no sibling modules.
fn compile_wat_multi(
    prog: &Program,
    path: &str,
    module_name: &str,
    assert_mode: AssertMode,
) -> String {
    let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let sibling_modules = loader::load_sibling_modules_transitive(prog, entry_dir);

    // Fail loudly, before lowering/emission, if entry+siblings declare a
    // free function with the same name — `emit_program` derives everything
    // from one flat `TirProgram`, so a collision here would otherwise only
    // surface as an opaque `wasm-tools parse` "duplicate func identifier"
    // error at assembly time (#2036).
    let dup_labeled_siblings: Vec<(&str, &Program)> = sibling_modules
        .iter()
        .map(|(_, sib_path, p)| (sib_path.as_str(), p))
        .collect();
    let dups = loader::find_duplicate_free_fn_names((path, prog), &dup_labeled_siblings);
    if !dups.is_empty() {
        for (name, (first_file, first_span), (second_file, second_span)) in &dups {
            eprintln!(
                "error: duplicate function `{name}`\n  first declared at {first_file}:{}:{}\n  again declared at {second_file}:{}:{}",
                first_span.line, first_span.col, second_span.line, second_span.col
            );
        }
        eprintln!(
            "Same-directory modules compiled to --backend=wasm share one flat symbol space; rename one of the above."
        );
        process::exit(1);
    }

    if sibling_modules.is_empty() {
        return compile_wat(prog, module_name, assert_mode);
    }

    let sibling_progs: Vec<&Program> = sibling_modules.iter().map(|(_, _, p)| p).collect();

    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(load_full_prelude(
        std::iter::once(prog).chain(sibling_progs.iter().copied()),
        PreludeMode::Transpile,
    ));
    prelude.extend(loader::load_rust_backed_stdlib_fns(
        std::slice::from_ref(prog),
        WASM_OPAQUE_PTR_TYPES,
    ));
    ensure_io_types_prelude(&mut prelude);
    ensure_transitive_rust_backed_stdlib(prog, &mut prelude);

    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_two_preludes(&prelude, &sibling_progs, prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            // See `compile_wat` — warning, not a hard failure (#2017).
            let span = err.span();
            eprintln!(
                "warning: [REQ{}] {} (line {}, col {})",
                err.requirement_number(),
                err.message(),
                span.line,
                span.col
            );
        }
    }
    expr_types.extend(check_result.expr_types);

    let all_fns = mvl::mvl::passes::mono::collect_fns(
        std::iter::once(prog)
            .chain(sibling_progs.iter().copied())
            .chain(prelude.iter()),
    );
    let all_types = collect_type_decls(
        std::iter::once(prog)
            .chain(sibling_progs.iter().copied())
            .chain(prelude.iter()),
    );
    let all_methods = collect_method_decls(
        std::iter::once(prog)
            .chain(sibling_progs.iter().copied())
            .chain(prelude.iter()),
    );
    let mono = mvl::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
    let entry_tir = mvl::mvl::ir::lower::lower(prog, &mono, &expr_types);

    // Each sibling is checked with the entry + all OTHER siblings as its
    // prelude, then lowered with its own expr_types.
    let sibling_tirs: Vec<TirProgram> = sibling_modules
        .iter()
        .enumerate()
        .map(|(i, (_, _, sibling))| {
            let (before, rest) = sibling_modules.split_at(i);
            let after = &rest[1..];
            let sibling_prelude: Vec<&Program> = std::iter::once(prog)
                .chain(before.iter().map(|(_, _, p)| p))
                .chain(after.iter().map(|(_, _, p)| p))
                .collect();
            let sib_check = checker::check_with_two_preludes(&prelude, &sibling_prelude, sibling);
            let mut sib_types = checker::collect_prelude_expr_types(&prelude);
            sib_types.extend(sib_check.expr_types);

            let sib_all_fns =
                mvl::mvl::passes::mono::collect_fns(std::iter::once(sibling).chain(prelude.iter()));
            let sib_mono = mvl::mvl::passes::mono::monomorphize(sibling, &sib_all_fns, &sib_types);
            mvl::mvl::ir::lower::lower(sibling, &sib_mono, &sib_types)
        })
        .collect();

    let mut merged = merge_tir_programs(
        &std::iter::once(entry_tir)
            .chain(sibling_tirs)
            .collect::<Vec<_>>(),
    );
    // Pull in prelude functions/types referenced by name but never lowered —
    // see `pull_in_missing_prelude_items` (#2045, #2046, #2056).
    pull_in_missing_prelude_items(&mut merged, &all_fns, &all_types, &all_methods, &expr_types);

    let mut compiler = WasmTextCompiler::new();
    compiler.assert_mode = assert_mode;
    let wat = compiler.emit_program(&merged, module_name);
    warn_about_stubs(&compiler, module_name);
    wat
}

/// Resolve `path` to an entry `.mvl` file — `path` itself if it's a file, or
/// `main.mvl` / `lib.mvl` within it if it's a directory (#2027).
fn resolve_entry_path(path: &str) -> String {
    let dir = Path::new(path);
    if !dir.is_dir() {
        return path.to_string();
    }
    ["main.mvl", "lib.mvl"]
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            eprintln!("No main.mvl / lib.mvl found in {path}");
            process::exit(1);
        })
        .display()
        .to_string()
}

/// `mvl build --backend=wasm <file>` — write `<stem>.wat`.
pub(super) fn build_project_wasm(path: &str, assert_mode: AssertMode, target: &str) {
    exit_if_wasm_browser_unimplemented(target);
    let file_path = resolve_entry_path(path);
    let (prog, _src) = super::parse_or_exit(&file_path);
    let module_name = loader::stem(&file_path);
    let wat = compile_wat_multi(&prog, &file_path, &module_name, assert_mode);
    let out_path = format!("{module_name}.wat");
    fs::write(&out_path, &wat).unwrap_or_else(|e| {
        eprintln!("error: cannot write {out_path}: {e}");
        process::exit(1);
    });
    println!("WAT written to: {out_path}");
}

// ── Test harness — mirrors cmd_test_llvm_text ─────────────────────────────────

/// Result of running one WASM test case, output pre-formatted so parallel
/// workers can print results in deterministic order after joining.
struct CaseResult {
    passed: bool,
    output: String,
    err_output: String,
}

/// Run one case: parse, lower, emit WAT, assemble, run under wasmtime, compare.
#[allow(clippy::too_many_arguments)]
fn run_one_case(
    file: &Path,
    expected: &str,
    is_pattern: bool,
    wasm_tools_bin: &Path,
    wasmtime_bin: &Path,
    runtime_wasm: Option<&Path>,
    quiet: bool,
    verbose: bool,
) -> CaseResult {
    let file_str = file.display().to_string();
    let module_name = loader::stem(&file_str);

    let src = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (read): {file_str}: {e}\n"),
            }
        }
    };
    let (mut parser, lex_errs) = Parser::new(&src);
    if !lex_errs.is_empty() {
        return CaseResult {
            passed: false,
            output: String::new(),
            err_output: format!("  FAIL (lex): {file_str}\n"),
        };
    }
    let prog = parser.parse_program();
    if !parser.errors().is_empty() {
        return CaseResult {
            passed: false,
            output: String::new(),
            err_output: format!("  FAIL (parse): {file_str}\n"),
        };
    }

    let wat = compile_wat_multi(&prog, &file_str, &module_name, AssertMode::Always);

    let wat_tmp = match tempfile::NamedTempFile::with_suffix(".wat") {
        Ok(t) => t,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (tempfile): {file_str}: {e}\n"),
            }
        }
    };
    if let Err(e) = fs::write(wat_tmp.path(), &wat) {
        return CaseResult {
            passed: false,
            output: String::new(),
            err_output: format!("  FAIL (write WAT): {file_str}: {e}\n"),
        };
    }

    let wasm_tmp = match tempfile::NamedTempFile::with_suffix(".wasm") {
        Ok(t) => t,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (tempfile wasm): {file_str}: {e}\n"),
            }
        }
    };

    let assemble = process::Command::new(wasm_tools_bin)
        .arg("parse")
        .arg(wat_tmp.path())
        .arg("-o")
        .arg(wasm_tmp.path())
        .output();
    match assemble {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let mut out = format!("\n  FAIL (assemble): {file_str}\n");
            if verbose {
                out.push_str(&format!("    wasm-tools: {stderr}\n"));
                out.push_str("    --- WAT ---\n");
                for line in wat.lines().take(40) {
                    out.push_str(&format!("    {line}\n"));
                }
            } else {
                let first = stderr.lines().next().unwrap_or("");
                out.push_str(&format!("    {first}\n"));
            }
            return CaseResult {
                passed: false,
                output: out,
                err_output: String::new(),
            };
        }
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (wasm-tools spawn): {file_str}: {e}\n"),
            }
        }
    }

    // `wasm-tools parse` only assembles — it accepts modules that are
    // structurally well-formed but type-invalid (a body that declares
    // `(result i32)` and pushes nothing assembles fine). Validation is a
    // separate pass, and skipping it let three classes of silently-invalid
    // module through: an uninterned literal leaving the stack short, a
    // dangling `call $Foo__Int` nobody emitted, and an undeclared local in a
    // monomorphized body. Each exited 0 with zero stubs reported (#2014).
    let validate = process::Command::new(wasm_tools_bin)
        .arg("validate")
        .arg(wasm_tmp.path())
        .output();
    match validate {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let mut out = format!("\n  FAIL (validate): {file_str}\n");
            if verbose {
                out.push_str(&format!("    wasm-tools: {stderr}\n"));
                out.push_str("    --- WAT ---\n");
                for line in wat.lines().take(40) {
                    out.push_str(&format!("    {line}\n"));
                }
            } else {
                let first = stderr.lines().next().unwrap_or("");
                out.push_str(&format!("    {first}\n"));
            }
            return CaseResult {
                passed: false,
                output: out,
                err_output: String::new(),
            };
        }
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (wasm-tools spawn): {file_str}: {e}\n"),
            }
        }
    }

    let mut wasmtime_cmd = process::Command::new(wasmtime_bin);
    wasmtime_cmd.arg("run");
    if let Some(runtime) = runtime_wasm {
        wasmtime_cmd
            .arg("--preload")
            .arg(format!("runtime={}", runtime.display()));
    }
    let run = wasmtime_cmd.arg(wasm_tmp.path()).output();
    let output = match run {
        Ok(o) => o,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (wasmtime spawn): {file_str}: {e}\n"),
            }
        }
    };

    let actual = String::from_utf8_lossy(&output.stdout);
    let actual_trimmed = actual.trim_end_matches('\n');
    let expected_trimmed = expected.trim_end_matches('\n');

    let matched = if is_pattern {
        lli::glob_match(expected_trimmed, actual_trimmed)
    } else {
        actual_trimmed == expected_trimmed
    };

    if matched {
        let out = if verbose {
            format!("  PASS: {file_str}\n")
        } else if !quiet {
            ".".to_string()
        } else {
            String::new()
        };
        CaseResult {
            passed: true,
            output: out,
            err_output: String::new(),
        }
    } else {
        let mut out = String::new();
        if !quiet {
            out.push_str(&format!("\n  FAIL: {file_str}\n"));
            if is_pattern {
                out.push_str(&format!("    pattern:  {expected_trimmed:?}\n"));
            } else {
                out.push_str(&format!("    expected: {expected_trimmed:?}\n"));
            }
            out.push_str(&format!("    got:      {actual_trimmed:?}\n"));
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let first = stderr.lines().next().unwrap_or("");
                if !first.is_empty() {
                    out.push_str(&format!("    trap:     {first}\n"));
                }
            }
            if verbose && !wat.is_empty() {
                out.push_str("    --- WAT ---\n");
                for line in wat.lines().take(40) {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
        CaseResult {
            passed: false,
            output: out,
            err_output: String::new(),
        }
    }
}

/// Bail out with a clear message until the `wasm-browser` target's JS-host
/// runtime and emitter path exist (tracked in #2093). `wasi` is a no-op here.
fn exit_if_wasm_browser_unimplemented(target: &str) {
    if target == "wasm-browser" {
        eprintln!(
            "error: --target=wasm-browser is not yet implemented (tracked in #2093) — use --target=wasi (default)"
        );
        process::exit(1);
    }
}

/// `mvl test <path> --backend=wasm` — discover files with `fn main` +
/// `// expect:` annotations, emit WAT, run under wasmtime, compare output.
pub(super) fn cmd_test_wasm(path: &str, quiet: bool, verbose: bool, target: &str) {
    exit_if_wasm_browser_unimplemented(target);
    let wasm_tools_bin = which("wasm-tools").unwrap_or_else(|| {
        eprintln!("error: `wasm-tools` not found — install with 'cargo install wasm-tools'");
        process::exit(1);
    });
    let wasmtime_bin = which("wasmtime").unwrap_or_else(|| {
        eprintln!("error: `wasmtime` not found — see https://wasmtime.dev/");
        process::exit(1);
    });
    let runtime_wasm = lli::find_mvl_runtime_wasm_lib();

    let all_mvl = loader::mvl_files_all(path);
    let mut test_cases: Vec<(PathBuf, String, bool)> = Vec::new();

    for file in &all_mvl {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !src.contains("fn main(") {
            continue;
        }
        if let Some(pat) = lli::parse_expect_pattern_annotation(&src) {
            test_cases.push((file.clone(), pat, true));
        } else if let Some(expected) = lli::parse_expect_annotation(&src) {
            test_cases.push((file.clone(), expected, false));
        }
    }

    if test_cases.is_empty() {
        if !quiet {
            println!("No WASM test cases found (files with `fn main` + `// expect:` annotations).");
        }
        return;
    }

    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(test_cases.len());
    let chunk_size = test_cases.len().div_ceil(parallelism).max(1);

    if !quiet {
        println!(
            "WASM backend: {} test file(s) across {} worker(s)",
            test_cases.len(),
            parallelism
        );
    }

    let wasm_tools_ref: &Path = &wasm_tools_bin;
    let wasmtime_ref: &Path = &wasmtime_bin;
    let runtime_wasm_ref: Option<&Path> = runtime_wasm.as_deref();

    let results: Vec<CaseResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = test_cases
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(f, e, p)| {
                            run_one_case(
                                f,
                                e,
                                *p,
                                wasm_tools_ref,
                                wasmtime_ref,
                                runtime_wasm_ref,
                                quiet,
                                verbose,
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("wasm test worker panicked"))
            .collect()
    });

    let mut passed = 0usize;
    let mut failed = 0usize;
    for r in &results {
        if r.passed {
            passed += 1;
        } else {
            failed += 1;
        }
        if !r.err_output.is_empty() {
            eprint!("{}", r.err_output);
        }
        if !r.output.is_empty() {
            print!("{}", r.output);
        }
    }

    if !quiet && !verbose {
        println!();
    }
    if failed > 0 {
        eprintln!("\n{passed} passed, {failed} failed");
        process::exit(1);
    } else if !quiet {
        println!("{passed} passed, 0 failed");
    }
}

/// Locate a binary on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let output = process::Command::new("which").arg(name).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod stub_warning_tests {
    use super::*;

    #[test]
    fn stub_warning_names_each_fn() {
        let msg = stub_warning_message(&["List_take__Str".to_string()], "demo.mvl")
            .expect("a stubbed fn must warn");
        // The Makefile greps for this phrase and then for the `  - ` name lines.
        assert!(msg.contains(STUB_WARNING_PHRASE), "{msg}");
        assert!(msg.contains("\n  - List_take__Str\n"), "{msg}");
        assert!(
            msg.starts_with("warning: demo.mvl: 1 function(s) "),
            "{msg}"
        );
    }

    #[test]
    fn no_stubs_means_no_warning() {
        assert!(stub_warning_message(&[], "demo.mvl").is_none());
    }
}
