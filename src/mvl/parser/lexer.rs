// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis

//! MVL Lexer — tokenizes source text into a stream of typed tokens.
//!
//! Every token carries a [`Span`] (line, col, byte offset, length) so
//! the parser can produce high-quality error messages.  Keywords are
//! recognized by table lookup after identifier scanning (LL(1), no
//! backtracking, zero dependencies).

use std::fmt;

// ── Source location ────────────────────────────────────────────────────────

/// Half-open byte range in the source, with human-readable line/col.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (character, not byte).
    pub col: u32,
    /// Byte offset of the first character.
    pub offset: u32,
    /// Byte length of the token.
    pub len: u32,
}

impl Span {
    pub fn new(line: u32, col: u32, offset: u32, len: u32) -> Self {
        Span {
            line,
            col,
            offset,
            len,
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

// ── Token kinds ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Declaration keywords ──────────────────────────────────────────────
    Fn,
    Let,
    Mut,
    Match,
    If,
    Else,
    For,
    While,
    Type,
    Total,
    Partial,
    Return,
    Move,
    Consume,
    Declassify,
    Sanitize,
    Const,
    Where,
    In,
    /// `extern` — introduces a foreign-function trust boundary.
    Extern,
    /// `struct` — keyword inside type declarations (`type Foo = struct { … }`)
    Struct,
    /// `enum` — keyword inside type declarations (`type Foo = enum { … }`)
    Enum,
    /// `pub` — visibility modifier
    Pub,
    /// `use` — import declaration (`use path::to::Item;`)
    Use,
    /// `test` — marks a function as a unit test (`test fn name() { … }`)
    Test,
    /// `impl` — introduces a trait implementation block (`impl Trait for Type { … }`)
    Impl,
    /// `builtin` — marks a function as having a runtime-provided implementation.
    /// `builtin fn` declarations have no body; the compiler trusts the runtime.
    Builtin,
    /// `transparent` — marks a function as label-transparent (ADR-0024):
    /// the checker joins argument security labels and applies them to the return type.
    Transparent,
    /// `requires` — function contract precondition.
    Requires,
    /// `ensures` — function contract postcondition.
    Ensures,
    /// `invariant` — loop invariant predicate (Phase 3, #621).
    Invariant,
    /// `ghost` — marks a binding as ghost (specification-only, erased before codegen, Phase 4, #627).
    Ghost,
    /// `decreases` — loop termination measure; proves `while` terminates (Phase 5, #628).
    Decreases,
    /// `forall` — universal quantifier in ghost/contract predicates (Phase 5, #628).
    Forall,
    /// `exists` — existential quantifier in ghost/contract predicates (Phase 5, #628).
    Exists,

    // ── Security labels ───────────────────────────────────────────────────
    Public,
    Tainted,
    Secret,
    Clean,

    // ── Capability markers ────────────────────────────────────────────────
    Iso,
    Val,
    Ref,
    Tag,

    // ── Boolean literals (keyword form) ───────────────────────────────────
    True,
    False,

    // ── Identifiers ───────────────────────────────────────────────────────
    Ident(String),

    // ── Literals ──────────────────────────────────────────────────────────
    Integer(i64),
    Float(f64),
    Str(String),
    /// `"""…"""` — multiline string (escape sequences processed).
    MultilineStr(String),
    /// `r"…"` — raw single-line string (no escape processing).
    RawStr(String),
    /// `r"""…"""` — raw multiline string (no escape processing).
    RawMultilineStr(String),
    Char(char),

    // ── Operators ─────────────────────────────────────────────────────────
    Plus,       // +
    Minus,      // -
    Star,       // *
    Slash,      // /
    Percent,    // %
    Eq,         // =
    EqEq,       // ==
    BangEq,     // !=
    Lt,         // <
    Gt,         // >
    LtEq,       // <=
    GtEq,       // >=
    AmpAmp,     // &&
    PipePipe,   // ||
    Bang,       // !
    Question,   // ?
    Dot,        // .
    ColonColon, // ::
    Arrow,      // ->
    FatArrow,   // =>
    Pipe,       // |
    Amp,        // &
    Caret,      // ^
    Tilde,      // ~
    LtLt,       // <<
    GtGt,       // >>
    Colon,      // :
    Semicolon,  // ;
    Comma,      // ,
    Underscore, // _

    // ── Delimiters ────────────────────────────────────────────────────────
    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]

    // ── Sentinel ──────────────────────────────────────────────────────────
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenKind::Fn => write!(f, "fn"),
            TokenKind::Let => write!(f, "let"),
            TokenKind::Mut => write!(f, "mut"),
            TokenKind::Match => write!(f, "match"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::For => write!(f, "for"),
            TokenKind::While => write!(f, "while"),
            TokenKind::Type => write!(f, "type"),
            TokenKind::Total => write!(f, "total"),
            TokenKind::Partial => write!(f, "partial"),
            TokenKind::Return => write!(f, "return"),
            TokenKind::Move => write!(f, "move"),
            TokenKind::Consume => write!(f, "consume"),
            TokenKind::Declassify => write!(f, "declassify"),
            TokenKind::Sanitize => write!(f, "sanitize"),
            TokenKind::Const => write!(f, "const"),
            TokenKind::Where => write!(f, "where"),
            TokenKind::In => write!(f, "in"),
            TokenKind::Extern => write!(f, "extern"),
            TokenKind::Struct => write!(f, "struct"),
            TokenKind::Enum => write!(f, "enum"),
            TokenKind::Pub => write!(f, "pub"),
            TokenKind::Use => write!(f, "use"),
            TokenKind::Test => write!(f, "test"),
            TokenKind::Impl => write!(f, "impl"),
            TokenKind::Builtin => write!(f, "builtin"),
            TokenKind::Transparent => write!(f, "transparent"),
            TokenKind::Requires => write!(f, "requires"),
            TokenKind::Ensures => write!(f, "ensures"),
            TokenKind::Invariant => write!(f, "invariant"),
            TokenKind::Ghost => write!(f, "ghost"),
            TokenKind::Decreases => write!(f, "decreases"),
            TokenKind::Forall => write!(f, "forall"),
            TokenKind::Exists => write!(f, "exists"),
            TokenKind::Public => write!(f, "Public"),
            TokenKind::Tainted => write!(f, "Tainted"),
            TokenKind::Secret => write!(f, "Secret"),
            TokenKind::Clean => write!(f, "Clean"),
            TokenKind::Iso => write!(f, "iso"),
            TokenKind::Val => write!(f, "val"),
            TokenKind::Ref => write!(f, "ref"),
            TokenKind::Tag => write!(f, "tag"),
            TokenKind::True => write!(f, "true"),
            TokenKind::False => write!(f, "false"),
            TokenKind::Ident(s) => write!(f, "{}", s),
            TokenKind::Integer(n) => write!(f, "{}", n),
            TokenKind::Float(v) => write!(f, "{}", v),
            TokenKind::Str(s) => write!(f, "\"{}\"", s),
            TokenKind::MultilineStr(s) => write!(f, "\"\"\"{}\"\"\"", s),
            TokenKind::RawStr(s) => write!(f, "r\"{}\"", s),
            TokenKind::RawMultilineStr(s) => write!(f, "r\"\"\"{}\"\"\"", s),
            TokenKind::Char(c) => write!(f, "'{}'", c),
            TokenKind::Plus => write!(f, "+"),
            TokenKind::Minus => write!(f, "-"),
            TokenKind::Star => write!(f, "*"),
            TokenKind::Slash => write!(f, "/"),
            TokenKind::Percent => write!(f, "%"),
            TokenKind::Eq => write!(f, "="),
            TokenKind::EqEq => write!(f, "=="),
            TokenKind::BangEq => write!(f, "!="),
            TokenKind::Lt => write!(f, "<"),
            TokenKind::Gt => write!(f, ">"),
            TokenKind::LtEq => write!(f, "<="),
            TokenKind::GtEq => write!(f, ">="),
            TokenKind::AmpAmp => write!(f, "&&"),
            TokenKind::PipePipe => write!(f, "||"),
            TokenKind::Bang => write!(f, "!"),
            TokenKind::Question => write!(f, "?"),
            TokenKind::Dot => write!(f, "."),
            TokenKind::ColonColon => write!(f, "::"),
            TokenKind::Arrow => write!(f, "->"),
            TokenKind::FatArrow => write!(f, "=>"),
            TokenKind::Pipe => write!(f, "|"),
            TokenKind::Amp => write!(f, "&"),
            TokenKind::Caret => write!(f, "^"),
            TokenKind::Tilde => write!(f, "~"),
            TokenKind::LtLt => write!(f, "<<"),
            TokenKind::GtGt => write!(f, ">>"),
            TokenKind::Colon => write!(f, ":"),
            TokenKind::Semicolon => write!(f, ";"),
            TokenKind::Comma => write!(f, ","),
            TokenKind::Underscore => write!(f, "_"),
            TokenKind::LParen => write!(f, "("),
            TokenKind::RParen => write!(f, ")"),
            TokenKind::LBrace => write!(f, "{{"),
            TokenKind::RBrace => write!(f, "}}"),
            TokenKind::LBracket => write!(f, "["),
            TokenKind::RBracket => write!(f, "]"),
            TokenKind::Eof => write!(f, "<eof>"),
        }
    }
}

