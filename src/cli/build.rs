// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::backends::AssertMode;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::resolver;
use mvl::mvl::stdlib;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

fn inject_mod_bridge(source: &str) -> String {
    const MARKER: &str = "use mvl_runtime::prelude::*;";
    let mut result = String::with_capacity(source.len() + 20);
    let mut injected = false;
    for line in source.lines() {
        result.push_str(line);
        result.push('\n');
        if !injected && line.trim() == MARKER {
            result.push_str("mod bridge;\n");
            injected = true;
        }
    }
    if !injected {
        // Fallback: marker absent — prepend mod bridge;
        let mut fallback = String::with_capacity(result.len() + 20);
        fallback.push_str("mod bridge;\n");
        fallback.push_str(&result);
        return fallback;
    }
    result
}

/// Transpile a .mvl file to a Cargo project, build it, and optionally run it.
///
/// `run_args` are forwarded to the compiled binary when `run` is true; the
/// binary is executed with its working directory set to the source file's
/// parent directory so that relative paths in args (e.g. `--file logs.jsonl`)
/// resolve correctly.
pub fn run(path: &str, run: bool, run_args: &[String], assert_mode: AssertMode) {
    let stdlib_dir = stdlib::ensure_stdlib();
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

    let (prog, _src) = super::parse_or_exit(&file_path);
    let crate_name = loader::stem(path);

    // Collect sibling modules referenced via `use module::item` declarations.
    // Only load files that are actually imported — not all .mvl files in the directory.
    let entry_dir = Path::new(&file_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let imported_mod_names = loader::collect_imported_module_names(&prog);
    let mut sibling_modules: Vec<(String, mvl::mvl::parser::ast::Program)> = imported_mod_names
        .into_iter()
        .filter_map(|mod_name| {
            let sib_path = entry_dir.join(format!("{mod_name}.mvl"));
            if !sib_path.exists() {
                return None;
            }
            let (sib_prog, _) = super::parse_or_exit(&sib_path.display().to_string());
            Some((mod_name, sib_prog))
        })
        .collect();
    sibling_modules.sort_by(|(a, _), (b, _)| a.cmp(b));

    // Run module resolver to validate `use` imports across all modules.
    let mut all_modules = vec![(crate_name.clone(), prog.clone())];
    all_modules.extend(sibling_modules.iter().cloned());
    let resolve_result = resolver::resolve_project(all_modules, Some(&stdlib_dir));
    if !resolve_result.is_ok() {
        for err in &resolve_result.errors {
            eprintln!("error[resolver]: {err}");
        }
        process::exit(1);
    }

    // Load the implicit stdlib prelude: core.mvl + Phase 4 stdlib files
    // (strings.mvl, lists.mvl). Non-stub MVL functions
    // (e.g. range(), trim()) are transpiled from source rather than relying
    // on hardcoded Rust mappings in the transpiler. Embedded at compile time.
    let mut stdlib_prelude_progs = loader::load_implicit_prelude();
    // Extend with any pure-MVL stdlib modules imported by this program (e.g. json.mvl).
    let all_progs: Vec<_> = std::iter::once(&prog)
        .chain(sibling_modules.iter().map(|(_, p)| p))
        .cloned()
        .collect();
    stdlib_prelude_progs.extend(loader::load_mvl_native_stdlib_extras(&all_progs));

    // Load MVL source files from any `pkg.*` packages referenced by the program.
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    stdlib_prelude_progs.extend(loader::load_pkg_modules(&all_progs, &project_root));

    // Collect expression types from ALL programs (prelude + user) for the
    // transpiler to emit type-specific Rust at method-call sites (#554).
    let mut all_expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
    let check_result = checker::check_with_prelude(&stdlib_prelude_progs, &prog);
    all_expr_types.extend(check_result.expr_types);
    let out = transpiler::transpile_project(
        &crate_name,
        &prog,
        &sibling_modules,
        &stdlib_prelude_progs,
        all_expr_types,
        assert_mode,
    );

    // Write to a per-crate workspace so each build gets its own mvl_runtime copy.
    // Layout: temp/mvl_build_{name}/{name}/  (crate), temp/mvl_build_{name}/mvl_runtime/ (runtime)
    // The Cargo.toml path dep `../mvl_runtime` resolves correctly from within the crate dir.
    let tmp_workspace = std::env::temp_dir().join(format!("mvl_build_{crate_name}"));
    let tmp_dir = tmp_workspace.join(&crate_name);
    let src_dir = tmp_dir.join("src");
    fs::create_dir_all(&src_dir).unwrap_or_else(|e| {
        eprintln!("Cannot create temp dir {}: {e}", src_dir.display());
        process::exit(1);
    });

    let cargo_toml_path = tmp_dir.join("Cargo.toml");
    fs::write(&cargo_toml_path, &out.cargo_toml).unwrap_or_else(|e| {
        eprintln!("Cannot write Cargo.toml: {e}");
        process::exit(1);
    });

    // Detect a sibling bridge.rs — Rust implementations of extern "rust" fns.
    // Use canonicalize directly (no exists() pre-check) to eliminate the TOCTOU
    // race window. NotFound → no bridge. Any other error → hard fail.
    // Validate that the resolved path stays inside the source directory (symlink-escape guard).
    let mvl_dir = Path::new(&file_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let bridge_candidate = mvl_dir.join("bridge.rs");
    let mut bridge_path: Option<PathBuf> = match fs::canonicalize(&bridge_candidate) {
        Ok(canon_bridge) => {
            let canon_dir = fs::canonicalize(mvl_dir).unwrap_or_else(|e| {
                eprintln!("error: cannot canonicalize {}: {e}", mvl_dir.display());
                process::exit(1);
            });
            if !canon_bridge.starts_with(&canon_dir) {
                eprintln!("error: bridge.rs is outside source directory — refusing to copy",);
                process::exit(1);
            }
            Some(canon_bridge)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!(
                "error: cannot resolve bridge.rs at {}: {e}",
                bridge_candidate.display()
            );
            process::exit(1);
        }
    };

    if out.has_extern_rust && bridge_path.is_none() {
        // No user bridge.rs — check if a pkg.* package provides one.
        bridge_path = loader::find_pkg_bridge(&all_progs, &project_root);
    }

    if out.has_extern_rust && bridge_path.is_none() {
        eprintln!(
            "error: bridge.rs not found — {file_path} declares extern \"rust\" blocks but no bridge.rs exists at {}",
            bridge_candidate.display()
        );
        eprintln!("  Create bridge.rs with `pub extern \"Rust\" fn` implementations to link.");
        process::exit(1);
    }

    // Inject `mod bridge;` after `use mvl_runtime::prelude::*;`.
    let main_source = if bridge_path.is_some() {
        inject_mod_bridge(&out.main_rs)
    } else {
        out.main_rs
    };

    if out.has_main {
        // Binary crate: the transpiled code IS src/main.rs
        fs::write(src_dir.join("main.rs"), &main_source).unwrap_or_else(|e| {
            eprintln!("Cannot write main.rs: {e}");
            process::exit(1);
        });
    } else {
        // Library crate: lib.rs + a stub main for cargo build to succeed
        fs::write(src_dir.join("lib.rs"), &main_source).unwrap_or_else(|e| {
            eprintln!("Cannot write lib.rs: {e}");
            process::exit(1);
        });
        fs::write(
            src_dir.join("main.rs"),
            transpiler::cargo::emit_main_rs_stub(&crate_name),
        )
        .unwrap_or_else(|e| {
            eprintln!("Cannot write stub main.rs: {e}");
            process::exit(1);
        });
    }

    // Write each sibling module as src/{name}.rs so `pub mod name;` resolves.
    for (mod_name, mod_source) in &out.module_files {
        fs::write(src_dir.join(format!("{mod_name}.rs")), mod_source).unwrap_or_else(|e| {
            eprintln!("Cannot write {mod_name}.rs: {e}");
            process::exit(1);
        });
    }

    // Copy bridge.rs into src/ so `mod bridge;` resolves.
    // Use fs::copy (single syscall) to avoid the read→write TOCTOU window.
    if let Some(ref bp) = bridge_path {
        fs::copy(bp, src_dir.join("bridge.rs")).unwrap_or_else(|e| {
            eprintln!("Cannot copy bridge.rs: {e}");
            process::exit(1);
        });
    }

    // If the program uses mvl_runtime, copy it inside the build dir so the
    // relative path `./mvl_runtime` in Cargo.toml resolves.  Each build gets
    // its own copy, which eliminates races when multiple bridge programs are
    // built concurrently (e.g. parallel integration tests).
    //
    // Idempotent for concurrent invocations with identical source: create_dir_all
    // + fs::copy both tolerate pre-existing targets.  Stale artefacts from a
    // prior build of a different version are handled by cargo's incremental cache.
    if out.use_mvl_runtime {
        let runtime_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("runtime")
            .join("rust");
        let runtime_dst = tmp_dir.join("mvl_runtime");
        if !runtime_src.exists() {
            eprintln!(
                "error: mvl_runtime not found at {} — cannot build extern bridge",
                runtime_src.display()
            );
            process::exit(1);
        }
        super::copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
            eprintln!("error: failed to copy mvl_runtime: {e}");
            process::exit(1);
        });
    }

    println!("Transpiled to: {}", tmp_dir.display());
    println!("Running: cargo build");

    let build_status = process::Command::new("cargo")
        .arg("build")
        .current_dir(&tmp_dir)
        .status()
        .unwrap_or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!(
                    "error: `cargo` not found in PATH — install Rust from https://rustup.rs/"
                );
            } else {
                eprintln!("error: failed to run cargo: {e}");
            }
            process::exit(1);
        });

    if !build_status.success() {
        eprintln!("cargo build failed");
        process::exit(1);
    }

    if run && out.has_main {
        // Run the binary with the invocation CWD (where the user ran `mvl run`) so that
        // relative file paths in the program (and in run_args like --file logs.jsonl)
        // resolve against the caller's working directory, not the tmp build dir or
        // the source file's parent directory.
        let binary = tmp_dir.join("target").join("debug").join(&crate_name);
        let invocation_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let run_status = process::Command::new(&binary)
            .args(run_args)
            .current_dir(&invocation_dir)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("error: failed to run {}: {e}", binary.display());
                process::exit(1);
            });
        if !run_status.success() {
            process::exit(run_status.code().unwrap_or(1));
        }
    } else {
        println!("Build successful.");
        if run && !out.has_main {
            eprintln!("Note: no `fn main` in MVL source — nothing to run.");
        }
    }
}

