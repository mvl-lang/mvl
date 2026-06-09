// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::AssertMode;
use mvl::mvl::checker::passes::parse_req_filter;
use mvl::mvl::checker::SolverMode;
use std::process;

pub fn print_usage() {
    eprintln!("mvl compiler v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("Usage:");
    eprintln!("  mvl --version, -V                  — show version");
    eprintln!("  mvl --help, -h                     — show this help");
    eprintln!("  mvl check <file|dir>               — parse and type-check");
    eprintln!(
        "  mvl check <file|dir> --req <N>     — run only the Req N verification pass
  mvl check <file|dir> --error-limit=N — stop after N errors (default 10; 0 = show all)
  mvl check <file|dir> --format=json  — emit errors as machine-readable JSON
  mvl check <file|dir> --refinement-solver=layered|z3-only|fast-only — solver strategy (default: layered)
  mvl check <file|dir> --refinement-stats — print per-layer refinement proof counts
  mvl check --stdin                   — read MVL source from stdin and type-check it"
    );
    eprintln!("  mvl build <file|dir>               — transpile to Rust and run cargo build");
    eprintln!("  mvl build <file|dir> --release     — build with release optimizations");
    eprintln!("  mvl run   [--] <file.mvl>          — transpile, build, and execute");
    eprintln!("  mvl run   [--] <file.mvl> --release — build and run with release optimizations");
    eprintln!("  mvl run   [--] <file.mvl> -- ...   — pass args to the compiled binary");
    eprintln!("  mvl test  <file|dir>               — find *_test.mvl files and run cargo test");
    eprintln!("  mvl test  <file|dir> -q            — suppress MVL output, pass -q to cargo test (dot progress)");
    eprintln!("  mvl test  <file|dir> --verbose     — show transpile path and all test names with captured stdout");
    eprintln!(
        "  mvl test  <file|dir> --coverage    — run with native behavioral branch coverage report
  mvl build <file|dir> --backend=llvm          — compile to LLVM IR text and emit .ll file
  mvl run   <file|dir> --backend=llvm          — compile and run via lli
  mvl test  <file|dir> --expect                — compile + run via Rust transpiler, check // expect: annotations
  mvl test  <file|dir> --backend=llvm          — compile + run via lli, check // expect: annotations
  mvl build|run|check|test <file|dir> --stdlib=trusted — stdlib profile: trusted (default, 95 builtins)
  mvl build|run|check|test <file|dir> --stdlib=proven  — proven profile: verifies stdlib before user code (ADR-0023)
  mvl build|run <file|dir> --assert-mode=always     — enforce invariants unconditionally (default)
  mvl build|run <file|dir> --assert-mode=debug-only — enforce invariants in debug builds only
  mvl build|run <file|dir> --assert-mode=assume     — emit llvm.assume hint; no runtime trap
  mvl build|run <file|dir> --target=default         — actor runtime: std::thread + mpsc (default)
  mvl build|run <file|dir> --target=tokio           — actor runtime: tokio tasks + channels"
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
    eprintln!("  mvl fuzz   <file|dir>               — type-directed input fuzzing from Tainted[T] signatures (Phase 8)");
    eprintln!("  mvl fuzz   <file|dir> --list        — list fuzzable functions (those with Tainted[T] params)");
    eprintln!("  mvl fuzz   <file|dir> --target <fn> — fuzz a specific function");
    eprintln!("  mvl fuzz   <file|dir> --time <Ns>   — stop after N seconds (e.g. --time 60s)");
    eprintln!("  mvl fuzz   <file|dir> --corpus <dir> — seed corpus directory");
    eprintln!("  mvl mcdc   <file|dir>               — MC/DC coverage analysis (DO-178C DAL-A)");
    eprintln!("  mvl mcdc   <file|dir> -q            — quiet: only show MC/DC score");
    eprintln!("  mvl mcdc   <file|dir> --verbose     — full covered/missed clause report");
    eprintln!("  mvl mcdc   <file|dir> --masking     — masking MC/DC (DO-178C): exempt coupled obligations");
    eprintln!(
        "  mvl mcdc   <file|dir> --json        — machine-readable JSON output for CI integration"
    );
    eprintln!("  mvl mcdc   <file|dir> --json -q     — JSON summary only (no per-clause detail)");
    eprintln!("  mvl fmt   <file|dir>               — format MVL source files in place");
    eprintln!(
        "  mvl fmt   <file|dir> --check       — exit 1 if any file is not formatted (CI gate)"
    );
    eprintln!("  mvl fmt   <file|dir> --stdout      — write formatted output to stdout, do not modify file");
    eprintln!(
        "  mvl fmt   --stdin                  — read from stdin, write formatted output to stdout"
    );
    eprintln!("  mvl lint  <file|dir>               — check style rules");
    eprintln!("  mvl lint  <file|dir> --show-config — show active linter configuration");
    eprintln!("  mvl assurance <file|dir>           — emit assurance report");
    eprintln!("  mvl assurance <file|dir> --json    — emit assurance report as JSON");
    eprintln!("  mvl assurance <file|dir> --verbose — per-function requirement detail");
    eprintln!(
        "  mvl openapi <file|dir>              — generate OpenAPI 3.0.3 JSON from route table"
    );
    eprintln!("  mvl transpile <file.mvl>           — print transpiled Rust to stdout");
    eprintln!(
        "  mvl init [<name>]                  — scaffold a new project (mvl.toml + src/main.mvl)"
    );
    eprintln!("  mvl self init                      — extract stdlib to XDG_DATA_HOME/mvl/toolchains/VERSION/std/");
    eprintln!("  mvl self install <version>         — download and install a toolchain version");
    eprintln!("  mvl self use <version>             — activate an installed toolchain version");
    eprintln!("  mvl self list                      — list installed toolchain versions");
    eprintln!("  mvl self uninstall <version>       — remove an installed toolchain version");
    eprintln!("  mvl add <pkg-id> [<tag>]           — fetch package, add to mvl.toml + mvl.lock");
    eprintln!("  mvl install                        — fetch all deps from mvl.lock, verify hashes");
    eprintln!("  mvl update                         — re-resolve versions, update mvl.lock");
    eprintln!("  mvl pin [<version>]                — pin project to compiler version (writes .mvl-version)");
    eprintln!("  mvl sbom [--format=cyclonedx|spdx] — generate SBOM from mvl.lock (default: CycloneDX JSON)");
    eprintln!("           [--output=<file>]          — write SBOM to file instead of stdout");
}

/// Parse `--error-limit=N` from args; 0 means unlimited, default is 10.
pub(super) fn parse_error_limit(args: &[String]) -> usize {
    args.iter()
        .find_map(|a| a.strip_prefix("--error-limit="))
        .and_then(|s| s.parse().ok())
        .unwrap_or(10)
}

/// Parse `--assert-mode=<mode>` from args; defaults to `AssertMode::Always`.
pub(super) fn parse_assert_mode_or_exit(args: &[String]) -> AssertMode {
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
pub(super) fn parse_backend(args: &[String]) -> &str {
    args.iter()
        .find_map(|a| a.strip_prefix("--backend="))
        .unwrap_or("rust")
}

/// Parse `--target=<name>` from args; defaults to `"default"`.
///
/// Valid targets: `default` (std::thread + mpsc), `tokio` (tokio tasks + channels).
pub(super) fn parse_target_or_exit(args: &[String]) -> &str {
    let target = args
        .iter()
        .find_map(|a| a.strip_prefix("--target="))
        .unwrap_or("default");
    match target {
        "default" | "tokio" => target,
        other => {
            eprintln!("error: unknown target '{other}' (supported: default, tokio)");
            process::exit(1);
        }
    }
}

/// Parse `--stdlib=<profile>` from args; defaults to `"trusted"`.
pub(super) fn parse_stdlib_profile(args: &[String]) -> &'static str {
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

pub(super) fn parse_req_filter_or_exit(args: &[String]) -> Option<u8> {
    parse_req_filter(args).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        process::exit(1);
    })
}

/// Returns the index of the path argument, skipping flags and an optional `--` separator.
///
/// Handles `mvl check --verbose compiler/` (flags before path) and
/// `mvl check -- compiler/` (explicit separator) for all subcommands (#728).
pub(super) fn path_arg_index(args: &[String]) -> usize {
    let mut idx = 2;
    while idx < args.len() {
        if args[idx] == "--" {
            return idx + 1;
        }
        if !args[idx].starts_with('-') {
            return idx;
        }
        idx += 1;
    }
    idx
}

/// Parse `--refinement-solver=<mode>` from args; defaults to `SolverMode::Layered`.
pub(super) fn parse_solver_mode_or_exit(args: &[String]) -> SolverMode {
    let mode_str = args
        .iter()
        .find_map(|a| a.strip_prefix("--refinement-solver="))
        .unwrap_or("layered");
    match mode_str {
        "layered" => SolverMode::Layered,
        "z3-only" => SolverMode::Z3Only,
        "fast-only" => SolverMode::FastOnly,
        other => {
            eprintln!(
                "error: unknown refinement-solver '{other}' (supported: layered, z3-only, fast-only)"
            );
            process::exit(1);
        }
    }
}

pub(super) fn require_path_arg(args: &[String], cmd: &str) -> String {
    let idx = path_arg_index(args);
    if args.len() <= idx {
        eprintln!("Usage: mvl {cmd} [--] <file.mvl|directory>");
        process::exit(1);
    }
    args[idx].clone()
}
