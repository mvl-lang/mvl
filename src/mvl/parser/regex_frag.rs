// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Regex-fragment validator for refinement predicates (#1921, ADR-0057).
//!
//! MVL's refinement predicates admit only the **regular** fragment of regex
//! syntax — the subset that SMT solvers (Z3's `RegLan` theory) can decide.
//! PCRE-flavour features that make the language irregular (backreferences,
//! lookaround, recursion) are rejected at parse time with a diagnostic
//! naming the offending feature.
//!
//! # Admitted fragment
//! - Literal characters, escape sequences (`\.`, `\\`, `\n`, `\t`, `\r`, `\0`,
//!   `\-`, `\/`, quoted metacharacters)
//! - Character classes: `[a-z]`, `[^A-Z]`, POSIX-style ranges, negations
//! - Predefined classes: `\d`, `\D`, `\w`, `\W`, `\s`, `\S`
//! - Alternation: `a|b`
//! - Quantifiers: `*`, `+`, `?`, `{n}`, `{n,}`, `{n,m}` (and non-greedy `*?` etc.)
//! - Anchors: `^`, `$`
//! - Non-capturing groups: `(?:...)`
//! - Plain groups: `(...)` (treated as non-capturing; no captures needed for
//!   membership checks)
//!
//! # Rejected fragment
//! - Backreferences: `\1`..`\9`, `\k<name>`
//! - Lookaround: `(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)`
//! - Recursion: `(?R)`, `(?0)`, `(?1)`
//! - Named captures with references (the named-capture *declaration* `(?<name>...)`
//!   is technically regular, but MVL rejects it defensively because it opens
//!   the door to `\k<name>` backreferences downstream)
//! - Atomic groups: `(?>...)`
//! - Conditional expressions: `(?(...)...|...)`
//! - Inline flags mid-pattern: `(?i)`, `(?-i)` — regex crate/Z3 handling divergent
//!
//! # Design note
//! This validator is intentionally lightweight — it scans for the offending
//! *syntactic markers* rather than fully parsing the regex. The `regex` crate
//! and Z3 do the authoritative parsing later; this pass exists only to give
//! users an early, clearly-named diagnostic before those errors surface.

/// Error returned by [`validate`] when the pattern uses a rejected feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexFragError {
    /// Zero-based byte offset within the pattern where the rejected feature starts.
    pub offset: usize,
    /// Human-readable name of the offending feature (e.g. "backreference `\\1`").
    pub feature: String,
}

impl core::fmt::Display for RegexFragError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "regex pattern uses {} at offset {} — outside the admitted regular fragment (see ADR-0057)",
            self.feature, self.offset
        )
    }
}

