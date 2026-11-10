//! Nalu's portable regular expressions (D32).
//!
//! One syntax, checked at compile time and translated twice: to POSIX ERE for Bash (`[[ =~ ]]`) and to
//! .NET for PowerShell. The translation leaves nothing to the engines' defaults: ASCII ranges are written
//! out (no locale collation), `\d \w \s` become explicit classes, `.` matches any character including a
//! newline (and a whole surrogate pair in .NET), `^`/`$` are the start/end of the whole text, and `(?i)`
//! becomes `[aA]` classes.
//!
//! The two engines pick matches differently (POSIX: the longest at the leftmost start; .NET: the first one
//! its backtracking finds), so functions that return matched text (`find`, `find_all`, `groups`,
//! `replace`) only accept patterns with a single way to match ([`Regex::check_unambiguous`]); then both
//! engines walk the same path and return the same text and groups. `regex.test` accepts any valid pattern,
//! because whether a match exists does not depend on the engine.

use std::collections::BTreeSet;

/// A parsed pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    node: Node,
    /// Starts with `^` / ends with `$`.
    pub start: bool,
    pub end: bool,
    /// Number of capturing groups.
    pub groups: usize,
}

/// One character: what a literal, a class, `\d` or `.` accepts.
#[derive(Clone, Debug, PartialEq)]
struct Class {
    /// Sorted, merged code point ranges (what the class accepts, after negation).
    ranges: Vec<(u32, u32)>,
    /// How to write it: `.`, a negated class `[^…]` (`items` are the excluded ranges) or a plain set.
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Dot,
    Neg(Vec<(u32, u32)>),
    Set,
}

#[derive(Clone, Debug)]
enum Node {
    Char(Class),
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, u32, Option<u32>),
    Group(Box<Node>),
}

/// A pattern error: the message, and the help line.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub message: String,
    pub help: String,
}

fn error(message: impl Into<String>, help: impl Into<String>) -> Error {
    Error { message: message.into(), help: help.into() }
}

const MAX_REPEAT: u32 = 255;
const MAX_POSITIONS: usize = 4000;
const SPECIALS: &str = "\\.*+?|()[]{}^$/-";

// -------------------------------------------------------------------------------------------------
// Parsing
// -------------------------------------------------------------------------------------------------

struct Parser {
    chars: Vec<char>,
    pos: usize,
    groups: usize,
    fold: bool,
}

impl Regex {
    pub fn parse(src: &str) -> Result<Regex, Error> {
        let mut p = Parser { chars: src.chars().collect(), pos: 0, groups: 0, fold: false };
        if src.starts_with("(?i)") {
            p.fold = true;
            p.pos = 4;
        }
        let start = p.eat('^');
        let node = p.alt(0)?;
        let end = p.eat('$');
        if p.pos < p.chars.len() {
            let c = p.chars[p.pos];
            return Err(match c {
                ')' => error("`)` without a matching `(`", "for the character, write `\\)`"),
                '^' => error("`^` only goes at the start of the pattern", "for the character, write `\\^`"),
                '$' => error("`$` only goes at the end of the pattern", "for the character, write `\\$`"),
                _ => error(format!("unexpected `{}`", c), "for the character, put `\\` before it"),
            });
        }
        if matches!(&node, Node::Seq(v) if v.is_empty()) {
            return Err(error("the pattern is empty", "write what the text should contain, like `'\\d+'`"));
        }
        if (start || end) && matches!(node, Node::Alt(_)) {
            return Err(error("`|` next to `^` or `$` is unclear", "put the alternatives in a group: `^(a|b)$`"));
        }
        Ok(Regex { node, start, end, groups: p.groups })
    }
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// The `$` that ends the pattern (not one inside it).
    fn at_final_dollar(&self) -> bool {
        self.peek() == Some('$') && self.pos + 1 == self.chars.len()
    }

    fn alt(&mut self, depth: usize) -> Result<Node, Error> {
        let mut branches = vec![self.seq(depth)?];
        while self.eat('|') {
            branches.push(self.seq(depth)?);
        }
        if branches.len() == 1 {
            return Ok(branches.pop().unwrap());
        }
        if branches.iter().any(|b| matches!(b, Node::Seq(v) if v.is_empty())) {
            return Err(error("an empty branch in `|`", "to make something optional, use `?`: `(a|b)?`"));
        }
        Ok(Node::Alt(branches))
    }

