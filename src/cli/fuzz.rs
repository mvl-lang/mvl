// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Decl, TypeExpr};
use std::fs;
use std::path::PathBuf;
use std::process;

/// A fuzz target derived from a single function signature.
struct FuzzTarget {
    fn_name: String,
    /// Each Tainted[T] parameter: (param_name, inner_type_name).
    tainted_params: Vec<(String, String)>,
}

pub fn run(
    path: &str,
    target: Option<&str>,
    time_secs: Option<u64>,
    corpus: Option<&str>,
    list: bool,
) {
    let files = loader::mvl_files(path, false);
    if files.is_empty() {
        eprintln!("No .mvl files found at: {path}");
        process::exit(1);
    }

    let targets = collect_fuzz_targets(&files, path);

    if list {
        if targets.is_empty() {
            println!("No functions with Tainted parameters found in: {path}");
        } else {
            println!("Fuzzable functions in {path}:");
            for t in &targets {
                println!("  {} ({})", t.fn_name, format_params(&t.tainted_params));
            }
        }
        return;
    }

    if targets.is_empty() {
        eprintln!("No functions with Tainted parameters found in: {path}");
        eprintln!("  Fuzz targets are derived from functions with Tainted[T] parameters.");
        process::exit(1);
    }

    let fuzz_target = select_target(&targets, target, path);

    println!(
        "Fuzzing: {} ({})",
        fuzz_target.fn_name,
        format_params(&fuzz_target.tainted_params)
    );

    let tmp_dir = std::env::temp_dir().join(format!("mvl_fuzz_{}", process::id()));
    build_fuzz_workspace(&files, fuzz_target, &tmp_dir, corpus);

    run_cargo_fuzz(fuzz_target, &tmp_dir, time_secs, corpus);
}

// ── Target collection ────────────────────────────────────────────────────────

fn collect_fuzz_targets(files: &[PathBuf], path: &str) -> Vec<FuzzTarget> {
    let mut targets = Vec::new();
    for file in files {
        let file_str = file.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if fd.is_builtin || fd.is_test {
                    continue;
                }
                let tainted: Vec<(String, String)> = fd
                    .params
                    .iter()
                    .filter_map(|p| {
                        if let TypeExpr::Labeled { label, inner, .. } = &p.ty {
                            if label == "Tainted" {
                                return Some((p.name.clone(), inner_type_name(inner)));
                            }
                        }
                        None
                    })
                    .collect();
                if !tainted.is_empty() {
                    targets.push(FuzzTarget {
                        fn_name: fd.name.clone(),
                        tainted_params: tainted,
                    });
                }
            }
        }
    }
    // Warn about non-fuzzable files only when no targets found (avoid noise).
    if targets.is_empty() {
        eprintln!("hint: searched {} file(s) under {path}", files.len());
    }
    targets
}

fn select_target<'a>(targets: &'a [FuzzTarget], name: Option<&str>, path: &str) -> &'a FuzzTarget {
    if let Some(n) = name {
        targets.iter().find(|t| t.fn_name == n).unwrap_or_else(|| {
            eprintln!("error: no fuzzable function named '{n}'");
            eprintln!("  Run `mvl fuzz {path} --list` to see available targets");
            process::exit(1);
        })
    } else if targets.len() == 1 {
        &targets[0]
    } else {
        eprintln!(
            "error: {} fuzzable functions found — use --target <fn>",
            targets.len()
        );
        for t in targets {
            eprintln!("  {}", t.fn_name);
        }
        process::exit(1);
    }
}

// ── Workspace generation ─────────────────────────────────────────────────────

