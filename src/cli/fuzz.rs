// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::parser::ast::{Block, Decl, ElseBranch, Expr, ExternDecl, MatchBody, Stmt, TypeExpr};
use mvl::mvl::pipeline::{load_full_prelude, PreludeMode};
use std::fs;
use std::path::PathBuf;
use std::process;

struct FuzzParam {
    name: String,
    ty_name: String,
    is_tainted: bool,
}

/// A fuzz target derived from a single function signature.
struct FuzzTarget {
    fn_name: String,
    /// Source file this function lives in (used to build a minimal fuzz workspace).
    source_file: PathBuf,
    /// All parameters; Tainted ones receive fuzz input, others get zero-value defaults.
    params: Vec<FuzzParam>,
}

impl FuzzTarget {
    fn tainted_params(&self) -> impl Iterator<Item = &FuzzParam> {
        self.params.iter().filter(|p| p.is_tainted)
    }
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
                println!("  {} ({})", t.fn_name, format_tainted_params(t));
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
        format_tainted_params(fuzz_target)
    );

    let tmp_dir = std::env::temp_dir().join(format!("mvl_fuzz_{}", process::id()));
    // Only compile the file that contains the target function — avoids pulling in
    // unrelated files whose generated code may have compile issues under nightly/ASAN.
    build_fuzz_workspace(
        std::slice::from_ref(&fuzz_target.source_file),
        fuzz_target,
        &tmp_dir,
        corpus,
    );

    run_cargo_fuzz(fuzz_target, &tmp_dir, time_secs, corpus);
}

// ── Target collection ────────────────────────────────────────────────────────

fn file_extern_names(decls: &[Decl]) -> Vec<String> {
    decls
        .iter()
        .filter_map(|d| {
            if let Decl::Extern(e) = d {
                Some(e)
            } else {
                None
            }
        })
        .flat_map(|e: &ExternDecl| e.fns.iter().map(|f| f.name.clone()))
        .collect()
}

// ── Extern call detection (AST walk) ─────────────────────────────────────────

fn block_calls_extern(block: &Block, names: &[String]) -> bool {
    block.stmts.iter().any(|s| stmt_calls_extern(s, names))
}

fn stmt_calls_extern(stmt: &Stmt, names: &[String]) -> bool {
    match stmt {
        Stmt::Let { init, .. } => expr_calls_extern(init, names),
        Stmt::Assign { value, .. } => expr_calls_extern(value, names),
        Stmt::Return { value, .. } => value.as_ref().is_some_and(|e| expr_calls_extern(e, names)),
        Stmt::If {
            cond, then, else_, ..
        } => {
            expr_calls_extern(cond, names)
                || block_calls_extern(then, names)
                || else_.as_ref().is_some_and(|e| match e {
                    ElseBranch::Block(b) => block_calls_extern(b, names),
                    ElseBranch::If(s) => stmt_calls_extern(s, names),
                })
        }
        Stmt::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_extern(scrutinee, names)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_calls_extern(e, names),
                    MatchBody::Block(b) => block_calls_extern(b, names),
                })
        }
        Stmt::For { iter, body, .. } => {
            expr_calls_extern(iter, names) || block_calls_extern(body, names)
        }
        Stmt::While {
            cond,
            body,
            decreases,
            ..
        } => {
            expr_calls_extern(cond, names)
                || block_calls_extern(body, names)
                || decreases
                    .as_ref()
                    .is_some_and(|d| expr_calls_extern(d, names))
        }
        Stmt::Expr { expr, .. } => expr_calls_extern(expr, names),
    }
}

