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
            cmd_build(&path);
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
    println!("// === src/lib.rs ===");
    println!("{}", out.lib_rs);
}

/// Transpile a .mvl file to a Cargo project in a temp directory and run `cargo build`.
fn cmd_build(path: &str) {
    let (prog, _src) = parse_or_exit(path);
    let crate_name = stem(path);
    let out = transpiler::transpile(&prog, &crate_name);

    // Write to a temporary Cargo project
    let tmp_dir = std::env::temp_dir().join(format!("mvl_build_{crate_name}"));
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");

    let cargo_toml_path = tmp_dir.join("Cargo.toml");
    let lib_rs_path = src_dir.join("lib.rs");
    let main_rs_path = src_dir.join("main.rs");

    fs::write(&cargo_toml_path, &out.cargo_toml).expect("write Cargo.toml");
    fs::write(&lib_rs_path, &out.lib_rs).expect("write lib.rs");

    let main_content = transpiler::cargo::emit_main_rs(None);
    fs::write(&main_rs_path, main_content).expect("write main.rs");

    println!("Transpiled to: {}", tmp_dir.display());
    println!("Running: cargo build");

    let status = process::Command::new("cargo")
        .arg("build")
        .current_dir(&tmp_dir)
        .status()
        .expect("failed to run cargo");

    if !status.success() {
        eprintln!("cargo build failed");
        process::exit(1);
    }
    println!("Build successful.");
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
