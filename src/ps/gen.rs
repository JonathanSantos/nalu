//! PowerShell generator: IR → `.ps1` text (with `\n`; the CLI writes it with a BOM and CRLF).
//!
//! Execution model (summary; details in docs/architecture.md):
//! - Nalu variables become `$o_name` (lowercase, encoded, because PowerShell ignores case);
//!   at the top level they live in the script scope; inside functions they are locals; top-level variables
//!   read/written inside functions use `$script:o_name`;
//! - user functions return the value in `$script:__ret` and are called as statements, **never** inside
//!   `$( )` or assigned: the output of external programs inside them goes to the console, and 1-item
//!   lists never unroll;
//! - failures are exceptions (`__Throw` with `Data['nalu_code']`); `Propagate` emits nothing (the exception rises to the
//!   right `catch`: `try`, `retry`, `parallel`, the calling function or the top level); the other forms of `OnFail`
//!   become `try { } catch { }` around the operation;
//! - `match` becomes `if/elseif` (a `break` inside `switch` would stop the `switch`, not the loop);
//! - lists are `List[object]`; maps and structs are `OrderedDictionary` with ordinal keys.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::ir::*;
use crate::stdlib::PATH_PROPS;
use crate::target::Mode;

const LIST_NEW: &str = "[System.Collections.Generic.List[object]]::new()";

// ---------------------------------------------------------------------------------------------
// Literals and names
// ---------------------------------------------------------------------------------------------

/// Text literal. Printable ASCII only: single quotes, doubling `'`. With a control character (`\n`…) or any
/// non-ASCII character: double quotes with backtick escapes (`` ` ``, `$`, `"`) and `$([char]0x…)` for the rest.
/// This keeps the whole `.ps1` ASCII and needs no BOM: it works with `-File` on 5.1 and with `irm … | iex`.
pub fn lit(s: &str) -> String {
    if s.chars().any(|c| (c as u32) < 0x20 || (c as u32) >= 0x7f) {
        let mut o = String::from("\"");
        for c in s.chars() {
            match c {
                '`' => o.push_str("``"),
                '$' => o.push_str("`$"),
                '"' => o.push_str("`\""),
                '\n' => o.push_str("`n"),
                '\r' => o.push_str("`r"),
                '\t' => o.push_str("`t"),
                '\0' => o.push_str("`0"),
                c if (c as u32) < 0x20 || c == '\u{7f}' => {
                    let _ = write!(o, "$([char]0x{:02x})", c as u32);
                }
                c if (c as u32) > 0xffff => {
                    let _ = write!(o, "$([char]::ConvertFromUtf32(0x{:x}))", c as u32);
                }
                c if (c as u32) > 0x7f => {
                    let _ = write!(o, "$([char]0x{:04x})", c as u32);
                }
                c => o.push(c),
            }
        }
        o.push('"');
        o
    } else {
        let mut o = String::from("'");
        for c in s.chars() {
            if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                o.push(c);
            }
            o.push(c);
        }
        o.push('\'');
        o
    }
}

/// Nalu variable name → PowerShell-safe name (PowerShell is case-insensitive).
pub fn mangle(name: &str) -> String {
    let mut o = String::from("o_");
    for c in name.chars() {
        match c {
            'a'..='z' | '0'..='9' | '_' => o.push(c),
            'A'..='Z' => {
                o.push('_');
                o.push(c.to_ascii_lowercase());
                o.push('_')
            }
            '.' => o.push_str("__"),
            c => {
                let _ = write!(o, "_u{:x}_", c as u32);
            }
        }
    }
    o
}

fn fn_name(name: &str) -> String {
    format!("fn_{}", &mangle(name)[2..])
}

/// Integer always in parentheses: in argument mode (`fn 5L`) PowerShell would read `5L` as text.
fn int_lit(n: i64) -> String {
    if n == i64::MIN {
        "([long]::MinValue)".into()
    } else if n < 0 {
        format!("(-{}L)", -n)
    } else {
        format!("({}L)", n)
    }
}

fn is_text(t: &Ty) -> bool {
    matches!(t, Ty::Str | Ty::Path | Ty::Enum(_) | Ty::Secret)
}

fn is_coll(t: &Ty) -> bool {
    matches!(t, Ty::List(_) | Ty::Map(_, _) | Ty::Struct(_) | Ty::Error)
}

fn zero_of(t: &Ty) -> String {
    match t {
        Ty::Int | Ty::Duration => "(0L)".into(),
        Ty::Bool => "$false".into(),
        Ty::List(_) => format!("({})", LIST_NEW),
        Ty::Map(_, _) | Ty::Struct(_) | Ty::Error => "(__Map @())".into(),
        _ => "''".into(),
    }
}

// ---------------------------------------------------------------------------------------------
// Context and name collection
// ---------------------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Ctx {
    in_func: bool,
        /// Local names of the current function (parameters and declarations).
    locals: HashSet<String>,
        /// Locals of the function enclosing a lambda (read through PowerShell's dynamic scoping).
    outer: HashSet<String>,
}

fn collect_locals(b: &Block, out: &mut HashSet<String>) {
    for s in &b.stmts {
        collect_locals_stmt(s, out);
    }
}

fn collect_onfail(o: &OnFail, out: &mut HashSet<String>) {
    match o {
        OnFail::Block { err, body, .. } => {
            if let Some(e) = err {
                out.insert(e.clone());
            }
            collect_locals(body, out);
        }
        OnFail::Default(r) => {
            if let Rvalue::Call { on_fail, .. } | Rvalue::Capture { on_fail, .. } = r.as_ref() {
                collect_onfail(on_fail, out)
            }
        }
        _ => {}
    }
}

fn collect_locals_stmt(s: &Stmt, out: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::Let { var, value, .. } => {
            out.insert(var.clone());
            if let Rvalue::Call { on_fail, .. } | Rvalue::Capture { on_fail, .. } = value {
                collect_onfail(on_fail, out)
            }
        }
        StmtKind::Assign { value, .. } => {
            if let Rvalue::Call { on_fail, .. } | Rvalue::Capture { on_fail, .. } = value {
                collect_onfail(on_fail, out)
            }
        }
        StmtKind::Call { on_fail, .. } | StmtKind::Command { on_fail, .. } => collect_onfail(on_fail, out),
        StmtKind::If { then, els, .. } => {
            collect_locals(then, out);
            if let Some(e) = els {
                collect_locals(e, out)
            }
        }
        StmtKind::While { body, .. } => collect_locals(body, out),
        StmtKind::ForRange { var, body, .. } => {
            out.insert(var.clone());
            collect_locals(body, out)
        }
        StmtKind::ForList { var, index, body, .. } => {
            out.insert(var.clone());
            if let Some(i) = index {
                out.insert(i.clone());
            }
            collect_locals(body, out)
        }
        StmtKind::ForMap { key, value, body, .. } => {
            out.insert(key.clone());
            out.insert(value.clone());
            collect_locals(body, out)
        }
        StmtKind::Match { arms, default, .. } => {
            for a in arms {
                collect_locals(&a.body, out)
            }
            if let Some(d) = default {
                collect_locals(d, out)
            }
        }
        StmtKind::Try { body, on_fail } => {
            collect_locals(body, out);
            collect_onfail(on_fail, out)
        }
        StmtKind::Retry { body, on_fail, .. } => {
            collect_locals(body, out);
            collect_onfail(on_fail, out)
        }
        StmtKind::Parallel { body, on_fail, .. } => {
            match body {
                ParallelBody::For { var, body, .. } => {
                    out.insert(var.clone());
                    collect_locals(body, out)
                }
                ParallelBody::Tasks(bs) => {
                    for b in bs {
                        collect_locals(b, out)
                    }
                }
            }
            collect_onfail(on_fail, out)
        }
        StmtKind::AssertFails(inner) => collect_locals_stmt(inner, out),
        _ => {}
    }
}

/// Variables read by a statement (to capture a `defer`'s arguments right away).
fn vars_in_stmt(s: &Stmt, out: &mut Vec<(String, Ty)>) {
    fn e(x: &Expr, out: &mut Vec<(String, Ty)>) {
        match &x.kind {
            ExprKind::Var(n) => {
                if !out.iter().any(|(m, _)| m == n) {
                    out.push((n.clone(), x.ty.clone()))
                }
            }
            ExprKind::Field(b, _) | ExprKind::Len(b) | ExprKind::Unary(_, b) => e(b, out),
            ExprKind::Index(a, b) | ExprKind::Binary(_, a, b) => {
                e(a, out);
                e(b, out)
            }
            ExprKind::Interp(ps) => {
                for p in ps {
                    if let Part::Expr(x) = p {
                        e(x, out)
                    }
                }
            }
            _ => {}
        }
    }
    fn cmd(c: &Command, out: &mut Vec<(String, Ty)>) {
        for sc in &c.pipeline {
            for w in &sc.words {
                match w {
                    Word::Lit(_) => {}
                    Word::Interp(ps) => {
                        for p in ps {
                            if let Part::Expr(x) = p {
                                e(x, out)
                            }
                        }
                    }
                    Word::Spread(x) => e(x, out),
                }
            }
        }
        for r in &c.redirects {
            match r {
                Redirect::Out { path, .. } | Redirect::Err { path, .. } | Redirect::In(path) => e(path, out),
                Redirect::ErrToOut => {}
            }
        }
        if let Some(x) = &c.stdin {
            e(x, out)
        }
    }
    match &s.kind {
        StmtKind::Call { call, .. } => {
            for a in &call.args {
                if let Arg::Expr(x) = a {
                    e(x, out)
                }
            }
        }
        StmtKind::Command { cmd: c, .. } => cmd(c, out),
        _ => {}
    }
}

fn has_defer(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Defer(_) => true,
        StmtKind::If { then, els, .. } => has_defer(then) || els.as_ref().map(has_defer).unwrap_or(false),
        StmtKind::While { body, .. } | StmtKind::ForRange { body, .. } | StmtKind::ForList { body, .. } | StmtKind::ForMap { body, .. } => has_defer(body),
        StmtKind::Match { arms, default, .. } => arms.iter().any(|a| has_defer(&a.body)) || default.as_ref().map(has_defer).unwrap_or(false),
        StmtKind::Try { body, .. } | StmtKind::Retry { body, .. } => has_defer(body),
        _ => false,
    })
}

