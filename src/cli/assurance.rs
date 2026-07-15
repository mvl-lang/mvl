// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::checker;
use mvl::mvl::checker::ifc;
use mvl::mvl::checker::passes::{
    aggregate_verdicts, count_handling_sites, count_memory_safety_sites, source_hash,
    HandlingCounts, PassRegistry, Verdict, VerdictCache,
};
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Decl, Program, Totality, TypeBody};
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use std::path::Path;
use std::process;

pub fn run(path: &str, json: bool, verbose: bool) {
    let stdlib_dir = stdlib::ensure_stdlib();
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let all_mvl_count = loader::mvl_files_all(path).len();
    let excluded_count = all_mvl_count - files.len();

    // Run the module resolver to surface `use` errors before reporting.
    let base_dir: std::path::PathBuf = if std::path::Path::new(path).is_dir() {
        std::path::Path::new(path).to_path_buf()
    } else {
        loader::infer_base_dir_from_qualified_imports(std::path::Path::new(path))
    };
    let modules: Vec<(String, String, Program)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, _) = super::parse_or_exit(&file_str);
            let qname = loader::qualified_stem(&base_dir, f.as_path());
            (qname, file_str.clone(), prog)
        })
        .collect();
    let resolve_result = resolver::resolve_project(modules, Some(&stdlib_dir));
    for err in &resolve_result.errors {
        eprintln!("error[resolver]: {err}");
    }

    let mut total_fns: usize = 0;
    let mut total_verified: usize = 0; // `total fn` (MVL-defined)
    let mut total_explicit: usize = 0; // `total fn` with explicit `total` keyword
    let mut total_partial: usize = 0; // `partial fn` (MVL-defined)
    let mut _total_pub: usize = 0; // `pub fn` — always 0 until module resolver (#96) is merged
    let mut total_extern: usize = 0; // extern fn signatures (trust boundaries)
    let mut total_test_fns: usize = 0; // `test fn` — internal unit tests
    let mut check_errors: usize = 0;
    let mut file_count = 0;
    // Aggregate per-requirement error counts (index 1-11).
    let mut req_errors = [0usize; 12];
    // Aggregate per-file stats.
    let mut total_struct_types: usize = 0;
    let mut total_enum_types: usize = 0;
    let mut total_effects_fns: usize = 0;
    let mut all_fn_details: Vec<FnDetail> = Vec::new();
    // Verification activity counters (wired from existing counting functions).
    let mut total_let_bindings: usize = 0;
    let mut total_ref_bindings: usize = 0;
    let mut total_consume_sites: usize = 0;
    let mut handling = HandlingCounts::default();
    let mut total_relabel_ops: usize = 0;
    let mut total_audit_relabels: usize = 0;
    let mut total_labeled_params: usize = 0;
    let mut total_flow_checks: usize = 0;
    let mut total_refined_fields: usize = 0;
    let mut total_struct_fields: usize = 0;
    let mut total_struct_invariants: usize = 0;
    // Refinement proof layer breakdown (aggregated across files).
    let mut agg_ref_proven: usize = 0;
    let mut agg_ref_runtime: usize = 0;
    let mut agg_ref_by_layer: [usize; 6] = [0; 6];
    let mut all_proof_entries: Vec<mvl::mvl::checker::refinements::ProofEntry> = Vec::new();
    // Verification pass infrastructure.
    let registry = PassRegistry::default_registry();
    let mut verdict_cache = VerdictCache::default();
    let mut per_file_verdicts: Vec<[Verdict; 12]> = Vec::new();

    let mut parsed_assurance: Vec<(String, Program, String)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, src) = super::parse_or_exit(&file_str);
            (file_str, prog, src)
        })
        .collect();
    // Number of files explicitly requested by the user; auto-loaded siblings
    // appended below are excluded from per-file iteration and the report.
    let requested_count = parsed_assurance.len();

    // When the user passes a single file, auto-load any imported sibling modules
    // so cross-module type and function references resolve (mirrors check.rs).
    // Without this, `use types::{Order, ...}` from a sibling file would surface
    // as unresolved-name errors even though the project as a whole type-checks.
    if Path::new(path).is_file() {
        let already_loaded: std::collections::HashSet<String> = parsed_assurance
            .iter()
            .map(|(f, _, _)| loader::stem(f))
            .collect();
        let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        if let Some((_, entry_prog, _)) = parsed_assurance.first() {
            // Transitive sibling load — see build.rs / check.rs for rationale.
            let siblings = loader::load_sibling_modules_transitive(entry_prog, entry_dir);
            for (mod_name, mod_str, sib_prog) in siblings {
                if already_loaded.contains(&mod_name) {
                    continue;
                }
                let sib_src = std::fs::read_to_string(&mod_str).unwrap_or_default();
                parsed_assurance.push((mod_str, sib_prog, sib_src));
            }
        }
    }
    // Implicit prelude first (core.mvl, strings.mvl, lists.mvl, effects.mvl):
    // these are always visible without an explicit `use std.…` and must be loaded
    // before any user-imported stdlib modules so the checker can resolve built-in
    // effects (Console, Log, …) and primitive operations.
    let mut assurance_prelude = loader::load_implicit_prelude();
    assurance_prelude.extend(loader::load_stdlib_prelude(
        parsed_assurance.iter().map(|(_, p, _)| p),
        &stdlib_dir,
    ));
    let all_assurance_progs: Vec<Program> =
        parsed_assurance.iter().map(|(_, p, _)| p.clone()).collect();
    // Load any `pkg.*` package modules referenced by the checked files so the
    // checker can resolve their types and functions (mirrors check.rs behaviour).
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project_root = super::find_project_root(&cwd);
    assurance_prelude.extend(loader::load_pkg_modules(
        &all_assurance_progs,
        &project_root,
        &mut std::collections::HashSet::new(),
    ));

    // Count kernel builtins from the implicit stdlib prelude (strings.mvl, lists.mvl).
    // These are always part of the trust boundary for any MVL program, even though they
    // are not declared in user code. ADR-0006: trust boundaries must be declared and
    // countable — this surfaces the kernel builtin count in every assurance report.
    let kernel_extern_count: usize = loader::load_implicit_prelude()
        .iter()
        .flat_map(|p| p.declarations.iter())
        .filter(|d| {
            matches!(d,
                mvl::mvl::parser::ast::Decl::Fn(fd) if fd.is_builtin
            )
        })
        .count();

    for (idx, (file_str, prog, src)) in parsed_assurance.iter().take(requested_count).enumerate() {
        let file_str = file_str.as_str();
        let stats = collect_assurance_stats(prog, verbose);
        let (before, after_with_self) = all_assurance_progs.split_at(idx);
        let after = &after_with_self[1..];
        let user_prelude: Vec<&Program> = before.iter().chain(after.iter()).collect();
        let result = checker::check_with_two_preludes(&assurance_prelude, &user_prelude, prog);

        total_fns += stats.fn_count;
        total_verified += stats.total_fn_count;
        total_explicit += stats.explicit_total_fn_count;
        total_partial += stats.partial_fn_count;
        _total_pub += stats.pub_fn_count;
        total_extern += stats.extern_fn_count; // fn signatures, not block count
        total_test_fns += stats.test_fn_count;
        check_errors += result.errors.len();
        total_struct_types += stats.struct_type_count;
        total_enum_types += stats.enum_type_count;
        total_effects_fns += stats.effects_fn_count;
        // Verification activity: wire existing counting functions.
        let mc = count_memory_safety_sites(prog);
        total_let_bindings += mc.let_bindings;
        total_ref_bindings += mc.ref_bindings;
        total_consume_sites += mc.consume_sites;
        let hc = count_handling_sites(prog);
        handling.option_types += hc.option_types;
        handling.result_types += hc.result_types;
        handling.some_patterns += hc.some_patterns;
        handling.none_patterns += hc.none_patterns;
        handling.ok_patterns += hc.ok_patterns;
        handling.err_patterns += hc.err_patterns;
        handling.propagate_sites += hc.propagate_sites;
        handling.assign_sites += hc.assign_sites;
        total_relabel_ops += ifc::count_relabels(prog);
        total_audit_relabels += ifc::count_audit_relabels(prog);
        total_labeled_params += ifc::count_labeled_params(prog);
        total_flow_checks += ifc::count_flow_check_sites(prog);
        // Refinement proof layer breakdown.
        agg_ref_proven += result.refinement_counts.proven;
        agg_ref_runtime += result.refinement_counts.runtime_checked;
        for (i, &count) in result.refinement_counts.by_layer.iter().enumerate() {
            agg_ref_by_layer[i] += count;
        }
        all_proof_entries.extend(result.refinement_counts.proof_log.iter().cloned().map(
            |mut e| {
                if e.file.is_empty() {
                    e.file = file_str.to_string();
                }
                e
            },
        ));
        // Count struct field refinements.
        for decl in &prog.declarations {
            if let Decl::Type(td) = decl {
                if let TypeBody::Struct {
                    fields, invariant, ..
                } = &td.body
                {
                    total_struct_fields += fields.len();
                    total_refined_fields +=
                        fields.iter().filter(|f| f.refinement.is_some()).count();
                    if invariant.is_some() {
                        total_struct_invariants += 1;
                    }
                }
            }
        }
        for (i, count) in result.req_errors.iter().enumerate().skip(1) {
            req_errors[i] += count;
        }
        if verbose {
            all_fn_details.extend(stats.fn_details);
        }

        // Run verification passes (with incremental cache).
        let hash = source_hash(src);
        let file_path = Path::new(file_str);
        let verdicts: [Verdict; 12] = if let Some(cached) = verdict_cache.get(file_path, hash) {
            cached.to_owned()
        } else {
            let v = registry.run_all(prog, &result);
            verdict_cache.insert(file_path.to_path_buf(), hash, v.clone());
            v
        };
        per_file_verdicts.push(verdicts);

        file_count += 1;
    }

    // Aggregate verdicts across all files.
    let project_verdicts = aggregate_verdicts(&per_file_verdicts);
    let proven_count = (1u8..=11)
        .filter(|&i| project_verdicts[i as usize].is_proven())
        .count();

    let implemented = total_fns.saturating_sub(total_extern);
    let verified_pct = if implemented > 0 {
        (total_verified as f64 / implemented as f64 * 100.0).round() as u32
    } else {
        0
    };
    let extern_pct = if total_fns > 0 {
        (total_extern as f64 / total_fns as f64 * 100.0).round() as u32
    } else {
        0
    };

    if json && verbose {
        eprintln!("warning: --verbose is ignored with --json; per-function detail is not included in JSON output");
    }

    if json {
        let assign_sites = handling.assign_sites;
        let option_types = handling.option_types;
        let result_types = handling.result_types;
        let some_patterns = handling.some_patterns;
        let none_patterns = handling.none_patterns;
        let ok_patterns = handling.ok_patterns;
        let err_patterns = handling.err_patterns;
        let propagate_sites = handling.propagate_sites;
        // NOTE(#96): "pub" is always 0 until the module resolver is merged.
        let req_json: String = (1..=11)
            .map(|i| format!("    \"{i}\": {}", req_errors[i]))
            .collect::<Vec<_>>()
            .join(",\n");
        let verdicts_json: String = (1u8..=11)
            .map(|i| {
                let v = &project_verdicts[i as usize];
                format!(
                    "    \"{i}\": {{ \"status\": \"{}\", \"detail\": \"{}\" }}",
                    v.label(),
                    super::json_escape(v.detail())
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        println!(
            r#"{{
  "files": {file_count},
  "functions": {{
    "total": {total_fns},
    "verified_total": {total_verified},
    "partial": {total_partial},
    "extern": {total_extern},
    "kernel_extern": {kernel_extern_count},
    "implemented": {implemented},
    "test": {total_test_fns}
  }},
  "types": {{
    "structs": {total_struct_types},
    "enums": {total_enum_types}
  }},
  "percentages": {{
    "verified_pct": {verified_pct},
    "extern_pct": {extern_pct}
  }},
  "verification_activity": {{
    "let_bindings": {total_let_bindings},
    "ref_bindings": {total_ref_bindings},
    "consume_sites": {total_consume_sites},
    "assign_sites": {assign_sites},
    "option_types": {option_types},
    "result_types": {result_types},
    "some_patterns": {some_patterns},
    "none_patterns": {none_patterns},
    "ok_patterns": {ok_patterns},
    "err_patterns": {err_patterns},
    "propagate_sites": {propagate_sites},
    "refinement_proven": {agg_ref_proven},
    "refinement_runtime": {agg_ref_runtime},
    "relabel_operations": {total_relabel_ops},
    "audit_relabels": {total_audit_relabels},
    "labeled_params": {total_labeled_params},
    "flow_checks": {total_flow_checks},
    "effect_annotations": {total_effects_fns},
    "struct_fields": {total_struct_fields},
    "refined_fields": {total_refined_fields},
    "struct_invariants": {total_struct_invariants}
  }},
  "requirements": {{
{req_json}
  }},
  "verdicts": {{
{verdicts_json}
  }},
  "proven": {proven_count},
  "check_errors": {check_errors}
}}"#
        );
    } else {
        println!("MVL Assurance Report");
        println!("====================");
        if excluded_count > 0 {
            println!("Files checked:       {file_count} source files  ({excluded_count} *_test.mvl excluded)");
        } else {
            println!("Files checked:       {file_count}");
        }
        println!("Functions:           {total_fns}");
        let total_implicit = total_verified.saturating_sub(total_explicit);
        println!("  total fn:          {total_verified} ({total_explicit} explicit, {total_implicit} implicit)");
        if total_implicit > 0 {
            println!(
                "  implicit total:    {total_implicit}  ⚠ consider adding explicit `total` keyword"
            );
        }
        println!("  partial fn:        {total_partial}");
        println!("  extern fn:         {total_extern} ({extern_pct}% trust boundary)");
        println!("  kernel builtins:   {kernel_extern_count} (stdlib strings.mvl + lists.mvl)");
        println!("  implemented:       {implemented}");
        println!("  test fn:           {total_test_fns}");
        println!(
            "Totality coverage:   {}/{} implemented fns are total ({} explicit, {} implicit)",
            total_verified, implemented, total_explicit, total_implicit,
        );
        println!();
        println!("Verification activity:");
        println!(
            "  let bindings checked: {:<8} ref bindings: {:<8} consume sites: {}",
            total_let_bindings, total_ref_bindings, total_consume_sites,
        );
        println!(
            "  refinement proofs:    {} proven, {} runtime-checked (by layer: L1={} L2={} L3={} L4={} L5={})",
            agg_ref_proven, agg_ref_runtime,
            agg_ref_by_layer[1], agg_ref_by_layer[2], agg_ref_by_layer[3],
            agg_ref_by_layer[4], agg_ref_by_layer[5],
        );
        let audit_note = if total_audit_relabels > 0 {
            format!(", {} audit-marked", total_audit_relabels)
        } else {
            String::new()
        };
        println!(
            "  relabel operations:   {:<8} flow checks: {}{}",
            total_relabel_ops, total_flow_checks, audit_note,
        );
        println!(
            "  effect annotations:   {} fns declare effects",
            total_effects_fns
        );
        if total_struct_fields > 0 {
            println!(
                "  struct fields:        {} total, {} refined, {} struct invariant(s)",
                total_struct_fields, total_refined_fields, total_struct_invariants,
            );
        }
        if verbose && !all_proof_entries.is_empty() {
            let layer_name = |l: usize| match l {
                1 => "L1 trivial",
                2 => "L2 interval",
                3 => "L3 symbolic",
                4 => "L4 Cooper",
                5 => "L5 Z3",
                _ => "runtime",
            };
            println!();
            println!("Refinement proof detail:");
            for entry in &all_proof_entries {
                let fname = if entry.file.is_empty() {
                    ""
                } else {
                    std::path::Path::new(&entry.file)
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or(&entry.file)
                };
                let loc = format!("{}:{}", fname, entry.line);
                println!(
                    "  {:<10} {:<16} {:<24} {}",
                    layer_name(entry.layer),
                    loc,
                    entry.callee,
                    entry.predicate,
                );
            }
        }
        println!();
        let violated_count = (1..=11usize).filter(|&i| req_errors[i] > 0).count();
        let not_proven_count = 11 - proven_count - violated_count;
        println!("Requirements verified:  {proven_count} proven, {not_proven_count} not proven, {violated_count} violated");
        print_req_row(
            1,
            "Type safety",
            &req_errors,
            project_verdicts[1].is_proven(),
            &format!(
                "{} types ({} struct, {} enum), {} errors",
                total_struct_types + total_enum_types,
                total_struct_types,
                total_enum_types,
                req_errors[1]
            ),
        );
        print_req_row(
            2,
            "Memory safety",
            &req_errors,
            project_verdicts[2].is_proven(),
            &if req_errors[2] == 0 {
                format!(
                    "{} let bindings, {} ref bindings, {} consume sites — no violations",
                    total_let_bindings, total_ref_bindings, total_consume_sites,
                )
            } else {
                format!("{} use-after-move", req_errors[2])
            },
        );
        print_req_row(
            3,
            "Totality",
            &req_errors,
            project_verdicts[3].is_proven(),
            &format!(
                "{} total fn, {} non-exhaustive match",
                total_verified, req_errors[3]
            ),
        );
        let option_matches = handling.some_patterns + handling.none_patterns;
        let result_matches = handling.ok_patterns + handling.err_patterns;
        let req4_detail = if req_errors[4] > 0 {
            format!("{} direct Option access", req_errors[4])
        } else {
            format!(
                "{} Option types, {} matches ({} Some + {} None), {} ? propagations, 0 direct access",
                handling.option_types,
                option_matches,
                handling.some_patterns,
                handling.none_patterns,
                handling.propagate_sites,
            )
        };
        print_req_row(
            4,
            "Null elimination",
            &req_errors,
            project_verdicts[4].is_proven(),
            &req4_detail,
        );
        let req5_detail = if req_errors[5] > 0 {
            format!("{} unhandled Result", req_errors[5])
        } else {
            format!(
                "{} Result types, {} matches ({} Ok + {} Err), {} ? propagations, 0 unhandled",
                handling.result_types,
                result_matches,
                handling.ok_patterns,
                handling.err_patterns,
                handling.propagate_sites,
            )
        };
        print_req_row(
            5,
            "Error visibility",
            &req_errors,
            project_verdicts[5].is_proven(),
            &req5_detail,
        );
        let immutable_bindings = total_let_bindings.saturating_sub(total_ref_bindings);
        let req6_detail = if req_errors[6] > 0 {
            format!("{} immutability violations", req_errors[6])
        } else {
            format!(
                "{} immutable + {} ref bindings, {} reassignments, 0 violations",
                immutable_bindings, total_ref_bindings, handling.assign_sites,
            )
        };
        print_req_row(
            6,
            "Ownership",
            &req_errors,
            project_verdicts[6].is_proven(),
            &req6_detail,
        );
        print_req_row(
            7,
            "Effects",
            &req_errors,
            project_verdicts[7].is_proven(),
            &format!(
                "{} fns declare effects, {} undeclared",
                total_effects_fns, req_errors[7]
            ),
        );
        print_req_row(
            8,
            "Termination",
            &req_errors,
            project_verdicts[8].is_proven(),
            &format!(
                "{} total fn, {} partial fn, {} violations",
                total_verified, total_partial, req_errors[8]
            ),
        );
        // Req 9–11: qualitative verdicts (counts live in Prover verdicts section).
        let req9_detail = if project_verdicts[9].is_proven() {
            // Extract "N/M" prefix from verdict detail "N/M fns race-free ..."
            let d = project_verdicts[9].detail();
            let ratio = d.split(" fns race-free").next().unwrap_or("all");
            format!("{ratio} fns race-free — isolation and sendability verified")
        } else if project_verdicts[9].is_failed() {
            format!("{} capability violation(s)", req_errors[9])
        } else {
            project_verdicts[9].detail().to_string()
        };
        // Req 9–11: "Requirements verified" shows numerical detail from the prover
        // (v.detail()); "Prover verdicts" shows the qualitative summary.
        // Qualitative summaries are computed here and used in the verdicts loop below.
        print_req_row(
            9,
            "Data race freedom",
            &req_errors,
            project_verdicts[9].is_proven(),
            project_verdicts[9].detail(),
        );
        let req9_verdict = req9_detail;
        let req10_verdict = if project_verdicts[10].is_proven() {
            let total = agg_ref_proven + agg_ref_runtime;
            if agg_ref_runtime == 0 {
                format!("{total} call site(s) — all statically proven")
            } else {
                format!(
                    "{total} call site(s) — {agg_ref_proven} proven, {agg_ref_runtime} runtime-checked"
                )
            }
        } else if project_verdicts[10].is_failed() {
            format!("{} refinement violation(s)", req_errors[10])
        } else if total_refined_fields > 0 {
            // #1863: don't hardcode "0 call sites proven" — the aggregated
            // counters may still show real proven / runtime-checked totals
            // even when the project verdict is Unchecked (e.g. some fns
            // deferred to runtime while others are fully proven).
            format!(
                "{total_refined_fields} struct field(s) refined; \
                 {agg_ref_proven} call site(s) proven, \
                 {agg_ref_runtime} runtime-checked"
            )
        } else {
            "no refined types used".to_string()
        };
        print_req_row(
            10,
            "Refinements",
            &req_errors,
            project_verdicts[10].is_proven(),
            project_verdicts[10].detail(),
        );
        let req11_verdict = if project_verdicts[11].is_proven() {
            if total_relabel_ops > 0 || total_labeled_params > 0 {
                format!(
                    "opaque labels enforced; {} relabel point(s) auditable",
                    total_relabel_ops,
                )
            } else {
                "no information flow violations".to_string()
            }
        } else if project_verdicts[11].is_failed() {
            format!("{} information flow violation(s)", req_errors[11])
        } else {
            "no security-labeled types — not exercised".to_string()
        };
        print_req_row(
            11,
            "IFC",
            &req_errors,
            project_verdicts[11].is_proven(),
            project_verdicts[11].detail(),
        );
        println!();
        println!("Prover verdicts:");
        for req in 1u8..=11 {
            let v = &project_verdicts[req as usize];
            let name = registry.pass_name(req).unwrap_or("unknown");
            let detail: &str = match req {
                9 => &req9_verdict,
                10 => &req10_verdict,
                11 => &req11_verdict,
                _ => v.detail(),
            };
            println!(
                "  Req {:>2}  {:<20} {}  {}",
                req,
                name,
                v.status_char(),
                detail
            );
        }
        println!();
        println!("  ✓ proven  ✗ failed  ~ unchecked (SMT prover active; some call sites deferred to runtime)");
        println!();
        println!("Type errors:         {check_errors}");
        if check_errors == 0 {
            println!("Status:              PASS");
        } else {
            println!("Status:              FAIL ({check_errors} errors)");
        }

        if verbose && !all_fn_details.is_empty() {
            println!();
            println!("Functions:");
            println!(
                "{:<30} {:<8} {:<8} {:<12} {:<12} Refinements",
                "Name", "Kind", "Totality", "Effects", "Caps"
            );
            println!("{}", "-".repeat(80));
            for fd in &all_fn_details {
                let kind = if fd.is_extern {
                    "extern"
                } else if fd.is_test {
                    "test"
                } else {
                    "fn"
                };
                let totality = match &fd.totality {
                    Some(Totality::Total) => "total",
                    Some(Totality::Partial) => "partial",
                    None => "total*",
                };
                let effects = if fd.effects.is_empty() {
                    "-".to_string()
                } else {
                    fd.effects.join(",")
                };
                let caps = if fd.capabilities.is_empty() {
                    "-".to_string()
                } else {
                    fd.capabilities.join(",")
                };
                let (rp, rq, re) = fd.refinement_counts;
                let refs = if rp + rq + re == 0 {
                    "-".to_string()
                } else {
                    format!("{}p/{}r/{}e", rp, rq, re)
                };
                println!(
                    "{:<30} {:<8} {:<8} {:<12} {:<12} {}",
                    fd.name, kind, totality, effects, caps, refs
                );
            }
            println!("  * implicit total (no explicit keyword)");
        }
    }

    if check_errors > 0 {
        process::exit(1);
    }
}

