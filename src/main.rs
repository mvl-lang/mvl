// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

#[cfg(feature = "llvm")]
use mvl::mvl::backends::llvm as codegen;
use mvl::mvl::backends::AssertMode;
use mvl::mvl::checker::passes::parse_req_filter;
use mvl::mvl::loader;
use mvl::mvl::packages;
use mvl::mvl::parser::Parser;
use mvl::mvl::stdlib;
use mvl::mvl::toolchain;
use std::fs;
use std::path::PathBuf;
use std::process;

mod cli;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    // ── Phase C: version resolution chain (ADR-0009) ──────────────────────────
    //
    // Skip re-exec for `mvl self …`, `mvl --version`, and `mvl version` — these
    // must always run with the current binary regardless of any project pin.
    let cmd = &args[1];
    // Commands that must always run with the current binary, regardless of any
    // project pin.  Keep this list in sync with the `match cmd.as_str()` arm below.
    let is_toolchain_meta = matches!(
        cmd.as_str(),
        "self" | "--version" | "-V" | "version" | "--help" | "-h" | "help" | "init" | "pin"
    );

    if !is_toolchain_meta {
        if let Some(target_version) = toolchain::resolve::resolve_version(
            &args[0],
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        ) {
            let target_binary = toolchain::toolchain_bin(&target_version);
            // Only re-exec if the target binary differs from the current one.
            let current = std::env::current_exe().unwrap_or_default();
            let same = std::fs::canonicalize(&target_binary)
                .ok()
                .map(|t| t == current)
                .unwrap_or(false);
            if !same {
                toolchain::resolve::reexec(&target_binary, &args);
            }
        }
    }

    // --help / -h anywhere after the subcommand → print full usage and exit (#728).
    if args.iter().skip(2).any(|a| a == "--help" || a == "-h") {
        print_usage();
        process::exit(0);
    }

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
            let error_limit = parse_error_limit(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            let format_json = args.iter().any(|a| a == "--format=json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            cli::check::run(
                &path,
                req_filter,
                error_limit,
                stdlib_profile,
                format_json,
                verbose,
            );
        }
        "build" => {
            let path = require_path_arg(&args, "build");
            let backend = parse_backend(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            let assert_mode = parse_assert_mode_or_exit(&args);
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            if verbose {
                eprintln!("stdlib profile: {stdlib_profile}");
            }
            cli::check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                build_project_llvm(&path, assert_mode);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                cli::build::run(&path, false, &[], assert_mode);
            }
        }
        "run" => {
            let path = require_path_arg(&args, "run");
            let backend = parse_backend(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            let assert_mode = parse_assert_mode_or_exit(&args);
            cli::check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let path_idx = path_arg_index(&args);
            let run_args: Vec<String> = args[path_idx + 1..]
                .iter()
                .skip_while(|a| a.as_str() != "--")
                .skip(1)
                .cloned()
                .collect();
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                run_project_llvm(&path, assert_mode);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                cli::build::run(&path, true, &run_args, assert_mode);
            }
        }
        "transpile" => {
            let path = require_path_arg(&args, "transpile");
            cli::transpile::run(&path);
        }
        "test" => {
            let path = require_path_arg(&args, "test");
            let backend = parse_backend(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            cli::check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let coverage = args.iter().any(|a| a == "--coverage");
            let bdd = args.iter().any(|a| a == "--bdd");
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                cmd_test_llvm(&path, quiet, verbose);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                cli::test::run(&path, quiet, verbose, coverage, bdd);
            }
        }
        "mutate" => {
            let path = require_path_arg(&args, "mutate");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let gen_boundary = args.iter().any(|a| a == "--gen-boundary");
            let limit: Option<usize> = args
                .windows(2)
                .find(|w| w[0] == "--limit")
                .and_then(|w| w[1].parse().ok());
            cli::mutate::run(&path, quiet, gen_boundary, limit);
        }
        "mcdc" => {
            let path = require_path_arg(&args, "mcdc");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let masking = args.iter().any(|a| a == "--masking");
            let json = args.iter().any(|a| a == "--json");
            cli::mcdc::run(&path, quiet, verbose, masking, json);
        }
        "complexity" => {
            let path = require_path_arg(&args, "complexity");
            let format_json = args.iter().any(|a| a == "--format=json");
            cli::complexity::run(&path, format_json);
        }
        "lint" => {
            let path = require_path_arg(&args, "lint");
            let show_config = args.iter().any(|a| a == "--show-config");
            cli::lint::run(&path, show_config);
        }
        "assurance" => {
            let path = require_path_arg(&args, "assurance");
            let json = args.iter().any(|a| a == "--format=json" || a == "--json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            cli::assurance::run(&path, json, verbose);
        }
        "init" => {
            // Accept optional --stdlib flag (as documented in ADR-0009); it is the
            // only init target for now so the flag is accepted but not required.
            cmd_init();
        }
        "self" => {
            cmd_self(&args);
        }
        "add" => {
            cmd_pkg_add(&args);
        }
        "install" => {
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            packages::cmd_install(&project_root);
        }
        "update" => {
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            packages::cmd_update(&project_root);
        }
        "pin" => {
            let version_arg = args.get(2).map(|s| s.as_str());
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            toolchain::cmd_pin(version_arg, &project_root);
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
    eprintln!(
        "  mvl check <file|dir> --req <N>     — run only the Req N verification pass
  mvl check <file|dir> --error-limit=N — stop after N errors (default 10; 0 = show all)
  mvl check <file|dir> --format=json  — emit errors as machine-readable JSON"
    );
    eprintln!("  mvl build <file|dir>               — transpile to Rust and run cargo build");
    eprintln!("  mvl run   [--] <file.mvl>          — transpile, build, and execute");
    eprintln!("  mvl run   [--] <file.mvl> -- ...   — pass args to the compiled binary");
    eprintln!("  mvl test  <file|dir>               — find *_test.mvl files and run cargo test");
    eprintln!("  mvl test  <file|dir> -q            — suppress MVL output, pass -q to cargo test (dot progress)");
    eprintln!("  mvl test  <file|dir> --verbose     — show transpile path and all test names with captured stdout");
    eprintln!(
        "  mvl test  <file|dir> --coverage    — run with native behavioral branch coverage report
  mvl test  <file|dir> --backend=llvm  — compile + run via LLVM/lli, check // expect: annotations
  mvl build <file|dir> --backend=llvm  — compile to LLVM IR and invoke lli (requires --features llvm)
  mvl run   <file|dir> --backend=llvm  — compile and run via LLVM lli (requires --features llvm)
  mvl build|run|check|test <file|dir> --stdlib=trusted — stdlib profile: trusted (default, 95 builtins)
  mvl build|run|check|test <file|dir> --stdlib=proven  — proven profile: verifies stdlib before user code (ADR-0023)
  mvl build|run <file|dir> --assert-mode=always     — enforce invariants unconditionally (default)
  mvl build|run <file|dir> --assert-mode=debug-only — enforce invariants in debug builds only
  mvl build|run <file|dir> --assert-mode=assume     — emit llvm.assume hint; no runtime trap"
    );
    eprintln!(
        "  mvl complexity <file|dir>           — static complexity analysis (CC, fan-out, traits)"
    );
    eprintln!("  mvl complexity <file|dir> --format=json — JSON complexity report");
    eprintln!("  mvl mutate <file|dir>               — behavioral mutation testing (ADR-0014)");
    eprintln!("  mvl mutate <file|dir> -q            — quiet: only show mutation score");
    eprintln!(
        "  mvl mutate <file|dir> --limit N     — take the first N mutants (faster, approximate score)"
    );
    eprintln!(
        "  mvl mutate <file|dir> --gen-boundary — show boundary values that kill surviving comparison/threshold mutants"
    );
    eprintln!("  mvl mcdc   <file|dir>               — MC/DC coverage analysis (DO-178C DAL-A)");
    eprintln!("  mvl mcdc   <file|dir> -q            — quiet: only show MC/DC score");
    eprintln!("  mvl mcdc   <file|dir> --verbose     — full covered/missed clause report");
    eprintln!("  mvl mcdc   <file|dir> --masking     — masking MC/DC (DO-178C): exempt coupled obligations");
    eprintln!(
        "  mvl mcdc   <file|dir> --json        — machine-readable JSON output for CI integration"
    );
    eprintln!("  mvl mcdc   <file|dir> --json -q     — JSON summary only (no per-clause detail)");
    eprintln!("  mvl lint  <file|dir>               — check style rules");
    eprintln!("  mvl lint  <file|dir> --show-config — show active linter configuration");
    eprintln!("  mvl assurance <file|dir>           — emit assurance report");
    eprintln!("  mvl assurance <file|dir> --json    — emit assurance report as JSON");
    eprintln!("  mvl assurance <file|dir> --verbose — per-function requirement detail");
    eprintln!("  mvl transpile <file.mvl>           — print transpiled Rust to stdout");
    eprintln!("  mvl init [--stdlib]                — extract stdlib to XDG_DATA_HOME/mvl/toolchains/VERSION/std/");
    eprintln!("  mvl self install <version>         — download and install a toolchain version");
    eprintln!("  mvl self use <version>             — activate an installed toolchain version");
    eprintln!("  mvl self list                      — list installed toolchain versions");
    eprintln!("  mvl self uninstall <version>       — remove an installed toolchain version");
    eprintln!("  mvl add <pkg-id> [<tag>]           — fetch package, add to mvl.toml + mvl.lock");
    eprintln!("  mvl install                        — fetch all deps from mvl.lock, verify hashes");
    eprintln!("  mvl update                         — re-resolve versions, update mvl.lock");
    eprintln!("  mvl pin [<version>]                — pin project to compiler version (writes .mvl-version)");
}

/// Parse `--error-limit=N` from args; 0 means unlimited, default is 10.
fn parse_error_limit(args: &[String]) -> usize {
    args.iter()
        .find_map(|a| a.strip_prefix("--error-limit="))
        .and_then(|s| s.parse().ok())
        .unwrap_or(10)
}

/// Parse `--assert-mode=<mode>` from args; defaults to `AssertMode::Always`.
///
/// Supported modes (issue #662):
/// - `always`     — enforce invariants unconditionally (default, current behaviour).
/// - `debug-only` — enforce only in debug builds (`debug_assert!` / conditional trap).
/// - `assume`     — emit optimizer hint only; no runtime check.
fn parse_assert_mode_or_exit(args: &[String]) -> AssertMode {
    let mode_str = args
        .iter()
        .find_map(|a| a.strip_prefix("--assert-mode="))
        .unwrap_or("always");
    AssertMode::parse(mode_str).unwrap_or_else(|| {
        eprintln!(
            "error: unknown assert-mode '{mode_str}' (supported: always, debug-only, assume)"
        );
        process::exit(1);
    })
}

/// Parse `--backend=<name>` from args; defaults to `"rust"`.
fn parse_backend(args: &[String]) -> &str {
    args.iter()
        .find_map(|a| a.strip_prefix("--backend="))
        .unwrap_or("rust")
}

/// Parse `--stdlib=<profile>` from args; defaults to `"trusted"`.
///
/// Supported profiles:
/// - `trusted` (default) — `pub builtin fn` declarations backed directly by
///   mvl_runtime / mvl_runtime_c; fast compilation, 95 builtins.
/// - `proven` — extends verification to all pure-MVL stdlib function bodies,
///   applying all 11 compiler requirements to both user code and stdlib.
///   OS/hardware builtins (I/O, memory, entropy, process) remain trusted.
fn parse_stdlib_profile(args: &[String]) -> &'static str {
    let profile = args
        .iter()
        .find_map(|a| a.strip_prefix("--stdlib="))
        .unwrap_or("trusted");
    match profile {
        "trusted" => "trusted",
        "proven" => "proven",
        other => {
            eprintln!("error: unknown stdlib profile '{other}' (supported: trusted, proven)");
            process::exit(1);
        }
    }
}

