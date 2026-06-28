// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::checker;
use mvl::mvl::ir::TirProgram;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
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
/// `expr_types` are available for accurate type dispatch in the emitter (#1302).
/// Exits the process if the checker reports errors — type-incorrect programs
/// must not proceed to LLVM codegen.
fn prepare_llvm_text(prog: &Program) -> (Vec<Program>, LlvmTextCompiler) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        prog,
    )));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));
    let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(prog));

    // Run checker to get expression types for the LLVM emitter (#1302).
    // Checker errors are logged as warnings rather than halting compilation:
    // the checker does not yet have full stdlib coverage, so some valid programs
    // produce false-positive errors (UndefinedFunction, MissingEffect).
    // The emitter falls back to AST inference for spans with missing types.
    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_prelude(&prelude, prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            eprintln!("warning: checker: {err:?}");
        }
    }
    expr_types.extend(check_result.expr_types);

    let compiler = LlvmTextCompiler::with_context(builtins, expr_types);
    (prelude, compiler)
}

/// Lower an entry program and its prelude to TIR for the TIR-walking emitter
/// path (#1612, Phase 3b PR 1).
///
/// Mirrors what `src/mvl/backends/rust.rs::transpile_project_with_options` does
/// before invoking the Rust backend: run `mono::collect_fns` + `monomorphize`,
/// then `ir::lower::lower` for the entry program and each prelude module.
///
/// Returns `(prelude_tirs, entry_tir, compiler)`.  The compiler shares its
/// `builtin_symbols` and `expr_types` with the AST path so call-site dispatch
/// remains identical.
#[allow(dead_code)] // wired into the cross_backend_tir test target in a follow-up
pub(super) fn prepare_llvm_text_tir(
    prog: &Program,
) -> (Vec<TirProgram>, TirProgram, LlvmTextCompiler) {
    let (prelude, compiler) = prepare_llvm_text(prog);

    // Lower entry program to TIR.
    let entry_all_fns = mvl::mvl::passes::mono::collect_fns(
        std::iter::once(prog).chain(prelude.iter()),
    );
    let entry_mono =
        mvl::mvl::passes::mono::monomorphize(prog, &entry_all_fns, &compiler.expr_types);
    let entry_tir = mvl::mvl::ir::lower::lower(prog, &entry_mono, &compiler.expr_types);

    // Lower each prelude program independently (matches Rust backend usage).
    let prelude_tirs: Vec<TirProgram> = prelude
        .iter()
        .map(|p| {
            let all_fns = mvl::mvl::passes::mono::collect_fns([p]);
            let m = mvl::mvl::passes::mono::monomorphize(p, &all_fns, &compiler.expr_types);
            mvl::mvl::ir::lower::lower(p, &m, &compiler.expr_types)
        })
        .collect();

    (prelude_tirs, entry_tir, compiler)
}

/// Compile an MVL file to LLVM IR text and write the `.ll` file.
/// `mvl build --backend=llvm <file>`
pub(super) fn build_project_llvm_text(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, compiler) = prepare_llvm_text(&prog);
    match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
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
    let (prelude, compiler) = prepare_llvm_text(&prog);
    let ir = match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
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

/// Run LLVM text backend tests: discover `.mvl` files with `// expect:` annotations,
/// compile via `LlvmTextCompiler`, execute via `lli`, and compare output.
/// `mvl test <path> --backend=llvm`
pub(super) fn cmd_test_llvm_text(path: &str, quiet: bool, verbose: bool) {
    let lli_bin = lli::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM (brew install llvm)");
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
            println!(
                "No LLVM text test cases found (files with `fn main` + `// expect:` annotations)."
            );
        }
        return;
    }

    if !quiet {
        println!("LLVM text backend: {} test file(s)", test_cases.len());
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (file, expected, is_pattern) in &test_cases {
        let file_str = file.display().to_string();
        let module_name = loader::stem(&file_str);

        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  FAIL (read): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };
        let (mut parser, lex_errs) = Parser::new(&src);
        if !lex_errs.is_empty() {
            eprintln!("  FAIL (lex): {file_str}");
            failed += 1;
            continue;
        }
        let prog = parser.parse_program();
        if !parser.errors().is_empty() {
            eprintln!("  FAIL (parse): {file_str}");
            failed += 1;
            continue;
        }

        let (prelude, compiler) = prepare_llvm_text(&prog);
        let ir = match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
            Ok(ir) => ir,
            Err(e) => {
                eprintln!("  FAIL (codegen): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };

        let tmp = match tempfile::NamedTempFile::with_suffix(".ll") {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  FAIL (tempfile): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };
        if let Err(e) = fs::write(tmp.path(), &ir) {
            eprintln!("  FAIL (write IR): {file_str}: {e}");
            failed += 1;
            continue;
        }

        let mut lli_cmd = process::Command::new(&lli_bin);
        if let Some(lib) = lli::find_mvl_runtime_llvm_lib() {
            lli_cmd.arg(format!("--load={}", lib.display()));
        }
        let output = match lli_cmd.arg(tmp.path()).output() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("  FAIL (lli): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };

        let actual = String::from_utf8_lossy(&output.stdout);
        let actual_trimmed = actual.trim_end_matches('\n');
        let expected_trimmed = expected.trim_end_matches('\n');

        let matched = if *is_pattern {
            lli::glob_match(expected_trimmed, actual_trimmed)
        } else {
            actual_trimmed == expected_trimmed
        };

        if matched {
            if verbose {
                println!("  PASS: {file_str}");
            } else if !quiet {
                print!(".");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            passed += 1;
        } else {
            if !quiet {
                println!("\n  FAIL: {file_str}");
                if *is_pattern {
                    println!("    pattern:  {expected_trimmed:?}");
                } else {
                    println!("    expected: {expected_trimmed:?}");
                }
                println!("    got:      {actual_trimmed:?}");
                if verbose && !ir.is_empty() {
                    println!("    --- IR ---");
                    for line in ir.lines().take(40) {
                        println!("    {line}");
                    }
                }
            }
            failed += 1;
        }
    }

    if !quiet && !verbose {
        println!();
    }
    println!("{} passed, {} failed", passed, failed);
    if failed > 0 {
        process::exit(1);
    }
}