/// Validate that `pattern` uses only features from the admitted regular fragment.
///
/// Returns `Ok(())` if the pattern is admitted, `Err(RegexFragError)` naming
/// the first rejected feature encountered.
pub fn validate(pattern: &str) -> Result<(), RegexFragError> {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    let mut in_class = false; // inside `[...]`

    while i < bytes.len() {
        let b = bytes[i];

        // Character class opens/closes track separately because escape handling
        // and grouping semantics differ inside a class.
        if !in_class {
            match b {
                b'\\' => {
                    // Escape sequence: check whether the following character is a
                    // backreference digit or a named-backreference marker.
                    if i + 1 >= bytes.len() {
                        // Trailing backslash — will fail in the regex crate anyway;
                        // no fragment concern.
                        i += 1;
                        continue;
                    }
                    let next = bytes[i + 1];
                    if next.is_ascii_digit() {
                        return Err(RegexFragError {
                            offset: i,
                            feature: format!("backreference `\\{}`", next as char),
                        });
                    }
                    if next == b'k' {
                        return Err(RegexFragError {
                            offset: i,
                            feature: "named backreference `\\k<...>`".to_string(),
                        });
                    }
                    if next == b'g' {
                        // `\g<name>` or `\g{name}` — subroutine call, also irregular.
                        return Err(RegexFragError {
                            offset: i,
                            feature: "subroutine call `\\g<...>`".to_string(),
                        });
                    }
                    i += 2;
                    continue;
                }
                b'[' => {
                    in_class = true;
                    i += 1;
                    continue;
                }
                b'(' => {
                    // Group open — inspect the following characters to identify
                    // rejected group syntaxes.
                    let rest = &bytes[i + 1..];
                    if let Some(&next) = rest.first() {
                        if next == b'?' {
                            // (? ... ) family
                            let after_q = rest.get(1).copied();
                            match after_q {
                                Some(b':') => {
                                    // Non-capturing group — admitted.
                                    i += 3;
                                    continue;
                                }
                                Some(b'=') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "lookahead `(?=...)`".to_string(),
                                    });
                                }
                                Some(b'!') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "negative lookahead `(?!...)`".to_string(),
                                    });
                                }
                                Some(b'<') => {
                                    // Could be lookbehind `(?<=...)` / `(?<!...)`
                                    // or named-capture `(?<name>...)`.
                                    let after_lt = rest.get(2).copied();
                                    return Err(match after_lt {
                                        Some(b'=') => RegexFragError {
                                            offset: i,
                                            feature: "lookbehind `(?<=...)`".to_string(),
                                        },
                                        Some(b'!') => RegexFragError {
                                            offset: i,
                                            feature: "negative lookbehind `(?<!...)`".to_string(),
                                        },
                                        _ => RegexFragError {
                                            offset: i,
                                            feature: "named capture `(?<name>...)`".to_string(),
                                        },
                                    });
                                }
                                Some(b'P') => {
                                    // (?P<name>...) or (?P=name) — Python-style named capture / backref.
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "named capture / backref `(?P...)`".to_string(),
                                    });
                                }
                                Some(b'>') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "atomic group `(?>...)`".to_string(),
                                    });
                                }
                                Some(b'R') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "recursion `(?R)`".to_string(),
                                    });
                                }
                                Some(b'0'..=b'9') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "subroutine call `(?N)`".to_string(),
                                    });
                                }
                                Some(b'(') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "conditional expression `(?(...)...)`".to_string(),
                                    });
                                }
                                Some(b'i') | Some(b'm') | Some(b's') | Some(b'x') | Some(b'U')
                                | Some(b'-') => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "inline flags `(?flags)` or `(?flags:...)`"
                                            .to_string(),
                                    });
                                }
                                _ => {
                                    return Err(RegexFragError {
                                        offset: i,
                                        feature: "unsupported `(?...)` construct".to_string(),
                                    });
                                }
                            }
                        }
                    }
                    // Plain `(...)` group — admitted, treated as non-capturing.
                    i += 1;
                    continue;
                }
                _ => {
                    i += 1;
                    continue;
                }
            }
        } else {
            // Inside `[...]` — most metacharacters lose their meaning, but backrefs
            // inside a class are not a thing (regex escapes are just literals).
            match b {
                b'\\' => {
                    // Skip escape and next char.
                    i += 2.min(bytes.len() - i);
                    continue;
                }
                b']' => {
                    in_class = false;
                    i += 1;
                    continue;
                }
                _ => {
                    i += 1;
                    continue;
                }
            }
        }
    }

    Ok(())
}

// ── Length-interval extraction (L2 helper for #1921) ─────────────────────────

/// A length interval extracted from a regex pattern.
///
/// The `max` bound is `None` when the pattern permits unbounded length
/// (e.g. `^.+$`, `^abc$` with a Kleene star inside).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LengthInterval {
    pub min: usize,
    pub max: Option<usize>,
}

