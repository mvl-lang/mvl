// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl harden` — contract strengthening via proof feedback (#1913).
//!
//! # Design
//!
//! This command implements three axes of contract hardening.
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
//! ## Axis 2: Contract Tightening
//!
//! Binary-searches over Z3 to find the tightest provable bound for each
//! `ensures` clause and suggests strengthening the declared postcondition.
//! See `src/mvl/checker/solver/layer5.rs::try_z3_tighten`.
//!
//! ## Axis 3: Boundary Test Generation
//!
//! Queries Z3 for concrete witness inputs that reach each return branch and
//! satisfy the tighter postcondition.  With `--emit-tests`, synthesizes and
//! writes `*_boundary_test.mvl` files containing `test fn` blocks.
//! See `try_z3_witness` in `layer5.rs`.

use mvl::mvl::checker;
use mvl::mvl::checker::refinements::{
    synthesize_witness, ProofOutcome, TighteningCandidate, WitnessArg, WitnessValue,
};
use mvl::mvl::checker::SolverMode;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Decl, Program, TypeBody, TypeExpr};
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

fn print_json(
    sites: &[HardenSite<'_>],
    tightenings: &[&TighteningCandidate],
    total_proven: usize,
    total_runtime: usize,
    total_failed: usize,
) {
    println!("{{");
    println!("  \"total_proven\": {total_proven},");
    println!("  \"total_runtime\": {total_runtime},");
    println!("  \"total_failed\": {total_failed},");
    println!("  \"axis1_promotion_candidates\": [");
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
    println!("  ],");
    println!("  \"axis2_tightening_candidates\": [");
    for (i, t) in tightenings.iter().enumerate() {
        let comma = if i + 1 < tightenings.len() { "," } else { "" };
        println!("    {{");
        println!("      \"fn_name\": \"{}\",", t.fn_name.replace('"', "\\\""));
        println!("      \"line\": {},", t.span.line);
        println!(
            "      \"declared\": \"{}\",",
            t.declared_pred.replace('"', "\\\"")
        );
        println!(
            "      \"tighter\": \"{}\"",
            t.tighter_pred.replace('"', "\\\"")
        );
        println!("    }}{comma}");
    }
    println!("  ]");
    println!("}}");
}

// ── Tightening deduplication ──────────────────────────────────────────────────

/// Deduplicate tightening candidates per `(fn_name, declared_pred)`.
///
/// Multiple candidates arise when a function has several return points (branches).
/// We keep the globally-sound tighter bound: the minimum for lower-bound predicates
/// (`>=`/`>`), or the maximum for upper-bound predicates (`<=`/`<`).
fn deduplicate_tightenings(
    candidates: &[TighteningCandidate],
) -> Vec<&TighteningCandidate> {
    // Map (fn_name, declared_pred) → index of the "best" (most conservative) candidate.
    let mut best: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::new();
    for (idx, c) in candidates.iter().enumerate() {
        let key = (c.fn_name.as_str(), c.declared_pred.as_str());
        let keep = best.get(&key).map_or(true, |&prev_idx| {
            let prev = &candidates[prev_idx];
            if c.take_min {
                c.tighter_bound < prev.tighter_bound
            } else {
                c.tighter_bound > prev.tighter_bound
            }
        });
        if keep {
            best.insert(key, idx);
        }
    }
    let mut result: Vec<&TighteningCandidate> = best.values().map(|&i| &candidates[i]).collect();
    result.sort_by(|a, b| a.span.line.cmp(&b.span.line).then(a.fn_name.cmp(&b.fn_name)));
    result
}

// ── Entry point ────────────────────────────────────────────────────────────────

// ── Struct field map ──────────────────────────────────────────────────────────

/// Build `type_name → [(field_name, base_type_name)]` from all parsed programs.
///
/// Used by `try_z3_witness` to create `param__field` Z3 variables for struct
/// parameters and by the test synthesizer to emit struct constructor expressions.
fn build_struct_fields(programs: &[(String, Program)]) -> HashMap<String, Vec<(String, String)>> {
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (_, prog) in programs {
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                if let TypeBody::Struct { fields, .. } = &td.body {
                    let field_list: Vec<(String, String)> = fields
                        .iter()
                        .map(|f| {
                            let base = match &f.ty {
                                TypeExpr::Base { name, .. } => name.clone(),
                                TypeExpr::Refined { inner, .. } => match inner.as_ref() {
                                    TypeExpr::Base { name, .. } => name.clone(),
                                    _ => "?".to_string(),
                                },
                                _ => "?".to_string(),
                            };
                            (f.name.clone(), base)
                        })
                        .collect();
                    out.entry(td.name.clone()).or_default().extend(field_list);
                }
            }
        }
    }
    out
}

// ── Witness formatting ────────────────────────────────────────────────────────

/// Format a `WitnessValue` as a MVL literal or constructor expression.
fn format_witness_value(val: &WitnessValue) -> String {
    match val {
        WitnessValue::Int(n) => n.to_string(),
        WitnessValue::Struct { type_name, fields } => {
            if fields.is_empty() {
                return format!("{type_name} {{}}");
            }
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, v)| format!("{name}: {}", format_witness_value(v)))
                .collect();
            format!("{type_name} {{ {} }}", field_strs.join(", "))
        }
        WitnessValue::Unknown => "_".to_string(),
    }
}

