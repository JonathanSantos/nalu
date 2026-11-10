//! Lowering (SPEC-001): typed tree (`check::TProgram`) + target → `ir::Program`, and the frontend entry point
//! (`compile`).
//!
//! Guarantees the rules at the top of `ir.rs`:
//! - **A-normal**: every call, capture, command and collection becomes an `Rvalue` in a `Let`/`Assign`/`Call`; an
//!   `Expr` is pure and scalar (collections only as `Var`/`Field`, or `Args`). Temporaries: `__t1`, `__t2`…
//! - **`OnFail` on everything that can fail** (`Propagate` when there is no `catch`). A `catch` whose left side has
//!   nested fallible parts (`f(g()) catch x`) becomes `Let slot` + `Try { … } catch …`, so the `catch` covers everything.
//! - **Unique names** per function/test and at the top level (`x`, `x_2`…); functions never reuse global names.
//! - `os` resolved (dead arm removed, `match os` reduced), `embed` is already text, `+=` is already a `Binary`,
//!   complete positional arguments, `if`/`match`/`parallel` as a value become `Let` + assignments,
//!   `while` with an impure condition becomes `while true` + `if !c { break }`, `&&`/`||` with an impure right side
//!   become a nested `if` (short-circuit preserved).
//! - `Interp` only with `Str/Path/Int/Bool/Enum` parts (`Secret` only in command words): duration →
//!   `Builtin duration_str`; error → `Field(err, message)`.
//! - `usage` in the format of `docs/language.md` (params); `deps` in order of appearance; `W010` per target.
//!
//! `--explain`: [`explain`] returns each point that can fail without `catch` and where the failure goes
//! (``app.nalu:10:16: `fs.read` can fail → `version` fails``).

use crate::check::{self, TArg, TBlock, TCall, TCmd, TExpr, THandler, TLValue, TLambda, TLambdaBody, TPar, TPart, TPattern, TProgram, TRedir, TStmt, TTarget, TWord, TK, TS};
use crate::diag::{Diag, Severity, Span};
use crate::ir::*;
use crate::target::Target;
use crate::Compiled;
use std::collections::HashSet;

/// Shell builtins that do not go into `Program.deps`.
const SHELL_BUILTINS: &[&str] = &["cd", "echo", "test", "[", "true", "false", "printf", "read", "exit", "export", "set", "unset"];
/// Programs that do not exist on Windows (`W010`).
const UNIX_ONLY: &[&str] = &[
    "grep", "sed", "awk", "head", "tail", "cut", "tr", "wc", "sort", "uniq", "xargs", "find", "chmod", "chown", "ln", "ps", "kill", "sudo", "brew", "apt-get", "apt",
    "dnf", "yum", "pacman", "apk",
];
/// Programs that only exist on Windows (`W010`).
const WINDOWS_ONLY: &[&str] = &["winget", "choco", "scoop"];

// =================================================================================================
// Entry point
// =================================================================================================

/// Runs the whole frontend. On a thread with a large stack: even absurdly nested input does not overflow it.
pub fn compile(source: &str, file: &str, embed_root: &std::path::Path, target: Target) -> Compiled {
    let r = std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("nalu-frontend".into())
            .stack_size(256 << 20)
            .spawn_scoped(s, || compile_inner(source, file, embed_root, target))
            .ok()
            .map(|h| h.join())
    });
    match r {
        Some(Ok(c)) => c,
        // could not create the thread: run here instead
        None => compile_inner(source, file, embed_root, target),
        Some(Err(_)) => Compiled {
            program: None,
            diags: vec![Diag::error("E999", Span::new(0, 0), "internal compiler error (please report it with the file that caused it)")],
        },
    }
}

fn finish(mut diags: Vec<Diag>) -> Vec<Diag> {
    diags.sort_by_key(|d| (d.span.start, d.severity != Severity::Error, d.code));
    diags.dedup();
    diags
}

fn compile_inner(source: &str, file: &str, embed_root: &std::path::Path, target: Target) -> Compiled {
    let (ast, diags) = crate::parser::parse(source);
    if diags.iter().any(|d| d.is_error()) {
        return Compiled { program: None, diags: finish(diags) };
    }
    let (tp, mut d2) = check::check(&ast, source, embed_root);
    let mut diags = diags;
    diags.append(&mut d2);
    if diags.iter().any(|d| d.is_error()) {
        return Compiled { program: None, diags: finish(diags) };
    }
    let (mut prog, mut d3) = lower(&tp, source, file, target);
    crate::nest::flatten_structs(&mut prog);
    crate::nest::encode(&mut prog);
    diags.append(&mut d3);
    Compiled { program: Some(prog), diags: finish(diags) }
}

/// The IR as text (`nalu emit --ir`).
pub fn program_to_string(p: &Program) -> String {
    format!("{:#?}", p)
}

/// A point that can fail and has no `catch` around it: where it is, what it is, and where the failure goes.
#[derive(Clone, Debug, PartialEq)]
pub struct FailPoint {
    pub span: Span,
    /// What can fail, as the checker describes it: `` `fs.read` ``, `` `$ git` ``, `` `list[i]` ``.
    pub what: String,
    /// Where the failure goes: `the script stops`, `` `publish` fails ``, `test "x" fails`.
    pub effect: String,
}

/// Result of [`explain`]: the points that can fail, in order, and the functions that can fail.
#[derive(Clone, Debug, Default)]
pub struct Explained {
    pub points: Vec<FailPoint>,
    pub fallible_fns: Vec<String>,
}

/// `nalu check --explain` (D23): **every** point that can fail without a `catch`, with its effect. It answers the
/// question that implicit propagation leaves open ("can this line stop the script?"). With compile
/// errors, returns the diagnostics.
pub fn explain(source: &str, embed_root: &std::path::Path) -> Result<Explained, Vec<Diag>> {
    let (ast, diags) = crate::parser::parse(source);
    if diags.iter().any(|d| d.is_error()) {
        return Err(diags);
    }
    let (tp, d2) = check::check(&ast, source, embed_root);
    if d2.iter().any(|d| d.is_error()) {
        return Err(d2);
    }
    let fnf = &tp.fallible;
    let mut points = Vec::new();
    let mut add = |stmts: &[check::TStmt], effect: String| {
        let mut r = Vec::new();
        check::reasons_stmts(stmts, fnf, &mut r);
        for (what, span) in r {
            points.push(FailPoint { span, what, effect: effect.clone() });
        }
    };
    add(&tp.body, "the script stops".into());
    for (i, f) in tp.funcs.iter().enumerate() {
        add(&f.body, format!("`{}` fails", tp.fns[i].name));
    }
    for t in &tp.tests {
        add(&t.body, format!("test \"{}\" fails", t.name));
    }
    points.sort_by_key(|p| (p.span.start, p.span.end));
    points.dedup();
    let fallible_fns = tp.fns.iter().enumerate().filter(|(i, _)| tp.fallible[*i]).map(|(_, f)| f.name.clone()).collect();
    Ok(Explained { points, fallible_fns })
}

/// Duration (ms) as text: `0s`, `250ms`, `1.5s`, `1m30s`, `1d2h`, `1m500ms`.
pub fn format_duration(ms: i64) -> String {
    if ms < 0 {
        return format!("-{}", format_duration(ms.checked_neg().unwrap_or(i64::MAX)));
    }
    if ms == 0 {
        return "0s".into();
    }
    let mut out = String::new();
    let mut r = ms;
    for (unit, size) in [("d", 86_400_000i64), ("h", 3_600_000), ("m", 60_000)] {
        if r >= size {
            out.push_str(&format!("{}{}", r / size, unit));
            r %= size;
        }
    }
    if r > 0 {
        if r < 1000 {
            out.push_str(&format!("{}ms", r));
        } else if r % 1000 == 0 {
            out.push_str(&format!("{}s", r / 1000));
        } else {
            let frac = format!("{:03}", r % 1000);
            out.push_str(&format!("{}.{}s", r / 1000, frac.trim_end_matches('0')));
        }
    }
    out
}

// =================================================================================================
// Lowering
// =================================================================================================

#[derive(Default, Clone)]
struct Ns {
    used: HashSet<String>,
    temp: u32,
}

#[derive(Clone)]
enum ErrMode {
    Current,
    Var(String),
}

struct Lowerer<'a> {
    tp: &'a TProgram,
    target: Target,
    diags: Vec<Diag>,
    names: Vec<Option<String>>,
    ns: Ns,
    line_starts: Vec<usize>,
    deps: Vec<(usize, String)>,
    collect_deps: bool,
    /// Programs a surrounding `if proc.exists("x")` already checked: not required up front, no `W010`.
    guarded: Vec<String>,
    line: u32,
    err_mode: Vec<ErrMode>,
}

/// Turns the typed tree into the IR for a target. Also returns the target-dependent warnings (`W010`).
pub fn lower(tp: &TProgram, source: &str, file: &str, target: Target) -> (Program, Vec<Diag>) {
    let mut line_starts = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let mut l = Lowerer {
        tp,
        target,
        diags: Vec::new(),
        names: vec![None; tp.vars.len()],
        ns: Ns::default(),
        line_starts,
        deps: Vec::new(),
        collect_deps: true,
        guarded: Vec::new(),
        line: 1,
        err_mode: vec![ErrMode::Current],
    };
    let prog = l.program(file);
    (prog, l.diags)
}

fn zero_lit(ty: &Ty) -> Option<Expr> {
    Some(match ty {
        Ty::Str => Expr::str(""),
        Ty::Path => Expr { kind: ExprKind::Str(String::new()), ty: Ty::Path },
        Ty::Int => Expr::int(0),
        Ty::Bool => Expr::bool(false),
        Ty::Duration => Expr { kind: ExprKind::Duration(0), ty: Ty::Duration },
        _ => return None,
    })
}

fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
    Expr { kind: ExprKind::Binary(op, Box::new(a), Box::new(b)), ty: Ty::Bool }
}

fn is_place(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Args)
}

fn merge_parts(parts: Vec<Part>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for p in parts {
        if let (Some(Part::Lit(a)), Part::Lit(b)) = (out.last_mut(), &p) {
            a.push_str(b);
            continue;
        }
        if let Part::Lit(s) = &p {
            if s.is_empty() {
                continue;
            }
        }
        out.push(p);
    }
    out
}

impl<'a> Lowerer<'a> {
    // ---------------------------------------------------------------------------------------------
    // Names and helpers
    // ---------------------------------------------------------------------------------------------

