// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use crate::mvl::backends::rust::RUST_BACKED_STDLIB;
use crate::mvl::packages;
use crate::mvl::parser::ast::{Decl, Program};
use crate::mvl::parser::Parser;
use crate::mvl::stdlib;
use std::fs;
use std::path::{Path, PathBuf};

const IMPLICIT_PRELUDE_STEMS: &[&str] = &["core", "strings", "lists"];

/// Find all `.mvl` files under `path`, filtering by whether they are test files.
pub fn mvl_files(path: &str, test_only: bool) -> Vec<PathBuf> {
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
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if is_dir {
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

/// Find all `.mvl` files under `path` regardless of test/non-test classification.
pub fn mvl_files_all(path: &str) -> Vec<PathBuf> {
    let root = Path::new(path);
    if root.is_file() {
        if root.extension().map(|e| e == "mvl").unwrap_or(false) {
            return vec![root.to_path_buf()];
        }
        return vec![];
    }
    let mut result = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().map(|e| e == "mvl").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    walk(root, &mut result);
    result.sort();
    result
}

/// Parse the given `.mvl` file, returning `Err` with a human-readable message on failure.
pub fn parse_file(path: &str) -> Result<(Program, String), String> {
    let src = fs::read_to_string(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    let (mut parser, lex_errors) = Parser::new(&src);
    if !lex_errors.is_empty() {
        let lines: Vec<_> = lex_errors
            .iter()
            .map(|e| format!("lex error: {e:?}"))
            .collect();
        return Err(lines.join("\n"));
    }
    let prog = parser.parse_program();
    let parse_errors = parser.errors();
    if !parse_errors.is_empty() {
        let lines: Vec<_> = parse_errors
            .iter()
            .map(|e| format!("parse error: {e:?}"))
            .collect();
        return Err(lines.join("\n"));
    }
    Ok((prog, src))
}

/// Collect unique top-level module names referenced by `use` declarations in `prog`,
/// excluding `std` (which is provided by the runtime, not sibling files).
pub fn collect_imported_module_names(prog: &Program) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            if ud.path.len() >= 2 {
                let mod_name = &ud.path[0];
                if mod_name != "std" && seen.insert(mod_name.clone()) {
                    names.push(mod_name.clone());
                }
            }
        }
    }
    names
}

/// Extract the file or directory stem from a path.
/// Prefixes the stem with `mvl_` if it starts with a digit (Rust package name constraint).
///
/// Special case: `foo/mod.mvl` returns `"foo"` (the directory name) rather than `"mod"`,
/// matching the Rust 2018 module naming convention where the directory gives the module name.
pub fn stem(path: &str) -> String {
    let p = Path::new(path);
    let raw = if p.is_dir() {
        p.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program")
            .to_string()
    } else {
        let file_stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mvl_program");
        // foo/mod.mvl → module name is "foo" (the directory), not "mod"
        if file_stem == "mod" {
            if let Some(dir_name) = p
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
            {
                dir_name.to_string()
            } else {
                file_stem.to_string()
            }
        } else {
            file_stem.to_string()
        }
    };
    if raw.starts_with(|c: char| c.is_ascii_digit()) {
        format!("mvl_{raw}")
    } else {
        raw
    }
}

/// Locate the `.mvl` source file for a module named `mod_name` relative to `entry_dir`.
///
/// Resolution order (Rust 2018 style, Spec 005):
/// 1. `{entry_dir}/{mod_name}.mvl`        — preferred (sibling file)
/// 2. `{entry_dir}/{mod_name}/mod.mvl`    — deprecated; emits a warning and still works
///
/// Returns `None` if neither path exists.
pub fn find_module_file(entry_dir: &Path, mod_name: &str) -> Option<PathBuf> {
    let sibling = entry_dir.join(format!("{mod_name}.mvl"));
    if sibling.exists() {
        return Some(sibling);
    }
    let legacy = entry_dir.join(mod_name).join("mod.mvl");
    if legacy.exists() {
        eprintln!(
            "warning: `{mod_name}/mod.mvl` is deprecated; \
             rename to `{mod_name}.mvl` alongside the `{mod_name}/` directory"
        );
        return Some(legacy);
    }
    None
}

/// Build the implicit prelude: `core.mvl` + `strings.mvl` + `lists.mvl`.
/// Every compile path loads these three files so their builtins are always visible.
pub fn load_implicit_prelude() -> Vec<Program> {
    const IMPLICIT: &[&str] = &["core.mvl", "strings.mvl", "lists.mvl"];
    let mut progs = Vec::new();
    for name in IMPLICIT {
        let content = stdlib::stdlib_content(name)
            .unwrap_or_else(|| panic!("{name} is embedded at compile time and must be present"));
        let (mut parser, _) = Parser::new(content);
        progs.push(parser.parse_program());
    }
    progs
}

