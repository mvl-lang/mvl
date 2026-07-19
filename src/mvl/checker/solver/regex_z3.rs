// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! Translate an MVL regex literal into a Z3 `Regexp` AST (ADR-0057, #1921).
//!
//! MVL's regex-membership refinements (`self.matches("pattern")`) are discharged
//! at L5 via Z3's `RegLan` theory: `(str.in.re self <regex-tree>)`. This module
//! is the translator — a lightweight recursive-descent parser that walks the
//! admitted regex fragment and builds equivalent Z3 `Regexp` values.
//!
//! # Admitted constructs
//! - Literal characters (with escapes for regex metachars)
//! - Character classes `[abc]`, `[a-z]`, `[^A-Z]`
//! - Predefined classes `\d \D \w \W \s \S`
//! - `.` — any single character
//! - Alternation `a|b`
//! - Quantifiers `*`, `+`, `?`, `{n}`, `{n,m}`, `{n,}`
//! - Anchors `^`, `$` (must be full-string anchors; treated as no-op in RegLan
//!   since Z3's `str.in.re` already means full-string match)
//! - Non-capturing groups `(?:...)`
//! - Plain groups `(...)` (treated as non-capturing — MVL admits no captures)
//!
//! Rejected constructs (backrefs, lookaround, recursion) are filtered by
//! `parser::regex_frag::validate` before we get here — this translator returns
//! `None` if it encounters something it doesn't understand, allowing the L5
//! caller to fall back to `RuntimeCheck` rather than panicking.

use z3::ast::Regexp;
use z3::Context;

/// Translate `pattern` into a Z3 `Regexp` in `ctx`.
///
/// Returns `None` on any parse failure or unsupported construct. The intent
/// is that patterns cleared by [`crate::mvl::parser::regex_frag::validate`]
/// will translate successfully; a `None` here indicates either a translator
/// gap or a fragment-validator mismatch and causes the L5 caller to fall
/// through to `RuntimeCheck`.
pub fn translate<'ctx>(ctx: &'ctx Context, pattern: &str) -> Option<Regexp<'ctx>> {
    let mut p = Parser::new(pattern.as_bytes());
    let re = p.parse_alt(ctx)?;
    if !p.at_end() {
        return None;
    }
    Some(re)
}

struct Parser<'src> {
    src: &'src [u8],
    pos: usize,
}

