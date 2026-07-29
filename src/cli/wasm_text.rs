// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl build --backend=wasm` and `mvl test --backend=wasm` drivers (#1571).
//!
//! Reuses the same prelude/checker/TIR pipeline as the llvm_text backend.
//! The test harness mirrors `cmd_test_llvm_text`: discover `fn main` +
//! `// expect:` files, emit WAT, assemble via `wasm-tools`, run via
//! `wasmtime`, compare stdout to the expected string.

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::wasm_text::WasmTextCompiler;
use mvl::mvl::backends::{AssertMode, Backend};
use mvl::mvl::checker;
use mvl::mvl::checker::types::Ty;
use mvl::mvl::ir::visit::{walk_tir_expr, Visit};
use mvl::mvl::ir::{TirExpr, TirExprKind, TirFn, TirProgram};
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Decl, FnDecl, Program, TypeDecl};
use mvl::mvl::parser::lexer::Span;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// Collects every free-function name referenced via `TirExprKind::FnCall`,
/// and every qualified enum-variant reference (`TirExprKind::Var("Type::Variant")`)
/// in a function body — used to find prelude functions/types a program
/// references but that never got lowered into the emitted module (#2045,
/// #2046).
#[derive(Default)]
struct RefCollector {
    fn_calls: HashSet<String>,
    variant_refs: HashSet<String>,
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
            _ => {}
        }
        walk_tir_expr(self, e);
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
/// name as a plain local (`local.get $ArgType::Int`, "unknown local").
///
/// The LLVM backend avoids both gaps by lowering *every* prelude module and
/// merging all of it in — too broad here: prelude functions the WASM backend
/// doesn't support yet (e.g. `stdout()`, `exit()`) would break every program
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
    expr_types: &HashMap<Span, Ty>,
) {
    let mut known_fns: HashSet<String> = merged.fns.iter().map(|f| f.name.clone()).collect();
    let mut known_types: HashSet<String> = merged.types.iter().map(|t| t.name.clone()).collect();
    let mut frontier: Vec<TirFn> = merged.fns.clone();

    while !frontier.is_empty() {
        let mut collector = RefCollector::default();
        for f in &frontier {
            collector.visit_tir_block(&f.body);
        }

        for name in collector.variant_refs {
            let Some((type_name, _)) = name.split_once("::") else {
                continue;
            };
            if known_types.contains(type_name) {
                continue;
            }
            known_types.insert(type_name.to_string());

            let Some(td) = all_type_decls.get(type_name) else {
                continue;
            };
            let synthetic = Program {
                declarations: vec![Decl::Type(td.clone())],
                span: td.span,
            };
            let syn_mono =
                mvl::mvl::passes::mono::monomorphize(&synthetic, &HashMap::new(), expr_types);
            let syn_tir = mvl::mvl::ir::lower::lower(&synthetic, &syn_mono, expr_types);
            merged.types.extend(syn_tir.types);
        }

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
            // Generics/methods/builtins are handled by their own dispatch
            // paths — `emit_program`'s `fns` filter drops them regardless.
            if !fd.type_params.is_empty() || fd.receiver_type.is_some() || fd.is_builtin {
                continue;
            }

            let synthetic = Program {
                declarations: vec![Decl::Fn(fd.clone())],
                span: fd.span,
            };
            let syn_fns = mvl::mvl::passes::mono::collect_fns([&synthetic]);
            let syn_mono = mvl::mvl::passes::mono::monomorphize(&synthetic, &syn_fns, expr_types);
            let syn_tir = mvl::mvl::ir::lower::lower(&synthetic, &syn_mono, expr_types);
            newly_added.extend(syn_tir.fns);
        }

        merged.fns.extend(newly_added.iter().cloned());
        frontier = newly_added;
    }
}

/// Lower `prog` (with prelude) to TIR and emit a WAT string.
fn compile_wat(prog: &Program, module_name: &str, assert_mode: AssertMode) -> String {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(load_full_prelude(
        std::iter::once(prog),
        PreludeMode::Transpile,
    ));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));

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
    let mono = mvl::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
    let mut entry_tir = mvl::mvl::ir::lower::lower(prog, &mono, &expr_types);
    pull_in_missing_prelude_items(&mut entry_tir, &all_fns, &all_types, &expr_types);

    let mut compiler = WasmTextCompiler::new();
    compiler.assert_mode = assert_mode;
    compiler.emit_program(&entry_tir, module_name)
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
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));

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
    // see `pull_in_missing_prelude_items` (#2045, #2046).
    pull_in_missing_prelude_items(&mut merged, &all_fns, &all_types, &expr_types);

    let mut compiler = WasmTextCompiler::new();
    compiler.assert_mode = assert_mode;
    compiler.emit_program(&merged, module_name)
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
pub(super) fn build_project_wasm(path: &str, assert_mode: AssertMode) {
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
fn run_one_case(
    file: &Path,
    expected: &str,
    is_pattern: bool,
    wasm_tools_bin: &Path,
    wasmtime_bin: &Path,
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

    let run = process::Command::new(wasmtime_bin)
        .arg("run")
        .arg(wasm_tmp.path())
        .output();
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

/// `mvl test <path> --backend=wasm` — discover files with `fn main` +
/// `// expect:` annotations, emit WAT, run under wasmtime, compare output.
pub(super) fn cmd_test_wasm(path: &str, quiet: bool, verbose: bool) {
    let wasm_tools_bin = which("wasm-tools").unwrap_or_else(|| {
        eprintln!("error: `wasm-tools` not found — install with 'cargo install wasm-tools'");
        process::exit(1);
    });
    let wasmtime_bin = which("wasmtime").unwrap_or_else(|| {
        eprintln!("error: `wasmtime` not found — see https://wasmtime.dev/");
        process::exit(1);
    });

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

    let results: Vec<CaseResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = test_cases
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(f, e, p)| {
                            run_one_case(f, e, *p, wasm_tools_ref, wasmtime_ref, quiet, verbose)
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
