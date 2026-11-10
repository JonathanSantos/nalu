//! Bash backend tests: hand-built IR → script → `/bin/bash` → compare the output.
//!
//! The scripts are generated for the target of the machine running the tests (`macos` with its Bash 3.2 on
//! macOS, `linux` elsewhere), because each target relies on that system's tools (`stat`, `date`, …).
//! The module is not compiled on Windows, where the PowerShell backend is the one that runs.

use super::generate;
use crate::ir::*;
use crate::target::{Mode, Target};
use std::process::Command as Proc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ----------------------------------------------------------------------------------------------
// Short constructors
// ----------------------------------------------------------------------------------------------

pub fn st(k: StmtKind) -> Stmt {
    Stmt::new(1, k)
}
pub fn s(x: &str) -> Expr {
    Expr::str(x)
}
pub fn i(n: i64) -> Expr {
    Expr::int(n)
}
pub fn b(x: bool) -> Expr {
    Expr::bool(x)
}
pub fn dur(ms: i64) -> Expr {
    Expr { kind: ExprKind::Duration(ms), ty: Ty::Duration }
}
pub fn v(n: &str, ty: Ty) -> Expr {
    Expr::var(n, ty)
}
pub fn vs(n: &str) -> Expr {
    v(n, Ty::Str)
}
pub fn vi(n: &str) -> Expr {
    v(n, Ty::Int)
}
pub fn lstr() -> Ty {
    Ty::List(Box::new(Ty::Str))
}
pub fn lint() -> Ty {
    Ty::List(Box::new(Ty::Int))
}
pub fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
    let ty = match op {
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::And | BinOp::Or => Ty::Bool,
        BinOp::Concat => Ty::Str,
        BinOp::PathJoin => Ty::Path,
        _ => a.ty.clone(),
    };
    Expr { kind: ExprKind::Binary(op, Box::new(a), Box::new(b)), ty }
}
pub fn not(a: Expr) -> Expr {
    Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(a)), ty: Ty::Bool }
}
pub fn neg(a: Expr) -> Expr {
    let ty = a.ty.clone();
    Expr { kind: ExprKind::Unary(UnOp::Neg, Box::new(a)), ty }
}
pub fn interp(parts: Vec<Expr>) -> Expr {
    let ps = parts
        .into_iter()
        .map(|e| match e.kind {
            ExprKind::Str(ref x) => Part::Lit(x.clone()),
            _ => Part::Expr(e),
        })
        .collect();
    Expr { kind: ExprKind::Interp(ps), ty: Ty::Str }
}
pub fn field(base: Expr, f: &str, ty: Ty) -> Expr {
    Expr { kind: ExprKind::Field(Box::new(base), f.to_string()), ty }
}
pub fn index(list: Expr, ix: Expr) -> Expr {
    let ty = match &list.ty {
        Ty::List(e) => (**e).clone(),
        _ => Ty::Str,
    };
    Expr { kind: ExprKind::Index(Box::new(list), Box::new(ix)), ty }
}
pub fn len(x: Expr) -> Expr {
    Expr { kind: ExprKind::Len(Box::new(x)), ty: Ty::Int }
}
pub fn cur_err(f: &str) -> Expr {
    let ty = if f == "code" { Ty::Int } else { Ty::Str };
    field(Expr { kind: ExprKind::CurrentErr, ty: Ty::Error }, f, ty)
}
pub fn let_(n: &str, ty: Ty, rv: Rvalue) -> Stmt {
    st(StmtKind::Let { var: n.into(), ty, mutable: false, value: rv })
}
pub fn var_(n: &str, ty: Ty, rv: Rvalue) -> Stmt {
    st(StmtKind::Let { var: n.into(), ty, mutable: true, value: rv })
}
pub fn ex(e: Expr) -> Rvalue {
    Rvalue::Expr(e)
}
pub fn set(n: &str, rv: Rvalue) -> Stmt {
    st(StmtKind::Assign { target: LValue::Var(n.into()), value: rv })
}
pub fn call_b(name: &str, args: Vec<Expr>, ret: Ty) -> Call {
    Call { target: CallTarget::Builtin(name.into()), args: args.into_iter().map(Arg::Expr).collect(), ret, fallible: false }
}
pub fn call_std(m: &str, n: &str, args: Vec<Expr>, ret: Ty) -> Call {
    let fallible = crate::stdlib::find_fn(m, n).map_or(false, |f| f.fallible);
    Call { target: CallTarget::Std { module: m.into(), name: n.into() }, args: args.into_iter().map(Arg::Expr).collect(), ret, fallible }
}
pub fn call_m(recv: Ty, n: &str, args: Vec<Arg>, ret: Ty) -> Call {
    let rk = match &recv {
        Ty::List(_) => "list",
        Ty::Map(_, _) => "map",
        Ty::Enum(_) => "enum",
        Ty::Int => "int",
        Ty::Bool => "bool",
        Ty::Duration => "duration",
        _ => "str",
    };
    let fallible = crate::stdlib::find_method(rk, n).map_or(false, |m| m.fallible);
    Call { target: CallTarget::Method { recv_ty: recv, name: n.into() }, args, ret, fallible }
}
pub fn call_u(n: &str, args: Vec<Expr>, ret: Ty, fallible: bool) -> Call {
    Call { target: CallTarget::User(n.into()), args: args.into_iter().map(Arg::Expr).collect(), ret, fallible }
}
pub fn a(e: Expr) -> Arg {
    Arg::Expr(e)
}
pub fn rcall(c: Call) -> Rvalue {
    Rvalue::Call { call: c, on_fail: OnFail::Propagate }
}
pub fn rcall_or(c: Call, of: OnFail) -> Rvalue {
    Rvalue::Call { call: c, on_fail: of }
}
pub fn do_(c: Call) -> Stmt {
    st(StmtKind::Call { call: c, on_fail: OnFail::Propagate })
}
pub fn do_or(c: Call, of: OnFail) -> Stmt {
    st(StmtKind::Call { call: c, on_fail: of })
}
pub fn print(args: Vec<Expr>) -> Stmt {
    do_(call_b("print", args, Ty::Unit))
}
pub fn blk(stmts: Vec<Stmt>) -> Block {
    Block { stmts }
}
pub fn if_(c: Expr, t: Vec<Stmt>, e: Option<Vec<Stmt>>) -> Stmt {
    st(StmtKind::If { cond: c, then: blk(t), els: e.map(blk) })
}
pub fn fail(m: &str) -> Stmt {
    st(StmtKind::Fail { message: s(m), code: None })
}
pub fn ret(e: Expr) -> Stmt {
    st(StmtKind::Return(Some(e)))
}
pub fn func(n: &str, params: Vec<(&str, Ty)>, ret: Ty, body: Vec<Stmt>, fallible: bool) -> Func {
    Func {
        name: n.into(),
        params: params.into_iter().map(|(n, t)| FParam { name: n.into(), ty: t }).collect(),
        ret,
        body: blk(body),
        fallible,
        doc: String::new(),
    }
}
pub fn word_lit(x: &str) -> Word {
    Word::Lit(x.into())
}
pub fn word_e(e: Expr) -> Word {
    Word::Interp(vec![Part::Expr(e)])
}
pub fn cmd(words: Vec<Word>, text: &str) -> Command {
    Command { pipeline: vec![SimpleCmd::new(words)], redirects: vec![], stdin: None, text: text.into() }
}
pub fn lit_cmd(text: &str) -> Command {
    cmd(text.split(' ').map(word_lit).collect(), text)
}
pub fn run_cmd(c: Command) -> Stmt {
    st(StmtKind::Command { cmd: c, on_fail: OnFail::Propagate })
}
pub fn lam(params: Vec<(&str, Ty)>, ret_ty: Ty, body: Vec<Stmt>) -> Arg {
    Arg::Lambda(Lambda { params: params.into_iter().map(|(n, t)| FParam { name: n.into(), ty: t }).collect(), ret: ret_ty, body: blk(body) })
}
/// The Bash target that can run on this machine.
pub fn host_target() -> Target {
    if cfg!(target_os = "macos") {
        Target::Macos
    } else {
        Target::Linux
    }
}
pub fn prog(body: Vec<Stmt>) -> Program {
    let mut p = Program::empty("t", host_target());
    p.body = blk(body);
    p
}

// ----------------------------------------------------------------------------------------------
// Execution
// ----------------------------------------------------------------------------------------------

static N: AtomicUsize = AtomicUsize::new(0);

pub fn scratch() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nalu-bash-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct Out {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub script: String,
}

pub fn run_script(script: &str, args: &[&str], stdin: Option<&str>) -> Out {
    let k = N.fetch_add(1, Ordering::SeqCst);
    let path = scratch().join(format!("t{}.sh", k));
    std::fs::write(&path, script).unwrap();
    let n = Proc::new("/bin/bash").arg("-n").arg(&path).output().unwrap();
    assert!(n.status.success(), "bash -n falhou:\n{}\n---\n{}", String::from_utf8_lossy(&n.stderr), numbered(script));
    let mut c = Proc::new("/bin/bash");
    c.arg(&path).args(args).env("LC_ALL", "en_US.UTF-8").current_dir(scratch());
    c.stdin(std::process::Stdio::piped());
    c.stdout(std::process::Stdio::piped());
    c.stderr(std::process::Stdio::piped());
    let mut ch = c.spawn().unwrap();
    {
        use std::io::Write;
        let mut si = ch.stdin.take().unwrap();
        if let Some(x) = stdin {
            si.write_all(x.as_bytes()).unwrap();
        }
    }
    let o = ch.wait_with_output().unwrap();
    Out {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
        script: script.to_string(),
    }
}

pub fn numbered(s: &str) -> String {
    s.lines().enumerate().map(|(i, l)| format!("{:4} {}", i + 1, l)).collect::<Vec<_>>().join("\n")
}

pub fn run(p: &Program, args: &[&str]) -> Out {
    let sc = generate(p, Mode::Script);
    run_script(&sc, args, None)
}

/// Runs and checks stdout and exit code; shows the script on failure.
#[track_caller]
pub fn check(p: &Program, args: &[&str], want_out: &str, want_code: i32) -> Out {
    let o = run(p, args);
    if o.stdout != want_out || o.code != want_code {
        panic!(
            "output differs\n--- expected (code {}):\n{}\n--- got (code {}):\n{}\n--- stderr:\n{}\n--- script:\n{}",
            want_code,
            want_out,
            o.code,
            o.stdout,
            o.stderr,
            numbered(&o.script)
        );
    }
    o
}

// ----------------------------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------------------------