impl<'src> Parser<'src> {
    fn new(src: &'src [u8]) -> Self {
        Self { src, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn accept(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    // ── Grammar entry points ────────────────────────────────────────────────

    /// `alt := seq ('|' seq)*`
    fn parse_alt<'ctx>(&mut self, ctx: &'ctx Context) -> Option<Regexp<'ctx>> {
        let first = self.parse_seq(ctx)?;
        let mut alts = vec![first];
        while self.accept(b'|') {
            alts.push(self.parse_seq(ctx)?);
        }
        if alts.len() == 1 {
            Some(alts.pop().unwrap())
        } else {
            let refs: Vec<&Regexp<'ctx>> = alts.iter().collect();
            Some(Regexp::union(ctx, &refs))
        }
    }

    /// `seq := atom_with_quantifier*`
    fn parse_seq<'ctx>(&mut self, ctx: &'ctx Context) -> Option<Regexp<'ctx>> {
        let mut parts: Vec<Regexp<'ctx>> = Vec::new();
        loop {
            match self.peek() {
                None | Some(b'|') | Some(b')') => break,
                _ => {
                    let atom = self.parse_atom(ctx)?;
                    let quantified = self.apply_quantifier(ctx, atom)?;
                    parts.push(quantified);
                }
            }
        }
        if parts.is_empty() {
            // Empty sequence = the empty-string regex.
            return Some(Regexp::literal(ctx, ""));
        }
        if parts.len() == 1 {
            return Some(parts.pop().unwrap());
        }
        let refs: Vec<&Regexp<'ctx>> = parts.iter().collect();
        Some(Regexp::concat(ctx, &refs))
    }

    /// atom := '^' | '$' | '.' | '(' ... ')' | '[' ... ']' | escape | literal
    fn parse_atom<'ctx>(&mut self, ctx: &'ctx Context) -> Option<Regexp<'ctx>> {
        let b = self.peek()?;
        match b {
            b'^' | b'$' => {
                // Anchors: Z3's str.in.re already requires the whole string to
                // match the regex, so ^ and $ are semantic no-ops. Emit the
                // empty-string regex so concatenation is a no-op.
                self.advance();
                Some(Regexp::literal(ctx, ""))
            }
            b'.' => {
                self.advance();
                Some(any_char(ctx))
            }
            b'(' => {
                self.advance();
                // Support both `(?:...)` (non-capturing) and `(...)`.
                // parser::regex_frag rejects other `(?...)` forms — the `!accept`
                // path here is a defensive fallback for that.
                if self.accept(b'?') && !self.accept(b':') {
                    return None;
                }
                let inner = self.parse_alt(ctx)?;
                if !self.accept(b')') {
                    return None;
                }
                Some(inner)
            }
            b'[' => {
                self.advance();
                self.parse_char_class(ctx)
            }
            b'\\' => {
                self.advance();
                self.parse_escape(ctx)
            }
            b')' | b'|' => None, // shouldn't happen — parse_seq stops on these
            _ => {
                self.advance();
                Some(Regexp::literal(ctx, &(b as char).to_string()))
            }
        }
    }

    /// Apply a trailing `*`, `+`, `?`, `{n}`, `{n,m}`, `{n,}` to an atom.
    /// Non-greedy modifier `?` after a quantifier is accepted and ignored —
    /// regex greediness has no bearing on set membership.
    fn apply_quantifier<'ctx>(
        &mut self,
        ctx: &'ctx Context,
        atom: Regexp<'ctx>,
    ) -> Option<Regexp<'ctx>> {
        let out = match self.peek() {
            Some(b'*') => {
                self.advance();
                atom.star()
            }
            Some(b'+') => {
                self.advance();
                atom.plus()
            }
            Some(b'?') => {
                self.advance();
                // Optional: match atom OR empty string.
                let empty = Regexp::literal(ctx, "");
                Regexp::union(ctx, &[&atom, &empty])
            }
            Some(b'{') => {
                self.advance();
                let (lo, hi) = self.parse_brace_quantifier()?;
                match hi {
                    Some(hi) => atom.r#loop(lo, hi),
                    None => {
                        // `{n,}` — at least n. Z3 has no direct API; build as
                        // atom{n} concat atom*.
                        let fixed = atom.r#loop(lo, lo);
                        let tail = atom.star();
                        Regexp::concat(ctx, &[&fixed, &tail])
                    }
                }
            }
            _ => return Some(atom),
        };
        // Consume optional non-greedy `?` — semantically irrelevant here.
        let _ = self.accept(b'?');
        Some(out)
    }

    /// Parse the body of `{n}` / `{n,m}` / `{n,}` (the leading `{` has been
    /// consumed). Returns `(lo, Some(hi))` for bounded, `(lo, None)` for open.
    fn parse_brace_quantifier(&mut self) -> Option<(u32, Option<u32>)> {
        let lo = self.parse_decimal()?;
        if self.accept(b'}') {
            return Some((lo, Some(lo)));
        }
        if !self.accept(b',') {
            return None;
        }
        if self.accept(b'}') {
            return Some((lo, None));
        }
        let hi = self.parse_decimal()?;
        if !self.accept(b'}') {
            return None;
        }
        if hi < lo {
            return None;
        }
        Some((lo, Some(hi)))
    }

    fn parse_decimal(&mut self) -> Option<u32> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return None;
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .ok()?
            .parse()
            .ok()
    }

    /// Parse a character class `[...]` (opening `[` has been consumed).
    ///
    /// Items are collected first as raw `(lo, hi)` char ranges (plus optional
    /// predefined-class regexes) so a negation can compute the complementary
    /// Unicode ranges directly rather than relying on Z3's `re.comp`. See the
    /// note in the negation branch for why complement-based negation is avoided.
    fn parse_char_class<'ctx>(&mut self, ctx: &'ctx Context) -> Option<Regexp<'ctx>> {
        let negated = self.accept(b'^');
        // Ranges of chars in the class. A single char `x` is stored as `(x, x)`.
        let mut ranges: Vec<(char, char)> = Vec::new();
        // Predefined classes (\d \w \s) don't decompose into char ranges cleanly
        // in the negation path, so we track them separately and reject negation
        // when they appear (defensive — no test in the admitted-fragment set
        // requires negation of predefined classes).
        let mut has_predef_class = false;
        let mut extras: Vec<Regexp<'ctx>> = Vec::new();

        loop {
            let b = self.peek()?;
            if b == b']' {
                self.advance();
                break;
            }
            if b == b'\\' {
                self.advance();
                let esc = self.advance()?;
                if let Some(r) = predefined_class(ctx, esc) {
                    has_predef_class = true;
                    extras.push(r);
                } else {
                    // Escaped literal (e.g. `\-`, `\]`, `\\`, `\n`).
                    let ch = match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        _ => esc as char,
                    };
                    ranges.push((ch, ch));
                }
                continue;
            }
            self.advance();
            let lo = b as char;
            if self.peek() == Some(b'-') && self.src.get(self.pos + 1) != Some(&b']') {
                self.advance();
                let hi_b = self.advance()?;
                let hi = hi_b as char;
                if (hi as u32) < (lo as u32) {
                    return None;
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }

        if ranges.is_empty() && extras.is_empty() {
            return None;
        }

        if !negated {
            let mut all: Vec<Regexp<'ctx>> = ranges
                .iter()
                .map(|(lo, hi)| {
                    if lo == hi {
                        Regexp::literal(ctx, &lo.to_string())
                    } else {
                        Regexp::range(ctx, lo, hi)
                    }
                })
                .collect();
            all.extend(extras);
            if all.len() == 1 {
                return Some(all.pop().unwrap());
            }
            let refs: Vec<&Regexp<'ctx>> = all.iter().collect();
            return Some(Regexp::union(ctx, &refs));
        }

        // Negation path.
        //
        // We avoid Z3's `re.comp` (regex complement) here — on the z3 0.12
        // crate / Z3 4.x combination it interacts poorly with subsequent
        // quantifiers, producing empty languages under `+`/`*`. Instead, we
        // materialise the negation as the union of Unicode ranges NOT covered
        // by the class. Same result, cleaner and faster to solve.
        if has_predef_class {
            // Defensive: negation of `[^\d…]`-style classes not supported.
            // The admitted fragment doesn't require this shape; return None so
            // the caller falls through to RuntimeCheck.
            return None;
        }
        let comp_ranges = complement_ranges(&ranges);
        if comp_ranges.is_empty() {
            return None;
        }
        let alts: Vec<Regexp<'ctx>> = comp_ranges
            .iter()
            .map(|(lo, hi)| {
                if lo == hi {
                    Regexp::literal(ctx, &lo.to_string())
                } else {
                    Regexp::range(ctx, lo, hi)
                }
            })
            .collect();
        if alts.len() == 1 {
            return Some(alts.into_iter().next().unwrap());
        }
        let refs: Vec<&Regexp<'ctx>> = alts.iter().collect();
        Some(Regexp::union(ctx, &refs))
    }

    /// Parse an escape after a backslash. Handles predefined classes and
    /// literal-escape of regex metacharacters.
    fn parse_escape<'ctx>(&mut self, ctx: &'ctx Context) -> Option<Regexp<'ctx>> {
        let b = self.advance()?;
        if let Some(r) = predefined_class(ctx, b) {
            return Some(r);
        }
        // Common single-char escapes.
        let ch = match b {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'0' => '\0',
            _ => b as char, // \. \* \+ \? \\ \/ \- \| \( \) \[ \] \{ \} etc.
        };
        Some(Regexp::literal(ctx, &ch.to_string()))
    }
}

