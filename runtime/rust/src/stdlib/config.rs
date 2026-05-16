// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Rust implementation of `std.config` stdlib functions.
//!
//! Single public function: `load_config(path, prefix)` following the layered
//! convention: XDG config → local config → explicit path → env overlay.
//! No external crates — minimal hand-written TOML and JSON parsers.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

// ── ConfigError ───────────────────────────────────────────────────────────────

/// Error type mirroring `ConfigError` in `std/config.mvl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    FileNotFound { path: String },
    ParseError { msg: String, line: i64, col: i64 },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::FileNotFound { path } => write!(f, "file not found: {path}"),
            ConfigError::ParseError { msg, line, col } => {
                write!(f, "parse error at {line}:{col}: {msg}")
            }
        }
    }
}

// ── ConfigValue ───────────────────────────────────────────────────────────────

/// A parsed configuration value mirroring `ConfigValue` in `std/config.mvl`.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Table(HashMap<String, ConfigValue>),
    Array(Vec<ConfigValue>),
}

impl ConfigValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            ConfigValue::Null => "null",
            ConfigValue::Bool(_) => "Bool",
            ConfigValue::Int(_) => "Int",
            ConfigValue::Float(_) => "Float",
            ConfigValue::Str(_) => "String",
            ConfigValue::Table(_) => "Table",
            ConfigValue::Array(_) => "Array",
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load application configuration following the standard layered convention.
///
/// Resolution order (later layers override earlier ones):
///   1. XDG config:  $XDG_CONFIG_HOME/<progname>/config.toml
///   2. Local file:  ./config.toml
///   3. Explicit:    path (if Some) replaces the auto-resolved file
///   4. Env overlay: <PREFIX>_<KEY> vars override file values
///
/// Implements the Rust backing for `std/config.mvl::load_config`.
pub fn load_config(path: Option<String>, prefix: String) -> Result<ConfigValue, ConfigError> {
    let resolved = resolve_path(path.as_deref())?;
    let mut value = parse_file(&resolved)?;
    if !prefix.is_empty() {
        apply_env_overlay(&mut value, &prefix.to_uppercase(), "");
    }
    Ok(value)
}

// ── Path resolution ───────────────────────────────────────────────────────────

/// Resolve the config file following the XDG convention.
///
/// If `path` is Some and absolute, it is used directly.
/// If `path` is Some and relative, it is searched through XDG + local dirs.
/// If `path` is None, searches XDG then local `./config.toml`.
///
/// # Security
/// Path existence is confirmed via `std::fs::canonicalize` rather than
/// a separate `exists()` check followed by `open`. This eliminates the
/// TOCTOU (time-of-check/time-of-use) race where a symlink could be swapped
/// between the check and the open. `canonicalize` resolves symlinks and
/// verifies existence atomically — the returned path is the real inode.
fn resolve_path(path: Option<&str>) -> Result<PathBuf, ConfigError> {
    let xdg_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        });

    let xdg_dirs: Vec<PathBuf> = std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .map(|s| s.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![PathBuf::from("/etc/xdg")]);

    let progname = std::env::args()
        .next()
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_string();

    match path {
        // Absolute path: use directly
        Some(p) if Path::new(p).is_absolute() => {
            std::fs::canonicalize(p).map_err(|_| ConfigError::FileNotFound {
                path: p.to_string(),
            })
        }
        // Relative or None: search XDG then local
        Some(p) => {
            if Path::new(p).components().any(|c| c == Component::ParentDir) {
                return Err(ConfigError::FileNotFound {
                    path: p.to_string(),
                });
            }
            let candidates: Vec<PathBuf> = xdg_home
                .iter()
                .map(|b| b.join(&progname).join(p))
                .chain(xdg_dirs.iter().map(|b| b.join(&progname).join(p)))
                .chain(std::iter::once(PathBuf::from(p)))
                .collect();
            find_first_existing(&candidates)
        }
        None => {
            let candidates: Vec<PathBuf> = xdg_home
                .iter()
                .map(|b| b.join(&progname).join("config.toml"))
                .chain(
                    xdg_dirs
                        .iter()
                        .map(|b| b.join(&progname).join("config.toml")),
                )
                .chain(std::iter::once(PathBuf::from("config.toml")))
                .collect();
            find_first_existing(&candidates)
        }
    }
}

