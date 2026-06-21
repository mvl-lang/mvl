// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Decl;
use mvl::mvl::parser::Parser;
use std::fs;
use std::path::PathBuf;
use std::process;

pub fn run(path: &str, quiet: bool, verbose: bool, coverage: bool, bdd: bool) {
    if quiet && verbose {
        eprintln!(
            "warning: --quiet and --verbose are mutually exclusive; --verbose takes precedence"
        );
    }
    let quiet = quiet && !verbose;

    let test_files = loader::mvl_files(path, true); // test_only=true
    if test_files.is_empty() {
        eprintln!("No *_test.mvl files found at: {path}");
        process::exit(1);
    }

    if !quiet {
        println!("Found {} test file(s):", test_files.len());
        for f in &test_files {
            println!("  {}", f.display());
        }
    }

    // Check for duplicate module names before generating output.
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

    // Use a per-invocation temp directory for source files — avoids concurrent
    // collision when multiple `mvl test` processes run on the same input.
    // A stable CARGO_TARGET_DIR (keyed on the canonical input path) is shared
    // across runs so compiled dependencies are cached between invocations.
    let crate_name = "mvl_test";
    let path_hash = {
        use std::hash::{Hash, Hasher};
        let canonical =
            std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut h);
        h.finish()
    };
    let tmp_dir = std::env::temp_dir().join(format!("mvl_test_{}", process::id()));
    let src_dir = tmp_dir.join("src");
    // Stable target dir shared across runs: compiled deps are reused even when
    // source files are regenerated (e.g. on every `make test` invocation).
    let cargo_target_dir = std::env::temp_dir().join(format!("mvl_test_target_{path_hash:016x}"));

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

    // Load the implicit stdlib prelude (core + Phase 4 stdlib files).
    let mut stdlib_prelude_progs = loader::load_implicit_prelude();
    // Pre-scan all test files to discover pure-MVL stdlib imports (e.g. json) and
    // extend the prelude so their types/functions are available during transpilation.
    // Also load any pkg.* package modules referenced by the test files.
    {
        let all_test_progs: Vec<_> = test_files
            .iter()
            .map(|f| super::parse_or_exit(&f.display().to_string()).0)
            .collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_root = super::find_project_root(&cwd);
        // Use a frontier loop (mirroring build.rs) to load transitive package
        // dependencies — e.g. if a test file imports pkg-health which uses pkg-http,
        // pkg-http's types must be in the prelude (#1477).
        {
            let mut seen_pkgs = std::collections::HashSet::new();
            let mut frontier = all_test_progs.clone();
            loop {
                let new_pkgs = loader::load_pkg_modules(&frontier, &project_root, &mut seen_pkgs);
                if new_pkgs.is_empty() {
                    break;
                }
                frontier = new_pkgs.clone();
                stdlib_prelude_progs.extend(new_pkgs);
            }
        }

        // Pre-compute which module names are explicitly imported by test files so
        // that pure-function sibling modules (no types/extern blocks) are also
        // loaded into the prelude when a test file uses `use module::fn`.
        // This fixes the cross-module import limitation tracked in issue #96.
        let imported_by_test_files: std::collections::HashSet<String> = all_test_progs
            .iter()
            .flat_map(loader::collect_imported_module_names)
            .collect();

        // For packages tested from their own src/ directory, also load sibling
        // .mvl files (non-test, including internal/) so types and extern
        // declarations are in scope during transpilation.
        // Track already-loaded paths so that multiple test files in the same
        // directory don't add the same library file multiple times.
        let mut loaded_prelude_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
        let mut sibling_progs: Vec<mvl::mvl::parser::ast::Program> = Vec::new();
        for f in &test_files {
            let dir = f.parent().unwrap_or_else(|| std::path::Path::new("."));
            // Scan src/ and src/internal/ (package convention per ADR-0012).
            let dirs_to_scan: Vec<std::path::PathBuf> =
                vec![dir.to_path_buf(), dir.join("internal")];
            for scan_dir in dirs_to_scan {
                if let Ok(entries) = fs::read_dir(&scan_dir) {
                    // Symlink escape guard (#715): canonicalize the scan root once so
                    // that symlinks pointing outside the directory are silently skipped.
                    let canon_scan_dir = fs::canonicalize(&scan_dir).ok();
                    for entry in entries.flatten() {
                        let p = entry.path();
                        // Skip entries that resolve outside the scanned directory.
                        if let Some(ref canon_root) = canon_scan_dir {
                            match fs::canonicalize(&p) {
                                Ok(canon_p) if canon_p.starts_with(canon_root) => {}
                                _ => continue,
                            }
                        }
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if p.extension().map(|e| e == "mvl").unwrap_or(false)
                            && !fname.contains("_test")
                            && p != **f
                            && loaded_prelude_paths.insert(p.clone())
                        {
                            if let Ok(src) = fs::read_to_string(&p) {
                                let (mut pp, _) = Parser::new(&src);
                                let parsed = pp.parse_program();
                                // Skip entry-point files (those defining `main`) — they are
                                // programs, not library modules.  Including them in the prelude
                                // causes duplicate type/function definitions when combined with
                                // test-local re-declarations.
                                //
                                // Load files that have extern blocks or type declarations.
                                // Also load pure-function files (no types/extern) only when a
                                // test file explicitly imports them via `use module::fn` — this
                                // resolves cross-module imports without risking shadowing of
                                // runtime primitives by unreferenced helpers (fix for #96).
                                let file_stem = p
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_owned();
                                if !transpiler::has_main_fn(&parsed)
                                    && (transpiler::has_extern_or_type_decls(&parsed)
                                        || imported_by_test_files.contains(&file_stem))
                                {
                                    sibling_progs.push(parsed.clone());
                                    stdlib_prelude_progs.push(parsed);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Single pass over all programs (test files + sibling library files) mirrors
        // cli/build.rs and ensures transitive stdlib deps are discovered together (#865).
        let all_for_extras: Vec<_> = all_test_progs
            .iter()
            .chain(sibling_progs.iter())
            .cloned()
            .collect();
        stdlib_prelude_progs.extend(loader::load_mvl_native_stdlib_extras(&all_for_extras));
    }

    // Collect native Cargo deps and bridge.rs from the full pkg.* closure so
    // that the test crate's Cargo.toml mirrors what `mvl build` would emit (#1481).
    // The frontier loop above loaded all transitive packages into stdlib_prelude_progs.
    let project_root = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        super::find_project_root(&cwd)
    };
    let native_dep_lines = loader::collect_pkg_native_dep_lines(&stdlib_prelude_progs, &project_root);
    let pkg_bridge = loader::find_pkg_bridge(&stdlib_prelude_progs, &project_root);

    // Build a combined Rust test file from all test modules.
    // Each entry: (module_name, display_label, content)
    let mut modules: Vec<(String, String, String)> = Vec::new();
    let mut all_branches: Vec<transpiler::BranchInfo> = Vec::new();
    let mut next_branch_id = 0usize;
    let mut file_stems: Vec<String> = Vec::new(); // ordered list for the coverage report
                                                  // BDD: collect scenario names (fn names starting with "scenario_") for Gherkin report.
    let mut scenarios: Vec<String> = Vec::new();
    // The stdlib prelude (strings.mvl, lists.mvl, …) uses extern "rust" blocks,
    // so the runtime crate is always needed when the prelude is loaded.
    let mut need_mvl_runtime = transpiler::prelude_requires_runtime(&stdlib_prelude_progs);

    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        let s = loader::stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        if bdd {
            for decl in &prog.declarations {
                if let Decl::Fn(fd) = decl {
                    if fd.is_test && fd.name.starts_with("scenario_") {
                        scenarios.push(fd.name.clone());
                    }
                }
            }
        }
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let (out, branches) = if coverage {
            {
                let r = transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_file_stem(&module_name)
                        .with_prelude(stdlib_prelude_progs.clone())
                        .with_coverage(next_branch_id)
                        .for_test_crate(),
                );
                (r.output, r.branches)
            }
        } else {
            (
                transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_prelude(stdlib_prelude_progs.clone())
                        .for_test_crate(),
                )
                .output,
                Vec::new(),
            )
        };
        if out.has_extern_rust || transpiler::has_std_imports(&prog) {
            need_mvl_runtime = true;
        }
        next_branch_id += branches.len();
        all_branches.extend(branches);
        file_stems.push(module_name.clone());
        // Strip per-file inner #![allow] — they're invalid inside mod blocks and
        // we already have the file-level allow at the top.
        let module_content: String = out
            .lib_rs
            .lines()
            .filter(|l| !l.trim_start().starts_with("#![allow("))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        modules.push((module_name, file_str, module_content));
    }

    // Also include source .mvl files that contain `test fn` declarations but
    // have no corresponding `*_test.mvl` counterpart.  This lets inline tests
    // (e.g. in `main.mvl`) run and appear in the coverage report.
    let covered_stems: std::collections::HashSet<String> = file_stems.iter().cloned().collect();
    let source_files = loader::mvl_files(path, false); // non-test files
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = loader::stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue; // already covered by a *_test.mvl file
        }
        let (prog, _src) = super::parse_or_exit(&file_str);
        // Only include if the file has at least one test fn.
        let has_tests = prog.declarations.iter().any(|d| {
            if let Decl::Fn(fd) = d {
                fd.is_test
            } else {
                false
            }
        });
        if !has_tests {
            continue;
        }
        if !quiet {
            println!("  (inline tests) {file_str}");
        }
        let mut expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
        expr_types.extend(checker::check_with_prelude(&stdlib_prelude_progs, &prog).expr_types);
        let all_fns = mvl::mvl::passes::mono::collect_fns([&prog]);
        let mono = mvl::mvl::passes::mono::monomorphize(&prog, &all_fns, &expr_types);
        let tir = mvl::mvl::ir::lower::lower(&prog, &mono, &expr_types);
        let (out, branches) = if coverage {
            {
                let r = transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_file_stem(&module_name)
                        .with_prelude(stdlib_prelude_progs.clone())
                        .with_coverage(next_branch_id)
                        .for_test_crate(),
                );
                (r.output, r.branches)
            }
        } else {
            (
                transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_prelude(stdlib_prelude_progs.clone())
                        .for_test_crate(),
                )
                .output,
                Vec::new(),
            )
        };
        if out.has_extern_rust || transpiler::has_std_imports(&prog) {
            need_mvl_runtime = true;
        }
        next_branch_id += branches.len();
        all_branches.extend(branches);
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

    let total_branches = next_branch_id;
    let mut combined_rs = String::new();
    combined_rs.push_str("// MVL test runner — generated by `mvl test`\n");
    combined_rs.push_str("// Do not edit; regenerate with `mvl test`.\n");
    // File-level allow — inner attributes must appear at the top of the file,
    // before any items.  We strip per-module copies below.
    combined_rs.push_str(
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe, unused_assignments, non_shorthand_field_patterns, unpredictable_function_pointer_comparisons)]\n\n",
    );

    if coverage {
        combined_rs.push_str(&transpiler::emit_cov_preamble(total_branches));
    }

    for (module_name, label, module_content) in &modules {
        combined_rs.push_str(&format!("// === {label} ===\n"));
        combined_rs.push_str("#[cfg(test)]\n");
        combined_rs.push_str(&format!("mod {module_name} {{\n"));
        combined_rs.push_str("    #[allow(unused)]\n");
        combined_rs.push_str("    use super::*;\n");
        combined_rs.push_str(module_content);
        combined_rs.push_str("}\n\n");
    }

    if coverage {
        combined_rs.push_str(&transpiler::emit_cov_report_test(total_branches));
    }

    // Write Cargo.toml for the test runner, pointing mvl_runtime at its absolute
    // source path so no per-invocation copy is needed (and the shared target dir
    // caches the compiled crate across runs).
    let mvl_runtime_dep = if need_mvl_runtime {
        let runtime_src = mvl::mvl::runtime_xdg::ensure_runtime_rust();
        format!(
            "mvl_runtime = {{ path = \"{}\", package = \"mvl_runtime_rust\" }}  # MVL security labels and prelude\n",
            runtime_src.display()
        )
    } else {
        String::new()
    };
    // Native Cargo deps from any `pkg.*` package's `[native]` section (#1481).
    let native_deps_block = if native_dep_lines.is_empty() {
        String::new()
    } else {
        let mut s = String::new();
        for line in &native_dep_lines {
            s.push_str(line);
            s.push('\n');
        }
        s
    };
    let cargo_toml = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{mvl_runtime_dep}{native_deps_block}"
    );

    fs::write(tmp_dir.join("Cargo.toml"), &cargo_toml).unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });

    // If a `pkg.*` package supplies a bridge.rs (Rust implementations of
    // extern "rust" fns), copy it into src/ and inject `mod bridge;` so the
    // test crate links the symbols — same wiring `mvl build` performs (#1481).
    // The declaration must come AFTER any inner attributes (`#![...]`), so we
    // insert it on the first blank line following the file-level allow.
    if let Some(ref bp) = pkg_bridge {
        fs::copy(bp, src_dir.join("bridge.rs")).unwrap_or_else(|e| {
            eprintln!("Cannot copy bridge.rs: {e}");
            process::exit(1);
        });
        let mut injected = String::with_capacity(combined_rs.len() + 16);
        let mut done = false;
        for line in combined_rs.lines() {
            injected.push_str(line);
            injected.push('\n');
            if !done && line.trim_start().starts_with("#![allow(") {
                injected.push_str("\nmod bridge;\n");
                done = true;
            }
        }
        if !done {
            // No inner-attribute line found — fall back to prepending after the
            // header comments by simply appending at the start.
            injected = format!("mod bridge;\n{combined_rs}");
        }
        combined_rs = injected;
    }

    let lib_rs_path = src_dir.join("lib.rs");
    fs::write(&lib_rs_path, &combined_rs).unwrap_or_else(|e| {
        eprintln!("Cannot write lib.rs: {e}");
        process::exit(1);
    });

    if verbose {
        println!("Transpiled tests to: {}", tmp_dir.display());
    }
    if !quiet {
        println!("Running: cargo test");
    }

    let cov_out_path = tmp_dir.join("mvl_cov.txt");

    let mut cmd = process::Command::new("cargo");
    cmd.arg("test")
        .arg("--lib")
        .arg("--target-dir")
        .arg(&cargo_target_dir)
        .current_dir(&tmp_dir);
    if quiet && !coverage {
        cmd.arg("-q");
    }
    if verbose || coverage {
        // Coverage requires --nocapture so the report test's println! reaches us.
        // With --coverage we also serialize tests to guarantee report runs last.
        cmd.arg("--").arg("--nocapture");
        if coverage {
            cmd.arg("--test-threads=1");
        }
    }
    if coverage {
        cmd.env("MVL_COV_OUT", &cov_out_path);
    }

    let status = if coverage {
        // Pipe stdout so we can filter out the internal `zzz_mvl_cov_report` test
        // line — it's an implementation detail, not a real user test.
        use std::io::{BufRead, BufReader};
        cmd.stdout(process::Stdio::piped());
        let mut child = cmd.spawn().unwrap_or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!(
                    "error: `cargo` not found in PATH — install Rust from https://rustup.rs/"
                );
            } else {
                eprintln!("error: failed to run cargo: {e}");
            }
            process::exit(1);
        });
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap_or_default();
                if !line.contains("zzz_mvl_cov_report") {
                    println!("{line}");
                }
            }
        }
        child.wait().unwrap_or_else(|e| {
            eprintln!("error: failed to wait for cargo: {e}");
            process::exit(1);
        })
    } else {
        cmd.status().unwrap_or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!(
                    "error: `cargo` not found in PATH — install Rust from https://rustup.rs/"
                );
            } else {
                eprintln!("error: failed to run cargo: {e}");
            }
            process::exit(1);
        })
    };

    if !status.success() {
        eprintln!("cargo test failed");
        process::exit(1);
    }

    if !quiet {
        println!("All tests passed.");
    }

    // ── BDD report ────────────────────────────────────────────────────────
    if bdd && !scenarios.is_empty() {
        println!();
        println!("BDD scenarios:");
        for name in &scenarios {
            let label = name
                .strip_prefix("scenario_")
                .unwrap_or(name)
                .replace('_', " ");
            println!("  Scenario: {label} ... ok");
        }
    }

    // ── Coverage report ───────────────────────────────────────────────────
    if coverage && !all_branches.is_empty() {
        let hits: Vec<u64> = match fs::read_to_string(&cov_out_path) {
            Ok(raw) => raw
                .lines()
                .filter_map(|l| l.trim().parse::<u64>().ok())
                .collect(),
            Err(_) => {
                eprintln!("warning: coverage data not found (report test may have been skipped)");
                Vec::new()
            }
        };
        let stems: Vec<&str> = file_stems.iter().map(|s| s.as_str()).collect();
        print!(
            "{}",
            transpiler::format_report(&all_branches, &hits, &stems)
        );
    }
}

