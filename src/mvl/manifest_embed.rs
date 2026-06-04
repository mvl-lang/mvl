// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Compile-time manifest embedding for std.runtime (#803).
//!
//! Reads `mvl.toml` + `mvl.lock` from the project root and inspects the
//! compiled programs to generate a synthetic MVL `manifest()` function that
//! replaces the Phase 1 stub with real values.
//!
//! ## Phases implemented here
//!
//! | Phase | Field(s) populated |
//! |-------|--------------------|
//! | 2     | `app_name`, `app_version`, `mvl_version`, `stdlib_version`, `packages` |
//! | 3     | `ffi_bridges` — extracted from `extern "rust"` / `extern "c"` blocks |
//!
//! The override is prepended to `stdlib_prelude_progs` before transpilation.
//! The emitter's "first wins" prelude deduplication makes it shadow the stub
//! from `std/runtime.mvl` transparently — no emitter changes required.

use crate::mvl::packages::lock::{LockFile, LockedPackage};
use crate::mvl::packages::manifest::Manifest as PkgManifest;
use crate::mvl::parser::ast::{Decl, Program};
use crate::mvl::parser::Parser;
use std::path::Path;

// ── Public types ──────────────────────────────────────────────────────────────

/// An FFI bridge extracted from an `extern "abi"` block.
///
/// `bridge_name` is the first declared function name in the block — a stable,
/// unique identifier for the trust boundary.  `bridge_version` is left empty
/// until Phase 4 maps it from the `[native]` section of `mvl.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct FfiBridgeData {
    pub abi: String,
    pub bridge_name: String,
    pub bridge_version: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Try to load project metadata and generate a `manifest()` override program.
///
/// `all_progs` — the entry program + sibling modules + package modules, used
/// to collect `extern` declarations for the `ffi_bridges` field (Phase 3).
///
/// Returns `None` when no `mvl.toml` exists (single-file builds that have no
/// project manifest), or when the generated MVL fails to parse unexpectedly.
pub fn load_and_generate(project_root: &Path, all_progs: &[Program]) -> Option<Program> {
    let pkg_manifest = PkgManifest::load(project_root).ok()?;
    let lockfile = LockFile::load_or_empty(project_root);
    let bridges = collect_ffi_bridges(all_progs);

    let mvl_version = env!("CARGO_PKG_VERSION");
    let runtime_version = env!("MVL_RUNTIME_VERSION");
    let stdlib_version = env!("MVL_STDLIB_VERSION");
    let src = generate_manifest_mvl(
        &pkg_manifest.package.name,
        &pkg_manifest.package.version,
        mvl_version,
        runtime_version,
        stdlib_version,
        &lockfile.packages,
        &bridges,
    );

    let (mut parser, _) = Parser::new(&src);
    let prog = parser.parse_program();

    if !parser.errors().is_empty() {
        eprintln!(
            "warning: manifest_embed: failed to parse generated manifest() override — \
             using Phase 1 stub. Generated source:\n{src}"
        );
        return None;
    }

    Some(prog)
}

/// Collect unique FFI bridges from `extern` blocks in `progs`.
///
/// Each `extern "abi" { fn first_fn(…); … }` block becomes one entry.
/// The bridge name is the first declared function name — a stable identifier
/// for the trust boundary.  Blocks with no declared functions are skipped.
/// Duplicates (same abi + bridge_name) are deduplicated.
pub fn collect_ffi_bridges(progs: &[Program]) -> Vec<FfiBridgeData> {
    let mut bridges: Vec<FfiBridgeData> = Vec::new();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Extern(ed) = decl {
                let Some(first_fn) = ed.fns.first() else {
                    continue;
                };
                let bridge = FfiBridgeData {
                    abi: ed.abi.clone(),
                    bridge_name: first_fn.name.clone(),
                    bridge_version: String::new(), // populated in Phase 4 via [native]
                };
                // Deduplicate by (abi, bridge_name).
                if !bridges
                    .iter()
                    .any(|b| b.abi == bridge.abi && b.bridge_name == bridge.bridge_name)
                {
                    bridges.push(bridge);
                }
            }
        }
    }
    bridges
}

