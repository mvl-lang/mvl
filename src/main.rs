use mvl::mvl::checker;
use mvl::mvl::parser::ast::{Decl, Program, Totality};
use mvl::mvl::parser::Parser;
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
        "check" => {
            let path = require_path_arg(&args, "check");
            cmd_check(&path);
        }
        "build" => {
            let path = require_path_arg(&args, "build");
            build_project(&path, false);
        }
        "run" => {
            let path = require_path_arg(&args, "run");
            build_project(&path, true);
        }
        "transpile" => {
            let path = require_path_arg(&args, "transpile");
            cmd_transpile(&path);
        }
        "test" => {
            let path = require_path_arg(&args, "test");
            cmd_test(&path);
        }
        "assurance" => {
            let path = require_path_arg(&args, "assurance");
            let json = args.iter().any(|a| a == "--format=json" || a == "--json");
            cmd_assurance(&path, json);
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
    eprintln!("  mvl check <file|dir>               — parse and type-check");
    eprintln!("  mvl build <file|dir>               — transpile to Rust and run cargo build");
    eprintln!("  mvl run   <file.mvl>               — transpile, build, and execute");
    eprintln!("  mvl test  <file|dir>               — find *_test.mvl files and run cargo test");
    eprintln!("  mvl assurance <file|dir>           — emit assurance report");
    eprintln!("  mvl assurance <file|dir> --json    — emit assurance report as JSON");
    eprintln!("  mvl transpile <file.mvl>           — print transpiled Rust to stdout");
}

fn require_path_arg(args: &[String], cmd: &str) -> String {
    if args.len() < 3 {
        eprintln!("Usage: mvl {cmd} <file.mvl|directory>");
        process::exit(1);
    }
    args[2].clone()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Parse and type-check a .mvl file or all .mvl files in a directory.
fn cmd_check(path: &str) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut had_errors = false;
    for file in &files {
        let file_str = file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let result = checker::check(&prog);
        if result.is_ok() {
            println!("{file_str}: OK");
        } else {
            had_errors = true;
            for err in &result.errors {
                eprintln!("error in {file_str}: {err:?}");
            }
        }
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

/// Transpile a .mvl file to a Cargo project, build it, and optionally run it.
fn build_project(path: &str, run: bool) {
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

    // Transpile each test file and run cargo test in a combined temp project
    let crate_name = "mvl_test";
    let tmp_dir = std::env::temp_dir().join("mvl_test_run");
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");

    // Build a combined Rust test file from all test modules
    let mut combined_rs = String::new();
    combined_rs.push_str("// MVL test runner — generated by `mvl test`\n");
    combined_rs.push_str("// Do not edit; regenerate with `mvl test`.\n\n");

    for test_file in &test_files {
        let file_str = test_file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let module_name = stem(&file_str).trim_end_matches("_test").replace('-', "_");
        let out = transpiler::transpile(&prog, &module_name);
        combined_rs.push_str(&format!("// === {} ===\n", test_file.display()));
        combined_rs.push_str("#[cfg(test)]\n");
        combined_rs.push_str(&format!("mod {module_name} {{\n"));
        combined_rs.push_str("    #[allow(unused)]\n");
        combined_rs.push_str("    use super::*;\n");
        combined_rs.push_str(&out.lib_rs);
        combined_rs.push_str("}\n\n");
    }

    // Write a minimal Cargo.toml for the test runner
    let cargo_toml = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n"
    );
    fs::write(tmp_dir.join("Cargo.toml"), &cargo_toml).expect("write Cargo.toml");
    fs::write(src_dir.join("lib.rs"), &combined_rs).expect("write lib.rs");

    println!("Transpiled tests to: {}", tmp_dir.display());
    println!("Running: cargo test");

    let status = process::Command::new("cargo")
        .arg("test")
        .current_dir(&tmp_dir)
        .status()
        .expect("failed to run cargo test");

    if !status.success() {
        eprintln!("cargo test failed");
        process::exit(1);
    }

    println!("All tests passed.");
}

/// Emit an assurance report for a file or directory.
fn cmd_assurance(path: &str, json: bool) {
    let files = mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let mut total_fns: usize = 0;
    let mut total_verified: usize = 0; // `total fn`
    let mut total_partial: usize = 0; // `partial fn`
    let mut total_pub: usize = 0; // `pub fn`
    let mut total_extern: usize = 0; // extern fn blocks
    let mut check_errors: usize = 0;
    let mut file_count = 0;

    for file in &files {
        let file_str = file.display().to_string();
        let (prog, _src) = parse_or_exit(&file_str);
        let stats = collect_assurance_stats(&prog);
        let result = checker::check(&prog);

        total_fns += stats.fn_count;
        total_verified += stats.total_fn_count;
        total_partial += stats.partial_fn_count;
        total_pub += stats.pub_fn_count;
        total_extern += result.extern_count;
        check_errors += result.errors.len();
        file_count += 1;
    }

    let implemented = total_fns.saturating_sub(total_extern);
    let verified_pct = if total_fns > 0 {
        (total_verified as f64 / total_fns as f64 * 100.0).round() as u32
    } else {
        0
    };
    let extern_pct = if total_fns > 0 {
        (total_extern as f64 / total_fns as f64 * 100.0).round() as u32
    } else {
        0
    };

    if json {
        println!(
            r#"{{
  "files": {file_count},
  "functions": {{
    "total": {total_fns},
    "verified_total": {total_verified},
    "partial": {total_partial},
    "pub": {total_pub},
    "extern": {total_extern},
    "implemented": {implemented}
  }},
  "percentages": {{
    "verified_pct": {verified_pct},
    "extern_pct": {extern_pct}
  }},
  "check_errors": {check_errors}
}}"#
        );
    } else {
        println!("MVL Assurance Report");
        println!("====================");
        println!("Files checked:       {file_count}");
        println!("Functions:           {total_fns}");
        println!("  total fn:          {total_verified} ({verified_pct}% verified)");
        println!("  partial fn:        {total_partial}");
        println!("  pub fn:            {total_pub}");
        println!("  extern fn:         {total_extern} ({extern_pct}% trust boundary)");
        println!("  implemented:       {implemented}");
        println!("Type errors:         {check_errors}");
        if check_errors == 0 {
            println!("Status:              PASS");
        } else {
            println!("Status:              FAIL ({check_errors} errors)");
        }
    }

    if check_errors > 0 {
        process::exit(1);
    }
}

// ── Assurance stats ───────────────────────────────────────────────────────

struct AssuranceStats {
    fn_count: usize,
    total_fn_count: usize,
    partial_fn_count: usize,
    pub_fn_count: usize,
}

fn collect_assurance_stats(prog: &Program) -> AssuranceStats {
    let mut stats = AssuranceStats {
        fn_count: 0,
        total_fn_count: 0,
        partial_fn_count: 0,
        pub_fn_count: 0,
    };
    collect_stats_from_decls(&prog.declarations, &mut stats);
    stats
}

fn collect_stats_from_decls(decls: &[Decl], stats: &mut AssuranceStats) {
    for decl in decls {
        match decl {
            Decl::Fn(fd) => {
                stats.fn_count += 1;
                match fd.totality {
                    Some(Totality::Total) => stats.total_fn_count += 1,
                    Some(Totality::Partial) => stats.partial_fn_count += 1,
                    None => stats.total_fn_count += 1, // implicitly total
                }
                // NOTE: pub_fn_count populated once module resolver (#96) is merged
                let _ = &mut stats.pub_fn_count;
            }
            Decl::Module(md) => {
                collect_stats_from_decls(&md.declarations, stats);
            }
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
