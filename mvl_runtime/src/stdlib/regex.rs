//! Rust implementations of `std.regex` stdlib functions.
//!
//! Backed by the `regex` crate. All functions are pure — no I/O or side effects.
//! `Regex` is an opaque compiled pattern; pass it to `find`, `replace`, etc.

/// A compiled regular expression. Mirrors `type Regex = struct {}` in `std/regex.mvl`.
pub struct Regex(pub(crate) ::regex::Regex);

impl Regex {
    /// Replace all matches in `s` with `replacement`, returning the result.
    /// Provided for C-ABI callers that borrow the handle without consuming it.
    pub fn replace_all_borrowed(&self, s: &str, replacement: &str) -> String {
        self.0.replace_all(s, replacement).into_owned()
    }
}

/// A single match — the matched text and its byte offsets in the input.
/// Mirrors `type Match = struct { text: String, start: Int, end: Int }` in `std/regex.mvl`.
pub struct Match {
    /// The matched text.
    pub text: String,
    /// Byte offset of the start of the match.
    pub start: i64,
    /// Byte offset one past the end of the match.
    pub end: i64,
}

/// Named and positional captures from a match.
/// Mirrors `type Captures = struct { groups: List[Option[String]], named: Map[String, Option[String]] }`.
pub struct Captures {
    /// Positional capture groups (index 0 = full match).
    pub groups: Vec<Option<String>>,
    /// Named capture groups, keyed by group name.
    pub named: std::collections::HashMap<String, Option<String>>,
}

/// Compiles a regex pattern. Returns `Err` if the pattern is invalid.
pub fn compile(pattern: String) -> Result<Regex, String> {
    ::regex::Regex::new(&pattern)
        .map(Regex)
        .map_err(|e| e.to_string())
}

/// Returns the first match of `re` in `s`, or `None`.
pub fn find(re: Regex, s: String) -> Option<Match> {
    re.0.find(&s).map(|m| Match {
        text: m.as_str().to_owned(),
        start: m.start() as i64,
        end: m.end() as i64,
    })
}

/// Returns all non-overlapping matches of `re` in `s`.
pub fn find_all(re: Regex, s: String) -> Vec<Match> {
    re.0.find_iter(&s)
        .map(|m| Match {
            text: m.as_str().to_owned(),
            start: m.start() as i64,
            end: m.end() as i64,
        })
        .collect()
}

/// Replaces all matches of `re` in `s` with `replacement`.
pub fn replace(re: Regex, s: String, replacement: String) -> String {
    re.0.replace_all(&s, replacement.as_str()).into_owned()
}

/// Returns the captures of the first match of `re` in `s`, or `None`.
pub fn captures(re: Regex, s: String) -> Option<Captures> {
    re.0.captures(&s).map(|caps| {
        let groups = caps
            .iter()
            .map(|m| m.map(|m| m.as_str().to_owned()))
            .collect();
        let named: std::collections::HashMap<String, Option<String>> =
            re.0.capture_names()
                .flatten()
                .map(|name| {
                    let val = caps.name(name).map(|m| m.as_str().to_owned());
                    (name.to_owned(), val)
                })
                .collect();
        Captures { groups, named }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_valid() {
        assert!(compile(r"\d+".into()).is_ok());
    }

    #[test]
    fn compile_invalid() {
        assert!(compile(r"[unclosed".into()).is_err());
    }

    #[test]
    fn find_match() {
        let re = compile(r"\d+".into()).unwrap();
        let m = find(re, "abc 123 def".into()).unwrap();
        assert_eq!(m.text, "123");
        assert_eq!(m.start, 4);
        assert_eq!(m.end, 7);
    }

    #[test]
    fn find_no_match() {
        let re = compile(r"\d+".into()).unwrap();
        assert!(find(re, "no digits here".into()).is_none());
    }

    #[test]
    fn find_all_multiple() {
        let re = compile(r"\d+".into()).unwrap();
        let matches = find_all(re, "1 22 333".into());
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].text, "1");
        assert_eq!(matches[1].text, "22");
        assert_eq!(matches[2].text, "333");
    }

    #[test]
    fn replace_all() {
        let re = compile(r"\d+".into()).unwrap();
        let result = replace(re, "a1b22c333".into(), "N".into());
        assert_eq!(result, "aNbNcN");
    }

    #[test]
    fn captures_groups() {
        let re = compile(r"(\w+)@(\w+)".into()).unwrap();
        let caps = captures(re, "user@host".into()).unwrap();
        assert_eq!(caps.groups[1].as_deref(), Some("user"));
        assert_eq!(caps.groups[2].as_deref(), Some("host"));
    }

    #[test]
    fn captures_named() {
        let re = compile(r"(?P<user>\w+)@(?P<host>\w+)".into()).unwrap();
        let caps = captures(re, "alice@example".into()).unwrap();
        assert_eq!(caps.named["user"].as_deref(), Some("alice"));
        assert_eq!(caps.named["host"].as_deref(), Some("example"));
    }
}
