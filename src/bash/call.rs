//! Calls: user functions, builtins, standard library, methods (with lambdas) and `parallel`.

use super::gen::{fn_name, is_int, Gen, LoopKind};
use super::quote::{sq, W};
use super::val::{Cond, Dest, Loc, Val};
use crate::ir::*;

/// What a call produces: `op` (a command that can fail, with the error already set) and the value.
pub struct CallOut {
    pub op: Option<String>,
    pub val: Val,
}

fn out(op: Option<String>, val: Val) -> CallOut {
    CallOut { op, val }
}

fn ret_w() -> Val {
    Val::Word(W::var("__ret"))
}

fn ret_a() -> Val {
    Val::List("\"${__ret_a[@]}\"".into())
}

/// Duration in ms → text (`1m30s`), same as the `__dur_str` helper.
pub fn dur_str(ms: i64) -> String {
    if ms == 0 {
        return "0s".into();
    }
    let neg = ms < 0;
    let ms = ms.unsigned_abs();
    let mut o = String::new();
    let d = ms / 86_400_000;
    let h = ms / 3_600_000 % 24;
    let m = ms / 60_000 % 60;
    let s = ms / 1000 % 60;
    let r = ms % 1000;
    if d > 0 {
        o.push_str(&format!("{}d", d));
    }
    if h > 0 {
        o.push_str(&format!("{}h", h));
    }
    if m > 0 {
        o.push_str(&format!("{}m", m));
    }
    if ms < 1000 {
        o.push_str(&format!("{}ms", r));
    } else if r > 0 {
        let frac = format!("{:03}", r);
        o.push_str(&format!("{}.{}s", s, frac.trim_end_matches('0')));
    } else if s > 0 {
        o.push_str(&format!("{}s", s));
    }
    if neg {
        format!("-{}", o)
    } else {
        o
    }
}

/// Can any path of the block fail (propagate)?
pub fn block_may_fail(b: &Block) -> bool {
    b.stmts.iter().any(stmt_may_fail)
}

fn of_may_fail(of: &OnFail) -> bool {
    match of {
        OnFail::Propagate | OnFail::Fail { .. } => true,
        OnFail::Ignore => false,
        OnFail::Default(rv) => rv_may_fail(rv),
        OnFail::Block { body, .. } => block_may_fail(body),
    }
}

fn rv_may_fail(rv: &Rvalue) -> bool {
    match rv {
        Rvalue::Call { call, on_fail } => call.fallible && of_may_fail(on_fail),
        Rvalue::Capture { on_fail, .. } => of_may_fail(on_fail),
        _ => false,
    }
}

fn stmt_may_fail(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Let { value, .. } | StmtKind::Assign { value, .. } => rv_may_fail(value),
        StmtKind::Call { call, on_fail } => call.fallible && of_may_fail(on_fail),
        StmtKind::Command { on_fail, .. } => of_may_fail(on_fail),
        StmtKind::Fail { .. } => true,
        StmtKind::If { then, els, .. } => block_may_fail(then) || els.as_ref().map_or(false, block_may_fail),
        StmtKind::While { body, .. } | StmtKind::ForRange { body, .. } | StmtKind::ForList { body, .. } | StmtKind::ForMap { body, .. } => block_may_fail(body),
        StmtKind::Match { arms, default, .. } => arms.iter().any(|a| block_may_fail(&a.body)) || default.as_ref().map_or(false, block_may_fail),
        StmtKind::Try { on_fail, .. } | StmtKind::Retry { on_fail, .. } | StmtKind::Parallel { on_fail, .. } => of_may_fail(on_fail),
        _ => false,
    }
}

impl<'a> Gen<'a> {
    pub fn call_into(&mut self, dest: &Dest, ty: &Ty, call: &Call, on_fail: &OnFail) {
        let co = self.call(call);
        match co.op {
            None => self.store(dest, &co.val),
            Some(op) => {
                let val = co.val;
                let d = dest.clone();
                self.with_op(&op, on_fail, dest, ty, &mut |g| g.store(&d, &val));
            }
        }
    }

    pub fn call_stmt(&mut self, call: &Call, on_fail: &OnFail) {
        self.call_into(&Dest::Discard, &Ty::Unit, call, on_fail);
    }

    pub fn call(&mut self, c: &Call) -> CallOut {
        match &c.target {
            CallTarget::User(n) => self.user_call(n, c),
            CallTarget::Builtin(n) => self.builtin_call(n, c),
            CallTarget::Std { module, name } => self.std_call(module, name, c),
            CallTarget::Method { recv_ty, name } => self.method_call(recv_ty, name, c),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Arguments
    // ------------------------------------------------------------------------------------------

    fn arg<'c>(&self, c: &'c Call, i: usize) -> &'c Expr {
        match c.args.get(i) {
            Some(Arg::Expr(e)) => e,
            _ => panic!("argumento {} ausente ou lambda", i),
        }
    }

    /// Word (rendered) of a scalar argument.
    fn aw(&mut self, c: &Call, i: usize) -> String {
        let e = self.arg(c, i).clone();
        self.sw(&e)
    }

    /// Word (rendered) of a scalar expression (a computed bool becomes `true`/`false`).
    pub fn sw(&mut self, e: &Expr) -> String {
        match self.expr_val(e) {
            Val::Cond(c) => self.cond_word(&c).render(),
            Val::Word(w) => w.render(),
            _ => "''".into(),
        }
    }

    fn arg_w(&mut self, c: &Call, i: usize) -> W {
        let e = self.arg(c, i).clone();
        match self.expr_val(&e) {
            Val::Cond(c) => self.cond_word(&c),
            Val::Word(w) => w,
            _ => W::lit(""),
        }
    }

    /// Items of `print`/`log`: one word with the items separated by spaces.
    fn items_word(&mut self, c: &Call, from: usize) -> W {
        let mut w = W::default();
        for i in from..c.args.len() {
            if i > from {
                w.push_lit(" ");
            }
            let x = self.arg_w(c, i);
            w.push(x);
        }
        w
    }

    /// Name of the array of a list field of a struct (decodes it if the struct is a list item).
    fn field_arr(&mut self, loc: &Loc, f: &Field) -> String {
        match loc {
            Loc::Base(p) => format!("{}__{}", p, f.name),
            Loc::Elem { list, idx } => {
                let t = self.tmp(&f.ty);
                match &f.ty {
                    Ty::Map(_, _) => {
                        self.need("__dec_m");
                        self.line(format!("__dec_m \"${{{}__{}[{}]}}\"; {t}__k=(\"${{__ret_a[@]}}\"); {t}__v=(\"${{__ret_b[@]}}\")", list, f.name, idx, t = t));
                    }
                    _ => {
                        self.need("__dec_a");
                        self.line(format!("__dec_a \"${{{}__{}[{}]}}\"; {}=(\"${{__ret_a[@]}}\")", list, f.name, idx, t));
                    }
                }
                t
            }
        }
    }

