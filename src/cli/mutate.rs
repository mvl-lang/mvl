// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Decl;
use std::fs;
use std::path::PathBuf;
use std::process;

pub fn run(path: &str, quiet: bool, gen_boundary: bool, limit: Option<usize>) {
    let test_files = loader::mvl_files(path, true);
    if test_files.is_empty() {
        eprintln!("No *_test.mvl files found at: {path}");
        process::exit(1);
    }

    // Duplicate module name check (same as cmd_test)
    let mut seen: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let s = loader::stem(&file_str);
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

    let crate_name = "mvl_mutate";
    let tmp_dir = std::env::temp_dir().join(format!("mvl_mutate_{}", process::id()));
    let src_dir = tmp_dir.join("src");

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

    // Load the implicit stdlib prelude (core + Phase 4 stdlib files).
    let stdlib_prelude_progs = loader::load_implicit_prelude();

    // Transpile all test files with mutation instrumentation
    let mut modules: Vec<(String, String, String)> = Vec::new();
    let mut all_mutants: Vec<transpiler::MutantInfo> = Vec::new();
    let mut file_stems: Vec<String> = Vec::new();
    // module_name → original file path, used by --gen-boundary to read source lines
    let mut file_paths: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // The stdlib prelude (strings.mvl, lists.mvl, …) uses extern "rust" blocks,
    // so the runtime crate is always needed when the prelude is loaded.
    let mut need_mvl_runtime = transpiler::prelude_requires_runtime(&stdlib_prelude_progs);

    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        let s = loader::stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let result = transpiler::transpile(
            &tir,
            transpiler::TranspileConfig::new(&module_name)
                .with_file_stem(&module_name)
                .with_prelude(stdlib_prelude_progs.clone())
                .with_mutation()
                .for_test_file(),
        );
        let (out, mutants) = (result.output, result.mutants);
        if out.has_extern_rust || transpiler::has_std_imports(&prog) {
            need_mvl_runtime = true;
        }
        all_mutants.extend(mutants);
        file_stems.push(module_name.clone());
        file_paths
            .entry(module_name.clone())
            .or_insert_with(|| file_str.clone());
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        modules.push((module_name, file_str, module_content));
    }

    // Include source files with inline test fns
    let covered_stems: std::collections::HashSet<String> = file_stems.iter().cloned().collect();
    let source_files = loader::mvl_files(path, false);
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = loader::stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue;
        }
        let (prog, _src) = super::parse_or_exit(&file_str);
        let has_tests = prog
            .declarations
            .iter()
            .any(|d| matches!(d, Decl::Fn(fd) if fd.is_test));
        if !has_tests {
            continue;
        }
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let result = transpiler::transpile(
            &tir,
            transpiler::TranspileConfig::new(&module_name)
                .with_file_stem(&module_name)
                .with_prelude(stdlib_prelude_progs.clone())
                .with_mutation()
                .for_test_crate(),
        );
        let (out, mutants) = (result.output, result.mutants);
        if out.has_extern_rust || transpiler::has_std_imports(&prog) {
            need_mvl_runtime = true;
        }
        all_mutants.extend(mutants);
        file_stems.push(module_name.clone());
        file_paths
            .entry(module_name.clone())
            .or_insert_with(|| file_str.clone());
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        modules.push((module_name, file_str, module_content));
    }

    if all_mutants.is_empty() {
        println!("No mutation points found (no arithmetic/comparison/logic operators or Bool/Int literals in non-test code).");
        return;
    }

    // Apply limit: take first N mutants
    let all_mutants: Vec<transpiler::MutantInfo> = if let Some(n) = limit {
        all_mutants.into_iter().take(n).collect()
    } else {
        all_mutants
    };

    if !quiet {
        println!(
            "Found {} test file(s), {} mutation point(s){}",
            test_files.len(),
            all_mutants.len(),
            if limit.is_some() { " (limited)" } else { "" }
        );
    }

    // Build combined lib.rs with all mutation dispatch wrappers embedded
    let mut combined_rs = String::new();
    combined_rs.push_str("// MVL mutation runner — generated by `mvl mutate`\n");
    combined_rs.push_str("// Do not edit; regenerate with `mvl mutate`.\n");
    combined_rs.push_str(
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe, unpredictable_function_pointer_comparisons)]\n\n",
    );
    for (module_name, label, module_content) in &modules {
        combined_rs.push_str(&format!("// === {label} ===\n"));
        combined_rs.push_str("#[cfg(test)]\n");
        combined_rs.push_str(&format!("mod {module_name} {{\n"));
        combined_rs.push_str("    #[allow(unused)]\n");
        combined_rs.push_str("    use super::*;\n");
        combined_rs.push_str(module_content);
        combined_rs.push_str("}\n\n");
    }

    // Write Cargo.toml
    let mvl_runtime_dep = if need_mvl_runtime {
        "mvl_runtime = { path = \"./mvl_runtime\" }  # MVL security labels and prelude\n"
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
        let runtime_src = mvl::mvl::runtime_embed::ensure_runtime_rust();
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

    // ── Phase 1: compile once ─────────────────────────────────────────────
    if !quiet {
        println!(
            "Compiling mutant binary (one build for all {} mutants)…",
            all_mutants.len()
        );
    }
    let build_output = process::Command::new("cargo")
        .args(["test", "--no-run", "--message-format=json"])
        .current_dir(&tmp_dir)
        .output()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run cargo: {e}");
            process::exit(1);
        });
    if !build_output.status.success() {
        eprintln!("error: cargo build failed for mutation crate");
        eprintln!("{}", String::from_utf8_lossy(&build_output.stderr));
        process::exit(1);
    }

    let binary_path = super::test::find_test_binary_from_cargo_output(&build_output.stdout)
        .unwrap_or_else(|| {
            eprintln!("error: could not locate compiled test binary from cargo output");
            process::exit(1);
        });

    // ── Phase 2: baseline run (no MVL_MUTANT) ────────────────────────────
    let baseline = process::Command::new(&binary_path)
        .env_remove("MVL_MUTANT") // guard against inherited env in CI
        .args(["--quiet", "--test-threads=1"])
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .status()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run baseline test: {e}");
            process::exit(1);
        });
    if !baseline.success() {
        eprintln!("error: baseline tests fail (without any mutation) — fix tests before running mutation analysis");
        process::exit(1);
    }

    // ── Phase 3: run all mutants in parallel ─────────────────────────────
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let chunk_size = all_mutants.len().div_ceil(parallelism);

    if !quiet {
        println!(
            "Running {} mutants across {} workers…",
            all_mutants.len(),
            parallelism
        );
    }

    let killed_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut results: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    std::thread::scope(|scope| {
        let handles: Vec<_> = all_mutants
            .chunks(chunk_size.max(1))
            .map(|chunk| {
                let bin = binary_path.clone();
                let kc = std::sync::Arc::clone(&killed_count);
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|m| {
                            let status = process::Command::new(&bin)
                                .env("MVL_MUTANT", &m.id)
                                .args(["--quiet", "--test-threads=1"])
                                .stdout(process::Stdio::null())
                                .stderr(process::Stdio::null())
                                .status()
                                .unwrap_or_else(|e| {
                                    eprintln!("warning: failed to run mutant {}: {e}", m.id);
                                    // treat as survived to avoid false-positives
                                    process::ExitStatus::default()
                                });
                            let killed = !status.success();
                            if killed {
                                kc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            }
                            (m.id.clone(), killed)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        for handle in handles {
            for (id, killed) in handle.join().expect("mutant worker thread panicked") {
                results.insert(id, killed);
            }
        }
    });

    // ── Report ────────────────────────────────────────────────────────────
    let stems: Vec<&str> = file_stems.iter().map(|s| s.as_str()).collect();
    if !quiet {
        print!(
            "{}",
            transpiler::format_mutation_report(&all_mutants, &results, &stems)
        );
    } else {
        let total = all_mutants.len();
        let killed = killed_count.load(std::sync::atomic::Ordering::Relaxed);
        let pct = (killed * 100).checked_div(total).unwrap_or(100);
        println!("Mutation score: {killed}/{total} ({pct}%)");
    }

    // ── Boundary value analysis (--gen-boundary) ──────────────────────────
    if gen_boundary {
        print!(
            "{}",
            transpiler::format_boundary_report(&all_mutants, &results, &file_paths)
        );
    }
}