#[test]
fn minimal_hello() {
    let mut p = prog(vec![
        let_("x", Ty::Str, ex(s("mundo"))),
        print(vec![interp(vec![s("olá, "), vs("x"), s("!")])]),
        print(vec![s("a"), i(1), b(true)]),
    ]);
    p.params.push(Param { name: "nome".into(), flag: "--nome".into(), ty: Ty::Str, default: Some(ex(s("ana"))), doc: String::new(), env_var: None });
    p.body.stmts.push(print(vec![vs("nome")]));
    check(&p, &[], "olá, mundo!\na 1 true\nana\n", 0);
    check(&p, &["--nome", "zé"], "olá, mundo!\na 1 true\nzé\n", 0);
    check(&p, &["--nome=a b"], "olá, mundo!\na 1 true\na b\n", 0);
    let o = check(&p, &["--help"], &format!("{}\n", p.usage), 0);
    assert!(o.script.starts_with("#!/usr/bin/env bash\n# Generated by nalu"));
    let o = check(&p, &["--xyz"], "", 2);
    assert!(o.stderr.contains("unknown option: --xyz"), "{}", o.stderr);
    assert!(o.stderr.contains("run with --help"), "{}", o.stderr);
}

#[test]
fn arithmetic_and_text() {
    let p = prog(vec![
        let_("a", Ty::Int, ex(i(-7))),
        print(vec![bin(BinOp::Div, vi("a"), i(2)), bin(BinOp::Mod, vi("a"), i(2)), bin(BinOp::Mul, bin(BinOp::Add, i(1), i(2)), i(3))]),
        print(vec![bin(BinOp::Sub, i(10), bin(BinOp::Sub, i(3), i(2))), neg(vi("a"))]),
        let_("u", Ty::Str, ex(s("ação👍"))),
        print(vec![len(vs("u"))]),
        print(vec![bin(BinOp::Lt, s("B"), s("a")), bin(BinOp::Lt, s("a"), s("B")), bin(BinOp::Gt, s("é"), s("z"))]),
        print(vec![bin(BinOp::Concat, vs("u"), s("-x")), bin(BinOp::Eq, vs("u"), s("ação👍")), bin(BinOp::Ne, i(1), i(1))]),
        let_("pth", Ty::Path, ex(bin(BinOp::PathJoin, v("u", Ty::Path), s("f.txt")))),
        print(vec![v("pth", Ty::Path), bin(BinOp::PathJoin, s(""), s("b")), bin(BinOp::PathJoin, s("/"), s("b"))]),
        let_("q", Ty::Str, ex(s("it's \"x\" $HOME `a` \\ \n fim\t."))),
        print(vec![vs("q")]),
        print(vec![bin(BinOp::And, b(true), not(bin(BinOp::Lt, i(3), i(2))))]),
        print(vec![s("{literal}")]),
    ]);
    check(
        &p,
        &[],
        "-3 -1 9\n9 7\n5\ntrue false true\nação👍-x true false\nação👍/f.txt b /b\nit's \"x\" $HOME `a` \\ \n fim\t.\ntrue\n{literal}\n",
        0,
    );
}

#[test]
fn control_flow() {
    let p = prog(vec![
        var_("n", Ty::Int, ex(i(0))),
        st(StmtKind::While {
            cond: bin(BinOp::Lt, vi("n"), i(5)),
            body: blk(vec![
                set("n", ex(bin(BinOp::Add, vi("n"), i(1)))),
                if_(bin(BinOp::Eq, vi("n"), i(2)), vec![st(StmtKind::Continue)], None),
                if_(bin(BinOp::Eq, vi("n"), i(4)), vec![st(StmtKind::Break)], None),
                print(vec![vi("n")]),
            ]),
        }),
        st(StmtKind::ForRange { var: "k".into(), from: i(1), to: i(3), inclusive: true, body: blk(vec![print(vec![vi("k")])]) }),
        st(StmtKind::ForRange { var: "k2".into(), from: i(0), to: vi("n"), inclusive: false, body: blk(vec![set("n", ex(i(100))), print(vec![vi("k2")])]) }),
        if_(bin(BinOp::Gt, vi("n"), i(1000)), vec![print(vec![s("a")])], Some(vec![if_(bin(BinOp::Eq, vi("n"), i(100)), vec![print(vec![s("b")])], Some(vec![print(vec![s("c")])]))])),
    ]);
    check(&p, &[], "1\n3\n1\n2\n3\n0\n1\n2\n3\nb\n", 0);
}

#[test]
fn match_literals_and_ranges() {
    let mut p = prog(vec![]);
    p.enums.push(EnumDef { name: "Amb".into(), cases: vec!["dev".into(), "prod".into()] });
    let amb = Ty::Enum("Amb".into());
    let ec = |c: &str| Expr { kind: ExprKind::EnumCase { enum_: "Amb".into(), case: c.into() }, ty: Ty::Enum("Amb".into()) };
    p.body.stmts = vec![
        let_("a", amb.clone(), ex(ec("prod"))),
        st(StmtKind::Match {
            subject: v("a", amb.clone()),
            arms: vec![Arm { patterns: vec![Pattern::Lit(ec("dev"))], body: blk(vec![print(vec![s("é dev")])]) }, Arm { patterns: vec![Pattern::Lit(ec("prod"))], body: blk(vec![print(vec![s("é prod")])]) }],
            default: None,
        }),
        st(StmtKind::ForList {
            var: "h".into(),
            index: None,
            list: v("hs", lint()),
            body: blk(vec![st(StmtKind::Match {
                subject: vi("h"),
                arms: vec![
                    Arm { patterns: vec![Pattern::Range { from: i(0), to: i(12), inclusive: false }], body: blk(vec![print(vec![s("manhã")])]) },
                    Arm { patterns: vec![Pattern::Lit(i(12)), Pattern::Range { from: i(13), to: i(18), inclusive: true }], body: blk(vec![print(vec![s("tarde")])]) },
                ],
                default: Some(blk(vec![print(vec![s("noite")])])),
            })]),
        }),
        st(StmtKind::Match {
            subject: s("x y"),
            arms: vec![Arm { patterns: vec![Pattern::Lit(s("x")), Pattern::Lit(s("x y"))], body: blk(vec![print(vec![s("casou")])]) }],
            default: Some(blk(vec![])),
        }),
    ];
    p.body.stmts.insert(0, let_("hs", lint(), Rvalue::List { elem_ty: Ty::Int, items: vec![i(0), i(11), i(12), i(18), i(19)] }));
    check(&p, &[], "é prod\nmanhã\nmanhã\ntarde\ntarde\nnoite\ncasou\n", 0);
}

#[test]
fn functions_and_errors() {
    let mut p = prog(vec![]);
    p.funcs.push(func(
        "dividir",
        vec![("a", Ty::Int), ("b", Ty::Int)],
        Ty::Int,
        vec![if_(bin(BinOp::Eq, vi("b"), i(0)), vec![fail("divisão por zero")], None), ret(bin(BinOp::Div, vi("a"), vi("b")))],
        true,
    ));
    p.funcs.push(func(
        "fat",
        vec![("n", Ty::Int)],
        Ty::Int,
        vec![
            if_(bin(BinOp::Le, vi("n"), i(1)), vec![ret(i(1))], None),
            let_("r", Ty::Int, rcall(call_u("fat", vec![bin(BinOp::Sub, vi("n"), i(1))], Ty::Int, false))),
            ret(bin(BinOp::Mul, vi("n"), vi("r"))),
        ],
        false,
    ));
    p.body.stmts = vec![
        let_("x", Ty::Int, rcall(call_u("dividir", vec![i(10), i(3)], Ty::Int, true))),
        print(vec![vi("x")]),
        let_("f", Ty::Int, rcall(call_u("fat", vec![i(10)], Ty::Int, false))),
        print(vec![vi("f")]),
        // or value
        let_("y", Ty::Int, rcall_or(call_u("dividir", vec![i(1), i(0)], Ty::Int, true), OnFail::Default(Box::new(ex(i(-1)))))),
        print(vec![vi("y")]),
        // or block with err
        let_(
            "z",
            Ty::Int,
            rcall_or(
                call_u("dividir", vec![i(1), i(0)], Ty::Int, true),
                OnFail::Block {
                    err: Some("err".into()),
                    body: blk(vec![print(vec![interp(vec![s("falhou: "), field(v("err", Ty::Error), "message", Ty::Str), s(" ("), field(v("err", Ty::Error), "code", Ty::Int), s(")")])])]),
                    value: Some(i(7)),
                },
            ),
        ),
        print(vec![vi("z")]),
        // or fail with err
        do_or(
            call_u("dividir", vec![i(2), i(0)], Ty::Int, true),
            OnFail::Fail { message: interp(vec![s("não deu: "), cur_err("message")]), code: Some(i(3)) },
        ),
        print(vec![s("não chega")]),
    ];
    let o = check(&p, &[], "3\n3628800\n-1\nfalhou: divisão por zero (1)\n7\n", 3);
    assert_eq!(o.stderr, "error: não deu: divisão por zero\n");
}

#[test]
fn try_with_break_continue_return() {
    let mut p = prog(vec![]);
    p.funcs.push(func(
        "f",
        vec![("n", Ty::Int)],
        Ty::Str,
        vec![
            st(StmtKind::Try {
                body: blk(vec![
                    if_(bin(BinOp::Eq, vi("n"), i(1)), vec![ret(s("um"))], None),
                    if_(bin(BinOp::Eq, vi("n"), i(2)), vec![fail("dois")], None),
                ]),
                on_fail: OnFail::Block { err: None, body: blk(vec![ret(interp(vec![s("pegou "), cur_err("message")]))]), value: None },
            }),
            ret(s("fim")),
        ],
        false,
    ));
    p.body.stmts = vec![
        st(StmtKind::ForRange {
            var: "k".into(),
            from: i(0),
            to: i(4),
            inclusive: false,
            body: blk(vec![
                let_("r", Ty::Str, rcall(call_u("f", vec![vi("k")], Ty::Str, false))),
                print(vec![vs("r")]),
                st(StmtKind::Try {
                    body: blk(vec![if_(bin(BinOp::Eq, vi("k"), i(0)), vec![st(StmtKind::Continue)], None), if_(bin(BinOp::Eq, vi("k"), i(3)), vec![st(StmtKind::Break)], None), print(vec![s("dentro")])]),
                    on_fail: OnFail::Ignore,
                }),
            ]),
        }),
        st(StmtKind::Try { body: blk(vec![fail("x"), print(vec![s("nunca")])]), on_fail: OnFail::Fail { message: interp(vec![s("try: "), cur_err("message")]), code: None } }),
    ];
    let o = check(&p, &[], "fim\num\ndentro\npegou dois\ndentro\nfim\n", 1);
    assert_eq!(o.stderr, "error: try: x\n");
}

fn lit_list(elem: Ty, items: Vec<Expr>) -> Rvalue {
    Rvalue::List { elem_ty: elem, items }
}
fn m_list(n: &str, args: Vec<Arg>, ret: Ty) -> Call {
    call_m(lstr(), n, args, ret)
}
fn xs() -> Expr {
    vs_list("xs")
}
fn vs_list(n: &str) -> Expr {
    v(n, lstr())
}

