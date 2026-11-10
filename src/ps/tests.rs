//! Tests for the PowerShell backend with hand-built IR.
//!
//! Every generated script goes through the minimal 5.1 syntax checker ([`super::check_syntax`]). When
//! PowerShell is available (`NALU_PWSH`, or `pwsh`/`powershell` on PATH), the scripts also **run** and the output is
//! compared; without PowerShell, that part is skipped with a warning on stderr.

use super::runtime;
use super::*;
use crate::ir::*;
use crate::stdlib::{BUILTINS, METHODS, PATH_PROPS, STDLIB};
use crate::target::{Mode, Target};
use std::path::PathBuf;
use std::process::{Command as PCommand, Stdio};

// ---------------------------------------------------------------------------------------------
// Short constructors
// ---------------------------------------------------------------------------------------------

fn st(kind: StmtKind) -> Stmt {
    Stmt::new(0, kind)
}
fn b(stmts: Vec<Stmt>) -> Block {
    Block { stmts }
}
fn s(x: &str) -> Expr {
    Expr::str(x)
}
fn i(n: i64) -> Expr {
    Expr::int(n)
}
fn v(n: &str, t: Ty) -> Expr {
    Expr::var(n, t)
}
fn dur(ms: i64) -> Expr {
    Expr { kind: ExprKind::Duration(ms), ty: Ty::Duration }
}
fn bin(op: BinOp, a: Expr, c: Expr, t: Ty) -> Expr {
    Expr { kind: ExprKind::Binary(op, Box::new(a), Box::new(c)), ty: t }
}
fn interp(parts: Vec<Part>) -> Expr {
    Expr { kind: ExprKind::Interp(parts), ty: Ty::Str }
}
fn pl(x: &str) -> Part {
    Part::Lit(x.into())
}
fn pe(e: Expr) -> Part {
    Part::Expr(e)
}
fn field(base: Expr, f: &str, t: Ty) -> Expr {
    Expr { kind: ExprKind::Field(Box::new(base), f.into()), ty: t }
}
fn index(l: Expr, ix: Expr, t: Ty) -> Expr {
    Expr { kind: ExprKind::Index(Box::new(l), Box::new(ix)), ty: t }
}
fn len(x: Expr) -> Expr {
    Expr { kind: ExprKind::Len(Box::new(x)), ty: Ty::Int }
}
fn err_field(f: &str, t: Ty) -> Expr {
    field(Expr { kind: ExprKind::CurrentErr, ty: Ty::Error }, f, t)
}
fn ea(e: Expr) -> Arg {
    Arg::Expr(e)
}
fn call(target: CallTarget, args: Vec<Arg>, ret: Ty, fallible: bool) -> Call {
    Call { target, args, ret, fallible }
}
fn builtin(name: &str, args: Vec<Arg>, ret: Ty) -> Call {
    call(CallTarget::Builtin(name.into()), args, ret, false)
}
fn stdf(m: &str, n: &str, args: Vec<Arg>, ret: Ty, fallible: bool) -> Call {
    call(CallTarget::Std { module: m.into(), name: n.into() }, args, ret, fallible)
}
fn meth(recv: Ty, n: &str, args: Vec<Arg>, ret: Ty, fallible: bool) -> Call {
    call(CallTarget::Method { recv_ty: recv, name: n.into() }, args, ret, fallible)
}
fn user(n: &str, args: Vec<Arg>, ret: Ty, fallible: bool) -> Call {
    call(CallTarget::User(n.into()), args, ret, fallible)
}
fn print(args: Vec<Expr>) -> Stmt {
    st(StmtKind::Call { call: builtin("print", args.into_iter().map(ea).collect(), Ty::Unit), on_fail: OnFail::Propagate })
}
fn let_(n: &str, t: Ty, rv: Rvalue) -> Stmt {
    st(StmtKind::Let { var: n.into(), ty: t, mutable: true, value: rv })
}
fn let_e(n: &str, e: Expr) -> Stmt {
    let t = e.ty.clone();
    let_(n, t, Rvalue::Expr(e))
}
fn let_call(n: &str, c: Call, on_fail: OnFail) -> Stmt {
    let t = c.ret.clone();
    let_(n, t, Rvalue::Call { call: c, on_fail })
}
fn assign(n: &str, e: Expr) -> Stmt {
    st(StmtKind::Assign { target: LValue::Var(n.into()), value: Rvalue::Expr(e) })
}
fn list(elem: Ty, items: Vec<Expr>) -> Rvalue {
    Rvalue::List { elem_ty: elem, items }
}
fn lambda(p: &str, pt: Ty, ret: Ty, body: Vec<Stmt>) -> Arg {
    Arg::Lambda(Lambda { params: vec![FParam { name: p.into(), ty: pt }], ret, body: b(body) })
}
fn ret(e: Expr) -> Stmt {
    st(StmtKind::Return(Some(e)))
}
fn fail(msg: Expr, code: Option<i64>) -> Stmt {
    st(StmtKind::Fail { message: msg, code: code.map(i) })
}
fn func(name: &str, params: Vec<(&str, Ty)>, ret: Ty, body: Vec<Stmt>, fallible: bool) -> Func {
    Func { name: name.into(), params: params.into_iter().map(|(n, t)| FParam { name: n.into(), ty: t }).collect(), ret, body: b(body), fallible, doc: String::new() }
}
fn lit_words(ws: &[&str]) -> SimpleCmd {
    SimpleCmd { words: ws.iter().map(|w| Word::Lit(w.to_string())).collect() }
}
fn command(pipeline: Vec<SimpleCmd>, text: &str) -> Command {
    Command { pipeline, redirects: vec![], stdin: None, text: text.into() }
}
fn prog(body: Vec<Stmt>) -> Program {
    let mut p = Program::empty("t", Target::Windows);
    p.body = b(body);
    p
}

// ---------------------------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------------------------

fn pwsh() -> Option<String> {
    if let Ok(p) = std::env::var("NALU_PWSH") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    ["pwsh", "powershell"]
        .into_iter()
        .find(|c| PCommand::new(c).args(["-NoProfile", "-Command", "exit 0"]).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false))
        .map(String::from)
}

fn tmpdir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("nalu-ps-{}-{}-{}", std::process::id(), tag, N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The whole runtime goes into the generated scripts, which must be ASCII (no BOM, for `irm … | iex`).
#[test]
fn runtime_powershell_is_ascii() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ps");
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "ps1").unwrap_or(false) && p.file_name().map(|n| n != "tests.ps1").unwrap_or(false) {
            let t = std::fs::read_to_string(&p).unwrap();
            assert!(t.is_ascii(), "{} has non-ASCII characters", p.display());
        }
    }
}

/// As `nalu build` writes it: CRLF, and a BOM only if there is anything outside ASCII.
fn write_ps1(path: &std::path::Path, text: &str) {
    let mut bytes = if text.is_ascii() { vec![] } else { vec![0xEF, 0xBB, 0xBF] };
    bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
    std::fs::write(path, bytes).unwrap();
}

struct Ran {
    out: String,
    err: String,
    code: i32,
}