    fn seq(&mut self, depth: usize) -> Result<Node, Error> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') => break,
                Some(')') if depth > 0 => break,
                Some(')') => return Err(error("`)` without a matching `(`", "for the character, write `\\)`")),
                Some('$') if self.at_final_dollar() => break,
                _ => {}
            }
            let atom = self.atom(depth)?;
            items.push(self.quantified(atom)?);
        }
        Ok(if items.len() == 1 { items.pop().unwrap() } else { Node::Seq(items) })
    }

    fn quantified(&mut self, atom: Node) -> Result<Node, Error> {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let save = self.pos;
                match self.bounds()? {
                    Some(b) => {
                        self.pos -= 1; // `bounds` consumed the `}`; the common path below consumes one char
                        b
                    }
                    None => {
                        self.pos = save;
                        return Err(error("`{` starts a repetition like `{3}` or `{2,5}`", "for the character, write `\\{`"));
                    }
                }
            }
            _ => return Ok(atom),
        };
        self.pos += 1;
        match self.peek() {
            Some('?') => return Err(error("lazy repetition (`*?`, `+?`…) isn't supported", "describe exactly what may repeat, like `[^\"]*` instead of `.*?`")),
            Some('+') => return Err(error("possessive repetition (`*+`, `++`…) isn't supported", "remove the second `+`")),
            Some('*') | Some('{') => return Err(error("two repetitions in a row", "put the repeated part in a group: `(a+)*`")),
            _ => {}
        }
        Ok(Node::Repeat(Box::new(atom), min, max))
    }

    /// `{n}`, `{n,}`, `{n,m}` starting at `{`; `None` if it isn't a repetition.
    fn bounds(&mut self) -> Result<Option<(u32, Option<u32>)>, Error> {
        self.pos += 1;
        let n = self.number();
        let Some(n) = n else { return Ok(None) };
        let max = if self.eat(',') {
            if self.peek() == Some('}') {
                None
            } else {
                match self.number() {
                    Some(m) => Some(m),
                    None => return Ok(None),
                }
            }
        } else {
            Some(n)
        };
        if !self.eat('}') {
            return Ok(None);
        }
        if n > MAX_REPEAT || max.is_some_and(|m| m > MAX_REPEAT) {
            return Err(error(format!("repetitions go up to {}", MAX_REPEAT), "Bash's regular expressions don't accept more"));
        }
        if let Some(m) = max {
            if m < n {
                return Err(error(format!("`{{{},{}}}`: the maximum is smaller than the minimum", n, m), "write the smaller number first"));
            }
            if m == 0 {
                return Err(error("`{0}` repeats nothing", "remove the part"));
            }
        }
        Ok(Some((n, max)))
    }

    fn number(&mut self) -> Option<u32> {
        let s = self.pos;
        while matches!(self.peek(), Some('0'..='9')) {
            self.pos += 1;
        }
        if s == self.pos || self.pos - s > 4 {
            return None;
        }
        self.chars[s..self.pos].iter().collect::<String>().parse().ok()
    }

    fn atom(&mut self, depth: usize) -> Result<Node, Error> {
        let c = self.peek().unwrap();
        self.pos += 1;
        match c {
            '(' => {
                if self.peek() == Some('?') {
                    return Err(error(
                        "`(?…)` isn't supported",
                        "use a plain group `( … )`; `(?i)` is accepted only at the very start of the pattern",
                    ));
                }
                self.groups += 1;
                let inner = self.alt(depth + 1)?;
                if !self.eat(')') {
                    return Err(error("`(` without a matching `)`", "close the group, or write `\\(` for the character"));
                }
                if matches!(&inner, Node::Seq(v) if v.is_empty()) {
                    return Err(error("an empty group `()`", "remove it"));
                }
                Ok(Node::Group(Box::new(inner)))
            }
            '[' => self.class(),
            '.' => Ok(Node::Char(Class { ranges: vec![(0, 0x10FFFF)], kind: Kind::Dot })),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err(error(format!("`{}` has nothing to repeat", c), format!("for the character, write `\\{}`", c))),
            '{' => {
                self.pos -= 1;
                if self.bounds()?.is_some() {
                    return Err(error("a repetition with nothing to repeat", "for the character, write `\\{`"));
                }
                Err(error("`{` starts a repetition like `{3}` or `{2,5}`", "for the character, write `\\{`"))
            }
            '^' => Err(error("`^` only goes at the start of the pattern", "for the character, write `\\^`")),
            '$' => Err(error("`$` only goes at the end of the pattern", "for the character, write `\\$`")),
            _ => Ok(Node::Char(self.literal(c))),
        }
    }

    fn literal(&self, c: char) -> Class {
        let mut r = vec![(c as u32, c as u32)];
        if self.fold && c.is_ascii_alphabetic() {
            let o = if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() };
            r.push((o as u32, o as u32));
        }
        Class { ranges: merge(r), kind: Kind::Set }
    }

    fn escape(&mut self) -> Result<Node, Error> {
        let Some(c) = self.peek() else {
            return Err(error("the pattern ends with `\\`", "for the character, write `\\\\`"));
        };
        self.pos += 1;
        if let Some(r) = shorthand(c) {
            return Ok(Node::Char(Class { ranges: merge(r), kind: Kind::Set }));
        }
        if let Some(r) = shorthand(c.to_ascii_lowercase()).filter(|_| c.is_ascii_uppercase()) {
            let r = merge(r);
            return Ok(Node::Char(Class { ranges: complement(&r), kind: Kind::Neg(r) }));
        }
        match control(c) {
            Some(x) => Ok(Node::Char(self.literal(x))),
            None if SPECIALS.contains(c) => Ok(Node::Char(self.literal(c))),
            None => Err(unknown_escape(c)),
        }
    }

    fn class(&mut self) -> Result<Node, Error> {
        let neg = self.eat('^');
        let mut items: Vec<(u32, u32)> = Vec::new();
        if self.peek() == Some(']') {
            return Err(error("an empty class `[]`", "for the character `]`, write `\\]`"));
        }
        loop {
            let Some(c) = self.peek() else {
                return Err(error("`[` without a matching `]`", "close the class, or write `\\[` for the character"));
            };
            self.pos += 1;
            let lo = match c {
                ']' => break,
                '[' if matches!(self.peek(), Some(':') | Some('.') | Some('=')) => {
                    return Err(error("POSIX classes like `[[:alpha:]]` aren't supported", "write the characters: `[a-zA-Z]`, `\\d`, `\\w`, `\\s`"));
                }
                '\\' => {
                    let Some(e) = self.peek() else {
                        return Err(error("the pattern ends with `\\`", "for the character, write `\\\\`"));
                    };
                    self.pos += 1;
                    if let Some(r) = shorthand(e) {
                        items.extend(r);
                        continue;
                    }
                    if e.is_ascii_uppercase() && shorthand(e.to_ascii_lowercase()).is_some() {
                        return Err(error(format!("`\\{}` inside `[ ]` isn't supported", e), "write the class out, like `[^0-9]` for `\\D`"));
                    }
                    match control(e) {
                        Some(x) => x,
                        None if SPECIALS.contains(e) => e,
                        None => return Err(unknown_escape(e)),
                    }
                }
                _ => c,
            };
            // a range `a-z` (a `-` first or last is the character itself)
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1;
                let mut hi = self.peek().unwrap();
                self.pos += 1;
                if hi == '\\' {
                    let Some(e) = self.peek() else {
                        return Err(error("the pattern ends with `\\`", "for the character, write `\\\\`"));
                    };
                    self.pos += 1;
                    hi = match control(e) {
                        Some(x) => x,
                        None if SPECIALS.contains(e) => e,
                        None => return Err(error(format!("`\\{}` can't end a range", e), "write the range with two characters, like `a-z`")),
                    };
                }
                if !lo.is_ascii() || !hi.is_ascii() {
                    return Err(error(format!("`{}-{}`: ranges only go between ASCII characters", lo, hi), "list the other characters one by one"));
                }
                if hi < lo {
                    return Err(error(format!("`{}-{}`: the range goes backwards", lo, hi), format!("write `{}-{}`", hi, lo)));
                }
                items.push((lo as u32, hi as u32));
            } else {
                if (lo as u32) > 0xFFFF {
                    return Err(error(format!("`{}` inside `[ ]` isn't supported", lo), "put it outside the class, with `|`"));
                }
                items.push((lo as u32, lo as u32));
            }
        }
        if self.fold {
            let mut extra = Vec::new();
            for &(a, b) in &items {
                for x in a..=b.min(127) {
                    let ch = char::from_u32(x).unwrap();
                    if ch.is_ascii_alphabetic() {
                        let o = if ch.is_ascii_lowercase() { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() };
                        extra.push((o as u32, o as u32));
                    }
                }
            }
            items.extend(extra);
        }
        let items = merge(items);
        Ok(Node::Char(if neg { Class { ranges: complement(&items), kind: Kind::Neg(items) } } else { Class { ranges: items, kind: Kind::Set } }))
    }
}

