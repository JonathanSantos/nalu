//! External commands: `$ cmd`, `$(cmd)`, `if $ cmd`, with `--verbose`/`--dry-run` and error messages.

use super::gen::Gen;
use super::quote::{sq, W};
use super::val::{Cond, Dest, Loc, Val};
use crate::ir::*;

/// The `{…}` pieces of the original text (outside single quotes), in order: (start, end) in bytes.
pub(crate) fn brace_groups(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut in_sq = false;
    while i < b.len() {
        let c = b[i];
        if in_sq {
            if c == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' => in_sq = true,
            b'\\' => i += 1,
            b'{' => {
                let start = i;
                let mut depth = 0;
                while i < b.len() {
                    match b[i] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        b'"' => {
                            // string inside the expression
                            i += 1;
                            while i < b.len() && b[i] != b'"' {
                                if b[i] == b'\\' {
                                    i += 1;
                                }
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if i >= b.len() {
                    return out;
                }
                out.push((start, i + 1));
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn is_secret(e: &Expr) -> bool {
    e.ty == Ty::Secret
}

impl<'a> Gen<'a> {
    /// The value of an expression for display (secrets as `******`, lists separated by spaces).
    fn show_expr(&mut self, e: &Expr, spread: bool) -> W {
        if is_secret(e) {
            return W::lit("******");
        }
        if spread {
            return match self.loc(e) {
                Loc::Base(b) => W::exp(format!("${{{}[*]}}", b)),
                Loc::Elem { .. } => W::lit("…"),
            };
        }
        let mut w = W::default();
        if let ExprKind::Interp(parts) = &e.kind {
            for p in parts {
                match p {
                    Part::Lit(s) => w.push_lit(s),
                    Part::Expr(x) => {
                        let xw = self.show_expr(x, false);
                        w.push(xw);
                    }
                }
            }
            return w;
        }
        match self.expr_val(e) {
            Val::Word(w) => w,
            Val::Cond(c) => self.cond_word(&c),
            _ => W::lit(""),
        }
    }

    /// The expressions that appear as `{…}` in the command text, in order.
    fn shown_exprs(cmd: &Command) -> Vec<(Expr, bool)> {
        let mut v = Vec::new();
        for sc in &cmd.pipeline {
            // `NAME=value` comes before the program in the text
            for w in sc.env.iter().map(|(_, w)| w).chain(&sc.words) {
                match w {
                    Word::Lit(_) => {}
                    Word::Interp(parts) => {
                        for p in parts {
                            if let Part::Expr(e) = p {
                                v.push((e.clone(), false));
                            }
                        }
                    }
                    Word::Spread(e) => v.push((e.clone(), true)),
                }
            }
        }
        for r in &cmd.redirects {
            match r {
                Redirect::Out { path, .. } | Redirect::Err { path, .. } | Redirect::In(path) => match &path.kind {
                    ExprKind::Str(_) => {}
                    ExprKind::Interp(parts) => {
                        for p in parts {
                            if let Part::Expr(e) = p {
                                v.push((e.clone(), false));
                            }
                        }
                    }
                    _ => v.push((path.clone(), false)),
                },
                Redirect::ErrToOut => {}
            }
        }
        v
    }

    /// The command text for `--verbose`/`--dry-run`/messages, with the values in place of `{…}`.
    pub fn cmd_display(&mut self, cmd: &Command) -> W {
        let exprs = Self::shown_exprs(cmd);
        let groups = brace_groups(&cmd.text);
        if !cmd.text.is_empty() && groups.len() == exprs.len() {
            let mut w = W::default();
            let mut last = 0;
            for ((s, e), (x, spread)) in groups.iter().zip(exprs.iter()) {
                w.push_lit(&cmd.text[last..*s]);
                let xw = self.show_expr(x, *spread);
                w.push(xw);
                last = *e;
            }
            w.push_lit(&cmd.text[last..]);
            return w;
        }
        // no reliable text: rebuild it from the words
        let mut w = W::default();
        for (si, sc) in cmd.pipeline.iter().enumerate() {
            if si > 0 {
                w.push_lit(" | ");
            }
            for (name, _) in &sc.env {
                w.push_lit(&format!("{}=… ", name));
            }
            for (wi, word) in sc.words.iter().enumerate() {
                if wi > 0 {
                    w.push_lit(" ");
                }
                match word {
                    Word::Lit(s) => w.push_lit(s),
                    Word::Interp(parts) => {
                        for p in parts {
                            match p {
                                Part::Lit(s) => w.push_lit(s),
                                Part::Expr(e) => {
                                    let x = self.show_expr(e, false);
                                    w.push(x);
                                }
                            }
                        }
                    }
                    Word::Spread(e) => {
                        let x = self.show_expr(e, true);
                        w.push(x);
                    }
                }
            }
        }
        w
    }

    /// A command word as shell argument(s).
    fn cmd_word(&mut self, w: &Word) -> String {
        match w {
            // an unquoted `~` already became `{dirs.home()}` in the lexer: a literal is just text
            Word::Lit(s) => sq(s),
            Word::Interp(parts) => {
                let mut out = W::default();
                for p in parts {
                    match p {
                        Part::Lit(s) => out.push_lit(s),
                        Part::Expr(e) => {
                            let x = match self.expr_val(e) {
                                Val::Word(w) => w,
                                Val::Cond(c) => self.cond_word(&c),
                                _ => W::lit(""),
                            };
                            out.push(x);
                        }
                    }
                }
                out.render()
            }
            Word::Spread(e) => match self.loc(e) {
                Loc::Base(b) => format!("\"${{{}[@]}}\"", b),
                Loc::Elem { .. } => "''".into(),
            },
        }
    }

    /// A stage as shell text: `NAME=value … program arguments`. The assignments sit right before the
    /// program, so they reach only that process (and never its argument list).
    fn stage_text(&mut self, sc: &SimpleCmd) -> String {
        let mut ws: Vec<String> = Vec::new();
        for (name, value) in &sc.env {
            let v = self.cmd_word(value);
            ws.push(format!("{}={}", name, v));
        }
        for w in &sc.words {
            let x = self.cmd_word(w);
            ws.push(x);
        }
        ws.join(" ")
    }

    fn prog_name(sc: &SimpleCmd) -> String {
        match sc.words.first() {
            Some(Word::Lit(s)) => s.clone(),
            _ => String::new(),
        }
    }

    fn redir_path(&mut self, e: &Expr) -> String {
        if let ExprKind::Str(s) = &e.kind {
            return sq(s);
        }
        let w = self.word(e);
        w.render()
    }

    /// The pipeline as shell text (with redirections and stdin).
    fn pipeline_text(&mut self, cmd: &Command) -> String {
        let n = cmd.pipeline.len();
        let mut stages: Vec<String> = Vec::new();
        for sc in &cmd.pipeline {
            let s = self.stage_text(sc);
            stages.push(s);
        }
        if let Some(e) = &cmd.stdin {
            let w = self.word(e).render();
            stages[0].push_str(&format!(" < <(printf '%s' {})", w));
        }
        for r in &cmd.redirects {
            match r {
                Redirect::In(p) => {
                    let w = self.redir_path(p);
                    stages[0].push_str(&format!(" < {}", w));
                }
                Redirect::Out { path, append } => {
                    let w = self.redir_path(path);
                    stages[n - 1].push_str(&format!(" {} {}", if *append { ">>" } else { ">" }, w));
                }
                Redirect::Err { path, append } => {
                    let w = self.redir_path(path);
                    stages[n - 1].push_str(&format!(" {} {}", if *append { "2>>" } else { "2>" }, w));
                }
                Redirect::ErrToOut => stages[n - 1].push_str(" 2>&1"),
            }
        }
        stages.join(" | ")
    }

    fn is_simple(cmd: &Command) -> bool {
        // with `NAME=value` the program is written in place (not behind `__cmd`), so the assignment applies to it
        cmd.pipeline.len() == 1 && cmd.redirects.is_empty() && cmd.stdin.is_none() && cmd.pipeline[0].env.is_empty()
    }

    pub fn command_stmt(&mut self, cmd: &Command, on_fail: &OnFail) {
        let disp = self.cmd_display(cmd).render();
        if Self::is_simple(cmd) {
            self.need("__cmd");
            let ws: Vec<String> = cmd.pipeline[0].words.iter().map(|w| self.cmd_word(w)).collect();
            let op = format!("__cmd {} {}", disp, ws.join(" "));
            self.with_op(&op, on_fail, &Dest::Discard, &Ty::Unit, &mut |_: &mut Gen<'a>| {});
            return;
        }
        self.need("__show");
        self.need("__pipe_chk");
        let pl = self.pipeline_text(cmd);
        // the program of each stage, for "was not found" (a name that comes from a value included)
        let mut progs: Vec<String> = Vec::new();
        for s in &cmd.pipeline {
            let p = match s.words.first() {
                Some(w @ Word::Interp(_)) => self.cmd_word(w),
                _ => sq(&Self::prog_name(s)),
            };
            progs.push(p);
        }
        self.line(format!("if __show {}; then", disp));
        self.ind += 1;
        self.line(pl);
        let op = format!("__pipe_chk {} {} {} \"${{PIPESTATUS[@]}}\"", disp, progs.len(), progs.join(" "));
        self.with_op(&op, on_fail, &Dest::Discard, &Ty::Unit, &mut |_: &mut Gen<'a>| {});
        self.ind -= 1;
        self.line("fi");
    }

    pub fn capture_into(&mut self, dest: &Dest, ty: &Ty, cmd: &Command, on_fail: &OnFail) {
        self.need("__verb");
        self.need("__cmd_fail");
        let disp = self.cmd_display(cmd).render();
        let pl = self.pipeline_text(cmd);
        let prog = sq(&Self::prog_name(&cmd.pipeline[0]));
        let target = match dest {
            Dest::Var(b, t) if t.is_scalar() => b.clone(),
            _ => self.tmp(&Ty::Str),
        };
        let cap = format!("{}=$(__verb {}; {})", target, disp, pl);
        let d = dest.clone();
        let tgt = target.clone();
        let mut ok = move |g: &mut Gen<'a>| {
            if let Dest::Var(b, _) = &d {
                if *b == tgt {
                    return;
                }
            }
            g.store(&d, &Val::Word(W::var(&tgt)));
        };
        if let OnFail::Propagate = on_fail {
            let fa = self.fail_action();
            self.line(format!("{} || {{ __cmd_fail {} \"$?\" {}; {}; }}", cap, disp, prog, fa));
            ok(self);
            return;
        }
        let op = format!("{{ {} || __cmd_fail {} \"$?\" {}; }}", cap, disp, prog);
        self.with_op(&op, on_fail, dest, ty, &mut ok);
    }

    pub fn command_ok(&mut self, cmd: &Command) -> Cond {
        let disp = self.cmd_display(cmd).render();
        if Self::is_simple(cmd) {
            self.need("__ok");
            let ws: Vec<String> = cmd.pipeline[0].words.iter().map(|w| self.cmd_word(w)).collect();
            return Cond::Cmd(format!("__ok {} {}", disp, ws.join(" ")));
        }
        self.need("__verb");
        let pl = self.pipeline_text(cmd);
        Cond::Cmd(format!("{{ __verb {}; {}; }}", disp, pl))
    }
}

/// Literal command word; a leading `~` and `~/…` become `$HOME`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grupos() {
        assert_eq!(brace_groups("git add {a}").len(), 1);
        assert_eq!(brace_groups("echo '{}' {x} \"r {v}\"").len(), 2);
        assert_eq!(brace_groups("x {f(\"}\")} y").len(), 1);
    }
}
