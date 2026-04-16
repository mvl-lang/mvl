use mvl::mvl::checker;
use mvl::mvl::checker::passes::{
    aggregate_verdicts, parse_req_filter, source_hash, PassRegistry, Verdict, VerdictCache,
};
use mvl::mvl::linter::{self, config::LintConfig};
use mvl::mvl::parser::ast::{Decl, Program, Totality, TypeBody};
use mvl::mvl::parser::Parser;
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
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
            let req_filter = parse_req_filter_or_exit(&args);
            cmd_check(&path, req_filter);
        }
        "build" => {
            let path = require_path_arg(&args, "build");
            build_project(&path, false, &[]);
        }
        "run" => {
            let path = require_path_arg(&args, "run");
            let path_idx = path_arg_index(&args);
            let run_args: Vec<String> = args[path_idx + 1..]
                .iter()
                .skip_while(|a| a.as_str() != "--")
                .skip(1)
                .cloned()
                .collect();
            build_project(&path, true, &run_args);
        }
        "transpile" => {
            let path = require_path_arg(&args, "transpile");
            cmd_transpile(&path);
        }
        "test" => {
            let path = require_path_arg(&args, "test");
            cmd_test(&path);
        }
        "lint" => {
            let path = require_path_arg(&args, "lint");
            let show_config = args.iter().any(|a| a == "--show-config");
            cmd_lint(&path, show_config);
        }
        "assurance" => {
            let path = require_path_arg(&args, "assurance");
            let json = args.iter().any(|a| a == "--format=json" || a == "--json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            cmd_assurance(&path, json, verbose);
        }
        "init" => {
            // Accept optional --stdlib flag (as documented in ADR-0009); it is the
            // only init target for now so the flag is accepted but not required.
            cmd_init();
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
    eprintln!("  mvl check <file|dir> --req <N>     — run only the Req N verification pass");
    eprintln!("  mvl build <file|dir>               — transpile to Rust and run cargo build");
    eprintln!("  mvl run   [--] <file.mvl>          — transpile, build, and execute");
    eprintln!("  mvl run   [--] <file.mvl> -- ...   — pass args to the compiled binary");
    eprintln!("  mvl test  <file|dir>               — find *_test.mvl files and run cargo test");
    eprintln!("  mvl lint  <file|dir>               — check style rules");
    eprintln!("  mvl lint  <file|dir> --show-config — show active linter configuration");
    eprintln!("  mvl assurance <file|dir>           — emit assurance report");
    eprintln!("  mvl assurance <file|dir> --json    — emit assurance report as JSON");
    eprintln!("  mvl assurance <file|dir> --verbose — per-function requirement detail");
    eprintln!("  mvl transpile <file.mvl>           — print transpiled Rust to stdout");
    eprintln!("  mvl init [--stdlib]                — extract stdlib to XDG_DATA_HOME/mvl/std/");
}

fn cmd_init() {
    let path = stdlib::ensure_stdlib();
    println!(
        "mvl stdlib v{} ready at {}",
        stdlib::STDLIB_VERSION,
        path.display()
    );
}

/// Escape a string for embedding in a JSON string literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Parse an optional `--req N` or `--req=N` flag; exits on invalid input.
fn parse_req_filter_or_exit(args: &[String]) -> Option<u8> {
    parse_req_filter(args).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    })
}

/// Returns the index of the path argument, skipping an optional `--` separator.
fn path_arg_index(args: &[String]) -> usize {
    if args.get(2).map(|s| s.as_str()) == Some("--") {
        3
    } else {
        2
    }
}

