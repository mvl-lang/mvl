// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::LlvmTextCompiler;
use mvl::mvl::loader;
use std::fs;
use std::process;

/// Compile an MVL file to LLVM IR text and write the `.ll` file.
/// `mvl build --backend=llvm <file>`
pub(super) fn build_project_llvm_text(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let module_name = loader::stem(path);
    match LlvmTextCompiler::new().compile_to_ir(&prog, &module_name) {
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