fn cmd_self(args: &[String]) {
    let subcmd = args.get(2).map(|s| s.as_str()).unwrap_or("");
    match subcmd {
        "install" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self install <version>");
                process::exit(1);
            });
            toolchain::cmd_self_install(version);
        }
        "use" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self use <version>");
                process::exit(1);
            });
            toolchain::cmd_self_use(version);
        }
        "list" => {
            toolchain::cmd_self_list();
        }
        "uninstall" => {
            let version = args.get(3).unwrap_or_else(|| {
                eprintln!("Usage: mvl self uninstall <version>");
                process::exit(1);
            });
            toolchain::cmd_self_uninstall(version);
        }
        other => {
            if other.is_empty() {
                eprintln!("Usage: mvl self <install|use|list|uninstall>");
            } else {
                eprintln!("Unknown self subcommand: {other}");
                eprintln!("Usage: mvl self <install|use|list|uninstall>");
            }
            process::exit(1);
        }
    }
}

fn cmd_pkg_add(args: &[String]) {
    let pkg_id = args.get(2).unwrap_or_else(|| {
        eprintln!("Usage: mvl add <pkg-id> [<tag>]");
        eprintln!("  pkg-id: git URL or github.com/user/repo style identifier");
        eprintln!("  tag:    optional version tag (e.g. v1.2.0); omit to use latest");
        process::exit(1);
    });
    let tag = args.get(3).map(|s| s.as_str());
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    packages::cmd_add(pkg_id, tag, &project_root);
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
fn parse_req_filter_or_exit(args: &[String]) -> Option<u8> {
    parse_req_filter(args).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    })
}