// ── Token ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Token { kind, span }
    }
}

// ── Lex error ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error at {}: {}", self.span, self.message)
    }
}

// ── Lexer ──────────────────────────────────────────────────────────────────

/// Hand-written lexer for MVL source text.
///
/// ```
/// use mvl::mvl::parser::lexer::{Lexer, TokenKind};
///
/// let (tokens, errors) = Lexer::new("fn add(a: Int) -> Int { a }").tokenize();
/// assert!(errors.is_empty());
/// assert_eq!(tokens[0].kind, TokenKind::Fn);
/// ```
pub struct Lexer<'src> {
    src: &'src str,
    /// Iterator yielding `(byte_offset, char)`.
    chars: std::str::CharIndices<'src>,
    /// The character currently at the read head (not yet consumed).
    current: Option<(usize, char)>,
    line: u32,
    col: u32,
    errors: Vec<LexError>,
}

impl<'src> Lexer<'src> {
    pub fn new(src: &'src str) -> Self {
        let mut chars = src.char_indices();
        let current = chars.next();
        Lexer {
            src,
            chars,
            current,
            line: 1,
            col: 1,
            errors: Vec::new(),
        }
    }

    /// Tokenize the entire source.  Returns `(tokens, lex_errors)`.
    /// The last token is always `Eof`.
    pub fn tokenize(mut self) -> (Vec<Token>, Vec<LexError>) {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let done = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if done {
                break;
            }
        }
        (tokens, self.errors)
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    fn peek_char(&self) -> Option<char> {
        self.current.map(|(_, c)| c)
    }