#[test]
fn lists() {
    let p = prog(vec![
        var_("xs", lstr(), lit_list(Ty::Str, vec![s("b"), s("a b"), s(""), s("c\nd"), s("B")])),
        print(vec![len(xs()), index(xs(), i(1)), index(xs(), i(9)), index(xs(), i(-1))]),
        do_(m_list("push", vec![a(xs()), a(s("z"))], Ty::Unit)),
        let_("ys", lstr(), ex(xs())),
        do_(m_list("push", vec![a(xs()), a(s("só em xs"))], Ty::Unit)),
        print(vec![len(xs()), len(vs_list("ys"))]),
        let_("j", Ty::Str, rcall(m_list("join", vec![a(vs_list("ys")), a(s(","))], Ty::Str))),
        print(vec![vs("j")]),
        let_("so", lstr(), rcall(m_list("sort", vec![a(vs_list("ys"))], lstr()))),
        let_("j2", Ty::Str, rcall(m_list("join", vec![a(vs_list("so")), a(s("|"))], Ty::Str))),
        print(vec![vs("j2")]),
        let_("u", Ty::Str, rcall(m_list("pop", vec![a(xs())], Ty::Str))),
        print(vec![vs("u"), len(xs())]),
        let_("fi", Ty::Str, rcall(m_list("first", vec![a(xs())], Ty::Str))),
        let_("la", Ty::Str, rcall(m_list("last", vec![a(xs())], Ty::Str))),
        print(vec![vs("fi"), vs("la")]),
        let_("c", Ty::Bool, rcall(m_list("contains", vec![a(xs()), a(s("a b"))], Ty::Bool))),
        let_("ix", Ty::Int, rcall(m_list("index_of", vec![a(xs()), a(s("B"))], Ty::Int))),
        print(vec![v("c", Ty::Bool), vi("ix")]),
        let_("rv", lstr(), rcall(m_list("reverse", vec![a(vs_list("ys"))], lstr()))),
        let_("sl", lstr(), rcall(m_list("slice", vec![a(vs_list("rv")), a(i(1)), a(i(3))], lstr()))),
        print(vec![index(vs_list("rv"), i(0)), len(vs_list("sl")), index(vs_list("sl"), i(0))]),
        let_("dup", lstr(), lit_list(Ty::Str, vec![s("a"), s("b"), s("a"), s(""), s("b"), s("")])),
        let_("un", lstr(), rcall(m_list("unique", vec![a(vs_list("dup"))], lstr()))),
        let_("j3", Ty::Str, rcall(m_list("join", vec![a(vs_list("un")), a(s("+"))], Ty::Str))),
        print(vec![vs("j3"), len(vs_list("un"))]),
        let_("ns", lint(), lit_list(Ty::Int, vec![i(10), i(-2), i(33), i(3)])),
        let_("nso", lint(), rcall(call_m(lint(), "sort", vec![a(v("ns", lint()))], lint()))),
        let_("j4", Ty::Str, rcall(call_m(lint(), "join", vec![a(v("nso", lint())), a(s(" "))], Ty::Str))),
        let_("sm", Ty::Int, rcall(call_m(lint(), "sum", vec![a(v("ns", lint()))], Ty::Int))),
        print(vec![vs("j4"), vi("sm")]),
        let_("vazia", lstr(), lit_list(Ty::Str, vec![])),
        let_("e", Ty::Bool, rcall(m_list("is_empty", vec![a(vs_list("vazia"))], Ty::Bool))),
        let_("pe", Ty::Str, rcall_or(m_list("pop", vec![a(vs_list("vazia"))], Ty::Str), OnFail::Block { err: None, body: blk(vec![]), value: Some(cur_err("message")) })),
        print(vec![v("e", Ty::Bool), vs("pe")]),
        st(StmtKind::ForList { var: "x".into(), index: Some("k".into()), list: vs_list("so"), body: blk(vec![print(vec![vi("k"), interp(vec![s("["), vs("x"), s("]")])])]) }),
        st(StmtKind::Assign { target: LValue::Index { list: "xs".into(), index: i(0) }, value: ex(s("novo")) }),
        st(StmtKind::Assign { target: LValue::Index { list: "xs".into(), index: i(50) }, value: ex(s("fora")) }),
        print(vec![index(xs(), i(0)), len(xs())]),
    ]);
    check(
        &p,
        &[],
        "5 a b  \n7 6\nb,a b,,c\nd,B,z\n|B|a b|b|c\nd|z\nsó em xs 6\nb z\ntrue 4\nz 2 B\na+b+ 3\n-2 3 10 33 44\ntrue pop: the list is empty\n0 []\n1 [B]\n2 [a b]\n3 [b]\n4 [c\nd]\n5 [z]\nnovo 6\n",
        0,
    );
}

#[test]
fn lambdas() {
    let x = |n: &str| vs(n);
    let p = prog(vec![
        let_("xs", lstr(), lit_list(Ty::Str, vec![s("banana"), s("kiwi"), s("abacaxi"), s("uva")])),
        let_("lim", Ty::Int, ex(i(4))),
        let_("gr", lstr(), rcall(m_list("filter", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Bool, vec![ret(bin(BinOp::Gt, len(x("w")), vi("lim")))])], lstr()))),
        let_("j", Ty::Str, rcall(m_list("join", vec![a(vs_list("gr")), a(s(","))], Ty::Str))),
        print(vec![vs("j")]),
        let_("ls", lint(), rcall(m_list("map", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Int, vec![ret(len(x("w")))])], lint()))),
        let_("j2", Ty::Str, rcall(call_m(lint(), "join", vec![a(v("ls", lint())), a(s(","))], Ty::Str))),
        print(vec![vs("j2")]),
        let_("an", Ty::Bool, rcall(m_list("any", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Bool, vec![ret(bin(BinOp::Eq, x("w"), s("uva")))])], Ty::Bool))),
        let_("al", Ty::Bool, rcall(m_list("all", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Bool, vec![ret(bin(BinOp::Gt, len(x("w")), i(3)))])], Ty::Bool))),
        print(vec![v("an", Ty::Bool), v("al", Ty::Bool)]),
        let_("fd", Ty::Str, rcall(m_list("find", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Bool, vec![ret(bin(BinOp::Eq, len(x("w")), i(4)))])], Ty::Str))),
        let_(
            "nf",
            Ty::Str,
            rcall_or(m_list("find", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Bool, vec![ret(b(false))])], Ty::Str), OnFail::Default(Box::new(ex(s("nada"))))),
        ),
        print(vec![vs("fd"), vs("nf")]),
        let_("sb", lstr(), rcall(m_list("sort_by", vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Int, vec![ret(len(x("w")))])], lstr()))),
        let_("j3", Ty::Str, rcall(m_list("join", vec![a(vs_list("sb")), a(s(","))], Ty::Str))),
        print(vec![vs("j3")]),
        let_(
            "rot",
            lstr(),
            rcall(m_list(
                "map",
                vec![a(xs()), lam(vec![("w", Ty::Str)], Ty::Str, vec![if_(bin(BinOp::Gt, len(x("w")), i(4)), vec![ret(s("longa"))], None), ret(s("curta"))])],
                lstr(),
            )),
        ),
        let_("j4", Ty::Str, rcall(m_list("join", vec![a(vs_list("rot")), a(s(" "))], Ty::Str))),
        print(vec![vs("j4")]),
    ]);
    check(&p, &[], "banana,abacaxi\n6,4,7,3\ntrue false\nkiwi nada\nuva,kiwi,banana,abacaxi\nlonga curta longa curta\n", 0);
}

fn servico(p: &mut Program) -> Ty {
    p.structs.push(StructDef {
        name: "Servico".into(),
        fields: vec![Field { name: "nome".into(), ty: Ty::Str }, Field { name: "porta".into(), ty: Ty::Int }, Field { name: "deps".into(), ty: lstr() }],
    });
    Ty::Struct("Servico".into())
}

fn srv(nome: &str, porta: i64, deps: Expr) -> Rvalue {
    Rvalue::Struct { ty: "Servico".into(), spread: None, fields: vec![("nome".into(), s(nome)), ("porta".into(), i(porta)), ("deps".into(), deps)] }
}

