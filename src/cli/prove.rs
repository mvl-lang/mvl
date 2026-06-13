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
pub fn run(path: &str, verbose: bool, stdlib_profile: &str) {
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
    let modules: Vec<(String, Program)> = parsed
        .iter()
        .map(|(file_str, prog)| (loader::stem(file_str), prog.clone()))
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

        let sites = &result.refinement_counts.sites;
        if sites.is_empty() {
            continue;
        }

        println!("{file_str}: refinement proof breakdown");
        for site in sites {
            let loc = format!("line {:>3}", site.span.line);
            let call = if verbose {
                format!(
                    "{}({}) — `{}`",
                    site.fn_name, site.param_name, site.predicate
                )
            } else {
                format!("{}({})", site.fn_name, site.param_name)
            };
            let verdict = match &site.outcome {
                ProofOutcome::Proven { layer } => {
                    format!("Layer {} ({})", layer, LAYER_NAMES[*layer])
                }
                ProofOutcome::RuntimeCheck => "runtime check".to_string(),
                ProofOutcome::Failed => "FAILED (static violation)".to_string(),
            };
            println!("  {loc}  {call:<40}  {verdict}");
        }

        let rc = &result.refinement_counts;
        total_proven += rc.proven;
        total_runtime += rc.runtime_checked;
        total_failed += rc.failed;
        for (dst, src) in total_by_layer[1..=5]
            .iter_mut()
            .zip(rc.by_layer[1..=5].iter())
        {
            *dst += src;
        }

        let layer_breakdown: String = (1..=5)
            .map(|i| format!("L{}:{}", i, rc.by_layer[i]))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "\n  Summary: {} proven ({}), {} runtime, {} failed\n",
            rc.proven, layer_breakdown, rc.runtime_checked, rc.failed
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
