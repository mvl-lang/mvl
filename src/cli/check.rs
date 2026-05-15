// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::checker;
use mvl::mvl::checker::passes::PassRegistry;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::parser::Parser;
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use std::path::Path;
use std::process;

fn check_proven_stdlib() -> Vec<(String, mvl::mvl::checker::CheckResult)> {
    // Parse all proven-stdlib files up front.
    let programs: Vec<(String, mvl::mvl::parser::ast::Program)> = super::PROVEN_STDLIB_FILES
        .iter()
        .filter_map(|name| {
            stdlib::stdlib_content(name).map(|src| {
                let (mut p, _) = Parser::new(src);
                (name.to_string(), p.parse_program())
            })
        })
        .collect();

    let mut results = Vec::new();
    for (i, (name, prog)) in programs.iter().enumerate() {
        // Build a prelude from all OTHER proven-stdlib programs.
        let prelude: Vec<&mvl::mvl::parser::ast::Program> = programs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, (_, p))| p)
            .collect();
        let prelude_owned: Vec<mvl::mvl::parser::ast::Program> =
            prelude.iter().map(|p| (*p).clone()).collect();

        let result = checker::check_with_prelude(&prelude_owned, prog);
        if result.has_errors() {
            results.push((name.clone(), result));
        }
    }
    results
}

/// Verify pure-MVL stdlib files when `profile == "proven"`.
/// Prints errors and exits with code 1 if any failures are found.  No-op for "trusted".
pub fn maybe_check_proven_stdlib_or_exit(profile: &str) {
    if profile != "proven" {
        return;
    }
    let stdlib_errors = check_proven_stdlib();
    if stdlib_errors.is_empty() {
        return;
    }
    eprintln!(
        "note: --stdlib=proven: {} stdlib file(s) have verification errors:",
        stdlib_errors.len()
    );
    for (name, result) in &stdlib_errors {
        for err in &result.errors {
            eprintln!(
                "std/{name}:{}:{}: error[req{}]: {}",
                err.span().line,
                err.span().col,
                err.requirement_number(),
                err.message()
            );
        }
    }
    process::exit(1);
}

