// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

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
//! ### Style toggle
//!
//! | Key     | Default | Description                                                              |
//! |---------|---------|--------------------------------------------------------------------------|
//! | `style` | `false` | Master toggle: enables all style rules with standard values.             |
//!
//! Setting `style = true` enables: `line_length` (120), `trailing_ws`, `indentation`,
//! `final_newline`, and `consistent_comment_style`. Individual keys override the toggle.
//!
//! ### Phase 1 — style rules (OFF by default)
//!
//! | Key               | Default | Description                                               |
//! |-------------------|---------|-----------------------------------------------------------|
//! | `line_length`     | `0`     | Maximum line length (characters; 0 = disabled)            |
//! | `indent_size`     | `4`     | Number of spaces per indent level (used when enabled)     |
//! | `indent_style`    | `spaces`| `spaces` or `tabs` (used when indentation is enabled)    |
//! | `indentation`     | `false` | Flag wrong indent style/width                             |
//! | `final_newline`   | `false` | Require file to end with exactly one newline              |
//! | `max_fn_length`   | `100`   | Maximum lines in a function body (0 = disabled)           |
//! | `naming`          | `true`  | Enforce `snake_case` / `PascalCase` conventions           |
//! | `trailing_ws`     | `false` | Flag trailing whitespace                                  |
//! | `unused_bindings` | `true`  | Flag unused `let` bindings (future)                       |
//!
//! ### Phase 2 — semantic rules
//!
//! | Key                    | Default | Description                                          |
//! |------------------------|---------|------------------------------------------------------|
//! | `unreachable_code`     | `true`  | Flag statements after `return` in a block            |
//! | `redundant_match`      | `true`  | Flag `match` with a single irrefutable arm           |
//! | `suggest_if_let`       | `true`  | Suggest `if let` for 2-arm match with `_ => ()` fallback |
//! | `redundant_effects`    | `true`  | Flag effect declarations on call-free functions      |
//! | `redundant_ifc_labels` | `true`  | Flag `Public<T>` annotations (redundant base label)  |
//! | `missing_annotations`       | `false` | Warn on functions with calls but no effect annotation (opt-in) |
//! | `require_explicit_totality` | `false` | Warn on non-test fns missing explicit `total`/`partial` keyword (opt-in) |
//! | `for_iter_antipattern`      | `true`  | Error on `while/.get(i)/match/None=>()` list-iteration anti-pattern (#705) |
//! | `deprecated_extern_rust`    | `true`  | Warn on `extern "rust"` — deprecated; use `extern "C"` instead (#561)      |
//! | `unused_functions`          | `true`  | Flag non-pub, non-main functions with no call sites (#1373)                 |
//! | `silent_result_discard`     | `true`  | Flag silently-discarded `Result` values (#1465)                             |
//! | `relabel_tag_hygiene`       | `true`  | Flag reused or boilerplate audit tags on `relabel` expressions (#1466)      |
//!
//! ### Phase 3 — LLM corpus quality rules
//!
//! | Key                         | Default | Description                                              |
//! |-----------------------------|---------|----------------------------------------------------------|
//! | `consistent_comment_style`  | `false` | Flag block comments `/* */` (only `//` allowed)          |
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
//! | `min_fns_for_extern_ratio`   | `10`    | Min total fns before extern-ratio check fires (0 = always)         |
//! | `composition_root_depth`     | `2`     | Call-graph hops from `main` exempt from effect-width (binary only) |

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
    /// Whether indentation style/size rules are active.
    pub indentation: bool,
    /// Whether the final-newline rule is active.
    pub final_newline: bool,
    /// Whether unused-binding rule is active.
    pub unused_bindings: bool,

    // ── Phase 2: semantic rules ───────────────────────────────────────────
    /// Flag statements that follow a `return` in the same block.
    pub unreachable_code: bool,
    /// Flag `match` expressions/statements with a single irrefutable arm.
    pub redundant_match: bool,
    /// Suggest `if let` when a 2-arm `match` has a variant pattern and a `_ => ()`
    /// fallback — the `if let` form is more concise and expresses "only care
    /// about this variant" more directly (#1776).
    pub suggest_if_let: bool,
    /// Flag functions that declare effects but contain no function calls.
    pub redundant_effects: bool,
    /// Flag `Public<T>` type annotations (the base IFC label, always redundant).
    pub redundant_ifc_labels: bool,
    /// Warn on functions that have calls but no declared effects (opt-in; default off).
    /// Enable with `missing_annotations = true` in `.mvllintrc`.
    pub missing_annotations: bool,
    /// Warn on unannotated `pub fn` with no explicit `total` or `partial` keyword (on by default).
    /// Disable with `require_explicit_totality = false` in `.mvllintrc`.
    pub require_explicit_totality: bool,
    /// Error on `while / .get(i) / match / None => ()` iteration anti-pattern.
    /// Always use `for x in list { }` instead (rule `for-iter-antipattern`, #705).
    pub for_iter_antipattern: bool,
    /// Warn on zero-arg pure functions whose body is a single literal — these
    /// should be `const` declarations so the solver can inline them at every
    /// use site (rule `zero_arg_literal_fn_as_const`, #1805).
    pub zero_arg_literal_fn_as_const: bool,
    /// Warn on `while VAR < END { ...; VAR = VAR + N }` counter loops that can be
    /// rewritten as `for VAR in range(START, END)` (provably total, #1004).
    pub while_to_for_range: bool,
    /// Warn on `extern "rust"` blocks — deprecated in favour of `extern "C"` (#561).
    pub deprecated_extern_rust: bool,
    /// Flag non-pub, non-main functions that are never called within the program (#1373).
    pub unused_functions: bool,
    /// Flag `Result` values that are silently discarded without inspecting the `Err` (#1465).
    pub silent_result_discard: bool,
    /// Flag reused or boilerplate audit tags on `relabel trust` / `relabel classify` (#1466).
    pub relabel_tag_hygiene: bool,

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
    /// Minimum total fn count before `max_extern_ratio` fires. `0` disables the guard.
    pub min_fns_for_extern_ratio: usize,
    /// Max call-graph hops from `fn main` for `complexity-effect-width` exemption.
    /// Binary crates only; `0` exempts only `main` itself. Default: 2.
    pub composition_root_depth: usize,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            // Style rules — OFF by default; enable with `style = true` in .mvllintrc
            line_length: 0, // 0 = disabled; `style = true` sets this to 120
            indent_size: 4,
            indent_spaces: true,
            trailing_ws: false,
            indentation: false,
            final_newline: false,
            // Semantic / complexity rules — ON by default
            max_fn_length: 100,
            naming: true,
            unused_bindings: true,
            unreachable_code: true,
            redundant_match: true,
            suggest_if_let: true,
            redundant_effects: true,
            redundant_ifc_labels: true,
            missing_annotations: false,
            require_explicit_totality: true,
            for_iter_antipattern: true,
            zero_arg_literal_fn_as_const: true,
            while_to_for_range: true,
            deprecated_extern_rust: true,
            unused_functions: true,
            silent_result_discard: true,
            relabel_tag_hygiene: true,
            consistent_comment_style: false,
            require_doc_comments: true,
            doc_comment_examples: false,
            max_cyclomatic_complexity: 10,
            max_nested_match_depth: 3,
            max_effect_signature_width: 3,
            max_trait_impl_count: 5,
            max_module_fanout: 15,
            max_extern_ratio: 0.2,
            min_fns_for_extern_ratio: 10,
            composition_root_depth: 2,
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

    // Collect all key=value pairs (skip blank lines and comments).
    let pairs: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
        })
        .collect();

    // Phase 1: start from defaults, then apply the `style` master toggle if present.
    // This ensures individual key overrides (phase 2) always win regardless of file order.
    let mut cfg = LintConfig::default();
    if pairs.iter().any(|(k, v)| *k == "style" && parse_bool(v)) {
        cfg.line_length = 120;
        cfg.trailing_ws = true;
        cfg.indentation = true;
        cfg.final_newline = true;
        cfg.consistent_comment_style = true;
    }

    // Phase 2: apply individual key overrides.
    for (key, val) in pairs {
        match key {
            "style" => {} // already handled in phase 1
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
            "indentation" => cfg.indentation = parse_bool(val),
            "final_newline" => cfg.final_newline = parse_bool(val),
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
            "suggest_if_let" => cfg.suggest_if_let = parse_bool(val),
            "redundant_effects" => cfg.redundant_effects = parse_bool(val),
            "redundant_ifc_labels" => cfg.redundant_ifc_labels = parse_bool(val),
            "missing_annotations" => cfg.missing_annotations = parse_bool(val),
            "require_explicit_totality" => cfg.require_explicit_totality = parse_bool(val),
            "for_iter_antipattern" => cfg.for_iter_antipattern = parse_bool(val),
            "zero_arg_literal_fn_as_const" => cfg.zero_arg_literal_fn_as_const = parse_bool(val),
            "while_to_for_range" => cfg.while_to_for_range = parse_bool(val),
            "deprecated_extern_rust" => cfg.deprecated_extern_rust = parse_bool(val),
            "unused_functions" => cfg.unused_functions = parse_bool(val),
            "silent_result_discard" => cfg.silent_result_discard = parse_bool(val),
            "relabel_tag_hygiene" => cfg.relabel_tag_hygiene = parse_bool(val),
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
            "min_fns_for_extern_ratio" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.min_fns_for_extern_ratio = n;
                }
            }
            "composition_root_depth" => {
                if let Ok(n) = val.parse::<usize>() {
                    cfg.composition_root_depth = n;
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

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── parse_bool ────────────────────────────────────────────────────────

    #[test]
    fn parse_bool_truthy_values() {
        for val in &["true", "yes", "1", "on"] {
            assert!(parse_bool(val), "expected true for '{val}'");
        }
    }

    #[test]
    fn parse_bool_falsy_values() {
        for val in &["false", "no", "0", "off", "", "TRUE", "YES"] {
            assert!(!parse_bool(val), "expected false for '{val}'");
        }
    }

    // ── LintConfig::default ───────────────────────────────────────────────

    #[test]
    fn default_config_has_expected_values() {
        let cfg = LintConfig::default();
        // Style rules — OFF by default
        assert_eq!(cfg.line_length, 0);
        assert!(!cfg.trailing_ws);
        assert!(!cfg.indentation);
        assert!(!cfg.final_newline);
        assert!(!cfg.consistent_comment_style);
        // Style parameters kept for when style is enabled
        assert_eq!(cfg.indent_size, 4);
        assert!(cfg.indent_spaces);
        // Semantic / complexity rules — ON by default
        assert_eq!(cfg.max_fn_length, 100);
        assert!(cfg.naming);
        assert!(cfg.unreachable_code);
        assert!(cfg.redundant_match);
        assert_eq!(cfg.max_cyclomatic_complexity, 10);
        assert_eq!(cfg.max_nested_match_depth, 3);
    }

    // ── load_from via temp file ───────────────────────────────────────────

    fn write_temp_config(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        write!(f, "{content}").expect("write");
        f
    }

    #[test]
    fn load_from_parses_line_length() {
        let f = write_temp_config("line_length = 80\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 80);
    }

    #[test]
    fn load_from_parses_indent_style_tabs() {
        let f = write_temp_config("indent_style = tabs\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert!(!cfg.indent_spaces);
    }

    #[test]
    fn load_from_parses_indent_style_spaces() {
        let f = write_temp_config("indent_style = spaces\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert!(cfg.indent_spaces);
    }

    #[test]
    fn load_from_parses_bool_flags() {
        let content = "trailing_ws = false\nnaming = false\nunreachable_code = false\n";
        let f = write_temp_config(content);
        let cfg = load_from(f.path()).expect("load_from");
        assert!(!cfg.trailing_ws);
        assert!(!cfg.naming);
        assert!(!cfg.unreachable_code);
    }

    #[test]
    fn load_from_parses_complexity_limits() {
        let content = "max_cyclomatic_complexity = 5\nmax_nested_match_depth = 2\nmax_effect_signature_width = 1\nmax_trait_impl_count = 3\nmax_module_fanout = 8\nmax_extern_ratio = 0.1\n";
        let f = write_temp_config(content);
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.max_cyclomatic_complexity, 5);
        assert_eq!(cfg.max_nested_match_depth, 2);
        assert_eq!(cfg.max_effect_signature_width, 1);
        assert_eq!(cfg.max_trait_impl_count, 3);
        assert_eq!(cfg.max_module_fanout, 8);
        assert!((cfg.max_extern_ratio - 0.1).abs() < 1e-9);
    }

    #[test]
    fn load_from_ignores_comments_and_blank_lines() {
        let content = "# This is a comment\n\nline_length = 100\n# another comment\n";
        let f = write_temp_config(content);
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 100);
    }

    #[test]
    fn load_from_ignores_unknown_keys() {
        let content = "line_length = 90\nunknown_future_key = somevalue\n";
        let f = write_temp_config(content);
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 90);
    }

    #[test]
    fn load_from_returns_none_for_missing_file() {
        let result = load_from(Path::new("/nonexistent/path/.mvllintrc"));
        assert!(result.is_none());
    }

    #[test]
    fn load_from_parses_final_newline_and_indentation() {
        let f = write_temp_config("final_newline = true\nindentation = true\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert!(cfg.final_newline);
        assert!(cfg.indentation);
    }

    #[test]
    fn style_toggle_enables_all_style_rules() {
        let f = write_temp_config("style = true\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 120);
        assert!(cfg.trailing_ws);
        assert!(cfg.indentation);
        assert!(cfg.final_newline);
        assert!(cfg.consistent_comment_style);
        // Semantic rules unaffected
        assert!(cfg.unreachable_code);
        assert!(cfg.redundant_match);
    }

    #[test]
    fn style_toggle_individual_override_wins() {
        // Individual keys set after style toggle must win regardless of file order
        let f = write_temp_config("style = true\nline_length = 80\ntrailing_ws = false\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 80);
        assert!(!cfg.trailing_ws);
        // Other style rules still enabled by the toggle
        assert!(cfg.indentation);
        assert!(cfg.final_newline);
    }

    #[test]
    fn style_toggle_individual_override_wins_regardless_of_order() {
        // Individual key appears before `style = true` in the file — still wins
        let f = write_temp_config("line_length = 60\nstyle = true\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.line_length, 60);
        assert!(cfg.trailing_ws); // style toggle still applied
    }

    #[test]
    fn load_from_rejects_out_of_range_extern_ratio() {
        let default_ratio = LintConfig::default().max_extern_ratio;
        let f = write_temp_config("max_extern_ratio = 2.5\n");
        let cfg = load_from(f.path()).expect("load_from");
        // Out-of-range value should not be applied — keeps default
        assert!((cfg.max_extern_ratio - default_ratio).abs() < 1e-9);
    }

    // ── LintConfig::load ──────────────────────────────────────────────────

    #[test]
    fn load_returns_default_when_no_file_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cfg = LintConfig::load(dir.path());
        assert_eq!(cfg, LintConfig::default());
    }

    #[test]
    fn load_reads_local_mvllintrc() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rc_path = dir.path().join(".mvllintrc");
        std::fs::write(&rc_path, "line_length = 60\n").expect("write");
        let cfg = LintConfig::load(dir.path());
        assert_eq!(cfg.line_length, 60);
    }

    // ── LintConfig::config_file ───────────────────────────────────────────

    #[test]
    fn config_file_returns_none_when_no_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(LintConfig::config_file(dir.path()).is_none());
    }

    #[test]
    fn config_file_returns_local_path_when_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rc_path = dir.path().join(".mvllintrc");
        std::fs::write(&rc_path, "# empty\n").expect("write");
        let found = LintConfig::config_file(dir.path());
        assert_eq!(found, Some(rc_path));
    }

    #[test]
    fn load_from_parses_composition_root_depth() {
        let f = write_temp_config("composition_root_depth = 5\n");
        let cfg = load_from(f.path()).expect("load_from");
        assert_eq!(cfg.composition_root_depth, 5);
    }

    #[test]
    fn default_composition_root_depth_is_two() {
        assert_eq!(LintConfig::default().composition_root_depth, 2);
    }
}