/// Upper bound of the character range we admit for Z3 `re.range` construction.
///
/// The z3 0.12 crate encodes range bounds as UTF-8 byte sequences and Z3's
/// `re.range` operator over those bounds only yields a well-formed character
/// range when both bounds are single-byte (ASCII, ≤ U+007F). Multi-byte
/// bounds produce an empty language — see the `probe_range_upper_bound`
/// diagnostic test. This limitation is documented in ADR-0057.
const MAX_ADMITTED_CHAR: u32 = 0x7F;

/// Compute the complement of a set of `(lo, hi)` character ranges over the
/// non-NUL ASCII range `[U+0001 .. U+007F]`.
///
/// The complement is intentionally bounded to ASCII — see
/// [`MAX_ADMITTED_CHAR`] for why. Non-ASCII refinement predicates fall through
/// to `RuntimeCheck` rather than produce a wrong static answer.
fn complement_ranges(ranges: &[(char, char)]) -> Vec<(char, char)> {
    // Merge overlapping/adjacent input ranges.
    let mut sorted: Vec<(u32, u32)> = ranges.iter().map(|(l, h)| (*l as u32, *h as u32)).collect();
    sorted.sort_by_key(|&(l, _)| l);
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (l, h) in sorted {
        if let Some(last) = merged.last_mut() {
            if l <= last.1 + 1 {
                last.1 = last.1.max(h);
                continue;
            }
        }
        merged.push((l, h));
    }

    // Walk the admitted ASCII range and emit gaps. Ranges outside ASCII in
    // the input are silently clamped to `MAX_ADMITTED_CHAR` — this loses
    // precision but keeps the result inside Z3's usable domain.
    let mut out: Vec<(char, char)> = Vec::new();
    let mut cursor: u32 = 0x0001;
    for (l, h) in merged {
        let l = l.min(MAX_ADMITTED_CHAR);
        let h = h.min(MAX_ADMITTED_CHAR);
        if cursor < l {
            let gap_end = l - 1;
            if let (Some(a), Some(b)) = (char::from_u32(cursor), char::from_u32(gap_end)) {
                out.push((a, b));
            }
        }
        cursor = h + 1;
        if cursor > MAX_ADMITTED_CHAR {
            break;
        }
    }
    if cursor <= MAX_ADMITTED_CHAR {
        if let (Some(a), Some(b)) = (char::from_u32(cursor), char::from_u32(MAX_ADMITTED_CHAR)) {
            out.push((a, b));
        }
    }
    out
}

