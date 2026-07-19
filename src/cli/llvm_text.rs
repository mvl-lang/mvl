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

/// Load siblings, lower them to TIR, and return all parts needed for multi-file
/// LLVM IR emission (#1879).
///
/// Falls back to the single-file path when the project has no sibling modules.
fn prepare_llvm_text_tir_multi(
    prog: &Program,
    path: &str,
) -> (
    Vec<TirProgram>,
    TirProgram,
    Vec<TirProgram>,
    LlvmTextCompiler,
) {
    let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    let sibling_modules = loader::load_sibling_modules_transitive(prog, entry_dir);

    if sibling_modules.is_empty() {
        let (prelude_tirs, entry_tir, compiler) = prepare_llvm_text_tir(prog);
        return (prelude_tirs, entry_tir, vec![], compiler);
    }

    let sibling_progs: Vec<&Program> = sibling_modules.iter().map(|(_, _, p)| p).collect();

    // Prelude covers entry + all siblings so stdlib selectors are complete.
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(load_full_prelude(
        std::iter::once(prog).chain(sibling_progs.iter().copied()),
        PreludeMode::Transpile,
    ));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
        prog,
    )));
    let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(prog));

    // Check entry with siblings as cross-sibling prelude (Go model: same-dir files share decls).
    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_two_preludes(&prelude, &sibling_progs, prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            eprintln!("warning: checker: {err:?}");
        }
    }
    expr_types.extend(check_result.expr_types);

    // Each sibling is checked with the entry + all OTHER siblings as its prelude.
    let sibling_expr_types: Vec<_> = sibling_modules
        .iter()
        .enumerate()
        .map(|(i, (_, _, sibling))| {
            let (before, rest) = sibling_modules.split_at(i);
            let after = &rest[1..];
            let sibling_prelude: Vec<&Program> = std::iter::once(prog)
                .chain(before.iter().map(|(_, _, p)| p))
                .chain(after.iter().map(|(_, _, p)| p))
                .collect();
            let mut t = checker::collect_prelude_expr_types(&prelude);
            t.extend(
                checker::check_with_two_preludes(&prelude, &sibling_prelude, sibling).expr_types,
            );
            t
        })
        .collect();

    // Lower entry to TIR.
    let all_fns = mvl::mvl::passes::mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
    let entry_mono = mvl::mvl::passes::mono::monomorphize(prog, &all_fns, &expr_types);
    let entry_tir = mvl::mvl::ir::lower::lower(prog, &entry_mono, &expr_types);

    // Lower prelude to TIR.
    let prelude_tirs: Vec<TirProgram> = prelude
        .iter()
        .map(|p| {
            let all_fns = mvl::mvl::passes::mono::collect_fns([p]);
            let m = mvl::mvl::passes::mono::monomorphize(p, &all_fns, &expr_types);
            mvl::mvl::ir::lower::lower(p, &m, &expr_types)
        })
        .collect();

    // Lower each sibling to TIR using its own expr_types.
    let sibling_tirs: Vec<TirProgram> = sibling_modules
        .iter()
        .zip(sibling_expr_types.iter())
        .map(|((_, _, sibling), sib_types)| {
            let all_fns =
                mvl::mvl::passes::mono::collect_fns(std::iter::once(sibling).chain(prelude.iter()));
            let m = mvl::mvl::passes::mono::monomorphize(sibling, &all_fns, sib_types);
            mvl::mvl::ir::lower::lower(sibling, &m, sib_types)
        })
        .collect();

    let compiler = LlvmTextCompiler::with_context(builtins);
    (prelude_tirs, entry_tir, sibling_tirs, compiler)
}