/// Returns the index of the path argument, skipping flags and an optional `--` separator.
///
/// Handles `mvl check --verbose compiler/` (flags before path) and
/// `mvl check -- compiler/` (explicit separator) for all subcommands (#728).
/// Stops at the first non-flag argument so that trailing separators like
/// `mvl run dir/ -- --program-arg` are not mistaken for the `--` separator.
fn path_arg_index(args: &[String]) -> usize {
    let mut idx = 2;
    while idx < args.len() {
        if args[idx] == "--" {
            return idx + 1; // explicit separator: path follows immediately
        }
        if !args[idx].starts_with('-') {
            return idx; // first non-flag is the path
        }
        idx += 1; // skip this flag
    }
    idx // past the end; require_path_arg will handle the missing-arg error
}

fn require_path_arg(args: &[String], cmd: &str) -> String {
    let idx = path_arg_index(args);
    if args.len() <= idx {
        eprintln!("Usage: mvl {cmd} [--] <file.mvl|directory>");
        process::exit(1);
    }
    args[idx].clone()
}

// ── LLVM backend commands (feature = "llvm") ──────────────────────────────────

#[cfg(feature = "llvm")]
fn prepare_llvm(
    prog: &mvl::mvl::parser::ast::Program,
) -> (Vec<mvl::mvl::parser::ast::Program>, codegen::LlvmCompiler) {
    let mut prelude = loader::load_implicit_prelude();
    prelude.extend(loader::load_mvl_native_stdlib_extras(std::slice::from_ref(
        prog,
    )));
    (prelude, codegen::LlvmCompiler::new())
}