fn require_path_arg(args: &[String], cmd: &str) -> String {
    let idx = path_arg_index(args);
    if args.len() <= idx {
        eprintln!("Usage: mvl {cmd} [--] <file.mvl|directory>");
        process::exit(1);
    }
    args[idx].clone()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Parse and type-check a .mvl file or all .mvl files in a directory.
///
/// When `req_filter` is `Some(N)`, only the verification pass for Req N is run
/// and its verdict is printed; errors for other requirements are suppressed.
fn cmd_check(path: &str, req_filter: Option<u8>) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let stdlib_dir = stdlib::ensure_stdlib();

    // Parse all files once so we can pass them to both the resolver and the checker.
    let mut parsed: Vec<(String, Program, String)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, src) = parse_or_exit(&file_str);
            (file_str, prog, src)
        })
        .collect();

    // When checking a single file, also load imported sibling modules so the
    // resolver can validate cross-module imports (mirrors build_project behaviour).
    // Track how many entries are "requested" vs "resolver-only" siblings.
    let check_count = parsed.len();
    if Path::new(path).is_file() {
        let already_loaded: std::collections::HashSet<String> =
            parsed.iter().map(|(f, _, _)| stem(f)).collect();
        let entry_dir = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
        if let Some((_, entry_prog, _)) = parsed.first() {
            let extra_mods = collect_imported_module_names(entry_prog);
            for mod_name in extra_mods {
                if already_loaded.contains(&mod_name) {
                    continue;
                }
                let sib_path = entry_dir.join(format!("{mod_name}.mvl"));
                if sib_path.exists() {
                    let sib_str = sib_path.display().to_string();
                    let (sib_prog, sib_src) = parse_or_exit(&sib_str);
                    parsed.push((sib_str, sib_prog, sib_src));
                }
            }
        }
    }

    // Run the module resolver across all files, wiring in the extracted stdlib.
    let modules: Vec<(String, Program)> = parsed
        .iter()
        .map(|(file_str, prog, _)| (stem(file_str), prog.clone()))
        .collect();
    let resolve_result = resolver::resolve_project(modules, Some(&stdlib_dir));
    let mut had_errors = !resolve_result.is_ok();
    for err in &resolve_result.errors {
        eprintln!("error[resolver]: {err}");
    }

    let registry = PassRegistry::default_registry();

    // Pre-parse stdlib files imported by user programs so the checker knows
    // about their types and functions.  This covers `use std.io.{...}` etc.
    let prelude = load_stdlib_prelude(
        parsed.iter().take(check_count).map(|(_, p, _)| p),
        &stdlib_dir,
    );

    // Only run the checker on explicitly requested files (not resolver-only siblings).
    for (file_str, prog, _src) in parsed.iter().take(check_count) {
        let result = checker::check_with_prelude(&prelude, prog);

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
                for err in &result.errors {
                    eprintln!(
                        "{}:{}:{}: error[req{}]: {}",
                        file_str,
                        err.span().line,
                        err.span().col,
                        err.requirement_number(),
                        err.message()
                    );
                }
                let failed = (1u8..=11)
                    .filter(|&i| verdicts[i as usize].is_failed())
                    .count();
                eprintln!("{file_str}: FAIL  ({proven}/11 proven, {failed} failed)");
            }
        }
    }

    if had_errors {
        process::exit(1);
    }
}

