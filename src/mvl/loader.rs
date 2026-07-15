// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

use crate::mvl::backends::llvm_text::c_symbols::derive_builtin_c_symbol;
use crate::mvl::backends::llvm_text::BuiltinSymbolInfo;
use crate::mvl::backends::rust::{RUST_BACKED_STDLIB, RUST_RUNTIME_IMPORTS};
use crate::mvl::packages;
use crate::mvl::parser::ast::{Decl, Program, TypeExpr};
use crate::mvl::parser::lexer::Span;
use crate::mvl::parser::Parser;
use crate::mvl::stdlib;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

// `collections` joins the implicit prelude in v1.3.2 so `Map::new()` and
// `Set::new()` resolve at first use without an explicit `use std.collections.{Map}`.
// #1842 was the "why does Map::new() emit a raw @Map::new symbol" report — the
// answer was that the loader never visited collections.mvl without the `use`.
const IMPLICIT_PRELUDE_STEMS: &[&str] =
    &["core", "strings", "lists", "collections", "effects", "io"];

/// Format an error message with source line and caret indicator.
fn format_error_with_source(src: &str, span: Span, message: &str) -> String {
    let line_text = src.lines().nth((span.line - 1) as usize).unwrap_or("");
    let line_num = span.line.to_string();
    let padding = " ".repeat(line_num.len());
    let caret_col = (span.col as usize).saturating_sub(1);
    let caret = "^".repeat((span.len as usize).max(1));
    format!(
        "error at {line_num}:{col}: {message}\n{padding} | {line_text}\n{padding} | {spaces}{caret}",
        col = span.col,
        spaces = " ".repeat(caret_col),
    )
}

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
            // Skip `.mvl/` — the package install directory (analogous to node_modules).
            // Package files are loaded from the XDG cache via load_pkg_modules; including
            // them here would double-load them as user programs and corrupt the prelude.
            if path.file_name().and_then(|n| n.to_str()) == Some(".mvl") {
                continue;
            }
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
                if p.file_name().and_then(|n| n.to_str()) == Some(".mvl") {
                    continue;
                }
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
            .map(|e| format_error_with_source(&src, e.span, &e.message))
            .collect();
        return Err(lines.join("\n"));
    }
    let prog = parser.parse_program();
    let parse_errors = parser.errors();
    if !parse_errors.is_empty() {
        let lines: Vec<_> = parse_errors
            .iter()
            .map(|e| format_error_with_source(&src, e.span, &e.message))
            .collect();
        return Err(lines.join("\n"));
    }
    Ok((prog, src))
}

/// Collect the unique module paths referenced in `use` declarations.
///
/// Returns dot-joined module paths for non-stdlib, non-pkg imports.
/// For `use backends.llvm.context::X` → `"backends.llvm.context"`.
/// For `use context::{A, B}` → `"context"`.
/// `std.*` and `pkg.*` are excluded (handled separately).
pub fn collect_imported_module_names(prog: &Program) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for decl in &prog.declarations {
        if let Decl::Use(ud) = decl {
            // Determine the module path segments (all but the item).
            // Bare-style  `use mod;`       — path IS the module name.
            // Brace-style `use mod::{A, B}` — path IS the module path.
            // Item-style  `use mod::Item`  — module is path[..len-1].
            let module_segs: &[String] = if ud.items.is_empty() {
                if ud.path.len() < 2 {
                    // Bare module import `use models;` — entire path is module.
                    &ud.path[..]
                } else {
                    &ud.path[..ud.path.len() - 1]
                }
            } else {
                &ud.path[..]
            };
            if module_segs.is_empty() {
                continue;
            }
            let first = module_segs[0].as_str();
            if first == "std" || first == "pkg" {
                continue;
            }
            let mod_name = module_segs.join(".");
            if seen.insert(mod_name.clone()) {
                names.push(mod_name);
            }
        }
    }
    names
}

/// Collect user-level sibling modules imported by `prog` transitively — walk
/// each newly-discovered module's own `use` declarations until fixed point.
///
/// Solves the emitter-style case where the entry file imports peers that
/// themselves import peers (e.g. `compiler/backends/llvm/emitter.mvl` imports
/// `emit_program.mvl` which imports `emit_types.mvl`).  The single-hop
/// collection in earlier CLI implementations missed transitive siblings.
///
/// `std.*` and `pkg.*` modules are filtered out by
/// [`collect_imported_module_names`] and handled on separate load paths.
/// Files that cannot be read or parsed are silently skipped — downstream
/// resolver/checker passes surface the resulting errors with proper
/// diagnostics.
///
/// Returns `(mod_name, path_str, parsed_prog)` triples sorted by mod_name.
pub fn load_sibling_modules_transitive(
    prog: &Program,
    entry_dir: &Path,
) -> Vec<(String, String, Program)> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut result: Vec<(String, String, Program)> = Vec::new();
    let mut frontier: Vec<Program> = vec![prog.clone()];
    while !frontier.is_empty() {
        let mut next: Vec<Program> = Vec::new();
        for p in &frontier {
            for mod_name in collect_imported_module_names(p) {
                if !seen.insert(mod_name.clone()) {
                    continue;
                }
                let mod_path = match find_module_file(entry_dir, &mod_name) {
                    Some(path) => path,
                    None => continue, // Resolver will surface the missing-module error.
                };
                let content = match fs::read_to_string(&mod_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let (mut parser, _) = Parser::new(&content);
                let sib_prog = parser.parse_program();
                let path_str = mod_path.display().to_string();
                next.push(sib_prog.clone());
                result.push((mod_name, path_str, sib_prog));
            }
        }
        frontier = next;
    }
    result.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    result
}

