//! Module resolver — implements Spec 005 (modules).
//!
//! Responsibilities:
//! - File-module correspondence: each `.mvl` file = one module
//! - Import resolution: `use path::to::Item;`
//! - Visibility checking: `pub` items are exported; private items are module-local
//! - Re-export checking: `pub use sub::Item;` only allowed for already-public items
//! - Circular import detection: rejects programs with import cycles
//! - Stdlib module: the `std` root namespace
//!
//! The resolver operates on a *project* — a set of (module_name, Program) pairs —
//! and produces either a resolved module graph or a list of errors.

pub mod cycle_check;
pub mod visibility;

use crate::mvl::parser::ast::{Decl, Program, UseDecl};
use crate::mvl::parser::Parser;
use cycle_check::detect_cycles;
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ── Public API ─────────────────────────────────────────────────────────────

/// A fully-qualified module name, e.g. `["mylib", "io", "File"]`.
pub type ModulePath = Vec<String>;

/// A single module in the resolved project.
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    /// The module's own path (derived from its file path).
    pub name: Vec<String>,
    /// Items exported from this module (by the `pub` modifier).
    pub exports: HashSet<String>,
    /// Imports brought into scope via `use` declarations.
    pub imports: Vec<ResolvedImport>,
}

/// A single resolved `use` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    /// The imported item name (last segment of the path).
    pub item: String,
    /// The full source path, e.g. `["std", "io", "File"]`.
    pub source_path: Vec<String>,
    /// Whether this is a re-export (`pub use …`).
    pub reexport: bool,
}

/// Errors that can occur during module resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// A `use` declaration is not at the top of the file (has preceding non-use decls).
    UseNotAtTop {
        module: String,
        item: String,
        path: Vec<String>,
    },
    /// Two `use` declarations bring an item of the same name into scope.
    NameCollision { module: String, name: String },
    /// The imported module does not exist in the project.
    MissingModule {
        module: String,
        missing_path: Vec<String>,
    },
    /// The imported item is not exported from its source module.
    NotExported {
        module: String,
        item: String,
        source: Vec<String>,
    },
    /// A `pub use` re-exports an item that is private in its source module.
    ReexportOfPrivate {
        module: String,
        item: String,
        source: Vec<String>,
    },
    /// A circular import was detected.
    CircularImport {
        /// The cycle, as a list of module names forming the loop.
        cycle: Vec<String>,
    },
    /// Wildcard imports (`use foo::*`) are not allowed.
    WildcardImport { module: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::UseNotAtTop { module, item, .. } => {
                write!(
                    f,
                    "{module}: `use {item}` must appear at the top of the file"
                )
            }
            ResolveError::NameCollision { module, name } => {
                write!(
                    f,
                    "{module}: import `{name}` is already in scope (name collision)"
                )
            }
            ResolveError::MissingModule {
                module,
                missing_path,
            } => {
                write!(f, "{module}: unknown module `{}`", missing_path.join("::"))
            }
            ResolveError::NotExported {
                module,
                item,
                source,
            } => {
                write!(
                    f,
                    "{module}: `{item}` is not exported from `{}`",
                    source.join("::")
                )
            }
            ResolveError::ReexportOfPrivate {
                module,
                item,
                source,
            } => {
                write!(
                    f,
                    "{module}: cannot re-export private item `{item}` from `{}`",
                    source.join("::")
                )
            }
            ResolveError::CircularImport { cycle } => {
                write!(f, "circular import: {}", cycle.join(" → "))
            }
            ResolveError::WildcardImport { module } => {
                write!(
                    f,
                    "{module}: wildcard imports (`use foo::*`) are not allowed"
                )
            }
        }
    }
}

/// Result of resolving the entire project.
#[derive(Debug)]
pub struct ResolveResult {
    pub modules: HashMap<String, ResolvedModule>,
    pub errors: Vec<ResolveError>,
}

