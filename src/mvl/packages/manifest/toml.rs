// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Minimal hand-rolled TOML lexer/parser sufficient for `mvl.toml`.
//!
//! Split out of `manifest.rs` (#1562).  All items are `pub(super)` so the
//! parent module's `Manifest::parse` and the per-section parsers in the
//! `sections` sibling module can use them.

use std::collections::HashMap;

pub(super) type TomlTable = HashMap<String, TomlValue>;

// ── Minimal TOML parser ────────────────────────────────────────────────────

/// A minimal TOML value sufficient for mvl.toml parsing.
#[derive(Debug, Clone)]
pub(super) enum TomlValue {
    String(String),
    Table(TomlTable),
    /// Boolean literal (`true` / `false`).
    Bool(bool),
    /// Integer literal.
    Integer(i64),
    /// String arrays (e.g. license allow/deny lists).
    Array(Vec<String>),
}

impl TomlValue {
    pub(super) fn as_str(&self) -> Option<&str> {
        if let TomlValue::String(s) = self {
            Some(s.as_str())
        } else {
            None
        }
    }

    pub(super) fn as_table(&self) -> Option<&TomlTable> {
        if let TomlValue::Table(t) = self {
            Some(t)
        } else {
            None
        }
    }

    pub(super) fn as_bool(&self) -> Option<bool> {
        if let TomlValue::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    pub(super) fn as_integer(&self) -> Option<i64> {
        if let TomlValue::Integer(n) = self {
            Some(*n)
        } else {
            None
        }
    }

    pub(super) fn as_string_array(&self) -> Option<&[String]> {
        if let TomlValue::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }
}

/// Parse a TOML document into a nested table structure.
/// This is a minimal parser that handles:
/// - `[section]` headers
/// - `key = "value"` string assignments
/// - `key = { key2 = "value" }` inline tables
pub(super) fn parse_toml_table(content: &str) -> Result<TomlTable, String> {
    let mut root: TomlTable = HashMap::new();
    let mut current_section: Option<String> = None;

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = strip_comment(raw_line).trim().to_string();

        if line.is_empty() {
            continue;
        }

        // Section header: [section] or [[section]]
        if line.starts_with('[') && !line.starts_with("[[") {
            let inner = line.trim_start_matches('[').trim_end_matches(']').trim();
            current_section = Some(inner.to_string());
            // Ensure section exists as a Table
            let tbl = root
                .entry(inner.to_string())
                .or_insert_with(|| TomlValue::Table(HashMap::new()));
            if tbl.as_table().is_none() {
                return Err(format!(
                    "line {}: section '{inner}' conflicts with scalar",
                    line_num + 1
                ));
            }
            continue;
        }

        // Key = value assignment
        if let Some(eq_pos) = line.find('=') {
            let raw_key = line[..eq_pos].trim();
            let key = unquote_key(raw_key);
            let raw_val = line[eq_pos + 1..].trim();
            let value = parse_value(raw_val, line_num + 1)?;

            if let Some(ref section) = current_section {
                let tbl = root
                    .entry(section.clone())
                    .or_insert_with(|| TomlValue::Table(HashMap::new()));
                if let TomlValue::Table(ref mut t) = tbl {
                    t.insert(key, value);
                }
            } else {
                root.insert(key, value);
            }
        }
    }

    Ok(root)
}

pub(super) fn strip_comment(s: &str) -> &str {
    // Strip # comments but not inside strings
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_str {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if c == '#' && !in_str {
            return &s[..i];
        }
    }
    s
}

pub(super) fn unquote_key(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

pub(super) fn parse_value(s: &str, line: usize) -> Result<TomlValue, String> {
    let s = s.trim();
    // Quoted string
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return Ok(TomlValue::String(unescape_string(&s[1..s.len() - 1])));
    }
    // Inline table: { key = "val", ... }
    if s.starts_with('{') && s.ends_with('}') {
        let inner = s[1..s.len() - 1].trim();
        let mut tbl: TomlTable = HashMap::new();
        // Split on commas that are not inside strings
        for part in split_on_comma(inner) {
            let part = part.trim();
            if let Some(eq) = part.find('=') {
                let k = unquote_key(part[..eq].trim());
                let v_str = part[eq + 1..].trim();
                tbl.insert(k, parse_value(v_str, line)?);
            }
        }
        return Ok(TomlValue::Table(tbl));
    }
    // Array: [ "a", "b", "c" ] — extract string elements
    if s.starts_with('[') && s.ends_with(']') {
        let inner = s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return Ok(TomlValue::Array(vec![]));
        }
        let mut items = Vec::new();
        for part in split_on_comma(inner) {
            let part = part.trim();
            if part.starts_with('"') && part.ends_with('"') && part.len() >= 2 {
                items.push(unescape_string(&part[1..part.len() - 1]));
            }
        }
        return Ok(TomlValue::Array(items));
    }
    // Boolean literals
    if s == "true" {
        return Ok(TomlValue::Bool(true));
    }
    if s == "false" {
        return Ok(TomlValue::Bool(false));
    }
    // Integer literals
    if let Ok(n) = s.parse::<i64>() {
        return Ok(TomlValue::Integer(n));
    }
    Err(format!("line {line}: unsupported TOML value: {s:?}"))
}

pub(super) fn split_on_comma(s: &str) -> Vec<String> {
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
            if c == '[' {
                bracket_depth += 1;
                current.push(c);
                continue;
            }
            if c == ']' {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(c);
                continue;
            }
            if c == ',' && bracket_depth == 0 {
                parts.push(current.clone());
                current.clear();
                continue;
            }
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

pub(super) fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
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
            out.push(c);
        }
    }
    out
}

pub(super) fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