/// Lint a .mvl file or all .mvl files in a directory for style violations.
fn cmd_lint(path: &str, show_config: bool) {
    // Resolve project root: directory of the path arg, or cwd for dirs.
    let project_root = {
        let p = Path::new(path);
        if p.is_file() {
            p.parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            p.to_path_buf()
        }
    };

    let cfg = LintConfig::load(&project_root);

    if show_config {
        if let Some(f) = LintConfig::config_file(&project_root) {
            eprintln!("config: {}", f.display());
        } else {
            eprintln!("config: <defaults — no .mvllintrc or XDG config found>");
        }
        eprintln!("  [phase-1: style]");
        eprintln!("  line_length          = {}", cfg.line_length);
        eprintln!("  indent_size          = {}", cfg.indent_size);
        eprintln!(
            "  indent_style         = {}",
            if cfg.indent_spaces { "spaces" } else { "tabs" }
        );
        eprintln!("  max_fn_length        = {}", cfg.max_fn_length);
        eprintln!("  naming               = {}", cfg.naming);
        eprintln!("  trailing_ws          = {}", cfg.trailing_ws);
        eprintln!("  unused_bindings      = {}", cfg.unused_bindings);
        eprintln!("  [phase-2: semantic]");
        eprintln!("  unreachable_code     = {}", cfg.unreachable_code);
        eprintln!("  redundant_match      = {}", cfg.redundant_match);
        eprintln!(
            "  unnecessary_annotations = {}",
            cfg.unnecessary_annotations
        );
        eprintln!("  redundant_effects    = {}", cfg.redundant_effects);
        eprintln!("  redundant_ifc_labels = {}", cfg.redundant_ifc_labels);
        eprintln!("  [phase-3: llm corpus quality]");
        eprintln!(
            "  consistent_comment_style = {}",
            cfg.consistent_comment_style
        );
        eprintln!("  require_doc_comments = {}", cfg.require_doc_comments);
        eprintln!("  doc_comment_examples = {}", cfg.doc_comment_examples);
        eprintln!("  [phase-4: complexity]");
        eprintln!(
            "  max_cyclomatic_complexity  = {}",
            cfg.max_cyclomatic_complexity
        );
        eprintln!(
            "  max_nested_match_depth     = {}",
            cfg.max_nested_match_depth
        );
        eprintln!(
            "  max_effect_signature_width = {}",
            cfg.max_effect_signature_width
        );
        eprintln!(
            "  max_trait_impl_count       = {}",
            cfg.max_trait_impl_count
        );
        eprintln!("  max_module_fanout          = {}", cfg.max_module_fanout);
        eprintln!("  max_extern_ratio           = {:.2}", cfg.max_extern_ratio);
        return;
    }

    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut total_warnings = 0usize;
    let mut total_errors = 0usize;
    let mut had_errors = false;

    for file in &files {
        let file_str = file.display().to_string();
        let (prog, src) = parse_or_exit(&file_str);
        let result = linter::lint(&prog, &src, &cfg);

        for diag in &result.diags {
            eprintln!("{}", diag.render(&file_str));
        }

        total_warnings += result.warning_count();
        total_errors += result.error_count();

        if !result.is_ok() {
            had_errors = true;
        } else if result.diags.is_empty() {
            println!("{file_str}: OK");
        }
    }

    if files.len() > 1 {
        eprintln!(
            "\n{} warning(s), {} error(s) across {} file(s)",
            total_warnings,
            total_errors,
            files.len()
        );
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

/// Inject `mod bridge;` into generated Rust source on the line immediately
/// following the `use mvl_runtime::prelude::*;` import.
///
/// Fallback: prepends `mod bridge;\n` when the marker line is absent (should
/// not occur in normal codegen, but is exercised by the compiler path).
///
/// Pure function — no I/O.
fn inject_mod_bridge(source: &str) -> String {
    const MARKER: &str = "use mvl_runtime::prelude::*;";
    let mut result = String::with_capacity(source.len() + 20);
    let mut injected = false;
    for line in source.lines() {
        result.push_str(line);
        result.push('\n');
        if !injected && line.trim() == MARKER {
            result.push_str("mod bridge;\n");
            injected = true;
        }
    }
    if !injected {
        // Fallback: marker absent — prepend mod bridge;
        let mut fallback = String::with_capacity(result.len() + 20);
        fallback.push_str("mod bridge;\n");
        fallback.push_str(&result);
        return fallback;
    }
    result
}

/// Transpile a .mvl file to a Cargo project, build it, and optionally run it.
///
/// `run_args` are forwarded to the compiled binary when `run` is true; the
/// binary is executed with its working directory set to the source file's
/// parent directory so that relative paths in args (e.g. `--file logs.jsonl`)
/// resolve correctly.
fn build_project(path: &str, run: bool, run_args: &[String]) {
    let stdlib_dir = stdlib::ensure_stdlib();
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

    // Collect sibling modules referenced via `use module::item` declarations.
    // Only load files that are actually imported — not all .mvl files in the directory.
    let entry_dir = Path::new(&file_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let imported_mod_names = collect_imported_module_names(&prog);
    let mut sibling_modules: Vec<(String, mvl::mvl::parser::ast::Program)> = imported_mod_names
        .into_iter()
        .filter_map(|mod_name| {
            let sib_path = entry_dir.join(format!("{mod_name}.mvl"));
            if !sib_path.exists() {
                return None;
            }
            let (sib_prog, _) = parse_or_exit(&sib_path.display().to_string());
            Some((mod_name, sib_prog))
        })
        .collect();
    sibling_modules.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Run module resolver to validate `use` imports across all modules.
    let mut all_modules = vec![(crate_name.clone(), prog.clone())];
    all_modules.extend(sibling_modules.iter().cloned());
    let resolve_result = resolver::resolve_project(all_modules, Some(&stdlib_dir));
    if !resolve_result.is_ok() {
        for err in &resolve_result.errors {
            eprintln!("error[resolver]: {err}");
        }
        process::exit(1);
    }

    let out = transpiler::transpile_project(&crate_name, &prog, &sibling_modules);

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

    // Detect a sibling bridge.rs — Rust implementations of extern "rust" fns.
    // Use canonicalize directly (no exists() pre-check) to eliminate the TOCTOU
    // race window. NotFound → no bridge. Any other error → hard fail.
    // Validate that the resolved path stays inside the source directory (symlink-escape guard).
    let mvl_dir = Path::new(&file_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let bridge_candidate = mvl_dir.join("bridge.rs");
    let bridge_path: Option<PathBuf> = match fs::canonicalize(&bridge_candidate) {
        Ok(canon_bridge) => {
            let canon_dir = fs::canonicalize(mvl_dir).unwrap_or_else(|e| {
                eprintln!("error: cannot canonicalize {}: {e}", mvl_dir.display());
                process::exit(1);
            });
            if !canon_bridge.starts_with(&canon_dir) {
                eprintln!("error: bridge.rs is outside source directory — refusing to copy",);
                process::exit(1);
            }
            Some(canon_bridge)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!(
                "error: cannot resolve bridge.rs at {}: {e}",
                bridge_candidate.display()
            );
            process::exit(1);
        }
    };

    if out.has_extern_rust && bridge_path.is_none() {
        eprintln!(
            "error: bridge.rs not found — {file_path} declares extern \"rust\" blocks but no bridge.rs exists at {}",
            bridge_candidate.display()
        );
        eprintln!("  Create bridge.rs with `pub extern \"Rust\" fn` implementations to link.");
        process::exit(1);
    }

    // Inject `mod bridge;` after `use mvl_runtime::prelude::*;`.
    let main_source = if bridge_path.is_some() {
        inject_mod_bridge(&out.main_rs)
    } else {
        out.main_rs
    };

    if out.has_main {
        // Binary crate: the transpiled code IS src/main.rs
        fs::write(src_dir.join("main.rs"), &main_source).unwrap_or_else(|e| {
            eprintln!("Cannot write main.rs: {e}");
            process::exit(1);
        });
    } else {
        // Library crate: lib.rs + a stub main for cargo build to succeed
        fs::write(src_dir.join("lib.rs"), &main_source).unwrap_or_else(|e| {
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

    // Write each sibling module as src/{name}.rs so `pub mod name;` resolves.
    for (mod_name, mod_source) in &out.module_files {
        fs::write(src_dir.join(format!("{mod_name}.rs")), mod_source).unwrap_or_else(|e| {
            eprintln!("Cannot write {mod_name}.rs: {e}");
            process::exit(1);
        });
    }

    // Copy bridge.rs into src/ so `mod bridge;` resolves.
    // Use fs::copy (single syscall) to avoid the read→write TOCTOU window.
    if let Some(ref bp) = bridge_path {
        fs::copy(bp, src_dir.join("bridge.rs")).unwrap_or_else(|e| {
            eprintln!("Cannot copy bridge.rs: {e}");
            process::exit(1);
        });
    }

    // If the program uses mvl_runtime, copy it as a sibling directory
    // so the relative path `../mvl_runtime` in Cargo.toml resolves.
    // Always overwrite so stale cached copies don't hide runtime changes.
    if out.has_extern_rust {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mvl_runtime");
        let runtime_dst = tmp_dir.parent().unwrap().join("mvl_runtime");
        if runtime_src.exists() {
            if runtime_dst.exists() {
                fs::remove_dir_all(&runtime_dst).expect("remove stale mvl_runtime");
            }
            copy_dir_recursive(&runtime_src, &runtime_dst).expect("copy mvl_runtime");
        }
    }

    println!("Transpiled to: {}", tmp_dir.display());
    println!("Running: cargo build");

    let build_status = process::Command::new("cargo")
        .arg("build")
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

    if !build_status.success() {
        eprintln!("cargo build failed");
        process::exit(1);
    }

    if run && out.has_main {
        // Run the binary with the source file's parent as working dir so that
        // relative file paths in run_args (e.g. --file logs.jsonl) resolve
        // against where the user invoked `mvl run`, not the tmp build dir.
        let binary = tmp_dir.join("target").join("debug").join(&crate_name);
        let source_dir = Path::new(&file_path)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let run_status = process::Command::new(&binary)
            .args(run_args)
            .current_dir(&source_dir)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("error: failed to run {}: {e}", binary.display());
                process::exit(1);
            });
        if !run_status.success() {
            process::exit(run_status.code().unwrap_or(1));
        }
    } else {
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
    let stdlib_dir = stdlib::ensure_stdlib();
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    // Run the module resolver to surface `use` errors before reporting.
    let modules: Vec<(String, Program)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, _) = parse_or_exit(&file_str);
            (stem(&file_str), prog)
        })
        .collect();
    let resolve_result = resolver::resolve_project(modules, Some(&stdlib_dir));
    for err in &resolve_result.errors {
        eprintln!("error[resolver]: {err}");
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
    // Verification pass infrastructure.
    let registry = PassRegistry::default_registry();
    let mut verdict_cache = VerdictCache::default();
    let mut per_file_verdicts: Vec<[Verdict; 12]> = Vec::new();

    let parsed_assurance: Vec<(String, Program, String)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let (prog, src) = parse_or_exit(&file_str);
            (file_str, prog, src)
        })
        .collect();
    let assurance_prelude =
        load_stdlib_prelude(parsed_assurance.iter().map(|(_, p, _)| p), &stdlib_dir);

    for (file_str, prog, src) in &parsed_assurance {
        let file_str = file_str.as_str();
        let stats = collect_assurance_stats(prog, verbose);
        let result = checker::check_with_prelude(&assurance_prelude, prog);

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

        // Run verification passes (with incremental cache).
        let hash = source_hash(src);
        let file_path = Path::new(file_str);
        let verdicts = if let Some(cached) = verdict_cache.get(file_path, hash) {
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
                    json_escape(v.detail())
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
        println!("Files checked:       {file_count}");
        println!("Functions:           {total_fns}");
        println!("  total fn:          {total_verified} ({verified_pct}% of implemented)");
        println!("  partial fn:        {total_partial}");
        println!("  extern fn:         {total_extern} ({extern_pct}% trust boundary)");
        println!("  implemented:       {implemented}");
        println!("  test fn:           {total_test_fns}");
        println!();
        println!("Requirements verified:  {proven_count}/11 proven");
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
        println!("Prover verdicts:");
        for req in 1u8..=11 {
            let v = &project_verdicts[req as usize];
            let name = registry.pass_name(req).unwrap_or("unknown");
            println!(
                "  Req {:>2}  {:<20} {}  {}",
                req,
                name,
                v.status_char(),
                v.detail()
            );
        }
        println!();
        println!("  ✓ proven  ✗ failed  ~ unchecked (Phase 3 prover pending)");
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

/// Collect unique top-level module names referenced by `use` declarations in `prog`.
///
/// For `use utils::clamp_display;` this returns `"utils"`.
/// The `std` namespace is excluded — it is provided by the runtime, not a sibling file.
/// Parse the stdlib files imported by the given programs and return them as
/// prelude programs for the checker.  For `use std.io.{...}` the path stored
/// is `["std", "io"]`, so we look for `<stdlib_dir>/io.mvl`.
/// Errors (missing file, parse failure) are silently ignored — the checker
/// will surface "undefined function" errors for any symbol that wasn't loaded.
fn load_stdlib_prelude<'a>(
    progs: impl Iterator<Item = &'a mvl::mvl::parser::ast::Program>,
    stdlib_dir: &Path,
) -> Vec<mvl::mvl::parser::ast::Program> {
    use mvl::mvl::parser::ast::Decl;
    use std::collections::HashSet;
    let mut loaded: HashSet<String> = HashSet::new();
    let mut prelude = Vec::new();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                // `use std.X.{...}` stores path = ["std", "X", ...]
                if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                    if let Some(module) = ud.path.get(1) {
                        if loaded.insert(module.clone()) {
                            let filename = format!("{module}.mvl");
                            let stdlib_file = stdlib_dir.join(&filename);
                            // Prefer the on-disk file; fall back to the embedded copy so
                            // the prelude is populated even when the stdlib has not been
                            // extracted (read-only CI, missing MVL_HOME, etc.).
                            let src_opt = fs::read_to_string(&stdlib_file).ok().or_else(|| {
                                mvl::mvl::stdlib::STDLIB_FILES
                                    .iter()
                                    .find(|(name, _)| *name == filename)
                                    .map(|(_, content)| content.to_string())
                            });
                            if let Some(src) = src_opt {
                                let (mut p, _) = Parser::new(&src);
                                prelude.push(p.parse_program());
                            }
                        }
                    }
                }
            }
        }
    }
    prelude
}

