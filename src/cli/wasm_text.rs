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
use mvl::mvl::backends::Backend;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// Lower `prog` (with prelude) to TIR and emit a WAT string.
fn compile_wat(prog: &Program, module_name: &str) -> String {
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
            eprintln!("warning: checker: {err:?}");
        }
    }
    expr_types.extend(check_result.expr_types);

    let all_fns = mvl::mvl::passes::mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
    let mono = mvl::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
    let entry_tir = mvl::mvl::ir::lower::lower(prog, &mono, &expr_types);

    let compiler = WasmTextCompiler::new();
    compiler.emit_program(&entry_tir, module_name)
}

/// `mvl build --backend=wasm <file>` — write `<stem>.wat`.
pub(super) fn build_project_wasm(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let wat = compile_wat(&prog, &module_name);
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

    let wat = compile_wat(&prog, &module_name);

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