fn print_req_row(req: u8, name: &str, req_errors: &[usize; 12], proven: bool, detail: &str) {
    debug_assert!((1..=11).contains(&req), "req must be 1–11, got {req}");
    let status = if req_errors[req as usize] > 0 {
        "✗"
    } else if proven {
        "✓"
    } else {
        "–"
    };
    println!("  Req {:>2}  {:<20} {}  {}", req, name, status, detail);
}

// ── Assurance stats ───────────────────────────────────────────────────────

struct AssuranceStats {
    fn_count: usize,
    total_fn_count: usize,
    explicit_total_fn_count: usize,
    partial_fn_count: usize,
    // NOTE(#96): pub_fn_count populated once module resolver is merged; always 0 for now.
    pub_fn_count: usize,
    extern_fn_count: usize,
    test_fn_count: usize,
    /// Number of `struct` type declarations (Req 1 evidence).
    struct_type_count: usize,
    /// Number of `enum` type declarations (Req 1 evidence).
    enum_type_count: usize,
    /// Number of functions that declare at least one effect (Req 7 evidence).
    effects_fn_count: usize,
    /// Number of functions that have at least one parameter with a capability (Req 9 evidence).
    capabilities_fn_count: usize,
    /// Number of functions with at least one refinement predicate (Req 10 evidence).
    refinements_fn_count: usize,
    /// Per-function details for `--verbose` output.
    fn_details: Vec<FnDetail>,
}