/// Infer the module root (`base_dir`) for a single-file entry point by walking
/// ancestors until one of its qualified imports resolves.
///
/// When `mvl check / assurance / prove` is invoked on a file that lives inside a
/// qualified module tree (e.g. `compiler/backends/llvm/emitter.mvl` which imports
/// `use backends.llvm.emit_context::X`), using the file's parent as `base_dir`
/// produces bare module names ("emit_context") that don't match the qualified
/// names in `use` declarations ("backends.llvm.emit_context").
///
/// Strategy: for the first qualified import (containing a `.`), convert to a
/// relative file path and walk `entry_dir` ancestors until
/// `{ancestor}/{mod/path}.mvl` exists — that ancestor is the module root.
/// The walk is bounded by project-root markers (`mvl.toml` / `mvl.lock` /
/// `.git`) so it never escapes the user's workspace.  Falls back to
/// `entry_dir` (the file's parent) when no qualified import resolves,
/// preserving existing behaviour for flat-layout projects.
pub fn infer_base_dir_from_qualified_imports(entry_file: &Path) -> PathBuf {
    let entry_dir = entry_file.parent().unwrap_or(Path::new("."));
    let content = match fs::read_to_string(entry_file) {
        Ok(c) => c,
        Err(_) => return entry_dir.to_path_buf(),
    };
    let (mut parser, _) = Parser::new(&content);
    let prog = parser.parse_program();
    for mod_name in collect_imported_module_names(&prog) {
        if !mod_name.contains('.') {
            continue;
        }
        let rel_file = format!("{}.mvl", mod_name.split('.').collect::<Vec<_>>().join("/"));
        for ancestor in entry_dir.ancestors() {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            if ancestor.join(&rel_file).exists() {
                return ancestor.to_path_buf();
            }
            if ancestor.join("mvl.toml").exists()
                || ancestor.join("mvl.lock").exists()
                || ancestor.join(".git").exists()
            {
                break;
            }
        }
    }
    entry_dir.to_path_buf()
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

/// Derive a dot-separated module name from `file_path` relative to `base_dir`.
///
/// `base_dir` is the directory passed to the CLI command (e.g. `mvl check src/`).
/// Files directly under `base_dir` get a bare name; files in subdirectories get
/// a dot-qualified name matching their relative path.
///
/// Examples (base_dir = "src/"):
/// - `src/context.mvl`              → `"context"`
/// - `src/backends/llvm/context.mvl` → `"backends.llvm.context"`
/// - `src/foo/mod.mvl`              → `"foo"` (mod.mvl is transparent)
pub fn qualified_stem(base_dir: &Path, file_path: &Path) -> String {
    let rel = file_path.strip_prefix(base_dir).unwrap_or(file_path);
    let no_ext = rel.with_extension("");
    let parts: Vec<&str> = no_ext
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    // Transparent mod.mvl: foo/mod.mvl → ["foo", "mod"] → drop "mod" → ["foo"]
    let parts = if parts.last() == Some(&"mod") {
        &parts[..parts.len() - 1]
    } else {
        &parts[..]
    };
    if parts.is_empty() {
        return "mvl_program".to_string();
    }
    let raw = parts.join(".");
    if raw.starts_with(|c: char| c.is_ascii_digit()) {
        format!("mvl_{raw}")
    } else {
        raw
    }
}

/// Return paths to all non-test `.mvl` files in `dir` (non-recursive).
///
/// Used to discover ambient sibling modules in a directory: all files in the same
/// directory form a single module scope (Go model — #1706) and can call each other's
/// extension methods via type dispatch without explicit `use` imports.
pub fn sibling_module_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?;
            if path.is_file() && name.ends_with(".mvl") && !name.ends_with("_test.mvl") {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    files
}

/// Locate the `.mvl` source file for a module named `mod_name` relative to `entry_dir`.
///
/// `mod_name` may be a dot-qualified path (e.g. `"backends.llvm.context"`) or a
/// bare single-segment name (e.g. `"context"`). Dots are treated as path separators,
/// so `"backends.llvm.context"` resolves to `entry_dir/backends/llvm/context.mvl`.
///
/// Resolution order (Rust 2018 style, Spec 005):
/// 1. `{entry_dir}/{mod/path}.mvl`         — preferred
/// 2. Qualified only: walk `entry_dir` ancestors and try
///    `{ancestor}/{mod/path}.mvl`.  Bounded by project-root markers
///    (`mvl.toml` / `mvl.lock` / `.git`) so the walk doesn't escape the
///    user's workspace.  Enables `mvl run` on entry files that live
///    inside the qualified module tree (e.g.
///    `mvl run compiler/backends/llvm/emitter.mvl` importing
///    `use backends.llvm.emit_context::X` finds
///    `compiler/backends/llvm/emit_context.mvl`).
/// 3. `{entry_dir}/{mod_name}/mod.mvl`     — single-segment only, deprecated
///
/// Returns `None` if no candidate exists.
pub fn find_module_file(entry_dir: &Path, mod_name: &str) -> Option<PathBuf> {
    // Convert dot-path to filesystem path: "backends.llvm.context" → "backends/llvm/context"
    let rel_path: PathBuf = mod_name.split('.').collect::<Vec<_>>().join("/").into();
    let rel_file = format!("{}.mvl", rel_path.display());
    let sibling = entry_dir.join(&rel_file);
    if sibling.exists() {
        return Some(sibling);
    }
    // Qualified (dot-separated) module names may resolve against an ancestor
    // of `entry_dir` — required when an entry file itself lives inside the
    // qualified module tree (e.g. entry = `compiler/backends/llvm/emitter.mvl`
    // importing `use backends.llvm.emit_context::X` — the file lives at
    // `compiler/backends/llvm/emit_context.mvl` alongside `emitter.mvl`, so
    // the resolver must strip the shared trailing prefix).
    //
    // The walk is bounded by project-root markers so it never leaks outside
    // the user's mental workspace.
    if mod_name.contains('.') {
        for ancestor in entry_dir.ancestors().skip(1) {
            // Skip empty path component that `Path::ancestors` may yield for
            // relative inputs — `Path::new("").join(...)` still succeeds but
            // is semantically the CWD, which rule 1 already covered.
            if ancestor.as_os_str().is_empty() {
                break;
            }
            let candidate = ancestor.join(&rel_file);
            if candidate.exists() {
                return Some(candidate);
            }
            // Stop after checking the candidate at a project root — walking
            // any higher would leave the workspace.
            if ancestor.join("mvl.toml").exists()
                || ancestor.join("mvl.lock").exists()
                || ancestor.join(".git").exists()
            {
                break;
            }
        }
    }
    // Legacy mod.mvl form: only valid for single-segment names.
    if !mod_name.contains('.') {
        let legacy = entry_dir.join(mod_name).join("mod.mvl");
        if legacy.exists() {
            eprintln!(
                "warning: `{mod_name}/mod.mvl` is deprecated; \
                 rename to `{mod_name}.mvl` alongside the `{mod_name}/` directory"
            );
            return Some(legacy);
        }
    }
    None
}

/// Build the implicit prelude: `core.mvl` + `strings.mvl` + `lists.mvl` + `effects.mvl`.
/// Every compile path loads these so their builtins and the effect hierarchy
/// (`Log > Clock`, `IO > Log + …`) are always visible.
pub fn load_implicit_prelude() -> Vec<Program> {
    const IMPLICIT: &[&str] = &[
        "core.mvl",
        "strings.mvl",
        "lists.mvl",
        "collections.mvl",
        "effects.mvl",
    ];
    let mut progs = Vec::new();
    for name in IMPLICIT {
        let content = stdlib::stdlib_content(name).unwrap_or_else(|| {
            panic!(
                "stdlib file `{name}` not found — run `make install` or `mvl self install` to install the stdlib"
            )
        });
        let (mut parser, _) = Parser::new(&content);
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
                    if ud.path.first().map(|s| s == "std").unwrap_or(false) && ud.path.len() >= 2 {
                        // Top-level module name, e.g. "kv" in std.kv.file or "toml" in std.toml.
                        let m = ud.path[1].as_str();
                        if RUST_BACKED_STDLIB.contains(&m) || IMPLICIT_PRELUDE_STEMS.contains(&m) {
                            continue;
                        }
                        // Cache key: "kv.file" for std.kv.file, "toml" for std.toml.
                        // Filename: "kv/file.mvl" for std.kv.file, "toml.mvl" for std.toml.
                        let subpath = &ud.path[1..];
                        let cache_key = subpath.join(".");
                        let filename = format!("{}.mvl", subpath.join("/"));
                        if loaded.insert(cache_key) {
                            if let Some(content) = stdlib::stdlib_content(&filename) {
                                let (mut p, _) = Parser::new(&content);
                                let mut loaded_prog = p.parse_program();
                                // For hybrid modules (in RUST_RUNTIME_IMPORTS but not in
                                // RUST_BACKED_STDLIB), types normally come from
                                // `use mvl_runtime::stdlib::X::*` and must be stripped to
                                // avoid duplicate definitions (#897).
                                // Exception: if RUNTIME_OWNED_TYPES lists specific names for
                                // this module, only those are stripped — any types absent from
                                // the list exist only in MVL and must pass through (e.g.
                                // `Logger` in the `log` module, which is MVL-only).
                                if RUST_RUNTIME_IMPORTS.contains(&m)
                                    && !RUST_BACKED_STDLIB.contains(&m)
                                {
                                    // Hybrid modules: types come from `use mvl_runtime::stdlib::X::*`
                                    // — strip MVL type decls to avoid duplicate definitions (#897).
                                    loaded_prog
                                        .declarations
                                        .retain(|d| !matches!(d, Decl::Type(_)));
                                    // Inject a synthetic `use std.<module>` so the TIR lowerer
                                    // records the dependency.  The emitter's all_modules logic
                                    // (#1744) then emits `use mvl_runtime::stdlib::<module>::*;`
                                    // in every file that receives these prelude functions,
                                    // making types like `Signal` visible without a re-declaration.
                                    loaded_prog.declarations.insert(
                                        0,
                                        Decl::Use(crate::mvl::parser::ast::UseDecl {
                                            reexport: false,
                                            path: vec!["std".to_string(), m.to_string()],
                                            items: vec![],
                                            module_only: false,
                                            span: Default::default(),
                                        }),
                                    );
                                }
                                next_pending.push(loaded_prog.clone());
                                extras.push(loaded_prog);
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

/// Load pure-MVL function bodies from `RUST_BACKED_STDLIB` modules for the LLVM backend.
///
/// The Rust transpiler handles these modules entirely via `mvl_runtime::stdlib::X::*`,
/// but the LLVM backend only dispatches `builtin fn` declarations to C-ABI symbols.
/// Non-builtin functions (`find_all`, `replace`, `format_datetime`, etc.) are written
/// in MVL and need their bodies compiled to LLVM IR.
///
/// This function loads each referenced RUST_BACKED_STDLIB module's `.mvl` source,
/// strips type declarations (manually registered by `collect_stdlib_imports`) and
/// `builtin fn` declarations (routed via the C-ABI dispatch table), and returns
/// programs containing only the pure MVL function bodies.
pub fn load_rust_backed_stdlib_fns(progs: &[Program]) -> Vec<Program> {
    use std::collections::HashSet;
    let mut loaded: HashSet<String> = HashSet::new();
    let mut extras: Vec<Program> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                    if let Some(module) = ud.path.get(1) {
                        let m = module.as_str();
                        if !RUST_BACKED_STDLIB.contains(&m) {
                            continue;
                        }
                        if !loaded.insert(module.clone()) {
                            continue;
                        }
                        let filename = format!("{m}.mvl");
                        if let Some(content) = stdlib::stdlib_content(&filename) {
                            let (mut p, _) = Parser::new(&content);
                            let mut loaded_prog = p.parse_program();
                            // Collect names of builtin fns — the LLVM C-ABI dispatch
                            // handles these, so we must not emit conflicting MVL bodies.
                            // Also skip non-builtin pub fns that share a name with a
                            // builtin (e.g. io.path shadows _mvl_io_path).
                            let builtin_names: HashSet<String> = loaded_prog
                                .declarations
                                .iter()
                                .filter_map(|d| {
                                    if let Decl::Fn(fd) = d {
                                        if fd.is_builtin {
                                            return Some(fd.name.clone());
                                        }
                                    }
                                    None
                                })
                                .collect();
                            // Types the LLVM backend handles as opaque ptrs where
                            // struct construction/destruction in MVL would fail.
                            // Excludes String/List/Map etc. which are fine as opaque
                            // ptrs in function signatures.
                            const OPAQUE_PTR_TYPES: &[&str] =
                                &["Path", "TcpListener", "TcpStream", "Stdout", "Stderr"];
                            // Keep type declarations (structs/enums needed by
                            // pure MVL functions) except for opaque-ptr types,
                            // and non-builtin function declarations that don't
                            // reference opaque-ptr types in their signature.
                            loaded_prog.declarations.retain(|d| match d {
                                Decl::Type(td) => !OPAQUE_PTR_TYPES.contains(&td.name.as_str()),
                                Decl::Fn(fd) => {
                                    if fd.is_builtin || builtin_names.contains(&fd.name) {
                                        return false;
                                    }
                                    // Skip functions that use opaque-ptr types in
                                    // params or return — the LLVM backend can't
                                    // construct or destructure those types.
                                    let uses_opaque = fd
                                        .params
                                        .iter()
                                        .any(|p| type_uses_opaque(&p.ty, OPAQUE_PTR_TYPES))
                                        || type_uses_opaque(&fd.return_type, OPAQUE_PTR_TYPES);
                                    !uses_opaque
                                }
                                _ => false,
                            });
                            if !loaded_prog.declarations.is_empty() {
                                extras.push(loaded_prog);
                            }
                        }
                    }
                }
            }
        }
    }

    extras
}

/// Check if a `TypeExpr` references any type in the opaque list.
fn type_uses_opaque(ty: &crate::mvl::parser::ast::TypeExpr, opaque: &[&str]) -> bool {
    use crate::mvl::parser::ast::TypeExpr;
    match ty {
        TypeExpr::Base { name, args, .. } => {
            opaque.contains(&name.as_str()) || args.iter().any(|a| type_uses_opaque(a, opaque))
        }
        TypeExpr::Option { inner, .. }
        | TypeExpr::Labeled { inner, .. }
        | TypeExpr::Refined { inner, .. }
        | TypeExpr::Ref { inner, .. } => type_uses_opaque(inner, opaque),
        TypeExpr::Result { ok, err, .. } => {
            type_uses_opaque(ok, opaque) || type_uses_opaque(err, opaque)
        }
        _ => false,
    }
}

/// Build a map from package short name (e.g. `"http"`) to its source directory
/// in the XDG cache (e.g. `~/.local/share/mvl/pkg/github.com_mvl-lang_pkg-http/0.2.0`).
///
/// Resolution order:
///   1. Self-package: if `project_root/mvl.toml` names a package, map it to `project_root`
///      so a package's own smoke tests can `use pkg.<name>` without a published release.
///   2. Locked packages: read `mvl.lock` from `project_root`, look up each entry in the
///      XDG cache, and insert its short name from the cached `mvl.toml`.
///   3. Transitive packages: for each mapped package, read its `mvl.toml` and add any
///      declared dependency that exists in the XDG cache but is not yet in the map.
///      This repeats until no new packages are found (#1477).
///
/// Returns an empty map if no lock file exists or the cache is empty.
fn build_pkg_name_map(project_root: &Path) -> std::collections::HashMap<String, PathBuf> {
    build_pkg_name_map_with_cache(project_root, &packages::fetch::pkg_cache_root())
}

/// Inner implementation of [`build_pkg_name_map`] with an explicit `cache_root` so tests
/// can pass a temporary directory without touching environment variables.
fn build_pkg_name_map_with_cache(
    project_root: &Path,
    cache_root: &Path,
) -> std::collections::HashMap<String, PathBuf> {
    let lockfile = packages::lock::LockFile::load_or_empty(project_root);
    let mut map = std::collections::HashMap::new();

    // Self-package: if project_root IS a package, make it importable by its own smoke tests.
    if let Ok(manifest) = packages::manifest::Manifest::load(project_root) {
        map.insert(manifest.package.name, project_root.to_path_buf());
    }

    for pkg in &lockfile.packages {
        let cache_dir = pkg_cache_dir_in(cache_root, &pkg.name, &pkg.version);
        if !cache_dir.exists() {
            continue;
        }
        if let Ok(manifest) = packages::manifest::Manifest::load(&cache_dir) {
            map.insert(manifest.package.name, cache_dir);
        }
    }

    // Transitively expand: for each mapped package, read its manifest and add any
    // dependencies that are present in the XDG cache but not yet in the map.
    // This handles the common pattern where a downstream project only declares a
    // direct dependency (e.g. pkg-health) but that package uses types from another
    // package (e.g. pkg-http) — without this expansion the transitive types would
    // be invisible to the compiler (#1477).
    let mut seen_dirs: std::collections::HashSet<PathBuf> = map.values().cloned().collect();
    let mut frontier: Vec<PathBuf> = map.values().cloned().collect();
    while !frontier.is_empty() {
        let mut next_frontier = Vec::new();
        for pkg_dir in &frontier {
            let Ok(manifest) = packages::manifest::Manifest::load(pkg_dir) else {
                continue;
            };
            for (dep_id, dep_spec) in &manifest.dependencies {
                let tag = dep_spec.version_str();
                let version = tag.strip_prefix('v').unwrap_or(tag);
                let dep_dir = pkg_cache_dir_in(cache_root, dep_id, version);
                if !dep_dir.exists() || !seen_dirs.insert(dep_dir.clone()) {
                    continue;
                }
                if let Ok(dep_manifest) = packages::manifest::Manifest::load(&dep_dir) {
                    map.insert(dep_manifest.package.name, dep_dir.clone());
                    next_frontier.push(dep_dir);
                }
            }
        }
        frontier = next_frontier;
    }

    map
}

/// Compute the cache directory for a package using an explicit `cache_root` instead of the
/// XDG default.  Mirrors [`packages::fetch::pkg_cache_dir`] but without the global env lookup.
fn pkg_cache_dir_in(cache_root: &Path, name: &str, version: &str) -> PathBuf {
    cache_root.join(sanitize_pkg_name(name)).join(version)
}

/// Replace path-unsafe characters in a package ID with underscores so it can be used as a
/// directory component.  Mirrors the `sanitize` function in `packages::fetch`:
/// `/`, `:`, `\`, `\0` → `_`; `.` and `..` components are removed.
fn sanitize_pkg_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned: Vec<&str> = replaced
        .split('_')
        .filter(|c| *c != "." && *c != "..")
        .collect();
    cleaned.join("_")
}
/// Collect `builtin fn` declarations from all stdlib modules visible to `progs`.
///
/// Returns a map: MVL function name → [`BuiltinSymbolInfo`] (C-ABI symbol +
/// MVL return / parameter types).
///
/// Scans transitively:
/// - Implicit prelude modules (`core`, `strings`, `lists`, `effects`, `io`) — always loaded.
/// - Modules explicitly imported via `use std.X.{...}` in `progs`.
/// - Modules transitively imported by any of the above (e.g. `std.log` uses
///   `std.time`, so `now`/`format_instant` must also be registered).
///
/// Used by the llvm_text backend to dispatch `builtin fn` calls to C runtime symbols.
pub fn collect_llvm_text_builtins(progs: &[Program]) -> HashMap<String, BuiltinSymbolInfo> {
    let mut result: HashMap<String, BuiltinSymbolInfo> = HashMap::new();
    let mut loaded_modules: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Worklist: implicit prelude + direct user imports + (discovered transitively below).
    let mut worklist: Vec<String> = IMPLICIT_PRELUDE_STEMS
        .iter()
        .map(|s| s.to_string())
        .collect();
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "std").unwrap_or(false) {
                    if let Some(module) = ud.path.get(1) {
                        worklist.push(module.clone());
                    }
                }
            }
        }
    }

    while let Some(m) = worklist.pop() {
        if !loaded_modules.insert(m.clone()) {
            continue;
        }
        let filename = format!("{m}.mvl");
        let Some(content) = stdlib::stdlib_content(&filename) else {
            continue;
        };
        let (mut p, _) = Parser::new(&content);
        let mod_prog = p.parse_program();
        for mod_decl in &mod_prog.declarations {
            match mod_decl {
                Decl::Fn(fd) if fd.is_builtin => {
                    // Derive C-ABI symbol.  For extension methods (receiver_type is Some),
                    // the MVL call site uses `Type::method` syntax so we register both
                    // the short name and the qualified name.
                    let fn_key = match &fd.receiver_type {
                        Some(recv) => format!("{recv}::{}", fd.name),
                        None => fd.name.clone(),
                    };
                    let c_sym = derive_builtin_c_symbol(&m, &fd.receiver_type, &fd.name);
                    let param_tys: Vec<TypeExpr> = fd.params.iter().map(|p| p.ty.clone()).collect();
                    result.insert(
                        fn_key,
                        BuiltinSymbolInfo {
                            c_sym,
                            ret_ty: fd.return_type.as_ref().clone(),
                            param_tys,
                        },
                    );
                }
                // Follow transitive std.* imports so dependent stdlib modules
                // (e.g. std.log → std.time) contribute their builtins too.
                Decl::Use(ud) if ud.path.first().map(|s| s == "std").unwrap_or(false) => {
                    if let Some(dep) = ud.path.get(1) {
                        if !loaded_modules.contains(dep) {
                            worklist.push(dep.clone());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    result
}

/// Load MVL source files from `pkg.*` packages referenced by `progs`.
/// Resolves packages directly from the XDG cache using `mvl.lock`.
pub fn load_pkg_modules(
    progs: &[Program],
    project_root: &Path,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<Program> {
    let pkg_map = build_pkg_name_map(project_root);
    let mut result: Vec<Program> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if !seen.insert(pkg_name.clone()) {
                            continue;
                        }
                        let Some(pkg_dir) = pkg_map.get(pkg_name.as_str()) else {
                            continue;
                        };
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
                                        if path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .map(|n| n.ends_with("_test.mvl"))
                                            .unwrap_or(false)
                                        {
                                            continue;
                                        }
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

/// Like [`load_pkg_modules`] but returns `(pkg_name, Program)` pairs so callers
/// can track which package each source file came from (used by the Rust backend
/// to emit collision-free names when two packages export the same function).
pub fn load_pkg_modules_tagged(
    progs: &[Program],
    project_root: &Path,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<(String, Program)> {
    let pkg_map = build_pkg_name_map(project_root);
    let mut result: Vec<(String, Program)> = Vec::new();

    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if !seen.insert(pkg_name.clone()) {
                            continue;
                        }
                        let Some(pkg_dir) = pkg_map.get(pkg_name.as_str()) else {
                            continue;
                        };
                        for sub in &["src", "src/internal"] {
                            let dir = pkg_dir.join(sub);
                            if let Ok(entries) = fs::read_dir(&dir) {
                                for entry in entries.flatten() {
                                    if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false)
                                    {
                                        continue;
                                    }
                                    let path = entry.path();
                                    if path.extension().map(|e| e == "mvl").unwrap_or(false) {
                                        if path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .map(|n| n.ends_with("_test.mvl"))
                                            .unwrap_or(false)
                                        {
                                            continue;
                                        }
                                        if let Ok(src) = fs::read_to_string(&path) {
                                            let (mut p, _) = Parser::new(&src);
                                            result.push((pkg_name.clone(), p.parse_program()));
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
/// Resolves packages directly from the XDG cache using `mvl.lock`.
pub fn find_pkg_bridge(progs: &[Program], project_root: &Path) -> Option<PathBuf> {
    let pkg_map = build_pkg_name_map(project_root);
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if let Some(pkg_dir) = pkg_map.get(pkg_name.as_str()) {
                            let bridge = pkg_dir.join("bridge.rs");
                            if let Ok(canon_bridge) = fs::canonicalize(&bridge) {
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

/// Find an `llvm.rs` from a `pkg.*` package used by `progs` (#811).
///
/// Parallel to `find_pkg_bridge` but for the LLVM backend path.
/// `llvm.rs` provides `#[no_mangle] pub extern "C" fn` implementations
/// that are compiled into a shared library and loaded via `lli --load=`.
pub fn find_pkg_llvm_bridge(progs: &[Program], project_root: &Path) -> Option<PathBuf> {
    let pkg_map = build_pkg_name_map(project_root);
    for prog in progs {
        for decl in &prog.declarations {
            if let Decl::Use(ud) = decl {
                if ud.path.first().map(|s| s == "pkg").unwrap_or(false) {
                    if let Some(pkg_name) = ud.path.get(1) {
                        if let Some(pkg_dir) = pkg_map.get(pkg_name.as_str()) {
                            let llvm_bridge = pkg_dir.join("llvm.rs");
                            if let Ok(canon) = fs::canonicalize(&llvm_bridge) {
                                return Some(canon);
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
/// Resolves packages directly from the XDG cache using `mvl.lock`.
pub fn collect_pkg_native_dep_lines(progs: &[Program], project_root: &Path) -> Vec<String> {
    use std::collections::HashSet;
    let pkg_map = build_pkg_name_map(project_root);
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
                        if let Some(pkg_dir) = pkg_map.get(pkg_name.as_str()) {
                            if let Ok(content) = fs::read_to_string(pkg_dir.join("mvl.toml")) {
                                lines.extend(extract_native_dep_lines(&content));
                            }
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
                    result.push(strip_cargo_foreign_keys(line));
                }
            }
        }
    }
    result
}

/// Keys valid in `[native]` mvl.toml entries but NOT valid Cargo dependency fields.
const CARGO_FOREIGN_KEYS: &[&str] = &["license"];

/// Strip MVL-only metadata keys from a `[native]` dep line before it is written
/// into a generated Cargo.toml.
///
/// `rusqlite = { version = "0.31", features = ["bundled"], license = "MIT" }`
/// → `rusqlite = { version = "0.31", features = ["bundled"] }`
///
/// Non-inline-table lines are returned unchanged.
fn strip_cargo_foreign_keys(line: &str) -> String {
    let Some(eq_pos) = line.find('=') else {
        return line.to_string();
    };
    let dep_name = line[..eq_pos].trim();
    let val = line[eq_pos + 1..].trim();

    if !(val.starts_with('{') && val.ends_with('}')) {
        return line.to_string();
    }
    let inner = &val[1..val.len() - 1];
    let filtered: Vec<String> = split_inline_table_parts(inner)
        .into_iter()
        .filter(|part| {
            if let Some(k_eq) = part.trim().find('=') {
                let k = part.trim()[..k_eq].trim();
                !CARGO_FOREIGN_KEYS.contains(&k)
            } else {
                true
            }
        })
        .map(|p| p.trim().to_string())
        .collect();

    format!("{dep_name} = {{ {} }}", filtered.join(", "))
}

/// Split the inner content of an inline TOML table on commas, respecting
/// quoted strings and `[...]` arrays.
fn split_inline_table_parts(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_str = false;
    let mut escaped = false;
    let mut bracket_depth: u32 = 0;
    for c in s.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && in_str {
            current.push(c);
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            current.push(c);
            continue;
        }
        if !in_str {
            match c {
                '[' => {
                    bracket_depth += 1;
                    current.push(c);
                    continue;
                }
                ']' => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    current.push(c);
                    continue;
                }
                ',' if bracket_depth == 0 => {
                    parts.push(current.clone());
                    current.clear();
                    continue;
                }
                _ => {}
            }
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
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

    // collect_imported_module_names: brace-style import `use mod::{A, B}` is collected.
    // Regression for the `use config::{...}` case where path.len() == 1.
    #[test]
    fn collect_imported_module_names_brace_import() {
        let src = "use config::{ServerConfig, load_config}";
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        let names = collect_imported_module_names(&prog);
        assert_eq!(names, vec!["config".to_string()]);
    }

    // collect_imported_module_names: `use mod::item;` form is still collected.
    #[test]
    fn collect_imported_module_names_dotted_import() {
        let src = "use parser::parse_line;";
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        let names = collect_imported_module_names(&prog);
        assert_eq!(names, vec!["parser".to_string()]);
    }

    // collect_imported_module_names: bare `use models;` (no item, no braces) is collected.
    #[test]
    fn collect_imported_module_names_bare_import() {
        let src = "use models;";
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        let names = collect_imported_module_names(&prog);
        assert_eq!(names, vec!["models".to_string()]);
    }

    // collect_imported_module_names: qualified path `use backends.llvm.context::X`
    // returns the full dot-joined module name.
    #[test]
    fn collect_imported_module_names_qualified_path() {
        let src = "use backends.llvm.context::EmitCtx;";
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        let names = collect_imported_module_names(&prog);
        assert_eq!(names, vec!["backends.llvm.context".to_string()]);
    }

    // collect_imported_module_names: `use std.io.{...}` must NOT appear in sibling list.
    #[test]
    fn collect_imported_module_names_excludes_std() {
        let src = "use std.io.{read_file}";
        let (mut p, _) = crate::mvl::parser::Parser::new(src);
        let prog = p.parse_program();
        let names = collect_imported_module_names(&prog);
        assert!(names.is_empty());
    }

    // qualified_stem: direct child → bare name.
    #[test]
    fn qualified_stem_direct_child() {
        let base = std::path::Path::new("src");
        assert_eq!(
            qualified_stem(base, std::path::Path::new("src/context.mvl")),
            "context"
        );
        assert_eq!(
            qualified_stem(base, std::path::Path::new("src/main.mvl")),
            "main"
        );
    }

    // qualified_stem: nested file → dot-separated path.
    #[test]
    fn qualified_stem_nested() {
        let base = std::path::Path::new("compiler");
        assert_eq!(
            qualified_stem(
                base,
                std::path::Path::new("compiler/backends/llvm/context.mvl")
            ),
            "backends.llvm.context"
        );
    }

    // qualified_stem: two files sharing a basename get distinct qualified names.
    #[test]
    fn qualified_stem_no_collision_for_same_basename() {
        let base = std::path::Path::new("compiler");
        let a = qualified_stem(base, std::path::Path::new("compiler/context.mvl"));
        let b = qualified_stem(
            base,
            std::path::Path::new("compiler/backends/llvm/context.mvl"),
        );
        assert_eq!(a, "context");
        assert_eq!(b, "backends.llvm.context");
        assert_ne!(a, b);
    }

    // qualified_stem: mod.mvl is transparent.
    #[test]
    fn qualified_stem_mod_mvl_transparent() {
        let base = std::path::Path::new("src");
        assert_eq!(
            qualified_stem(base, std::path::Path::new("src/math/mod.mvl")),
            "math"
        );
    }

    // find_module_file: qualified dot-path resolves to nested file.
    #[test]
    fn find_module_file_qualified_dot_path() {
        let dir = tmpdir();
        let nested = dir.path().join("backends").join("llvm");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("context.mvl");
        fs::write(&file, "").unwrap();

        let found = find_module_file(dir.path(), "backends.llvm.context")
            .expect("should find nested file via dot-path");
        assert_eq!(found, file);
    }

    // find_module_file: entry file living inside the qualified module tree
    // resolves siblings by their fully-qualified name.  This is the shape
    // triggered by `mvl run compiler/backends/llvm/emitter.mvl` importing
    // `use backends.llvm.emit_context::X`.
    #[test]
    fn find_module_file_qualified_from_inside_module_tree() {
        let dir = tmpdir();
        // Set up: <root>/compiler/backends/llvm/{emitter,emit_context}.mvl
        //         <root>/mvl.toml    ← project marker bounding the walk
        let llvm_dir = dir.path().join("compiler").join("backends").join("llvm");
        fs::create_dir_all(&llvm_dir).unwrap();
        fs::write(dir.path().join("mvl.toml"), "").unwrap();
        fs::write(llvm_dir.join("emitter.mvl"), "").unwrap();
        let target = llvm_dir.join("emit_context.mvl");
        fs::write(&target, "").unwrap();

        // Entry file's parent is inside the qualified tree; the qualified
        // path "backends.llvm.emit_context" duplicates the trailing
        // "backends/llvm/" already present in entry_dir.
        let found = find_module_file(&llvm_dir, "backends.llvm.emit_context")
            .expect("qualified sibling should resolve by walking to ancestor");
        assert_eq!(found, target);
    }

    // find_module_file: qualified-path ancestor walk is bounded by a
    // project-root marker (mvl.toml).  A candidate file living OUTSIDE
    // the marked project must not be discovered.
    #[test]
    fn find_module_file_qualified_walk_bounded_by_project_root() {
        let dir = tmpdir();
        // outer/                       ← candidate file lives here (should NOT be found)
        //   backends/llvm/other.mvl
        //   project/                   ← project root (mvl.toml)
        //     src/entry.mvl            ← entry point
        let outer_target = dir.path().join("backends").join("llvm").join("other.mvl");
        fs::create_dir_all(outer_target.parent().unwrap()).unwrap();
        fs::write(&outer_target, "").unwrap();
        let project = dir.path().join("project");
        let src = project.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(project.join("mvl.toml"), "").unwrap();
        fs::write(src.join("entry.mvl"), "").unwrap();

        // Walk starts at src/, goes to project/, checks mvl.toml (no match
        // in candidate), then stops.  The outer file remains hidden.
        let found = find_module_file(&src, "backends.llvm.other");
        assert!(
            found.is_none(),
            "walk must stop at project root; got {found:?}"
        );
    }

    // find_module_file: bare (single-segment) name does NOT walk ancestors
    // — Rust 2018 sibling-only semantics.
    #[test]
    fn find_module_file_single_segment_no_ancestor_walk() {
        let dir = tmpdir();
        // Put a `point.mvl` in an ancestor of the entry dir.
        let ancestor_file = dir.path().join("point.mvl");
        fs::write(&ancestor_file, "").unwrap();
        let entry_dir = dir.path().join("nested").join("subdir");
        fs::create_dir_all(&entry_dir).unwrap();

        // Bare "point" import should NOT reach up to the ancestor.
        let found = find_module_file(&entry_dir, "point");
        assert!(
            found.is_none(),
            "single-segment name must not walk ancestors; got {found:?}"
        );
    }

    // ── infer_base_dir_from_qualified_imports ─────────────────────────────────

    // Entry file inside a qualified module tree → walks up to the module root.
    #[test]
    fn infer_base_dir_qualified_entry_finds_ancestor() {
        let dir = tmpdir();
        // Layout:  <root>/compiler/backends/llvm/{emitter,emit_context}.mvl
        //          <root>/mvl.toml
        let llvm_dir = dir.path().join("compiler").join("backends").join("llvm");
        fs::create_dir_all(&llvm_dir).unwrap();
        fs::write(dir.path().join("mvl.toml"), "").unwrap();
        fs::write(llvm_dir.join("emit_context.mvl"), "").unwrap();
        let emitter = llvm_dir.join("emitter.mvl");
        fs::write(&emitter, "use backends.llvm.emit_context::EmitCtx;\n").unwrap();

        let inferred = infer_base_dir_from_qualified_imports(&emitter);
        let expected = dir.path().join("compiler");
        assert_eq!(inferred, expected);
    }

    // Entry file with only bare (single-segment) imports → stays at entry_dir.
    #[test]
    fn infer_base_dir_bare_imports_stays_at_entry_dir() {
        let dir = tmpdir();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let entry = src.join("main.mvl");
        fs::write(&entry, "use helpers::Foo;\n").unwrap();
        fs::write(src.join("helpers.mvl"), "").unwrap();

        let inferred = infer_base_dir_from_qualified_imports(&entry);
        assert_eq!(inferred, src);
    }

    // Entry file with no imports → falls back to entry_dir.
    #[test]
    fn infer_base_dir_no_imports_falls_back_to_entry_dir() {
        let dir = tmpdir();
        let entry = dir.path().join("main.mvl");
        fs::write(&entry, "fn main() -> Unit { }\n").unwrap();

        let inferred = infer_base_dir_from_qualified_imports(&entry);
        assert_eq!(inferred, dir.path());
    }

    // Walk is bounded by mvl.toml — does not escape the project root.
    #[test]
    fn infer_base_dir_bounded_by_project_root() {
        let dir = tmpdir();
        // outer/backends/llvm/other.mvl  ← should NOT be found (outside project)
        // outer/project/mvl.toml
        // outer/project/src/entry.mvl   importing `use backends.llvm.other::X`
        let outer_module = dir.path().join("backends").join("llvm").join("other.mvl");
        fs::create_dir_all(outer_module.parent().unwrap()).unwrap();
        fs::write(&outer_module, "").unwrap();
        let project = dir.path().join("project");
        let src = project.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(project.join("mvl.toml"), "").unwrap();
        let entry = src.join("entry.mvl");
        fs::write(&entry, "use backends.llvm.other::X;\n").unwrap();

        let inferred = infer_base_dir_from_qualified_imports(&entry);
        // Walk stops at mvl.toml in project/; module not found → falls back to src/.
        assert_eq!(inferred, src);
    }

    // ── build_pkg_name_map_with_cache: transitive expansion (#1477) ──────────

    /// Write a minimal `mvl.toml` with just a `[package]` section and optional deps.
    fn write_pkg_manifest(dir: &std::path::Path, name: &str, deps: &[(&str, &str, &str)]) {
        let mut toml = format!(
            "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nlicense = \"Apache-2.0\"\nrequires-mvl = \">=0.1.0\"\n"
        );
        if !deps.is_empty() {
            toml.push_str("\n[dependencies]\n");
            for (dep_id, git_url, tag) in deps {
                toml.push_str(&format!(
                    "\"{dep_id}\" = {{ git = \"{git_url}\", tag = \"{tag}\" }}\n"
                ));
            }
        }
        fs::write(dir.join("mvl.toml"), toml).unwrap();
    }

    /// Write a minimal `mvl.lock` with a single package entry.
    fn write_lockfile(dir: &std::path::Path, name: &str, version: &str) {
        let content = format!(
            "[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nhash = \"sha256:00\"\n"
        );
        fs::write(dir.join("mvl.lock"), content).unwrap();
    }

    #[test]
    fn build_pkg_name_map_includes_direct_dep_from_lockfile() {
        let cache = tmpdir();
        let project = tmpdir();

        // cache/github.com_mvl-lang_pkg-http/1.0.0/ with mvl.toml declaring name = "http"
        let http_dir = cache
            .path()
            .join("github.com_mvl-lang_pkg-http")
            .join("1.0.0");
        fs::create_dir_all(&http_dir).unwrap();
        write_pkg_manifest(&http_dir, "http", &[]);

        // Lock file lists pkg-http directly
        write_lockfile(project.path(), "github.com/mvl-lang/pkg-http", "1.0.0");

        let map = build_pkg_name_map_with_cache(project.path(), cache.path());
        assert!(
            map.contains_key("http"),
            "direct dep from lockfile must be in map; got: {map:?}"
        );
    }

    #[test]
    fn build_pkg_name_map_expands_transitive_dep() {
        let cache = tmpdir();
        let project = tmpdir();

        // pkg-http: name = "http", no deps
        let http_dir = cache
            .path()
            .join("github.com_mvl-lang_pkg-http")
            .join("1.0.0");
        fs::create_dir_all(&http_dir).unwrap();
        write_pkg_manifest(&http_dir, "http", &[]);

        // pkg-health: name = "health", depends on pkg-http @ v1.0.0
        let health_dir = cache
            .path()
            .join("github.com_mvl-lang_pkg-health")
            .join("1.0.0");
        fs::create_dir_all(&health_dir).unwrap();
        write_pkg_manifest(
            &health_dir,
            "health",
            &[(
                "github.com/mvl-lang/pkg-http",
                "https://github.com/mvl-lang/pkg-http",
                "v1.0.0",
            )],
        );

        // Downstream project: lock file only lists pkg-health (NOT pkg-http)
        write_lockfile(project.path(), "github.com/mvl-lang/pkg-health", "1.0.0");

        let map = build_pkg_name_map_with_cache(project.path(), cache.path());
        assert!(
            map.contains_key("health"),
            "direct dep must be in map; got: {map:?}"
        );
        assert!(
            map.contains_key("http"),
            "transitive dep (pkg-http via pkg-health) must be expanded into map; got: {map:?}"
        );
    }

    #[test]
    fn build_pkg_name_map_transitive_dep_absent_from_cache_is_skipped() {
        let cache = tmpdir();
        let project = tmpdir();

        // pkg-health depends on pkg-http, but pkg-http is NOT in the cache
        let health_dir = cache
            .path()
            .join("github.com_mvl-lang_pkg-health")
            .join("1.0.0");
        fs::create_dir_all(&health_dir).unwrap();
        write_pkg_manifest(
            &health_dir,
            "health",
            &[(
                "github.com/mvl-lang/pkg-http",
                "https://github.com/mvl-lang/pkg-http",
                "v1.0.0",
            )],
        );

        write_lockfile(project.path(), "github.com/mvl-lang/pkg-health", "1.0.0");

        let map = build_pkg_name_map_with_cache(project.path(), cache.path());
        assert!(
            map.contains_key("health"),
            "direct dep must be present; got: {map:?}"
        );
        assert!(
            !map.contains_key("http"),
            "absent transitive dep must not appear in map; got: {map:?}"
        );
    }

    // ── strip_cargo_foreign_keys ──────────────────────────────────────────────

    #[test]
    fn strip_cargo_foreign_keys_removes_license_from_inline_table() {
        let input = r#"rusqlite = { version = "0.31", features = ["bundled"], license = "MIT" }"#;
        let output = strip_cargo_foreign_keys(input);
        assert!(
            !output.contains("license"),
            "license key must be stripped; got: {output}"
        );
        assert!(output.contains("version"), "version must be preserved");
        assert!(output.contains("features"), "features must be preserved");
    }

    #[test]
    fn strip_cargo_foreign_keys_plain_version_string_unchanged() {
        let input = r#"serde = "1.0""#;
        assert_eq!(strip_cargo_foreign_keys(input), input);
    }

    #[test]
    fn strip_cargo_foreign_keys_no_license_in_table_unchanged() {
        let input = r#"tokio = { version = "1", features = ["full"] }"#;
        assert_eq!(strip_cargo_foreign_keys(input), input);
    }

    #[test]
    fn extract_native_dep_lines_strips_license() {
        let content = "[native]\nrusqlite = { version = \"0.31\", features = [\"bundled\"], license = \"MIT\" }\n";
        let lines = extract_native_dep_lines(content);
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains("license"),
            "license must not appear in output; got: {}",
            lines[0]
        );
        assert!(lines[0].contains("version"));
        assert!(lines[0].contains("features"));
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
                        // Skip modules already loaded by the implicit prelude
                        // (v1.3.2 added `collections` there; explicit
                        // `use std.collections.{...}` then triggered double
                        // registration and "duplicate method" errors).
                        if IMPLICIT_PRELUDE_STEMS.contains(&module.as_str()) {
                            continue;
                        }
                        if loaded.insert(module.clone()) {
                            let filename = format!("{module}.mvl");
                            let stdlib_file = stdlib_dir.join(&filename);
                            let src_opt = fs::read_to_string(&stdlib_file)
                                .ok()
                                .or_else(|| crate::mvl::stdlib::stdlib_content(&filename));
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
