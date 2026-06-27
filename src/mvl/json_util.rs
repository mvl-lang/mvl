// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! JSON encoding helpers shared across the compiler.
//!
//! Centralizes string escaping so emitter, audit, SBOM, complexity report,
//! and CLI diagnostic output stay byte-identical and spec-conformant.

/// Escape a string for safe inclusion in a JSON double-quoted value.
///
/// Handles:
/// - Required escapes: `"`, `\`, `\n`, `\r`, `\t`
/// - All other C0 control characters (< 0x20) as `\u{:04x}`
/// - U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR (valid in JSON but
///   illegal as bare characters in JavaScript string literals — escaping
///   keeps JSON-embedded-in-JS output safe).
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_backslashes() {
        assert_eq!(json_escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn control_characters() {
        assert_eq!(json_escape("a\nb\tc"), "a\\nb\\tc");
        assert_eq!(json_escape("x\r\n"), "x\\r\\n");
    }

    #[test]
    fn low_control_codes() {
        let input = String::from('\u{0001}');
        assert_eq!(json_escape(&input), "\\u0001");
    }

    #[test]
    fn js_line_separators() {
        assert_eq!(json_escape("a\u{2028}b"), "a\\u2028b");
        assert_eq!(json_escape("a\u{2029}b"), "a\\u2029b");
    }

    #[test]
    fn safe_string_unchanged() {
        assert_eq!(json_escape("openssl"), "openssl");
        assert_eq!(json_escape("1.3.0"), "1.3.0");
    }
}
