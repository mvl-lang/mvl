// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::backends::rust::{
    emit_mcdc_preamble, emit_mcdc_report_test, MCDCDecision, TranspileConfig,
};
use mvl::mvl::pipeline::lower_prelude;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Decl;
use mvl::mvl::passes::mcdc::analysis::{analyze_mcdc, DecisionInfo};
use mvl::mvl::stdlib;
use std::collections::HashSet;
use std::fs;
use std::process;

fn validate_module_name(name: &str, source_path: &str) {
    let valid = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        eprintln!(
            "error: file '{source_path}' produces invalid module name '{name}'; \
             rename the file to use only lowercase ASCII letters, digits, and hyphens"
        );
        process::exit(1);
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Run full 11-requirement verification on all pure-MVL stdlib files.
///
/// Each file is checked with the other proven-stdlib files as its prelude so
/// that cross-module references (e.g. lists.mvl calling list_len from core)
/// resolve correctly.
///
/// Returns `(stdlib_file_name, errors)` for any file that has errors.
pub fn run(path: &str, quiet: bool, verbose: bool, masking: bool, json: bool) {
    let test_files = loader::mvl_files(path, true);
    if test_files.is_empty() {
        eprintln!("No *_test.mvl files found at: {path}");
        process::exit(1);
    }

    let crate_name = "mvl_mcdc";
    // Use a randomly-named temp dir (avoids PID-based TOCTOU attacks on shared machines).
    let tmp_dir_guard = tempfile::tempdir().unwrap_or_else(|e| {
        eprintln!("Cannot create temp dir: {e}");
        process::exit(1);
    });
    let tmp_dir = tmp_dir_guard.path().to_path_buf();
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap_or_else(|e| {
        eprintln!("Cannot create temp src dir {}: {e}", src_dir.display());
        process::exit(1);
    });

    let stdlib_prelude_progs = loader::load_implicit_prelude();

    // The implicit prelude always has `pub builtin fn` declarations (strings.mvl,
    // lists.mvl), so mvl_runtime is always required for MC/DC instrumented builds.
    let need_mvl_runtime = transpiler::prelude_requires_runtime(&stdlib_prelude_progs);

    // Build a fn_name → prelude_stem map and preload prelude source lines so
    // that JSON source-fragment lookup works for decisions in stdlib functions.
    // IMPLICIT_STEMS must mirror loader::load_implicit_prelude().
    const IMPLICIT_STEMS: &[&str] = &["core", "strings", "lists"];
    const IMPLICIT_FILES: &[&str] = &["core.mvl", "strings.mvl", "lists.mvl"];
    let mut prelude_fn_to_stem: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut prelude_sources: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for ((stem, file), prog) in IMPLICIT_STEMS
        .iter()
        .zip(IMPLICIT_FILES.iter())
        .zip(stdlib_prelude_progs.iter())
    {
        if let Some(content) = stdlib::stdlib_content(file) {
            prelude_sources.insert(
                stem.to_string(),
                content.lines().map(String::from).collect::<Vec<String>>(),
            );
        }
        for d in &prog.declarations {
            if let Decl::Fn(fd) = d {
                prelude_fn_to_stem
                    .entry(fd.name.clone())
                    .or_insert_with(|| stem.to_string());
            }
        }
    }

    // Transpile all test files with MC/DC instrumentation.
    let mut modules: Vec<(String, String, String)> = Vec::new();
    let mut all_decisions: Vec<MCDCDecision> = Vec::new();
    // Map (module, fn_name) → fn start line in the *_test.mvl file.
    // Used to compute source offsets for non-Return decision line patching.
    let mut test_fn_starts: std::collections::HashMap<(String, String), u32> =
        std::collections::HashMap::new();
    let mut all_static_decisions: Vec<DecisionInfo> = Vec::new();
    let mut file_stems: Vec<String> = Vec::new();
    // Map module_name → source lines for JSON source fragment lookup.
    let mut module_sources: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let (prog, src) = super::parse_or_exit(&file_str);
        let s = loader::stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        module_sources.insert(module_name.clone(), src.lines().map(String::from).collect());
        // Record non-test function start lines (needed for line-offset patching below).
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if !fd.is_test {
                    test_fn_starts
                        .entry((module_name.clone(), fd.name.clone()))
                        .or_insert(fd.span.line);
                }
            }
        }
        validate_module_name(&module_name, &file_str);
        let start_id = all_decisions.len();
        let static_d = analyze_mcdc(&prog, &module_name, start_id);
        all_static_decisions.extend(static_d);
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let result = transpiler::transpile(
            &tir,
            TranspileConfig::new(&module_name)
                .with_file_stem(&module_name)
                .with_prelude(lower_prelude(&stdlib_prelude_progs))
                .with_mcdc(start_id),
        );
        let out = result.output;
        let decisions = result.decisions;
        all_decisions.extend(decisions);
        file_stems.push(module_name.clone());
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        modules.push((module_name, file_str, module_content));
    }

    // Include source files that contain inline test fns.
    let covered_stems: HashSet<String> = file_stems.iter().cloned().collect();
    let source_files = loader::mvl_files(path, false);
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = loader::stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue;
        }
        let (prog, src) = super::parse_or_exit(&file_str);
        let has_tests = prog
            .declarations
            .iter()
            .any(|d| matches!(d, Decl::Fn(fd) if fd.is_test));
        if !has_tests {
            continue;
        }
        module_sources.insert(module_name.clone(), src.lines().map(String::from).collect());
        validate_module_name(&module_name, &file_str);
        let start_id = all_decisions.len();
        let static_d = analyze_mcdc(&prog, &module_name, start_id);
        all_static_decisions.extend(static_d);
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let result = transpiler::transpile(
            &tir,
            TranspileConfig::new(&module_name)
                .with_file_stem(&module_name)
                .with_prelude(lower_prelude(&stdlib_prelude_progs))
                .with_mcdc(start_id)
                .for_test_crate(),
        );
        let out = result.output;
        let decisions = result.decisions;
        all_decisions.extend(decisions);
        file_stems.push(module_name.clone());
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        modules.push((module_name, file_str, module_content));
    }

    // Fix line numbers for Return decisions: *_test.mvl re-declares functions
    // (workaround for #96) at different line numbers than the original source.
    // Build a (module, fn_name) → line map from source files and override
    // the line for any Return decision whose function exists in the source.
    {
        use mvl::mvl::passes::mcdc::transform::DecisionKind;
        use std::collections::HashMap;
        let mut source_fn_lines: HashMap<(String, String), u32> = HashMap::new();
        for src_file in &source_files {
            let file_str = src_file.display().to_string();
            let s = loader::stem(&file_str);
            let module_name = s.replace('-', "_");
            let (prog, _src) = super::parse_or_exit(&file_str);
            for decl in &prog.declarations {
                if let Decl::Fn(fd) = decl {
                    if !fd.is_test {
                        source_fn_lines
                            .insert((module_name.clone(), fd.name.clone()), fd.span.line);
                    }
                }
            }
        }
        for decision in &mut all_decisions {
            let key = (decision.file.clone(), decision.fn_name.clone());
            if let Some(&src_fn_line) = source_fn_lines.get(&key) {
                if matches!(decision.kind, DecisionKind::Return) {
                    decision.line = src_fn_line;
                } else if let Some(&test_fn_line) = test_fn_starts.get(&key) {
                    // Offset non-Return decisions by the function start difference
                    // between the source and the test-file redeclaration.
                    let offset = src_fn_line as i64 - test_fn_line as i64;
                    decision.line = (decision.line as i64 + offset).max(1) as u32;
                }
            }
        }
    }

    // Fix decision.file for prelude functions: they are emitted under the test
    // module's file stem but their line numbers reference the stdlib source.
    for decision in &mut all_decisions {
        if let Some(prelude_stem) = prelude_fn_to_stem.get(&decision.fn_name) {
            decision.file = prelude_stem.clone();
        }
    }
    // Add prelude sources to module_sources so the JSON source-fragment lookup
    // finds the correct line for stdlib decisions.
    module_sources.extend(prelude_sources);

    // Build exempt set: decisions inside functions with ! effects are shown in
    // the EXEMPT tier and excluded from the coverage percentage denominator.
    let exempt_ids: std::collections::HashSet<usize> = all_static_decisions
        .iter()
        .filter(|d| d.is_effectful)
        .map(|d| d.id)
        .collect();

    let total_decisions = all_decisions.len();

    if total_decisions == 0 {
        if json {
            println!(
                "{{\n  \"version\": \"1.0\",\n  \"mode\": \"{}\",\n  \"summary\": {{\n    \"files_analyzed\": {},\n    \"test_files\": {},\n    \"tests_run\": 0,\n    \"tests_passed\": 0,\n    \"tests_failed\": 0,\n    \"decisions\": 0,\n    \"obligations_total\": 0,\n    \"obligations_met\": 0,\n    \"obligations_missed\": 0,\n    \"obligations_coupled\": 0,\n    \"coverage_percent\": 100.00,\n    \"pass\": true\n  }},\n  \"decisions\": []\n}}",
                if masking { "masking" } else { "unique-cause" },
                modules.len(),
                test_files.len(),
            );
        } else {
            println!("No compound boolean conditions found — no MC/DC obligations.");
        }
        return;
    }

    if !quiet && !json {
        // all_decisions contains only compound decisions (clause_count > 1)
        let pure_obligations: usize = all_decisions
            .iter()
            .filter(|d| !exempt_ids.contains(&d.id))
            .map(|d| d.clause_count)
            .sum();
        let exempt_count = exempt_ids.len();
        let pure_count = total_decisions - exempt_count;
        if exempt_count > 0 {
            println!(
                "Found {} test file(s), {} compound decisions ({} pure, {} exempt), {} pure obligations",
                test_files.len(),
                total_decisions,
                pure_count,
                exempt_count,
                pure_obligations,
            );
        } else {
            println!(
                "Found {} test file(s), {} compound decisions, {} obligations",
                test_files.len(),
                total_decisions,
                pure_obligations,
            );
        }
    }

    // Build combined lib.rs.
    let mut combined_rs = String::new();
    combined_rs.push_str("// MVL MC/DC runner — generated by `mvl mcdc`\n");
    combined_rs.push_str(
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe, unpredictable_function_pointer_comparisons)]\n\n",
    );
    combined_rs.push_str(&emit_mcdc_preamble(total_decisions));
    for (module_name, label, module_content) in &modules {
        combined_rs.push_str(&format!("// === {label} ===\n"));
        combined_rs.push_str("#[cfg(test)]\n");
        combined_rs.push_str(&format!("mod {module_name} {{\n"));
        combined_rs.push_str("    #[allow(unused)]\n");
        combined_rs.push_str("    use super::*;\n");
        combined_rs.push_str(module_content);
        combined_rs.push_str("}\n\n");
    }
    combined_rs.push_str(&emit_mcdc_report_test(total_decisions));

    // Write Cargo.toml + lib.rs.
    let mvl_runtime_dep = if need_mvl_runtime {
        "mvl_runtime = { path = \"./mvl_runtime\", package = \"mvl_runtime_rust\" }\n"
    } else {
        ""
    };
    let cargo_toml = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{mvl_runtime_dep}"
    );
    fs::write(tmp_dir.join("Cargo.toml"), &cargo_toml).unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });
    if need_mvl_runtime {
        let runtime_src = mvl::mvl::runtime_xdg::ensure_runtime_rust();
        let runtime_dst = tmp_dir.join("mvl_runtime");
        super::copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
            eprintln!("error: cannot copy mvl_runtime: {e}");
            process::exit(1);
        });
    }
    fs::write(src_dir.join("lib.rs"), &combined_rs).unwrap_or_else(|e| {
        eprintln!("Cannot write lib.rs: {e}");
        process::exit(1);
    });

    // Resolve cargo binary: honour rustup's CARGO env var if set.
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    // Compile.
    let build_status = std::process::Command::new(&cargo_bin)
        .args(["build", "--tests", "--quiet"])
        .current_dir(&tmp_dir)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: cargo build failed: {e}");
            process::exit(1);
        });
    if !build_status.success() {
        eprintln!("error: MC/DC instrumented build failed");
        process::exit(1);
    }

    // Run tests with MVL_MCDC_OUT set.
    let mcdc_out_path = tmp_dir.join("mcdc_observations.txt");
    let test_output = std::process::Command::new(&cargo_bin)
        .args(["test", "--lib", "--quiet"])
        .env("MVL_MCDC_OUT", &mcdc_out_path)
        .current_dir(&tmp_dir)
        .output()
        .unwrap_or_else(|e| {
            eprintln!("error: cargo test failed: {e}");
            process::exit(1);
        });

    let test_stdout = String::from_utf8_lossy(&test_output.stdout).into_owned();

    // Filter out the internal report test from stdout.
    if !json {
        for line in test_stdout.lines() {
            if !line.contains("zzz_mvl_mcdc_report") {
                println!("{line}");
            }
        }
    }

    // Parse observations.
    let raw_obs = fs::read_to_string(&mcdc_out_path).unwrap_or_default();
    let observations: Vec<Vec<u32>> = raw_obs
        .lines()
        .map(|line| {
            if line.is_empty() {
                Vec::new()
            } else {
                line.split(',')
                    .filter_map(|hex| u32::from_str_radix(hex.trim(), 16).ok())
                    .collect()
            }
        })
        .collect();

    // Independence analysis.
    use mvl::mvl::passes::mcdc::transform::{
        is_clause_covered, is_match_arm_covered, DecisionKind as TransformKind,
    };
    let mut covered = 0usize;
    let mut total_obligations = 0usize;

    // Collect per-decision results.
    // coupled_missed: number of obligations that are uncovered AND in a coupled pair.
    let mut decision_results: Vec<(usize, Vec<bool>)> = Vec::new();
    let mut coupled_missed = 0usize;

    for decision in &all_decisions {
        let is_exempt = exempt_ids.contains(&decision.id);
        let obs = observations
            .get(decision.id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let mut clause_results = Vec::new();
        if matches!(decision.kind, TransformKind::Match) {
            // Match arm coverage: each arm must be taken at least once.
            // Observations are arm indices (plain u32), not the 2N+1 bit encoding.
            for arm_idx in 0..decision.clause_count {
                let ok = is_match_arm_covered(arm_idx, obs);
                clause_results.push(ok);
                if !is_exempt {
                    total_obligations += 1;
                    if ok {
                        covered += 1;
                    }
                }
                // Match arms are never "coupled" in the boolean-condition sense.
            }
        } else {
            for clause_bit in 0..decision.clause_count {
                let ok = is_clause_covered(decision.clause_count, clause_bit, obs);
                clause_results.push(ok);
                if !is_exempt {
                    total_obligations += 1;
                    if ok {
                        covered += 1;
                    } else {
                        // Count as coupled-missed if this clause appears in any coupled pair.
                        let is_coupled = decision
                            .coupled_pairs
                            .iter()
                            .any(|(i, j, _)| *i == clause_bit || *j == clause_bit);
                        if is_coupled {
                            coupled_missed += 1;
                        }
                    }
                }
            }
        }
        decision_results.push((decision.id, clause_results));
    }

    // In masking mode, exempt coupled-missed obligations from the failure count.
    let effective_missed = (total_obligations - covered) - if masking { coupled_missed } else { 0 };

    // Report.
    let exempt_obligation_count: usize = all_decisions
        .iter()
        .filter(|d| exempt_ids.contains(&d.id))
        .map(|d| d.clause_count)
        .sum();
    if !quiet && !json {
        let pct = (covered * 100)
            .checked_div(total_obligations)
            .unwrap_or(100);
        println!("\nMC/DC coverage: {covered}/{total_obligations} pure obligations met ({pct}%)");
        if exempt_obligation_count > 0 {
            let exempt_decision_count = exempt_ids.len();
            println!(
                "  {exempt_obligation_count} obligation(s) in {exempt_decision_count} effectful decision(s) exempt (! effects — integration coverage only)"
            );
        }
        if coupled_missed > 0 {
            if masking {
                println!("  Coupled (structurally exempt under masking MC/DC): {coupled_missed}");
            } else {
                println!("  Coupled (unique-cause independence impossible): {coupled_missed}");
                println!("  Use --masking to apply DO-178C masking MC/DC rules");
            }
        }
    }

    if verbose && !json {
        println!("\nDETAILED RESULTS");
        println!("{}", "─".repeat(60));
        // Pure decisions (not in effectful functions).
        for (decision, (_, clause_results)) in all_decisions.iter().zip(decision_results.iter()) {
            if exempt_ids.contains(&decision.id) {
                continue;
            }
            let kind_label = decision.kind.label();
            let status: Vec<&str> = clause_results
                .iter()
                .map(|ok| if *ok { "✓" } else { "✗" })
                .collect();
            let all_ok = clause_results.iter().all(|ok| *ok);
            let unit = if matches!(decision.kind, TransformKind::Match) {
                "arms"
            } else {
                "clauses"
            };
            println!(
                "  {}:{:<4} {} ({} {unit}) [{}] {}",
                decision.file,
                decision.line,
                kind_label,
                decision.clause_count,
                status.join(" "),
                if all_ok { "COVERED" } else { "MISSED" }
            );
            // Show coupling info for any missed clause that is part of a coupled pair.
            for (clause_bit, ok) in clause_results.iter().enumerate() {
                if *ok {
                    continue;
                }
                for (ci, cj, shared) in &decision.coupled_pairs {
                    if *ci == clause_bit || *cj == clause_bit {
                        let other = if *ci == clause_bit { *cj } else { *ci };
                        println!(
                            "    └─ clause {} COUPLED with clause {} via: {}",
                            clause_bit,
                            other,
                            shared.join(", ")
                        );
                        println!("       unique-cause independence may be structurally impossible");
                        if masking {
                            println!("       masking MC/DC: exempt (--masking)");
                        }
                    }
                }
            }
        }
        // Exempt decisions (inside effectful functions).
        if !exempt_ids.is_empty() {
            println!("\nEXEMPT (! effects — integration coverage only)");
            for decision in all_decisions.iter().filter(|d| exempt_ids.contains(&d.id)) {
                let kind_label = decision.kind.label();
                let unit = if matches!(decision.kind, TransformKind::Match) {
                    "arms"
                } else {
                    "clauses"
                };
                let dashes = vec!["—"; decision.clause_count].join(" ");
                println!(
                    "  {}:{:<4} {} ({} {unit}) [{}] IO-BOUNDARY",
                    decision.file, decision.line, kind_label, decision.clause_count, dashes,
                );
            }
        }
        println!("{}", "─".repeat(60));
    }

    let all_covered = effective_missed == 0;
    if !quiet && !json {
        if all_covered {
            println!("PASS");
        } else {
            println!("FAIL");
        }
    }

    if json {
        // Parse test counts from cargo test output.
        let (tests_run, tests_passed, tests_failed) = {
            let mut passed = 0usize;
            let mut failed = 0usize;
            for line in test_stdout.lines() {
                if let Some(rest) = line.strip_prefix("test result:") {
                    if let Some(p) = rest
                        .split("passed")
                        .next()
                        .and_then(|s| s.trim().rsplit(' ').next())
                        .and_then(|s| s.parse::<usize>().ok())
                    {
                        passed = p;
                    }
                    if let Some(f) = rest
                        .split("failed")
                        .next()
                        .and_then(|s| s.trim().rsplit(' ').next())
                        .and_then(|s| s.parse::<usize>().ok())
                    {
                        failed = f;
                    }
                }
            }
            (passed + failed, passed, failed)
        };

        let mode_str = if masking { "masking" } else { "unique-cause" };
        let pct = if total_obligations == 0 {
            100.0f64
        } else {
            (covered as f64 / total_obligations as f64) * 100.0
        };

        let mut out = String::new();
        out.push_str("{\n");
        out.push_str("  \"version\": \"1.0\",\n");
        out.push_str(&format!("  \"mode\": \"{mode_str}\",\n"));
        out.push_str("  \"summary\": {\n");
        out.push_str(&format!("    \"files_analyzed\": {},\n", modules.len()));
        out.push_str(&format!("    \"test_files\": {},\n", test_files.len()));
        out.push_str(&format!("    \"tests_run\": {tests_run},\n"));
        out.push_str(&format!("    \"tests_passed\": {tests_passed},\n"));
        out.push_str(&format!("    \"tests_failed\": {tests_failed},\n"));
        out.push_str(&format!("    \"decisions\": {},\n", all_decisions.len()));
        out.push_str(&format!(
            "    \"obligations_total\": {total_obligations},\n"
        ));
        out.push_str(&format!("    \"obligations_met\": {covered},\n"));
        out.push_str(&format!(
            "    \"obligations_missed\": {},\n",
            total_obligations - covered
        ));
        out.push_str(&format!("    \"obligations_coupled\": {coupled_missed},\n"));
        out.push_str(&format!("    \"coverage_percent\": {pct:.2},\n"));
        out.push_str(&format!("    \"pass\": {all_covered}\n"));
        out.push_str("  }");

        if !quiet {
            out.push_str(",\n  \"decisions\": [");
            let mut first_d = true;
            for (decision, (_, clause_results)) in all_decisions.iter().zip(decision_results.iter())
            {
                if !first_d {
                    out.push(',');
                }
                first_d = false;
                let kind_label = decision.kind.label();
                let d_met: usize = clause_results.iter().filter(|&&ok| ok).count();
                let d_total = decision.clause_count;
                let d_covered = d_met == d_total;
                let source_frag = module_sources
                    .get(&decision.file)
                    .and_then(|lines| lines.get(decision.line as usize - 1))
                    .map(|l| l.trim().to_string())
                    .unwrap_or_default();
                out.push_str("\n    {\n");
                out.push_str(&format!(
                    "      \"file\": \"{}\",\n",
                    super::json_escape(&decision.file)
                ));
                out.push_str(&format!("      \"line\": {},\n", decision.line));
                out.push_str(&format!("      \"kind\": \"{kind_label}\",\n"));
                out.push_str(&format!(
                    "      \"source\": \"{}\",\n",
                    super::json_escape(&source_frag)
                ));
                out.push_str(&format!("      \"clauses\": {d_total},\n"));
                out.push_str(&format!("      \"obligations_met\": {d_met},\n"));
                out.push_str(&format!("      \"obligations_total\": {d_total},\n"));
                out.push_str(&format!("      \"covered\": {d_covered},\n"));
                out.push_str("      \"clauses_detail\": [");
                let mut first_c = true;
                for (clause_bit, ok) in clause_results.iter().enumerate() {
                    if !first_c {
                        out.push(',');
                    }
                    first_c = false;
                    let coupled_with = if !ok {
                        decision
                            .coupled_pairs
                            .iter()
                            .find(|(ci, cj, _)| *ci == clause_bit || *cj == clause_bit)
                            .map(|(ci, cj, shared)| {
                                let other = if *ci == clause_bit { *cj } else { *ci };
                                let dep = super::json_escape(&shared.join(", "));
                                format!(
                                    "{{ \"clause_index\": {other}, \"shared_dependency\": \"{dep}\" }}"
                                )
                            })
                    } else {
                        None
                    };
                    let coupled_json = coupled_with.as_deref().unwrap_or("null");
                    out.push_str(&format!(
                        "\n        {{ \"index\": {clause_bit}, \"covered\": {ok}, \"independence_pair\": null, \"coupled_with\": {coupled_json} }}"
                    ));
                }
                out.push_str("\n      ]\n    }");
            }
            out.push_str("\n  ]");
        }

        out.push_str("\n}\n");
        print!("{out}");
    }

    if !all_covered {
        process::exit(1);
    }
}