/// Parse and type-check a .mvl file or all .mvl files in a directory.
///
/// When `req_filter` is `Some(N)`, only the verification pass for Req N is run
/// and its verdict is printed; errors for other requirements are suppressed.
pub fn run(
    path: &str,
    req_filter: Option<u8>,
    error_limit: usize,
    stdlib_profile: &str,
    format_json: bool,
    verbose: bool,
) {
    if verbose {
        eprintln!("stdlib profile: {stdlib_profile}");
    }
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let stdlib_dir = stdlib::ensure_stdlib();

    // --stdlib=proven: verify all pure-MVL stdlib function bodies against all
    // 11 requirements before checking user code (ADR-0023, #538).
    let mut stdlib_proven_failed = false;
    if stdlib_profile == "proven" {
        let stdlib_errors = check_proven_stdlib();
        if !stdlib_errors.is_empty() {
            eprintln!(
                "note: --stdlib=proven: {} stdlib file(s) have verification errors:",
                stdlib_errors.len()
            );
            for (name, result) in &stdlib_errors {
                for err in &result.errors {
                    eprintln!(
                        "std/{name}:{}:{}: error[req{}]: {}",
                        err.span().line,
                        err.span().col,
                        err.requirement_number(),
                        err.message()
                    );
                }
            }
            stdlib_proven_failed = true;
        }
    }

    // Parse all files once so we can pass them to both the resolver and the checker.
    let mut parsed: Vec<(String, Program, String)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, src) = loader::parse_or_exit(&file_str);
            (file_str, prog, src)
        })
        .collect();

    // When checking a single file, also load imported sibling modules so the
    // resolver can validate cross-module imports (mirrors build_project behaviour).
    // Track how many entries are "requested" vs "resolver-only" siblings.
    let check_count = parsed.len();
    if Path::new(path).is_file() {
        let already_loaded: std::collections::HashSet<String> =
            parsed.iter().map(|(f, _, _)| loader::stem(f)).collect();
        let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        if let Some((_, entry_prog, _)) = parsed.first() {
            let extra_mods = loader::collect_imported_module_names(entry_prog);
            for mod_name in extra_mods {
                if already_loaded.contains(&mod_name) {
                    continue;
                }
                let sib_path = entry_dir.join(format!("{mod_name}.mvl"));
                if sib_path.exists() {
                    let sib_str = sib_path.display().to_string();
                    let (sib_prog, sib_src) = loader::parse_or_exit(&sib_str);
                    parsed.push((sib_str, sib_prog, sib_src));
                }
            }
        }
    }

    // Run the module resolver across all files, wiring in the extracted stdlib.
    let modules: Vec<(String, Program)> = parsed
        .iter()
        .map(|(file_str, prog, _)| (loader::stem(file_str), prog.clone()))
        .collect();
    let resolve_result = resolver::resolve_project(modules, Some(&stdlib_dir));
    let mut had_errors = !resolve_result.is_ok() || stdlib_proven_failed;
    for err in &resolve_result.errors {
        eprintln!("error[resolver]: {err}");
    }

    let registry = PassRegistry::default_registry();

    // Pre-parse stdlib files imported by user programs so the checker knows
    // about their types and functions.  This covers `use std.io.{...}` etc.
    let stdlib_prelude = loader::load_stdlib_prelude(
        parsed.iter().take(check_count).map(|(_, p, _)| p),
        &stdlib_dir,
    );

    // Snapshot all parsed user programs for cross-module prelude building.
    // Intentionally includes resolver-only siblings (auto-loaded to satisfy imports,
    // not explicitly requested): they may define types or functions that the
    // explicitly-checked files call and must therefore be visible to the checker.
    let all_user_progs: Vec<Program> = parsed.iter().map(|(_, p, _)| p.clone()).collect();

    // Collect errors across all files for JSON output (when --format=json).
    let mut json_error_items: Vec<String> = Vec::new();

    // Only run the checker on explicitly requested files (not resolver-only siblings).
    for (idx, (file_str, prog, _src)) in parsed.iter().take(check_count).enumerate() {
        // Build per-file prelude: stdlib + all OTHER user modules so that
        // cross-file function and type references resolve (whole-program checking).
        // Flanking slices of all_user_progs avoid cloning individual Programs;
        // check_with_two_preludes chains prelude_a (&[Program]) and prelude_b
        // (&[&Program]) without any additional allocation.
        let (before, after_with_self) = all_user_progs.split_at(idx);
        let after = &after_with_self[1..];
        let user_prelude: Vec<&Program> = before.iter().chain(after.iter()).collect();
        let result = checker::check_with_two_preludes(&stdlib_prelude, &user_prelude, prog);

        if let Some(req) = req_filter {
            // Single-requirement mode: run only the requested pass.
            let verdict = registry.run_req(req, prog, &result);
            let name = registry.pass_name(req).unwrap_or("unknown");
            if let Some(loc) = verdict.location() {
                println!(
                    "{file_str}:{loc}: Req {req} ({name}) — {} — {}",
                    verdict.label(),
                    verdict.detail()
                );
            } else {
                println!(
                    "{file_str}: Req {req} ({name}) — {} — {}",
                    verdict.label(),
                    verdict.detail()
                );
            }
            if verdict.is_failed() {
                had_errors = true;
            }
        } else if format_json {
            // JSON output mode: accumulate errors; emit a single document at end.
            if !result.is_ok() {
                had_errors = true;
                let display_count = if error_limit == 0 {
                    result.errors.len()
                } else {
                    error_limit.min(result.errors.len())
                };
                for err in result.errors.iter().take(display_count) {
                    let span = err.span();
                    let req = err.requirement_number();
                    let item = format!(
                        "    {{\n      \"code\": \"E{req:04}\",\n      \"requirement\": {req},\n      \"message\": \"{msg}\",\n      \"location\": {{ \"file\": \"{file}\", \"line\": {line}, \"column\": {col} }}\n    }}",
                        req = req,
                        msg = super::json_escape(&err.message()),
                        file = super::json_escape(file_str),
                        line = span.line,
                        col = span.col,
                    );
                    json_error_items.push(item);
                }
            }
        } else {
            // Full check mode: report type errors then show verdict summary.
            let verdicts = registry.run_all(prog, &result);
            let proven = (1u8..=11)
                .filter(|&i| verdicts[i as usize].is_proven())
                .count();
            if result.is_ok() {
                println!("{file_str}: OK  ({proven}/11 requirements proven)");
            } else {
                had_errors = true;
                let errors = &result.errors;
                let display_count = if error_limit == 0 {
                    errors.len()
                } else {
                    error_limit.min(errors.len())
                };
                for err in errors.iter().take(display_count) {
                    eprintln!(
                        "{}:{}:{}: error[req{}]: {}",
                        file_str,
                        err.span().line,
                        err.span().col,
                        err.requirement_number(),
                        err.message()
                    );
                }
                if error_limit > 0 && errors.len() > error_limit {
                    eprintln!(
                        "... and {} more errors (use --error-limit=0 to show all)",
                        errors.len() - error_limit
                    );
                }
                let failed = (1u8..=11)
                    .filter(|&i| verdicts[i as usize].is_failed())
                    .count();
                eprintln!("{file_str}: FAIL  ({proven}/11 proven, {failed} failed)");
            }
            if verbose {
                for req in 1u8..=11 {
                    let name = registry.pass_name(req).unwrap_or("unknown");
                    let v = &verdicts[req as usize];
                    eprintln!(
                        "  {} Req {:2} ({}): {}",
                        v.status_char(),
                        req,
                        name,
                        v.detail()
                    );
                }
            }
        }
    }

    // Emit JSON document after processing all files.
    if format_json && req_filter.is_none() {
        let error_count = json_error_items.len();
        println!("{{");
        println!("  \"errors\": [");
        for (i, item) in json_error_items.iter().enumerate() {
            let comma = if i + 1 < error_count { "," } else { "" };
            println!("{item}{comma}");
        }
        println!("  ],");
        println!("  \"warnings\": [],");
        println!("  \"summary\": {{ \"errors\": {error_count}, \"warnings\": 0 }}");
        println!("}}");
    }

    if had_errors {
        process::exit(1);
    }
}
