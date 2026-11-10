//! Command-line parameter reader (`--x v`, `--x=v`, flags, `--help`, `--`, `args`).

use super::gen::Gen;
use super::quote::sq;
use super::val::Dest;
use crate::ir::*;

impl<'a> Gen<'a> {
    pub fn gen_params(&mut self) {
        let p = self.p;
        // --help: one line per printf item
        self.line("__usage() {");
        let lines: Vec<String> = p.usage.split('\n').map(|l| sq(l)).collect();
        if lines.len() == 1 {
            self.line(format!("  printf '%s\\n' {}", lines[0]));
        } else {
            self.line("  printf '%s\\n' \\");
            let n = lines.len();
            for (i, l) in lines.iter().enumerate() {
                self.line(format!("    {}{}", l, if i + 1 < n { " \\" } else { "" }));
            }
        }
        self.line("}");
        self.need("__usage_err");
        if !p.params.is_empty() {
            let names: Vec<String> = p.params.iter().map(|x| format!("__o_{}", x.name)).collect();
            self.line(format!("unset {}", names.join(" ")));
        }
        self.line("args=()");
        self.line("while (( $# > 0 )); do");
        self.line("  case $1 in");
        self.line("    --dry-run) __dry=true ;;");
        self.line("    --verbose) __verbose=true ;;");
        self.line("    -h|--help) __usage; exit 0 ;;");
        for prm in &p.params {
            let o = format!("__o_{}", prm.name);
            let f = &prm.flag;
            if prm.ty == Ty::Bool {
                self.line(format!("    {}) {}=true ;;", f, o));
            } else {
                self.line(format!(
                    "    {f}) [[ $# -ge 2 ]] || __usage_err {m}; {o}=$2; shift ;;",
                    f = f,
                    m = sq(&format!("{} needs a value", f)),
                    o = o
                ));
            }
            self.line(format!("    {}=*) {}=${{1#*=}} ;;", f, o));
        }
        self.line("    --) shift; args+=(\"$@\"); break ;;");
        self.line("    -[0-9]*) args+=(\"$1\") ;;");
        self.line("    -?*) __usage_err \"unknown option: $1\" ;;");
        self.line("    *) args+=(\"$1\") ;;");
        self.line("  esac");
        self.line("  shift");
        self.line("done");
        for prm in &p.params {
            self.param_value(prm);
        }
    }

    fn param_value(&mut self, prm: &Param) {
        let o = format!("__o_{}", prm.name);
        let v = self.vname(&prm.name);
        let f = prm.flag.clone();
        let given = format!("[[ -n ${{{}+x}} ]]", o);
        // secret: also from the environment variable
        if let (Ty::Secret, Some(env)) = (&prm.ty, &prm.env_var) {
            self.line(format!("if ! {} && [[ -n ${{{}+x}} ]]; then {}=${}; fi", given, env, o, env));
        }
        let conv = self.sub(|g| {
        let v = v.clone();
        let f = f.clone();
        let o = o.clone();
        let raw = format!("\"${}\"", o);
        match &prm.ty {
            Ty::Int => {
                g.need("__int_parse");
                g.line(format!(
                    "__int_parse {raw} || __usage_err \"{f} must be an integer (got '${o}')\"",
                    raw = raw,
                    f = f,
                    o = o
                ));
                g.line(format!("{}=$__ret", v));
            }
            Ty::Duration => {
                g.need("__dur_parse");
                g.line(format!(
                    "__dur_parse {raw} || __usage_err \"{f} must be a duration like 30s, 1m30s or 500ms (got '${o}')\"",
                    raw = raw,
                    f = f,
                    o = o
                ));
                g.line(format!("{}=$__ret", v));
            }
            Ty::Bool => {
                g.line(format!(
                    "case ${o} in true|'') {v}=true ;; false) {v}=false ;; *) __usage_err \"{f} takes true or false (got '${o}')\" ;; esac",
                    o = o,
                    v = v,
                    f = f
                ));
            }
            Ty::Enum(e) => {
                let cases = g.enums.get(e).map(|d| d.cases.clone()).unwrap_or_default();
                let pats: Vec<String> = cases.iter().map(|c| sq(c)).collect();
                g.line(format!(
                    "case ${o} in {p}) {v}=${o} ;; *) __usage_err \"{f} must be one of: {l} (got '${o}')\" ;; esac",
                    o = o,
                    p = pats.join("|"),
                    v = v,
                    f = f,
                    l = cases.join(", ")
                ));
            }
            Ty::List(inner) => {
                g.need("__str_split");
                g.line(format!("{}=()", v));
                g.line(format!("if [[ -n ${} ]]; then __str_split {} ','; {}=(\"${{__ret_a[@]}}\"); fi", o, raw, v));
                if matches!(**inner, Ty::Int | Ty::Duration) {
                    g.need("__int_parse");
                    g.line(format!("for __i in \"${{!{}[@]}}\"; do", v));
                    g.line(format!(
                        "  __int_parse \"${{{v}[__i]}}\" || __usage_err \"{f} must be a list of integers (got '${o}')\"",
                        v = v,
                        f = f,
                        o = o
                    ));
                    g.line(format!("  {}[__i]=$__ret", v));
                    g.line("done");
                }
            }
            _ => g.line(format!("{}=${}", v, o)),
        }
        });
        let def = self.sub(|g| match &prm.default {
            Some(rv) => g.assign_rv(&Dest::Var(v.clone(), prm.ty.clone()), &prm.ty, rv),
            None if prm.ty == Ty::Bool => g.line(format!("{}=false", v)),
            None => {
                let m = match &prm.env_var {
                    Some(env) => format!("{} is required (or set {})", f, env),
                    None => format!("{} is required", f),
                };
                g.line(format!("__usage_err {}", sq(&m)));
            }
        });
        let one = |t: &str| t.trim().to_string();
        if !conv.trim().contains('\n') && !def.trim().contains('\n') {
            self.line(format!("if {}; then {}; else {}; fi", given, one(&conv), one(&def)));
        } else {
            self.line(format!("if {}; then", given));
            self.out.push_str(&reind(&conv));
            self.line("else");
            self.out.push_str(&reind(&def));
            self.line("fi");
        }
    }
}

fn reind(t: &str) -> String {
    t.lines().map(|l| format!("  {}\n", l)).collect()
}
