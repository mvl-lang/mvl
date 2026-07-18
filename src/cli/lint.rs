// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::linter::{self, config::LintConfig, errors::LintDiag};
use mvl::mvl::loader;
use mvl::mvl::parser::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

pub fn run(path: &str, show_config: bool) {
    // Resolve project root: directory of the path arg, or cwd for dirs.
    let project_root = {
        let p = Path::new(path);
        if p.is_file() {
            p.parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            p.to_path_buf()
        }
    };

    let cfg = LintConfig::load(&project_root);

    if show_config {
        if let Some(f) = LintConfig::config_file(&project_root) {
            eprintln!("config: {}", f.display());
        } else {
            eprintln!("config: <defaults — no .mvllintrc or XDG config found>");
        }
        eprintln!("  [phase-1: style]");
        eprintln!("  line_length          = {}", cfg.line_length);
        eprintln!("  indent_size          = {}", cfg.indent_size);
        eprintln!(
            "  indent_style         = {}",
            if cfg.indent_spaces { "spaces" } else { "tabs" }
        );
        eprintln!("  max_fn_length        = {}", cfg.max_fn_length);
        eprintln!("  naming               = {}", cfg.naming);
        eprintln!("  trailing_ws          = {}", cfg.trailing_ws);
        eprintln!("  unused_bindings      = {}", cfg.unused_bindings);
        eprintln!("  [phase-2: semantic]");
        eprintln!("  unreachable_code     = {}", cfg.unreachable_code);
        eprintln!("  redundant_match      = {}", cfg.redundant_match);
        eprintln!("  redundant_effects    = {}", cfg.redundant_effects);
        eprintln!("  redundant_ifc_labels = {}", cfg.redundant_ifc_labels);
        eprintln!("  [phase-3: llm corpus quality]");
        eprintln!(
            "  consistent_comment_style = {}",
            cfg.consistent_comment_style
        );
        eprintln!("  require_doc_comments = {}", cfg.require_doc_comments);
        eprintln!("  doc_comment_examples = {}", cfg.doc_comment_examples);
        eprintln!("  [phase-4: complexity]");
        eprintln!(
            "  max_cyclomatic_complexity  = {}",
            cfg.max_cyclomatic_complexity
        );
        eprintln!(
            "  max_nested_match_depth     = {}",
            cfg.max_nested_match_depth
        );
        eprintln!(
            "  max_effect_signature_width = {}",
            cfg.max_effect_signature_width
        );
        eprintln!(
            "  max_trait_impl_count       = {}",
            cfg.max_trait_impl_count
        );
        eprintln!("  max_module_fanout          = {}", cfg.max_module_fanout);
        eprintln!("  max_extern_ratio           = {:.2}", cfg.max_extern_ratio);
        eprintln!(
            "  composition_root_depth     = {}",
            cfg.composition_root_depth
        );
        return;
    }

    let files = loader::mvl_files_all(path);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut had_errors = false;

    for file in &files {
        let file_str = file.display().to_string();
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file_str}:1:1: error: [io-error] cannot read file: {e}");
                total_errors += 1;
                had_errors = true;
                continue;
            }
        };
        let (mut parser, lex_errors) = Parser::new(&src);
        let prog = parser.parse_program();

        // Surface lex and parse errors as lint diagnostics.
        let mut pre_diags: Vec<LintDiag> = Vec::new();
        for err in &lex_errors {
            pre_diags.push(LintDiag::error(
                "lex-error",
                err.message.clone(),
                err.span.line,
                err.span.col,
            ));
        }
        for err in parser.errors() {
            pre_diags.push(LintDiag::error(
                "parse-error",
                err.message.clone(),
                err.span.line,
                err.span.col,
            ));
        }

        let result = linter::lint(&prog, &src, &cfg, file);

        for diag in pre_diags.iter().chain(result.diags.iter()) {
            eprintln!("{}", diag.render(&file_str));
        }

        total_warnings += result.warning_count();
        total_errors += result.error_count() + pre_diags.len();

        if !pre_diags.is_empty() || !result.is_ok() {
            had_errors = true;
        } else if result.diags.is_empty() {
            println!("{file_str}: OK");
        }
    }

    if files.len() > 1 {
        eprintln!(
            "\n{} warning(s), {} error(s) across {} file(s)",
            total_warnings,
            total_errors,
            files.len()
        );
    }

    if had_errors {
        process::exit(1);
    }
}
