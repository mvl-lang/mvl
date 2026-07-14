// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Corpus TIR compilation smoke-test (#1612 Phase 3b).
//!
//! Walks every `.mvl` file under `tests/corpus_old/` that has a `fn main(` entry
//! point and verifies it compiles without error via the TIR-walking LLVM emitter.
//! This replaced the AST/TIR parity test once the AST walker was deleted in
//! #1612 Phase 3b. Path retargeted per #1823 phase 1.

use std::path::PathBuf;

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::checker;
use mvl::mvl::ir::lower;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::passes::mono;

fn parse(src: &str) -> Program {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

fn corpus_main_files() -> Vec<PathBuf> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(manifest).join("tests/corpus_old");
    let mut out = Vec::new();
    walk(&root, &mut out);
    out.sort();
    out
}

fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "mvl") {
            if let Ok(src) = std::fs::read_to_string(&p) {
                if src.contains("fn main(") {
                    out.push(p);
                }
            }
        }
    }
}

#[test]
fn corpus_ir_parity_ast_vs_tir() {
    let files = corpus_main_files();
    assert!(
        !files.is_empty(),
        "no corpus files with `fn main(` were discovered — wrong working directory?"
    );

    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    for file in &files {
        let src = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                failures.push((file.clone(), format!("read error: {e}")));
                continue;
            }
        };
        let prog = parse(&src);
        let mut prelude = loader::load_implicit_prelude();
        prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
            &prog,
        )));
        prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(
            &prog,
        )));
        let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(&prog));
        let mut expr_types = checker::collect_prelude_expr_types(&prelude);
        let cr = checker::check_with_prelude(&prelude, &prog);
        expr_types.extend(cr.expr_types);
        let compiler = LlvmTextCompiler::with_context(builtins);

        let entry_all_fns = mono::collect_fns(std::iter::once(&prog).chain(prelude.iter()));
        let entry_mono = mono::monomorphize(&prog, &entry_all_fns, &expr_types);
        let entry_tir = lower::lower(&prog, &entry_mono, &expr_types);
        let prelude_tirs = prelude
            .iter()
            .map(|p| {
                let all = mono::collect_fns([p]);
                let m = mono::monomorphize(p, &all, &expr_types);
                lower::lower(p, &m, &expr_types)
            })
            .collect::<Vec<_>>();

        let module_name = loader::stem(&file.to_string_lossy());
        if let Err(e) =
            compiler.compile_to_ir_with_prelude_tir(&prelude_tirs, &entry_tir, &module_name)
        {
            failures.push((file.clone(), e));
        }
    }

    eprintln!(
        "Corpus TIR compilation: scanned {} files, {} failures",
        files.len(),
        failures.len()
    );

    if !failures.is_empty() {
        let mut report = String::new();
        for (file, err) in &failures {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(file);
            report.push_str(&format!("\n--- {} ---\n{err}\n", rel.display()));
        }
        panic!(
            "{} corpus file(s) failed TIR compilation:{report}",
            failures.len()
        );
    }
}
