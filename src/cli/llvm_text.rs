// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::checker;
use mvl::mvl::ir::TirProgram;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// Build the prelude, builtin dispatch table, and checker-resolved types
/// for the llvm_text backend.
///
/// Mirrors `prepare_llvm` in `src/cli/llvm.rs` but uses `collect_llvm_text_builtins`
/// instead of the inkwell StdlibSig dispatch table.
///
/// Runs the type checker on the program with prelude context so that
/// `expr_types` are available for TIR lowering (#1302).
/// Exits the process if the checker reports errors — type-incorrect programs
/// must not proceed to LLVM codegen.
fn prepare_llvm_text(
    prog: &Program,
) -> (
    Vec<Program>,
    LlvmTextCompiler,
    std::collections::HashMap<mvl::mvl::parser::lexer::Span, mvl::mvl::checker::types::Ty>,
) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(load_full_prelude(
        std::iter::once(prog),
        PreludeMode::Transpile,
    ));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));
    let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(prog));

    // Run checker to get expression types for the TIR lowering pass (#1302).
    // Checker errors are logged as warnings rather than halting compilation:
    // the checker does not yet have full stdlib coverage, so some valid programs
    // produce false-positive errors (UndefinedFunction, MissingEffect).
    // Missing types propagate as `Ty::Unknown` through TIR lowering.
    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_prelude(&prelude, prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            eprintln!("warning: checker: {err:?}");
        }
    }
    expr_types.extend(check_result.expr_types);

    let compiler = LlvmTextCompiler::with_context(builtins);
    (prelude, compiler, expr_types)
}

/// Compile `prog` to LLVM IR text via the TIR-walking emitter (#1612 Phase 3b).
fn compile_ir(prog: &Program, module_name: &str) -> Result<String, String> {
    let (prelude_tirs, entry_tir, compiler) = prepare_llvm_text_tir(prog);
    compiler.compile_to_ir_with_prelude_tir(&prelude_tirs, &entry_tir, module_name)
}

/// Lower an entry program and its prelude to TIR for the TIR-walking emitter
/// (#1612 Phase 3b).
///
/// Mirrors what `src/mvl/pipeline::transpile_project_with_options` does before
/// invoking the Rust backend: run `mono::collect_fns` + `monomorphize`, then
/// `ir::lower::lower` for the entry program and each prelude module.
///
/// Returns `(prelude_tirs, entry_tir, compiler)`. Callers hand `compiler` to
/// `compile_to_ir_with_prelude_tir`; `expr_types` is a lowering-time input only.
pub(super) fn prepare_llvm_text_tir(
    prog: &Program,
) -> (Vec<TirProgram>, TirProgram, LlvmTextCompiler) {
    let (prelude, compiler, expr_types) = prepare_llvm_text(prog);

    // Lower entry program to TIR.
    let entry_all_fns =
        mvl::mvl::passes::mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
    let entry_mono = mvl::mvl::passes::mono::monomorphize(prog, &entry_all_fns, &expr_types);
    let entry_tir = mvl::mvl::ir::lower::lower(prog, &entry_mono, &expr_types);

    // Lower each prelude program independently (matches Rust backend usage).
    let prelude_tirs: Vec<TirProgram> = prelude
        .iter()
        .map(|p| {
            let all_fns = mvl::mvl::passes::mono::collect_fns([p]);
            let m = mvl::mvl::passes::mono::monomorphize(p, &all_fns, &expr_types);
            mvl::mvl::ir::lower::lower(p, &m, &expr_types)
        })
        .collect();

    (prelude_tirs, entry_tir, compiler)
}

/// Compile an MVL file to LLVM IR text and write the `.ll` file.
/// `mvl build --backend=llvm <file>`
pub(super) fn build_project_llvm_text(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    match compile_ir(&prog, &module_name) {
        Ok(ir) => {
            let out_path = format!("{module_name}.ll");
            fs::write(&out_path, &ir).unwrap_or_else(|e| {
                eprintln!("error: cannot write {out_path}: {e}");
                process::exit(1);
            });
            println!("LLVM IR written to: {out_path}");
        }
        Err(e) => {
            eprintln!("error: llvm codegen failed: {e}");
            process::exit(1);
        }
    }
}

/// Compile a package `llvm.rs` to a cdylib shared library (#811).
///
/// Returns the path to the compiled `.dylib`/`.so` on success, or `None`
/// if compilation fails. The output is placed in a temp directory.
fn compile_llvm_bridge(llvm_rs: &Path) -> Option<PathBuf> {
    let tmp_dir = tempfile::tempdir().ok()?;
    let ext = if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "so"
    };
    let lib_name = format!("libpkg_llvm_bridge.{ext}");
    let output_path = tmp_dir.path().join(&lib_name);
    // Keep the temp directory alive until the caller is done with the library.
    // `forget` prevents cleanup so lli can load the .dylib/.so later.
    std::mem::forget(tmp_dir);

    let status = process::Command::new("rustc")
        .arg("--crate-type=cdylib")
        .arg("--edition=2021")
        .arg("-o")
        .arg(&output_path)
        .arg(llvm_rs)
        .status();

    match status {
        Ok(s) if s.success() => Some(output_path),
        Ok(_) => {
            eprintln!(
                "warning: failed to compile llvm.rs at {} — extern \"c\" symbols may be unresolved",
                llvm_rs.display()
            );
            None
        }
        Err(e) => {
            eprintln!("warning: rustc not found for llvm.rs compilation: {e}");
            None
        }
    }
}

