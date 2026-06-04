// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Phase 2 of std.runtime manifest embedding (#803).
//!
//! Reads `mvl.toml` + `mvl.lock` from the project root and generates a
//! synthetic MVL program that overrides the stub `manifest()` from
//! `std/runtime.mvl` with real compile-time data.
//!
//! The override is prepended to `stdlib_prelude_progs` before transpilation.
//! Because prelude functions are deduplicated by name with "first wins" semantics
//! in `emit_program_core`, the override takes precedence over the stub.

use crate::mvl::packages::lock::{LockFile, LockedPackage};
use crate::mvl::packages::manifest::Manifest as PkgManifest;
use crate::mvl::parser::{ast::Program, Parser};
use std::path::Path;

/// Try to load project metadata and generate a manifest() override program.
///
/// Returns `None` when no `mvl.toml` exists (e.g. single-file builds that have
/// no project manifest), or when parsing the generated MVL fails unexpectedly.
pub fn load_and_generate(project_root: &Path) -> Option<Program> {
    let pkg_manifest = PkgManifest::load(project_root).ok()?;
    let lockfile = LockFile::load_or_empty(project_root);

    let mvl_version = env!("CARGO_PKG_VERSION");
    let src = generate_manifest_mvl(
        &pkg_manifest.package.name,
        &pkg_manifest.package.version,
        mvl_version,
        &lockfile.packages,
    );

    let (mut parser, _) = Parser::new(&src);
    let prog = parser.parse_program();

    if !parser.errors().is_empty() {
        // Generated code should always parse; log a warning and fall back to stub.
        eprintln!(
            "warning: manifest_embed: failed to parse generated manifest() override — \
             using Phase 1 stub. Generated source:\n{src}"
        );
        return None;
    }

    Some(prog)
}