fn expr_calls_extern(expr: &Expr, names: &[String]) -> bool {
    match expr {
        Expr::FnCall { name, args, .. } => {
            names.iter().any(|n| n == name) || args.iter().any(|a| expr_calls_extern(a, names))
        }
        Expr::MethodCall { receiver, args, .. } => {
            expr_calls_extern(receiver, names) || args.iter().any(|a| expr_calls_extern(a, names))
        }
        Expr::FieldAccess { expr, .. }
        | Expr::Unary { expr, .. }
        | Expr::Propagate { expr, .. }
        | Expr::Consume { expr, .. }
        | Expr::Borrow { expr, .. }
        | Expr::As { expr, .. } => expr_calls_extern(expr, names),
        Expr::Relabel { expr, .. } => expr_calls_extern(expr, names),
        Expr::Binary { left, right, .. } => {
            expr_calls_extern(left, names) || expr_calls_extern(right, names)
        }
        Expr::If {
            cond, then, else_, ..
        } => {
            expr_calls_extern(cond, names)
                || block_calls_extern(then, names)
                || else_.as_ref().is_some_and(|e| expr_calls_extern(e, names))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_calls_extern(scrutinee, names)
                || arms.iter().any(|arm| match &arm.body {
                    MatchBody::Expr(e) => expr_calls_extern(e, names),
                    MatchBody::Block(b) => block_calls_extern(b, names),
                })
        }
        Expr::Lambda { body, .. } => expr_calls_extern(body, names),
        Expr::Block(b) => block_calls_extern(b, names),
        Expr::Construct { fields, .. } | Expr::Spawn { fields, .. } => {
            fields.iter().any(|(_, e)| expr_calls_extern(e, names))
        }
        Expr::List { elems, .. } | Expr::Set { elems, .. } => {
            elems.iter().any(|e| expr_calls_extern(e, names))
        }
        Expr::Map { pairs, .. } => pairs
            .iter()
            .any(|(k, v)| expr_calls_extern(k, names) || expr_calls_extern(v, names)),
        Expr::Select { arms, .. } => arms
            .iter()
            .any(|arm| expr_calls_extern(&arm.expr, names) || block_calls_extern(&arm.body, names)),
        Expr::Literal(_, _) | Expr::Ident(_, _) | Expr::Quantifier(..) => false,
    }
}

