//! Nested collections (D34): `[][]T` and `map[K][]T`, with `T` a scalar.
//!
//! The backends only know flat collections, so this pass, between the lowering and the backends, stores
//! each inner list as one text: its items, each followed by `\x1e` (an empty list is `""`). A `[][]int`
//! becomes a `[]str`, a `map[str][]str` a `map[str]str`. Whatever moves whole collections (copies,
//! arguments, `return`, `len`, `slice`, `reverse`, `unique`, `filter`, `keys`, `values`…) only changes type;
//! the places that take an item out decode it (`xs[i]`, `for row in xs`, `pop`/`first`/`last`/`find`,
//! `m[k]`, the parameter of a lambda), and the places that put one in encode it (`xs[i] = v`, `m[k] = v`,
//! `push`/`contains`/`index_of`, literals, a lambda that returns a list). Equal lists have equal texts,
//! so `contains`, `index_of` and `unique` keep working. Both targets run the same rewritten program, so
//! they behave the same.

use crate::ir::*;

/// Builtins the backends implement: `enc_list(xs: []T) -> str` and `dec_list(s: str) -> []T`.
pub const ENC: &str = "enc_list";
pub const DEC: &str = "dec_list";

/// `[][]T` or `map[K][]T`.
pub fn nested(t: &Ty) -> bool {
    match t {
        Ty::List(e) => matches!(**e, Ty::List(_)),
        Ty::Map(_, v) => matches!(**v, Ty::List(_)),
        _ => false,
    }
}

/// The type the backends see.
pub fn enc_ty(t: &Ty) -> Ty {
    match t {
        Ty::List(e) if matches!(**e, Ty::List(_)) => Ty::list(Ty::Str),
        Ty::Map(k, v) if matches!(**v, Ty::List(_)) => Ty::map((**k).clone(), Ty::Str),
        _ => t.clone(),
    }
}

/// The item of a nested collection (`[]T`).
fn item_ty(t: &Ty) -> Ty {
    match t {
        Ty::List(e) => (**e).clone(),
        Ty::Map(_, v) => (**v).clone(),
        _ => Ty::Str,
    }
}

fn rv_ty(rv: &Rvalue) -> Ty {
    match rv {
        Rvalue::Expr(e) => e.ty.clone(),
        Rvalue::List { elem_ty, .. } => Ty::list(elem_ty.clone()),
        Rvalue::Map { key_ty, val_ty, .. } => Ty::map(key_ty.clone(), val_ty.clone()),
        Rvalue::Struct { ty, .. } => Ty::Struct(ty.clone()),
        Rvalue::Call { call, .. } => call.ret.clone(),
        Rvalue::Capture { .. } => Ty::Str,
        Rvalue::CommandOk(_) => Ty::Bool,
    }
}

fn builtin(name: &str, arg: Expr, ret: Ty) -> Rvalue {
    Rvalue::Call { call: Call { target: CallTarget::Builtin(name.into()), args: vec![Arg::Expr(arg)], ret, fallible: false }, on_fail: OnFail::Propagate }
}

