// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl build --backend=wasm` — drive the WASM text emitter (#1571).
//!
//! Spike scope: emit WAT for the `add.mvl` case. Reuses the same
//! prelude/checker/TIR pipeline as the llvm_text backend.

use mvl::mvl::backends::wasm_text::WasmTextCompiler;
use mvl::mvl::backends::Backend;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use std::fs;
use std::process;

/// Lower `prog` (with prelude) to TIR and emit a WAT string.
fn compile_wat(prog: &Program, module_name: &str) -> String {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        prog,
    )));
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
