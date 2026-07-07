// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl prove` — per-call-site refinement proof layer breakdown (#836).
//!
//! Runs the 5-layer SMT solver over a file or directory and prints, for each
//! call site that has a refined parameter, which solver layer proved it (or
//! that it fell back to a runtime check / failed statically).
//!
//! The data comes from `RefinementCounts.sites` populated by
//! `check_call_site()` in `src/mvl/checker/refinements.rs`.

use mvl::mvl::checker;
use mvl::mvl::checker::refinements::ProofOutcome;
use mvl::mvl::checker::SolverMode;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use std::path::Path;
use std::process;

const LAYER_NAMES: [&str; 6] = ["", "trivial", "interval", "symbolic", "cooper", "z3"];

/// Parse and run the refinement prover over a .mvl file or directory.
pub fn run(path: &str, verbose: bool, stdlib_profile: &str, callee_filter: Option<&str>) {
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let stdlib_dir = stdlib::ensure_stdlib();

    super::check::maybe_check_proven_stdlib_or_exit(stdlib_profile);

    // Parse all user files.
    let mut parsed: Vec<(String, Program)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, _src) = super::parse_or_exit(&file_str);
            (file_str, prog)
        })
        .collect();

    let check_count = parsed.len();

    // Auto-load imported sibling modules (mirrors check behaviour).
    if Path::new(path).is_file() {
        let already_loaded: std::collections::HashSet<String> =
            parsed.iter().map(|(f, _)| loader::stem(f)).collect();
        let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        if let Some((_, entry_prog)) = parsed.first() {
            for mod_name in loader::collect_imported_module_names(entry_prog) {
                if already_loaded.contains(&mod_name) {
                    continue;
                }
                if let Some(mod_path) = loader::find_module_file(entry_dir, &mod_name) {
                    let mod_str = mod_path.display().to_string();
                    let (sib_prog, _) = super::parse_or_exit(&mod_str);
                    parsed.push((mod_str, sib_prog));
                }
            }
        }
    }

    // Run resolver so cross-module references work the same as `mvl check`.
    let modules: Vec<(String, String, Program)> = parsed
        .iter()
        .map(|(file_str, prog)| (loader::stem(file_str), file_str.clone(), prog.clone()))
        .collect();
    let _ = resolver::resolve_project(modules, Some(&stdlib_dir));

    // Build the same prelude stack as check.rs: implicit prelude + stdlib + pkg modules.
    let mut stdlib_prelude = loader::load_implicit_prelude();
    stdlib_prelude.extend(loader::load_stdlib_prelude(
        parsed.iter().take(check_count).map(|(_, p)| p),
        &stdlib_dir,
    ));
    let all_user_progs: Vec<Program> = parsed.iter().map(|(_, p)| p.clone()).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_root = super::find_project_root(&cwd);
    stdlib_prelude.extend(loader::load_pkg_modules(
        &all_user_progs,
        &project_root,
        &mut std::collections::HashSet::new(),
    ));

    // Accumulate totals across all files.
    let mut total_proven = 0usize;
    let mut total_runtime = 0usize;
    let mut total_failed = 0usize;
    let mut total_by_layer = [0usize; 6];

    for (idx, (file_str, prog)) in parsed.iter().take(check_count).enumerate() {
        let (before, after_with_self) = all_user_progs.split_at(idx);
        let after = &after_with_self[1..];
        let user_prelude: Vec<&Program> = before.iter().chain(after.iter()).collect();

        let result = checker::check_with_two_preludes_mode(
            &stdlib_prelude,
            &user_prelude,
            prog,
            SolverMode::Layered,
        );

        let all_sites = &result.refinement_counts.sites;
        let sites: Vec<_> = if let Some(callee) = callee_filter {
            all_sites.iter().filter(|s| s.fn_name == callee).collect()
        } else {
            all_sites.iter().collect()
        };
        if sites.is_empty() {
            if let Some(callee) = callee_filter {
                if !all_sites.is_empty() {
                    println!("{file_str}: no proof sites for callee `{callee}`");
                }
            }
            continue;
        }

        println!("{file_str}: refinement proof breakdown");
        // Adaptive widths for alignment.
        let counter_width = if sites.len() >= 100 { 3 } else { 2 };
        let line_width = sites
            .iter()
            .map(|s| s.span.line)
            .max()
            .unwrap_or(1)
            .to_string()
            .len();
        let caller_width = sites
            .iter()
            .map(|s| s.caller_fn.chars().count())
            .max()
            .unwrap_or(0);
        // callee_width is always arrow-only (no predicate) so verbose predicates
        // don't inflate the column for every other line.
        let callee_width = sites
            .iter()
            .map(|s| {
                format!(
                    "{:<cw$} \u{2192} {}({})",
                    s.caller_fn,
                    s.fn_name,
                    s.param_name,
                    cw = caller_width
                )
                .chars()
                .count()
            })
            .max()
            .unwrap_or(40);
        let verdict_width = sites
            .iter()
            .map(|s| match &s.outcome {
                ProofOutcome::Proven { layer } => {
                    format!("({layer}:{})", LAYER_NAMES[*layer]).chars().count()
                }
                ProofOutcome::RuntimeCheck => "(runtime)".len(),
                ProofOutcome::Failed => "(FAILED)".len(),
            })
            .max()
            .unwrap_or(10);
        // Predicate indent matches the prefix "  NN:[LLL]  ".
        let pred_indent =
            " ".repeat(2 + counter_width + 1 + (line_width + 2) + 2 + caller_width + 4);
        let term_width = std::env::var("COLUMNS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100);
        for (idx, site) in sites.iter().enumerate() {
            let counter = format!("{:0>width$}", idx + 1, width = counter_width);
            let loc = format!("[{:>width$}]", site.span.line, width = line_width);
            let arrow = format!(
                "{:<cw$} → {}({})",
                site.caller_fn,
                site.fn_name,
                site.param_name,
                cw = caller_width
            );
            let verdict = match &site.outcome {
                ProofOutcome::Proven { layer } => {
                    format!("({layer}:{})", LAYER_NAMES[*layer])
                }
                ProofOutcome::RuntimeCheck => "(runtime)".to_string(),
                ProofOutcome::Failed => "(FAILED)".to_string(),
            };
            if verbose {
                // Try fitting everything on one line; wrap predicate below if too wide.
                let one_line = format!(
                    "  {counter}:{loc}  {arrow} — `{}`  {verdict}",
                    site.predicate
                );
                if one_line.chars().count() <= term_width {
                    println!("{one_line}");
                } else {
                    println!(
                        "  {counter}:{loc}  {arrow:<callee_width$}  {verdict:<verdict_width$}"
                    );
                    println!("{pred_indent}— `{}`", site.predicate);
                }
            } else {
                println!("  {counter}:{loc}  {arrow:<callee_width$}  {verdict:<verdict_width$}");
            }
        }

        // Compute summary from sites (not counts.proven/runtime/failed) so the
        // numbers always match the lines we just printed.  counts.* still tracks
        // the underlying solver attempts including negation-prove steps for
        // decreases/invariant-preservation that don't correspond to user-visible sites.
        let mut file_proven = 0usize;
        let mut file_runtime = 0usize;
        let mut file_failed = 0usize;
        let mut file_by_layer = [0usize; 6];
        for site in sites {
            match &site.outcome {
                ProofOutcome::Proven { layer } => {
                    file_proven += 1;
                    file_by_layer[*layer] += 1;
                }
                ProofOutcome::RuntimeCheck => file_runtime += 1,
                ProofOutcome::Failed => file_failed += 1,
            }
        }
        total_proven += file_proven;
        total_runtime += file_runtime;
        total_failed += file_failed;
        for (dst, src) in total_by_layer[1..=5]
            .iter_mut()
            .zip(file_by_layer[1..=5].iter())
        {
            *dst += src;
        }

        let layer_breakdown: String = (1..=5)
            .map(|i| format!("L{}:{}", i, file_by_layer[i]))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "\n  Summary: {file_proven} proven ({layer_breakdown}), {file_runtime} runtime, {file_failed} failed\n"
        );
    }

    // Multi-file grand total.
    if check_count > 1 {
        let layer_breakdown: String = (1..=5)
            .map(|i| format!("L{}:{}", i, total_by_layer[i]))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "Total: {total_proven} proven ({layer_breakdown}), {total_runtime} runtime, {total_failed} failed"
        );
    }

    if total_failed > 0 {
        process::exit(1);
    }
}
