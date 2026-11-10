//! **Minimal** PowerShell 5.1 syntax checker, for when the machine has no `pwsh`.
//!
//! Not a parser: it checks what most often breaks generated scripts.
//! - closed strings `'…'`, `"…"` and here-strings `@'…'@` / `@"…"@`, treating **typographic quotes**
//!   (`‘ ’ ‚ ‛` and `“ ” „`) the way PowerShell does: as real quotes;
//! - `$( … )` inside double-quoted strings, analyzed as code;
//! - balanced `{ } ( ) [ ]` (outside strings and `#`/`<# #>` comments);
//! - no PowerShell 7-only construct outside strings: `??`, `??=`, `?.`, `?[`, `&&`, `||`,
//!   ternary ` ? … : `, `-Parallel`, `Join-String`.
//!
//! The real test is `tests/ps_syntax.ps1` (the official parser), which runs when PowerShell is available.

fn is_sq(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

fn is_dq(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}')
}

struct Scanner<'a> {
    c: Vec<char>,
    i: usize,
    line: usize,
    problems: Vec<String>,
    _src: &'a str,
}

impl<'a> Scanner<'a> {
    fn peek(&self, k: usize) -> Option<char> {
        self.c.get(self.i + k).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.c.get(self.i).copied();
        if ch == Some('\n') {
            self.line += 1;
        }
        self.i += 1;
        ch
    }

    fn err(&mut self, line: usize, msg: impl Into<String>) {
        self.problems.push(format!("linha {}: {}", line, msg.into()));
    }

    fn prev_is_boundary(&self) -> bool {
        if self.i == 0 {
            return true;
        }
        matches!(self.c[self.i - 1], ' ' | '\t' | '\n' | '\r' | ';' | '{' | '(' | '|' | '}' | ')')
    }