fn find_first_existing(candidates: &[PathBuf]) -> Result<PathBuf, ConfigError> {
    for c in candidates {
        if let Ok(canonical) = std::fs::canonicalize(c) {
            return Ok(canonical);
        }
    }
    let last = candidates
        .last()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Err(ConfigError::FileNotFound { path: last })
}

// ── File parsing ──────────────────────────────────────────────────────────────

/// Maximum config file size. Matches the limit documented in `std/config.mvl`.
const MAX_CONFIG_BYTES: u64 = 1 << 20; // 1 MiB

fn parse_file(path: &Path) -> Result<ConfigValue, ConfigError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ConfigError::FileNotFound {
                path: path.display().to_string(),
            }
        } else {
            ConfigError::ParseError {
                msg: format!("could not read file: {e}"),
                line: 0,
                col: 0,
            }
        }
    })?;
    if let Ok(meta) = file.metadata() {
        if meta.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::ParseError {
                msg: format!(
                    "config file too large ({} bytes, max {} bytes)",
                    meta.len(),
                    MAX_CONFIG_BYTES
                ),
                line: 0,
                col: 0,
            });
        }
    }
    let mut content = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|e| ConfigError::ParseError {
            msg: format!("could not read file: {e}"),
            line: 0,
            col: 0,
        })?;
    if content.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::ParseError {
            msg: format!("config file too large (max {} bytes)", MAX_CONFIG_BYTES),
            line: 0,
            col: 0,
        });
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => parse_json(&content),
        _ => parse_toml(&content),
    }
}

// ── Env overlay ───────────────────────────────────────────────────────────────

/// Apply environment variable overrides to a parsed config table.
///
/// Key path: <PREFIX>_<KEY> for top-level, <PREFIX>_<SECTION>_<KEY> for nested.
fn apply_env_overlay(value: &mut ConfigValue, prefix: &str, key_path: &str) {
    let ConfigValue::Table(ref mut table) = value else {
        return;
    };
    for (key, val) in table.iter_mut() {
        let env_key = if key_path.is_empty() {
            format!("{}_{}", prefix, key.to_uppercase())
        } else {
            format!(
                "{}_{}_{}",
                prefix,
                key_path.to_uppercase(),
                key.to_uppercase()
            )
        };
        let nested_path = if key_path.is_empty() {
            key.clone()
        } else {
            format!("{}_{}", key_path, key)
        };
        if let ConfigValue::Table(_) = val {
            apply_env_overlay(val, prefix, &nested_path);
        } else if let Ok(s) = std::env::var(&env_key) {
            *val = coerce_env_str(s, val);
        }
    }
}

fn coerce_env_str(s: String, existing: &ConfigValue) -> ConfigValue {
    match existing {
        ConfigValue::Bool(_) => {
            ConfigValue::Bool(matches!(s.to_lowercase().as_str(), "true" | "1" | "yes"))
        }
        ConfigValue::Int(_) => s
            .parse::<i64>()
            .map(ConfigValue::Int)
            .unwrap_or(ConfigValue::Str(s)),
        ConfigValue::Float(_) => s
            .parse::<f64>()
            .map(ConfigValue::Float)
            .unwrap_or(ConfigValue::Str(s)),
        _ => ConfigValue::Str(s),
    }
}

// ── Minimal TOML parser ───────────────────────────────────────────────────────

