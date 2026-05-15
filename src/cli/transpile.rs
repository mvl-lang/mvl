// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::loader;

pub fn run(path: &str) {
    let (prog, _src) = loader::parse_or_exit(path);
    let crate_name = loader::stem(path);
    let out = transpiler::transpile(&prog, transpiler::TranspileConfig::new(&crate_name)).output;
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
