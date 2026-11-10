//! All quoting of the Bash backend goes through here: `sq` (literal), `dq_esc` (inside double quotes) and
//! [`W`] (a shell word built from literal pieces and expansions).

/// Characters that cannot appear raw in the script (they break parsing or are invisible).
fn is_ctrl(c: char) -> bool {
    (c as u32) < 0x20 || c == '\u{7f}'
}

fn ansi_c(s: &str) -> String {
    let mut o = String::from("$'");
    for c in s.chars() {
        match c {
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            '\\' => o.push_str("\\\\"),
            '\'' => o.push_str("\\'"),
            c if is_ctrl(c) => o.push_str(&format!("\\x{:02x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('\'');
    o
}

/// Literal text as **one** shell word, with no expansion: `'text'` (with `'\''`), or
/// `$'…'` when there are control characters (`\n`, `\t`…).
pub fn sq(s: &str) -> String {
    if s.chars().any(is_ctrl) {
        return ansi_c(s);
    }
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-./,:+@%^".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escapes text for use inside `"…"` (no control characters: the caller splits them out).
pub fn dq_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Lit(String),
    /// A valid expansion inside double quotes: `${x}`, `${#x}`, `$(( a + 1 ))`, `${xs[i]}`.
    Exp(String),
}

/// A shell word: a concatenation of literals and expansions; it becomes a single argument.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct W {
    pub segs: Vec<Seg>,
}

impl W {
    pub fn lit(s: &str) -> W {
        W { segs: vec![Seg::Lit(s.to_string())] }
    }
    pub fn exp(s: impl Into<String>) -> W {
        W { segs: vec![Seg::Exp(s.into())] }
    }
    /// `${name}` (variable name or `xs[i]`).
    pub fn var(name: &str) -> W {
        W::exp(format!("${{{}}}", name))
    }
    pub fn push(&mut self, w: W) {
        for s in w.segs {
            match (self.segs.last_mut(), s) {
                (Some(Seg::Lit(a)), Seg::Lit(b)) => a.push_str(&b),
                (_, s) => self.segs.push(s),
            }
        }
    }
    pub fn push_lit(&mut self, s: &str) {
        self.push(W::lit(s));
    }
    pub fn concat(mut self, w: W) -> W {
        self.push(w);
        self
    }
    /// If the word is only fixed text, that text.
    pub fn as_lit(&self) -> Option<String> {
        let mut o = String::new();
        for s in &self.segs {
            match s {
                Seg::Lit(l) => o.push_str(l),
                Seg::Exp(_) => return None,
            }
        }
        Some(o)
    }
    /// If the word is exactly one simple variable (`${x}`), its name.
    pub fn as_simple_var(&self) -> Option<String> {
        if self.segs.len() != 1 {
            return None;
        }
        match &self.segs[0] {
            Seg::Exp(e) => {
                let inner = e.strip_prefix("${")?.strip_suffix('}')?;
                if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !inner.starts_with(|c: char| c.is_ascii_digit()) {
                    Some(inner.to_string())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Always inside double quotes (for patterns inside `${x#…}`, where single quotes do not work).
    pub fn render_dq(&self) -> String {
        let mut o = String::from("\"");
        for s in &self.segs {
            match s {
                Seg::Lit(l) => o.push_str(&dq_esc(l)),
                Seg::Exp(e) => o.push_str(e),
            }
        }
        o.push('"');
        o
    }

    /// Right-hand side of an assignment `x=…`: a lone expansion needs no quotes (there is no word
    /// splitting in an assignment), and Bash is faster without them in hot loops.
    pub fn render_assign(&self) -> String {
        if self.segs.len() == 1 {
            if let Seg::Exp(e) = &self.segs[0] {
                if let Some(v) = self.as_simple_var() {
                    return format!("${}", v);
                }
                return e.clone();
            }
        }
        self.render()
    }

    /// The word ready for the shell (one argument).
    pub fn render(&self) -> String {
        if let Some(l) = self.as_lit() {
            return sq(&l);
        }
        if let Some(v) = self.as_simple_var() {
            return format!("\"${}\"", v);
        }
        let mut o = String::new();
        let mut open = false;
        for s in &self.segs {
            match s {
                Seg::Exp(e) => {
                    if !open {
                        o.push('"');
                        open = true;
                    }
                    o.push_str(e);
                }
                Seg::Lit(l) => {
                    // split into normal pieces (inside "…") and control pieces ($'…')
                    let mut buf = String::new();
                    let mut ctrl = String::new();
                    for c in l.chars() {
                        if is_ctrl(c) {
                            if !buf.is_empty() {
                                if !open {
                                    o.push('"');
                                    open = true;
                                }
                                o.push_str(&dq_esc(&buf));
                                buf.clear();
                            }
                            ctrl.push(c);
                        } else {
                            if !ctrl.is_empty() {
                                if open {
                                    o.push('"');
                                    open = false;
                                }
                                o.push_str(&ansi_c(&ctrl));
                                ctrl.clear();
                            }
                            buf.push(c);
                        }
                    }
                    if !ctrl.is_empty() {
                        if open {
                            o.push('"');
                            open = false;
                        }
                        o.push_str(&ansi_c(&ctrl));
                    }
                    if !buf.is_empty() {
                        if !open {
                            o.push('"');
                            open = true;
                        }
                        o.push_str(&dq_esc(&buf));
                    }
                }
            }
        }
        if open {
            o.push('"');
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literais() {
        assert_eq!(sq("abc"), "abc");
        assert_eq!(sq("a b"), "'a b'");
        assert_eq!(sq("it's"), "'it'\\''s'");
        assert_eq!(sq(""), "''");
        assert_eq!(sq("a\nb"), "$'a\\nb'");
        assert_eq!(sq("$HOME"), "'$HOME'");
    }

    #[test]
    fn palavras() {
        let mut w = W::lit("oi, ");
        w.push(W::var("nome"));
        w.push_lit("!\n");
        assert_eq!(w.render(), "\"oi, ${nome}!\"$'\\n'");
        assert_eq!(W::var("x").render(), "\"$x\"");
        assert_eq!(W::lit("a\"$b").concat(W::var("c")).render(), "\"a\\\"\\$b${c}\"");
    }
}
