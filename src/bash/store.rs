//! Copying values to destinations: `let`, assignment, return, list/map/struct literals and maps.
//! Every copy is by value (arrays copied, structs field by field, maps with a new index).

use super::gen::Gen;
use super::quote::W;
use super::val::{Cond, Dest, Loc, Val};
use crate::ir::*;

/// Transport variables (`__ret…`): maps without an index.
fn is_transfer(base: &str) -> bool {
    base.starts_with("__ret")
}

impl<'a> Gen<'a> {
    /// The value of an expression (scalar as word/condition; collections where they live).
    pub fn expr_val(&mut self, e: &Expr) -> Val {
        match &e.ty {
            Ty::List(inner) => {
                let l = self.loc(e);
                let b = match l {
                    Loc::Base(b) => b,
                    Loc::Elem { .. } => unreachable!(),
                };
                match &**inner {
                    Ty::Struct(s) => Val::SList(b, s.clone()),
                    _ => Val::List(format!("\"${{{}[@]}}\"", b)),
                }
            }
            Ty::Map(_, _) => match self.loc(e) {
                Loc::Base(b) => Val::Map(b),
                Loc::Elem { .. } => unreachable!(),
            },
            Ty::Struct(s) => {
                let l = self.loc(e);
                Val::Struct(l, s.clone())
            }
            Ty::Error => {
                let l = self.loc(e);
                Val::Struct(l, String::new())
            }
            Ty::Bool => match &e.kind {
                ExprKind::Binary(_, _, _) | ExprKind::Unary(UnOp::Not, _) => Val::Cond(self.cond(e)),
                _ => Val::Word(self.word(e)),
            },
            _ => Val::Word(self.word(e)),
        }
    }

    /// The value of a field of a struct at `loc`.
    pub fn field_val(&mut self, loc: &Loc, f: &Field) -> Val {
        match (loc, &f.ty) {
            (Loc::Base(p), Ty::List(_)) => Val::List(format!("\"${{{}__{}[@]}}\"", p, f.name)),
            (Loc::Base(p), Ty::Map(_, _)) => Val::Map(format!("{}__{}", p, f.name)),
            (Loc::Base(p), _) => Val::Word(W::var(&format!("{}__{}", p, f.name))),
            (Loc::Elem { list, idx }, Ty::List(_)) => {
                self.need("__dec_a");
                let t = self.tmp(&f.ty);
                self.line(format!("__dec_a \"${{{}__{}[{}]}}\"; {}=(\"${{__ret_a[@]}}\")", list, f.name, idx, t));
                Val::List(format!("\"${{{}[@]}}\"", t))
            }
            (Loc::Elem { list, idx }, Ty::Map(_, _)) => {
                self.need("__dec_m");
                let t = self.tmp(&Ty::List(Box::new(Ty::Str)));
                self.line(format!(
                    "__dec_m \"${{{l}__{f}[{i}]}}\"; {t}__k=(\"${{__ret_a[@]}}\"); {t}__v=(\"${{__ret_b[@]}}\")",
                    l = list,
                    f = f.name,
                    i = idx,
                    t = t
                ));
                // `t` only has k/v: good enough for copying
                Val::Map(t)
            }
            (Loc::Elem { list, idx }, _) => Val::Word(W::var(&format!("{}__{}[{}]", list, f.name, idx))),
        }
    }

    /// Encodes a list/map as text (composite field of a list-of-structs item).
    pub fn encode(&mut self, v: &Val) -> W {
        match v {
            Val::List(c) => {
                self.need("__enc_a");
                let t = self.tmp(&Ty::Str);
                self.line(format!("__enc_a {}; {}=$__ret", c, t));
                W::var(&t)
            }
            Val::Map(p) => {
                self.need("__enc_m");
                let t = self.tmp(&Ty::Str);
                self.line(format!("__enc_m \"${{#{p}__k[@]}}\" \"${{{p}__k[@]}}\" \"${{{p}__v[@]}}\"; {t}=$__ret", p = p, t = t));
                W::var(&t)
            }
            Val::Word(w) => w.clone(),
            Val::Cond(c) => self.cond_word(c),
            _ => W::lit(""),
        }
    }

