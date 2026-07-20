// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

pub mod args;
pub mod assurance;
pub mod build;
pub mod check;
pub mod doctor;
pub mod fmt;
pub mod fuzz;
pub mod harden;
pub mod kloc;
pub mod lint;
pub mod llvm_text;
pub mod mcdc;
pub mod meta;
pub mod mutate;
#[cfg(feature = "openapi")]
pub mod openapi;
pub mod prove;
pub mod test;
pub mod tir;
pub mod wasm_text;

use mvl::mvl::checker::errors::CheckError;
use mvl::mvl::loader;
use mvl::mvl::packages;
use mvl::mvl::parser::ast::Program;
use mvl::mvl::toolchain;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

// ── Pure-MVL stdlib files verified in proven mode (ADR-0023, #538) ───────────
//
// These files contain pure MVL function bodies that can be verified against
// all 11 requirements.  OS/hardware-backed modules (io, env, process, crypto,
// random, time, regex, args, log) are excluded — they are only `pub builtin fn`
// declarations with no body to check.
pub(super) const PROVEN_STDLIB_FILES: &[&str] = &[
    "core.mvl",
    "strings.mvl",
    "lists.mvl",
    "math.mvl",
    "collections.mvl",
    "json.mvl",
    "toml.mvl",
    // pbt.mvl: excluded pending checker fix for while-loop return type in
    // generic match arms (#538 follow-up, tracked separately)
];

