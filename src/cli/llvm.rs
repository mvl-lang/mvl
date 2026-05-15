// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm as codegen;
use mvl::mvl::backends::AssertMode;
use mvl::mvl::loader;
use mvl::mvl::parser::Parser;
use std::fs;
use std::path::PathBuf;
use std::process;

pub(super) fn prepare_llvm(
    prog: &mvl::mvl::parser::ast::Program,
) -> (Vec<mvl::mvl::parser::ast::Program>, codegen::LlvmCompiler) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        prog,
    )));
    (prelude, codegen::LlvmCompiler::new())
}

/// Compile an MVL file to LLVM IR and write the .ll file to the current directory.
/// `mvl build --backend=llvm <file>`
pub(super) fn build_project_llvm(path: &str, assert_mode: AssertMode) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, mut compiler) = prepare_llvm(&prog);
    compiler.assert_mode = assert_mode;
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
            eprintln!("error: LLVM codegen failed: {e}");
            process::exit(1);
        }
    }
}

/// Compile an MVL file to LLVM IR and execute it via `lli`.
/// `mvl run --backend=llvm <file>`
pub(super) fn run_project_llvm(path: &str, assert_mode: AssertMode) {
    let lli = codegen::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM 22 (brew install llvm)");
        process::exit(1);
    });

    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, mut compiler) = prepare_llvm(&prog);
    compiler.assert_mode = assert_mode;
    let ir = match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("error: LLVM codegen failed: {e}");
            process::exit(1);
        }
    };

    let tmp = tempfile::NamedTempFile::with_suffix(".ll").unwrap_or_else(|e| {
        eprintln!("error: cannot create temp file: {e}");
        process::exit(1);
    });
    fs::write(tmp.path(), &ir).unwrap_or_else(|e| {
        eprintln!("error: cannot write IR: {e}");
        process::exit(1);
    });
    let mut cmd = process::Command::new(&lli);
    if let Some(lib) = codegen::find_mvl_runtime_c_lib() {
        cmd.arg(format!("--load={}", lib.display()));
    }
    let status = cmd.arg(tmp.path()).status().unwrap_or_else(|e| {
        eprintln!("error: failed to run lli: {e}");
        process::exit(1);
    });
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
}