/// Generates, validates the syntax and (if PowerShell is available) runs. `None` = no PowerShell.
fn run_mode(p: &Program, mode: Mode, args: &[&str], env: &[(&str, &str)], stdin: Option<&str>) -> Option<Ran> {
    let text = generate(p, mode);
    let problems = check_syntax(&text);
    assert!(problems.is_empty(), "5.1 syntax refused:\n{}\n---\n{}", problems.join("\n"), numbered(&text));
    assert_no_ps7(&text);
    let ps = match pwsh() {
        Some(ps) => ps,
        None => {
            eprintln!("warning: no PowerShell (NALU_PWSH); only the syntax was checked");
            return None;
        }
    };
    let dir = tmpdir("run");
    let f = dir.join("t.windows.ps1");
    write_ps1(&f, &text);
        // the official parser, refusing PowerShell 7 syntax
    let syn = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ps_syntax.ps1");
    let o = PCommand::new(&ps).args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&syn).arg(&f).output().unwrap();
    assert!(o.status.success(), "tests/ps_syntax.ps1 refused:\n{}{}\n---\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr), numbered(&text));
    let mut c = PCommand::new(ps);
    c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&f).args(args).current_dir(&dir);
    for (k, val) in env {
        c.env(k, val);
    }
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    if let Some(data) = stdin {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(data.as_bytes()).unwrap();
    }
    let o = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Some(Ran {
        out: String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n"),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    })
}

fn numbered(t: &str) -> String {
    t.lines().enumerate().map(|(i, l)| format!("{:4} {}", i + 1, l)).collect::<Vec<_>>().join("\n")
}

fn expect(p: &Program, args: &[&str], out: &str, code: i32) -> Option<Ran> {
    let r = run_mode(p, Mode::Script, args, &[], None)?;
    assert_eq!(
        (r.out.as_str(), r.code),
        (out, code),
        "\n--- stderr ---\n{}\n--- script ---\n{}",
        r.err,
        numbered(&generate(p, Mode::Script))
    );
    Some(r)
}

fn assert_no_ps7(text: &str) {
    let p = check_syntax(text);
    assert!(p.is_empty(), "{:?}", p);
}

// ---------------------------------------------------------------------------------------------
// Text tests (without PowerShell)
// ---------------------------------------------------------------------------------------------

#[test]
fn launcher_contract() {
    assert_eq!(launcher("x"), "@echo off\r\npowershell -NoProfile -ExecutionPolicy Bypass -File \"%~dp0x.windows.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n");
}

#[test]
fn literals_escape_typographic_quotes() {
        // outside ASCII it becomes `$([char]0x…)` (the `.ps1` is ASCII: no BOM, works with `irm … | iex`)
    assert_eq!(lit("it's"), "'it''s'");
    assert_eq!(lit("it’s"), "\"it$([char]0x2019)s\"");
    assert_eq!(lit("x\n“y” $z `w"), "\"x`n$([char]0x201c)y$([char]0x201d) `$z ``w\"");
    assert_eq!(lit("é👍"), "\"$([char]0x00e9)$([char]::ConvertFromUtf32(0x1f44d))\"");
    assert!(lit("‘ ’ “ ” „ ‚ ‛ ç").is_ascii());
    assert!(check_syntax(&format!("$a = {}", lit("‘ ’ “ ” „ ‚ ‛ ' \""))).is_empty());
    assert!(check_syntax(&format!("$a = {}", lit("“\n”\"$(x)`"))).is_empty());
}

#[test]
fn names_without_case_collisions() {
    assert_ne!(mangle("a"), mangle("A"));
    assert_eq!(mangle("limpar_cache"), "o_limpar_cache");
    assert_eq!(mangle("Versao.proxima"), "o__v_ersao__proxima");
}

#[test]
fn runtime_without_ps7_and_consistent_needs() {
    let hs = runtime::helpers();
    assert!(hs.len() > 40);
    for (file, src) in runtime::FILES {
        let p = check_syntax(src);
        assert!(p.is_empty(), "{}: {:?}", file, p);
        for bad in ["??", "?.", "&&", "||", "-Parallel", "Join-String"] {
            for (n, l) in src.lines().enumerate() {
                let code = l.split(" #").next().unwrap_or("");
                if code.trim_start().starts_with('#') {
                    continue;
                }
                                // outside simple strings: close enough approximation for the runtime
                let stripped: String = code.split('\'').step_by(2).collect();
                assert!(!stripped.contains(bad), "{}:{}: `{}` is PowerShell 7 only: {}", file, n + 1, bad, l);
            }
        }
    }
        // every helper mentioned in another's body must be in its @needs
    for h in &hs {
        for o in &hs {
            if o.name != h.name && runtime::mentions(&h.text, &o.name) {
                assert!(h.needs.contains(&o.name), "{} uses {} without `# @needs {}`", h.name, o.name, o.name);
            }
        }
        for n in &h.needs {
            assert!(hs.iter().any(|x| &x.name == n), "{} needs {}, which does not exist", h.name, n);
        }
    }
        // unique names
    for (k, a) in hs.iter().enumerate() {
        for c in &hs[k + 1..] {
            assert_ne!(a.name, c.name, "duplicate helper");
        }
    }
}

#[test]
fn only_includes_used_helpers() {
    let p = prog(vec![print(vec![s("oi")])]);
    let t = generate(&p, Mode::Script);
    assert!(!t.contains("function __FsRead"), "unused helper included");
    assert!(t.contains("function __ErrOf"));
    assert!(t.contains("function __ParseArgs"));
    assert!(t.starts_with("# Generated by nalu"));
    assert!(t.lines().count() < 400);
}

/// Every `v1` entry of STDLIB, METHODS, PATH_PROPS and BUILTINS has a translation (no `__Unimplemented`).
#[test]
fn stdlib_coverage() {
    fn ty_of(t: &str) -> Ty {
        match t {
            "str" | "any" | "..." => Ty::Str,
            "int" => Ty::Int,
            "bool" => Ty::Bool,
            "path" => Ty::Path,
            "duration" => Ty::Duration,
            "secret" => Ty::Secret,
            "[]str" => Ty::list(Ty::Str),
            "[]path" => Ty::list(Ty::Path),
            "unit" => Ty::Unit,
            _ => Ty::Str,
        }
    }
    let mut missing = Vec::new();
    let mut check = |what: String, c: Call| {
        let mut p = prog(vec![]);
        p.enums.push(EnumDef { name: "E".into(), cases: vec!["a".into(), "b".into()] });
        let stmt = if c.ret == Ty::Unit {
            st(StmtKind::Call { call: c, on_fail: OnFail::Ignore })
        } else {
            let_call("x", c, OnFail::Propagate)
        };
        p.body = b(vec![stmt]);
        let t = generate(&p, Mode::Script);
        let main = t.split("# ---- programa ----").nth(1).unwrap_or("");
        if main.contains("__Unimplemented") {
            missing.push(what.clone());
        }
        let probs = check_syntax(&t);
        assert!(probs.is_empty(), "{}: {:?}", what, probs);
    };
    for f in STDLIB.iter().filter(|f| f.v1) {
        let args = f.params.iter().map(|p| ea(Expr { kind: ExprKind::Var("a".into()), ty: ty_of(p.ty) })).collect();
        check(format!("{}.{}", f.module, f.name), stdf(f.module, f.name, args, ty_of(f.ret), f.fallible));
    }
    for f in BUILTINS.iter().filter(|f| f.v1) {
        let ret = if f.name == "enum_parse" { Ty::Enum("E".into()) } else { ty_of(f.ret) };
        let args = f.params.iter().map(|p| ea(Expr { kind: ExprKind::Var("a".into()), ty: ty_of(p.ty) })).collect();
        check(format!("builtin {}", f.name), call(CallTarget::Builtin(f.name.into()), args, ret, f.fallible));
    }
    for m in METHODS {
        let recv = match m.recv {
            "str" => Ty::Str,
            "list" if matches!(m.name, "sum" | "sort") => Ty::list(Ty::Int),
            "list" => Ty::list(Ty::Str),
            "map" => Ty::map(Ty::Str, Ty::Int),
            "enum" => Ty::Enum("E".into()),
            "int" => Ty::Int,
            "bool" => Ty::Bool,
            _ => Ty::Duration,
        };
        let mut args = vec![ea(v("r", recv.clone()))];
        for p in m.params {
            if p.ty.starts_with('(') {
                args.push(lambda("x", Ty::Str, Ty::Bool, vec![ret(Expr::bool(true))]));
            } else {
                args.push(ea(Expr { kind: ExprKind::Var("a".into()), ty: ty_of(p.ty) }));
            }
        }
        let rt = if m.ret == "Enum" { Ty::Enum("E".into()) } else { ty_of(m.ret) };
        check(format!("{}.{}", m.recv, m.name), meth(recv, m.name, args, rt, m.fallible));
    }
    for (name, ret_ty, _) in PATH_PROPS {
        check(format!("path.{}", name), meth(Ty::Path, name, vec![ea(v("r", Ty::Path))], ty_of(ret_ty), false));
    }
    assert!(missing.is_empty(), "no PowerShell implementation: {:?}", missing);
}

// ---------------------------------------------------------------------------------------------
// Execution (with PowerShell)
// ---------------------------------------------------------------------------------------------

#[test]
fn print_interp_and_arithmetic() {
    let p = prog(vec![
        let_e("n", i(7)),
        print(vec![s("a"), i(1), Expr::bool(true), Expr::bool(false)]),
        print(vec![interp(vec![pl("n="), pe(v("n", Ty::Int)), pl(" b="), pe(Expr::bool(true)), pl(" {x}")])]),
        print(vec![
            bin(BinOp::Div, i(-7), i(2), Ty::Int),
            bin(BinOp::Mod, i(-7), i(2), Ty::Int),
            bin(BinOp::Div, i(9007199254740995), i(2), Ty::Int),
            bin(BinOp::Mul, v("n", Ty::Int), i(3), Ty::Int),
        ]),
        print(vec![bin(BinOp::Lt, s("a"), s("B"), Ty::Bool), bin(BinOp::Lt, s("B"), s("a"), Ty::Bool), bin(BinOp::Eq, s("é"), s("e\u{301}"), Ty::Bool)]),
        print(vec![len(s("ação")), len(s("👍ok"))]),
        print(vec![s("it’s “ok” ‚x‛ $HOME `t")]),
        print(vec![s("linha\ncom\ttab")]),
        print(vec![]),
        print(vec![dur(90_000), dur(250), dur(1500), dur(93_600_000), dur(0)]),
    ]);
    expect(
        &p,
        &[],
        "a 1 true false\nn=7 b=true {x}\n-3 -1 4503599627370497 21\nfalse true false\n4 3\nit’s “ok” ‚x‛ $HOME `t\nlinha\ncom\ttab\n\n1m30s 250ms 1.5s 1d2h 0s\n",
        0,
    );
}

#[test]
fn strings_by_code_point() {
    let st_ = |n: &str, args: Vec<Expr>, r: Ty| -> Call { meth(Ty::Str, n, args.into_iter().map(ea).collect(), r, false) };
    let p = prog(vec![
        let_call("a", st_("slice", vec![s("👍👎x"), i(1), i(2)], Ty::Str), OnFail::Propagate),
        let_call("b", st_("index_of", vec![s("👍x"), s("x")], Ty::Int), OnFail::Propagate),
        let_call("c", st_("chars", vec![s("a👍b")], Ty::list(Ty::Str)), OnFail::Propagate),
        let_call("d", meth(Ty::list(Ty::Str), "join", vec![ea(v("c", Ty::list(Ty::Str))), ea(s("|"))], Ty::Str, false), OnFail::Propagate),
        let_call("e", st_("pad_left", vec![s("é"), i(3), s("*")], Ty::Str), OnFail::Propagate),
        let_call("f", st_("upper", vec![s("ação")], Ty::Str), OnFail::Propagate),
        let_call("g", st_("split", vec![s("x"), s(",")], Ty::list(Ty::Str)), OnFail::Propagate),
        let_call("h", st_("lines", vec![s("a\r\nb\n")], Ty::list(Ty::Str)), OnFail::Propagate),
        let_call("k", st_("to_int", vec![s(" -42 ")], Ty::Int), OnFail::Propagate),
        let_call("m", st_("to_int", vec![s("+5")], Ty::Int), OnFail::Default(Box::new(Rvalue::Expr(i(-3))))),
        let_call("n", st_("to_int", vec![s("9223372036854775808")], Ty::Int), OnFail::Default(Box::new(Rvalue::Expr(i(-4))))),
        let_call("o", st_("replace", vec![s("aXbX"), s("X"), s("-")], Ty::Str), OnFail::Propagate),
        print(vec![v("a", Ty::Str), v("b", Ty::Int), v("d", Ty::Str), v("e", Ty::Str), v("f", Ty::Str), len(v("g", Ty::list(Ty::Str))), len(v("h", Ty::list(Ty::Str))), v("k", Ty::Int), v("m", Ty::Int), v("n", Ty::Int), v("o", Ty::Str)]),
    ]);
    expect(&p, &[], "👎 1 a|👍|b **é AÇÃO 1 2 -42 -3 -4 a-b-\n", 0);
}

#[test]
fn lists_lambdas_and_single_item() {
    let li = Ty::list(Ty::Int);
    let ls = Ty::list(Ty::Str);
    let mut p = prog(vec![
        let_("xs", li.clone(), list(Ty::Int, vec![i(3), i(1), i(2)])),
        st(StmtKind::Call { call: meth(li.clone(), "push", vec![ea(v("xs", li.clone())), ea(i(10))], Ty::Unit, false), on_fail: OnFail::Propagate }),
        let_call("f", meth(li.clone(), "filter", vec![ea(v("xs", li.clone())), lambda("x", Ty::Int, Ty::Bool, vec![ret(bin(BinOp::Gt, v("x", Ty::Int), v("lim", Ty::Int), Ty::Bool))])], li.clone(), false), OnFail::Propagate),
        let_call("srt", meth(li.clone(), "sort", vec![ea(v("xs", li.clone()))], li.clone(), false), OnFail::Propagate),
        let_call("j1", meth(li.clone(), "join", vec![ea(v("f", li.clone())), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("j2", meth(li.clone(), "join", vec![ea(v("srt", li.clone())), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("um", user("um", vec![], ls.clone(), false), OnFail::Propagate),
        let_call("nada", meth(li.clone(), "find", vec![ea(v("xs", li.clone())), lambda("x", Ty::Int, Ty::Bool, vec![ret(bin(BinOp::Gt, v("x", Ty::Int), i(100), Ty::Bool))])], Ty::Int, true), OnFail::Default(Box::new(Rvalue::Expr(i(-1))))),
        let_("ws", ls.clone(), list(Ty::Str, vec![s("b"), s("B"), s("a"), s("_"), s("A")])),
        let_call("ord", meth(ls.clone(), "sort", vec![ea(v("ws", ls.clone()))], ls.clone(), false), OnFail::Propagate),
        let_call("by", meth(ls.clone(), "sort_by", vec![ea(v("ws", ls.clone())), lambda("w", Ty::Str, Ty::Str, vec![let_call("lw", meth(Ty::Str, "lower", vec![ea(v("w", Ty::Str))], Ty::Str, false), OnFail::Propagate), ret(v("lw", Ty::Str))])], ls.clone(), false), OnFail::Propagate),
        let_call("j3", meth(ls.clone(), "join", vec![ea(v("ord", ls.clone())), ea(s(" "))], Ty::Str, false), OnFail::Propagate),
        let_call("j4", meth(ls.clone(), "join", vec![ea(v("by", ls.clone())), ea(s(" "))], Ty::Str, false), OnFail::Propagate),
        let_e("copia", v("xs", li.clone())),
        st(StmtKind::Call { call: meth(li.clone(), "push", vec![ea(v("copia", li.clone())), ea(i(99))], Ty::Unit, false), on_fail: OnFail::Propagate }),
        print(vec![v("j1", Ty::Str), v("j2", Ty::Str), len(v("um", ls.clone())), index(v("um", ls.clone()), i(0), Ty::Str), v("nada", Ty::Int), index(v("xs", li.clone()), i(99), Ty::Int)]),
        print(vec![v("j3", Ty::Str), v("j4", Ty::Str), len(v("xs", li.clone())), len(v("copia", li.clone()))]),
        st(StmtKind::ForList { var: "x".into(), index: Some("k".into()), list: v("um", ls.clone()), body: b(vec![print(vec![v("k", Ty::Int), v("x", Ty::Str)])]) }),
    ]);
    p.body.stmts.insert(0, let_e("lim", i(1)));
    p.funcs.push(func("um", vec![], ls.clone(), vec![let_("r", ls.clone(), list(Ty::Str, vec![s("só")])), ret(v("r", ls.clone()))], false));
    expect(&p, &[], "3,2,10 1,2,3,10 1 só -1 0\nA B _ a b _ a A b B 4 5\n0 só\n", 0);
}

#[test]
fn maps_and_structs() {
    let m = Ty::map(Ty::Str, Ty::Int);
    let sv = Ty::Struct("S".into());
    let mut p = prog(vec![
        let_("m", m.clone(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![(s("api"), i(1)), (s("API"), i(2))] }),
        st(StmtKind::Assign { target: LValue::MapEntry { map: "m".into(), key: s("web") }, value: Rvalue::Expr(i(3)) }),
        st(StmtKind::Call { call: meth(m.clone(), "remove", vec![ea(v("m", m.clone())), ea(s("api"))], Ty::Unit, false), on_fail: OnFail::Propagate }),
        let_call("k", meth(m.clone(), "keys", vec![ea(v("m", m.clone()))], Ty::list(Ty::Str), false), OnFail::Propagate),
        let_call("kj", meth(Ty::list(Ty::Str), "join", vec![ea(v("k", Ty::list(Ty::Str))), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("g", meth(m.clone(), "get", vec![ea(v("m", m.clone())), ea(s("x"))], Ty::Int, true), OnFail::Default(Box::new(Rvalue::Expr(i(0))))),
        st(StmtKind::ForMap { key: "kk".into(), value: "vv".into(), map: v("m", m.clone()), body: b(vec![print(vec![interp(vec![pe(v("kk", Ty::Str)), pl("="), pe(v("vv", Ty::Int))])])]) }),
        let_("a", sv.clone(), Rvalue::Struct { ty: "S".into(), spread: None, fields: vec![(s_name("nome"), s("api")), (s_name("porta"), i(80))] }),
        let_("c", sv.clone(), Rvalue::Struct { ty: "S".into(), spread: Some(v("a", sv.clone())), fields: vec![(s_name("porta"), i(81))] }),
        let_e("d", v("a", sv.clone())),
        st(StmtKind::Assign { target: LValue::Field { base: "d".into(), field: "porta".into() }, value: Rvalue::Expr(i(1)) }),
        let_call("u", user("S.url", vec![ea(v("c", sv.clone()))], Ty::Str, false), OnFail::Propagate),
        print(vec![v("kj", Ty::Str), v("g", Ty::Int), field(v("a", sv.clone()), "porta", Ty::Int), field(v("c", sv.clone()), "porta", Ty::Int), field(v("d", sv.clone()), "porta", Ty::Int), v("u", Ty::Str)]),
    ]);
    p.structs.push(StructDef { name: "S".into(), fields: vec![Field { name: "nome".into(), ty: Ty::Str }, Field { name: "porta".into(), ty: Ty::Int }] });
    p.funcs.push(func("S.url", vec![("self", sv.clone())], Ty::Str, vec![ret(interp(vec![pl("http://x:"), pe(field(v("self", sv.clone()), "porta", Ty::Int))]))], false));
    expect(&p, &[], "API=2\nweb=3\nAPI,web 0 80 81 1 http://x:81\n", 0);
}

fn s_name(n: &str) -> String {
    n.to_string()
}

#[test]
fn errors_onfail_try_and_code() {
    let mut p = prog(vec![
        let_call("a", user("div", vec![ea(i(10)), ea(i(2))], Ty::Int, true), OnFail::Default(Box::new(Rvalue::Expr(i(0))))),
        let_call("b", user("div", vec![ea(i(1)), ea(i(0))], Ty::Int, true), OnFail::Default(Box::new(Rvalue::Expr(i(-1))))),
        let_call(
            "c",
            user("div", vec![ea(i(1)), ea(i(0))], Ty::Int, true),
            OnFail::Block { err: Some("err".into()), body: b(vec![print(vec![interp(vec![pl("tratado: "), pe(field(v("err", Ty::Error), "message", Ty::Str)), pl(" ("), pe(field(v("err", Ty::Error), "code", Ty::Int)), pl(")")])])]), value: Some(i(0)) },
        ),
        print(vec![v("a", Ty::Int), v("b", Ty::Int), v("c", Ty::Int)]),
        st(StmtKind::Call { call: user("avisa", vec![], Ty::Unit, true), on_fail: OnFail::Ignore }),
        st(StmtKind::Try {
            body: b(vec![print(vec![s("antes")]), st(StmtKind::Call { call: user("avisa", vec![], Ty::Unit, true), on_fail: OnFail::Propagate }), print(vec![s("nunca")])]),
            on_fail: OnFail::Block { err: None, body: b(vec![print(vec![interp(vec![pl("pegou: "), pe(err_field("message", Ty::Str))])])]), value: None },
        }),
        st(StmtKind::ForRange {
            var: "i".into(),
            from: i(0),
            to: i(5),
            inclusive: false,
            body: b(vec![
                let_call("r", user("div", vec![ea(v("i", Ty::Int)), ea(bin(BinOp::Sub, v("i", Ty::Int), i(2), Ty::Int))], Ty::Int, true), OnFail::Block { err: None, body: b(vec![print(vec![interp(vec![pl("parou em "), pe(v("i", Ty::Int))])]), st(StmtKind::Break)]), value: None }),
                print(vec![v("r", Ty::Int)]),
            ]),
        }),
        let_call("z", user("div", vec![ea(i(1)), ea(i(0))], Ty::Int, true), OnFail::Fail { message: interp(vec![pl("contexto: "), pe(err_field("message", Ty::Str))]), code: Some(i(4)) }),
        print(vec![s("nunca")]),
    ]);
    p.funcs.push(func("div", vec![("a", Ty::Int), ("b", Ty::Int)], Ty::Int, vec![st(StmtKind::If { cond: bin(BinOp::Eq, v("b", Ty::Int), i(0), Ty::Bool), then: b(vec![fail(s("divisão por zero"), Some(3))]), els: None }), ret(bin(BinOp::Div, v("a", Ty::Int), v("b", Ty::Int), Ty::Int))], true));
    p.funcs.push(func("avisa", vec![], Ty::Unit, vec![fail(s("falhou aviso"), None)], true));
    if let Some(r) = expect(&p, &[], "tratado: divisão por zero (3)\n5 -1 0\nantes\npegou: falhou aviso\n0\n-1\nparou em 2\n", 4) {
        assert!(r.err.contains("error: contexto: divisão por zero"), "{}", r.err);
    }
}

#[test]
fn defer_and_exit_inside_function() {
    let lim = |x: Expr| st(StmtKind::Defer(Box::new(st(StmtKind::Call { call: user("limpar", vec![ea(x)], Ty::Unit, false), on_fail: OnFail::Ignore }))));
    let mut p = prog(vec![
        st(StmtKind::Call { call: user("trabalho", vec![], Ty::Unit, false), on_fail: OnFail::Propagate }),
        let_e("v", s("v1")),
        lim(v("v", Ty::Str)),
        assign("v", s("v2")),
        lim(s("topo")),
        st(StmtKind::Call { call: user("sai", vec![], Ty::Unit, false), on_fail: OnFail::Propagate }),
        print(vec![s("nunca")]),
    ]);
    p.funcs.push(func("limpar", vec![("s", Ty::Str)], Ty::Unit, vec![print(vec![interp(vec![pl("limpar "), pe(v("s", Ty::Str))])])], false));
    p.funcs.push(func("trabalho", vec![], Ty::Unit, vec![lim(s("primeiro")), lim(s("segundo")), print(vec![s("trabalhando")])], false));
    p.funcs.push(func("sai", vec![], Ty::Unit, vec![lim(s("função")), st(StmtKind::Exit(Some(i(300))))], false));
    expect(&p, &[], "trabalhando\nlimpar segundo\nlimpar primeiro\nlimpar função\nlimpar topo\nlimpar v1\n", 44);
}

#[test]
fn retry_and_parallel() {
    let li = Ty::list(Ty::Int);
    let mut p = prog(vec![
        let_e("n", i(0)),
        st(StmtKind::Retry {
            times: i(3),
            every: dur(5),
            body: b(vec![assign("n", bin(BinOp::Add, v("n", Ty::Int), i(1), Ty::Int)), st(StmtKind::If { cond: bin(BinOp::Lt, v("n", Ty::Int), i(3), Ty::Bool), then: b(vec![fail(s("ainda não"), None)]), els: None })]),
            on_fail: OnFail::Propagate,
        }),
        st(StmtKind::Retry {
            times: i(2),
            every: dur(1),
            body: b(vec![fail(s("sempre"), None)]),
            on_fail: OnFail::Block { err: None, body: b(vec![print(vec![interp(vec![pl("desistiu: "), pe(err_field("message", Ty::Str))])])]), value: None },
        }),
        let_("xs", li.clone(), list(Ty::Int, vec![i(1), i(7), i(2), i(9)])),
        let_("r", li.clone(), list(Ty::Int, vec![])),
        st(StmtKind::Try {
            body: b(vec![st(StmtKind::Parallel {
                max: Some(i(2)),
                body: ParallelBody::For { var: "x".into(), list: v("xs", li.clone()), body: b(vec![let_call("t", user("dobro", vec![ea(v("x", Ty::Int))], Ty::Int, true), OnFail::Propagate)]), collect: Some(("r".into(), v("t", Ty::Int))) },
                on_fail: OnFail::Propagate,
            })]),
            on_fail: OnFail::Block { err: None, body: b(vec![print(vec![err_field("message", Ty::Str)])]), value: None },
        }),
        let_("ys", li.clone(), list(Ty::Int, vec![i(1), i(2)])),
        st(StmtKind::Parallel {
            max: None,
            body: ParallelBody::For { var: "y".into(), list: v("ys", li.clone()), body: b(vec![let_call("u", user("dobro", vec![ea(v("y", Ty::Int))], Ty::Int, true), OnFail::Propagate)]), collect: Some(("r".into(), v("u", Ty::Int))) },
            on_fail: OnFail::Propagate,
        }),
        let_call("rj", meth(li.clone(), "join", vec![ea(v("r", li.clone())), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        print(vec![v("n", Ty::Int), v("rj", Ty::Str)]),
    ]);
    p.funcs.push(func("dobro", vec![("n", Ty::Int)], Ty::Int, vec![st(StmtKind::If { cond: bin(BinOp::Gt, v("n", Ty::Int), i(5), Ty::Bool), then: b(vec![fail(interp(vec![pl("falhou "), pe(v("n", Ty::Int))]), None)]), els: None }), ret(bin(BinOp::Mul, v("n", Ty::Int), i(2), Ty::Int))], true));
    expect(&p, &[], "desistiu: sempre\nparallel: falhou 7; falhou 9\n3 2,4\n", 0);
}

#[test]
fn match_with_ranges_and_break_in_loop() {
    let arm = |pats: Vec<Pattern>, body: Vec<Stmt>| Arm { patterns: pats, body: b(body) };
    let p = prog(vec![st(StmtKind::ForRange {
        var: "x".into(),
        from: i(0),
        to: i(10),
        inclusive: true,
        body: b(vec![st(StmtKind::Match {
            subject: v("x", Ty::Int),
            arms: vec![
                arm(vec![Pattern::Range { from: i(0), to: i(2), inclusive: false }], vec![print(vec![s("baixo")])]),
                arm(vec![Pattern::Lit(i(2)), Pattern::Lit(i(3))], vec![st(StmtKind::Continue)]),
                arm(vec![Pattern::Range { from: i(4), to: i(5), inclusive: true }], vec![print(vec![s("meio")])]),
            ],
            default: Some(b(vec![st(StmtKind::Break)])),
        })]),
    })]);
    expect(&p, &[], "baixo\nbaixo\nmeio\nmeio\n", 0);
}

#[test]
fn parameters_and_help() {
    let mut p = prog(vec![
        print(vec![v("nome", Ty::Str), v("vezes", Ty::Int), v("forcar", Ty::Bool), v("amb", Ty::Enum("A".into()))]),
    ]);
    p.body.stmts.push(let_call("e", builtin("duration_str", vec![ea(v("espera", Ty::Duration))], Ty::Str), OnFail::Propagate));
    p.body.stmts.push(let_call("pj", meth(Ty::list(Ty::Str), "join", vec![ea(v("pastas", Ty::list(Ty::Str))), ea(s("|"))], Ty::Str, false), OnFail::Propagate));
    p.body.stmts.push(let_call("aj", meth(Ty::list(Ty::Str), "join", vec![ea(Expr { kind: ExprKind::Args, ty: Ty::list(Ty::Str) }), ea(s("|"))], Ty::Str, false), OnFail::Propagate));
    p.body.stmts.push(print(vec![v("e", Ty::Str), v("pj", Ty::Str), v("aj", Ty::Str), Expr { kind: ExprKind::DryRun, ty: Ty::Bool }]));
    p.enums.push(EnumDef { name: "A".into(), cases: vec!["dev".into(), "prod".into()] });
    let par = |n: &str, f: &str, t: Ty, d: Option<Rvalue>| Param { name: n.into(), flag: f.into(), ty: t, default: d, doc: String::new(), env_var: None };
    p.params = vec![
        par("nome", "--nome", Ty::Str, None),
        par("vezes", "--vezes", Ty::Int, Some(Rvalue::Expr(i(1)))),
        par("forcar", "--forcar", Ty::Bool, None),
        par("amb", "--amb", Ty::Enum("A".into()), Some(Rvalue::Expr(Expr { kind: ExprKind::EnumCase { enum_: "A".into(), case: "dev".into() }, ty: Ty::Enum("A".into()) }))),
        par("espera", "--espera", Ty::Duration, Some(Rvalue::Expr(dur(30_000)))),
        par("pastas", "--pastas", Ty::list(Ty::Str), Some(list(Ty::Str, vec![s("docs")]))),
    ];
    p.usage = "usage: t --nome <str> [options]\n\n  --nome <str>  quem".into();
    expect(&p, &["--nome", "Ana Maria", "--vezes", "3", "--forcar", "--amb=prod", "--espera", "1m30s", "--pastas", "a,b", "pos", "--dry-run", "--", "--x"], "Ana Maria 3 true prod\n1m30s a|b pos|--x true\n", 0);
    expect(&p, &["--help"], "usage: t --nome <str> [options]\n\n  --nome <str>  quem\n", 0);
    if let Some(r) = expect(&p, &[], "", 2) {
        assert!(r.err.contains("--nome") && r.err.contains("run with --help"), "{}", r.err);
    }
    expect(&p, &["--nome", "x", "--vezes", "3x"], "", 2);
    expect(&p, &["--nome", "x", "--amb", "stage"], "", 2);
    expect(&p, &["--nome", "x", "--nada"], "", 2);
}

#[test]
fn external_commands() {
    if cfg!(windows) {
        return;
    }
    let x = || v("x", Ty::Str);
    let words = |ws: Vec<Word>| SimpleCmd { words: ws };
    let mut c1 = command(vec![words(vec![Word::Lit("printf".into()), Word::Lit("[%s]\n".into()), Word::Interp(vec![pe(x())]), Word::Lit("".into()), Word::Lit("a\"b c".into()), Word::Lit("\\\"x\\".into())])], "printf \"[%s]\\n\" {x} \"\" 'a\"b c' '\\\"x\\'");
    c1.redirects = vec![];
    let mut c2 = command(vec![lit_words(&["tr", "a-z", "A-Z"])], "tr a-z A-Z");
    c2.stdin = Some(s("abc"));
    let c3 = command(vec![lit_words(&["echo", "abc", "def"]), lit_words(&["tr", "a-z", "A-Z"])], "echo abc def | tr a-z A-Z");
    let mut c4 = command(vec![lit_words(&["echo", "ação"])], "echo ação > f.txt");
    c4.redirects = vec![Redirect::Out { path: s("f.txt"), append: false }];
    let mut c5 = command(vec![lit_words(&["sh", "-c", "echo erro 1>&2; exit 3"])], "sh -c ...");
    c5.redirects = vec![Redirect::Err { path: s("e.txt"), append: false }];
    let c6 = command(vec![lit_words(&["sh", "-c", "exit 1"]), lit_words(&["cat"])], "sh -c 'exit 1' | cat");
    let cap = command(vec![lit_words(&["printf", "a\\nb\\n\\n"])], "printf ...");
    let p = prog(vec![
        let_e("x", s("c d; $HOME *")),
        st(StmtKind::Command { cmd: c1, on_fail: OnFail::Propagate }),
        st(StmtKind::Command { cmd: c2, on_fail: OnFail::Propagate }),
        print(vec![s("|")]),
        st(StmtKind::Command { cmd: c3, on_fail: OnFail::Propagate }),
        st(StmtKind::Command { cmd: c4, on_fail: OnFail::Propagate }),
        let_call("t", stdf("fs", "read", vec![ea(s("f.txt"))], Ty::Str, true), OnFail::Propagate),
        print(vec![len(v("t", Ty::Str)), Expr { kind: ExprKind::Len(Box::new(v("t", Ty::Str))), ty: Ty::Int }]),
        st(StmtKind::Command { cmd: c5, on_fail: OnFail::Block { err: Some("err".into()), body: b(vec![print(vec![interp(vec![pl("código "), pe(field(v("err", Ty::Error), "code", Ty::Int))])])]), value: None } }),
        let_call("et", stdf("fs", "read", vec![ea(s("e.txt"))], Ty::Str, true), OnFail::Propagate),
        print(vec![v("et", Ty::Str)]),
        let_(
            "c",
            Ty::Str,
            Rvalue::Capture { cmd: cap, on_fail: OnFail::Propagate },
        ),
        print(vec![interp(vec![pl("["), pe(v("c", Ty::Str)), pl("]")])]),
        let_("ok", Ty::Bool, Rvalue::CommandOk(command(vec![lit_words(&["test", "-d", "."])], "test -d ."))),
        let_("nao", Ty::Bool, Rvalue::CommandOk(command(vec![lit_words(&["programa-que-nao-existe-xyz"])], "programa-que-nao-existe-xyz"))),
        print(vec![v("ok", Ty::Bool), v("nao", Ty::Bool)]),
        st(StmtKind::Command { cmd: c6, on_fail: OnFail::Block { err: None, body: b(vec![print(vec![err_field("message", Ty::Str)])]), value: None } }),
        st(StmtKind::Command { cmd: command(vec![lit_words(&["programa-que-nao-existe-xyz"])], "programa-que-nao-existe-xyz"), on_fail: OnFail::Block { err: None, body: b(vec![print(vec![err_field("code", Ty::Int)])]), value: None } }),
        st(StmtKind::Command { cmd: command(vec![lit_words(&["sh", "-c", "exit 5"])], "sh -c 'exit 5'"), on_fail: OnFail::Propagate }),
    ]);
    if let Some(r) = expect(&p, &[], "[c d; $HOME *]\n[]\n[a\"b c]\n[\\\"x\\]\nABC|\nABC DEF\n5 5\ncódigo 3\nerro\n\n[a\nb]\ntrue false\ncommand `sh` failed with exit code 1\n127\n", 5) {
        assert!(r.err.contains("failed with exit code 5"), "{}", r.err);
    }
}

#[test]
fn dry_run_and_verbose_with_secret() {
    let tok = || Expr { kind: ExprKind::Var("token".into()), ty: Ty::Secret };
    let c = command(vec![SimpleCmd { words: vec![Word::Lit("printf".into()), Word::Lit("%s".into()), Word::Interp(vec![pe(tok())])] }], "printf %s {token}");
    let mut p = prog(vec![
        st(StmtKind::Command { cmd: c, on_fail: OnFail::Propagate }),
        let_call("t", stdf("fs", "temp_dir", vec![], Ty::Path, true), OnFail::Propagate),
        st(StmtKind::Call { call: stdf("fs", "write", vec![ea(bin(BinOp::PathJoin, v("t", Ty::Path), s("a"), Ty::Path)), ea(s("dentro"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "write", vec![ea(s("fora.txt")), ea(s("x"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        let_call("r", stdf("fs", "read", vec![ea(bin(BinOp::PathJoin, v("t", Ty::Path), s("a"), Ty::Path))], Ty::Str, true), OnFail::Propagate),
        let_call("e", stdf("fs", "exists", vec![ea(s("fora.txt"))], Ty::Bool, false), OnFail::Propagate),
        st(StmtKind::Call { call: stdf("log", "debug", vec![ea(s("detalhe"))], Ty::Unit, false), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "rm", vec![ea(v("t", Ty::Path))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        print(vec![v("r", Ty::Str), v("e", Ty::Bool)]),
    ]);
    p.params = vec![Param { name: "token".into(), flag: "--token".into(), ty: Ty::Secret, default: None, doc: String::new(), env_var: Some("NALU_TOKEN".into()) }];
    if let Some(r) = run_mode(&p, Mode::Script, &["--dry-run", "--verbose"], &[("NALU_TOKEN", "s3gr3d0")], None) {
        assert_eq!((r.out.as_str(), r.code), ("dentro false\n", 0), "{}", r.err);
        assert!(r.err.contains("[dry-run] printf %s ******"), "{}", r.err);
        assert!(r.err.contains("[dry-run] fs.write fora.txt"), "{}", r.err);
        assert!(r.err.contains("debug: detalhe"), "{}", r.err);
        assert!(!r.err.contains("s3gr3d0") && !r.out.contains("s3gr3d0"), "secret leaked: {}", r.err);
    }
}

#[test]
fn test_mode() {
    let mut p = prog(vec![let_e("base", i(10)), print(vec![s("não roda")])]);
    p.funcs.push(func("div", vec![("a", Ty::Int), ("b", Ty::Int)], Ty::Int, vec![st(StmtKind::If { cond: bin(BinOp::Eq, v("b", Ty::Int), i(0), Ty::Bool), then: b(vec![fail(s("divisão por zero"), None)]), els: None }), ret(bin(BinOp::Div, v("a", Ty::Int), v("b", Ty::Int), Ty::Int))], true));
    let divc = |a: i64, c: i64| user("div", vec![ea(i(a)), ea(i(c))], Ty::Int, true);
    p.tests = vec![
        TestDef { name: "divide".into(), body: b(vec![let_call("x", divc(10, 3), OnFail::Propagate), st(StmtKind::Assert { cond: bin(BinOp::Eq, v("x", Ty::Int), i(3), Ty::Bool), text: "dividir(10, 3) == 3".into() })]) },
        TestDef { name: "zero falha".into(), body: b(vec![st(StmtKind::AssertFails(Box::new(st(StmtKind::Call { call: divc(1, 0), on_fail: OnFail::Propagate }))))]) },
        TestDef { name: "de propósito".into(), body: b(vec![st(StmtKind::Assert { cond: bin(BinOp::Eq, v("base", Ty::Int), i(4), Ty::Bool), text: "base == 4".into() })]) },
        TestDef { name: "erro".into(), body: b(vec![let_call("y", divc(1, 0), OnFail::Propagate)]) },
    ];
    if let Some(r) = run_mode(&p, Mode::Tests, &[], &[], None) {
        assert_eq!(r.out, "ok      divide\nok      zero falha\nFAILED  de propósito: assert failed: base == 4\nFAILED  erro: divisão por zero\n4 test(s), 2 failure(s)\n", "{}", r.err);
        assert_eq!(r.code, 1);
    }
}

#[test]
fn fs_basics() {
    let p_ = |x: &str| s(x);
    let p = prog(vec![
        let_call("t", stdf("fs", "temp_dir", vec![], Ty::Path, true), OnFail::Propagate),
        st(StmtKind::Call { call: stdf("sys", "cd", vec![ea(v("t", Ty::Path))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "write", vec![ea(p_("a.txt")), ea(s("l1\n"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "append", vec![ea(p_("a.txt")), ea(s("l2\n"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "mkdir", vec![ea(p_("sub/d"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "write", vec![ea(p_("sub/d/z.txt")), ea(s("z"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "write", vec![ea(p_(".oculto")), ea(s("o"))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        let_call("ls", stdf("fs", "ls", vec![ea(p_("."))], Ty::list(Ty::Str), true), OnFail::Propagate),
        let_call("w", stdf("fs", "walk", vec![ea(p_("sub"))], Ty::list(Ty::Path), true), OnFail::Propagate),
        let_call("g", stdf("fs", "glob", vec![ea(s("*"))], Ty::list(Ty::Path), false), OnFail::Propagate),
        let_call("g2", stdf("fs", "glob", vec![ea(s("sub"))], Ty::list(Ty::Path), false), OnFail::Propagate),
        let_call("sz", stdf("fs", "size", vec![ea(p_("a.txt"))], Ty::Int, true), OnFail::Propagate),
        st(StmtKind::Call { call: stdf("fs", "rm", vec![ea(p_("."))], Ty::Unit, true), on_fail: OnFail::Block { err: None, body: b(vec![print(vec![s("recusou")])]), value: None } }),
        let_call("nx", stdf("fs", "read", vec![ea(p_("nao"))], Ty::Str, true), OnFail::Default(Box::new(Rvalue::Expr(s("padrão"))))),
        let_call("j1", meth(Ty::list(Ty::Str), "join", vec![ea(v("ls", Ty::list(Ty::Str))), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("j2", meth(Ty::list(Ty::Path), "join", vec![ea(v("w", Ty::list(Ty::Path))), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("j3", meth(Ty::list(Ty::Path), "join", vec![ea(v("g", Ty::list(Ty::Path))), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("j4", meth(Ty::list(Ty::Path), "join", vec![ea(v("g2", Ty::list(Ty::Path))), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        let_call("pn", meth(Ty::Path, "ext", vec![ea(s("dir/.bashrc"))], Ty::Str, false), OnFail::Propagate),
        let_call("pp", meth(Ty::Path, "parent", vec![ea(s("/"))], Ty::Path, false), OnFail::Propagate),
        let_call("ps", meth(Ty::Path, "stem", vec![ea(s("a.tar.gz"))], Ty::Str, false), OnFail::Propagate),
        print(vec![v("j1", Ty::Str), v("j2", Ty::Str), v("j3", Ty::Str), v("j4", Ty::Str), v("sz", Ty::Int), v("nx", Ty::Str)]),
        print(vec![interp(vec![pl("["), pe(v("pn", Ty::Str)), pl("]")]), v("pp", Ty::Path), v("ps", Ty::Str)]),
        let_call("tmpb", stdf("dirs", "temp", vec![], Ty::Path, false), OnFail::Propagate),
        st(StmtKind::Call { call: stdf("sys", "cd", vec![ea(v("tmpb", Ty::Path))], Ty::Unit, true), on_fail: OnFail::Propagate }),
        st(StmtKind::Call { call: stdf("fs", "rm", vec![ea(v("t", Ty::Path))], Ty::Unit, true), on_fail: OnFail::Propagate }),
    ]);
    if cfg!(windows) {
        return;
    }
    expect(&p, &[], "recusou\n.oculto,a.txt,sub sub/d/z.txt a.txt,sub sub 6 padrão\n[] / a.tar\n", 0);
}

/// `runtime/ps/tests.ps1`: unit tests of the helpers, when PowerShell is available.
#[test]
fn runtime_tests_ps1() {
    let Some(ps) = pwsh() else {
        eprintln!("warning: no PowerShell (NALU_PWSH); runtime/ps/tests.ps1 did not run");
        return;
    };
    let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ps/tests.ps1");
    let o = PCommand::new(ps).args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&f).output().unwrap();
    assert!(o.status.success(), "runtime/ps/tests.ps1 failed:\n{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
}


#[test]
fn chained_else_if() {
    let iff = |c: Expr, t: Vec<Stmt>, e: Option<Vec<Stmt>>| st(StmtKind::If { cond: c, then: b(t), els: e.map(b) });
    let mut stmts = vec![];
    for n in [0, 1, 5] {
        stmts.push(let_e(&format!("n{}", n), i(n)));
        stmts.push(iff(
            bin(BinOp::Eq, v(&format!("n{}", n), Ty::Int), i(0), Ty::Bool),
            vec![print(vec![s("zero")])],
            Some(vec![iff(bin(BinOp::Eq, v(&format!("n{}", n), Ty::Int), i(1), Ty::Bool), vec![print(vec![s("um")])], Some(vec![print(vec![s("outro")])]))]),
        ));
    }
    expect(&prog(stmts), &[], "zero\num\noutro\n", 0);
}

/// `parallel` runs the tasks **at the same time** (RunspacePool): three 1s tasks run together (the last
/// starts before the first one ends) and finish before the 3s they would take in sequence; with `max: 1`,
/// one at a time. Each task prints the window it ran in (`name start end`, in ms).
#[test]
fn parallel_runs_at_the_same_time() {
    let agora = |n: &str| let_call(n, stdf("time", "now_ms", vec![], Ty::Int, false), OnFail::Propagate);
        // ms since `t` (duration / 1ms)
    let desde = |n: &str, t: &str| {
        let d = format!("{}_d", n);
        vec![
            let_call(&d, stdf("time", "since_ms", vec![ea(v(t, Ty::Int))], Ty::Duration, false), OnFail::Propagate),
            let_e(n, bin(BinOp::Div, v(&d, Ty::Duration), dur(1), Ty::Int)),
        ]
    };
    let tarefa = |nome: &str, t: &str, ms: i64| {
        let mut st_ = desde("ini", t);
        st_.push(st(StmtKind::Call { call: stdf("time", "sleep", vec![ea(dur(ms))], Ty::Unit, false), on_fail: OnFail::Propagate }));
        st_.extend(desde("fim", t));
        st_.push(print(vec![s(nome), v("ini", Ty::Int), v("fim", Ty::Int)]));
        b(st_)
    };
    let mut body = vec![agora("t0")];
    body.push(st(StmtKind::Parallel { max: Some(i(3)), body: ParallelBody::Tasks(vec![tarefa("a", "t0", 1000), tarefa("b", "t0", 1000), tarefa("c", "t0", 1000)]), on_fail: OnFail::Propagate }));
    body.extend(desde("total", "t0"));
    body.push(agora("t1"));
    body.push(st(StmtKind::Parallel { max: Some(i(1)), body: ParallelBody::Tasks(vec![tarefa("x", "t1", 300), tarefa("y", "t1", 300)]), on_fail: OnFail::Propagate }));
    body.push(print(vec![s("total"), v("total", Ty::Int)]));
    let p = prog(body);
    let text = generate(&p, Mode::Script);
    assert!(text.contains("CreateRunspacePool") && text.contains("function __task_"), "{}", numbered(&text));
    let Some(r) = run_mode(&p, Mode::Script, &[], &[], None) else { return };
    let mut w: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    let mut total = -1;
    for l in r.out.lines() {
        let ps: Vec<&str> = l.split_whitespace().collect();
        match ps.as_slice() {
            ["total", t] => total = t.parse().unwrap(),
            [n, a, z] => {
                w.insert(n.to_string(), (a.parse().unwrap(), z.parse().unwrap()));
            }
            _ => panic!("linha inesperada: {:?}\n{}", l, r.out),
        }
    }
    assert!(r.code == 0 && w.len() == 5 && total >= 0, "code {}\n{}\n--- stderr ---\n{}", r.code, r.out, r.err);
    let juntas = ["a", "b", "c"];
    let ultimo_inicio = juntas.iter().map(|n| w[*n].0).max().unwrap();
    let primeiro_fim = juntas.iter().map(|n| w[*n].1).min().unwrap();
    assert!(ultimo_inicio < primeiro_fim, "the three tasks did not run at the same time:\n{}", r.out);
        // in sequence it would take at least 3000 ms; the slack covers runspace creation on a loaded machine
    assert!(total < 2900, "three 1s tasks took {} ms (in sequence: 3s):\n{}", total, r.out);
    let (x, y) = (w["x"], w["y"]);
    assert!(y.0 >= x.1 || x.0 >= y.1, "with max: 1 the tasks overlapped:\n{}", r.out);
    println!("parallel on PowerShell: three 1s tasks in {} ms (windows a {:?}, b {:?}, c {:?}); max: 1 x {:?}, y {:?}", total, w["a"], w["b"], w["c"], x, y);
}

/// Inside a task: `defer` runs at its end (also when it fails); `exit` ends only the task
/// ("task N finished without a result", without its `defer`s, like the Bash subshell); the aggregated error
/// carries the first failure's code; what `native` writes to the pipeline (and `Write-Host`) goes to stdout.
#[test]
fn parallel_defer_exit_and_code() {
    let defer_print = |x: &str| st(StmtKind::Defer(Box::new(print(vec![s(x)]))));
    let tarefas = vec![
        b(vec![defer_print("limpa 1"), fail(s("um"), Some(4))]),
        b(vec![defer_print("limpa 2"), st(StmtKind::Exit(Some(i(9))))]),
        b(vec![st(StmtKind::Native { bash: "echo nativo".into(), powershell: "Write-Output 'nativo'\nWrite-Host 'host'".into() })]),
        b(vec![fail(s("quatro"), Some(6))]),
    ];
    let p = prog(vec![st(StmtKind::Parallel { max: Some(i(1)), body: ParallelBody::Tasks(tarefas), on_fail: OnFail::Propagate }), print(vec![s("nunca")])]);
    let Some(r) = expect(&p, &[], "limpa 1\nnativo\nhost\n", 4) else { return };
    assert!(r.err.contains("error: parallel: um; task 2 finished without a result; quatro"), "{}", r.err);
}

/// A task reads the parameters and variables of the enclosing function (passed to the runspace) and the globals, which are
/// copied: a function that changes a global inside the task does not change the script's (like in the Bash subshell).
#[test]
fn parallel_reads_enclosing_function_and_copies_globals() {
    let li = Ty::list(Ty::Int);
    let mut p = prog(vec![
        let_("g", li.clone(), list(Ty::Int, vec![i(1)])),
        let_call("r", user("calc", vec![ea(i(100))], li.clone(), true), OnFail::Propagate),
        let_call("rj", meth(li.clone(), "join", vec![ea(v("r", li.clone())), ea(s(","))], Ty::Str, false), OnFail::Propagate),
        print(vec![v("rj", Ty::Str), len(v("g", li.clone()))]),
    ]);
    let push = st(StmtKind::Call { call: meth(li.clone(), "push", vec![ea(v("g", li.clone())), ea(v("x", Ty::Int))], Ty::Unit, false), on_fail: OnFail::Propagate });
    p.funcs.push(func("poe", vec![("x", Ty::Int)], Ty::Unit, vec![push], false));
    let tarefa = b(vec![
        st(StmtKind::Call { call: user("poe", vec![ea(v("x", Ty::Int))], Ty::Unit, false), on_fail: OnFail::Propagate }),
        let_e("y", bin(BinOp::Add, bin(BinOp::Mul, v("x", Ty::Int), v("fator", Ty::Int), Ty::Int), v("base", Ty::Int), Ty::Int)),
    ]);
    p.funcs.push(func(
        "calc",
        vec![("base", Ty::Int)],
        li.clone(),
        vec![
            let_e("fator", i(3)),
            let_("xs", li.clone(), list(Ty::Int, vec![i(1), i(2), i(3)])),
            let_("res", li.clone(), list(Ty::Int, vec![])),
            st(StmtKind::Parallel { max: None, body: ParallelBody::For { var: "x".into(), list: v("xs", li.clone()), body: tarefa, collect: Some(("res".into(), v("y", Ty::Int))) }, on_fail: OnFail::Propagate }),
            ret(v("res", li.clone())),
        ],
        true,
    ));
    expect(&p, &[], "103,106,109 1\n", 0);
}