#[test]
fn structs_and_lists_of_structs() {
    let mut p = prog(vec![]);
    let sv = servico(&mut p);
    let lsv = Ty::List(Box::new(sv.clone()));
    p.funcs.push(func(
        "Servico.url",
        vec![("self", sv.clone())],
        Ty::Str,
        vec![ret(interp(vec![s("http://"), field(v("self", sv.clone()), "nome", Ty::Str), s(":"), field(v("self", sv.clone()), "porta", Ty::Int)]))],
        false,
    ));
    p.funcs.push(func(
        "Servico.mais",
        vec![("self", sv.clone()), ("n", Ty::Int)],
        sv.clone(),
        vec![
            let_("r", sv.clone(), Rvalue::Struct { ty: "Servico".into(), spread: Some(v("self", sv.clone())), fields: vec![("porta".into(), bin(BinOp::Add, field(v("self", sv.clone()), "porta", Ty::Int), vi("n")))] }),
            ret(v("r", sv.clone())),
        ],
        false,
    ));
    let sel = |e: Expr, f: &str, t: Ty| field(e, f, t);
    p.body.stmts = vec![
        let_("d", lstr(), lit_list(Ty::Str, vec![s("db"), s("cache x")])),
        let_("api", sv.clone(), srv("api", 8080, vs_list("d"))),
        var_("s2", sv.clone(), ex(v("api", sv.clone()))),
        st(StmtKind::Assign { target: LValue::Field { base: "s2".into(), field: "porta".into() }, value: ex(i(1)) }),
        print(vec![sel(v("api", sv.clone()), "porta", Ty::Int), sel(v("s2", sv.clone()), "porta", Ty::Int), len(sel(v("s2", sv.clone()), "deps", lstr()))]),
        let_("u", Ty::Str, rcall(call_u("Servico.url", vec![v("api", sv.clone())], Ty::Str, false))),
        print(vec![vs("u")]),
        let_("m", sv.clone(), rcall(call_u("Servico.mais", vec![v("api", sv.clone()), i(5)], sv.clone(), false))),
        print(vec![sel(v("m", sv.clone()), "nome", Ty::Str), sel(v("m", sv.clone()), "porta", Ty::Int), index(sel(v("m", sv.clone()), "deps", lstr()), i(1))]),
        let_("vz", lstr(), lit_list(Ty::Str, vec![])),
        let_("web", sv.clone(), srv("web", 3000, vs_list("vz"))),
        var_("ss", lsv.clone(), lit_list(sv.clone(), vec![v("api", sv.clone()), v("web", sv.clone())])),
        do_(call_m(lsv.clone(), "push", vec![a(v("ss", lsv.clone())), a(v("m", sv.clone()))], Ty::Unit)),
        print(vec![len(v("ss", lsv.clone())), sel(index(v("ss", lsv.clone()), i(1)), "nome", Ty::Str), len(sel(index(v("ss", lsv.clone()), i(0)), "deps", lstr()))]),
        st(StmtKind::ForList {
            var: "x".into(),
            index: None,
            list: v("ss", lsv.clone()),
            body: blk(vec![
                let_("ux", Ty::Str, rcall(call_u("Servico.url", vec![v("x", sv.clone())], Ty::Str, false))),
                let_("jd", Ty::Str, rcall(m_list("join", vec![a(sel(v("x", sv.clone()), "deps", lstr())), a(s("/"))], Ty::Str))),
                print(vec![vs("ux"), vs("jd")]),
            ]),
        }),
        let_(
            "altos",
            lsv.clone(),
            rcall(call_m(lsv.clone(), "filter", vec![a(v("ss", lsv.clone())), lam(vec![("q", sv.clone())], Ty::Bool, vec![ret(bin(BinOp::Gt, sel(v("q", sv.clone()), "porta", Ty::Int), i(5000)))])], lsv.clone())),
        ),
        let_("nomes", lstr(), rcall(call_m(lsv.clone(), "map", vec![a(v("altos", lsv.clone())), lam(vec![("q", sv.clone())], Ty::Str, vec![ret(sel(v("q", sv.clone()), "nome", Ty::Str))])], lstr()))),
        let_("jn", Ty::Str, rcall(m_list("join", vec![a(vs_list("nomes")), a(s(","))], Ty::Str))),
        print(vec![vs("jn")]),
        let_(
            "ord",
            lsv.clone(),
            rcall(call_m(lsv.clone(), "sort_by", vec![a(v("ss", lsv.clone())), lam(vec![("q", sv.clone())], Ty::Int, vec![ret(sel(v("q", sv.clone()), "porta", Ty::Int))])], lsv.clone())),
        ),
        print(vec![sel(index(v("ord", lsv.clone()), i(0)), "nome", Ty::Str), sel(index(v("ord", lsv.clone()), i(2)), "porta", Ty::Int)]),
        let_("w", sv.clone(), rcall(call_m(lsv.clone(), "find", vec![a(v("ss", lsv.clone())), lam(vec![("q", sv.clone())], Ty::Bool, vec![ret(bin(BinOp::Eq, sel(v("q", sv.clone()), "nome", Ty::Str), s("web")))])], sv.clone()))),
        print(vec![sel(v("w", sv.clone()), "porta", Ty::Int)]),
        let_("tem", Ty::Bool, rcall(call_m(lsv.clone(), "contains", vec![a(v("ss", lsv.clone())), a(v("web", sv.clone()))], Ty::Bool))),
        let_("pos", Ty::Int, rcall(call_m(lsv.clone(), "index_of", vec![a(v("ss", lsv.clone())), a(v("m", sv.clone()))], Ty::Int))),
        print(vec![v("tem", Ty::Bool), vi("pos")]),
        st(StmtKind::Assign { target: LValue::Index { list: "ss".into(), index: i(0) }, value: ex(v("web", sv.clone())) }),
        let_("un", lsv.clone(), rcall(call_m(lsv.clone(), "unique", vec![a(v("ss", lsv.clone()))], lsv.clone()))),
        print(vec![len(v("un", lsv.clone())), sel(index(v("ss", lsv.clone()), i(0)), "nome", Ty::Str)]),
    ];
    check(
        &p,
        &[],
        "8080 1 2\nhttp://api:8080\napi 8085 cache x\n3 web 2\nhttp://api:8080 db/cache x\nhttp://web:3000 \nhttp://api:8085 db/cache x\napi,api\nweb 8085\n3000\ntrue 2\n2 web\n",
        0,
    );
}

fn mapa() -> Ty {
    Ty::Map(Box::new(Ty::Str), Box::new(Ty::Int))
}

#[test]
fn maps() {
    let mt = mapa();
    let mm = |n: &str| v(n, mapa());
    let get = |m: &str, k: Expr| call_m(mapa(), "get", vec![a(v(m, mapa())), a(k)], Ty::Int);
    let mut p = prog(vec![
        var_("m", mt.clone(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![(s("api"), i(8080)), (s("web"), i(3000))] }),
        st(StmtKind::Assign { target: LValue::MapEntry { map: "m".into(), key: s("worker") }, value: ex(i(9090)) }),
        st(StmtKind::Assign { target: LValue::MapEntry { map: "m".into(), key: s("api") }, value: ex(i(1)) }),
        let_("a1", Ty::Int, rcall(get("m", s("api")))),
        let_("a2", Ty::Int, rcall_or(get("m", s("nao")), OnFail::Default(Box::new(ex(i(-1)))))),
        print(vec![vi("a1"), vi("a2"), len(mm("m"))]),
        let_("c", mt.clone(), ex(mm("m"))),
        do_(call_m(mapa(), "remove", vec![a(mm("m")), a(s("web"))], Ty::Unit)),
        do_(call_m(mapa(), "remove", vec![a(mm("m")), a(s("nao existe"))], Ty::Unit)),
        do_(call_m(mapa(), "set", vec![a(mm("m")), a(s("web")), a(i(7))], Ty::Unit)),
        let_("h1", Ty::Bool, rcall(call_m(mapa(), "has", vec![a(mm("m")), a(s("web"))], Ty::Bool))),
        let_("h2", Ty::Bool, rcall(call_m(mapa(), "has", vec![a(mm("c")), a(s("x"))], Ty::Bool))),
        let_("ks", lstr(), rcall(call_m(mapa(), "keys", vec![a(mm("m"))], lstr()))),
        let_("jk", Ty::Str, rcall(m_list("join", vec![a(vs_list("ks")), a(s(","))], Ty::Str))),
        print(vec![v("h1", Ty::Bool), v("h2", Ty::Bool), vs("jk"), len(mm("c"))]),
        st(StmtKind::ForMap { key: "k".into(), value: "val".into(), map: mm("c"), body: blk(vec![print(vec![vs("k"), vi("val")])]) }),
        let_("w2", Ty::Int, rcall(get("c", s("web")))),
        let_("w3", Ty::Int, rcall(get("m", s("web")))),
        print(vec![vi("w2"), vi("w3")]),
        // odd keys
        var_("e", mt.clone(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![] }),
    ]);
    let weird = ["", " ", "a b", "aspas \"x\" 'y'", "$HOME", "*", "[x]", "ç é 👍", "a\nb", "\\", "-n", "a_b", "a_5fb", "_"];
    for (n, k) in weird.iter().enumerate() {
        p.body.stmts.push(st(StmtKind::Assign { target: LValue::MapEntry { map: "e".into(), key: s(k) }, value: ex(i(n as i64)) }));
    }
    let mut want = String::from("1 -1 3\ntrue false api,worker,web 3\napi 1\nweb 3000\nworker 9090\n3000 7\n");
    for (n, k) in weird.iter().enumerate() {
        p.body.stmts.push(let_(&format!("g{}", n), Ty::Int, rcall(get("e", s(k)))));
        p.body.stmts.push(print(vec![vi(&format!("g{}", n))]));
        want.push_str(&format!("{}\n", n));
    }
    p.body.stmts.push(print(vec![len(mm("e"))]));
    want.push_str(&format!("{}\n", weird.len()));
    let o = check(&p, &[], &want, 0);
    // error for a missing key
    let p2 = prog(vec![let_("m", mt.clone(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![(s("a"), i(1))] }), let_("x", Ty::Int, rcall(get("m", s("b"))))]);
    let o2 = check(&p2, &[], "", 1);
    assert_eq!(o2.stderr, "error: key 'b' does not exist\n");
    let _ = o;
}

#[test]
fn maps_in_functions() {
    let mt = mapa();
    let mut p = prog(vec![]);
    // count(words) -> map
    p.funcs.push(func(
        "conta",
        vec![("ps", lstr())],
        mt.clone(),
        vec![
            var_("m", mt.clone(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![] }),
            st(StmtKind::ForList {
                var: "w".into(),
                index: None,
                list: vs_list("ps"),
                body: blk(vec![
                    let_("atual", Ty::Int, rcall_or(call_m(mapa(), "get", vec![a(v("m", mapa())), a(vs("w"))], Ty::Int), OnFail::Default(Box::new(ex(i(0)))))),
                    st(StmtKind::Assign { target: LValue::MapEntry { map: "m".into(), key: vs("w") }, value: ex(bin(BinOp::Add, vi("atual"), i(1))) }),
                ]),
            }),
            ret(v("m", mt.clone())),
        ],
        false,
    ));
    p.funcs.push(func(
        "resumo",
        vec![("m", mt.clone())],
        Ty::Str,
        vec![
            var_("partes", lstr(), lit_list(Ty::Str, vec![])),
            st(StmtKind::ForMap { key: "k".into(), value: "x".into(), map: v("m", mapa()), body: blk(vec![do_(m_list("push", vec![a(vs_list("partes")), a(interp(vec![vs("k"), s("="), vi("x")]))], Ty::Unit))]) }),
            let_("r", Ty::Str, rcall(m_list("join", vec![a(vs_list("partes")), a(s(" "))], Ty::Str))),
            ret(vs("r")),
        ],
        false,
    ));
    p.body.stmts = vec![
        let_("ws", lstr(), lit_list(Ty::Str, vec![s("x"), s("y"), s("x"), s("z z"), s("x")])),
        let_("c", mt.clone(), rcall(call_u("conta", vec![vs_list("ws")], mt.clone(), false))),
        let_("r", Ty::Str, rcall(call_u("resumo", vec![v("c", mapa())], Ty::Str, false))),
        print(vec![vs("r")]),
        let_("c2", mt.clone(), rcall(call_u("conta", vec![vs_list("ws")], mt.clone(), false))),
        let_("x2", Ty::Int, rcall(call_m(mapa(), "get", vec![a(v("c2", mapa())), a(s("z z"))], Ty::Int))),
        print(vec![vi("x2"), len(v("c2", mapa()))]),
    ];
    let o = check(&p, &[], "x=3 y=1 z z=1\n1 3\n", 0);
    // no index variable leaks (macOS): those of the local map are unset at the end of the function
    if host_target() == Target::Macos {
        assert!(o.script.contains("unset \"${m__n[@]}\""), "{}", o.script);
    }
}

fn cap(c: Command) -> Rvalue {
    Rvalue::Capture { cmd: c, on_fail: OnFail::Propagate }
}