fn uses_cd(b: &Block) -> bool {
    fn call_is_cd(c: &Call) -> bool {
        matches!(&c.target, CallTarget::Std { module, name } if module == "sys" && name == "cd")
    }
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Call { call, .. } => call_is_cd(call),
        StmtKind::If { then, els, .. } => uses_cd(then) || els.as_ref().map(uses_cd).unwrap_or(false),
        StmtKind::While { body, .. } | StmtKind::ForRange { body, .. } | StmtKind::ForList { body, .. } | StmtKind::ForMap { body, .. } => uses_cd(body),
        StmtKind::Match { arms, default, .. } => arms.iter().any(|a| uses_cd(&a.body)) || default.as_ref().map(uses_cd).unwrap_or(false),
        StmtKind::Try { body, .. } | StmtKind::Retry { body, .. } => uses_cd(body),
        _ => false,
    })
}

/// Visits every call in a block, including in nested handlers, lambdas, `defer` and `parallel`.
fn each_call<'b>(b: &'b Block, f: &mut dyn FnMut(&'b Call)) {
    for s in &b.stmts {
        each_call_stmt(s, f);
    }
}

fn each_call_call<'b>(c: &'b Call, f: &mut dyn FnMut(&'b Call)) {
    f(c);
    for a in &c.args {
        if let Arg::Lambda(l) = a {
            each_call(&l.body, f);
        }
    }
}

fn each_call_rv<'b>(rv: &'b Rvalue, f: &mut dyn FnMut(&'b Call)) {
    match rv {
        Rvalue::Call { call, on_fail } => {
            each_call_call(call, f);
            each_call_of(on_fail, f);
        }
        Rvalue::Capture { on_fail, .. } => each_call_of(on_fail, f),
        _ => {}
    }
}

fn each_call_of<'b>(o: &'b OnFail, f: &mut dyn FnMut(&'b Call)) {
    match o {
        OnFail::Default(rv) => each_call_rv(rv, f),
        OnFail::Block { body, .. } => each_call(body, f),
        _ => {}
    }
}