/// Route a subcommand to the appropriate cli module.
pub(super) fn dispatch(args: &[String]) {
    let cmd = &args[1];
    match cmd.as_str() {
        "--version" | "-V" | "version" => {
            println!("mvl {}", env!("CARGO_PKG_VERSION"));
        }
        "--help" | "-h" | "help" => {
            args::print_usage();
        }
        "check" => {
            let stdin = args.iter().any(|a| a == "--stdin");
            let req_filter = args::parse_req_filter_or_exit(args);
            let error_limit = args::parse_error_limit(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let format_json = args.iter().any(|a| a == "--format=json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let check_opts = check::CheckOptions {
                error_limit,
                stdlib_profile,
                format_json,
                verbose,
                solver_mode: args::parse_solver_mode_or_exit(args),
                refinement_stats: args.iter().any(|a| a == "--refinement-stats"),
            };
            if stdin {
                check::run_stdin(req_filter, check_opts);
            } else {
                let path = args::require_path_arg(args, "check");
                check::run(&path, req_filter, check_opts);
            }
        }
        "build" => {
            let path = args::require_path_arg(args, "build");
            let backend = args::parse_backend(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let assert_mode = args::parse_assert_mode_or_exit(args);
            let target = args::parse_target_or_exit(args);
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            if verbose {
                eprintln!("stdlib profile: {stdlib_profile}");
            }
            check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let release = args.iter().any(|a| a == "--release");
            let emit_only = args.iter().any(|a| a == "--emit-only");
            if backend == "llvm" {
                llvm_text::build_project_llvm_text(&path);
            } else if backend == "wasm" {
                wasm_text::build_project_wasm(&path);
            } else {
                build::run(&path, false, &[], assert_mode, target, release, emit_only);
            }
        }
        "run" => {
            let path = args::require_path_arg(args, "run");
            let backend = args::parse_backend(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let assert_mode = args::parse_assert_mode_or_exit(args);
            let target = args::parse_target_or_exit(args);
            let release = args.iter().any(|a| a == "--release");
            check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let path_idx = args::path_arg_index(args);
            let run_args: Vec<String> = args[path_idx + 1..]
                .iter()
                .skip_while(|a| a.as_str() != "--")
                .skip(1)
                .cloned()
                .collect();
            if backend == "llvm" {
                llvm_text::run_project_llvm_text(&path);
            } else {
                build::run(&path, true, &run_args, assert_mode, target, release, false);
            }
        }
        "test" => {
            let path = args::require_path_arg(args, "test");
            let backend = args::parse_backend(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let coverage = args.iter().any(|a| a == "--coverage");
            let bdd = args.iter().any(|a| a == "--bdd");
            let use_tokio = args.iter().any(|a| a == "--target=tokio");
            if backend == "llvm" {
                llvm_text::cmd_test_llvm_text(&path, quiet, verbose);
            } else if backend == "wasm" {
                wasm_text::cmd_test_wasm(&path, quiet, verbose);
            } else {
                let expect_only = args.iter().any(|a| a == "--expect");
                if expect_only {
                    // Only run // expect: annotated files through the Rust transpiler
                    test::run_expect_tests(&path, quiet, verbose);
                } else {
                    test::run(&path, quiet, verbose, coverage, bdd, use_tokio);
                }
            }
        }
        "tir" => {
            let path = args::require_path_arg(args, "tir");
            tir::run(&path);
        }
        "mutate" => {
            let path = args::require_path_arg(args, "mutate");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let gen_boundary = args.iter().any(|a| a == "--gen-boundary");
            let limit: Option<usize> = args
                .windows(2)
                .find(|w| w[0] == "--limit")
                .and_then(|w| w[1].parse().ok());
            mutate::run(&path, quiet, gen_boundary, limit);
        }
        "fuzz" => {
            let path = args::require_path_arg(args, "fuzz");
            let target = args
                .windows(2)
                .find(|w| w[0] == "--target")
                .map(|w| w[1].as_str());
            let time_secs: Option<u64> = args
                .windows(2)
                .find(|w| w[0] == "--time")
                .and_then(|w| w[1].trim_end_matches('s').parse().ok());
            let corpus = args
                .windows(2)
                .find(|w| w[0] == "--corpus")
                .map(|w| w[1].as_str());
            let list = args.iter().any(|a| a == "--list");
            fuzz::run(&path, target, time_secs, corpus, list);
        }
        "mcdc" => {
            let path = args::require_path_arg(args, "mcdc");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let masking = args.iter().any(|a| a == "--masking");
            let json = args.iter().any(|a| a == "--json");
            mcdc::run(&path, quiet, verbose, masking, json);
        }
        "prove" => {
            let path = args::require_path_arg(args, "prove");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let stdlib_profile = args::parse_stdlib_profile(args);
            let callee_filter = if let Some(pos) = args.iter().position(|a| a == "--callee") {
                match args.get(pos + 1).map(|s| s.as_str()) {
                    Some(name) if !name.starts_with("--") => Some(name),
                    _ => {
                        eprintln!("error: --callee requires a function name argument");
                        eprintln!("usage: mvl prove <file|dir> --callee <fn>");
                        process::exit(1);
                    }
                }
            } else {
                None
            };
            prove::run(&path, verbose, stdlib_profile, callee_filter);
        }
        "harden" => {
            let path = args::require_path_arg(args, "harden");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let json = args.iter().any(|a| a == "--json");
            let stdlib_profile = args::parse_stdlib_profile(args);
            let callee_filter = if let Some(pos) = args.iter().position(|a| a == "--callee") {
                match args.get(pos + 1).map(|s| s.as_str()) {
                    Some(name) if !name.starts_with("--") => Some(name),
                    _ => {
                        eprintln!("error: --callee requires a function name argument");
                        eprintln!("usage: mvl harden <file|dir> --callee <fn>");
                        process::exit(1);
                    }
                }
            } else {
                None
            };
            let emit_tests = args.iter().any(|a| a == "--emit-tests");
            let mcdc = args.iter().any(|a| a == "--mcdc");
            harden::run(
                &path,
                verbose,
                json,
                emit_tests,
                stdlib_profile,
                callee_filter,
                mcdc,
            );
        }
        "complexity" => {
            use mvl::mvl::passes::complexity;
            let path = args::require_path_arg(args, "complexity");
            let format_json = args.iter().any(|a| a == "--format=json");
            let files = loader::mvl_files(&path, false);
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
        #[cfg(feature = "openapi")]
        "openapi" => {
            let path = args::require_path_arg(args, "openapi");
            openapi::run(&path);
        }
        "fmt" => {
            let stdin = args.iter().any(|a| a == "--stdin");
            let check = args.iter().any(|a| a == "--check");
            let stdout = args.iter().any(|a| a == "--stdout");
            if stdin {
                fmt::run(
                    "",
                    fmt::FmtOptions {
                        check,
                        stdin: true,
                        stdout: false,
                    },
                );
            } else {
                let path = args::require_path_arg(args, "fmt");
                fmt::run(
                    &path,
                    fmt::FmtOptions {
                        check,
                        stdin: false,
                        stdout,
                    },
                );
            }
        }
        "lint" => {
            let path = args::require_path_arg(args, "lint");
            let show_config = args.iter().any(|a| a == "--show-config");
            lint::run(&path, show_config);
        }
        "assurance" => {
            let path = args::require_path_arg(args, "assurance");
            let json = args.iter().any(|a| a == "--format=json" || a == "--json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            assurance::run(&path, json, verbose);
        }
        "doctor" => {
            doctor::run();
        }
        "kloc" => {
            let csv = args.iter().any(|a| a == "--csv");
            let path = args
                .iter()
                .skip(2)
                .find(|a| !a.starts_with("--"))
                .map(|s| s.as_str())
                .unwrap_or(".");
            kloc::run(path, csv);
        }
        "init" => {
            meta::cmd_init(args);
        }
        "self" => {
            meta::cmd_self(args);
        }
        "add" => {
            meta::cmd_pkg_add(args);
        }
        "install" => {
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let global_only = args.iter().any(|a| a == "--global");
            if let Err(e) = packages::cmd_install(&project_root, global_only) {
                eprintln!("error: {e}");
                if matches!(e, packages::PackageError::Lock(_)) {
                    eprintln!("hint: run 'mvl add <package>' to create mvl.lock");
                }
                process::exit(1);
            }
        }
        "update" => {
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut opts = packages::UpdateOptions::default();
            let mut iter = args.iter().skip(2);
            while let Some(arg) = iter.next() {
                match arg.as_str() {
                    "--force" => opts.force = true,
                    "--offline" => opts.offline = true,
                    "--dry-run" => opts.dry_run = true,
                    "--package" => match iter.next() {
                        Some(name) => opts.only = Some(name.clone()),
                        None => {
                            eprintln!("error: --package requires a value");
                            process::exit(2);
                        }
                    },
                    other if other.starts_with("--package=") => {
                        opts.only = Some(other.trim_start_matches("--package=").to_string());
                    }
                    other => {
                        eprintln!("error: unknown flag for 'mvl update': {other}");
                        eprintln!(
                            "usage: mvl update [--force] [--offline] [--dry-run] [--package <name>]"
                        );
                        process::exit(2);
                    }
                }
            }
            if opts.force && opts.offline {
                eprintln!("error: --force and --offline are mutually exclusive");
                process::exit(2);
            }
            if let Err(e) = packages::cmd_update(&project_root, &opts) {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        "pin" => {
            let version_arg = args.get(2).map(|s| s.as_str());
            let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            toolchain::cmd_pin(version_arg, &project_root);
        }
        "sbom" => {
            meta::cmd_sbom(args);
        }
        "audit" => {
            meta::cmd_audit(args);
        }
        "package" => {
            meta::cmd_package(args);
        }
        other => {
            eprintln!("Unknown command: {other}");
            args::print_usage();
            process::exit(1);
        }
    }
}

/// Render a single [`CheckError`] in rustc-style source-context format.
///
/// ```text
/// error[REQ10]: refinement predicate violated
///  --> main.mvl:6:23
///   |
/// 6 |     let result: Int = double(-2);
///   |                       ^^^^^^^^^^ argument to `double` violates refinement `self > 0`
/// ```
///
/// Note: `Span::len` covers the full call expression, so the carets span the
/// entire call site rather than just the invalid argument.
///
/// The message is split on the first `": "` into a short title (header line)
/// and a detail annotation (caret line).  When no colon is present the full
/// message appears on both lines.
pub(super) fn render_diagnostic(file_path: &str, src: &str, err: &CheckError) {
    let span = err.span();
    let req = err.requirement_number();
    let msg = err.message();

    let (title, annotation) = match msg.find(": ") {
        Some(pos) => (&msg[..pos], &msg[pos + 2..]),
        None => (msg.as_str(), msg.as_str()),
    };

    let lines: Vec<&str> = src.lines().collect();
    let source_line = lines
        .get((span.line as usize).saturating_sub(1))
        .copied()
        .unwrap_or("");

    let line_no_str = span.line.to_string();
    let w = line_no_str.len();
    let line_pad = " ".repeat(w); // w spaces  — lines up with `-->`
    let gutter = " ".repeat(w + 1); // w+1 spaces — lines up with line number + space

    let col_0 = (span.col as usize).saturating_sub(1);
    let caret_len = (span.len as usize).max(1);
    let spaces = " ".repeat(col_0);
    let carets = "^".repeat(caret_len);

    eprintln!("error[REQ{req}]: {title}");
    eprintln!(
        "{line_pad}--> {file_path}:{line}:{col}",
        line = span.line,
        col = span.col
    );
    eprintln!("{gutter}|");
    eprintln!("{line_no_str} | {source_line}");
    eprintln!("{gutter}| {spaces}{carets} {annotation}");
}

/// Parse the given `.mvl` file, printing errors and exiting on failure.
pub(super) fn parse_or_exit(path: &str) -> (Program, String) {
    loader::parse_file(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1);
    })
}

/// Find the project root by walking up from `start` until a directory containing
/// `mvl.lock` or `mvl.toml` is found.  Falls back to `start` if neither is found.
///
/// Allows running `mvl check` from any subdirectory (e.g. `make -C examples/foo check`)
/// and still resolve packages declared in the root-level `mvl.lock`.
pub(super) fn find_project_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("mvl.lock").exists() || dir.join("mvl.toml").exists() {
            return dir;
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => return start.to_path_buf(),
        }
    }
}

pub(super) fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_symlink() {
            // Skip symlinks — prevents escaping the source tree (#715).
        } else if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

pub(super) use mvl::mvl::json_util::json_escape;