#[test]
fn commands() {
    let mut pipe = lit_cmd("printf a\\nb\\nc\\n");
    pipe.pipeline[0].words = vec![word_lit("printf"), word_lit("%s\\n"), word_lit("a"), word_lit("b b"), word_lit("c")];
    pipe.pipeline.push(SimpleCmd::new(vec![word_lit("sort"), word_lit("-r")]));
    pipe.text = "printf '%s\\n' a 'b b' c | sort -r".into();
    let mut red = cmd(vec![word_lit("echo"), word_e(vs("x"))], "echo {x}");
    red.redirects.push(Redirect::Out { path: s("saida.txt"), append: false });
    let mut red2 = cmd(vec![word_lit("echo"), word_lit("mais")], "echo mais");
    red2.redirects.push(Redirect::Out { path: s("saida.txt"), append: true });
    let mut stdin_c = cmd(vec![word_lit("wc"), word_lit("-c")], "wc -c");
    stdin_c.stdin = Some(s("abc"));
    let p = prog(vec![
        let_("x", Ty::Str, ex(s("a  b;$c"))),
        let_("xs", lstr(), lit_list(Ty::Str, vec![s("1 1"), s("2")])),
        run_cmd(cmd(vec![word_lit("printf"), word_lit("[%s]"), word_e(vs("x")), Word::Spread(vs_list("xs")), Word::Interp(vec![Part::Lit("k=".into()), Part::Expr(vs("x"))])], "printf [%s] {x} {xs} k={x}")),
        print(vec![]),
        run_cmd(pipe),
        run_cmd(red),
        run_cmd(red2),
        let_("t", Ty::Str, cap(cmd(vec![word_lit("cat"), word_lit("saida.txt")], "cat saida.txt"))),
        print(vec![vs("t")]),
        let_("n", Ty::Str, cap(stdin_c)),
        print(vec![interp(vec![s("<"), vs("n"), s(">")])]),
        let_("ok", Ty::Bool, Rvalue::CommandOk(lit_cmd("test -f saida.txt"))),
        let_("nok", Ty::Bool, Rvalue::CommandOk(lit_cmd("test -d saida.txt"))),
        print(vec![v("ok", Ty::Bool), v("nok", Ty::Bool)]),
        let_("f", Ty::Str, Rvalue::Capture { cmd: lit_cmd("false"), on_fail: OnFail::Default(Box::new(ex(s("padrão")))) }),
        print(vec![vs("f")]),
        st(StmtKind::Command { cmd: cmd(vec![word_lit("sh"), word_lit("-c"), word_lit("exit 3")], "sh -c 'exit 3'"), on_fail: OnFail::Block { err: Some("e".into()), body: blk(vec![print(vec![field(v("e", Ty::Error), "code", Ty::Int)])]), value: None } }),
        st(StmtKind::Command { cmd: lit_cmd("nao-existe-xyz"), on_fail: OnFail::Ignore }),
        run_cmd(lit_cmd("cat /nao/existe/mesmo")),
        print(vec![s("nunca")]),
    ]);
    // BSD `wc` pads the count; GNU and BusyBox don't. `cat` of a missing file exits 1 everywhere.
    let wc = if cfg!(target_os = "macos") { "       3" } else { "3" };
    let want = format!("[a  b;$c][1 1][2][k=a  b;$c]\nc\nb b\na\na  b;$c\nmais\n<{}>\ntrue false\npadrão\n3\n", wc);
    let o = check(&p, &[], &want, 1);
    assert!(o.stderr.ends_with("error: command `cat /nao/existe/mesmo` failed with exit code 1\n"), "{}", o.stderr);
    let p2 = prog(vec![run_cmd(lit_cmd("nao-existe-xyz --x"))]);
    let o2 = check(&p2, &[], "", 127);
    assert!(o2.stderr.ends_with("error: command `nao-existe-xyz` was not found\n"), "{}", o2.stderr);
}

#[test]
fn dry_run_and_verbose() {
    let mut p = prog(vec![]);
    p.params.push(Param { name: "token".into(), flag: "--token".into(), ty: Ty::Secret, default: None, doc: String::new(), env_var: Some("NALU_TOKEN".into()) });
    let mut login = cmd(vec![word_lit("cat")], "cat");
    login.stdin = Some(v("token", Ty::Secret));
    p.body.stmts = vec![
        run_cmd(cmd(vec![word_lit("echo"), word_lit("oi"), word_e(v("token", Ty::Secret))], "echo oi {token}")),
        let_("cap", Ty::Str, cap(lit_cmd("echo capturado"))),
        print(vec![vs("cap"), st_dry()]),
        run_cmd(login),
        print(vec![]),
    ];
    p.deps = vec!["nao-existe-dep".into()];
    let o = check(&p, &["--dry-run", "--token", "s3gr3do"], "capturado true\n\n", 0);
    assert_eq!(o.stderr, "[dry-run] echo oi ******\n[dry-run] cat\n");
    let o = check(&p, &["--token", "x"], "", 127);
    assert_eq!(o.stderr, "error: this script needs: nao-existe-dep\n");
    p.deps.clear();
    let o = check(&p, &["--verbose", "--token", "s3gr3do"], "oi s3gr3do\ncapturado false\ns3gr3do\n", 0);
    assert_eq!(o.stderr, "+ echo oi ******\n+ echo capturado\n+ cat\n");
    let o = check(&p, &[], "", 2);
    assert!(o.stderr.contains("--token is required"), "{}", o.stderr);
}

fn st_dry() -> Expr {
    Expr { kind: ExprKind::DryRun, ty: Ty::Bool }
}

#[test]
fn defer_order_failure_and_exit() {
    let mut p = prog(vec![]);
    let pr = |m: &str| Box::new(print(vec![s(m)]));
    p.funcs.push(func(
        "trabalho",
        vec![("n", Ty::Int)],
        Ty::Unit,
        vec![
            st(StmtKind::Defer(Box::new(print(vec![interp(vec![s("defer f1 "), vi("n")])])))),
            st(StmtKind::Defer(pr("defer f2"))),
            if_(bin(BinOp::Eq, vi("n"), i(1)), vec![fail("falhou")], None),
            if_(bin(BinOp::Eq, vi("n"), i(2)), vec![st(StmtKind::Exit(Some(i(4))))], None),
            print(vec![s("corpo")]),
        ],
        true,
    ));
    p.body.stmts = vec![
        var_("x", Ty::Str, ex(s("antes"))),
        st(StmtKind::Defer(Box::new(print(vec![vs("x")])))),
        set("x", ex(s("depois"))),
        st(StmtKind::Defer(pr("topo 2"))),
        do_(call_u("trabalho", vec![i(0)], Ty::Unit, true)),
        do_or(call_u("trabalho", vec![i(1)], Ty::Unit, true), OnFail::Ignore),
        do_(call_u("trabalho", vec![i(2)], Ty::Unit, true)),
        print(vec![s("nunca")]),
    ];
    check(&p, &[], "corpo\ndefer f2\ndefer f1 0\ndefer f2\ndefer f1 1\ndefer f2\ndefer f1 2\ntopo 2\nantes\n", 4);
    // a failure at top level also runs the defers
    let p2 = prog(vec![st(StmtKind::Defer(pr("limpo"))), fail("x")]);
    let o = check(&p2, &[], "limpo\n", 1);
    assert_eq!(o.stderr, "error: x\n");
}

#[test]
fn retry_tries_again() {
    let p = prog(vec![
        var_("n", Ty::Int, ex(i(0))),
        st(StmtKind::Retry {
            times: i(5),
            every: dur(10),
            body: blk(vec![set("n", ex(bin(BinOp::Add, vi("n"), i(1)))), print(vec![interp(vec![s("tentativa "), vi("n")])]), if_(bin(BinOp::Lt, vi("n"), i(3)), vec![fail("ainda não")], None)]),
            on_fail: OnFail::Propagate,
        }),
        print(vec![s("ok")]),
        st(StmtKind::Retry { times: i(2), every: dur(0), body: blk(vec![fail("sempre")]), on_fail: OnFail::Block { err: None, body: blk(vec![print(vec![cur_err("message")])]), value: None } }),
        st(StmtKind::Retry { times: i(2), every: dur(0), body: blk(vec![fail("de vez")]), on_fail: OnFail::Propagate }),
    ]);
    let o = check(&p, &[], "tentativa 1\ntentativa 2\ntentativa 3\nok\nsempre\n", 1);
    assert_eq!(o.stderr, "error: de vez\n");
}

#[test]
fn parallel_order_and_errors() {
    let mut p = prog(vec![]);
    p.funcs.push(func(
        "quadrado",
        vec![("n", Ty::Int)],
        Ty::Int,
        vec![if_(bin(BinOp::Eq, vi("n"), i(3)), vec![fail("três não")], None), run_cmd(lit_cmd("sleep 0.0")), ret(bin(BinOp::Mul, vi("n"), vi("n")))],
        true,
    ));
    let ns = |items: Vec<i64>| lit_list(Ty::Int, items.into_iter().map(i).collect());
    p.body.stmts = vec![
        let_("xs", lint(), ns(vec![4, 1, 2, 5])),
        var_("rs", lint(), ns(vec![])),
        st(StmtKind::Parallel {
            max: Some(i(2)),
            body: ParallelBody::For {
                var: "x".into(),
                list: v("xs", lint()),
                body: blk(vec![let_("q", Ty::Int, rcall(call_u("quadrado", vec![vi("x")], Ty::Int, true)))]),
                collect: Some(("rs".into(), vi("q"))),
            },
            on_fail: OnFail::Propagate,
        }),
        let_("j", Ty::Str, rcall(call_m(lint(), "join", vec![a(v("rs", lint())), a(s(","))], Ty::Str))),
        print(vec![vs("j")]),
        let_("ys", lint(), ns(vec![3, 2, 3])),
        st(StmtKind::Parallel {
            max: None,
            body: ParallelBody::For { var: "y".into(), list: v("ys", lint()), body: blk(vec![do_(call_u("quadrado", vec![vi("y")], Ty::Int, true))]), collect: None },
            on_fail: OnFail::Block { err: None, body: blk(vec![print(vec![cur_err("message")])]), value: None },
        }),
        st(StmtKind::Parallel { max: None, body: ParallelBody::Tasks(vec![blk(vec![print(vec![s("t1")])]), blk(vec![fail("t2 falhou")])]), on_fail: OnFail::Propagate }),
    ];
    let o = check(&p, &[], "16,1,4,25\nparallel: três não; três não\nt1\n", 1);
    assert_eq!(o.stderr, "error: parallel: t2 falhou\n");
}