fn each_call_stmt<'b>(s: &'b Stmt, f: &mut dyn FnMut(&'b Call)) {
    match &s.kind {
        StmtKind::Let { value, .. } | StmtKind::Assign { value, .. } => each_call_rv(value, f),
        StmtKind::Call { call, on_fail } => {
            each_call_call(call, f);
            each_call_of(on_fail, f);
        }
        StmtKind::Command { on_fail, .. } => each_call_of(on_fail, f),
        StmtKind::If { then, els, .. } => {
            each_call(then, f);
            if let Some(e) = els {
                each_call(e, f);
            }
        }
        StmtKind::While { body, .. } | StmtKind::ForRange { body, .. } | StmtKind::ForList { body, .. } | StmtKind::ForMap { body, .. } => each_call(body, f),
        StmtKind::Match { arms, default, .. } => {
            for a in arms {
                each_call(&a.body, f);
            }
            if let Some(d) = default {
                each_call(d, f);
            }
        }
        StmtKind::Try { body, on_fail } | StmtKind::Retry { body, on_fail, .. } => {
            each_call(body, f);
            each_call_of(on_fail, f);
        }
        StmtKind::Parallel { body, on_fail, .. } => {
            match body {
                ParallelBody::For { body, .. } => each_call(body, f),
                ParallelBody::Tasks(bs) => {
                    for b in bs {
                        each_call(b, f);
                    }
                }
            }
            each_call_of(on_fail, f);
        }
        StmtKind::Defer(inner) | StmtKind::AssertFails(inner) => each_call_stmt(inner, f),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------------------------

pub struct Gen<'a> {
    p: &'a Program,
    out: String,
    indent: usize,
    tmp: usize,
    ctx: Ctx,
        /// Generated helper functions (lambdas, defers, tests), emitted before the body.
    extra: Vec<String>,
    last_line: u32,
}

/// The result of translating a library call.
enum Impl {
        /// A PowerShell expression with the value.
    Val(String),
        /// Statements (no value).
    Stmt(String),
}

impl<'a> Gen<'a> {
    pub fn new(p: &'a Program) -> Gen<'a> {
        Gen { p, out: String::new(), indent: 0, tmp: 0, ctx: Ctx::default(), extra: vec![], last_line: 0 }
    }

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn fresh(&mut self, base: &str) -> String {
        self.tmp += 1;
        format!("$__{}{}", base, self.tmp)
    }

        /// Generates `f` into a separate buffer (with its own indentation) and returns the text.
    fn capture<F: FnOnce(&mut Self)>(&mut self, indent: usize, f: F) -> String {
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = std::mem::replace(&mut self.indent, indent);
        let saved_line = std::mem::replace(&mut self.last_line, 0);
        f(self);
        self.indent = saved_indent;
        self.last_line = saved_line;
        std::mem::replace(&mut self.out, saved_out)
    }

    fn var(&self, name: &str) -> String {
        if self.ctx.in_func && !self.ctx.locals.contains(name) && !self.ctx.outer.contains(name) {
            format!("$script:{}", mangle(name))
        } else {
            format!("${}", mangle(name))
        }
    }

    // -----------------------------------------------------------------------------------------
        // Expressions
    // -----------------------------------------------------------------------------------------

    pub fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Str(s) => lit(s),
            ExprKind::Int(n) | ExprKind::Duration(n) => int_lit(*n),
            ExprKind::Bool(b) => if *b { "$true".into() } else { "$false".into() },
            ExprKind::EnumCase { case, .. } => lit(case),
            ExprKind::Var(n) => self.var(n),
            ExprKind::DryRun => "$script:__dry".into(),
            ExprKind::Args => "$script:__args".into(),
            ExprKind::CurrentErr => "$__e".into(),
            ExprKind::Field(b, f) => format!("({}[{}])", self.expr(b), lit(f)),
            ExprKind::Index(l, i) => {
                let elem = match &l.ty {
                    Ty::List(t) => (**t).clone(),
                    _ => Ty::Str,
                };
                format!("(__Idx {} {} {})", self.expr(l), self.expr(i), zero_of(&elem))
            }
            ExprKind::Len(x) => match &x.ty {
                Ty::List(_) | Ty::Map(_, _) => format!("([long]{}.Count)", self.expr(x)),
                _ => format!("(__Len {})", self.expr(x)),
            },
            ExprKind::Unary(UnOp::Neg, x) => format!("(-{})", self.expr(x)),
            ExprKind::Unary(UnOp::Not, x) => format!("(-not {})", self.expr(x)),
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b),
            ExprKind::Interp(parts) => self.interp(parts),
        }
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr) -> String {
        let (x, y) = (self.expr(a), self.expr(b));
        let text = is_text(&a.ty) || is_text(&b.ty);
        match op {
            BinOp::Add => format!("({} + {})", x, y),
            BinOp::Sub => format!("({} - {})", x, y),
            BinOp::Mul => format!("({} * {})", x, y),
                        // Integer division truncating toward zero without going through double.
            BinOp::Div => format!("([long][decimal]::Truncate([decimal]{} / [decimal]{}))", x, y),
            BinOp::Mod => format!("({} % {})", x, y),
            BinOp::Eq if text => format!("([string]::Equals({}, {}))", x, y),
            BinOp::Ne if text => format!("(-not [string]::Equals({}, {}))", x, y),
            BinOp::Eq => format!("({} -eq {})", x, y),
            BinOp::Ne => format!("({} -ne {})", x, y),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let o = match op {
                    BinOp::Lt => "-lt",
                    BinOp::Le => "-le",
                    BinOp::Gt => "-gt",
                    _ => "-ge",
                };
                if text {
                    format!("((__Cmp {} {}) {} 0)", x, y, o)
                } else {
                    format!("({} {} {})", x, o, y)
                }
            }
            BinOp::And => format!("({} -and {})", x, y),
            BinOp::Or => format!("({} -or {})", x, y),
            BinOp::Concat => format!("([string]{} + {})", x, self.text(b)),
            BinOp::PathJoin => format!("(__PJ {} {})", x, y),
        }
    }

        /// An expression as text (for `print`, interpolation, `log`, `join`).
    fn text(&self, e: &Expr) -> String {
        let x = self.expr(e);
        match &e.ty {
            Ty::Bool => format!("(@('false','true')[[int]{}])", x),
            Ty::Int => format!("([string]{})", x),
            Ty::Duration => format!("(__DurStr {})", x),
            Ty::Error => format!("({}['message'])", x),
            _ => x,
        }
    }

    fn interp(&self, parts: &[Part]) -> String {
        if parts.is_empty() {
            return "''".into();
        }
        let mut items: Vec<String> = Vec::new();
        for p in parts {
            match p {
                Part::Lit(s) => items.push(lit(s)),
                Part::Expr(e) => items.push(self.text(e)),
            }
        }
        if items.len() == 1 && matches!(parts[0], Part::Lit(_)) {
            return items.remove(0);
        }
        format!("('' + {})", items.join(" + "))
    }

        /// Variadic items (`print`, `log`) joined by a space.
    fn joined_text(&self, args: &[Arg]) -> String {
        let items: Vec<String> = args
            .iter()
            .filter_map(|a| match a {
                Arg::Expr(e) => Some(self.text(e)),
                Arg::Lambda(_) => None,
            })
            .collect();
        match items.len() {
            0 => "''".into(),
            1 => format!("[string]{}", items[0]),
            _ => format!("('' + {})", items.join(" + ' ' + ")),
        }
    }

        /// Value to store: collections are copied.
    fn stored(&self, e: &Expr) -> String {
        if is_coll(&e.ty) {
            format!("(__Copy {})", self.expr(e))
        } else {
            self.expr(e)
        }
    }

    fn arg(&self, a: &Arg) -> String {
        match a {
            Arg::Expr(e) => format!("({})", self.expr(e)),
            Arg::Lambda(_) => "$null".into(),
        }
    }

    // -----------------------------------------------------------------------------------------
        // Blocks and statements
    // -----------------------------------------------------------------------------------------

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        if s.line != 0 && s.line != self.last_line {
            self.last_line = s.line;
            self.line(&format!("# line {}", s.line));
        }
        match &s.kind {
            StmtKind::Let { var, value, .. } => {
                let dst = self.var(var);
                self.assign_rvalue(&dst, value);
            }
            StmtKind::Assign { target, value } => {
                let dst = self.lvalue(target);
                self.assign_rvalue(&dst, value);
            }
            StmtKind::Call { call, on_fail } => self.call_into(None, call, on_fail),
            StmtKind::Command { cmd, on_fail } => {
                let spec = self.cmd_spec(cmd);
                self.guarded(on_fail, None, |g| g.line(&format!("__Cmd {}", spec)));
            }
            StmtKind::If { cond, then, els } => self.gen_if(cond, then, els.as_ref(), false),
            StmtKind::While { cond, body } => {
                let c = self.expr(cond);
                self.line(&format!("while ({}) {{", c));
                self.indent += 1;
                self.block(body);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::ForRange { var, from, to, inclusive, body } => {
                let end = self.fresh("end");
                let v = self.var(var);
                let (f, t) = (self.expr(from), self.expr(to));
                self.line(&format!("{} = [long]{}", end, t));
                let op = if *inclusive { "-le" } else { "-lt" };
                self.line(&format!("for ({v} = [long]{f}; {v} {op} {end}; {v}++) {{", v = v, f = f, op = op, end = end));
                self.indent += 1;
                self.block(body);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::ForList { var, index, list, body } => {
                let it = self.fresh("it");
                let idx = self.fresh("i");
                let l = self.expr(list);
                let elem_coll = matches!(&list.ty, Ty::List(t) if is_coll(t));
                if index.is_some() {
                    self.line(&format!("{} = 0L", idx));
                }
                self.line(&format!("foreach ({} in {}.ToArray()) {{", it, l));
                self.indent += 1;
                if let Some(i) = index {
                    let iv = self.var(i);
                    self.line(&format!("{} = {}", iv, idx));
                    self.line(&format!("{}++", idx));
                }
                let v = self.var(var);
                if elem_coll {
                    self.line(&format!("{} = __Copy {}", v, it));
                } else {
                    self.line(&format!("{} = {}", v, it));
                }
                self.block(body);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::ForMap { key, value, map, body } => {
                let k = self.fresh("k");
                let m = self.expr(map);
                let int_keys = matches!(&map.ty, Ty::Map(k, _) if matches!(**k, Ty::Int));
                self.line(&format!("foreach ({} in @({}.Keys)) {{", k, m));
                self.indent += 1;
                let kv = self.var(key);
                if int_keys {
                    self.line(&format!("{} = [long]{}", kv, k));
                } else {
                    self.line(&format!("{} = {}", kv, k));
                }
                let vv = self.var(value);
                self.line(&format!("{} = {}[{}]", vv, m, k));
                self.block(body);
                self.indent -= 1;
                self.line("}");
            }
            StmtKind::Match { subject, arms, default } => self.gen_match(subject, arms, default.as_ref()),
            StmtKind::Try { body, on_fail } => {
                self.guarded(on_fail, None, |g| g.block(body));
            }
            StmtKind::Retry { times, every, body, on_fail } => self.gen_retry(times, every, body, on_fail),
            StmtKind::Parallel { max, body, on_fail } => self.gen_parallel(max.as_ref(), body, on_fail),
            StmtKind::Fail { message, code } => {
                let m = self.text(message);
                let c = code.as_ref().map(|c| self.expr(c)).unwrap_or_else(|| "(1L)".into());
                self.line(&format!("__Throw {} {}", m, c));
            }
            StmtKind::Return(v) => {
                if let Some(e) = v {
                    let x = self.stored(e);
                    self.line(&format!("$script:__ret = {}", x));
                }
                self.line("return");
            }
            StmtKind::Break => self.line("break"),
            StmtKind::Continue => self.line("continue"),
            StmtKind::Exit(code) => {
                let c = code.as_ref().map(|c| self.expr(c)).unwrap_or_else(|| "(0L)".into());
                self.line("[Console]::Out.Flush()");
                self.line(&format!("exit (__ExitCode {})", c));
            }
            StmtKind::Defer(inner) => self.gen_defer(inner),
            StmtKind::Native { powershell, .. } => {
                for l in powershell.lines() {
                    self.line(l);
                }
            }
            StmtKind::Assert { cond, text } => {
                let c = self.expr(cond);
                self.line(&format!("if (-not {}) {{ __Throw {} 1 }}", c, lit(&format!("assert failed: {}", text))));
            }
            StmtKind::AssertFails(inner) => {
                let ok = self.fresh("af");
                self.line(&format!("{} = $false", ok));
                self.line("try {");
                self.indent += 1;
                self.stmt(inner);
                self.indent -= 1;
                self.line(&format!("}} catch {{ {} = $true }}", ok));
                self.line(&format!("if (-not {}) {{ __Throw 'assert_fails: the call did not fail' 1 }}", ok));
            }
        }
    }

    fn lvalue(&self, l: &LValue) -> String {
        match l {
            LValue::Var(n) => self.var(n),
            LValue::Field { base, field } => format!("{}[{}]", self.var(base), lit(field)),
            LValue::Index { list, index } => format!("{}[[int]{}]", self.var(list), self.expr(index)),
            LValue::MapEntry { map, key } => format!("{}[[string]{}]", self.var(map), self.expr(key)),
        }
    }

    fn gen_if(&mut self, cond: &Expr, then: &Block, els: Option<&Block>, is_elseif: bool) {
        let c = self.expr(cond);
        if is_elseif {
                        // the "} elseif" was already written by the caller
            self.out.push_str(&format!("elseif ({}) {{\n", c));
        } else {
            self.line(&format!("if ({}) {{", c));
        }
        self.indent += 1;
        self.block(then);
        self.indent -= 1;
        match els {
            None => self.line("}"),
            Some(b) => {
                if let [Stmt { kind: StmtKind::If { cond, then, els }, .. }] = b.stmts.as_slice() {
                    for _ in 0..self.indent {
                        self.out.push_str("    ");
                    }
                    self.out.push_str("} ");
                    self.gen_if(cond, then, els.as_ref(), true);
                } else {
                    self.line("} else {");
                    self.indent += 1;
                    self.block(b);
                    self.indent -= 1;
                    self.line("}");
                }
            }
        }
    }

    fn gen_match(&mut self, subject: &Expr, arms: &[Arm], default: Option<&Block>) {
        let s = self.fresh("m");
        let sx = self.expr(subject);
        self.line(&format!("{} = {}", s, sx));
        let text = is_text(&subject.ty);
        let mut first = true;
        for arm in arms {
            let conds: Vec<String> = arm
                .patterns
                .iter()
                .map(|p| match p {
                    Pattern::Lit(e) => {
                        if text {
                            format!("([string]::Equals({}, {}))", s, self.expr(e))
                        } else {
                            format!("({} -eq {})", s, self.expr(e))
                        }
                    }
                    Pattern::Range { from, to, inclusive } => {
                        let op = if *inclusive { "-le" } else { "-lt" };
                        format!("({} -ge {} -and {} {} {})", s, self.expr(from), s, op, self.expr(to))
                    }
                })
                .collect();
            let c = conds.join(" -or ");
            if first {
                self.line(&format!("if ({}) {{", c));
                first = false;
            } else {
                self.line(&format!("}} elseif ({}) {{", c));
            }
            self.indent += 1;
            self.block(&arm.body);
            self.indent -= 1;
        }
        match (first, default) {
            (true, Some(d)) => self.block(d),
            (true, None) => {}
            (false, Some(d)) => {
                self.line("} else {");
                self.indent += 1;
                self.block(d);
                self.indent -= 1;
                self.line("}");
            }
            (false, None) => self.line("}"),
        }
    }

    // -----------------------------------------------------------------------------------------
        // Failures
    // -----------------------------------------------------------------------------------------

        /// Runs `attempt`; if `on_fail` is not `Propagate`, inside `try { } catch { handler }`.
    fn guarded<F: FnOnce(&mut Self)>(&mut self, on_fail: &OnFail, dst: Option<&str>, attempt: F) {
        if let OnFail::Propagate = on_fail {
            attempt(self);
            return;
        }
        self.line("try {");
        self.indent += 1;
        attempt(self);
        self.indent -= 1;
        self.line("} catch {");
        self.indent += 1;
        self.handler(on_fail, dst, "(__ErrOf $_)");
        self.indent -= 1;
        self.line("}");
    }

        /// The handling of a failure whose error (`{message, code}`) is `err`.
    fn handler(&mut self, on_fail: &OnFail, dst: Option<&str>, err: &str) {
        match on_fail {
            OnFail::Propagate => {
                self.line(&format!("$__e = {}", err));
                self.line("__Throw $__e['message'] $__e['code']");
            }
            OnFail::Ignore => {
                self.line(&format!("$__e = {}", err));
                self.line("if ($script:__verbose) { __ErrLine ('ignorado: ' + $__e['message']) }");
            }
            OnFail::Default(rv) => match dst {
                Some(d) => self.assign_rvalue(d, rv),
                None => self.assign_rvalue("$null", rv),
            },
            OnFail::Fail { message, code } => {
                self.line(&format!("$__e = {}", err));
                let m = self.text(message);
                let c = code.as_ref().map(|c| self.expr(c)).unwrap_or_else(|| "(1L)".into());
                self.line(&format!("__Throw {} {}", m, c));
            }
            OnFail::Block { err: name, body, value } => {
                self.line(&format!("$__e = {}", err));
                if let Some(n) = name {
                    let v = self.var(n);
                    self.line(&format!("{} = $__e", v));
                }
                self.block(body);
                if let (Some(v), Some(d)) = (value, dst) {
                    let x = self.stored(v);
                    self.line(&format!("{} = {}", d, x));
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
        // Values
    // -----------------------------------------------------------------------------------------

    fn assign_rvalue(&mut self, dst: &str, rv: &Rvalue) {
        match rv {
            Rvalue::Expr(e) => {
                let x = self.stored(e);
                self.line(&format!("{} = {}", dst, x));
            }
            Rvalue::List { items, .. } => {
                if items.is_empty() {
                    self.line(&format!("{} = {}", dst, LIST_NEW));
                } else {
                    let xs: Vec<String> = items.iter().map(|i| self.stored(i)).collect();
                    self.line(&format!("{} = [System.Collections.Generic.List[object]]::new([object[]]@({}))", dst, xs.join(", ")));
                }
            }
            Rvalue::Map { entries, .. } => {
                let xs: Vec<String> = entries.iter().map(|(k, v)| format!("([string]{}), {}", self.expr(k), self.stored(v))).collect();
                self.line(&format!("{} = __Map @({})", dst, xs.join(", ")));
            }
            Rvalue::Struct { ty, spread, fields } => {
                match spread {
                    None => {
                                                // in definition order, when known
                        let order: Vec<String> = self.p.structs.iter().find(|s| &s.name == ty).map(|s| s.fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                        let mut fs: Vec<&(Name, Expr)> = fields.iter().collect();
                        fs.sort_by_key(|(n, _)| order.iter().position(|o| o == n).unwrap_or(usize::MAX));
                        let xs: Vec<String> = fs.iter().map(|(n, v)| format!("{}, {}", lit(n), self.stored(v))).collect();
                        self.line(&format!("{} = __Map @({})", dst, xs.join(", ")));
                    }
                    Some(base) => {
                        let t = self.fresh("s");
                        let b = self.expr(base);
                        self.line(&format!("{} = __Copy {}", t, b));
                        for (n, v) in fields {
                            let x = self.stored(v);
                            self.line(&format!("{}[{}] = {}", t, lit(n), x));
                        }
                        self.line(&format!("{} = {}", dst, t));
                    }
                }
            }
            Rvalue::Call { call, on_fail } => self.call_into(Some(dst), call, on_fail),
            Rvalue::Capture { cmd, on_fail } => {
                let spec = self.cmd_spec(cmd);
                self.guarded(on_fail, Some(dst), |g| g.line(&format!("{} = __Capture {}", dst, spec)));
            }
            Rvalue::CommandOk(cmd) => {
                let spec = self.cmd_spec(cmd);
                self.line(&format!("{} = __CmdOk {}", dst, spec));
            }
        }
    }

    // -----------------------------------------------------------------------------------------
        // Calls
    // -----------------------------------------------------------------------------------------

    fn call_into(&mut self, dst: Option<&str>, call: &Call, on_fail: &OnFail) {
        self.guarded(on_fail, dst, |g| g.call_raw(dst, call));
    }

    fn call_raw(&mut self, dst: Option<&str>, call: &Call) {
        match &call.target {
            CallTarget::User(name) => {
                let args: Vec<String> = call.args.iter().map(|a| self.arg(a)).collect();
                let mut l = fn_name(name);
                for a in args {
                    l.push(' ');
                    l.push_str(&a);
                }
                self.line(&l);
                if let Some(d) = dst {
                    self.line(&format!("{} = $script:__ret", d));
                }
            }
            CallTarget::Method { recv_ty, name } if matches!(name.as_str(), "filter" | "map" | "any" | "all" | "find" | "sort_by") => {
                self.lambda_method(dst, recv_ty, name, call)
            }
            _ => {
                let imp = self.std_impl(call);
                match (imp, dst) {
                    (Impl::Val(v), Some(d)) => self.line(&format!("{} = {}", d, v)),
                    (Impl::Val(v), None) => self.line(&format!("$null = {}", v)),
                    (Impl::Stmt(s), Some(d)) => {
                        self.line(&s);
                        self.line(&format!("{} = $null", d));
                    }
                    (Impl::Stmt(s), None) => self.line(&s),
                }
            }
        }
    }

    fn std_impl(&mut self, call: &Call) -> Impl {
        let a: Vec<String> = call.args.iter().map(|x| self.arg(x)).collect();
        let a0 = a.first().cloned().unwrap_or_default();
        use Impl::{Stmt as S, Val as V};
        match &call.target {
            CallTarget::Builtin(name) => match name.as_str() {
                "print" => S(format!("[Console]::Out.Write({} + \"`n\")", self.joined_text(&call.args))),
                "fmt" => {
                    let items: Vec<String> = call.args.iter().skip(1).filter_map(|x| if let Arg::Expr(e) = x { Some(format!("({})", self.text(e))) } else { None }).collect();
                    V(format!("(__Fmt {} @({}))", a0, items.join(", ")))
                }
                "secret" => V(a0),
                "duration_str" => V(format!("(__DurStr {})", a0)),
                "enum_parse" => V(self.enum_parse(&a0, &call.ret)),
                "enc_list" => V(format!("(__EncL {})", a0)),
                "dec_list" => {
                    let kind = match &call.ret {
                        Ty::List(e) if matches!(**e, Ty::Int | Ty::Duration) => "int",
                        Ty::List(e) if **e == Ty::Bool => "bool",
                        _ => "str",
                    };
                    V(format!("(__DecL {} '{}')", a0, kind))
                }
                other => V(format!("(__Unimplemented {})", lit(other))),
            },
            CallTarget::Std { module, name } => self.std_fn(module, name, &a, call),
            CallTarget::Method { recv_ty, name } => self.method(recv_ty, name, &a, call),
            CallTarget::User(_) => unreachable!(),
        }
    }

    fn enum_parse(&self, text: &str, ret: &Ty) -> String {
        let name = match ret {
            Ty::Enum(n) => n.clone(),
            _ => String::new(),
        };
        let cases: Vec<String> = self.p.enums.iter().find(|e| e.name == name).map(|e| e.cases.iter().map(|c| lit(c)).collect()).unwrap_or_default();
        format!("(__EnumParse {} @({}) {})", text, cases.join(", "), lit(&name))
    }

    fn std_fn(&mut self, module: &str, name: &str, a: &[String], call: &Call) -> Impl {
        use Impl::{Stmt as S, Val as V};
        let g = |i: usize| a.get(i).map(String::as_str).unwrap_or_default();
        let (a0, a1, a2, a3, a4, a5) = (g(0), g(1), g(2), g(3), g(4), g(5));
        match (module, name) {
            ("env", "get") => V(format!("(__EnvGet {})", a0)),
            ("env", "has") => V(format!("($null -ne [Environment]::GetEnvironmentVariable([string]{}))", a0)),
            ("env", "set") => S(format!("[Environment]::SetEnvironmentVariable([string]{}, [string]{})", a0, a1)),
            ("env", "unset") => S(format!("[Environment]::SetEnvironmentVariable([string]{}, [NullString]::Value)", a0)),
            ("fs", "exists") => V(format!("(__PathExists {})", a0)),
            ("fs", "is_dir") => V(format!("(__PathIsDir {})", a0)),
            ("fs", "is_file") => V(format!("(__PathIsFile {})", a0)),
            ("fs", "read") => V(format!("(__FsRead {})", a0)),
            ("fs", "write") => S(format!("__FsWrite {} {} $false", a0, a1)),
            ("fs", "append") => S(format!("__FsWrite {} {} $true", a0, a1)),
            ("fs", "rm") => S(format!("__FsRm {}", a0)),
            ("fs", "mkdir") => S(format!("__FsMkdir {}", a0)),
            ("fs", "copy") => S(format!("__FsCopy {} {}", a0, a1)),
            ("fs", "move") => S(format!("__FsMove {} {}", a0, a1)),
            ("fs", "ls") => V(format!("(__FsLs {})", a0)),
            ("fs", "walk") => V(format!("(__FsWalk {})", a0)),
            ("fs", "glob") => V(format!("(__FsGlob {})", a0)),
            ("fs", "temp_dir") => V("(__FsTemp $true)".into()),
            ("fs", "temp_file") => V("(__FsTemp $false)".into()),
            ("fs", "size") => V(format!("(__FsSize {})", a0)),
            ("fs", "modified") => V(format!("(__FsModified {})", a0)),
            ("fs", "replace") => S(format!("__FsReplace {} {} {}", a0, a1, a2)),
            ("path", "abs") => V(format!("(__PathAbs {})", a0)),
            ("path", "__home") => V(format!("(__P {})", a0)),
            ("dirs", "home") => V("(__Home)".into()),
            ("dirs", "temp") => V("(__TempBase)".into()),
            ("dirs", "cwd") => V("(__Cwd)".into()),
            ("dirs", "config") => V(format!("(__AppDir 'config' {})", a0)),
            ("dirs", "cache") => V(format!("(__AppDir 'cache' {})", a0)),
            ("dirs", "data") => V(format!("(__AppDir 'data' {})", a0)),
            ("sys", "arch") => V("(__SysArch)".into()),
            ("sys", "hostname") => V("([Environment]::MachineName)".into()),
            ("sys", "user") => V("([Environment]::UserName)".into()),
            ("sys", "is_admin") => V("(__SysIsAdmin)".into()),
            ("sys", "require_admin") => S("if (-not (__SysIsAdmin)) { __Throw 'this script must be run as administrator (root)' 77 }".into()),
            ("sys", "cpus") => V("([long][Environment]::ProcessorCount)".into()),
            ("sys", "is_ci") => V("(-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('CI')))".into()),
            ("sys", "cd") => S(format!("__SysCd {}", a0)),
            ("proc", "exists") => V(format!("($null -ne (__Which {}))", a0)),
            ("proc", "which") => V(format!("(__ProcWhich {})", a0)),
            ("time", "now") => V("(__Now)".into()),
            ("time", "now_ms") => V("(__NowMs)".into()),
            ("time", "since") => V(format!("(((__Now) - [long]{}) * 1000L)", a0)),
            ("time", "since_ms") => V(format!("((__NowMs) - [long]{})", a0)),
            ("time", "format") => V(format!("(__TimeFormat {} {})", a0, a1)),
            ("time", "sleep") => S(format!("__Sleep {}", a0)),
            ("time", "hour") => V(format!("([long](__TimeAt {}).Hour)", a0)),
            ("log", "info") => S(format!("__Log 'info: ' {}", self.paren_text(call))),
            ("log", "warn") => S(format!("__Log 'warning: ' {}", self.paren_text(call))),
            ("log", "error") => S(format!("__Log 'error: ' {}", self.paren_text(call))),
            ("log", "debug") => S(format!("if ($script:__verbose) {{ __Log 'debug: ' {} }}", self.paren_text(call))),
            ("ui", "confirm") => V(format!("(__UiConfirm {})", a0)),
            ("ui", "ask") => V(format!("(__UiAsk {} {})", a0, a1)),
            ("ui", "password") => V(format!("(__UiPassword {})", a0)),
            ("ui", "choose") => V(format!("(__UiChoose {} {})", a0, a1)),
            ("math", "min") => V(format!("([long][Math]::Min([long]{}, [long]{}))", a0, a1)),
            ("math", "max") => V(format!("([long][Math]::Max([long]{}, [long]{}))", a0, a1)),
            ("math", "abs") => V(format!("([long][Math]::Abs([long]{}))", a0)),
            ("http", "get") => V(format!("(__Http {} {} 'get' '' {} {} '' '')", a0, a1, a2, a3)),
            ("http", "status") => V(format!("(__Http {} {} 'status' '' {} {} '' '')", a0, a1, a2, a3)),
            ("http", "download") => S(format!("if (-not (__DryW ('http.download ' + {a} + ' ' + {b}) {b})) {{ __Http {a} 300000L 'download' {b} {h} {t} '' '' }}", a = a0, b = a1, h = a2, t = a3)),
            ("http", "request") => V(format!("(__Http {} {} 'request' '' {} {} {} {} {})", a1, a4, a5, g(6), a2, a3, a0)),
            ("http", "post") => V(format!("(__Http {} {} 'post' '' {} {} {} {})", a0, a3, a4, a5, a1, a2)),
            ("fs", "chmod") => S(format!("__FsChmod {} {}", a0, a1)),
            ("hash", "sha256") => V(format!("(__HashSha256 {})", a0)),
            ("pkg", "manager") => V("(__PkgManager)".to_string()),
            ("pkg", "install") => S(format!("__PkgInstall {}", a0)),
            ("semver", "compare") => V(format!("(__SemverCompare {} {})", a0, a1)),
            ("encoding", "base64_encode") => V(format!("(__B64Enc {})", a0)),
            ("encoding", "base64_decode") => V(format!("(__B64Dec {})", a0)),
            ("regex", _) => {
                // the checker made sure the pattern is valid fixed text (D32)
                let src = match call.args.get(1) {
                    Some(Arg::Expr(Expr { kind: ExprKind::Str(s), .. })) => s.clone(),
                    _ => String::new(),
                };
                let re = lit(&crate::regex::Regex::parse(&src).map(|r| r.to_dotnet()).unwrap_or_default());
                match name {
                    "test" => V(format!("([regex]::IsMatch([string]{}, {}))", a0, re)),
                    "find" => V(format!("(__ReFind {} {} {})", a0, re, lit(&src))),
                    "find_all" => V(format!("(__ReAll {} {})", a0, re)),
                    "groups" => V(format!("(__ReGroups {} {} {})", a0, re, lit(&src))),
                    _ => V(format!("(__ReReplace {} {} {})", a0, re, a2)),
                }
            }
            ("hash", "sha256_file") => V(format!("(__HashSha256File {})", a0)),
            ("archive", "tar_create") => S(format!("__ArchiveTarCreate {} {}", a0, a1)),
            ("archive", "tar_extract") => S(format!("__ArchiveTarExtract {} {}", a0, a1)),
            ("archive", "zip_create") => S(format!("__ArchiveZipCreate {} {}", a0, a1)),
            ("archive", "zip_extract") => S(format!("__ArchiveZipExtract {} {}", a0, a1)),
            ("json", "get") => V(format!("(__JsonGet {} {})", a0, a1)),
            ("json", "get_list") => V(format!("(__JsonChildren {} {} 'a' 'item')", a0, a1)),
            ("json", "keys") => V(format!("(__JsonChildren {} {} 'o' 'key')", a0, a1)),
            ("json", "has") => V(format!("(__JsonHas {} {})", a0, a1)),
            ("json", "quote") => V(format!("(__JsonQuote {})", a0)),
            _ => {
                let what = lit(&format!("{}.{}", module, name));
                if call.ret == Ty::Unit {
                    S(format!("__Unimplemented {}", what))
                } else {
                    V(format!("(__Unimplemented {})", what))
                }
            }
        }
    }

    fn paren_text(&self, call: &Call) -> String {
        format!("({})", self.joined_text(&call.args))
    }

    fn method(&mut self, recv_ty: &Ty, name: &str, a: &[String], call: &Call) -> Impl {
        use Impl::{Stmt as S, Val as V};
        let r = a.first().cloned().unwrap_or_default();
        let x = |i: usize| a.get(i).cloned().unwrap_or_default();
        let arg_expr = |i: usize| -> Option<&Expr> {
            match call.args.get(i) {
                Some(Arg::Expr(e)) => Some(e),
                _ => None,
            }
        };
        match recv_ty {
            Ty::Str | Ty::Path | Ty::Secret if PATH_PROPS.iter().any(|(n, _, _)| *n == name) => {
                let f = match name {
                    "name" => "__PathName",
                    "stem" => "__PathStem",
                    "ext" => "__PathExt",
                    _ => "__PathParent",
                };
                V(format!("({} {})", f, r))
            }
            Ty::Str | Ty::Path | Ty::Secret => match name {
                "trim" => V(format!("([string]{}).Trim()", r)),
                "trim_start" => V(format!("([string]{}).TrimStart()", r)),
                "trim_end" => V(format!("([string]{}).TrimEnd()", r)),
                "upper" => V(format!("([string]{}).ToUpperInvariant()", r)),
                "lower" => V(format!("([string]{}).ToLowerInvariant()", r)),
                "contains" => V(format!("([string]{}).Contains([string]{})", r, x(1))),
                "starts_with" => V(format!("([string]{}).StartsWith([string]{}, [StringComparison]::Ordinal)", r, x(1))),
                "ends_with" => V(format!("([string]{}).EndsWith([string]{}, [StringComparison]::Ordinal)", r, x(1))),
                "strip_prefix" => V(format!("(__StrStripPrefix {} {})", r, x(1))),
                "strip_suffix" => V(format!("(__StrStripSuffix {} {})", r, x(1))),
                "index_of" => V(format!("(__StrIndexOf {} {})", r, x(1))),
                "slice" => V(format!("(__StrSlice {} {} {})", r, x(1), x(2))),
                "split" => V(format!("(__StrSplit {} {})", r, x(1))),
                "lines" => V(format!("(__StrLines {})", r)),
                "replace" => V(format!("(__StrReplace {} {} {})", r, x(1), x(2))),
                "replace_first" => V(format!("(__StrReplaceFirst {} {} {})", r, x(1), x(2))),
                "repeat" => V(format!("(__StrRepeat {} {})", r, x(1))),
                "pad_left" => V(format!("(__StrPad {} {} {} $true)", r, x(1), x(2))),
                "pad_right" => V(format!("(__StrPad {} {} {} $false)", r, x(1), x(2))),
                "to_int" => V(format!("(__StrToInt {})", r)),
                "chars" => V(format!("(__Cps {})", r)),
                "is_empty" => V(format!("(([string]{}).Length -eq 0)", r)),
                "to_str" => V(r),
                "len" => V(format!("(__Len {})", r)),
                _ => V(format!("(__Unimplemented {})", lit(&format!("str.{}", name)))),
            },
            Ty::List(elem) => match name {
                "push" => {
                    let v = arg_expr(1).map(|e| self.stored(e)).unwrap_or_default();
                    S(format!("[void]{}.Add({})", r, v))
                }
                "pop" => V(format!("(__ListPop {})", r)),
                "contains" => V(format!("((__ListIndexOf {} {}) -ge 0)", r, x(1))),
                "index_of" => V(format!("(__ListIndexOf {} {})", r, x(1))),
                "join" => V(format!("(__ListJoin {} {})", r, x(1))),
                "sum" => V(format!("(__ListSum {})", r)),
                "sort" => match &**elem {
                    Ty::Int | Ty::Duration => V(format!("(__ListSort {} $false)", r)),
                    Ty::Bool => V(format!("(__ListSortBy {r} {r} $true)", r = r)),
                    _ => V(format!("(__ListSort {} $true)", r)),
                },
                "reverse" => V(format!("(__ListReverse {})", r)),
                "concat" => V(format!("(__ListConcat {} {})", r, x(1))),
                "slice" => V(format!("(__ListSlice {} {} {})", r, x(1), x(2))),
                "unique" => V(format!("(__ListUnique {})", r)),
                "first" => V(format!("(__Copy (__ListFirst {}))", r)),
                "last" => V(format!("(__Copy (__ListLast {}))", r)),
                "is_empty" => V(format!("({}.Count -eq 0)", r)),
                "len" => V(format!("([long]{}.Count)", r)),
                _ => V(format!("(__Unimplemented {})", lit(&format!("list.{}", name)))),
            },
            Ty::Map(k, _) => match name {
                "get" => V(format!("(__Copy (__MapGet {} {}))", r, x(1))),
                "has" => V(format!("({}.Contains([string]{}))", r, x(1))),
                "set" => {
                    let v = arg_expr(2).map(|e| self.stored(e)).unwrap_or_default();
                    S(format!("{}[[string]{}] = {}", r, x(1), v))
                }
                "remove" => S(format!("{}.Remove([string]{})", r, x(1))),
                "keys" => V(format!("(__MapKeys {} {})", r, if matches!(**k, Ty::Int) { "$true" } else { "$false" })),
                "values" => V(format!("(__MapValues {})", r)),
                "is_empty" => V(format!("({}.Count -eq 0)", r)),
                "len" => V(format!("([long]{}.Count)", r)),
                _ => V(format!("(__Unimplemented {})", lit(&format!("map.{}", name)))),
            },
            Ty::Enum(_) => match name {
                "parse" => V(self.enum_parse(&x(1), &call.ret)),
                _ => V(format!("([string]{})", r)),
            },
            Ty::Int => V(format!("([string]{})", r)),
            Ty::Bool => V(format!("(@('false','true')[[int]{}])", r)),
            Ty::Duration => V(format!("(__DurStr {})", r)),
            _ => {
                if name == "parse" {
                                        // `Type.parse(text)` with no value receiver: args[0] is the text.
                    return V(self.enum_parse(&r, &call.ret));
                }
                V(format!("(__Unimplemented {})", lit(name)))
            }
        }
    }

        /// `filter`, `map`, `any`, `all`, `find`, `sort_by`: loop generated in place, calling the lambda (a
        /// `__lam_N` function that returns in `$script:__ret`) as a statement.
    fn lambda_method(&mut self, dst: Option<&str>, recv_ty: &Ty, name: &str, call: &Call) {
        let recv = match call.args.first() {
            Some(Arg::Expr(e)) => self.expr(e),
            _ => "$null".into(),
        };
        let lam = match call.args.get(1) {
            Some(Arg::Lambda(l)) => l,
            _ => {
                self.line("__Unimplemented 'lambda'");
                return;
            }
        };
        let f = self.lambda_fn(lam);
        let it = self.fresh("it");
        let acc = self.fresh("acc");
        let elem_coll = matches!(recv_ty, Ty::List(t) if is_coll(t));
        match name {
            "filter" | "map" => {
                self.line(&format!("{} = {}", acc, LIST_NEW));
                self.line(&format!("foreach ({} in {}.ToArray()) {{", it, recv));
                self.indent += 1;
                self.line(&format!("{} {}", f, it));
                if name == "filter" {
                    let item = if elem_coll { format!("(__Copy {})", it) } else { it.clone() };
                    self.line(&format!("if ($script:__ret) {{ [void]{}.Add({}) }}", acc, item));
                } else {
                    self.line(&format!("[void]{}.Add($script:__ret)", acc));
                }
                self.indent -= 1;
                self.line("}");
            }
            "any" | "all" => {
                let (init, test, set) = if name == "any" { ("$false", "$script:__ret", "$true") } else { ("$true", "-not $script:__ret", "$false") };
                self.line(&format!("{} = {}", acc, init));
                self.line(&format!("foreach ({} in {}.ToArray()) {{", it, recv));
                self.indent += 1;
                self.line(&format!("{} {}", f, it));
                self.line(&format!("if ({}) {{ {} = {}; break }}", test, acc, set));
                self.indent -= 1;
                self.line("}");
            }
            "find" => {
                let found = self.fresh("found");
                self.line(&format!("{} = $false", found));
                self.line(&format!("{} = $null", acc));
                self.line(&format!("foreach ({} in {}.ToArray()) {{", it, recv));
                self.indent += 1;
                self.line(&format!("{} {}", f, it));
                self.line(&format!("if ($script:__ret) {{ {} = __Copy {}; {} = $true; break }}", acc, it, found));
                self.indent -= 1;
                self.line("}");
                self.line(&format!("if (-not {}) {{ __Throw 'find: no item matched' 1 }}", found));
            }
            _ => {
                // sort_by
                let keys = self.fresh("keys");
                let src = self.fresh("src");
                self.line(&format!("{} = {}", src, recv));
                self.line(&format!("{} = {}", keys, LIST_NEW));
                self.line(&format!("foreach ({} in {}.ToArray()) {{", it, src));
                self.indent += 1;
                self.line(&format!("{} {}", f, it));
                self.line(&format!("[void]{}.Add($script:__ret)", keys));
                self.indent -= 1;
                self.line("}");
                let text = if is_text(&lam.ret) { "$true" } else { "$false" };
                self.line(&format!("{} = __ListSortBy {} {} {}", acc, src, keys, text));
            }
        }
        if let Some(d) = dst {
            self.line(&format!("{} = {}", d, acc));
        }
    }

        /// Generates `function __lam_N($o_x) { … }` and returns the name.
    fn lambda_fn(&mut self, lam: &Lambda) -> String {
        self.tmp += 1;
        let name = format!("__lam_{}", self.tmp);
        let mut locals = HashSet::new();
        for p in &lam.params {
            locals.insert(p.name.clone());
        }
        collect_locals(&lam.body, &mut locals);
        let mut outer = self.ctx.outer.clone();
        if self.ctx.in_func {
            outer.extend(self.ctx.locals.iter().cloned());
        }
        let ctx = Ctx { in_func: true, locals, outer };
        let params: Vec<String> = lam.params.iter().map(|p| format!("${}", mangle(&p.name))).collect();
        let saved = std::mem::replace(&mut self.ctx, ctx);
        let body = self.capture(1, |g| g.block(&lam.body));
        self.ctx = saved;
        self.extra.push(format!("function {}({}) {{\n{}}}\n", name, params.join(", "), body));
        name
    }

    // -----------------------------------------------------------------------------------------
        // Commands
    // -----------------------------------------------------------------------------------------

    fn word(&self, w: &Word) -> (String, bool) {
        match w {
            Word::Lit(s) => (lit(s), false),
            Word::Interp(parts) => (self.interp(parts), false),
            Word::Spread(e) => (format!("@({}.ToArray())", self.expr(e)), true),
        }
    }

    fn stage(&self, sc: &SimpleCmd) -> String {
                // groups of simple words in an @(…), spread lists added together
        let mut groups: Vec<String> = Vec::new();
        let mut cur: Vec<String> = Vec::new();
        for w in &sc.words {
            let (x, spread) = self.word(w);
            if spread {
                if !cur.is_empty() {
                    groups.push(format!("@({})", cur.join(", ")));
                    cur.clear();
                }
                groups.push(x);
            } else {
                cur.push(x);
            }
        }
        if !cur.is_empty() {
            groups.push(format!("@({})", cur.join(", ")));
        }
        format!("[object[]]({})", groups.join(" + "))
    }

        /// The text shown by `--verbose`/`--dry-run`: `Command.text` with each `{…}` replaced by its value
        /// (secrets as `******`). If the count of `{…}` does not match the expressions, it is built from the words.
    fn cmd_display(&self, cmd: &Command) -> String {
        let mut vals: Vec<String> = Vec::new();
        let show = |e: &Expr| -> String {
            match &e.ty {
                Ty::Secret => "'******'".into(),
                Ty::List(t) if **t == Ty::Secret => "'******'".into(),
                Ty::List(_) => format!("(__ListJoin {} ' ')", self.expr(e)),
                _ => self.text(e),
            }
        };
        for sc in &cmd.pipeline {
            // `NAME=value` comes before the program in the text
            for w in sc.env.iter().map(|(_, w)| w).chain(&sc.words) {
                match w {
                    Word::Lit(_) => {}
                    Word::Interp(ps) => {
                        for p in ps {
                            if let Part::Expr(e) = p {
                                vals.push(show(e))
                            }
                        }
                    }
                    Word::Spread(e) => vals.push(show(e)),
                }
            }
        }
        for r in &cmd.redirects {
            if let Redirect::Out { path, .. } | Redirect::Err { path, .. } | Redirect::In(path) = r {
                match &path.kind {
                    // a fixed path has no `{…}` in the text
                    ExprKind::Str(_) => {}
                    ExprKind::Interp(ps) => {
                        for p in ps {
                            if let Part::Expr(e) = p {
                                vals.push(show(e))
                            }
                        }
                    }
                    _ => vals.push(show(path)),
                }
            }
        }
        // `{…}` segments of the text (the same rule as the Bash backend: not inside single quotes)
        let text = &cmd.text;
        let segs = crate::bash::brace_groups(text);
        if segs.len() == vals.len() && !text.is_empty() {
            let mut parts: Vec<String> = Vec::new();
            let mut last = 0;
            for ((s, e), v) in segs.iter().zip(vals) {
                if *s > last {
                    parts.push(lit(&text[last..*s]));
                }
                parts.push(v);
                last = *e;
            }
            if last < text.len() {
                parts.push(lit(&text[last..]));
            }
            return format!("('' + {})", parts.join(" + "));
        }
                // fallback: the evaluated words
        let mut stages: Vec<String> = Vec::new();
        for sc in &cmd.pipeline {
            let ws: Vec<String> = sc
                .words
                .iter()
                .map(|w| match w {
                    Word::Lit(s) => lit(s),
                    Word::Interp(ps) => {
                        let secret = ps.iter().any(|p| matches!(p, Part::Expr(e) if e.ty == Ty::Secret));
                        if secret {
                            "'******'".into()
                        } else {
                            self.interp(ps)
                        }
                    }
                    Word::Spread(e) => show(e),
                })
                .collect();
            stages.push(ws.join(" + ' ' + "));
        }
        format!("('' + {})", stages.join(" + ' | ' + "))
    }

    fn cmd_spec(&self, cmd: &Command) -> String {
        let stages: Vec<String> = cmd.pipeline.iter().map(|s| self.stage(s)).collect();
        let s = if stages.len() == 1 { format!("@(,{})", stages[0]) } else { format!("@({})", stages.join(", ")) };
        let mut o = "$null".to_string();
        let mut oa = "$false";
        let mut e = "$null".to_string();
        let mut ea = "$false";
        let mut i = "$null".to_string();
        let mut e2o = "$false";
        for r in &cmd.redirects {
            match r {
                Redirect::Out { path, append } => {
                    o = self.expr(path);
                    oa = if *append { "$true" } else { "$false" };
                }
                Redirect::Err { path, append } => {
                    e = self.expr(path);
                    ea = if *append { "$true" } else { "$false" };
                }
                Redirect::In(path) => i = self.expr(path),
                Redirect::ErrToOut => e2o = "$true",
            }
        }
        let (hd, d) = match &cmd.stdin {
            Some(x) => ("$true", self.text(x)),
            None => ("$false", "$null".to_string()),
        };
        // v = per stage, the `NAME=value` pairs as a flat list (name, value, …), or $null
        let v = if cmd.pipeline.iter().any(|s| !s.env.is_empty()) {
            let per: Vec<String> = cmd
                .pipeline
                .iter()
                .map(|s| {
                    if s.env.is_empty() {
                        return "$null".to_string();
                    }
                    let kv: Vec<String> = s.env.iter().map(|(n, w)| format!("{}, {}", lit(n), self.word(w).0)).collect();
                    format!("@({})", kv.join(", "))
                })
                .collect();
            // a single stage needs the `,` so the inner list is not flattened
            if per.len() == 1 {
                format!("; v = @(,{})", per[0])
            } else {
                format!("; v = @({})", per.join(", "))
            }
        } else {
            String::new()
        };
        format!(
            "@{{ t = {}; s = {}; o = {}; oa = {}; e = {}; ea = {}; i = {}; e2o = {}; hd = {}; d = {}{} }}",
            self.cmd_display(cmd),
            s,
            o,
            oa,
            e,
            ea,
            i,
            e2o,
            hd,
            d,
            v
        )
    }

    // -----------------------------------------------------------------------------------------
        // defer, retry, parallel
    // -----------------------------------------------------------------------------------------

        /// `defer x`: the values of the variables read are captured now; the statement becomes a function
        /// `__defer_N` that receives those values, and goes onto the `$__defers` stack of the function (or the top level).
    fn gen_defer(&mut self, inner: &Stmt) {
        self.tmp += 1;
        let name = format!("__defer_{}", self.tmp);
        let mut vars = Vec::new();
        vars_in_stmt(inner, &mut vars);
        let mut locals: HashSet<String> = vars.iter().map(|(n, _)| n.clone()).collect();
        collect_locals_stmt(inner, &mut locals);
        let ctx = Ctx { in_func: true, locals, outer: HashSet::new() };
                // captured values (collections are copied)
        let da = self.fresh("da");
        self.line(&format!("{} = {}", da, LIST_NEW));
        for (v, t) in &vars {
            let r = self.var(v);
            if is_coll(t) {
                self.line(&format!("[void]{}.Add((__Copy {}))", da, r));
            } else {
                self.line(&format!("[void]{}.Add({})", da, r));
            }
        }
        self.line(&format!("[void]$__defers.Add(@('{}', {}.ToArray()))", name, da));
        let params: Vec<String> = vars.iter().map(|(v, _)| format!("${}", mangle(v))).collect();
        let saved = std::mem::replace(&mut self.ctx, ctx);
        let body = self.capture(1, |g| match &inner.kind {
            StmtKind::Call { call, .. } => g.call_into(None, call, &OnFail::Propagate),
            StmtKind::Command { cmd, .. } => {
                let spec = g.cmd_spec(cmd);
                g.line(&format!("__Cmd {}", spec));
            }
            _ => g.stmt(inner),
        });
        self.ctx = saved;
        self.extra.push(format!("function {}({}) {{\n{}}}\n", name, params.join(", "), body));
    }

    fn gen_retry(&mut self, times: &Expr, every: &Expr, body: &Block, on_fail: &OnFail) {
        let n = self.fresh("rn");
        let w = self.fresh("rw");
        let i = self.fresh("ri");
        let ok = self.fresh("rok");
        let er = self.fresh("rerr");
        let (t, e) = (self.expr(times), self.expr(every));
        self.line(&format!("{} = [long]{}", n, t));
        self.line(&format!("{} = [long]{}", w, e));
        self.line(&format!("{} = $false", ok));
        self.line(&format!("{} = __Map @('message', 'retry: no attempts', 'code', 1L)", er));
        self.line(&format!("for ({i} = 1L; {i} -le {n}; {i}++) {{", i = i, n = n));
        self.indent += 1;
        self.line("try {");
        self.indent += 1;
        self.block(body);
        self.line(&format!("{} = $true", ok));
        self.line("break");
        self.indent -= 1;
        self.line("} catch {");
        self.indent += 1;
        self.line(&format!("{} = __ErrOf $_", er));
        self.line(&format!("if ({} -lt {}) {{ __Sleep {} }}", i, n, w));
        self.indent -= 1;
        self.line("}");
        self.indent -= 1;
        self.line("}");
        self.line(&format!("if (-not {}) {{", ok));
        self.indent += 1;
        self.handler(on_fail, None, &er);
        self.indent -= 1;
        self.line("}");
    }

        /// `parallel`: each task becomes a `__task_N` function (as in Bash) and `__ParRun` (runtime) runs the tasks
        /// at the same time in a RunspacePool, at most `max` at once (default: the number of CPUs), and waits for all of them.
        /// As in Bash: the results of `parallel for` go into the list in list order; errors are joined
        /// in order (`parallel: e1; e2`) with the first one's code; `exit` inside a task only ends the task
        /// ("task N finished without a result"). Each runspace gets the script's functions and a copy of the script
        /// variables; those of the enclosing function are read-only (the checker forbids the task from changing them).
        /// Tasks that change the directory (`sys.cd`) or the environment (`env.set`), which are process-wide, run
        /// one at a time. Without a RunspacePool (Constrained Language), they run sequentially, as in v0.1.
    fn gen_parallel(&mut self, max: Option<&Expr>, body: &ParallelBody, on_fail: &OnFail) {
                // variables of the enclosing function that tasks may read (at the top level they are in the script scope),
                // without the tasks' own
        let mut own: HashSet<String> = HashSet::new();
        match body {
            ParallelBody::For { var, body, .. } => {
                own.insert(var.clone());
                collect_locals(body, &mut own);
            }
            ParallelBody::Tasks(blocks) => blocks.iter().for_each(|b| collect_locals(b, &mut own)),
        }
        let mut names: Vec<String> = Vec::new();
        if self.ctx.in_func {
            names = self.ctx.locals.iter().chain(self.ctx.outer.iter()).filter(|n| !own.contains(*n)).map(|n| mangle(n)).collect();
            names.sort();
            names.dedup();
        }
        let mut fns: Vec<String> = Vec::new();
        let items;
        let mut collect_to: Option<&Name> = None;
        let serial;
        match body {
            ParallelBody::For { var, list, body, collect } => {
                fns.push(self.task_fn(Some(var), body, collect.as_ref().map(|(_, e)| e)));
                items = self.expr(list);
                collect_to = collect.as_ref().map(|(n, _)| n);
                serial = self.touches_process(&[body]);
            }
            ParallelBody::Tasks(blocks) => {
                for b in blocks {
                    fns.push(self.task_fn(None, b, None));
                }
                items = "$null".to_string();
                serial = self.touches_process(&blocks.iter().collect::<Vec<_>>());
            }
        }
        let mx = match max {
            Some(e) => self.expr(e),
            None => "([long][Environment]::ProcessorCount)".to_string(),
        };
        let pr = self.fresh("par");
        let fl: Vec<String> = fns.iter().map(|f| lit(f)).collect();
        let nl: Vec<String> = names.iter().map(|n| lit(n)).collect();
        self.line(&format!(
            "{} = __ParRun @({}) {} {} @({}) {}",
            pr,
            fl.join(", "),
            items,
            mx,
            nl.join(", "),
            if serial { "$true" } else { "$false" }
        ));
        let err = format!("{}['err']", pr);
        match collect_to {
            Some(name) => {
                let n = self.var(name);
                self.line(&format!("if ({}['ok']) {{", pr));
                self.indent += 1;
                self.line(&format!("{} = {}['res']", n, pr));
                self.indent -= 1;
                self.line("} else {");
            }
            None => self.line(&format!("if (-not {}['ok']) {{", pr)),
        }
        self.indent += 1;
        self.handler(on_fail, None, &err);
        self.indent -= 1;
        self.line("}");
    }

        /// A `parallel` task's function: `function __task_N($o_x) { … }`, with the task's own `defer` and
        /// `sys.cd`. It sees the enclosing function's variables through dynamic scoping (in the runspace,
        /// `__ParTask` defines them in a scope above it); top-level ones, with `$script:`. With `collect`, the value goes in
        /// `$script:__ret`. As in Bash (where the task is a subshell), an `exit` inside the task does not run its
        /// `defer`s: hence `catch` + `throw` instead of `finally`.
    fn task_fn(&mut self, param: Option<&Name>, body: &Block, collect: Option<&Expr>) -> String {
        self.tmp += 1;
        let name = format!("__task_{}", self.tmp);
        let mut locals: HashSet<String> = HashSet::new();
        if let Some(p) = param {
            locals.insert(p.clone());
        }
        collect_locals(body, &mut locals);
        let mut outer = self.ctx.outer.clone();
        if self.ctx.in_func {
            outer.extend(self.ctx.locals.iter().cloned());
        }
        let ctx = Ctx { in_func: true, locals, outer };
        let saved = std::mem::replace(&mut self.ctx, ctx);
        let defers = has_defer(body);
        let cd = uses_cd(body);
        let text = self.capture(1, |g| {
            if cd {
                g.line("$__cwd0 = (Get-Location).ProviderPath");
            }
            if defers {
                g.line(&format!("$__defers = {}", LIST_NEW));
            }
            let wrap = defers || cd;
            if wrap {
                g.line("try {");
                g.indent += 1;
            }
            g.block(body);
            if let Some(e) = collect {
                let x = g.stored(e);
                g.line(&format!("$script:__ret = {}", x));
            }
            if wrap {
                g.indent -= 1;
                g.line("} catch {");
                g.indent += 1;
                g.line("$__te = $_");
                g.task_cleanup(defers, cd);
                g.line("throw $__te");
                g.indent -= 1;
                g.line("}");
                g.task_cleanup(defers, cd);
            }
        });
        self.ctx = saved;
        let params = param.map(|p| format!("${}", mangle(p))).unwrap_or_default();
        self.extra.push(format!("function {}({}) {{\n{}}}\n", name, params, text));
        name
    }

    fn task_cleanup(&mut self, defers: bool, cd: bool) {
        if defers {
            self.line("__RunDefers $__defers");
        }
        if cd {
            self.line("Set-Location -LiteralPath $__cwd0");
            self.line("[Environment]::CurrentDirectory = $__cwd0");
        }
    }

        /// Do the tasks change the directory (`sys.cd`) or the environment (`env.set`), directly or through a user function?
        /// Both are **process**-wide (.NET resolves relative paths from the process directory), shared
        /// among the runspaces: such tasks run one at a time so they do not interfere.
    fn touches_process(&self, blocks: &[&Block]) -> bool {
        let mut seen: HashSet<String> = HashSet::new();
        let mut todo: Vec<&Block> = blocks.to_vec();
        let mut hit = false;
        while let Some(b) = todo.pop() {
            each_call(b, &mut |c: &Call| match &c.target {
                CallTarget::Std { module, name } if (module == "sys" && name == "cd") || (module == "env" && name == "set") => hit = true,
                CallTarget::User(n) => {
                    if seen.insert(n.clone()) {
                        if let Some(f) = self.p.funcs.iter().find(|f| &f.name == n) {
                            todo.push(&f.body);
                        }
                    }
                }
                _ => {}
            });
            if hit {
                return true;
            }
        }
        false
    }

    // -----------------------------------------------------------------------------------------
        // Functions, parameters and the program
    // -----------------------------------------------------------------------------------------

    fn func(&mut self, f: &Func) -> String {
        let mut locals: HashSet<String> = f.params.iter().map(|p| p.name.clone()).collect();
        collect_locals(&f.body, &mut locals);
        let ctx = Ctx { in_func: true, locals, outer: HashSet::new() };
        let saved = std::mem::replace(&mut self.ctx, ctx);
        let defers = has_defer(&f.body);
        let cd = uses_cd(&f.body);
        let body = if defers || cd {
            self.capture(1, |g| {
                if cd {
                    g.line("$__cwd0 = (Get-Location).ProviderPath");
                }
                if defers {
                    g.line(&format!("$__defers = {}", LIST_NEW));
                }
                g.line("try {");
                g.indent += 1;
                g.block(&f.body);
                g.indent -= 1;
                g.line("} finally {");
                g.indent += 1;
                if defers {
                    g.line("__RunDefers $__defers");
                }
                if cd {
                    g.line("Set-Location -LiteralPath $__cwd0");
                    g.line("[Environment]::CurrentDirectory = $__cwd0");
                }
                g.indent -= 1;
                g.line("}");
            })
        } else {
            self.capture(1, |g| g.block(&f.body))
        };
        self.ctx = saved;
        let params: Vec<String> = f.params.iter().map(|p| format!("${}", mangle(&p.name))).collect();
        let mut s = String::new();
        if !f.doc.is_empty() {
            for l in f.doc.lines() {
                let _ = writeln!(s, "# {}", l);
            }
        }
        let _ = write!(s, "function {}({}) {{\n{}}}\n", fn_name(&f.name), params.join(", "), body);
        s
    }

    fn param_kind(&self, t: &Ty) -> (&'static str, String) {
        match t {
            Ty::Int => ("int", "$null".into()),
            Ty::Bool => ("bool", "$null".into()),
            Ty::Duration => ("duration", "$null".into()),
            Ty::Enum(n) => {
                let cases: Vec<String> = self.p.enums.iter().find(|e| &e.name == n).map(|e| e.cases.iter().map(|c| lit(c)).collect()).unwrap_or_default();
                ("enum", format!("@({})", cases.join(", ")))
            }
            Ty::List(e) if **e == Ty::Int => ("list_int", "$null".into()),
            Ty::List(_) => ("list_str", "$null".into()),
            _ => ("str", "$null".into()),
        }
    }

    fn params(&mut self) {
        let p = self.p;
        let spec: Vec<String> = p
            .params
            .iter()
            .map(|x| {
                let (k, c) = self.param_kind(&x.ty);
                format!("@{{ f = {}; k = '{}'; c = {} }}", lit(&x.flag), k, c)
            })
            .collect();
        self.line(&format!("$__pspec = @({})", spec.join(", ")));
        self.line(&format!("$__pa = __ParseArgs $args $__pspec {}", lit(&p.usage)));
        self.line("$script:__args = $__pa['']");
        for x in &p.params {
            let v = self.var(&x.name);
            self.line(&format!("if ($__pa.Contains({})) {{", lit(&x.flag)));
            self.indent += 1;
            self.line(&format!("{} = $__pa[{}]", v, lit(&x.flag)));
            self.indent -= 1;
            let env = x.env_var.clone().filter(|_| x.ty == Ty::Secret);
            if let Some(ev) = &env {
                self.line(&format!("}} elseif ($null -ne [Environment]::GetEnvironmentVariable({})) {{", lit(ev)));
                self.indent += 1;
                self.line(&format!("{} = [Environment]::GetEnvironmentVariable({})", v, lit(ev)));
                self.indent -= 1;
            }
            self.line("} else {");
            self.indent += 1;
            match (&x.default, &x.ty) {
                (Some(d), _) => self.assign_rvalue(&v, d),
                (None, Ty::Bool) => self.line(&format!("{} = $false", v)),
                (None, _) => {
                    let extra = env.map(|e| format!(" (or set {})", e)).unwrap_or_default();
                    self.line(&format!("__UsageErr {}", lit(&format!("{} is required{}", x.flag, extra))));
                }
            }
            self.indent -= 1;
            self.line("}");
        }
    }

    fn prelude(&mut self) {
        let p = self.p;
                // ASCII only in the file (the source name may have accents)
        let source: String = p.source_file.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' }).collect();
        self.line(&format!("# Generated by nalu {} from {} (target: windows). Do not edit.", env!("CARGO_PKG_VERSION"), source));
        self.line("# Windows PowerShell 5.1+ (also runs on PowerShell 7). ASCII with CRLF, so it also runs with irm | iex.");
        self.line("Set-StrictMode -Version 2");
        self.line("$ErrorActionPreference = 'Stop'");
        self.line("$ProgressPreference = 'SilentlyContinue'");
        self.line("$__utf8nb = New-Object System.Text.UTF8Encoding $false");
        self.line("try { [Console]::OutputEncoding = $__utf8nb } catch { }");
        self.line("$OutputEncoding = $__utf8nb");
        self.line("[Environment]::CurrentDirectory = (Get-Location).ProviderPath");
        self.line("$script:__dry = $false");
        self.line("$script:__verbose = $false");
        self.line(&format!("$script:__args = {}", LIST_NEW));
        self.line("$script:__ret = $null");
        self.line("$__e = $null");
    }

    pub fn program(mut self, mode: Mode) -> String {
        let p = self.p;
                // user functions
        let mut funcs = String::new();
        for f in &p.funcs {
            funcs.push_str(&self.func(f));
            funcs.push('\n');
        }
        let main = match mode {
            Mode::Script => self.capture(0, |g| g.main_script()),
            Mode::Tests => self.capture(0, |g| g.main_tests()),
        };
        let extra = self.extra.join("\n");
        let head = self.capture(0, |g| g.prelude());
        let code = format!("{}{}{}", funcs, extra, main);
        let rt = super::runtime::select(&format!("{}{}", head, code));
        let mut out = head;
        out.push_str("\n# ---- runtime ----\n");
        out.push_str(&rt);
        if !funcs.is_empty() || !extra.is_empty() {
            out.push_str("# ---- functions ----\n");
            out.push_str(&funcs);
            out.push_str(&extra);
            if !extra.is_empty() {
                out.push('\n');
            }
        }
        out.push_str("# ---- script ----\n");
        out.push_str(&main);
        out
    }

    fn main_script(&mut self) {
        let p = self.p;
        self.line(&format!("$__defers = {}", LIST_NEW));
        self.line("$__code = 0");
        self.line("try {");
        self.indent += 1;
        self.params();
        if !p.deps.is_empty() {
            let ds: Vec<String> = p.deps.iter().map(|d| lit(d)).collect();
            self.line(&format!("__Deps @({})", ds.join(", ")));
        }
        self.block(&p.body);
        self.indent -= 1;
        self.line("} catch {");
        self.indent += 1;
        self.line("$__e = __ErrOf $_");
        self.line("[Console]::Out.Flush()");
        self.line("__ErrLine ('error: ' + $__e['message'])");
        self.line("$__code = __ExitCode $__e['code']");
        self.indent -= 1;
        self.line("} finally {");
        self.indent += 1;
        self.line("__RunDefers $__defers");
        self.line("[Console]::Out.Flush()");
        self.indent -= 1;
        self.line("}");
                // Running from a file (`-File`, `& .\\x.ps1`), exit with the code; with `irm … | iex` in the console, `exit`
                // would close the user's window: the code stays in `$LASTEXITCODE`.
        self.line("if ($PSCommandPath) { exit $__code }");
        self.line("$global:LASTEXITCODE = $__code");
    }

    fn main_tests(&mut self) {
        let p = self.p;
        self.line(&format!("$__defers = {}", LIST_NEW));
                // parameters with the default (or the zero value)
        for x in &p.params {
            let v = self.var(&x.name);
            match &x.default {
                Some(d) => self.assign_rvalue(&v, d),
                None => {
                    let z = if x.ty == Ty::Bool { "$false".into() } else { zero_of(&x.ty) };
                    self.line(&format!("{} = {}", v, z));
                }
            }
        }
                // top-level pure declarations
        for s in &p.body.stmts {
            if let StmtKind::Let { value: Rvalue::Expr(_) | Rvalue::List { .. } | Rvalue::Map { .. } | Rvalue::Struct { .. }, .. } = &s.kind {
                self.line("try {");
                self.indent += 1;
                self.stmt(s);
                self.indent -= 1;
                self.line("} catch { }");
            }
        }
        self.line("$__nf = 0");
        for (i, t) in p.tests.iter().enumerate() {
            let fname = format!("__test_{}", i + 1);
            let f = Func { name: String::new(), params: vec![], ret: Ty::Unit, body: t.body.clone(), fallible: true, doc: String::new() };
            let text = self.func(&f).replacen("function fn_(", &format!("function {}(", fname), 1);
            self.extra.push(text);
            self.line(&format!("# test {}", lit(&t.name)));
            self.line("try {");
            self.indent += 1;
            self.line(&fname);
            self.line(&format!("[Console]::Out.Write({} + \"`n\")", lit(&format!("ok      {}", t.name))));
            self.indent -= 1;
            self.line("} catch {");
            self.indent += 1;
            self.line("$__nf++");
            self.line(&format!("[Console]::Out.Write({} + (__ErrOf $_)['message'] + \"`n\")", lit(&format!("FAILED  {}: ", t.name))));
            self.indent -= 1;
            self.line("}");
        }
        self.line(&format!("[Console]::Out.Write('{} test(s), ' + $__nf + \" failure(s)`n\")", p.tests.len()));
        self.line("__RunDefers $__defers");
        self.line("[Console]::Out.Flush()");
        self.line("if ($__nf -gt 0) { exit 1 }");
        self.line("exit 0");
    }
}
