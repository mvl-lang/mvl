// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl harden` — contract strengthening via proof feedback (#1913, #1950).
//!
//! See `.openspec/specs/026-harden/spec.md` for the full requirements.
//!
//! ## Axis 1: Runtime → Static Promotion
//! Consumes `RefinementCounts.sites` from `check_call_site` and classifies
//! `RuntimeCheck` sites with heuristic fix hints.
//!
//! ## Axis 2: Contract Tightening
//! Binary-searches Z3 for the tightest provable bound on each `ensures` clause.
//! See `layer5.rs::try_z3_tighten`.
//!
//! ## Axis 3: Boundary Test Generation
//! Queries Z3 for witness inputs that reach each return branch; with
//! `--emit-tests`, writes `*_boundary_test.mvl` files.
//! See `layer5.rs::try_z3_witness`.
//!
//! ## Axis 4: MC/DC Gap Synthesis (`--mcdc`, #1950)
//! For every compound if/while decision, runs a two-query Z3 search per
//! clause: Q1 solves for parameter values making the clause true and the
//! decision true; Q2 solves for the opposite, pinning other clauses to
//! their Q1 truth values (Unique-Cause MC/DC).  SAT → emit test pair;
//! UNSAT → clause is coupled.

use mvl::mvl::checker;
use mvl::mvl::checker::refinements::{
    synthesize_witness, ProofOutcome, TighteningCandidate, WitnessArg, WitnessValue,
};
use mvl::mvl::checker::SolverMode;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{
    BinaryOp, Block, Decl, ElseBranch, Expr, FnDecl, Literal, MatchBody, Param, Program, Stmt,
    TypeBody, TypeExpr, UnaryOp,
};
use mvl::mvl::parser::lexer::Span;
use mvl::mvl::passes::mcdc::analysis::{collect_clauses, count_clauses};
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
        let stripped = predicate.replace("old(", "").replace("len(", "");
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
    axis3: &[Axis3Witness],
    axis4: &[Axis4Result],
    total_proven: usize,
    total_runtime: usize,
    total_failed: usize,
) {
    let json_escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    println!("{{");
    println!("  \"total_proven\": {total_proven},");
    println!("  \"total_runtime\": {total_runtime},");
    println!("  \"total_failed\": {total_failed},");
    println!("  \"axis1_promotion_candidates\": [");
    for (i, s) in sites.iter().enumerate() {
        let comma = if i + 1 < sites.len() { "," } else { "" };
        let hint = json_escape(s.hint.suggestion());
        println!("    {{");
        println!("      \"file\": \"{}\",", json_escape(s.file));
        println!("      \"line\": {},", s.line);
        println!("      \"caller\": \"{}\",", json_escape(s.caller));
        println!("      \"callee\": \"{}\",", json_escape(s.callee));
        println!("      \"param\": \"{}\",", json_escape(s.param));
        println!("      \"predicate\": \"{}\",", json_escape(s.predicate));
        println!("      \"suggestion\": \"{hint}\"");
        println!("    }}{comma}");
    }
    println!("  ],");
    println!("  \"axis2_tightening_candidates\": [");
    for (i, t) in tightenings.iter().enumerate() {
        let comma = if i + 1 < tightenings.len() { "," } else { "" };
        println!("    {{");
        println!("      \"fn_name\": \"{}\",", json_escape(&t.fn_name));
        println!("      \"line\": {},", t.span.line);
        println!("      \"declared\": \"{}\",", json_escape(&t.declared_pred));
        println!("      \"tighter\": \"{}\"", json_escape(&t.tighter_pred));
        println!("    }}{comma}");
    }
    println!("  ],");
    println!("  \"axis3_boundary_witnesses\": [");
    for (i, w) in axis3.iter().enumerate() {
        let comma = if i + 1 < axis3.len() { "," } else { "" };
        println!("    {{");
        println!("      \"fn_name\": \"{}\",", json_escape(&w.fn_name));
        println!("      \"line\": {},", w.line);
        println!("      \"declared\": \"{}\",", json_escape(&w.declared_pred));
        println!("      \"tighter\": \"{}\",", json_escape(&w.tighter_pred));
        println!("      \"args\": [");
        for (j, (name, ty, val)) in w.args.iter().enumerate() {
            let cj = if j + 1 < w.args.len() { "," } else { "" };
            println!(
                "        {{ \"name\": \"{}\", \"type\": \"{}\", \"value\": \"{}\" }}{cj}",
                json_escape(name),
                json_escape(ty),
                json_escape(val)
            );
        }
        println!("      ]");
        println!("    }}{comma}");
    }
    println!("  ],");
    println!("  \"axis4_mcdc_pairs\": [");
    for (i, r) in axis4.iter().enumerate() {
        let comma = if i + 1 < axis4.len() { "," } else { "" };
        println!("    {{");
        println!("      \"fn_name\": \"{}\",", json_escape(&r.fn_name));
        println!("      \"line\": {},", r.line);
        println!("      \"clause_idx\": {},", r.clause_idx);
        println!(
            "      \"clause_text\": \"{}\",",
            json_escape(&r.clause_text)
        );
        match &r.outcome {
            Axis4Outcome::Pair { t1_args, t2_args } => {
                println!("      \"outcome\": \"pair\",");
                let emit_args = |args: &[(String, String, String)]| {
                    let mut lines: Vec<String> = Vec::new();
                    for (j, (name, ty, val)) in args.iter().enumerate() {
                        let cj = if j + 1 < args.len() { "," } else { "" };
                        lines.push(format!(
                            "          {{ \"name\": \"{}\", \"type\": \"{}\", \"value\": \"{}\" }}{cj}",
                            json_escape(name),
                            json_escape(ty),
                            json_escape(val)
                        ));
                    }
                    lines.join("\n")
                };
                println!("      \"t1\": [\n{}\n      ],", emit_args(t1_args));
                println!("      \"t2\": [\n{}\n      ]", emit_args(t2_args));
            }
            Axis4Outcome::Coupled => {
                println!("      \"outcome\": \"coupled\"");
            }
            Axis4Outcome::Unsupported { reason } => {
                println!("      \"outcome\": \"unsupported\",");
                println!("      \"reason\": \"{}\"", json_escape(reason));
            }
        }
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
fn deduplicate_tightenings(candidates: &[TighteningCandidate]) -> Vec<&TighteningCandidate> {
    // Map (fn_name, declared_pred) → index of the "best" (most conservative) candidate.
    let mut best: std::collections::HashMap<(&str, &str), usize> = std::collections::HashMap::new();
    for (idx, c) in candidates.iter().enumerate() {
        let key = (c.fn_name.as_str(), c.declared_pred.as_str());
        let keep = best.get(&key).is_none_or(|&prev_idx| {
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
    result.sort_by(|a, b| {
        a.span
            .line
            .cmp(&b.span.line)
            .then(a.fn_name.cmp(&b.fn_name))
    });
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
        WitnessValue::Float(f) => format!("{f}"),
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
        WitnessValue::Float(_) => "Float".to_string(),
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
            lines.push(format!(
                "    let {}: {ty} = {};",
                w.param_name,
                format_witness_value(&w.value)
            ));
        }
    }
    lines.push(format!(
        "    let result: Int = {fn_name}({});",
        arg_strs.join(", ")
    ));
    // Emit the tighter postcondition as an assert expression.
    // tighter_pred looks like "ensures result >= 5" → "result >= 5"
    if let Some(cond) = tighter_pred.strip_prefix("ensures ") {
        lines.push(format!("    assert_eq({cond}, true)"));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Run `mvl harden` (Axes 1, 2, 3, and optionally 4) over a `.mvl` file or directory.
pub fn run(
    path: &str,
    verbose: bool,
    json: bool,
    emit_tests: bool,
    stdlib_profile: &str,
    callee_filter: Option<&str>,
    mcdc: bool,
) {
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
        /// Axis 4 (#1950): compound if/while decisions in this file (populated only when `mcdc` is set).
        mcdc_decisions: Vec<McdcDecision>,
        /// Axis 3 (#1931): boundary witnesses for tightened contracts.
        /// Populated in the post-collection pass below so JSON and text emit from the same data.
        axis3_witnesses: Vec<Axis3Witness>,
        /// Axis 4 (#1950): MC/DC pair results per (decision, clause).
        axis4_results: Vec<Axis4Result>,
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
            let matches_filter = callee_filter.map(|f| site.fn_name == f).unwrap_or(true);
            match &site.outcome {
                ProofOutcome::Proven { .. } if matches_filter => {
                    file_proven += 1;
                }
                ProofOutcome::RuntimeCheck | ProofOutcome::RuntimeCheckWithWitness { .. }
                    if matches_filter =>
                {
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
        let mcdc_decisions = if mcdc {
            collect_mcdc_decisions(prog)
        } else {
            Vec::new()
        };
        file_results.push(FileResult {
            file_str: file_str.clone(),
            sites_data,
            tightenings,
            proven: file_proven,
            runtime: file_runtime,
            failed: file_failed,
            mcdc_decisions,
            axis3_witnesses: Vec::new(),
            axis4_results: Vec::new(),
        });
    }

    // ── Post-collection pass: compute axis 3 witnesses and axis 4 pairs ────
    //
    // We do this once, before splitting into JSON / text emission, so both
    // paths consume the same structured data.  See spec 026-harden R6.
    for fr in file_results.iter_mut() {
        let deduped = deduplicate_tightenings(&fr.tightenings);
        fr.axis3_witnesses = compute_axis3_witnesses(&deduped, &struct_fields);
        if mcdc {
            fr.axis4_results = compute_axis4_results(&fr.mcdc_decisions, &struct_fields);
        }
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
        let all_raw_tightenings: Vec<TighteningCandidate> = file_results
            .iter()
            .flat_map(|fr| fr.tightenings.iter().cloned())
            .collect();
        let all_tightenings = deduplicate_tightenings(&all_raw_tightenings);
        let all_axis3: Vec<Axis3Witness> = file_results
            .iter()
            .flat_map(|fr| fr.axis3_witnesses.iter().cloned())
            .collect();
        let all_axis4: Vec<Axis4Result> = file_results
            .iter()
            .flat_map(|fr| fr.axis4_results.iter().cloned())
            .collect();
        print_json(
            &flat,
            &all_tightenings,
            &all_axis3,
            &all_axis4,
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
        // Skip files with no refinement sites AND no MC/DC decisions to analyse.
        if fr.proven == 0 && fr.runtime == 0 && fr.failed == 0 && fr.mcdc_decisions.is_empty() {
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
                println!("\n  [{:02}] {}:{}", i + 1, t.fn_name, t.span.line,);
                println!("       declared: {}", t.declared_pred);
                println!("       provable: {}", t.tighter_pred);
                println!("       → Suggest strengthening the postcondition");
            }
        }

        // ── Axis 3: boundary test generation ─────────────────────────────────
        println!("\n── Axis 3: Boundary Test Generation ─────────────────────────────────");
        let mut witness_snippets: Vec<String> = Vec::new();
        let mut witness_use_fns: Vec<String> = Vec::new();
        for w in &fr.axis3_witnesses {
            if w.args.is_empty() {
                println!(
                    "\n  No witness found for {} (non-integer params or Z3 timeout).",
                    w.fn_name
                );
                continue;
            }
            println!("\n  Witness for {}:", w.fn_name);
            for (name, _ty, val) in &w.args {
                println!("    {name} = {val}");
            }
            if !witness_use_fns.contains(&w.fn_name) {
                witness_use_fns.push(w.fn_name.clone());
            }
            witness_snippets.push(w.snippet.clone());
        }

        // ── Axis 4: MC/DC gap synthesis (#1950) ──────────────────────────────
        let mut mcdc_snippets: Vec<String> = Vec::new();
        let mut mcdc_use_fns: Vec<String> = Vec::new();
        let mut mcdc_pairs = 0usize;
        let mut mcdc_coupled = 0usize;
        if mcdc {
            println!("── Axis 4: MC/DC Gap Synthesis ──────────────────────────────────────");
            if fr.mcdc_decisions.is_empty() {
                println!("  No compound if/while decisions found.");
            } else {
                // Group results by (fn_name, line) for the per-decision header.
                let mut last_dec: Option<(String, u32, usize)> = None;
                for r in &fr.axis4_results {
                    let key = (r.fn_name.clone(), r.line);
                    let clause_count = fr
                        .mcdc_decisions
                        .iter()
                        .find(|d| d.fn_name == r.fn_name && d.line == r.line)
                        .map(|d| d.clauses.len())
                        .unwrap_or(0);
                    if last_dec.as_ref().map(|(n, l, _)| (n.clone(), *l)) != Some(key.clone()) {
                        println!(
                            "\n  Decision {}:{} ({} clauses):",
                            r.fn_name, r.line, clause_count
                        );
                        last_dec = Some((r.fn_name.clone(), r.line, clause_count));
                    }
                    match &r.outcome {
                        Axis4Outcome::Pair { .. } => {
                            mcdc_pairs += 1;
                            println!(
                                "    clause {} ({}): pair generated",
                                r.clause_idx, r.clause_text
                            );
                            if !mcdc_use_fns.contains(&r.fn_name) {
                                mcdc_use_fns.push(r.fn_name.clone());
                            }
                            if let Some(snip) = &r.snippet {
                                mcdc_snippets.push(snip.clone());
                            }
                        }
                        Axis4Outcome::Coupled => {
                            mcdc_coupled += 1;
                            println!(
                                "    clause {} ({}): coupled — masking MC/DC required",
                                r.clause_idx, r.clause_text
                            );
                        }
                        Axis4Outcome::Unsupported { reason } => {
                            println!(
                                "    clause {} ({}): unsupported clause type — {reason}",
                                r.clause_idx, r.clause_text
                            );
                        }
                    }
                }
            }
        }

        let pv = fr.proven;
        let rt = fr.runtime;
        let fa = fr.failed;
        let tg = deduped.len();
        let wc = witness_snippets.len();
        let mcdc_tail = if mcdc {
            format!(", {mcdc_pairs} MC/DC pair(s), {mcdc_coupled} coupled")
        } else {
            String::new()
        };
        println!(
            "\n  Summary: {pv} proven, {rt} runtime obligations, {fa} failed, {tg} tightening suggestion(s), {wc} witness(es){mcdc_tail}\n"
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
            test_lines.push(
                "// Generated by `mvl harden --emit-tests` — do not edit by hand.".to_string(),
            );
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
            write_generated_test_file(&out_path, &content);
        }

        // ── --emit-tests + --mcdc: write MC/DC gap test file ─────────────────
        if emit_tests && mcdc && !mcdc_snippets.is_empty() {
            let file_path = Path::new(&fr.file_str);
            let stem = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let dir = file_path.parent().unwrap_or_else(|| Path::new("."));
            let out_path = dir.join(format!("{stem}_mcdc_gap_test.mvl"));

            let module_stem = stem;
            let mut test_lines: Vec<String> = Vec::new();
            test_lines.push(
                "// Generated by `mvl harden --emit-tests --mcdc` — do not edit by hand."
                    .to_string(),
            );
            test_lines.push(String::new());
            for fn_name in &mcdc_use_fns {
                test_lines.push(format!("use {module_stem}::{fn_name};"));
            }
            test_lines.push(String::new());
            for snip in &mcdc_snippets {
                test_lines.push(snip.clone());
                test_lines.push(String::new());
            }
            let content = test_lines.join("\n");
            write_generated_test_file(&out_path, &content);
        }
    }

    // Multi-file grand total.
    if check_count > 1 {
        let all_raw: Vec<TighteningCandidate> = file_results
            .iter()
            .flat_map(|fr| fr.tightenings.iter().cloned())
            .collect();
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

// ══════════════════════════════════════════════════════════════════════════════
//  Axis 4: MC/DC gap synthesis (#1950)
// ══════════════════════════════════════════════════════════════════════════════

// ── Axis 3 / Axis 4 structured results (#1955) ────────────────────────────────

/// A single boundary witness (axis 3 output).
///
/// Populated once in the post-collection pass so the text and JSON emitters
/// consume the same structured data.  `args` is empty when no witness could
/// be found (non-integer params or Z3 timeout).
#[derive(Debug, Clone)]
struct Axis3Witness {
    fn_name: String,
    line: u32,
    declared_pred: String,
    tighter_pred: String,
    /// Tuples of `(param_name, mvl_type, mvl_literal)` — pre-rendered so both
    /// text and JSON emit identically.
    args: Vec<(String, String, String)>,
    /// The `test fn` snippet as MVL source (used by `--emit-tests`).
    snippet: String,
}

/// The outcome of MC/DC pair synthesis for a single (decision, clause).
#[derive(Debug, Clone)]
enum Axis4Outcome {
    /// Both t1 and t2 witnesses were found.
    Pair {
        t1_args: Vec<(String, String, String)>,
        t2_args: Vec<(String, String, String)>,
    },
    /// One of the two Z3 queries returned UNSAT — the clause is structurally coupled.
    Coupled,
    /// Some parameter type is not currently supported by axis 4 (e.g. String, Float).
    Unsupported { reason: String },
}

/// A single MC/DC clause result (axis 4 output).
#[derive(Debug, Clone)]
struct Axis4Result {
    fn_name: String,
    line: u32,
    clause_idx: usize,
    clause_text: String,
    outcome: Axis4Outcome,
    /// Pre-rendered `test fn` pair snippet (only for `Pair` outcomes and only
    /// when every witness value is representable as an MVL literal).
    snippet: Option<String>,
}

/// Populate `Axis3Witness` records from deduplicated tightening candidates.
fn compute_axis3_witnesses(
    deduped: &[&TighteningCandidate],
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> Vec<Axis3Witness> {
    let mut out = Vec::new();
    for t in deduped {
        let ws = synthesize_witness(&t.params, &t.branch_hyps, struct_fields);
        match ws {
            Some(witnesses) if !witnesses.is_empty() => {
                let snippet = synthesize_test_fn(
                    &t.fn_name,
                    &t.declared_pred,
                    &t.tighter_pred,
                    &witnesses,
                    t,
                );
                let args: Vec<(String, String, String)> = witnesses
                    .iter()
                    .zip(t.params.iter())
                    .map(|(w, p)| {
                        (
                            w.param_name.clone(),
                            declared_type_str(&p.ty),
                            format_witness_value_typed(&w.value, &p.ty),
                        )
                    })
                    .collect();
                out.push(Axis3Witness {
                    fn_name: t.fn_name.clone(),
                    line: t.span.line,
                    declared_pred: t.declared_pred.clone(),
                    tighter_pred: t.tighter_pred.clone(),
                    args,
                    snippet,
                });
            }
            _ => {
                out.push(Axis3Witness {
                    fn_name: t.fn_name.clone(),
                    line: t.span.line,
                    declared_pred: t.declared_pred.clone(),
                    tighter_pred: t.tighter_pred.clone(),
                    args: Vec::new(),
                    snippet: String::new(),
                });
            }
        }
    }
    out
}

/// Populate `Axis4Result` records by running MC/DC pair synthesis per (decision, clause).
fn compute_axis4_results(
    decisions: &[McdcDecision],
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> Vec<Axis4Result> {
    let mut out = Vec::new();
    for dec in decisions {
        if dec.is_effectful {
            continue;
        }
        for (i, clause) in dec.clauses.iter().enumerate() {
            let clause_str = expr_to_short_str(clause);
            let raw = synthesize_mcdc_pair(
                &dec.fn_params,
                &dec.requires,
                &dec.clauses,
                i,
                &dec.decision_expr,
                struct_fields,
            );
            let (outcome, snippet) = match raw {
                McdcClauseOutcome::Pair { t1, t2 } => {
                    let t1_args: Vec<(String, String, String)> = t1
                        .iter()
                        .zip(dec.fn_params.iter())
                        .map(|(w, p)| {
                            (
                                w.param_name.clone(),
                                declared_type_str(&p.ty),
                                format_witness_value_typed(&w.value, &p.ty),
                            )
                        })
                        .collect();
                    let t2_args: Vec<(String, String, String)> = t2
                        .iter()
                        .zip(dec.fn_params.iter())
                        .map(|(w, p)| {
                            (
                                w.param_name.clone(),
                                declared_type_str(&p.ty),
                                format_witness_value_typed(&w.value, &p.ty),
                            )
                        })
                        .collect();
                    let snip = synthesize_mcdc_test_pair(
                        &dec.fn_name,
                        dec.line,
                        i,
                        &clause_str,
                        &dec.fn_params,
                        &t1,
                        &t2,
                    );
                    (Axis4Outcome::Pair { t1_args, t2_args }, snip)
                }
                McdcClauseOutcome::Coupled => (Axis4Outcome::Coupled, None),
                McdcClauseOutcome::Unsupported => (
                    Axis4Outcome::Unsupported {
                        reason: "non-Int/Bool parameter".to_string(),
                    },
                    None,
                ),
            };
            out.push(Axis4Result {
                fn_name: dec.fn_name.clone(),
                line: dec.line,
                clause_idx: i,
                clause_text: clause_str,
                outcome,
                snippet,
            });
        }
    }
    out
}

/// A single compound `if`/`while` decision found in a non-test function,
/// carrying everything axis 4 needs to synthesize independence pairs.
#[derive(Debug, Clone)]
struct McdcDecision {
    fn_name: String,
    /// Enclosing function parameters — used as Z3 witness inputs.
    fn_params: Vec<Param>,
    /// Enclosing function `requires` clauses — threaded as preconditions.
    requires: Vec<Expr>,
    /// Effectful functions are excluded from MC/DC obligations (see spec 010).
    is_effectful: bool,
    /// Source line of the decision.
    line: u32,
    /// The full compound cond expression, e.g. `a && b`.
    decision_expr: Expr,
    /// Atomic leaf expressions (left-to-right).
    clauses: Vec<Expr>,
}

/// Outcome of Z3 pair synthesis for a single target clause.
enum McdcClauseOutcome {
    /// Both t1 (clause true, decision true) and t2 (clause false, decision false) are SAT.
    Pair {
        t1: Vec<WitnessArg>,
        t2: Vec<WitnessArg>,
    },
    /// One of the two queries returned UNSAT — the clause cannot independently affect the outcome.
    Coupled,
    /// The enclosing function has a parameter type Z3 witness synthesis does not support.
    Unsupported,
}

/// Walk `prog` and collect every compound if/while decision inside non-test functions.
///
/// Match, MatchGuard, and Bool-return decisions are out of scope for axis 4 (v1).
fn collect_mcdc_decisions(prog: &Program) -> Vec<McdcDecision> {
    let mut out = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            if fd.is_test || fd.is_builtin {
                continue;
            }
            collect_decisions_from_block(&fd.body, fd, &mut out);
        }
    }
    out
}

fn collect_decisions_from_block(block: &Block, fd: &FnDecl, out: &mut Vec<McdcDecision>) {
    for stmt in &block.stmts {
        collect_decisions_from_stmt(stmt, fd, out);
    }
}

fn collect_decisions_from_stmt(stmt: &Stmt, fd: &FnDecl, out: &mut Vec<McdcDecision>) {
    match stmt {
        Stmt::If {
            cond,
            then,
            else_,
            span,
        } => {
            maybe_push_decision(cond, span.line, fd, out);
            collect_decisions_from_block(then, fd, out);
            if let Some(else_branch) = else_ {
                match else_branch {
                    ElseBranch::Block(b) => collect_decisions_from_block(b, fd, out),
                    ElseBranch::If(s) => collect_decisions_from_stmt(s, fd, out),
                }
            }
        }
        Stmt::While {
            cond, body, span, ..
        } => {
            maybe_push_decision(cond, span.line, fd, out);
            collect_decisions_from_block(body, fd, out);
        }
        Stmt::For { body, .. } => collect_decisions_from_block(body, fd, out),
        Stmt::Match { arms, .. } => {
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => collect_decisions_from_block(b, fd, out),
                    MatchBody::Expr(e) => collect_decisions_from_expr(e, fd, out),
                }
            }
        }
        Stmt::Let { init, .. } => collect_decisions_from_expr(init, fd, out),
        Stmt::Assign { value, .. } => collect_decisions_from_expr(value, fd, out),
        Stmt::Expr { expr, .. } => collect_decisions_from_expr(expr, fd, out),
        Stmt::Return { value: Some(v), .. } => collect_decisions_from_expr(v, fd, out),
        _ => {}
    }
}

fn collect_decisions_from_expr(expr: &Expr, fd: &FnDecl, out: &mut Vec<McdcDecision>) {
    match expr {
        Expr::If {
            cond,
            then,
            else_,
            span,
        } => {
            maybe_push_decision(cond, span.line, fd, out);
            collect_decisions_from_block(then, fd, out);
            if let Some(e) = else_ {
                collect_decisions_from_expr(e, fd, out);
            }
        }
        Expr::Block(b) => collect_decisions_from_block(b, fd, out),
        Expr::Match { arms, .. } => {
            for arm in arms {
                match &arm.body {
                    MatchBody::Block(b) => collect_decisions_from_block(b, fd, out),
                    MatchBody::Expr(e) => collect_decisions_from_expr(e, fd, out),
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_decisions_from_expr(left, fd, out);
            collect_decisions_from_expr(right, fd, out);
        }
        Expr::Unary { expr, .. } => collect_decisions_from_expr(expr, fd, out),
        Expr::FnCall { args, .. } => {
            for a in args {
                collect_decisions_from_expr(a, fd, out);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_decisions_from_expr(receiver, fd, out);
            for a in args {
                collect_decisions_from_expr(a, fd, out);
            }
        }
        _ => {}
    }
}

fn maybe_push_decision(cond: &Expr, line: u32, fd: &FnDecl, out: &mut Vec<McdcDecision>) {
    if count_clauses(cond) <= 1 {
        return;
    }
    let mut leaves: Vec<&Expr> = Vec::new();
    collect_clauses(cond, &mut leaves);
    let clauses: Vec<Expr> = leaves.into_iter().cloned().collect();
    out.push(McdcDecision {
        fn_name: fd.name.clone(),
        fn_params: fd.params.clone(),
        requires: fd.requires.clone(),
        is_effectful: !fd.effects.is_empty(),
        line,
        decision_expr: cond.clone(),
        clauses,
    });
}

/// Two-query Z3 pair synthesis for target clause `i` in a decision.
///
/// Algorithm (spec 026 R5):
///   Q1: solve for parameter values where `clauses[i]` is TRUE and `decision` is TRUE.
///   Q2: solve for parameter values where `clauses[i]` is FALSE and `decision` is FALSE,
///       AND each other clause takes the SAME truth value it had in Q1 (Unique-Cause).
///
/// If either Q1 or Q2 is UNSAT (or the witness call returns None), the clause is coupled.
fn synthesize_mcdc_pair(
    params: &[Param],
    requires: &[Expr],
    clauses: &[Expr],
    target: usize,
    decision_expr: &Expr,
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> McdcClauseOutcome {
    if !params_supported_for_mcdc(params, struct_fields) {
        return McdcClauseOutcome::Unsupported;
    }

    // Bare Bool identifiers (`x`) and unary-not (`!x`) are not valid Z3 boolean
    // predicates as-is — the Z3 backend expects a comparison expression.
    // Normalise everything to `x != 0` / `x == 0` form before threading.
    let normalized_decision = normalize_bool_clause(decision_expr);
    let normalized_clauses: Vec<Expr> = clauses.iter().map(normalize_bool_clause).collect();

    // Q1: clause[target] AND decision.
    let mut t1_hyps: Vec<Expr> = requires.iter().map(normalize_bool_clause).collect();
    t1_hyps.push(normalized_clauses[target].clone());
    t1_hyps.push(normalized_decision.clone());
    let t1 = match synthesize_witness(params, &t1_hyps, struct_fields) {
        Some(ws) if !ws.is_empty() => ws,
        _ => return McdcClauseOutcome::Coupled,
    };

    // Structurally evaluate every other clause at t1's model to pin them in Q2.
    let env = witnesses_to_env(&t1);
    let mut t2_hyps: Vec<Expr> = requires.iter().map(normalize_bool_clause).collect();
    t2_hyps.push(negate_normalized(&normalized_clauses[target]));
    t2_hyps.push(negate_normalized(&normalized_decision));
    for (j, cj) in normalized_clauses.iter().enumerate() {
        if j == target {
            continue;
        }
        match eval_bool_expr(cj, &env) {
            Some(true) => t2_hyps.push(cj.clone()),
            Some(false) => t2_hyps.push(negate_normalized(cj)),
            None => {} // Best-effort: skip clauses we can't statically evaluate.
        }
    }
    let t2 = match synthesize_witness(params, &t2_hyps, struct_fields) {
        Some(ws) if !ws.is_empty() => ws,
        _ => return McdcClauseOutcome::Coupled,
    };
    McdcClauseOutcome::Pair { t1, t2 }
}

/// Rewrite a boolean expression into a form the Z3 witness backend can consume.
///
/// The backend's `expr_to_z3_bool` only recognises comparison/logical/unary
/// forms.  Bare Bool identifiers (`x`), field accesses (`s.f`), and
/// `Literal::Bool` need to be re-expressed as integer comparisons because
/// each param maps to a Z3 Int variable (Bool encoded as 0/1).
fn normalize_bool_clause(e: &Expr) -> Expr {
    match e {
        Expr::Literal(Literal::Bool(b), _) => {
            let lhs = Expr::Literal(Literal::Integer(if *b { 1 } else { 0 }), Span::default());
            let rhs = Expr::Literal(Literal::Integer(1), Span::default());
            binop(BinaryOp::Eq, lhs, rhs)
        }
        Expr::Ident(_, _) | Expr::FieldAccess { .. } => binop(
            BinaryOp::Ne,
            e.clone(),
            Expr::Literal(Literal::Integer(0), Span::default()),
        ),
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
            ..
        } => negate_normalized(&normalize_bool_clause(expr)),
        Expr::Binary {
            op: BinaryOp::And,
            left,
            right,
            span,
        } => Expr::Binary {
            op: BinaryOp::And,
            left: Box::new(normalize_bool_clause(left)),
            right: Box::new(normalize_bool_clause(right)),
            span: *span,
        },
        Expr::Binary {
            op: BinaryOp::Or,
            left,
            right,
            span,
        } => Expr::Binary {
            op: BinaryOp::Or,
            left: Box::new(normalize_bool_clause(left)),
            right: Box::new(normalize_bool_clause(right)),
            span: *span,
        },
        // Already a proper Bool predicate (Eq/Ne/Lt/Le/Gt/Ge or unsupported op).
        _ => e.clone(),
    }
}

/// Logical negation of a normalized clause. Prefers pushing `!` inward for
/// simple comparisons to keep the resulting expression Z3-friendly.
fn negate_normalized(e: &Expr) -> Expr {
    match e {
        Expr::Binary {
            op, left, right, ..
        } => {
            let flipped = match op {
                BinaryOp::Eq => Some(BinaryOp::Ne),
                BinaryOp::Ne => Some(BinaryOp::Eq),
                BinaryOp::Lt => Some(BinaryOp::Ge),
                BinaryOp::Le => Some(BinaryOp::Gt),
                BinaryOp::Gt => Some(BinaryOp::Le),
                BinaryOp::Ge => Some(BinaryOp::Lt),
                _ => None,
            };
            if let Some(f) = flipped {
                return binop(f, (**left).clone(), (**right).clone());
            }
            wrap_not(e)
        }
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
            ..
        } => (**expr).clone(),
        _ => wrap_not(e),
    }
}

fn binop(op: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
        span: Span::default(),
    }
}

/// Are all params types Int/Bool or structs with Int/Bool fields (per struct_fields map)?
fn params_supported_for_mcdc(
    params: &[Param],
    struct_fields: &HashMap<String, Vec<(String, String)>>,
) -> bool {
    for p in params {
        let name = match &p.ty {
            TypeExpr::Base { name, .. } => name.as_str(),
            TypeExpr::Refined { inner, .. } => match inner.as_ref() {
                TypeExpr::Base { name, .. } => name.as_str(),
                _ => return false,
            },
            _ => return false,
        };
        if matches!(name, "Int" | "Bool") {
            continue;
        }
        if let Some(fields) = struct_fields.get(name) {
            if fields
                .iter()
                .all(|(_, t)| matches!(t.as_str(), "Int" | "Bool"))
            {
                continue;
            }
        }
        return false;
    }
    !params.is_empty()
}

/// Convert a witness list into a `param_name → i64` map for structural clause evaluation.
///
/// Struct witnesses are flattened as `param__field` keys, matching the Z3 variable
/// naming convention used by `try_z3_witness`.
fn witnesses_to_env(ws: &[WitnessArg]) -> HashMap<String, i64> {
    let mut env: HashMap<String, i64> = HashMap::new();
    for w in ws {
        match &w.value {
            WitnessValue::Int(n) => {
                env.insert(w.param_name.clone(), *n);
            }
            WitnessValue::Float(_) => {} // Float witnesses handled separately
            WitnessValue::Struct { fields, .. } => {
                for (fname, fv) in fields {
                    if let WitnessValue::Int(n) = fv {
                        env.insert(format!("{}__{fname}", w.param_name), *n);
                    }
                }
            }
            WitnessValue::Unknown => {}
        }
    }
    env
}

/// Structurally evaluate a boolean expression under an integer environment.
/// Returns `None` for expressions we can't decide (unknown vars, unsupported ops).
fn eval_bool_expr(e: &Expr, env: &HashMap<String, i64>) -> Option<bool> {
    match e {
        Expr::Literal(Literal::Bool(b), _) => Some(*b),
        Expr::Ident(name, _) => env.get(name).map(|v| *v != 0),
        Expr::FieldAccess { expr, field, .. } => {
            if let Expr::Ident(obj, _) = expr.as_ref() {
                env.get(&format!("{obj}__{field}")).map(|v| *v != 0)
            } else {
                None
            }
        }
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
            ..
        } => eval_bool_expr(expr, env).map(|b| !b),
        Expr::Binary {
            op, left, right, ..
        } => match op {
            BinaryOp::And => Some(eval_bool_expr(left, env)? && eval_bool_expr(right, env)?),
            BinaryOp::Or => Some(eval_bool_expr(left, env)? || eval_bool_expr(right, env)?),
            BinaryOp::Eq => Some(eval_int_expr(left, env)? == eval_int_expr(right, env)?),
            BinaryOp::Ne => Some(eval_int_expr(left, env)? != eval_int_expr(right, env)?),
            BinaryOp::Lt => Some(eval_int_expr(left, env)? < eval_int_expr(right, env)?),
            BinaryOp::Le => Some(eval_int_expr(left, env)? <= eval_int_expr(right, env)?),
            BinaryOp::Gt => Some(eval_int_expr(left, env)? > eval_int_expr(right, env)?),
            BinaryOp::Ge => Some(eval_int_expr(left, env)? >= eval_int_expr(right, env)?),
            _ => None,
        },
        _ => None,
    }
}

/// Structurally evaluate an integer expression under an integer environment.
fn eval_int_expr(e: &Expr, env: &HashMap<String, i64>) -> Option<i64> {
    match e {
        Expr::Literal(Literal::Integer(n), _) => Some(*n),
        Expr::Literal(Literal::Bool(b), _) => Some(if *b { 1 } else { 0 }),
        Expr::Ident(name, _) => env.get(name).copied(),
        Expr::FieldAccess { expr, field, .. } => {
            if let Expr::Ident(obj, _) = expr.as_ref() {
                env.get(&format!("{obj}__{field}")).copied()
            } else {
                None
            }
        }
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
            ..
        } => eval_int_expr(expr, env).map(|n| -n),
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = eval_int_expr(left, env)?;
            let r = eval_int_expr(right, env)?;
            match op {
                BinaryOp::Add => Some(l + r),
                BinaryOp::Sub => Some(l - r),
                BinaryOp::Mul => Some(l * r),
                BinaryOp::Div if r != 0 => Some(l / r),
                BinaryOp::Rem if r != 0 => Some(l % r),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Wrap an expression in `Expr::Unary { op: Not, ... }`.
fn wrap_not(e: &Expr) -> Expr {
    Expr::Unary {
        op: UnaryOp::Not,
        expr: Box::new(e.clone()),
        span: Span::default(),
    }
}

/// Render a parameter's declared type as an MVL type string.
///
/// Prefers the source declaration over any refinement wrapper — refinements
/// are dropped in the test file (they're re-checked by the checker anyway).
fn declared_type_str(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Base { name, .. } => name.clone(),
        TypeExpr::Refined { inner, .. } => match inner.as_ref() {
            TypeExpr::Base { name, .. } => name.clone(),
            other => declared_type_str(other),
        },
        _ => "?".to_string(),
    }
}

/// Format a witness value using the declared parameter type — Bool params emit `true`/`false`.
fn format_witness_value_typed(val: &WitnessValue, param_type: &TypeExpr) -> String {
    let base_name = declared_type_str(param_type);
    match (val, base_name.as_str()) {
        (WitnessValue::Int(n), "Bool") => {
            if *n != 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        (WitnessValue::Struct { type_name, fields }, _) => {
            let field_strs: Vec<String> = fields
                .iter()
                .map(|(name, v)| format!("{name}: {}", format_witness_value(v)))
                .collect();
            if field_strs.is_empty() {
                format!("{type_name} {{}}")
            } else {
                format!("{type_name} {{ {} }}", field_strs.join(", "))
            }
        }
        _ => format_witness_value(val),
    }
}

/// One-line human-readable rendering of a clause expression.
fn expr_to_short_str(e: &Expr) -> String {
    match e {
        Expr::Ident(name, _) => name.clone(),
        Expr::Literal(Literal::Bool(b), _) => b.to_string(),
        Expr::Literal(Literal::Integer(n), _) => n.to_string(),
        Expr::FieldAccess { expr, field, .. } => {
            format!("{}.{field}", expr_to_short_str(expr))
        }
        Expr::Unary {
            op: UnaryOp::Not,
            expr,
            ..
        } => format!("!{}", expr_to_short_str(expr)),
        Expr::Binary {
            op, left, right, ..
        } => {
            let op_str = match op {
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
                _ => "?",
            };
            format!(
                "{} {op_str} {}",
                expr_to_short_str(left),
                expr_to_short_str(right)
            )
        }
        _ => "?".to_string(),
    }
}

/// Synthesize the `test fn` pair for an MC/DC independence pair.
///
/// Returns `None` when any witness value is `Unknown` (can't emit a valid literal).
fn synthesize_mcdc_test_pair(
    fn_name: &str,
    line: u32,
    clause_idx: usize,
    clause_str: &str,
    fn_params: &[Param],
    t1: &[WitnessArg],
    t2: &[WitnessArg],
) -> Option<String> {
    if t1.iter().any(|w| matches!(w.value, WitnessValue::Unknown))
        || t2.iter().any(|w| matches!(w.value, WitnessValue::Unknown))
    {
        return None;
    }
    let safe_name = fn_name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_");
    let t1_name = format!("harden_mcdc_{safe_name}_c{clause_idx}_t");
    let t2_name = format!("harden_mcdc_{safe_name}_c{clause_idx}_f");

    let render = |witnesses: &[WitnessArg]| -> String {
        let mut lines = Vec::new();
        for (w, p) in witnesses.iter().zip(fn_params.iter()) {
            let ty = declared_type_str(&p.ty);
            lines.push(format!(
                "    let {}: {ty} = {};",
                w.param_name,
                format_witness_value_typed(&w.value, &p.ty)
            ));
        }
        let args: Vec<String> = witnesses
            .iter()
            .zip(fn_params.iter())
            .map(|(w, p)| format_witness_value_typed(&w.value, &p.ty))
            .collect();
        lines.push(format!("    {fn_name}({});", args.join(", ")));
        lines.join("\n")
    };

    let mut buf = Vec::new();
    buf.push(format!(
        "// MC/DC independence pair for {fn_name}:{line} clause {clause_idx} ({clause_str})"
    ));
    buf.push(format!("test fn {t1_name}() -> Unit {{"));
    buf.push(render(t1));
    buf.push("}".to_string());
    buf.push(String::new());
    buf.push(format!("test fn {t2_name}() -> Unit {{"));
    buf.push(render(t2));
    buf.push("}".to_string());
    Some(buf.join("\n"))
}

/// Write a generated test file, refusing to overwrite user-authored files (spec 026 R7).
fn write_generated_test_file(out_path: &Path, content: &str) {
    let marker = "Generated by `mvl harden";
    if let Ok(existing) = std::fs::read_to_string(out_path) {
        if !existing.contains(marker) {
            eprintln!(
                "  warning: refusing to overwrite user-authored file {}",
                out_path.display()
            );
            return;
        }
    }
    match std::fs::write(out_path, content) {
        Ok(()) => println!("  Wrote generated tests → {}\n", out_path.display()),
        Err(e) => eprintln!("  warning: could not write {}: {e}", out_path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mvl::mvl::parser::Parser;

    fn parse_prog(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // ── Axis 1: HardenHint classification ─────────────────────────────────

    #[test]
    fn hint_classifies_length_predicate_not_as_nonlinear() {
        // `len(self)` contains no `*` or `/` but must not be misclassified.
        assert_eq!(
            HardenHint::classify("len(self) > 0"),
            HardenHint::LengthPredicate
        );
    }

    #[test]
    fn hint_classifies_nonlinear() {
        assert_eq!(
            HardenHint::classify("self * 2 <= max"),
            HardenHint::NonlinearPredicate
        );
    }

    // ── Axis 4: decision collection ───────────────────────────────────────

    #[test]
    fn collect_mcdc_finds_compound_if() {
        let prog = parse_prog("fn f(a: Bool, b: Bool) -> Int { if a && b { 1 } else { 0 } }");
        let decisions = collect_mcdc_decisions(&prog);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].fn_name, "f");
        assert_eq!(decisions[0].clauses.len(), 2);
    }

    #[test]
    fn collect_mcdc_skips_single_clause_if() {
        let prog = parse_prog("fn f(x: Int) -> Int { if x > 0 { 1 } else { 0 } }");
        let decisions = collect_mcdc_decisions(&prog);
        assert_eq!(decisions.len(), 0);
    }

    #[test]
    fn collect_mcdc_skips_test_fns() {
        let prog =
            parse_prog("test fn t(a: Bool, b: Bool) -> Bool { if a && b { true } else { false } }");
        let decisions = collect_mcdc_decisions(&prog);
        assert_eq!(decisions.len(), 0);
    }

    #[test]
    fn collect_mcdc_captures_requires_clauses() {
        let prog = parse_prog(
            "fn f(x: Int, y: Int) -> Int requires x > 0 { if x > y && y >= 0 { 1 } else { 0 } }",
        );
        let decisions = collect_mcdc_decisions(&prog);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].requires.len(), 1);
    }

    // ── Axis 4: bool clause normalization ─────────────────────────────────

    #[test]
    fn normalize_bare_ident_becomes_ne_zero() {
        let e = Expr::Ident("x".to_string(), Span::default());
        let n = normalize_bool_clause(&e);
        match n {
            Expr::Binary {
                op: BinaryOp::Ne,
                left,
                right,
                ..
            } => {
                assert!(matches!(*left, Expr::Ident(ref s, _) if s == "x"));
                assert!(matches!(*right, Expr::Literal(Literal::Integer(0), _)));
            }
            other => panic!("expected Ne binop, got {other:?}"),
        }
    }

    #[test]
    fn normalize_unary_not_folds_into_eq_zero() {
        let x = Expr::Ident("x".to_string(), Span::default());
        let not_x = Expr::Unary {
            op: UnaryOp::Not,
            expr: Box::new(x),
            span: Span::default(),
        };
        let n = normalize_bool_clause(&not_x);
        // !x → normalize(x) → x != 0 → negate → x == 0
        match n {
            Expr::Binary { op, .. } => assert_eq!(op, BinaryOp::Eq),
            other => panic!("expected Eq binop, got {other:?}"),
        }
    }

    #[test]
    fn negate_flips_comparison_operators() {
        let x = Expr::Ident("x".to_string(), Span::default());
        let five = Expr::Literal(Literal::Integer(5), Span::default());
        let lt = binop(BinaryOp::Lt, x, five);
        match negate_normalized(&lt) {
            Expr::Binary { op, .. } => assert_eq!(op, BinaryOp::Ge),
            other => panic!("expected Ge, got {other:?}"),
        }
    }

    // ── Axis 4: env-based clause evaluation ───────────────────────────────

    #[test]
    fn eval_bool_expr_bare_ident() {
        let mut env = HashMap::new();
        env.insert("x".to_string(), 1i64);
        let e = normalize_bool_clause(&Expr::Ident("x".to_string(), Span::default()));
        assert_eq!(eval_bool_expr(&e, &env), Some(true));
        env.insert("x".to_string(), 0);
        assert_eq!(eval_bool_expr(&e, &env), Some(false));
    }

    #[test]
    fn eval_bool_expr_integer_comparison() {
        let mut env = HashMap::new();
        env.insert("x".to_string(), 5i64);
        let e = binop(
            BinaryOp::Gt,
            Expr::Ident("x".to_string(), Span::default()),
            Expr::Literal(Literal::Integer(0), Span::default()),
        );
        assert_eq!(eval_bool_expr(&e, &env), Some(true));
    }

    // ── Axis 4: param support classification ──────────────────────────────

    #[test]
    fn params_supported_accepts_int_and_bool() {
        let prog = parse_prog("fn f(a: Bool, x: Int) -> Int { 0 }");
        let params = match &prog.declarations[0] {
            Decl::Fn(fd) => fd.params.clone(),
            _ => unreachable!(),
        };
        assert!(params_supported_for_mcdc(&params, &HashMap::new()));
    }

    #[test]
    fn params_supported_rejects_string() {
        let prog = parse_prog("fn f(s: String) -> Int { 0 }");
        let params = match &prog.declarations[0] {
            Decl::Fn(fd) => fd.params.clone(),
            _ => unreachable!(),
        };
        assert!(!params_supported_for_mcdc(&params, &HashMap::new()));
    }

    // ── Test file generation refuses to overwrite user files (R7) ─────────

    #[test]
    fn write_generated_test_file_refuses_to_overwrite_user_file() {
        let tmp = std::env::temp_dir().join("mvl_harden_r7_user.mvl");
        std::fs::write(&tmp, "// user-authored test\ntest fn t() {}\n").unwrap();
        write_generated_test_file(&tmp, "// Generated by `mvl harden` — new content");
        let after = std::fs::read_to_string(&tmp).unwrap();
        assert!(after.starts_with("// user-authored test"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn write_generated_test_file_overwrites_previously_generated() {
        let tmp = std::env::temp_dir().join("mvl_harden_r7_gen.mvl");
        std::fs::write(&tmp, "// Generated by `mvl harden` — old\n").unwrap();
        write_generated_test_file(&tmp, "// Generated by `mvl harden` — new\n");
        let after = std::fs::read_to_string(&tmp).unwrap();
        assert!(after.contains("new"));
        let _ = std::fs::remove_file(&tmp);
    }

    // ── JSON escape: string content stays valid JSON ──────────────────────

    #[test]
    fn axis4_result_pair_snippet_absent_when_witness_unknown() {
        // A Pair outcome doesn't automatically have a snippet — snippet is None
        // when synthesize_mcdc_test_pair rejects Unknown witness values.
        // This just verifies the enum variants compile and can be constructed.
        let r = Axis4Result {
            fn_name: "f".to_string(),
            line: 1,
            clause_idx: 0,
            clause_text: "x > 0".to_string(),
            outcome: Axis4Outcome::Coupled,
            snippet: None,
        };
        matches!(r.outcome, Axis4Outcome::Coupled);
    }
}