#[test]
fn parameters() {
    let mut p = prog(vec![]);
    p.enums.push(EnumDef { name: "Amb".into(), cases: vec!["dev".into(), "stage".into(), "prod".into()] });
    let pr = |n: &str, ty: Ty, d: Option<Rvalue>| Param { name: n.into(), flag: format!("--{}", n.replace('_', "-")), ty, default: d, doc: String::new(), env_var: None };
    p.params = vec![
        pr("amb", Ty::Enum("Amb".into()), Some(ex(Expr { kind: ExprKind::EnumCase { enum_: "Amb".into(), case: "dev".into() }, ty: Ty::Enum("Amb".into()) }))),
        pr("vezes", Ty::Int, Some(ex(i(1)))),
        pr("espera", Ty::Duration, Some(ex(dur(30000)))),
        pr("limpar_cache", Ty::Bool, None),
        pr("pastas", lstr(), Some(lit_list(Ty::Str, vec![s("docs")]))),
        pr("ns", lint(), Some(lit_list(Ty::Int, vec![]))),
        pr("versao", Ty::Str, None),
    ];
    p.body.stmts = vec![
        let_("d", Ty::Str, rcall(call_b("duration_str", vec![v("espera", Ty::Duration)], Ty::Str))),
        let_("jp", Ty::Str, rcall(m_list("join", vec![a(vs_list("pastas")), a(s("+"))], Ty::Str))),
        let_("sn", Ty::Int, rcall(call_m(lint(), "sum", vec![a(v("ns", lint()))], Ty::Int))),
        let_("ja", Ty::Str, rcall(m_list("join", vec![a(v("args", lstr())), a(s("+"))], Ty::Str))),
        print(vec![v("amb", Ty::Enum("Amb".into())), vi("vezes"), vs("d"), v("limpar_cache", Ty::Bool), vs("jp"), vi("sn"), vs("versao"), vs("ja")]),
    ];
    // `args` is ExprKind::Args
    for s in p.body.stmts.iter_mut() {
        if let StmtKind::Let { value: Rvalue::Call { call, .. }, .. } = &mut s.kind {
            if let Some(Arg::Expr(e)) = call.args.get_mut(0) {
                if matches!(&e.kind, ExprKind::Var(n) if n == "args") {
                    e.kind = ExprKind::Args;
                }
            }
        }
    }
    check(&p, &["--versao", "1.0"], "dev 1 30s false docs 0 1.0 \n", 0);
    check(
        &p,
        &["--amb=prod", "--vezes", "-3", "--espera", "1m30s", "--limpar-cache", "--pastas", "a,b c,", "--ns=1,2,-3", "--versao=x", "p1", "--", "--p2"],
        "prod -3 1m30s true a+b c+ 0 x p1+--p2\n",
        0,
    );
    check(&p, &["--versao", "v", "--espera=1.5s", "--limpar-cache=false"], "dev 1 1.5s false docs 0 v \n", 0);
    let o = check(&p, &["--versao", "v", "--amb", "qa"], "", 2);
    assert!(o.stderr.contains("--amb must be one of: dev, stage, prod (got 'qa')"), "{}", o.stderr);
    let o = check(&p, &["--versao", "v", "--vezes", "12x"], "", 2);
    assert!(o.stderr.contains("--vezes must be an integer"), "{}", o.stderr);
    let o = check(&p, &["--versao", "v", "--vezes", "99999999999999999999"], "", 2);
    assert!(o.stderr.contains("--vezes must be an integer"), "{}", o.stderr);
    let o = check(&p, &["--versao", "v", "--espera", "5"], "", 2);
    assert!(o.stderr.contains("--espera must be a duration"), "{}", o.stderr);
    let o = check(&p, &[], "", 2);
    assert!(o.stderr.contains("--versao is required"), "{}", o.stderr);
    let o = check(&p, &["--versao"], "", 2);
    assert!(o.stderr.contains("--versao needs a value"), "{}", o.stderr);
}

#[test]
fn test_mode() {
    let mut p = prog(vec![let_("base", Ty::Int, ex(i(10))), run_cmd(lit_cmd("echo nao-roda"))]);
    p.funcs.push(func(
        "dividir",
        vec![("a", Ty::Int), ("b", Ty::Int)],
        Ty::Int,
        vec![if_(bin(BinOp::Eq, vi("b"), i(0)), vec![fail("divisão por zero")], None), ret(bin(BinOp::Div, vi("a"), vi("b")))],
        true,
    ));
    let dv = |x: i64, y: i64| call_u("dividir", vec![i(x), i(y)], Ty::Int, true);
    p.tests = vec![
        TestDef { name: "divide".into(), body: blk(vec![let_("r", Ty::Int, rcall(dv(10, 3))), st(StmtKind::Assert { cond: bin(BinOp::Eq, vi("r"), i(3)), text: "dividir(10, 3) == 3".into() })]) },
        TestDef { name: "usa o topo".into(), body: blk(vec![st(StmtKind::Assert { cond: bin(BinOp::Eq, vi("base"), i(11)), text: "base == 11".into() })]) },
        TestDef { name: "zero falha".into(), body: blk(vec![st(StmtKind::AssertFails(Box::new(do_(dv(1, 0)))))]) },
        TestDef { name: "não falha".into(), body: blk(vec![st(StmtKind::AssertFails(Box::new(do_(dv(1, 1)))))]) },
        TestDef { name: "erro solto".into(), body: blk(vec![do_(dv(1, 0))]) },
        TestDef { name: "sai".into(), body: blk(vec![st(StmtKind::Exit(Some(i(3))))]) },
    ];
    let sc = generate(&p, Mode::Tests);
    let o = run_script(&sc, &[], None);
    assert_eq!(
        o.stdout,
        "ok      divide\nFAILED  usa o topo: assert failed: base == 11\nok      zero falha\nFAILED  não falha: assert_fails: the call did not fail\nFAILED  erro solto: divisão por zero\nFAILED  sai: the test called exit(3)\n6 test(s), 4 failure(s)\n",
        "{}",
        numbered(&sc)
    );
    assert_eq!(o.code, 1);
}

// ----------------------------------------------------------------------------------------------
// Standard library coverage
// ----------------------------------------------------------------------------------------------

fn ty_of(t: &str) -> Ty {
    match t {
        "str" | "T" | "U" | "K" | "any" | "..." => Ty::Str,
        "int" | "V" => Ty::Int,
        "bool" => Ty::Bool,
        "path" => Ty::Path,
        "duration" => Ty::Duration,
        "secret" => Ty::Secret,
        "unit" => Ty::Unit,
        "Enum" => Ty::Enum("Cor".into()),
        "[]str" | "[]T" | "[]U" | "[]K" | "[]path" => lstr(),
        "[]V" => lint(),
        _ => Ty::Str,
    }
}

fn dummy(t: &str) -> Arg {
    match t {
        "int" => a(i(1)),
        "bool" => a(b(true)),
        "duration" => a(dur(1000)),
        "path" => a(Expr { kind: ExprKind::Str("/tmp/x".into()), ty: Ty::Path }),
        "[]str" => a(vs_list("lista")),
        "(T) -> bool" => lam(vec![("e", Ty::Str)], Ty::Bool, vec![ret(b(true))]),
        "(T) -> U" => lam(vec![("e", Ty::Str)], Ty::Str, vec![ret(vs("e"))]),
        "K" | "T" | "str" | "secret" | "..." | "any" => a(s("x")),
        "V" => a(i(2)),
        _ => a(s("x")),
    }
}

fn coverage_program(target: Target) -> (Program, Vec<String>) {
    let mut p = Program::empty("cob", target);
    p.enums.push(EnumDef { name: "Cor".into(), cases: vec!["azul".into()] });
    let mut names = Vec::new();
    p.body.stmts.push(var_("lista", lstr(), lit_list(Ty::Str, vec![s("a")])));
    p.body.stmts.push(var_("mapa", mapa(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![(s("a"), i(1))] }));
    let mut push = |p: &mut Program, name: String, c: Call| {
        names.push(name);
        let k = p.body.stmts.len();
        if c.ret == Ty::Unit {
            p.body.stmts.push(do_or(c, OnFail::Ignore));
        } else {
            let ty = c.ret.clone();
            p.body.stmts.push(let_(&format!("r{}", k), ty, Rvalue::Call { call: c, on_fail: OnFail::Propagate }));
        }
    };
    for f in crate::stdlib::STDLIB.iter().filter(|f| f.v1) {
        let args: Vec<Arg> = f.params.iter().map(|x| dummy(x.ty)).collect();
        let c = Call { target: CallTarget::Std { module: f.module.into(), name: f.name.into() }, args, ret: ty_of(f.ret), fallible: f.fallible };
        push(&mut p, format!("{}.{}", f.module, f.name), c);
    }
    for f in crate::stdlib::BUILTINS.iter().filter(|f| f.v1) {
        let args: Vec<Arg> = match f.name {
            "duration_str" => vec![a(v("dd", Ty::Duration))],
            "enc_list" => vec![a(vs_list("lista"))],
            _ => f.params.iter().map(|x| dummy(x.ty)).collect(),
        };
        let ret = if f.name == "enum_parse" { Ty::Enum("Cor".into()) } else { ty_of(f.ret) };
        let c = Call { target: CallTarget::Builtin(f.name.into()), args, ret, fallible: f.fallible };
        push(&mut p, f.name.to_string(), c);
    }
    p.body.stmts.insert(0, let_("dd", Ty::Duration, ex(dur(1500))));
    for m in crate::stdlib::METHODS {
        let recv: (Ty, Arg) = match m.recv {
            "list" => (lstr(), a(vs_list("lista"))),
            "map" => (mapa(), a(v("mapa", mapa()))),
            "enum" => (Ty::Enum("Cor".into()), a(Expr { kind: ExprKind::EnumCase { enum_: "Cor".into(), case: "azul".into() }, ty: Ty::Enum("Cor".into()) })),
            "int" => (Ty::Int, a(i(3))),
            "bool" => (Ty::Bool, a(b(false))),
            "duration" => (Ty::Duration, a(v("dd", Ty::Duration))),
            _ => (Ty::Str, a(s(" Texto "))),
        };
        let mut args = vec![recv.1];
        if !(m.recv == "enum" && m.name == "parse") {
            args.extend(m.params.iter().map(|x| dummy(x.ty)));
        } else {
            args = vec![a(s("azul"))];
        }
        let ret = match (m.recv, m.ret) {
            ("list", "T") => Ty::Str,
            (_, r) => ty_of(r),
        };
        let c = Call { target: CallTarget::Method { recv_ty: recv.0, name: m.name.into() }, args, ret, fallible: m.fallible };
        push(&mut p, format!("{}.{}", m.recv, m.name), c);
    }
    for (prop, ret, _) in crate::stdlib::PATH_PROPS {
        let c = Call { target: CallTarget::Method { recv_ty: Ty::Path, name: prop.to_string() }, args: vec![a(Expr { kind: ExprKind::Str("/tmp".into()), ty: Ty::Path })], ret: ty_of(ret), fallible: false };
        push(&mut p, format!("path.{}", prop), c);
    }
    (p, names)
}