/// Generate MVL source for a `manifest()` function with real embedded values.
///
/// Uses `let` bindings for package entries (avoiding the struct-in-list
/// ambiguity) and returns a bare `Manifest { … }` struct literal.
fn generate_manifest_mvl(
    app_name: &str,
    app_version: &str,
    mvl_version: &str,
    packages: &[LockedPackage],
) -> String {
    let mut src = String::from("pub fn manifest() -> Manifest {\n");

    // Emit one let binding per locked package.
    for (i, pkg) in packages.iter().enumerate() {
        src.push_str(&format!(
            "    let p{i}: PackageInfo = PackageInfo {{ \
             name: {name}, version: {ver}, license: {lic} }};\n",
            name = mvl_str(&pkg.name),
            ver = mvl_str(&pkg.version),
            lic = mvl_str(""), // license aggregation deferred to Phase 6
        ));
    }

    // Build the packages list expression: `[p0, p1, …]` or `[]`.
    let pkg_list = if packages.is_empty() {
        "[]".to_string()
    } else {
        let vars: Vec<String> = (0..packages.len()).map(|i| format!("p{i}")).collect();
        format!("[{}]", vars.join(", "))
    };
    src.push_str(&format!("    let pkgs: List[PackageInfo] = {pkg_list};\n"));

    // Emit the return expression — a Manifest struct literal with real values.
    let lines: &[&str] = &[
        "    Manifest {",
        &format!("        app_name:       {},", mvl_str(app_name)),
        &format!("        app_version:    {},", mvl_str(app_version)),
        &format!("        mvl_version:    {},", mvl_str(mvl_version)),
        // stdlib version tracks compiler version for now (Phase 4 adds real build info)
        &format!("        stdlib_version: {},", mvl_str(mvl_version)),
        "        stdlib_profile: StdlibProfile::Trusted,",
        "        packages:       pkgs,",
        "        ffi_bridges:    [],",
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

/// Return `s` as a double-quoted MVL string literal with minimal escaping.
fn mvl_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Return true if any program in `progs` contains `use std.runtime.*`.
pub fn any_uses_std_runtime(progs: &[Program]) -> bool {
    use crate::mvl::parser::ast::Decl;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mvl_str_plain() {
        assert_eq!(mvl_str("hello"), "\"hello\"");
    }

    #[test]
    fn mvl_str_escapes_backslash_and_quote() {
        assert_eq!(mvl_str(r#"a\b"c"#), r#""a\\b\"c""#);
    }

    #[test]
    fn generate_manifest_mvl_no_packages() {
        let src = generate_manifest_mvl("my_app", "1.0.0", "0.100.0", &[]);
        assert!(src.contains("pub fn manifest() -> Manifest"));
        assert!(src.contains(r#"app_name:       "my_app""#));
        assert!(src.contains(r#"app_version:    "1.0.0""#));
        assert!(src.contains(r#"mvl_version:    "0.100.0""#));
        assert!(src.contains("let pkgs: List[PackageInfo] = [];"));
        assert!(src.contains("packages:       pkgs,"));
    }

    #[test]
    fn generate_manifest_mvl_with_packages() {
        let pkgs = vec![
            LockedPackage {
                name: "github.com/mvl-lang/pkg-http".to_string(),
                version: "0.2.0".to_string(),
                hash: "sha256:abc".to_string(),
                commit: None,
                git: None,
            },
            LockedPackage {
                name: "github.com/mvl-lang/pkg-sqlite".to_string(),
                version: "0.1.2".to_string(),
                hash: "sha256:def".to_string(),
                commit: None,
                git: None,
            },
        ];
        let src = generate_manifest_mvl("my_app", "1.0.0", "0.100.0", &pkgs);
        assert!(src.contains(r#"let p0: PackageInfo = PackageInfo { name: "github.com/mvl-lang/pkg-http", version: "0.2.0", license: "" };"#));
        assert!(src.contains(r#"let p1: PackageInfo = PackageInfo { name: "github.com/mvl-lang/pkg-sqlite", version: "0.1.2", license: "" };"#));
        assert!(src.contains("let pkgs: List[PackageInfo] = [p0, p1];"));
    }

    #[test]
    fn generated_mvl_parses_without_errors_no_packages() {
        let src = generate_manifest_mvl("myapp", "0.1.0", "0.184.0", &[]);
        let (mut parser, _) = Parser::new(&src);
        let _prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "parse errors in generated MVL: {:?}\nSource:\n{src}",
            parser.errors()
        );
    }

    #[test]
    fn generated_mvl_parses_without_errors_with_packages() {
        let pkgs = vec![LockedPackage {
            name: "github.com/mvl-lang/pkg-http".to_string(),
            version: "0.2.0".to_string(),
            hash: "sha256:abc".to_string(),
            commit: None,
            git: None,
        }];
        let src = generate_manifest_mvl("myapp", "0.1.0", "0.184.0", &pkgs);
        let (mut parser, _) = Parser::new(&src);
        let _prog = parser.parse_program();
        assert!(
            parser.errors().is_empty(),
            "parse errors in generated MVL: {:?}\nSource:\n{src}",
            parser.errors()
        );
    }

    #[test]
    fn any_uses_std_runtime_detects_import() {
        use crate::mvl::parser::Parser;
        let src = "use std.runtime.{manifest, Manifest}\nfn main() -> Unit {}\n";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(any_uses_std_runtime(&[prog]));
    }

    #[test]
    fn any_uses_std_runtime_ignores_other_modules() {
        use crate::mvl::parser::Parser;
        let src = "use std.log.{log_info}\nfn main() -> Unit {}\n";
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(!any_uses_std_runtime(&[prog]));
    }

    #[test]
    fn load_and_generate_returns_none_for_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        // No mvl.toml → should return None gracefully
        assert!(load_and_generate(tmp.path()).is_none());
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
        // No mvl.lock → empty packages
        let prog = load_and_generate(tmp.path()).unwrap();
        // Program should have a single `manifest` function declaration
        use crate::mvl::parser::ast::Decl;
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
}