    fn line_of(&self, offset: usize) -> u32 {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i as u32 + 1,
            Err(i) => i as u32,
        }
    }

    fn st(&self, kind: StmtKind) -> Stmt {
        Stmt { line: self.line, kind }
    }

    fn fresh(&mut self, base: &str) -> String {
        let base = {
            let b = base.trim_start_matches('_');
            if b.is_empty() {
                "v".to_string()
            } else if base.starts_with("__") {
                format!("_{}", b)
            } else {
                base.to_string()
            }
        };
        let mut cand = base.clone();
        let mut n = 2;
        while self.ns.used.contains(&cand) {
            cand = format!("{}_{}", base, n);
            n += 1;
        }
        self.ns.used.insert(cand.clone());
        cand
    }

    fn bind(&mut self, id: check::VarId) -> String {
        let n = self.fresh(&self.tp.vars[id].name.clone());
        self.names[id] = Some(n.clone());
        n
    }

    fn name(&self, id: check::VarId) -> String {
        self.names[id].clone().unwrap_or_else(|| self.tp.vars[id].name.clone())
    }

    fn temp(&mut self) -> String {
        loop {
            self.ns.temp += 1;
            let n = format!("__t{}", self.ns.temp);
            if self.ns.used.insert(n.clone()) {
                return n;
            }
        }
    }

    fn struct_fields(&self, name: &str) -> Vec<check::FieldInfo> {
        self.tp.structs.iter().find(|s| s.name == name).map(|s| s.fields.clone()).unwrap_or_default()
    }

    /// An initial `Rvalue` for a temporary that will be filled in by branches.
    fn zero(&mut self, ty: &Ty, out: &mut Vec<Stmt>) -> Rvalue {
        if let Some(e) = zero_lit(ty) {
            return Rvalue::Expr(e);
        }
        match ty {
            Ty::Enum(n) => {
                let case = self.tp.enums.iter().find(|e| &e.name == n).and_then(|e| e.cases.first().cloned()).unwrap_or_default();
                Rvalue::Expr(Expr { kind: ExprKind::EnumCase { enum_: n.clone(), case }, ty: ty.clone() })
            }
            Ty::Secret => Rvalue::Call {
                call: Call { target: CallTarget::Builtin("secret".into()), args: vec![Arg::Expr(Expr::str(""))], ret: Ty::Secret, fallible: false },
                on_fail: OnFail::Propagate,
            },
            Ty::List(t) => Rvalue::List { elem_ty: (**t).clone(), items: vec![] },
            Ty::Map(k, v) => Rvalue::Map { key_ty: (**k).clone(), val_ty: (**v).clone(), entries: vec![] },
            Ty::Struct(n) => {
                let mut fields = Vec::new();
                for f in self.struct_fields(n) {
                    let e = match zero_lit(&f.ty) {
                        Some(e) => e,
                        None => {
                            let rv = self.zero(&f.ty, out);
                            let t = self.temp();
                            out.push(self.st(StmtKind::Let { var: t.clone(), ty: f.ty.clone(), mutable: false, value: rv }));
                            Expr::var(&t, f.ty.clone())
                        }
                    };
                    fields.push((f.name.clone(), e));
                }
                Rvalue::Struct { ty: n.clone(), spread: None, fields }
            }
            _ => Rvalue::Expr(Expr::str("")),
        }
    }

    /// Declares a mutable temporary with the "zero" initial value of the type.
    fn slot(&mut self, ty: &Ty, out: &mut Vec<Stmt>) -> String {
        let rv = self.zero(ty, out);
        let t = self.temp();
        out.push(self.st(StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: true, value: rv }));
        t
    }

    fn assign_var(&self, name: &str, value: Rvalue) -> Stmt {
        self.st(StmtKind::Assign { target: LValue::Var(name.to_string()), value })
    }

    // ---------------------------------------------------------------------------------------------
    // Program
    // ---------------------------------------------------------------------------------------------

    fn program(&mut self, file: &str) -> Program {
        let tp = self.tp;
        let name = std::path::Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or("script").to_string();
        // params
        let mut params = Vec::new();
        for p in &tp.params {
            self.line = self.line_of(p.span.start);
            let pname = self.bind(p.var);
            let default = p.default.as_ref().map(|d| {
                let mut scratch = Vec::new();
                self.rvalue(d, &mut scratch)
            });
            let src_name = tp.vars[p.var].name.clone();
            params.push(Param {
                name: pname,
                flag: format!("--{}", src_name.replace('_', "-")),
                ty: p.ty.clone(),
                default,
                doc: p.doc.clone(),
                env_var: if p.ty == Ty::Secret { Some(format!("NALU_{}", src_name.to_ascii_uppercase())) } else { None },
            });
        }
        // top-level body
        let body = self.block(&tp.body);
        let top_ns = self.ns.clone();
        let globals: HashSet<String> = tp
            .vars
            .iter()
            .enumerate()
            .filter(|(_, v)| matches!(v.kind, check::VarKind::Global | check::VarKind::ScriptParam))
            .filter_map(|(i, _)| self.names[i].clone())
            .collect();
        // functions
        let mut funcs = Vec::new();
        for (i, f) in tp.fns.iter().enumerate() {
            self.ns = Ns { used: globals.clone(), temp: 0 };
            self.line = self.line_of(f.span.start);
            let tf = &tp.funcs[i];
            let mut fparams = Vec::new();
            for &pid in &tf.params {
                let n = self.bind(pid);
                fparams.push(FParam { name: n, ty: tp.vars[pid].ty.clone() });
            }
            let fbody = self.block(&tf.body);
            funcs.push(Func { name: f.name.clone(), params: fparams, ret: f.ret.clone(), body: fbody, fallible: tp.fallible[i], doc: f.doc.clone() });
        }
        // tests
        self.collect_deps = false;
        let mut tests = Vec::new();
        for t in &tp.tests {
            self.ns = Ns { used: globals.clone(), temp: 0 };
            self.line = self.line_of(t.span.start);
            let b = self.block(&t.body);
            tests.push(TestDef { name: t.name.clone(), body: b });
        }
        self.ns = top_ns;
        // types
        let structs = tp.structs.iter().map(|s| StructDef { name: s.name.clone(), fields: s.fields.iter().map(|f| Field { name: f.name.clone(), ty: f.ty.clone() }).collect() }).collect();
        let enums = tp.enums.iter().map(|e| EnumDef { name: e.name.clone(), cases: e.cases.clone() }).collect();
        // deps in order of appearance
        let mut deps_sorted = std::mem::take(&mut self.deps);
        deps_sorted.sort_by_key(|(o, _)| *o);
        let mut deps: Vec<String> = Vec::new();
        for (_, d) in deps_sorted {
            if !deps.contains(&d) {
                deps.push(d);
            }
        }
        let usage = self.usage(&name, &params);
        Program {
            name,
            source_file: file.to_string(),
            target: self.target,
            usage,
            structs,
            enums,
            params,
            funcs,
            body,
            tests,
            deps,
        }
    }

    fn usage(&self, name: &str, params: &[Param]) -> String {
        let ty_text = |p: &Param| -> String {
            match &p.ty {
                Ty::Enum(n) => self.tp.enums.iter().find(|e| &e.name == n).map(|e| e.cases.join("|")).unwrap_or_else(|| n.clone()),
                t => t.display(),
            }
        };
        let mut first = format!("usage: {}", name);
        for p in params {
            if p.default.is_none() && p.ty != Ty::Bool {
                first.push_str(&format!(" {} <{}>", p.flag, ty_text(p)));
            }
        }
        // every script accepts positionals in `args` (§18), whether it uses them or not
        first.push_str(" [options] [args…]");
        let mut rows: Vec<(String, String)> = Vec::new();
        for p in params {
            let left = if p.ty == Ty::Bool { p.flag.clone() } else { format!("{} <{}>", p.flag, ty_text(p)) };
            let suffix = match &p.default {
                Some(d) => {
                    let txt = default_text(d);
                    if p.ty == Ty::Bool && txt == "false" {
                        String::new()
                    } else {
                        format!("(default: {})", if txt.is_empty() { "\"\"".to_string() } else { txt })
                    }
                }
                None if p.ty == Ty::Bool => String::new(),
                None => match &p.env_var {
                    Some(env) => format!("(required; or set {})", env),
                    None => "(required)".into(),
                },
            };
            let right = match (p.doc.is_empty(), suffix.is_empty()) {
                (false, false) => format!("{} {}", p.doc, suffix),
                (false, true) => p.doc.clone(),
                (true, _) => suffix,
            };
            rows.push((left, right));
        }
        rows.push(("--dry-run".into(), "show what would happen, without running commands or changing files".into()));
        rows.push(("--verbose".into(), "print each command before running it".into()));
        rows.push(("-h, --help".into(), "show this help".into()));
        let width = rows.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) + 2;
        let mut out = first;
        out.push_str("\n\n");
        let lines: Vec<String> = rows
            .iter()
            .map(|(l, r)| {
                let pad = width - l.chars().count();
                format!("  {}{}{}", l, " ".repeat(pad), r).trim_end().to_string()
            })
            .collect();
        out.push_str(&lines.join("\n"));
        out
    }

    // ---------------------------------------------------------------------------------------------
    // Statements
    // ---------------------------------------------------------------------------------------------

    fn block(&mut self, stmts: &[TStmt]) -> Block {
        let line = self.line;
        let mut out = Vec::new();
        for s in stmts {
            self.stmt(s, &mut out);
        }
        self.line = line;
        Block { stmts: out }
    }

    fn stmt(&mut self, s: &TStmt, out: &mut Vec<Stmt>) {
        self.line = self.line_of(s.span.start);
        match &s.kind {
            TS::Let { var, value } => {
                let rv = self.rvalue(value, out);
                let n = self.bind(*var);
                let v = &self.tp.vars[*var];
                out.push(self.st(StmtKind::Let { var: n, ty: v.ty.clone(), mutable: v.mutable, value: rv }));
            }
            TS::Assign { target, value } => {
                let lv = match target {
                    TLValue::Var(id) => LValue::Var(self.name(*id)),
                    TLValue::Field(id, f) => LValue::Field { base: self.name(*id), field: f.clone() },
                    TLValue::Index(id, i) => {
                        let ie = self.expr(i, out);
                        let list = Expr::var(&self.name(*id), self.tp.vars[*id].ty.clone());
                        let ie = self.index_guard(&list, ie, out);
                        LValue::Index { list: self.name(*id), index: ie }
                    }
                    TLValue::MapEntry(id, k) => {
                        let ke = self.expr(k, out);
                        LValue::MapEntry { map: self.name(*id), key: ke }
                    }
                };
                let rv = self.rvalue(value, out);
                out.push(self.st(StmtKind::Assign { target: lv, value: rv }));
            }
            TS::Expr(e) => self.stmt_expr(e, out),
            TS::ForRange { var, from, to, inclusive, body } => {
                let f = self.expr(from, out);
                let t = self.expr(to, out);
                let n = self.bind(*var);
                let b = self.block(body);
                self.line = self.line_of(s.span.start);
                out.push(self.st(StmtKind::ForRange { var: n, from: f, to: t, inclusive: *inclusive, body: b }));
            }
            TS::ForList { var, index, list, body } => {
                let l = self.place_expr(list, out);
                let idx = index.map(|i| self.bind(i));
                let n = self.bind(*var);
                let b = self.block(body);
                self.line = self.line_of(s.span.start);
                out.push(self.st(StmtKind::ForList { var: n, index: idx, list: l, body: b }));
            }
            TS::ForMap { key, value, map, body } => {
                let m = self.place_expr(map, out);
                let k = self.bind(*key);
                let v = self.bind(*value);
                let b = self.block(body);
                self.line = self.line_of(s.span.start);
                out.push(self.st(StmtKind::ForMap { key: k, value: v, map: m, body: b }));
            }
            TS::While { cond, body } => {
                let mut pre = Vec::new();
                let c = self.expr(cond, &mut pre);
                let mut b = self.block(body);
                self.line = self.line_of(s.span.start);
                if pre.is_empty() {
                    out.push(self.st(StmtKind::While { cond: c, body: b }));
                } else {
                    let neg = negate(c);
                    pre.push(self.st(StmtKind::If { cond: neg, then: Block { stmts: vec![self.st(StmtKind::Break)] }, els: None }));
                    pre.append(&mut b.stmts);
                    out.push(self.st(StmtKind::While { cond: Expr::bool(true), body: Block { stmts: pre } }));
                }
            }
            TS::Fail { message, code } => {
                let m = self.expr(message, out);
                let c = code.as_ref().map(|c| self.expr(c, out));
                out.push(self.st(StmtKind::Fail { message: m, code: c }));
            }
            TS::Return(v) => {
                let e = v.as_ref().map(|v| self.place_expr(v, out));
                out.push(self.st(StmtKind::Return(e)));
            }
            TS::Break => out.push(self.st(StmtKind::Break)),
            TS::Continue => out.push(self.st(StmtKind::Continue)),
            TS::Exit(c) => {
                let e = c.as_ref().map(|c| self.expr(c, out));
                out.push(self.st(StmtKind::Exit(e)));
            }
            TS::Defer(e) => {
                let inner = match &e.kind {
                    TK::Call(c) => {
                        let call = self.call(c, out);
                        self.st(StmtKind::Call { call, on_fail: OnFail::Ignore })
                    }
                    TK::Command(c) => {
                        let cmd = self.cmd(c, out);
                        self.st(StmtKind::Command { cmd, on_fail: OnFail::Ignore })
                    }
                    _ => return,
                };
                out.push(self.st(StmtKind::Defer(Box::new(inner))));
            }
            TS::Native { bash, powershell } => out.push(self.st(StmtKind::Native { bash: bash.clone(), powershell: powershell.clone() })),
            TS::Assert { cond, text } => {
                let c = self.expr(cond, out);
                out.push(self.st(StmtKind::Assert { cond: c, text: text.clone() }));
            }
            TS::AssertFails(e) => {
                let inner = match &e.kind {
                    TK::Call(c) => {
                        let call = self.call(c, out);
                        self.st(StmtKind::Call { call, on_fail: OnFail::Propagate })
                    }
                    TK::Command(c) => {
                        let cmd = self.cmd(c, out);
                        self.st(StmtKind::Command { cmd, on_fail: OnFail::Propagate })
                    }
                    _ => return,
                };
                out.push(self.st(StmtKind::AssertFails(Box::new(inner))));
            }
        }
    }

    /// Expression as a statement (result discarded).
    fn stmt_expr(&mut self, e: &TExpr, out: &mut Vec<Stmt>) {
        match &e.kind {
            TK::Call(c) => {
                let call = self.call(c, out);
                out.push(self.st(StmtKind::Call { call, on_fail: OnFail::Propagate }));
            }
            TK::Command(c) => {
                let cmd = self.cmd(c, out);
                out.push(self.st(StmtKind::Command { cmd, on_fail: OnFail::Propagate }));
            }
            TK::Capture(_) | TK::CmdTest(_) => {
                let rv = self.rvalue(e, out);
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t, ty: e.ty.clone(), mutable: false, value: rv }));
            }
            TK::If { cond, then, els } => {
                if let Some(b) = self.fold_bool(cond) {
                    let chosen = if b { Some(then) } else { els.as_ref() };
                    if let Some(blk) = chosen {
                        for s in &blk.stmts {
                            self.stmt(s, out);
                        }
                        if let Some(v) = &blk.value {
                            self.stmt_expr(v, out);
                        }
                    }
                    return;
                }
                let c = self.expr(cond, out);
                let line = self.line;
                let tb = self.guarded_by(cond, true, |l| l.tblock_stmts(then));
                let eb = els.as_ref().map(|b| self.guarded_by(cond, false, |l| l.tblock_stmts(b)));
                self.line = line;
                out.push(self.st(StmtKind::If { cond: c, then: tb, els: eb }));
            }
            TK::Match(m) => {
                if m.is_os {
                    if let Some(b) = self.os_arm(m) {
                        for s in &b.stmts {
                            self.stmt(s, out);
                        }
                        if let Some(v) = &b.value {
                            self.stmt_expr(v, out);
                        }
                    }
                    return;
                }
                if m.ordered {
                    self.match_chain(m, None, out);
                    return;
                }
                let subj = self.expr(&m.subject, out);
                let line = self.line;
                let arms = m.arms.iter().map(|a| Arm { patterns: a.patterns.iter().map(|p| self.pattern(p)).collect(), body: self.tblock_stmts(&a.body) }).collect();
                let default = m.default.as_ref().map(|d| self.tblock_stmts(d));
                self.line = line;
                out.push(self.st(StmtKind::Match { subject: subj, arms, default }));
            }
            TK::Or(lhs, h) => self.stmt_or(lhs, h, out),
            TK::Try(b) => {
                let body = self.tblock_stmts(b);
                out.push(self.st(StmtKind::Try { body, on_fail: OnFail::Propagate }));
            }
            TK::Retry { .. } => self.retry(e, OnFail::Propagate, out),
            TK::Parallel { .. } => self.parallel(e, None, OnFail::Propagate, out),
            _ => {
                // pure expression: evaluate only for the effects of its parts (should not happen: E030)
                let _ = self.expr(e, out);
            }
        }
    }

    /// Typed block as statements (value, if any, evaluated and discarded).
    fn tblock_stmts(&mut self, b: &TBlock) -> Block {
        let line = self.line;
        let mut out = Vec::new();
        for s in &b.stmts {
            self.stmt(s, &mut out);
        }
        if let Some(v) = &b.value {
            self.line = self.line_of(v.span.start);
            self.stmt_expr(v, &mut out);
        }
        self.line = line;
        Block { stmts: out }
    }

    /// Typed block whose value goes into `slot`.
    fn tblock_into(&mut self, b: &TBlock, slot: &str) -> Block {
        let line = self.line;
        let mut out = Vec::new();
        for s in &b.stmts {
            self.stmt(s, &mut out);
        }
        if let Some(v) = &b.value {
            self.line = self.line_of(v.span.start);
            let rv = self.rvalue(v, &mut out);
            out.push(self.assign_var(slot, rv));
        }
        self.line = line;
        Block { stmts: out }
    }

    fn pattern(&mut self, p: &TPattern) -> Pattern {
        match p {
            TPattern::Lit(e) => {
                let mut scratch = Vec::new();
                Pattern::Lit(self.expr(e, &mut scratch))
            }
            TPattern::Range { from, to, inclusive } => Pattern::Range { from: Expr::int(*from), to: Expr::int(*to), inclusive: *inclusive },
            // only in ordered matches, which `match_chain` lowers
            TPattern::Any | TPattern::Bind(_) | TPattern::Struct(_) => Pattern::Lit(Expr::bool(true)),
        }
    }

    /// A `match` with guards, bindings or struct patterns (D33): the arms are tested in order, as `if`s.
    /// Without guards it is an `if`/`else` chain; with them, a flag says an arm already ran (a failed guard
    /// goes on to the next arm).
    fn match_chain(&mut self, m: &check::TMatch, slot: Option<&str>, out: &mut Vec<Stmt>) {
        let x = self.place_expr(&m.subject, out);
        let x = if is_place(&x) {
            x
        } else {
            let t = self.temp();
            out.push(self.st(StmtKind::Let { var: t.clone(), ty: x.ty.clone(), mutable: false, value: Rvalue::Expr(x.clone()) }));
            Expr::var(&t, x.ty.clone())
        };
        let line = self.line;
        if !m.arms.iter().any(|a| a.guard.is_some()) {
            let mut chain: Option<Vec<Stmt>> = None;
            for a in m.arms.iter().rev() {
                let test = self.arm_test(&a.patterns, &x);
                let mut body = self.arm_binds(&a.patterns, &x);
                body.extend(self.arm_body(&a.body, slot).stmts);
                self.line = line;
                chain = Some(match test {
                    None => body,
                    Some(c) => vec![self.st(StmtKind::If { cond: c, then: Block { stmts: body }, els: chain.map(|stmts| Block { stmts }) })],
                });
            }
            out.extend(chain.unwrap_or_default());
            return;
        }
        let done = self.temp();
        out.push(self.st(StmtKind::Let { var: done.clone(), ty: Ty::Bool, mutable: true, value: Rvalue::Expr(Expr::bool(false)) }));
        for (i, a) in m.arms.iter().enumerate() {
            let test = self.arm_test(&a.patterns, &x);
            let mut inner = self.arm_binds(&a.patterns, &x);
            let mut then = vec![self.assign_var(&done, Rvalue::Expr(Expr::bool(true)))];
            then.extend(self.arm_body(&a.body, slot).stmts);
            self.line = line;
            match &a.guard {
                Some(g) => {
                    let c = self.expr(g, &mut inner);
                    inner.push(self.st(StmtKind::If { cond: c, then: Block { stmts: then }, els: None }));
                }
                None => inner.extend(then),
            }
            let not_done = Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(Expr::var(&done, Ty::Bool))), ty: Ty::Bool };
            let cond = match (i, test) {
                (0, Some(t)) => t,
                (0, None) => Expr::bool(true),
                (_, Some(t)) => bin(BinOp::And, not_done, t),
                (_, None) => not_done,
            };
            if matches!(cond.kind, ExprKind::Bool(true)) {
                out.extend(inner);
            } else {
                out.push(self.st(StmtKind::If { cond, then: Block { stmts: inner }, els: None }));
            }
        }
        self.line = line;
    }

    fn arm_body(&mut self, b: &TBlock, slot: Option<&str>) -> Block {
        match slot {
            Some(s) => self.tblock_into(b, s),
            None => self.tblock_stmts(b),
        }
    }

    /// The test of an arm (`None`: every value matches).
    fn arm_test(&mut self, pats: &[TPattern], x: &Expr) -> Option<Expr> {
        let mut any: Option<Expr> = None;
        for p in pats {
            let t = self.pat_test(p, x)?;
            any = Some(match any {
                None => t,
                Some(a) => bin(BinOp::Or, a, t),
            });
        }
        any
    }

    fn pat_test(&mut self, p: &TPattern, x: &Expr) -> Option<Expr> {
        match p {
            TPattern::Any | TPattern::Bind(_) => None,
            TPattern::Lit(e) => {
                let mut scratch = Vec::new();
                let mut l = self.expr(e, &mut scratch);
                l.ty = x.ty.clone(); // a `str` literal against a `path`
                Some(bin(BinOp::Eq, x.clone(), l))
            }
            TPattern::Range { from, to, inclusive } => {
                let lo = bin(BinOp::Ge, x.clone(), Expr::int(*from));
                let hi = bin(if *inclusive { BinOp::Le } else { BinOp::Lt }, x.clone(), Expr::int(*to));
                Some(bin(BinOp::And, lo, hi))
            }
            TPattern::Struct(fs) => {
                let sname = match &x.ty {
                    Ty::Struct(n) => n.clone(),
                    _ => String::new(),
                };
                let fields = self.struct_fields(&sname);
                let mut all: Option<Expr> = None;
                for (f, fp) in fs {
                    let fty = fields.iter().find(|fi| fi.name == *f).map(|fi| fi.ty.clone()).unwrap_or(Ty::Str);
                    let fx = Expr { kind: ExprKind::Field(Box::new(x.clone()), f.clone()), ty: fty };
                    if let Some(t) = self.pat_test(fp, &fx) {
                        all = Some(match all {
                            None => t,
                            Some(a) => bin(BinOp::And, a, t),
                        });
                    }
                }
                all
            }
        }
    }

    /// `let` for the names an arm binds: the value itself, or struct fields.
    fn arm_binds(&mut self, pats: &[TPattern], x: &Expr) -> Vec<Stmt> {
        let mut out = Vec::new();
        for p in pats {
            match p {
                TPattern::Bind(v) => {
                    let n = self.bind(*v);
                    out.push(self.st(StmtKind::Let { var: n, ty: x.ty.clone(), mutable: false, value: Rvalue::Expr(x.clone()) }));
                }
                TPattern::Struct(fs) => {
                    let sname = match &x.ty {
                        Ty::Struct(n) => n.clone(),
                        _ => String::new(),
                    };
                    let fields = self.struct_fields(&sname);
                    for (f, fp) in fs {
                        if let TPattern::Bind(v) = fp {
                            let fty = fields.iter().find(|fi| fi.name == *f).map(|fi| fi.ty.clone()).unwrap_or(Ty::Str);
                            let n = self.bind(*v);
                            let fx = Expr { kind: ExprKind::Field(Box::new(x.clone()), f.clone()), ty: fty.clone() };
                            out.push(self.st(StmtKind::Let { var: n, ty: fty, mutable: false, value: Rvalue::Expr(fx) }));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The `match os` arm that applies to this target.
    fn os_arm<'b>(&self, m: &'b check::TMatch) -> Option<&'b TBlock> {
        let os = self.target.os_name();
        for a in &m.arms {
            if a.patterns.iter().any(|p| matches!(p, TPattern::Lit(TExpr { kind: TK::Str(s), .. }) if s == os)) {
                return Some(&a.body);
            }
        }
        m.default.as_ref()
    }

    /// Evaluates at compile time conditions that depend only on `os` (and `true`/`false`).
    fn fold_bool(&self, e: &TExpr) -> Option<bool> {
        match &e.kind {
            TK::Bool(b) => Some(*b),
            TK::Binary(op @ (BinOp::Eq | BinOp::Ne), l, r) => {
                let s = match (&l.kind, &r.kind) {
                    (TK::Os, TK::Str(s)) | (TK::Str(s), TK::Os) => s,
                    _ => return None,
                };
                let eq = s == self.target.os_name();
                Some(if *op == BinOp::Eq { eq } else { !eq })
            }
            TK::Unary(UnOp::Not, x) => self.fold_bool(x).map(|b| !b),
            TK::Binary(BinOp::And, l, r) => match (self.fold_bool(l), self.fold_bool(r)) {
                (Some(false), _) => Some(false),
                (Some(true), Some(b)) => Some(b),
                _ => None,
            },
            TK::Binary(BinOp::Or, l, r) => match (self.fold_bool(l), self.fold_bool(r)) {
                (Some(true), _) => Some(true),
                (Some(false), Some(b)) => Some(b),
                _ => None,
            },
            _ => None,
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Pure expressions
    // ---------------------------------------------------------------------------------------------

    /// A pure `Expr`; whatever is not pure becomes statements in `out` (temporaries).
    fn expr(&mut self, e: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        let ty = e.ty.clone();
        let mk = |kind: ExprKind, ty: Ty| Expr { kind, ty };
        match &e.kind {
            TK::Int(n) => Expr::int(*n),
            TK::Str(s) => mk(ExprKind::Str(s.clone()), ty),
            TK::Bool(b) => Expr::bool(*b),
            TK::Duration(d) => mk(ExprKind::Duration(*d), Ty::Duration),
            TK::EnumCase(en, c) => mk(ExprKind::EnumCase { enum_: en.clone(), case: c.clone() }, ty),
            TK::Interp(parts) => {
                let ps = self.parts(parts, out, false);
                match ps.as_slice() {
                    [] => mk(ExprKind::Str(String::new()), ty),
                    [Part::Lit(s)] => mk(ExprKind::Str(s.clone()), ty),
                    _ => mk(ExprKind::Interp(ps), ty),
                }
            }
            TK::Var(id) => mk(ExprKind::Var(self.name(*id)), ty),
            TK::DryRun => mk(ExprKind::DryRun, Ty::Bool),
            TK::Args => mk(ExprKind::Args, ty),
            TK::Os => Expr::str(self.target.os_name()),
            TK::CurrentErr => match self.err_mode.last().cloned().unwrap_or(ErrMode::Current) {
                ErrMode::Current => mk(ExprKind::CurrentErr, Ty::Error),
                ErrMode::Var(n) => mk(ExprKind::Var(n), Ty::Error),
            },
            TK::Field(b, f) => {
                let be = self.place_base(b, out);
                mk(ExprKind::Field(Box::new(be), f.clone()), ty)
            }
            TK::Index(l, i) => {
                let le = self.expr(l, out);
                let ie = self.expr(i, out);
                let ie = self.index_guard(&le, ie, out);
                let idx = mk(ExprKind::Index(Box::new(le), Box::new(ie)), ty.clone());
                if let Ty::Struct(sn) = &ty {
                    // a list item of structs as a value: copy field by field (rule 6)
                    let fields = self.struct_fields(sn).into_iter().map(|f| (f.name.clone(), Expr { kind: ExprKind::Field(Box::new(idx.clone()), f.name.clone()), ty: f.ty })).collect();
                    let t = self.temp();
                    out.push(self.st(StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: false, value: Rvalue::Struct { ty: sn.clone(), spread: None, fields } }));
                    return Expr::var(&t, ty);
                }
                idx
            }
            TK::Len(x) => {
                let xe = self.expr(x, out);
                mk(ExprKind::Len(Box::new(xe)), Ty::Int)
            }
            TK::Unary(op, x) => {
                if *op == UnOp::Not {
                    if let Some(b) = self.fold_bool(e) {
                        return Expr::bool(b);
                    }
                }
                let xe = self.expr(x, out);
                mk(ExprKind::Unary(*op, Box::new(xe)), ty)
            }
            TK::Binary(op @ (BinOp::And | BinOp::Or), l, r) => {
                if let Some(b) = self.fold_bool(e) {
                    return Expr::bool(b);
                }
                let is_and = *op == BinOp::And;
                match self.fold_bool(l) {
                    Some(true) if is_and => return self.expr(r, out),
                    Some(false) if !is_and => return self.expr(r, out),
                    _ => {}
                }
                let le = self.expr(l, out);
                let mut pre_r = Vec::new();
                let re = self.expr(r, &mut pre_r);
                if pre_r.is_empty() {
                    return mk(ExprKind::Binary(*op, Box::new(le), Box::new(re)), Ty::Bool);
                }
                // short-circuit: the right side only runs if needed
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t.clone(), ty: Ty::Bool, mutable: true, value: Rvalue::Expr(le) }));
                let tv = Expr::var(&t, Ty::Bool);
                let cond = if is_and { tv.clone() } else { mk(ExprKind::Unary(UnOp::Not, Box::new(tv.clone())), Ty::Bool) };
                pre_r.push(self.assign_var(&t, Rvalue::Expr(re)));
                out.push(self.st(StmtKind::If { cond, then: Block { stmts: pre_r }, els: None }));
                tv
            }
            TK::Binary(op @ (BinOp::Eq | BinOp::Ne), _, _) if self.fold_bool(e).is_some() => {
                let _ = op;
                Expr::bool(self.fold_bool(e).unwrap_or(false))
            }
            TK::Binary(BinOp::Concat, l, r) => {
                let le = self.text_side(l, out);
                let re = self.text_side(r, out);
                mk(ExprKind::Binary(BinOp::Concat, Box::new(le), Box::new(re)), Ty::Str)
            }
            TK::Binary(op, l, r) => {
                let le = self.expr(l, out);
                let mut re = self.expr(r, out);
                if check::may_divide_by_zero(*op, r) {
                    re = self.zero_guard(re, out);
                }
                mk(ExprKind::Binary(*op, Box::new(le), Box::new(re)), ty)
            }
            TK::If { .. } | TK::Match(_) => self.value_branch(e, out),
            TK::Parallel { .. } => {
                let slot = self.slot(&ty, out);
                self.parallel(e, Some(slot.clone()), OnFail::Propagate, out);
                Expr::var(&slot, ty)
            }
            _ => {
                let rv = self.rvalue(e, out);
                if let Rvalue::Expr(x) = rv {
                    return x;
                }
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: false, value: rv }));
                Expr::var(&t, ty)
            }
        }
    }

    /// `xs[i]` out of range fails: before the access, `if i < 0 || i >= xs.len() { fail "…" }`.
    /// The `Fail` follows the context like any failure (function, `try`, `catch`, `retry`, `parallel`, top level),
    /// so the backends do not change and `Index` stays a pure `Expr` (rule 1). Returns the index to use:
    /// a temporary when the expression is neither a literal nor a variable (so it is not computed three times).
    fn index_guard(&mut self, list: &Expr, index: Expr, out: &mut Vec<Stmt>) -> Expr {
        let index = match &index.kind {
            ExprKind::Int(_) | ExprKind::Var(_) => index,
            _ => {
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t.clone(), ty: Ty::Int, mutable: false, value: Rvalue::Expr(index) }));
                Expr::var(&t, Ty::Int)
            }
        };
        let bin = |op: BinOp, l: Expr, r: Expr| Expr { kind: ExprKind::Binary(op, Box::new(l), Box::new(r)), ty: Ty::Bool };
        let len = Expr { kind: ExprKind::Len(Box::new(list.clone())), ty: Ty::Int };
        let past_end = bin(BinOp::Ge, index.clone(), len.clone());
        let cond = match index.kind {
            ExprKind::Int(n) if n >= 0 => past_end,
            _ => bin(BinOp::Or, bin(BinOp::Lt, index.clone(), Expr::int(0)), past_end),
        };
        let message = Expr {
            kind: ExprKind::Interp(vec![Part::Lit("index ".into()), Part::Expr(index.clone()), Part::Lit(" is out of range (length ".into()), Part::Expr(len), Part::Lit(")".into())]),
            ty: Ty::Str,
        };
        let fail = self.st(StmtKind::Fail { message, code: None });
        out.push(self.st(StmtKind::If { cond, then: Block { stmts: vec![fail] }, els: None }));
        index
    }

    /// Dividing by zero fails on all three targets, with the same message (Bash would carry on with a warning; PowerShell
    /// would throw its own exception): `if b == 0 { fail "division by zero" }` before the operation. Returns the
    /// divisor to use (a temporary when it is neither a literal nor a variable).
    fn zero_guard(&mut self, divisor: Expr, out: &mut Vec<Stmt>) -> Expr {
        let ty = divisor.ty.clone();
        let divisor = match &divisor.kind {
            ExprKind::Var(_) => divisor,
            _ => {
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: false, value: Rvalue::Expr(divisor) }));
                Expr::var(&t, ty.clone())
            }
        };
        let zero = Expr { kind: if ty == Ty::Duration { ExprKind::Duration(0) } else { ExprKind::Int(0) }, ty: ty.clone() };
        let cond = Expr { kind: ExprKind::Binary(BinOp::Eq, Box::new(divisor.clone()), Box::new(zero)), ty: Ty::Bool };
        let fail = self.st(StmtKind::Fail { message: Expr { kind: ExprKind::Str("division by zero".into()), ty: Ty::Str }, code: None });
        out.push(self.st(StmtKind::If { cond, then: Block { stmts: vec![fail] }, els: None }));
        divisor
    }

    /// Base of `Field`: accepts `Index` without copying the whole item.
    fn place_base(&mut self, b: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        match &b.kind {
            TK::Index(l, i) => {
                let le = self.expr(l, out);
                let ie = self.expr(i, out);
                let ie = self.index_guard(&le, ie, out);
                Expr { kind: ExprKind::Index(Box::new(le), Box::new(ie)), ty: b.ty.clone() }
            }
            TK::Field(bb, f) => {
                let be = self.place_base(bb, out);
                Expr { kind: ExprKind::Field(Box::new(be), f.clone()), ty: b.ty.clone() }
            }
            _ => self.expr(b, out),
        }
    }

    /// An `Expr`; if it is a collection/struct, guarantees `Var`/`Field` (rule 6).
    fn place_expr(&mut self, e: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        let x = self.expr(e, out);
        if x.ty.is_scalar() || is_place(&x) {
            return x;
        }
        let t = self.temp();
        let ty = x.ty.clone();
        out.push(self.st(StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: false, value: Rvalue::Expr(x) }));
        Expr::var(&t, ty)
    }

    /// Side of a concatenation, as text.
    fn text_side(&mut self, e: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        if e.ty == Ty::Str {
            return self.expr(e, out);
        }
        let ps = self.parts(&[TPart::Expr(e.clone())], out, false);
        match ps.as_slice() {
            [Part::Lit(s)] => Expr::str(s),
            _ => Expr { kind: ExprKind::Interp(ps), ty: Ty::Str },
        }
    }

    /// Interpolation parts: duration → `duration_str`; error → `.message`; literals merged.
    fn parts(&mut self, parts: &[TPart], out: &mut Vec<Stmt>, allow_secret: bool) -> Vec<Part> {
        let mut ps = Vec::new();
        for p in parts {
            match p {
                TPart::Lit(s) => ps.push(Part::Lit(s.clone())),
                TPart::Expr(x) => ps.push(self.text_part(x, out, allow_secret)),
            }
        }
        merge_parts(ps)
    }

    fn text_part(&mut self, x: &TExpr, out: &mut Vec<Stmt>, _allow_secret: bool) -> Part {
        match &x.ty {
            Ty::Duration => {
                if let TK::Duration(d) = x.kind {
                    return Part::Lit(format_duration(d));
                }
                let de = self.expr(x, out);
                let t = self.temp();
                let call = Call { target: CallTarget::Builtin("duration_str".into()), args: vec![Arg::Expr(de)], ret: Ty::Str, fallible: false };
                out.push(self.st(StmtKind::Let { var: t.clone(), ty: Ty::Str, mutable: false, value: Rvalue::Call { call, on_fail: OnFail::Propagate } }));
                Part::Expr(Expr::var(&t, Ty::Str))
            }
            Ty::Error => {
                let ee = self.expr(x, out);
                Part::Expr(Expr { kind: ExprKind::Field(Box::new(ee), "message".into()), ty: Ty::Str })
            }
            _ => {
                let xe = self.expr(x, out);
                match &xe.kind {
                    ExprKind::Str(s) => Part::Lit(s.clone()),
                    ExprKind::Int(n) => Part::Lit(n.to_string()),
                    ExprKind::Bool(b) => Part::Lit(b.to_string()),
                    ExprKind::EnumCase { case, .. } => Part::Lit(case.clone()),
                    _ => Part::Expr(xe),
                }
            }
        }
    }

    /// Argument "any scalar becomes text" (`print`, `log`, `fmt`): duration and error converted.
    fn any_arg(&mut self, x: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        match &x.ty {
            Ty::Duration | Ty::Error => match self.text_part(x, out, false) {
                Part::Lit(s) => Expr::str(&s),
                Part::Expr(e) => e,
            },
            _ => self.place_expr(x, out),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Values (Rvalue)
    // ---------------------------------------------------------------------------------------------

    fn rvalue(&mut self, e: &TExpr, out: &mut Vec<Stmt>) -> Rvalue {
        match &e.kind {
            TK::Call(c) => {
                let call = self.call(c, out);
                Rvalue::Call { call, on_fail: OnFail::Propagate }
            }
            TK::Capture(c) => {
                let cmd = self.cmd(c, out);
                Rvalue::Capture { cmd, on_fail: OnFail::Propagate }
            }
            TK::CmdTest(c) => {
                let cmd = self.cmd(c, out);
                Rvalue::CommandOk(cmd)
            }
            TK::List(items) => {
                let elem_ty = match &e.ty {
                    Ty::List(t) => (**t).clone(),
                    t => t.clone(),
                };
                let items = items.iter().map(|x| self.place_expr(x, out)).collect();
                Rvalue::List { elem_ty, items }
            }
            TK::Map(entries) => {
                let (key_ty, val_ty) = match &e.ty {
                    Ty::Map(k, v) => ((**k).clone(), (**v).clone()),
                    _ => (Ty::Str, Ty::Str),
                };
                let entries = entries
                    .iter()
                    .map(|(k, v)| {
                        let ke = self.expr(k, out);
                        let ve = self.expr(v, out);
                        (ke, ve)
                    })
                    .collect();
                Rvalue::Map { key_ty, val_ty, entries }
            }
            TK::Struct { name, spread, fields } => {
                let sp = spread.as_ref().map(|s| {
                    let x = self.expr(s, out);
                    if matches!(x.kind, ExprKind::Var(_)) {
                        x
                    } else {
                        let t = self.temp();
                        out.push(self.st(StmtKind::Let { var: t.clone(), ty: x.ty.clone(), mutable: false, value: Rvalue::Expr(x.clone()) }));
                        Expr::var(&t, x.ty)
                    }
                });
                // evaluate the written fields in source order
                let mut given: Vec<(String, Expr)> = Vec::new();
                for (f, v) in fields {
                    let ve = self.place_expr(v, out);
                    given.push((f.clone(), ve));
                }
                let mut out_fields = Vec::new();
                for fi in self.struct_fields(name) {
                    if let Some(pos) = given.iter().position(|(n, _)| *n == fi.name) {
                        out_fields.push(given.remove(pos));
                    } else if sp.is_none() {
                        let d = match &fi.default {
                            Some(d) => self.place_expr(d, out),
                            None => match zero_lit(&fi.ty) {
                                Some(z) => z,
                                None => {
                                    let rv = self.zero(&fi.ty, out);
                                    let t = self.temp();
                                    out.push(self.st(StmtKind::Let { var: t.clone(), ty: fi.ty.clone(), mutable: false, value: rv }));
                                    Expr::var(&t, fi.ty.clone())
                                }
                            },
                        };
                        out_fields.push((fi.name.clone(), d));
                    }
                }
                Rvalue::Struct { ty: name.clone(), spread: sp, fields: out_fields }
            }
            TK::Or(lhs, h) => self.or_value(lhs, h, &e.ty, out),
            _ => Rvalue::Expr(self.expr(e, out)),
        }
    }

    /// `if`/`match` used as a value: `Let __t = zero` + branches that assign.
    fn value_branch(&mut self, e: &TExpr, out: &mut Vec<Stmt>) -> Expr {
        let ty = e.ty.clone();
        match &e.kind {
            TK::If { cond, then, els } => {
                if let Some(b) = self.fold_bool(cond) {
                    let blk = if b { Some(then) } else { els.as_ref() };
                    if let Some(blk) = blk {
                        return self.inline_value(blk, &ty, out);
                    }
                }
                let slot = self.slot(&ty, out);
                let c = self.expr(cond, out);
                let line = self.line;
                let tb = self.guarded_by(cond, true, |l| l.tblock_into(then, &slot));
                let eb = els.as_ref().map(|b| self.guarded_by(cond, false, |l| l.tblock_into(b, &slot)));
                self.line = line;
                out.push(self.st(StmtKind::If { cond: c, then: tb, els: eb }));
                Expr::var(&slot, ty)
            }
            TK::Match(m) => {
                if m.is_os {
                    if let Some(b) = self.os_arm(m) {
                        return self.inline_value(b, &ty, out);
                    }
                }
                if m.ordered {
                    let slot = self.slot(&ty, out);
                    self.match_chain(m, Some(&slot), out);
                    return Expr::var(&slot, ty);
                }
                let subj = self.expr(&m.subject, out);
                let slot = self.slot(&ty, out);
                let line = self.line;
                let arms = m.arms.iter().map(|a| Arm { patterns: a.patterns.iter().map(|p| self.pattern(p)).collect(), body: self.tblock_into(&a.body, &slot) }).collect();
                let default = m.default.as_ref().map(|d| self.tblock_into(d, &slot));
                self.line = line;
                out.push(self.st(StmtKind::Match { subject: subj, arms, default }));
                Expr::var(&slot, ty)
            }
            _ => self.expr(e, out),
        }
    }

    /// The branch left over from an `if os`/`match os` as a value: its statements go straight into `out`.
    fn inline_value(&mut self, b: &TBlock, ty: &Ty, out: &mut Vec<Stmt>) -> Expr {
        for s in &b.stmts {
            self.stmt(s, out);
        }
        match &b.value {
            Some(v) => self.place_expr(v, out),
            None => {
                // the branch diverges (fail/return/exit): any value of the type
                let slot = self.slot(ty, out);
                Expr::var(&slot, ty.clone())
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Calls
    // ---------------------------------------------------------------------------------------------

    fn call(&mut self, c: &TCall, out: &mut Vec<Stmt>) -> Call {
        let any_from = match &c.target {
            TTarget::Builtin("print") => Some(0),
            TTarget::Builtin("fmt") => Some(1),
            TTarget::Std("log", _) => Some(0),
            _ => None,
        };
        let mut args = Vec::new();
        for (i, a) in c.args.iter().enumerate() {
            match a {
                TArg::Expr(x) => {
                    let e = if any_from.map(|k| i >= k).unwrap_or(false) { self.any_arg(x, out) } else { self.place_expr(x, out) };
                    args.push(Arg::Expr(e));
                }
                TArg::Lambda(l) => args.push(Arg::Lambda(self.lambda(l))),
            }
        }
        let target = match &c.target {
            TTarget::User(i) => CallTarget::User(self.tp.fns[*i].name.clone()),
            TTarget::Std(m, n) => CallTarget::Std { module: m.to_string(), name: n.to_string() },
            TTarget::Builtin(n) => CallTarget::Builtin(n.to_string()),
            TTarget::Method { recv_ty, name } => CallTarget::Method { recv_ty: recv_ty.clone(), name: name.clone() },
        };
        Call { target, args, ret: c.ret.clone(), fallible: check::call_fallible(c, &self.tp.fallible) }
    }

    fn lambda(&mut self, l: &TLambda) -> Lambda {
        let params = l.params.iter().map(|&p| FParam { name: self.bind(p), ty: self.tp.vars[p].ty.clone() }).collect();
        let line = self.line;
        let body = match &l.body {
            TLambdaBody::Expr(e) => {
                let mut stmts = Vec::new();
                let v = self.place_expr(e, &mut stmts);
                stmts.push(self.st(StmtKind::Return(Some(v))));
                Block { stmts }
            }
            TLambdaBody::Block(s) => self.block(s),
        };
        self.line = line;
        Lambda { params, ret: l.ret.clone(), body }
    }

    // ---------------------------------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------------------------------

    fn cmd(&mut self, c: &TCmd, out: &mut Vec<Stmt>) -> Command {
        let mut pipeline = Vec::new();
        for (si, st) in c.stages.iter().enumerate() {
            let mut words = Vec::new();
            for w in st {
                words.push(match w {
                    TWord::Lit(s) => Word::Lit(s.clone()),
                    TWord::Interp(parts) => {
                        let mut ps = self.parts(parts, out, true);
                        // a `path` in a command reaches the backend as a variable: a leading `~` becomes the user's home directory
                        // when the argument is passed (on all three targets, as `fs` already does)
                        for p in ps.iter_mut() {
                            if let Part::Expr(e) = p {
                                if e.ty == Ty::Path {
                                    let t = self.temp();
                                    let v = std::mem::replace(e, Expr::var(&t, Ty::Path));
                                    let call = Call {
                                        target: CallTarget::Std { module: "path".into(), name: "__home".into() },
                                        args: vec![Arg::Expr(v)],
                                        ret: Ty::Path,
                                        fallible: false,
                                    };
                                    out.push(self.st(StmtKind::Let { var: t, ty: Ty::Path, mutable: false, value: Rvalue::Call { call, on_fail: OnFail::Propagate } }));
                                }
                            }
                        }
                        match ps.as_slice() {
                            [] => Word::Lit(String::new()),
                            [Part::Lit(s)] => Word::Lit(s.clone()),
                            _ => Word::Interp(ps),
                        }
                    }
                    TWord::Spread(e) if e.ty == Ty::List(Box::new(Ty::Path)) => {
                        // a list of paths: each one goes to the program like a single `{p}` (`~` expanded)
                        let base = self.place_expr(e, out);
                        let x = self.temp();
                        let h = self.temp();
                        let home = Call {
                            target: CallTarget::Std { module: "path".into(), name: "__home".into() },
                            args: vec![Arg::Expr(Expr::var(&x, Ty::Path))],
                            ret: Ty::Path,
                            fallible: false,
                        };
                        let body = Block {
                            stmts: vec![
                                self.st(StmtKind::Let { var: h.clone(), ty: Ty::Path, mutable: false, value: Rvalue::Call { call: home, on_fail: OnFail::Propagate } }),
                                self.st(StmtKind::Return(Some(Expr::var(&h, Ty::Path)))),
                            ],
                        };
                        let lam = Lambda { params: vec![FParam { name: x, ty: Ty::Path }], ret: Ty::Path, body };
                        let map = Call {
                            target: CallTarget::Method { recv_ty: e.ty.clone(), name: "map".into() },
                            args: vec![Arg::Expr(base), Arg::Lambda(lam)],
                            ret: e.ty.clone(),
                            fallible: false,
                        };
                        let t = self.temp();
                        out.push(self.st(StmtKind::Let { var: t.clone(), ty: e.ty.clone(), mutable: false, value: Rvalue::Call { call: map, on_fail: OnFail::Propagate } }));
                        Word::Spread(Expr::var(&t, e.ty.clone()))
                    }
                    TWord::Spread(e) => Word::Spread(self.place_expr(e, out)),
                });
            }
            if let Some(Word::Lit(prog)) = words.first() {
                let sp = c.prog_spans.get(si).copied().unwrap_or(c.span);
                self.program_used(prog, sp);
            }
            if !words.is_empty() {
                pipeline.push(SimpleCmd { words });
            }
        }
        let mut redirects = Vec::new();
        for r in &c.redirects {
            redirects.push(match r {
                TRedir::Out { path, append } => Redirect::Out { path: self.expr(path, out), append: *append },
                TRedir::Err { path, append } => Redirect::Err { path: self.expr(path, out), append: *append },
                TRedir::In(p) => Redirect::In(self.expr(p, out)),
                TRedir::ErrToOut => Redirect::ErrToOut,
            });
        }
        let stdin = c.stdin.as_ref().map(|s| match &s.ty {
            Ty::Str | Ty::Secret | Ty::Path => self.expr(s, out),
            _ => match self.text_part(s, out, false) {
                Part::Lit(t) => Expr::str(&t),
                Part::Expr(e) if e.ty == Ty::Str || e.ty == Ty::Path => e,
                Part::Expr(e) => Expr { kind: ExprKind::Interp(vec![Part::Expr(e)]), ty: Ty::Str },
            },
        });
        Command { pipeline, redirects, stdin, text: c.text.clone() }
    }

    /// Programs known to exist when `cond` evaluates to `when`: `proc.exists("x")`, combined with `&&`, `||` and `!`.
    fn existing_when(cond: &TExpr, when: bool) -> Vec<String> {
        match &cond.kind {
            TK::Call(c) if when && matches!(c.target, TTarget::Std("proc", "exists")) => match c.args.first() {
                Some(TArg::Expr(TExpr { kind: TK::Str(s), .. })) => vec![s.clone()],
                _ => Vec::new(),
            },
            TK::Unary(UnOp::Not, x) => Self::existing_when(x, !when),
            TK::Binary(BinOp::And, l, r) if when => [Self::existing_when(l, true), Self::existing_when(r, true)].concat(),
            TK::Binary(BinOp::Or, l, r) if !when => [Self::existing_when(l, false), Self::existing_when(r, false)].concat(),
            _ => Vec::new(),
        }
    }

    /// Lowers a branch of `if cond` with the programs that `cond` checked marked as optional.
    fn guarded_by<T>(&mut self, cond: &TExpr, when: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let progs = Self::existing_when(cond, when);
        let n = self.guarded.len();
        self.guarded.extend(progs);
        let r = f(self);
        self.guarded.truncate(n);
        r
    }

    /// Records a program used (`deps`) and checks that it exists on the target (`W010`).
    fn program_used(&mut self, prog: &str, span: Span) {
        let offset = span.start;
        if prog.is_empty() || prog.contains('/') || prog.contains('\\') || SHELL_BUILTINS.contains(&prog) || self.guarded.iter().any(|g| g == prog) {
            return;
        }
        if self.collect_deps {
            self.deps.push((offset, prog.to_string()));
        }
        if self.target == Target::Windows && UNIX_ONLY.contains(&prog) {
            self.diags.push(
                Diag::warning("W010", span, format!("program `{}` does not exist on Windows", prog))
                    .with_help("use the standard library (`fs`, `str`…) or put this inside `if os != \"windows\" { … }`"),
            );
        }
        if self.target != Target::Windows && WINDOWS_ONLY.contains(&prog) {
            self.diags.push(
                Diag::warning("W010", span, format!("program `{}` only exists on Windows", prog))
                    .with_help("put this inside `if os == \"windows\" { … }`"),
            );
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Failures: `catch`, `try`, `retry`, `parallel`
    // ---------------------------------------------------------------------------------------------

    /// Can the left side of the `catch` take the `OnFail` directly (nothing fallible nested)?
    fn direct_ok(&self, lhs: &TExpr) -> bool {
        let fnf = &self.tp.fallible;
        let mut r = Vec::new();
        match &lhs.kind {
            TK::Call(c) => {
                for a in &c.args {
                    if let TArg::Expr(x) = a {
                        check::reasons_expr(x, fnf, &mut r);
                    }
                }
            }
            TK::Capture(c) | TK::Command(c) => check::reasons_cmd(c, fnf, &mut r),
            _ => return false,
        }
        r.is_empty()
    }

    fn or_value(&mut self, lhs: &TExpr, h: &THandler, ty: &Ty, out: &mut Vec<Stmt>) -> Rvalue {
        if self.direct_ok(lhs) {
            match &lhs.kind {
                TK::Call(c) => {
                    let call = self.call(c, out);
                    let on_fail = self.onfail_value(h);
                    return Rvalue::Call { call, on_fail };
                }
                TK::Capture(c) => {
                    let cmd = self.cmd(c, out);
                    let on_fail = self.onfail_value(h);
                    return Rvalue::Capture { cmd, on_fail };
                }
                _ => {}
            }
        }
        let slot = self.slot(ty, out);
        if let TK::Parallel { .. } = &lhs.kind {
            let on_fail = self.onfail_assign(h, &slot);
            self.parallel(lhs, Some(slot.clone()), on_fail, out);
            return Rvalue::Expr(Expr::var(&slot, ty.clone()));
        }
        let mut body = Vec::new();
        let rv = self.rvalue(lhs, &mut body);
        body.push(self.assign_var(&slot, rv));
        let on_fail = self.onfail_assign(h, &slot);
        out.push(self.st(StmtKind::Try { body: Block { stmts: body }, on_fail }));
        Rvalue::Expr(Expr::var(&slot, ty.clone()))
    }

    fn stmt_or(&mut self, lhs: &TExpr, h: &THandler, out: &mut Vec<Stmt>) {
        match &lhs.kind {
            TK::Call(c) if self.direct_ok(lhs) => {
                let call = self.call(c, out);
                let on_fail = self.onfail_stmt(h);
                out.push(self.st(StmtKind::Call { call, on_fail }));
            }
            TK::Command(c) if self.direct_ok(lhs) => {
                let cmd = self.cmd(c, out);
                let on_fail = self.onfail_stmt(h);
                out.push(self.st(StmtKind::Command { cmd, on_fail }));
            }
            TK::Capture(c) if self.direct_ok(lhs) => {
                let cmd = self.cmd(c, out);
                let on_fail = self.onfail_value(h);
                let t = self.temp();
                out.push(self.st(StmtKind::Let { var: t, ty: Ty::Str, mutable: false, value: Rvalue::Capture { cmd, on_fail } }));
            }
            TK::Try(b) => {
                let body = self.tblock_stmts(b);
                let on_fail = self.onfail_stmt(h);
                out.push(self.st(StmtKind::Try { body, on_fail }));
            }
            TK::Retry { .. } => {
                let on_fail = self.onfail_stmt(h);
                self.retry(lhs, on_fail, out);
            }
            TK::Parallel { .. } if lhs.ty == Ty::Unit => {
                let on_fail = self.onfail_stmt(h);
                self.parallel(lhs, None, on_fail, out);
            }
            _ => {
                let mut body = Vec::new();
                self.stmt_expr(lhs, &mut body);
                let on_fail = self.onfail_stmt(h);
                out.push(self.st(StmtKind::Try { body: Block { stmts: body }, on_fail }));
            }
        }
    }

    /// `OnFail` of a used value (Default/Fail/Ignore/Block with a value).
    fn onfail_value(&mut self, h: &THandler) -> OnFail {
        match h {
            THandler::Ignore => OnFail::Ignore,
            THandler::Default(v) => {
                let mut scratch = Vec::new();
                let rv = self.rvalue(v, &mut scratch);
                if scratch.is_empty() {
                    return OnFail::Default(Box::new(rv));
                }
                let ev = match rv {
                    Rvalue::Expr(x) => x,
                    other => {
                        let t = self.temp();
                        scratch.push(self.st(StmtKind::Let { var: t.clone(), ty: v.ty.clone(), mutable: false, value: other }));
                        Expr::var(&t, v.ty.clone())
                    }
                };
                OnFail::Block { err: None, body: Block { stmts: scratch }, value: Some(ev) }
            }
            THandler::Fail { message, code } => self.onfail_fail(message, code.as_ref()),
            THandler::Block { err, body } => {
                let en = err.map(|id| self.bind(id));
                let line = self.line;
                let mut stmts = Vec::new();
                for s in &body.stmts {
                    self.stmt(s, &mut stmts);
                }
                let value = body.value.as_ref().map(|v| {
                    self.line = self.line_of(v.span.start);
                    self.place_expr(v, &mut stmts)
                });
                self.line = line;
                OnFail::Block { err: en, body: Block { stmts }, value }
            }
        }
    }

    /// `OnFail` of a statement (result discarded).
    fn onfail_stmt(&mut self, h: &THandler) -> OnFail {
        match h {
            THandler::Ignore => OnFail::Ignore,
            THandler::Default(v) => {
                let mut scratch = Vec::new();
                self.stmt_expr(v, &mut scratch);
                OnFail::Block { err: None, body: Block { stmts: scratch }, value: None }
            }
            THandler::Fail { message, code } => self.onfail_fail(message, code.as_ref()),
            THandler::Block { err, body } => {
                let en = err.map(|id| self.bind(id));
                let line = self.line;
                let b = self.tblock_stmts(body);
                self.line = line;
                OnFail::Block { err: en, body: b, value: None }
            }
        }
    }

    /// `OnFail` that stores the recovery value in `slot`.
    fn onfail_assign(&mut self, h: &THandler, slot: &str) -> OnFail {
        match h {
            THandler::Ignore => OnFail::Ignore,
            THandler::Default(v) => {
                let mut body = Vec::new();
                let rv = self.rvalue(v, &mut body);
                body.push(self.assign_var(slot, rv));
                OnFail::Block { err: None, body: Block { stmts: body }, value: None }
            }
            THandler::Fail { message, code } => self.onfail_fail(message, code.as_ref()),
            THandler::Block { err, body } => {
                let en = err.map(|id| self.bind(id));
                let line = self.line;
                let b = self.tblock_into(body, slot);
                self.line = line;
                OnFail::Block { err: en, body: b, value: None }
            }
        }
    }

    /// `or fail "…"`: `OnFail::Fail` when the message is pure; otherwise a block with the named error.
    fn onfail_fail(&mut self, message: &TExpr, code: Option<&TExpr>) -> OnFail {
        let saved_ns = self.ns.clone();
        let mut scratch = Vec::new();
        self.err_mode.push(ErrMode::Current);
        let m = self.expr(message, &mut scratch);
        let c = code.map(|c| self.expr(c, &mut scratch));
        self.err_mode.pop();
        if scratch.is_empty() {
            return OnFail::Fail { message: m, code: c };
        }
        self.ns = saved_ns;
        let en = self.fresh("err");
        let mut body = Vec::new();
        self.err_mode.push(ErrMode::Var(en.clone()));
        let m = self.expr(message, &mut body);
        let c = code.map(|c| self.expr(c, &mut body));
        self.err_mode.pop();
        body.push(self.st(StmtKind::Fail { message: m, code: c }));
        OnFail::Block { err: Some(en), body: Block { stmts: body }, value: None }
    }

    fn retry(&mut self, e: &TExpr, on_fail: OnFail, out: &mut Vec<Stmt>) {
        let TK::Retry { times, every, body } = &e.kind else { return };
        let t = self.expr(times, out);
        let ev = self.expr(every, out);
        let line = self.line;
        let b = self.tblock_stmts(body);
        self.line = line;
        out.push(self.st(StmtKind::Retry { times: t, every: ev, body: b, on_fail }));
    }

    /// `parallel`; with `slot`, the results (`parallel for` with a value) go into that already declared list.
    fn parallel(&mut self, e: &TExpr, slot: Option<String>, on_fail: OnFail, out: &mut Vec<Stmt>) {
        let TK::Parallel { max, body } = &e.kind else { return };
        let m = max.as_ref().map(|m| self.expr(m, out));
        let line = self.line;
        let pb = match &**body {
            TPar::For { var, list, body } => {
                let l = self.place_expr(list, out);
                let v = self.bind(*var);
                let mut stmts = Vec::new();
                for s in &body.stmts {
                    self.stmt(s, &mut stmts);
                }
                if let Some(v) = &body.value {
                    self.line = self.line_of(v.span.start);
                }
                let collect = match (&slot, &body.value) {
                    (Some(sl), Some(val)) => {
                        let ve = self.place_expr(val, &mut stmts);
                        Some((sl.clone(), ve))
                    }
                    (None, Some(val)) => {
                        self.stmt_expr(val, &mut stmts);
                        None
                    }
                    _ => None,
                };
                ParallelBody::For { var: v, list: l, body: Block { stmts }, collect }
            }
            TPar::Tasks(stmts) => {
                let mut tasks = Vec::new();
                for s in stmts {
                    let mut b = Vec::new();
                    self.stmt(s, &mut b);
                    if !b.is_empty() {
                        tasks.push(Block { stmts: b });
                    }
                }
                ParallelBody::Tasks(tasks)
            }
        };
        self.line = line;
        out.push(self.st(StmtKind::Parallel { max: m, body: pb, on_fail }));
    }
}

/// `!c`, without double negation.
fn negate(c: Expr) -> Expr {
    match c.kind {
        ExprKind::Unary(UnOp::Not, inner) => *inner,
        ExprKind::Bool(b) => Expr::bool(!b),
        kind => Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(Expr { kind, ty: Ty::Bool })), ty: Ty::Bool },
    }
}

/// Text of a `param`'s pattern for `--help`.
fn default_text(rv: &Rvalue) -> String {
    fn e(x: &Expr) -> String {
        match &x.kind {
            ExprKind::Str(s) => s.clone(),
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Duration(d) => format_duration(*d),
            ExprKind::EnumCase { case, .. } => case.clone(),
            ExprKind::Unary(UnOp::Neg, inner) => format!("-{}", e(inner)),
            _ => String::new(),
        }
    }
    match rv {
        Rvalue::Expr(x) => e(x),
        Rvalue::List { items, .. } => items.iter().map(e).collect::<Vec<_>>().join(","),
        _ => String::new(),
    }
}

// =================================================================================================
// Tests
// =================================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn comp(src: &str, target: Target) -> Compiled {
        compile(src, "t.nalu", std::path::Path::new("."), target)
    }

    fn prog(src: &str) -> Program {
        let c = comp(src, Target::Linux);
        let errs: Vec<_> = c.diags.iter().filter(|d| d.is_error()).collect();
        assert!(errs.is_empty(), "{}\n{:#?}", src, errs);
        c.program.unwrap()
    }

    // ---------- validator for the IR rules ----------

    struct V {
        problems: Vec<String>,
    }

    impl V {
        fn expr(&mut self, e: &Expr, ctx: &str) {
            if !e.ty.is_scalar() && e.ty != Ty::Error && !matches!(e.kind, ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Args | ExprKind::Index(_, _)) {
                self.problems.push(format!("{}: collection Expr that is not Var/Field: {:?}", ctx, e.kind));
            }
            match &e.kind {
                ExprKind::Field(b, _) => self.expr_base(b, ctx),
                ExprKind::Index(a, b) => {
                    self.expr(a, ctx);
                    self.expr(b, ctx);
                    if let Ty::Struct(_) = e.ty {
                        // only as the base of a Field (checked in expr_base)
                    }
                }
                ExprKind::Len(x) | ExprKind::Unary(_, x) => self.expr(x, ctx),
                ExprKind::Binary(_, a, b) => {
                    self.expr(a, ctx);
                    self.expr(b, ctx);
                }
                ExprKind::Interp(parts) => {
                    for p in parts {
                        if let Part::Expr(x) = p {
                            if !matches!(x.ty, Ty::Str | Ty::Path | Ty::Int | Ty::Bool | Ty::Enum(_)) {
                                self.problems.push(format!("{}: Interp part with type {:?}", ctx, x.ty));
                            }
                            self.expr(x, ctx);
                        }
                    }
                }
                _ => {}
            }
        }
        fn expr_base(&mut self, e: &Expr, ctx: &str) {
            match &e.kind {
                ExprKind::Index(a, b) => {
                    self.expr(a, ctx);
                    self.expr(b, ctx);
                }
                _ => self.expr(e, ctx),
            }
        }
        fn struct_index_standalone(&mut self, e: &Expr, ctx: &str) {
            if matches!(e.kind, ExprKind::Index(_, _)) && matches!(e.ty, Ty::Struct(_)) {
                self.problems.push(format!("{}: Index de struct solto", ctx));
            }
        }
        fn rv(&mut self, r: &Rvalue, ctx: &str) {
            match r {
                Rvalue::Expr(e) => {
                    self.struct_index_standalone(e, ctx);
                    self.expr(e, ctx)
                }
                Rvalue::List { items, .. } => items.iter().for_each(|i| self.expr(i, ctx)),
                Rvalue::Map { entries, .. } => entries.iter().for_each(|(k, v)| {
                    self.expr(k, ctx);
                    self.expr(v, ctx);
                }),
                Rvalue::Struct { spread, fields, .. } => {
                    if let Some(s) = spread {
                        if !matches!(s.kind, ExprKind::Var(_)) {
                            self.problems.push(format!("{}: spread is not a Var", ctx));
                        }
                    }
                    fields.iter().for_each(|(_, v)| self.expr(v, ctx));
                }
                Rvalue::Call { call, on_fail } => {
                    self.call(call, ctx);
                    self.onfail(on_fail, ctx);
                }
                Rvalue::Capture { cmd, on_fail } => {
                    self.cmd(cmd, ctx);
                    self.onfail(on_fail, ctx);
                }
                Rvalue::CommandOk(c) => self.cmd(c, ctx),
            }
        }
        fn call(&mut self, c: &Call, ctx: &str) {
            for a in &c.args {
                match a {
                    Arg::Expr(e) => {
                        self.struct_index_standalone(e, ctx);
                        self.expr(e, ctx)
                    }
                    Arg::Lambda(l) => self.block(&l.body, ctx),
                }
            }
        }
        fn cmd(&mut self, c: &Command, ctx: &str) {
            for s in &c.pipeline {
                assert!(!s.words.is_empty());
                for w in &s.words {
                    match w {
                        Word::Interp(ps) => ps.iter().for_each(|p| {
                            if let Part::Expr(x) = p {
                                self.expr(x, ctx)
                            }
                        }),
                        Word::Spread(e) => self.expr(e, ctx),
                        Word::Lit(_) => {}
                    }
                }
            }
        }
        fn onfail(&mut self, o: &OnFail, ctx: &str) {
            match o {
                OnFail::Default(r) => self.rv(r, ctx),
                OnFail::Fail { message, code } => {
                    self.expr(message, ctx);
                    if let Some(c) = code {
                        self.expr(c, ctx);
                    }
                }
                OnFail::Block { body, value, .. } => {
                    self.block(body, ctx);
                    if let Some(v) = value {
                        self.expr(v, ctx);
                    }
                }
                OnFail::Propagate | OnFail::Ignore => {}
            }
        }
        fn block(&mut self, b: &Block, ctx: &str) {
            for s in &b.stmts {
                self.stmt(s, ctx);
            }
        }
        fn stmt(&mut self, s: &Stmt, ctx: &str) {
            match &s.kind {
                StmtKind::Let { value, .. } | StmtKind::Assign { value, .. } => self.rv(value, ctx),
                StmtKind::Call { call, on_fail } => {
                    self.call(call, ctx);
                    self.onfail(on_fail, ctx);
                }
                StmtKind::Command { cmd, on_fail } => {
                    self.cmd(cmd, ctx);
                    self.onfail(on_fail, ctx);
                }
                StmtKind::If { cond, then, els } => {
                    self.expr(cond, ctx);
                    self.block(then, ctx);
                    if let Some(e) = els {
                        self.block(e, ctx);
                    }
                }
                StmtKind::While { cond, body } => {
                    self.expr(cond, ctx);
                    self.block(body, ctx);
                }
                StmtKind::ForRange { from, to, body, .. } => {
                    self.expr(from, ctx);
                    self.expr(to, ctx);
                    self.block(body, ctx);
                }
                StmtKind::ForList { list, body, .. } | StmtKind::ForMap { map: list, body, .. } => {
                    if !matches!(list.kind, ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Args) {
                        self.problems.push(format!("{}: loop over something that is not Var/Field", ctx));
                    }
                    self.block(body, ctx);
                }
                StmtKind::Match { subject, arms, default } => {
                    self.expr(subject, ctx);
                    for a in arms {
                        assert!(!a.patterns.is_empty());
                        self.block(&a.body, ctx);
                    }
                    if let Some(d) = default {
                        self.block(d, ctx);
                    }
                }
                StmtKind::Try { body, on_fail } => {
                    self.block(body, ctx);
                    self.onfail(on_fail, ctx);
                }
                StmtKind::Retry { times, every, body, on_fail } => {
                    self.expr(times, ctx);
                    self.expr(every, ctx);
                    self.block(body, ctx);
                    self.onfail(on_fail, ctx);
                }
                StmtKind::Parallel { body, on_fail, .. } => {
                    match body {
                        ParallelBody::For { body, collect, .. } => {
                            self.block(body, ctx);
                            if let Some((_, e)) = collect {
                                self.expr(e, ctx);
                            }
                        }
                        ParallelBody::Tasks(ts) => ts.iter().for_each(|t| self.block(t, ctx)),
                    }
                    self.onfail(on_fail, ctx);
                }
                StmtKind::Fail { message, code } => {
                    self.expr(message, ctx);
                    if let Some(c) = code {
                        self.expr(c, ctx);
                    }
                }
                StmtKind::Return(Some(e)) | StmtKind::Exit(Some(e)) => {
                    self.struct_index_standalone(e, ctx);
                    self.expr(e, ctx)
                }
                StmtKind::Defer(inner) | StmtKind::AssertFails(inner) => self.stmt(inner, ctx),
                StmtKind::Assert { cond, .. } => self.expr(cond, ctx),
                _ => {}
            }
        }
    }

    /// Names declared in a function scope: all distinct.
    fn declared(b: &Block, out: &mut Vec<String>) {
        fn of(o: &OnFail, out: &mut Vec<String>) {
            match o {
                OnFail::Block { err, body, .. } => {
                    if let Some(e) = err {
                        out.push(e.clone());
                    }
                    declared(body, out);
                }
                OnFail::Default(r) => rv(r, out),
                _ => {}
            }
        }
        fn rv(r: &Rvalue, out: &mut Vec<String>) {
            match r {
                Rvalue::Call { call, on_fail } => {
                    for a in &call.args {
                        if let Arg::Lambda(l) = a {
                            for p in &l.params {
                                out.push(p.name.clone());
                            }
                            declared(&l.body, out);
                        }
                    }
                    of(on_fail, out);
                }
                Rvalue::Capture { on_fail, .. } => of(on_fail, out),
                _ => {}
            }
        }
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { var, value, .. } => {
                    out.push(var.clone());
                    rv(value, out);
                }
                StmtKind::Assign { value, .. } => rv(value, out),
                StmtKind::Call { call, on_fail } => rv(&Rvalue::Call { call: call.clone(), on_fail: on_fail.clone() }, out),
                StmtKind::Command { on_fail, .. } => of(on_fail, out),
                StmtKind::If { then, els, .. } => {
                    declared(then, out);
                    if let Some(e) = els {
                        declared(e, out);
                    }
                }
                StmtKind::While { body, .. } => declared(body, out),
                StmtKind::ForRange { var, body, .. } => {
                    out.push(var.clone());
                    declared(body, out);
                }
                StmtKind::ForList { var, index, body, .. } => {
                    out.push(var.clone());
                    if let Some(i) = index {
                        out.push(i.clone());
                    }
                    declared(body, out);
                }
                StmtKind::ForMap { key, value, body, .. } => {
                    out.push(key.clone());
                    out.push(value.clone());
                    declared(body, out);
                }
                StmtKind::Match { arms, default, .. } => {
                    for a in arms {
                        declared(&a.body, out);
                    }
                    if let Some(d) = default {
                        declared(d, out);
                    }
                }
                StmtKind::Try { body, on_fail } | StmtKind::Retry { body, on_fail, .. } => {
                    declared(body, out);
                    of(on_fail, out);
                }
                StmtKind::Parallel { body, on_fail, .. } => {
                    match body {
                        ParallelBody::For { var, body, .. } => {
                            out.push(var.clone());
                            declared(body, out);
                        }
                        ParallelBody::Tasks(ts) => ts.iter().for_each(|t| declared(t, out)),
                    }
                    of(on_fail, out);
                }
                StmtKind::Defer(inner) | StmtKind::AssertFails(inner) => declared(&Block { stmts: vec![(**inner).clone()] }, out),
                _ => {}
            }
        }
    }

    fn unique(names: &[String], ctx: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut probs = Vec::new();
        for n in names {
            if !seen.insert(n) {
                probs.push(format!("{}: nome repetido `{}`", ctx, n));
            }
        }
        probs
    }

    pub fn validate(p: &Program) -> Vec<String> {
        let mut v = V { problems: vec![] };
        v.block(&p.body, "topo");
        let mut top: Vec<String> = p.params.iter().map(|x| x.name.clone()).collect();
        declared(&p.body, &mut top);
        v.problems.extend(unique(&top, "topo"));
        // direct globals: params + `Let` at the top level
        let mut globals: HashSet<String> = p.params.iter().map(|x| x.name.clone()).collect();
        for st in &p.body.stmts {
            if let StmtKind::Let { var, .. } = &st.kind {
                globals.insert(var.clone());
            }
        }
        for f in &p.funcs {
            v.block(&f.body, &f.name);
            let mut names: Vec<String> = f.params.iter().map(|x| x.name.clone()).collect();
            declared(&f.body, &mut names);
            v.problems.extend(unique(&names, &f.name));
            for n in &names {
                if globals.contains(n) && !n.starts_with("__") {
                    v.problems.push(format!("{}: local `{}` has the same name as a top-level variable", f.name, n));
                }
            }
        }
        for t in &p.tests {
            v.block(&t.body, &t.name);
            let mut names = Vec::new();
            declared(&t.body, &mut names);
            v.problems.extend(unique(&names, &t.name));
        }
        v.problems
    }

    #[test]
    fn examples_compile_and_follow_the_ir_rules() {
        for f in ["hello", "backup", "deploy", "installer", "release"] {
            let path = format!("{}/examples/{}.nalu", env!("CARGO_MANIFEST_DIR"), f);
            let src = std::fs::read_to_string(&path).unwrap();
            for t in Target::ALL {
                let c = compile(&src, &format!("{}.nalu", f), std::path::Path::new("examples"), t);
                let errs: Vec<_> = c.diags.iter().filter(|d| d.is_error()).collect();
                assert!(errs.is_empty(), "{} {:?}: {:#?}", f, t, errs);
                let p = c.program.expect("programa");
                let probs = validate(&p);
                assert!(probs.is_empty(), "{} {:?}:\n{}", f, t, probs.join("\n"));
            }
        }
    }

    #[test]
    fn release_with_tests() {
        let dir = format!("{}/examples", env!("CARGO_MANIFEST_DIR"));
        let a = std::fs::read_to_string(format!("{}/release.nalu", dir)).unwrap();
        let b = std::fs::read_to_string(format!("{}/release_test.nalu", dir)).unwrap();
        let src = format!("{}\n// --- release_test.nalu\n{}", a, b);
        let c = compile(&src, "release.nalu", std::path::Path::new(&dir), Target::Macos);
        assert!(c.diags.iter().all(|d| !d.is_error()), "{:#?}", c.diags);
        let p = c.program.unwrap();
        assert_eq!(p.tests.len(), 6);
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
        // alone: name errors, no panic
        let c = compile(&b, "release_test.nalu", std::path::Path::new(&dir), Target::Linux);
        assert!(c.program.is_none());
        assert!(c.diags.iter().any(|d| d.code == "E001"));
    }

    #[test]
    fn usage_exact_format() {
        let src = "/// ambiente de destino\nparam ambiente: Ambiente = Ambiente.dev\n/// versão a publicar\nparam versao: str\n/// pastas para processar\nparam pastas: []str = [\"docs\"]\n/// apaga também o cache\nparam limpar_cache: bool\n/// espera entre tentativas\nparam espera: duration = 30s\n/// token do registry\nparam token: secret\n\nenum Ambiente { dev, stage, prod }\nprint(ambiente, versao, pastas.len(), limpar_cache, espera)\ntoken | $ login\nprint(args.len())\n";
        let p = prog(src);
        let esperado = "usage: t --versao <str> --token <secret> [options] [args…]

  --ambiente <dev|stage|prod>  ambiente de destino (default: dev)
  --versao <str>               versão a publicar (required)
  --pastas <[]str>             pastas para processar (default: docs)
  --limpar-cache               apaga também o cache
  --espera <duration>          espera entre tentativas (default: 30s)
  --token <secret>             token do registry (required; or set NALU_TOKEN)
  --dry-run                    show what would happen, without running commands or changing files
  --verbose                    print each command before running it
  -h, --help                   show this help";
        assert_eq!(p.usage, esperado);
        assert_eq!(p.params[5].env_var.as_deref(), Some("NALU_TOKEN"));
        assert_eq!(p.params[3].flag, "--limpar-cache");
        // no params: only the default flags, same as `Program::empty` in `ir.rs`
        let p = prog("print(1)");
        assert_eq!(p.usage, Program::empty("t", Target::Linux).usage);
    }

    #[test]
    fn os_resolved() {
        let src = "if os == \"windows\" {\n $ winget install x\n} else {\n $ brew install x\n}\nlet r = match os {\n \"linux\" => 1\n \"macos\" => 2\n \"windows\" => 3\n}\nprint(r, os)\n";
        let p = compile(src, "t.nalu", std::path::Path::new("."), Target::Windows).program.unwrap();
        let s = program_to_string(&p);
        assert!(s.contains("winget") && !s.contains("brew"), "{}", s);
        assert_eq!(p.deps, vec!["winget".to_string()]);
        assert!(s.contains("Int(\n                            3,") || s.contains("Int(3)") || s.contains("3,"), "{}", s);
        let p = compile(src, "t.nalu", std::path::Path::new("."), Target::Macos).program.unwrap();
        let s = program_to_string(&p);
        assert!(!s.contains("winget") && s.contains("brew"));
        assert!(s.contains("\"macos\""));
    }

    #[test]
    fn w010_per_target() {
        let src = "$ grep -r x .\nif os != \"windows\" {\n $ head -n 1 x\n}\n";
        let c = comp(src, Target::Windows);
        let w: Vec<_> = c.diags.iter().filter(|d| d.code == "W010").collect();
        assert_eq!(w.len(), 1, "{:#?}", c.diags);
        let c = comp(src, Target::Linux);
        assert!(c.diags.iter().all(|d| d.code != "W010"));
    }

    #[test]
    fn deps_in_order_without_builtins() {
        let p = prog("fn f() {\n $ docker ps\n}\n$ git pull\n$ echo oi\n$ cd x\n$ git push\nlet x = $(kubectl get pods | grep api)\nprint(x)\nf()\n");
        assert_eq!(p.deps, vec!["docker", "git", "kubectl", "grep"]);
    }

    #[test]
    fn a_normal_and_onfail() {
        let p = prog("let u = env.get(\"USER\") catch env.get(\"USERNAME\") catch \"x\"\nprint(\"oi, {u}\")\n");
        match &p.body.stmts[0].kind {
            StmtKind::Let { value: Rvalue::Call { on_fail: OnFail::Default(d), .. }, .. } => {
                assert!(matches!(&**d, Rvalue::Call { on_fail: OnFail::Default(_), .. }));
            }
            o => panic!("{:#?}", o),
        }
        // or covering nested fallible parts
        let p = prog("fn f(n: int) -> int {\n return n\n}\nlet x = f(\"1\".to_int()) catch 0\nprint(x)\n");
        let s = program_to_string(&p);
        assert!(s.contains("Try"), "{}", s);
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn short_circuit() {
        let p = prog("let ok = fs.exists(\"a\") && \"1\".to_int() == 1 catch false\nprint(ok)\n");
        let s = program_to_string(&p);
        assert!(s.contains("If"), "{}", s);
        assert!(validate(&p).is_empty());
        let p = prog("var i = 0\nwhile i < \"3\".to_int() {\n i += 1\n}\n");
        match &p.body.stmts[1].kind {
            StmtKind::While { cond, body } => {
                assert!(matches!(cond.kind, ExprKind::Bool(true)));
                assert!(matches!(body.stmts[1].kind, StmtKind::If { .. }));
            }
            o => panic!("{:#?}", o),
        }
    }

    #[test]
    fn unique_names_with_shadowing() {
        let p = prog("let x = 1\nfn f(x: int) -> int {\n let y = x\n if y > 0 {\n  let y = 2\n  return y\n }\n return y\n}\nif x > 0 {\n let x = 3\n print(x)\n}\nprint(f(x))\n");
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
        let s = program_to_string(&p);
        assert!(s.contains("\"x_2\""), "{}", s);
        assert!(s.contains("\"y_2\""), "{}", s);
    }

    #[test]
    fn methods_and_structs() {
        let p = prog("struct S {\n a: int\n b: []str = []\n fn dobro() -> int {\n  return self.a * 2\n }\n}\nlet s = S { a: 1 }\nlet xs = [s, { a: 2 }]\nprint(s.dobro(), xs[1].a, xs[0].dobro())\nlet t = S { ..s, a: 3 }\nprint(t.a)\n");
        assert!(validate(&p).is_empty(), "{:?}", validate(&p));
        assert_eq!(p.funcs[0].name, "S.dobro");
        assert_eq!(p.funcs[0].params[0].name, "self");
        let s = program_to_string(&p);
        assert!(s.contains("User(\n") && s.contains("\"S.dobro\""), "{}", s);
    }

    #[test]
    fn interp_converts_duration_and_error() {
        let p = prog("let d = 90s\nprint(\"{d} {5s}\")\n$ x catch {\n print(\"{err}\")\n}\n");
        let s = program_to_string(&p);
        assert!(s.contains("duration_str"), "{}", s);
        assert!(s.contains("\"message\""), "{}", s);
        assert!(s.contains("5s"));
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn or_fail_with_call_in_message() {
        let p = prog("fn nome() -> str {\n return \"x\"\n}\nlet v = fs.read(\"a\") catch fail \"erro em {nome()}: {err}\"\nprint(v)\n");
        let s = program_to_string(&p);
        assert!(s.contains("err: Some("), "{}", s);
        assert!(validate(&p).is_empty());
    }

    #[test]
    fn duration_text() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(250), "250ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(90_000), "1m30s");
        assert_eq!(format_duration(93_600_000), "1d2h");
    }

    #[test]
    fn explain_points_at_each_line() {
        use crate::diag::line_col;
        let src = "fn a() -> int {\n return \"1\".to_int()\n}\nfn b() -> int {\n return 1\n}\nprint(a(), b())\nlet t = fs.read(\"x\") catch \"\"\n$ git status\ntest \"t\" {\n let xs = [1]\n assert xs[0] == 1\n}\n";
        let e = explain(src, std::path::Path::new(".")).unwrap();
        let got: Vec<(usize, &str, &str)> = e.points.iter().map(|p| (line_col(src, p.span.start).0, p.what.as_str(), p.effect.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (2, "`.to_int()`", "`a` fails"),
                (7, "`a`", "the script stops"),
                (9, "`$ git`", "the script stops"),
                (12, "`list[i]`", "test \"t\" fails"),
            ],
            "{:#?}",
            e.points
        );
        // `fs.read(…) or ""` is handled: it does not appear
        assert_eq!(e.fallible_fns, vec!["a".to_string()]);
    }

    #[test]
    fn thousand_lines_fast() {
        let mut src = String::new();
        for i in 0..200 {
            src.push_str(&format!("fn f{}(x: int) -> int {{\n  let y = x * 2 + {}\n  return y\n}}\nprint(f{}({}))\n", i, i, i, i));
        }
        let t = std::time::Instant::now();
        let c = comp(&src, Target::Linux);
        assert!(c.program.is_some());
        // debug is much slower than release; the criterion (< 50 ms) applies to release
        assert!(t.elapsed().as_millis() < 2000);
    }

    // ---------- IR pseudo-code (for review and snapshots) ----------

    fn pe(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Str(s) => format!("{:?}", s),
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Duration(d) => format!("{}ms", d),
            ExprKind::EnumCase { enum_, case } => format!("{}.{}", enum_, case),
            ExprKind::Var(n) => n.clone(),
            ExprKind::DryRun => "dry_run".into(),
            ExprKind::Args => "args".into(),
            ExprKind::CurrentErr => "ERR".into(),
            ExprKind::Field(b, f) => format!("{}.{}", pe(b), f),
            ExprKind::Index(a, b) => format!("{}[{}]", pe(a), pe(b)),
            ExprKind::Len(x) => format!("len({})", pe(x)),
            ExprKind::Unary(op, x) => format!("{}{}", if *op == UnOp::Neg { "-" } else { "!" }, pe(x)),
            ExprKind::Binary(op, a, b) => format!("({} {:?} {})", pe(a), op, pe(b)),
            ExprKind::Interp(ps) => {
                let s: String = ps.iter().map(|p| match p {
                    Part::Lit(t) => t.replace('\n', "\\n"),
                    Part::Expr(x) => format!("{{{}}}", pe(x)),
                }).collect();
                format!("f\"{}\"", s)
            }
        }
    }
    fn pcall(c: &Call) -> String {
        let t = match &c.target {
            CallTarget::User(n) => n.clone(),
            CallTarget::Std { module, name } => format!("{}.{}", module, name),
            CallTarget::Builtin(n) => format!("@{}", n),
            CallTarget::Method { recv_ty, name } => format!("<{}>.{}", recv_ty.display(), name),
        };
        let args: Vec<String> = c.args.iter().map(|a| match a {
            Arg::Expr(e) => pe(e),
            Arg::Lambda(l) => format!("|{}| {{ {} }}", l.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(","), pb(&l.body, 0).trim().replace('\n', "; ")),
        }).collect();
        format!("{}({}){}", t, args.join(", "), if c.fallible { "!" } else { "" })
    }
    fn pcmd(c: &Command) -> String {
        let st: Vec<String> = c.pipeline.iter().map(|s| s.words.iter().map(|w| match w {
            Word::Lit(t) => format!("{:?}", t),
            Word::Interp(ps) => pe(&Expr { kind: ExprKind::Interp(ps.clone()), ty: Ty::Str }),
            Word::Spread(e) => format!("...{}", pe(e)),
        }).collect::<Vec<_>>().join(" ")).collect();
        let mut s = format!("$[{}]", st.join(" | "));
        if let Some(i) = &c.stdin { s = format!("{} | {}", pe(i), s); }
        if !c.redirects.is_empty() { s.push_str(&format!(" redir{:?}", c.redirects.len())); }
        s
    }
    fn prv(r: &Rvalue, ind: usize) -> String {
        match r {
            Rvalue::Expr(e) => pe(e),
            Rvalue::List { items, .. } => format!("[{}]", items.iter().map(pe).collect::<Vec<_>>().join(", ")),
            Rvalue::Map { entries, .. } => format!("{{{}}}", entries.iter().map(|(k, v)| format!("{}: {}", pe(k), pe(v))).collect::<Vec<_>>().join(", ")),
            Rvalue::Struct { ty, spread, fields } => format!("{} {{ {}{} }}", ty, spread.as_ref().map(|s| format!("..{}, ", pe(s))).unwrap_or_default(), fields.iter().map(|(n, v)| format!("{}: {}", n, pe(v))).collect::<Vec<_>>().join(", ")),
            Rvalue::Call { call, on_fail } => format!("{}{}", pcall(call), pof(on_fail, ind)),
            Rvalue::Capture { cmd, on_fail } => format!("capture {}{}", pcmd(cmd), pof(on_fail, ind)),
            Rvalue::CommandOk(c) => format!("ok? {}", pcmd(c)),
        }
    }
    fn pof(o: &OnFail, ind: usize) -> String {
        match o {
            OnFail::Propagate => String::new(),
            OnFail::Ignore => " or ignore".into(),
            OnFail::Default(r) => format!(" or {}", prv(r, ind)),
            OnFail::Fail { message, code } => format!(" or fail {}{}", pe(message), code.as_ref().map(|c| format!(" code {}", pe(c))).unwrap_or_default()),
            OnFail::Block { err, body, value } => format!(" or |{}| {{\n{}{}}}{}", err.clone().unwrap_or_default(), pb(body, ind + 1), "  ".repeat(ind), value.as_ref().map(|v| format!(" => {}", pe(v))).unwrap_or_default()),
        }
    }
    fn pb(b: &Block, ind: usize) -> String {
        let mut s = String::new();
        for st in &b.stmts {
            let pad = "  ".repeat(ind);
            let line = match &st.kind {
                StmtKind::Let { var, ty, mutable, value } => format!("{} {}: {} = {}", if *mutable { "var" } else { "let" }, var, ty.display(), prv(value, ind)),
                StmtKind::Assign { target, value } => format!("{:?} = {}", target, prv(value, ind)).replace("Var(\"", "").replace("\")", ""),
                StmtKind::Call { call, on_fail } => format!("{}{}", pcall(call), pof(on_fail, ind)),
                StmtKind::Command { cmd, on_fail } => format!("{}{}", pcmd(cmd), pof(on_fail, ind)),
                StmtKind::If { cond, then, els } => format!("if {} {{\n{}{}}}{}", pe(cond), pb(then, ind + 1), pad, els.as_ref().map(|e| format!(" else {{\n{}{}}}", pb(e, ind + 1), pad)).unwrap_or_default()),
                StmtKind::While { cond, body } => format!("while {} {{\n{}{}}}", pe(cond), pb(body, ind + 1), pad),
                StmtKind::ForRange { var, from, to, inclusive, body } => format!("for {} in {}{}{} {{\n{}{}}}", var, pe(from), if *inclusive { "..=" } else { ".." }, pe(to), pb(body, ind + 1), pad),
                StmtKind::ForList { var, index, list, body } => format!("for {}{} in {} {{\n{}{}}}", index.as_ref().map(|i| format!("{}, ", i)).unwrap_or_default(), var, pe(list), pb(body, ind + 1), pad),
                StmtKind::ForMap { key, value, map, body } => format!("for {}, {} in {} {{\n{}{}}}", key, value, pe(map), pb(body, ind + 1), pad),
                StmtKind::Match { subject, arms, default } => {
                    let mut m = format!("match {} {{\n", pe(subject));
                    for a in arms {
                        let pats: Vec<String> = a.patterns.iter().map(|p| match p { Pattern::Lit(e) => pe(e), Pattern::Range { from, to, inclusive } => format!("{}{}{}", pe(from), if *inclusive { "..=" } else { ".." }, pe(to)) }).collect();
                        m.push_str(&format!("{}  {} => {{\n{}{}  }}\n", pad, pats.join(" | "), pb(&a.body, ind + 2), pad));
                    }
                    if let Some(d) = default { m.push_str(&format!("{}  _ => {{\n{}{}  }}\n", pad, pb(d, ind + 2), pad)); }
                    m.push_str(&format!("{}}}", pad));
                    m
                }
                StmtKind::Try { body, on_fail } => format!("try {{\n{}{}}}{}", pb(body, ind + 1), pad, pof(on_fail, ind)),
                StmtKind::Retry { times, every, body, on_fail } => format!("retry({}, {}) {{\n{}{}}}{}", pe(times), pe(every), pb(body, ind + 1), pad, pof(on_fail, ind)),
                StmtKind::Parallel { max, body, on_fail } => match body {
                    ParallelBody::For { var, list, body, collect } => format!("parallel(max {}) for {} in {} {{\n{}{}}}{}{}", max.as_ref().map(pe).unwrap_or_default(), var, pe(list), pb(body, ind + 1), pad, collect.as_ref().map(|(n, e)| format!(" collect {} <- {}", n, pe(e))).unwrap_or_default(), pof(on_fail, ind)),
                    ParallelBody::Tasks(ts) => format!("parallel tasks {}{}", ts.len(), pof(on_fail, ind)),
                },
                StmtKind::Fail { message, code } => format!("fail {}{}", pe(message), code.as_ref().map(|c| format!(" code {}", pe(c))).unwrap_or_default()),
                StmtKind::Return(v) => format!("return {}", v.as_ref().map(pe).unwrap_or_default()),
                StmtKind::Break => "break".into(),
                StmtKind::Continue => "continue".into(),
                StmtKind::Exit(v) => format!("exit {}", v.as_ref().map(pe).unwrap_or_default()),
                StmtKind::Defer(i) => format!("defer {}", pb(&Block { stmts: vec![(**i).clone()] }, 0).trim()),
                StmtKind::Native { .. } => "native".into(),
                StmtKind::Assert { cond, text } => format!("assert {}  // {}", pe(cond), text),
                StmtKind::AssertFails(i) => format!("assert_fails {}", pb(&Block { stmts: vec![(**i).clone()] }, 0).trim()),
            };
            s.push_str(&format!("{}{:>3}| {}\n", pad, st.line, line));
        }
        s
    }
    pub fn pseudo(p: &Program) -> String {
        let mut s = format!("deps {:?}\n", p.deps);
        for f in &p.funcs {
            s.push_str(&format!("fn {}({}) -> {} fallible={} {{\n{}}}\n", f.name, f.params.iter().map(|x| format!("{}: {}", x.name, x.ty.display())).collect::<Vec<_>>().join(", "), f.ret.display(), f.fallible, pb(&f.body, 1)));
        }
        s.push_str(&pb(&p.body, 0));
        for t in &p.tests {
            s.push_str(&format!("test {:?} {{\n{}}}\n", t.name, pb(&t.body, 1)));
        }
        s
    }

    /// `NALU_PSEUDO=deploy:windows cargo test --lib show_pseudo -- --nocapture`
    #[test]
    fn show_pseudo() {
        let Ok(v) = std::env::var("NALU_PSEUDO") else { return };
        let (f, t) = v.split_once(':').unwrap_or((&v, "linux"));
        let path = if f.ends_with(".nalu") { f.to_string() } else { format!("{}/examples/{}.nalu", env!("CARGO_MANIFEST_DIR"), f) };
        let src = std::fs::read_to_string(&path).unwrap();
        let root = std::path::Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let c = compile(&src, &path, &root, Target::parse(t).unwrap());
        for d in &c.diags { eprintln!("{}", d.render(&src, &path)); }
        if let Some(p) = c.program {
            println!("{}", pseudo(&p));
            for pr in validate(&p) { println!("PROBLEMA: {}", pr); }
        }
    }

    /// Varied valid programs: they compile without errors on all three targets and the IR follows the rules.
    #[test]
    fn valid_programs() {
        let progs = [
            "let x = fs.read(\"a\") catch {\n    fail err\n}\nprint(x)",
            "fn f(n: int) -> int {\n    if n <= 1 { return 1 }\n    return n * f(n - 1)\n}\nprint(f(5))",
            "let xs = [1, 2, 3]\nfor i in 0..xs.len() {\n    print(xs[i])\n}",
            "let a = 1\nif a == 1 {\n    print(1)\n} else if a == 2 {\n    print(2)\n} else {\n    print(3)\n}",
            "let n = 7\nlet t = match n {\n    1 => \"um\"\n    _ => {\n        log.warn(\"outro\")\n        \"muitos\"\n    }\n}\nprint(t)",
            "let m = {\n    \"a\": 1,\n    \"b\": 2,\n}\nprint(m.len(), m[\"a\"] catch 0)",
            "var xs: []int = []\nxs.push(1)\nlet u = xs.pop() catch 0\nprint(u, xs.len())",
            "let s = \"a \\\"b\\\"\"\nprint(s)\nprint()",
            "fn ok() -> bool {\n    return \"1\".to_int() == 1 catch false\n}\nif ok() {\n    print(\"sim\")\n}",
            "let id = \"abc\"\ndefer $ docker rm -f {id}\nexit()",
            "var total = 0\nfn somar(n: int) {\n    total += n\n}\nsomar(2)\nprint(total)",
            "let x = -5\nlet y = -x\nprint(x % 2 == 0, y, \"a\" < \"b\")",
            "let p: path = \"/tmp/a/b.txt\"\nlet q = p.parent / \"x\"\nprint(q, fs.read(dirs.home() / \"x\") catch \"\")",
            "let xs = [\"a\"]\nprint(\"{xs[0]} {xs.len()}\")",
            "enum Cor { verde, azul }\nprint(\"{Cor.verde}\", \"{5}\", \"x\".len(), true && !false)",
            "fn f() {}\nf()\nif true { // comentário\n    print(1)\n}",
            "test \"cmd falha\" {\n    assert_fails($ false)\n    assert 1 + 1 == 2\n}",
            "fn f(xs: []str) -> []int {\n    let r = parallel for x in xs {\n        x.len()\n    }\n    return r\n}\nprint(f([\"a\"]).len())",
            "let d = 30s\nif d > 10s && d / 1s < 100 {\n    time.sleep(d)\n}",
            "let ok = $(git status) catch \"\"\nlet n = if ok == \"\" { 0 } else { ok.lines().len() }\nprint(n)",
            "struct V {\n    a: int\n    fn mais(n: int = 1) -> V {\n        return V { ..self, a: self.a + n }\n    }\n}\nlet v = V { a: 1 }.mais().mais(n: 2)\nprint(v.a)",
            "let xs = [3, 1]\nlet ys = xs\n    .sort()\n    .reverse()\n    .map(x => x * 2)\nprint(ys.join(\",\"), ys.any(y => y > 5), ys.all(y => y > 0))",
            "let r = retry_n()\nfn retry_n() -> int {\n    var n = 0\n    retry(3) {\n        n += 1\n        if n < 2 { fail \"de novo\" }\n    }\n    return n\n}\nprint(r)",
            "let t = try_it()\nfn try_it() -> str {\n    try {\n        $ ls\n        return \"ok\"\n    } catch {\n        return \"falhou: {err}\"\n    }\n    return \"?\"\n}\nprint(t)",
            "let x = env.get(\"A\") catch env.get(\"B\") catch fail \"sem A nem B\"\nprint(x)",
            "let usuarios = { \"ana\": 1 }\nfor nome, idade in usuarios {\n    if idade > 0 { continue }\n    print(nome)\n}",
            "fn f() -> int {\n    let x = match os {\n        \"windows\" => 1\n        _ => 2\n    }\n    return x\n}\nprint(f())",
            "let v = if os == \"windows\" { \"win\" } else { \"unix\" }\nprint(v)",
            "let lista = [\"b\", \"a\"].sort_by(x => x)\nprint(lista.first() catch \"\", lista.last() catch \"\", lista.unique().len())",
            "let tokens = fmt(\"{}-{}\", 1, true)\nlet h = fs.glob(\"*.txt\")\nprint(tokens, h.len())",
            "let n = \"5\".to_int() catch 0\nlet s = \"x\" + n + true\nprint(s)",
        ];
        for src in progs {
            for t in Target::ALL {
                let c = compile(src, "t.nalu", std::path::Path::new("."), t);
                let errs: Vec<String> = c.diags.iter().filter(|d| d.is_error()).map(|d| d.render(src, "t.nalu")).collect();
                assert!(errs.is_empty(), "{}\n{}", src, errs.join("\n"));
                let p = c.program.unwrap();
                let probs = validate(&p);
                assert!(probs.is_empty(), "{}\n{}", src, probs.join("\n"));
            }
        }
    }

    /// `tests/errors/*.nalu`: the first line gives the expected code (with all three targets together, as `check` does).
    #[test]
    fn error_corpus() {
        let dir = format!("{}/tests/errors", env!("CARGO_MANIFEST_DIR"));
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x == "nalu").unwrap_or(false)).collect();
        files.sort();
        assert!(files.len() >= 40, "only {} cases", files.len());
        let mut fails = Vec::new();
        for f in &files {
            let src = std::fs::read_to_string(f).unwrap();
            let first = src.lines().next().unwrap_or("");
            let (want_err, code) = match (first.strip_prefix("// error:"), first.strip_prefix("// warning:")) {
                (Some(c), _) => (true, c.trim()),
                (_, Some(c)) => (false, c.trim()),
                _ => {
                    fails.push(format!("{}: first line is invalid", f.display()));
                    continue;
                }
            };
            let mut all = Vec::new();
            for t in Target::ALL {
                all.extend(compile(&src, "x.nalu", std::path::Path::new(&dir), t).diags);
            }
            let has_err = all.iter().any(|d| d.is_error());
            if want_err != has_err || !all.iter().any(|d| d.code == code) {
                fails.push(format!("{}: expected {} ({}), got {:?}", f.display(), code, if want_err { "error" } else { "only a warning" }, all.iter().map(|d| (d.code, d.message.clone())).collect::<Vec<_>>()));
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    /// Random (deterministic) mutations of the examples: there must never be a panic (E999).
    #[test]
    fn fuzz_mutations() {
        let iters: usize = std::env::var("NALU_FUZZ").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
        let pieces = ["{", "}", "(", ")", "[", "]", "\"", "'", "$ ", "$(", "|", "\n", " or ", "=>", "..", ".", ",", ":", "{x}", "\"{", "fail ", "match ", "if ", "let ", "fn ", "1s", "0x", "///", "/*", "é", "\\", "? ", "??", "++", "or {", "err", "self", "os == \"x\"", "parallel ", "retry(1) "];
        let mut seed: u64 = 0x2545F4914F6CDD1D;
        let mut rnd = move |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % (n.max(1) as u64)) as usize
        };
        for f in ["hello", "backup", "deploy", "installer", "release", "release_test"] {
            let base = std::fs::read_to_string(format!("{}/examples/{}.nalu", env!("CARGO_MANIFEST_DIR"), f)).unwrap();
            let chars: Vec<char> = base.chars().collect();
            for _ in 0..iters {
                let mut c = chars.clone();
                for _ in 0..1 + rnd(4) {
                    let at = rnd(c.len() + 1);
                    match rnd(3) {
                        0 => {
                            let len = rnd(20).min(c.len() - at.min(c.len()));
                            c.drain(at..at + len);
                        }
                        1 => {
                            let p: Vec<char> = pieces[rnd(pieces.len())].chars().collect();
                            for (k, ch) in p.into_iter().enumerate() {
                                c.insert(at + k, ch);
                            }
                        }
                        _ => {
                            let from = rnd(c.len());
                            let len = rnd(40).min(c.len() - from);
                            let seg: Vec<char> = c[from..from + len].to_vec();
                            for (k, ch) in seg.into_iter().enumerate() {
                                c.insert(at.min(c.len()) + k, ch);
                            }
                        }
                    }
                }
                let src: String = c.into_iter().collect();
                let t = Target::ALL[rnd(3)];
                let out = compile(&src, "f.nalu", std::path::Path::new("."), t);
                assert!(!out.diags.iter().any(|d| d.code == "E999"), "panic with:\n{}", src);
                if let Some(p) = &out.program {
                    let probs = validate(p);
                    assert!(probs.is_empty(), "invalid IR ({:?}) for:\n{}\n{}", t, src, probs.join("\n"));
                }
            }
        }
    }

    #[test]
    fn fuzz_no_panic() {
        let base = std::fs::read_to_string(format!("{}/examples/deploy.nalu", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let bytes = base.as_bytes();
        // truncations at several points (on character boundaries)
        let mut i = 0;
        while i < bytes.len() {
            if base.is_char_boundary(i) {
                let _ = comp(&base[..i], Target::Linux);
            }
            i += 37;
        }
        for s in ["{", "fn f() {", "\"abc", "let x = $(", "match x {", "\u{0}\u{1}", "if $ x {", "parallel for x in y {", "x = = 1"] {
            let _ = comp(s, Target::Windows);
        }
    }
}