fn build_fuzz_workspace(
    files: &[PathBuf],
    target: &FuzzTarget,
    tmp_dir: &std::path::Path,
    corpus: Option<&str>,
) {
    let src_dir = tmp_dir.join("src");
    let fuzz_dir = tmp_dir.join("fuzz");
    let targets_dir = fuzz_dir.join("fuzz_targets");

    for dir in [&src_dir, &targets_dir] {
        fs::create_dir_all(dir).unwrap_or_else(|e| {
            eprintln!("Cannot create temp dir {}: {e}", dir.display());
            process::exit(1);
        });
    }

    // Transpile all source files into a single lib.rs.
    let (lib_rs, need_runtime) = transpile_to_lib(files);
    fs::write(src_dir.join("lib.rs"), &lib_rs).unwrap_or_else(|e| {
        eprintln!("Cannot write lib.rs: {e}");
        process::exit(1);
    });

    // Root Cargo.toml (lib crate).
    let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("runtime")
        .join("rust");
    let runtime_dep = if need_runtime && runtime_src.exists() {
        format!("mvl_runtime = {{ path = \"{}\" }}\n", runtime_src.display())
    } else {
        String::new()
    };
    fs::write(
        tmp_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"mvl_fuzz_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"mvl_fuzz_lib\"\n\n\
             [dependencies]\n{runtime_dep}"
        ),
    )
    .unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });

    // fuzz/Cargo.toml — cargo-fuzz sub-crate.
    fs::write(
        fuzz_dir.join("Cargo.toml"),
        format!(
            "[workspace]\n\n\
             [package]\nname = \"mvl-fuzz\"\nversion = \"0.0.0\"\npublish = false\nedition = \"2021\"\n\n\
             [package.metadata]\ncargo-fuzz = true\n\n\
             [dependencies]\nlibfuzzer-sys = \"0.4\"\n\n\
             [dependencies.mvl_fuzz_lib]\npath = \"..\"\n\n\
             [[bin]]\nname = \"{fn_name}\"\npath = \"fuzz_targets/{fn_name}.rs\"\ntest = false\ndoc = false\n",
            fn_name = target.fn_name
        ),
    )
    .unwrap_or_else(|e| {
        eprintln!("Cannot write fuzz/Cargo.toml: {e}");
        process::exit(1);
    });

    // fuzz/fuzz_targets/<fn_name>.rs
    let harness = generate_harness(target);
    fs::write(targets_dir.join(format!("{}.rs", target.fn_name)), &harness).unwrap_or_else(|e| {
        eprintln!("Cannot write fuzz harness: {e}");
        process::exit(1);
    });

    // Seed corpus directory inside the workspace (cargo fuzz default).
    if let Some(c) = corpus {
        let corpus_path = std::path::Path::new(c);
        if !corpus_path.exists() {
            eprintln!("error: corpus directory not found: {c}");
            process::exit(1);
        }
    }

    println!("Fuzz workspace: {}", tmp_dir.display());
}

fn transpile_to_lib(files: &[PathBuf]) -> (String, bool) {
    let mut stdlib_prelude = loader::load_implicit_prelude();
    let all_progs: Vec<_> = files
        .iter()
        .map(|f| super::parse_or_exit(&f.display().to_string()).0)
        .collect();
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    stdlib_prelude.extend(loader::load_pkg_modules(&all_progs, &project_root));
    stdlib_prelude.extend(loader::load_mvl_native_stdlib_extras(&all_progs));

    let mut need_runtime = transpiler::prelude_requires_runtime(&stdlib_prelude);
    let mut lib_rs = String::from(
        "#![allow(dead_code, unused_variables, unused_imports, unused_parens, unused_unsafe, non_snake_case)]\n\n",
    );

    for file in files {
        let file_str = file.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        let s = loader::stem(&file_str);
        let module_name = s.replace('-', "_");
        let out = transpiler::transpile(
            &prog,
            transpiler::TranspileConfig::new(&module_name).with_prelude(stdlib_prelude.clone()),
        )
        .output;
        if out.has_extern_rust || transpiler::has_std_imports(&prog) {
            need_runtime = true;
        }
        lib_rs.push_str(&out.lib_rs);
        lib_rs.push('\n');
    }

    (lib_rs, need_runtime)
}

// ── Harness generation ───────────────────────────────────────────────────────

fn generate_harness(target: &FuzzTarget) -> String {
    let fn_name = &target.fn_name;
    let count = target.tainted_params.len();

    let mut setup = String::new();
    let mut call_args: Vec<String> = Vec::new();

    if count == 1 {
        let (name, ty) = &target.tainted_params[0];
        setup.push_str(&gen_param_from_bytes(name, ty, "data"));
        call_args.push(name.clone());
    } else {
        // Split the fuzz input evenly across multiple Tainted params.
        setup.push_str(&format!("    let chunk = data.len() / {count};\n"));
        for (i, (name, ty)) in target.tainted_params.iter().enumerate() {
            let slice = if i + 1 == count {
                format!("&data[chunk * {i}..]")
            } else {
                format!("&data[chunk * {i}..chunk * {}]", i + 1)
            };
            let var = format!("{name}_bytes");
            setup.push_str(&format!("    let {var} = {slice};\n"));
            setup.push_str(&gen_param_from_bytes(name, ty, &var));
            call_args.push(name.clone());
        }
    }

    format!(
        "#![no_main]\n\
         use libfuzzer_sys::fuzz_target;\n\
         #[allow(unused_imports)]\n\
         use mvl_fuzz_lib::*;\n\
         #[allow(unused_imports)]\n\
         use mvl_runtime::{{Tainted, Clean, Secret}};\n\n\
         fuzz_target!(|data: &[u8]| {{\n\
         {setup}\
             let _ = {fn_name}({args});\n\
         }});\n",
        args = call_args.join(", ")
    )
}