/// Run `// expect:` annotation tests through the Rust transpiler backend.
///
/// Mirrors the LLVM text backend's `cmd_test_llvm_text` — discovers `.mvl` files
/// with `fn main` + `// expect:` annotations, compiles via the Rust transpiler,
/// runs the resulting binary, and compares stdout against the expected output.
///
/// This creates parity between `test-backend-rust` and `test-backend-llvm` so both
/// backends are exercised against the same corpus of expect-annotated test programs.
pub fn run_expect_tests(path: &str, quiet: bool, verbose: bool) {
    let all_mvl = loader::mvl_files_all(path);
    let mut test_cases: Vec<(PathBuf, String, bool)> = Vec::new();

    for file in &all_mvl {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !src.contains("fn main(") {
            continue;
        }
        // Skip files annotated for LLVM-only testing (e.g. IFC differences)
        // or known Rust transpiler limitations (e.g. closure capture → fn pointer)
        if src.contains("corpus:llvm") || src.contains("rust-expect-skip:") {
            continue;
        }
        if let Some(pat) = lli::parse_expect_pattern_annotation(&src) {
            test_cases.push((file.clone(), pat, true));
        } else if let Some(expected) = lli::parse_expect_annotation(&src) {
            test_cases.push((file.clone(), expected, false));
        }
    }

    if test_cases.is_empty() {
        if !quiet {
            println!(
                "No Rust transpiler expect-tests found (files with `fn main` + `// expect:`)."
            );
        }
        return;
    }

    if !quiet {
        println!("Rust transpiler: {} expect-test file(s)", test_cases.len());
    }

    let mvl_bin = std::env::current_exe().unwrap_or_else(|e| {
        eprintln!("error: cannot determine mvl binary path: {e}");
        process::exit(1);
    });
    let compiler_version = env!("CARGO_PKG_VERSION");
    let mut passed = 0usize;
    let mut failed = 0usize;

    for (file, expected, is_pattern) in &test_cases {
        let file_str = file.display().to_string();
        let crate_name = loader::stem(&file_str);

        // Build the file silently via `mvl build`
        let build_output = process::Command::new(&mvl_bin)
            .args(["build", &file_str])
            .stdout(process::Stdio::null())
            .stderr(process::Stdio::piped())
            .output();

        match build_output {
            Ok(ref o) if o.status.success() => {}
            Ok(ref o) => {
                if verbose {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let lines: Vec<&str> = stderr.lines().collect();
                    let start = lines.len().saturating_sub(10);
                    eprintln!("  FAIL (build): {file_str}");
                    for line in &lines[start..] {
                        eprintln!("    {line}");
                    }
                } else {
                    eprintln!("  FAIL (build): {file_str}");
                }
                failed += 1;
                continue;
            }
            Err(e) => {
                eprintln!("  FAIL (build): {file_str}: {e}");
                failed += 1;
                continue;
            }
        }

        // Locate the compiled binary
        let binary = std::env::temp_dir()
            .join(format!("mvl_build_{compiler_version}_{crate_name}"))
            .join(&crate_name)
            .join("target")
            .join("debug")
            .join(&crate_name);

        if !binary.exists() {
            eprintln!("  FAIL (binary not found): {file_str}");
            eprintln!("    expected at: {}", binary.display());
            failed += 1;
            continue;
        }

        // Run the binary and capture stdout
        let output = match process::Command::new(&binary).output() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("  FAIL (run): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("  FAIL (exit {}): {file_str}", output.status);
            if verbose {
                for line in stderr.lines().take(10) {
                    eprintln!("    {line}");
                }
            }
            failed += 1;
            continue;
        }

        let actual = String::from_utf8_lossy(&output.stdout);
        let actual_trimmed = actual.trim_end_matches('\n');
        let expected_trimmed = expected.trim_end_matches('\n');

        let matched = if *is_pattern {
            lli::glob_match(expected_trimmed, actual_trimmed)
        } else {
            actual_trimmed == expected_trimmed
        };

        if matched {
            if verbose {
                println!("  PASS: {file_str}");
            } else if !quiet {
                print!(".");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
            passed += 1;
        } else {
            if !quiet {
                println!("\n  FAIL: {file_str}");
                if *is_pattern {
                    println!("    pattern:  {expected_trimmed:?}");
                } else {
                    println!("    expected: {expected_trimmed:?}");
                }
                println!("    got:      {actual_trimmed:?}");
            }
            failed += 1;
        }
    }

    if !quiet && !verbose {
        println!();
    }
    println!("{passed} passed, {failed} failed");
    if failed > 0 {
        process::exit(1);
    }
}

/// Native behavioral mutation testing (ADR-0014).
///
/// Execution model: single compile embeds all mutants behind `MVL_MUTANT` env-var
/// dispatch; N parallel test-binary runs determine which mutants are killed.
pub(super) fn find_test_binary_from_cargo_output(output: &[u8]) -> Option<std::path::PathBuf> {
    let text = String::from_utf8_lossy(output);
    for line in text.lines() {
        if line.contains(r#""compiler-artifact""#) && line.contains(r#""executable""#) {
            // Find `"executable":"<path>"` — Cargo JSON uses no spaces around `:`.
            if let Some(pos) = line.find(r#""executable":""#) {
                let rest = &line[pos + 14..]; // skip `"executable":"`
                if let Some(end) = rest.find('"') {
                    // Unescape backslash sequences on Windows paths
                    let raw = rest[..end].replace("\\\\", "\\");
                    return Some(std::path::PathBuf::from(raw));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod find_test_binary_tests {
    use super::find_test_binary_from_cargo_output;

    fn cargo_artifact_line(executable: &str) -> String {
        format!(
            r#"{{"reason":"compiler-artifact","package_id":"mvl_mutate 0.1.0","executable":"{executable}","features":[]}}"#
        )
    }

    #[test]
    fn happy_path_returns_path() {
        let line = cargo_artifact_line("/tmp/mvl_mutate/target/debug/mvl_mutate-abc123");
        let out = find_test_binary_from_cargo_output(line.as_bytes());
        assert_eq!(
            out.unwrap().to_str().unwrap(),
            "/tmp/mvl_mutate/target/debug/mvl_mutate-abc123"
        );
    }

    #[test]
    fn no_matching_line_returns_none() {
        let line = r#"{"reason":"build-script-executed","package_id":"foo"}"#;
        assert!(find_test_binary_from_cargo_output(line.as_bytes()).is_none());
    }

    #[test]
    fn empty_input_returns_none() {
        assert!(find_test_binary_from_cargo_output(b"").is_none());
    }

    #[test]
    fn compiler_artifact_without_executable_string_returns_none() {
        let line = r#"{"reason":"compiler-artifact","executable":null}"#;
        assert!(find_test_binary_from_cargo_output(line.as_bytes()).is_none());
    }

    #[test]
    fn first_matching_line_wins() {
        let line1 = cargo_artifact_line("/tmp/first");
        let line2 = cargo_artifact_line("/tmp/second");
        let input = format!("{line1}\n{line2}\n");
        let out = find_test_binary_from_cargo_output(input.as_bytes());
        assert_eq!(out.unwrap().to_str().unwrap(), "/tmp/first");
    }

    #[test]
    fn windows_backslash_unescaping() {
        let line = cargo_artifact_line("C:\\\\tmp\\\\mvl\\\\test.exe");
        let out = find_test_binary_from_cargo_output(line.as_bytes());
        assert_eq!(out.unwrap().to_str().unwrap(), "C:\\tmp\\mvl\\test.exe");
    }
}