    fn peek_offset(&self) -> usize {
        self.current.map_or(self.src.len(), |(i, _)| i)
    }

    /// Peek at the character *after* the current one without advancing.
    fn peek_second(&self) -> Option<char> {
        self.current.and_then(|(i, _)| self.src[i..].chars().nth(1))
    }

    /// Consume the current character and advance.
    fn advance(&mut self) -> Option<char> {
        let result = self.current.map(|(_, c)| c);
        if let Some((_, ch)) = self.current {
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        self.current = self.chars.next();
        result
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek_char() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.advance();
                }
                Some('/') if self.peek_second() == Some('/') => {
                    // Line comment — skip to end of line
                    while self.peek_char().is_some() && self.peek_char() != Some('\n') {
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    fn make_span(&self, start_offset: usize, start_line: u32, start_col: u32) -> Span {
        let end = self.peek_offset();
        // Fix #3: guard against silent truncation on hypothetical very large files
        debug_assert!(
            start_offset <= u32::MAX as usize,
            "source offset {} exceeds u32::MAX",
            start_offset
        );
        Span::new(
            start_line,
            start_col,
            start_offset as u32,
            (end - start_offset) as u32,
        )
    }

    // ── Token dispatch ────────────────────────────────────────────────────

    /// Produce the next token.  Unknown characters are recorded as lex errors
    /// and skipped iteratively (no recursion, no stack growth).
    pub fn next_token(&mut self) -> Token {
        // Fix #1: iterative loop instead of recursive call for unknown chars.
        loop {
            self.skip_whitespace_and_comments();

            let start_line = self.line;
            let start_col = self.col;
            let start_offset = self.peek_offset();

            let ch = match self.advance() {
                None => {
                    return Token::new(
                        TokenKind::Eof,
                        Span::new(start_line, start_col, start_offset as u32, 0),
                    );
                }
                Some(c) => c,
            };

            let kind = match ch {
                // ── Single-character tokens ───────────────────────────────
                '+' => TokenKind::Plus,
                '*' => TokenKind::Star,
                '/' => TokenKind::Slash,
                '%' => TokenKind::Percent,
                ';' => TokenKind::Semicolon,
                ',' => TokenKind::Comma,
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                '[' => TokenKind::LBracket,
                ']' => TokenKind::RBracket,
                '?' => TokenKind::Question,
                '.' => TokenKind::Dot,

                // ── One-or-two character tokens ───────────────────────────
                '|' => {
                    if self.peek_char() == Some('|') {
                        self.advance();
                        TokenKind::PipePipe
                    } else {
                        TokenKind::Pipe
                    }
                }
                '&' => {
                    if self.peek_char() == Some('&') {
                        self.advance();
                        TokenKind::AmpAmp
                    } else {
                        TokenKind::Amp
                    }
                }
                '!' => {
                    if self.peek_char() == Some('=') {
                        self.advance();
                        TokenKind::BangEq
                    } else {
                        TokenKind::Bang
                    }
                }
                '=' => {
                    if self.peek_char() == Some('=') {
                        self.advance();
                        TokenKind::EqEq
                    } else if self.peek_char() == Some('>') {
                        self.advance();
                        TokenKind::FatArrow
                    } else {
                        TokenKind::Eq
                    }
                }
                '<' => {
                    if self.peek_char() == Some('<') {
                        self.advance();
                        TokenKind::LtLt
                    } else if self.peek_char() == Some('=') {
                        self.advance();
                        TokenKind::LtEq
                    } else {
                        TokenKind::Lt
                    }
                }
                '>' => {
                    if self.peek_char() == Some('>') {
                        self.advance();
                        TokenKind::GtGt
                    } else if self.peek_char() == Some('=') {
                        self.advance();
                        TokenKind::GtEq
                    } else {
                        TokenKind::Gt
                    }
                }
                '^' => TokenKind::Caret,
                '~' => TokenKind::Tilde,
                '-' => {
                    if self.peek_char() == Some('>') {
                        self.advance();
                        TokenKind::Arrow
                    } else {
                        TokenKind::Minus
                    }
                }
                ':' => {
                    if self.peek_char() == Some(':') {
                        self.advance();
                        TokenKind::ColonColon
                    } else {
                        TokenKind::Colon
                    }
                }

                // ── String literal ────────────────────────────────────────
                '"' => {
                    // Detect `"""` multiline string
                    if self.peek_char() == Some('"') {
                        self.advance(); // consume second `"`
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume third `"`
                            self.lex_multiline_string(start_line, start_col, start_offset)
                        } else {
                            // `""` — empty string (two quotes consumed)
                            TokenKind::Str(String::new())
                        }
                    } else {
                        self.lex_string(start_line, start_col, start_offset)
                    }
                }

                // ── Char literal ──────────────────────────────────────────
                '\'' => self.lex_char(start_line, start_col, start_offset),

                // ── Numeric literal ───────────────────────────────────────
                c if c.is_ascii_digit() => self.lex_number(c, start_line, start_col, start_offset),

                // ── Identifier or keyword (including `_foo`) ──────────────
                c if c.is_alphabetic() || c == '_' => {
                    let mut s = String::from(c);
                    while self
                        .peek_char()
                        .is_some_and(|nc| nc.is_alphanumeric() || nc == '_')
                    {
                        s.push(self.advance().unwrap());
                    }
                    // Single bare `_` is the wildcard pattern, not an ident
                    if s == "_" {
                        TokenKind::Underscore
                    } else if s == "r" && self.peek_char() == Some('"') {
                        // Raw string prefix: r"…" or r"""…"""
                        self.advance(); // consume opening `"`
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume second `"`
                            if self.peek_char() == Some('"') {
                                self.advance(); // consume third `"`
                                self.lex_raw_multiline_string(start_line, start_col, start_offset)
                            } else {
                                // r"" — empty raw string
                                TokenKind::RawStr(String::new())
                            }
                        } else {
                            self.lex_raw_string(start_line, start_col, start_offset)
                        }
                    } else {
                        keyword_or_ident(s)
                    }
                }

                // ── Unknown character ─────────────────────────────────────
                c => {
                    let span = Span::new(start_line, start_col, start_offset as u32, 1);
                    self.errors.push(LexError {
                        message: format!("unexpected character '{}'", c),
                        span,
                    });
                    // Fix #1: continue the loop instead of recursing, preventing
                    // stack overflow on inputs with many consecutive unknown chars.
                    continue;
                }
            };

            let span = self.make_span(start_offset, start_line, start_col);
            return Token::new(kind, span);
        } // end loop
    }

    // ── Literal helpers ───────────────────────────────────────────────────

    fn lex_string(&mut self, start_line: u32, start_col: u32, start_offset: usize) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 1),
                    });
                    break;
                }
                Some('"') => break,
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some(c) => s.push(c),
                    None => break,
                },
                Some(c) => s.push(c),
            }
        }
        TokenKind::Str(s)
    }

    /// Lex `"""…"""` multiline string (escape sequences processed, literal newlines preserved).
    fn lex_multiline_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated multiline string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 3),
                    });
                    break;
                }
                Some('"') => {
                    if self.peek_char() == Some('"') {
                        self.advance();
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume closing third `"`
                            break;
                        } else {
                            s.push('"');
                            s.push('"');
                        }
                    } else {
                        s.push('"');
                    }
                }
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some('0') => s.push('\0'),
                    Some(c) => s.push(c),
                    None => break,
                },
                Some(c) => s.push(c),
            }
        }
        TokenKind::MultilineStr(s)
    }

    /// Lex `r"…"` raw single-line string (no escape processing).
    fn lex_raw_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated raw string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 2),
                    });
                    break;
                }
                Some('"') => break,
                Some(c) => s.push(c),
            }
        }
        TokenKind::RawStr(s)
    }

    /// Lex `r"""…"""` raw multiline string (no escape processing).
    fn lex_raw_multiline_string(
        &mut self,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    self.errors.push(LexError {
                        message: "unterminated raw multiline string literal".into(),
                        span: Span::new(start_line, start_col, start_offset as u32, 4),
                    });
                    break;
                }
                Some('"') => {
                    if self.peek_char() == Some('"') {
                        self.advance();
                        if self.peek_char() == Some('"') {
                            self.advance(); // consume closing third `"`
                            break;
                        } else {
                            s.push('"');
                            s.push('"');
                        }
                    } else {
                        s.push('"');
                    }
                }
                Some(c) => s.push(c),
            }
        }
        TokenKind::RawMultilineStr(s)
    }

    fn lex_char(&mut self, start_line: u32, start_col: u32, start_offset: usize) -> TokenKind {
        let c = match self.advance() {
            None => '\0',
            Some('\\') => match self.advance() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('\\') => '\\',
                Some('\'') => '\'',
                Some('0') => '\0',
                Some(c) => c,
                None => '\0',
            },
            Some(c) => c,
        };
        if self.peek_char() == Some('\'') {
            self.advance();
        } else {
            self.errors.push(LexError {
                message: "unterminated character literal".into(),
                span: Span::new(start_line, start_col, start_offset as u32, 1),
            });
        }
        TokenKind::Char(c)
    }

    fn lex_number(
        &mut self,
        first: char,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        // Handle base prefixes: 0x (hex), 0b (binary), 0o (octal)
        if first == '0' {
            match self.peek_char() {
                Some('x') | Some('X') => {
                    self.advance(); // consume 'x'
                    return self.lex_integer_base(16, start_line, start_col, start_offset);
                }
                Some('b') | Some('B') => {
                    self.advance(); // consume 'b'
                    return self.lex_integer_base(2, start_line, start_col, start_offset);
                }
                Some('o') | Some('O') => {
                    self.advance(); // consume 'o'
                    return self.lex_integer_base(8, start_line, start_col, start_offset);
                }
                _ => {}
            }
        }

        let mut s = String::from(first);
        let mut is_float = false;

        loop {
            match self.peek_char() {
                Some(c) if c.is_ascii_digit() => {
                    s.push(c);
                    self.advance();
                }
                Some('.') if !is_float => {
                    // Only treat as float if the character after '.' is a digit
                    if self.peek_second().is_some_and(|c| c.is_ascii_digit()) {
                        is_float = true;
                        s.push('.');
                        self.advance(); // consume '.'
                    } else {
                        break;
                    }
                }
                // Scientific notation: 1.5e10, 2e-3, 1E+4
                Some('e') | Some('E') => {
                    is_float = true;
                    s.push('e');
                    self.advance(); // consume 'e'/'E'
                                    // Optional sign
                    if matches!(self.peek_char(), Some('+') | Some('-')) {
                        s.push(self.peek_char().unwrap());
                        self.advance();
                    }
                    // Exponent digits
                    while self.peek_char().is_some_and(|c| c.is_ascii_digit()) {
                        s.push(self.peek_char().unwrap());
                        self.advance();
                    }
                    break;
                }
                _ => break,
            }
        }

        if is_float {
            match s.parse::<f64>() {
                Ok(f) => TokenKind::Float(f),
                Err(_) => {
                    self.errors.push(LexError {
                        message: format!("invalid float literal `{s}`"),
                        span: Span::new(start_line, start_col, start_offset as u32, s.len() as u32),
                    });
                    TokenKind::Float(0.0)
                }
            }
        } else {
            // Fix #3: report overflow instead of silently producing 0
            match s.parse::<i64>() {
                Ok(n) => TokenKind::Integer(n),
                Err(_) => {
                    self.errors.push(LexError {
                        message: format!(
                            "integer literal `{}` overflows i64; value is too large",
                            s
                        ),
                        span: Span::new(start_line, start_col, start_offset as u32, s.len() as u32),
                    });
                    TokenKind::Integer(0)
                }
            }
        }
    }

    /// Scan digits for a non-decimal integer literal (hex/binary/octal).
    fn lex_integer_base(
        &mut self,
        radix: u32,
        start_line: u32,
        start_col: u32,
        start_offset: usize,
    ) -> TokenKind {
        let valid_digit = |c: char| c.is_digit(radix);
        let mut s = String::new();
        while self.peek_char().is_some_and(|c| valid_digit(c) || c == '_') {
            let c = self.peek_char().unwrap();
            self.advance();
            if c != '_' {
                s.push(c);
            }
        }
        if s.is_empty() {
            self.errors.push(LexError {
                message: "empty integer literal (no digits after base prefix)".to_string(),
                span: Span::new(start_line, start_col, start_offset as u32, 2),
            });
            return TokenKind::Integer(0);
        }
        match i64::from_str_radix(&s, radix) {
            Ok(n) => TokenKind::Integer(n),
            Err(_) => {
                self.errors.push(LexError {
                    message: "integer literal overflows i64".to_string(),
                    span: Span::new(
                        start_line,
                        start_col,
                        start_offset as u32,
                        s.len() as u32 + 2,
                    ),
                });
                TokenKind::Integer(0)
            }
        }
    }
}

