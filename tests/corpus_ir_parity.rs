// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Bulk corpus IR-parity oracle for #1612 task 2c.
//!
//! Walks every `.mvl` file under `tests/corpus/` that has a `fn main(` entry
//! point, lowers it through both the AST walker and the TIR walker (via
//! `LlvmTextCompiler::compile_to_ir_with_prelude` and
//! `compile_to_ir_with_prelude_tir`), and asserts byte-identical IR.
//!
//! A single divergence fails the test. The harness mirrors the production
//! prelude/checker/mono wiring from `src/cli/llvm_text.rs::prepare_llvm_text`
//! and `prepare_llvm_text_tir` so the IR matches what `mvl run --backend=llvm`
//! emits at the CLI.
//!
//! If individual corpus files are intentionally divergent (e.g. they exercise
//! a TIR-walker gap that is documented and accepted), add them to
//! [`ALLOWLIST`] with a comment naming the gap.

use std::collections::HashSet;
use std::path::PathBuf;

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::checker;
use mvl::mvl::ir::{lower, TirProgram};
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::passes::mono;

/// Files whose AST/TIR walker outputs are known to diverge for a documented
/// reason. Populated only when an investigated divergence is judged not worth
/// fixing before PR 2 deletes the AST walker.
///
/// All current entries share one root cause: AST's static `llvm_ty` helper
/// (used by `result_ok_llvm_ty` and the `Err(...)` arm of `emit_result_match`)
/// doesn't consult `module.enum_variants`, so it returns `"ptr"` for an enum
/// `TypeExpr::Base { name: "Foo", ... }` — even when `Foo` is a payload enum
/// with LLVM repr `{ i8, ptr }`. TIR's `llvm_ty_ctx` consults the registry and
/// emits the correct `load { i8, ptr }`. The loaded value is dead code in the
/// arms surfaced here (the arm body never uses the SSA, or uses one from
/// another arm — pre-existing structural bug). No runtime effect; the
/// divergence disappears when PR 2 deletes the AST emitter.
const ALLOWLIST: &[&str] = &[
    "tests/corpus/03_types/nested_enum_pattern_annotation.mvl",
    "tests/corpus/13_stdlib/process_echo.mvl",
    "tests/corpus/13_stdlib/io_basic.mvl",
    // json_log_imports also has a drop-ordering divergence (two _mvl_string_drop
    // calls placed in different basic blocks) on top of the same enum-load
    // shape mismatch. Both surface together; both resolve with the AST delete.
    "tests/corpus/13_stdlib/json_log_imports.mvl",
];

fn parse(src: &str) -> Program {
    let (mut p, errs) = Parser::new(src);
    assert!(errs.is_empty(), "lex errors: {errs:?}");
    let prog = p.parse_program();
    assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
    prog
}

fn prepare(
    prog: &Program,
) -> (Vec<Program>, Vec<TirProgram>, TirProgram, LlvmTextCompiler) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(prog)));
    prelude.extend(loader::load_rust_backed_stdlib_fns(std::slice::from_ref(prog)));
    let builtins = loader::collect_llvm_text_builtins(std::slice::from_ref(prog));

    let mut expr_types = checker::collect_prelude_expr_types(&prelude);
    let check_result = checker::check_with_prelude(&prelude, prog);
    expr_types.extend(check_result.expr_types);

    let compiler = LlvmTextCompiler::with_context(builtins, expr_types);

    let entry_all_fns =
        mono::collect_fns(std::iter::once(prog).chain(prelude.iter()));
    let entry_mono =
        mono::monomorphize(prog, &entry_all_fns, &compiler.expr_types);
    let entry_tir = lower::lower(prog, &entry_mono, &compiler.expr_types);

    let prelude_tirs: Vec<TirProgram> = prelude
        .iter()
        .map(|p| {
            let all = mono::collect_fns([p]);
            let m = mono::monomorphize(p, &all, &compiler.expr_types);
            lower::lower(p, &m, &compiler.expr_types)
        })
        .collect();

    (prelude, prelude_tirs, entry_tir, compiler)
}

/// Walk `tests/corpus/` and return every `.mvl` file whose source contains a
/// `fn main(` entry point — these are the files that actually produce IR for
/// `main`. Test-fn-only fixtures are excluded (they compile to fns but not to
/// a runnable module, and the AST/TIR walker outputs converge trivially).
fn corpus_main_files() -> Vec<PathBuf> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = std::path::Path::new(manifest).join("tests/corpus");
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