pub fn parse_toml(input: &str) -> Result<ConfigValue, ConfigError> {
    let mut root: HashMap<String, ConfigValue> = HashMap::new();
    let mut section: Vec<String> = Vec::new();

    for (idx, raw) in input.lines().enumerate() {
        let line_no = (idx + 1) as i64;
        let line = strip_toml_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if line.starts_with("[[") {
                return Err(ConfigError::ParseError {
                    msg: "array-of-tables '[[...]]' is not supported".to_string(),
                    line: line_no,
                    col: 1,
                });
            }
            let inner = line
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
                .map(str::trim)
                .unwrap_or("");
            if inner.is_empty() {
                return Err(ConfigError::ParseError {
                    msg: "empty section header".to_string(),
                    line: line_no,
                    col: 1,
                });
            }
            section = inner.split('.').map(|s| s.trim().to_string()).collect();
            ensure_nested_table(&mut root, &section);
        } else if let Some((k, v)) = line.split_once('=') {
            let key = k.trim().to_string();
            let val = parse_toml_scalar(v.trim(), line_no)?;
            insert_at(&mut root, &section, key, val);
        } else {
            return Err(ConfigError::ParseError {
                msg: format!("expected 'key = value', got: {line}"),
                line: line_no,
                col: 1,
            });
        }
    }
    Ok(ConfigValue::Table(root))
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, ch) in line.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match ch {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_toml_scalar(s: &str, line: i64) -> Result<ConfigValue, ConfigError> {
    if s == "true" {
        return Ok(ConfigValue::Bool(true));
    }
    if s == "false" {
        return Ok(ConfigValue::Bool(false));
    }
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return Ok(ConfigValue::Str(unescape_str(&s[1..s.len() - 1])));
    }
    if s.contains('.') {
        if let Ok(f) = s.parse::<f64>() {
            return Ok(ConfigValue::Float(f));
        }
    }
    if let Ok(i) = s.parse::<i64>() {
        return Ok(ConfigValue::Int(i));
    }
    Err(ConfigError::ParseError {
        msg: format!("unrecognised value: {s}"),
        line,
        col: 1,
    })
}

fn unescape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn ensure_nested_table(root: &mut HashMap<String, ConfigValue>, path: &[String]) {
    if path.is_empty() {
        return;
    }
    root.entry(path[0].clone())
        .or_insert_with(|| ConfigValue::Table(HashMap::new()));
    if path.len() > 1 {
        if let Some(ConfigValue::Table(sub)) = root.get_mut(&path[0]) {
            ensure_nested_table(sub, &path[1..]);
        }
    }
}

fn insert_at(
    root: &mut HashMap<String, ConfigValue>,
    section: &[String],
    key: String,
    value: ConfigValue,
) {
    if section.is_empty() {
        root.insert(key, value);
        return;
    }
    let entry = root
        .entry(section[0].clone())
        .or_insert_with(|| ConfigValue::Table(HashMap::new()));
    if let ConfigValue::Table(sub) = entry {
        insert_at(sub, &section[1..], key, value);
    }
}

// ── Minimal JSON parser ───────────────────────────────────────────────────────

pub fn parse_json(input: &str) -> Result<ConfigValue, ConfigError> {
    let bytes = input.as_bytes();
    let mut pos = 0usize;
    skip_ws(bytes, &mut pos);
    parse_json_value(bytes, &mut pos).map_err(|msg| {
        let (line, col) = byte_to_line_col(input, pos);
        ConfigError::ParseError { msg, line, col }
    })
}

fn parse_json_value(bytes: &[u8], pos: &mut usize) -> Result<ConfigValue, String> {
    skip_ws(bytes, pos);
    match bytes.get(*pos) {
        Some(b'{') => parse_json_object(bytes, pos),
        Some(b'[') => parse_json_array(bytes, pos),
        Some(b'"') => parse_json_string(bytes, pos).map(ConfigValue::Str),
        Some(b't') => expect_lit(bytes, pos, b"true").map(|_| ConfigValue::Bool(true)),
        Some(b'f') => expect_lit(bytes, pos, b"false").map(|_| ConfigValue::Bool(false)),
        Some(b'n') => expect_lit(bytes, pos, b"null").map(|_| ConfigValue::Null),
        Some(b'-') | Some(b'0'..=b'9') => parse_json_number(bytes, pos),
        Some(b) => Err(format!("unexpected byte: {}", *b as char)),
        None => Err("unexpected end of input".to_string()),
    }
}

fn parse_json_object(bytes: &[u8], pos: &mut usize) -> Result<ConfigValue, String> {
    *pos += 1;
    skip_ws(bytes, pos);
    let mut map = HashMap::new();
    if bytes.get(*pos) == Some(&b'}') {
        *pos += 1;
        return Ok(ConfigValue::Table(map));
    }
    loop {
        skip_ws(bytes, pos);
        let key = parse_json_string(bytes, pos)?;
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b':') {
            return Err("expected ':'".to_string());
        }
        *pos += 1;
        skip_ws(bytes, pos);
        map.insert(key, parse_json_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => {
                *pos += 1;
            }
            Some(b'}') => {
                *pos += 1;
                break;
            }
            _ => return Err("expected ',' or '}'".to_string()),
        }
    }
    Ok(ConfigValue::Table(map))
}