    /// `true`/`false` of a condition, as a word.
    pub fn cond_word(&mut self, c: &Cond) -> W {
        match c.clone().simplify() {
            Cond::True => W::lit("true"),
            Cond::False => W::lit("false"),
            c => {
                let t = self.tmp(&Ty::Bool);
                self.line(format!("if {}; then {t}=true; else {t}=false; fi", c.render(), t = t));
                W::var(&t)
            }
        }
    }

    // ------------------------------------------------------------------------------------------
    // store
    // ------------------------------------------------------------------------------------------

    pub fn store(&mut self, dest: &Dest, val: &Val) {
        match dest {
            Dest::Discard => {
                if let Val::Cond(c) = val {
                    if matches!(c, Cond::Cmd(_)) {
                        self.line(format!("{} || :", c.render()));
                    }
                }
            }
            Dest::Var(base, ty) => self.store_var(base, ty, val),
            Dest::ListElem(arr, idx, _) => {
                let w = match val {
                    Val::Cond(c) => self.cond_word(c),
                    Val::Word(w) => w.clone(),
                    _ => W::lit(""),
                };
                self.line(format!("if (( {i} >= 0 && {i} < ${{#{a}[@]}} )); then {a}[{i}]={w}; fi", i = idx, a = arr, w = w.render()));
            }
            Dest::StructElem(list, idx, s) => {
                let fields = self.struct_fields(s);
                let Val::Struct(src, _) = val else { return };
                let mut sets = Vec::new();
                for f in &fields {
                    let v = self.field_val(src, f);
                    let w = self.encode(&v);
                    sets.push(format!("{}__{}[{}]={}", list, f.name, idx, w.render()));
                }
                let first = fields.first().map(|f| f.name.clone()).unwrap_or_default();
                self.line(format!("if (( {i} >= 0 && {i} < ${{#{l}__{f}[@]}} )); then", i = idx, l = list, f = first));
                for s in sets {
                    self.line(format!("  {}", s));
                }
                self.line("fi");
            }
            Dest::MapEntry(m, key, _) => {
                let w = match val {
                    Val::Cond(c) => self.cond_word(c),
                    Val::Word(w) => w.clone(),
                    _ => W::lit(""),
                };
                self.map_set(m, key, &w);
            }
        }
    }