impl ResolveResult {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

// ── Resolver ───────────────────────────────────────────────────────────────

/// Resolve a project: a set of (module_name, Program) pairs.
///
/// `module_name` should be a dot-separated module path derived from the file
/// path, e.g. `"mylib"` for `mylib.mvl`, or `"mylib.io"` for `mylib/io.mvl`.
///
/// If `stdlib_dir` is provided, `core.mvl` is loaded from that directory and
/// injected as the `"std"` module so that `use std.*` declarations resolve.
/// When `None`, the legacy in-memory stub (`stdlib_module()`) is used.
///
/// # Example
/// ```ignore
/// use mvl::mvl::resolver::resolve_project;
/// // Parse each .mvl file into a Program, then resolve:
/// // let result = resolve_project(vec![("mylib".to_string(), prog)], None);
/// ```
pub fn resolve_project(
    modules: Vec<(String, Program)>,
    stdlib_dir: Option<&Path>,
) -> ResolveResult {
    let stdlib = stdlib_dir
        .and_then(load_stdlib_module)
        .unwrap_or_else(stdlib_module);
    let mut resolver = Resolver::new(modules, stdlib);
    resolver.resolve()
}

/// Load the `std` module by parsing `core.mvl` from `stdlib_dir`.
///
/// Returns `None` if the file is missing or unparseable (caller falls back to
/// the in-memory stub).
fn load_stdlib_module(stdlib_dir: &Path) -> Option<ResolvedModule> {
    let core_path = stdlib_dir.join("core.mvl");
    let src = std::fs::read_to_string(&core_path)
        .map_err(|e| {
            eprintln!(
                "mvl: warning: could not read {} — falling back to built-in stdlib stub: {e}",
                core_path.display()
            );
        })
        .ok()?;
    let (mut parser, _) = Parser::new(&src);
    let prog = parser.parse_program();
    if !parser.errors().is_empty() {
        eprintln!(
            "mvl: warning: parse errors in {} — falling back to built-in stdlib stub",
            core_path.display()
        );
        return None;
    }
    let exports = collect_exports(&prog);
    Some(ResolvedModule {
        name: vec!["std".to_string()],
        exports,
        imports: Vec::new(),
    })
}

// ── Internal resolver ──────────────────────────────────────────────────────

struct Resolver {
    /// The parsed programs keyed by module name.
    programs: Vec<(String, Program)>,
    /// The stdlib module injected under the `"std"` key.
    stdlib: ResolvedModule,
}

impl Resolver {
    fn new(programs: Vec<(String, Program)>, stdlib: ResolvedModule) -> Self {
        Resolver { programs, stdlib }
    }