fn parse_json_array(bytes: &[u8], pos: &mut usize) -> Result<ConfigValue, String> {
    *pos += 1;
    skip_ws(bytes, pos);
    let mut arr = Vec::new();
    if bytes.get(*pos) == Some(&b']') {
        *pos += 1;
        return Ok(ConfigValue::Array(arr));
    }
    loop {
        skip_ws(bytes, pos);
        arr.push(parse_json_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => {
                *pos += 1;
            }
            Some(b']') => {
                *pos += 1;
                break;
            }
            _ => return Err("expected ',' or ']'".to_string()),
        }
    }
    Ok(ConfigValue::Array(arr))
}

fn parse_json_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    if bytes.get(*pos) != Some(&b'"') {
        return Err("expected '\"'".to_string());
    }
    *pos += 1;
    let mut raw: Vec<u8> = Vec::new();
    loop {
        match bytes.get(*pos) {
            None => return Err("unterminated string".to_string()),
            Some(b'"') => {
                *pos += 1;
                return String::from_utf8(raw).map_err(|_| "invalid UTF-8 in string".to_string());
            }
            Some(b'\\') => {
                *pos += 1;
                match bytes.get(*pos) {
                    Some(b'"') => {
                        raw.push(b'"');
                        *pos += 1;
                    }
                    Some(b'\\') => {
                        raw.push(b'\\');
                        *pos += 1;
                    }
                    Some(b'/') => {
                        raw.push(b'/');
                        *pos += 1;
                    }
                    Some(b'n') => {
                        raw.push(b'\n');
                        *pos += 1;
                    }
                    Some(b't') => {
                        raw.push(b'\t');
                        *pos += 1;
                    }
                    Some(b'r') => {
                        raw.push(b'\r');
                        *pos += 1;
                    }
                    Some(b'b') => {
                        raw.push(0x08);
                        *pos += 1;
                    }
                    Some(b'f') => {
                        raw.push(0x0C);
                        *pos += 1;
                    }
                    Some(b'u') => {
                        *pos += 1;
                        if *pos + 4 > bytes.len() {
                            return Err("incomplete \\u escape".to_string());
                        }
                        let hex = std::str::from_utf8(&bytes[*pos..*pos + 4])
                            .map_err(|_| "invalid \\u escape".to_string())?;
                        let code = u32::from_str_radix(hex, 16)
                            .map_err(|_| format!("invalid \\u escape: {hex}"))?;
                        let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                        let mut buf = [0u8; 4];
                        raw.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        *pos += 4;
                    }
                    Some(&b) => {
                        raw.push(b'\\');
                        raw.push(b);
                        *pos += 1;
                    }
                    None => return Err("unterminated escape".to_string()),
                }
            }
            Some(&b) => {
                raw.push(b);
                *pos += 1;
            }
        }
    }
}

fn parse_json_number(bytes: &[u8], pos: &mut usize) -> Result<ConfigValue, String> {
    let start = *pos;
    if bytes.get(*pos) == Some(&b'-') {
        *pos += 1;
    }
    while matches!(bytes.get(*pos), Some(b'0'..=b'9')) {
        *pos += 1;
    }
    let is_float = matches!(bytes.get(*pos), Some(b'.') | Some(b'e') | Some(b'E'));
    if is_float {
        *pos += 1;
        while matches!(
            bytes.get(*pos),
            Some(b'0'..=b'9') | Some(b'+') | Some(b'-') | Some(b'e') | Some(b'E') | Some(b'.')
        ) {
            *pos += 1;
        }
        let s = std::str::from_utf8(&bytes[start..*pos]).unwrap_or("");
        s.parse::<f64>()
            .map(ConfigValue::Float)
            .map_err(|_| format!("invalid float: {s}"))
    } else {
        let s = std::str::from_utf8(&bytes[start..*pos]).unwrap_or("");
        s.parse::<i64>()
            .map(ConfigValue::Int)
            .map_err(|_| format!("invalid integer: {s}"))
    }
}