/// Per-function information for the verbose assurance report.
struct FnDetail {
    name: String,
    totality: Option<Totality>,
    effects: Vec<String>,
    /// Capability names used by parameters (e.g., "iso", "val", "tag").
    capabilities: Vec<String>,
    /// Refinement counts: (refined_params, requires_clauses, ensures_clauses).
    refinement_counts: (usize, usize, usize),
    is_test: bool,
    is_extern: bool,
}

fn collect_assurance_stats(prog: &Program, collect_details: bool) -> AssuranceStats {
    let mut stats = AssuranceStats {
        fn_count: 0,
        total_fn_count: 0,
        explicit_total_fn_count: 0,
        partial_fn_count: 0,
        pub_fn_count: 0,
        extern_fn_count: 0,
        test_fn_count: 0,
        struct_type_count: 0,
        enum_type_count: 0,
        effects_fn_count: 0,
        capabilities_fn_count: 0,
        refinements_fn_count: 0,
        fn_details: Vec::new(),
    };
    collect_stats_from_decls(&prog.declarations, &mut stats, collect_details);
    stats
}

fn collect_stats_from_decls(decls: &[Decl], stats: &mut AssuranceStats, collect_details: bool) {
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                let has_caps = fd.params.iter().any(|p| p.capability.is_some());
                let has_refs = fd.return_refinement.is_some()
                    || fd.params.iter().any(|p| p.refinement.is_some())
                    || !fd.requires.is_empty()
                    || !fd.ensures.is_empty();
                if fd.is_test {
                    stats.test_fn_count += 1;
                } else {
                    stats.fn_count += 1;
                    match fd.totality {
                        Some(Totality::Total) => {
                            stats.total_fn_count += 1;
                            stats.explicit_total_fn_count += 1;
                        }
                        Some(Totality::Partial) => stats.partial_fn_count += 1,
                        None => stats.total_fn_count += 1, // implicitly total
                    }
                    if !fd.effects.is_empty() {
                        stats.effects_fn_count += 1;
                    }
                    if has_caps {
                        stats.capabilities_fn_count += 1;
                    }
                    if has_refs {
                        stats.refinements_fn_count += 1;
                    }
                }
                if collect_details {
                    let cap_names: Vec<String> = fd
                        .params
                        .iter()
                        .filter_map(|p| {
                            p.capability.as_ref().map(|c| match c {
                                mvl::mvl::parser::ast::Capability::Iso => "iso",
                                mvl::mvl::parser::ast::Capability::Val => "val",
                                mvl::mvl::parser::ast::Capability::Ref => "ref",
                                mvl::mvl::parser::ast::Capability::Tag => "tag",
                            })
                        })
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .map(String::from)
                        .collect();
                    let refined_params =
                        fd.params.iter().filter(|p| p.refinement.is_some()).count();
                    let requires_count = fd.requires.len();
                    let ensures_count = fd.ensures.len();
                    stats.fn_details.push(FnDetail {
                        name: fd.name.clone(),
                        totality: fd.totality.clone(),
                        effects: fd.effects.iter().map(|e| e.to_string()).collect(),
                        capabilities: cap_names,
                        refinement_counts: (refined_params, requires_count, ensures_count),
                        is_test: fd.is_test,
                        is_extern: false,
                    });
                }
            }
            Decl::Extern(ed) => {
                // Each signature inside an extern block is a trust-boundary function.
                // Extern fns are NOT counted toward total_fn_count — they are trust
                // boundaries, not verified code. Counting them inflates the
                // "verified %" denominator (was causing >100% bug).
                stats.extern_fn_count += ed.fns.len();
                stats.fn_count += ed.fns.len();
                for ef in &ed.fns {
                    if collect_details {
                        stats.fn_details.push(FnDetail {
                            name: ef.name.clone(),
                            totality: ef.totality.clone(),
                            effects: ef.effects.iter().map(|e| e.to_string()).collect(),
                            capabilities: vec![],
                            refinement_counts: (0, 0, 0),
                            is_test: false,
                            is_extern: true,
                        });
                    }
                }
            }
            Decl::Type(td) => match &td.body {
                TypeBody::Struct { .. } => stats.struct_type_count += 1,
                TypeBody::Enum(_) => stats.enum_type_count += 1,
                TypeBody::Alias(_) => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod assurance_tests {
    use super::*;
    use mvl::mvl::checker;
    use mvl::mvl::parser::Parser;

    fn parse_prog(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    /// Spec 004 Req 3: assurance report counts test fns separately from impl fns.
    #[test]
    fn test_fn_count_is_separate_from_fn_count() {
        let src = "fn add(a: Int, b: Int) -> Int { a + b }\ntest fn check_add() -> Unit { }\ntest fn check_zero() -> Unit { }";
        let prog = parse_prog(src);
        let stats = collect_assurance_stats(&prog, false);
        assert_eq!(stats.test_fn_count, 2, "expected 2 test fns");
        assert_eq!(stats.fn_count, 1, "test fns must not inflate fn_count");
    }

    #[test]
    fn no_test_fns_means_zero_count() {
        let src = "fn add(a: Int, b: Int) -> Int { a + b }";
        let prog = parse_prog(src);
        let stats = collect_assurance_stats(&prog, false);
        assert_eq!(stats.test_fn_count, 0);
        assert_eq!(stats.fn_count, 1);
    }

    #[test]
    fn struct_and_enum_types_counted() {
        let src = "type Point = struct { x: Int, y: Int }\ntype Color = enum { Red, Green, Blue }\nfn id(p: Point) -> Point { p }";
        let prog = parse_prog(src);
        let stats = collect_assurance_stats(&prog, false);
        assert_eq!(stats.struct_type_count, 1, "expected 1 struct");
        assert_eq!(stats.enum_type_count, 1, "expected 1 enum");
    }

    #[test]
    fn effects_fn_counted() {
        let src = "fn pure(x: Int) -> Int { x }\nfn effectful(x: Int) -> Int ! DB { x }";
        let prog = parse_prog(src);
        let stats = collect_assurance_stats(&prog, false);
        assert_eq!(stats.effects_fn_count, 1, "expected 1 fn with effects");
        assert_eq!(stats.fn_count, 2);
    }

    #[test]
    fn req_errors_populated_from_checker() {
        let src = "fn f() -> Int { let x: Int = 1; let _y: Int = consume(x); x }";
        let prog = parse_prog(src);
        let result = checker::check(&prog);
        assert!(
            result.req_errors[2] >= 1,
            "req 2 (memory safety) should have errors"
        );
        assert_eq!(
            result.req_errors[1], 0,
            "req 1 (type safety) should be clean"
        );
    }

    #[test]
    fn req_errors_zero_on_clean_program() {
        let src = "fn add(a: Int, b: Int) -> Int { a + b }";
        let prog = parse_prog(src);
        let result = checker::check(&prog);
        for i in 1..=11 {
            assert_eq!(result.req_errors[i], 0, "req {i} should have 0 errors");
        }
    }

    #[test]
    fn fn_details_populated() {
        let src = "fn effectful(x: Int) -> Int ! DB { x }\ntest fn check_it() -> Unit { }";
        let prog = parse_prog(src);
        let stats = collect_assurance_stats(&prog, true);
        assert_eq!(stats.fn_details.len(), 2);
        let eff = stats
            .fn_details
            .iter()
            .find(|d| d.name == "effectful")
            .unwrap();
        assert_eq!(eff.effects, vec!["DB".to_string()]);
        assert!(!eff.is_test);
        let test = stats
            .fn_details
            .iter()
            .find(|d| d.name == "check_it")
            .unwrap();
        assert!(test.is_test);
    }
}