/// Compile an MVL file to LLVM IR and run it via `lli`.
/// `mvl run --backend=llvm <file>`
pub(super) fn run_project_llvm_text(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let ir = match compile_ir(&prog, &module_name) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("error: llvm codegen failed: {e}");
            process::exit(1);
        }
    };

    let lli_bin = lli::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM (brew install llvm)");
        process::exit(1);
    });

    let tmp = tempfile::NamedTempFile::with_suffix(".ll").unwrap_or_else(|e| {
        eprintln!("error: cannot create temp file: {e}");
        process::exit(1);
    });
    fs::write(tmp.path(), &ir).unwrap_or_else(|e| {
        eprintln!("error: cannot write IR: {e}");
        process::exit(1);
    });

    let mut cmd = process::Command::new(&lli_bin);
    if let Some(lib) = lli::find_mvl_runtime_llvm_lib() {
        cmd.arg(format!("--load={}", lib.display()));
    }

    // Discover and load package llvm.rs shared libraries (#811).
    let project_root = Path::new(path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if let Some(llvm_rs) = loader::find_pkg_llvm_bridge(std::slice::from_ref(&prog), &project_root)
    {
        if let Some(lib) = compile_llvm_bridge(&llvm_rs) {
            cmd.arg(format!("--load={}", lib.display()));
        }
    }

    let status = cmd.arg(tmp.path()).status().unwrap_or_else(|e| {
        eprintln!("error: failed to run lli: {e}");
        process::exit(1);
    });
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
}

/// Result of running one LLVM text test case, with any output pre-formatted
/// so the caller can print in deterministic order after parallel workers finish.
struct CaseResult {
    passed: bool,
    output: String,
    err_output: String,
}

/// Run one LLVM text test case: parse, lower, emit IR, run under `lli`, compare.
fn run_one_case(
    file: &Path,
    expected: &str,
    is_pattern: bool,
    lli_bin: &Path,
    runtime_lib: Option<&Path>,
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

    let ir = match compile_ir(&prog, &module_name) {
        Ok(ir) => ir,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (codegen): {file_str}: {e}\n"),
            }
        }
    };

    let tmp = match tempfile::NamedTempFile::with_suffix(".ll") {
        Ok(t) => t,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (tempfile): {file_str}: {e}\n"),
            }
        }
    };
    if let Err(e) = fs::write(tmp.path(), &ir) {
        return CaseResult {
            passed: false,
            output: String::new(),
            err_output: format!("  FAIL (write IR): {file_str}: {e}\n"),
        };
    }

    let mut lli_cmd = process::Command::new(lli_bin);
    if let Some(lib) = runtime_lib {
        lli_cmd.arg(format!("--load={}", lib.display()));
    }
    let output = match lli_cmd.arg(tmp.path()).output() {
        Ok(o) => o,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (lli): {file_str}: {e}\n"),
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
            if verbose && !ir.is_empty() {
                out.push_str("    --- IR ---\n");
                for line in ir.lines().take(40) {
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

/// Run LLVM text backend tests: discover `.mvl` files with `// expect:` annotations,
/// compile via `LlvmTextCompiler`, execute via `lli`, and compare output.
/// `mvl test <path> --backend=llvm`
pub(super) fn cmd_test_llvm_text(path: &str, quiet: bool, verbose: bool) {
    let lli_bin = lli::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM (brew install llvm)");
        process::exit(1);
    });
    let runtime_lib = lli::find_mvl_runtime_llvm_lib();

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
            println!(
                "No LLVM text test cases found (files with `fn main` + `// expect:` annotations)."
            );
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
            "LLVM text backend: {} test file(s) across {} worker(s)",
            test_cases.len(),
            parallelism
        );
    }

    let lli_bin_ref: &Path = &lli_bin;
    let runtime_lib_ref: Option<&Path> = runtime_lib.as_deref();

    let results: Vec<CaseResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = test_cases
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(f, e, p)| {
                            run_one_case(f, e, *p, lli_bin_ref, runtime_lib_ref, quiet, verbose)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("llvm test worker panicked"))
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
        // Newline after progress dots.
        let _ = std::io::Write::flush(&mut std::io::stdout());
        println!();
    }
    println!("{} passed, {} failed", passed, failed);
    if failed > 0 {
        process::exit(1);
    }
}