/// Extract a length interval from a fully-anchored, fixed-quantifier pattern.
///
/// Recognises the shapes we can reason about without a full regex analyser:
///   - `^.{n}$`     → `[n, n]`
///   - `^.{n,m}$`   → `[n, m]`
///   - `^.{n,}$`    → `[n, ∞)`
///   - `^literal$`  → `[byte_len, byte_len]`
///
/// Returns `None` for patterns outside those shapes. This is intentionally
/// narrow — better to return `None` than to give a wrong answer that a
/// downstream solver would trust.
///
/// Length is measured in UTF-8 code units (byte length), matching how
/// `String::len()` behaves in MVL and Rust.
///
/// # Future work
/// Bridging this helper into L2 requires a length-interval abstraction over
/// `len(self)`; L2 today is integer-domain over `self` and does not model
/// string-length hypotheses. Tracked as follow-up to ADR-0057.
pub fn regex_length_interval(pattern: &str) -> Option<LengthInterval> {
    // Must be fully anchored — otherwise the regex only constrains a substring.
    let inner = pattern.strip_prefix('^')?.strip_suffix('$')?;

    // Shape 1: `.{n}` / `.{n,m}` / `.{n,}` — fixed quantifier over any char.
    if let Some(rest) = inner.strip_prefix(".{") {
        let body = rest.strip_suffix('}')?;
        // Body is one of: "n" | "n," | "n,m"
        if let Some((lo_s, hi_s)) = body.split_once(',') {
            let lo: usize = lo_s.parse().ok()?;
            if hi_s.is_empty() {
                return Some(LengthInterval { min: lo, max: None });
            }
            let hi: usize = hi_s.parse().ok()?;
            if hi < lo {
                return None;
            }
            return Some(LengthInterval {
                min: lo,
                max: Some(hi),
            });
        }
        let n: usize = body.parse().ok()?;
        return Some(LengthInterval {
            min: n,
            max: Some(n),
        });
    }

    // Shape 2: `^literal$` — literal body with no regex metacharacters.
    if inner.bytes().all(|b| {
        !matches!(
            b,
            b'\\'
                | b'.'
                | b'*'
                | b'+'
                | b'?'
                | b'['
                | b']'
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'|'
                | b'^'
                | b'$'
        )
    }) {
        return Some(LengthInterval {
            min: inner.len(),
            max: Some(inner.len()),
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_feature(pattern: &str) -> String {
        validate(pattern).unwrap_err().feature
    }

    #[test]
    fn admits_char_classes_and_quantifiers() {
        assert!(validate("^[A-Z]{2}[0-9]{2}[A-Z0-9]+$").is_ok());
        assert!(validate("[a-zA-Z_][a-zA-Z0-9_]*").is_ok());
        assert!(validate("^Bearer [A-Za-z0-9._~+/=-]+$").is_ok());
    }

    #[test]
    fn admits_alternation_and_anchors() {
        assert!(validate("^(foo|bar|baz)$").is_ok());
        assert!(validate("cat|dog").is_ok());
    }

    #[test]
    fn admits_non_capturing_groups() {
        assert!(validate("(?:abc)+").is_ok());
        assert!(validate("^(?:https?|ftp)://").is_ok());
    }

    #[test]
    fn admits_predefined_classes() {
        assert!(validate(r"\d+").is_ok());
        assert!(validate(r"\w+\s+\w+").is_ok());
        assert!(validate(r"[^\d]").is_ok());
    }

    #[test]
    fn admits_escaped_metachars() {
        assert!(validate(r"\.\*\+\?").is_ok());
        assert!(validate(r"\(hello\)").is_ok());
    }

    #[test]
    fn admits_bounded_quantifiers() {
        assert!(validate("a{3}").is_ok());
        assert!(validate("a{2,5}").is_ok());
        assert!(validate("a{2,}").is_ok());
    }

    #[test]
    fn rejects_backreferences() {
        assert_eq!(err_feature(r"(a)\1"), "backreference `\\1`");
        assert_eq!(err_feature(r"(a)(b)\2"), "backreference `\\2`");
    }

    #[test]
    fn rejects_named_backreferences() {
        assert_eq!(err_feature(r"(?<a>x)\k<a>"), "named capture `(?<name>...)`");
    }

    #[test]
    fn rejects_lookahead() {
        assert_eq!(err_feature("a(?=b)"), "lookahead `(?=...)`");
        assert_eq!(err_feature("a(?!b)"), "negative lookahead `(?!...)`");
    }

    #[test]
    fn rejects_lookbehind() {
        assert_eq!(err_feature("(?<=a)b"), "lookbehind `(?<=...)`");
        assert_eq!(err_feature("(?<!a)b"), "negative lookbehind `(?<!...)`");
    }

    #[test]
    fn rejects_recursion() {
        assert_eq!(err_feature("^(?R)$"), "recursion `(?R)`");
    }

    #[test]
    fn rejects_atomic_groups() {
        assert_eq!(err_feature("(?>abc)"), "atomic group `(?>...)`");
    }

    #[test]
    fn rejects_inline_flags() {
        assert_eq!(
            err_feature("(?i)hello"),
            "inline flags `(?flags)` or `(?flags:...)`"
        );
    }

    #[test]
    fn rejects_conditional() {
        assert_eq!(
            err_feature("(?(1)a|b)"),
            "conditional expression `(?(...)...)`"
        );
    }

    #[test]
    fn backslash_inside_class_is_ignored() {
        // `\d` inside a class is not a backref — just a shorthand class.
        assert!(validate(r"[\d\s]+").is_ok());
    }

    #[test]
    fn empty_pattern_is_admitted() {
        assert!(validate("").is_ok());
    }

    // ── length-interval extraction ──────────────────────────────────────────

    #[test]
    fn length_from_fixed_quantifier() {
        assert_eq!(
            regex_length_interval("^.{5}$"),
            Some(LengthInterval {
                min: 5,
                max: Some(5),
            })
        );
    }

    #[test]
    fn length_from_range_quantifier() {
        assert_eq!(
            regex_length_interval("^.{3,7}$"),
            Some(LengthInterval {
                min: 3,
                max: Some(7),
            })
        );
    }

    #[test]
    fn length_from_open_range() {
        assert_eq!(
            regex_length_interval("^.{2,}$"),
            Some(LengthInterval { min: 2, max: None })
        );
    }

    #[test]
    fn length_from_literal_anchor() {
        assert_eq!(
            regex_length_interval("^abc$"),
            Some(LengthInterval {
                min: 3,
                max: Some(3),
            })
        );
    }

    #[test]
    fn no_length_from_unanchored() {
        assert!(regex_length_interval(".{5}").is_none());
        assert!(regex_length_interval("^.{5}").is_none());
        assert!(regex_length_interval(".{5}$").is_none());
    }

    #[test]
    fn no_length_from_complex_pattern() {
        assert!(regex_length_interval("^[A-Z]{2}[0-9]{2}$").is_none());
        assert!(regex_length_interval("^abc*$").is_none());
    }

    #[test]
    fn no_length_from_inverted_range() {
        assert!(regex_length_interval("^.{5,3}$").is_none());
    }
}