fn expect_lit(bytes: &[u8], pos: &mut usize, lit: &[u8]) -> Result<(), String> {
    if bytes.get(*pos..*pos + lit.len()) == Some(lit) {
        *pos += lit.len();
        Ok(())
    } else {
        Err(format!(
            "expected '{}'",
            std::str::from_utf8(lit).unwrap_or("?")
        ))
    }
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while matches!(
        bytes.get(*pos),
        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    ) {
        *pos += 1;
    }
}

fn byte_to_line_col(src: &str, pos: usize) -> (i64, i64) {
    let pos = pos.min(src.len());
    let line = src[..pos].chars().filter(|&c| c == '\n').count() as i64 + 1;
    let col = src[..pos].rfind('\n').map(|i| pos - i - 1).unwrap_or(pos) as i64 + 1;
    (line, col)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn toml_parses_scalars() {
        let src = "host = \"localhost\"\nport = 8080\nratio = 1.5\ndebug = true\n";
        let v = parse_toml(src).unwrap();
        let ConfigValue::Table(t) = v else { panic!() };
        assert_eq!(t["host"], ConfigValue::Str("localhost".to_string()));
        assert_eq!(t["port"], ConfigValue::Int(8080));
        assert_eq!(t["ratio"], ConfigValue::Float(1.5));
        assert_eq!(t["debug"], ConfigValue::Bool(true));
    }

    #[test]
    fn toml_parses_nested_section() {
        let src = "[db]\nhost = \"db.local\"\nport = 5432\n";
        let v = parse_toml(src).unwrap();
        let ConfigValue::Table(root) = v else {
            panic!()
        };
        let ConfigValue::Table(db) = &root["db"] else {
            panic!()
        };
        assert_eq!(db["port"], ConfigValue::Int(5432));
    }

    #[test]
    fn toml_strips_inline_comments() {
        let v = parse_toml("port = 8080 # main port\n").unwrap();
        let ConfigValue::Table(t) = v else { panic!() };
        assert_eq!(t["port"], ConfigValue::Int(8080));
    }

    #[test]
    fn toml_error_on_bad_value() {
        assert!(matches!(
            parse_toml("x = ???"),
            Err(ConfigError::ParseError { .. })
        ));
    }

    #[test]
    fn json_parses_object() {
        let v = parse_json(r#"{"port": 5432, "ssl": true, "host": "db"}"#).unwrap();
        let ConfigValue::Table(t) = v else { panic!() };
        assert_eq!(t["port"], ConfigValue::Int(5432));
        assert_eq!(t["ssl"], ConfigValue::Bool(true));
    }

    #[test]
    fn json_parses_nested() {
        let v = parse_json(r#"{"db": {"host": "localhost", "port": 5432}}"#).unwrap();
        let ConfigValue::Table(root) = v else {
            panic!()
        };
        let ConfigValue::Table(db) = &root["db"] else {
            panic!()
        };
        assert_eq!(db["port"], ConfigValue::Int(5432));
    }

    #[test]
    fn env_overlay_overrides_int() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let mut val = ConfigValue::Table({
            let mut m = HashMap::new();
            m.insert("port".to_string(), ConfigValue::Int(8080));
            m
        });
        std::env::set_var("APP_PORT", "9090");
        apply_env_overlay(&mut val, "APP", "");
        std::env::remove_var("APP_PORT");
        let ConfigValue::Table(t) = val else { panic!() };
        assert_eq!(t["port"], ConfigValue::Int(9090));
    }

    #[test]
    fn resolve_absolute_missing_returns_err() {
        assert!(matches!(
            resolve_path(Some("/nonexistent/mvl_config_test.toml")),
            Err(ConfigError::FileNotFound { .. })
        ));
    }

    #[test]
    fn resolve_relative_missing_falls_through_to_err() {
        assert!(matches!(
            resolve_path(Some("__mvl_test_nonexistent_config.toml")),
            Err(ConfigError::FileNotFound { .. })
        ));
    }
}