fn unknown_escape(c: char) -> Error {
    let help = match c {
        'b' | 'B' => "word boundaries aren't portable; match the characters around the word instead, like `(^|[^a-z])`".to_string(),
        '1'..='9' => "back-references aren't supported".to_string(),
        _ => "escapes: `\\d \\w \\s` (and `\\D \\W \\S`), `\\n \\t \\r`, and `\\` before a special character like `\\.`".to_string(),
    };
    error(format!("`\\{}` isn't a supported escape", c), help)
}

fn shorthand(c: char) -> Option<Vec<(u32, u32)>> {
    let r = |a: char, b: char| (a as u32, b as u32);
    Some(match c {
        'd' => vec![r('0', '9')],
        'w' => vec![r('0', '9'), r('A', 'Z'), r('_', '_'), r('a', 'z')],
        's' => vec![r(' ', ' '), (9, 13)],
        _ => return None,
    })
}

fn control(c: char) -> Option<char> {
    Some(match c {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'f' => '\x0c',
        'v' => '\x0b',
        _ => return None,
    })
}

fn merge(mut r: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    r.sort();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(last) if a <= last.1.saturating_add(1) => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn complement(r: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut next = 0u32;
    for &(a, b) in r {
        if a > next {
            out.push((next, a - 1));
        }
        next = b + 1;
    }
    if next <= 0x10FFFF {
        out.push((next, 0x10FFFF));
    }
    out
}

fn overlap(a: &[(u32, u32)], b: &[(u32, u32)]) -> Option<u32> {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo <= hi {
            return Some(lo);
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    None
}

// -------------------------------------------------------------------------------------------------
// The single-way check
// -------------------------------------------------------------------------------------------------

struct Glushkov {
    classes: Vec<Class>,
    follow: Vec<BTreeSet<usize>>,
}

struct Info {
    nullable: bool,
    first: BTreeSet<usize>,
    last: BTreeSet<usize>,
}

fn nullable(n: &Node) -> bool {
    match n {
        Node::Char(_) => false,
        Node::Seq(v) => v.iter().all(nullable),
        Node::Alt(v) => v.iter().any(nullable),
        Node::Repeat(x, min, _) => *min == 0 || nullable(x),
        Node::Group(x) => nullable(x),
    }
}

fn has_group(n: &Node) -> bool {
    match n {
        Node::Char(_) => false,
        Node::Seq(v) | Node::Alt(v) => v.iter().any(has_group),
        Node::Repeat(x, _, _) => has_group(x),
        Node::Group(_) => true,
    }
}

impl Glushkov {
    fn leaf(&mut self, c: &Class) -> Result<Info, Error> {
        if self.classes.len() >= MAX_POSITIONS {
            return Err(error("the pattern is too long to check", "split it, or use `regex.test`"));
        }
        let p = self.classes.len();
        self.classes.push(c.clone());
        self.follow.push(BTreeSet::new());
        Ok(Info { nullable: false, first: [p].into(), last: [p].into() })
    }

    fn seq(&mut self, a: Info, b: Info) -> Info {
        for &p in &a.last {
            self.follow[p].extend(b.first.iter().copied());
        }
        let mut first = a.first.clone();
        if a.nullable {
            first.extend(b.first.iter().copied());
        }
        let mut last = b.last.clone();
        if b.nullable {
            last.extend(a.last.iter().copied());
        }
        Info { nullable: a.nullable && b.nullable, first, last }
    }

    fn star(&mut self, x: Info) -> Info {
        for &p in &x.last {
            self.follow[p].extend(x.first.iter().copied());
        }
        Info { nullable: true, first: x.first, last: x.last }
    }

    fn empty() -> Info {
        Info { nullable: true, first: BTreeSet::new(), last: BTreeSet::new() }
    }

    fn opt(x: Info) -> Info {
        Info { nullable: true, ..x }
    }

    fn walk(&mut self, n: &Node) -> Result<Info, Error> {
        Ok(match n {
            Node::Char(c) => self.leaf(c)?,
            Node::Group(x) => self.walk(x)?,
            Node::Seq(v) => {
                let mut acc = Self::empty();
                for x in v {
                    let i = self.walk(x)?;
                    acc = self.seq(acc, i);
                }
                acc
            }
            Node::Alt(v) => {
                let mut acc = Info { nullable: false, first: BTreeSet::new(), last: BTreeSet::new() };
                for x in v {
                    let i = self.walk(x)?;
                    acc.nullable |= i.nullable;
                    acc.first.extend(i.first);
                    acc.last.extend(i.last);
                }
                acc
            }
            // x{n,m} = x … x (x (x)?)? with fresh positions for each copy
            Node::Repeat(x, min, max) => {
                let mut acc = Self::empty();
                for _ in 0..*min {
                    let i = self.walk(x)?;
                    acc = self.seq(acc, i);
                }
                match max {
                    None => {
                        let i = self.walk(x)?;
                        let s = self.star(i);
                        acc = self.seq(acc, s);
                    }
                    Some(m) => {
                        let extra = m - min;
                        let mut tail = Self::empty();
                        for _ in 0..extra {
                            let i = self.walk(x)?;
                            let inner = self.seq(i, tail);
                            tail = Self::opt(inner);
                        }
                        acc = self.seq(acc, tail);
                    }
                }
                acc
            }
        })
    }
}

impl Regex {
    /// For functions that return matched text: a single way to match, so Bash and .NET agree.
    pub fn check_unambiguous(&self) -> Result<(), Error> {
        fn rules(n: &Node, in_repeat: bool) -> Result<(), Error> {
            match n {
                Node::Char(_) => Ok(()),
                Node::Seq(v) => v.iter().try_for_each(|x| rules(x, in_repeat)),
                Node::Alt(v) => {
                    if v.iter().any(nullable) {
                        return Err(error(
                            "a branch of `|` can match empty text, so the targets could choose different branches",
                            "make every branch match at least one character, and put `?` on the group: `(a|b)?`",
                        ));
                    }
                    v.iter().try_for_each(|x| rules(x, in_repeat))
                }
                Node::Repeat(x, _, max) => {
                    if nullable(x) {
                        return Err(error("this repeats something that can match empty text", "remove the inner `*` or `?`, like `(a*)+` → `a*`"));
                    }
                    let many = max.map_or(true, |m| m > 1);
                    let inner = match &**x {
                        Node::Group(g) => &**g,
                        other => other,
                    };
                    if (many && has_group(inner)) || (in_repeat && has_group(x)) {
                        return Err(error(
                            "a group inside a repeated group: the targets keep different values for it",
                            "capture the whole repetition with one group and a class, like `([a-z.]+)`",
                        ));
                    }
                    rules(x, in_repeat || many)
                }
                Node::Group(x) => rules(x, in_repeat),
            }
        }
        rules(&self.node, false)?;
        let mut g = Glushkov { classes: Vec::new(), follow: Vec::new() };
        let info = g.walk(&self.node)?;
        let conflict = |set: &BTreeSet<usize>| -> Option<(usize, usize, u32)> {
            let v: Vec<usize> = set.iter().copied().collect();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    if let Some(c) = overlap(&g.classes[v[i]].ranges, &g.classes[v[j]].ranges) {
                        return Some((v[i], v[j], c));
                    }
                }
            }
            None
        };
        let found = conflict(&info.first).or_else(|| g.follow.iter().find_map(|f| conflict(f)));
        if let Some((a, b, c)) = found {
            let ch = char::from_u32(c).map(show_char).unwrap_or_else(|| format!("U+{:04X}", c));
            let ka = &g.classes[a].kind;
            let kb = &g.classes[b].kind;
            let help = if *ka == Kind::Dot || *kb == Kind::Dot {
                format!("`.` also matches {}: exclude it, like `[^{}]*` instead of `.*`", ch, class_char(c))
            } else {
                format!("make the parts that can come next start with different characters (both can be {}), or use `regex.test`", ch)
            };
            return Err(error(format!("this pattern can match the same text in more than one way ({} can be read by two parts of it)", ch), help));
        }
        Ok(())
    }

    pub fn can_match_empty(&self) -> bool {
        nullable(&self.node)
    }
}