/// Compile an MVL file to LLVM IR and write the .ll file to the current directory.
/// `mvl build --backend=llvm <file>`
#[cfg(feature = "llvm")]
fn build_project_llvm(path: &str, assert_mode: AssertMode) {
    let (prog, _src) = loader::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, mut compiler) = prepare_llvm(&prog);
    compiler.assert_mode = assert_mode;
    match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
        Ok(ir) => {
            let out_path = format!("{module_name}.ll");
            fs::write(&out_path, &ir).unwrap_or_else(|e| {
                eprintln!("error: cannot write {out_path}: {e}");
                process::exit(1);
            });
            println!("LLVM IR written to: {out_path}");
        }
        Err(e) => {
            eprintln!("error: LLVM codegen failed: {e}");
            process::exit(1);
        }
    }
}

/// Compile an MVL file to LLVM IR and execute it via `lli`.
/// `mvl run --backend=llvm <file>`
#[cfg(feature = "llvm")]
fn run_project_llvm(path: &str, assert_mode: AssertMode) {
    let lli = codegen::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM 22 (brew install llvm)");
        process::exit(1);
    });

    let (prog, _src) = loader::parse_or_exit(path);
    let module_name = loader::stem(path);
    let (prelude, mut compiler) = prepare_llvm(&prog);
    compiler.assert_mode = assert_mode;
    let ir = match compiler.compile_to_ir_with_prelude(&prelude, &prog, &module_name) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("error: LLVM codegen failed: {e}");
            process::exit(1);
        }
    };

    // Write IR to a temp file and run via lli.
    let tmp = tempfile::NamedTempFile::with_suffix(".ll").unwrap_or_else(|e| {
        eprintln!("error: cannot create temp file: {e}");
        process::exit(1);
    });
    fs::write(tmp.path(), &ir).unwrap_or_else(|e| {
        eprintln!("error: cannot write IR: {e}");
        process::exit(1);
    });
    let mut cmd = process::Command::new(&lli);
    // ADR-0019: load the C-ABI stdlib runtime (merged with mvl_memory since #646).
    if let Some(lib) = codegen::find_mvl_runtime_c_lib() {
        cmd.arg(format!("--load={}", lib.display()));
    }
    let status = cmd.arg(tmp.path()).status().unwrap_or_else(|e| {
        eprintln!("error: failed to run lli: {e}");
        process::exit(1);
    });
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
}