#[test]
fn stdlib_coverage() {
    for target in [Target::Macos, Target::Linux] {
        let (p, names) = coverage_program(target);
        // each entry on its own: the script must not have `__unimplemented`
        for (k, st_) in p.body.stmts.iter().enumerate().skip(3) {
            let mut q = p.clone();
            q.body.stmts = vec![p.body.stmts[0].clone(), p.body.stmts[1].clone(), p.body.stmts[2].clone(), st_.clone()];
            let sc = generate(&q, Mode::Script);
            assert!(!sc.contains("__unimplemented"), "{:?}: no implementation for {}", target, names[k - 3]);
        }
        let sc = generate(&p, Mode::Script);
        let path = scratch().join(format!("cobertura-{}.sh", target.os_name()));
        std::fs::write(&path, &sc).unwrap();
        let n = Proc::new("/bin/bash").arg("-n").arg(&path).output().unwrap();
        assert!(n.status.success(), "{}", String::from_utf8_lossy(&n.stderr));
    }
}

#[test]
fn planned_functions_fail_with_a_message() {
    // a function the backend does not know (the checker would refuse it with E090)
    let p = prog(vec![let_("t", Ty::Str, rcall(call_std("pkg", "uninstall_everything", vec![], Ty::Str)))]);
    let o = check(&p, &[], "", 70);
    assert_eq!(o.stderr, "error: pkg.uninstall_everything is not implemented yet\n");
}

#[test]
fn runtime_passes_bash_n() {
    for (name, src) in super::runtime::FILES {
        let path = scratch().join(format!("rt-{}", name));
        std::fs::write(&path, src).unwrap();
        let n = Proc::new("/bin/bash").arg("-n").arg(&path).output().unwrap();
        assert!(n.status.success(), "{}: {}", name, String::from_utf8_lossy(&n.stderr));
    }
    // each helper declares a target and dependencies that exist
    for linux in [true, false] {
        let all = super::runtime::for_target(linux);
        for h in &all {
            for d in &h.needs {
                assert!(all.iter().any(|x| &x.name == d), "{} needs {} (missing)", h.name, d);
            }
        }
    }
}

fn sm(n: &str, recv: Expr, args: Vec<Expr>, ret: Ty) -> Rvalue {
    let mut v = vec![a(recv)];
    v.extend(args.into_iter().map(a));
    rcall(call_m(Ty::Str, n, v, ret))
}

#[test]
fn text_methods() {
    let t = || vs("t");
    let jl = |n: &str| rcall(m_list("join", vec![a(vs_list(n)), a(s("|"))], Ty::Str));
    let p = prog(vec![
        let_("t", Ty::Str, ex(s("  Ação é Boa \t"))),
        let_("a1", Ty::Str, sm("trim", t(), vec![], Ty::Str)),
        let_("a2", Ty::Str, sm("trim_start", t(), vec![], Ty::Str)),
        let_("a3", Ty::Str, sm("trim_end", t(), vec![], Ty::Str)),
        print(vec![interp(vec![s("["), vs("a1"), s("]["), vs("a2"), s("]["), vs("a3"), s("]")])]),
        let_("u", Ty::Str, sm("upper", vs("a1"), vec![], Ty::Str)),
        let_("l", Ty::Str, sm("lower", vs("a1"), vec![], Ty::Str)),
        print(vec![vs("u"), vs("l")]),
        let_("c1", Ty::Bool, sm("contains", vs("a1"), vec![s("é B")], Ty::Bool)),
        let_("c2", Ty::Bool, sm("starts_with", vs("a1"), vec![s("Aç")], Ty::Bool)),
        let_("c3", Ty::Bool, sm("ends_with", vs("a1"), vec![s("*")], Ty::Bool)),
        print(vec![v("c1", Ty::Bool), v("c2", Ty::Bool), v("c3", Ty::Bool)]),
        let_("sp", Ty::Str, sm("strip_prefix", s("v1.2.3"), vec![s("v")], Ty::Str)),
        let_("ss", Ty::Str, sm("strip_suffix", s("a*b*"), vec![s("*")], Ty::Str)),
        let_("ix", Ty::Int, sm("index_of", vs("a1"), vec![s("é")], Ty::Int)),
        let_("nx", Ty::Int, sm("index_of", vs("a1"), vec![s("zz")], Ty::Int)),
        let_("sl", Ty::Str, sm("slice", vs("a1"), vec![i(1), i(4)], Ty::Str)),
        let_("sl2", Ty::Str, sm("slice", vs("a1"), vec![i(5), i(i64::MAX)], Ty::Str)),
        print(vec![vs("sp"), vs("ss"), vi("ix"), vi("nx"), vs("sl"), vs("sl2")]),
        let_("parts", lstr(), sm("split", s("a,,b,"), vec![s(",")], lstr())),
        let_("jp", Ty::Str, jl("parts")),
        let_("ls", lstr(), sm("lines", s("x\r\ny\n\nz\n"), vec![], lstr())),
        let_("jls", Ty::Str, jl("ls")),
        let_("ch", lstr(), sm("chars", s("añ👍"), vec![], lstr())),
        let_("jch", Ty::Str, jl("ch")),
        print(vec![vs("jp"), len(vs_list("parts")), vs("jls"), vs("jch")]),
        let_("r1", Ty::Str, sm("replace", s("a.b.c"), vec![s("."), s("&\\")], Ty::Str)),
        let_("r2", Ty::Str, sm("replace_first", s("a.b.c"), vec![s("."), s("*")], Ty::Str)),
        let_("rp", Ty::Str, sm("repeat", s("ab"), vec![i(3)], Ty::Str)),
        let_("pl", Ty::Str, sm("pad_left", s("7"), vec![i(3), s("0")], Ty::Str)),
        let_("pr", Ty::Str, sm("pad_right", s("ç"), vec![i(3), s(" ")], Ty::Str)),
        print(vec![vs("r1"), vs("r2"), vs("rp"), vs("pl"), interp(vec![s("["), vs("pr"), s("]")])]),
        let_("n1", Ty::Int, sm("to_int", s(" -0042 "), vec![], Ty::Int)),
        let_("n2", Ty::Int, sm("to_int", s("9223372036854775807"), vec![], Ty::Int)),
        let_("n3", Ty::Int, rcall_or(call_m(Ty::Str, "to_int", vec![a(s("9223372036854775808"))], Ty::Int), OnFail::Block { err: None, body: blk(vec![print(vec![cur_err("message")])]), value: Some(i(0)) })),
        let_("n4", Ty::Int, rcall_or(call_m(Ty::Str, "to_int", vec![a(s("1x"))], Ty::Int), OnFail::Default(Box::new(ex(i(-1)))))),
        print(vec![vi("n1"), vi("n2"), vi("n3"), vi("n4")]),
        let_("e1", Ty::Bool, sm("is_empty", s(""), vec![], Ty::Bool)),
        let_("e2", Ty::Bool, sm("is_empty", t(), vec![], Ty::Bool)),
        print(vec![v("e1", Ty::Bool), v("e2", Ty::Bool)]),
        let_("f", Ty::Str, rcall(call_b("fmt", vec![s("{} está na porta {}!"), s("api"), i(80)], Ty::Str))),
        let_("tpl", Ty::Str, ex(s("<{}|{}>"))),
        let_("f2", Ty::Str, rcall(call_b("fmt", vec![vs("tpl"), s("a"), s("b c")], Ty::Str))),
        print(vec![vs("f"), vs("f2")]),
        let_("d", Ty::Duration, ex(dur(0))),
        let_("ds", Ty::Str, rcall(call_m(Ty::Duration, "to_str", vec![a(v("d", Ty::Duration))], Ty::Str))),
    ]);
    let mut p = p;
    for (k, ms) in [90_000i64, 250, 1500, 93_784_005, -2000, 7 * 86_400_000].iter().enumerate() {
        p.body.stmts.push(let_(&format!("dx{}", k), Ty::Duration, ex(dur(*ms))));
        p.body.stmts.push(let_(&format!("dz{}", k), Ty::Str, rcall(call_b("duration_str", vec![v(&format!("dx{}", k), Ty::Duration)], Ty::Str))));
        p.body.stmts.push(print(vec![vs(&format!("dz{}", k)), s(&super::call::dur_str(*ms))]));
    }
    p.body.stmts.push(print(vec![vs("ds")]));
    check(
        &p,
        &[],
        "[Ação é Boa][Ação é Boa \t][  Ação é Boa]\nAÇÃO É BOA ação é boa\ntrue true false\n1.2.3 a*b 5 -1 ção é Boa\na||b| 4 x|y||z a|ñ|👍\na&\\b&\\c a*b.c ababab 007 [ç  ]\n'9223372036854775808' is not an integer\n-42 9223372036854775807 0 -1\ntrue false\napi está na porta 80! <a|b c>\n1m30s 1m30s\n250ms 250ms\n1.5s 1.5s\n1d2h3m4.005s 1d2h3m4.005s\n-2s -2s\n7d 7d\n0s\n",
        0,
    );
}

