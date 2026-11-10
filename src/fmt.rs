//! `nalu fmt`: the formatter. It preserves lines and works on tokens, like `gofmt`: it never joins or
//! breaks lines; it fixes indentation, spaces within a line, blank lines and the position of
//! comments. It is the house style, with no options.
//!
//! 1. **Broken input is refused.** If the lexer or the parser report an error, [`format`] returns the
//!    diagnostics and changes nothing. Type errors do not matter, only syntax. The diagnostics' spans are
//!    relative to the text without the BOM (as in `nalu check`).
//! 2. **Literal tokens.** Each token comes out with the exact source text: `1_000`, `0xFF`, `1m30s`, strings,
//!    `$ command` and `$( )` byte for byte (spaces inside a command are meaningful). Only the spaces at the
//!    end of a `$ command` are dropped (the lexer includes them in the token, but they are not part of any word).
//!    Lines inside a `"""` stay exactly as they are.
//! 3. **Indentation.** 4 spaces per level; tabs in the indentation become spaces. `{ ( [` opened on a line and
//!    still open at its end indent the following lines by ONE level in total, no matter how many
//!    were opened (`f(x, {` → one level; the `})` line goes back). A line that starts with a closer goes back
//!    to the level of the line that opened the pair. Continuation gets +1: a line that starts with `.` (method
//!    chain) and the line after one that ends in a binary operator, `=` (and `+=`…), `catch`, `|`, `=>`,
//!    `->`, `&&`, `||`, `.`, `..` and `!`, because the lexer puts no `Nl` after any of them (a line that ends
//!    by opening `{ ( [` is already indented by the previous rule).
//! 4. **Spaces within a line** (always exactly one): around binary operators
//!    (`+ - * / % == != < <= > >= && || = += -= *= /= %= => -> |`) and after `,` and `:`; none before
//!    `, : ) ] .` nor after `( [ .`; `..`/`..=` stuck together (`0..10`, `1..=n`, `{ ..self }`); unary `-` and `!`
//!    stuck to the operand (`-1`, `!ok`, `f(-x)`, `return -1`); call and index stuck together (`f(x)`,
//!    `xs[i]`), but a keyword before `(`/`[` gets a space (`if (a)`, `in [1, 2]`,
//!    `return [x]`), except `retry(` and `parallel(`, which take arguments like a call. `{` gets a
//!    space before it (except after `(`/`[`) and, when there is content on the same line, a space inside
//!    (`{ 6 }`, `Servico { nome: "api" }`, `enum A { a, b }`); `{}` stays `{}`. Types stuck together, as in Go:
//!    `[]str`, `map[str]int`, `map[str][]int`.
//! 5. **Blank lines.** Several in a row become one; none right after a line that ends by opening
//!    `{ ( [`, nor right before one that starts by closing; none at the start of the file; the file ends with
//!    exactly one line break; no trailing spaces on lines (except inside a `"""`).
//! 6. **Comments** stay where they are, with the text intact (they only lose trailing spaces). A comment
//!    alone on a line gets the indentation of the next code line; if that line starts by closing a block (the
//!    comment is the last one in the block), the block content's indentation. `/* */` comes out as is.
//! 7. **Alignment: only two cases.** (a) `//` comments at the end of consecutive lines (no blank line
//!    or comment-only line between them, same indentation) align in one column: that of the longest
//!    code + 2 spaces; a lone comment sits 2 spaces after the code. (b) In a `match`, consecutive
//!    one-line arms align the `=>`: the pattern is padded to the widest of the sequence
//!    (`dev   => "dev.exemplo.com"`); a blank line, a comment line or a multi-line arm breaks the
//!    sequence. Everything else that was aligned by hand becomes a single space.
//! 8. **Line endings.** If the first line ends in CRLF, the whole output uses CRLF; the UTF-8 BOM at the
//!    start is kept.
//!
//! How it works: the lexer gives the tokens with spans; what lies between two tokens is only whitespace, line breaks and
//! comments. The text becomes a list of logical lines (pieces: tokens and comments; a multi-line `"""` or
//! `/* */` stays whole inside the line where it starts). A stack of `{ ( [` gives the
//! indentation of each line; then each line is built with the spacing rules and, finally, the two
//! alignments are applied.

use crate::diag::Diag;
use crate::lexer::{self, Kw, Tok, Token};
use crate::parser;

/// Spaces per indentation level.
const INDENT: usize = 4;

/// Formats a whole `.nalu` file. `Err` with the diagnostics if the syntax is broken.
pub fn format(src: &str) -> Result<String, Vec<Diag>> {
    let (bom, body) = match src.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, src),
    };
        // 1. Refuse broken input. The analysis runs on the original text (without the BOM), so the spans
        //    point to the right place also in a file with CRLF.
    let (_, diags) = parser::parse(body);
    if diags.iter().any(Diag::is_error) {
        return Err(diags);
    }
        // 8. Line endings: work with `\n` and return CRLF at the end, if needed. To the lexer, `\r` is just
        //    whitespace (even inside `"""`), so replacing `\r\n` with `\n` changes no token.
    let crlf = body.find('\n').map_or(false, |i| body[..i].ends_with('\r'));
    let text = body.replace("\r\n", "\n");
    let (toks, ldiags) = lexer::lex(&text, 0);
    if ldiags.iter().any(Diag::is_error) {
        return Err(ldiags);
    }
    let toks: Vec<Token> = toks.into_iter().filter(|t| !matches!(t.tok, Tok::Nl | Tok::Eof)).collect();
    let mut out = Fmt::new(&text, toks).run();
    if crlf {
        out = out.replace('\n', "\r\n");
    }
    if bom {
        out.insert(0, '\u{feff}');
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Logical lines
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
        /// Index into `Fmt::toks`.
    Tok(usize),
        /// `// …` (always the last piece of the line).
    LineComment,
        /// `/* … */` (may span several lines).
    BlockComment,
}

#[derive(Debug)]
struct Piece {
    kind: Kind,
        /// The text that comes out (the source's; comments and `$ command` without trailing spaces).
    text: String,
        /// Was there whitespace between the previous piece (on the same line) and this one? Only matters near `/* */`.
    ws_before: bool,
}

#[derive(Debug)]
struct Line {
    pieces: Vec<Piece>,
        /// There was at least one blank line before this one in the source.
    blank_before: bool,
}