/// Load pure-MVL stdlib modules (e.g. `json`, `collections`) imported by `progs`.
/// Resolves transitive dependencies. Excludes Rust-backed and implicit-prelude modules.
pub fn load_mvl_native_stdlib_extras(progs: &[Program]) -> Vec<Program> {
    use std::collections::HashSet;
    let mut loaded: HashSet<String> = HashSet::new();
    let mut extras: Vec<Program> = Vec::new();

    let mut pending: Vec<Program> = progs.to_vec();

    while !pending.is_empty() {
        let mut next_pending = Vec::new();
        for prog in &pending {
            for decl in &prog.declarations {
                if let Decl::Use(ud) = decl {
                    if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                        if let Some(module) = ud.path.get(1) {
                            let m = module.as_str();
                            if RUST_BACKED_STDLIB.contains(&m)
                                || IMPLICIT_PRELUDE_STEMS.contains(&m)
                            {
                                continue;
                            }
                            if loaded.insert(module.clone()) {
                                let filename = format!("{m}.mvl");
                                if let Some(content) = stdlib::stdlib_content(&filename) {
                                    let (mut p, _) = Parser::new(content);
                                    let loaded_prog = p.parse_program();
                                    next_pending.push(loaded_prog.clone());
                                    extras.push(loaded_prog);
                                }
                            }
                        }
                    }
                }
            }
        }
        pending = next_pending;
    }

    extras
}