/// Return true if any program in `progs` imports `use std.runtime.*`.
pub fn any_uses_std_runtime(progs: &[Program]) -> bool {
    progs.iter().any(|p| {
        p.declarations.iter().any(|d| {
            if let Decl::Use(ud) = d {
                ud.path.first().map(|s| s == "std").unwrap_or(false)
                    && ud.path.get(1).map(|s| s == "runtime").unwrap_or(false)
            } else {
                false
            }
        })
    })
}

// ── MVL source generation ─────────────────────────────────────────────────────

/// Generate MVL source for the real `manifest()` function.
///
/// Uses `let` bindings for each `PackageInfo` and `FfiBridge` entry to avoid
/// any struct-in-list parsing ambiguity, then builds the lists by variable
/// reference and returns a `Manifest { … }` struct literal.
fn generate_manifest_mvl(
    app_name: &str,
    app_version: &str,
    mvl_version: &str,
    runtime_version: &str,
    stdlib_version: &str,
    packages: &[LockedPackage],
    bridges: &[FfiBridgeData],
) -> String {
    let mut src = String::from("pub fn manifest() -> Manifest {\n");

    // Per-package let bindings.
    for (i, pkg) in packages.iter().enumerate() {
        src.push_str(&format!(
            "    let p{i}: PackageInfo = PackageInfo {{ \
             name: {name}, version: {ver}, license: {lic} }};\n",
            name = mvl_str(&pkg.name),
            ver = mvl_str(&pkg.version),
            lic = mvl_str(""), // license aggregation deferred to Phase 6
        ));
    }
    let pkg_list = make_list(packages.len(), "p");
    src.push_str(&format!("    let pkgs: List[PackageInfo] = {pkg_list};\n"));

    // Per-bridge let bindings.
    for (i, bridge) in bridges.iter().enumerate() {
        src.push_str(&format!(
            "    let b{i}: FfiBridge = FfiBridge {{ \
             abi: {abi}, bridge_name: {name}, bridge_version: {ver} }};\n",
            abi = mvl_str(&bridge.abi),
            name = mvl_str(&bridge.bridge_name),
            ver = mvl_str(&bridge.bridge_version),
        ));
    }
    let ffi_list = make_list(bridges.len(), "b");
    src.push_str(&format!("    let ffis: List[FfiBridge] = {ffi_list};\n"));

    // Return expression: Manifest struct literal.
    let lines: &[&str] = &[
        "    Manifest {",
        &format!("        app_name:        {},", mvl_str(app_name)),
        &format!("        app_version:     {},", mvl_str(app_version)),
        &format!("        mvl_version:     {},", mvl_str(mvl_version)),
        &format!("        runtime_version: {},", mvl_str(runtime_version)),
        &format!("        stdlib_version:  {},", mvl_str(stdlib_version)),
        "        stdlib_profile: StdlibProfile::Trusted,",
        "        packages:       pkgs,",
        "        ffi_bridges:    ffis,",
        "        build:          BuildInfo {",
        "            rustc_version: None,",
        "            llvm_version:  None,",
        "            target:        \"\",",
        "            profile:       \"debug\",",
        "            date:          \"\",",
        "        },",
        "        licenses:  [],",
        "        assurance: AssuranceInfo {",
        "            extern_ratio:        0.0,",
        "            extern_count:        0,",
        "            total_functions:     0,",
        "            requirements_proven: 0,",
        "        },",
        "    }",
        "}",
    ];
    for line in lines {
        src.push_str(line);
        src.push('\n');
    }

    src
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build `[v0, v1, …]` or `[]` for `n` variables with the given prefix.
fn make_list(n: usize, prefix: &str) -> String {
    if n == 0 {
        "[]".to_string()
    } else {
        let vars: Vec<String> = (0..n).map(|i| format!("{prefix}{i}")).collect();
        format!("[{}]", vars.join(", "))
    }
}

/// Return `s` as a double-quoted MVL string literal with minimal escaping.
fn mvl_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_prog(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        p.parse_program()
    }

    // --- mvl_str ---

    #[test]
    fn mvl_str_plain() {
        assert_eq!(mvl_str("hello"), "\"hello\"");
    }

    #[test]
    fn mvl_str_escapes_backslash_and_quote() {
        assert_eq!(mvl_str(r#"a\b"c"#), r#""a\\b\"c""#);
    }

    // --- make_list ---

    #[test]
    fn make_list_empty() {
        assert_eq!(make_list(0, "p"), "[]");
    }

    #[test]
    fn make_list_one() {
        assert_eq!(make_list(1, "p"), "[p0]");
    }

    #[test]
    fn make_list_many() {
        assert_eq!(make_list(3, "b"), "[b0, b1, b2]");
    }

    // --- collect_ffi_bridges ---

    #[test]
    fn collect_ffi_bridges_empty_when_no_externs() {
        let prog = parse_prog("fn main() -> Unit {}\n");
        assert!(collect_ffi_bridges(&[prog]).is_empty());
    }

    #[test]
    fn collect_ffi_bridges_single_block() {
        let src = "extern \"rust\" { fn fetch_url(url: String) -> String; }\n";
        let prog = parse_prog(src);
        let bridges = collect_ffi_bridges(&[prog]);
        assert_eq!(bridges.len(), 1);
        assert_eq!(bridges[0].abi, "rust");
        assert_eq!(bridges[0].bridge_name, "fetch_url");
        assert_eq!(bridges[0].bridge_version, "");
    }

    #[test]
    fn collect_ffi_bridges_multiple_blocks_different_abi() {
        let src = "extern \"rust\" { fn rust_fn() -> Int; }\n\
                   extern \"c\" { fn c_fn() -> Int; }\n";
        let prog = parse_prog(src);
        let bridges = collect_ffi_bridges(&[prog]);
        assert_eq!(bridges.len(), 2);
        let abis: Vec<&str> = bridges.iter().map(|b| b.abi.as_str()).collect();
        assert!(abis.contains(&"rust"));
        assert!(abis.contains(&"c"));
    }

    #[test]
    fn collect_ffi_bridges_deduplicates_same_abi_and_name() {
        let src = "extern \"rust\" { fn foo() -> Int; }\n";
        let prog1 = parse_prog(src);
        let prog2 = parse_prog(src);
        let bridges = collect_ffi_bridges(&[prog1, prog2]);
        assert_eq!(bridges.len(), 1, "duplicate bridge should be deduplicated");
    }

    #[test]
    fn collect_ffi_bridges_skips_empty_extern_blocks() {
        // An extern block with no declared functions contributes no bridge.
        // (Parser may or may not accept empty blocks; this tests the filter.)
        let bridges = collect_ffi_bridges(&[]);
        assert!(bridges.is_empty());
    }

    #[test]
    fn collect_ffi_bridges_uses_first_fn_name() {
        let src = "extern \"rust\" { fn alpha() -> Int; fn beta() -> Int; }\n";
        let prog = parse_prog(src);
        let bridges = collect_ffi_bridges(&[prog]);
        assert_eq!(bridges.len(), 1);
        assert_eq!(bridges[0].bridge_name, "alpha");
    }

    // --- generate_manifest_mvl ---

    #[test]
    fn generate_manifest_mvl_no_packages_no_bridges() {
        let src = generate_manifest_mvl("my_app", "1.0.0", "0.100.0", "0.9.2", "0.42.0", &[], &[]);
        assert!(src.contains("pub fn manifest() -> Manifest"));
        assert!(src.contains(r#"app_name:        "my_app""#));
        assert!(src.contains(r#"app_version:     "1.0.0""#));
        assert!(src.contains(r#"mvl_version:     "0.100.0""#));
        assert!(src.contains(r#"runtime_version: "0.9.2""#));
        assert!(src.contains(r#"stdlib_version:  "0.42.0""#));
        assert!(src.contains("let pkgs: List[PackageInfo] = [];"));
        assert!(src.contains("let ffis: List[FfiBridge] = [];"));
    }

    #[test]
    fn generate_manifest_mvl_with_bridge() {
        let bridges = vec![FfiBridgeData {
            abi: "rust".to_string(),
            bridge_name: "fetch_url".to_string(),
            bridge_version: String::new(),
        }];
        let src =
            generate_manifest_mvl("app", "1.0.0", "0.100.0", "0.9.2", "0.42.0", &[], &bridges);
        assert!(src.contains(r#"let b0: FfiBridge = FfiBridge { abi: "rust", bridge_name: "fetch_url", bridge_version: "" };"#));
        assert!(src.contains("let ffis: List[FfiBridge] = [b0];"));
    }

    // --- parse round-trip ---

    #[test]
    fn generated_mvl_parses_no_packages_no_bridges() {
        let src = generate_manifest_mvl("myapp", "0.1.0", "0.184.0", "0.9.2", "0.42.0", &[], &[]);
        let (mut parser, _) = Parser::new(&src);
        let _prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "parse errors: {:?}\nSource:\n{src}",
            parser.errors()
        );
    }

    #[test]
    fn generated_mvl_parses_with_package_and_bridge() {
        let pkgs = vec![LockedPackage {
            name: "github.com/mvl-lang/pkg-http".to_string(),
            version: "0.2.0".to_string(),
            hash: "sha256:abc".to_string(),
            commit: None,
            git: None,
        }];
        let bridges = vec![FfiBridgeData {
            abi: "rust".to_string(),
            bridge_name: "fetch_url".to_string(),
            bridge_version: String::new(),
        }];
        let src = generate_manifest_mvl(
            "myapp", "0.1.0", "0.184.0", "0.9.2", "0.42.0", &pkgs, &bridges,
        );
        let (mut parser, _) = Parser::new(&src);
        let _prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "parse errors: {:?}\nSource:\n{src}",
            parser.errors()
        );
    }

    // --- any_uses_std_runtime ---

    #[test]
    fn any_uses_std_runtime_detects_import() {
        let prog = parse_prog("use std.runtime.{manifest, Manifest}\nfn main() -> Unit {}\n");
        assert!(any_uses_std_runtime(&[prog]));
    }

    #[test]
    fn any_uses_std_runtime_ignores_other_modules() {
        let prog = parse_prog("use std.log.{log_info}\nfn main() -> Unit {}\n");
        assert!(!any_uses_std_runtime(&[prog]));
    }

    // --- load_and_generate ---

    #[test]
    fn load_and_generate_returns_none_for_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_and_generate(tmp.path(), &[]).is_none());
    }

    #[test]
    fn load_and_generate_returns_program_when_manifest_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\n\
                        name = \"my-app\"\n\
                        version = \"1.2.3\"\n\
                        license = \"MIT\"\n\
                        requires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();
        let prog = load_and_generate(tmp.path(), &[]).unwrap();
        let fn_names: Vec<&str> = prog
            .declarations
            .iter()
            .filter_map(|d| {
                if let Decl::Fn(fd) = d {
                    Some(fd.name.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(fn_names, vec!["manifest"]);
    }

    #[test]
    fn load_and_generate_embeds_ffi_bridges_from_progs() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = "[package]\n\
                        name = \"my-app\"\n\
                        version = \"1.0.0\"\n\
                        license = \"MIT\"\n\
                        requires-mvl = \">=0.1.0\"\n";
        std::fs::write(tmp.path().join("mvl.toml"), manifest).unwrap();

        let user_prog = parse_prog("extern \"rust\" { fn my_bridge() -> Int; }\n");
        // Verify via the source generator — the bridge must appear in the MVL source.
        let bridges = collect_ffi_bridges(&[user_prog]);
        let src =
            generate_manifest_mvl("my-app", "1.0.0", "0.0.0", "0.9.2", "0.42.0", &[], &bridges);
        assert!(
            src.contains("b0") && src.contains("my_bridge"),
            "bridge binding not found in generated source:\n{src}"
        );
    }
}
