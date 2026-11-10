//! Nalu lexer (SPEC-001).
//!
//! - `NL` is significant (one statement per line). Line breaks are dropped after operators, `,`, `(`, `[`,
//!   `{` and when the next line starts with `.` (`xs\n    .filter(...)`); several breaks collapse into one.
//! - `"…"` strings keep the `{expr}` parts as raw text **with the offset in the source**; the parser
//!   re-parses each part, so errors inside `{ }` point at the right column. An empty `{}` is a literal
//!   (`fmt` templates). `'raw'` has no escapes and no interpolation. `"""` drops the first line break,
//!   the last line when it holds only spaces, and the common indentation.
//! - `$ …` enters **command mode** until the end of the line, an unquoted ` catch `, a `{` that ends the line (or, in an
//!   `if`/`while` condition, a `{ … }` block that ends the line) or `//`. `$(` captures up to the matching `)`.
//! - `///` becomes the documentation of the next token.
//! - Never panics: errors become `Diag` (`E005`/`E006`) and the lexer keeps going.

use crate::ast::RedirKind;
use crate::diag::{Diag, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kw {
    Let,
    Var,
    Fn,
    Return,
    Struct,
    Enum,
    Param,
    If,
    Else,
    For,
    In,
    While,
    Match,
    Break,
    Continue,
    Fail,
    Catch,
    Ignore,
    Try,
    Defer,
    Parallel,
    Retry,
    Native,
    Test,
    SelfKw,
    True,
    False,
}

impl Kw {
    pub fn from_str(s: &str) -> Option<Kw> {
        Some(match s {
            "let" => Kw::Let,
            "var" => Kw::Var,
            "fn" => Kw::Fn,
            "return" => Kw::Return,
            "struct" => Kw::Struct,
            "enum" => Kw::Enum,
            "param" => Kw::Param,
            "if" => Kw::If,
            "else" => Kw::Else,
            "for" => Kw::For,
            "in" => Kw::In,
            "while" => Kw::While,
            "match" => Kw::Match,
            "break" => Kw::Break,
            "continue" => Kw::Continue,
            "fail" => Kw::Fail,
            "catch" => Kw::Catch,
            "ignore" => Kw::Ignore,
            "try" => Kw::Try,
            "defer" => Kw::Defer,
            "parallel" => Kw::Parallel,
            "retry" => Kw::Retry,
            "native" => Kw::Native,
            "test" => Kw::Test,
            "self" => Kw::SelfKw,
            "true" => Kw::True,
            "false" => Kw::False,
            _ => return None,
        })
    }
    pub fn text(self) -> &'static str {
        match self {
            Kw::Let => "let",
            Kw::Var => "var",
            Kw::Fn => "fn",
            Kw::Return => "return",
            Kw::Struct => "struct",
            Kw::Enum => "enum",
            Kw::Param => "param",
            Kw::If => "if",
            Kw::Else => "else",
            Kw::For => "for",
            Kw::In => "in",
            Kw::While => "while",
            Kw::Match => "match",
            Kw::Break => "break",
            Kw::Continue => "continue",
            Kw::Fail => "fail",
            Kw::Catch => "catch",
            Kw::Ignore => "ignore",
            Kw::Try => "try",
            Kw::Defer => "defer",
            Kw::Parallel => "parallel",
            Kw::Retry => "retry",
            Kw::Native => "native",
            Kw::Test => "test",
            Kw::SelfKw => "self",
            Kw::True => "true",
            Kw::False => "false",
        }
    }
}