fn show_char(c: char) -> String {
    match c {
        '\n' => "a newline".into(),
        '\t' => "a tab".into(),
        ' ' => "a space".into(),
        c if c.is_control() => format!("U+{:04X}", c as u32),
        c => format!("`{}`", c),
    }
}

fn class_char(c: u32) -> String {
    match char::from_u32(c) {
        Some('\n') => "\\n".into(),
        Some('\t') => "\\t".into(),
        Some(c) if "\\]^-[".contains(c) => format!("\\{}", c),
        Some(c) => c.to_string(),
        None => String::new(),
    }
}

// -------------------------------------------------------------------------------------------------
// Translation
// -------------------------------------------------------------------------------------------------

impl Regex {
    /// POSIX ERE for Bash's `[[ text =~ $re ]]` (a UTF-8 locale is set by the runtime).
    pub fn to_ere(&self) -> String {
        let mut s = String::new();
        if self.start {
            s.push('^');
        }
        ere(&self.node, &mut s, false);
        if self.end {
            s.push('$');
        }
        s
    }

    /// .NET (PowerShell), with no options needed: the semantics are in the text.
    pub fn to_dotnet(&self) -> String {
        let mut s = String::new();
        if self.start {
            s.push_str("\\A");
        }
        net(&self.node, &mut s, false);
        if self.end {
            s.push_str("\\z");
        }
        s
    }
}