/// LLVM integration test harness (L5-03).
/// `mvl test --backend=llvm <path>`
pub(super) fn cmd_test_llvm(path: &str, quiet: bool, verbose: bool) {
    let lli = codegen::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM 22 (brew install llvm)");
        process::exit(1);
    });

    let all_mvl = loader::mvl_files_all(path);
    let mut test_cases: Vec<(PathBuf, String, bool)> = Vec::new();
    let mut harness_cases: Vec<PathBuf> = Vec::new();
    for file in &all_mvl {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let has_main = src.contains("fn main(");
        let is_test_file = file
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("_test.mvl"))
            .unwrap_or(false);

        if has_main {
            if let Some(pat) = codegen::parse_expect_pattern_annotation(&src) {
                test_cases.push((file.clone(), pat, true));
            } else if let Some(expected) = codegen::parse_expect_annotation(&src) {
                test_cases.push((file.clone(), expected, false));
            }
        } else if is_test_file && src.contains("test fn ") {
            harness_cases.push(file.clone());
        }
    }

    if test_cases.is_empty() && harness_cases.is_empty() {
        if !quiet {
            println!("No LLVM test cases found (files with `fn main` + `// expect:` annotations, or `*_test.mvl` with `test fn`).");
        }
        return;
    }

    if !quiet {
        let total = test_cases.len() + harness_cases.len();
        println!("LLVM backend: {} test file(s)", total);
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (file, expected, is_pattern) in &test_cases {
        let file_str = file.display().to_string();
        let module_name = loader::stem(&file_str);
        let (prog, _src) = super::parse_or_exit(&file_str);
        let ok = run_llvm_prog(
            &lli,
            &prog,
            &module_name,
            &file_str,
            expected,
            *is_pattern,
            quiet,
            verbose,
        );
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    for file in &harness_cases {
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
        let test_fns = collect_test_fn_names(&src);
        if test_fns.is_empty() {
            continue;
        }
        let harness_src = synthesize_test_harness(&src, &test_fns);
        let (mut parser, lex_errors) = Parser::new(&harness_src);
        if !lex_errors.is_empty() {
            eprintln!("  FAIL (lex): {file_str}");
            failed += 1;
            continue;
        }
        let prog = parser.parse_program();
        if !parser.errors().is_empty() {
            eprintln!("  FAIL (parse): {file_str}");
            for err in parser.errors() {
                eprintln!("    {err:?}");
            }
            failed += 1;
            continue;
        }
        let ok = run_llvm_prog(
            &lli,
            &prog,
            &module_name,
            &file_str,
            "ok",
            false,
            quiet,
            verbose,
        );
        if ok {
            passed += 1;
        } else {
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

/// Compile `prog` to LLVM IR, run via `lli`, and compare stdout to `expected`.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_llvm_prog(
    lli: &std::path::Path,
    prog: &mvl::mvl::parser::ast::Program,
    module_name: &str,
    file_str: &str,
    expected: &str,
    is_pattern: bool,
    quiet: bool,
    verbose: bool,
) -> bool {
    let (prelude, compiler) = prepare_llvm(prog);
    let ir = match compiler.compile_to_ir_with_prelude(&prelude, prog, module_name) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("  FAIL (codegen): {file_str}");
            eprintln!("    {e}");
            return false;
        }
    };

    let tmp = match tempfile::NamedTempFile::with_suffix(".ll") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  FAIL (tempfile): {file_str}: {e}");
            return false;
        }
    };
    if let Err(e) = fs::write(tmp.path(), &ir) {
        eprintln!("  FAIL (write IR): {file_str}: {e}");
        return false;
    }

    let mut lli_cmd = process::Command::new(lli);
    if let Some(lib) = codegen::find_mvl_runtime_c_lib() {
        lli_cmd.arg(format!("--load={}", lib.display()));
    }
    let output = lli_cmd.arg(tmp.path()).output().unwrap_or_else(|e| {
        eprintln!("error: failed to run lli: {e}");
        process::exit(1);
    });

    let actual = String::from_utf8_lossy(&output.stdout);
    let actual_trimmed = actual.trim_end_matches('\n');
    let expected_trimmed = expected.trim_end_matches('\n');

    let matched = if is_pattern {
        codegen::glob_match(expected_trimmed, actual_trimmed)
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
    } else if !quiet {
        println!("\n  FAIL: {file_str}");
        if is_pattern {
            println!("    pattern:  {:?}", expected_trimmed);
        } else {
            println!("    expected: {:?}", expected_trimmed);
        }
        println!("    got:      {:?}", actual_trimmed);
        if verbose && !ir.is_empty() {
            println!("    --- IR ---");
            for line in ir.lines().take(40) {
                println!("    {line}");
            }
        }
    }
    matched
}

/// Extract names of all `test fn` declarations from MVL source text.
pub(super) fn collect_test_fn_names(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("test fn ")
                .and_then(|rest| rest.split('(').next().map(|name| name.trim().to_string()))
        })
        .collect()
}

/// Build a runnable MVL source by stripping `test ` from `test fn` declarations
/// and appending a `fn main()` harness that calls each test function.
pub(super) fn synthesize_test_harness(src: &str, test_fns: &[String]) -> String {
    let body = src.replace("test fn ", "fn ");
    let calls: String = test_fns
        .iter()
        .map(|name| format!("    {name}();\n"))
        .collect();
    format!("{body}\nfn main() -> Unit ! Console {{\n{calls}    println(\"ok\")\n}}\n")
}