/// Derive a MVL type expression string for a `WitnessValue`.
fn witness_type_str(val: &WitnessValue, param_type: &TypeExpr) -> String {
    match val {
        WitnessValue::Int(_) => "Int".to_string(),
        WitnessValue::Struct { type_name, .. } => type_name.clone(),
        WitnessValue::Unknown => {
            // Fall back to the declared parameter type.
            match param_type {
                TypeExpr::Base { name, .. } => name.clone(),
                _ => "?".to_string(),
            }
        }
    }
}

/// Synthesize a `test fn` MVL snippet for a single witness.
///
/// Returns the full `test fn` block as a string (no trailing newline).
fn synthesize_test_fn(
    fn_name: &str,
    declared_pred: &str,
    tighter_pred: &str,
    witnesses: &[WitnessArg],
    candidate: &TighteningCandidate,
) -> String {
    // Derive a safe identifier from the function name + bound.
    let safe_name = fn_name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_");
    let test_name = format!("harden_boundary_{safe_name}");

    // Build parameter list and call expression.
    let param_strs: Vec<String> = witnesses
        .iter()
        .zip(candidate.params.iter())
        .map(|(w, p)| {
            let ty = witness_type_str(&w.value, &p.ty);
            format!("{}: {ty}", w.param_name)
        })
        .collect();

    let arg_strs: Vec<String> = witnesses
        .iter()
        .map(|w| format_witness_value(&w.value))
        .collect();

    let mut lines = Vec::new();
    lines.push(format!("// Boundary witness for: {declared_pred}"));
    lines.push(format!("// Z3 proved tighter:     {tighter_pred}"));
    lines.push(format!("test fn {test_name}() -> Unit {{"));
    if !param_strs.is_empty() {
        for (w, p) in witnesses.iter().zip(candidate.params.iter()) {
            let ty = witness_type_str(&w.value, &p.ty);
            lines.push(format!("    let {}: {ty} = {};", w.param_name, format_witness_value(&w.value)));
        }
    }
    lines.push(format!("    let result: Int = {fn_name}({});", arg_strs.join(", ")));
    // Emit the tighter postcondition as an assert expression.
    // tighter_pred looks like "ensures result >= 5" → "result >= 5"
    if let Some(cond) = tighter_pred.strip_prefix("ensures ") {
        lines.push(format!("    assert_eq({cond}, true)"));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Run `mvl harden` (Axes 1, 2, and 3) over a `.mvl` file or directory.
pub fn run(path: &str, verbose: bool, json: bool, emit_tests: bool, stdlib_profile: &str, callee_filter: Option<&str>) {
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

    // Build struct field map across all parsed programs for witness synthesis.
    let struct_fields = build_struct_fields(&parsed);

    let mut grand_total_runtime = 0usize;
    let mut grand_total_proven = 0usize;
    let mut grand_total_failed = 0usize;

    // Collect results per file so we can print the human report inline.
    struct FileResult {
        file_str: String,
        sites_data: Vec<(u32, String, String, String, String, HardenHint)>,
        tightenings: Vec<TighteningCandidate>,
        proven: usize,
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
        let mut file_proven = 0usize;
        let mut file_runtime = 0usize;
        let mut file_failed = 0usize;

        // Collect runtime sites (count proven/failed) for this file.
        let mut sites_data: Vec<(u32, String, String, String, String, HardenHint)> = Vec::new();
        for site in all_sites {
            let matches_filter = callee_filter
                .map(|f| site.fn_name == f)
                .unwrap_or(true);
            match &site.outcome {
                ProofOutcome::Proven { .. } if matches_filter => {
                    file_proven += 1;
                }
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

        let tightenings = result.refinement_counts.tightening_candidates.clone();
        grand_total_proven += file_proven;
        grand_total_runtime += file_runtime;
        grand_total_failed += file_failed;
        file_results.push(FileResult {
            file_str: file_str.clone(),
            sites_data,
            tightenings,
            proven: file_proven,
            runtime: file_runtime,
            failed: file_failed,
        });
    }

    if json {
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
        let all_raw_tightenings: Vec<TighteningCandidate> =
            file_results.iter().flat_map(|fr| fr.tightenings.iter().cloned()).collect();
        let all_tightenings = deduplicate_tightenings(&all_raw_tightenings);
        print_json(
            &flat,
            &all_tightenings,
            grand_total_proven,
            grand_total_runtime,
            grand_total_failed,
        );
        if grand_total_failed > 0 {
            process::exit(1);
        }
        return;
    }

    // ── Human-readable report ──────────────────────────────────────────────────

    let sep = "═".repeat(70);

    for fr in &file_results {
        // Skip files with no refinement sites at all (no contracts to analyse).
        if fr.proven == 0 && fr.runtime == 0 && fr.failed == 0 {
            continue;
        }

        println!("\n{sep}");
        println!("  HARDEN REPORT: {}", fr.file_str);
        println!("{sep}");

        // ── Axis 1: runtime → static promotion ────────────────────────────────
        println!("\n── Axis 1: Runtime → Static Promotion ──────────────────────────────");
        if fr.sites_data.is_empty() {
            println!(
                "  {} site(s) proven statically — no runtime obligations.",
                fr.proven
            );
        } else {
            let mut by_caller: HashMap<&str, Vec<_>> = HashMap::new();
            for (line, caller, callee, param, pred, hint) in &fr.sites_data {
                by_caller
                    .entry(caller.as_str())
                    .or_default()
                    .push((line, callee, param, pred, hint));
            }
            let mut callers: Vec<&str> = by_caller.keys().copied().collect();
            callers.sort();

            let mut counter = 0usize;
            for caller in &callers {
                let entries = &by_caller[caller];
                for (line, callee, param, pred, hint) in entries {
                    counter += 1;
                    println!("\n  [{counter:02}] {caller}:{line}  →  {callee}({param})");
                    if verbose {
                        println!("       predicate: {pred}");
                    }
                    println!("       hint: {}", hint.suggestion());
                }
            }
        }

        // ── Axis 2: contract tightening ───────────────────────────────────────
        // Deduplicate per (fn_name, declared_pred): keep the weakest tighter bound
        // that holds across all return branches (min for >=/>; max for <=/< ).
        let deduped = deduplicate_tightenings(&fr.tightenings);
        println!("\n── Axis 2: Contract Tightening ──────────────────────────────────────");
        if deduped.is_empty() {
            println!("  No tightening opportunities found.");
        } else {
            for (i, t) in deduped.iter().enumerate() {
                println!(
                    "\n  [{:02}] {}:{}",
                    i + 1,
                    t.fn_name,
                    t.span.line,
                );
                println!("       declared: {}", t.declared_pred);
                println!("       provable: {}", t.tighter_pred);
                println!("       → Suggest strengthening the postcondition");
            }
        }

        // ── Axis 3: boundary test generation ─────────────────────────────────
        println!("\n── Axis 3: Boundary Test Generation ─────────────────────────────────");
        let mut witness_snippets: Vec<String> = Vec::new();
        let mut witness_use_fns: Vec<String> = Vec::new();
        for t in &deduped {
            match synthesize_witness(&t.params, &t.branch_hyps, &struct_fields) {
                Some(witnesses) if !witnesses.is_empty() => {
                    let snip = synthesize_test_fn(
                        &t.fn_name,
                        &t.declared_pred,
                        &t.tighter_pred,
                        &witnesses,
                        t,
                    );
                    println!("\n  Witness for {}:", t.fn_name);
                    for w in &witnesses {
                        println!("    {} = {}", w.param_name, format_witness_value(&w.value));
                    }
                    if !witness_use_fns.contains(&t.fn_name) {
                        witness_use_fns.push(t.fn_name.clone());
                    }
                    witness_snippets.push(snip);
                }
                _ => {
                    println!("\n  No witness found for {} (non-integer params or Z3 timeout).", t.fn_name);
                }
            }
        }

        let pv = fr.proven;
        let rt = fr.runtime;
        let fa = fr.failed;
        let tg = deduped.len();
        let wc = witness_snippets.len();
        println!(
            "\n  Summary: {pv} proven, {rt} runtime obligations, {fa} failed, {tg} tightening suggestion(s), {wc} witness(es)\n"
        );
        println!("{sep}\n");

        // ── --emit-tests: write boundary test file ────────────────────────────
        if emit_tests && !witness_snippets.is_empty() {
            let file_path = Path::new(&fr.file_str);
            let stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let dir = file_path.parent().unwrap_or_else(|| Path::new("."));
            let out_path = dir.join(format!("{stem}_boundary_test.mvl"));

            // Derive module name from file stem for use imports.
            let module_stem = stem;
            let mut test_lines: Vec<String> = Vec::new();
            test_lines.push(format!("// Generated by `mvl harden --emit-tests` — do not edit by hand."));
            test_lines.push(String::new());
            for fn_name in &witness_use_fns {
                test_lines.push(format!("use {module_stem}::{fn_name};"));
            }
            test_lines.push(String::new());
            for snip in &witness_snippets {
                test_lines.push(snip.clone());
                test_lines.push(String::new());
            }
            let content = test_lines.join("\n");
            match std::fs::write(&out_path, &content) {
                Ok(()) => println!("  Wrote boundary tests → {}\n", out_path.display()),
                Err(e) => eprintln!("  warning: could not write {}: {e}", out_path.display()),
            }
        }
    }

    // Multi-file grand total.
    if check_count > 1 {
        let all_raw: Vec<TighteningCandidate> =
            file_results.iter().flat_map(|fr| fr.tightenings.iter().cloned()).collect();
        let grand_deduped = deduplicate_tightenings(&all_raw).len();
        println!(
            "Total: {grand_total_proven} proven, {grand_total_runtime} runtime obligations, \
             {grand_total_failed} failed, {grand_deduped} tightening suggestion(s)"
        );
    }

    if grand_total_failed > 0 {
        process::exit(1);
    }
}