/// Emit Rust setup code to produce `let <name> = Tainted::new(...)` from `bytes_var`.
fn gen_param_from_bytes(name: &str, ty: &str, bytes_var: &str) -> String {
    match ty {
        "String" => format!(
            "    let {name} = Tainted::new(String::from_utf8_lossy({bytes_var}).into_owned());\n"
        ),
        "Int" => format!(
            "    if {bytes_var}.len() < 8 {{ return; }}\n\
             \x20   let {name} = Tainted::new(i64::from_le_bytes({bytes_var}[..8].try_into().unwrap()));\n"
        ),
        "Float" => format!(
            "    if {bytes_var}.len() < 8 {{ return; }}\n\
             \x20   let {name} = Tainted::new(f64::from_le_bytes({bytes_var}[..8].try_into().unwrap()));\n"
        ),
        "Bool" => format!(
            "    if {bytes_var}.is_empty() {{ return; }}\n\
             \x20   let {name} = Tainted::new({bytes_var}[0] & 1 == 1);\n"
        ),
        // List[Byte] or Bytes -> Vec<u8>
        "List" | "Byte" | "Bytes" => {
            format!("    let {name} = Tainted::new({bytes_var}.to_vec());\n")
        }
        // Fallback: treat as String
        _ => format!(
            "    let {name} = Tainted::new(String::from_utf8_lossy({bytes_var}).into_owned());\n"
        ),
    }
}

/// Extract the base type name from a TypeExpr (strips labels, refs, refinements).
fn inner_type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Base { name, .. } => name.clone(),
        TypeExpr::Labeled { inner, .. }
        | TypeExpr::Refined { inner, .. }
        | TypeExpr::Ref { inner, .. }
        | TypeExpr::Option { inner, .. } => inner_type_name(inner),
        _ => "String".to_string(),
    }
}

fn format_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(n, ty)| format!("{n}: Tainted[{ty}]"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ── Execution ────────────────────────────────────────────────────────────────

fn run_cargo_fuzz(
    target: &FuzzTarget,
    tmp_dir: &std::path::Path,
    time_secs: Option<u64>,
    corpus: Option<&str>,
) {
    let fn_name = &target.fn_name;
    print!("Running: cargo +nightly fuzz run {fn_name}");
    if let Some(t) = time_secs {
        print!(" (timeout: {t}s)");
    }
    println!();

    let mut cmd = process::Command::new("cargo");
    cmd.arg("+nightly")
        .arg("fuzz")
        .arg("run")
        .arg(fn_name)
        .current_dir(tmp_dir);

    if let Some(c) = corpus {
        cmd.arg(c);
    }

    if let Some(t) = time_secs {
        cmd.arg("--").arg(format!("-max_total_time={t}"));
    }

    let status = cmd.status().unwrap_or_else(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            eprintln!("error: `cargo` not found — install Rust from https://rustup.rs/");
        } else {
            eprintln!("error: failed to run cargo fuzz: {e}");
            eprintln!("  cargo-fuzz requires nightly: rustup toolchain install nightly");
            eprintln!("  install cargo-fuzz:          cargo install cargo-fuzz");
        }
        process::exit(1);
    });

    // Crash artifacts are in tmp_dir/fuzz/artifacts/<fn_name>/ — console output is
    // the primary report; libfuzzer prints findings directly to stderr.
    if !status.success() {
        let artifacts = tmp_dir.join("fuzz").join("artifacts").join(fn_name);
        if artifacts.exists() {
            eprintln!("\nCrash inputs saved to: {}", artifacts.display());
            eprintln!("Reproduce with: cargo +nightly fuzz run {fn_name} <crash-input>");
        }
    }

    process::exit(status.code().unwrap_or(1));
}