/// Builds the logical lines from the pieces, counting the breaks between them.
#[derive(Default)]
struct Builder {
    lines: Vec<Line>,
    cur: Vec<Piece>,
        /// Line breaks since the last piece.
    nls: usize,
    ws: bool,
    blank: bool,
}

impl Builder {
    fn newline(&mut self) {
        self.nls += 1;
        self.ws = false;
    }

    fn push(&mut self, kind: Kind, text: String) {
        if !self.cur.is_empty() && self.nls > 0 {
            let pieces = std::mem::take(&mut self.cur);
            self.lines.push(Line { pieces, blank_before: self.blank });
            self.blank = self.nls >= 2;
        }
        let ws_before = self.ws && !self.cur.is_empty();
        self.cur.push(Piece { kind, text, ws_before });
        self.nls = 0;
        self.ws = false;
    }

    fn finish(mut self) -> Vec<Line> {
        if !self.cur.is_empty() {
            let pieces = std::mem::take(&mut self.cur);
            self.lines.push(Line { pieces, blank_before: self.blank });
        }
        self.lines
    }
}

/// What is known about each line after passing through the `{ ( [` stack.
#[derive(Clone, Debug, Default)]
struct Info {
        /// Has at least one token (otherwise, it is a comment-only line).
    code: bool,
    first: usize,
    last: usize,
        /// Starts with `) ] }`.
    closer: bool,
        /// Continuation line (+1).
    cont: bool,
    indent: usize,
        /// The innermost `{ ( [` still open at the start and at the end of the line.
    top_start: Option<usize>,
    top_end: Option<usize>,
        /// Comes out with a blank line before.
    blank: bool,
}

/// A built line (without the indentation).
#[derive(Debug)]
struct Rendered {
    code: String,
        /// `//` comment at the end of a line that has code.
    comment: Option<String>,
        /// Where the `match` arm's pattern ends (the first `=>` outside parentheses), in bytes of `code`.
    arrow: Option<usize>,
}

fn is_open(t: &Tok) -> bool {
    matches!(t, Tok::LParen | Tok::LBracket | Tok::LBrace)
}

fn is_close(t: &Tok) -> bool {
    matches!(t, Tok::RParen | Tok::RBracket | Tok::RBrace)
}

/// Can `a` and `b` be glued without the lexer reading something else (`- -x` must not become `--x`)?
fn glue_ok(a: &str, b: &str) -> bool {
    let (Some(x), Some(y)) = (a.chars().last(), b.chars().next()) else {
        return true;
    };
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if word(x) && word(y) {
        return false;
    }
    !matches!(
        (x, y),
        ('-', '-')
            | ('-', '>')
            | ('-', '=')
            | ('+', '+')
            | ('+', '=')
            | ('=', '=')
            | ('=', '>')
            | ('!', '=')
            | ('<', '=')
            | ('>', '=')
            | ('&', '&')
            | ('|', '|')
            | ('.', '.')
            | ('.', '=')
            | ('/', '/')
            | ('/', '*')
            | ('/', '=')
            | ('*', '=')
            | ('*', '/')
            | ('%', '=')
            | ('?', '?')
    )
}

struct Fmt<'a> {
    src: &'a str,
        /// The tokens, without `Nl` or `Eof`.
    toks: Vec<Token>,
        /// Unary `-`.
    unary: Vec<bool>,
}

