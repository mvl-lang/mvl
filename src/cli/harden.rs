// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl harden` — contract strengthening via proof feedback (#1913).
//!
//! # Design
//!
//! This command has three planned axes of hardening.  Only **Axis 1** is
//! implemented here; Axes 2 and 3 are tracked in the follow-up issue.
//!
//! ## Axis 1 (this file): Runtime → Static Promotion
//!
//! Identifies call-site refinement obligations that fell back to a runtime
//! assertion (`ProofOutcome::RuntimeCheck`) and reports them with the
//! predicate, source location, and a heuristic explanation of *why* the
//! solver could not discharge the obligation statically.
//!
//! The data comes from `RefinementCounts.sites` populated by
//! `check_call_site()` in `src/mvl/checker/refinements.rs` — the same
//! source used by `mvl prove`.  No new solver infrastructure is needed.
//!
//! ## Axis 2 (future): Contract Tightening
//!
//! Binary-search over Z3 to find the tightest provable bound for each
//! `ensures` clause and suggest strengthening the declared postcondition.
//! Requires new Z3 query infrastructure (counterexample extraction +
//! bound refinement loop).  See follow-up issue.
//!
//! ## Axis 3 (future): Boundary Test Generation
//!
//! Query Z3 for edge-case witnesses (values that hit contract boundaries)
//! and emit `*_boundary_test.mvl` files exercising those paths.
//! Requires Z3 model extraction and MVL test-file synthesis.
//! See follow-up issue.

use mvl::mvl::checker;
use mvl::mvl::checker::refinements::ProofOutcome;
use mvl::mvl::checker::SolverMode;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use std::collections::HashMap;
use std::path::Path;
use std::process;

// ── Heuristic classification ───────────────────────────────────────────────────
//
// `ProofSite` records the final outcome but not per-layer failure reasons.
// We classify the predicate text heuristically to suggest a likely fix.
// This is deliberately conservative — we only flag patterns we're confident
// about; everything else gets a generic "predicate too complex for static
// solver" message.

#[derive(Debug, Clone, PartialEq, Eq)]
enum HardenHint {
    /// Predicate uses `*` or `/` — nonlinear, beyond L2/L4 interval arithmetic.
    NonlinearPredicate,
    /// Predicate references `len(...)` — string/list length, needs axiom.
    LengthPredicate,
    /// Predicate references `old(...)` — postcondition with pre-state, L5 only.
    OldPredicate,
    /// Predicate uses quantifiers (`forall`/`exists`) — beyond current layers.
    QuantifiedPredicate,
    /// No recognisable pattern — generic fallback.
    Complex,
}

impl HardenHint {
    fn classify(predicate: &str) -> Self {
        if predicate.contains("forall") || predicate.contains("exists") {
            return HardenHint::QuantifiedPredicate;
        }
        if predicate.contains("old(") {
            return HardenHint::OldPredicate;
        }
        if predicate.contains("len(") {
            return HardenHint::LengthPredicate;
        }
        // Nonlinear: multiplication or division present (but not inside len/old).
        let stripped = predicate
            .replace("old(", "")
            .replace("len(", "");
        if stripped.contains('*') || stripped.contains('/') {
            return HardenHint::NonlinearPredicate;
        }
        HardenHint::Complex
    }

    fn suggestion(&self) -> &'static str {
        match self {
            HardenHint::NonlinearPredicate => {
                "nonlinear arithmetic — factor into linear steps or introduce a refined intermediate type"
            }
            HardenHint::LengthPredicate => {
                "length predicate — add a `where len(self) > N` refinement on the parameter type"
            }
            HardenHint::OldPredicate => {
                "old() in postcondition — only reachable via L5 (Z3); enable with --refinement-solver=z3-only"
            }
            HardenHint::QuantifiedPredicate => {
                "quantifier — beyond current solver layers; introduce a refined wrapper type instead"
            }
            HardenHint::Complex => {
                "predicate too complex for static layers — add a proof anchor assertion before the call"
            }
        }
    }
}

