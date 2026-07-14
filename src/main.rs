// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::toolchain;
use std::path::PathBuf;
use std::process;

mod cli;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        cli::args::print_usage();
        process::exit(1);
    }

    // ── Phase C: version resolution chain (ADR-0009) ──────────────────────────
    //
    // Skip re-exec for `mvl self …`, `mvl --version`, and `mvl version` — these
    // must always run with the current binary regardless of any project pin.
    let cmd = &args[1];
    // Commands that must always run with the current binary, regardless of any
    // project pin.  Keep this list in sync with the dispatch match arms below.
    let is_toolchain_meta = matches!(
        cmd.as_str(),
        "self"
            | "--version"
            | "-V"
            | "version"
            | "--help"
            | "-h"
            | "help"
            | "init"
            | "pin"
            | "doctor"
    );

    // MVL_NO_REEXEC=1 pins execution to the current binary regardless of any
    // project-level `requires-mvl`. Useful in test loops that want to exercise
    // a freshly-built compiler without a `make install` between iterations
    // (e.g. `make test-rust-llvm` after a code change on the emitter).
    let no_reexec = std::env::var("MVL_NO_REEXEC")
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false);

    if !is_toolchain_meta && !no_reexec {
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
        cli::args::print_usage();
        process::exit(0);
    }

    cli::dispatch(&args);
}