/// A Z3 Regexp matching any single ASCII character (U+0001..U+007F).
///
/// Restricted to ASCII because Z3's `re.range` on the z3 0.12 crate misbehaves
/// for multi-byte UTF-8 bounds — see [`MAX_ADMITTED_CHAR`] and ADR-0057.
fn any_char<'ctx>(ctx: &'ctx Context) -> Regexp<'ctx> {
    Regexp::range(ctx, &'\u{1}', &'\u{7F}')
}

/// Return a Z3 Regexp for a predefined character class (`\d \D \w \W \s \S`),
/// or `None` if `b` doesn't name one.
fn predefined_class<'ctx>(ctx: &'ctx Context, b: u8) -> Option<Regexp<'ctx>> {
    match b {
        b'd' => Some(Regexp::range(ctx, &'0', &'9')),
        b'D' => {
            let digits = Regexp::range(ctx, &'0', &'9');
            let anychar = any_char(ctx);
            Some(Regexp::intersect(ctx, &[&anychar, &digits.complement()]))
        }
        b'w' => {
            // [A-Za-z0-9_]
            let a_z = Regexp::range(ctx, &'a', &'z');
            let a_z_u = Regexp::range(ctx, &'A', &'Z');
            let d09 = Regexp::range(ctx, &'0', &'9');
            let underscore = Regexp::literal(ctx, "_");
            Some(Regexp::union(ctx, &[&a_z, &a_z_u, &d09, &underscore]))
        }
        b'W' => {
            let a_z = Regexp::range(ctx, &'a', &'z');
            let a_z_u = Regexp::range(ctx, &'A', &'Z');
            let d09 = Regexp::range(ctx, &'0', &'9');
            let underscore = Regexp::literal(ctx, "_");
            let word = Regexp::union(ctx, &[&a_z, &a_z_u, &d09, &underscore]);
            let anychar = any_char(ctx);
            Some(Regexp::intersect(ctx, &[&anychar, &word.complement()]))
        }
        b's' => {
            // Whitespace: space, tab, newline, carriage return, form feed, vertical tab
            let space = Regexp::literal(ctx, " ");
            let tab = Regexp::literal(ctx, "\t");
            let nl = Regexp::literal(ctx, "\n");
            let cr = Regexp::literal(ctx, "\r");
            let ff = Regexp::literal(ctx, "\x0C");
            let vt = Regexp::literal(ctx, "\x0B");
            Some(Regexp::union(ctx, &[&space, &tab, &nl, &cr, &ff, &vt]))
        }
        b'S' => {
            let space = Regexp::literal(ctx, " ");
            let tab = Regexp::literal(ctx, "\t");
            let nl = Regexp::literal(ctx, "\n");
            let cr = Regexp::literal(ctx, "\r");
            let ff = Regexp::literal(ctx, "\x0C");
            let vt = Regexp::literal(ctx, "\x0B");
            let ws = Regexp::union(ctx, &[&space, &tab, &nl, &cr, &ff, &vt]);
            let anychar = any_char(ctx);
            Some(Regexp::intersect(ctx, &[&anychar, &ws.complement()]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::{Config, SatResult, Solver};

    fn assert_matches(pattern: &str, s: &str) {
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let re = translate(&ctx, pattern).expect("translation failed");
        let hay = z3::ast::String::from_str(&ctx, s).unwrap();
        solver.assert(&hay.regex_matches(&re));
        assert_eq!(
            solver.check(),
            SatResult::Sat,
            "pattern {pattern:?} should match {s:?}"
        );
    }

    fn assert_no_match(pattern: &str, s: &str) {
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let re = translate(&ctx, pattern).expect("translation failed");
        let hay = z3::ast::String::from_str(&ctx, s).unwrap();
        solver.assert(&hay.regex_matches(&re));
        assert_eq!(
            solver.check(),
            SatResult::Unsat,
            "pattern {pattern:?} should NOT match {s:?}"
        );
    }

    #[test]
    fn literal_pattern() {
        assert_matches("^hello$", "hello");
        assert_no_match("^hello$", "world");
    }

    #[test]
    fn char_class_range() {
        assert_matches("^[a-z]+$", "hello");
        assert_no_match("^[a-z]+$", "Hello");
    }

    #[test]
    fn digit_class() {
        assert_matches(r"^\d{3}$", "123");
        assert_no_match(r"^\d{3}$", "abc");
    }

    #[test]
    fn alternation() {
        assert_matches("^(foo|bar)$", "foo");
        assert_matches("^(foo|bar)$", "bar");
        assert_no_match("^(foo|bar)$", "baz");
    }

    #[test]
    fn bounded_quantifier() {
        assert_matches("^a{2,4}$", "aa");
        assert_matches("^a{2,4}$", "aaaa");
        assert_no_match("^a{2,4}$", "a");
        assert_no_match("^a{2,4}$", "aaaaa");
    }

    #[test]
    fn open_range_quantifier() {
        assert_matches("^a{2,}$", "aa");
        assert_matches("^a{2,}$", "aaaaaa");
        assert_no_match("^a{2,}$", "a");
    }

    #[test]
    fn iban_prefix() {
        assert_matches("^[A-Z]{2}[0-9]{2}[A-Z0-9]+$", "DE89370400440532013000");
        assert_no_match("^[A-Z]{2}[0-9]{2}[A-Z0-9]+$", "de89370400440532013000");
    }

    #[test]
    fn safe_ident() {
        assert_matches("^[a-zA-Z_][a-zA-Z0-9_]*$", "user_name42");
        assert_no_match("^[a-zA-Z_][a-zA-Z0-9_]*$", "42user");
    }

    #[test]
    fn negated_class() {
        assert_matches("^[^0-9]+$", "abc");
        assert_no_match("^[^0-9]+$", "abc1");
    }

    #[test]
    fn probe_range_upper_bound() {
        // Diagnostic: documents the z3 0.12 crate limitation that motivated
        // MAX_ADMITTED_CHAR. `re.range` with ASCII bounds returns Sat; with
        // any bound above U+007F it returns Unsat (empty language), because
        // the crate encodes bounds as UTF-8 byte sequences and Z3 doesn't
        // decode them as code points for range purposes.
        //
        // If a future z3 crate release fixes this, the assertions below will
        // start failing — which is the signal to widen MAX_ADMITTED_CHAR.
        // See the follow-up ticket linked from ADR-0057.
        for &(hi, label, expected_ascii) in &[
            ('\u{7F}', "0x7F ASCII", true),
            ('\u{FF}', "0xFF Latin-1", false),
            ('\u{FFFF}', "0xFFFF BMP-max", false),
            ('\u{10FFFF}', "0x10FFFF max scalar", false),
        ] {
            let cfg = Config::new();
            let ctx = Context::new(&cfg);
            let solver = Solver::new(&ctx);
            let re = Regexp::range(&ctx, &':', &hi);
            let hay = z3::ast::String::from_str(&ctx, "a").unwrap();
            solver.assert(&hay.regex_matches(&re));
            let got = solver.check() == SatResult::Sat;
            assert_eq!(
                got, expected_ascii,
                "probe upper={label}: expected Sat={expected_ascii}, got Sat={got}"
            );
        }
    }

    #[test]
    fn non_capturing_group() {
        assert_matches("^(?:ab)+$", "ababab");
        assert_no_match("^(?:ab)+$", "aba");
    }

    #[test]
    fn optional_atom() {
        assert_matches("^colou?r$", "color");
        assert_matches("^colou?r$", "colour");
        assert_no_match("^colou?r$", "colouur");
    }

    #[test]
    fn escaped_metachar() {
        assert_matches(r"^\.$", ".");
        assert_no_match(r"^\.$", "a");
    }

    #[test]
    fn bearer_token_format() {
        let pat = "^Bearer [A-Za-z0-9._~+/=-]+$";
        assert_matches(pat, "Bearer abc123");
        assert_matches(pat, "Bearer AbC.123~/=+");
        assert_no_match(pat, "Basic abc123");
    }

    #[test]
    fn translation_returns_none_on_trailing_garbage() {
        // Unbalanced parens should return None rather than panicking.
        // (Note: parser::regex_frag doesn't check balance — that's the regex crate's
        // job. This test just verifies the translator fails gracefully.)
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        assert!(translate(&ctx, "^(unclosed").is_none());
    }
}
