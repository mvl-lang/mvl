// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! `mvl fmt` — format MVL source files.

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process;

use mvl::mvl::printer::format_source;

pub struct FmtOptions {
    /// Exit 1 if any file is not already formatted; do not write files.
    pub check: bool,
    /// Read from stdin and write formatted output to stdout.
    pub stdin: bool,
    /// Write formatted output to stdout instead of modifying the file.
    pub stdout: bool,
}

/// Entry point called from cli::dispatch.
pub fn run(path: &str, opts: FmtOptions) {
    if opts.stdin {
        run_stdin();
        return;
    }

    let p = Path::new(path);
    if p.is_dir() {
        run_dir(p, &opts);
    } else {
        run_file(p, &opts);
    }
}

fn run_stdin() {
    let mut source = String::new();
    io::stdin().read_to_string(&mut source).unwrap_or_else(|e| {
        eprintln!("error reading stdin: {e}");
        process::exit(1);
    });
    match format_source(&source) {
        Ok(formatted) => print!("{}", formatted),
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}

fn run_dir(dir: &Path, opts: &FmtOptions) {
    let mut any_unformatted = false;

    for entry in walkdir(dir) {
        if !fmt_file_entry(&entry, opts) {
            any_unformatted = true;
        }
    }
    if opts.check && any_unformatted {
        process::exit(1);
    }
}

fn fmt_file_entry(path: &Path, opts: &FmtOptions) -> bool {
    run_file_inner(path, opts).unwrap_or_else(|e| {
        eprintln!("{}: {}", path.display(), e);
        false
    })
}

fn run_file(path: &Path, opts: &FmtOptions) {
    match run_file_inner(path, opts) {
        Ok(true) => {}
        Ok(false) => {
            if opts.check {
                eprintln!("{}: not formatted", path.display());
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}

/// Format a single file. Returns `Ok(true)` if file was already formatted (or
/// was successfully written), `Ok(false)` if `--check` would fail, `Err` on
/// I/O or parse error.
fn run_file_inner(path: &Path, opts: &FmtOptions) -> Result<bool, String> {
    let source = fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;

    let formatted = format_source(&source).map_err(|e| format!("{}: {}", path.display(), e))?;

    if opts.check {
        if source == formatted {
            return Ok(true);
        }
        eprintln!("{}: not formatted", path.display());
        return Ok(false);
    }

    if opts.stdout {
        print!("{}", formatted);
        return Ok(true);
    }

    // Write back only if content changed.
    if source != formatted {
        fs::write(path, &formatted).map_err(|e| format!("{}: {}", path.display(), e))?;
    }
    Ok(true)
}

/// Recursively collect `.mvl` files under `dir`.
fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                result.extend(walkdir(&p));
            } else if p.extension().and_then(|s| s.to_str()) == Some("mvl") {
                result.push(p);
            }
        }
    }
    result
}
