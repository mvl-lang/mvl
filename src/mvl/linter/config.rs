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
//! | Key               | Default | Description                                     |
//! |-------------------|---------|-------------------------------------------------|
//! | `line_length`     | `120`   | Maximum line length (characters)                |
//! | `indent_size`     | `4`     | Number of spaces per indent level               |
//! | `indent_style`    | `spaces`| `spaces` or `tabs`                              |
//! | `max_fn_length`   | `50`    | Maximum lines in a function body (0 = disabled) |
//! | `naming`          | `true`  | Enforce `snake_case` / `PascalCase` conventions |
//! | `trailing_ws`     | `true`  | Flag trailing whitespace                        |
//! | `unused_bindings` | `true`  | Flag unused `let` bindings                      |

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
            _ => {} // unknown keys are silently ignored (forward-compat)
        }
    }
    Some(cfg)
}

fn parse_bool(s: &str) -> bool {
    matches!(s, "true" | "yes" | "1" | "on")
}
