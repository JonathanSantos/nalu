//! The generator: state, frames (top level, functions, lambdas, tasks), script assembly and statements.

use super::quote::sq;
use super::runtime;
use super::val::{Cond, Dest, Loc, Val};
use crate::ir::*;
use crate::target::{Mode, Target};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub ty: Ty,
    pub mutable: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LoopKind {
    /// User loop (`for`, `while`).
    User,
    /// Fake loop of a `try`: failure = `break`.
    Try,
    /// `retry` loop: failure = `continue`.
    Retry,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FrameKind {
    Top,
    Func,
}

pub struct Frame {
    pub kind: FrameKind,
    /// Also looks up names in the frame below (lambdas and `parallel` tasks read the function's variables).
    pub inherit: bool,
    pub vars: HashMap<String, VarInfo>,
    pub rename: HashMap<String, String>,
    pub loops: Vec<LoopKind>,
    /// Temporaries created by the generator (declared `local` in functions).
    pub temps: Vec<(String, Ty)>,
    /// Other local scalar names (`__mi`…).
    pub extra: BTreeSet<String>,
    pub has_defer: bool,
    pub has_cd: bool,
    /// Variables that are never negative (`for` indexes): they skip the index guard.
    pub nonneg: HashSet<String>,
    /// List parameters received by reference (`local -n`, linux target): Nalu names.
    pub refs: HashSet<String>,
}

impl Frame {
    fn new(kind: FrameKind, inherit: bool) -> Frame {
        Frame {
            kind,
            inherit,
            vars: HashMap::new(),
            rename: HashMap::new(),
            loops: vec![],
            temps: vec![],
            extra: BTreeSet::new(),
            has_defer: false,
            has_cd: false,
            nonneg: HashSet::new(),
            refs: HashSet::new(),
        }
    }
}

pub struct Gen<'a> {
    pub p: &'a Program,
    pub mode: Mode,
    pub linux: bool,
    pub needs: BTreeSet<String>,
    pub out: String,
    pub ind: usize,
    pub frames: Vec<Frame>,
    pub counter: usize,
    /// Generated functions (lambdas, tasks, defers, tests), in order.
    pub aux: Vec<String>,
    pub structs: HashMap<String, &'a StructDef>,
    pub enums: HashMap<String, &'a EnumDef>,
    pub funcs: HashMap<String, &'a Func>,
    pub last_line: u32,
    pub uses_defer: bool,
    pub uses_temps: bool,
    pub uses_maps: bool,
    /// Generating the function parameter binding (outside the function frame).
    pub binding: bool,
    /// Linux target: for each user function, which parameters (lists of scalars) come by
    /// reference (`local -n`) instead of being copied (D25). Recursive functions receive a copy.
    pub byref: HashMap<String, Vec<bool>>,
    /// The `byref` line of the function that `gen_function` is generating.
    pub cur_byref: Option<Vec<bool>>,
}

/// D25: which parameters of each function come by reference on the linux target. A list of scalars is
/// passed by array name (`local -n`), without copying (6 ms per call with 3,000 items on Bash 5).
/// Functions that can call themselves (directly or indirectly) keep the copy: in a recursion, the
/// same local name would appear in two frames and the reference could point to the wrong one.
pub fn byref_table(p: &Program) -> HashMap<String, Vec<bool>> {
    let names: Vec<&str> = p.funcs.iter().map(|f| f.name.as_str()).collect();
    // who calls whom: the IR's Debug shows `User("name")` on every call, including in lambdas and tasks
    let edges: HashMap<&str, Vec<&str>> = p
        .funcs
        .iter()
        .map(|f| {
            let d = format!("{:?}", f.body);
            (f.name.as_str(), names.iter().copied().filter(|n| d.contains(&format!("User({:?})", n))).collect())
        })
        .collect();
    let recursive = |start: &str| -> bool {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = edges.get(start).cloned().unwrap_or_default();
        while let Some(n) = stack.pop() {
            if n == start {
                return true;
            }
            if seen.insert(n) {
                stack.extend(edges.get(n).cloned().unwrap_or_default());
            }
        }
        false
    };
    p.funcs
        .iter()
        .map(|f| {
            let rec = recursive(&f.name);
            let v = f.params.iter().map(|x| !rec && matches!(&x.ty, Ty::List(e) if e.is_scalar())).collect();
            (f.name.clone(), v)
        })
        .collect()
}

/// Shell name for the user function `name` (or `Type.method`).
pub fn fn_name(n: &str) -> String {
    format!("fn_{}", n.replace('.', "__"))
}

/// Nalu variable names that need a prefix in the shell (uppercase and `_` collide with Bash's own).
pub fn mangle(n: &str) -> String {
    if n == "_" || n.chars().any(|c| c.is_ascii_uppercase()) || n == "args" {
        format!("v_{}", n)
    } else {
        n.to_string()
    }
}

pub fn is_int(t: &Ty) -> bool {
    matches!(t, Ty::Int | Ty::Duration)
}

impl<'a> Gen<'a> {
    pub fn new(p: &'a Program, mode: Mode) -> Gen<'a> {
        let mut g = Gen {
            p,
            mode,
            linux: p.target != Target::Macos,
            needs: BTreeSet::new(),
            out: String::new(),
            ind: 0,
            frames: vec![Frame::new(FrameKind::Top, false)],
            counter: 0,
            aux: vec![],
            structs: p.structs.iter().map(|s| (s.name.clone(), s)).collect(),
            enums: p.enums.iter().map(|e| (e.name.clone(), e)).collect(),
            funcs: p.funcs.iter().map(|f| (f.name.clone(), f)).collect(),
            last_line: 0,
            uses_defer: false,
            uses_temps: false,
            uses_maps: false,
            binding: false,
            byref: if p.target == Target::Macos { HashMap::new() } else { byref_table(p) },
            cur_byref: None,
        };
        // top-level variables
        let mut vars = HashMap::new();
        for prm in &p.params {
            vars.insert(prm.name.clone(), VarInfo { ty: prm.ty.clone(), mutable: false });
        }
        let mut fl = ScanFlags::default();
        scan_block(&p.body, &mut vars, &mut fl);
        g.frames[0].vars = vars;
        g.frames[0].has_defer = fl.defer;
        g
    }

    // ------------------------------------------------------------------------------------------
    // Output
    // ------------------------------------------------------------------------------------------

    pub fn line(&mut self, s: impl AsRef<str>) {
        for l in s.as_ref().split('\n') {
            for _ in 0..self.ind {
                self.out.push_str("  ");
            }
            self.out.push_str(l);
            self.out.push('\n');
        }
    }

    /// Runs `f` writing to a separate buffer and returns the text.
    pub fn sub<F: FnOnce(&mut Self)>(&mut self, f: F) -> String {
        let saved = std::mem::take(&mut self.out);
        f(self);
        std::mem::replace(&mut self.out, saved)
    }

    pub fn need(&mut self, h: &str) {
        self.needs.insert(h.to_string());
    }

    pub fn frame(&self) -> &Frame {
        self.frames.last().unwrap()
    }

    pub fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().unwrap()
    }

    pub fn in_top(&self) -> bool {
        self.frames.len() == 1
    }

    /// A new temporary name (`__vN`), declared `local` if inside a function.
    pub fn tmp(&mut self, ty: &Ty) -> String {
        self.counter += 1;
        let n = format!("__v{}", self.counter);
        if !self.in_top() {
            self.frame_mut().temps.push((n.clone(), ty.clone()));
        }
        n
    }

