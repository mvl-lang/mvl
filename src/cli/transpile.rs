// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::checker;
use mvl::mvl::loader;

pub fn run(path: &str) {
    let (prog, _src) = super::parse_or_exit(path);
    let crate_name = loader::stem(path);
    let expr_types = checker::check(&prog).expr_types;
    let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
    let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
    let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
    let out = transpiler::transpile(&tir, transpiler::TranspileConfig::new(&crate_name)).output;
    println!("// === Cargo.toml ===");
    println!("{}", out.cargo_toml);
    let file_label = if out.has_main {
        "src/main.rs"
    } else {
        "src/lib.rs"
    };
    println!("// === {file_label} ===");
    println!("{}", out.lib_rs);
}