    /// Code up to `close` (exclusive; consumed) or to the end.
    fn code(&mut self, close: Option<char>) {
        let mut stack: Vec<(char, usize)> = Vec::new();
        while let Some(ch) = self.peek(0) {
            let line = self.line;
            match ch {
                '`' => {
                    self.bump();
                    self.bump();
                }
                '#' if self.prev_is_boundary() => {
                    while let Some(c) = self.peek(0) {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                '<' if self.peek(1) == Some('#') => {
                    self.bump();
                    self.bump();
                    let mut closed = false;
                    while self.peek(0).is_some() {
                        if self.peek(0) == Some('#') && self.peek(1) == Some('>') {
                            self.bump();
                            self.bump();
                            closed = true;
                            break;
                        }
                        self.bump();
                    }
                    if !closed {
                        self.err(line, "`<#` comment without `#>`");
                    }
                }
                '@' if matches!(self.peek(1), Some(q) if is_sq(q) || is_dq(q)) && self.rest_of_line_blank(2) => {
                    let dq = is_dq(self.peek(1).unwrap());
                    self.here_string(dq);
                }
                c if is_sq(c) => {
                    self.bump();
                    self.single();
                }
                c if is_dq(c) => {
                    self.bump();
                    self.double();
                }
                '$' if self.peek(1) == Some('{') => {
                    self.bump();
                    self.bump();
                    while let Some(c) = self.bump() {
                        if c == '}' {
                            break;
                        }
                    }
                }
                '{' | '(' | '[' => {
                    stack.push((ch, line));
                    self.bump();
                }
                '}' | ')' | ']' => {
                    let open = match ch {
                        '}' => '{',
                        ')' => '(',
                        _ => '[',
                    };
                    match stack.pop() {
                        Some((o, _)) if o == open => {
                            self.bump();
                        }
                        Some((o, l)) => {
                            self.err(line, format!("`{}` closes the `{}` opened on line {}", ch, o, l));
                            self.bump();
                        }
                        None => {
                            if close == Some(ch) {
                                self.bump();
                                return;
                            }
                            self.err(line, format!("`{}` without an opening", ch));
                            self.bump();
                        }
                    }
                }
                '?' => {
                    let prev = if self.i > 0 { self.c[self.i - 1] } else { ' ' };
                    let next = self.peek(1).unwrap_or(' ');
                    if next == '?' {
                        self.err(line, "`??` only exists in PowerShell 7");
                    } else if (next == '.' || next == '[') && !prev.is_whitespace() {
                        self.err(line, format!("`?{}` only exists in PowerShell 7", next));
                    } else if prev == ' ' && next == ' ' {
                        self.err(line, "the ternary `? :` only exists in PowerShell 7");
                    }
                    self.bump();
                }
                '&' if self.peek(1) == Some('&') => {
                    self.err(line, "`&&` only exists in PowerShell 7");
                    self.bump();
                    self.bump();
                }
                '|' if self.peek(1) == Some('|') => {
                    self.err(line, "`||` only exists in PowerShell 7");
                    self.bump();
                    self.bump();
                }
                '-' | 'J' | 'j' => {
                    let word = self.word_here();
                    let lw = word.to_ascii_lowercase();
                    if lw == "-parallel" {
                        self.err(line, "`-Parallel` only exists in PowerShell 7");
                    } else if lw == "join-string" {
                        self.err(line, "`Join-String` only exists in PowerShell 7");
                    }
                    for _ in 0..word.chars().count().max(1) {
                        self.bump();
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
        for (o, l) in stack {
            self.err(l, format!("`{}` opened and never closed", o));
        }
        if let Some(c) = close {
            self.err(self.line, format!("faltou `{}` para fechar `$(`", c));
        }
    }

    fn word_here(&self) -> String {
        let mut s = String::new();
        let mut k = self.i;
        while let Some(&c) = self.c.get(k) {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                s.push(c);
                k += 1;
            } else {
                break;
            }
        }
        s
    }

    fn rest_of_line_blank(&self, from: usize) -> bool {
        let mut k = self.i + from;
        while let Some(&c) = self.c.get(k) {
            if c == '\n' {
                return true;
            }
            if !(c == ' ' || c == '\t' || c == '\r') {
                return false;
            }
            k += 1;
        }
        false
    }

    fn single(&mut self) {
        let line = self.line;
        loop {
            match self.bump() {
                None => {
                    self.err(line, "unclosed '…' string (remember: ‘ ’ ‚ ‛ also close strings in PowerShell)");
                    return;
                }
                Some(c) if is_sq(c) => {
                    if matches!(self.peek(0), Some(n) if is_sq(n)) {
                        self.bump();
                    } else {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn double(&mut self) {
        let line = self.line;
        loop {
            match self.peek(0) {
                None => {
                    self.err(line, "unclosed \"…\" string (remember: “ ” „ also close strings in PowerShell)");
                    return;
                }
                Some('`') => {
                    self.bump();
                    self.bump();
                }
                Some('$') if self.peek(1) == Some('(') => {
                    self.bump();
                    self.bump();
                    self.code(Some(')'));
                }
                Some('$') if self.peek(1) == Some('{') => {
                    self.bump();
                    self.bump();
                    while let Some(c) = self.bump() {
                        if c == '}' {
                            break;
                        }
                    }
                }
                Some(c) if is_dq(c) => {
                    self.bump();
                    if matches!(self.peek(0), Some(n) if is_dq(n)) {
                        self.bump();
                    } else {
                        return;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn here_string(&mut self, dq: bool) {
        let line = self.line;
        self.bump();
        self.bump();
        // to the end of the line
        while let Some(c) = self.bump() {
            if c == '\n' {
                break;
            }
        }
        loop {
            // start of line
            let a = self.peek(0);
            let b = self.peek(1);
            match (a, b) {
                (None, _) => {
                    self.err(line, "here-string without its closing line");
                    return;
                }
                (Some(q), Some('@')) if (dq && is_dq(q)) || (!dq && is_sq(q)) => {
                    self.bump();
                    self.bump();
                    return;
                }
                _ => {}
            }
            while let Some(c) = self.peek(0) {
                if dq && c == '$' && self.peek(1) == Some('(') {
                    self.bump();
                    self.bump();
                    self.code(Some(')'));
                    continue;
                }
                self.bump();
                if c == '\n' {
                    break;
                }
            }
        }
    }
}

/// Returns the problems found (empty = looks valid for 5.1).
pub fn check_syntax(src: &str) -> Vec<String> {
    let mut s = Scanner { c: src.chars().collect(), i: 0, line: 1, problems: Vec::new(), _src: src };
    s.code(None);
    s.problems
}

#[cfg(test)]
mod tests {
    use super::check_syntax;

    #[test]
    fn accepts_common_code() {
        let ok = "$x = 'it''s' + \"a`\"b $($y + 1) ${z}\"\nif ($a -lt 2) { [Console]::Out.WriteLine(@(1,2)[0]) } # coment ) \n<# bloco { #>\n$h = @'\nqualquer ' \" { coisa\n'@\n";
        assert_eq!(check_syntax(ok), Vec::<String>::new());
    }

    #[test]
    fn refuses_unbalanced_and_quotes() {
        assert!(!check_syntax("if ($a) { 1 ").is_empty());
        assert!(!check_syntax("$a = (1]").is_empty());
        assert!(!check_syntax("$a = 'it\u{2019}s'").is_empty(), "typographic quote closes the string");
        assert!(check_syntax("$a = 'it\u{2019}\u{2019}s'").is_empty());
        assert!(!check_syntax("$a = \"x \u{201D} y\"").is_empty());
        assert!(!check_syntax("$a = \"$(1 + \"").is_empty());
    }

    #[test]
    fn refuses_ps7() {
        for bad in ["$a = $b ?? 1", "$a ??= 1", "$a = $b?.c", "git a && git b", "a || b", "$x = $c ? 1 : 2", "1..3 | ForEach-Object -Parallel { $_ }", "$a | Join-String"] {
            assert!(!check_syntax(bad).is_empty(), "should refuse: {}", bad);
        }
        assert!(check_syntax("$a = 'a ?? b && c'").is_empty());
        assert!(check_syntax("# a && b\n$x = 1").is_empty());
    }
}