    pub fn fresh(&mut self, prefix: &str) -> usize {
        self.counter += 1;
        let _ = prefix;
        self.counter
    }

    /// Auxiliary local scalar name (e.g. `__mi`), declared in functions.
    pub fn extra_local(&mut self, n: &str) {
        if !self.in_top() {
            self.frame_mut().extra.insert(n.to_string());
        }
    }

    // ------------------------------------------------------------------------------------------
    // Names and types
    // ------------------------------------------------------------------------------------------

    pub(crate) fn lookup(&self, n: &str) -> Option<(usize, &VarInfo)> {
        let mut i = self.frames.len() - 1;
        loop {
            if let Some(v) = self.frames[i].vars.get(n) {
                return Some((i, v));
            }
            if i == 0 {
                return None;
            }
            if self.frames[i].inherit {
                i -= 1;
            } else {
                i = 0;
            }
        }
    }

    /// The shell name of Nalu variable `n`.
    pub fn vname(&self, n: &str) -> String {
        match self.lookup(n) {
            Some((i, _)) => self.frames[i].rename.get(n).cloned().unwrap_or_else(|| mangle(n)),
            None => mangle(n),
        }
    }

    pub fn var_info(&self, n: &str) -> Option<VarInfo> {
        self.lookup(n).map(|(_, v)| v.clone())
    }

    pub fn var_ty(&self, n: &str) -> Ty {
        self.var_info(n).map(|v| v.ty).unwrap_or(Ty::Str)
    }

    pub fn is_mutable(&self, n: &str) -> bool {
        self.var_info(n).map(|v| v.mutable).unwrap_or(true)
    }

    pub fn struct_fields(&self, s: &str) -> Vec<Field> {
        self.structs.get(s).map(|d| d.fields.clone()).unwrap_or_default()
    }

    // ------------------------------------------------------------------------------------------
    // Storage by type
    // ------------------------------------------------------------------------------------------

    /// The shell variables that hold a value of type `ty` starting at `base`:
    /// (scalars, arrays, associative arrays).
    pub fn storage(&self, base: &str, ty: &Ty, sc: &mut Vec<String>, ar: &mut Vec<String>, assoc: &mut Vec<String>) {
        match ty {
            Ty::List(e) => match &**e {
                Ty::Struct(s) => {
                    for f in self.struct_fields(s) {
                        ar.push(format!("{}__{}", base, f.name));
                    }
                }
                _ => ar.push(base.to_string()),
            },
            Ty::Map(_, _) => {
                ar.push(format!("{}__k", base));
                ar.push(format!("{}__v", base));
                if self.linux {
                    assoc.push(format!("{}__i", base));
                } else {
                    sc.push(format!("{}__id", base));
                    ar.push(format!("{}__n", base));
                }
            }
            Ty::Struct(s) => {
                for f in self.struct_fields(s) {
                    self.storage(&format!("{}__{}", base, f.name), &f.ty, sc, ar, assoc);
                }
            }
            Ty::Error => {
                sc.push(format!("{}__message", base));
                sc.push(format!("{}__code", base));
            }
            Ty::Unit => {}
            _ => sc.push(base.to_string()),
        }
    }

