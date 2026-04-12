use mvl::mvl::checker;
use mvl::mvl::parser::ast::{Decl, Program, Totality, TypeBody};
use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    let cmd = &args[1];
    match cmd.as_str() {
        "--version" | "-V" | "version" => {
            println!("mvl {}", env!("CARGO_PKG_VERSION"));
        }
        "--help" | "-h" | "help" => {
            print_usage();
        }
        "check" => {
            let path = require_path_arg(&args, "check");
            cmd_check(&path);
        }
        "build" => {
            let path = require_path_arg(&args, "build");
            build_project(&path, false);
        }
        "run" => {
            let path = require_path_arg(&args, "run");
            build_project(&path, true);
        }
        "transpile" => {
            let path = require_path_arg(&args, "transpile");
            cmd_transpile(&path);
        }
        "test" => {
            let path = require_path_arg(&args, "test");
            cmd_test(&path);
        }
        "assurance" => {
            let path = require_path_arg(&args, "assurance");
            let json = args.iter().any(|a| a == "--format=json" || a == "--json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            cmd_assurance(&path, json, verbose);
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("mvl compiler v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("Usage:");
    eprintln!("  mvl --version, -V                  — show version");
    eprintln!("  mvl --help, -h                     — show this help");
    eprintln!("  mvl check <file|dir>               — parse and type-check");
    eprintln!("  mvl build <file|dir>               — transpile to Rust and run cargo build");
    eprintln!("  mvl run   <file.mvl>               — transpile, build, and execute");
    eprintln!("  mvl test  <file|dir>               — find *_test.mvl files and run cargo test");
    eprintln!("  mvl assurance <file|dir>           — emit assurance report");
    eprintln!("  mvl assurance <file|dir> --json    — emit assurance report as JSON");
    eprintln!("  mvl assurance <file|dir> --verbose — per-function requirement detail");
    eprintln!("  mvl transpile <file.mvl>           — print transpiled Rust to stdout");
}

fn require_path_arg(args: &[String], cmd: &str) -> String {
    if args.len() < 3 {
        eprintln!("Usage: mvl {cmd} <file.mvl|directory>");
        process::exit(1);
    }
    args[2].clone()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Parse and type-check a .mvl file or all .mvl files in a directory.
fn cmd_check(path: &str) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut had_errors = false;
    for file in &files {
        let file_str = file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let result = checker::check(&prog);
        if result.is_ok() {
            println!("{file_str}: OK");
        } else {
            had_errors = true;
            for err in &result.errors {
                eprintln!("error in {file_str}: {err:?}");
            }
        }
    }

    if had_errors {
        process::exit(1);
    }
}

/// Transpile a .mvl file to Rust and print the output to stdout.
fn cmd_transpile(path: &str) {
    let (prog, _src) = parse_or_exit(path);
    let crate_name = stem(path);
    let out = transpiler::transpile(&prog, &crate_name);
    println!("// === Cargo.toml ===");
    println!("{}", out.cargo_toml);
    let file_label = if out.has_main {
        "src/main.rs"
    } else {
        "src/lib.rs"
    };
    println!("// === {file_label} ===");
    println!("{}", out.lib_rs);
}

/// Transpile a .mvl file to a Cargo project, build it, and optionally run it.
fn build_project(path: &str, run: bool) {
    // For directory inputs, use the directory stem as the crate name and
    // concatenate all .mvl files (simple Phase 1 approach: single-crate multi-file).
    let file_path = if Path::new(path).is_dir() {
        // Build requires a main file in the directory
        let main_candidates = ["main.mvl", "mod.mvl", "lib.mvl"];
        let dir = Path::new(path);
        main_candidates
            .iter()
            .find_map(|name| {
                let p = dir.join(name);
                if p.exists() {
                    Some(p.display().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                eprintln!("No main.mvl / mod.mvl / lib.mvl found in {path}");
                process::exit(1);
            })
    } else {
        path.to_string()
    };

    let (prog, _src) = parse_or_exit(&file_path);
    let crate_name = stem(path);
    let out = transpiler::transpile(&prog, &crate_name);

    // Write to a deterministic temp directory per crate name
    let tmp_dir = std::env::temp_dir().join(format!("mvl_build_{crate_name}"));
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap_or_else(|e| {
        eprintln!("Cannot create temp dir {}: {e}", src_dir.display());
        process::exit(1);
    });

    let cargo_toml_path = tmp_dir.join("Cargo.toml");
    fs::write(&cargo_toml_path, &out.cargo_toml).unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });

    if out.has_main {
        // Binary crate: the transpiled code IS src/main.rs
        fs::write(src_dir.join("main.rs"), &out.lib_rs).unwrap_or_else(|e| {
            eprintln!("Cannot write main.rs: {e}");
            process::exit(1);
        });
    } else {
        // Library crate: lib.rs + a stub main for cargo build to succeed
        fs::write(src_dir.join("lib.rs"), &out.lib_rs).unwrap_or_else(|e| {
            eprintln!("Cannot write lib.rs: {e}");
            process::exit(1);
        });
        fs::write(
            src_dir.join("main.rs"),
            transpiler::cargo::emit_main_rs_stub(&crate_name),
        )
        .unwrap_or_else(|e| {
            eprintln!("Cannot write stub main.rs: {e}");
            process::exit(1);
        });
    }

    // If the program uses mvl_runtime, copy it as a sibling directory
    // so the relative path `../mvl_runtime` in Cargo.toml resolves.
    if out.extern_count > 0 {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mvl_runtime");
        let runtime_dst = tmp_dir.parent().unwrap().join("mvl_runtime");
        if runtime_src.exists() && !runtime_dst.exists() {
            copy_dir_recursive(&runtime_src, &runtime_dst).expect("copy mvl_runtime");
        }
    }

    println!("Transpiled to: {}", tmp_dir.display());

    let cargo_cmd = if run && out.has_main { "run" } else { "build" };
    println!("Running: cargo {cargo_cmd}");

    let status = process::Command::new("cargo")
        .arg(cargo_cmd)
        .current_dir(&tmp_dir)
        .status()
        .unwrap_or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!(
                    "error: `cargo` not found in PATH — install Rust from https://rustup.rs/"
                );
            } else {
                eprintln!("error: failed to run cargo: {e}");
            }
            process::exit(1);
        });

    if !status.success() {
        eprintln!("cargo {cargo_cmd} failed");
        process::exit(1);
    }

    if !run || !out.has_main {
        println!("Build successful.");
        if run && !out.has_main {
            eprintln!("Note: no `fn main` in MVL source — nothing to run.");
        }
    }
}

