// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::llvm_text::lli;
use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::Decl;
use mvl::mvl::parser::Parser;
use mvl::mvl::pipeline::lower_prelude;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

/// Derive a Rust-identifier-safe module name from a test-file path (#1707).
///
/// The corpus contains files that share a stem in different directories
/// (e.g. `07_effects/propagation.mvl` + `08_ifc/propagation.mvl`).  A
/// stem-only derivation collides both onto `mod propagation`, so the
/// bundled crate rejects with E0428.  Qualifying by path segment makes
/// each fixture unique.
///
/// Rules:
///   - Strip a leading `./` if present.
///   - Strip the trailing `.mvl` extension.
///   - Replace `/`, `\`, `-`, `.` with `_` so the result is a valid Rust ident.
///   - Strip a trailing `_test` so `foo_test.mvl` and `foo.mvl` still collapse
///     onto the same module (deliberate — see the covered_stems dedup in
///     `run()`).
///   - If the first character is not a letter/underscore, prefix with `_`.
fn qualified_module_name(path: &str) -> String {
    let stem = path.strip_suffix(".mvl").unwrap_or(path);
    let stem = stem.strip_prefix("./").unwrap_or(stem);
    let mut name: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | '-' | '.' => '_',
            other => other,
        })
        .collect();
    if let Some(base) = name.strip_suffix("_test") {
        name = base.to_string();
    }
    if !name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        name.insert(0, '_');
    }
    // Rust `mod` identifiers must be snake_case — dev-on-macOS paths
    // contain `/Users/…` which would otherwise trigger `non_snake_case`
    // warnings on every generated module.  Case doesn't discriminate
    // between MVL source files (they're path-unique already), so
    // collapsing to lowercase is safe.
    name.make_ascii_lowercase();
    name
}

/// Returns true if the file contains the `// corpus:expect-fail` annotation.
///
/// These files are negative test cases for `mvl check` — they intentionally
/// contain violations (IFC, type, ownership, effects) that MUST cause the
/// checker to reject them.  `make test-corpus` handles them via the Makefile
/// annotation.  `mvl test` should skip them entirely: bundling them into the
/// test crate produces spurious rustc errors from code that MVL itself
/// declared invalid (#1707 phase 4).
fn is_expect_fail(path: &std::path::Path) -> bool {
    fs::read_to_string(path)
        .map(|s| s.contains("corpus:expect-fail"))
        .unwrap_or(false)
}

/// Returns true if every `test fn` in the file is a typecheck-only fixture —
/// name ends with `_typecheck` OR the entire body is a sequence of `touch(...)`
/// calls (the MVL corpus convention for a smoke test that verifies the
/// declarations parse and type-check but never runs).
///
/// `mvl test` transpiles fully to a Rust crate and links against the runtime.
/// Corpus files that reference undeclared types (`Channel`, `Buffer`,
/// `monitor`, etc.), rely on non-emitted stdlib methods (`IoError::user_message`),
/// or otherwise cannot round-trip to a runnable Rust program still parse and
/// type-check cleanly under MVL's own rules — that's what `make test-corpus`
/// exercises.  Trying to *compile* them as part of the shared test crate
/// produces cascading rustc errors from code that was never meant to run.
///
/// A body of only `touch(x)` (or `touch(&x); touch(&y);`) is the MVL idiom for
/// "reference this identifier so the linter records it as used; no runtime
/// behaviour intended."  Skipping such files is safe: `mvl check` (and thus
/// `make test-corpus`) still validates their contents (#1707 phase 8).
fn is_typecheck_only(prog: &mvl::mvl::parser::ast::Program) -> bool {
    use mvl::mvl::parser::ast::{Expr, Stmt};
    let mut saw_test = false;
    for decl in &prog.declarations {
        if let Decl::Fn(fd) = decl {
            if !fd.is_test {
                continue;
            }
            saw_test = true;
            if fd.name.ends_with("_typecheck") {
                continue;
            }
            // Fall back to a body-shape check: every statement is a `touch(...)` call.
            let all_touches = fd.body.stmts.iter().all(|stmt| {
                if let Stmt::Expr { expr, .. } = stmt {
                    matches!(
                        expr,
                        Expr::FnCall { name, .. } if name == "touch"
                    )
                } else {
                    false
                }
            });
            if !all_touches {
                return false;
            }
        }
    }
    saw_test
}