/// `atom`: the node is under a quantifier and must be a single unit.
fn ere(n: &Node, s: &mut String, atom: bool) {
    match n {
        Node::Char(c) => ere_class(c, s),
        Node::Group(x) => {
            s.push('(');
            ere(x, s, false);
            s.push(')');
        }
        Node::Seq(v) => {
            if atom {
                s.push('(');
            }
            for x in v {
                ere(x, s, false);
            }
            if atom {
                s.push(')');
            }
        }
        Node::Alt(v) => {
            // `Alt` is only ever the whole pattern (without anchors) or the inside of a group
            for (i, x) in v.iter().enumerate() {
                if i > 0 {
                    s.push('|');
                }
                ere(x, s, false);
            }
        }
        Node::Repeat(x, min, max) => {
            ere(x, s, true);
            quant(s, *min, *max);
        }
    }
}

fn quant(s: &mut String, min: u32, max: Option<u32>) {
    match (min, max) {
        (0, None) => s.push('*'),
        (1, None) => s.push('+'),
        (0, Some(1)) => s.push('?'),
        (n, None) => s.push_str(&format!("{{{},}}", n)),
        (n, Some(m)) if n == m => s.push_str(&format!("{{{}}}", n)),
        (n, Some(m)) => s.push_str(&format!("{{{},{}}}", n, m)),
    }
}