fn collect_imported_module_names(prog: &mvl::mvl::parser::ast::Program) -> Vec<String> {
    use mvl::mvl::parser::ast::Decl;
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if ud.path.len() >= 2 {
                let mod_name = &ud.path[0];
                if mod_name != "std" && seen.insert(mod_name.clone()) {
                    names.push(mod_name.clone());
                }
            }
        }
    }
    names
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

// ── inject_mod_bridge unit tests ──────────────────────────────────────────

#[cfg(test)]
mod bridge_inject_tests {
    use super::inject_mod_bridge;

    const PRELUDE: &str = "use mvl_runtime::prelude::*;";

    /// Inserts `mod bridge;` on the line immediately after `use mvl_runtime::prelude::*;`.
    #[test]
    fn inserts_after_prelude_marker() {
        let source = format!("{PRELUDE}\n\nfn main() {{}}\n");
        let out = inject_mod_bridge(&source);
        let lines: Vec<&str> = out.lines().collect();
        let marker_pos = lines
            .iter()
            .position(|l| l.trim() == PRELUDE)
            .expect("prelude line not found");
        assert_eq!(
            lines[marker_pos + 1],
            "mod bridge;",
            "mod bridge; must follow immediately after prelude"
        );
    }

    /// Fallback: prepends `mod bridge;` when the prelude marker is absent.
    #[test]
    fn prepends_when_marker_absent() {
        let source = "fn main() {}\n";
        let out = inject_mod_bridge(source);
        assert!(
            out.starts_with("mod bridge;\n"),
            "expected mod bridge; at start when marker absent, got:\n{out}"
        );
        assert!(
            out.contains("fn main()"),
            "original content must be preserved"
        );
    }

    /// Content after the insertion point is not truncated or duplicated.
    #[test]
    fn content_not_truncated_or_duplicated() {
        let source = format!("{PRELUDE}\n\nfn foo() -> i64 {{ 1 }}\nfn bar() -> i64 {{ 2 }}\n");
        let out = inject_mod_bridge(&source);
        assert!(out.contains("mod bridge;"), "mod bridge; must be present");
        assert_eq!(out.matches(PRELUDE).count(), 1, "prelude duplicated");
        assert_eq!(out.matches("fn foo()").count(), 1, "fn foo() duplicated");
        assert_eq!(out.matches("fn bar()").count(), 1, "fn bar() duplicated");
        assert_eq!(
            out.matches("mod bridge;").count(),
            1,
            "mod bridge; duplicated"
        );
    }
}