/// Find all `*_test.mvl` files, transpile to Rust test crates, and run `cargo test`.
fn cmd_test(path: &str) {
    let test_files = mvl_files(path, true); // test_only=true
    if test_files.is_empty() {
        eprintln!("No *_test.mvl files found at: {path}");
        process::exit(1);
    }

    println!("Found {} test file(s):", test_files.len());
    for f in &test_files {
        println!("  {}", f.display());
    }

    // Check for duplicate module names before generating output.
    let mut seen: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let s = stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        if let Some(prev) = seen.get(&module_name) {
            eprintln!(
                "error: duplicate test module name `{module_name}` from:\n  {}\n  {}",
                prev.display(),
                test_file.display()
            );
            process::exit(1);
        }
        seen.insert(module_name, test_file.clone());
    }

    // Use a per-invocation temp directory to avoid concurrent-run collisions.
    let crate_name = "mvl_test";
    let tmp_dir = std::env::temp_dir().join(format!("mvl_test_{}", process::id()));
    let src_dir = tmp_dir.join("src");

    // Remove any stale directory from a previous run at this path, then recreate.
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir).unwrap_or_else(|e| {
            eprintln!("Cannot clean temp dir {}: {e}", tmp_dir.display());
            process::exit(1);
        });
    }
    fs::create_dir_all(&src_dir).unwrap_or_else(|e| {
        eprintln!("Cannot create temp dir {}: {e}", src_dir.display());
        process::exit(1);
    });

    // Build a combined Rust test file from all test modules.
    let mut combined_rs = String::new();
    combined_rs.push_str("// MVL test runner — generated by `mvl test`\n");
    combined_rs.push_str("// Do not edit; regenerate with `mvl test`.\n");
    // File-level allow — inner attributes must appear at the top of the file,
    // before any items.  We strip per-module copies below.
    combined_rs
        .push_str("#![allow(dead_code, unused_variables, unused_imports, unused_parens)]\n\n");

    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let s = stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        let out = transpiler::transpile(&prog, &module_name);
        // Strip per-file inner #![allow] — they're invalid inside mod blocks and
        // we already have the file-level allow at the top.
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        combined_rs.push_str(&format!("// === {} ===\n", test_file.display()));
        combined_rs.push_str("#[cfg(test)]\n");
        combined_rs.push_str(&format!("mod {module_name} {{\n"));
        combined_rs.push_str("    #[allow(unused)]\n");
        combined_rs.push_str("    use super::*;\n");
        combined_rs.push_str(&module_content);
        combined_rs.push_str("}\n\n");
    }

    // Write a minimal Cargo.toml for the test runner.
    let cargo_toml = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"
    );
    fs::write(tmp_dir.join("Cargo.toml"), &cargo_toml).unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });
    fs::write(src_dir.join("lib.rs"), &combined_rs).unwrap_or_else(|e| {
        eprintln!("Cannot write lib.rs: {e}");
        process::exit(1);
    });

    println!("Transpiled tests to: {}", tmp_dir.display());
    println!("Running: cargo test");

    let status = process::Command::new("cargo")
        .arg("test")
        .current_dir(&tmp_dir)
        .status()
        .unwrap_or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!(
                    "error: `cargo` not found in PATH — install Rust from https://rustup.rs/"
                );
            } else {
                eprintln!("error: failed to run cargo: {e}");
            }
            process::exit(1);
        });

    if !status.success() {
        eprintln!("cargo test failed");
        process::exit(1);
    }

    println!("All tests passed.");
}