fn ere_class(c: &Class, s: &mut String) {
    match &c.kind {
        Kind::Dot => s.push('.'),
        Kind::Set if c.ranges.len() == 1 && c.ranges[0].0 == c.ranges[0].1 => {
            let ch = char::from_u32(c.ranges[0].0).unwrap();
            if "\\.[()*+?{|^$".contains(ch) {
                s.push('\\');
            }
            s.push(ch);
        }
        Kind::Set => ere_bracket(&c.ranges, false, s),
        Kind::Neg(items) => ere_bracket(items, true, s),
    }
}

/// A bracket expression with every character written out (no ranges: they depend on the locale).
/// `]` goes first, `[` `^` `-` last, and `^` never first (it would negate).
fn ere_bracket(items: &[(u32, u32)], neg: bool, s: &mut String) {
    let mut chars: Vec<char> = Vec::new();
    for &(a, b) in items {
        for x in a..=b {
            if let Some(ch) = char::from_u32(x) {
                chars.push(ch);
            }
        }
    }
    let has = |c: char| chars.contains(&c);
    let (close, dash, open, caret) = (has(']'), has('-'), has('['), has('^'));
    let rest: Vec<char> = chars.iter().copied().filter(|c| !matches!(c, ']' | '-' | '[' | '^')).collect();
    if !neg && caret && !close && !dash && !open && rest.is_empty() {
        s.push_str("\\^");
        return;
    }
    s.push('[');
    if neg {
        s.push('^');
    }
    if close {
        s.push(']');
    }
    s.extend(rest.iter());
    let nothing_yet = !close && rest.is_empty();
    if open {
        s.push('[');
    }
    if caret {
        if !neg && nothing_yet && !open {
            // only `^` and `-`: `[-^]`
            s.push_str("-^]");
            return;
        }
        s.push('^');
    }
    if dash {
        s.push('-');
    }
    s.push(']');
}

