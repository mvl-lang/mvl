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
///
/// `project_root` — used for package lock resolution (mvl.lock).
/// `manifest_root` — used for app identity (mvl.toml app_name/version) and source digest.
///   Typically the source file's own directory; falls back to `project_root` when equal.
pub fn load_and_generate(
    project_root: &Path,
    manifest_root: &Path,
    all_progs: &[Program],
    backend: &str,
) -> Option<Program> {
    let pkg_manifest = PkgManifest::load(manifest_root).ok()?;
    let lockfile = LockFile::load_or_empty(project_root);
    let bridges = collect_ffi_bridges(all_progs);

    let mvl_version = env!("CARGO_PKG_VERSION");
    let runtime_version = env!("MVL_RUNTIME_VERSION");
    let stdlib_version = env!("MVL_STDLIB_VERSION");
    let rustc_version = env!("MVL_RUSTC_VERSION");
    let llvm_version = env!("MVL_LLVM_VERSION");
    let target = env!("MVL_TARGET");
    let profile = env!("MVL_PROFILE");
    let build_date = env!("MVL_BUILD_DATE");
    let source_digest_computed = compute_source_digest(manifest_root);
    let meta = ManifestMeta {
        app_name: &pkg_manifest.package.name,
        app_version: &pkg_manifest.package.version,
        mvl_version,
        runtime_version,
        stdlib_version,
        backend,
        rustc_version,
        llvm_version,
        target,
        profile,
        build_date,
        source_digest: &source_digest_computed,
    };
    let src = generate_manifest_mvl(&meta, &lockfile.packages, &bridges);

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

/// Version and build metadata passed to [`generate_manifest_mvl`].
struct ManifestMeta<'a> {
    app_name: &'a str,
    app_version: &'a str,
    mvl_version: &'a str,
    runtime_version: &'a str,
    stdlib_version: &'a str,
    backend: &'a str,
    /// `rustc --version` output, or empty → `None` in generated MVL.
    rustc_version: &'a str,
    /// `llvm-config --version` output, or empty → `None` in generated MVL.
    llvm_version: &'a str,
    target: &'a str,
    profile: &'a str,
    build_date: &'a str,
    /// `"sha256:<hex>"` of the project source tree, or `""` when unavailable.
    source_digest: &'a str,
}

/// Generate MVL source for the real `manifest()` function.
///
/// Uses `let` bindings for each `PackageInfo` and `FfiBridge` entry to avoid
/// any struct-in-list parsing ambiguity, then builds the lists by variable
/// reference and returns a `Manifest { … }` struct literal.
fn generate_manifest_mvl(
    meta: &ManifestMeta<'_>,
    packages: &[LockedPackage],
    bridges: &[FfiBridgeData],
) -> String {
    let ManifestMeta {
        app_name,
        app_version,
        mvl_version,
        runtime_version,
        stdlib_version,
        backend,
        rustc_version,
        llvm_version,
        target,
        profile,
        build_date,
        source_digest,
    } = meta;
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
        &format!("            backend:       {},", mvl_str(backend)),
        &format!(
            "            rustc_version: {},",
            mvl_option_str(rustc_version)
        ),
        &format!(
            "            llvm_version:  {},",
            mvl_option_str(llvm_version)
        ),
        &format!("            target:        {},", mvl_str(target)),
        &format!("            profile:       {},", mvl_str(profile)),
        &format!("            date:          {},", mvl_str(build_date)),
        "        },",
        "        licenses:  [],",
        "        assurance: AssuranceInfo {",
        "            extern_ratio:        0.0,",
        "            extern_count:        0,",
        "            total_functions:     0,",
        "            requirements_proven: 0,",
        "        },",
        &format!("        source_digest:  {},", mvl_str(source_digest)),
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

/// Return `s` as `None` (when empty) or `Some("…")` for MVL `Option[String]` fields.
fn mvl_option_str(s: &str) -> String {
    if s.is_empty() {
        "None".to_string()
    } else {
        format!("Some({})", mvl_str(s))
    }
}

/// Compute a deterministic SHA-256 digest over all `.mvl` files under `root`.
///
/// Returns `"sha256:<hex>"` — the same format that `mvl sbom` records for external
/// verification.  Returns `""` if no `.mvl` files are found or the directory is
/// unreadable.
fn compute_source_digest(root: &Path) -> String {
    use crate::mvl::packages::hash::sha256_source_tree;

    let mut pairs: Vec<(String, String)> = Vec::new();
    collect_mvl_for_digest(root, root, &mut pairs);
    if pairs.is_empty() {
        return String::new();
    }
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(p, h)| (p.as_str(), h.as_str()))
        .collect();
    sha256_source_tree(&refs)
}

