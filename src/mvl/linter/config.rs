//! Linter configuration — loaded from `.mvllintrc` or `~/.config/mvl/lintrc`.
//!
//! Resolution order (first file found wins):
//!   1. `.mvllintrc` in the current working directory (project-local)
//!   2. `$XDG_CONFIG_HOME/mvl/lintrc`  (defaults to `~/.config/mvl/lintrc`)
//!
//! Format: simple `key = value` pairs, one per line.
//! Lines starting with `#` are comments; blank lines are ignored.
//!
//! ## Supported keys
//!
//! ### Phase 1 — style rules
//!
//! | Key               | Default | Description                                     |
//! |-------------------|---------|-------------------------------------------------|
//! | `line_length`     | `120`   | Maximum line length (characters)                |
//! | `indent_size`     | `4`     | Number of spaces per indent level               |
//! | `indent_style`    | `spaces`| `spaces` or `tabs`                              |
//! | `max_fn_length`   | `50`    | Maximum lines in a function body (0 = disabled) |
//! | `naming`          | `true`  | Enforce `snake_case` / `PascalCase` conventions |
//! | `trailing_ws`     | `true`  | Flag trailing whitespace                        |
//! | `unused_bindings` | `true`  | Flag unused `let` bindings (future)             |
//!
//! ### Phase 2 — semantic rules
//!
//! | Key                    | Default | Description                                          |
//! |------------------------|---------|------------------------------------------------------|
//! | `unreachable_code`     | `true`  | Flag statements after `return` in a block            |
//! | `redundant_match`      | `true`  | Flag `match` with a single irrefutable arm           |
//! | `unnecessary_annotations` | `true` | Flag literal `let` bindings with obvious types    |
//! | `redundant_effects`    | `true`  | Flag effect declarations on call-free functions      |
//! | `redundant_ifc_labels` | `true`  | Flag `Public<T>` annotations (redundant base label)  |
//!
//! ### Phase 3 — LLM corpus quality rules
//!
//! | Key                         | Default | Description                                              |
//! |-----------------------------|---------|----------------------------------------------------------|
//! | `consistent_comment_style`  | `true`  | Flag block comments `/* */` (only `//` allowed)          |
//! | `require_doc_comments`      | `true`  | Require `///` doc comments on public functions and types |
//! | `doc_comment_examples`      | `false` | Recommend `Example:` blocks in doc comments (warning)   |
//!
//! ### Phase 4 — Complexity rules (regenerability metrics)
//!
//! | Key                          | Default | Description                                                        |
//! |------------------------------|---------|--------------------------------------------------------------------|
//! | `max_cyclomatic_complexity`  | `10`    | Max cyclomatic complexity per function (0 = disabled)              |
//! | `max_nested_match_depth`     | `3`     | Max nesting depth of `match` expressions (0 = disabled)            |
//! | `max_effect_signature_width` | `3`     | Max number of declared effects per function (0 = disabled)         |
//! | `max_trait_impl_count`       | `5`     | Max number of trait impls per type (0 = disabled)                  |
//! | `max_module_fanout`          | `15`    | Max number of distinct modules imported (0 = disabled)             |
//! | `max_extern_ratio`           | `0.2`   | Max ratio of extern fns to total fns (0.0 = disabled)              |

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Resolved linter configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct LintConfig {
    /// Maximum allowed line length in characters.
    pub line_length: usize,
    /// Number of spaces per indentation level.
    pub indent_size: usize,
    /// Whether indentation must use spaces (`true`) or tabs (`false`).
    pub indent_spaces: bool,
    /// Maximum function body length in lines. `0` disables the check.
    pub max_fn_length: usize,
    /// Whether naming-convention rules are active.
    pub naming: bool,
    /// Whether trailing-whitespace rule is active.
    pub trailing_ws: bool,
    /// Whether unused-binding rule is active.
    pub unused_bindings: bool,

    // ── Phase 2: semantic rules ───────────────────────────────────────────
    /// Flag statements that follow a `return` in the same block.
    pub unreachable_code: bool,
    /// Flag `match` expressions/statements with a single irrefutable arm.
    pub redundant_match: bool,
    /// Flag `let` bindings that annotate a literal with its obvious type.
    pub unnecessary_annotations: bool,
    /// Flag functions that declare effects but contain no function calls.
    pub redundant_effects: bool,
    /// Flag `Public<T>` type annotations (the base IFC label, always redundant).
    pub redundant_ifc_labels: bool,

    // ── Phase 3: LLM corpus quality rules ────────────────────────────────
    /// Flag block comments `/* */`; only `//` line comments are allowed.
    pub consistent_comment_style: bool,
    /// Require `///` doc comments on public functions and types.
    pub require_doc_comments: bool,
    /// Recommend an `Example:` block inside doc comments (warning, opt-in).
    pub doc_comment_examples: bool,

    // ── Phase 4: Complexity rules (regenerability metrics) ───────────────
    /// Maximum cyclomatic complexity per function. `0` disables the check.
    pub max_cyclomatic_complexity: usize,
    /// Maximum nested `match` depth per function. `0` disables the check.
    pub max_nested_match_depth: usize,
    /// Maximum number of declared effects per function. `0` disables the check.
    pub max_effect_signature_width: usize,
    /// Maximum number of trait `impl` blocks per type. `0` disables the check.
    pub max_trait_impl_count: usize,
    /// Maximum number of distinct modules imported per file. `0` disables the check.
    pub max_module_fanout: usize,
    /// Maximum ratio of extern fns to total fns (0.0..=1.0). `0.0` disables the check.
    pub max_extern_ratio: f64,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            line_length: 120,
            indent_size: 4,
            indent_spaces: true,
            max_fn_length: 50,
            naming: true,
            trailing_ws: true,
            unused_bindings: true,
            unreachable_code: true,
            redundant_match: true,
            unnecessary_annotations: true,
            redundant_effects: true,
            redundant_ifc_labels: true,
            consistent_comment_style: true,
            require_doc_comments: true,
            doc_comment_examples: false,
            max_cyclomatic_complexity: 10,
            max_nested_match_depth: 3,
            max_effect_signature_width: 3,
            max_trait_impl_count: 5,
            max_module_fanout: 15,
            max_extern_ratio: 0.2,
        }
    }
}