impl<'a> Fmt<'a> {
    fn new(src: &'a str, toks: Vec<Token>) -> Fmt<'a> {
        let n = toks.len();
        let mut f = Fmt { src, toks, unary: vec![false; n] };
        f.mark_unary();
        f
    }

    fn tok(&self, k: usize) -> &Tok {
        &self.toks[k].tok
    }

    // -----------------------------------------------------------------------------------------
    // Token classification
    // -----------------------------------------------------------------------------------------

    /// `-` is unary at the start of a line and after something that does not end a value (`= -1`, `(-x`, `return -1`).
    fn mark_unary(&mut self) {
        for k in 0..self.toks.len() {
            if *self.tok(k) == Tok::Minus {
                self.unary[k] = k == 0 || !self.same_line(k - 1, k) || !self.value_end(k - 1);
            }
        }
    }

    fn same_line(&self, a: usize, b: usize) -> bool {
        self.src.get(self.toks[a].span.end..self.toks[b].span.start).map_or(false, |gap| !gap.contains('\n'))
    }

        /// Can the token end a value (so a `-` after it is binary and a `(`/`[` is a call/index)?
    fn value_end(&self, k: usize) -> bool {
        match self.tok(k) {
            Tok::Ident(_)
            | Tok::Int(_)
            | Tok::Dur(_)
            | Tok::Str(_)
            | Tok::Capture(_)
            | Tok::RParen
            | Tok::RBracket
            | Tok::RBrace => true,
            Tok::Kw(kw) => matches!(kw, Kw::SelfKw | Kw::True | Kw::False),
            _ => false,
        }
    }

        /// Binary operator (with one space on each side).
    fn binop(&self, k: usize) -> bool {
        match self.tok(k) {
            Tok::Plus
            | Tok::Star
            | Tok::Slash
            | Tok::Percent
            | Tok::EqEq
            | Tok::Ne
            | Tok::Le
            | Tok::Ge
            | Tok::AndAnd
            | Tok::OrOr
            | Tok::Assign
            | Tok::PlusEq
            | Tok::MinusEq
            | Tok::StarEq
            | Tok::SlashEq
            | Tok::PercentEq
            | Tok::FatArrow
            | Tok::Arrow
            | Tok::Pipe => true,
            Tok::Minus => !self.unary[k],
            Tok::Lt | Tok::Gt => true,
            _ => false,
        }
    }

        /// A line that ends in this token continues on the next (+1 indentation). Follows the lexer, which does not
        /// put `Nl` after these tokens.
    fn continues(&self, k: usize) -> bool {
        self.binop(k)
            || matches!(
                self.tok(k),
                Tok::Minus | Tok::Lt | Tok::Gt | Tok::Bang | Tok::Dot | Tok::DotDot | Tok::DotDotEq | Tok::Kw(Kw::Catch)
            )
    }

        /// Does a space go between tokens `p` and `n`, neighbors on the same line? (rule 4)
    fn space(&self, p: usize, n: usize) -> bool {
        let (pt, nt) = (self.tok(p), self.tok(n));
                // nothing after `( [ . .. ..= !` and unary `-`
        if matches!(pt, Tok::LParen | Tok::LBracket | Tok::Dot | Tok::DotDot | Tok::DotDotEq | Tok::Bang) || (*pt == Tok::Minus && self.unary[p]) {
            return false;
        }
                // nothing before `) ] , : .`
        if matches!(nt, Tok::RParen | Tok::RBracket | Tok::Comma | Tok::Colon | Tok::Dot) {
            return false;
        }
                // types: `[]str`, `map[str]int` (a `]` followed by a name only appears in types)
        if *pt == Tok::RBracket && matches!(nt, Tok::Ident(_)) {
            return false;
        }
                // `{ x }`, `{}`, ` {`
        if *pt == Tok::LBrace {
            return *nt != Tok::RBrace;
        }
        if matches!(nt, Tok::LBrace | Tok::RBrace) {
            return true;
        }
                // `0..10`, but `{ ..self` (above) and `, ..x`
        if matches!(nt, Tok::DotDot | Tok::DotDotEq) {
            return !self.value_end(p);
        }
        if matches!(pt, Tok::Comma | Tok::Colon) {
            return true;
        }
                // `= -1`, `return -1`, `if !ok`
        if *nt == Tok::Bang || (*nt == Tok::Minus && self.unary[n]) {
            return true;
        }
        if self.binop(p) || self.binop(n) {
            return true;
        }
                // call and index stuck together; keyword before `(`/`[` gets a space
        if matches!(nt, Tok::LParen | Tok::LBracket) {
            if let Tok::Kw(kw) = pt {
                // a keyword after `.` is a member name: `regex.test(…)`
                let member = p > 0 && *self.tok(p - 1) == Tok::Dot;
                if *nt == Tok::LParen && (member || matches!(kw, Kw::Retry | Kw::Parallel)) {
                    return false;
                }
                if !matches!(kw, Kw::SelfKw | Kw::True | Kw::False) {
                    return true;
                }
            }
            return !self.value_end(p);
        }
        true
    }

    // -----------------------------------------------------------------------------------------
    // Lines
    // -----------------------------------------------------------------------------------------

        /// Splits the source into logical lines of pieces.
    fn split(&self) -> Vec<Line> {
        let mut b = Builder::default();
        let mut pos = 0;
        for (k, t) in self.toks.iter().enumerate() {
            let start = t.span.start.max(pos);
            self.scan_gap(&mut b, pos, start);
            let end = t.span.end.max(start);
            let mut text = &self.src[start..end];
            let mut trailing_ws = false;
            if matches!(t.tok, Tok::Cmd(_)) {
                                // the `$ …` token includes the spaces up to the following `catch`/`{`/`//`; they are not part of the command
                let trimmed = text.trim_end_matches([' ', '\t', '\r']);
                trailing_ws = trimmed.len() < text.len();
                text = trimmed;
            }
            b.push(Kind::Tok(k), text.to_string());
            b.ws = trailing_ws;
            pos = end;
        }
        self.scan_gap(&mut b, pos, self.src.len());
        b.finish()
    }

        /// What lies between two tokens: whitespace, line breaks and comments.
    fn scan_gap(&self, b: &mut Builder, from: usize, to: usize) {
        let s = self.src;
        let bytes = s.as_bytes();
        let mut i = from;
        while i < to {
            match bytes[i] {
                b'\n' => {
                    b.newline();
                    i += 1;
                }
                b' ' | b'\t' | b'\r' => {
                    b.ws = true;
                    i += 1;
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    let end = s[i..to].find('\n').map_or(to, |k| i + k);
                    b.push(Kind::LineComment, s[i..end].trim_end().to_string());
                    i = end;
                }
                b'/' if bytes.get(i + 1) == Some(&b'*') => {
                    let end = s[i + 2..to].find("*/").map_or(to, |k| i + 2 + k + 2);
                    let text: Vec<&str> = s[i..end].split('\n').map(str::trim_end).collect();
                    b.push(Kind::BlockComment, text.join("\n"));
                    i = end;
                }
                _ => {
                                        // Cannot happen (the lexer only leaves whitespace and comments between tokens); to be safe, the
                                        // character comes out as is.
                    let len = s[i..].chars().next().map_or(1, char::len_utf8);
                    b.push(Kind::BlockComment, s[i..i + len].to_string());
                    i += len;
                }
            }
        }
    }

    fn run(&self) -> String {
        let lines = self.split();
        let n = lines.len();
        let mut info = vec![Info::default(); n];

                // Indentation of code lines: stack of `{ ( [` with the indentation of the line that opened each one.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut prev_last: Option<usize> = None;
        for (i, line) in lines.iter().enumerate() {
            let toks: Vec<usize> = line
                .pieces
                .iter()
                .filter_map(|p| match p.kind {
                    Kind::Tok(k) => Some(k),
                    _ => None,
                })
                .collect();
            let inf = &mut info[i];
            inf.top_start = stack.last().map(|e| e.0);
            inf.top_end = inf.top_start;
            let (Some(&first), Some(&last)) = (toks.first(), toks.last()) else {
                continue;
            };
            inf.code = true;
            inf.first = first;
            inf.last = last;
            let ft = &self.toks[first].tok;
            inf.closer = is_close(ft);
            inf.cont = !inf.closer && (*ft == Tok::Dot || prev_last.map_or(false, |p| self.continues(p)));
            inf.indent = if inf.closer {
                stack.last().map_or(0, |e| e.1)
            } else {
                stack.last().map_or(0, |e| e.1 + 1) + inf.cont as usize
            };
            for &k in &toks {
                let t = &self.toks[k].tok;
                if is_open(t) {
                    stack.push((k, inf.indent));
                } else if is_close(t) {
                    stack.pop();
                }
            }
            inf.top_end = stack.last().map(|e| e.0);
            prev_last = Some(last);
        }

                // Comment-only line: the indentation of the next code line (or that of the content of the block it
                // closes).
        let mut next_code: Option<usize> = None;
        for i in (0..n).rev() {
            if info[i].code {
                next_code = Some(i);
                continue;
            }
            info[i].indent = match next_code {
                Some(j) => info[j].indent + info[j].closer as usize,
                None => 0,
            };
        }

                // Blank lines.
        for i in 0..n {
            let mut blank = lines[i].blank_before && i > 0;
            if blank && info[i - 1].code && is_open(self.tok(info[i - 1].last)) {
                blank = false;
            }
            if blank && info[i].code && info[i].closer {
                blank = false;
            }
            info[i].blank = blank;
        }

        let mut outs: Vec<Rendered> = lines.iter().zip(&info).map(|(l, inf)| self.render(l, inf.code)).collect();
        self.align_arms(&info, &mut outs);

                // Trailing comments: common column per sequence.
        let width = |k: usize| info[k].indent * INDENT + outs[k].code.chars().count();
        let eligible = |k: usize| info[k].code && outs[k].comment.is_some() && !outs[k].code.contains('\n');
        let mut pad = vec![2usize; n];
        let mut i = 0;
        while i < n {
            if !eligible(i) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j < n && eligible(j) && !info[j].blank && info[j].indent == info[i].indent {
                j += 1;
            }
            let col = (i..j).map(width).max().unwrap_or(0) + 2;
            for k in i..j {
                pad[k] = col - width(k);
            }
            i = j;
        }

        let mut out = String::new();
        for i in 0..n {
            if info[i].blank {
                out.push('\n');
            }
            out.push_str(&" ".repeat(info[i].indent * INDENT));
            out.push_str(&outs[i].code);
            if let Some(c) = &outs[i].comment {
                out.push_str(&" ".repeat(pad[i]));
                out.push_str(c);
            }
            out.push('\n');
        }
        out
    }

        /// Builds the text of a line (without the indentation or the trailing comment).
    fn render(&self, line: &Line, has_code: bool) -> Rendered {
        let mut code = String::new();
        let mut comment = None;
        let mut arrow = None;
        let mut depth = 0i32;
        let mut prev: Option<&Piece> = None;
        for p in &line.pieces {
            if p.kind == Kind::LineComment && has_code {
                comment = Some(p.text.clone());
                continue;
            }
            if let Some(q) = prev {
                let sep = match (q.kind, p.kind) {
                    (Kind::Tok(a), Kind::Tok(b)) => self.space(a, b) || !glue_ok(&q.text, &p.text),
                    _ => p.ws_before,
                };
                if let Kind::Tok(b) = p.kind {
                    if *self.tok(b) == Tok::FatArrow && depth == 0 && arrow.is_none() {
                        arrow = Some(code.len());
                    }
                }
                if sep {
                    code.push(' ');
                }
            }
            if let Kind::Tok(k) = p.kind {
                let t = self.tok(k);
                if is_open(t) {
                    depth += 1;
                } else if is_close(t) {
                    depth -= 1;
                }
            }
            code.push_str(&p.text);
            prev = Some(p);
        }
        Rendered { code, comment, arrow }
    }

        /// The `{` that opens the body of each `match`.
    fn match_braces(&self) -> Vec<bool> {
        let mut is = vec![false; self.toks.len()];
        for m in 0..self.toks.len() {
            if *self.tok(m) != Tok::Kw(Kw::Match) {
                continue;
            }
            let mut depth = 0i32;
            for j in m + 1..self.toks.len() {
                match self.tok(j) {
                    Tok::LParen | Tok::LBracket => depth += 1,
                    Tok::RParen | Tok::RBracket => {
                        depth -= 1;
                        if depth < 0 {
                            break;
                        }
                    }
                    Tok::LBrace if depth == 0 => {
                        is[j] = true;
                        break;
                    }
                    Tok::RBrace => break,
                    _ => {}
                }
            }
        }
        is
    }

        /// Aligns the `=>` of consecutive one-line arms of a `match` (rule 7b).
    fn align_arms(&self, info: &[Info], outs: &mut [Rendered]) {
        let n = info.len();
        let mb = self.match_braces();
                // the `{` of the match whose one-line arm the line is
        let arms: Vec<Option<usize>> = (0..n)
            .map(|i| {
                let inf = &info[i];
                if !inf.code || inf.closer || inf.cont || outs[i].arrow.is_none() || outs[i].code.contains('\n') {
                    return None;
                }
                let top = inf.top_start?;
                (mb[top] && inf.top_end == Some(top)).then_some(top)
            })
            .collect();
        let mut i = 0;
        while i < n {
            let Some(m) = arms[i] else {
                i += 1;
                continue;
            };
            let mut j = i + 1;
            while j < n && arms[j] == Some(m) && !info[j].blank {
                j += 1;
            }
            if j - i >= 2 {
                let pat = |r: &Rendered| r.code[..r.arrow.unwrap_or(0)].chars().count();
                let w = outs[i..j].iter().map(pat).max().unwrap_or(0);
                for r in &mut outs[i..j] {
                    let a = r.arrow.unwrap_or(0);
                    let fill = w - pat(r);
                    r.code.insert_str(a, &" ".repeat(fill));
                }
            }
            i = j;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Span;
    use crate::lexer::{CmdTok, LStrPart, LWord, StrTok, WPart};

        /// Formats, checks that it is idempotent and returns the output.
    fn f(src: &str) -> String {
        let out = match format(src) {
            Ok(o) => o,
            Err(d) => panic!("did not format:\n{}\n{:#?}", src, d),
        };
        let again = format(&out).expect("the formatted output must be valid");
        assert_eq!(again, out, "not idempotent; input:\n{}", src);
        assert_tokens_equal(src, &out);
        out
    }

        // --- rule 1 -----------------------------------------------------------------------------

    #[test]
    fn refuses_broken_syntax() {
        for src in ["let a = (1\n", "let x = 1;\n", "let s = \"abc\n", "fn f( {\n}\n", "x++\n", "let a = b ?? c\n"] {
            let r = format(src);
            assert!(matches!(&r, Err(d) if d.iter().any(Diag::is_error)), "devia recusar {:?}: {:?}", src, r);
        }
                // a type error does not matter
        assert_eq!(f("let x: int = \"a\"\n"), "let x: int = \"a\"\n");
    }

    #[test]
    fn diagnostic_points_at_original_source_with_crlf() {
        let src = "let a = 1\r\nlet b = 2 +* 3\r\nlet c = 4\r\n";
        let d = format(src).unwrap_err();
        let (line, _) = d[0].position(src);
        assert_eq!(line, 2, "{:?}", d);
    }

        // --- rule 2 -----------------------------------------------------------------------------

    #[test]
    fn keyword_as_member_name_is_a_call() {
        assert_eq!(f("let ok=regex.test (v,'a')\n"), "let ok = regex.test(v, 'a')\n");
    }

    #[test]
    fn tokens_come_out_literal() {
        assert_eq!(f("let n=1_000+0xFF\n"), "let n = 1_000 + 0xFF\n");
        assert_eq!(f("let d=1m30s+500ms\n"), "let d = 1m30s + 500ms\n");
        assert_eq!(f("let s='a  b'+\"c   {n}\"\n"), "let s = 'a  b' + \"c   {n}\"\n");
                // inside the command, spaces are meaningful; the trailing ones are dropped
        assert_eq!(f("$ echo  \"a   b\"   {n}   \n"), "$ echo  \"a   b\"   {n}\n");
        assert_eq!(f("$ false   catch   ignore\n"), "$ false catch ignore\n");
        assert_eq!(f("let x=$(git  log   -1)   catch \"\"\n"), "let x = $(git  log   -1) catch \"\"\n");
        assert_eq!(f("assert_fails($ false )\n"), "assert_fails($ false)\n");
        assert_eq!(f("if  $ git diff --quiet   {\nprint(1)\n}\n"), "if $ git diff --quiet {\n    print(1)\n}\n");
        assert_eq!(f("while !$ nc -z localhost 5432 {   time.sleep(1s)   }\n"), "while !$ nc -z localhost 5432 { time.sleep(1s) }\n");
        assert_eq!(f("token  |  $ cat\n"), "token | $ cat\n");
    }

    #[test]
    fn multiline_text_stays_as_is() {
        let src = "fn f() -> str {\n  let t = \"\"\"\n      linha 1  \n\t  linha 2: {x}\n   \"\"\"\n  return t\n}\n";
        assert_eq!(
            f(src),
            "fn f() -> str {\n    let t = \"\"\"\n      linha 1  \n\t  linha 2: {x}\n   \"\"\"\n    return t\n}\n"
        );
        // tokens depois do `"""` final continuam a linha
        assert_eq!(f("print(\"\"\"\n  a\n  \"\"\" .trim( ))\n"), "print(\"\"\"\n  a\n  \"\"\".trim())\n");
    }

        // --- rule 3 -----------------------------------------------------------------------------

    #[test]
    fn indentation_by_groups() {
        assert_eq!(f("if a {\n  if b {\n\t\tc()\n  }\n}\n"), "if a {\n    if b {\n        c()\n    }\n}\n");
                // several opened on a line: just one level; the closing line goes back
        assert_eq!(f("let r = xs.map(n => {\nreturn n * 2\n})\n"), "let r = xs.map(n => {\n    return n * 2\n})\n");
        assert_eq!(f("let x = [[\n1,\n]]\n"), "let x = [[\n    1,\n]]\n");
        assert_eq!(
            f("print(a, f(b,\nc), d,\ne)\nprint(1)\n"),
            "print(a, f(b,\n    c), d,\n    e)\nprint(1)\n"
        );
        assert_eq!(
            f("let servicos = [\nServico { nome: \"api\" },\n  Servico { nome: \"web\" },\n]\n"),
            "let servicos = [\n    Servico { nome: \"api\" },\n    Servico { nome: \"web\" },\n]\n"
        );
        assert_eq!(f("if a {\nx()\n} else if b {\ny()\n} else {\nz()\n}\n"), "if a {\n    x()\n} else if b {\n    y()\n} else {\n    z()\n}\n");
    }

    #[test]
    fn continuation() {
        assert_eq!(f("let total = 1 +\n2 +\n        3\nlet y = 4\n"), "let total = 1 +\n    2 +\n    3\nlet y = 4\n");
        assert_eq!(f("let s = xs\n.map(n => n)\n  .join(\",\")\n"), "let s = xs\n    .map(n => n)\n    .join(\",\")\n");
        assert_eq!(f("let ok = a &&\nb ||\nc\n"), "let ok = a &&\n    b ||\n    c\n");
        assert_eq!(f("let v = env.get(\"X\") catch\n\"padrão\"\n"), "let v = env.get(\"X\") catch\n    \"padrão\"\n");
        assert_eq!(f("let x =\nif a { 1 } else { 2 }\n"), "let x =\n    if a { 1 } else { 2 }\n");
                // inside a block opened on a continuation line
        assert_eq!(
            f("let r = xs\n.map(n => {\nreturn n\n})\n"),
            "let r = xs\n    .map(n => {\n        return n\n    })\n"
        );
                // a line that ends by opening a pair does not get the +1 of continuation
        assert_eq!(f("let x = if a {\n1\n} else {\n2\n}\n"), "let x = if a {\n    1\n} else {\n    2\n}\n");
                // a type ends the statement: `=` on the next line is an error
        assert!(format("var m: map[str]int\n= {}\n").is_err());
    }

        // --- rule 4 -----------------------------------------------------------------------------

    #[test]
    fn spaces_around_operators() {
        assert_eq!(f("let x=a+b*c-d/e%f\n"), "let x = a + b * c - d / e % f\n");
        assert_eq!(f("x+=1\nx-=1\nx*=2\nx/=2\nx%=2\n"), "x += 1\nx -= 1\nx *= 2\nx /= 2\nx %= 2\n");
        assert_eq!(f("let b=a==1&&c!=2||d<3&&e>=4\n"), "let b = a == 1 && c != 2 || d < 3 && e >= 4\n");
        assert_eq!(f("print(f(a ,b) , xs [ 0 ] , s . len ( ))\n"), "print(f(a, b), xs[0], s.len())\n");
        assert_eq!(f("fn f(a:int,b:str=\"x\")->str {\nreturn b\n}\n"), "fn f(a: int, b: str = \"x\") -> str {\n    return b\n}\n");
        assert_eq!(f("param   nome :str=\"a\"\n"), "param nome: str = \"a\"\n");
        assert_eq!(f("retry (10, every : 5s) {\nx()\n}\n"), "retry(10, every: 5s) {\n    x()\n}\n");
        assert_eq!(f("let t = parallel (max: 4) for p in ps {\nx(p)\n}\n"), "let t = parallel(max: 4) for p in ps {\n    x(p)\n}\n");
        assert_eq!(f("fail  \"x\" , code:3\n"), "fail \"x\", code: 3\n");
        assert_eq!(f("let ys = xs.map( n=>n+1 )\n"), "let ys = xs.map(n => n + 1)\n");
        assert_eq!(f("let g = (a,b)=>a+b\n"), "let g = (a, b) => a + b\n");
    }

    #[test]
    fn unary_operators_stuck() {
        assert_eq!(f("let a = - 1\nlet b = f( - a )\nlet c = !  ok\nlet d = a - -b\n"), "let a = -1\nlet b = f(-a)\nlet c = !ok\nlet d = a - -b\n");
        assert_eq!(f("fn g() -> int {\nreturn - 1\n}\n"), "fn g() -> int {\n    return -1\n}\n");
        assert_eq!(f("print(xs[99] catch - 1, [- 1, -2], { \"a\": - 3 })\n"), "print(xs[99] catch -1, [-1, -2], { \"a\": -3 })\n");
        assert_eq!(f("if ! fs.exists(p) {\nx()\n}\n"), "if !fs.exists(p) {\n    x()\n}\n");
        assert_eq!(f("let e = - - x\n"), "let e = - -x\n");
        assert_eq!(f("let s = xs.sort_by(n => - n)\n"), "let s = xs.sort_by(n => -n)\n");
    }

    #[test]
    fn keyword_before_parentheses_and_brackets() {
        assert_eq!(f("if(a) {\nx()\n}\n"), "if (a) {\n    x()\n}\n");
        assert_eq!(f("for x in[1, 2] {\nprint(x)\n}\n"), "for x in [1, 2] {\n    print(x)\n}\n");
        assert_eq!(f("let l = fs.ls(\"x\") catch[\"a\"]\n"), "let l = fs.ls(\"x\") catch [\"a\"]\n");
        assert_eq!(f("fn g() -> []str {\nreturn[\"a\"]\n}\n"), "fn g() -> []str {\n    return [\"a\"]\n}\n");
    }

    #[test]
    fn ranges_stuck_together() {
        assert_eq!(f("for i in 0 .. 3 {\nprint(i)\n}\nfor i in 1 ..= n {\nprint(i)\n}\n"), "for i in 0..3 {\n    print(i)\n}\nfor i in 1..=n {\n    print(i)\n}\n");
        assert_eq!(f("let v = Versao {.. self, patch: 0}\n"), "let v = Versao { ..self, patch: 0 }\n");
    }

    #[test]
    fn generic_types() {
        assert_eq!(f("var m : map [ str ] int = {}\n"), "var m: map[str]int = {}\n");
        assert_eq!(f("var m: map[str] []int = {}\n"), "var m: map[str][]int = {}\n");
        assert_eq!(f("let xs: [ ] str = []\nlet ys: [] map[str]int = []\n"), "let xs: []str = []\nlet ys: []map[str]int = []\n");
        assert_eq!(f("fn m() -> map[str]int {\nreturn {}\n}\nfn n(xs: []str) -> []int {\nreturn [xs.len()]\n}\n"), "fn m() -> map[str]int {\n    return {}\n}\nfn n(xs: []str) -> []int {\n    return [xs.len()]\n}\n");
                // an ordinary comparison keeps its spaces
        assert_eq!(f("let ok = a<b && c>d\n"), "let ok = a < b && c > d\n");
                // a type at the end of a line does not indent the next one as a continuation
        assert_eq!(f("struct S {\nm: map[str]int\nn: int\n}\n"), "struct S {\n    m: map[str]int\n    n: int\n}\n");
    }

    #[test]
    fn braces() {
        assert_eq!(f("let s = Servico{nome:\"api\",porta:8080}\n"), "let s = Servico { nome: \"api\", porta: 8080 }\n");
        assert_eq!(f("let s = Servico { nome: \"api\",    porta: 8080 }\n"), "let s = Servico { nome: \"api\", porta: 8080 }\n");
        assert_eq!(f("let m = {\"api\":8080}\nlet v = {}\nlet w = {  }\n"), "let m = { \"api\": 8080 }\nlet v = {}\nlet w = {}\n");
        assert_eq!(f("enum Ambiente {dev,stage,prod}\n"), "enum Ambiente { dev, stage, prod }\n");
        assert_eq!(f("let r = if a {6} else {1}\n"), "let r = if a { 6 } else { 1 }\n");
        assert_eq!(f("let x = f({nome: \"a\"})\n"), "let x = f({ nome: \"a\" })\n");
        assert_eq!(f("struct S{\nnome: str\n}\n"), "struct S {\n    nome: str\n}\n");
    }

        // --- rule 5 -----------------------------------------------------------------------------

    #[test]
    fn blank_lines() {
        assert_eq!(
            f("\n\n\nlet a = 1\n\n\n\nlet b = 2\nif a {\n\n\nx()\n\n}\n\n\n"),
            "let a = 1\n\nlet b = 2\nif a {\n    x()\n}\n"
        );
        assert_eq!(f("let xs = [\n\n1,\n\n2,\n\n]\n"), "let xs = [\n    1,\n\n    2,\n]\n");
        // no line break at the end: one is added
        assert_eq!(f("print(1)"), "print(1)\n");
        assert_eq!(f("print(1)   \t"), "print(1)\n");
        assert_eq!(f(""), "");
        assert_eq!(f("\n\n  \n"), "");
    }

        // --- rule 6 -----------------------------------------------------------------------------

    #[test]
    fn comments() {
        assert_eq!(
            f("// topo   \nfn f() {\nx()\n        // último do bloco\n}\n  // antes do let\nlet a = 1 // fim\n/// doc\nparam_x()\n"),
            "// topo\nfn f() {\n    x()\n    // último do bloco\n}\n// antes do let\nlet a = 1  // fim\n/// doc\nparam_x()\n"
        );
                // the comment text does not change
        assert_eq!(f("//x    y\nlet a = 1 //sem espaço\n"), "//x    y\nlet a = 1  //sem espaço\n");
                // a comment between continuation lines gets the indentation of the following line
        assert_eq!(f("let s = xs\n// filtra\n.filter(x => x > 1)\n"), "let s = xs\n    // filtra\n    .filter(x => x > 1)\n");
                // empty block with a comment
        assert_eq!(f("fn f() {\n// nada\n}\n"), "fn f() {\n    // nada\n}\n");
                // /* */ stays as is (only loses trailing spaces on lines)
        assert_eq!(f("/* a  \n   b */\nlet a = 1 /* c */ + 2\n"), "/* a\n   b */\nlet a = 1 /* c */ + 2\n");
                // comment at the end of the file
        assert_eq!(f("let a = 1\n\n\n   // fim\n"), "let a = 1\n\n// fim\n");
    }

        // --- rule 7 -----------------------------------------------------------------------------

    #[test]
    fn aligns_trailing_comments() {
        assert_eq!(
            f("let a = 1 // um\nlet bbb = 22 // dois\n\nlet c = 3      // sozinho\n"),
            "let a = 1     // um\nlet bbb = 22  // dois\n\nlet c = 3  // sozinho\n"
        );
                // a comment-only line, a line without a comment or a different indentation breaks the sequence
        assert_eq!(
            f("let a = 1 // um\n// meio\nlet bb = 2 // dois\nlet c = 3\nlet ddd = 4 // quatro\nif a {\nx() // x\n}\n"),
            "let a = 1  // um\n// meio\nlet bb = 2  // dois\nlet c = 3\nlet ddd = 4  // quatro\nif a {\n    x()  // x\n}\n"
        );
        assert_eq!(
            f("struct S {\nnome: str // o nome\nporta: int    // a porta\n}\n"),
            "struct S {\n    nome: str   // o nome\n    porta: int  // a porta\n}\n"
        );
    }

    #[test]
    fn aligns_match_arms() {
        let src = "let host = match ambiente {\ndev => \"d\"\nstage   =>   \"s\"\nprod => \"p\"\n}\n";
        assert_eq!(f(src), "let host = match ambiente {\n    dev   => \"d\"\n    stage => \"s\"\n    prod  => \"p\"\n}\n");
                // a blank line, a comment and a multi-line arm break the sequence
        let src = "match x {\n0..10 => a()\n100 => b()\n\n\"x\" | \"yy\" => c()\n// c\n7 => d()\n_ => {\ne()\n}\n8 => f()\n99 => g()\n}\n";
        assert_eq!(
            f(src),
            "match x {\n    0..10 => a()\n    100   => b()\n\n    \"x\" | \"yy\" => c()\n    // c\n    7 => d()\n    _ => {\n        e()\n    }\n    8  => f()\n    99 => g()\n}\n"
        );
                // one-line-block arms align; lambdas inside the value do not count
        let src = "match x {\na => { print(1) }\nbbb => xs.map(n => n)\n}\n";
        assert_eq!(f(src), "match x {\n    a   => { print(1) }\n    bbb => xs.map(n => n)\n}\n");
        // match aninhado: cada um alinha os seus
        let src = "let r = match a {\n1 => match b {\nx => 1\nyyy => 2\n}\n22 => 3\n}\n";
        assert_eq!(f(src), "let r = match a {\n    1 => match b {\n        x   => 1\n        yyy => 2\n    }\n    22 => 3\n}\n");
        // os dois alinhamentos juntos
        let src = "let n = match x {\na => 1 // um\nbbbb => 22 // dois\n}\n";
        assert_eq!(f(src), "let n = match x {\n    a    => 1   // um\n    bbbb => 22  // dois\n}\n");
    }

        // --- rule 8 -----------------------------------------------------------------------------

    #[test]
    fn crlf_and_bom() {
        assert_eq!(f("let a=1\r\nif a {\r\nx()\r\n}\r\n"), "let a = 1\r\nif a {\r\n    x()\r\n}\r\n");
        assert_eq!(f("\u{feff}let a=1\n"), "\u{feff}let a = 1\n");
        assert_eq!(f("\u{feff}let a=1\r\n// c  \r\n"), "\u{feff}let a = 1\r\n// c\r\n");
                // `"""` with CRLF stays with CRLF inside
        assert_eq!(f("let t = \"\"\"\r\n  a\r\n  \"\"\"\r\n"), "let t = \"\"\"\r\n  a\r\n  \"\"\"\r\n");
    }

        // --- corpus ------------------------------------------------------------------------------

    fn zero_word(w: &mut LWord) {
        w.span = Span::default();
        for p in &mut w.parts {
            if let WPart::Code { offset, .. } = p {
                *offset = 0;
            }
        }
    }

    fn zero_cmd(c: &mut CmdTok) {
        for st in &mut c.stages {
            st.iter_mut().for_each(zero_word);
        }
        for r in &mut c.redirects {
            r.span = Span::default();
            if let Some(w) = &mut r.target {
                zero_word(w);
            }
        }
    }

    fn zero_str(s: &mut StrTok) {
        for p in &mut s.parts {
            if let LStrPart::Code { offset, .. } = p {
                *offset = 0;
            }
        }
    }

        /// The token sequence without positions (including those inside strings and commands).
    fn shape(src: &str) -> Vec<(Tok, Option<String>)> {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let (toks, _) = lexer::lex(src, 0);
        toks.into_iter()
            .map(|t| {
                let mut tok = t.tok;
                match &mut tok {
                    Tok::Str(s) => zero_str(s),
                    Tok::Cmd(c) | Tok::Capture(c) => zero_cmd(c),
                    _ => {}
                }
                (tok, t.doc)
            })
            .collect()
    }

    fn assert_tokens_equal(a: &str, b: &str) {
        let (sa, sb) = (shape(a), shape(b));
        if sa != sb {
            let i = sa.iter().zip(&sb).position(|(x, y)| x != y).unwrap_or(sa.len().min(sb.len()));
            panic!("the tokens changed at token {}: {:?} != {:?}\n--- formatted:\n{}", i, sa.get(i), sb.get(i), b);
        }
    }

        /// Lines of the output that are inside a multi-line token (`"""`), which stay as they are.
    fn verbatim_lines(src: &str) -> Vec<bool> {
        let n = src.split('\n').count();
        let mut v = vec![false; n];
        let (toks, _) = lexer::lex(src, 0);
        for t in toks {
            if matches!(t.tok, Tok::Nl | Tok::Eof) || t.span.end > src.len() {
                continue;
            }
            let text = &src[t.span.start..t.span.end];
            if text.contains('\n') {
                let first = src[..t.span.start].matches('\n').count();
                let k = text.matches('\n').count();
                for l in first + 1..=first + k {
                    v[l] = true;
                }
            }
        }
        v
    }

    fn corpus_files() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["examples", "tests/cases", "tests/pacote", "tests/errors", "tests/bench"] {
            let Ok(rd) = std::fs::read_dir(root.join(dir)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "nalu") {
                    files.push(p);
                }
            }
        }
        files.sort();
        files
    }

        /// Checks the three properties on a text; returns `false` if the syntax does not pass (skipped).
    fn check_corpus_text(name: &str, src: &str, fails: &mut Vec<String>) -> bool {
        let once = match format(src) {
            Ok(o) => o,
            Err(_) => return false,
        };
        match format(&once) {
            Ok(twice) if twice == once => {}
            Ok(twice) => fails.push(format!("{}: not idempotent:\n--- 1st\n{}\n--- 2nd\n{}", name, once, twice)),
            Err(d) => fails.push(format!("{}: the output does not pass the parser: {:?}", name, d)),
        }
        if shape(src) != shape(&once) {
            fails.push(format!("{}: the token sequence changed:\n{}", name, once));
        }
        let verb = verbatim_lines(&once);
        for (i, l) in once.split('\n').enumerate() {
            if verb.get(i).copied().unwrap_or(false) {
                continue;
            }
            if l.ends_with(' ') || l.ends_with('\t') {
                fails.push(format!("{}:{}: trailing space: {:?}", name, i + 1, l));
            }
            if l.trim_start_matches(' ').starts_with('\t') {
                fails.push(format!("{}:{}: tab in the indentation: {:?}", name, i + 1, l));
            }
        }
        if !once.is_empty() && (!once.ends_with('\n') || once.ends_with("\n\n")) {
            fails.push(format!("{}: must end with exactly one line break", name));
        }
        true
    }

    #[test]
    fn corpus_idempotent_and_same_tokens() {
        let mut fails = Vec::new();
        let (mut ok, mut skipped) = (0, Vec::new());
        for p in corpus_files() {
            let src = std::fs::read_to_string(&p).unwrap();
            let name = p.display().to_string();
            if check_corpus_text(&name, &src, &mut fails) {
                ok += 1;
            } else {
                                // only the deliberate error cases may be refused
                assert!(name.contains("tests/errors") || name.contains("tests\\errors"), "{} did not format", name);
                skipped.push(name);
            }
        }
        assert!(fails.is_empty(), "{} falha(s):\n{}", fails.len(), fails.join("\n\n"));
        assert!(ok >= 50, "corpus pequeno demais: {} formatados, {} pulados", ok, skipped.len());
        assert!(!skipped.is_empty(), "tests/errors has broken-syntax cases that should be refused");
    }

        /// The same program with the spacing between tokens messed up (extra spaces and tabs, changed indentation
        /// or removed); comments and line breaks stay.
    fn mangle(src: &str) -> String {
        let (toks, _) = lexer::lex(src, 0);
        let mut out = String::new();
        let mut pos = 0;
        for (k, t) in toks.iter().filter(|t| !matches!(t.tok, Tok::Nl | Tok::Eof)).enumerate() {
            let gap = &src[pos..t.span.start];
            if gap.contains("//") || gap.contains("/*") {
                out.push_str(gap);
            } else if gap.contains('\n') {
                out.push_str(&"\n".repeat(gap.matches('\n').count()));
                out.push_str(["", "\t ", "  "][k % 3]);
            } else {
                out.push_str(["  \t", " ", "   "][k % 3]);
            }
            out.push_str(&src[t.span.start..t.span.end]);
            pos = t.span.end;
        }
        out.push_str(&src[pos..]);
        out
    }

    #[test]
    fn original_spacing_does_not_matter() {
        let mut n = 0;
        for p in corpus_files() {
            let src = std::fs::read_to_string(&p).unwrap();
            let Ok(want) = format(&src) else { continue };
            let bagunca = mangle(&src);
            match format(&bagunca) {
                Ok(got) => assert_eq!(got, want, "{}: the output depends on the original's spacing", p.display()),
                Err(d) => panic!("{}: the messed-up version did not pass the parser: {:?}\n{}", p.display(), d, bagunca),
            }
            n += 1;
        }
        assert!(n >= 50);
    }

        /// Each initial piece of the corpus files: never panics and, when the syntax passes, the same
        /// properties hold (catches odd cases, such as blocks and comments at the end of the file).
    #[test]
    fn corpus_prefixes() {
        let mut fails = Vec::new();
        let mut ok = 0;
        for p in corpus_files() {
            let src = std::fs::read_to_string(&p).unwrap();
            let mut cut = 0;
            while cut < src.len() {
                cut = (cut + 13).min(src.len());
                while !src.is_char_boundary(cut) {
                    cut += 1;
                }
                if check_corpus_text(&format!("{}[..{}]", p.display(), cut), &src[..cut], &mut fails) {
                    ok += 1;
                }
            }
        }
        assert!(fails.is_empty(), "{} falha(s):\n{}", fails.len(), fails.iter().take(5).cloned().collect::<Vec<_>>().join("\n\n"));
        assert!(ok > 500, "only {} prefixes formatted", ok);
    }

    #[test]
    fn examples_and_cases_format() {
                // the requested directories: examples, tests/cases, tests/pacote (none is broken)
        let n = corpus_files()
            .iter()
            .filter(|p| !p.display().to_string().contains("errors") && !p.display().to_string().contains("bench"))
            .inspect(|p| {
                let src = std::fs::read_to_string(p).unwrap();
                assert!(format(&src).is_ok(), "{} did not format", p.display());
            })
            .count();
        assert!(n >= 40, "only {} files", n);
    }

        /// The code snippets in the documentation too (those that pass the parser).
    #[test]
    fn documentation_snippets() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut docs: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("docs"))
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "md")).collect())
            .unwrap_or_default();
        docs.push(root.join("README.md"));
        let mut fails = Vec::new();
        let mut ok = 0;
        for d in docs {
            let Ok(text) = std::fs::read_to_string(&d) else { continue };
            let mut block: Option<String> = None;
            for (i, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    match block.take() {
                        Some(b) => {
                            if check_corpus_text(&format!("{}:{}", d.display(), i), &b, &mut fails) {
                                ok += 1;
                            }
                        }
                        None => block = Some(String::new()),
                    }
                } else if let Some(b) = &mut block {
                    b.push_str(line);
                    b.push('\n');
                }
            }
        }
        assert!(fails.is_empty(), "{} falha(s):\n{}", fails.len(), fails.join("\n\n"));
        assert!(ok >= 10, "only {} snippets", ok);
    }
}