/// Whether any type in the program is nested (the pass is skipped otherwise).
fn uses_nested(p: &Program) -> bool {
    let s = format!("{:?}", p);
    if s.contains("List(List(") {
        return true;
    }
    // `Map(<key>, List(`: the key never has a comma at depth 0
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(k) = s[i..].find("Map(") {
        let mut j = i + k + 4;
        let mut depth = 0i32;
        while j < b.len() {
            match b[j] {
                b'(' => depth += 1,
                b')' if depth == 0 => break,
                b')' => depth -= 1,
                b',' if depth == 0 => {
                    if s[j..].starts_with(", List(") {
                        return true;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        i = i + k + 4;
    }
    false
}

/// Rewrites the program in place. A program without nested collections is left as it is.
pub fn encode(p: &mut Program) {
    if !uses_nested(p) {
        return;
    }
    let mut n = Nest { k: 0, ret_enc: vec![false] };
    for s in &mut p.structs {
        for f in &mut s.fields {
            f.ty = enc_ty(&f.ty);
        }
    }
    for f in &mut p.funcs {
        for prm in &mut f.params {
            prm.ty = enc_ty(&prm.ty);
        }
        f.ret = enc_ty(&f.ret);
        n.block(&mut f.body);
    }
    n.block(&mut p.body);
    for t in &mut p.tests {
        n.block(&mut t.body);
    }
}

struct Nest {
    k: usize,
    /// Per function or lambda: does `return` hand back an item that must be encoded (a lambda returning a list)?
    ret_enc: Vec<bool>,
}

impl Nest {
    fn tmp(&mut self) -> String {
        self.k += 1;
        format!("__n{}", self.k)
    }

    fn let_(line: u32, var: &str, ty: Ty, value: Rvalue) -> Stmt {
        Stmt { line, kind: StmtKind::Let { var: var.to_string(), ty, mutable: false, value } }
    }

    /// `x` (a list) as its text, in a new variable.
    fn enc(&mut self, x: Expr, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let t = self.tmp();
        pre.push(Self::let_(line, &t, Ty::Str, builtin(ENC, x, Ty::Str)));
        Expr::var(&t, Ty::Str)
    }

    /// The list `x` (a text) holds, in a new variable.
    fn dec(&mut self, x: Expr, ty: Ty, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let t = self.tmp();
        pre.push(Self::let_(line, &t, ty.clone(), builtin(DEC, x, ty.clone())));
        Expr::var(&t, ty)
    }

    /// An rvalue as something an expression can name.
    fn place(&mut self, rv: Rvalue, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        if let Rvalue::Expr(e) = &rv {
            if matches!(e.kind, ExprKind::Var(_) | ExprKind::Field(_, _)) {
                return e.clone();
            }
        }
        let ty = rv_ty(&rv);
        let t = self.tmp();
        pre.push(Self::let_(line, &t, ty.clone(), rv));
        Expr::var(&t, ty)
    }

    fn block(&mut self, b: &mut Block) {
        let old = std::mem::take(&mut b.stmts);
        for s in old {
            self.stmt(s, &mut b.stmts);
        }
    }

    fn blk(&mut self, mut b: Block) -> Block {
        self.block(&mut b);
        b
    }

    /// One statement, with whatever it needs computed before it.
    fn stmt(&mut self, s: Stmt, out: &mut Vec<Stmt>) {
        let line = s.line;
        let kind = match s.kind {
            StmtKind::Let { var, ty, mutable, value } => {
                let value = self.rvalue(value, line, out);
                StmtKind::Let { var, ty: enc_ty(&ty), mutable, value }
            }
            StmtKind::Assign { target, value } => {
                // a list put into a list or a map is an item of a nested collection
                let item = matches!(target, LValue::Index { .. } | LValue::MapEntry { .. }) && matches!(rv_ty(&value), Ty::List(_));
                let target = match target {
                    LValue::Index { list, index } => LValue::Index { list, index: self.expr(index, line, out) },
                    LValue::MapEntry { map, key } => LValue::MapEntry { map, key: self.expr(key, line, out) },
                    t => t,
                };
                let mut value = self.rvalue(value, line, out);
                if item {
                    let x = self.place(value, line, out);
                    value = Rvalue::Expr(self.enc(x, line, out));
                }
                StmtKind::Assign { target, value }
            }
            StmtKind::Call { call, on_fail } => {
                let (call, _) = self.call(call, line, out);
                StmtKind::Call { call, on_fail: self.on_fail(on_fail, false, line, out) }
            }
            StmtKind::Command { cmd, on_fail } => {
                let cmd = self.cmd(cmd, line, out);
                StmtKind::Command { cmd, on_fail: self.on_fail(on_fail, false, line, out) }
            }
            StmtKind::If { cond, then, els } => {
                let cond = self.expr(cond, line, out);
                StmtKind::If { cond, then: self.blk(then), els: els.map(|b| self.blk(b)) }
            }
            StmtKind::While { cond, body } => {
                let mut pre = Vec::new();
                let c = self.expr(cond, line, &mut pre);
                let body = self.blk(body);
                if pre.is_empty() {
                    StmtKind::While { cond: c, body }
                } else {
                    // `while true { pre; if !c { break }; body }`: the condition's items are decoded every time
                    let not_c = Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(c)), ty: Ty::Bool };
                    let mut stmts = pre;
                    stmts.push(Stmt { line, kind: StmtKind::If { cond: not_c, then: Block { stmts: vec![Stmt { line, kind: StmtKind::Break }] }, els: None } });
                    stmts.extend(body.stmts);
                    StmtKind::While { cond: Expr::bool(true), body: Block { stmts } }
                }
            }
            StmtKind::ForRange { var, from, to, inclusive, body } => {
                let from = self.expr(from, line, out);
                let to = self.expr(to, line, out);
                StmtKind::ForRange { var, from, to, inclusive, body: self.blk(body) }
            }
            StmtKind::ForList { var, index, list, body } => {
                let item = nested(&list.ty).then(|| item_ty(&list.ty));
                let list = self.expr(list, line, out);
                let (var, body) = self.item_var(var, item, body, line);
                StmtKind::ForList { var, index, list, body }
            }
            StmtKind::ForMap { key, value, map, body } => {
                let item = nested(&map.ty).then(|| item_ty(&map.ty));
                let map = self.expr(map, line, out);
                let (value, body) = self.item_var(value, item, body, line);
                StmtKind::ForMap { key, value, map, body }
            }
            StmtKind::Match { subject, arms, default } => {
                let subject = self.expr(subject, line, out);
                let arms = arms.into_iter().map(|a| Arm { patterns: a.patterns, body: self.blk(a.body) }).collect();
                StmtKind::Match { subject, arms, default: default.map(|b| self.blk(b)) }
            }
            StmtKind::Try { body, on_fail } => {
                let body = self.blk(body);
                StmtKind::Try { body, on_fail: self.on_fail(on_fail, false, line, out) }
            }
            StmtKind::Retry { times, every, body, on_fail } => {
                let times = self.expr(times, line, out);
                let every = self.expr(every, line, out);
                let body = self.blk(body);
                StmtKind::Retry { times, every, body, on_fail: self.on_fail(on_fail, false, line, out) }
            }
            StmtKind::Parallel { max, body, on_fail } => {
                let max = max.map(|m| self.expr(m, line, out));
                let body = match body {
                    ParallelBody::For { var, list, body, collect } => {
                        let item = nested(&list.ty).then(|| item_ty(&list.ty));
                        let list = self.expr(list, line, out);
                        let (var, mut body) = self.item_var(var, item, body, line);
                        let collect = collect.map(|(name, e)| {
                            let e = self.expr(e, line, &mut body.stmts);
                            (name, e)
                        });
                        ParallelBody::For { var, list, body, collect }
                    }
                    ParallelBody::Tasks(bs) => ParallelBody::Tasks(bs.into_iter().map(|b| self.blk(b)).collect()),
                };
                StmtKind::Parallel { max, body, on_fail: self.on_fail(on_fail, false, line, out) }
            }
            StmtKind::Fail { message, code } => {
                let message = self.expr(message, line, out);
                StmtKind::Fail { message, code: code.map(|c| self.expr(c, line, out)) }
            }
            StmtKind::Return(v) => {
                let v = v.map(|e| {
                    let x = self.expr(e, line, out);
                    if self.ret_enc.last() == Some(&true) {
                        self.enc(x, line, out)
                    } else {
                        x
                    }
                });
                StmtKind::Return(v)
            }
            StmtKind::Exit(c) => StmtKind::Exit(c.map(|c| self.expr(c, line, out))),
            StmtKind::Assert { cond, text } => StmtKind::Assert { cond: self.expr(cond, line, out), text },
            // their arguments are computed when they are reached: what they need goes just before them
            StmtKind::Defer(inner) => StmtKind::Defer(Box::new(self.inner(*inner, out))),
            StmtKind::AssertFails(inner) => StmtKind::AssertFails(Box::new(self.inner(*inner, out))),
            k @ (StmtKind::Break | StmtKind::Continue | StmtKind::Native { .. }) => k,
        };
        out.push(Stmt { line, kind });
    }

    /// A statement inside `defer`/`assert_fails`: the statements it needs go to `out`, before it.
    fn inner(&mut self, s: Stmt, out: &mut Vec<Stmt>) -> Stmt {
        let mut v = Vec::new();
        self.stmt(s, &mut v);
        let last = v.pop().expect("stmt always pushes itself");
        out.extend(v);
        last
    }

    /// A loop variable that gets items of a nested collection: the loop goes over the texts and the body
    /// starts by decoding the item into the original name.
    fn item_var(&mut self, var: Name, item: Option<Ty>, body: Block, line: u32) -> (Name, Block) {
        let mut body = self.blk(body);
        match item {
            Some(ty) => {
                let e = self.tmp();
                body.stmts.insert(0, Self::let_(line, &var, ty.clone(), builtin(DEC, Expr::var(&e, Ty::Str), ty)));
                (e, body)
            }
            None => (var, body),
        }
    }

    fn rvalue(&mut self, rv: Rvalue, line: u32, pre: &mut Vec<Stmt>) -> Rvalue {
        match rv {
            Rvalue::Expr(e) => Rvalue::Expr(self.expr(e, line, pre)),
            Rvalue::List { elem_ty, items } => {
                let item = matches!(elem_ty, Ty::List(_));
                let mut xs = Vec::new();
                for x in items {
                    let x = self.expr(x, line, pre);
                    xs.push(if item { self.enc(x, line, pre) } else { x });
                }
                Rvalue::List { elem_ty: if item { Ty::Str } else { enc_ty(&elem_ty) }, items: xs }
            }
            Rvalue::Map { key_ty, val_ty, entries } => {
                let item = matches!(val_ty, Ty::List(_));
                let mut es = Vec::new();
                for (k, v) in entries {
                    let k = self.expr(k, line, pre);
                    let v = self.expr(v, line, pre);
                    es.push((k, if item { self.enc(v, line, pre) } else { v }));
                }
                Rvalue::Map { key_ty, val_ty: if item { Ty::Str } else { enc_ty(&val_ty) }, entries: es }
            }
            Rvalue::Struct { ty, spread, fields } => {
                let spread = spread.map(|s| self.expr(s, line, pre));
                let fields = fields.into_iter().map(|(n, e)| (n, self.expr(e, line, pre))).collect();
                Rvalue::Struct { ty, spread, fields }
            }
            Rvalue::Call { call, on_fail } => {
                let (call, item) = self.call(call, line, pre);
                match item {
                    // the call gives an item's text: decode it afterwards
                    Some(ty) => {
                        let on_fail = self.on_fail(on_fail, true, line, pre);
                        let t = self.tmp();
                        pre.push(Self::let_(line, &t, Ty::Str, Rvalue::Call { call, on_fail }));
                        builtin(DEC, Expr::var(&t, Ty::Str), ty)
                    }
                    None => Rvalue::Call { call, on_fail: self.on_fail(on_fail, false, line, pre) },
                }
            }
            Rvalue::Capture { cmd, on_fail } => {
                let cmd = self.cmd(cmd, line, pre);
                Rvalue::Capture { cmd, on_fail: self.on_fail(on_fail, false, line, pre) }
            }
            Rvalue::CommandOk(cmd) => Rvalue::CommandOk(self.cmd(cmd, line, pre)),
        }
    }

    /// `item`: the guarded call now gives an item's text, so a value the handler gives must be a text too.
    /// A handler stays lazy: what its value needs is computed inside it.
    fn on_fail(&mut self, of: OnFail, item: bool, line: u32, _pre: &mut Vec<Stmt>) -> OnFail {
        match of {
            OnFail::Propagate => OnFail::Propagate,
            OnFail::Ignore => OnFail::Ignore,
            OnFail::Default(rv) => {
                let mut body = Vec::new();
                let rv = self.rvalue(*rv, line, &mut body);
                if !item && body.is_empty() {
                    return OnFail::Default(Box::new(rv));
                }
                let mut v = self.place(rv, line, &mut body);
                if item {
                    v = self.enc(v, line, &mut body);
                }
                OnFail::Block { err: None, body: Block { stmts: body }, value: Some(v) }
            }
            OnFail::Fail { message, code } => {
                let mut body = Vec::new();
                let message = self.expr(message, line, &mut body);
                let code = code.map(|c| self.expr(c, line, &mut body));
                if body.is_empty() {
                    return OnFail::Fail { message, code };
                }
                body.push(Stmt { line, kind: StmtKind::Fail { message, code } });
                OnFail::Block { err: None, body: Block { stmts: body }, value: None }
            }
            OnFail::Block { err, body, value } => {
                let mut body = self.blk(body);
                let value = value.map(|v| {
                    let x = self.expr(v, line, &mut body.stmts);
                    if item {
                        self.enc(x, line, &mut body.stmts)
                    } else {
                        x
                    }
                });
                OnFail::Block { err, body, value }
            }
        }
    }

    /// The call, and the type of the item it gives when it takes one out of a nested collection.
    fn call(&mut self, c: Call, line: u32, pre: &mut Vec<Stmt>) -> (Call, Option<Ty>) {
        let Call { target, args, ret, fallible } = c;
        let mut item_ret = None;
        let mut item_arg = false;
        let mut item_param = false;
        if let CallTarget::Method { recv_ty, name } = &target {
            if nested(recv_ty) {
                match name.as_str() {
                    "pop" | "first" | "last" | "find" | "get" => item_ret = Some(item_ty(recv_ty)),
                    "push" | "contains" | "index_of" => item_arg = true,
                    _ => {}
                }
                item_param = matches!(recv_ty, Ty::List(_));
            }
        }
        let mut xs = Vec::new();
        for (i, a) in args.into_iter().enumerate() {
            match a {
                Arg::Expr(e) => {
                    let mut x = self.expr(e, line, pre);
                    if item_arg && i == 1 {
                        x = self.enc(x, line, pre);
                    }
                    xs.push(Arg::Expr(x));
                }
                Arg::Lambda(l) => xs.push(Arg::Lambda(self.lambda(l, item_param))),
            }
        }
        let target = match target {
            CallTarget::Method { recv_ty, name } => CallTarget::Method { recv_ty: enc_ty(&recv_ty), name },
            t => t,
        };
        let ret = if item_ret.is_some() { Ty::Str } else { enc_ty(&ret) };
        (Call { target, args: xs, ret, fallible }, item_ret)
    }

    /// `item_param`: the lambda gets items of a nested list (texts, decoded first). A lambda that returns a
    /// list (`map` building a `[][]T`) returns its text.
    fn lambda(&mut self, l: Lambda, item_param: bool) -> Lambda {
        let Lambda { mut params, ret, body } = l;
        let enc_ret = matches!(ret, Ty::List(_));
        self.ret_enc.push(enc_ret);
        let mut body = self.blk(body);
        self.ret_enc.pop();
        if item_param {
            if let Some(p) = params.get_mut(0).filter(|p| matches!(p.ty, Ty::List(_))) {
                let line = body.stmts.first().map(|s| s.line).unwrap_or(0);
                let (orig, ty) = (p.name.clone(), p.ty.clone());
                let e = self.tmp();
                p.name = e.clone();
                p.ty = Ty::Str;
                body.stmts.insert(0, Self::let_(line, &orig, ty.clone(), builtin(DEC, Expr::var(&e, Ty::Str), ty)));
            }
        }
        Lambda { params, ret: if enc_ret { Ty::Str } else { enc_ty(&ret) }, body }
    }

    fn cmd(&mut self, c: Command, line: u32, pre: &mut Vec<Stmt>) -> Command {
        let Command { pipeline, redirects, stdin, text } = c;
        let pipeline = pipeline
            .into_iter()
            .map(|sc| {
                let mut w = |w: Word, pre: &mut Vec<Stmt>| match w {
                    Word::Lit(s) => Word::Lit(s),
                    Word::Interp(ps) => Word::Interp(self.parts(ps, line, pre)),
                    Word::Spread(e) => Word::Spread(self.expr(e, line, pre)),
                };
                let env = sc.env.into_iter().map(|(n, x)| (n, w(x, pre))).collect();
                let words = sc.words.into_iter().map(|x| w(x, pre)).collect();
                SimpleCmd { words, env }
            })
            .collect();
        let redirects = redirects
            .into_iter()
            .map(|r| match r {
                Redirect::Out { path, append } => Redirect::Out { path: self.expr(path, line, pre), append },
                Redirect::Err { path, append } => Redirect::Err { path: self.expr(path, line, pre), append },
                Redirect::In(p) => Redirect::In(self.expr(p, line, pre)),
                Redirect::ErrToOut => Redirect::ErrToOut,
            })
            .collect();
        let stdin = stdin.map(|e| self.expr(e, line, pre));
        Command { pipeline, redirects, stdin, text }
    }

    fn parts(&mut self, ps: Vec<Part>, line: u32, pre: &mut Vec<Stmt>) -> Vec<Part> {
        ps.into_iter()
            .map(|p| match p {
                Part::Lit(s) => Part::Lit(s),
                Part::Expr(e) => Part::Expr(self.expr(e, line, pre)),
            })
            .collect()
    }

    fn expr(&mut self, e: Expr, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let Expr { kind, ty } = e;
        let kind = match kind {
            ExprKind::Index(b, i) => {
                let item = nested(&b.ty);
                let b = self.expr(*b, line, pre);
                let i = self.expr(*i, line, pre);
                if item {
                    // `xs[i]` of a `[][]T`: the text at `i`, decoded (the index guard already ran before)
                    let x = Expr { kind: ExprKind::Index(Box::new(b), Box::new(i)), ty: Ty::Str };
                    return self.dec(x, ty, line, pre);
                }
                ExprKind::Index(Box::new(b), Box::new(i))
            }
            ExprKind::Field(b, f) => ExprKind::Field(Box::new(self.expr(*b, line, pre)), f),
            ExprKind::Len(x) => ExprKind::Len(Box::new(self.expr(*x, line, pre))),
            ExprKind::Unary(op, x) => ExprKind::Unary(op, Box::new(self.expr(*x, line, pre))),
            ExprKind::Binary(op, a, b) => {
                let a = self.expr(*a, line, pre);
                let b = self.expr(*b, line, pre);
                ExprKind::Binary(op, Box::new(a), Box::new(b))
            }
            ExprKind::Interp(ps) => ExprKind::Interp(self.parts(ps, line, pre)),
            k => k,
        };
        Expr { kind, ty: enc_ty(&ty) }
    }
}

// =================================================================================================
// Structs inside structs (D35)
// =================================================================================================

/// A struct field that is a struct becomes that struct's fields, prefixed: `svc: Service` turns into
/// `svc__name` and `svc__port`. A field that is a list of structs becomes one list per field of the item:
/// `members: []Member` turns into `members__name: []str` and `members__age: []int` (D35).
/// `d.svc.port` reads `d.svc__port` and `t.members[i].name` reads `t.members__name[i]`; a whole value
/// (`d.svc`, `t.members`, `t.members[i]`) is rebuilt from those fields; `d.svc = s`, `t.members = ms` and
/// literals write each field; `t.members.push(m)` and `pop()` change a rebuilt copy that is written back.
/// The backends keep seeing structs of scalars and collections, so lists of these structs work as before.
pub fn flatten_structs(p: &mut Program) {
    let defs: std::collections::HashMap<String, Vec<Field>> = p.structs.iter().map(|s| (s.name.clone(), s.fields.clone())).collect();
    let flattened = |t: &Ty| matches!(t, Ty::Struct(_)) || matches!(t, Ty::List(e) if matches!(**e, Ty::Struct(_)));
    if !p.structs.iter().any(|s| s.fields.iter().any(|f| flattened(&f.ty))) {
        return;
    }
    let mut fl = Flat { defs, k: 0, post: Vec::new() };
    for s in &mut p.structs {
        s.fields = fl.leaves(&s.name);
    }
    for f in &mut p.funcs {
        fl.block(&mut f.body);
    }
    fl.block(&mut p.body);
    for t in &mut p.tests {
        fl.block(&mut t.body);
    }
}

struct Flat {
    /// The structs as written (before flattening).
    defs: std::collections::HashMap<String, Vec<Field>>,
    k: usize,
    /// Statements that must run right after the current one (writing back a changed copy).
    post: Vec<Stmt>,
}

fn fld(b: Expr, name: String, ty: Ty) -> Expr {
    Expr { kind: ExprKind::Field(Box::new(b), name), ty }
}

/// Where the fields of a struct value are.
enum Holder {
    /// A place (`Var`, `Index`…) whose fields are read by name.
    Place(Expr),
    /// A struct-typed field flattened into `base` under a prefix.
    Prefix(Expr, String),
    /// Item `idx` of a list-of-structs field flattened into `base` under a prefix: each of its fields is a
    /// list, indexed once per level in `idx`.
    Cols(Expr, String, Vec<Expr>),
}

impl Holder {
    fn field(&self, name: &str, ty: Ty) -> Expr {
        match self {
            Holder::Place(e) => fld(e.clone(), name.to_string(), ty),
            Holder::Prefix(b, p) => fld(b.clone(), format!("{}__{}", p, name), ty),
            Holder::Cols(b, p, idx) => {
                let mut col = ty;
                for _ in idx {
                    col = Ty::list(col);
                }
                let mut x = fld(b.clone(), format!("{}__{}", p, name), col);
                for i in idx {
                    let item = match &x.ty {
                        Ty::List(e) => (**e).clone(),
                        t => t.clone(),
                    };
                    x = Expr { kind: ExprKind::Index(Box::new(x), Box::new(i.clone())), ty: item };
                }
                x
            }
        }
    }

    /// The holder of struct-typed field `f`.
    fn sub(self, f: &str) -> Holder {
        match self {
            Holder::Place(x) => Holder::Prefix(x, f.to_string()),
            Holder::Prefix(x, p) => Holder::Prefix(x, format!("{}__{}", p, f)),
            Holder::Cols(x, p, idx) => Holder::Cols(x, format!("{}__{}", p, f), idx),
        }
    }

    /// The holder of item `i` of list-of-structs field `f`.
    fn item(self, f: &str, i: Expr) -> Holder {
        match self {
            Holder::Place(x) => Holder::Cols(x, f.to_string(), vec![i]),
            Holder::Prefix(x, p) => Holder::Cols(x, format!("{}__{}", p, f), vec![i]),
            Holder::Cols(x, p, mut idx) => {
                idx.push(i);
                Holder::Cols(x, format!("{}__{}", p, f), idx)
            }
        }
    }

    /// What an assignment to field `name` of this holder writes (only when it lives in a variable).
    fn lvalue(&self, name: &str) -> Option<LValue> {
        match self {
            Holder::Place(Expr { kind: ExprKind::Var(v), .. }) => Some(LValue::Field { base: v.clone(), field: name.to_string() }),
            Holder::Prefix(Expr { kind: ExprKind::Var(v), .. }, p) => Some(LValue::Field { base: v.clone(), field: format!("{}__{}", p, name) }),
            _ => None,
        }
    }
}

/// How a field is flattened.
enum Flattened {
    Struct(String),
    List(String),
}

impl Flat {
    fn tmp(&mut self) -> String {
        self.k += 1;
        format!("__f{}", self.k)
    }

    /// The flattened fields of a struct, in order.
    fn leaves(&self, s: &str) -> Vec<Field> {
        let mut out = Vec::new();
        for f in self.defs.get(s).cloned().unwrap_or_default() {
            match &f.ty {
                Ty::Struct(inner) => {
                    for g in self.leaves(inner) {
                        out.push(Field { name: format!("{}__{}", f.name, g.name), ty: g.ty });
                    }
                }
                Ty::List(e) => match &**e {
                    Ty::Struct(inner) => {
                        for g in self.leaves(inner) {
                            out.push(Field { name: format!("{}__{}", f.name, g.name), ty: Ty::list(g.ty) });
                        }
                    }
                    _ => out.push(f),
                },
                _ => out.push(f),
            }
        }
        out
    }

    fn field_ty(&self, owner: &Ty, f: &str) -> Option<Ty> {
        match owner {
            Ty::Struct(s) => self.defs.get(s)?.iter().find(|x| x.name == f).map(|x| x.ty.clone()),
            _ => None,
        }
    }

    fn is_struct_field(&self, owner: &Ty, f: &str) -> bool {
        matches!(self.field_ty(owner, f), Some(Ty::Struct(_)))
    }

    /// The item struct when field `f` of `owner` is a list of structs.
    fn struct_list_field(&self, owner: &Ty, f: &str) -> Option<String> {
        match self.field_ty(owner, f)? {
            Ty::List(e) => match *e {
                Ty::Struct(n) => Some(n),
                _ => None,
            },
            _ => None,
        }
    }

    fn block(&mut self, b: &mut Block) {
        let old = std::mem::take(&mut b.stmts);
        for s in old {
            self.stmt(s, &mut b.stmts);
        }
    }

    fn blk(&mut self, mut b: Block) -> Block {
        self.block(&mut b);
        b
    }

    fn st(line: u32, kind: StmtKind) -> Stmt {
        Stmt { line, kind }
    }

    fn push_call(list: Expr, item: Expr) -> StmtKind {
        let recv_ty = list.ty.clone();
        StmtKind::Call {
            call: Call { target: CallTarget::Method { recv_ty, name: "push".into() }, args: vec![Arg::Expr(list), Arg::Expr(item)], ret: Ty::Unit, fallible: false },
            on_fail: OnFail::Propagate,
        }
    }

    /// Rebuilds a whole struct value from the fields `h` holds, in a new variable (with the flattened fields
    /// of its own inner structs).
    fn rebuild(&mut self, h: &Holder, s: &str, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let fields = self.leaves(s).into_iter().map(|leaf| (leaf.name.clone(), h.field(&leaf.name, leaf.ty))).collect();
        let t = self.tmp();
        pre.push(Self::st(line, StmtKind::Let { var: t.clone(), ty: Ty::Struct(s.to_string()), mutable: false, value: Rvalue::Struct { ty: s.to_string(), spread: None, fields } }));
        Expr::var(&t, Ty::Struct(s.to_string()))
    }

    /// The per-field lists of list-of-structs field `f` (items of struct `s`) that `h` holds.
    fn cols(&self, h: &Holder, f: &str, s: &str) -> Vec<(Field, Expr)> {
        self.leaves(s).into_iter().map(|g| {
            let c = h.field(&format!("{}__{}", f, g.name), Ty::list(g.ty.clone()));
            (g, c)
        }).collect()
    }

    /// Rebuilds the list of structs of field `f` (items of struct `s`) into a new variable.
    fn rebuild_list(&mut self, h: &Holder, f: &str, s: &str, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let cols = self.cols(h, f, s);
        let sty = Ty::Struct(s.to_string());
        let lty = Ty::list(sty.clone());
        let t = self.tmp();
        pre.push(Self::st(line, StmtKind::Let { var: t.clone(), ty: lty.clone(), mutable: true, value: Rvalue::List { elem_ty: sty.clone(), items: vec![] } }));
        let n = match cols.first() {
            Some((_, c)) => Expr { kind: ExprKind::Len(Box::new(c.clone())), ty: Ty::Int },
            None => Expr::int(0),
        };
        let (i, e) = (self.tmp(), self.tmp());
        let iv = Expr::var(&i, Ty::Int);
        let fields = cols.iter().map(|(g, c)| (g.name.clone(), Expr { kind: ExprKind::Index(Box::new(c.clone()), Box::new(iv.clone())), ty: g.ty.clone() })).collect();
        let body = vec![
            Self::st(line, StmtKind::Let { var: e.clone(), ty: sty.clone(), mutable: false, value: Rvalue::Struct { ty: s.to_string(), spread: None, fields } }),
            Self::st(line, Self::push_call(Expr::var(&t, lty.clone()), Expr::var(&e, sty))),
        ];
        pre.push(Self::st(line, StmtKind::ForRange { var: i, from: Expr::int(0), to: n, inclusive: false, body: Block { stmts: body } }));
        Expr::var(&t, lty)
    }

    /// Splits list `v` (items of struct `s`, already a place) into one list per field: the flattened
    /// `(name, value)` pairs of field `f`.
    fn spread_list(&mut self, f: &str, s: &str, v: Expr, line: u32, pre: &mut Vec<Stmt>) -> Vec<(Name, Expr)> {
        let leaves = self.leaves(s);
        let names: Vec<String> = leaves.iter().map(|_| self.tmp()).collect();
        for (g, c) in leaves.iter().zip(&names) {
            pre.push(Self::st(line, StmtKind::Let { var: c.clone(), ty: Ty::list(g.ty.clone()), mutable: true, value: Rvalue::List { elem_ty: g.ty.clone(), items: vec![] } }));
        }
        let x = self.tmp();
        let xv = Expr::var(&x, Ty::Struct(s.to_string()));
        let body = leaves.iter().zip(&names).map(|(g, c)| Self::st(line, Self::push_call(Expr::var(c, Ty::list(g.ty.clone())), fld(xv.clone(), g.name.clone(), g.ty.clone())))).collect();
        pre.push(Self::st(line, StmtKind::ForList { var: x, index: None, list: v, body: Block { stmts: body } }));
        leaves.iter().zip(names).map(|(g, c)| (format!("{}__{}", f, g.name), Expr::var(&c, Ty::list(g.ty.clone())))).collect()
    }

    /// The fields a struct-typed expression holds, without copying it.
    fn holder(&mut self, e: Expr, line: u32, pre: &mut Vec<Stmt>) -> Holder {
        match &e.kind {
            ExprKind::Field(b, f) if self.is_struct_field(&b.ty, f) => {
                let (b, f) = (*b.clone(), f.clone());
                return self.holder(b, line, pre).sub(&f);
            }
            ExprKind::Index(l, i) => {
                if let ExprKind::Field(b, f) = &l.kind {
                    if self.struct_list_field(&b.ty, f).is_some() {
                        let (b, f, i) = (*b.clone(), f.clone(), *i.clone());
                        let i = self.expr(i, line, pre);
                        return self.holder(b, line, pre).item(&f, i);
                    }
                }
            }
            _ => {}
        }
        Holder::Place(self.expr(e, line, pre))
    }

    /// The flattened `(name, value)` pairs for a struct-typed field `f` with value `v`.
    fn spread_field(&mut self, f: &str, s: &str, v: Expr, line: u32, pre: &mut Vec<Stmt>) -> Vec<(Name, Expr)> {
        let h = self.holder(v, line, pre);
        self.leaves(s).into_iter().map(|leaf| (format!("{}__{}", f, leaf.name), h.field(&leaf.name, leaf.ty))).collect()
    }

    /// For `base.field = value` and literals: how `field` is flattened, if it is.
    fn flattened(&self, value_ty: &Ty, field: &str) -> Option<Flattened> {
        // only a field declared with this type is flattened (any struct owner that has it)
        let declared = self.defs.values().any(|fs| fs.iter().any(|f| f.name == field && f.ty == *value_ty));
        if !declared {
            return None;
        }
        match value_ty {
            Ty::Struct(s) => Some(Flattened::Struct(s.clone())),
            Ty::List(e) => match &**e {
                Ty::Struct(s) => Some(Flattened::List(s.clone())),
                _ => None,
            },
            _ => None,
        }
    }

    /// The flattened `(name, value)` pairs for field `f` of a value being written.
    fn spread(&mut self, how: &Flattened, f: &str, v: Expr, line: u32, pre: &mut Vec<Stmt>) -> Vec<(Name, Expr)> {
        match how {
            Flattened::Struct(s) => self.spread_field(f, s, v, line, pre),
            Flattened::List(s) => {
                let v = self.expr(v, line, pre);
                let v = self.place(Rvalue::Expr(v), line, pre);
                self.spread_list(f, s, v, line, pre)
            }
        }
    }

    fn stmt(&mut self, s: Stmt, out: &mut Vec<Stmt>) {
        let saved = std::mem::take(&mut self.post);
        self.stmt_inner(s, out);
        let post = std::mem::replace(&mut self.post, saved);
        out.extend(post);
    }

    fn stmt_inner(&mut self, s: Stmt, out: &mut Vec<Stmt>) {
        let line = s.line;
        let kind = match s.kind {
            StmtKind::Let { var, ty, mutable, value } => StmtKind::Let { var, ty, mutable, value: self.rvalue(value, line, out) },
            StmtKind::Assign { target: LValue::Field { base, field }, value } => match self.flattened(&rv_ty(&value), &field) {
                // `d.svc = s`, `t.members = ms`: every flattened field
                Some(how) => {
                    let v = self.rvalue(value, line, out);
                    let place = self.place(v, line, out);
                    for (name, x) in self.spread(&how, &field, place, line, out) {
                        out.push(Self::st(line, StmtKind::Assign { target: LValue::Field { base: base.clone(), field: name }, value: Rvalue::Expr(x) }));
                    }
                    return;
                }
                None => StmtKind::Assign { target: LValue::Field { base, field }, value: self.rvalue(value, line, out) },
            },
            StmtKind::Assign { target, value } => {
                let target = match target {
                    LValue::Index { list, index } => LValue::Index { list, index: self.expr(index, line, out) },
                    LValue::MapEntry { map, key } => LValue::MapEntry { map, key: self.expr(key, line, out) },
                    t => t,
                };
                StmtKind::Assign { target, value: self.rvalue(value, line, out) }
            }
            StmtKind::Call { call, on_fail } => {
                let call = self.call(call, line, out);
                StmtKind::Call { call, on_fail: self.on_fail(on_fail, line) }
            }
            StmtKind::Command { cmd, on_fail } => StmtKind::Command { cmd: self.cmd(cmd, line, out), on_fail: self.on_fail(on_fail, line) },
            StmtKind::If { cond, then, els } => {
                let cond = self.expr(cond, line, out);
                StmtKind::If { cond, then: self.blk(then), els: els.map(|b| self.blk(b)) }
            }
            StmtKind::While { cond, body } => {
                let mut pre = Vec::new();
                let c = self.expr(cond, line, &mut pre);
                let body = self.blk(body);
                if pre.is_empty() {
                    StmtKind::While { cond: c, body }
                } else {
                    let not_c = Expr { kind: ExprKind::Unary(UnOp::Not, Box::new(c)), ty: Ty::Bool };
                    let mut stmts = pre;
                    stmts.push(Self::st(line, StmtKind::If { cond: not_c, then: Block { stmts: vec![Self::st(line, StmtKind::Break)] }, els: None }));
                    stmts.extend(body.stmts);
                    StmtKind::While { cond: Expr::bool(true), body: Block { stmts } }
                }
            }
            StmtKind::ForRange { var, from, to, inclusive, body } => {
                let from = self.expr(from, line, out);
                let to = self.expr(to, line, out);
                StmtKind::ForRange { var, from, to, inclusive, body: self.blk(body) }
            }
            StmtKind::ForList { var, index, list, body } => {
                let list = self.expr(list, line, out);
                StmtKind::ForList { var, index, list, body: self.blk(body) }
            }
            StmtKind::ForMap { key, value, map, body } => {
                let map = self.expr(map, line, out);
                StmtKind::ForMap { key, value, map, body: self.blk(body) }
            }
            StmtKind::Match { subject, arms, default } => {
                let subject = self.expr(subject, line, out);
                let arms = arms.into_iter().map(|a| Arm { patterns: a.patterns, body: self.blk(a.body) }).collect();
                StmtKind::Match { subject, arms, default: default.map(|b| self.blk(b)) }
            }
            StmtKind::Try { body, on_fail } => StmtKind::Try { body: self.blk(body), on_fail: self.on_fail(on_fail, line) },
            StmtKind::Retry { times, every, body, on_fail } => StmtKind::Retry { times, every, body: self.blk(body), on_fail: self.on_fail(on_fail, line) },
            StmtKind::Parallel { max, body, on_fail } => {
                let body = match body {
                    ParallelBody::For { var, list, body, collect } => {
                        let list = self.expr(list, line, out);
                        let mut body = self.blk(body);
                        let collect = collect.map(|(n, e)| (n, self.expr(e, line, &mut body.stmts)));
                        ParallelBody::For { var, list, body, collect }
                    }
                    ParallelBody::Tasks(bs) => ParallelBody::Tasks(bs.into_iter().map(|b| self.blk(b)).collect()),
                };
                StmtKind::Parallel { max, body, on_fail: self.on_fail(on_fail, line) }
            }
            StmtKind::Fail { message, code } => StmtKind::Fail { message: self.expr(message, line, out), code: code.map(|c| self.expr(c, line, out)) },
            StmtKind::Return(v) => StmtKind::Return(v.map(|e| self.expr(e, line, out))),
            StmtKind::Exit(c) => StmtKind::Exit(c.map(|c| self.expr(c, line, out))),
            StmtKind::Assert { cond, text } => StmtKind::Assert { cond: self.expr(cond, line, out), text },
            StmtKind::Defer(inner) => {
                let mut v = Vec::new();
                self.stmt_inner(*inner, &mut v);
                let last = v.pop().expect("stmt always pushes itself");
                out.extend(v);
                StmtKind::Defer(Box::new(last))
            }
            StmtKind::AssertFails(inner) => {
                let mut v = Vec::new();
                self.stmt_inner(*inner, &mut v);
                let last = v.pop().expect("stmt always pushes itself");
                out.extend(v);
                StmtKind::AssertFails(Box::new(last))
            }
            k => k,
        };
        out.push(Self::st(line, kind));
    }

    fn place(&mut self, rv: Rvalue, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        if let Rvalue::Expr(e) = &rv {
            if matches!(e.kind, ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Index(_, _)) {
                return e.clone();
            }
        }
        let ty = rv_ty(&rv);
        let t = self.tmp();
        pre.push(Self::st(line, StmtKind::Let { var: t.clone(), ty: ty.clone(), mutable: false, value: rv }));
        Expr::var(&t, ty)
    }

    fn rvalue(&mut self, rv: Rvalue, line: u32, pre: &mut Vec<Stmt>) -> Rvalue {
        match rv {
            Rvalue::Expr(e) => Rvalue::Expr(self.expr(e, line, pre)),
            Rvalue::List { elem_ty, items } => Rvalue::List { elem_ty, items: items.into_iter().map(|x| self.expr(x, line, pre)).collect() },
            Rvalue::Map { key_ty, val_ty, entries } => Rvalue::Map { key_ty, val_ty, entries: entries.into_iter().map(|(k, v)| (self.expr(k, line, pre), self.expr(v, line, pre))).collect() },
            Rvalue::Struct { ty, spread, fields } => {
                let spread = spread.map(|x| self.expr(x, line, pre));
                let owner = Ty::Struct(ty.clone());
                let mut out = Vec::new();
                for (f, v) in fields {
                    let how = if self.is_struct_field(&owner, &f) || self.struct_list_field(&owner, &f).is_some() { self.flattened(&v.ty, &f) } else { None };
                    match how {
                        Some(how) => out.extend(self.spread(&how, &f, v, line, pre)),
                        None => out.push((f, self.expr(v, line, pre))),
                    }
                }
                Rvalue::Struct { ty, spread, fields: out }
            }
            Rvalue::Call { call, on_fail } => Rvalue::Call { call: self.call(call, line, pre), on_fail: self.on_fail(on_fail, line) },
            Rvalue::Capture { cmd, on_fail } => Rvalue::Capture { cmd: self.cmd(cmd, line, pre), on_fail: self.on_fail(on_fail, line) },
            Rvalue::CommandOk(cmd) => Rvalue::CommandOk(self.cmd(cmd, line, pre)),
        }
    }

    fn cmd(&mut self, c: Command, line: u32, pre: &mut Vec<Stmt>) -> Command {
        let Command { pipeline, redirects, stdin, text } = c;
        let mut x = |e: Expr, pre: &mut Vec<Stmt>| self.expr(e, line, pre);
        let pipeline = pipeline
            .into_iter()
            .map(|sc| {
                let mut w = |w: Word, pre: &mut Vec<Stmt>| match w {
                    Word::Interp(ps) => Word::Interp(
                        ps.into_iter()
                            .map(|p| match p {
                                Part::Expr(e) => Part::Expr(x(e, pre)),
                                l => l,
                            })
                            .collect(),
                    ),
                    Word::Spread(e) => Word::Spread(x(e, pre)),
                    w => w,
                };
                let env = sc.env.into_iter().map(|(n, v)| (n, w(v, pre))).collect();
                let words = sc.words.into_iter().map(|v| w(v, pre)).collect();
                SimpleCmd { words, env }
            })
            .collect();
        let redirects = redirects
            .into_iter()
            .map(|r| match r {
                Redirect::Out { path, append } => Redirect::Out { path: x(path, pre), append },
                Redirect::Err { path, append } => Redirect::Err { path: x(path, pre), append },
                Redirect::In(p) => Redirect::In(x(p, pre)),
                Redirect::ErrToOut => Redirect::ErrToOut,
            })
            .collect();
        let stdin = stdin.map(|e| x(e, pre));
        Command { pipeline, redirects, stdin, text }
    }

    fn on_fail(&mut self, of: OnFail, line: u32) -> OnFail {
        match of {
            OnFail::Default(rv) => {
                let mut body = Vec::new();
                let rv = self.rvalue(*rv, line, &mut body);
                if body.is_empty() {
                    return OnFail::Default(Box::new(rv));
                }
                let v = self.place(rv, line, &mut body);
                OnFail::Block { err: None, body: Block { stmts: body }, value: Some(v) }
            }
            OnFail::Block { err, body, value } => {
                let mut body = self.blk(body);
                let value = value.map(|v| self.expr(v, line, &mut body.stmts));
                OnFail::Block { err, body, value }
            }
            o => o,
        }
    }

    fn call(&mut self, c: Call, line: u32, pre: &mut Vec<Stmt>) -> Call {
        // `t.members.push(m)` / `pop()`: the call changes a rebuilt copy, written back after the statement
        if let CallTarget::Method { name, .. } = &c.target {
            if matches!(name.as_str(), "push" | "pop") {
                if let Some(Arg::Expr(Expr { kind: ExprKind::Field(b, f), .. })) = c.args.first() {
                    if let Some(s) = self.struct_list_field(&b.ty, f) {
                        let (b, f) = (*b.clone(), f.clone());
                        let h = self.holder(b, line, pre);
                        let copy = self.rebuild_list(&h, &f, &s, line, pre);
                        let mut post = Vec::new();
                        let back = self.spread_list(&f, &s, copy.clone(), line, &mut post);
                        let mut ok = true;
                        for (name, x) in back {
                            match h.lvalue(&name) {
                                Some(lv) => post.push(Self::st(line, StmtKind::Assign { target: lv, value: Rvalue::Expr(x) })),
                                None => ok = false,
                            }
                        }
                        if ok {
                            self.post.extend(post);
                        }
                        let mut args: Vec<Arg> = vec![Arg::Expr(copy.clone())];
                        for a in c.args.into_iter().skip(1) {
                            args.push(match a {
                                Arg::Expr(e) => Arg::Expr(self.expr(e, line, pre)),
                                l => l,
                            });
                        }
                        let target = CallTarget::Method { recv_ty: copy.ty.clone(), name: name.clone() };
                        return Call { target, args, ret: c.ret, fallible: c.fallible };
                    }
                }
            }
        }
        let args = c
            .args
            .into_iter()
            .map(|a| match a {
                Arg::Expr(e) => Arg::Expr(self.expr(e, line, pre)),
                Arg::Lambda(l) => Arg::Lambda(Lambda { params: l.params, ret: l.ret, body: self.blk(l.body) }),
            })
            .collect();
        Call { target: c.target, args, ret: c.ret, fallible: c.fallible }
    }

    /// An expression with no flattened field left: `d.svc.port` → `d.svc__port`, `t.members[i].name` →
    /// `t.members__name[i]`; a whole struct or list of structs → a rebuilt copy.
    fn expr(&mut self, e: Expr, line: u32, pre: &mut Vec<Stmt>) -> Expr {
        let Expr { kind, ty } = e;
        let kind = match kind {
            ExprKind::Field(b, f) => {
                if self.is_struct_field(&b.ty, &f) {
                    let s = match &ty {
                        Ty::Struct(s) => s.clone(),
                        _ => String::new(),
                    };
                    let h = self.holder(Expr { kind: ExprKind::Field(b, f), ty: ty.clone() }, line, pre);
                    return self.rebuild(&h, &s, line, pre);
                }
                if let Some(s) = self.struct_list_field(&b.ty, &f) {
                    let h = self.holder(*b, line, pre);
                    return self.rebuild_list(&h, &f, &s, line, pre);
                }
                let h = self.holder(*b, line, pre);
                return h.field(&f, ty);
            }
            ExprKind::Index(l, i) => {
                if let ExprKind::Field(b, f) = &l.kind {
                    if let Some(s) = self.struct_list_field(&b.ty, f) {
                        // a whole item: rebuilt from its per-field lists
                        let h = self.holder(Expr { kind: ExprKind::Index(l, i), ty: ty.clone() }, line, pre);
                        return self.rebuild(&h, &s, line, pre);
                    }
                }
                ExprKind::Index(Box::new(self.expr(*l, line, pre)), Box::new(self.expr(*i, line, pre)))
            }
            ExprKind::Len(x) => {
                if let ExprKind::Field(b, f) = &x.kind {
                    if let Some(s) = self.struct_list_field(&b.ty, f) {
                        let (b, f) = (*b.clone(), f.clone());
                        let h = self.holder(b, line, pre);
                        return match self.cols(&h, &f, &s).into_iter().next() {
                            Some((_, c)) => Expr { kind: ExprKind::Len(Box::new(c)), ty: Ty::Int },
                            None => Expr::int(0),
                        };
                    }
                }
                ExprKind::Len(Box::new(self.expr(*x, line, pre)))
            }
            ExprKind::Unary(op, x) => ExprKind::Unary(op, Box::new(self.expr(*x, line, pre))),
            ExprKind::Binary(op, a, b) => {
                let a = self.expr(*a, line, pre);
                let b = self.expr(*b, line, pre);
                ExprKind::Binary(op, Box::new(a), Box::new(b))
            }
            ExprKind::Interp(ps) => ExprKind::Interp(
                ps.into_iter()
                    .map(|p| match p {
                        Part::Expr(e) => Part::Expr(self.expr(e, line, pre)),
                        l => l,
                    })
                    .collect(),
            ),
            k => k,
        };
        Expr { kind, ty }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_types() {
        assert!(nested(&Ty::list(Ty::list(Ty::Int))));
        assert!(nested(&Ty::map(Ty::Str, Ty::list(Ty::Str))));
        assert!(!nested(&Ty::list(Ty::Int)));
        assert!(!nested(&Ty::map(Ty::Str, Ty::Int)));
        assert_eq!(enc_ty(&Ty::list(Ty::list(Ty::Int))), Ty::list(Ty::Str));
        assert_eq!(enc_ty(&Ty::map(Ty::Enum("E".into()), Ty::list(Ty::Int))), Ty::map(Ty::Enum("E".into()), Ty::Str));
    }

    #[test]
    fn skips_programs_without_nesting() {
        let mut p = Program::empty("x", crate::Target::Linux);
        p.body.stmts.push(Stmt { line: 1, kind: StmtKind::Let { var: "m".into(), ty: Ty::map(Ty::Str, Ty::Int), mutable: false, value: Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::Int, entries: vec![] } } });
        assert!(!uses_nested(&p));
        p.body.stmts.push(Stmt { line: 2, kind: StmtKind::Let { var: "g".into(), ty: Ty::map(Ty::Enum("E".into()), Ty::list(Ty::Str)), mutable: false, value: Rvalue::Map { key_ty: Ty::Str, val_ty: Ty::list(Ty::Str), entries: vec![] } } });
        assert!(uses_nested(&p));
    }
}