pub fn run(path: &str, quiet: bool, verbose: bool, coverage: bool, bdd: bool) {
    if quiet && verbose {
        eprintln!(
            "warning: --quiet and --verbose are mutually exclusive; --verbose takes precedence"
        );
    }
    let quiet = quiet && !verbose;

    let test_files: Vec<PathBuf> = loader::mvl_files(path, true) // test_only=true
        .into_iter()
        .filter(|f| !is_expect_fail(f))
        .collect();
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
        let module_name = qualified_module_name(&file_str);
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
    // Parallel array tracking each prelude entry's source file stem.  Library
    // files (`Some(stem)`) feed per-file coverage routing; stdlib/internal
    // entries stay `None`.  Used to instrument paired sibling files (#1489).
    let mut prelude_stems: Vec<Option<String>> = vec![None; stdlib_prelude_progs.len()];
    // Stems of sibling library files discovered next to test files.
    let mut sibling_stems: Vec<String> = Vec::new();
    // Split index between universal prelude entries (always emitted into every
    // file's transpile) and stdlib extras (per-file filtered).  Set inside the
    // pre-scan block below.
    let n_universal_prelude_outer: usize;
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
        //
        // `seen_pkgs` is shared with the post-sibling-discovery pass below so that a
        // package referenced by both a test file and a sibling library file is only
        // loaded once.
        let mut seen_pkgs = std::collections::HashSet::new();
        let mut pkg_progs: Vec<mvl::mvl::parser::ast::Program> = Vec::new();
        {
            let mut frontier = all_test_progs.clone();
            loop {
                let new_pkgs = loader::load_pkg_modules(&frontier, &project_root, &mut seen_pkgs);
                if new_pkgs.is_empty() {
                    break;
                }
                frontier = new_pkgs.clone();
                let n = new_pkgs.len();
                stdlib_prelude_progs.extend(new_pkgs.clone());
                prelude_stems.extend(std::iter::repeat_n(None, n));
                pkg_progs.extend(new_pkgs);
            }
        }

        // Pre-compute which module names are explicitly imported by test files so
        // that pure-function sibling modules (no types/extern blocks) are also
        // loaded into the prelude when a test file uses `use module::fn`.
        // This fixes the cross-module import limitation tracked in issue #96.
        //
        // Also fold in imports from entry-point files (those with `fn main`) sitting
        // next to the test files — they're loaded as siblings further down so their
        // pure-function dependencies must come along for the test crate to link
        // (#1489).
        let mut imported_by_test_files: std::collections::HashSet<String> = all_test_progs
            .iter()
            .flat_map(loader::collect_imported_module_names)
            .collect();
        // Function and type names declared in test files — entry-point files whose
        // symbols overlap with these are using a #96 workaround re-declaration and
        // must not be pulled into the prelude (it would cause type/signature conflicts).
        let test_decl_names: std::collections::HashSet<String> = all_test_progs
            .iter()
            .flat_map(|p| p.declarations.iter())
            .filter_map(|d| match d {
                Decl::Fn(f) => Some(f.name.clone()),
                Decl::Type(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect();
        for f in &test_files {
            let dir = f.parent().unwrap_or_else(|| std::path::Path::new("."));
            for scan_dir in [dir.to_path_buf(), dir.join("internal")] {
                let entries = match fs::read_dir(&scan_dir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let p = entry.path();
                    let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if p.extension().map(|e| e == "mvl").unwrap_or(false)
                        && !fname.contains("_test")
                    {
                        if let Ok(src) = fs::read_to_string(&p) {
                            let (mut pp, _) = Parser::new(&src);
                            let parsed = pp.parse_program();
                            let imports = loader::collect_imported_module_names(&parsed);
                            if transpiler::has_main_fn(&parsed) && !imports.is_empty() {
                                imported_by_test_files.extend(imports);
                            }
                        }
                    }
                }
            }
        }

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
                                // Load files that have extern blocks or type declarations.
                                // Also load pure-function files (no types/extern) only when a
                                // test file explicitly imports them via `use module::fn` — this
                                // resolves cross-module imports without risking shadowing of
                                // runtime primitives by unreferenced helpers (fix for #96).
                                //
                                // Entry-point files (those defining `fn main`) join the prelude
                                // when they `use` at least one non-stdlib module — i.e. the
                                // entry point is integrated with sibling library modules and
                                // not a standalone demo.  This makes complex `main.mvl` helpers
                                // visible in coverage (#1489) while keeping isolated demos
                                // (e.g. `access_smoke.mvl`, which has zero cross-module imports
                                // and re-declares the project's types) out of the test crate,
                                // where they'd collide with the integrated module's types.
                                let file_stem = p
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("")
                                    .to_owned();
                                // Post-#1714, `imported_by_test_files` holds
                                // dot-qualified module names (e.g.
                                // `backends.llvm.emit_program`).  Comparing
                                // the bare file_stem alone would fail to
                                // match any nested library file, so also
                                // derive the qualified stem relative to the
                                // CLI base directory and check both forms.
                                let qual_stem = loader::qualified_stem(
                                    std::path::Path::new(path),
                                    &p,
                                );
                                let is_entry_point = transpiler::has_main_fn(&parsed);
                                let entry_point_ok = !is_entry_point || {
                                    // Include an entry-point file only when it is integrated
                                    // (imports non-stdlib modules) AND its symbols don't
                                    // overlap with test-file re-declarations (#96 workaround).
                                    !loader::collect_imported_module_names(&parsed).is_empty()
                                        && !parsed.declarations.iter().any(|d| match d {
                                            Decl::Fn(f) if f.name != "main" => {
                                                test_decl_names.contains(&f.name)
                                            }
                                            Decl::Type(t) => test_decl_names.contains(&t.name),
                                            _ => false,
                                        })
                                };
                                if entry_point_ok
                                    && (transpiler::has_extern_or_type_decls(&parsed)
                                        || imported_by_test_files.contains(&file_stem)
                                        || imported_by_test_files.contains(&qual_stem))
                                {
                                    let stem = file_stem.replace('-', "_");
                                    sibling_progs.push(parsed.clone());
                                    // Package files (.mvl/pkg/) are tested independently and
                                    // must not appear in this project's coverage report (#1513).
                                    // Exclude their stems from instrumentation routing, but keep
                                    // the programs in sibling_progs so native bridge discovery
                                    // (load_mvl_native_stdlib_extras) still finds extern blocks.
                                    let under_dot_mvl =
                                        f.components().any(|c| c.as_os_str() == ".mvl");
                                    if !under_dot_mvl {
                                        sibling_stems.push(stem.clone());
                                    }
                                    stdlib_prelude_progs.push(parsed);
                                    prelude_stems.push(Some(stem));
                                }
                            }
                        }
                    }
                }
            }
        }
        // Second pkg.* frontier pass: load packages imported by sibling library
        // files (e.g. `db.mvl` imports `pkg.sqlite` but `db_test.mvl` does not).
        // Before #1520 these files were picked up incidentally from `.mvl/pkg/`
        // via recursive scans; with that path closed off, package source must
        // reach the test crate through `load_pkg_modules` for every import site,
        // not just the test files.
        {
            let mut frontier = sibling_progs.clone();
            loop {
                let new_pkgs = loader::load_pkg_modules(&frontier, &project_root, &mut seen_pkgs);
                if new_pkgs.is_empty() {
                    break;
                }
                frontier = new_pkgs.clone();
                let n = new_pkgs.len();
                stdlib_prelude_progs.extend(new_pkgs.clone());
                prelude_stems.extend(std::iter::repeat_n(None, n));
                pkg_progs.extend(new_pkgs);
            }
        }

        // Inline-test source files (regular `.mvl` with `test fn` decls) are
        // transpiled further down without appearing in `test_files`.  Their
        // `use std.X` imports must still reach `load_mvl_native_stdlib_extras`
        // below — otherwise pure-MVL stdlib modules referenced only by inline
        // tests (e.g. `std.actors`, `std.audit`, `std.log` used by the
        // `12_actors/` and `13_stdlib/` corpus) are never loaded into the
        // prelude, and the bundled test crate fails to compile with E0433 on
        // types like `RestartStrategy`, `AuditEvent`, `DeadLetter` (#1707
        // phase 3).
        let inline_test_source_progs: Vec<mvl::mvl::parser::ast::Program> =
            loader::mvl_files(path, false)
                .into_iter()
                .filter(|f| !is_expect_fail(f))
                .filter_map(|f| {
                    let (prog, _) = super::parse_or_exit(&f.display().to_string());
                    let has_test = prog
                        .declarations
                        .iter()
                        .any(|d| matches!(d, Decl::Fn(fd) if fd.is_test));
                    if !has_test || is_typecheck_only(&prog) {
                        return None;
                    }
                    Some(prog)
                })
                .collect();

        // Single pass over all programs (test files + sibling library files +
        // loaded pkg.* modules + inline-test source files) mirrors cli/build.rs
        // and ensures transitive stdlib deps are discovered together (#865).
        // pkg_progs must be included so pure-MVL stdlib imports inside packages
        // (e.g. pkg-trace's `use std.crypto.{uuid_v4}`) reach the extras loader.
        let all_for_extras: Vec<_> = all_test_progs
            .iter()
            .chain(sibling_progs.iter())
            .chain(pkg_progs.iter())
            .chain(inline_test_source_progs.iter())
            .cloned()
            .collect();
        let extras = loader::load_mvl_native_stdlib_extras(&all_for_extras);
        let n_extras = extras.len();
        stdlib_prelude_progs.extend(extras);
        prelude_stems.extend(std::iter::repeat_n(None, n_extras));
        n_universal_prelude_outer = stdlib_prelude_progs.len() - n_extras;
    }
    // Universal prelude = implicit + pkg + siblings. Everything at indices
    // >= n_universal_prelude is a stdlib extra, filtered per-file below so a
    // file that doesn't `use std.log` doesn't get `Logger` injected into its
    // mod (which would collide with a user-declared `actor Logger`, #1707
    // phase 3).
    let n_universal_prelude = n_universal_prelude_outer;

    // Collect native Cargo deps and bridge.rs from the full pkg.* closure so
    // that the test crate's Cargo.toml mirrors what `mvl build` would emit (#1481).
    // The frontier loop above loaded all transitive packages into stdlib_prelude_progs.
    let project_root = {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        super::find_project_root(&cwd)
    };
    let native_dep_lines =
        loader::collect_pkg_native_dep_lines(&stdlib_prelude_progs, &project_root);
    let pkg_bridge = loader::find_pkg_bridge(&stdlib_prelude_progs, &project_root);

    // Coverage routing for sibling library files (#1489).
    //
    // Library code is emitted into each test module's prelude.  By default it
    // carries no branch probes, hiding all `if`/`match` arms from the report.
    // To fix this, mark each library stem for instrumentation in exactly one
    // test module's transpile:
    //   * Paired siblings (`json.mvl` ↔ `json_test.mvl`) — instrument in the
    //     paired test module so coverage reflects its own test suite.
    //   * Unpaired helpers (`testing.mvl` with no `_test.mvl` partner) —
    //     instrument in the first test module's transpile; subsequent modules
    //     re-emit them uninstrumented (call hits land on the instrumented copy
    //     only when the first module's tests exercise them).
    let sibling_stems_set: std::collections::HashSet<String> =
        sibling_stems.iter().cloned().collect();
    let paired_test_stems: std::collections::HashSet<String> = test_files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let s = loader::stem(&file_str);
            s.strip_suffix("_test").unwrap_or(&s).replace('-', "_")
        })
        .collect();
    let unpaired_sibling_stems: Vec<String> = sibling_stems
        .iter()
        .filter(|s| !paired_test_stems.contains(*s))
        .cloned()
        .collect();
    let mut instrumented_stems: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut unpaired_emitted = false;

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
        let module_name = qualified_module_name(&file_str);
        // Bare stem (with `_test` stripped) — used only to intersect with
        // `sibling_stems_set` for coverage instrumentation routing.  The
        // qualified `module_name` above is the Rust identifier under which
        // this file will be emitted; the bare stem exists purely to pair a
        // `foo_test.mvl` with its sibling library `foo.mvl` (#1489).
        let s = loader::stem(&file_str);
        let bare_stem = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
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
        // Build a per-file prelude: always include the universal entries
        // (implicit + pkg + siblings) but scope stdlib extras to only the
        // modules THIS file — plus its siblings — transitively imports.
        // Including siblings is essential: sibling library code emitted into
        // the same mod may reference stdlib types (`Logger`, `IoError`, …)
        // even when the primary file doesn't `use std.X` directly.  Without
        // that, examples like `access_control/audit_test.mvl` fail with E0425
        // for `Logger` in `audit.mvl`'s emitted code (#1707 phase 3).
        let mut extras_scope: Vec<_> = stdlib_prelude_progs[..n_universal_prelude].to_vec();
        extras_scope.push(prog.clone());
        let file_extras = loader::load_mvl_native_stdlib_extras(&extras_scope);
        let file_prelude_progs: Vec<_> = stdlib_prelude_progs[..n_universal_prelude]
            .iter()
            .cloned()
            .chain(file_extras)
            .collect();
        let file_prelude_stems: Vec<Option<String>> = prelude_stems[..n_universal_prelude]
            .iter()
            .cloned()
            .chain(std::iter::repeat_n(
                None,
                file_prelude_progs.len() - n_universal_prelude,
            ))
            .collect();
        // Decide which sibling library stems to instrument in this test file's
        // transpile (#1489).  Each stem is marked at most once across the run.
        let mut instrument_this: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        if coverage {
            if sibling_stems_set.contains(&bare_stem)
                && instrumented_stems.insert(bare_stem.clone())
            {
                instrument_this.insert(bare_stem.clone());
            }
            if !unpaired_emitted {
                for s in &unpaired_sibling_stems {
                    if instrumented_stems.insert(s.clone()) {
                        instrument_this.insert(s.clone());
                        file_stems.push(s.clone());
                    }
                }
                unpaired_emitted = true;
            }
        }
        let (out, branches) = if coverage {
            {
                let r = transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_file_stem(&module_name)
                        .with_prelude(lower_prelude(&file_prelude_progs))
                        .with_coverage_prelude(file_prelude_stems, instrument_this)
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
                        .with_prelude(lower_prelude(&file_prelude_progs))
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
        // Package test files (.mvl/pkg/) must not appear in this project's coverage
        // report — their source is tracked by the packages themselves (#1513).
        let under_dot_mvl = test_file.components().any(|c| c.as_os_str() == ".mvl");
        if !under_dot_mvl {
            file_stems.push(module_name.clone());
        }
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
    //
    // Build the dedup set from the emitted modules (not `file_stems`) — pkg.*
    // test files are excluded from `file_stems` for coverage reasons (#1513),
    // but their module names still occupy the test crate's namespace, so a
    // sibling `core.mvl` with inline tests must not be re-emitted as a second
    // `mod core` (which would collide on the Rust side).
    let covered_stems: std::collections::HashSet<String> =
        modules.iter().map(|(m, _, _)| m.clone()).collect();
    let source_files: Vec<PathBuf> = loader::mvl_files(path, false)
        .into_iter()
        .filter(|f| !is_expect_fail(f))
        .collect();
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let module_name = qualified_module_name(&file_str);
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
        // Skip files whose test fns are purely typecheck-only — see
        // `is_typecheck_only` (#1707 phase 8).  `make test-corpus` still
        // validates them via `mvl check`; bundling them into the shared test
        // crate only surfaces spurious rustc errors from code that MVL
        // considers valid but references undeclared / runtime-only symbols.
        if is_typecheck_only(&prog) {
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
        // Per-file prelude filtering — see the main test-file loop above for
        // rationale (#1707 phase 3).
        let mut extras_scope: Vec<_> = stdlib_prelude_progs[..n_universal_prelude].to_vec();
        extras_scope.push(prog.clone());
        let file_extras = loader::load_mvl_native_stdlib_extras(&extras_scope);
        let file_prelude_progs: Vec<_> = stdlib_prelude_progs[..n_universal_prelude]
            .iter()
            .cloned()
            .chain(file_extras)
            .collect();
        let (out, branches) = if coverage {
            {
                let r = transpiler::transpile(
                    &tir,
                    transpiler::TranspileConfig::new(&module_name)
                        .with_file_stem(&module_name)
                        .with_prelude(lower_prelude(&file_prelude_progs))
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
                        .with_prelude(lower_prelude(&file_prelude_progs))
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
    // non_snake_case: module names are derived from file paths (e.g.
    // `_Users_foo_bar_config`) which may contain uppercase letters.
    combined_rs.push_str(
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe, unused_assignments, non_shorthand_field_patterns, unpredictable_function_pointer_comparisons, non_snake_case)]\n\n",
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
    //
    // `mvl test` wraps each test file's transpiled output in `#[cfg(test)] mod
    // <name> { ... }`, so pkg.* types like `Terminal` end up at
    // `crate::<mod>::Terminal` — not at the crate root where `mvl build` puts
    // them.  A pkg-provided bridge.rs that imports `use crate::Terminal;` (the
    // documented pattern) would fail to resolve.  Re-export public type
    // declarations from the pkg.* closure at the crate root via the first test
    // module so those imports resolve.  Both `mod bridge;` and the re-export
    // are gated with `#[cfg(test)]` to match the test mods' gating — the test
    // crate is only ever `cargo test`'d, so this loses no functionality.
    if let Some(ref bp) = pkg_bridge {
        fs::copy(bp, src_dir.join("bridge.rs")).unwrap_or_else(|e| {
            eprintln!("Cannot copy bridge.rs: {e}");
            process::exit(1);
        });

        // Collect public type names from the bridge-owning package by parsing
        // its `src/` and `src/internal/` .mvl files.  We start from the bridge
        // path (its parent is the package root) rather than `pkg_progs` because
        // the frontier loop's seed (`all_test_progs`) only catches packages
        // imported directly by `*_test.mvl` files — a package imported via a
        // sibling source file (e.g. `input.mvl`) is loaded into the prelude
        // through a different code path and would be missed here.
        let pkg_type_names: Vec<String> = {
            let pkg_root = bp.parent().map(std::path::Path::to_path_buf);
            let mut names: Vec<String> = Vec::new();
            if let Some(pkg_root) = pkg_root {
                for sub in &["src", "src/internal"] {
                    let dir = pkg_root.join(sub);
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.extension().map(|e| e == "mvl").unwrap_or(false) {
                                if let Ok(src) = fs::read_to_string(&path) {
                                    let (mut p, _) = Parser::new(&src);
                                    let parsed = p.parse_program();
                                    for decl in &parsed.declarations {
                                        if let Decl::Type(t) = decl {
                                            if t.visible {
                                                names.push(t.name.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            names.sort();
            names.dedup();
            names
        };
        let first_mod_name = modules.first().map(|(m, _, _)| m.clone());
        let reexport_line = match (first_mod_name, pkg_type_names.is_empty()) {
            (Some(m), false) => {
                let joined = pkg_type_names.join(", ");
                format!("#[cfg(test)] pub use {m}::{{{joined}}};\n")
            }
            _ => String::new(),
        };

        let mut injected = String::with_capacity(combined_rs.len() + 64);
        let mut done = false;
        for line in combined_rs.lines() {
            injected.push_str(line);
            injected.push('\n');
            if !done && line.trim_start().starts_with("#![allow(") {
                injected.push_str("\n#[cfg(test)] mod bridge;\n");
                injected.push_str(&reexport_line);
                done = true;
            }
        }
        if !done {
            // No inner-attribute line found — fall back to prepending after the
            // header comments by simply appending at the start.
            injected = format!("#[cfg(test)] mod bridge;\n{reexport_line}{combined_rs}");
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

    // Clean stale native-dep OUT_DIR artifacts before running cargo.
    //
    // The `cargo_target_dir` is shared across runs (keyed on input path hash)
    // so that compiled crates are cached for faster subsequent runs.  However,
    // if a previous build partially succeeded — e.g. the C library compiled but
    // the Rust FFI bindings (bindgen.rs / bindings.rs) were never written — the
    // OUT_DIR inside `debug/build/` persists with an incomplete artifact set.
    // On the next run cargo finds the matching artifact hash, skips recompilation,
    // and fails with a cryptic error like `couldn't read .../out/bindgen.rs`.
    //
    // Removing `debug/build/` forces cargo to re-run all build scripts cleanly
    // while keeping every already-compiled `.rlib` / `.rmeta` in `debug/deps/`
    // so dependency compilation is still cached.
    let stale_build_dir = cargo_target_dir.join("debug").join("build");
    if stale_build_dir.exists() {
        fs::remove_dir_all(&stale_build_dir).unwrap_or_else(|e| {
            eprintln!(
                "warning: could not clean stale build cache {}: {e}",
                stale_build_dir.display()
            );
        });
    }

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

    // Parallelize across worker threads.  Each case is an independent
    // `mvl build` + run — no shared state, no ordering.  Cargo's per-crate
    // temp dir is derived from the file stem so builds don't collide.
    // Matches the pattern used by `llvm_text::cmd_test_llvm_text` and
    // `mutate::run` (#1699).  Output is buffered per-case and flushed in
    // input order after all workers finish so `--verbose` PASS/FAIL
    // reporting stays deterministic.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(test_cases.len());
    let chunk_size = test_cases.len().div_ceil(parallelism).max(1);

    if !quiet && verbose {
        println!(
            "Rust transpiler: dispatching {} case(s) across {} worker(s)",
            test_cases.len(),
            parallelism
        );
    }

    let mvl_bin_ref: &Path = &mvl_bin;
    let results: Vec<ExpectCaseResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = test_cases
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(f, e, p)| {
                            run_one_expect_case(mvl_bin_ref, compiler_version, f, e, *p, verbose)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("rust expect-test worker panicked"))
            .collect()
    });

    let mut passed = 0usize;
    let mut failed = 0usize;
    for r in &results {
        if r.passed {
            passed += 1;
            if verbose {
                if !r.stdout.is_empty() {
                    print!("{}", r.stdout);
                }
            } else if !quiet {
                print!(".");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
        } else {
            failed += 1;
            if !r.stderr.is_empty() {
                eprint!("{}", r.stderr);
            }
            if !r.stdout.is_empty() {
                print!("{}", r.stdout);
            }
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

/// Result of one `run_expect_tests` worker.  Buffered so main-thread output
/// stays in the caller's file order.
struct ExpectCaseResult {
    passed: bool,
    stdout: String,
    stderr: String,
}

/// Run a single `// expect:` case through the Rust transpiler backend.
///
/// Pure function of its inputs — invoked concurrently by
/// [`run_expect_tests`] via `std::thread::scope`.  Output is buffered into
/// the returned struct rather than written directly so the main thread can
/// serialize final PASS/FAIL reporting deterministically.
fn run_one_expect_case(
    mvl_bin: &Path,
    compiler_version: &str,
    file: &Path,
    expected: &str,
    is_pattern: bool,
    verbose: bool,
) -> ExpectCaseResult {
    let file_str = file.display().to_string();
    let crate_name = loader::stem(&file_str);
    let mut stdout = String::new();
    let mut stderr = String::new();

    // Build the file silently via `mvl build`
    let build_output = process::Command::new(mvl_bin)
        .args(["build", &file_str])
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::piped())
        .output();

    match build_output {
        Ok(ref o) if o.status.success() => {}
        Ok(ref o) => {
            if verbose {
                let err = String::from_utf8_lossy(&o.stderr);
                let lines: Vec<&str> = err.lines().collect();
                let start = lines.len().saturating_sub(10);
                stderr.push_str(&format!("  FAIL (build): {file_str}\n"));
                for line in &lines[start..] {
                    stderr.push_str(&format!("    {line}\n"));
                }
            } else {
                stderr.push_str(&format!("  FAIL (build): {file_str}\n"));
            }
            return ExpectCaseResult {
                passed: false,
                stdout,
                stderr,
            };
        }
        Err(e) => {
            stderr.push_str(&format!("  FAIL (build): {file_str}: {e}\n"));
            return ExpectCaseResult {
                passed: false,
                stdout,
                stderr,
            };
        }
    }

    let binary = std::env::temp_dir()
        .join(format!("mvl_build_{compiler_version}_{crate_name}"))
        .join(&crate_name)
        .join("target")
        .join("debug")
        .join(&crate_name);

    if !binary.exists() {
        stderr.push_str(&format!("  FAIL (binary not found): {file_str}\n"));
        stderr.push_str(&format!("    expected at: {}\n", binary.display()));
        return ExpectCaseResult {
            passed: false,
            stdout,
            stderr,
        };
    }

    let output = match process::Command::new(&binary).output() {
        Ok(o) => o,
        Err(e) => {
            stderr.push_str(&format!("  FAIL (run): {file_str}: {e}\n"));
            return ExpectCaseResult {
                passed: false,
                stdout,
                stderr,
            };
        }
    };

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        stderr.push_str(&format!("  FAIL (exit {}): {file_str}\n", output.status));
        if verbose {
            for line in err.lines().take(10) {
                stderr.push_str(&format!("    {line}\n"));
            }
        }
        return ExpectCaseResult {
            passed: false,
            stdout,
            stderr,
        };
    }

    let actual = String::from_utf8_lossy(&output.stdout);
    let actual_trimmed = actual.trim_end_matches('\n');
    let expected_trimmed = expected.trim_end_matches('\n');
    let matched = if is_pattern {
        lli::glob_match(expected_trimmed, actual_trimmed)
    } else {
        actual_trimmed == expected_trimmed
    };

    if matched {
        if verbose {
            stdout.push_str(&format!("  PASS: {file_str}\n"));
        }
        ExpectCaseResult {
            passed: true,
            stdout,
            stderr,
        }
    } else {
        stdout.push_str(&format!("\n  FAIL: {file_str}\n"));
        if is_pattern {
            stdout.push_str(&format!("    pattern:  {expected_trimmed:?}\n"));
        } else {
            stdout.push_str(&format!("    expected: {expected_trimmed:?}\n"));
        }
        stdout.push_str(&format!("    got:      {actual_trimmed:?}\n"));
        ExpectCaseResult {
            passed: false,
            stdout,
            stderr,
        }
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
mod qualified_module_name_tests {
    use super::qualified_module_name;

    #[test]
    fn same_stem_different_dirs_are_distinct() {
        // The bug fixed here (#1707 phase 1): two corpus files with the same
        // filename in different subdirs used to collapse onto one `mod` name.
        let a = qualified_module_name("tests/corpus/07_effects/propagation.mvl");
        let b = qualified_module_name("tests/corpus/08_ifc/propagation.mvl");
        assert_ne!(a, b);
        assert_eq!(a, "tests_corpus_07_effects_propagation");
        assert_eq!(b, "tests_corpus_08_ifc_propagation");
    }

    #[test]
    fn foo_test_and_foo_still_collapse() {
        // Deliberate: covered_stems dedup treats foo_test.mvl and foo.mvl as
        // the same module (#96 workaround — the sibling library file is
        // re-declared inside the test file). Preserving that behaviour.
        let a = qualified_module_name("tests/mymod/foo_test.mvl");
        let b = qualified_module_name("tests/mymod/foo.mvl");
        assert_eq!(a, b);
        assert_eq!(a, "tests_mymod_foo");
    }

    #[test]
    fn leading_digit_dir_gets_underscore_prefix() {
        // If cwd is inside tests/corpus, paths look like `07_effects/...`.
        let n = qualified_module_name("07_effects/propagation.mvl");
        assert!(n.starts_with('_'), "must be a valid Rust ident");
        assert_eq!(n, "_07_effects_propagation");
    }

    #[test]
    fn hyphens_and_dots_become_underscores() {
        let n = qualified_module_name("dir/some-file.name.mvl");
        assert_eq!(n, "dir_some_file_name");
    }

    #[test]
    fn leading_dot_slash_is_stripped() {
        let n = qualified_module_name("./tests/foo.mvl");
        assert_eq!(n, "tests_foo");
    }

    #[test]
    fn absolute_path_with_uppercase_dirs_lowercased() {
        // Dev-on-macOS: paths like `/Users/xyz/...` would otherwise leak
        // uppercase letters into the generated `mod` name and trigger
        // `warning: module `_Users_...` should have a snake case name`
        // on every test run.
        let n = qualified_module_name("/Users/dev/wc/foo/bar.mvl");
        assert_eq!(n, "_users_dev_wc_foo_bar");
    }
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
