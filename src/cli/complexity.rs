// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::loader;
use mvl::mvl::passes::complexity;
use std::process;

pub fn run(path: &str, format_json: bool) {
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let mut reports = Vec::new();
    for f in &files {
        let file_str = f.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        reports.push(complexity::analyze(&file_str, &prog));
    }
    if format_json {
        complexity::print_json(&reports);
    } else {
        for report in &reports {
            complexity::print_human(report);
        }
    }
}