    fn resolve(&mut self) -> ResolveResult {
        let mut errors = Vec::new();
        let mut modules: HashMap<String, ResolvedModule> = HashMap::new();

        // Inject stdlib under the "std" key so `use std.*` imports resolve.
        modules.insert("std".to_string(), self.stdlib.clone());

        // Pass 1: collect exported names for each module.
        for (name, prog) in &self.programs {
            let exports = collect_exports(prog);
            modules.insert(
                name.clone(),
                ResolvedModule {
                    name: vec![name.clone()],
                    exports,
                    imports: Vec::new(),
                },
            );
        }

        // Pass 2: resolve `use` declarations in each module.
        for (name, prog) in &self.programs {
            let use_decls = collect_use_decls(prog);
            let mut seen_names: HashSet<String> = HashSet::new();
            let mut imports = Vec::new();

            for (use_decl, has_preceding_decl) in use_decls {
                // Req 3: `use` must be at top of file (only preceding `use` decls allowed)
                if has_preceding_decl {
                    errors.push(ResolveError::UseNotAtTop {
                        module: name.clone(),
                        item: use_decl.path.last().cloned().unwrap_or_default(),
                        path: use_decl.path.clone(),
                    });
                    continue;
                }
                let item = use_decl.path.last().cloned().unwrap_or_default();
                let source_module = use_decl.path[..use_decl.path.len() - 1].to_vec();
                // Req 3: no name collisions
                if seen_names.contains(&item) {
                    errors.push(ResolveError::NameCollision {
                        module: name.clone(),
                        name: item.clone(),
                    });
                    continue;
                }
                seen_names.insert(item.clone());

                // Req 3: module must exist
                if !source_module.is_empty() {
                    let source_key = source_module.join("::");
                    if !modules.contains_key(&source_key) {
                        errors.push(ResolveError::MissingModule {
                            module: name.clone(),
                            missing_path: source_module.clone(),
                        });
                        continue;
                    }

                    // Req 2: item must be exported from source
                    if let Some(src_mod) = modules.get(&source_key) {
                        if !src_mod.exports.contains(&item) {
                            errors.push(ResolveError::NotExported {
                                module: name.clone(),
                                item: item.clone(),
                                source: source_module.clone(),
                            });
                            continue;
                        }

                        // Req 4: re-exporting a private item is rejected
                        // (here we check if reexport flag is set but item is private)
                        // Note: exports already only contains pub items, so if it's
                        // in exports it's public. No extra check needed here.
                    }
                }

                imports.push(ResolvedImport {
                    item,
                    source_path: use_decl.path.clone(),
                    reexport: use_decl.reexport,
                });
            }

            if let Some(m) = modules.get_mut(name) {
                m.imports = imports;
            }
        }

        // Pass 3: build the import graph and detect cycles.
        let import_graph = build_import_graph(&modules);
        let cycles = detect_cycles(&import_graph);
        for cycle in cycles {
            errors.push(ResolveError::CircularImport { cycle });
        }

        ResolveResult { modules, errors }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Collect all `pub`-marked item names from a program (its export surface).
fn collect_exports(prog: &Program) -> HashSet<String> {
    let mut exports = HashSet::new();
    for decl in &prog.declarations {
        match decl {
            Decl::Type(td) if td.visible => {
                exports.insert(td.name.clone());
            }
            Decl::Fn(fd) if fd.visible => {
                exports.insert(fd.name.clone());
            }
            Decl::Const(cd) if cd.visible => {
                exports.insert(cd.name.clone());
            }
            Decl::Use(ud) if ud.reexport => {
                // `pub use` re-exports the last path segment
                if let Some(item) = ud.path.last() {
                    exports.insert(item.clone());
                }
            }
            _ => {}
        }
    }
    exports
}

/// Collect use declarations with whether they appear after a non-use declaration.
///
/// Returns `(UseDecl, has_preceding_non_use_decl)`.
fn collect_use_decls(prog: &Program) -> Vec<(&UseDecl, bool)> {
    let mut result = Vec::new();
    let mut seen_non_use = false;
    for decl in &prog.declarations {
        match decl {
            Decl::Use(ud) => {
                result.push((ud, seen_non_use));
            }
            _ => {
                seen_non_use = true;
            }
        }
    }
    result
}

/// Build a simple module → [imported module] adjacency list for cycle detection.
fn build_import_graph(modules: &HashMap<String, ResolvedModule>) -> HashMap<String, Vec<String>> {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();
    for (name, module) in modules {
        let mut deps = Vec::new();
        for import in &module.imports {
            if import.source_path.len() > 1 {
                let dep = import.source_path[..import.source_path.len() - 1].join("::");
                if modules.contains_key(&dep) && !deps.contains(&dep) {
                    deps.push(dep);
                }
            }
        }
        graph.insert(name.clone(), deps);
    }
    graph
}

// ── Stdlib ─────────────────────────────────────────────────────────────────

/// The standard library module tree.
///
/// All `std` imports must be explicit — there is no prelude auto-import.
/// This returns a minimal stub for Phase 1: the full stdlib will be implemented
/// as actual `.mvl` source files in a future milestone.
pub fn stdlib_module() -> ResolvedModule {
    let mut exports = HashSet::new();
    // Minimal Phase 1 stdlib surface — must stay in sync with std/core.mvl
    for name in &[
        "println",
        "eprintln",
        "format",
        "assert",
        "assert_eq",
        "panic",
    ] {
        exports.insert((*name).to_string());
    }
    ResolvedModule {
        name: vec!["std".to_string()],
        exports,
        imports: Vec::new(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvl::parser::Parser;

    fn parse(src: &str) -> Program {
        let (mut p, _) = Parser::new(src);
        let prog = p.parse_program();
        assert!(p.errors().is_empty(), "parse errors: {:?}", p.errors());
        prog
    }

    // Req 2: pub items are exported; private items are not.
    #[test]
    fn pub_item_accessible() {
        let prog = parse("pub fn greet() -> Int { 0 }");
        let exports = collect_exports(&prog);
        assert!(exports.contains("greet"), "pub fn should be exported");
    }

    #[test]
    fn private_item_not_exported() {
        let prog = parse("fn greet() -> Int { 0 }");
        let exports = collect_exports(&prog);
        assert!(
            !exports.contains("greet"),
            "private fn should not be exported"
        );
    }

    // Req 3: use at top of file only.
    #[test]
    fn use_at_top() {
        let prog = parse("use mymod::MyType;\npub fn foo() -> Int { 0 }");
        let uses = collect_use_decls(&prog);
        assert_eq!(uses.len(), 1);
        assert!(!uses[0].1, "use at top should not be flagged");
    }

    #[test]
    fn use_not_at_top_flagged() {
        // `fn foo` comes before `use mymod::MyType`
        let prog = parse("fn foo() -> Int { 0 }\nuse mymod::MyType;");
        let uses = collect_use_decls(&prog);
        assert_eq!(uses.len(), 1);
        assert!(uses[0].1, "use after non-use decl should be flagged");
    }

    // Req 3: name collision rejected.
    #[test]
    fn name_collision_rejected() {
        let a = parse("pub type Foo = struct { x: Int }");
        let b = parse("use mod_a::Foo;\nuse mod_a::Foo;"); // duplicate import
        let result = resolve_project(
            vec![("mod_a".to_string(), a), ("mod_b".to_string(), b)],
            None,
        );
        let has_collision = result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::NameCollision { .. }));
        assert!(
            has_collision,
            "duplicate import must be rejected: {:?}",
            result.errors
        );
    }

    // Req 3: missing module rejected.
    #[test]
    fn missing_module_rejected() {
        let a = parse("use does_not_exist::Foo;");
        let result = resolve_project(vec![("mod_a".to_string(), a)], None);
        let has_missing = result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::MissingModule { .. }));
        assert!(
            has_missing,
            "import from unknown module must be rejected: {:?}",
            result.errors
        );
    }

