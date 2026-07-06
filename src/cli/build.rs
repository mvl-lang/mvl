// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use mvl::mvl::backends::rust as transpiler;
use mvl::mvl::backends::AssertMode;
use mvl::mvl::checker;
use mvl::mvl::loader;
use mvl::mvl::manifest_embed;
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
pub fn run(
    path: &str,
    run: bool,
    run_args: &[String],
    assert_mode: AssertMode,
    target: &str,
    release: bool,
) {
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
                    if *name == "mod.mvl" {
                        eprintln!(
                            "warning: `mod.mvl` as project entry is deprecated; \
                             rename to `lib.mvl`"
                        );
                    }
                    Some(p.display().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                eprintln!("No main.mvl / lib.mvl found in {path}");
                process::exit(1);
            })
    } else {
        path.to_string()
    };

    let (prog, src) = super::parse_or_exit(&file_path);
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
            let mod_path = loader::find_module_file(entry_dir, &mod_name)?;
            let (sib_prog, _) = super::parse_or_exit(&mod_path.display().to_string());
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

    let all_progs: Vec<_> = std::iter::once(&prog)
        .chain(sibling_modules.iter().map(|(_, p)| p))
        .cloned()
        .collect();

    // Load pkg.* packages transitively: pkg.anthropic may import pkg.tls, etc.
    // Each round scans the newly-added programs for further pkg.* imports until
    // the frontier is empty (handles arbitrary dependency depth).
    // `seen_pkgs` prevents infinite loops when a package's own sources contain
    // `use pkg.<self>` imports — without it, the same package would be loaded
    // every round (#1050).
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // project_root: used for package resolution (mvl.lock). Starts from cwd so that
    // Makefiles that invoke `cd REPO_ROOT && mvl build examples/foo/main.mvl` use the
    // repo-level lock file which has all packages installed.
    let project_root = super::find_project_root(&cwd);
    // manifest_root: used only for app metadata (app_name, app_version, source_digest).
    // Starts from the source file's own directory so that the example's own mvl.toml is
    // used instead of the repo root's mvl.toml.
    let manifest_root = {
        let abs = entry_dir
            .canonicalize()
            .unwrap_or_else(|_| cwd.join(entry_dir));
        super::find_project_root(&abs)
    };
    let mut pkg_progs: Vec<_> = Vec::new();
    let mut pkg_progs_names: Vec<String> = Vec::new();
    let mut seen_pkgs = std::collections::HashSet::<String>::new();
    let mut frontier: Vec<_> = all_progs.clone();
    loop {
        let new_pkgs = loader::load_pkg_modules_tagged(&frontier, &project_root, &mut seen_pkgs);
        if new_pkgs.is_empty() {
            break;
        }
        let new_frontier: Vec<_> = new_pkgs.iter().map(|(_, p)| p.clone()).collect();
        pkg_progs_names.extend(new_pkgs.iter().map(|(n, _)| n.clone()));
        pkg_progs.extend(new_pkgs.into_iter().map(|(_, p)| p));
        frontier = new_frontier;
    }

    // Extend with any pure-MVL stdlib modules imported by this program OR by any
    // loaded package (e.g. pkg.anthropic imports std.json → json.mvl must be in prelude).
    let all_with_pkgs: Vec<_> = all_progs.iter().chain(pkg_progs.iter()).cloned().collect();
    let stdlib_extras = loader::load_mvl_native_stdlib_extras(&all_with_pkgs);
    let stdlib_extras_len = stdlib_extras.len();
    stdlib_prelude_progs.extend(stdlib_extras);
    stdlib_prelude_progs.extend(pkg_progs.clone());

    // Build a parallel pkg-name vector so the transpiler can prefix colliding
    // cross-package function names in the generated Rust (#1475).
    // Entries are None for stdlib (load_implicit_prelude + stdlib_extras) and
    // Some(pkg_name) for each program loaded from a `pkg.*` package.
    let stdlib_only_len = stdlib_prelude_progs.len() - pkg_progs.len();
    let _ = stdlib_extras_len; // used above to track, now consumed
    let prelude_pkg_names: Vec<Option<String>> = std::iter::repeat_n(None, stdlib_only_len)
        .chain(pkg_progs_names.into_iter().map(Some))
        .collect();

    // Phase 2+3 (#803): embed real manifest data when the program uses std.runtime.
    // A synthetic manifest() override is prepended so it wins the "first wins"
    // deduplication in emit_program_core, replacing the Phase 1 stub values.
    // Phase 3 additionally collects FFI bridges from extern blocks in all_with_pkgs.
    // Phase 5: requirements_proven is 0 here — build does not run the pass registry.
    if manifest_embed::any_uses_std_runtime(&all_with_pkgs) {
        if let Some(override_prog) = manifest_embed::load_and_generate(
            &project_root,
            &manifest_root,
            &all_with_pkgs,
            "rust",
            0,
        ) {
            stdlib_prelude_progs.insert(0, override_prog);
        }
    }

    // Collect expression types from ALL programs (prelude + user) for the
    // transpiler to emit type-specific Rust at method-call sites (#554).
    let mut all_expr_types = checker::collect_prelude_expr_types(&stdlib_prelude_progs);

    // Build a checker-specific prelude that includes ALL imported stdlib modules
    // (Rust-backed ones like std.net/std.io included), so the type checker can
    // resolve their declarations. The transpiler continues to use stdlib_prelude_progs
    // which handles Rust-backed modules via direct Rust emission rather than MVL bodies.
    // This mirrors what `mvl check` does via load_stdlib_prelude.
    let user_progs_for_stdlib =
        std::iter::once(&prog).chain(sibling_modules.iter().map(|(_, p)| p));
    let mut checker_stdlib = loader::load_implicit_prelude();
    checker_stdlib.extend(loader::load_stdlib_prelude(
        user_progs_for_stdlib,
        &stdlib_dir,
    ));
    checker_stdlib.extend(pkg_progs.iter().cloned());

    let sibling_refs: Vec<&mvl::mvl::parser::ast::Program> =
        sibling_modules.iter().map(|(_, p)| p).collect();
    let check_result = checker::check_with_two_preludes(&checker_stdlib, &sibling_refs, &prog);
    if check_result.has_errors() {
        for err in &check_result.errors {
            super::render_diagnostic(&file_path, &src, err);
        }
        process::exit(1);
    }
    all_expr_types.extend(check_result.expr_types);
    // Pre-check each sibling so the backend receives ready-made expr_types (#1110).
    // Include prog + all OTHER siblings as prelude so cross-sibling method dispatch
    // resolves: Go-model — files in the same directory share method declarations
    // without explicit `use` imports (#1706).
    let sibling_expr_types: Vec<_> = sibling_modules
        .iter()
        .enumerate()
        .map(|(i, (_, sibling))| {
            let (before, after_with_self) = sibling_modules.split_at(i);
            let after = &after_with_self[1..];
            let sibling_prelude: Vec<&mvl::mvl::parser::ast::Program> =
                std::iter::once(&prog)
                    .chain(before.iter().map(|(_, p)| p))
                    .chain(after.iter().map(|(_, p)| p))
                    .collect();
            let mut t = checker::collect_prelude_expr_types(&stdlib_prelude_progs);
            t.extend(
                checker::check_with_two_preludes(&checker_stdlib, &sibling_prelude, sibling)
                    .expr_types,
            );
            t
        })
        .collect();
    let out = transpiler::transpile_project_with_pkg_names(
        &crate_name,
        &prog,
        &sibling_modules,
        &stdlib_prelude_progs,
        all_expr_types,
        sibling_expr_types,
        assert_mode,
        &prelude_pkg_names,
    );

    // Write to a per-crate, per-version workspace so each compiler release gets
    // its own mvl_runtime copy. Including the compiler version prevents stale
    // Cargo artifacts from a previous mvl release from causing type mismatches
    // when the runtime signature changes between versions.
    // Layout: temp/mvl_build_{version}_{name}/{name}/  (crate)
    //         temp/mvl_build_{version}_{name}/mvl_runtime/ (runtime)
    // The Cargo.toml path dep `./mvl_runtime` resolves from within the crate dir.
    let compiler_version = env!("CARGO_PKG_VERSION");
    let tmp_workspace =
        std::env::temp_dir().join(format!("mvl_build_{compiler_version}_{crate_name}"));
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

    let mut bridge_from_pkg = false;
    if out.has_extern_rust && bridge_path.is_none() {
        // No user bridge.rs — check if a pkg.* package (including transitive deps) provides one.
        bridge_path = loader::find_pkg_bridge(&all_with_pkgs, &project_root);
        bridge_from_pkg = bridge_path.is_some();
    }

    // Re-emit Cargo.toml when non-default settings are needed:
    // - native deps from a pkg.* bridge (bridge_from_pkg)
    // - tokio runtime for --target=tokio
    // Both cases are unified here so only one final Cargo.toml write occurs.
    let native_dep_lines: Vec<String> = if bridge_from_pkg {
        loader::collect_pkg_native_dep_lines(&all_with_pkgs, &project_root)
    } else {
        Vec::new()
    };
    let needs_cargo_patch = !native_dep_lines.is_empty() || target != "default";
    if needs_cargo_patch {
        let tokio_runtime_path: Option<String> = if target == "tokio" {
            Some(
                mvl::mvl::runtime_xdg::ensure_runtime_tokio()
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        };
        let opts = transpiler::cargo::CargoOptions {
            crate_name: &crate_name,
            use_mvl_runtime: out.use_mvl_runtime,
            extern_crates: Vec::new(),
            native_dep_lines,
            mvl_runtime_path: tokio_runtime_path,
            use_tokio: target == "tokio",
        };
        let patched = if out.has_main {
            transpiler::cargo::emit_cargo_toml_binary_opts(&opts)
        } else {
            transpiler::cargo::emit_cargo_toml_library_opts(&opts)
        };
        fs::write(&cargo_toml_path, &patched).unwrap_or_else(|e| {
            eprintln!("Cannot write Cargo.toml: {e}");
            process::exit(1);
        });
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

    // If the program uses mvl_runtime, make it available for the build:
    //
    // --target=default: copy runtime/rust to ./mvl_runtime (relative path in Cargo.toml).
    //   Each build gets its own copy to eliminate concurrent-build races.
    //
    // --target=tokio: the generated Cargo.toml already references runtime/rust-tokio
    //   via an absolute path (ADR-0027 §"--target selects the runtime"), so no copy
    //   is needed. We just verify the source exists.
    if out.use_mvl_runtime {
        if target == "tokio" {
            // No copy — Cargo.toml references tokio runtime via absolute path
            // (already resolved by ensure_runtime_tokio() above).
        } else {
            let runtime_src = mvl::mvl::runtime_xdg::ensure_runtime_rust();
            let runtime_dst = tmp_dir.join("mvl_runtime");
            super::copy_dir_recursive(&runtime_src, &runtime_dst).unwrap_or_else(|e| {
                eprintln!("error: failed to copy mvl_runtime: {e}");
                process::exit(1);
            });
        }
    }

    println!("Transpiled to: {}", tmp_dir.display());
    let profile_label = if release { "release" } else { "dev" };
    println!("Running: cargo build (profile: {profile_label})");

    let mut cmd = process::Command::new("cargo");
    cmd.arg("build").current_dir(&tmp_dir);
    if release {
        cmd.arg("--release");
    }
    let build_status = cmd.status().unwrap_or_else(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            eprintln!("error: `cargo` not found in PATH — install Rust from https://rustup.rs/");
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
        // Run the binary from the source file's parent directory so that relative
        // paths in config files (config.toml, seed_file = "users.csv", etc.) resolve
        // against the project directory regardless of where `mvl run` was invoked.
        // Package/bridge resolution uses project_root (invocation CWD) and happens
        // earlier, so it is unaffected by this CWD change.
        let binary = tmp_dir
            .join("target")
            .join(if release { "release" } else { "debug" })
            .join(&crate_name);
        let source_dir = Path::new(&file_path)
            .parent()
            .and_then(|p| fs::canonicalize(p).ok())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let run_status = process::Command::new(&binary)
            .args(run_args)
            .current_dir(&source_dir)
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