// ── JSON output ────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct HardenSite<'a> {
    file: &'a str,
    line: u32,
    caller: &'a str,
    callee: &'a str,
    param: &'a str,
    predicate: &'a str,
    hint: HardenHint,
}

fn print_json(sites: &[HardenSite<'_>], total_runtime: usize, total_failed: usize) {
    println!("{{");
    println!("  \"axis\": 1,");
    println!("  \"axis_name\": \"runtime_to_static_promotion\",");
    println!("  \"total_runtime\": {total_runtime},");
    println!("  \"total_failed\": {total_failed},");
    println!("  \"promotion_candidates\": [");
    for (i, s) in sites.iter().enumerate() {
        let comma = if i + 1 < sites.len() { "," } else { "" };
        let hint = s.hint.suggestion().replace('"', "\\\"");
        println!("    {{");
        println!("      \"file\": \"{}\",", s.file.replace('"', "\\\""));
        println!("      \"line\": {},", s.line);
        println!("      \"caller\": \"{}\",", s.caller.replace('"', "\\\""));
        println!("      \"callee\": \"{}\",", s.callee.replace('"', "\\\""));
        println!("      \"param\": \"{}\",", s.param.replace('"', "\\\""));
        println!(
            "      \"predicate\": \"{}\",",
            s.predicate.replace('"', "\\\"")
        );
        println!("      \"suggestion\": \"{hint}\"");
        println!("    }}{comma}");
    }
    println!("  ]");
    println!("}}");
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Run `mvl harden` (Axis 1) over a `.mvl` file or directory.
pub fn run(path: &str, verbose: bool, json: bool, stdlib_profile: &str, callee_filter: Option<&str>) {
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let stdlib_dir = stdlib::ensure_stdlib();

    super::check::maybe_check_proven_stdlib_or_exit(stdlib_profile);

    let mut parsed: Vec<(String, Program)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, _src) = super::parse_or_exit(&file_str);
            (file_str, prog)
        })
        .collect();

    let check_count = parsed.len();

    let base_dir: std::path::PathBuf = if Path::new(path).is_dir() {
        Path::new(path).to_path_buf()
    } else {
        loader::infer_base_dir_from_qualified_imports(Path::new(path))
    };

    if Path::new(path).is_file() {
        let already_loaded: std::collections::HashSet<String> = parsed
            .iter()
            .map(|(f, _)| loader::qualified_stem(&base_dir, Path::new(f)))
            .collect();
        let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        if let Some((_, entry_prog)) = parsed.first() {
            let siblings = loader::load_sibling_modules_transitive(entry_prog, entry_dir);
            for (mod_name, mod_str, sib_prog) in siblings {
                if already_loaded.contains(&mod_name) {
                    continue;
                }
                parsed.push((mod_str, sib_prog));
            }
        }
    }

    let modules: Vec<(String, String, Program)> = parsed
        .iter()
        .map(|(file_str, prog)| {
            let qname = loader::qualified_stem(&base_dir, Path::new(file_str));
            (qname, file_str.clone(), prog.clone())
        })
        .collect();
    let _ = resolver::resolve_project(modules, Some(&stdlib_dir));

    let mut stdlib_prelude = loader::load_implicit_prelude();
    stdlib_prelude.extend(load_full_prelude(
        parsed.iter().take(check_count).map(|(_, p)| p),
        PreludeMode::TypeCheck {
            stdlib_dir: &stdlib_dir,
        },
    ));
    let all_user_progs: Vec<Program> = parsed.iter().map(|(_, p)| p.clone()).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_root = super::find_project_root(&cwd);
    stdlib_prelude.extend(loader::load_pkg_modules(
        &all_user_progs,
        &project_root,
        &mut std::collections::HashSet::new(),
    ));

    let mut grand_total_runtime = 0usize;
    let mut grand_total_failed = 0usize;

    // Collect results per file so we can print the human report inline.
    struct FileResult {
        file_str: String,
        sites_data: Vec<(u32, String, String, String, String, HardenHint)>,
        runtime: usize,
        failed: usize,
    }
    let mut file_results: Vec<FileResult> = Vec::new();

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
        let mut file_runtime = 0usize;
        let mut file_failed = 0usize;

        // Collect runtime sites (and count failed) for this file.
        let mut sites_data: Vec<(u32, String, String, String, String, HardenHint)> = Vec::new();
        for site in all_sites {
            let matches_filter = callee_filter
                .map(|f| site.fn_name == f)
                .unwrap_or(true);
            match &site.outcome {
                ProofOutcome::RuntimeCheck if matches_filter => {
                    file_runtime += 1;
                    let hint = HardenHint::classify(&site.predicate);
                    sites_data.push((
                        site.span.line,
                        site.caller_fn.clone(),
                        site.fn_name.clone(),
                        site.param_name.clone(),
                        site.predicate.clone(),
                        hint,
                    ));
                }
                ProofOutcome::Failed if matches_filter => {
                    file_failed += 1;
                }
                _ => {}
            }
        }

        grand_total_runtime += file_runtime;
        grand_total_failed += file_failed;
        file_results.push(FileResult {
            file_str: file_str.clone(),
            sites_data,
            runtime: file_runtime,
            failed: file_failed,
        });
    }

    if json {
        // Flatten all sites into the JSON output.
        let mut flat: Vec<HardenSite<'_>> = Vec::new();
        for fr in &file_results {
            for (line, caller, callee, param, pred, hint) in &fr.sites_data {
                flat.push(HardenSite {
                    file: &fr.file_str,
                    line: *line,
                    caller,
                    callee,
                    param,
                    predicate: pred,
                    hint: hint.clone(),
                });
            }
        }
        print_json(&flat, grand_total_runtime, grand_total_failed);
        if grand_total_failed > 0 {
            process::exit(1);
        }
        return;
    }

    // ── Human-readable report ──────────────────────────────────────────────────

    let sep = "═".repeat(70);
    let dash = "─".repeat(70);

    for fr in &file_results {
        if fr.sites_data.is_empty() && fr.failed == 0 {
            continue;
        }

        println!("\n{sep}");
        println!("  HARDEN REPORT (Axis 1 — runtime → static): {}", fr.file_str);
        println!("{sep}");

        if fr.sites_data.is_empty() {
            println!("  No runtime obligations — all call sites proven statically.");
        } else {
            // Group by caller function.
            let mut by_caller: HashMap<&str, Vec<_>> = HashMap::new();
            for (line, caller, callee, param, pred, hint) in &fr.sites_data {
                by_caller
                    .entry(caller.as_str())
                    .or_default()
                    .push((line, callee, param, pred, hint));
            }
            let mut callers: Vec<&str> = by_caller.keys().copied().collect();
            callers.sort();

            println!("\n── Runtime Obligations (promotion candidates) {dash}");
            let mut counter = 0usize;
            for caller in &callers {
                let entries = &by_caller[caller];
                for (line, callee, param, pred, hint) in entries {
                    counter += 1;
                    println!(
                        "\n  [{counter:02}] {caller}:{line}  →  {callee}({param})"
                    );
                    if verbose {
                        println!("       predicate: {pred}");
                    }
                    println!("       hint: {}", hint.suggestion());
                }
            }
        }

        let rt = fr.runtime;
        let fa = fr.failed;
        println!("\n  Summary (Axis 1): {rt} runtime obligations, {fa} failed\n");
        println!("{sep}\n");
        println!(
            "  Axes 2 (contract tightening) and 3 (boundary test generation) are not yet\n  \
             implemented. See follow-up issue for Z3 query infrastructure required.\n"
        );
    }

    // Multi-file grand total.
    if check_count > 1 {
        println!(
            "Total: {grand_total_runtime} runtime obligations, {grand_total_failed} failed"
        );
    }

    if grand_total_failed > 0 {
        process::exit(1);
    }
}