    /// Arguments for a generated function (same order as `leaves`).
    pub fn flatten_loc(&mut self, loc: &Loc, ty: &Ty) -> Vec<String> {
        let mut o = Vec::new();
        match ty {
            Ty::Struct(s) => {
                for f in self.struct_fields(s) {
                    match &f.ty {
                        Ty::List(_) => {
                            let a = self.field_arr(loc, &f);
                            o.push(format!("\"${{#{}[@]}}\"", a));
                            o.push(format!("\"${{{}[@]}}\"", a));
                        }
                        Ty::Map(_, _) => {
                            let a = self.field_arr(loc, &f);
                            o.push(format!("\"${{#{}__k[@]}}\"", a));
                            o.push(format!("\"${{{}__k[@]}}\"", a));
                            o.push(format!("\"${{{}__v[@]}}\"", a));
                        }
                        _ => {
                            let v = self.field_val(loc, &f);
                            if let Val::Word(w) = v {
                                o.push(w.render());
                            }
                        }
                    }
                }
            }
            Ty::List(e) => {
                let Loc::Base(b) = loc else { return o };
                match &**e {
                    Ty::Struct(s) => {
                        for f in self.struct_fields(s) {
                            o.push(format!("\"${{#{}__{}[@]}}\"", b, f.name));
                            o.push(format!("\"${{{}__{}[@]}}\"", b, f.name));
                        }
                    }
                    _ => {
                        o.push(format!("\"${{#{}[@]}}\"", b));
                        o.push(format!("\"${{{}[@]}}\"", b));
                    }
                }
            }
            Ty::Map(_, _) => {
                let Loc::Base(b) = loc else { return o };
                o.push(format!("\"${{#{}__k[@]}}\"", b));
                o.push(format!("\"${{{}__k[@]}}\"", b));
                o.push(format!("\"${{{}__v[@]}}\"", b));
            }
            Ty::Error => match loc {
                Loc::Base(b) if b == "__err" => {
                    o.push("\"$__err_msg\"".into());
                    o.push("\"$__err_code\"".into());
                }
                Loc::Base(b) => {
                    o.push(format!("\"${}__message\"", b));
                    o.push(format!("\"${}__code\"", b));
                }
                _ => {}
            },
            _ => {}
        }
        o
    }

    /// Arguments (rendered) of an expression of any type, in the protocol of generated functions.
    pub fn flatten_arg(&mut self, e: &Expr) -> Vec<String> {
        if e.ty.is_scalar() {
            return vec![self.sw(e)];
        }
        let l = self.loc(e);
        self.flatten_loc(&l, &e.ty)
    }