/// LLVM integration test harness (L5-03).
///
/// Finds all `.mvl` files under `path` that have `// expect:` or
/// `// Expected stdout:` annotations, compiles each via the LLVM backend,
/// runs the IR with `lli`, and asserts that stdout matches the annotation.
///
/// Also handles `*_test.mvl` files with `test fn` declarations by synthesizing
/// a `fn main()` harness that calls each test function in sequence (#500).
///
/// `mvl test --backend=llvm <path>`
#[cfg(feature = "llvm")]
fn cmd_test_llvm(path: &str, quiet: bool, verbose: bool) {
    let lli = codegen::find_lli().unwrap_or_else(|| {
        eprintln!("error: `lli` not found — install LLVM 22 (brew install llvm)");
        process::exit(1);
    });

    // Collect all .mvl files with expect annotations + fn main.
    // Each entry: (file, expected_text, is_pattern)
    let all_mvl = loader::mvl_files_all(path);
    let mut test_cases: Vec<(PathBuf, String, bool)> = Vec::new();
    // *_test.mvl files with `test fn` declarations — harness synthesized at run time.
    let mut harness_cases: Vec<PathBuf> = Vec::new();
    for file in &all_mvl {
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let has_main = src.contains("fn main(");
        let is_test_file = file
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("_test.mvl"))
            .unwrap_or(false);

        if has_main {
            if let Some(pat) = codegen::parse_expect_pattern_annotation(&src) {
                test_cases.push((file.clone(), pat, true));
            } else if let Some(expected) = codegen::parse_expect_annotation(&src) {
                test_cases.push((file.clone(), expected, false));
            }
        } else if is_test_file && src.contains("test fn ") {
            harness_cases.push(file.clone());
        }
    }

    if test_cases.is_empty() && harness_cases.is_empty() {
        if !quiet {
            println!("No LLVM test cases found (files with `fn main` + `// expect:` annotations, or `*_test.mvl` with `test fn`).");
        }
        return;
    }

    if !quiet {
        let total = test_cases.len() + harness_cases.len();
        println!("LLVM backend: {} test file(s)", total);
    }

    let mut passed = 0usize;
    let mut failed = 0usize;

    for (file, expected, is_pattern) in &test_cases {
        let file_str = file.display().to_string();
        let module_name = loader::stem(&file_str);

        let (prog, _src) = loader::parse_or_exit(&file_str);
        let ok = run_llvm_prog(
            &lli,
            &prog,
            &module_name,
            &file_str,
            expected,
            *is_pattern,
            quiet,
            verbose,
        );
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    for file in &harness_cases {
        let file_str = file.display().to_string();
        let module_name = loader::stem(&file_str);

        let src = match fs::read_to_string(file) {
            Ok(s) => s,

            Err(e) => {
                eprintln!("  FAIL (read): {file_str}: {e}");
                failed += 1;
                continue;
            }
        };

        let test_fns = collect_test_fn_names(&src);
        if test_fns.is_empty() {
            continue;
        }

        let harness_src = synthesize_test_harness(&src, &test_fns);
        let (mut parser, lex_errors) = Parser::new(&harness_src);
        if !lex_errors.is_empty() {
            eprintln!("  FAIL (lex): {file_str}");
            failed += 1;
            continue;
        }
        let prog = parser.parse_program();
        if !parser.errors().is_empty() {
            eprintln!("  FAIL (parse): {file_str}");
            for err in parser.errors() {
                eprintln!("    {err:?}");
            }
            failed += 1;
            continue;
        }

        let ok = run_llvm_prog(
            &lli,
            &prog,
            &module_name,
            &file_str,
            "ok",
            false,
            quiet,
            verbose,
        );
        if ok {
            passed += 1;
        } else {
            failed += 1;
        }
    }

    if !quiet && !verbose {
        println!(); // newline after dots
    }
    println!("{} passed, {} failed", passed, failed);
    if failed > 0 {
        process::exit(1);
    }
}