    fn store_var(&mut self, base: &str, ty: &Ty, val: &Val) {
        match (ty, val) {
            (_, Val::Unit) => {}
            (Ty::List(inner), _) if matches!(**inner, Ty::Struct(_)) => {
                let Ty::Struct(s) = &**inner else { unreachable!() };
                let fields = self.struct_fields(s);
                if let Val::SList(src, _) = val {
                    if src == base {
                        return;
                    }
                    for f in &fields {
                        self.line(format!("{b}__{f}=(\"${{{s}__{f}[@]}}\")", b = base, f = f.name, s = src));
                    }
                }
            }
            (Ty::List(_), Val::List(c)) => {
                if *c == format!("\"${{{}[@]}}\"", base) {
                    return;
                }
                self.line(format!("{}=({})", base, c));
            }
            (Ty::Map(_, _), Val::Map(src)) => {
                if src != base {
                    self.line(format!("{b}__k=(\"${{{s}__k[@]}}\"); {b}__v=(\"${{{s}__v[@]}}\")", b = base, s = src));
                }
                if !is_transfer(base) {
                    self.map_reindex(base, false);
                }
            }
            (Ty::Struct(s), Val::Struct(src, _)) => {
                let fields = self.struct_fields(s);
                if let Loc::Base(b) = src {
                    if b == base {
                        return;
                    }
                }
                for f in &fields {
                    let v = self.field_val(src, f);
                    self.store_var(&format!("{}__{}", base, f.name), &f.ty, &v);
                }
            }
            (Ty::Error, Val::Struct(src, _)) => {
                let (m, c) = match src {
                    Loc::Base(b) if b == "__err" => ("__err_msg".to_string(), "__err_code".to_string()),
                    Loc::Base(b) => (format!("{}__message", b), format!("{}__code", b)),
                    Loc::Elem { .. } => return,
                };
                self.line(format!("{b}__message=${m}; {b}__code=${c}", b = base, m = m, c = c));
            }
            (_, Val::Cond(c)) => match c.clone().simplify() {
                Cond::True => self.line(format!("{}=true", base)),
                Cond::False => self.line(format!("{}=false", base)),
                c => self.line(format!("if {}; then {b}=true; else {b}=false; fi", c.render(), b = base)),
            },
            (_, Val::Word(w)) => {
                if w.as_simple_var().as_deref() == Some(base) {
                    return;
                }
                self.line(format!("{}={}", base, w.render_assign()));
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------
    // Rvalues
    // ------------------------------------------------------------------------------------------

    pub fn assign_rv(&mut self, dest: &Dest, ty: &Ty, rv: &Rvalue) {
        match rv {
            Rvalue::Expr(e) => {
                let v = self.expr_val(e);
                self.store(dest, &v);
            }
            Rvalue::List { elem_ty, items } => self.list_literal(dest, elem_ty, items),
            Rvalue::Map { entries, .. } => self.map_literal(dest, ty, entries),
            Rvalue::Struct { ty: sname, spread, fields } => self.struct_literal(dest, sname, spread.as_ref(), fields),
            Rvalue::Call { call, on_fail } => self.call_into(dest, ty, call, on_fail),
            Rvalue::Capture { cmd, on_fail } => self.capture_into(dest, ty, cmd, on_fail),
            Rvalue::CommandOk(cmd) => {
                let c = self.command_ok(cmd);
                self.store(dest, &Val::Cond(c));
            }
        }
    }

    pub fn assign_rv_ret(&mut self, dest: &Dest, ty: &Ty, e: &Expr) {
        let v = self.expr_val(e);
        self.store(dest, &v);
        let _ = ty;
    }

    pub fn assign_lvalue(&mut self, lv: &LValue, rv: &Rvalue) {
        match lv {
            LValue::Var(n) => {
                let ty = self.var_ty(n);
                let b = self.vname(n);
                // `x += y` (x = x op y with integers) becomes `(( x += y ))`: faster in loops
                if let Rvalue::Expr(Expr { kind: ExprKind::Binary(op @ (BinOp::Add | BinOp::Sub | BinOp::Mul), l, r), .. }) = rv {
                    if matches!(&l.kind, ExprKind::Var(x) if x == n) && super::gen::is_int(&ty) {
                        let ra = self.arith(r);
                        let o = match op {
                            BinOp::Add => "+=",
                            BinOp::Sub => "-=",
                            _ => "*=",
                        };
                        let ra = if ra.contains(' ') { format!("({})", ra) } else { ra };
                        self.line(format!("(( {} {} {} ))", b, o, ra));
                        return;
                    }
                }
                self.assign_rv(&Dest::Var(b, ty.clone()), &ty, rv);
            }
            LValue::Field { base, field } => {
                let bty = self.var_ty(base);
                let fty = match &bty {
                    Ty::Struct(s) => self.struct_fields(s).into_iter().find(|f| &f.name == field).map(|f| f.ty).unwrap_or(Ty::Str),
                    _ => Ty::Str,
                };
                let b = format!("{}__{}", self.vname(base), field);
                self.assign_rv(&Dest::Var(b, fty.clone()), &fty, rv);
            }
            LValue::Index { list, index } => {
                let lty = self.var_ty(list);
                let elem = match &lty {
                    Ty::List(e) => (**e).clone(),
                    _ => Ty::Str,
                };
                let b = self.vname(list);
                let i = self.arith(index);
                let i = if i.contains(' ') { format!("({})", i) } else { i };
                match &elem {
                    Ty::Struct(s) => self.assign_rv(&Dest::StructElem(b, i, s.clone()), &elem, rv),
                    _ => self.assign_rv(&Dest::ListElem(b, i, elem.clone()), &elem, rv),
                }
            }
            LValue::MapEntry { map, key } => {
                let mty = self.var_ty(map);
                let vty = match &mty {
                    Ty::Map(_, v) => (**v).clone(),
                    _ => Ty::Str,
                };
                let b = self.vname(map);
                let k = self.word(key);
                let k = self.stable_word(k);
                self.assign_rv(&Dest::MapEntry(b, k, vty.clone()), &vty, rv);
            }
        }
    }

    /// A word that does not change with calls in between (copies `$__ret` etc. to a temporary).
    pub fn stable_word(&mut self, w: W) -> W {
        let r = w.render();
        if r.contains("__ret") || r.contains("__mp") {
            let t = self.tmp(&Ty::Str);
            self.line(format!("{}={}", t, r));
            W::var(&t)
        } else {
            w
        }
    }

    fn list_literal(&mut self, dest: &Dest, elem_ty: &Ty, items: &[Expr]) {
        match elem_ty {
            Ty::Struct(s) => {
                let fields = self.struct_fields(s);
                let mut cols: Vec<Vec<String>> = vec![Vec::new(); fields.len()];
                for it in items {
                    let l = self.loc(it);
                    for (fi, f) in fields.iter().enumerate() {
                        let v = self.field_val(&l, f);
                        let w = self.encode(&v);
                        cols[fi].push(w.render());
                    }
                }
                let base = match dest {
                    Dest::Var(b, _) => b.clone(),
                    _ => return,
                };
                for (fi, f) in fields.iter().enumerate() {
                    self.line(format!("{}__{}=({})", base, f.name, cols[fi].join(" ")));
                }
            }
            _ => {
                let mut ws = Vec::new();
                for it in items {
                    let v = self.expr_val(it);
                    let w = self.encode(&v);
                    ws.push(w.render());
                }
                self.store(dest, &Val::List(ws.join(" ")));
            }
        }
    }

    fn map_literal(&mut self, dest: &Dest, ty: &Ty, entries: &[(Expr, Expr)]) {
        let Dest::Var(base, _) = dest else { return };
        let _ = ty;
        self.uses_maps = true;
        let mut lits = Vec::new();
        let mut all_lit = true;
        for (k, _) in entries {
            match self.word(k).as_lit() {
                Some(l) if !lits.contains(&l) => lits.push(l),
                _ => all_lit = false,
            }
        }
        if all_lit {
            let mut ks = Vec::new();
            let mut vs = Vec::new();
            for (k, v) in entries {
                ks.push(self.word(k).render());
                let vv = self.expr_val(v);
                let w = self.encode(&vv);
                vs.push(w.render());
            }
            self.line(format!("{b}__k=({k}); {b}__v=({v})", b = base, k = ks.join(" "), v = vs.join(" ")));
            if !is_transfer(base) {
                self.map_reindex(base, true);
            }
        } else {
            self.line(format!("{b}__k=(); {b}__v=()", b = base));
            self.map_reindex(base, true);
            for (k, v) in entries {
                let kw = self.word(k);
                let vv = self.expr_val(v);
                let w = self.encode(&vv);
                self.map_set(base, &kw, &w);
            }
        }
    }

    fn struct_literal(&mut self, dest: &Dest, sname: &str, spread: Option<&Expr>, fields: &[(String, Expr)]) {
        let defs = self.struct_fields(sname);
        let spread_loc = spread.map(|e| self.loc(e));
        let mut vals: Vec<(Field, Val)> = Vec::new();
        for f in &defs {
            let v = match fields.iter().find(|(n, _)| n == &f.name) {
                Some((_, e)) => self.expr_val(e),
                None => match &spread_loc {
                    Some(l) => self.field_val(l, f),
                    None => Val::Word(W::lit("")),
                },
            };
            vals.push((f.clone(), v));
        }
        match dest {
            Dest::Var(base, _) => {
                // if any value reads the destination itself (`s = T { a: s.b, b: s.a }`), copy first
                let prefix = format!("{}__", base);
                let reads_dest = vals.iter().any(|(_, v)| match v {
                    Val::Word(w) => w.render().contains(&prefix),
                    Val::List(c) => c.contains(&prefix),
                    Val::Map(p) => p.starts_with(&prefix),
                    _ => false,
                });
                if reads_dest {
                    let mut fixed = Vec::new();
                    for (f, v) in vals {
                        let v2 = match v {
                            Val::Word(w) if f.ty.is_scalar() => {
                                let t = self.tmp(&f.ty);
                                self.line(format!("{}={}", t, w.render()));
                                Val::Word(W::var(&t))
                            }
                            Val::List(c) => {
                                let t = self.tmp(&f.ty);
                                self.line(format!("{}=({})", t, c));
                                Val::List(format!("\"${{{}[@]}}\"", t))
                            }
                            Val::Map(p) => {
                                let t = self.tmp(&Ty::List(Box::new(Ty::Str)));
                                self.line(format!("{t}__k=(\"${{{p}__k[@]}}\"); {t}__v=(\"${{{p}__v[@]}}\")", t = t, p = p));
                                Val::Map(t)
                            }
                            v => v,
                        };
                        fixed.push((f, v2));
                    }
                    vals = fixed;
                }
                for (f, v) in &vals {
                    self.store_var(&format!("{}__{}", base, f.name), &f.ty, v);
                }
            }
            Dest::StructElem(list, idx, _) => {
                let mut sets = Vec::new();
                for (f, v) in &vals {
                    let w = self.encode(v);
                    sets.push(format!("{}__{}[{}]={}", list, f.name, idx, w.render()));
                }
                let first = defs.first().map(|f| f.name.clone()).unwrap_or_default();
                self.line(format!("if (( {i} >= 0 && {i} < ${{#{l}__{f}[@]}} )); then", i = idx, l = list, f = first));
                for s in sets {
                    self.line(format!("  {}", s));
                }
                self.line("fi");
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------
    // Maps
    // ------------------------------------------------------------------------------------------

    /// Rebuilds the map index at `base` from `base__k`.
    pub fn map_reindex(&mut self, base: &str, _fresh: bool) {
        self.uses_maps = true;
        if self.linux {
            self.extra_local("__mi");
            if self.in_top() && !self.binding {
                self.line(format!("declare -A {}__i", base));
            }
            self.line(format!("{b}__i=(); for (( __mi = 0; __mi < ${{#{b}__k[@]}}; __mi++ )); do {b}__i[\"_${{{b}__k[__mi]}}\"]=$__mi; done", b = base));
        } else {
            self.need("__map_reset");
            self.need("__map_index");
            self.line(format!("__map_reset {b}__id \"${{{b}__n[@]}}\"; __mi_k=(\"${{{b}__k[@]}}\"); __map_index \"${b}__id\"; {b}__n=(\"${{__ret_a[@]}}\")", b = base));
        }
    }

    /// Key for the Linux index: `_` + key.
    pub fn lkey(&self, key: &W) -> String {
        W::lit("_").concat(key.clone()).render()
    }

    /// `m[k] = v`.
    pub fn map_set(&mut self, m: &str, key: &W, val: &W) {
        let k = key.render();
        let v = val.render();
        if self.linux {
            let lk = self.lkey(key);
            self.line(format!(
                "if [[ -n ${{{m}__i[{lk}]+x}} ]]; then {m}__v[${{{m}__i[{lk}]}}]={v}; else {m}__i[{lk}]=${{#{m}__k[@]}}; {m}__k+=({k}); {m}__v+=({v}); fi",
                m = m,
                lk = lk,
                k = k,
                v = v
            ));
        } else {
            self.need("__map_put");
            self.line(format!(
                "__map_put \"${m}__id\" {k} \"${{#{m}__k[@]}}\" || {{ {m}__k[__mp]={k}; {m}__n[__mp]=$__mn; }}; {m}__v[__mp]={v}",
                m = m,
                k = k,
                v = v
            ));
        }
    }
}
