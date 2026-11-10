//! Pure expressions: words (`W`), arithmetic, conditions and where collections live.

use super::gen::{is_int, Gen};
use super::quote::W;
use super::val::{Cond, Loc};
use crate::ir::*;

fn prec(op: BinOp) -> u8 {
    match op {
        BinOp::Mul | BinOp::Div | BinOp::Mod => 2,
        BinOp::Add | BinOp::Sub => 1,
        _ => 0,
    }
}

fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => "?",
    }
}

impl<'a> Gen<'a> {
    // ------------------------------------------------------------------------------------------
    // Scalar places
    // ------------------------------------------------------------------------------------------

    /// The name usable in `${…}` for a stored scalar: `x`, `s__field`, `xs[i]`. `None` if the
    /// expression is not a place (it is a computation, a literal…).
    pub fn scalar_name(&mut self, e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Var(n) => Some(self.vname(n)),
            ExprKind::DryRun => Some("__dry".into()),
            ExprKind::Field(base, f) => {
                if base.ty == Ty::Error {
                    return Some(match (&base.kind, f.as_str()) {
                        (ExprKind::CurrentErr, "message") => "__err_msg".into(),
                        (ExprKind::CurrentErr, _) => "__err_code".into(),
                        (_, f) => match self.loc(base) {
                            Loc::Base(b) => format!("{}__{}", b, f),
                            Loc::Elem { list, idx } => format!("{}__{}[{}]", list, f, idx),
                        },
                    });
                }
                match self.loc(base) {
                    Loc::Base(b) => Some(format!("{}__{}", b, f)),
                    Loc::Elem { list, idx } => Some(format!("{}__{}[{}]", list, f, idx)),
                }
            }
            ExprKind::Index(list, i) => {
                let l = match self.loc(list) {
                    Loc::Base(b) => b,
                    Loc::Elem { .. } => return None,
                };
                let ix = self.idx(i);
                Some(format!("{}[{}]", l, ix))
            }
            _ => None,
        }
    }

    /// Safe list index: negative ones become an index that does not exist (empty value, as Nalu requires).
    pub fn idx(&mut self, i: &Expr) -> String {
        match &i.kind {
            ExprKind::Int(n) if *n >= 0 => n.to_string(),
            ExprKind::Var(v) if self.frame().nonneg.contains(&self.vname(v)) => self.vname(v),
            _ => {
                let a = self.arith(i);
                if a.contains(' ') {
                    format!("({a}) < 0 ? 1<<62 : ({a})", a = a)
                } else {
                    format!("{a} < 0 ? 1<<62 : {a}", a = a)
                }
            }
        }
    }

    /// A variable name holding the scalar value of `e` (creates a temporary if needed).
    pub fn as_var(&mut self, e: &Expr) -> String {
        if let Some(n) = self.scalar_name(e) {
            if !n.contains('[') {
                return n;
            }
        }
        let w = self.word(e);
        let t = self.tmp(&e.ty);
        self.line(format!("{}={}", t, w.render()));
        t
    }

    /// Where a list, map or struct lives.
    pub fn loc(&mut self, e: &Expr) -> Loc {
        match &e.kind {
            ExprKind::Var(n) => Loc::Base(self.vname(n)),
            ExprKind::Args => Loc::Base("args".into()),
            ExprKind::CurrentErr => Loc::Base("__err".into()),
            ExprKind::Field(base, f) => match self.loc(base) {
                Loc::Base(b) => Loc::Base(format!("{}__{}", b, f)),
                Loc::Elem { list, idx } => {
                    // list/map field of a list-of-structs item: decode into a temporary
                    let src = format!("\"${{{}__{}[{}]}}\"", list, f, idx);
                    let t = self.tmp(&e.ty);
                    match &e.ty {
                        Ty::Map(_, _) => {
                            self.need("__dec_m");
                            self.line(format!("__dec_m {}", src));
                            self.line(format!("{t}__k=(\"${{__ret_a[@]}}\"); {t}__v=(\"${{__ret_b[@]}}\")", t = t));
                            self.map_reindex(&t, true);
                        }
                        _ => {
                            self.need("__dec_a");
                            self.line(format!("__dec_a {}", src));
                            self.line(format!("{}=(\"${{__ret_a[@]}}\")", t));
                        }
                    }
                    Loc::Base(t)
                }
            },
            ExprKind::Index(list, i) => {
                let l = match self.loc(list) {
                    Loc::Base(b) => b,
                    Loc::Elem { .. } => unreachable!(),
                };
                let ix = self.idx(i);
                Loc::Elem { list: l, idx: ix }
            }
            _ => Loc::Base(self.vname("__invalido")),
        }
    }

    /// The array name of a list of scalars (decoding if needed).
    pub fn list_arr(&mut self, e: &Expr) -> String {
        match self.loc(e) {
            Loc::Base(b) => b,
            Loc::Elem { .. } => unreachable!(),
        }
    }

    /// Number of items of a list at `base` (scalars or structs).
    pub fn list_len(&self, base: &str, ty: &Ty) -> String {
        match ty {
            Ty::List(e) => match &**e {
                Ty::Struct(s) => match self.struct_fields(s).first() {
                    Some(f) => format!("${{#{}__{}[@]}}", base, f.name),
                    None => "0".into(),
                },
                _ => format!("${{#{}[@]}}", base),
            },
            Ty::Map(_, _) => format!("${{#{}__k[@]}}", base),
            _ => format!("${{#{}}}", base),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Words
    // ------------------------------------------------------------------------------------------

    /// The scalar value of `e` as a shell word.
    pub fn word(&mut self, e: &Expr) -> W {
        match &e.kind {
            ExprKind::Str(s) => W::lit(s),
            ExprKind::Int(n) => W::lit(&n.to_string()),
            ExprKind::Duration(n) => W::lit(&n.to_string()),
            ExprKind::Bool(b) => W::lit(if *b { "true" } else { "false" }),
            ExprKind::EnumCase { case, .. } => W::lit(case),
            ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Index(_, _) | ExprKind::DryRun => match self.scalar_name(e) {
                // list item out of range: 0 for numbers, false for bool (text: empty)
                Some(n) if n.contains('[') && is_int(&e.ty) => W::exp(format!("${{{}:-0}}", n)),
                Some(n) if n.contains('[') && e.ty == Ty::Bool => W::exp(format!("${{{}:-false}}", n)),
                Some(n) => W::var(&n),
                None => W::lit(""),
            },
            ExprKind::Args | ExprKind::CurrentErr => W::var("__err_msg"),
            ExprKind::Len(_) => W::exp(self.len_exp(e)),
            ExprKind::Interp(parts) => {
                let mut w = W::default();
                for p in parts {
                    match p {
                        Part::Lit(s) => w.push_lit(s),
                        Part::Expr(x) => {
                            let xw = self.word(x);
                            w.push(xw);
                        }
                    }
                }
                w
            }
            ExprKind::Unary(UnOp::Neg, _) => W::exp(format!("$(( {} ))", self.arith(e))),
            ExprKind::Binary(op, a, b) => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => W::exp(format!("$(( {} ))", self.arith(e))),
                BinOp::Concat => {
                    let mut w = self.word(a);
                    let wb = self.word(b);
                    w.push(wb);
                    w
                }
                BinOp::PathJoin => self.path_join(a, b),
                _ => self.bool_word(e),
            },
            ExprKind::Unary(UnOp::Not, _) => self.bool_word(e),
        }
    }

    /// `base / child`: `${b:+${b%/}/}child` (empty base = just the child).
    pub fn path_join(&mut self, a: &Expr, b: &Expr) -> W {
        let wa = self.word(a);
        let wb = self.word(b);
        if let Some(l) = wa.as_lit() {
            if l.is_empty() {
                return wb;
            }
            let base = l.strip_suffix('/').unwrap_or(&l).to_string();
            let mut w = W::lit(&format!("{}/", base));
            w.push(wb);
            return w;
        }
        let n = match wa.as_simple_var() {
            Some(v) => v,
            None => match self.scalar_name(a) {
                Some(n) => n,
                None => {
                    let t = self.tmp(&Ty::Path);
                    self.line(format!("{}={}", t, wa.render()));
                    t
                }
            },
        };
        let mut w = W::exp(format!("${{{n}:+${{{n}%/}}/}}", n = n));
        w.push(wb);
        w
    }

    /// A computed `bool` (comparison etc.) as a word: `true`/`false` in a temporary.
    fn bool_word(&mut self, e: &Expr) -> W {
        let c = self.cond(e).simplify();
        match c {
            Cond::True => W::lit("true"),
            Cond::False => W::lit("false"),
            c => {
                let t = self.tmp(&Ty::Bool);
                self.line(format!("if {}; then {t}=true; else {t}=false; fi", c.render(), t = t));
                W::var(&t)
            }
        }
    }

    fn len_exp(&mut self, e: &Expr) -> String {
        let ExprKind::Len(x) = &e.kind else { unreachable!() };
        match &x.ty {
            Ty::List(_) | Ty::Map(_, _) => {
                let b = match self.loc(x) {
                    Loc::Base(b) => b,
                    Loc::Elem { .. } => unreachable!(),
                };
                self.list_len(&b, &x.ty)
            }
            _ => {
                let n = self.as_var(x);
                format!("${{#{}}}", n)
            }
        }
    }

    /// A word for an integer in command position (`exit N`, `__err_code=N`).
    pub fn arith_word(&mut self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(n) | ExprKind::Duration(n) => n.to_string(),
            _ => match self.scalar_name(e) {
                Some(n) if !n.contains('[') => format!("\"${}\"", n),
                _ => format!("\"$(( {} ))\"", self.arith(e)),
            },
        }
    }

    // ------------------------------------------------------------------------------------------
    // Arithmetic
    // ------------------------------------------------------------------------------------------

    /// Expression for use inside `$(( ))`.
    pub fn arith(&mut self, e: &Expr) -> String {
        self.arith_p(e, 0, false)
    }

    fn arith_p(&mut self, e: &Expr, min: u8, right: bool) -> String {
        match &e.kind {
            ExprKind::Int(n) | ExprKind::Duration(n) => {
                if *n < 0 {
                    format!("({})", n)
                } else {
                    n.to_string()
                }
            }
            ExprKind::Bool(b) => (if *b { "1" } else { "0" }).into(),
            ExprKind::Var(_) | ExprKind::Field(_, _) | ExprKind::Index(_, _) => match self.scalar_name(e) {
                Some(n) => n,
                None => "0".into(),
            },
            ExprKind::Len(_) => self.len_exp(e),
            ExprKind::Unary(UnOp::Neg, x) => format!("-{}", self.arith_p(x, 3, false)),
            ExprKind::Binary(op, a, b) if prec(*op) > 0 => {
                let p = prec(*op);
                let l = self.arith_p(a, p, false);
                let r = self.arith_p(b, p, true);
                let s = format!("{} {} {}", l, op_str(*op), r);
                if p < min || (p == min && right) {
                    format!("({})", s)
                } else {
                    s
                }
            }
            _ => {
                // text that contains a number (e.g. a comparison result does not get here)
                let w = self.word(e);
                let t = self.tmp(&Ty::Int);
                self.line(format!("{}={}", t, w.render()));
                t
            }
        }
    }

    // ------------------------------------------------------------------------------------------
    // Conditions
    // ------------------------------------------------------------------------------------------

    /// Left side of a `[[ … ]]` test.
    fn test_lhs(&mut self, e: &Expr) -> String {
        let w = self.word(e);
        match w.as_simple_var() {
            Some(v) => format!("${}", v),
            None => w.render(),
        }
    }

    pub fn cond(&mut self, e: &Expr) -> Cond {
        match &e.kind {
            ExprKind::Bool(true) => Cond::True,
            ExprKind::Bool(false) => Cond::False,
            ExprKind::Unary(UnOp::Not, x) => Cond::not(self.cond(x)),
            ExprKind::Binary(BinOp::And, a, b) => {
                let ca = self.cond(a);
                let cb = self.cond(b);
                Cond::And(vec![ca, cb])
            }
            ExprKind::Binary(BinOp::Or, a, b) => {
                let ca = self.cond(a);
                let cb = self.cond(b);
                Cond::Or(vec![ca, cb])
            }
            ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge), a, b) => {
                if is_int(&a.ty) || is_int(&b.ty) {
                    let l = self.arith(a);
                    let r = self.arith(b);
                    return Cond::Arith(format!("{} {} {}", l, op_str(*op), r));
                }
                match op {
                    BinOp::Eq | BinOp::Ne => {
                        let l = self.test_lhs(a);
                        let r = self.word(b).render();
                        Cond::Test(format!("{} {} {}", l, op_str(*op), r))
                    }
                    _ => {
                        self.need("__str_lt");
                        let l = self.word(a).render();
                        let r = self.word(b).render();
                        match op {
                            BinOp::Lt => Cond::Cmd(format!("__str_lt {} {}", l, r)),
                            BinOp::Gt => Cond::Cmd(format!("__str_lt {} {}", r, l)),
                            BinOp::Le => Cond::not(Cond::Cmd(format!("__str_lt {} {}", r, l))),
                            _ => Cond::not(Cond::Cmd(format!("__str_lt {} {}", l, r))),
                        }
                    }
                }
            }
            ExprKind::DryRun => Cond::Test("$__dry == true".into()),
            _ => {
                // bool variable/field/index (stored as text)
                let l = self.test_lhs(e);
                Cond::Test(format!("{} == true", l))
            }
        }
    }
}