#[test]
fn paths_and_files() {
    let pth = |x: &str| Expr { kind: ExprKind::Str(x.into()), ty: Ty::Path };
    let prop = |n: &str, x: Expr, t: Ty| rcall(call_m(Ty::Path, n, vec![a(x)], t));
    let fsc = |n: &str, args: Vec<Expr>, ret: Ty| call_std("fs", n, args, ret);
    let tp = || v("tmp", Ty::Path);
    let j = |x: &str| bin(BinOp::PathJoin, tp(), s(x));
    let mut p = prog(vec![
        let_("n1", Ty::Str, prop("name", pth("/a/b.tar.gz/"), Ty::Str)),
        let_("n2", Ty::Str, prop("stem", pth("/a/b.tar.gz"), Ty::Str)),
        let_("n3", Ty::Str, prop("ext", pth("/a/b.tar.gz"), Ty::Str)),
        let_("n4", Ty::Str, prop("ext", pth(".bashrc"), Ty::Str)),
        let_("n5", Ty::Str, prop("stem", pth(".bashrc"), Ty::Str)),
        let_("n6", Ty::Path, prop("parent", pth("/a/b"), Ty::Path)),
        let_("n7", Ty::Path, prop("parent", pth("b"), Ty::Path)),
        let_("n8", Ty::Path, prop("parent", pth("/"), Ty::Path)),
        let_("n9", Ty::Path, prop("parent", pth("/x"), Ty::Path)),
        print(vec![vs("n1"), vs("n2"), vs("n3"), interp(vec![s("["), vs("n4"), s("]")]), vs("n5"), vs("n6"), vs("n7"), vs("n8"), vs("n9")]),
        let_("ab", Ty::Path, rcall(call_std("path", "abs", vec![pth("/a/./b/../c//d")], Ty::Path))),
        let_("pj", Ty::Path, ex(bin(BinOp::PathJoin, pth(""), s("b")))),
        print(vec![vs("ab"), vs("pj")]),
        let_("tmp", Ty::Path, rcall(fsc("temp_dir", vec![], Ty::Path))),
        st(StmtKind::Defer(Box::new(do_(fsc("rm", vec![tp()], Ty::Unit))))),
        do_(fsc("write", vec![j("a.txt"), s("\u{feff}linha 1\nlinha 2\n\n")], Ty::Unit)),
        do_(fsc("append", vec![j("a.txt"), s("fim")], Ty::Unit)),
        let_("txt", Ty::Str, rcall(fsc("read", vec![j("a.txt")], Ty::Str))),
        print(vec![len(vs("txt"))]),
        do_(fsc("mkdir", vec![j("sub/dir")], Ty::Unit)),
        do_(fsc("write", vec![j("sub/dir/z.log"), s("z")], Ty::Unit)),
        do_(fsc("write", vec![j(".oculto"), s("o")], Ty::Unit)),
        do_(fsc("write", vec![j("B.txt"), s("b")], Ty::Unit)),
        do_(fsc("copy", vec![j("a.txt"), j("c.txt")], Ty::Unit)),
        do_(fsc("move", vec![j("c.txt"), j("sub/c2.txt")], Ty::Unit)),
        do_(fsc("replace", vec![j("sub/c2.txt"), s("linha"), s("L")], Ty::Unit)),
        let_("c2", Ty::Str, rcall(fsc("read", vec![j("sub/c2.txt")], Ty::Str))),
        let_("c2l", lstr(), sm("lines", vs("c2"), vec![], lstr())),
        let_("c2j", Ty::Str, rcall(m_list("join", vec![a(vs_list("c2l")), a(s("/"))], Ty::Str))),
        print(vec![vs("c2j")]),
        let_("ls", lstr(), rcall(fsc("ls", vec![tp()], lstr()))),
        let_("jls", Ty::Str, rcall(m_list("join", vec![a(vs_list("ls")), a(s(","))], Ty::Str))),
        print(vec![vs("jls")]),
        let_("wk", lstr(), rcall(fsc("walk", vec![tp()], lstr()))),
        let_("rel", lstr(), rcall(m_list("map", vec![a(vs_list("wk")), lam(vec![("w", Ty::Str)], Ty::Str, vec![let_("q", Ty::Str, sm("strip_prefix", vs("w"), vec![tp()], Ty::Str)), ret(vs("q"))])], lstr()))),
        let_("jwk", Ty::Str, rcall(m_list("join", vec![a(vs_list("rel")), a(s(","))], Ty::Str))),
        print(vec![vs("jwk")]),
        let_("gb", lstr(), rcall(fsc("glob", vec![j("*.txt")], lstr()))),
        print(vec![len(vs_list("gb"))]),
        let_("sz", Ty::Int, rcall(fsc("size", vec![j("B.txt")], Ty::Int))),
        // `.size` of something that does not exist fails (D22); with `or 0`, it gives 0
        let_("sz2", Ty::Int, rcall_or(Call { fallible: true, ..fsc("size", vec![j("nao")], Ty::Int) }, OnFail::Default(Box::new(ex(i(0)))))),
        let_("ex1", Ty::Bool, rcall(fsc("exists", vec![j("sub")], Ty::Bool))),
        let_("ex2", Ty::Bool, rcall(fsc("is_file", vec![j("sub")], Ty::Bool))),
        let_("ex3", Ty::Bool, rcall(fsc("is_dir", vec![j("sub")], Ty::Bool))),
        let_("md", Ty::Int, rcall(fsc("modified", vec![j("B.txt")], Ty::Int))),
        print(vec![vi("sz"), vi("sz2"), v("ex1", Ty::Bool), v("ex2", Ty::Bool), v("ex3", Ty::Bool), bin(BinOp::Gt, vi("md"), i(1_600_000_000))]),
        do_or(fsc("rm", vec![pth(".")], Ty::Unit), OnFail::Block { err: None, body: blk(vec![print(vec![cur_err("message")])]), value: None }),
        do_or(fsc("read", vec![j("nao")], Ty::Str), OnFail::Block { err: None, body: blk(vec![let_("em", Ty::Str, ex(cur_err("message"))), let_("emok", Ty::Bool, sm("ends_with", vs("em"), vec![s("/nao' does not exist")], Ty::Bool)), print(vec![v("emok", Ty::Bool)])]), value: None }),
    ]);
    p.body.stmts.push(let_("home", Ty::Path, rcall(call_std("dirs", "home", vec![], Ty::Path))));
    p.body.stmts.push(let_("cfg", Ty::Path, rcall(call_std("dirs", "config", vec![s("nalu")], Ty::Path))));
    p.body.stmts.push(let_("cfgok", Ty::Bool, sm("starts_with", vs("cfg"), vec![vs("home")], Ty::Bool)));
    p.body.stmts.push(print(vec![v("cfgok", Ty::Bool)]));
    let o = check(
        &p,
        &[],
        "b.tar.gz b.tar gz [] .bashrc /a . / /\n/a/c/d b\n20\nL 1/L 2//fim\n.oculto,B.txt,a.txt,sub\n/.oculto,/B.txt,/a.txt,/sub/c2.txt,/sub/dir/z.log\n2\n1 0 true false true true\nfs.rm refused to delete '.'\ntrue\ntrue\n",
        0,
    )
    .stdout;
    let _ = o;
}

// ----------------------------------------------------------------------------------------------
// Benchmark (SPEC-002 criterion 5): run with `cargo test --lib bash::tests::benchmark -- --nocapture`
// ----------------------------------------------------------------------------------------------

pub fn bench_laco() -> Program {
    prog(vec![
        var_("s", Ty::Str, ex(s(""))),
        var_("n", Ty::Int, ex(i(0))),
        st(StmtKind::ForRange {
            var: "i".into(),
            from: i(0),
            to: i(20000),
            inclusive: false,
            body: blk(vec![set("n", ex(bin(BinOp::Add, vi("n"), vi("i")))), set("s", ex(interp(vec![s("item "), vi("i")])))]),
        }),
        print(vec![interp(vec![vi("n"), s(" "), vs("s")])]),
    ])
}

pub fn bench_mapa() -> Program {
    let k = || interp(vec![s("chave-"), vi("i")]);
    let k2 = || interp(vec![s("chave-"), vi("j")]);
    prog(vec![
        var_("m", mapa(), Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![] }),
        st(StmtKind::ForRange { var: "i".into(), from: i(0), to: i(2000), inclusive: false, body: blk(vec![do_(call_m(mapa(), "set", vec![a(v("m", mapa())), a(k()), a(vi("i"))], Ty::Unit))]) }),
        var_("s", Ty::Int, ex(i(0))),
        st(StmtKind::ForRange {
            var: "j".into(),
            from: i(0),
            to: i(2000),
            inclusive: false,
            body: blk(vec![
                let_("__t1", Ty::Int, rcall_or(call_m(mapa(), "get", vec![a(v("m", mapa())), a(k2())], Ty::Int), OnFail::Default(Box::new(ex(i(0)))))),
                set("s", ex(bin(BinOp::Add, vi("s"), vi("__t1")))),
            ]),
        }),
        print(vec![interp(vec![vi("s"), s(" "), len(v("m", mapa()))])]),
    ])
}

#[test]
fn benchmark() {
    for (name, p, want) in [("laço 20.000", bench_laco(), "199990000 item 19999\n"), ("mapa 2.000 chaves, 4.000 set/get", bench_mapa(), "1999000 2000\n")] {
        let t0 = std::time::Instant::now();
        let o = check(&p, &[], want, 0);
        let dt = t0.elapsed();
        let path = scratch().join(format!("bench-{}.sh", if name.starts_with("laço") { "laco" } else { "mapa" }));
        std::fs::write(&path, &o.script).unwrap();
        println!("benchmark {}: {:.3}s ({})", name, dt.as_secs_f64(), path.display());
        assert!(dt.as_secs_f64() < 2.0, "{} too slow: {:?}", name, dt);
    }
}

// ----------------------------------------------------------------------------------------------
// Real examples (frontend → IR → Bash)
// ----------------------------------------------------------------------------------------------

fn compile_example(name: &str, target: Target) -> Option<Program> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let src = std::fs::read_to_string(root.join(format!("{}.nalu", name))).ok()?;
    let c = crate::compile(&src, &format!("examples/{}.nalu", name), &root, target);
    c.program
}

/// Every included helper is called somewhere in the script.
fn assert_no_unused_helpers(sc: &str) {
    for l in sc.lines() {
        if let Some(name) = l.strip_prefix("__").and_then(|r| r.split("() {").next()).filter(|_| l.contains("() {")) {
            let n = format!("__{}", name);
            let uses = sc.matches(n.as_str()).count();
            assert!(uses >= 2, "helper {} incluído sem uso", n);
        }
    }
}

#[test]
fn examples_generate_valid_bash() {
    for name in ["hello", "backup", "deploy", "installer", "release"] {
        for target in [Target::Macos, Target::Linux] {
            let Some(p) = compile_example(name, target) else {
                eprintln!("frontend ainda não compila {}; pulando", name);
                return;
            };
            let sc = generate(&p, Mode::Script);
            assert!(sc.ends_with('\n'));
            let path = scratch().join(format!("ex-{}-{}.sh", name, target.os_name()));
            std::fs::write(&path, &sc).unwrap();
            let n = Proc::new("/bin/bash").arg("-n").arg(&path).output().unwrap();
            assert!(n.status.success(), "{} {:?}: {}", name, target, String::from_utf8_lossy(&n.stderr));
            assert_no_unused_helpers(&sc);
            if name == "hello" {
                assert!(sc.lines().count() <= 100, "hello.nalu gerou {} linhas para {:?}", sc.lines().count(), target);
            }
        }
    }
    let p = compile_example("hello", host_target()).unwrap();
    let o = run(&p, &["--name", "Ana", "--times", "2"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.ends_with("hello, Ana! (1/2)\nhello, Ana! (2/2)\n"), "{}", o.stdout);
}

#[test]
fn runtime_unit_tests() {
    let t = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/bash/tests.sh");
    let o = Proc::new("/bin/bash").arg(&t).arg(host_target().os_name()).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{}\n{}", out, String::from_utf8_lossy(&o.stderr));
    // on macOS with a Bash 4+ installed (Homebrew), also run the linux target
    if host_target() == Target::Linux {
        return;
    }
    for b in ["/usr/local/bin/bash", "/opt/homebrew/bin/bash", "/usr/bin/bash", "/bin/bash"] {
        let v = Proc::new(b).arg("-c").arg("(( BASH_VERSINFO[0] >= 4 ))").status();
        if matches!(v, Ok(s) if s.success()) {
            let o = Proc::new(b).arg(&t).arg("linux").output().unwrap();
            assert!(o.status.success(), "{} linux:\n{}", b, String::from_utf8_lossy(&o.stdout));
            break;
        }
    }
}

/// Generates the Linux scripts of the map tests for inspection (`cargo test dump_linux -- --ignored`).
#[test]
#[ignore]
fn dump_linux() {
    let mut p = bench_mapa();
    p.target = Target::Linux;
    std::fs::write(scratch().join("linux-mapa.sh"), generate(&p, Mode::Script)).unwrap();
    println!("{}", scratch().display());
}