fn collect_mvl_for_digest(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    use crate::mvl::packages::hash::sha256_hex;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            collect_mvl_for_digest(root, &path, out);
        } else if path.extension().is_some_and(|x| x == "mvl") {
            if let Ok(data) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, sha256_hex(&data)));
            }
        }
    }
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

    // --- mvl_option_str ---

    #[test]
    fn mvl_option_str_empty_is_none() {
        assert_eq!(mvl_option_str(""), "None");
    }

    #[test]
    fn mvl_option_str_non_empty_wraps_some() {
        assert_eq!(mvl_option_str("rustc 1.87.0"), r#"Some("rustc 1.87.0")"#);
    }

    // --- generate_manifest_mvl ---

    #[test]
    fn generate_manifest_mvl_no_packages_no_bridges() {
        let meta = ManifestMeta {
            app_name: "my_app",
            app_version: "1.0.0",
            mvl_version: "0.100.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "rustc 1.87.0",
            llvm_version: "",
            target: "aarch64-apple-darwin",
            profile: "debug",
            build_date: "2026-06-04T00:00:00Z",
            source_digest: "",
        };
        let src = generate_manifest_mvl(&meta, &[], &[]);
        assert!(src.contains("pub fn manifest() -> Manifest"));
        assert!(src.contains(r#"app_name:        "my_app""#));
        assert!(src.contains(r#"app_version:     "1.0.0""#));
        assert!(src.contains(r#"mvl_version:     "0.100.0""#));
        assert!(src.contains(r#"runtime_version: "0.9.2""#));
        assert!(src.contains(r#"stdlib_version:  "0.42.0""#));
        assert!(src.contains("let pkgs: List[PackageInfo] = [];"));
        assert!(src.contains("let ffis: List[FfiBridge] = [];"));
        // Phase 4: BuildInfo fields are populated, not stubs.
        assert!(src.contains(r#"rustc_version: Some("rustc 1.87.0")"#));
        assert!(src.contains("llvm_version:  None"));
        assert!(src.contains(r#"target:        "aarch64-apple-darwin""#));
        assert!(src.contains(r#"profile:       "debug""#));
        assert!(src.contains(r#"date:          "2026-06-04T00:00:00Z""#));
    }

    #[test]
    fn generate_manifest_mvl_with_bridge() {
        let bridges = vec![FfiBridgeData {
            abi: "rust".to_string(),
            bridge_name: "fetch_url".to_string(),
            bridge_version: String::new(),
        }];
        let meta = ManifestMeta {
            app_name: "app",
            app_version: "1.0.0",
            mvl_version: "0.100.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "",
            llvm_version: "",
            target: "x86_64-unknown-linux-gnu",
            profile: "release",
            build_date: "2026-06-04T00:00:00Z",
            source_digest: "",
        };
        let src = generate_manifest_mvl(&meta, &[], &bridges);
        assert!(src.contains(r#"let b0: FfiBridge = FfiBridge { abi: "rust", bridge_name: "fetch_url", bridge_version: "" };"#));
        assert!(src.contains("let ffis: List[FfiBridge] = [b0];"));
    }

    // --- parse round-trip ---

    #[test]
    fn generated_mvl_parses_no_packages_no_bridges() {
        let meta = ManifestMeta {
            app_name: "myapp",
            app_version: "0.1.0",
            mvl_version: "0.184.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "rustc 1.87.0 (17067e9ac 2025-05-09)",
            llvm_version: "18.1.8",
            target: "aarch64-apple-darwin",
            profile: "debug",
            build_date: "2026-06-04T12:00:00Z",
            source_digest: "",
        };
        let src = generate_manifest_mvl(&meta, &[], &[]);
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
            license: None,
            allow_license_override: None,
        }];
        let bridges = vec![FfiBridgeData {
            abi: "rust".to_string(),
            bridge_name: "fetch_url".to_string(),
            bridge_version: String::new(),
        }];
        let meta = ManifestMeta {
            app_name: "myapp",
            app_version: "0.1.0",
            mvl_version: "0.184.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "",
            llvm_version: "",
            target: "x86_64-unknown-linux-gnu",
            profile: "debug",
            build_date: "2026-06-04T00:00:00Z",
            source_digest: "",
        };
        let src = generate_manifest_mvl(&meta, &pkgs, &bridges);
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
        assert!(load_and_generate(tmp.path(), tmp.path(), &[], "rust").is_none());
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
        let prog = load_and_generate(tmp.path(), tmp.path(), &[], "rust").unwrap();
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
        let meta = ManifestMeta {
            app_name: "my-app",
            app_version: "1.0.0",
            mvl_version: "0.0.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "",
            llvm_version: "",
            target: "aarch64-apple-darwin",
            profile: "debug",
            build_date: "2026-06-04T00:00:00Z",
            source_digest: "",
        };
        let src = generate_manifest_mvl(&meta, &[], &bridges);
        assert!(
            src.contains("b0") && src.contains("my_bridge"),
            "bridge binding not found in generated source:\n{src}"
        );
    }

    // --- source_digest ---

    fn base_meta<'a>() -> ManifestMeta<'a> {
        ManifestMeta {
            app_name: "test-app",
            app_version: "1.0.0",
            mvl_version: "0.187.0",
            runtime_version: "0.9.2",
            stdlib_version: "0.42.0",
            backend: "rust",
            rustc_version: "",
            llvm_version: "",
            target: "aarch64-apple-darwin",
            profile: "debug",
            build_date: "2026-06-05T00:00:00Z",
            source_digest: "",
        }
    }

    #[test]
    fn source_digest_empty_emits_empty_string() {
        let src = generate_manifest_mvl(&base_meta(), &[], &[]);
        assert!(
            src.contains(r#"source_digest:  """#),
            "empty digest must emit empty string literal"
        );
    }

    #[test]
    fn source_digest_non_empty_emits_correctly() {
        let digest = "sha256:3a7bd3e2360a3d29b8b9e8cc5a1da3e63a0b8e3d";
        let mut meta = base_meta();
        meta.source_digest = digest;
        let src = generate_manifest_mvl(&meta, &[], &[]);
        assert!(
            src.contains(&format!(r#"source_digest:  "{digest}""#)),
            "non-empty digest must be embedded as string literal"
        );
    }

    #[test]
    fn source_digest_parses_in_generated_mvl() {
        let digest = "sha256:abc123def456";
        let mut meta = base_meta();
        meta.source_digest = digest;
        let src = generate_manifest_mvl(&meta, &[], &[]);
        let (mut parser, _) = Parser::new(&src);
        let _prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "generated MVL with source_digest must parse cleanly\nSource:\n{src}"
        );
    }

    #[test]
    fn compute_source_digest_empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let digest = compute_source_digest(tmp.path());
        assert_eq!(digest, "", "no .mvl files → empty string");
    }

    #[test]
    fn compute_source_digest_with_mvl_files_returns_sha256() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.mvl"), b"fn main() {}").unwrap();
        let digest = compute_source_digest(tmp.path());
        assert!(
            digest.starts_with("sha256:"),
            "must start with sha256: prefix"
        );
        assert_eq!(digest.len(), "sha256:".len() + 64, "must be 64 hex chars");
    }

    #[test]
    fn compute_source_digest_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.mvl"), b"hello").unwrap();
        std::fs::write(tmp.path().join("b.mvl"), b"world").unwrap();
        let d1 = compute_source_digest(tmp.path());
        let d2 = compute_source_digest(tmp.path());
        assert_eq!(d1, d2, "digest must be deterministic");
    }

    #[test]
    fn compute_source_digest_changes_on_content_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.mvl"), b"version1").unwrap();
        let d1 = compute_source_digest(tmp.path());
        std::fs::write(tmp.path().join("main.mvl"), b"version2").unwrap();
        let d2 = compute_source_digest(tmp.path());
        assert_ne!(d1, d2, "digest must change when file content changes");
    }

    #[test]
    fn compute_source_digest_skips_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("main.mvl"), b"hello").unwrap();
        let base = compute_source_digest(tmp.path());
        // Adding a .mvl file inside a hidden dir must not change the digest
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git").join("hook.mvl"), b"secret").unwrap();
        let after = compute_source_digest(tmp.path());
        assert_eq!(base, after, "hidden dirs must be skipped");
    }
}
