use mvl::mvl::checker;
use mvl::mvl::parser::Parser;
use mvl::mvl::transpiler;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("mvl compiler v{}", env!("CARGO_PKG_VERSION"));
        eprintln!("Usage:");
        eprintln!("  mvl check <file.mvl>      — parse and type-check");
        eprintln!("  mvl build <file.mvl>      — transpile to Rust and run cargo build");
        eprintln!("  mvl run   <file.mvl>      — transpile, build, and execute");
        eprintln!("  mvl transpile <file.mvl>  — print transpiled Rust to stdout");
        process::exit(1);
    }

    let cmd = &args[1];
    match cmd.as_str() {
        "check" => {
            let path = require_file_arg(&args, "check");
            cmd_check(&path);
        }
        "build" => {
            let path = require_file_arg(&args, "build");
            build_project(&path, false);
        }
        "run" => {
            let path = require_file_arg(&args, "run");
            build_project(&path, true);
        }
        "transpile" => {
            let path = require_file_arg(&args, "transpile");
            cmd_transpile(&path);
        }
        other => {
            eprintln!("Unknown command: {other}");
            process::exit(1);
        }
    }
}

fn require_file_arg(args: &[String], cmd: &str) -> String {
    if args.len() < 3 {
        eprintln!("Usage: mvl {cmd} <file.mvl>");
        process::exit(1);
    }
    args[2].clone()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Parse and type-check a .mvl file. Exit 0 on success, 1 on errors.
fn cmd_check(path: &str) {
    let (prog, _src) = parse_or_exit(path);
    let result = checker::check(&prog);
    if result.is_ok() {
        println!("{path}: OK");
    } else {
        for err in &result.errors {
            eprintln!("error: {err:?}");
        }
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

/// Transpile a .mvl file to a Cargo project, build it, and optionally run it.
fn build_project(path: &str, run: bool) {
    let (prog, _src) = parse_or_exit(path);
    let crate_name = stem(path);
    let out = transpiler::transpile(&prog, &crate_name);

    // Write to a deterministic temp directory per crate name
    let tmp_dir = std::env::temp_dir().join(format!("mvl_build_{crate_name}"));
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");

    let cargo_toml_path = tmp_dir.join("Cargo.toml");
    fs::write(&cargo_toml_path, &out.cargo_toml).expect("write Cargo.toml");

    if out.has_main {
        // Binary crate: the transpiled code IS src/main.rs
        fs::write(src_dir.join("main.rs"), &out.lib_rs).expect("write main.rs");
    } else {
        // Library crate: lib.rs + a stub main for cargo build to succeed
        fs::write(src_dir.join("lib.rs"), &out.lib_rs).expect("write lib.rs");
        fs::write(
            src_dir.join("main.rs"),
            transpiler::cargo::emit_main_rs_stub(&crate_name),
        )
        .expect("write stub main.rs");
    }

    // If the program uses mvl_runtime, copy it as a sibling directory
    // so the relative path `../mvl_runtime` in Cargo.toml resolves.
    if out.extern_count > 0 {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mvl_runtime");
        let runtime_dst = tmp_dir.parent().unwrap().join("mvl_runtime");
        if runtime_src.exists() && !runtime_dst.exists() {
            copy_dir_recursive(&runtime_src, &runtime_dst).expect("copy mvl_runtime");
        }
    }

    println!("Transpiled to: {}", tmp_dir.display());

    let cargo_cmd = if run && out.has_main { "run" } else { "build" };
    println!("Running: cargo {cargo_cmd}");

    let status = process::Command::new("cargo")
        .arg(cargo_cmd)
        .current_dir(&tmp_dir)
        .status()
        .expect("failed to run cargo");

    if !status.success() {
        eprintln!("cargo {cargo_cmd} failed");
        process::exit(1);
    }

    if !run || !out.has_main {
        println!("Build successful.");
        if run && !out.has_main {
            eprintln!("Note: no `fn main` in MVL source — nothing to run.");
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

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

/// Extract the file stem (basename without extension) from a path.
fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mvl_program")
        .to_string()
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
