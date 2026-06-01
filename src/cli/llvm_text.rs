// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use std::fs;
use std::path::PathBuf;
use std::process;

type BuiltinTable = std::collections::HashMap<
    String,
    (
        String,
        mvl::mvl::parser::ast::TypeExpr,
        Vec<mvl::mvl::parser::ast::TypeExpr>,
    ),
>;

/// Build the prelude and builtin dispatch table for the llvm_text backend.
///
/// Mirrors `prepare_llvm` in `src/cli/llvm.rs` but uses `collect_llvm_text_builtins`
/// instead of the inkwell StdlibSig dispatch table.
fn prepare_llvm_text(prog: &Program) -> (Vec<Program>, BuiltinTable) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        prog,
    )));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));
    let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(prog));
    (prelude, builtins)
}

/// Compile an MVL file to LLVM IR text and write the `.ll` file.
/// `mvl build --backend=llvm <file>`
pub(super) fn build_project_llvm_text(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, builtins) = prepare_llvm_text(&prog);
    let mut compiler = LlvmTextCompiler::new();
    compiler.builtin_symbols = builtins;
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

        let (prelude, builtins) = prepare_llvm_text(&prog);
        let mut compiler = LlvmTextCompiler::new();
        compiler.builtin_symbols = builtins;
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
        if let Some(lib) = lli::find_mvl_runtime_c_lib() {
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