    /// D25 (linux): the argument of a by-reference parameter is the **name** of an array that no called
    /// function hides: a top-level list, a local list (unique name `__aN_x`) or the target of a received
    /// reference (`${!__rN_x}`). Everything else (temporaries, fields) is copied to `__rvN`.
    fn ref_arg(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Var(n) => {
                if let Some((i, _)) = self.lookup(n) {
                    let shell = self.vname(n);
                    if i > 0 && self.frames[i].refs.contains(n) {
                        return format!("\"${{!{}}}\"", shell);
                    }
                    if i > 0 && shell.starts_with("__a") {
                        return shell;
                    }
                    if i == 0 && !n.starts_with("__") {
                        return shell;
                    }
                }
            }
            ExprKind::Args => return "args".into(),
            _ => {}
        }
        let arr = self.list_arr(e);
        self.counter += 1;
        let g = format!("__rv{}", self.counter);
        self.line(format!("{}=(\"${{{}[@]}}\")", g, arr));
        g
    }

    fn ret_val(&self, ty: &Ty) -> Val {
        match ty {
            Ty::Unit => Val::Unit,
            Ty::List(e) => match &**e {
                Ty::Struct(s) => Val::SList("__ret_a".into(), s.clone()),
                _ => ret_a(),
            },
            Ty::Struct(s) => Val::Struct(Loc::Base("__ret".into()), s.clone()),
            Ty::Map(_, _) => Val::Map("__ret_m".into()),
            _ => ret_w(),
        }
    }

    fn user_call(&mut self, n: &str, c: &Call) -> CallOut {
        let byref = self.byref.get(n).cloned().unwrap_or_default();
        let mut words = vec![fn_name(n)];
        for (i, a) in c.args.iter().enumerate() {
            if let Arg::Expr(e) = a {
                if byref.get(i).copied().unwrap_or(false) {
                    let w = self.ref_arg(e);
                    words.push(w);
                } else {
                    let w = self.flatten_arg(e);
                    words.extend(w);
                }
            }
        }
        let ret = self.funcs.get(n).map(|f| f.ret.clone()).unwrap_or_else(|| c.ret.clone());
        let fallible = c.fallible || self.funcs.get(n).map_or(false, |f| f.fallible);
        let line = words.join(" ");
        let v = self.ret_val(&ret);
        if fallible {
            out(Some(line), v)
        } else {
            self.line(line);
            out(None, v)
        }
    }

    /// Calls a helper that returns in `__ret`: `op` if fallible, otherwise a line.
    fn helper(&mut self, name: &str, args: &[String], fallible: bool, val: Val) -> CallOut {
        self.need(name);
        let mut l = name.to_string();
        for a in args {
            l.push(' ');
            l.push_str(a);
        }
        if fallible {
            out(Some(l), val)
        } else {
            self.line(l);
            out(None, val)
        }
    }

    fn pred(&mut self, name: &str, args: &[String]) -> CallOut {
        self.need(name);
        let mut l = name.to_string();
        for a in args {
            l.push(' ');
            l.push_str(a);
        }
        out(None, Val::Cond(Cond::Cmd(l)))
    }

    // ------------------------------------------------------------------------------------------
    // Builtins
    // ------------------------------------------------------------------------------------------

    fn builtin_call(&mut self, n: &str, c: &Call) -> CallOut {
        match n {
            "print" => {
                if c.args.is_empty() {
                    self.line("printf '\\n'");
                } else {
                    let w = self.items_word(c, 0);
                    self.line(format!("printf '%s\\n' {}", w.render()));
                }
                out(None, Val::Unit)
            }
            "fmt" => {
                let t = self.arg(c, 0).clone();
                let n_items = c.args.len() - 1;
                if let ExprKind::Str(tpl) = &t.kind {
                    if tpl.matches("{}").count() >= n_items {
                        let mut w = W::default();
                        let mut rest = tpl.as_str();
                        for i in 0..n_items {
                            let p = rest.find("{}").unwrap();
                            w.push_lit(&rest[..p]);
                            let x = self.arg_w(c, i + 1);
                            w.push(x);
                            rest = &rest[p + 2..];
                        }
                        w.push_lit(rest);
                        return out(None, Val::Word(w));
                    }
                }
                let mut args = vec![self.aw(c, 0)];
                for i in 1..c.args.len() {
                    args.push(self.aw(c, i));
                }
                self.helper("__fmt", &args, false, ret_w())
            }
            "secret" => {
                let w = self.arg_w(c, 0);
                out(None, Val::Word(w))
            }
            "enc_list" => {
                let e = self.arg(c, 0).clone();
                let items = match self.expr_val(&e) {
                    Val::List(x) => x,
                    _ => String::new(),
                };
                self.helper("__enc_l", &[items], false, ret_w())
            }
            "dec_list" => {
                let w = self.arg_w(c, 0);
                self.helper("__dec_l", &[w.render()], false, ret_a())
            }
            "duration_str" => {
                let e = self.arg(c, 0).clone();
                if let ExprKind::Duration(ms) | ExprKind::Int(ms) = e.kind {
                    return out(None, Val::Word(W::lit(&dur_str(ms))));
                }
                let a = self.arith_word(&e);
                self.helper("__dur_str", &[a], false, ret_w())
            }
            "enum_parse" => {
                let name = match &c.ret {
                    Ty::Enum(n) => n.clone(),
                    _ => String::new(),
                };
                let text = self.aw(c, c.args.len() - 1);
                self.enum_parse(&name, text)
            }
            other => self.unimplemented(other, &c.ret),
        }
    }

    fn enum_parse(&mut self, name: &str, text: String) -> CallOut {
        let cases = self.enums.get(name).map(|e| e.cases.clone()).unwrap_or_default();
        let mut args = vec![sq(name), text];
        args.extend(cases.iter().map(|c| sq(c)));
        self.helper("__enum_parse", &args, true, ret_w())
    }

    fn unimplemented(&mut self, what: &str, ret: &Ty) -> CallOut {
        self.need("__unimplemented");
        let v = match ret {
            Ty::Unit => Val::Unit,
            Ty::List(_) => ret_a(),
            _ => ret_w(),
        };
        out(Some(format!("__unimplemented {}", sq(what))), v)
    }

    // ------------------------------------------------------------------------------------------
    // Standard library
    // ------------------------------------------------------------------------------------------

    fn std_call(&mut self, module: &str, name: &str, c: &Call) -> CallOut {
        let full = format!("{}.{}", module, name);
        let nargs = c.args.len();
        let mut a: Vec<String> = Vec::new();
        let scalar_args = !matches!(full.as_str(), "ui.choose" | "log.info" | "log.warn" | "log.error" | "log.debug" | "math.min" | "math.max" | "math.abs" | "time.sleep" | "time.since" | "time.since_ms");
        if scalar_args {
            for i in 0..nargs {
                if let Some(Arg::Expr(e)) = c.args.get(i) {
                    if e.ty.is_scalar() {
                        let e = e.clone();
                        a.push(self.sw(&e));
                    }
                }
            }
        }
        match full.as_str() {
            "env.get" => self.helper("__env_get", &a, true, ret_w()),
            "env.has" => self.pred("__env_has", &a),
            "env.set" => self.helper("__env_set", &a, false, Val::Unit),
            "env.unset" => self.helper("__env_unset", &a, false, Val::Unit),
            "fs.exists" => self.pred("__fs_exists", &a),
            "fs.is_dir" => self.pred("__fs_is_dir", &a),
            "fs.is_file" => self.pred("__fs_is_file", &a),
            "fs.read" => self.helper("__fs_read", &a, true, ret_w()),
            "fs.write" => self.helper("__fs_write", &a, true, Val::Unit),
            "fs.append" => self.helper("__fs_append", &a, true, Val::Unit),
            "fs.rm" => self.helper("__fs_rm", &a, true, Val::Unit),
            "fs.mkdir" => self.helper("__fs_mkdir", &a, true, Val::Unit),
            "fs.copy" => self.helper("__fs_copy", &a, true, Val::Unit),
            "fs.move" => self.helper("__fs_move", &a, true, Val::Unit),
            "fs.ls" => self.helper("__fs_ls", &a, true, ret_a()),
            "fs.walk" => self.helper("__fs_walk", &a, true, ret_a()),
            "fs.glob" => self.helper("__fs_glob", &a, false, ret_a()),
            "fs.temp_dir" => {
                self.uses_temps = true;
                self.helper("__fs_temp_dir", &a, true, ret_w())
            }
            "fs.temp_file" => {
                self.uses_temps = true;
                self.helper("__fs_temp_file", &a, true, ret_w())
            }
            "fs.size" => self.helper("__fs_size", &a, true, ret_w()),
            "fs.modified" => self.helper("__fs_modified", &a, true, ret_w()),
            "fs.replace" => self.helper("__fs_replace", &a, true, Val::Unit),
            "path.abs" => self.helper("__path_abs", &a, false, ret_w()),
            // internal: a `path` passed to a program (lowering inserts the call)
            "path.__home" => self.helper("__path_home", &a, false, ret_w()),
            "dirs.home" => out(None, Val::Word(W::var("HOME"))),
            "dirs.temp" => self.helper("__dirs_temp", &a, false, ret_w()),
            "dirs.cwd" => out(None, Val::Word(W::var("PWD"))),
            "dirs.config" | "dirs.cache" | "dirs.data" => {
                let app = self.arg_w(c, 0);
                let base = match (self.linux, name) {
                    (true, "config") => "${XDG_CONFIG_HOME:-$HOME/.config}",
                    (true, "cache") => "${XDG_CACHE_HOME:-$HOME/.cache}",
                    (true, _) => "${XDG_DATA_HOME:-$HOME/.local/share}",
                    (false, "cache") => "${HOME}/Library/Caches",
                    (false, _) => "${HOME}/Library/Application Support",
                };
                let mut w = W::exp(base);
                w.push_lit("/");
                w.push(app);
                out(None, Val::Word(w))
            }
            "sys.arch" => self.helper("__sys_arch", &a, false, ret_w()),
            "sys.hostname" => self.helper("__sys_hostname", &a, false, ret_w()),
            "sys.user" => self.helper("__sys_user", &a, false, ret_w()),
            "sys.is_admin" => out(None, Val::Cond(Cond::Arith("EUID == 0".into()))),
            "sys.require_admin" => self.helper("__sys_require_admin", &a, true, Val::Unit),
            "sys.cpus" => self.helper("__sys_cpus", &a, false, ret_w()),
            "sys.is_ci" => out(None, Val::Cond(Cond::Test("-n ${CI:-}".into()))),
            "sys.cd" => self.helper("__sys_cd", &a, true, Val::Unit),
            "proc.exists" => out(None, Val::Cond(Cond::Cmd(format!("type -P {} >/dev/null 2>&1", a[0])))),
            "proc.which" => self.helper("__proc_which", &a, true, ret_w()),
            "time.now" => self.helper("__time_now", &a, false, ret_w()),
            "time.now_ms" => self.helper("__time_now_ms", &a, false, ret_w()),
            "time.since" | "time.since_ms" => {
                let t = self.arg(c, 0).clone();
                let ta = self.arith(&t);
                if name == "since" {
                    self.need("__time_now");
                    self.line("__time_now");
                    out(None, Val::Word(W::exp(format!("$(( (__ret - {}) * 1000 ))", ta))))
                } else {
                    self.need("__time_now_ms");
                    self.line("__time_now_ms");
                    out(None, Val::Word(W::exp(format!("$(( __ret - {} ))", ta))))
                }
            }
            "time.format" => self.helper("__time_format", &a, false, ret_w()),
            "time.sleep" => {
                let d = self.arg(c, 0).clone();
                let w = self.arith_word(&d);
                self.helper("__sleep_ms", &[w], false, Val::Unit)
            }
            "time.hour" => self.helper("__time_hour", &a, false, ret_w()),
            "log.info" | "log.warn" | "log.error" => {
                let w = self.items_word(c, 0);
                let pre = match name {
                    "info" => "info",
                    "warn" => "warning",
                    _ => "error",
                };
                self.line(format!("printf '{}: %s\\n' {} >&2", pre, w.render()));
                out(None, Val::Unit)
            }
            "log.debug" => {
                let w = self.items_word(c, 0);
                self.helper("__log_debug", &[w.render()], false, Val::Unit)
            }
            "ui.confirm" => self.pred("__ui_confirm", &a),
            "ui.ask" => self.helper("__ui_ask", &a, false, ret_w()),
            "ui.password" => self.helper("__ui_password", &a, true, ret_w()),
            "ui.choose" => {
                let q = self.aw(c, 0);
                let l = self.arg(c, 1).clone();
                let arr = self.list_arr(&l);
                self.helper("__ui_choose", &[q, format!("\"${{{}[@]}}\"", arr)], true, ret_w())
            }
            "math.min" | "math.max" => {
                let x = self.arg(c, 0).clone();
                let y = self.arg(c, 1).clone();
                let ax = self.arith(&x);
                let ay = self.arith(&y);
                let op = if name == "min" { "<" } else { ">" };
                out(None, Val::Word(W::exp(format!("$(( {a} {op} {b} ? {a} : {b} ))", a = paren(&ax), op = op, b = paren(&ay)))))
            }
            "math.abs" => {
                let x = self.arg(c, 0).clone();
                let ax = paren(&self.arith(&x));
                out(None, Val::Word(W::exp(format!("$(( {a} < 0 ? -{a} : {a} ))", a = ax))))
            }
            // headers (a list) go last, as "${arr[@]}"; the token is a function argument (never a process argument)
            "http.get" | "http.status" => {
                let u = self.aw(c, 0);
                let t = self.arg(c, 1).clone();
                let tw = self.arith_word(&t);
                let bearer = self.aw(c, 3);
                let h = self.arg(c, 2).clone();
                let arr = self.list_arr(&h);
                let f = if name == "get" { "__http_get" } else { "__http_status" };
                self.helper(f, &[u, tw, bearer, format!("\"${{{}[@]}}\"", arr)], true, ret_w())
            }
            "http.post" => {
                let u = self.aw(c, 0);
                let body = self.aw(c, 1);
                let ct = self.aw(c, 2);
                let t = self.arg(c, 3).clone();
                let tw = self.arith_word(&t);
                let bearer = self.aw(c, 5);
                let h = self.arg(c, 4).clone();
                let arr = self.list_arr(&h);
                self.helper("__http_post", &[u, body, ct, tw, bearer, format!("\"${{{}[@]}}\"", arr)], true, ret_w())
            }
            "http.request" => {
                let m = self.aw(c, 0);
                let u = self.aw(c, 1);
                let body = self.aw(c, 2);
                let ct = self.aw(c, 3);
                let t = self.arg(c, 4).clone();
                let tw = self.arith_word(&t);
                let bearer = self.aw(c, 6);
                let h = self.arg(c, 5).clone();
                let arr = self.list_arr(&h);
                self.helper("__http_request", &[m, u, body, ct, tw, bearer, format!("\"${{{}[@]}}\"", arr)], true, ret_w())
            }
            "http.download" => {
                let u = self.aw(c, 0);
                let to = self.aw(c, 1);
                let bearer = self.aw(c, 3);
                let h = self.arg(c, 2).clone();
                let arr = self.list_arr(&h);
                self.helper("__http_download", &[u, to, bearer, format!("\"${{{}[@]}}\"", arr)], true, Val::Unit)
            }
            "fs.chmod" => self.helper("__fs_chmod", &a, true, Val::Unit),
            "hash.sha256" => self.helper("__hash_sha256", &a, false, ret_w()),
            "semver.compare" => self.helper("__semver_compare", &a, true, ret_w()),
            "encoding.base64_encode" => self.helper("__b64_enc", &a, false, ret_w()),
            "encoding.base64_decode" => self.helper("__b64_dec", &a, true, ret_w()),
            "regex.test" | "regex.find" | "regex.find_all" | "regex.groups" | "regex.replace" => {
                // the checker made sure the pattern is valid fixed text (D32)
                let src = match c.args.get(1) {
                    Some(Arg::Expr(Expr { kind: ExprKind::Str(s), .. })) => s.clone(),
                    _ => String::new(),
                };
                let re = crate::regex::Regex::parse(&src).ok();
                let ere = sq(&re.as_ref().map(|r| r.to_ere()).unwrap_or_default());
                // anchored at the end: the match is the end of the text; at the start: at most one match
                let anchor = match &re {
                    Some(r) if r.end => "e",
                    Some(r) if r.start => "s",
                    _ => "0",
                };
                let text = a[0].clone();
                match name {
                    "test" => self.pred("__re_test", &[text, ere]),
                    "find" => self.helper("__re_find", &[text, ere, sq(&src)], true, ret_w()),
                    "find_all" => self.helper("__re_all", &[text, ere, anchor.into()], false, ret_a()),
                    "groups" => self.helper("__re_groups", &[text, ere, sq(&src)], true, ret_a()),
                    _ => self.helper("__re_replace", &[text, ere, a[2].clone(), anchor.into()], false, ret_w()),
                }
            }
            "hash.sha256_file" => self.helper("__hash_sha256_file", &a, true, ret_w()),
            "archive.tar_create" => self.helper("__archive_tar_create", &a, true, Val::Unit),
            "archive.tar_extract" => self.helper("__archive_tar_extract", &a, true, Val::Unit),
            "archive.zip_create" => self.helper("__archive_zip_create", &a, true, Val::Unit),
            "archive.zip_extract" => self.helper("__archive_zip_extract", &a, true, Val::Unit),
            "json.get" => self.helper("__json_get", &a, true, ret_w()),
            "json.get_list" => self.helper("__json_children", &[a[0].clone(), a[1].clone(), "a".into(), "item".into()], true, ret_a()),
            "json.keys" => self.helper("__json_children", &[a[0].clone(), a[1].clone(), "o".into(), "key".into()], true, ret_a()),
            "json.has" => self.pred("__json_has", &a),
            "json.quote" => self.helper("__json_quote", &a, false, ret_w()),
            _ => self.unimplemented(&full, &c.ret),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Methods
    // ------------------------------------------------------------------------------------------

    fn method_call(&mut self, recv_ty: &Ty, name: &str, c: &Call) -> CallOut {
        match recv_ty {
            Ty::List(elem) => self.list_method(elem, name, c),
            Ty::Map(_, _) => self.map_method(name, c),
            Ty::Enum(e) => match name {
                "parse" => {
                    let text = self.aw(c, c.args.len() - 1);
                    let e = e.clone();
                    self.enum_parse(&e, text)
                }
                _ => {
                    let w = self.arg_w(c, 0);
                    out(None, Val::Word(w))
                }
            },
            Ty::Duration if name == "to_str" => {
                let e = self.arg(c, 0).clone();
                if let ExprKind::Duration(ms) = e.kind {
                    return out(None, Val::Word(W::lit(&dur_str(ms))));
                }
                let a = self.arith_word(&e);
                self.helper("__dur_str", &[a], false, ret_w())
            }
            Ty::Int | Ty::Bool | Ty::Duration => {
                let w = self.arg_w(c, 0);
                out(None, Val::Word(w))
            }
            _ => self.str_method(name, c),
        }
    }

    fn str_method(&mut self, name: &str, c: &Call) -> CallOut {
        let mut a: Vec<String> = Vec::new();
        for i in 0..c.args.len() {
            a.push(self.aw(c, i));
        }
        match name {
            "trim" => self.helper("__str_trim", &a, false, ret_w()),
            "trim_start" => self.helper("__str_trim_start", &a, false, ret_w()),
            "trim_end" => self.helper("__str_trim_end", &a, false, ret_w()),
            "upper" | "lower" if self.linux => {
                let r = self.arg(c, 0).clone();
                let v = self.as_var(&r);
                let op = if name == "upper" { "^^" } else { ",," };
                out(None, Val::Word(W::exp(format!("${{{}{}}}", v, op))))
            }
            "upper" => self.helper("__str_upper", &a, false, ret_w()),
            "lower" => self.helper("__str_lower", &a, false, ret_w()),
            "contains" | "starts_with" | "ends_with" => {
                let r = self.arg(c, 0).clone();
                let v = self.as_var(&r);
                let x = self.arg_w(c, 1).render();
                let pat = match name {
                    "contains" => format!("*{}*", x),
                    "starts_with" => format!("{}*", x),
                    _ => format!("*{}", x),
                };
                out(None, Val::Cond(Cond::Test(format!("${} == {}", v, pat))))
            }
            "strip_prefix" | "strip_suffix" => {
                let r = self.arg(c, 0).clone();
                let v = self.as_var(&r);
                let x = self.arg_w(c, 1).render_dq();
                let op = if name == "strip_prefix" { "#" } else { "%" };
                out(None, Val::Word(W::exp(format!("${{{}{}{}}}", v, op, x))))
            }
            "index_of" => self.helper("__str_index", &a, false, ret_w()),
            "slice" => self.helper("__str_slice", &a, false, ret_w()),
            "split" => self.helper("__str_split", &a, false, ret_a()),
            "lines" => self.helper("__str_lines", &a, false, ret_a()),
            "replace" => self.helper("__str_replace", &a, false, ret_w()),
            "replace_first" => self.helper("__str_replace_first", &a, false, ret_w()),
            "repeat" => self.helper("__str_repeat", &a, false, ret_w()),
            "pad_left" | "pad_right" => {
                let mut args = vec![if name == "pad_left" { "left".to_string() } else { "right".to_string() }];
                args.extend(a.iter().cloned());
                if args.len() < 4 {
                    args.push("' '".into());
                }
                self.helper("__str_pad", &args, false, ret_w())
            }
            "to_int" => self.helper("__str_to_int", &a, true, ret_w()),
            "chars" => self.helper("__str_chars", &a, false, ret_a()),
            "is_empty" => {
                let r = self.arg(c, 0).clone();
                let v = self.as_var(&r);
                out(None, Val::Cond(Cond::Test(format!("-z ${}", v))))
            }
            "to_str" => {
                let w = self.arg_w(c, 0);
                out(None, Val::Word(w))
            }
            // path properties
            "name" => self.helper("__path_name", &a, false, ret_w()),
            "stem" => self.helper("__path_stem", &a, false, ret_w()),
            "ext" => self.helper("__path_ext", &a, false, ret_w()),
            "parent" => self.helper("__path_parent", &a, false, ret_w()),
            other => self.unimplemented(&format!("str.{}", other), &c.ret),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Maps
    // ------------------------------------------------------------------------------------------

    fn map_method(&mut self, name: &str, c: &Call) -> CallOut {
        let r = self.arg(c, 0).clone();
        let b = match self.loc(&r) {
            Loc::Base(b) => b,
            Loc::Elem { .. } => unreachable!(),
        };
        match name {
            "get" => {
                let k = self.arg_w(c, 1);
                let kr = k.render();
                if self.linux {
                    self.need("__map_err");
                    let lk = self.lkey(&k);
                    out(
                        Some(format!("{{ __mp=${{{b}__i[{lk}]:--1}}; (( __mp >= 0 )) || __map_err {k}; }}", b = b, lk = lk, k = kr)),
                        Val::Word(W::var(&format!("{}__v[__mp]", b))),
                    )
                } else {
                    self.need("__map_pos");
                    self.need("__map_err");
                    out(
                        Some(format!("{{ __map_pos \"${b}__id\" {k}; (( __mp >= 0 )) || __map_err {k}; }}", b = b, k = kr)),
                        Val::Word(W::var(&format!("{}__v[__mp]", b))),
                    )
                }
            }
            "has" => {
                let k = self.arg_w(c, 1);
                if self.linux {
                    let lk = self.lkey(&k);
                    out(None, Val::Cond(Cond::Test(format!("-n ${{{}__i[{}]+x}}", b, lk))))
                } else {
                    self.need("__map_has");
                    out(None, Val::Cond(Cond::Cmd(format!("__map_has \"${}__id\" {}", b, k.render()))))
                }
            }
            "set" => {
                let k = self.arg_w(c, 1);
                let v = self.arg_w(c, 2);
                self.map_set(&b, &k, &v);
                out(None, Val::Unit)
            }
            "remove" => {
                let k = self.arg_w(c, 1);
                if self.linux {
                    self.extra_local("__mk");
                    self.extra_local("__mi");
                    let lk = self.lkey(&k);
                    self.line(format!("__mk={}", lk));
                    self.line(format!("if [[ -n ${{{b}__i[$__mk]+x}} ]]; then", b = b));
                    self.line(format!("  __mp=${{{b}__i[$__mk]}}; unset -v '{b}__i[$__mk]'", b = b));
                    self.line(format!("  {b}__k=(\"${{{b}__k[@]:0:__mp}}\" \"${{{b}__k[@]:__mp+1}}\"); {b}__v=(\"${{{b}__v[@]:0:__mp}}\" \"${{{b}__v[@]:__mp+1}}\")", b = b));
                    self.line(format!("  for (( __mi = __mp; __mi < ${{#{b}__k[@]}}; __mi++ )); do {b}__i[\"_${{{b}__k[__mi]}}\"]=$__mi; done", b = b));
                    self.line("fi");
                } else {
                    self.need("__map_del");
                    self.line(format!("if __map_del \"${b}__id\" {k} \"${{{b}__n[@]}}\"; then", b = b, k = k.render()));
                    for s in ["k", "v", "n"] {
                        self.line(format!("  {b}__{s}=(\"${{{b}__{s}[@]:0:__mp}}\" \"${{{b}__{s}[@]:__mp+1}}\")", b = b, s = s));
                    }
                    self.line("fi");
                }
                out(None, Val::Unit)
            }
            "keys" => out(None, Val::List(format!("\"${{{}__k[@]}}\"", b))),
            "values" => out(None, Val::List(format!("\"${{{}__v[@]}}\"", b))),
            "is_empty" => out(None, Val::Cond(Cond::Arith(format!("${{#{}__k[@]}} == 0", b)))),
            "len" => out(None, Val::Word(W::exp(format!("${{#{}__k[@]}}", b)))),
            other => self.unimplemented(&format!("map.{}", other), &c.ret),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Lists
    // ------------------------------------------------------------------------------------------

    /// Generates the function of a lambda and returns its name and whether it can fail.
    fn lambda_fn(&mut self, l: &Lambda) -> (String, bool) {
        let k = self.fresh("lambda");
        let name = format!("__lambda_{}", k);
        let params: Vec<(String, Ty)> = l.params.iter().map(|p| (p.name.clone(), p.ty.clone())).collect();
        let txt = self.gen_function(&name, &params, &l.body, true, None);
        self.aux.push(txt);
        (name, block_may_fail(&l.body))
    }

    fn lambda_arg(&mut self, c: &Call, i: usize) -> (String, bool, Ty) {
        match c.args.get(i) {
            Some(Arg::Lambda(l)) => {
                let (n, f) = self.lambda_fn(l);
                (n, f, l.ret.clone())
            }
            _ => panic!("esperava lambda"),
        }
    }

    /// Selects items (indexes in `idx_arr`) of a list of structs into a temporary.
    fn pick_slist(&mut self, base: &str, s: &str, idx_arr: &str, ty: &Ty) -> Val {
        self.need("__list_pick");
        let t = self.tmp(ty);
        for f in self.struct_fields(s) {
            self.line(format!(
                "__list_pick \"${{#{i}[@]}}\" \"${{{i}[@]}}\" \"${{{b}__{f}[@]}}\"; {t}__{f}=(\"${{__ret_a[@]}}\")",
                i = idx_arr,
                b = base,
                f = f.name,
                t = t
            ));
        }
        Val::SList(t, s.to_string())
    }

    /// Composite key of a struct (all fields), to compare whole items.
    fn composite(&mut self, loc: &Loc, s: &str) -> W {
        let mut w = W::default();
        for (i, f) in self.struct_fields(s).iter().enumerate() {
            if i > 0 {
                w.push_lit("\u{1f}");
            }
            let v = self.field_val(loc, f);
            let x = self.encode(&v);
            w.push(x);
        }
        w
    }

    fn list_method(&mut self, elem: &Ty, name: &str, c: &Call) -> CallOut {
        let r = self.arg(c, 0).clone();
        let b = match self.loc(&r) {
            Loc::Base(b) => b,
            Loc::Elem { .. } => unreachable!(),
        };
        let lty = r.ty.clone();
        let len = self.list_len(&b, &lty);
        let sname = match elem {
            Ty::Struct(s) => Some(s.clone()),
            _ => None,
        };
        match name {
            "push" => {
                match &sname {
                    Some(s) => {
                        let x = self.arg(c, 1).clone();
                        let l = self.loc(&x);
                        let mut sets = Vec::new();
                        for f in self.struct_fields(s) {
                            let v = self.field_val(&l, &f);
                            let w = self.encode(&v);
                            sets.push(format!("{}__{}+=({})", b, f.name, w.render()));
                        }
                        self.line(sets.join("; "));
                    }
                    None => {
                        let w = self.aw(c, 1);
                        self.line(format!("{}+=({})", b, w));
                    }
                }
                out(None, Val::Unit)
            }
            "pop" => {
                self.need("__list_empty_err");
                let t = self.tmp(elem);
                let body = match &sname {
                    Some(s) => {
                        let mut parts = Vec::new();
                        for f in self.struct_fields(s) {
                            let a = format!("{}__{}", b, f.name);
                            parts.push(format!("{t}__{f}=${{{a}[${{#{a}[@]}}-1]}}; unset '{a}[${{#{a}[@]}}-1]'", t = t, f = f.name, a = a));
                        }
                        parts.join("; ")
                    }
                    None => format!("{t}=${{{b}[${{#{b}[@]}}-1]}}; unset '{b}[${{#{b}[@]}}-1]'", t = t, b = b),
                };
                let op = format!("if (( {} > 0 )); then {}; else __list_empty_err pop; fi", len, body);
                let v = match &sname {
                    Some(s) => Val::Struct(Loc::Base(t), s.clone()),
                    None => Val::Word(W::var(&t)),
                };
                out(Some(op), v)
            }
            "first" | "last" => {
                self.need("__list_empty_err");
                let i = if name == "first" { "0".to_string() } else { format!("{}-1", len) };
                let op = format!("{{ (( {} > 0 )) || __list_empty_err {}; }}", len, name);
                let v = match &sname {
                    Some(s) => Val::Struct(Loc::Elem { list: b.clone(), idx: i }, s.clone()),
                    None => Val::Word(W::var(&format!("{}[{}]", b, i))),
                };
                out(Some(op), v)
            }
            "is_empty" => out(None, Val::Cond(Cond::Arith(format!("{} == 0", len)))),
            "len" => out(None, Val::Word(W::exp(len))),
            "join" => {
                let sep = self.aw(c, 1);
                self.helper("__list_join", &[sep, format!("\"${{{}[@]}}\"", b)], false, ret_w())
            }
            "sum" => self.helper("__list_sum", &[format!("\"${{{}[@]}}\"", b)], false, ret_w()),
            "sort" => {
                let k = if is_int(elem) { "int" } else { "str" };
                self.helper("__list_sort", &[k.into(), format!("\"${{{}[@]}}\"", b)], false, ret_a())
            }
            "contains" | "index_of" => {
                let (target, items) = match &sname {
                    Some(s) => {
                        let x = self.arg(c, 1).clone();
                        let lx = self.loc(&x);
                        let tw = self.composite(&lx, s);
                        let kt = self.tmp(&Ty::List(Box::new(Ty::Str)));
                        let i = self.tmp(&Ty::Int);
                        self.line(format!("{}=()", kt));
                        self.line(format!("for (( {i} = 0; {i} < {n}; {i}++ )); do", i = i, n = len));
                        let ew = self.composite(&Loc::Elem { list: b.clone(), idx: i.clone() }, s);
                        self.line(format!("  {}+=({})", kt, ew.render()));
                        self.line("done");
                        (tw.render(), format!("\"${{{}[@]}}\"", kt))
                    }
                    None => (self.aw(c, 1), format!("\"${{{}[@]}}\"", b)),
                };
                if name == "contains" {
                    self.pred("__list_has", &[target, items])
                } else {
                    self.helper("__list_index", &[target, items], false, ret_w())
                }
            }
            "concat" => {
                let o = self.arg(c, 1).clone();
                let ob = match self.loc(&o) {
                    Loc::Base(x) => x,
                    Loc::Elem { .. } => unreachable!(),
                };
                match &sname {
                    Some(s) => {
                        let t = self.tmp(&lty);
                        for f in self.struct_fields(s) {
                            self.line(format!("{t}__{f}=(\"${{{b}__{f}[@]}}\" \"${{{o}__{f}[@]}}\")", t = t, f = f.name, b = b, o = ob));
                        }
                        out(None, Val::SList(t, s.clone()))
                    }
                    None => out(None, Val::List(format!("\"${{{}[@]}}\" \"${{{}[@]}}\"", b, ob))),
                }
            }
            "reverse" => match &sname {
                Some(s) => {
                    let p = self.tmp(&Ty::List(Box::new(Ty::Int)));
                    let i = self.tmp(&Ty::Int);
                    self.line(format!("{p}=(); for (( {i} = {n} - 1; {i} >= 0; {i}-- )); do {p}+=(\"${i}\"); done", p = p, i = i, n = len));
                    let v = self.pick_slist(&b, s, &p, &lty);
                    out(None, v)
                }
                None => self.helper("__list_reverse", &[format!("\"${{{}[@]}}\"", b)], false, ret_a()),
            },
            "slice" => {
                self.need("__list_range");
                let x = self.aw(c, 1);
                let y = if c.args.len() > 2 { self.aw(c, 2) } else { crate::stdlib::SLICE_END.into() };
                self.line(format!("__list_range {} {} \"{}\"", x, y, len));
                match &sname {
                    Some(s) => {
                        let t = self.tmp(&lty);
                        for f in self.struct_fields(s) {
                            self.line(format!("{t}__{f}=(\"${{{b}__{f}[@]:__ra:__rl}}\")", t = t, f = f.name, b = b));
                        }
                        out(None, Val::SList(t, s.clone()))
                    }
                    None => out(None, Val::List(format!("\"${{{}[@]:__ra:__rl}}\"", b))),
                }
            }
            "unique" => {
                self.need("__list_unique");
                self.need("__list_pick");
                match &sname {
                    Some(s) => {
                        let kt = self.tmp(&Ty::List(Box::new(Ty::Str)));
                        let i = self.tmp(&Ty::Int);
                        self.line(format!("{}=()", kt));
                        self.line(format!("for (( {i} = 0; {i} < {n}; {i}++ )); do", i = i, n = len));
                        let ew = self.composite(&Loc::Elem { list: b.clone(), idx: i.clone() }, s);
                        self.line(format!("  {}+=({})", kt, ew.render()));
                        self.line("done");
                        let p = self.tmp(&Ty::List(Box::new(Ty::Int)));
                        self.line(format!("__list_unique \"${{{}[@]}}\"; {}=(\"${{__ret_a[@]}}\")", kt, p));
                        let v = self.pick_slist(&b, s, &p, &lty);
                        out(None, v)
                    }
                    None => {
                        self.line(format!("__list_unique \"${{{b}[@]}}\"; __list_pick \"${{#__ret_a[@]}}\" \"${{__ret_a[@]}}\" \"${{{b}[@]}}\"", b = b));
                        out(None, ret_a())
                    }
                }
            }
            "filter" | "map" | "any" | "all" | "find" | "sort_by" => self.lambda_method(&b, elem, &lty, name, c),
            other => self.unimplemented(&format!("list.{}", other), &c.ret),
        }
    }

    fn lambda_method(&mut self, b: &str, elem: &Ty, lty: &Ty, name: &str, c: &Call) -> CallOut {
        let (lf, may_fail, lret) = self.lambda_arg(c, 1);
        let len = self.list_len(b, lty);
        let i = self.tmp(&Ty::Int);
        let sname = match elem {
            Ty::Struct(s) => Some(s.clone()),
            _ => None,
        };
        // lambda arguments for item i
        let item_args = |g: &mut Gen<'a>| -> String {
            match &sname {
                Some(s) => g.flatten_loc(&Loc::Elem { list: b.to_string(), idx: i.clone() }, &Ty::Struct(s.clone())).join(" "),
                None => format!("\"${{{}[{}]}}\"", b, i),
            }
        };
        let res = self.tmp(&Ty::List(Box::new(Ty::Str)));
        match name {
            "filter" | "map" | "sort_by" => self.line(format!("{}=()", res)),
            "any" => self.line(format!("{}=false", res)),
            "all" => self.line(format!("{}=true", res)),
            _ => self.line(format!("{}=-1", res)),
        }
        self.line(format!("for (( {i} = 0; {i} < {n}; {i}++ )); do", i = i, n = len));
        self.ind += 1;
        self.frame_mut().loops.push(LoopKind::User);
        let args = item_args(self);
        let call = format!("{} {}", lf, args);
        if may_fail {
            let fa = self.fail_action();
            self.line(format!("{} || {}", call, fa));
        } else {
            self.line(call);
        }
        match name {
            "filter" => self.line(format!("if [[ $__ret == true ]]; then {}+=(\"${}\"); fi", res, i)),
            "map" | "sort_by" => self.line(format!("{}+=(\"$__ret\")", res)),
            "any" => self.line(format!("if [[ $__ret == true ]]; then {}=true; break; fi", res)),
            "all" => self.line(format!("if [[ $__ret != true ]]; then {}=false; break; fi", res)),
            _ => self.line(format!("if [[ $__ret == true ]]; then {}={}; break; fi", res, i)),
        }
        self.frame_mut().loops.pop();
        self.ind -= 1;
        self.line("done");
        match name {
            "filter" => match &sname {
                Some(s) => {
                    let v = self.pick_slist(b, s, &res, lty);
                    out(None, v)
                }
                None => {
                    self.need("__list_pick");
                    self.line(format!("__list_pick \"${{#{r}[@]}}\" \"${{{r}[@]}}\" \"${{{b}[@]}}\"", r = res, b = b));
                    out(None, ret_a())
                }
            },
            "map" => out(None, Val::List(format!("\"${{{}[@]}}\"", res))),
            "any" | "all" => out(None, Val::Word(W::var(&res))),
            "find" => {
                self.need("__err");
                let op = format!("{{ (( {} >= 0 )) || __err 'find: no item matched'; }}", res);
                let v = match &sname {
                    Some(s) => Val::Struct(Loc::Elem { list: b.to_string(), idx: res.clone() }, s.clone()),
                    None => Val::Word(W::var(&format!("{}[{}]", b, res))),
                };
                out(Some(op), v)
            }
            _ => {
                // sort_by
                self.need("__list_perm");
                let k = if is_int(&lret) { "int" } else { "str" };
                let p = self.tmp(&Ty::List(Box::new(Ty::Int)));
                self.line(format!("__list_perm {} \"${{{}[@]}}\"; {}=(\"${{__ret_a[@]}}\")", k, res, p));
                match &sname {
                    Some(s) => {
                        let v = self.pick_slist(b, s, &p, lty);
                        out(None, v)
                    }
                    None => {
                        self.need("__list_pick");
                        self.line(format!("__list_pick \"${{#{p}[@]}}\" \"${{{p}[@]}}\" \"${{{b}[@]}}\"", p = p, b = b));
                        out(None, ret_a())
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------
    // parallel
    // ------------------------------------------------------------------------------------------

    pub fn gen_parallel(&mut self, max: Option<&Expr>, body: &ParallelBody, on_fail: &OnFail) {
        self.need("__par_dir");
        self.need("__par_slot");
        self.need("__par_task");
        self.need("__par_collect");
        let st = self.tmp(&Ty::Int);
        let dir = self.tmp(&Ty::Str);
        let n = self.tmp(&Ty::Int);
        let mx = self.tmp(&Ty::Int);
        // tasks
        let mut tasks: Vec<(String, Option<Expr>)> = Vec::new();
        let mut loop_info: Option<(String, Ty, Loc)> = None;
        let mut collect: Option<(String, Expr)> = None;
        match body {
            ParallelBody::For { var, list, body, collect: col } => {
                let et = match &list.ty {
                    Ty::List(e) => (**e).clone(),
                    _ => Ty::Str,
                };
                let k = self.fresh("task");
                let fname = format!("__task_{}", k);
                let ce = col.as_ref().map(|(_, e)| e.clone());
                let tail_expr = ce.clone();
                let tail = move |g: &mut Gen<'a>| {
                    let Some(e) = &tail_expr else { return };
                    if let Ty::Struct(sn) = &e.ty {
                        // a struct goes back as one record: each field as stored in a list of structs, ending in \x1d
                        let loc = g.loc(e);
                        let mut ws = Vec::new();
                        for f in g.struct_fields(sn) {
                            let v = g.field_val(&loc, &f);
                            ws.push(g.encode(&v).render());
                        }
                        g.line(format!("printf -v __ret '%s\\035' {}", ws.join(" ")));
                    } else {
                        let w = g.sw(e);
                        g.line(format!("__ret={}", w));
                    }
                };
                let txt = self.gen_function(&fname, &[(var.clone(), et.clone())], body, true, Some(&tail));
                self.aux.push(txt);
                let l = self.loc(list);
                loop_info = Some((fname, et, l));
                collect = col.clone();
            }
            ParallelBody::Tasks(blocks) => {
                for b in blocks {
                    let k = self.fresh("task");
                    let fname = format!("__task_{}", k);
                    let txt = self.gen_function(&fname, &[], b, true, None);
                    self.aux.push(txt);
                    tasks.push((fname, None));
                }
            }
        }
        match max {
            Some(e) => {
                let a = self.arith(e);
                self.line(format!("{}=$(( {} ))", mx, a));
            }
            None => {
                self.need("__sys_cpus");
                self.line(format!("__sys_cpus; {}=$__ret", mx));
            }
        }
        self.line(format!("{}=1", st));
        self.line("if __par_dir; then");
        self.ind += 1;
        self.line(format!("{}=$__ret; {}=0", dir, n));
        self.line("__par_pids=()");
        let launch = |g: &mut Gen<'a>, call: String| {
            g.line(format!("__par_slot \"${}\"", mx));
            g.line(format!("__par_task \"${}\" \"${}\" {} &", dir, n, call));
            g.line("__par_pids+=(\"$!\")");
            g.line(format!("{n}=$(( {n} + 1 ))", n = n));
        };
        if let Some((fname, et, l)) = loop_info {
            let Loc::Base(b) = l else { unreachable!() };
            let lt = Ty::List(Box::new(et.clone()));
            let len = self.list_len(&b, &lt);
            let i = self.tmp(&Ty::Int);
            self.line(format!("for (( {i} = 0; {i} < {n}; {i}++ )); do", i = i, n = len));
            self.ind += 1;
            let args = match &et {
                Ty::Struct(_) => self.flatten_loc(&Loc::Elem { list: b.clone(), idx: i.clone() }, &et).join(" "),
                _ => format!("\"${{{}[{}]}}\"", b, i),
            };
            launch(self, format!("{} {}", fname, args));
            self.ind -= 1;
            self.line("done");
        } else {
            for (f, _) in &tasks {
                launch(self, f.clone());
            }
        }
        self.line(format!("if __par_collect \"${}\" \"${}\"; then {}=0; fi", dir, n, st));
        if let Some((name, e)) = &collect {
            self.need("__par_read");
            let v = self.vname(name);
            if let Ty::Struct(sn) = &e.ty {
                // records → one array per field (the list-of-structs layout)
                let fields = self.struct_fields(sn);
                let r = self.tmp(&Ty::Str);
                self.line(format!("__par_read \"${}\" \"${}\"", dir, n));
                for f in &fields {
                    self.line(format!("{}__{}=()", v, f.name));
                }
                self.line(format!("for {} in \"${{__ret_a[@]}}\"; do", r));
                for f in &fields {
                    self.line(format!("  {v}__{f}+=(\"${{{r}%%$'\\035'*}}\"); {r}=${{{r}#*$'\\035'}}", v = v, f = f.name, r = r));
                }
                self.line("done");
            } else {
                self.line(format!("__par_read \"${}\" \"${}\"; {}=(\"${{__ret_a[@]}}\")", dir, n, v));
            }
        }
        self.line(format!("rm -rf -- \"${}\"", dir));
        self.ind -= 1;
        self.line("fi");
        let op = format!("(( {} == 0 ))", st);
        self.with_op(&op, on_fail, &Dest::Discard, &Ty::Unit, &mut |_: &mut Gen<'a>| {});
    }
}

fn paren(s: &str) -> String {
    if s.contains(' ') {
        format!("({})", s)
    } else {
        s.to_string()
    }
}