#[cfg(test)]
mod bridge_inject_tests {
    use super::inject_mod_bridge;

    const PRELUDE: &str = "use mvl_runtime::prelude::*;";

    #[test]
    fn inserts_after_prelude_marker() {
        let source = format!("{PRELUDE}\n\nfn main() {{}}\n");
        let out = inject_mod_bridge(&source);
        let lines: Vec<&str> = out.lines().collect();
        let marker_pos = lines
            .iter()
            .position(|l: &&str| l.trim() == PRELUDE)
            .expect("prelude line not found");
        assert_eq!(
            lines[marker_pos + 1],
            "mod bridge;",
            "mod bridge; must follow immediately after prelude"
        );
    }

    #[test]
    fn prepends_when_marker_absent() {
        let source = "fn main() {}\n";
        let out = inject_mod_bridge(source);
        assert!(
            out.starts_with("mod bridge;\n"),
            "expected mod bridge; at start when marker absent, got:\n{out}"
        );
        assert!(
            out.contains("fn main()"),
            "original content must be preserved"
        );
    }

    #[test]
    fn content_not_truncated_or_duplicated() {
        let source = format!("{PRELUDE}\n\nfn foo() -> i64 {{ 1 }}\nfn bar() -> i64 {{ 2 }}\n");
        let out = inject_mod_bridge(&source);
        assert!(out.contains("mod bridge;"), "mod bridge; must be present");
        assert_eq!(out.matches(PRELUDE).count(), 1, "prelude duplicated");
        assert_eq!(out.matches("fn foo()").count(), 1, "fn foo() duplicated");
        assert_eq!(out.matches("fn bar()").count(), 1, "fn bar() duplicated");
        assert_eq!(
            out.matches("mod bridge;").count(),
            1,
            "mod bridge; duplicated"
        );
    }
}
