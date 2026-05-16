// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

pub mod args;
pub mod assurance;
pub mod build;
pub mod check;
pub mod complexity;
pub mod lint;
#[cfg(feature = "llvm")]
pub mod llvm;
pub mod mcdc;
pub mod meta;
pub mod mutate;
pub mod test;
pub mod transpile;

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
    // toml.mvl: excluded pending checker fix — proven-mode conflates TomlValue with
    // json::Value when both modules are in scope (shared variant names: Bool, Array, String)
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
            let path = args::require_path_arg(args, "check");
            let req_filter = args::parse_req_filter_or_exit(args);
            let error_limit = args::parse_error_limit(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let format_json = args.iter().any(|a| a == "--format=json");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            check::run(
                &path,
                req_filter,
                check::CheckOptions {
                    error_limit,
                    stdlib_profile,
                    format_json,
                    verbose,
                    solver_mode: args::parse_solver_mode_or_exit(args),
                    refinement_stats: args.iter().any(|a| a == "--refinement-stats"),
                },
            );
        }
        "build" => {
            let path = args::require_path_arg(args, "build");
            let backend = args::parse_backend(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let assert_mode = args::parse_assert_mode_or_exit(args);
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            if verbose {
                eprintln!("stdlib profile: {stdlib_profile}");
            }
            check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                llvm::build_project_llvm(&path, assert_mode);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                build::run(&path, false, &[], assert_mode);
            }
        }
        "run" => {
            let path = args::require_path_arg(args, "run");
            let backend = args::parse_backend(args);
            let stdlib_profile = args::parse_stdlib_profile(args);
            let assert_mode = args::parse_assert_mode_or_exit(args);
            check::maybe_check_proven_stdlib_or_exit(stdlib_profile);
            let path_idx = args::path_arg_index(args);
            let run_args: Vec<String> = args[path_idx + 1..]
                .iter()
                .skip_while(|a| a.as_str() != "--")
                .skip(1)
                .cloned()
                .collect();
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                llvm::run_project_llvm(&path, assert_mode);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                build::run(&path, true, &run_args, assert_mode);
            }
        }
        "transpile" => {
            let path = args::require_path_arg(args, "transpile");
            transpile::run(&path);
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
            if backend == "llvm" {
                #[cfg(feature = "llvm")]
                llvm::cmd_test_llvm(&path, quiet, verbose);
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("error: --backend=llvm requires the `llvm` feature (rebuild with --features llvm)");
                    process::exit(1);
                }
            } else {
                test::run(&path, quiet, verbose, coverage, bdd);
            }
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
        "mcdc" => {
            let path = args::require_path_arg(args, "mcdc");
            let quiet = args.iter().any(|a| a == "--quiet" || a == "-q");
            let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
            let masking = args.iter().any(|a| a == "--masking");
            let json = args.iter().any(|a| a == "--json");
            mcdc::run(&path, quiet, verbose, masking, json);
        }
        "complexity" => {
            let path = args::require_path_arg(args, "complexity");
            let format_json = args.iter().any(|a| a == "--format=json");
            complexity::run(&path, format_json);
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
        "init" => {
            meta::cmd_init();
        }
        "self" => {
            meta::cmd_self(args);
        }
        "add" => {
            meta::cmd_pkg_add(args);
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
            args::print_usage();
            process::exit(1);
        }
    }
}

/// Parse the given `.mvl` file, printing errors and exiting on failure.
pub(super) fn parse_or_exit(path: &str) -> (Program, String) {
    loader::parse_file(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1);
    })
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

pub(super) fn json_escape(s: &str) -> String {
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
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}