fn collect_fuzz_targets(files: &[PathBuf], path: &str) -> Vec<FuzzTarget> {
    let mut targets = Vec::new();
    for file in files {
        let file_str = file.display().to_string();
        let (prog, _src) = super::parse_or_exit(&file_str);
        let extern_names = file_extern_names(&prog.declarations);

        for decl in &prog.declarations {
            if let Decl::Fn(fd) = decl {
                if fd.is_builtin || fd.is_test {
                    continue;
                }
                // Skip functions that directly call extern bridge functions — their stubs
                // panic on every input, making fuzzing meaningless without a linked bridge.
                if !extern_names.is_empty() && block_calls_extern(&fd.body, &extern_names) {
                    eprintln!(
                        "note: skipping {}::{} — calls extern bridge functions; \
                         fuzz via the bridge test suite instead",
                        file.file_name().unwrap_or_default().to_string_lossy(),
                        fd.name
                    );
                    continue;
                }
                let all_params: Vec<FuzzParam> = fd
                    .params
                    .iter()
                    .map(|p| {
                        let (ty_name, is_tainted) =
                            if let TypeExpr::Labeled { label, inner, .. } = &p.ty {
                                if label == "Tainted" {
                                    (inner_type_name(inner), true)
                                } else {
                                    (inner_type_name(inner), false)
                                }
                            } else {
                                (inner_type_name(&p.ty), false)
                            };
                        FuzzParam {
                            name: p.name.clone(),
                            ty_name,
                            is_tainted,
                        }
                    })
                    .collect();
                let has_tainted = all_params.iter().any(|p| p.is_tainted);
                if has_tainted {
                    targets.push(FuzzTarget {
                        fn_name: fd.name.clone(),
                        source_file: file.clone(),
                        params: all_params,
                    });
                }
            }
        }
    }
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

    // Transpile using transpile_project so each file becomes its own Rust module;
    // this avoids duplicate prelude definitions that arise from flat concatenation.
    let project_out = transpile_project(files);
    fs::write(src_dir.join("lib.rs"), &project_out.lib_rs).unwrap_or_else(|e| {
        eprintln!("Cannot write lib.rs: {e}");
        process::exit(1);
    });
    for (mod_name, mod_src) in &project_out.module_files {
        fs::write(src_dir.join(format!("{mod_name}.rs")), mod_src).unwrap_or_else(|e| {
            eprintln!("Cannot write {mod_name}.rs: {e}");
            process::exit(1);
        });
    }

    // Root Cargo.toml (lib crate).
    let need_runtime = project_out.need_runtime;
    let runtime_dep = if need_runtime {
        let runtime_src = mvl::mvl::runtime_xdg::ensure_runtime_rust();
        format!(
            "mvl_runtime = {{ path = \"{}\", package = \"mvl_runtime_rust\" }}\n",
            runtime_src.display()
        )
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
    // mvl_runtime is a direct dep so the harness can use Tainted/Clean/Secret directly.
    let fuzz_runtime_dep = if need_runtime {
        let runtime_src = mvl::mvl::runtime_xdg::ensure_runtime_rust();
        format!(
            "[dependencies.mvl_runtime]\npath = \"{}\"\npackage = \"mvl_runtime_rust\"\n\n",
            runtime_src.display()
        )
    } else {
        String::new()
    };
    fs::write(
        fuzz_dir.join("Cargo.toml"),
        format!(
            "[workspace]\n\n\
             [package]\nname = \"mvl-fuzz\"\nversion = \"0.0.0\"\npublish = false\nedition = \"2021\"\n\n\
             [package.metadata]\ncargo-fuzz = true\n\n\
             [dependencies]\nlibfuzzer-sys = \"0.4\"\n\n\
             [dependencies.mvl_fuzz_lib]\npath = \"..\"\n\n\
             {fuzz_runtime_dep}\
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

struct FuzzLibOutput {
    /// Content for src/lib.rs (entry module + pub mod declarations).
    lib_rs: String,
    /// (name, content) pairs for src/{name}.rs sibling modules.
    module_files: Vec<(String, String)>,
    need_runtime: bool,
}

fn transpile_project(files: &[PathBuf]) -> FuzzLibOutput {
    let all_progs: Vec<(String, mvl::mvl::parser::ast::Program)> = files
        .iter()
        .map(|f| {
            let file_str = f.display().to_string();
            let stem = loader::stem(&file_str).replace('-', "_");
            (stem, super::parse_or_exit(&file_str).0)
        })
        .collect();

    let mut stdlib_prelude = loader::load_implicit_prelude();
    let progs_only: Vec<_> = all_progs.iter().map(|(_, p)| p.clone()).collect();
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    stdlib_prelude.extend(loader::load_pkg_modules(
        &progs_only,
        &project_root,
        &mut std::collections::HashSet::new(),
    ));
    stdlib_prelude.extend(load_full_prelude(progs_only.iter(), PreludeMode::Transpile));

    // Treat the first file as entry and the rest as siblings — transpile_project
    // handles the prelude correctly: entry gets full emission, siblings share it.
    let (entry_name, entry_prog) = &all_progs[0];
    let siblings: Vec<(String, mvl::mvl::parser::ast::Program)> = all_progs[1..].to_vec();

    // Collect expression types from ALL programs (prelude + user) so the emitter
    // can distinguish list vs string concat etc. — same pattern as `mvl build`.
    let mut all_expr_types = checker::collect_prelude_expr_types(&stdlib_prelude);
    let check_result = checker::check_with_prelude(&stdlib_prelude, entry_prog);
    all_expr_types.extend(check_result.expr_types);
    // Pre-check each sibling so the backend receives ready-made expr_types (#1110).
    let sibling_expr_types: Vec<_> = siblings
        .iter()
        .map(|(_, sibling)| {
            let mut t = checker::collect_prelude_expr_types(&stdlib_prelude);
            t.extend(checker::check_with_prelude(&stdlib_prelude, sibling).expr_types);
            t
        })
        .collect();

    let out = transpiler::transpile_project_with_options(
        entry_name,
        entry_prog,
        &siblings,
        &stdlib_prelude,
        all_expr_types,
        &sibling_expr_types,
        mvl::mvl::backends::AssertMode::Assume, // don't panic on refinement violations in fuzz
        false, // optimize_proved: keep bounds checks in fuzz corpus for safety
        true,  // extern_stubs: extern "rust" → todo!() so the harness links without bridges
        &[],
    );

    // The entry `main_rs` may be a binary stub when the first file has `fn main`.
    // We need a lib crate, so wrap everything under pub mod if needed, or use the
    // library output directly. transpile_project gives us the right module structure.
    let need_runtime = out.use_mvl_runtime;

    // Re-export all public items from sibling modules so the harness can use
    // `use mvl_fuzz_lib::*;` and resolve any function regardless of which file it's in.
    let mut lib_rs = out.main_rs.clone();
    for (mod_name, _) in &out.module_files {
        lib_rs.push_str(&format!("pub use {mod_name}::*;\n"));
    }

    FuzzLibOutput {
        lib_rs,
        module_files: out.module_files,
        need_runtime,
    }
}

// ── Harness generation ───────────────────────────────────────────────────────

fn generate_harness(target: &FuzzTarget) -> String {
    let fn_name = &target.fn_name;
    let tainted_count = target.tainted_params().count();

    let mut setup = String::new();
    let mut call_args: Vec<String> = Vec::new();

    // Split fuzz input evenly across Tainted params; assign chunk indices.
    let mut tainted_idx = 0usize;
    if tainted_count > 1 {
        setup.push_str(&format!("    let chunk = data.len() / {tainted_count};\n"));
    }

    for p in &target.params {
        if p.is_tainted {
            let bytes_var = if tainted_count == 1 {
                "data".to_string()
            } else {
                let var = format!("{}_bytes", p.name);
                let slice = if tainted_idx + 1 == tainted_count {
                    format!("&data[chunk * {tainted_idx}..]")
                } else {
                    format!("&data[chunk * {tainted_idx}..chunk * {}]", tainted_idx + 1)
                };
                setup.push_str(&format!("    let {var} = {slice};\n"));
                tainted_idx += 1;
                var
            };
            setup.push_str(&gen_tainted_from_bytes(&p.name, &p.ty_name, &bytes_var));
        } else {
            setup.push_str(&gen_plain_default(&p.name, &p.ty_name));
        }
        call_args.push(p.name.clone());
    }

    let args = call_args.join(", ");
    format!(
        "#![no_main]\n\
         use libfuzzer_sys::fuzz_target;\n\
         #[allow(unused_imports)]\n\
         use mvl_fuzz_lib::*;\n\
         #[allow(unused_imports)]\n\
         use mvl_runtime::prelude::{{Tainted, Clean, Secret}};\n\n\
         fuzz_target!(|data: &[u8]| {{\n\
         {setup}\
             let _ = {fn_name}({args});\n\
         }});\n"
    )
}

fn gen_tainted_from_bytes(name: &str, ty: &str, bytes_var: &str) -> String {
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
        "List" | "Byte" | "Bytes" => {
            format!("    let {name} = Tainted::new({bytes_var}.to_vec());\n")
        }
        _ => format!(
            "    let {name} = Tainted::new(String::from_utf8_lossy({bytes_var}).into_owned());\n"
        ),
    }
}

/// Emit a zero-value default for a non-Tainted parameter.
fn gen_plain_default(name: &str, ty: &str) -> String {
    match ty {
        "String" => format!("    let {name} = String::new();\n"),
        "Int" => format!("    let {name} = 0i64;\n"),
        "Float" => format!("    let {name} = 0.0f64;\n"),
        "Bool" => format!("    let {name} = false;\n"),
        "List" | "Byte" | "Bytes" => format!("    let {name} = Vec::new();\n"),
        _ => format!("    let {name} = Default::default();\n"),
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

fn format_tainted_params(target: &FuzzTarget) -> String {
    target
        .tainted_params()
        .map(|p| format!("{}: Tainted[{}]", p.name, p.ty_name))
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