    /// Map prefixes (with an index) stored starting at `base`.
    pub fn map_prefixes(&self, base: &str, ty: &Ty, out: &mut Vec<String>) {
        match ty {
            Ty::Map(_, _) => out.push(base.to_string()),
            Ty::Struct(s) => {
                for f in self.struct_fields(s) {
                    self.map_prefixes(&format!("{}__{}", base, f.name), &f.ty, out);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------
    // Script assembly
    // ------------------------------------------------------------------------------------------

    pub fn run(mut self) -> String {
        let p = self.p;
        // body and functions first (they discover which helpers are used)
        let mut funcs_txt = String::new();
        for f in &p.funcs {
            let params: Vec<(String, Ty)> = f.params.iter().map(|x| (x.name.clone(), x.ty.clone())).collect();
            self.cur_byref = self.byref.get(&f.name).cloned();
            let t = self.gen_function(&fn_name(&f.name), &params, &f.body, false, None);
            funcs_txt.push_str(&t);
            funcs_txt.push('\n');
        }
        let body = match self.mode {
            Mode::Script => self.sub(|g| g.top_block(&p.body)),
            Mode::Tests => self.sub(|g| g.tests_body()),
        };
        let params_txt = match self.mode {
            Mode::Script => self.sub(|g| g.gen_params()),
            Mode::Tests => self.sub(|g| g.test_params()),
        };
        let deps_txt = if self.mode == Mode::Script && !p.deps.is_empty() {
            self.need("__need");
            let ws: Vec<String> = p.deps.iter().map(|d| sq(d)).collect();
            format!("[[ $__dry == true ]] || __need {}\n", ws.join(" "))
        } else {
            String::new()
        };
        if self.uses_defer {
            self.need("__run_defers");
        }

        let mut o = String::new();
        o.push_str("#!/usr/bin/env bash\n");
        o.push_str(&format!(
            "# Generated by nalu {} from {} (target: {}). Do not edit.\n",
            env!("CARGO_PKG_VERSION"),
            p.source_file,
            p.target.os_name()
        ));
        // no `set -e`: each failure is checked at the point; `patsub_replacement` (Bash 5.2) turned off
        if self.linux {
            o.push_str("set -o pipefail; shopt -u patsub_replacement 2>/dev/null; IFS=$' \\t\\n'\n");
        } else {
            o.push_str("set -o pipefail; IFS=$' \\t\\n'\n");
        }
        // UTF-8 locale tested by effect (`${#'ç'}` = 1), not by name
        o.push_str("__p='ç'; [ \"${#__p}\" = 1 ] || for __l in C.UTF-8 C.utf8 en_US.UTF-8 en_US.utf8; do\n");
        o.push_str("  export LC_ALL=\"$__l\"; __p='ç'; [ \"${#__p}\" = 1 ] && break; unset LC_ALL\n");
        o.push_str("done\n");
        let mut init = vec!["__err_msg=''", "__err_code=0", "__dry=false", "__verbose=false"];
        if self.uses_defer {
            init.push("__defers=()");
        }
        if self.uses_temps {
            init.push("__temps=()");
        }
        if self.uses_maps && !self.linux {
            init.push("__mid=0");
        }
        o.push_str(&init.join("; "));
        o.push('\n');

        let rt = runtime::emit(&self.needs, self.linux);
        if !rt.is_empty() {
            o.push_str("\n# ---- runtime ----\n");
            o.push_str(&rt);
        }
        if !params_txt.is_empty() {
            o.push_str("\n# ---- parameters ----\n");
            o.push_str(&params_txt);
        }
        o.push_str(&deps_txt);
        if !funcs_txt.is_empty() || !self.aux.is_empty() {
            o.push_str("\n# ---- functions ----\n");
            o.push_str(&funcs_txt);
            for a in &self.aux {
                o.push_str(a);
                o.push('\n');
            }
        }
        if self.uses_defer && self.mode == Mode::Script {
            o.push_str("\ntrap '__run_defers 0' EXIT\n");
            o.push_str("trap 'exit 130' INT\ntrap 'exit 143' TERM\ntrap 'exit 141' PIPE\n");
        }
        o.push_str(if self.mode == Mode::Script { "\n# ---- script ----\n" } else { "\n# ---- tests ----\n" });
        o.push_str(&body);
        if self.mode == Mode::Script {
            o.push_str("exit 0\n");
        }
        o
    }

    fn top_block(&mut self, b: &Block) {
        self.stmts_lines(b);
    }

    /// Top-level or function statements, with `# linha N` when the line changes.
    pub fn stmts_lines(&mut self, b: &Block) {
        for s in &b.stmts {
            if s.line != 0 && s.line != self.last_line {
                self.line(format!("# line {}", s.line));
                self.last_line = s.line;
            }
            self.stmt(s);
        }
    }

    pub fn stmts(&mut self, b: &Block) {
        if b.stmts.is_empty() {
            self.line(":");
        }
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    // ------------------------------------------------------------------------------------------
    // Functions (user and generated)
    // ------------------------------------------------------------------------------------------

    /// Generates a shell function. `params`: Nalu names and types. `inherit`: reads the variables of the current
    /// frame (lambdas and tasks). `tail`: extra code at the end of the body (e.g. task result).
    pub fn gen_function(&mut self, name: &str, params: &[(String, Ty)], body: &Block, inherit: bool, tail: Option<&dyn Fn(&mut Gen<'a>)>) -> String {
        let mut fr = Frame::new(FrameKind::Func, inherit);
        let mut fl = ScanFlags::default();
        for (n, t) in params {
            fr.vars.insert(n.clone(), VarInfo { ty: t.clone(), mutable: false });
        }
        scan_block(body, &mut fr.vars, &mut fl);
        fr.has_defer = fl.defer;
        fr.has_cd = fl.cd;
        // local names that collide with top-level ones (Bash dynamic scope) get a prefix
        let top: HashSet<String> = self.frames[0].vars.keys().cloned().collect();
        let mut enclosing: HashSet<String> = HashSet::new();
        if inherit {
            for f in &self.frames[1..] {
                for (k, _) in &f.vars {
                    enclosing.insert(k.clone());
                }
            }
        }
        let keys: Vec<String> = fr.vars.keys().cloned().collect();
        for k in keys {
            // lowering temporaries (`__t1`…) are never read by another function: they need no prefix
            let lowering_tmp = k.starts_with("__t") && k[3..].chars().all(|c| c.is_ascii_digit()) && k.len() > 3;
            if (top.contains(&k) || enclosing.contains(&k)) && !(lowering_tmp && !inherit) {
                fr.rename.insert(k.clone(), format!("__l_{}", mangle(&k)));
            }
        }
        // D25 (linux): this function's lists get a program-unique name (`__aN_x`), and by-reference
        // parameters become `local -n __rN_x`. That way a called function never hides (dynamic scope)
        // the caller's list, and a reference can be passed on with `${!__rN_x}`.
        let byref = self.cur_byref.take().unwrap_or_default();
        let mut ref_shell: HashSet<String> = HashSet::new();
        if self.linux {
            let k = self.fresh("f");
            for (i, (n, _)) in params.iter().enumerate() {
                if byref.get(i).copied().unwrap_or(false) {
                    let sn = format!("__r{}_{}", k, mangle(n));
                    fr.rename.insert(n.clone(), sn.clone());
                    fr.refs.insert(n.clone());
                    ref_shell.insert(sn);
                }
            }
            let lists: Vec<String> = fr.vars.iter().filter(|(n, v)| !fr.refs.contains(*n) && matches!(v.ty, Ty::List(_))).map(|(n, _)| n.clone()).collect();
            for n in lists {
                fr.rename.insert(n.clone(), format!("__a{}_{}", k, mangle(&n)));
            }
        }
        let saved_ind = self.ind;
        let saved_line = self.last_line;
        self.frames.push(fr);
        self.ind = 1;
        self.last_line = 0;
        let body_txt = self.sub(|g| {
            g.stmts_lines(body);
            if let Some(t) = tail {
                t(g);
            }
            let ends = body.stmts.last().map_or(false, |s| matches!(s.kind, StmtKind::Return(_) | StmtKind::Exit(_) | StmtKind::Fail { .. }));
            if !ends || tail.is_some() {
                g.line("return 0");
            }
        });
        let fr = self.frames.pop().unwrap();
        self.ind = saved_ind;
        self.last_line = saved_line;

        // local declarations
        let mut sc = Vec::new();
        let mut ar = Vec::new();
        let mut assoc = Vec::new();
        let pnames: HashSet<&String> = params.iter().map(|(n, _)| n).collect();
        let mut vars: Vec<(&String, &VarInfo)> = fr.vars.iter().filter(|(k, _)| !pnames.contains(k)).collect();
        vars.sort_by(|a, b| a.0.cmp(b.0));
        let rn = |k: &String| fr.rename.get(k).cloned().unwrap_or_else(|| mangle(k));
        for (k, v) in &vars {
            self.storage(&rn(k), &v.ty, &mut sc, &mut ar, &mut assoc);
        }
        for (t, ty) in &fr.temps {
            self.storage(t, ty, &mut sc, &mut ar, &mut assoc);
        }
        for e in &fr.extra {
            sc.push(e.clone());
        }
        let mut maps = Vec::new();
        if !self.linux {
            for (k, v) in fr.vars.iter() {
                self.map_prefixes(&rn(k), &v.ty, &mut maps);
            }
            for (t, ty) in &fr.temps {
                self.map_prefixes(t, ty, &mut maps);
            }
            maps.sort();
        }
        let wrap = fr.has_defer || fr.has_cd || !maps.is_empty();

        // parameters
        let mut pp: Vec<(String, Ty)> = Vec::new();
        for (n, t) in params {
            pp.push((rn(n), t.clone()));
        }
        let mut pdecl_sc = Vec::new();
        let mut pdecl_ar = Vec::new();
        let mut pdecl_as = Vec::new();
        for (n, t) in &pp {
            if !ref_shell.contains(n) {
                self.storage(n, t, &mut pdecl_sc, &mut pdecl_ar, &mut pdecl_as);
            }
        }
        self.binding = true;
        let bind = self.bind_params(&pp, !wrap, &ref_shell);
        self.binding = false;

        let mut decl = String::new();
        let all_scalar = pp.iter().all(|(_, t)| t.is_scalar());
        if wrap {
            let mut s2 = pdecl_sc.clone();
            s2.extend(sc.iter().cloned());
            let mut a2 = pdecl_ar.clone();
            a2.extend(ar.iter().cloned());
            let mut as2 = pdecl_as.clone();
            as2.extend(assoc.iter().cloned());
            if fr.has_defer {
                s2.push("__dm=${#__defers[@]}".into());
            }
            if fr.has_cd {
                s2.push("__pwd=$PWD".into());
            }
            push_decl(&mut decl, "local", &s2);
            push_decl(&mut decl, "local -a", &a2);
            push_decl(&mut decl, "local -A", &as2);
        } else {
            if !all_scalar {
                // composite parameters: `bind` uses `local` line by line
            }
            push_decl(&mut decl, "local", &sc);
            push_decl(&mut decl, "local -a", &ar);
            push_decl(&mut decl, "local -A", &assoc);
        }

        // With a large list in `$@`, Bash 3.2 gets slow on every variable operation
        // (3 s versus 0.2 s with 4,000 items): the parameters are copied into the variables and `$@` is
        // emptied before the body runs.
        let has_list = pp.iter().any(|(_, t)| !t.is_scalar());
        let mut o = String::new();
        if wrap {
            let bname = format!("{}__corpo", name);
            o.push_str(&format!("{}() {{\n", name));
            o.push_str(&decl);
            o.push_str(&bind);
            o.push_str(&format!("  {}\n", bname));
            o.push_str("  local __rc=$?\n");
            if fr.has_defer {
                o.push_str("  __run_defers \"$__dm\"\n");
                self.need("__run_defers");
                self.uses_defer = true;
            }
            for m in &maps {
                o.push_str(&format!("  unset \"${{{}__n[@]}}\"\n", m));
            }
            if fr.has_cd {
                o.push_str("  cd -- \"$__pwd\" 2>/dev/null\n");
            }
            o.push_str("  return \"$__rc\"\n}\n");
            o.push_str(&format!("{}() {{\n", bname));
            o.push_str(&body_txt);
            o.push_str("}\n");
        } else {
            o.push_str(&format!("{}() {{\n", name));
            o.push_str(&bind);
            if has_list {
                o.push_str("  set --\n");
            }
            o.push_str(&decl);
            o.push_str(&body_txt);
            o.push_str("}\n");
        }
        o
    }

    /// Code that binds `$1…` to the parameters (already with shell names). `local_kw`: declare with `local`.
    fn bind_params(&mut self, pp: &[(String, Ty)], local_kw: bool, refs: &HashSet<String>) -> String {
        let mut o = String::new();
        if pp.is_empty() {
            return o;
        }
        let kw = if local_kw { "local " } else { "" };
        let kwa = if local_kw { "local -a " } else { "" };
        if pp.iter().all(|(_, t)| t.is_scalar()) {
            let parts: Vec<String> = pp.iter().enumerate().map(|(i, (n, _))| format!("{}={}", n, pos(i + 1))).collect();
            o.push_str(&format!("  {}{}\n", kw, parts.join(" ")));
            return o;
        }
        let mut leaves: Vec<(String, Ty)> = Vec::new();
        for (n, t) in pp {
            self.leaves(n, t, &mut leaves);
        }
        if leaves.iter().all(|(_, t)| t.is_scalar()) {
            let parts: Vec<String> = leaves.iter().enumerate().map(|(i, (n, _))| format!("{}={}", n, pos(i + 1))).collect();
            o.push_str(&format!("  {}{}\n", kw, parts.join(" ")));
            return o;
        }
        let n = leaves.len();
        for (i, (name, t)) in leaves.into_iter().enumerate() {
            let last = i + 1 == n;
            match &t {
                // by reference (D25): a single argument, the name of the caller's array
                Ty::List(_) if refs.contains(&name) => {
                    o.push_str(&format!("  local -n {}=$1", name));
                    if !last {
                        o.push_str("; shift");
                    }
                    o.push('\n');
                }
                Ty::List(_) => {
                    o.push_str(&format!("  {}{}=(\"${{@:2:$1}}\")", kwa, name));
                    if !last {
                        o.push_str("; shift $(( $1 + 1 ))");
                    }
                    o.push('\n');
                }
                Ty::Map(_, _) => {
                    o.push_str(&format!("  {}{}__k=(\"${{@:2:$1}}\")\n", kwa, name));
                    o.push_str(&format!("  {}{}__v=(\"${{@:$1+2:$1}}\")", kwa, name));
                    if !last {
                        o.push_str("; shift $(( 2 * $1 + 1 ))");
                    }
                    o.push('\n');
                    let idx = self.sub(|g| {
                        g.ind = 1;
                        g.map_reindex(&name, true);
                    });
                    if local_kw {
                        if self.linux {
                            o.push_str(&format!("  local -A {}__i\n", name));
                        } else {
                            o.push_str(&format!("  local {}__id\n  local -a {}__n\n", name, name));
                        }
                    }
                    o.push_str(&idx);
                }
                Ty::Struct(_) => unreachable!(),
                _ => {
                    o.push_str(&format!("  {}{}=$1", kw, name));
                    if !last {
                        o.push_str("; shift");
                    }
                    o.push('\n');
                }
            }
        }
        o
    }

    /// Leaves of a value for argument passing: scalars, lists and maps, in order.
    /// Lists of structs become one block per field (each field is a list).
    pub fn leaves(&self, base: &str, ty: &Ty, out: &mut Vec<(String, Ty)>) {
        match ty {
            Ty::Struct(s) => {
                for f in self.struct_fields(s) {
                    self.leaves(&format!("{}__{}", base, f.name), &f.ty, out);
                }
            }
            Ty::List(e) => match &**e {
                Ty::Struct(s) => {
                    for f in self.struct_fields(s) {
                        out.push((format!("{}__{}", base, f.name), Ty::List(Box::new(Ty::Str))));
                    }
                }
                _ => out.push((base.to_string(), ty.clone())),
            },
            Ty::Error => {
                out.push((format!("{}__message", base), Ty::Str));
                out.push((format!("{}__code", base), Ty::Int));
            }
            _ => out.push((base.to_string(), ty.clone())),
        }
    }

    // ------------------------------------------------------------------------------------------
    // Failures and loops
    // ------------------------------------------------------------------------------------------

    /// What to do after an operation failed (with `__err_msg`/`__err_code` already set).
    pub fn fail_action(&mut self) -> String {
        let f = self.frame();
        for (i, l) in f.loops.iter().rev().enumerate() {
            match l {
                LoopKind::User => {}
                LoopKind::Try => return if i == 0 { "break".into() } else { format!("break {}", i + 1) },
                LoopKind::Retry => return if i == 0 { "continue".into() } else { format!("continue {}", i + 1) },
            }
        }
        match f.kind {
            FrameKind::Top => {
                self.need("__fail");
                "__fail".into()
            }
            FrameKind::Func => "return 1".into(),
        }
    }

    fn loop_jump(&self, kw: &str) -> String {
        let f = self.frame();
        for (i, l) in f.loops.iter().rev().enumerate() {
            if *l == LoopKind::User {
                return if i == 0 { kw.to_string() } else { format!("{} {}", kw, i + 1) };
            }
        }
        kw.to_string()
    }

    /// Emits `op` (a command that returns a status and sets the error) with the `on_fail` handling.
    /// `ok` generates the success path; `dest` receives the value of `Default`/`Block`.
    pub fn with_op(&mut self, op: &str, on_fail: &OnFail, dest: &Dest, ty: &Ty, ok: &mut dyn FnMut(&mut Gen<'a>)) {
        match on_fail {
            OnFail::Propagate => {
                let fa = self.fail_action();
                self.line(format!("{} || {}", op, fa));
                ok(self);
            }
            OnFail::Ignore => {
                self.line(format!("{} || :", op));
                ok(self);
            }
            _ => {
                let succ = self.sub(|g| {
                    g.ind += 1;
                    ok(g);
                    g.ind -= 1;
                });
                if succ.is_empty() {
                    self.line(format!("if ! {}; then", op));
                    self.ind += 1;
                    self.handler(on_fail, dest, ty);
                    self.ind -= 1;
                    self.line("fi");
                } else {
                    self.line(format!("if {}; then", op));
                    self.out.push_str(&succ);
                    let h = self.sub(|g| {
                        g.ind += 1;
                        g.handler(on_fail, dest, ty);
                        g.ind -= 1;
                    });
                    let pad = "  ".repeat(self.ind + 1);
                    match as_elif(&h, &pad) {
                        // `a or b or c`: the handler's `if` becomes `elif`
                        Some(e) => self.out.push_str(&e),
                        None => {
                            self.line("else");
                            self.out.push_str(&h);
                            self.line("fi");
                        }
                    }
                }
            }
        }
    }

    /// The failure path (inside an `if`/`else`): `__err_*` already set.
    pub fn handler(&mut self, on_fail: &OnFail, dest: &Dest, ty: &Ty) {
        match on_fail {
            OnFail::Propagate => {
                let fa = self.fail_action();
                self.line(fa);
            }
            OnFail::Ignore => self.line(":"),
            OnFail::Default(rv) => {
                if matches!(dest, Dest::Discard) {
                    let t = self.sub(|g| g.assign_rv(dest, ty, rv));
                    if t.trim().is_empty() {
                        self.line(":");
                    } else {
                        self.out.push_str(&t);
                    }
                } else {
                    self.assign_rv(dest, ty, rv);
                }
            }
            OnFail::Fail { message, code } => self.set_err(message, code.as_ref()),
            OnFail::Block { err, body, value } => {
                if let Some(e) = err {
                    let n = self.vname(e);
                    self.line(format!("{}__message=$__err_msg; {}__code=$__err_code", n, n));
                }
                self.stmts(body);
                if let Some(v) = value {
                    if !matches!(dest, Dest::Discard) {
                        self.assign_rv(dest, ty, &Rvalue::Expr(v.clone()));
                    }
                }
            }
        }
    }

    /// `fail` with message and code; then the context's failure action.
    fn set_err(&mut self, message: &Expr, code: Option<&Expr>) {
        let m = self.word(message).render();
        match code {
            Some(c) => {
                let cw = self.arith_word(c);
                let t = self.tmp(&Ty::Str);
                self.line(format!("{}={}; __err_code={}; __err_msg=${}", t, m, cw, t));
            }
            None => self.line(format!("__err_msg={}; __err_code=1", m)),
        }
        let fa = self.fail_action();
        self.line(fa);
    }

    // ------------------------------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------------------------------

    pub fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Let { var, ty, value, .. } => {
                let n = self.vname(var);
                if let Ty::Map(_, _) = ty {
                    self.uses_maps = true;
                }
                self.assign_rv(&Dest::Var(n, ty.clone()), ty, value);
            }
            StmtKind::Assign { target, value } => self.assign_lvalue(target, value),
            StmtKind::Call { call, on_fail } => self.call_stmt(call, on_fail),
            StmtKind::Command { cmd, on_fail } => self.command_stmt(cmd, on_fail),
            StmtKind::If { cond, then, els } => self.gen_if(cond, then, els.as_ref(), false),
            StmtKind::While { cond, body } => {
                let (pre, c) = self.cond_pre(cond);
                if pre.is_empty() {
                    self.line(format!("while {}; do", c.render()));
                    self.loop_body(body, None);
                } else {
                    self.line("while :; do");
                    self.ind += 1;
                    self.out.push_str(&reindent(&pre, 1));
                    self.line(format!("if {}; then break; fi", Cond::not(c).simplify().render()));
                    self.ind -= 1;
                    self.loop_body(body, None);
                }
                self.line("done");
            }
            StmtKind::ForRange { var, from, to, inclusive, body } => self.gen_for_range(var, from, to, *inclusive, body),
            StmtKind::ForList { var, index, list, body } => self.gen_for_list(var, index.as_deref(), list, body),
            StmtKind::ForMap { key, value, map, body } => self.gen_for_map(key, value, map, body),
            StmtKind::Match { subject, arms, default } => self.gen_match(subject, arms, default.as_ref()),
            StmtKind::Try { body, on_fail } => {
                let flag = self.tmp(&Ty::Int);
                self.line(format!("{}=1", flag));
                self.line("while :; do");
                self.ind += 1;
                self.frame_mut().loops.push(LoopKind::Try);
                self.stmts(body);
                self.frame_mut().loops.pop();
                self.line(format!("{}=0; break", flag));
                self.ind -= 1;
                self.line("done");
                self.after_flag(&flag, on_fail);
            }
            StmtKind::Retry { times, every, body, on_fail } => self.gen_retry(times, every, body, on_fail),
            StmtKind::Parallel { max, body, on_fail } => self.gen_parallel(max.as_ref(), body, on_fail),
            StmtKind::Fail { message, code } => self.set_err(message, code.as_ref()),
            StmtKind::Return(e) => self.gen_return(e.as_ref()),
            StmtKind::Break => {
                let j = self.loop_jump("break");
                self.line(j);
            }
            StmtKind::Continue => {
                let j = self.loop_jump("continue");
                self.line(j);
            }
            StmtKind::Exit(code) => {
                let c = match code {
                    Some(c) => self.arith_word(c),
                    None => "0".into(),
                };
                self.line(format!("exit {}", c));
            }
            StmtKind::Defer(inner) => self.gen_defer(inner),
            StmtKind::Native { bash, .. } => {
                for l in bash.lines() {
                    self.out.push_str(l);
                    self.out.push('\n');
                }
            }
            StmtKind::Assert { cond, text } => {
                let (pre, c) = self.cond_pre(cond);
                self.out.push_str(&pre);
                self.need("__err");
                let fa = self.fail_action();
                self.line(format!("if {}; then", Cond::not(c).simplify().render()));
                self.line(format!("  __err {}; {}", sq(&format!("assert failed: {}", text)), fa));
                self.line("fi");
            }
            StmtKind::AssertFails(inner) => {
                let flag = self.tmp(&Ty::Int);
                self.line(format!("{}=1", flag));
                self.line("while :; do");
                self.ind += 1;
                self.frame_mut().loops.push(LoopKind::Try);
                self.stmt(inner);
                self.frame_mut().loops.pop();
                self.line(format!("{}=0; break", flag));
                self.ind -= 1;
                self.line("done");
                self.need("__err");
                let fa = self.fail_action();
                self.line(format!("if (( {} == 0 )); then __err 'assert_fails: the call did not fail'; {}; fi", flag, fa));
            }
        }
    }

    /// After a fake loop (`try`): `flag` = 1 if it failed.
    fn after_flag(&mut self, flag: &str, on_fail: &OnFail) {
        match on_fail {
            OnFail::Ignore | OnFail::Default(_) => {}
            _ => {
                self.line(format!("if (( {} )); then", flag));
                self.ind += 1;
                self.handler(on_fail, &Dest::Discard, &Ty::Unit);
                self.ind -= 1;
                self.line("fi");
            }
        }
    }

    pub fn loop_body(&mut self, body: &Block, prefix: Option<String>) {
        self.ind += 1;
        if let Some(p) = prefix {
            self.line(p);
        }
        self.frame_mut().loops.push(LoopKind::User);
        self.stmts(body);
        self.frame_mut().loops.pop();
        self.ind -= 1;
    }

    /// Compiles a condition; returns (preparatory code, condition).
    pub fn cond_pre(&mut self, e: &Expr) -> (String, Cond) {
        let ind = self.ind;
        let mut c = Cond::True;
        let pre = self.sub(|g| {
            c = g.cond(e).simplify();
        });
        self.ind = ind;
        (pre, c)
    }

    fn gen_if(&mut self, cond: &Expr, then: &Block, els: Option<&Block>, _is_elif: bool) {
        let (pre, c) = self.cond_pre(cond);
        self.out.push_str(&pre);
        self.line(format!("if {}; then", c.render()));
        self.ind += 1;
        self.stmts(then);
        self.ind -= 1;
        let mut cur = els;
        while let Some(e) = cur {
            if e.stmts.len() == 1 {
                if let StmtKind::If { cond: c2, then: t2, els: e2 } = &e.stmts[0].kind {
                    let (pre2, cc2) = self.cond_pre(c2);
                    if pre2.is_empty() {
                        self.line(format!("elif {}; then", cc2.render()));
                        self.ind += 1;
                        self.stmts(t2);
                        self.ind -= 1;
                        cur = e2.as_ref();
                        continue;
                    }
                }
            }
            self.line("else");
            self.ind += 1;
            self.stmts(e);
            self.ind -= 1;
            break;
        }
        self.line("fi");
    }

    fn gen_for_range(&mut self, var: &str, from: &Expr, to: &Expr, inclusive: bool, body: &Block) {
        let v = self.vname(var);
        if matches!(from.kind, ExprKind::Int(n) if n >= 0) {
            self.frame_mut().nonneg.insert(v.clone());
        }
        let a = self.arith(from);
        let mut b = self.arith(to);
        if !self.stable_expr(to) {
            let t = self.tmp(&Ty::Int);
            self.line(format!("{}=$(( {} ))", t, b));
            b = t;
        }
        let op = if inclusive { "<=" } else { "<" };
        self.line(format!("for (( {v} = {a}; {v} {op} {b}; {v}++ )); do", v = v, a = a, op = op, b = b));
        self.loop_body(body, None);
        self.line("done");
    }

    /// The expression does not change during a loop (only literals and `let` variables).
    pub fn stable_expr(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Int(_) | ExprKind::Duration(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::EnumCase { .. } => true,
            ExprKind::Var(n) => !self.is_mutable(n),
            ExprKind::Field(b, _) => self.stable_expr(b),
            ExprKind::Len(x) | ExprKind::Unary(_, x) => self.stable_expr(x),
            ExprKind::Binary(_, a, b) => self.stable_expr(a) && self.stable_expr(b),
            _ => false,
        }
    }

    fn gen_for_list(&mut self, var: &str, index: Option<&str>, list: &Expr, body: &Block) {
        let elem = match &list.ty {
            Ty::List(e) => (**e).clone(),
            _ => Ty::Str,
        };
        let v = self.vname(var);
        let loc = self.loc(list);
        let base = match loc {
            Loc::Base(b) => b,
            Loc::Elem { .. } => unreachable!("lista dentro de lista"),
        };
        let iv = index.map(|i| self.vname(i));
        if let Some(i) = &iv {
            self.frame_mut().nonneg.insert(i.clone());
        }
        match &elem {
            Ty::Struct(s) => {
                let fields = self.struct_fields(s);
                // copy of the list if it can change in the body
                let src = if self.list_expr_mutable(list) {
                    let t = self.tmp(&list.ty);
                    for f in &fields {
                        self.line(format!("{t}__{f}=(\"${{{b}__{f}[@]}}\")", t = t, f = f.name, b = base));
                    }
                    t
                } else {
                    base.clone()
                };
                let i = match &iv {
                    Some(i) => i.clone(),
                    None => self.tmp(&Ty::Int),
                };
                let first = fields.first().map(|f| f.name.clone()).unwrap_or_default();
                self.line(format!("for (( {i} = 0; {i} < ${{#{s}__{f}[@]}}; {i}++ )); do", i = i, s = src, f = first));
                self.ind += 1;
                let sname = s.clone();
                self.store(&Dest::Var(v.clone(), elem.clone()), &Val::Struct(Loc::Elem { list: src.clone(), idx: i.clone() }, sname));
                self.ind -= 1;
                self.loop_body(body, None);
                self.line("done");
            }
            _ => {
                match &iv {
                    Some(i) => {
                        self.line(format!("{}=-1", i));
                        self.line(format!("for {} in \"${{{}[@]}}\"; do", v, base));
                        self.loop_body(body, Some(format!("{i}=$(( {i} + 1 ))", i = i)));
                    }
                    None => {
                        self.line(format!("for {} in \"${{{}[@]}}\"; do", v, base));
                        self.loop_body(body, None);
                    }
                }
                self.line("done");
            }
        }
    }

    /// The list can change during the loop (it is a `var`, or a field of a `var`).
    fn list_expr_mutable(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Var(n) => self.is_mutable(n),
            ExprKind::Field(b, _) | ExprKind::Index(b, _) => self.list_expr_mutable(b),
            _ => false,
        }
    }

    fn gen_for_map(&mut self, key: &str, value: &str, map: &Expr, body: &Block) {
        let k = self.vname(key);
        let v = self.vname(value);
        let base = match self.loc(map) {
            Loc::Base(b) => b,
            Loc::Elem { .. } => unreachable!(),
        };
        let src = if self.list_expr_mutable(map) {
            let t = self.tmp(&Ty::List(Box::new(Ty::Str)));
            let t2 = self.tmp(&Ty::List(Box::new(Ty::Str)));
            self.line(format!("{}=(\"${{{}__k[@]}}\"); {}=(\"${{{}__v[@]}}\")", t, base, t2, base));
            (t, t2)
        } else {
            (format!("{}__k", base), format!("{}__v", base))
        };
        let i = self.tmp(&Ty::Int);
        self.line(format!("for (( {i} = 0; {i} < ${{#{k}[@]}}; {i}++ )); do", i = i, k = src.0));
        self.loop_body(body, Some(format!("{k}=${{{sk}[{i}]}}; {v}=${{{sv}[{i}]}}", k = k, v = v, sk = src.0, sv = src.1, i = i)));
        self.line("done");
    }

    fn gen_match(&mut self, subject: &Expr, arms: &[Arm], default: Option<&Block>) {
        let has_range = arms.iter().any(|a| a.patterns.iter().any(|p| matches!(p, Pattern::Range { .. })));
        if !has_range {
            let w = self.word(subject);
            let subj = match w.as_simple_var() {
                Some(v) => format!("${}", v),
                None => w.render(),
            };
            self.line(format!("case {} in", subj));
            self.ind += 1;
            for a in arms {
                let pats: Vec<String> = a
                    .patterns
                    .iter()
                    .map(|p| match p {
                        Pattern::Lit(e) => {
                            let w = self.word(e);
                            w.render()
                        }
                        Pattern::Range { .. } => unreachable!(),
                    })
                    .collect();
                self.line(format!("{})", pats.join("|")));
                self.ind += 1;
                self.stmts(&a.body);
                self.line(";;");
                self.ind -= 1;
            }
            if let Some(d) = default {
                self.line("*)");
                self.ind += 1;
                self.stmts(d);
                self.line(";;");
                self.ind -= 1;
            }
            self.ind -= 1;
            self.line("esac");
            return;
        }
        // with ranges: if/elif over integers
        let mut s = self.arith(subject);
        if !matches!(subject.kind, ExprKind::Var(_)) {
            let t = self.tmp(&Ty::Int);
            self.line(format!("{}=$(( {} ))", t, s));
            s = t;
        }
        for (i, a) in arms.iter().enumerate() {
            let mut cs = Vec::new();
            for p in &a.patterns {
                match p {
                    Pattern::Lit(e) => {
                        let v = self.arith(e);
                        cs.push(Cond::Arith(format!("{} == {}", s, v)));
                    }
                    Pattern::Range { from, to, inclusive } => {
                        let f = self.arith(from);
                        let t = self.arith(to);
                        let op = if *inclusive { "<=" } else { "<" };
                        cs.push(Cond::Arith(format!("{s} >= {f} && {s} {op} {t}", s = s, f = f, op = op, t = t)));
                    }
                }
            }
            let c = Cond::Or(cs).simplify();
            self.line(format!("{} {}; then", if i == 0 { "if" } else { "elif" }, c.render()));
            self.ind += 1;
            self.stmts(&a.body);
            self.ind -= 1;
        }
        if let Some(d) = default {
            self.line("else");
            self.ind += 1;
            self.stmts(d);
            self.ind -= 1;
        }
        self.line("fi");
    }

    fn gen_retry(&mut self, times: &Expr, every: &Expr, body: &Block, on_fail: &OnFail) {
        self.need("__sleep_ms");
        let n = self.tmp(&Ty::Int);
        let t = match times.kind {
            ExprKind::Int(_) => self.arith(times),
            _ => {
                let tt = self.tmp(&Ty::Int);
                let a = self.arith(times);
                self.line(format!("{}=$(( {} ))", tt, a));
                tt
            }
        };
        let e = match every.kind {
            ExprKind::Duration(_) | ExprKind::Int(_) => self.arith(every),
            _ => {
                let tt = self.tmp(&Ty::Int);
                let a = self.arith(every);
                self.line(format!("{}=$(( {} ))", tt, a));
                format!("\"${}\"", tt)
            }
        };
        self.line(format!("{}=0", n));
        self.line("while :; do");
        self.ind += 1;
        self.line(format!("if (( {} > 0 )); then", n));
        self.line(format!("  if (( {} >= {} )); then {}=-1; break; fi", n, t, n));
        self.line(format!("  __sleep_ms {}", e));
        self.line("fi");
        self.line(format!("{n}=$(( {n} + 1 ))", n = n));
        self.frame_mut().loops.push(LoopKind::Retry);
        self.stmts(body);
        self.frame_mut().loops.pop();
        self.line("break");
        self.ind -= 1;
        self.line("done");
        match on_fail {
            OnFail::Ignore | OnFail::Default(_) => {}
            _ => {
                self.line(format!("if (( {} < 0 )); then", n));
                self.ind += 1;
                self.handler(on_fail, &Dest::Discard, &Ty::Unit);
                self.ind -= 1;
                self.line("fi");
            }
        }
    }

    fn gen_return(&mut self, e: Option<&Expr>) {
        let Some(e) = e else {
            self.line("return 0");
            return;
        };
        let ty = e.ty.clone();
        let dest = match &ty {
            Ty::List(inner) if matches!(**inner, Ty::Struct(_)) => Dest::Var("__ret_a".into(), ty.clone()),
            Ty::List(_) => Dest::Var("__ret_a".into(), ty.clone()),
            Ty::Map(_, _) => Dest::Var("__ret_m".into(), ty.clone()),
            _ => Dest::Var("__ret".into(), ty.clone()),
        };
        self.assign_rv_ret(&dest, &ty, e);
        self.line("return 0");
    }

    fn gen_defer(&mut self, inner: &Stmt) {
        self.uses_defer = true;
        self.need("__run_defers");
        let mut params: Vec<(String, Ty)> = Vec::new();
        let mut vals: Vec<Expr> = Vec::new();
        let rewritten = defer_rewrite(inner, &mut params, &mut vals);
        let k = self.fresh("defer");
        let fname = format!("__defer_{}", k);
        // the defer body: the statement with the arguments already evaluated; if it fails, the function fails and
        // `__run_defers` shows the warning with `--verbose` (as in PowerShell)
        let body = Block { stmts: vec![rewritten] };
        let txt = self.gen_function(&fname, &params, &body, false, None);
        self.aux.push(txt);
        // push: function + arguments + count
        let mut words = vec![fname];
        for v in &vals {
            let w = self.flatten_arg(v);
            words.extend(w);
        }
        let t = self.tmp(&Ty::List(Box::new(Ty::Str)));
        self.line(format!("{}=({})", t, words.join(" ")));
        self.line(format!("__defers+=(\"${{{t}[@]}}\" \"${{#{t}[@]}}\")", t = t));
    }

    // ------------------------------------------------------------------------------------------
    // Tests (Mode::Tests)
    // ------------------------------------------------------------------------------------------

    fn test_params(&mut self) {
        let p = self.p;
        for prm in &p.params {
            let n = self.vname(&prm.name);
            match &prm.default {
                Some(rv) => self.assign_rv(&Dest::Var(n, prm.ty.clone()), &prm.ty, rv),
                None => match &prm.ty {
                    Ty::List(_) => self.line(format!("{}=()", n)),
                    Ty::Bool => self.line(format!("{}=false", n)),
                    Ty::Int | Ty::Duration => self.line(format!("{}=0", n)),
                    _ => self.line(format!("{}=''", n)),
                },
            }
        }
        self.line("args=()");
    }

    fn tests_body(&mut self) {
        let p = self.p;
        for s in &p.body.stmts {
            if let StmtKind::Let { value, .. } = &s.kind {
                if matches!(value, Rvalue::Expr(_) | Rvalue::List { .. } | Rvalue::Map { .. } | Rvalue::Struct { .. }) {
                    self.stmt(s);
                }
            }
        }
        let mut runs = Vec::new();
        for t in &p.tests {
            let k = self.fresh("test");
            let fname = format!("__test_{}", k);
            let txt = self.gen_function(&fname, &[], &t.body, false, None);
            self.aux.push(txt);
            runs.push((t.name.clone(), fname));
        }
        self.need("__test_run");
        self.line("__tests_falhas=0");
        for (name, f) in &runs {
            self.line(format!("__test_run {} {} || __tests_falhas=$(( __tests_falhas + 1 ))", sq(name), f));
        }
        self.line(format!("printf '%d test(s), %d failure(s)\\n' {} \"$__tests_falhas\"", runs.len()));
        self.line("(( __tests_falhas == 0 )) || exit 1");
        self.line("exit 0");
    }
}

/// If `h` (indented with `pad`) is a single `if … fi`, returns it as `elif …` (one level up),
/// including the final `fi`.
fn as_elif(h: &str, pad: &str) -> Option<String> {
    let lines: Vec<&str> = h.lines().collect();
    let first = *lines.first()?;
    if !first.starts_with(&format!("{}if ", pad)) || lines.last()? != &format!("{}fi", pad) {
        return None;
    }
    let mut ifs = 0;
    for l in &lines {
        if let Some(rest) = l.strip_prefix(pad) {
            if !rest.starts_with(' ') {
                if rest.starts_with("if ") {
                    ifs += 1;
                } else if !(rest.starts_with("elif ") || rest == "else" || rest == "fi") {
                    return None;
                }
            }
        } else {
            return None;
        }
    }
    if ifs != 1 {
        return None;
    }
    let outer = &pad[2..];
    let mut o = String::new();
    for (i, l) in lines.iter().enumerate() {
        let body = &l[2..];
        if i == 0 {
            o.push_str(&format!("{}el{}\n", outer, &l[pad.len()..]));
        } else {
            o.push_str(body);
            o.push('\n');
        }
    }
    Some(o)
}

fn pos(i: usize) -> String {
    if i < 10 {
        format!("${}", i)
    } else {
        format!("${{{}}}", i)
    }
}

fn push_decl(o: &mut String, kw: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    let v: Vec<&String> = names.iter().filter(|n| seen.insert((*n).clone())).collect();
    for chunk in v.chunks(8) {
        let c: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
        o.push_str(&format!("  {} {}\n", kw, c.join(" ")));
    }
}

pub fn reindent(s: &str, _n: usize) -> String {
    s.to_string()
}

// ----------------------------------------------------------------------------------------------
// Declaration scan
// ----------------------------------------------------------------------------------------------

#[derive(Default)]
pub struct ScanFlags {
    pub defer: bool,
    pub cd: bool,
}

/// Collects the variables declared in a block (without entering lambdas or the body of `parallel`).
pub fn scan_block(b: &Block, vars: &mut HashMap<String, VarInfo>, fl: &mut ScanFlags) {
    for s in &b.stmts {
        scan_stmt(s, vars, fl);
    }
}

fn scan_stmt(s: &Stmt, vars: &mut HashMap<String, VarInfo>, fl: &mut ScanFlags) {
    match &s.kind {
        StmtKind::Let { var, ty, mutable, value } => {
            vars.insert(var.clone(), VarInfo { ty: ty.clone(), mutable: *mutable });
            scan_rv(value, vars, fl);
        }
        StmtKind::Assign { value, .. } => scan_rv(value, vars, fl),
        StmtKind::Call { call, on_fail } => {
            if let CallTarget::Std { module, name } = &call.target {
                if module == "sys" && name == "cd" {
                    fl.cd = true;
                }
            }
            scan_onfail(on_fail, vars, fl);
        }
        StmtKind::Command { on_fail, .. } => scan_onfail(on_fail, vars, fl),
        StmtKind::If { then, els, .. } => {
            scan_block(then, vars, fl);
            if let Some(e) = els {
                scan_block(e, vars, fl);
            }
        }
        StmtKind::While { body, .. } => scan_block(body, vars, fl),
        StmtKind::ForRange { var, body, .. } => {
            vars.insert(var.clone(), VarInfo { ty: Ty::Int, mutable: false });
            scan_block(body, vars, fl);
        }
        StmtKind::ForList { var, index, list, body } => {
            let et = match &list.ty {
                Ty::List(e) => (**e).clone(),
                _ => Ty::Str,
            };
            vars.insert(var.clone(), VarInfo { ty: et, mutable: false });
            if let Some(i) = index {
                vars.insert(i.clone(), VarInfo { ty: Ty::Int, mutable: false });
            }
            scan_block(body, vars, fl);
        }
        StmtKind::ForMap { key, value, map, body } => {
            let (kt, vt) = match &map.ty {
                Ty::Map(k, v) => ((**k).clone(), (**v).clone()),
                _ => (Ty::Str, Ty::Str),
            };
            vars.insert(key.clone(), VarInfo { ty: kt, mutable: false });
            vars.insert(value.clone(), VarInfo { ty: vt, mutable: false });
            scan_block(body, vars, fl);
        }
        StmtKind::Match { arms, default, .. } => {
            for a in arms {
                scan_block(&a.body, vars, fl);
            }
            if let Some(d) = default {
                scan_block(d, vars, fl);
            }
        }
        StmtKind::Try { body, on_fail } => {
            scan_block(body, vars, fl);
            scan_onfail(on_fail, vars, fl);
        }
        StmtKind::Retry { body, on_fail, .. } => {
            scan_block(body, vars, fl);
            scan_onfail(on_fail, vars, fl);
        }
        StmtKind::Parallel { on_fail, .. } => scan_onfail(on_fail, vars, fl),
        StmtKind::Defer(_) => fl.defer = true,
        StmtKind::AssertFails(inner) => scan_stmt(inner, vars, fl),
        _ => {}
    }
}

fn scan_rv(rv: &Rvalue, vars: &mut HashMap<String, VarInfo>, fl: &mut ScanFlags) {
    match rv {
        Rvalue::Call { call, on_fail } => {
            if let CallTarget::Std { module, name } = &call.target {
                if module == "sys" && name == "cd" {
                    fl.cd = true;
                }
            }
            scan_onfail(on_fail, vars, fl);
        }
        Rvalue::Capture { on_fail, .. } => scan_onfail(on_fail, vars, fl),
        _ => {}
    }
}

fn scan_onfail(of: &OnFail, vars: &mut HashMap<String, VarInfo>, fl: &mut ScanFlags) {
    match of {
        OnFail::Default(rv) => scan_rv(rv, vars, fl),
        OnFail::Block { err, body, .. } => {
            if let Some(e) = err {
                vars.insert(e.clone(), VarInfo { ty: Ty::Error, mutable: false });
            }
            scan_block(body, vars, fl);
        }
        _ => {}
    }
}

// ----------------------------------------------------------------------------------------------
// defer: replaces expressions with parameters evaluated at the `defer` point
// ----------------------------------------------------------------------------------------------

fn is_literal(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Str(_) | ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Duration(_) | ExprKind::EnumCase { .. })
}

fn defer_param(e: &Expr, params: &mut Vec<(String, Ty)>, vals: &mut Vec<Expr>) -> Expr {
    if is_literal(e) {
        return e.clone();
    }
    let n = format!("__d{}", params.len() + 1);
    params.push((n.clone(), e.ty.clone()));
    vals.push(e.clone());
    Expr { kind: ExprKind::Var(n), ty: e.ty.clone() }
}

fn defer_rewrite(s: &Stmt, params: &mut Vec<(String, Ty)>, vals: &mut Vec<Expr>) -> Stmt {
    let kind = match &s.kind {
        StmtKind::Call { call, .. } => {
            let mut c = call.clone();
            for a in c.args.iter_mut() {
                if let Arg::Expr(e) = a {
                    *e = defer_param(e, params, vals);
                }
            }
            StmtKind::Call { call: c, on_fail: OnFail::Propagate }
        }
        StmtKind::Command { cmd, .. } => {
            let mut c = cmd.clone();
            for sc in c.pipeline.iter_mut() {
                for w in sc.words.iter_mut() {
                    match w {
                        Word::Lit(_) => {}
                        Word::Interp(parts) => {
                            for p in parts.iter_mut() {
                                if let Part::Expr(e) = p {
                                    *e = defer_param(e, params, vals);
                                }
                            }
                        }
                        Word::Spread(e) => *e = defer_param(e, params, vals),
                    }
                }
            }
            for r in c.redirects.iter_mut() {
                match r {
                    Redirect::Out { path, .. } | Redirect::Err { path, .. } | Redirect::In(path) => *path = defer_param(path, params, vals),
                    Redirect::ErrToOut => {}
                }
            }
            if let Some(e) = c.stdin.as_mut() {
                *e = defer_param(e, params, vals);
            }
            StmtKind::Command { cmd: c, on_fail: OnFail::Propagate }
        }
        other => other.clone(),
    };
    Stmt { line: s.line, kind }
}