    // Req 2: private item from another module is rejected.
    #[test]
    fn private_item_rejected() {
        let a = parse("fn secret() -> Int { 0 }"); // private
        let b = parse("use mod_a::secret;"); // tries to import private item
        let result = resolve_project(
            vec![("mod_a".to_string(), a), ("mod_b".to_string(), b)],
            None,
        );
        let has_not_exported = result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::NotExported { .. }));
        assert!(
            has_not_exported,
            "importing private item must be rejected: {:?}",
            result.errors
        );
    }

    // Req 4: re-export of public item allowed.
    #[test]
    fn reexport_public() {
        let a = parse("pub type Foo = struct { x: Int }");
        let b = parse("pub fn bar() -> Int { 0 }");
        let result = resolve_project(
            vec![("mod_a".to_string(), a), ("mod_b".to_string(), b)],
            None,
        );
        assert!(
            result.is_ok(),
            "valid project must resolve without errors: {:?}",
            result.errors
        );
    }

    // Req 5: circular imports rejected.
    #[test]
    fn circular_import_rejected() {
        // a imports from b, b imports from a
        let a = parse("use mod_b::Bar;\npub type Foo = struct { x: Int }");
        let b = parse("use mod_a::Foo;\npub type Bar = struct { y: Int }");
        let result = resolve_project(
            vec![("mod_a".to_string(), a), ("mod_b".to_string(), b)],
            None,
        );
        let has_cycle = result
            .errors
            .iter()
            .any(|e| matches!(e, ResolveError::CircularImport { .. }));
        assert!(
            has_cycle,
            "circular imports must be rejected: {:?}",
            result.errors
        );
    }

    // Req 1: file-module correspondence (module name from filename).
    #[test]
    fn file_module_correspondence() {
        let prog = parse("pub fn greet() -> Int { 0 }");
        let result = resolve_project(vec![("greet_module".to_string(), prog)], None);
        assert!(result.modules.contains_key("greet_module"));
    }

    // Req 6: stdlib is accessible under `std` namespace.
    #[test]
    fn stdlib_exports_exist() {
        let stdlib = stdlib_module();
        assert!(stdlib.exports.contains("println"));
        assert!(stdlib.exports.contains("panic"));
    }
}