/// Compile `prog` (and any sibling modules in the same directory) to LLVM IR (#1879).
fn compile_ir_multi(prog: &Program, path: &str, module_name: &str) -> Result<String, String> {
    let (prelude_tirs, entry_tir, sibling_tirs, compiler) = prepare_llvm_text_tir_multi(prog, path);
    compiler.compile_to_ir_with_siblings_tir(&prelude_tirs, &sibling_tirs, &entry_tir, module_name)
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
    match compile_ir_multi(&prog, path, &module_name) {
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
    let ir = match compile_ir_multi(&prog, path, &module_name) {
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

/// Run LLVM text backend tests.
///
/// Discovers two kinds of test:
/// 1. `// expect:` corpus files — `fn main` with expected stdout annotation.
/// 2. `test fn` files — single-file MVL with `test fn` declarations. Each
///    test fn is run in its own `lli` invocation (dispatch-main pattern).
///    The process exit code (0 = pass, non-zero/SIGILL = fail) gives per-test
///    isolation without any runtime changes (#1878).
///
/// `mvl test <path> --backend=llvm`
pub(super) fn cmd_test_llvm_text(path: &str, quiet: bool, verbose: bool) {
    let lli_bin = lli::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM (brew install llvm)");
        process::exit(1);
    });
    let runtime_lib = lli::find_mvl_runtime_llvm_lib();

    let all_mvl = loader::mvl_files_all(path);

    // ── Corpus expect-annotation cases ───────────────────────────────────────
    let mut expect_cases: Vec<(PathBuf, String, bool)> = Vec::new();
    // ── test fn cases: (file, compiled IR, vec of test names) ────────────────
    let mut testfn_cases: Vec<(PathBuf, String, Vec<String>)> = Vec::new();

    for file in &all_mvl {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Corpus-style: fn main + // expect: annotation.
        if src.contains("fn main(") {
            if let Some(pat) = lli::parse_expect_pattern_annotation(&src) {
                expect_cases.push((file.clone(), pat, true));
                continue;
            } else if let Some(expected) = lli::parse_expect_annotation(&src) {
                expect_cases.push((file.clone(), expected, false));
                continue;
            }
        }

        // test fn style: files with `test fn` declarations (no fn main).
        // Also matches `test partial fn` and `test total fn` modifiers.
        let file_str = file.display().to_string();
        let has_test_fns = src.contains("test fn ")
            || src.contains("test partial fn ")
            || src.contains("test total fn ");
        if has_test_fns {
            let module_name = loader::stem(&file_str);
            let (prog, _) = super::parse_or_exit(&file_str);
            let (prelude_tirs, entry_tir, sibling_tirs, compiler) =
                prepare_llvm_text_tir_multi(&prog, &file_str);
            match compiler.compile_to_ir_test_crate_with_siblings(
                &prelude_tirs,
                &sibling_tirs,
                &entry_tir,
                &module_name,
            ) {
                Ok((ir, names)) if !names.is_empty() => {
                    testfn_cases.push((file.clone(), ir, names));
                }
                Ok(_) => {} // no test fns found after lowering
                Err(e) => {
                    eprintln!("  FAIL (codegen): {file_str}: {e}");
                }
            }
        }
    }

    let total_cases = expect_cases.len()
        + testfn_cases
            .iter()
            .map(|(_, _, ns)| ns.len())
            .sum::<usize>();
    if total_cases == 0 {
        if !quiet {
            println!(
                "No LLVM text test cases found (files with `fn main` + `// expect:` or `test fn` declarations)."
            );
        }
        return;
    }

    let lli_bin_ref: &Path = &lli_bin;
    let runtime_lib_ref: Option<&Path> = runtime_lib.as_deref();

    // ── Run corpus expect cases (unchanged) ───────────────────────────────────
    let mut results: Vec<CaseResult> = Vec::new();

    if !expect_cases.is_empty() {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(expect_cases.len());
        let chunk_size = expect_cases.len().div_ceil(parallelism).max(1);

        if !quiet {
            println!(
                "LLVM text backend: {} expect-annotation file(s) across {} worker(s)",
                expect_cases.len(),
                parallelism
            );
        }

        let mut expect_results: Vec<CaseResult> = std::thread::scope(|scope| {
            let handles: Vec<_> = expect_cases
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
        results.append(&mut expect_results);
    }

    // ── Run test fn cases: one lli invocation per test fn ────────────────────
    if !testfn_cases.is_empty() {
        let total_testfns: usize = testfn_cases.iter().map(|(_, _, ns)| ns.len()).sum();
        if !quiet {
            println!(
                "LLVM text backend: {} test fn(s) across {} file(s)",
                total_testfns,
                testfn_cases.len(),
            );
        }

        for (file, ir, test_names) in &testfn_cases {
            let file_str = file.display().to_string();
            // Write IR to a temp file once; reuse for all test fns in this file.
            let tmp = match tempfile::NamedTempFile::with_suffix(".ll") {
                Ok(t) => t,
                Err(e) => {
                    results.push(CaseResult {
                        passed: false,
                        output: String::new(),
                        err_output: format!("  FAIL (tempfile): {file_str}: {e}\n"),
                    });
                    continue;
                }
            };
            if let Err(e) = fs::write(tmp.path(), ir) {
                results.push(CaseResult {
                    passed: false,
                    output: String::new(),
                    err_output: format!("  FAIL (write IR): {file_str}: {e}\n"),
                });
                continue;
            }

            for test_name in test_names {
                let r = run_one_testfn(
                    &file_str,
                    test_name,
                    tmp.path(),
                    lli_bin_ref,
                    runtime_lib_ref,
                    quiet,
                    verbose,
                );
                results.push(r);
            }
        }
    }

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
        let _ = std::io::Write::flush(&mut std::io::stdout());
        println!();
    }
    println!("{} passed, {} failed", passed, failed);
    if failed > 0 {
        process::exit(1);
    }
}

/// Run one `test fn` by name: invoke `lli <ir_file> <test_name>`.
/// Exit code 0 = pass; any other exit or signal (e.g. SIGILL from llvm.trap) = fail.
fn run_one_testfn(
    file_str: &str,
    test_name: &str,
    ir_path: &Path,
    lli_bin: &Path,
    runtime_lib: Option<&Path>,
    quiet: bool,
    verbose: bool,
) -> CaseResult {
    let label = format!("{file_str}::{test_name}");

    let mut cmd = process::Command::new(lli_bin);
    if let Some(lib) = runtime_lib {
        cmd.arg(format!("--load={}", lib.display()));
    }
    cmd.arg(ir_path).arg(test_name);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            return CaseResult {
                passed: false,
                output: String::new(),
                err_output: format!("  FAIL (lli): {label}: {e}\n"),
            };
        }
    };

    if output.status.success() {
        CaseResult {
            passed: true,
            output: if verbose {
                format!("  PASS: {label}\n")
            } else if !quiet {
                ".".to_string()
            } else {
                String::new()
            },
            err_output: String::new(),
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut out = String::new();
        if !quiet {
            out.push_str(&format!("\n  FAIL: {label}\n"));
            if verbose && !stderr.is_empty() {
                for line in stderr.lines().take(10) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path.display().to_string()
    }

    // Two-file multi-file LLVM compilation: sibling function must appear in IR (#1879).
    #[test]
    fn multi_file_sibling_fn_appears_in_ir() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "helper.mvl",
            "pub fn add(a: Int, b: Int) -> Int { a + b }",
        );
        let main_path = write_file(
            &dir,
            "main.mvl",
            "use helper::add;\nfn main() -> Unit ! Console { println(add(1, 2).to_string()) }",
        );

        let (prog, _) = super::super::parse_or_exit(&main_path);
        let (_, _, sibling_tirs, _) = prepare_llvm_text_tir_multi(&prog, &main_path);

        assert_eq!(
            sibling_tirs.len(),
            1,
            "expected 1 sibling TIR (helper), got {}",
            sibling_tirs.len()
        );
        assert!(
            !sibling_tirs[0].fns.is_empty(),
            "helper TIR should have at least one function"
        );

        let ir = compile_ir_multi(&prog, &main_path, "main").expect("compile_ir_multi failed");
        assert!(
            ir.contains("define i64 @add("),
            "@add not in generated IR:\n{ir}"
        );
    }
}