fn net(n: &Node, s: &mut String, atom: bool) {
    match n {
        Node::Char(c) => net_class(c, s),
        Node::Group(x) => {
            s.push('(');
            net(x, s, false);
            s.push(')');
        }
        Node::Seq(v) => {
            if atom {
                s.push_str("(?:");
            }
            for x in v {
                net(x, s, false);
            }
            if atom {
                s.push(')');
            }
        }
        Node::Alt(v) => {
            for (i, x) in v.iter().enumerate() {
                if i > 0 {
                    s.push('|');
                }
                net(x, s, false);
            }
        }
        Node::Repeat(x, min, max) => {
            // a character outside the BMP is two UTF-16 units: the repetition must cover both
            let pair = matches!(&**x, Node::Char(c) if c.kind == Kind::Set && c.ranges.len() == 1 && c.ranges[0].0 > 0xFFFF);
            if pair {
                s.push_str("(?:");
            }
            net(x, s, true);
            if pair {
                s.push(')');
            }
            quant(s, *min, *max);
        }
    }
}

const PAIR: &str = "[\\uD800-\\uDBFF][\\uDC00-\\uDFFF]";

fn net_class(c: &Class, s: &mut String) {
    match &c.kind {
        // any character, a surrogate pair counting as one
        Kind::Dot => s.push_str(&format!("(?:{}|[\\s\\S])", PAIR)),
        Kind::Set if c.ranges.len() == 1 && c.ranges[0].0 == c.ranges[0].1 => net_char(c.ranges[0].0, false, s),
        Kind::Set => {
            s.push('[');
            for &(a, b) in &c.ranges {
                net_char(a, true, s);
                if b > a {
                    s.push('-');
                    net_char(b, true, s);
                }
            }
            s.push(']');
        }
        Kind::Neg(items) => {
            s.push_str(&format!("(?:{}|[^", PAIR));
            for &(a, b) in items {
                net_char(a, true, s);
                if b > a {
                    s.push('-');
                    net_char(b, true, s);
                }
            }
            s.push_str("])");
        }
    }
}