/// Emit an assurance report for a file or directory.
fn cmd_assurance(path: &str, json: bool, verbose: bool) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut total_fns: usize = 0;
    let mut total_verified: usize = 0; // `total fn` (MVL-defined)
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
    let mut total_capabilities_fns: usize = 0;
    let mut total_refinements_fns: usize = 0;
    let mut all_fn_details: Vec<FnDetail> = Vec::new();

    for file in &files {
        let file_str = file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let stats = collect_assurance_stats(&prog, verbose);
        let result = checker::check(&prog);

        total_fns += stats.fn_count;
        total_verified += stats.total_fn_count;
        total_partial += stats.partial_fn_count;
        _total_pub += stats.pub_fn_count;
        total_extern += stats.extern_fn_count; // fn signatures, not block count
        total_test_fns += stats.test_fn_count;
        check_errors += result.errors.len();
        total_struct_types += stats.struct_type_count;
        total_enum_types += stats.enum_type_count;
        total_effects_fns += stats.effects_fn_count;
        total_capabilities_fns += stats.capabilities_fn_count;
        total_refinements_fns += stats.refinements_fn_count;
        for (i, count) in result.req_errors.iter().enumerate().skip(1) {
            req_errors[i] += count;
        }
        if verbose {
            all_fn_details.extend(stats.fn_details);
        }
        file_count += 1;
    }

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
        // NOTE(#96): "pub" is always 0 until the module resolver is merged.
        let req_json: String = (1..=11)
            .map(|i| format!("    \"{i}\": {}", req_errors[i]))
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
  "requirements": {{
{req_json}
  }},
  "check_errors": {check_errors}
}}"#
        );
    } else {
        println!("MVL Assurance Report");
        println!("====================");
        println!("Files checked:       {file_count}");
        println!("Functions:           {total_fns}");
        println!("  total fn:          {total_verified} ({verified_pct}% of implemented)");
        println!("  partial fn:        {total_partial}");
        println!("  extern fn:         {total_extern} ({extern_pct}% trust boundary)");
        println!("  implemented:       {implemented}");
        println!("  test fn:           {total_test_fns}");
        println!();
        println!("Requirements verified:");
        print_req_row(
            1,
            "Type safety",
            &req_errors,
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
            if req_errors[2] == 0 {
                "no violations".to_string()
            } else {
                format!("{} use-after-move", req_errors[2])
            }
            .as_str(),
        );
        print_req_row(
            3,
            "Totality",
            &req_errors,
            &format!(
                "{} total fn, {} non-exhaustive match",
                total_verified, req_errors[3]
            ),
        );
        print_req_row(
            4,
            "Null elimination",
            &req_errors,
            &format!("{} direct Option access", req_errors[4]),
        );
        print_req_row(
            5,
            "Error visibility",
            &req_errors,
            &format!("{} unhandled Result", req_errors[5]),
        );
        print_req_row(
            6,
            "Ownership",
            &req_errors,
            &format!("{} immutability violations", req_errors[6]),
        );
        print_req_row(
            7,
            "Effects",
            &req_errors,
            &format!(
                "{} fns declare effects, {} undeclared",
                total_effects_fns, req_errors[7]
            ),
        );
        print_req_row(
            8,
            "Termination",
            &req_errors,
            &format!(
                "{} total fn, {} partial fn, {} violations",
                total_verified, total_partial, req_errors[8]
            ),
        );
        print_req_row(
            9,
            "Data race freedom",
            &req_errors,
            &format!(
                "{} fns use capabilities, {} violations",
                total_capabilities_fns, req_errors[9]
            ),
        );
        print_req_row(
            10,
            "Refinements",
            &req_errors,
            &format!(
                "{} fns with refinements, {} violations",
                total_refinements_fns, req_errors[10]
            ),
        );
        print_req_row(
            11,
            "IFC",
            &req_errors,
            &format!(
                "{} extern (trust boundary), {} flow violations",
                total_extern, req_errors[11]
            ),
        );
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
                let caps = if fd.has_capabilities { "yes" } else { "-" };
                let refs = if fd.has_refinements { "yes" } else { "-" };
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

fn print_req_row(req: u8, name: &str, req_errors: &[usize; 12], detail: &str) {
    debug_assert!((1..=11).contains(&req), "req must be 1–11, got {req}");
    let status = if req_errors[req as usize] == 0 {
        "✓"
    } else {
        "✗"
    };
    println!("  Req {:>2}  {:<20} {}  {}", req, name, status, detail);
}

// ── Assurance stats ───────────────────────────────────────────────────────

struct AssuranceStats {
    fn_count: usize,
    total_fn_count: usize,
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
    has_capabilities: bool,
    has_refinements: bool,
    is_test: bool,
    is_extern: bool,
}

fn collect_assurance_stats(prog: &Program, collect_details: bool) -> AssuranceStats {
    let mut stats = AssuranceStats {
        fn_count: 0,
        total_fn_count: 0,
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
                    || fd.params.iter().any(|p| p.refinement.is_some());
                if fd.is_test {
                    stats.test_fn_count += 1;
                } else {
                    stats.fn_count += 1;
                    match fd.totality {
                        Some(Totality::Total) => stats.total_fn_count += 1,
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
                    stats.fn_details.push(FnDetail {
                        name: fd.name.clone(),
                        totality: fd.totality.clone(),
                        effects: fd.effects.clone(),
                        has_capabilities: has_caps,
                        has_refinements: has_refs,
                        is_test: fd.is_test,
                        is_extern: false,
                    });
                }
            }
            Decl::Extern(ed) => {
                // Each signature inside an extern block is a trust-boundary function.
                stats.extern_fn_count += ed.fns.len();
                stats.fn_count += ed.fns.len();
                if collect_details {
                    for ef in &ed.fns {
                        stats.fn_details.push(FnDetail {
                            name: ef.name.clone(),
                            totality: None,
                            effects: ef.effects.clone(),
                            has_capabilities: false,
                            has_refinements: false,
                            is_test: false,
                            is_extern: true,
                        });
                    }
                }
            }
            Decl::Type(td) => match &td.body {
                TypeBody::Struct(_) => stats.struct_type_count += 1,
                TypeBody::Enum(_) => stats.enum_type_count += 1,
                TypeBody::Alias(_) => {}
            },
            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Collect .mvl files from a file path or directory.
///
/// If `test_only` is true, only returns files ending in `_test.mvl`.
/// Otherwise returns all `.mvl` files (excluding test files).
fn mvl_files(path: &str, test_only: bool) -> Vec<PathBuf> {
    let p = Path::new(path);
    if p.is_file() {
        let is_test = p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("_test.mvl"))
            .unwrap_or(false);
        if test_only && !is_test {
            return vec![];
        }
        if !test_only && is_test {
            return vec![];
        }
        return vec![p.to_path_buf()];
    }

    if p.is_dir() {
        let mut files: Vec<PathBuf> = Vec::new();
        collect_mvl_files_recursive(p, test_only, &mut files);
        files.sort();
        return files;
    }

    vec![]
}

fn collect_mvl_files_recursive(dir: &Path, test_only: bool, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Cannot read directory {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_mvl_files_recursive(&path, test_only, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".mvl") {
                let is_test = name.ends_with("_test.mvl");
                if test_only == is_test {
                    out.push(path);
                }
            }
        }
    }
}

/// Parse the given .mvl file or exit with an error message.
fn parse_or_exit(path: &str) -> (mvl::mvl::parser::ast::Program, String) {
    let src = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Cannot read {path}: {e}");
        process::exit(1);
    });
    let (mut parser, lex_errors) = Parser::new(&src);
    if !lex_errors.is_empty() {
        for err in &lex_errors {
            eprintln!("lex error: {err:?}");
        }
        process::exit(1);
    }
    let prog = parser.parse_program();
    let parse_errors = parser.errors();
    if !parse_errors.is_empty() {
        for err in parse_errors {
            eprintln!("parse error: {err:?}");
        }
        process::exit(1);
    }
    (prog, src)
}

/// Extract the file or directory stem from a path.
fn stem(path: &str) -> String {
    let p = Path::new(path);
    if p.is_dir() {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program")
            .to_string()
    } else {
        p.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program")
            .to_string()
    }
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

// ── Assurance stats tests ─────────────────────────────────────────────────

#[cfg(test)]
mod assurance_tests {
    use super::*;

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
        // UseAfterMove → requirement 2
        let src = "fn f() -> Int { let x = 1; let _y = move(x); x }";
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
        assert_eq!(eff.effects, vec!["DB"]);
        assert!(!eff.is_test);
        let test = stats
            .fn_details
            .iter()
            .find(|d| d.name == "check_it")
            .unwrap();
        assert!(test.is_test);
    }
}