fn first_diff_excerpt(a: &str, b: &str) -> String {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    let max = a_lines.len().max(b_lines.len());
    for i in 0..max {
        let ai = a_lines.get(i).copied().unwrap_or("<missing>");
        let bi = b_lines.get(i).copied().unwrap_or("<missing>");
        if ai != bi {
            let start = i.saturating_sub(2);
            let end = (i + 4).min(max);
            let mut out = String::new();
            for j in start..end {
                let marker = if j == i { ">> " } else { "   " };
                out.push_str(&format!(
                    "{marker}line {j}\n  AST: {}\n  TIR: {}\n",
                    a_lines.get(j).copied().unwrap_or(""),
                    b_lines.get(j).copied().unwrap_or(""),
                ));
            }
            return out;
        }
    }
    format!(
        "lines all match but lengths differ (AST={} lines, TIR={} lines)",
        a_lines.len(),
        b_lines.len()
    )
}

#[derive(Debug)]
struct ParityFailure {
    file: PathBuf,
    kind: String,
    detail: String,
}

fn check_file(file: &std::path::Path) -> Option<ParityFailure> {
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            return Some(ParityFailure {
                file: file.to_path_buf(),
                kind: "read".into(),
                detail: e.to_string(),
            })
        }
    };
    let prog = parse(&src);
    let (prelude, prelude_tirs, entry_tir, compiler) = prepare(&prog);
    let module_name = loader::stem(&file.to_string_lossy());

    let ast_ir = match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
        Ok(ir) => ir,
        Err(e) => {
            return Some(ParityFailure {
                file: file.to_path_buf(),
                kind: "ast-compile".into(),
                detail: e,
            })
        }
    };
    let tir_ir = match compiler.compile_to_ir_with_prelude_tir(
        &prelude_tirs,
        &entry_tir,
        &module_name,
    ) {
        Ok(ir) => ir,
        Err(e) => {
            return Some(ParityFailure {
                file: file.to_path_buf(),
                kind: "tir-compile".into(),
                detail: e,
            })
        }
    };

    if ast_ir == tir_ir {
        return None;
    }
    if std::env::var("MVL_PARITY_DUMP").is_ok() {
        let stem = file.file_stem().unwrap_or_default().to_string_lossy();
        let _ = std::fs::write(format!("/tmp/parity_{stem}_ast.ll"), &ast_ir);
        let _ = std::fs::write(format!("/tmp/parity_{stem}_tir.ll"), &tir_ir);
    }
    Some(ParityFailure {
        file: file.to_path_buf(),
        kind: format!("diff(AST={}B, TIR={}B)", ast_ir.len(), tir_ir.len()),
        detail: first_diff_excerpt(&ast_ir, &tir_ir),
    })
}

// Task 2c landed the harness; task 2d drains the inventory of TIR-walker gaps
// it surfaces (35 / 74 files on first run — unimplemented method arms, missing
// builtins, and a handful of byte-diff divergences). Once that backlog clears,
// drop the `#[ignore]` so the harness becomes the strict regression gate.
#[test]
#[ignore = "#1612 task 2d: TIR walker has 35 corpus gaps to drain before strict parity"]
fn corpus_ir_parity_ast_vs_tir() {
    let files = corpus_main_files();
    assert!(
        !files.is_empty(),
        "no corpus files with `fn main(` were discovered — wrong working directory?"
    );

    let allow: HashSet<&str> = ALLOWLIST.iter().copied().collect();
    let mut failures: Vec<ParityFailure> = Vec::new();
    let mut skipped: Vec<PathBuf> = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(file);
        let rel_str = rel.to_string_lossy().into_owned();
        if allow.contains(rel_str.as_str()) {
            skipped.push(file.clone());
            continue;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_file(file)));
        match result {
            Ok(None) => {}
            Ok(Some(f)) => failures.push(f),
            Err(payload) => {
                let msg = if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = payload.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "<unknown panic payload>".into()
                };
                failures.push(ParityFailure {
                    file: file.clone(),
                    kind: "panic".into(),
                    detail: msg,
                });
            }
        }
    }

    eprintln!(
        "Corpus IR parity: scanned {} files (skipped {} via ALLOWLIST), {} failures",
        files.len() - skipped.len(),
        skipped.len(),
        failures.len()
    );

    if !failures.is_empty() {
        let mut report = String::new();
        for f in &failures {
            let rel = f
                .file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&f.file);
            report.push_str(&format!("\n--- {} [{}] ---\n", rel.display(), f.kind));
            report.push_str(&f.detail);
            report.push('\n');
        }
        panic!(
            "{} corpus file(s) diverge between AST and TIR walkers:{report}",
            failures.len()
        );
    }
}