/// Compile `prog` to LLVM IR, run via `lli`, and compare stdout to `expected`.
/// Returns `true` if the output matches.
#[cfg(feature = "llvm")]
#[allow(clippy::too_many_arguments)]
fn run_llvm_prog(
    lli: &std::path::Path,
    prog: &mvl::mvl::parser::ast::Program,
    module_name: &str,
    file_str: &str,
    expected: &str,
    is_pattern: bool,
    quiet: bool,
    verbose: bool,
) -> bool {
    let (prelude, compiler) = prepare_llvm(prog);
    let ir = match compiler.compile_to_ir_with_prelude(&prelude, prog, module_name) {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("  FAIL (codegen): {file_str}");
            eprintln!("    {e}");
            return false;
        }
    };

    let tmp = match tempfile::NamedTempFile::with_suffix(".ll") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  FAIL (tempfile): {file_str}: {e}");
            return false;
        }
    };
    if let Err(e) = fs::write(tmp.path(), &ir) {
        eprintln!("  FAIL (write IR): {file_str}: {e}");
        return false;
    }

    // ADR-0019: load the C-ABI stdlib runtime (merged with mvl_memory since #646).
    let mut lli_cmd = process::Command::new(lli);
    if let Some(lib) = codegen::find_mvl_runtime_c_lib() {
        lli_cmd.arg(format!("--load={}", lib.display()));
    }
    let output = lli_cmd.arg(tmp.path()).output().unwrap_or_else(|e| {
        eprintln!("error: failed to run lli: {e}");
        process::exit(1);
    });

    let actual = String::from_utf8_lossy(&output.stdout);
    let actual_trimmed = actual.trim_end_matches('\n');
    let expected_trimmed = expected.trim_end_matches('\n');

    let matched = if is_pattern {
        codegen::glob_match(expected_trimmed, actual_trimmed)
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
    } else if !quiet {
        println!("\n  FAIL: {file_str}");
        if is_pattern {
            println!("    pattern:  {:?}", expected_trimmed);
        } else {
            println!("    expected: {:?}", expected_trimmed);
        }
        println!("    got:      {:?}", actual_trimmed);
        if verbose && !ir.is_empty() {
            println!("    --- IR ---");
            for line in ir.lines().take(40) {
                println!("    {line}");
            }
        }
    }
    matched
}

/// Extract names of all `test fn` declarations from MVL source text.
#[cfg(feature = "llvm")]
fn collect_test_fn_names(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("test fn ")
                .and_then(|rest| rest.split('(').next().map(|name| name.trim().to_string()))
        })
        .collect()
}

/// Build a runnable MVL source by stripping `test ` from `test fn` declarations
/// and appending a `fn main()` harness that calls each test function.
#[cfg(feature = "llvm")]
fn synthesize_test_harness(src: &str, test_fns: &[String]) -> String {
    let body = src.replace("test fn ", "fn ");
    let calls: String = test_fns
        .iter()
        .map(|name| format!("    {name}();\n"))
        .collect();
    format!("{body}\nfn main() -> Unit ! Console {{\n{calls}    println(\"ok\")\n}}\n")
}