/// One character in .NET syntax, ASCII only (the generated `.ps1` is ASCII).
fn net_char(x: u32, in_class: bool, s: &mut String) {
    match char::from_u32(x) {
        Some(c) if c.is_ascii_graphic() || c == ' ' => {
            let special = if in_class { "\\]^-[" } else { "\\*+?|{}[]()^$.#" };
            if special.contains(c) {
                s.push('\\');
            }
            s.push(c);
        }
        Some('\n') => s.push_str("\\n"),
        Some('\t') => s.push_str("\\t"),
        Some('\r') => s.push_str("\\r"),
        Some('\x0b') => s.push_str("\\v"),
        Some('\x0c') => s.push_str("\\f"),
        _ if x <= 0xFFFF => s.push_str(&format!("\\u{:04X}", x)),
        _ => {
            let v = x - 0x10000;
            s.push_str(&format!("\\u{:04X}\\u{:04X}", 0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(p: &str) -> Regex {
        Regex::parse(p).unwrap_or_else(|e| panic!("{}: {}", p, e.message))
    }

    fn bad(p: &str) -> String {
        match Regex::parse(p) {
            Ok(_) => panic!("{} should fail", p),
            Err(e) => e.message,
        }
    }

    fn single(p: &str) -> bool {
        ok(p).check_unambiguous().is_ok()
    }

    #[test]
    fn translates_to_both_engines() {
        let r = ok(r"^v(\d+)\.(\d+)$");
        assert_eq!(r.to_ere(), "^v([0123456789]+)\\.([0123456789]+)$");
        assert_eq!(r.to_dotnet(), "\\Av([0-9]+)\\.([0-9]+)\\z");
        assert_eq!(r.groups, 2);
        let r = ok("a.b");
        assert_eq!(r.to_ere(), "a.b");
        assert_eq!(r.to_dotnet(), format!("a(?:{}|[\\s\\S])b", PAIR));
        assert_eq!(ok("[^a-c]").to_ere(), "[^abc]");
        assert_eq!(ok(r"\s").to_dotnet(), "[\\t-\\r ]");
        assert_eq!(ok("👍+").to_dotnet(), "(?:\\uD83D\\uDC4D)+");
    }

    #[test]
    fn brackets_put_special_characters_where_they_are_literal() {
        assert_eq!(ok(r"[\]a\-]").to_ere(), "[]a-]");
        assert_eq!(ok(r"[\^]").to_ere(), "\\^");
        assert_eq!(ok(r"[\^a]").to_ere(), "[a^]");
        assert_eq!(ok(r"[\[:]").to_ere(), "[:[]");
        assert_eq!(ok(r"[\^\-]").to_ere(), "[-^]");
        assert_eq!(ok(r"[^\^]").to_ere(), "[^^]");
        assert_eq!(ok(r"[\]\[]").to_dotnet(), "[\\[\\]]");
    }

    #[test]
    fn case_folding_writes_both_cases() {
        assert_eq!(ok("(?i)yes").to_ere(), "[Yy][Ee][Ss]");
        assert_eq!(ok("(?i)[a-b]").to_dotnet(), "[A-Ba-b]");
    }

    #[test]
    fn non_ascii_is_written_as_escapes_in_dotnet() {
        assert_eq!(ok("é").to_dotnet(), "\\u00E9");
        assert_eq!(ok("👍").to_dotnet(), "\\uD83D\\uDC4D");
        assert_eq!(ok("é").to_ere(), "é");
    }

    #[test]
    fn errors_name_the_problem() {
        assert!(bad("a**").contains("two repetitions"));
        assert!(bad("*a").contains("nothing to repeat"));
        assert!(bad("(a").contains("without a matching `)`"));
        assert!(bad("a)").contains("without a matching `(`"));
        assert!(bad("[a").contains("without a matching `]`"));
        assert!(bad("a^").contains("only goes at the start"));
        assert!(bad("a$b").contains("only goes at the end"));
        assert!(bad(r"\bx").contains("isn't a supported escape"));
        assert!(bad("(?:a)").contains("isn't supported"));
        assert!(bad("a*?").contains("lazy"));
        assert!(bad("[[:alpha:]]").contains("POSIX classes"));
        assert!(bad("[z-a]").contains("backwards"));
        assert!(bad("a{3,2}").contains("smaller"));
        assert!(bad("a{300}").contains("255"));
        assert!(bad("a|").contains("empty branch"));
        assert!(bad("").contains("empty"));
        assert!(bad("{").contains("repetition"));
        assert!(bad("^a|b").contains("unclear"));
    }

    #[test]
    fn single_way_patterns() {
        assert!(single(r"\d+\.\d+\.\d+"));
        assert!(single(r"^v(\d+)\.(\d+)$"));
        assert!(single(r"^([^=]*)=(.*)$"));
        assert!(single(r"(dev|prod)-\d+"));
        assert!(single(r"-?\d+"));
        assert!(single(r"\d+(\.\d+)?"));
        assert!(single(r"[a-z]+@[a-z]+\.com"));
        assert!(single(r"a{2,4}"));
        assert!(single(r"(ab)+"));
    }

    #[test]
    fn ambiguous_patterns() {
        assert!(!single(r"^(.*)=(.*)$"));
        assert!(!single(r"a*(ab)*"));
        assert!(!single(r"(a|ab)c"));
        assert!(!single(r"\d*\d"));
        assert!(!single(r"(a*|b)"));
        assert!(!single(r"(a*)*"));
        assert!(!single(r"((a)|b)+"));
        let e = ok(r".*\.tar").check_unambiguous().unwrap_err();
        assert!(e.help.contains("[^.]*"), "{}", e.help);
    }

    #[test]
    fn empty_matches() {
        assert!(ok("a*").can_match_empty());
        assert!(!ok("a+").can_match_empty());
    }
}