/// A string part: text with escapes already resolved, or the code of a `{expr}` with the (absolute) offset of the
/// first character after the `{`.
#[derive(Clone, Debug, PartialEq)]
pub enum LStrPart {
    Lit(String),
    Code { src: String, offset: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrTok {
    pub parts: Vec<LStrPart>,
    /// `'raw'`
    pub raw: bool,
}

impl StrTok {
    /// The text, if there is no interpolation.
    pub fn plain(&self) -> Option<String> {
        let mut s = String::new();
        for p in &self.parts {
            match p {
                LStrPart::Lit(t) => s.push_str(t),
                LStrPart::Code { .. } => return None,
            }
        }
        Some(s)
    }
}

/// Part of a command word.
#[derive(Clone, Debug, PartialEq)]
pub enum WPart {
    Lit(String),
    Code { src: String, offset: usize, quoted: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LWord {
    pub parts: Vec<WPart>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LRedir {
    pub kind: RedirKind,
    pub target: Option<LWord>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CmdTok {
    pub stages: Vec<Vec<LWord>>,
    pub redirects: Vec<LRedir>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Kw(Kw),
    Int(i64),
    /// Milliseconds.
    Dur(i64),
    Str(StrTok),
    Cmd(CmdTok),
    Capture(CmdTok),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    DotDot,
    DotDotEq,
    FatArrow,
    Arrow,
    Dot,
    Comma,
    Colon,
    Pipe,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    /// `?` (only for the ternary's E006)
    Question,
    /// `??`
    QQ,
    /// `++`
    PlusPlus,
    /// `--`
    MinusMinus,
    /// `;`
    Semi,
    Nl,
    Eof,
}

impl Tok {
    /// Short description for messages: "`+`", "end of line", "the name `x`".
    pub fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("the name `{}`", s),
            Tok::Kw(k) => format!("the keyword `{}`", k.text()),
            Tok::Int(n) => format!("the number `{}`", n),
            Tok::Dur(_) => "a duration".into(),
            Tok::Str(_) => "a text literal".into(),
            Tok::Cmd(_) => "a `$` command".into(),
            Tok::Capture(_) => "a `$( )` capture".into(),
            Tok::Nl => "end of line".into(),
            Tok::Eof => "end of file".into(),
            other => format!("`{}`", other.punct_text()),
        }
    }
    pub fn punct_text(&self) -> &'static str {
        match self {
            Tok::Plus => "+",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::Percent => "%",
            Tok::EqEq => "==",
            Tok::Ne => "!=",
            Tok::Lt => "<",
            Tok::Le => "<=",
            Tok::Gt => ">",
            Tok::Ge => ">=",
            Tok::AndAnd => "&&",
            Tok::OrOr => "||",
            Tok::Bang => "!",
            Tok::Assign => "=",
            Tok::PlusEq => "+=",
            Tok::MinusEq => "-=",
            Tok::StarEq => "*=",
            Tok::SlashEq => "/=",
            Tok::PercentEq => "%=",
            Tok::DotDot => "..",
            Tok::DotDotEq => "..=",
            Tok::FatArrow => "=>",
            Tok::Arrow => "->",
            Tok::Dot => ".",
            Tok::Comma => ",",
            Tok::Colon => ":",
            Tok::Pipe => "|",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::Question => "?",
            Tok::QQ => "??",
            Tok::PlusPlus => "++",
            Tok::MinusMinus => "--",
            Tok::Semi => ";",
            _ => "?",
        }
    }
    /// After these tokens, a line break does not end the statement.
    fn continues_line(&self) -> bool {
        matches!(
            self,
            Tok::Plus
                | Tok::Minus
                | Tok::Star
                | Tok::Slash
                | Tok::Percent
                | Tok::EqEq
                | Tok::Ne
                | Tok::Lt
                | Tok::Le
                | Tok::Gt
                | Tok::Ge
                | Tok::AndAnd
                | Tok::OrOr
                | Tok::Bang
                | Tok::Assign
                | Tok::PlusEq
                | Tok::MinusEq
                | Tok::StarEq
                | Tok::SlashEq
                | Tok::PercentEq
                | Tok::DotDot
                | Tok::DotDotEq
                | Tok::FatArrow
                | Tok::Arrow
                | Tok::Dot
                | Tok::Comma
                | Tok::Colon
                | Tok::Pipe
                | Tok::LParen
                | Tok::LBracket
                | Tok::LBrace
                | Tok::Kw(Kw::Catch)
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// `///` documentation right above (only filled on the first token after it).
    pub doc: Option<String>,
}

/// Lexes `src`. `base` is the offset of `src` within the file (for re-lexing `{expr}`).
/// The list always ends with `Nl` and `Eof`.
pub fn lex(src: &str, base: usize) -> (Vec<Token>, Vec<Diag>) {
    let mut lx = Lexer { src, b: src.as_bytes(), pos: 0, base, toks: Vec::new(), diags: Vec::new(), doc: None, paren: 0 };
    lx.run();
    (lx.toks, lx.diags)
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    base: usize,
    toks: Vec<Token>,
    diags: Vec<Diag>,
    doc: Option<String>,
    /// Open `(` (outside commands): a `$ cmd` inside `( … )` ends at the closing `)`.
    paren: i32,
}

const MAX_NEST: usize = 64;

impl<'a> Lexer<'a> {
    fn sp(&self, s: usize, e: usize) -> Span {
        Span::new(self.base + s, self.base + e)
    }
    fn err(&mut self, s: usize, e: usize, msg: impl Into<String>) -> &mut Diag {
        let d = Diag::error("E005", self.sp(s, e.max(s + 1).min(self.b.len().max(s + 1))), msg);
        self.diags.push(d);
        self.diags.last_mut().unwrap()
    }
    fn peek(&self) -> u8 {
        *self.b.get(self.pos).unwrap_or(&0)
    }
    fn peek_at(&self, i: usize) -> u8 {
        *self.b.get(i).unwrap_or(&0)
    }
    fn push(&mut self, tok: Tok, s: usize, e: usize) {
        let doc = self.doc.take();
        self.toks.push(Token { tok, span: self.sp(s, e), doc });
    }
    fn last_tok(&self) -> Option<&Tok> {
        self.toks.last().map(|t| &t.tok)
    }

    fn push_nl(&mut self, at: usize) {
        match self.last_tok() {
            None | Some(Tok::Nl) => return,
            Some(t) if t.continues_line() => return,
            _ => {}
        }
        // Next line starting with `.` (but not `..`): continuation.
        let mut i = at + 1;
        loop {
            while i < self.b.len() && matches!(self.b[i], b' ' | b'\t' | b'\r' | b'\n') {
                i += 1;
            }
            if i + 1 < self.b.len() && self.b[i] == b'/' && self.b[i + 1] == b'/' {
                while i < self.b.len() && self.b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            break;
        }
        if i < self.b.len() && self.b[i] == b'.' && self.peek_at(i + 1) != b'.' {
            return;
        }
        let doc = None;
        self.toks.push(Token { tok: Tok::Nl, span: self.sp(at, at + 1), doc });
    }

    fn run(&mut self) {
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            let start = self.pos;
            match c {
                b' ' | b'\t' | b'\r' => self.pos += 1,
                b'\n' => {
                    self.push_nl(self.pos);
                    self.pos += 1;
                }
                b'/' if self.peek_at(self.pos + 1) == b'/' => self.line_comment(),
                b'/' if self.peek_at(self.pos + 1) == b'*' => self.block_comment(),
                b'0'..=b'9' => self.number(),
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                    while self.pos < self.b.len() && (self.b[self.pos].is_ascii_alphanumeric() || self.b[self.pos] == b'_') {
                        self.pos += 1;
                    }
                    let word = &self.src[start..self.pos];
                    let tok = match Kw::from_str(word) {
                        Some(k) => Tok::Kw(k),
                        None => Tok::Ident(word.to_string()),
                    };
                    self.push(tok, start, self.pos);
                }
                b'"' => {
                    if self.src[self.pos..].starts_with("\"\"\"") {
                        self.triple_string();
                    } else {
                        self.dq_string();
                    }
                }
                b'\'' => self.raw_string(),
                b'$' => self.dollar(),
                _ if c >= 0x80 => {
                    let ch = self.src[self.pos..].chars().next().unwrap_or('?');
                    let len = ch.len_utf8();
                    self.err(start, start + len, format!("unexpected character `{}`", ch))
                        .help = Some("names use only ASCII letters, digits and `_`; text goes inside quotes".into());
                    self.pos += len;
                }
                _ => self.punct(),
            }
        }
        let end = self.b.len();
        self.push_nl(end);
        if !matches!(self.last_tok(), Some(Tok::Nl)) {
            self.toks.push(Token { tok: Tok::Nl, span: self.sp(end, end), doc: None });
        }
        self.toks.push(Token { tok: Tok::Eof, span: self.sp(end, end), doc: None });
    }

    fn line_comment(&mut self) {
        let start = self.pos;
        while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        if text.starts_with("///") && !text.starts_with("////") {
            let line = text[3..].trim().to_string();
            match &mut self.doc {
                Some(d) => {
                    if !line.is_empty() {
                        if !d.is_empty() {
                            d.push(' ');
                        }
                        d.push_str(&line);
                    }
                }
                None => self.doc = Some(line),
            }
        }
    }

    fn block_comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        while self.pos < self.b.len() {
            if self.b[self.pos] == b'*' && self.peek_at(self.pos + 1) == b'/' {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
        self.err(start, start + 2, "`/*` comment without the closing `*/`");
    }

    fn punct(&mut self) {
        let s = self.pos;
        let c = self.b[s];
        let n = self.peek_at(s + 1);
        let n2 = self.peek_at(s + 2);
        let (tok, len) = match (c, n) {
            (b'.', b'.') if n2 == b'=' => (Tok::DotDotEq, 3),
            (b'.', b'.') => (Tok::DotDot, 2),
            (b'.', _) => (Tok::Dot, 1),
            (b'=', b'>') => (Tok::FatArrow, 2),
            (b'=', b'=') => (Tok::EqEq, 2),
            (b'=', _) => (Tok::Assign, 1),
            (b'!', b'=') => (Tok::Ne, 2),
            (b'!', _) => (Tok::Bang, 1),
            (b'<', b'=') => (Tok::Le, 2),
            (b'<', _) => (Tok::Lt, 1),
            (b'>', b'=') => (Tok::Ge, 2),
            (b'>', _) => (Tok::Gt, 1),
            (b'&', b'&') => (Tok::AndAnd, 2),
            (b'|', b'|') => (Tok::OrOr, 2),
            (b'|', _) => (Tok::Pipe, 1),
            (b'-', b'>') => (Tok::Arrow, 2),
            (b'-', b'=') => (Tok::MinusEq, 2),
            (b'-', b'-') => (Tok::MinusMinus, 2),
            (b'-', _) => (Tok::Minus, 1),
            (b'+', b'=') => (Tok::PlusEq, 2),
            (b'+', b'+') => (Tok::PlusPlus, 2),
            (b'+', _) => (Tok::Plus, 1),
            (b'*', b'=') => (Tok::StarEq, 2),
            (b'*', _) => (Tok::Star, 1),
            (b'/', b'=') => (Tok::SlashEq, 2),
            (b'/', _) => (Tok::Slash, 1),
            (b'%', b'=') => (Tok::PercentEq, 2),
            (b'%', _) => (Tok::Percent, 1),
            (b'?', b'?') => (Tok::QQ, 2),
            (b'?', _) => (Tok::Question, 1),
            (b':', _) => (Tok::Colon, 1),
            (b',', _) => (Tok::Comma, 1),
            (b'(', _) => (Tok::LParen, 1),
            (b')', _) => (Tok::RParen, 1),
            (b'[', _) => (Tok::LBracket, 1),
            (b']', _) => (Tok::RBracket, 1),
            (b'{', _) => (Tok::LBrace, 1),
            (b'}', _) => (Tok::RBrace, 1),
            (b';', _) => (Tok::Semi, 1),
            _ => {
                let msg = if c == b'&' {
                    "a single `&` does not exist; logical \"and\" is `&&`".to_string()
                } else if c == b'\\' {
                    "`\\` outside of text".to_string()
                } else {
                    format!("unexpected character `{}`", c as char)
                };
                self.err(s, s + 1, msg);
                self.pos += 1;
                return;
            }
        };
        match tok {
            Tok::LParen => self.paren += 1,
            Tok::RParen => self.paren = (self.paren - 1).max(0),
            _ => {}
        }
        self.pos += len;
        self.push(tok, s, s + len);
    }

    // ---------------------------------------------------------------------------------------------
    // Numbers and durations
    // ---------------------------------------------------------------------------------------------

    fn digits(&mut self) -> (u128, usize, bool) {
        // returns (value, digit count, overflowed)
        let mut v: u128 = 0;
        let mut n = 0;
        let mut over = false;
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            if c.is_ascii_digit() {
                v = v * 10 + (c - b'0') as u128;
                if v > u64::MAX as u128 {
                    over = true;
                    v = u64::MAX as u128;
                }
                n += 1;
                self.pos += 1;
            } else if c == b'_' && n > 0 && self.peek_at(self.pos + 1).is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        (v, n, over)
    }

    fn number(&mut self) {
        let s = self.pos;
        if self.b[s] == b'0' && matches!(self.peek_at(s + 1), b'x' | b'X') {
            self.pos += 2;
            let mut v: u128 = 0;
            let mut n = 0;
            let mut over = false;
            while self.pos < self.b.len() {
                let c = self.b[self.pos];
                let d = match c {
                    b'0'..=b'9' => c - b'0',
                    b'a'..=b'f' => c - b'a' + 10,
                    b'A'..=b'F' => c - b'A' + 10,
                    b'_' => {
                        self.pos += 1;
                        continue;
                    }
                    _ => break,
                };
                v = v * 16 + d as u128;
                if v > i64::MAX as u128 {
                    over = true;
                    v = 0;
                }
                n += 1;
                self.pos += 1;
            }
            if n == 0 {
                self.err(s, self.pos, "hexadecimal number without digits");
            } else if over {
                self.err(s, self.pos, "number too large (integers are 64-bit)");
            }
            self.after_number(s);
            self.push(Tok::Int(v as i64), s, self.pos);
            return;
        }
        let (v, _, over) = self.digits();
        let c = self.peek();
        let frac_follows = c == b'.' && self.peek_at(self.pos + 1).is_ascii_digit();
        if c.is_ascii_alphabetic() || frac_follows {
            self.duration(s, v);
            return;
        }
        if over || v > i64::MAX as u128 + 1 {
            self.err(s, self.pos, "number too large (integers are 64-bit)");
            self.push(Tok::Int(0), s, self.pos);
            return;
        }
        // i64::MIN is only accepted with the `-` in front; here we store it saturated.
        let n = if v > i64::MAX as u128 { i64::MAX } else { v as i64 };
        self.push(Tok::Int(n), s, self.pos);
    }

    fn after_number(&mut self, s: usize) {
        if self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            let e0 = self.pos;
            while self.pos < self.b.len() && (self.b[self.pos].is_ascii_alphanumeric() || self.b[self.pos] == b'_') {
                self.pos += 1;
            }
            let _ = e0;
            self.err(s, self.pos, "malformed number");
        }
    }

    fn duration(&mut self, s: usize, first: u128) {
        const UNITS: [(&str, i64); 5] = [("d", 86_400_000), ("h", 3_600_000), ("m", 60_000), ("s", 1_000), ("ms", 1)];
        let mut total: i128 = 0;
        let mut last_rank: i32 = -1;
        let mut value = first;
        let mut ok = true;
        loop {
            // fraction (seconds only)
            let mut frac: Option<(u128, usize)> = None;
            if self.peek() == b'.' && self.peek_at(self.pos + 1).is_ascii_digit() {
                self.pos += 1;
                let (f, n, _) = self.digits();
                frac = Some((f, n));
            }
            let us = self.pos;
            while self.pos < self.b.len() && self.b[self.pos].is_ascii_alphabetic() {
                self.pos += 1;
            }
            let unit = &self.src[us..self.pos];
            let rank = UNITS.iter().position(|(u, _)| *u == unit);
            let rank = match rank {
                Some(r) => r as i32,
                None => {
                    if ok {
                        let msg = if unit.is_empty() {
                            "duration without a unit".to_string()
                        } else {
                            format!("unknown duration unit `{}`", unit)
                        };
                        self.err(s, self.pos, msg).help = Some("units: `ms`, `s`, `m`, `h`, `d` (e.g. `500ms`, `1m30s`)".into());
                    }
                    ok = false;
                    break;
                }
            };
            if rank <= last_rank && ok {
                self.err(s, self.pos, "duration units go from largest to smallest, without repeating").help =
                    Some("write, for example, `1m30s` or `2h15m`".into());
                ok = false;
            }
            last_rank = rank;
            let mult = UNITS[rank as usize].1 as i128;
            total += value as i128 * mult;
            if let Some((f, n)) = frac {
                if unit != "s" {
                    if ok {
                        self.err(s, self.pos, "only seconds can have a fraction").help = Some("write `1m30s` instead of `1.5m`".into());
                    }
                    ok = false;
                } else if n > 3 {
                    if ok {
                        self.err(s, self.pos, "fractions of a second go down to milliseconds (3 digits)");
                    }
                    ok = false;
                } else {
                    let mut ms = f as i128;
                    for _ in n..3 {
                        ms *= 10;
                    }
                    total += ms;
                }
            }
            if total > i64::MAX as i128 {
                if ok {
                    self.err(s, self.pos, "duration too large");
                }
                ok = false;
                total = 0;
            }
            if self.peek().is_ascii_digit() {
                let (v, _, _) = self.digits();
                value = v;
                continue;
            }
            break;
        }
        if ok && (self.peek().is_ascii_alphanumeric() || self.peek() == b'_') {
            self.after_number(s);
        }
        self.push(Tok::Dur(total as i64), s, self.pos);
    }

    // ---------------------------------------------------------------------------------------------
    // Strings
    // ---------------------------------------------------------------------------------------------

    fn raw_string(&mut self) {
        let s = self.pos;
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.b.len() && self.b[self.pos] != b'\'' && self.b[self.pos] != b'\n' {
            self.pos += 1;
        }
        if self.peek() != b'\'' {
            self.err(s, self.pos, "`'` text without the closing quote").help =
                Some("single-quoted text stays on one line and cannot contain `'`".into());
            let text = self.src[start..self.pos].to_string();
            self.push(Tok::Str(StrTok { parts: vec![LStrPart::Lit(text)], raw: true }), s, self.pos);
            return;
        }
        let text = self.src[start..self.pos].to_string();
        self.pos += 1;
        self.push(Tok::Str(StrTok { parts: vec![LStrPart::Lit(text)], raw: true }), s, self.pos);
    }

    /// `"…"` on one line. Returns the parts.
    fn dq_string(&mut self) {
        let s = self.pos;
        let (parts, end) = scan_dq(self.src, s, self.base, &mut self.diags);
        self.pos = end;
        self.push(Tok::Str(StrTok { parts, raw: false }), s, end);
    }

    fn triple_string(&mut self) {
        let s = self.pos;
        let body_start = s + 3;
        // find the closing `"""`
        let mut i = body_start;
        let mut end_body = None;
        while i < self.b.len() {
            let c = self.b[i];
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' && self.src[i..].starts_with("\"\"\"") {
                end_body = Some(i);
                break;
            }
            if c == b'{' {
                if self.peek_at(i + 1) == b'{' {
                    i += 2;
                    continue;
                }
                if let Some(e) = scan_code_end(self.b, i, 0) {
                    i = e + 1;
                    continue;
                }
            }
            i += 1;
        }
        let end_body = match end_body {
            Some(e) => e,
            None => {
                self.err(s, s + 3, "`\"\"\"` text without the closing `\"\"\"`");
                self.pos = self.b.len();
                self.push(Tok::Str(StrTok { parts: vec![], raw: false }), s, self.pos);
                return;
            }
        };
        // lines with offsets
        let mut lines: Vec<(usize, usize)> = Vec::new(); // [start, end) without \n
        let mut ls = body_start;
        for j in body_start..end_body {
            if self.b[j] == b'\n' {
                lines.push((ls, j));
                ls = j + 1;
            }
        }
        lines.push((ls, end_body));
        let blank = |b: &[u8], (a, e): (usize, usize)| b[a..e].iter().all(|c| matches!(c, b' ' | b'\t' | b'\r'));
        if lines.len() > 1 && blank(self.b, lines[0]) {
            lines.remove(0);
        }
        if lines.len() > 1 && blank(self.b, *lines.last().unwrap()) {
            lines.pop();
        }
        let indent = lines
            .iter()
            .filter(|l| !blank(self.b, **l))
            .map(|(a, e)| self.b[*a..*e].iter().take_while(|c| matches!(c, b' ' | b'\t')).count())
            .min()
            .unwrap_or(0);
        let mut chars: Vec<(usize, char)> = Vec::new();
        for (k, (a, e)) in lines.iter().enumerate() {
            if k > 0 {
                chars.push((a.saturating_sub(1), '\n'));
            }
            let mut a = *a;
            let mut skipped = 0;
            while a < *e && skipped < indent && matches!(self.b[a], b' ' | b'\t') {
                a += 1;
                skipped += 1;
            }
            for (off, ch) in self.src[a..*e].char_indices() {
                if ch == '\r' {
                    continue;
                }
                chars.push((a + off, ch));
            }
        }
        let parts = parts_from_chars(self.src, &chars, self.base, &mut self.diags);
        self.pos = end_body + 3;
        self.push(Tok::Str(StrTok { parts, raw: false }), s, self.pos);
    }

    // ---------------------------------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------------------------------

    fn dollar(&mut self) {
        let s = self.pos;
        let n = self.peek_at(s + 1);
        if n == b'(' {
            self.pos += 2;
            let (cmd, ok) = self.command(true, s);
            let _ = ok;
            self.push(Tok::Capture(cmd), s, self.pos);
            return;
        }
        if n != b' ' && n != b'\t' {
            self.err(s, s + 1, "`$` must be followed by a space and the command").help =
                Some("write `$ command args` (runs it) or `$(command)` (captures its output); the shell's `$X` does not exist: use `env.get(\"X\")`".into());
        }
        self.pos += 1;
        let (cmd, _) = self.command(false, s);
        self.push(Tok::Cmd(cmd), s, self.pos);
    }

    /// Are we in a condition (`if $`, `while !$`)?
    fn in_condition(&self) -> bool {
        let n = self.toks.len();
        match self.toks.last().map(|t| &t.tok) {
            Some(Tok::Kw(Kw::If)) | Some(Tok::Kw(Kw::While)) => true,
            Some(Tok::Bang) => n >= 2 && matches!(self.toks[n - 2].tok, Tok::Kw(Kw::If) | Tok::Kw(Kw::While)),
            _ => false,
        }
    }

    fn rest_of_line_blank(&self, mut i: usize) -> bool {
        while i < self.b.len() && matches!(self.b[i], b' ' | b'\t' | b'\r') {
            i += 1;
        }
        i >= self.b.len() || self.b[i] == b'\n' || (self.b[i] == b'/' && self.peek_at(i + 1) == b'/')
    }

    fn command(&mut self, capture: bool, dollar: usize) -> (CmdTok, bool) {
        let cond = !capture && self.in_condition();
        // inside `( … )` (e.g. `assert_fails($ false)`), an unmatched `)` ends the command
        let in_paren = !capture && self.paren > 0;
        let stop_paren = capture || in_paren;
        // `{` opened on this line and not yet closed (`if x { $ a }`, `$ a catch { $ b }`): a `}` word
        // closes the block and ends the command
        let line_start = self.src[..self.pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let mut open_braces = 0i32;
        for t in self.toks.iter().rev() {
            if t.span.start < self.base + line_start {
                break;
            }
            match t.tok {
                Tok::LBrace => open_braces += 1,
                Tok::RBrace => open_braces -= 1,
                _ => {}
            }
        }
        let text_start = self.pos;
        let mut stages: Vec<Vec<LWord>> = vec![vec![]];
        let mut redirects: Vec<LRedir> = Vec::new();
        let mut closed = !capture;
        let mut text_end = self.pos;
        loop {
            while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t' | b'\r') {
                self.pos += 1;
            }
            if self.pos >= self.b.len() {
                break;
            }
            let c = self.b[self.pos];
            if c == b'\n' {
                break;
            }
            if in_paren && c == b')' {
                break;
            }
            if !capture && c == b'}' && open_braces > 0 && matches!(self.peek_at(self.pos + 1), b' ' | b'\t' | b'\r' | b'\n' | 0 | b'}') {
                break;
            }
            if capture && c == b')' {
                text_end = self.pos;
                self.pos += 1;
                closed = true;
                break;
            }
            if !capture && c == b'/' && self.peek_at(self.pos + 1) == b'/' {
                break;
            }
            if !capture
                && self.src[self.pos..].starts_with("catch")
                && matches!(self.peek_at(self.pos + 5), b' ' | b'\t' | b'\r' | b'\n' | 0)
            {
                break;
            }
            if !capture && c == b'{' {
                if self.rest_of_line_blank(self.pos + 1) {
                    break;
                }
                if cond && matches!(self.peek_at(self.pos + 1), b' ' | b'\t') {
                    if let Some(e) = scan_code_end(self.b, self.pos, 0) {
                        if self.rest_of_line_blank(e + 1) {
                            break;
                        }
                    }
                }
            }
            if c == b'|' {
                if self.peek_at(self.pos + 1) == b'|' {
                    self.err(self.pos, self.pos + 2, "`||` does not exist in commands").help =
                        Some("to run another command when this one fails: `$ cmd catch { $ other }`".into());
                    self.pos += 2;
                    continue;
                }
                let ps = self.pos;
                self.pos += 1;
                if stages.last().map(|s| s.is_empty()).unwrap_or(true) {
                    self.err(ps, ps + 1, "`|` without a command before it");
                } else {
                    stages.push(vec![]);
                }
                text_end = self.pos;
                continue;
            }
            // redirections
            let rest = &self.src[self.pos..];
            let redir = if rest.starts_with("2>&1") {
                Some((RedirKind::ErrToOut, 4))
            } else if rest.starts_with("2>>") {
                Some((RedirKind::ErrAppend, 3))
            } else if rest.starts_with("2>") {
                Some((RedirKind::Err, 2))
            } else if rest.starts_with(">>") {
                Some((RedirKind::Append, 2))
            } else if rest.starts_with('>') {
                Some((RedirKind::Out, 1))
            } else if rest.starts_with('<') {
                Some((RedirKind::In, 1))
            } else {
                None
            };
            if let Some((kind, len)) = redir {
                let rs = self.pos;
                self.pos += len;
                let mut target = None;
                if kind != RedirKind::ErrToOut {
                    while self.pos < self.b.len() && matches!(self.b[self.pos], b' ' | b'\t') {
                        self.pos += 1;
                    }
                    let w = self.word(stop_paren);
                    if w.parts.is_empty() {
                        self.err(rs, rs + len, "redirection without a file").help = Some("write, for example, `> out.txt`".into());
                    } else {
                        target = Some(w);
                    }
                }
                redirects.push(LRedir { kind, target, span: self.sp(rs, self.pos) });
                text_end = self.pos;
                continue;
            }
            let before = self.pos;
            let w = self.word(stop_paren);
            if self.pos == before {
                // nothing consumed: a character that ends a word but was not handled
                self.pos += 1;
                continue;
            }
            if !w.parts.is_empty() {
                stages.last_mut().unwrap().push(w);
            }
            text_end = self.pos;
        }
        if !closed {
            self.err(dollar, dollar + 2, "`$(` capture without the closing `)`").help = Some("a capture ends on the same line: `$(git rev-parse HEAD)`".into());
            text_end = self.pos;
        }
        if stages.last().map(|s| s.is_empty()).unwrap_or(false) && stages.len() > 1 {
            self.err(text_end.saturating_sub(1), text_end, "`|` without a command after it");
            stages.pop();
        }
        if stages.len() == 1 && stages[0].is_empty() {
            self.err(dollar, dollar + 1, "empty command after `$`");
        }
        let text = self.src[text_start..text_end.max(text_start)].trim().to_string();
        (CmdTok { stages, redirects, text }, closed)
    }

    /// A command word (up to space, `|`, `>`, `<`, end of line; an unmatched `)` in a capture or
    /// in a command inside parentheses).
    fn word(&mut self, capture: bool) -> LWord {
        let s = self.pos;
        let mut parts: Vec<WPart> = Vec::new();
        let mut lit = String::new();
        let mut depth = 0usize;
        // An unquoted `~` at the start of a word (alone or before `/`) is the user's home directory on all three targets, like
        // `{dirs.home()}`; inside quotes it is plain text (`'~/x'`).
        let next = self.peek_at(self.pos + 1);
        if self.peek() == b'~' && (matches!(next, b'/' | b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'>' | b'<' | 0) || (capture && next == b')')) {
            parts.push(WPart::Code { src: "dirs.home()".into(), offset: self.base + s, quoted: false });
            self.pos += 1;
        }
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            match c {
                b' ' | b'\t' | b'\r' | b'\n' | b'|' | b'>' | b'<' => break,
                b')' if capture && depth == 0 => break,
                b'(' => {
                    depth += 1;
                    lit.push('(');
                    self.pos += 1;
                }
                b')' => {
                    depth = depth.saturating_sub(1);
                    lit.push(')');
                    self.pos += 1;
                }
                b'\'' => {
                    let qs = self.pos;
                    self.pos += 1;
                    let st = self.pos;
                    while self.pos < self.b.len() && self.b[self.pos] != b'\'' && self.b[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    lit.push_str(&self.src[st..self.pos]);
                    if self.peek() == b'\'' {
                        self.pos += 1;
                    } else {
                        self.err(qs, self.pos, "`'` text without the closing quote");
                    }
                    if lit.is_empty() {
                        // explicit '': empty argument
                        parts.push(WPart::Lit(String::new()));
                    }
                }
                b'"' => {
                    let qs = self.pos;
                    let (sparts, end) = scan_dq(self.src, qs, self.base, &mut self.diags);
                    self.pos = end;
                    if sparts.is_empty() && lit.is_empty() {
                        parts.push(WPart::Lit(String::new()));
                    }
                    for p in sparts {
                        match p {
                            LStrPart::Lit(t) => lit.push_str(&t),
                            LStrPart::Code { src, offset } => {
                                if !lit.is_empty() {
                                    parts.push(WPart::Lit(std::mem::take(&mut lit)));
                                }
                                parts.push(WPart::Code { src, offset, quoted: true });
                            }
                        }
                    }
                }
                b'{' => {
                    let bs = self.pos;
                    match scan_code_end(self.b, bs, 0) {
                        Some(e) => {
                            let code = &self.src[bs + 1..e];
                            if code.trim().is_empty() {
                                lit.push_str(&self.src[bs..=e]);
                            } else {
                                if !lit.is_empty() {
                                    parts.push(WPart::Lit(std::mem::take(&mut lit)));
                                }
                                parts.push(WPart::Code { src: code.to_string(), offset: self.base + bs + 1, quoted: false });
                            }
                            self.pos = e + 1;
                        }
                        None => {
                            self.err(bs, bs + 1, "`{` without `}` in the command").help =
                                Some("each `{expr}` closes on the same line; for a literal `{` use single quotes: `'{'`".into());
                            self.pos = self.b.len().min(self.line_end(bs));
                        }
                    }
                }
                _ => {
                    let ch = self.src[self.pos..].chars().next().unwrap_or(' ');
                    lit.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        if !lit.is_empty() {
            parts.push(WPart::Lit(lit));
        }
        // merge adjacent literals
        let mut merged: Vec<WPart> = Vec::new();
        for p in parts {
            if let (Some(WPart::Lit(a)), WPart::Lit(b)) = (merged.last_mut(), &p) {
                a.push_str(b);
                continue;
            }
            merged.push(p);
        }
        LWord { parts: merged, span: self.sp(s, self.pos) }
    }

    fn line_end(&self, mut i: usize) -> usize {
        while i < self.b.len() && self.b[i] != b'\n' {
            i += 1;
        }
        i
    }
}

/// Given `b[open] == '{'`, returns the index of the matching `}` (on the same line), skipping nested strings.
pub fn scan_code_end(b: &[u8], open: usize, nest: usize) -> Option<usize> {
    if nest > MAX_NEST {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'"' => {
                i = skip_dq(b, i, nest + 1)?;
                continue;
            }
            b'\'' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'\'' && b[j] != b'\n' {
                    j += 1;
                }
                if j >= b.len() || b[j] != b'\'' {
                    return None;
                }
                i = j + 1;
                continue;
            }
            b'\n' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Given `b[i] == '"'`, returns the index right after the closing quote (on the same line).
fn skip_dq(b: &[u8], i: usize, nest: usize) -> Option<usize> {
    if nest > MAX_NEST {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            b'\n' => return None,
            b'{' => {
                if b.get(j + 1) == Some(&b'{') {
                    j += 2;
                } else {
                    j = scan_code_end(b, j, nest + 1)? + 1;
                }
            }
            _ => j += 1,
        }
    }
    None
}

/// Parses `"…"` starting at `src[s] == '"'`. Returns the parts and the index after the closing quote.
fn scan_dq(src: &str, s: usize, base: usize, diags: &mut Vec<Diag>) -> (Vec<LStrPart>, usize) {
    let b = src.as_bytes();
    let mut j = s + 1;
    let mut closed = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => {
                closed = true;
                break;
            }
            b'\n' => break,
            b'{' => {
                if b.get(j + 1) == Some(&b'{') {
                    j += 2;
                } else if let Some(e) = scan_code_end(b, j, 0) {
                    j = e + 1;
                } else {
                    j += 1;
                }
            }
            _ => j += 1,
        }
    }
    let j = j.min(b.len());
    if !closed {
        diags.push(
            Diag::error("E005", Span::new(base + s, base + j.max(s + 1)), "text without the closing quote `\"`")
                .with_help("`\"…\"` text stays on one line; for multiple lines use `\"\"\"`"),
        );
    }
    let chars: Vec<(usize, char)> = src[s + 1..j].char_indices().map(|(o, c)| (s + 1 + o, c)).collect();
    let parts = parts_from_chars(src, &chars, base, diags);
    (parts, if closed { j + 1 } else { j })
}

/// Converts the body of an interpolated string into parts. `chars` carries the offset (relative to `src`).
fn parts_from_chars(src: &str, chars: &[(usize, char)], base: usize, diags: &mut Vec<Diag>) -> Vec<LStrPart> {
    let b = src.as_bytes();
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let (off, c) = chars[i];
        match c {
            '\\' => {
                let next = chars.get(i + 1).map(|x| x.1);
                let rep = match next {
                    Some('n') => Some('\n'),
                    Some('t') => Some('\t'),
                    Some('r') => Some('\r'),
                    Some('"') => Some('"'),
                    Some('\\') => Some('\\'),
                    Some('{') => Some('{'),
                    Some('}') => Some('}'),
                    Some('0') => {
                        diags.push(
                            Diag::error("E005", Span::new(base + off, base + off + 2), "`\\0` is not allowed: Bash cannot store the NUL character"),
                        );
                        None
                    }
                    // `\u{1b}`: a character by its code point, 1 to 6 hex digits
                    Some('u') => {
                        let mut j = i + 2;
                        let mut hex = String::new();
                        let open = chars.get(j).map(|x| x.1) == Some('{');
                        if open {
                            j += 1;
                            while let Some((_, h)) = chars.get(j) {
                                if !h.is_ascii_hexdigit() || hex.len() > 6 {
                                    break;
                                }
                                hex.push(*h);
                                j += 1;
                            }
                        }
                        let closed = open && chars.get(j).map(|x| x.1) == Some('}');
                        let ch = u32::from_str_radix(&hex, 16).ok().filter(|n| *n != 0).and_then(char::from_u32);
                        match ch {
                            Some(ch) if closed && hex.len() <= 6 => {
                                lit.push(ch);
                                i = j + 1;
                            }
                            _ => {
                                let end = chars.get(j).map(|x| x.0).unwrap_or(off + 2);
                                diags.push(
                                    Diag::error("E005", Span::new(base + off, base + end.max(off + 2)), "`\\u` needs a code point in braces")
                                        .with_help("write `\\u{1b}` or `\\u{1F44D}`: 1 to 6 hex digits of a valid character (not 0)"),
                                );
                                i += 2;
                            }
                        }
                        continue;
                    }
                    Some(o) => {
                        diags.push(
                            Diag::error("E005", Span::new(base + off, base + off + 1 + o.len_utf8()), format!("unknown escape `\\{}`", o))
                                .with_help("escapes: `\\n \\t \\r \\\" \\\\ \\{ \\} \\u{1b}`; for a literal `\\` use `'raw'`"),
                        );
                        None
                    }
                    None => None,
                };
                if let Some(r) = rep {
                    lit.push(r);
                }
                i += 2;
            }
            '{' => {
                if chars.get(i + 1).map(|x| x.1) == Some('{') {
                    lit.push('{');
                    i += 2;
                    continue;
                }
                match scan_code_end(b, off, 0) {
                    Some(end) => {
                        let code = &src[off + 1..end];
                        if code.trim().is_empty() {
                            lit.push_str(&src[off..=end]);
                        } else {
                            if !lit.is_empty() {
                                parts.push(LStrPart::Lit(std::mem::take(&mut lit)));
                            }
                            parts.push(LStrPart::Code { src: code.to_string(), offset: base + off + 1 });
                        }
                        while i < chars.len() && chars[i].0 <= end {
                            i += 1;
                        }
                    }
                    None => {
                        diags.push(
                            Diag::error("E005", Span::new(base + off, base + off + 1), "`{` without `}` in the text")
                                .with_help("for a literal brace write `{{`"),
                        );
                        lit.push('{');
                        i += 1;
                    }
                }
            }
            '}' => {
                if chars.get(i + 1).map(|x| x.1) == Some('}') {
                    lit.push('}');
                    i += 2;
                } else {
                    diags.push(
                        Diag::error("E005", Span::new(base + off, base + off + 1), "lone `}` in the text").with_help("for a literal brace write `}}`"),
                    );
                    lit.push('}');
                    i += 1;
                }
            }
            _ => {
                lit.push(c);
                i += 1;
            }
        }
    }
    if !lit.is_empty() || parts.is_empty() {
        parts.push(LStrPart::Lit(lit));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        let (t, d) = lex(src, 0);
        assert!(d.is_empty(), "diags: {:?}", d);
        t.into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers_and_durations() {
        assert_eq!(toks("1_000 0xFF")[..2], [Tok::Int(1000), Tok::Int(255)]);
        assert_eq!(
            toks("500ms 30s 1.5s 5m 1m30s 2h 7d")[..7],
            [
                Tok::Dur(500),
                Tok::Dur(30_000),
                Tok::Dur(1_500),
                Tok::Dur(300_000),
                Tok::Dur(90_000),
                Tok::Dur(7_200_000),
                Tok::Dur(604_800_000)
            ]
        );
        let (_, d) = lex("30s1m", 0);
        assert_eq!(d.len(), 1);
        let (_, d) = lex("1.5m", 0);
        assert_eq!(d.len(), 1);
        let (_, d) = lex("99999999999999999999", 0);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn range_is_not_a_fraction() {
        assert_eq!(toks("0..10")[..3], [Tok::Int(0), Tok::DotDot, Tok::Int(10)]);
    }

    #[test]
    fn strings_with_braces() {
        let t = toks("\"a {x + 1} {{b}} {} \\n\"");
        match &t[0] {
            Tok::Str(s) => {
                assert_eq!(s.parts.len(), 3);
                assert_eq!(s.parts[0], LStrPart::Lit("a ".into()));
                assert_eq!(s.parts[1], LStrPart::Code { src: "x + 1".into(), offset: 4 });
                assert_eq!(s.parts[2], LStrPart::Lit(" {b} {} \n".into()));
            }
            o => panic!("{:?}", o),
        }
        // text inside interpolation
        let t = toks("\"{xs.join(\", \")}\"");
        match &t[0] {
            Tok::Str(s) => assert_eq!(s.parts, vec![LStrPart::Code { src: "xs.join(\", \")".into(), offset: 2 }]),
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn raw_and_triple_strings() {
        let t = toks("'C:\\x\\{y}'");
        assert_eq!(t[0], Tok::Str(StrTok { parts: vec![LStrPart::Lit("C:\\x\\{y}".into())], raw: true }));
        let t = toks("let t = \"\"\"\n    linha 1\n      linha 2: {v}\n    \"\"\"\n");
        match &t[3] {
            Tok::Str(s) => {
                assert_eq!(s.parts[0], LStrPart::Lit("linha 1\n  linha 2: ".into()));
                assert!(matches!(&s.parts[1], LStrPart::Code { src, .. } if src == "v"));
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn nl_and_continuation() {
        let t = toks("let x = 1 +\n  2\nxs\n  .len()\n\n\nfoo(a,\n b)");
        let nls = t.iter().filter(|t| **t == Tok::Nl).count();
        assert_eq!(nls, 3);
    }

    #[test]
    fn simple_command() {
        let t = toks("$ docker build -t {img}:{tag} . catch ignore");
        match &t[0] {
            Tok::Cmd(c) => {
                assert_eq!(c.stages[0].len(), 5);
                assert_eq!(c.text, "docker build -t {img}:{tag} .");
                assert_eq!(c.stages[0][3].parts.len(), 3);
            }
            o => panic!("{:?}", o),
        }
        assert_eq!(t[1], Tok::Kw(Kw::Catch));
        assert_eq!(t[2], Tok::Kw(Kw::Ignore));
    }

    #[test]
    fn command_pipe_redirect_quotes() {
        let t = toks("$ git log --pretty=format:\"- %s\" | head -n 20 > out.txt 2>&1");
        match &t[0] {
            Tok::Cmd(c) => {
                assert_eq!(c.stages.len(), 2);
                assert_eq!(c.stages[0][2].parts, vec![WPart::Lit("--pretty=format:- %s".into())]);
                assert_eq!(c.redirects.len(), 2);
                assert_eq!(c.redirects[0].kind, RedirKind::Out);
                assert_eq!(c.redirects[1].kind, RedirKind::ErrToOut);
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn command_with_trailing_brace() {
        let t = toks("if $ git diff --quiet {\n}");
        assert!(matches!(t[1], Tok::Cmd(_)));
        assert_eq!(t[2], Tok::LBrace);
        let t = toks("while !$ nc -z localhost 5432 { time.sleep(1s) }");
        match &t[2] {
            Tok::Cmd(c) => assert_eq!(c.stages[0].len(), 4),
            o => panic!("{:?}", o),
        }
        assert_eq!(t[3], Tok::LBrace);
    }

    #[test]
    fn map_at_end_of_line_ends_the_statement() {
        let kinds = |src: &str| lex(src, 0).0.into_iter().filter(|t| t.tok == Tok::Nl).count();
        // `map[str]int` ends in a name: the line break counts
        assert_eq!(kinds("struct S {\n    m: map[str]int\n    n: int\n}"), 3);
        assert_eq!(kinds("param m: map[str][]int\nprint(m)"), 2);
        // a comparison at the end of the line continues on the next
        assert_eq!(kinds("let a = 3 >\n    2\nprint(a)"), 2);
    }

    #[test]
    fn closing_brace_ends_one_line_block() {
        let t = toks("$ a catch { $ b }");
        assert_eq!(t.iter().filter(|x| matches!(x, Tok::RBrace)).count(), 1, "{:?}", t);
        let t = toks("if x { $ echo oi }");
        assert_eq!(t.iter().filter(|x| matches!(x, Tok::RBrace)).count(), 1, "{:?}", t);
        // without an open `{` on the line, `}` is an argument
        assert!(toks("$ echo }").iter().all(|x| !matches!(x, Tok::RBrace)));
    }

    #[test]
    fn catch_ends_the_command_and_or_is_just_a_name() {
        let t = toks("$ grep -e isto or aquilo catch ignore");
        // `or` is a program argument; `catch` ends the command
        assert_eq!(t[1], Tok::Kw(Kw::Catch));
        assert_eq!(toks("let or = 1")[1], Tok::Ident("or".into()));
    }

    #[test]
    fn command_inside_parentheses() {
        let t = toks("assert_fails($ false)\nf($ ls (a) b)");
        assert!(matches!(&t[2], Tok::Cmd(c) if c.stages[0].len() == 1));
        assert_eq!(t[3], Tok::RParen);
        match &t[7] {
            Tok::Cmd(c) => assert_eq!(c.stages[0].len(), 3),
            o => panic!("{:?}", o),
        }
        assert_eq!(t[8], Tok::RParen);
    }

    #[test]
    fn capture() {
        let t = toks("let x = $(git rev-parse --short HEAD) catch \"\"");
        match &t[3] {
            Tok::Capture(c) => {
                assert_eq!(c.stages[0].len(), 4);
                assert_eq!(c.text, "git rev-parse --short HEAD");
            }
            o => panic!("{:?}", o),
        }
        assert_eq!(t[4], Tok::Kw(Kw::Catch));
    }

    #[test]
    fn doc_comment() {
        let (t, _) = lex("/// quem\n/// cumprimentar\nparam nome: str", 0);
        assert_eq!(t[0].doc.as_deref(), Some("quem cumprimentar"));
    }

    #[test]
    fn broken_input_does_not_panic() {
        for src in ["\"abc", "'abc", "\"{abc", "$(ls", "$ echo {x", "/* x", "\"\"\"\nabc", "{{{{", "1.", "0x", "$", "é", "\"\\", "$ a |", "$ > "] {
            let _ = lex(src, 0);
        }
    }
}