/// Load MVL source files from `pkg.*` packages referenced by `progs`.
/// Checks local override first, then the global XDG cache.
pub fn load_pkg_modules(progs: &[Program], project_root: &Path) -> Vec<Program> {
    use std::collections::HashSet;

    let mut loaded: HashSet<String> = HashSet::new();
    let mut result: Vec<Program> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if !loaded.insert(pkg_name.clone()) {
                            continue;
                        }
                        let pkg_dir = packages::fetch::local_override_dir(project_root, pkg_name);
                        if !pkg_dir.exists() {
                            continue;
                        }
                        for sub in &["src", "src/internal"] {
                            let dir = pkg_dir.join(sub);
                            if let Ok(entries) = fs::read_dir(&dir) {
                                for entry in entries.flatten() {
                                    // Symlink escape guard (#715): skip symlinks so a
                                    // malicious package cannot point outside its directory.
                                    if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false)
                                    {
                                        continue;
                                    }
                                    let path = entry.path();
                                    if path.extension().map(|e| e == "mvl").unwrap_or(false) {
                                        if let Ok(src) = fs::read_to_string(&path) {
                                            let (mut p, _) = Parser::new(&src);
                                            result.push(p.parse_program());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    result
}

/// Find a `bridge.rs` from a `pkg.*` package used by `progs`.
/// Returns the path to the first valid package bridge found, or `None`.
///
/// Search order:
///   1. `.mvl/pkg/<name>/bridge.rs` — installed / cached package (must stay under `.mvl/pkg/`)
///   2. `pkg/<name>/bridge.rs`      — in-repo development package
pub fn find_pkg_bridge(progs: &[Program], project_root: &Path) -> Option<PathBuf> {
    let canon_pkg_root = fs::canonicalize(project_root.join(".mvl").join("pkg")).ok();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        // 1. Local override / cached install under .mvl/pkg/<name>/
                        let pkg_dir = packages::fetch::local_override_dir(project_root, pkg_name);
                        let bridge = pkg_dir.join("bridge.rs");
                        if let Ok(canon_bridge) = fs::canonicalize(&bridge) {
                            if canon_pkg_root
                                .as_ref()
                                .map(|r| canon_bridge.starts_with(r))
                                .unwrap_or(false)
                            {
                                return Some(canon_bridge);
                            }
                        }

                        // 2. In-repo development package at pkg/<name>/bridge.rs
                        let dev_bridge = project_root.join("pkg").join(pkg_name).join("bridge.rs");
                        if let Ok(canon_bridge) = fs::canonicalize(&dev_bridge) {
                            let canon_pkg = fs::canonicalize(project_root.join("pkg"))
                                .unwrap_or_else(|_| project_root.join("pkg"));
                            if canon_bridge.starts_with(&canon_pkg) {
                                return Some(canon_bridge);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Collect raw Cargo dep lines from the `[native]` section of `mvl.toml` for
/// any `pkg.*` package referenced by `progs`. Returns lines like:
///   `rusqlite = { version = "0.31", features = ["bundled"] }`
/// ready for inclusion in a generated `Cargo.toml`.
///
/// Search order mirrors `find_pkg_bridge`:
///   1. `.mvl/pkg/<name>/mvl.toml` — installed / cached package
///   2. `pkg/<name>/mvl.toml`      — in-repo development package
pub fn collect_pkg_native_dep_lines(progs: &[Program], project_root: &Path) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if !seen.insert(pkg_name.clone()) {
                            continue;
                        }
                        let pkg_dir = packages::fetch::local_override_dir(project_root, pkg_name);
                        // Try installed/cached package first; fall back to in-repo dev package.
                        // Use read_to_string directly to avoid TOCTOU race from .exists() checks.
                        let content = fs::read_to_string(pkg_dir.join("mvl.toml")).or_else(|_| {
                            fs::read_to_string(
                                project_root.join("pkg").join(pkg_name).join("mvl.toml"),
                            )
                        });
                        if let Ok(content) = content {
                            lines.extend(extract_native_dep_lines(&content));
                        }
                    }
                }
            }
        }
    }
    lines
}

/// Extract raw key=value lines from the `[native]` section of a `mvl.toml` string.
///
/// Only lines whose key is a valid Cargo crate name (`[a-zA-Z0-9_-]+`) are
/// accepted. This prevents a malicious mvl.toml from injecting arbitrary TOML
/// sections (e.g. `[patch.crates-io]`) into the generated Cargo.toml.
fn extract_native_dep_lines(content: &str) -> Vec<String> {
    let mut in_native = false;
    let mut result = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_native = line == "[native]";
            continue;
        }
        if in_native {
            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                // Accept only valid Cargo crate name characters to prevent injection.
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    result.push(line.to_string());
                }
            }
        }
    }
    result
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn tmpdir() -> TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    // stem: regular .mvl file
    #[test]
    fn stem_regular_file() {
        assert_eq!(stem("src/geometry.mvl"), "geometry");
        assert_eq!(stem("main.mvl"), "main");
    }

    // stem: foo/mod.mvl → "foo" (Rust 2018 module naming)
    #[test]
    fn stem_mod_mvl_returns_parent_dir_name() {
        assert_eq!(stem("math/mod.mvl"), "math");
        assert_eq!(stem("src/utils/mod.mvl"), "utils");
    }

    // stem: bare mod.mvl with no directory → "mod" (no parent to derive from)
    #[test]
    fn stem_bare_mod_mvl() {
        assert_eq!(stem("mod.mvl"), "mod");
    }

    // find_module_file: sibling .mvl preferred
    #[test]
    fn find_module_file_prefers_sibling() {
        let dir = tmpdir();
        let sibling = dir.path().join("math.mvl");
        let legacy = dir.path().join("math").join("mod.mvl");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&sibling, "").unwrap();
        fs::write(&legacy, "").unwrap();

        let found = find_module_file(dir.path(), "math").expect("should find a file");
        assert_eq!(
            found, sibling,
            "sibling .mvl must be preferred over mod.mvl"
        );
    }

    // find_module_file: falls back to mod.mvl when no sibling exists
    #[test]
    fn find_module_file_falls_back_to_mod_mvl() {
        let dir = tmpdir();
        let legacy = dir.path().join("math").join("mod.mvl");
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, "").unwrap();

        let found = find_module_file(dir.path(), "math").expect("should find mod.mvl");
        assert_eq!(found, legacy);
    }

    // find_module_file: returns None when neither exists
    #[test]
    fn find_module_file_returns_none_when_absent() {
        let dir = tmpdir();
        assert!(find_module_file(dir.path(), "missing").is_none());
    }
}

/// Load stdlib prelude files for all `use std.X` declarations found in `progs`.
/// Prefers on-disk files; falls back to embedded copies for read-only environments.
pub fn load_stdlib_prelude<'a>(
    progs: impl Iterator<Item = &'a Program>,
    stdlib_dir: &Path,
) -> Vec<Program> {
    use std::collections::HashSet;
    let mut loaded: HashSet<String> = HashSet::new();
    let mut prelude = Vec::new();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                    if let Some(module) = ud.path.get(1) {
                        if loaded.insert(module.clone()) {
                            let filename = format!("{module}.mvl");
                            let stdlib_file = stdlib_dir.join(&filename);
                            let src_opt = fs::read_to_string(&stdlib_file).ok().or_else(|| {
                                crate::mvl::stdlib::STDLIB_FILES
                                    .iter()
                                    .find(|(name, _)| *name == filename)
                                    .map(|(_, content)| content.to_string())
                            });
                            if let Some(src) = src_opt {
                                let (mut p, _) = Parser::new(&src);
                                prelude.push(p.parse_program());
                            }
                        }
                    }
                }
            }
        }
    }
    prelude
}