// ── Keyword table ──────────────────────────────────────────────────────────

fn keyword_or_ident(s: String) -> TokenKind {
    match s.as_str() {
        // Declaration keywords
        "fn" => TokenKind::Fn,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "match" => TokenKind::Match,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "for" => TokenKind::For,
        "while" => TokenKind::While,
        "type" => TokenKind::Type,
        "total" => TokenKind::Total,
        "partial" => TokenKind::Partial,
        "return" => TokenKind::Return,
        "move" => TokenKind::Move,
        "consume" => TokenKind::Consume,
        "declassify" => TokenKind::Declassify,
        "sanitize" => TokenKind::Sanitize,
        "const" => TokenKind::Const,
        "where" => TokenKind::Where,
        "in" => TokenKind::In,
        "extern" => TokenKind::Extern,
        "struct" => TokenKind::Struct,
        "enum" => TokenKind::Enum,
        "pub" => TokenKind::Pub,
        "use" => TokenKind::Use,
        "test" => TokenKind::Test,
        "impl" => TokenKind::Impl,
        "builtin" => TokenKind::Builtin,
        "transparent" => TokenKind::Transparent,
        "requires" => TokenKind::Requires,
        "ensures" => TokenKind::Ensures,
        "invariant" => TokenKind::Invariant,
        "ghost" => TokenKind::Ghost,
        "decreases" => TokenKind::Decreases,
        "forall" => TokenKind::Forall,
        "exists" => TokenKind::Exists,
        // Boolean literals
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        // Security labels (capitalized)
        "Public" => TokenKind::Public,
        "Tainted" => TokenKind::Tainted,
        "Secret" => TokenKind::Secret,
        "Clean" => TokenKind::Clean,
        // Capability markers
        "iso" => TokenKind::Iso,
        "val" => TokenKind::Val,
        "ref" => TokenKind::Ref,
        "tag" => TokenKind::Tag,
        // Everything else is an identifier
        _ => TokenKind::Ident(s),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(src: &str) -> Vec<TokenKind> {
        let (tokens, errors) = Lexer::new(src).tokenize();
        assert!(errors.is_empty(), "unexpected lex errors: {:?}", errors);
        tokens.into_iter().map(|t| t.kind).collect()
    }

    fn lex_kinds_no_eof(src: &str) -> Vec<TokenKind> {
        let mut kinds = lex(src);
        assert_eq!(kinds.last(), Some(&TokenKind::Eof));
        kinds.pop();
        kinds
    }

    // ── Requirement 1 / Scenario: Tokenize keywords ───────────────────────

    #[test]
    fn tokenize_declaration_keywords() {
        let src = "fn let mut match if else for type total partial return";
        let kinds = lex_kinds_no_eof(src);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Fn,
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::Match,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::For,
                TokenKind::Type,
                TokenKind::Total,
                TokenKind::Partial,
                TokenKind::Return,
            ]
        );
    }

    #[test]
    fn tokenize_extra_keywords() {
        let kinds = lex_kinds_no_eof("move consume declassify sanitize const where in while");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Move,
                TokenKind::Consume,
                TokenKind::Declassify,
                TokenKind::Sanitize,
                TokenKind::Const,
                TokenKind::Where,
                TokenKind::In,
                TokenKind::While,
            ]
        );
    }

    // ── Requirement 1 / Scenario: Tokenize operators ─────────────────────

    #[test]
    fn tokenize_operators() {
        let src = "+ - * / % = == != < > <= >= && || ! ? . :: -> => | & : ;";
        let kinds = lex_kinds_no_eof(src);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Eq,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::Bang,
                TokenKind::Question,
                TokenKind::Dot,
                TokenKind::ColonColon,
                TokenKind::Arrow,
                TokenKind::FatArrow,
                TokenKind::Pipe,
                TokenKind::Amp,
                TokenKind::Colon,
                TokenKind::Semicolon,
            ]
        );
    }

    #[test]
    fn tokenize_delimiters() {
        let kinds = lex_kinds_no_eof("( ) { } [ ]");
        assert_eq!(
            kinds,
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
            ]
        );
    }

    // ── Requirement 1 / Scenario: Tokenize security labels ───────────────

    #[test]
    fn tokenize_security_labels_and_capabilities() {
        let src = "Public Tainted Secret Clean iso val ref tag";
        let kinds = lex_kinds_no_eof(src);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Public,
                TokenKind::Tainted,
                TokenKind::Secret,
                TokenKind::Clean,
                TokenKind::Iso,
                TokenKind::Val,
                TokenKind::Ref,
                TokenKind::Tag,
            ],
            "must produce exactly 8 keyword tokens"
        );
    }

    // ── Requirement 1 / Scenario: Tokenize literals ───────────────────────

    #[test]
    fn tokenize_integer_literal() {
        let kinds = lex_kinds_no_eof("42");
        assert_eq!(kinds, vec![TokenKind::Integer(42)]);
    }

    #[test]
    fn tokenize_float_literal() {
        let kinds = lex_kinds_no_eof("3.14");
        assert_eq!(kinds, vec![TokenKind::Float(3.14)]);
    }

    #[test]
    fn tokenize_string_literal() {
        let kinds = lex_kinds_no_eof(r#""hello""#);
        assert_eq!(kinds, vec![TokenKind::Str("hello".into())]);
    }

    #[test]
    fn tokenize_char_literal() {
        let kinds = lex_kinds_no_eof("'c'");
        assert_eq!(kinds, vec![TokenKind::Char('c')]);
    }

    #[test]
    fn tokenize_bool_literals() {
        let kinds = lex_kinds_no_eof("true false");
        assert_eq!(kinds, vec![TokenKind::True, TokenKind::False]);
    }

    #[test]
    fn tokenize_all_literal_kinds() {
        // GIVEN: 42 3.14 "hello" 'c' true false
        // THEN: INTEGER FLOAT STRING CHAR BOOL BOOL
        let kinds = lex_kinds_no_eof(r#"42 3.14 "hello" 'c' true false"#);
        assert_eq!(
            kinds,
            vec![
                TokenKind::Integer(42),
                TokenKind::Float(3.14),
                TokenKind::Str("hello".into()),
                TokenKind::Char('c'),
                TokenKind::True,
                TokenKind::False,
            ]
        );
    }

    #[test]
    fn integer_followed_by_dot_method_call() {
        // `42.to_string()` — the `.` should NOT be consumed into the number
        let kinds = lex_kinds_no_eof("42.to_string()");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Integer(42),
                TokenKind::Dot,
                TokenKind::Ident("to_string".into()),
                TokenKind::LParen,
                TokenKind::RParen,
            ]
        );
    }

    // ── Requirement 1 / Scenario: Source locations ────────────────────────

    #[test]
    fn source_locations_single_line() {
        let (tokens, _) = Lexer::new("fn foo").tokenize();
        assert_eq!(tokens[0].span.line, 1);
        assert_eq!(tokens[0].span.col, 1);
        assert_eq!(tokens[1].span.line, 1);
        assert_eq!(tokens[1].span.col, 4);
    }

    #[test]
    fn source_locations_multiline() {
        let src = "fn\nlet\nmut";
        let (tokens, _) = Lexer::new(src).tokenize();
        assert_eq!(tokens[0].span, Span::new(1, 1, 0, 2));
        assert_eq!(tokens[1].span, Span::new(2, 1, 3, 3));
        assert_eq!(tokens[2].span, Span::new(3, 1, 7, 3));
    }

    #[test]
    fn eof_token_always_present() {
        let (tokens, _) = Lexer::new("").tokenize();
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    // ── Line comments ─────────────────────────────────────────────────────

    #[test]
    fn line_comments_skipped() {
        let src = "fn // this is a comment\nlet";
        let kinds = lex_kinds_no_eof(src);
        assert_eq!(kinds, vec![TokenKind::Fn, TokenKind::Let]);
    }

    // ── Wildcard ─────────────────────────────────────────────────────────

    #[test]
    fn underscore_is_wildcard() {
        let kinds = lex_kinds_no_eof("_");
        assert_eq!(kinds, vec![TokenKind::Underscore]);
    }

    #[test]
    fn underscore_prefix_is_ident() {
        let kinds = lex_kinds_no_eof("_foo");
        assert_eq!(kinds, vec![TokenKind::Ident("_foo".into())]);
    }

    // ── Error recovery ────────────────────────────────────────────────────

    #[test]
    fn unknown_char_produces_error_and_continues() {
        let (tokens, errors) = Lexer::new("fn @ let").tokenize();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains('@'));
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(kinds.contains(&&TokenKind::Fn));
        assert!(kinds.contains(&&TokenKind::Let));
    }

    // Fix #1: many unknown chars must not overflow the stack
    #[test]
    fn many_unknown_chars_no_stack_overflow() {
        let src = "@".repeat(10_000);
        let (tokens, errors) = Lexer::new(&src).tokenize();
        assert_eq!(errors.len(), 10_000);
        assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);
    }

    // Fix #3: integer overflow produces an error, not silent 0
    #[test]
    fn integer_overflow_produces_lex_error() {
        let src = "99999999999999999999999"; // too big for i64
        let (tokens, errors) = Lexer::new(src).tokenize();
        assert_eq!(errors.len(), 1, "expected one overflow error");
        assert!(
            errors[0].message.contains("overflows"),
            "got: {}",
            errors[0].message
        );
        // Still produces a token (value 0) for error-recovery
        assert!(matches!(tokens[0].kind, TokenKind::Integer(0)));
    }

    // Fix #5: struct and enum are reserved keywords, not plain identifiers
    #[test]
    fn struct_and_enum_are_keywords() {
        let kinds = lex_kinds_no_eof("struct enum");
        assert_eq!(kinds, vec![TokenKind::Struct, TokenKind::Enum]);
    }

    // ── Escape sequences ──────────────────────────────────────────────────

    #[test]
    fn string_escape_sequences() {
        let kinds = lex_kinds_no_eof(r#""\n\t\\\"" "#);
        assert_eq!(kinds, vec![TokenKind::Str("\n\t\\\"".into())]);
    }

    // ── Number literal formats (issue #65) ────────────────────────────────

    #[test]
    fn tokenize_hex_literal() {
        assert_eq!(lex_kinds_no_eof("0xFF"), vec![TokenKind::Integer(255)]);
        assert_eq!(lex_kinds_no_eof("0xDEAD"), vec![TokenKind::Integer(0xDEAD)]);
        assert_eq!(lex_kinds_no_eof("0x0"), vec![TokenKind::Integer(0)]);
    }

    #[test]
    fn tokenize_binary_literal() {
        assert_eq!(lex_kinds_no_eof("0b1010"), vec![TokenKind::Integer(10)]);
        assert_eq!(
            lex_kinds_no_eof("0b11111111"),
            vec![TokenKind::Integer(255)]
        );
        assert_eq!(lex_kinds_no_eof("0b0"), vec![TokenKind::Integer(0)]);
    }

    #[test]
    fn tokenize_octal_literal() {
        assert_eq!(lex_kinds_no_eof("0o77"), vec![TokenKind::Integer(63)]);
        assert_eq!(lex_kinds_no_eof("0o755"), vec![TokenKind::Integer(0o755)]);
        assert_eq!(lex_kinds_no_eof("0o0"), vec![TokenKind::Integer(0)]);
    }

    #[test]
    fn tokenize_scientific_notation() {
        let kinds = lex_kinds_no_eof("1.5e10");
        assert_eq!(kinds, vec![TokenKind::Float(1.5e10)]);

        let kinds = lex_kinds_no_eof("2e3");
        assert_eq!(kinds, vec![TokenKind::Float(2e3)]);

        let kinds = lex_kinds_no_eof("1.0E-2");
        assert_eq!(kinds, vec![TokenKind::Float(1.0e-2)]);
    }

    #[test]
    fn tokenize_impl_keyword() {
        let kinds = lex_kinds_no_eof("impl");
        assert_eq!(kinds, vec![TokenKind::Impl]);
    }

    // ── Multiline and raw strings ──────────────────────────────────────────

    #[test]
    fn tokenize_multiline_string() {
        let kinds = lex_kinds_no_eof("\"\"\"hello\nworld\"\"\"");
        assert_eq!(kinds, vec![TokenKind::MultilineStr("hello\nworld".into())]);
    }

    #[test]
    fn tokenize_multiline_string_with_escapes() {
        let kinds = lex_kinds_no_eof("\"\"\"tab:\\there\"\"\"");
        assert_eq!(kinds, vec![TokenKind::MultilineStr("tab:\there".into())]);
    }

    #[test]
    fn tokenize_empty_string_two_quotes() {
        // `""` is an empty string (two adjacent double-quotes)
        let kinds = lex_kinds_no_eof("\"\"");
        assert_eq!(kinds, vec![TokenKind::Str(String::new())]);
    }

    #[test]
    fn tokenize_raw_string() {
        let kinds = lex_kinds_no_eof(r#"r"C:\path""#);
        assert_eq!(kinds, vec![TokenKind::RawStr(r"C:\path".into())]);
    }

    #[test]
    fn tokenize_raw_string_backslash_not_escape() {
        // In raw strings `\n` is two chars, not a newline
        let kinds = lex_kinds_no_eof(r#"r"\n""#);
        assert_eq!(kinds, vec![TokenKind::RawStr(r"\n".into())]);
    }

    #[test]
    fn tokenize_raw_string_empty() {
        let kinds = lex_kinds_no_eof(r#"r"""#);
        // r"" → empty raw string
        assert_eq!(kinds, vec![TokenKind::RawStr(String::new())]);
    }

    #[test]
    fn tokenize_raw_multiline_string() {
        let src = "r\"\"\"line1\nline2\"\"\"";
        let kinds = lex_kinds_no_eof(src);
        assert_eq!(
            kinds,
            vec![TokenKind::RawMultilineStr("line1\nline2".into())]
        );
    }
}
