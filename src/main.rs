// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

#[cfg(feature = "llvm")]
use mvl::mvl::backends::llvm as codegen;
use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::backends::AssertMode;
use mvl::mvl::checker;
use mvl::mvl::checker::passes::{
    aggregate_verdicts, parse_req_filter, source_hash, PassRegistry, Verdict, VerdictCache,
};
use mvl::mvl::linter::{self, config::LintConfig, errors::LintDiag};
use mvl::mvl::packages;
use mvl::mvl::parser::ast::{Decl, Program, Totality, TypeBody};
use mvl::mvl::parser::Parser;
use mvl::mvl::passes::complexity;
use mvl::mvl::passes::mcdc::analysis::{analyze_mcdc, DecisionInfo};
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use mvl::mvl::toolchain;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

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
            cmd_check(
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
            maybe_check_proven_stdlib_or_exit(stdlib_profile);
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                build_project_llvm(&path, assert_mode);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                build_project(&path, false, &[], assert_mode);
            }
        }
        "run" => {
            let path = require_path_arg(&args, "run");
            let backend = parse_backend(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            let assert_mode = parse_assert_mode_or_exit(&args);
            maybe_check_proven_stdlib_or_exit(stdlib_profile);
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
                build_project(&path, true, &run_args, assert_mode);
            }
        }
        "transpile" => {
            let path = require_path_arg(&args, "transpile");
            cmd_transpile(&path);
        }
        "test" => {
            let path = require_path_arg(&args, "test");
            let backend = parse_backend(&args);
            let stdlib_profile = parse_stdlib_profile(&args);
            maybe_check_proven_stdlib_or_exit(stdlib_profile);
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
                cmd_test(&path, quiet, verbose, coverage, bdd);
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
            cmd_mutate(&path, quiet, gen_boundary, limit);
        }
        "mcdc" => {
            let path = require_path_arg(&args, "mcdc");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let masking = args.iter().any(|a| a == "--masking");
            let json = args.iter().any(|a| a == "--json");
            cmd_mcdc(&path, quiet, verbose, masking, json);
        }
        "complexity" => {
            let path = require_path_arg(&args, "complexity");
            let format_json = args.iter().any(|a| a == "--format=json");
            cmd_complexity(&path, format_json);
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

/// Validate that a derived module name is safe to embed in generated Rust source.
///
/// Module names must be non-empty, start with a letter, and contain only
/// ASCII lowercase letters, digits, or underscores.  A name that fails this
/// check could produce a malformed `mod {name} { … }` block or escape a
/// Rust comment (`// === {file} ===`) in the generated crate.
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

// ── Pure-MVL stdlib files verified in proven mode (ADR-0023, #538) ───────────
//
// These files contain pure MVL function bodies that can be verified against
// all 11 requirements.  OS/hardware-backed modules (io, env, process, crypto,
// random, time, regex, args, log) are excluded — they are only `pub builtin fn`
// declarations with no body to check.
const PROVEN_STDLIB_FILES: &[&str] = &[
    "core.mvl",
    "strings.mvl",
    "lists.mvl",
    "math.mvl",
    "collections.mvl",
    "json.mvl",
    // pbt.mvl: excluded pending checker fix for while-loop return type in
    // generic match arms (#538 follow-up, tracked separately)
];

/// Run full 11-requirement verification on all pure-MVL stdlib files.
///
/// Each file is checked with the other proven-stdlib files as its prelude so
/// that cross-module references (e.g. lists.mvl calling list_len from core)
/// resolve correctly.
///
/// Returns `(stdlib_file_name, errors)` for any file that has errors.
fn check_proven_stdlib() -> Vec<(String, mvl::mvl::checker::CheckResult)> {
    // Parse all proven-stdlib files up front.
    let programs: Vec<(String, mvl::mvl::parser::ast::Program)> = PROVEN_STDLIB_FILES
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
fn maybe_check_proven_stdlib_or_exit(profile: &str) {
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
fn cmd_check(
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
    let files = mvl_files(path, false);
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
    let mut had_errors = !resolve_result.is_ok() || stdlib_proven_failed;
    for err in &resolve_result.errors {
        eprintln!("error[resolver]: {err}");
    }

    let registry = PassRegistry::default_registry();

    // Pre-parse stdlib files imported by user programs so the checker knows
    // about their types and functions.  This covers `use std.io.{...}` etc.
    let stdlib_prelude = load_stdlib_prelude(
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
                        msg = json_escape(&err.message()),
                        file = json_escape(file_str),
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

/// Static complexity analysis (`mvl complexity`).
///
/// Computes per-function and per-module complexity metrics from the AST (issue #208):
/// - Cyclomatic complexity, function length, nested match depth, effect width
/// - Module fan-out, trait impl/fan-out counts, extern ratio
fn cmd_complexity(path: &str, format_json: bool) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let mut reports = Vec::new();
    for f in &files {
        let file_str = f.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        reports.push(complexity::analyze(&file_str, &prog));
    }
    if format_json {
        complexity::print_json(&reports);
    } else {
        for report in &reports {
            complexity::print_human(report);
        }
    }
}

/// MC/DC coverage analysis (DO-178C DAL-A).
///
/// Execution model (mirrors `mvl mutate`):
///   1. Parse + static analysis — build decision/clause table
///   2. Transpile with per-clause instrumentation
///   3. Compile + run tests — collect observations via `MVL_MCDC_OUT`
///   4. Independence check — for each clause, verify it independently toggles outcome
///   5. Report — score + optional verbose covered/missed table
fn cmd_mcdc(path: &str, quiet: bool, verbose: bool, masking: bool, json: bool) {
    use mvl::mvl::backends::rust::{
        emit_mcdc_preamble, emit_mcdc_report_test, transpile_mcdc_source_with_prelude,
        transpile_mcdc_with_prelude, MCDCDecision,
    };
    use std::collections::HashSet;

    let test_files = mvl_files(path, true);
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

    let stdlib_prelude_progs = load_implicit_prelude();

    // The implicit prelude always has `pub builtin fn` declarations (strings.mvl,
    // lists.mvl), so mvl_runtime is always required for MC/DC instrumented builds.
    let need_mvl_runtime = transpiler::prelude_requires_runtime(&stdlib_prelude_progs);

    // Build a fn_name → prelude_stem map and preload prelude source lines so
    // that JSON source-fragment lookup works for decisions in stdlib functions.
    // IMPLICIT_STEMS must mirror load_implicit_prelude().
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
                content.lines().map(String::from).collect(),
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
        let (prog, src) = parse_or_exit(&file_str);
        let s = stem(&file_str);
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
        let (out, decisions) = transpile_mcdc_with_prelude(
            &prog,
            &module_name,
            &module_name,
            start_id,
            &stdlib_prelude_progs,
        );
        let _ = out;
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
    let source_files = mvl_files(path, false);
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue;
        }
        let (prog, src) = parse_or_exit(&file_str);
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
        let (out, decisions) = transpile_mcdc_source_with_prelude(
            &prog,
            &module_name,
            &module_name,
            start_id,
            &stdlib_prelude_progs,
        );
        let _ = out;
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
            let s = stem(&file_str);
            let module_name = s.replace('-', "_");
            let (prog, _src) = parse_or_exit(&file_str);
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
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe)]\n\n",
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
        "mvl_runtime = { path = \"./mvl_runtime\" }\n"
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
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("rust");
        let runtime_dst = tmp_dir.join("mvl_runtime");
        copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
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
                    json_escape(&decision.file)
                ));
                out.push_str(&format!("      \"line\": {},\n", decision.line));
                out.push_str(&format!("      \"kind\": \"{kind_label}\",\n"));
                out.push_str(&format!(
                    "      \"source\": \"{}\",\n",
                    json_escape(&source_frag)
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
                                let dep = json_escape(&shared.join(", "));
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
        let src = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file_str}:1:1: error: [io-error] cannot read file: {e}");
                total_errors += 1;
                had_errors = true;
                continue;
            }
        };
        let (mut parser, lex_errors) = Parser::new(&src);
        let prog = parser.parse_program();

        // Surface lex and parse errors as lint diagnostics.
        let mut pre_diags: Vec<LintDiag> = Vec::new();
        for err in &lex_errors {
            pre_diags.push(LintDiag::error(
                "lex-error",
                err.message.clone(),
                err.span.line,
                err.span.col,
            ));
        }
        for err in parser.errors() {
            pre_diags.push(LintDiag::error(
                "parse-error",
                err.message.clone(),
                err.span.line,
                err.span.col,
            ));
        }

        let result = linter::lint(&prog, &src, &cfg);

        for diag in pre_diags.iter().chain(result.diags.iter()) {
            eprintln!("{}", diag.render(&file_str));
        }

        total_warnings += result.warning_count();
        total_errors += result.error_count() + pre_diags.len();

        if !pre_diags.is_empty() || !result.is_ok() {
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
fn build_project(path: &str, run: bool, run_args: &[String], assert_mode: AssertMode) {
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

    // Load the implicit stdlib prelude: core.mvl + Phase 4 stdlib files
    // (strings.mvl, lists.mvl). Non-stub MVL functions
    // (e.g. range(), trim()) are transpiled from source rather than relying
    // on hardcoded Rust mappings in the transpiler. Embedded at compile time.
    let mut stdlib_prelude_progs = load_implicit_prelude();
    // Extend with any pure-MVL stdlib modules imported by this program (e.g. json.mvl).
    let all_progs: Vec<_> = std::iter::once(&prog)
        .chain(sibling_modules.iter().map(|(_, p)| p))
        .cloned()
        .collect();
    stdlib_prelude_progs.extend(load_mvl_native_stdlib_extras(&all_progs));

    // Load MVL source files from any `pkg.*` packages referenced by the program.
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    stdlib_prelude_progs.extend(load_pkg_modules(&all_progs, &project_root));

    // Collect expression types from ALL programs (prelude + user) for the
    // transpiler to emit type-specific Rust at method-call sites (#554).
    let mut all_expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
    let check_result = checker::check_with_prelude(&stdlib_prelude_progs, &prog);
    all_expr_types.extend(check_result.expr_types);
    let out = transpiler::transpile_project(
        &crate_name,
        &prog,
        &sibling_modules,
        &stdlib_prelude_progs,
        all_expr_types,
        assert_mode,
    );

    // Write to a per-crate workspace so each build gets its own mvl_runtime copy.
    // Layout: temp/mvl_build_{name}/{name}/  (crate), temp/mvl_build_{name}/mvl_runtime/ (runtime)
    // The Cargo.toml path dep `../mvl_runtime` resolves correctly from within the crate dir.
    let tmp_workspace = std::env::temp_dir().join(format!("mvl_build_{crate_name}"));
    let tmp_dir = tmp_workspace.join(&crate_name);
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
    let mut bridge_path: Option<PathBuf> = match fs::canonicalize(&bridge_candidate) {
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
        // No user bridge.rs — check if a pkg.* package provides one.
        bridge_path = find_pkg_bridge(&all_progs, &project_root);
    }

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

    // If the program uses mvl_runtime, copy it inside the build dir so the
    // relative path `./mvl_runtime` in Cargo.toml resolves.  Each build gets
    // its own copy, which eliminates races when multiple bridge programs are
    // built concurrently (e.g. parallel integration tests).
    //
    // Idempotent for concurrent invocations with identical source: create_dir_all
    // + fs::copy both tolerate pre-existing targets.  Stale artefacts from a
    // prior build of a different version are handled by cargo's incremental cache.
    if out.use_mvl_runtime {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("rust");
        let runtime_dst = tmp_dir.join("mvl_runtime");
        if !runtime_src.exists() {
            eprintln!(
                "error: mvl_runtime not found at {} — cannot build extern bridge",
                runtime_src.display()
            );
            process::exit(1);
        }
        copy_dir_recursive(&runtime_src, &runtime_dst).expect("copy mvl_runtime");
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
        // Run the binary with the invocation CWD (where the user ran `mvl run`) so that
        // relative file paths in the program (and in run_args like --file logs.jsonl)
        // resolve against the caller's working directory, not the tmp build dir or
        // the source file's parent directory.
        let binary = tmp_dir.join("target").join("debug").join(&crate_name);
        let invocation_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let run_status = process::Command::new(&binary)
            .args(run_args)
            .current_dir(&invocation_dir)
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
fn cmd_test(path: &str, quiet: bool, verbose: bool, coverage: bool, bdd: bool) {
    if quiet && verbose {
        eprintln!(
            "warning: --quiet and --verbose are mutually exclusive; --verbose takes precedence"
        );
    }
    let quiet = quiet && !verbose;

    let test_files = mvl_files(path, true); // test_only=true
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

    // Load the implicit stdlib prelude (core + Phase 4 stdlib files).
    let mut stdlib_prelude_progs = load_implicit_prelude();
    // Pre-scan all test files to discover pure-MVL stdlib imports (e.g. json) and
    // extend the prelude so their types/functions are available during transpilation.
    // Also load any pkg.* package modules referenced by the test files.
    {
        let all_test_progs: Vec<_> = test_files
            .iter()
            .map(|f| parse_or_exit(&f.display().to_string()).0)
            .collect();
        stdlib_prelude_progs.extend(load_mvl_native_stdlib_extras(&all_test_progs));
        let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        stdlib_prelude_progs.extend(load_pkg_modules(&all_test_progs, &project_root));

        // For packages tested from their own src/ directory, also load sibling
        // .mvl files (non-test, including internal/) so types and extern
        // declarations are in scope during transpilation.
        // Track already-loaded paths so that multiple test files in the same
        // directory don't add the same library file multiple times.
        let mut loaded_prelude_paths: std::collections::HashSet<std::path::PathBuf> =
            std::collections::HashSet::new();
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
                                // Also skip pure-function helper/demo files (no extern blocks
                                // or type declarations): loading them shadows runtime primitives
                                // and causes compilation failures (e.g. collections.mvl defining
                                // `fn list_get()` that shadows `list_get<T>` from mvl_runtime).
                                if !transpiler::has_main_fn(&parsed)
                                    && transpiler::has_extern_or_type_decls(&parsed)
                                {
                                    stdlib_prelude_progs.push(parsed);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

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
        let (prog, _src) = parse_or_exit(&file_str);
        let s = stem(&file_str);
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
        let (out, branches) = if coverage {
            transpiler::transpile_covered_with_prelude(
                &prog,
                &module_name,
                &module_name,
                next_branch_id,
                &stdlib_prelude_progs,
            )
        } else {
            (
                transpiler::transpile_with_prelude(&prog, &module_name, &stdlib_prelude_progs),
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
    let source_files = mvl_files(path, false); // non-test files
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue; // already covered by a *_test.mvl file
        }
        let (prog, _src) = parse_or_exit(&file_str);
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
        let (out, branches) = if coverage {
            transpiler::transpile_covered_source_with_prelude(
                &prog,
                &module_name,
                &module_name,
                next_branch_id,
                &stdlib_prelude_progs,
            )
        } else {
            (
                transpiler::transpile_source_with_prelude(
                    &prog,
                    &module_name,
                    &stdlib_prelude_progs,
                ),
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
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe)]\n\n",
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

    // Write Cargo.toml for the test runner, adding mvl_runtime if any module needs it.
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

    // Copy mvl_runtime into the temp dir if needed (parallel builds each get their own copy).
    if need_mvl_runtime {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("rust");
        let runtime_dst = tmp_dir.join("mvl_runtime");
        if !runtime_src.exists() {
            eprintln!(
                "error: mvl_runtime not found at {} — cannot build test crate with stdlib/extern",
                runtime_src.display()
            );
            process::exit(1);
        }
        copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
            eprintln!("error: cannot copy mvl_runtime: {e}");
            process::exit(1);
        });
    }
    fs::write(src_dir.join("lib.rs"), &combined_rs).unwrap_or_else(|e| {
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
    cmd.arg("test").arg("--lib").current_dir(&tmp_dir);
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

/// Native behavioral mutation testing (ADR-0014).
///
/// Execution model: single compile embeds all mutants behind `MVL_MUTANT` env-var
/// dispatch; N parallel test-binary runs determine which mutants are killed.
fn cmd_mutate(path: &str, quiet: bool, gen_boundary: bool, limit: Option<usize>) {
    let test_files = mvl_files(path, true);
    if test_files.is_empty() {
        eprintln!("No *_test.mvl files found at: {path}");
        process::exit(1);
    }

    // Duplicate module name check (same as cmd_test)
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
    let stdlib_prelude_progs = load_implicit_prelude();

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
        let (prog, _src) = parse_or_exit(&file_str);
        let s = stem(&file_str);
        let module_name = s.strip_suffix("_test").unwrap_or(&s).replace('-', "_");
        let (out, mutants) = transpiler::transpile_mutated_with_prelude(
            &prog,
            &module_name,
            &module_name,
            &stdlib_prelude_progs,
        );
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
    let source_files = mvl_files(path, false);
    for src_file in &source_files {
        let file_str = src_file.display().to_string();
        let s = stem(&file_str);
        let module_name = s.replace('-', "_");
        if covered_stems.contains(&module_name) {
            continue;
        }
        let (prog, _src) = parse_or_exit(&file_str);
        let has_tests = prog
            .declarations
            .iter()
            .any(|d| matches!(d, Decl::Fn(fd) if fd.is_test));
        if !has_tests {
            continue;
        }
        let (out, mutants) = transpiler::transpile_mutated_source_with_prelude(
            &prog,
            &module_name,
            &module_name,
            &stdlib_prelude_progs,
        );
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
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe)]\n\n",
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
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("rust");
        let runtime_dst = tmp_dir.join("mvl_runtime");
        copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
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

    let binary_path =
        find_test_binary_from_cargo_output(&build_output.stdout).unwrap_or_else(|| {
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

/// Extract the test binary path from `cargo test --no-run --message-format=json` stdout.
fn find_test_binary_from_cargo_output(output: &[u8]) -> Option<std::path::PathBuf> {
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

/// Emit an assurance report for a file or directory.
fn cmd_assurance(path: &str, json: bool, verbose: bool) {
    let stdlib_dir = stdlib::ensure_stdlib();
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }
    let all_mvl_count = mvl_files_all(path).len();
    let excluded_count = all_mvl_count - files.len();

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
    let all_assurance_progs: Vec<Program> =
        parsed_assurance.iter().map(|(_, p, _)| p.clone()).collect();

    // Count kernel builtins from the implicit stdlib prelude (strings.mvl, lists.mvl).
    // These are always part of the trust boundary for any MVL program, even though they
    // are not declared in user code. ADR-0006: trust boundaries must be declared and
    // countable — this surfaces the kernel builtin count in every assurance report.
    let kernel_extern_count: usize = load_implicit_prelude()
        .iter()
        .flat_map(|p| p.declarations.iter())
        .filter(|d| {
            matches!(d,
                mvl::mvl::parser::ast::Decl::Fn(fd) if fd.is_builtin
            )
        })
        .count();

    for (idx, (file_str, prog, src)) in parsed_assurance.iter().enumerate() {
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
        let total_implicit = total_verified - total_explicit;
        println!("  total fn:          {total_verified} ({total_explicit} explicit, {total_implicit} implicit) — {verified_pct}% of implemented");
        println!("  partial fn:        {total_partial}");
        println!("  extern fn:         {total_extern} ({extern_pct}% trust boundary)");
        println!("  kernel builtins:   {kernel_extern_count} (stdlib strings.mvl + lists.mvl)");
        println!("  implemented:       {implemented}");
        println!("  test fn:           {total_test_fns}");
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
            project_verdicts[3].is_proven(),
            &format!(
                "{} total fn, {} non-exhaustive match",
                total_verified, req_errors[3]
            ),
        );
        print_req_row(
            4,
            "Null elimination",
            &req_errors,
            project_verdicts[4].is_proven(),
            &format!("{} direct Option access", req_errors[4]),
        );
        print_req_row(
            5,
            "Error visibility",
            &req_errors,
            project_verdicts[5].is_proven(),
            &format!("{} unhandled Result", req_errors[5]),
        );
        print_req_row(
            6,
            "Ownership",
            &req_errors,
            project_verdicts[6].is_proven(),
            &format!("{} immutability violations", req_errors[6]),
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
        print_req_row(
            9,
            "Data race freedom",
            &req_errors,
            project_verdicts[9].is_proven(),
            &format!(
                "{} fns use capabilities, {} violations",
                total_capabilities_fns, req_errors[9]
            ),
        );
        print_req_row(
            10,
            "Refinements",
            &req_errors,
            project_verdicts[10].is_proven(),
            &format!(
                "{} fns with refinements, {} violations",
                total_refinements_fns, req_errors[10]
            ),
        );
        print_req_row(
            11,
            "IFC",
            &req_errors,
            project_verdicts[11].is_proven(),
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
    has_capabilities: bool,
    has_refinements: bool,
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
                    stats.fn_details.push(FnDetail {
                        name: fd.name.clone(),
                        totality: fd.totality.clone(),
                        effects: fd.effects.iter().map(|e| e.to_string()).collect(),
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
                            effects: ef.effects.iter().map(|e| e.to_string()).collect(),
                            has_capabilities: false,
                            has_refinements: false,
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
/// Build the implicit prelude: core.mvl + Phase 4 stdlib files (strings, lists).
/// Every compile path loads these three files so that the string/list kernel
/// builtins and method implementations are always visible without requiring
/// an explicit `use std.*` in user programs.
///
/// Panics (via `process::exit`) if any embedded file fails to parse, since
/// that would be a compiler bug.
fn load_implicit_prelude() -> Vec<mvl::mvl::parser::ast::Program> {
    const IMPLICIT: &[&str] = &["core.mvl", "strings.mvl", "lists.mvl"];
    let mut progs = Vec::new();
    for name in IMPLICIT {
        let content = stdlib::stdlib_content(name)
            .unwrap_or_else(|| panic!("{name} is embedded at compile time and must be present"));
        let (mut parser, _) = Parser::new(content);
        progs.push(parser.parse_program());
    }
    progs
}

use transpiler::RUST_BACKED_STDLIB;

/// Modules already in the implicit prelude — never loaded twice.
const IMPLICIT_PRELUDE_STEMS: &[&str] = &["core", "strings", "lists"];

/// Load pure-MVL stdlib modules (e.g. json, collections) imported by `progs`
/// into the prelude so the emitter can inline their types and functions.
///
/// Resolves transitive dependencies: if a loaded module itself imports another
/// pure-MVL stdlib module, that module is loaded too.
///
/// Rust-backed modules (io, env, …) and the four always-implicit modules are
/// excluded: the former come from `mvl_runtime::stdlib`, the latter are already
/// in the base prelude returned by `load_implicit_prelude`.
fn load_mvl_native_stdlib_extras(
    progs: &[mvl::mvl::parser::ast::Program],
) -> Vec<mvl::mvl::parser::ast::Program> {
    use mvl::mvl::parser::ast::Decl;
    use std::collections::HashSet;
    let mut loaded: HashSet<String> = HashSet::new();
    let mut extras: Vec<mvl::mvl::parser::ast::Program> = Vec::new();

    // seed work queue with user programs
    let mut pending: Vec<mvl::mvl::parser::ast::Program> = progs.to_vec();

    while !pending.is_empty() {
        let mut next_pending = Vec::new();
        for prog in &pending {
            for decl in &prog.declarations {
                if let Decl::Use(ud) = decl {
                    if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                        if let Some(module) = ud.path.get(1) {
                            let m = module.as_str();
                            if RUST_BACKED_STDLIB.contains(&m)
                                || IMPLICIT_PRELUDE_STEMS.contains(&m)
                            {
                                continue;
                            }
                            if loaded.insert(module.clone()) {
                                let filename = format!("{m}.mvl");
                                if let Some(content) = stdlib::stdlib_content(&filename) {
                                    let (mut p, _) = Parser::new(content);
                                    let loaded_prog = p.parse_program();
                                    next_pending.push(loaded_prog.clone());
                                    extras.push(loaded_prog);
                                }
                            }
                        }
                    }
                }
            }
        }
        pending = next_pending;
    }

    extras
}

/// Load MVL source files from `pkg.*` packages referenced by user programs.
///
/// For each `use pkg.X` declaration, looks up the package in:
///   1. `<project_root>/.mvl/pkg/X/` (local override)
///   2. `$XDG_DATA_HOME/mvl/pkg/X/<version>/` (global cache)
///
/// Loads every `.mvl` file found in `src/` and `src/internal/` so that
/// type declarations and extern "rust" blocks are available to the transpiler.
fn load_pkg_modules(
    progs: &[mvl::mvl::parser::ast::Program],
    project_root: &std::path::Path,
) -> Vec<mvl::mvl::parser::ast::Program> {
    use mvl::mvl::parser::ast::Decl;
    use std::collections::HashSet;

    let mut loaded: HashSet<String> = HashSet::new();
    let mut result: Vec<mvl::mvl::parser::ast::Program> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if !loaded.insert(pkg_name.clone()) {
                            continue;
                        }
                        // Resolve package directory (local override takes precedence)
                        let pkg_dir =
                            mvl::mvl::packages::fetch::local_override_dir(project_root, pkg_name);
                        if !pkg_dir.exists() {
                            continue; // package not installed — checker will surface errors
                        }
                        // Load src/*.mvl and src/internal/*.mvl
                        for sub in &["src", "src/internal"] {
                            let dir = pkg_dir.join(sub);
                            if let Ok(entries) = fs::read_dir(&dir) {
                                for entry in entries.flatten() {
                                    // Symlink escape guard (#715): skip symlinks so a
                                    // malicious package cannot point outside its directory.
                                    if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false)
                                    {
                                        continue;
                                    }
                                    let path = entry.path();
                                    if path.extension().map(|e| e == "mvl").unwrap_or(false) {
                                        if let Ok(src) = fs::read_to_string(&path) {
                                            let (mut p, _) = Parser::new(&src);
                                            result.push(p.parse_program());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    result
}

/// Find a `bridge.rs` from a `pkg.*` package used by the given programs.
///
/// Returns the path to the first package bridge.rs found, or None.
/// Used as a fallback when the user program has no bridge.rs of its own.
fn find_pkg_bridge(
    progs: &[mvl::mvl::parser::ast::Program],
    project_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use mvl::mvl::parser::ast::Decl;

    // Canonicalize the package root once; reject any bridge that escapes it
    // (same symlink-escape guard applied to user-supplied bridge.rs at line 1572).
    let canon_pkg_root = match fs::canonicalize(project_root.join(".mvl").join("pkg")) {
        Ok(p) => p,
        Err(_) => return None,
    };

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        let pkg_dir =
                            mvl::mvl::packages::fetch::local_override_dir(project_root, pkg_name);
                        let bridge = pkg_dir.join("bridge.rs");
                        if let Ok(canon_bridge) = fs::canonicalize(&bridge) {
                            if canon_bridge.starts_with(&canon_pkg_root) {
                                return Some(canon_bridge);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

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
    let raw = if p.is_dir() {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program")
            .to_string()
    } else {
        p.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program")
            .to_string()
    };
    // Rust package names must not start with a digit.
    if raw.starts_with(|c: char| c.is_ascii_digit()) {
        format!("mvl_{raw}")
    } else {
        raw
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
        // UseAfterMove → requirement 2 (consume() is the explicit ownership-transfer form)
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

// ── find_test_binary_from_cargo_output unit tests ──────────────────────────

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
        // executable field is null, not a string — no `"executable":"` substring
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

// ── LLVM backend commands (feature = "llvm") ──────────────────────────────────

#[cfg(feature = "llvm")]
fn prepare_llvm(
    prog: &mvl::mvl::parser::ast::Program,
) -> (Vec<mvl::mvl::parser::ast::Program>, codegen::LlvmCompiler) {
    let mut prelude = load_implicit_prelude();
    prelude.extend(load_mvl_native_stdlib_extras(std::slice::from_ref(prog)));
    (prelude, codegen::LlvmCompiler::new())
}

/// Compile an MVL file to LLVM IR and write the .ll file to the current directory.
/// `mvl build --backend=llvm <file>`
#[cfg(feature = "llvm")]
fn build_project_llvm(path: &str, assert_mode: AssertMode) {
    let (prog, _src) = parse_or_exit(path);
    let module_name = stem(path);
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

    let (prog, _src) = parse_or_exit(path);
    let module_name = stem(path);
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
    let all_mvl = mvl_files_all(path);
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
        let module_name = stem(&file_str);

        let (prog, _src) = parse_or_exit(&file_str);
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
        let module_name = stem(&file_str);

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

/// Recursively find all `.mvl` files under `path` (both test and non-test).
#[cfg(feature = "llvm")]
fn mvl_files_all(path: &str) -> Vec<PathBuf> {
    let root = Path::new(path);
    if root.is_file() {
        if root.extension().map(|e| e == "mvl").unwrap_or(false) {
            return vec![root.to_path_buf()];
        }
        return vec![];
    }
    let mut result = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().map(|e| e == "mvl").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    walk(root, &mut result);
    result.sort();
    result
}
