//! Diagnostics: positions, errors and warnings. FROZEN CONTRACT (v0.1).
//!
//! Every stage returns `Vec<Diag>` instead of aborting. The parser is resilient: it accumulates several errors.
//! Each diagnostic has a stable code (`E0xx` error, `W0xx` warning) for search, docs and `--json`.

/// Byte range in the source (`start` inclusive, `end` exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub severity: Severity,
    /// Stable code: `E001`, `W003`… List in `docs/language.md`, section "Diagnostics".
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    /// Suggested fix, preferably with the right code in backticks.
    pub help: Option<String>,
}

impl Diag {
    pub fn error(code: &'static str, span: Span, message: impl Into<String>) -> Diag {
        Diag { severity: Severity::Error, code, message: message.into(), span, help: None }
    }
    pub fn warning(code: &'static str, span: Span, message: impl Into<String>) -> Diag {
        Diag { severity: Severity::Warning, code, message: message.into(), span, help: None }
    }
    pub fn with_help(mut self, help: impl Into<String>) -> Diag {
        self.help = Some(help.into());
        self
    }
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Line and column (1-based) of the start of the span. The column counts characters, not bytes.
    pub fn position(&self, source: &str) -> (usize, usize) {
        line_col(source, self.span.start)
    }

    /// Rendering in rustc style:
    /// ```text
    /// error[E012]: `fs.read(…)` takes `path`, but `times` is `int`
    ///   --> backup.nalu:12:9
    ///    |
    /// 12 |     let x = fs.read(times)
    ///    |                     ^^^^^
    ///    = help: convert with `"{times}"` or pass a path
    /// ```
    pub fn render(&self, source: &str, file: &str) -> String {
        let (line, col) = line_col(source, self.span.start);
        let label = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let mut out = format!("{}[{}]: {}\n", label, self.code, self.message);
        out.push_str(&format!("  --> {}:{}:{}\n", file, line, col));
        if let Some(text) = source.lines().nth(line.saturating_sub(1)) {
            let width = line.to_string().len();
            let pad = " ".repeat(width);
            out.push_str(&format!("{} |\n", pad));
            out.push_str(&format!("{} | {}\n", line, text));
            let (_, end_col) = line_col(source, self.span.end.max(self.span.start));
            let len = if end_col > col { end_col - col } else { 1 };
            let len = len.min(text.chars().count().saturating_sub(col - 1).max(1));
            out.push_str(&format!("{} | {}{}\n", pad, " ".repeat(col - 1), "^".repeat(len)));
            if let Some(h) = &self.help {
                out.push_str(&format!("{} = help: {}\n", pad, h));
            }
        } else if let Some(h) = &self.help {
            out.push_str(&format!("  = help: {}\n", h));
        }
        out
    }

    /// A JSON object (no dependencies): `{"file","line","column","severity","code","message","help"}`.
    pub fn to_json(&self, source: &str, file: &str) -> String {
        let (line, col) = line_col(source, self.span.start);
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let help = match &self.help {
            Some(h) => json_str(h),
            None => "null".to_string(),
        };
        format!(
            "{{\"file\":{},\"line\":{},\"column\":{},\"severity\":\"{}\",\"code\":\"{}\",\"message\":{},\"help\":{}}}",
            json_str(file), line, col, sev, self.code, json_str(&self.message), help
        )
    }
}

/// Line and column (1-based) of a byte offset.
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = source[line_start..offset].chars().count() + 1;
    (line, col)
}

/// Escapes a text as a JSON string (with quotes).
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Renders a whole list (errors and warnings, already sorted by position).
pub fn render_all(diags: &[Diag], source: &str, file: &str) -> String {
    let mut sorted: Vec<&Diag> = diags.iter().collect();
    sorted.sort_by_key(|d| (d.span.start, d.severity != Severity::Error));
    sorted.iter().map(|d| d.render(source, file)).collect::<Vec<_>>().join("\n")
}

/// `[ {...}, {...} ]`
pub fn render_json(diags: &[Diag], source: &str, file: &str) -> String {
    let items: Vec<String> = diags.iter().map(|d| "  ".to_string() + &d.to_json(source, file)).collect();
    format!("[\n{}\n]", items.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column() {
        let src = "ab\ncdé f\n";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 3), (2, 1));
        // 'é' is 2 bytes: the 'f' is at byte 8, column 5
        assert_eq!(line_col(src, 8), (2, 5));
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn render_shows_snippet() {
        let src = "let x = 1\nlet y = foo\n";
        let d = Diag::error("E001", Span::new(18, 21), "name `foo` does not exist").with_help("did you mean `fo`?");
        let r = d.render(src, "t.nalu");
        assert!(r.contains("error[E001]"));
        assert!(r.contains("t.nalu:2:9"));
        assert!(r.contains("^^^"));
        assert!(r.contains("help:"));
    }
}