impl LintConfig {
    /// Load config, searching local then XDG global.
    ///
    /// Returns the default config if no file is found.
    pub fn load(project_root: &Path) -> Self {
        if let Some(path) = local_path(project_root) {
            if let Some(cfg) = load_from(&path) {
                return cfg;
            }
        }
        if let Some(path) = xdg_path() {
            if let Some(cfg) = load_from(&path) {
                return cfg;
            }
        }
        Self::default()
    }

    /// Which config file was found (for diagnostics / `--show-config`).
    pub fn config_file(project_root: &Path) -> Option<PathBuf> {
        if let Some(p) = local_path(project_root) {
            if p.exists() {
                return Some(p);
            }
        }
        if let Some(p) = xdg_path() {
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

// ── Path resolution ────────────────────────────────────────────────────────

fn local_path(project_root: &Path) -> Option<PathBuf> {
    Some(project_root.join(".mvllintrc"))
}

fn xdg_path() -> Option<PathBuf> {
    let base = env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".config")))?;
    Some(base.join("mvl").join("lintrc"))
}

/// Minimal home-dir lookup without external crates.
fn dirs_home() -> Option<PathBuf> {
    env::var("HOME").ok().map(PathBuf::from)
}

// ── Parser ─────────────────────────────────────────────────────────────────

fn load_from(path: &Path) -> Option<LintConfig> {
    let text = fs::read_to_string(path).ok()?;
    let mut cfg = LintConfig::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        match key {
            "line_length" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.line_length = n;
                }
            }
            "indent_size" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.indent_size = n;
                }
            }
            "indent_style" => match val {
                "spaces" | "space" => cfg.indent_spaces = true,
                "tabs" | "tab" => cfg.indent_spaces = false,
                _ => {}
            },
            "max_fn_length" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_fn_length = n;
                }
            }
            "naming" => cfg.naming = parse_bool(val),
            "trailing_ws" => cfg.trailing_ws = parse_bool(val),
            "unused_bindings" => cfg.unused_bindings = parse_bool(val),
            "unreachable_code" => cfg.unreachable_code = parse_bool(val),
            "redundant_match" => cfg.redundant_match = parse_bool(val),
            "unnecessary_annotations" => cfg.unnecessary_annotations = parse_bool(val),
            "redundant_effects" => cfg.redundant_effects = parse_bool(val),
            "redundant_ifc_labels" => cfg.redundant_ifc_labels = parse_bool(val),
            "consistent_comment_style" => cfg.consistent_comment_style = parse_bool(val),
            "require_doc_comments" => cfg.require_doc_comments = parse_bool(val),
            "doc_comment_examples" => cfg.doc_comment_examples = parse_bool(val),
            "max_cyclomatic_complexity" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_cyclomatic_complexity = n;
                }
            }
            "max_nested_match_depth" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_nested_match_depth = n;
                }
            }
            "max_effect_signature_width" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_effect_signature_width = n;
                }
            }
            "max_trait_impl_count" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_trait_impl_count = n;
                }
            }
            "max_module_fanout" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.max_module_fanout = n;
                }
            }
            "max_extern_ratio" => {
                if let Ok(f) = val.parse::<f64>() {
                    // Accept only finite values in [0.0, 1.0]; NaN or out-of-range
                    // would silently disable the rule without the user setting 0.0.
                    if f.is_finite() && (0.0..=1.0).contains(&f) {
                        cfg.max_extern_ratio = f;
                    }
                }
            }
            _ => {} // unknown keys are silently ignored (forward-compat)
        }
    }
    Some(cfg)
}

fn parse_bool(s: &str) -> bool {
    matches!(s, "true" | "yes" | "1" | "on")
}
