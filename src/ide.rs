//! Editor: hover, completion, signature help and go to definition (D28).
//!
//! Everything comes from the index the checker builds ([`check::Index`]): what each name is and the type of each expression.
//! The analysis runs even with syntax errors (the parser recovers), because code in the editor is almost always
//! incomplete. Positions are byte offsets in the text; the CLI (`nalu ide`) converts from/to
//! line and column.
//!
//! - **Completion after `x.`**: the text is analyzed without the `.partial` (replaced by spaces, so the
//!   offsets do not change) and the type of `x` comes from the expression that ends before the dot.
//! - **Visible names**: each variable stores the range where it is visible (`VarInfo.vis`).
//! - Documentation comes from the stdlib tables (`stdlib.rs`) and from the script's own `///` comments.

use std::path::Path;

use crate::check::{self, Index, Sym, TProgram, VarKind};
use crate::diag::Span;
use crate::ir::Ty;
use crate::lexer::{self, LStrPart, Tok, WPart};
use crate::stdlib::{self, StdParam};

/// The checked program and the index.
pub struct Analysis {
    pub src: String,
    pub prog: TProgram,
    pub index: Index,
}

/// Analyzes on a thread with a large stack (like `compile`); `None` if the checker panics.
pub fn analyze(src: &str, embed_root: &Path) -> Option<Analysis> {
    let run = || {
        let (file, _) = crate::parser::parse(src);
        let (prog, _, index) = check::check_indexed(&file, src, embed_root);
        Analysis { src: src.to_string(), prog, index }
    };
    std::thread::scope(|s| {
        std::thread::Builder::new().name("nalu-ide".into()).stack_size(256 << 20).spawn_scoped(s, run).ok().and_then(|h| h.join().ok())
    })
}

// =================================================================================================
// Hover
// =================================================================================================

pub struct Hover {
    pub span: Span,
    pub markdown: String,
}

pub fn hover(src: &str, root: &Path, offset: usize) -> Option<Hover> {
    let a = analyze(src, root)?;
    if let Some((span, sym)) = sym_at(&a.index, offset) {
        let md = describe(&a, sym);
        if !md.is_empty() {
            return Some(Hover { span, markdown: md });
        }
    }
        // keywords and constants with no symbol (`catch`, `defer`, `retry`…)
    let (span, word) = word_at(src, offset)?;
    let (sig, doc) = stdlib::keyword_doc(word)?;
    Some(Hover { span, markdown: format!("{}\n\n{}", code(sig), doc) })
}

/// The symbol with the smallest range that covers `offset` (the last one registered, on a tie).
fn sym_at(idx: &Index, offset: usize) -> Option<(Span, &Sym)> {
    let mut best: Option<(Span, &Sym)> = None;
    for (sp, s) in &idx.syms {
        if sp.start <= offset && offset < sp.end.max(sp.start + 1) {
            let better = match best {
                None => true,
                Some((b, _)) => sp.end - sp.start <= b.end - b.start,
            };
            if better {
                best = Some((*sp, s));
            }
        }
    }
    best
}

/// The word (letters, digits, `_`) that contains `offset`.
fn word_at(src: &str, offset: usize) -> Option<(Span, &str)> {
    let b = src.as_bytes();
    if offset > b.len() {
        return None;
    }
    let is_w = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut s = offset;
    while s > 0 && is_w(b[s - 1]) {
        s -= 1;
    }
    let mut e = offset;
    while e < b.len() && is_w(b[e]) {
        e += 1;
    }
    if s == e {
        return None;
    }
    Some((Span::new(s, e), &src[s..e]))
}

fn code(s: &str) -> String {
    format!("```nalu\n{}\n```", s)
}

fn tyname(t: &Ty) -> String {
    if check::is_unk(t) {
        "?".into()
    } else {
        t.display()
    }
}

/// `--limpar-cache` from `limpar_cache`.
fn flag_name(n: &str) -> String {
    format!("--{}", n.replace('_', "-"))
}

fn src_of<'s>(src: &'s str, sp: Span) -> &'s str {
    src.get(sp.start..sp.end).unwrap_or("")
}

/// Signature of a user function: `fn name(a: int, b: str = "x") -> str`.
fn fn_signature(a: &Analysis, i: usize) -> String {
    let f = &a.prog.fns[i];
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.default {
            Some(d) => format!("{}: {} = {}", p.name, tyname(&p.ty), src_of(&a.src, d.span)),
            None => format!("{}: {}", p.name, tyname(&p.ty)),
        })
        .collect();
    let ret = if f.ret == Ty::Unit { String::new() } else { format!(" -> {}", tyname(&f.ret)) };
    format!("fn {}({}){}", f.name, params.join(", "), ret)
}

/// The generic types of the tables (`T`, `K`, `V`, `Enum`) replaced by the receiver's.
fn subst(ty: &str, recv: Option<&Ty>) -> String {
    let (t, k, v, en) = match recv {
        Some(Ty::List(t)) => (Some(tyname(t)), None, None, None),
        Some(Ty::Map(k, v)) => (None, Some(tyname(k)), Some(tyname(v)), None),
        Some(Ty::Enum(e)) => (None, None, None, Some(e.clone())),
        _ => (None, None, None, None),
    };
    let mut out = String::new();
    let mut word = String::new();
    let flush = |w: &mut String, out: &mut String| {
        let r = match w.as_str() {
            "T" => t.clone(),
            "K" => k.clone(),
            "V" => v.clone(),
            "Enum" => en.clone(),
            _ => None,
        };
        out.push_str(r.as_deref().unwrap_or(w));
        w.clear();
    };
    for c in ty.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn params_text(params: &[StdParam], recv: Option<&Ty>) -> Vec<String> {
    params
        .iter()
        .map(|p| {
            let ty = if p.ty == "..." { "any...".to_string() } else { subst(p.ty, recv) };
            match p.default {
                Some(d) if d == stdlib::SLICE_END => format!("{}: {} = end", p.name, ty),
                Some(d) => format!("{}: {} = {}", p.name, ty, d),
                None => format!("{}: {}", p.name, ty),
            }
        })
        .collect()
}

fn ret_text(ret: &str, recv: Option<&Ty>) -> String {
    if ret == "unit" {
        String::new()
    } else {
        format!(" -> {}", subst(ret, recv))
    }
}

fn std_signature(f: &stdlib::StdFn) -> String {
    let name = if f.module.is_empty() { f.name.to_string() } else { format!("{}.{}", f.module, f.name) };
    format!("{}({}){}", name, params_text(f.params, None).join(", "), ret_text(f.ret, None))
}

fn method_signature(m: &stdlib::Method, recv: &Ty) -> String {
    format!("{}.{}({}){}", tyname(recv), m.name, params_text(m.params, Some(recv)).join(", "), ret_text(m.ret, Some(recv)))
}

const CAN_FAIL: &str = "⚡ **Can fail.** Handle it with `catch`, or let the failure propagate.";
const WRITES: &str = "✎ **Writes.** With `--dry-run` it is only shown, not run.";
const MUTATES: &str = "Changes the value it is called on, so that value must be a `var`.";

fn type_doc(t: &Ty) -> &'static str {
    match t {
        Ty::Str => "Text.",
        Ty::Int => "A 64-bit integer.",
        Ty::Bool => "`true` or `false`.",
        Ty::Path => "A file or folder path. `/` joins with the system's separator (`dirs.home() / \".config\"`). Its properties (`.name`, `.stem`, `.ext`, `.parent`) only look at the text and never fail; anything on disk is in `fs` (`fs.exists(p)`, `fs.size(p)`).",
        Ty::Duration => "A length of time: `500ms`, `30s`, `1m30s`, `2h`, `7d`.",
        Ty::Secret => "Text that is never printed, logged or passed in a command's arguments; it reaches commands only through stdin (`token | $ cmd`).",
        _ => "",
    }
}

fn describe(a: &Analysis, sym: &Sym) -> String {
    let p = &a.prog;
    match sym {
        Sym::Var(id) => {
            let v = &p.vars[*id];
            let t = tyname(&v.ty);
            match v.kind {
                VarKind::ScriptParam => {
                    let tp = p.params.iter().find(|tp| tp.var == *id);
                    let default = tp.and_then(|tp| tp.default.as_ref()).map(|d| format!(" = {}", src_of(&a.src, d.span))).unwrap_or_default();
                    let mut md = code(&format!("param {}: {}{}", v.name, t, default));
                    if let Some(doc) = tp.map(|tp| tp.doc.trim()).filter(|d| !d.is_empty()) {
                        md.push_str(&format!("\n\n{}", doc));
                    }
                    let env = format!("NALU_{}", v.name.to_uppercase());
                    let how = if tp.and_then(|tp| tp.default.as_ref()).is_some() || v.ty == Ty::Bool {
                        format!("Command-line option `{}`.", flag_name(&v.name))
                    } else {
                        format!("Command-line option `{}` (required).", flag_name(&v.name))
                    };
                    md.push_str(&format!("\n\n{}", how));
                    if v.ty == Ty::Secret {
                        md.push_str(&format!(" Also read from the environment variable `{}`.", env));
                    }
                    md
                }
                VarKind::Global | VarKind::Local => code(&format!("{} {}: {}", if v.mutable { "var" } else { "let" }, v.name, t)),
                VarKind::FnParam => {
                    let owner = p.fns.iter().find(|f| f.span.start <= v.span.start && v.span.end <= f.span.end).map(|f| f.name.clone());
                    let mut md = code(&format!("{}: {}", v.name, t));
                    if let Some(o) = owner {
                        md.push_str(&format!("\n\nParameter of `{}`.", o));
                    }
                    md
                }
                VarKind::Loop => format!("{}\n\nLoop variable.", code(&format!("{}: {}", v.name, t))),
                VarKind::LambdaParam => format!("{}\n\nLambda parameter.", code(&format!("{}: {}", v.name, t))),
                VarKind::SelfParam => format!("{}\n\nThe value this method was called on (read-only).", code(&format!("self: {}", t))),
                VarKind::Err => describe_kw("err"),
            }
        }
        Sym::Fn(i) => {
            let f = &p.fns[*i];
            let mut md = code(&fn_signature(a, *i));
            if !f.doc.trim().is_empty() {
                md.push_str(&format!("\n\n{}", f.doc.trim()));
            }
            if p.fallible.get(*i).copied().unwrap_or(false) {
                let mut why: Vec<String> = Vec::new();
                for (w, _) in p.reasons.get(*i).map(|r| r.as_slice()).unwrap_or(&[]) {
                    if !why.contains(w) {
                        why.push(w.clone());
                    }
                }
                md.push_str(&format!("\n\n{}", CAN_FAIL));
                if !why.is_empty() {
                    let shown: Vec<String> = why.iter().take(4).cloned().collect();
                    let list = match shown.len() {
                        1 => shown[0].clone(),
                        n => format!("{} or {}", shown[..n - 1].join(", "), shown[n - 1]),
                    };
                    let more = if why.len() > 4 { " (among others)" } else { "" };
                    md.push_str(&format!(" It fails when {} fails{}.", list, more));
                }
            }
            md
        }
        Sym::Std(f) => {
            let mut md = code(&std_signature(f));
            if !f.doc.is_empty() {
                md.push_str(&format!("\n\n{}", f.doc));
            }
            if f.fallible {
                md.push_str(&format!("\n\n{}", CAN_FAIL));
            }
            if f.writes {
                md.push_str(&format!("\n\n{}", WRITES));
            }
            md
        }
        Sym::Builtin(name) => match stdlib::find_builtin(name) {
            Some(f) => format!("{}\n\n{}", code(&std_signature(f)), f.doc),
            None => describe_kw(name),
        },
        Sym::Method { recv, name } => {
            if name == "len" {
                let what = if matches!(recv, Ty::Str | Ty::Path) { "The number of characters." } else { "The number of items." };
                return format!("{}\n\n{}", code(&format!("{}.len() -> int", tyname(recv))), what);
            }
            let Some(cat) = check::method_cat(recv) else { return String::new() };
            let Some(m) = stdlib::find_method(cat, name) else { return String::new() };
            let mut md = code(&method_signature(m, recv));
            if !m.doc.is_empty() {
                md.push_str(&format!("\n\n{}", m.doc));
            }
            if m.mutates {
                md.push_str(&format!("\n\n{}", MUTATES));
            }
            if m.fallible {
                md.push_str(&format!("\n\n{}", CAN_FAIL));
            }
            md
        }
        Sym::Field { owner, name, ty } => format!("{}\n\nField of `{}`.", code(&format!("{}: {}", name, tyname(ty))), owner),
        Sym::Prop { recv, name, ty } => {
            let doc = match recv {
                Ty::Error if name == "message" => "The failure's message (what `{err}` shows).".to_string(),
                Ty::Error => "The exit code: 1 by default; for commands, the command's own code (127 if it was not found).".to_string(),
                _ => stdlib::PATH_PROPS.iter().find(|(n, _, _)| n == name).map(|(_, _, d)| d.to_string()).unwrap_or_default(),
            };
            let mut md = code(&format!("{}.{}: {}", tyname(recv), name, tyname(ty)));
            if !doc.is_empty() {
                md.push_str(&format!("\n\n{}", doc));
            }
            md
        }
        Sym::Struct(n) => {
            let Some(s) = p.structs.iter().find(|s| &s.name == n) else { return String::new() };
            let mut lines = vec![format!("struct {} {{", n)];
            for f in &s.fields {
                lines.push(format!("    {}: {}", f.name, tyname(&f.ty)));
            }
            for (i, f) in p.fns.iter().enumerate() {
                if f.method_of.as_deref() == Some(n.as_str()) {
                    let sig = fn_signature(a, i).replacen(&format!("fn {}.", n), "fn ", 1);
                    lines.push(format!("    {}", sig));
                }
            }
            lines.push("}".into());
            code(&lines.join("\n"))
        }
        Sym::Enum(n) => {
            let Some(e) = p.enums.iter().find(|e| &e.name == n) else { return String::new() };
            format!("{}\n\nRead a case with `{}.{}`, or from text with `{}.parse(text)`.", code(&format!("enum {} {{ {} }}", n, e.cases.join(", "))), n, e.cases.first().cloned().unwrap_or_default(), n)
        }
        Sym::Case(e, c) => format!("{}\n\nA case of `{}`.", code(&format!("{}.{}", e, c)), e),
        Sym::Module(m) => {
            let fns: Vec<String> = stdlib::STDLIB.iter().filter(|f| f.module == m && f.v1).map(|f| format!("`{}`", f.name)).collect();
            let mut md = code(&format!("module {}", m));
            if let Some(d) = stdlib::module_doc(m) {
                md.push_str(&format!("\n\n{}", d));
            }
            if !fns.is_empty() {
                md.push_str(&format!("\n\nFunctions: {}.", fns.join(", ")));
            }
            md
        }
        Sym::Type(t) => format!("{}\n\n{}", code(&format!("type {}", tyname(t))), type_doc(t)),
    }
}

fn describe_kw(name: &str) -> String {
    match stdlib::keyword_doc(name) {
        Some((sig, doc)) => format!("{}\n\n{}", code(sig), doc),
        None => String::new(),
    }
}

// =================================================================================================
// Go to definition
// =================================================================================================

pub fn definition(src: &str, root: &Path, offset: usize) -> Option<Span> {
    let a = analyze(src, root)?;
    let (_, sym) = sym_at(&a.index, offset)?;
    let p = &a.prog;
    let first = |pred: &dyn Fn(&Sym) -> bool| a.index.syms.iter().find(|(_, s)| pred(s)).map(|(sp, _)| *sp);
    match sym {
        Sym::Var(id) => {
            let v = &p.vars[*id];
            match v.kind {
                VarKind::SelfParam | VarKind::Err => None,
                _ => Some(v.span),
            }
        }
        Sym::Fn(i) => Some(p.fns[*i].name_span),
        Sym::Struct(n) => first(&|s| matches!(s, Sym::Struct(x) if x == n)),
        Sym::Enum(n) => first(&|s| matches!(s, Sym::Enum(x) if x == n)),
        Sym::Case(e, c) => first(&|s| matches!(s, Sym::Case(x, y) if x == e && y == c)),
        Sym::Field { owner, name, .. } => p.structs.iter().find(|s| &s.name == owner).and_then(|s| s.fields.iter().find(|f| &f.name == name)).map(|f| f.span),
        _ => None,
    }
}

// =================================================================================================
// Completion
// =================================================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    /// `variable`, `function`, `method`, `field`, `property`, `module`, `struct`, `enum`, `case`,
    /// `keyword`, `snippet`, `type`, `constant`.
    pub kind: &'static str,
    pub detail: String,
    pub doc: String,
        /// Text inserted (VS Code snippet syntax: `${1:x}`), if different from the label.
    pub insert: Option<String>,
}

pub struct Completion {
        /// Where the part to be replaced starts (the start of the word being typed).
    pub from: usize,
    pub items: Vec<Item>,
}

fn is_w(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub fn complete(src: &str, root: &Path, offset: usize) -> Completion {
    let offset = offset.min(src.len());
    let b = src.as_bytes();
    let mut from = offset;
    while from > 0 && is_w(b[from - 1]) {
        from -= 1;
    }
    let empty = Completion { from, items: vec![] };
    if !in_code(src, offset) {
        return empty;
    }
    let member = from > 0 && b[from - 1] == b'.' && !(from > 1 && b[from - 2] == b'.');
    let items = if member { complete_member(src, root, from - 1, offset) } else { complete_scope(src, root, from) };
    Completion { from, items }
}

/// Is `offset` in code (and not inside a string, a command outside `{…}`, or a comment)?
fn in_code(src: &str, offset: usize) -> bool {
    let (toks, _) = lexer::lex(src, 0);
    let mut prev_end = 0;
    for t in &toks {
        if t.span.start >= offset {
            break;
        }
        if t.span.start < offset && offset < t.span.end || (offset == t.span.end && matches!(t.tok, Tok::Str(_) | Tok::Cmd(_) | Tok::Capture(_)) && !closed(src, t.span)) {
            return match &t.tok {
                Tok::Str(st) => st.parts.iter().any(|p| matches!(p, LStrPart::Code { src: c, offset: o } if *o <= offset && offset <= o + c.len())) || open_brace(src, t.span.start, offset),
                Tok::Cmd(c) | Tok::Capture(c) => {
                    c.stages.iter().flatten().chain(c.redirects.iter().filter_map(|r| r.target.as_ref())).any(|w| {
                        w.parts.iter().any(|p| matches!(p, WPart::Code { src: c, offset: o, .. } if *o <= offset && offset <= o + c.len()))
                    }) || open_brace(src, t.span.start, offset)
                }
                _ => true,
            };
        }
        prev_end = t.span.end;
    }
        // between tokens: a `//` before the cursor on the same line is a comment
    let gap = &src[prev_end.min(offset)..offset];
    let line = gap.rsplit('\n').next().unwrap_or("");
    !line.contains("//") && !gap.contains("/*") || gap.contains("*/") && !gap.rsplit("*/").next().unwrap_or("").contains("/*") && !line.contains("//")
}

/// Is there an open `{` (not `{{` nor `\{`) between `start` and the cursor? Covers `"{|}"` and `"{nom|"`, which the lexer
/// does not yet see as interpolation.
fn open_brace(src: &str, start: usize, offset: usize) -> bool {
    let b = src.as_bytes();
    let mut i = offset;
    while i > start {
        i -= 1;
        match b[i] {
            b'}' => return false,
            b'{' => return !(i > start && matches!(b[i - 1], b'{' | b'\\')) && !(i + 1 < b.len() && b[i + 1] == b'{'),
            b'\n' => return false,
            _ => {}
        }
    }
    false
}

/// Does the string/command token end with its closer (quote, `)`)? Otherwise, the cursor at the end is still inside.
fn closed(src: &str, sp: Span) -> bool {
    let t = src_of(src, sp);
    t.len() >= 2 && (t.ends_with('"') || t.ends_with('\'') || t.ends_with(')'))
}

fn complete_member(src: &str, root: &Path, dot: usize, offset: usize) -> Vec<Item> {
    let b = src.as_bytes();
        // the receiver ends before the dot (skipping spaces and line breaks: `xs\n    .map`)
    let mut rend = dot;
    while rend > 0 && matches!(b[rend - 1], b' ' | b'\t' | b'\r' | b'\n') {
        rend -= 1;
    }
    let mut rstart = rend;
    while rstart > 0 && is_w(b[rstart - 1]) {
        rstart -= 1;
    }
    let plain = rstart < rend && (rstart == 0 || !matches!(b[rstart - 1], b'.')) && !b[rstart].is_ascii_digit();
    let rname = &src[rstart..rend];

        // the text without the `.partial`, with the same offsets
    let mut patched = String::with_capacity(src.len());
    patched.push_str(&src[..dot]);
    patched.push_str(&" ".repeat(offset - dot));
    patched.push_str(&src[offset..]);
    let a = analyze(&patched, root);

    if plain {
        let shadowed = a.as_ref().map(|a| matches!(sym_at(&a.index, rstart), Some((_, Sym::Var(_))))).unwrap_or(false);
        if !shadowed && stdlib::module_doc(rname).is_some() {
            return module_items(rname);
        }
        if let Some(a) = &a {
            if let Some(e) = a.prog.enums.iter().find(|e| e.name == rname) {
                if !shadowed {
                    return enum_items(e);
                }
            }
        }
    }
    let Some(a) = a else { return vec![] };
    let cstart = chain_start(src, rend);
    let mut best: Option<(Span, &Ty)> = None;
    for (sp, t) in &a.index.exprs {
        if sp.end != rend || check::is_unk(t) {
            continue;
        }
        let fits = sp.start >= cstart;
        let better = match best {
            None => true,
            Some((bs, _)) => {
                let bfits = bs.start >= cstart;
                (fits && !bfits) || (fits == bfits && if fits { sp.start < bs.start } else { sp.start > bs.start })
            }
        };
        if better {
            best = Some((*sp, t));
        }
    }
    match best {
        Some((_, t)) => type_members(&a, t),
        None => vec![],
    }
}

/// Start of the chain `a.b(c)[d].e` that ends at `end`, looking only at the text.
fn chain_start(src: &str, end: usize) -> usize {
    let b = src.as_bytes();
    let mut i = end;
    loop {
        while i > 0 && is_w(b[i - 1]) {
            i -= 1;
        }
        if i > 0 && matches!(b[i - 1], b')' | b']') {
            let mut depth = 0i32;
            let mut j = i;
            while j > 0 {
                j -= 1;
                match b[j] {
                    b')' | b']' => depth += 1,
                    b'(' | b'[' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    b'\n' if depth <= 0 => break,
                    _ => {}
                }
            }
            i = j;
            continue;
        }
        if i > 0 && matches!(b[i - 1], b'"' | b'\'') {
            let q = b[i - 1];
            let mut j = i - 1;
            while j > 0 {
                j -= 1;
                if b[j] == q && (j == 0 || b[j - 1] != b'\\') {
                    break;
                }
            }
            i = j;
            continue;
        }
        if i > 1 && b[i - 1] == b'.' && b[i - 2] != b'.' {
            i -= 1;
            while i > 0 && matches!(b[i - 1], b' ' | b'\t' | b'\r' | b'\n') {
                i -= 1;
            }
            continue;
        }
        return i;
    }
}

fn snippet_args(params: &[StdParam], recv: Option<&Ty>) -> String {
    let mut out = Vec::new();
    let mut n = 1;
    for p in params {
        if p.default.is_some() {
            break;
        }
        if p.ty.starts_with('(') {
            out.push(format!("${{{}:x}} => ${{{}}}", n, n + 1));
            n += 2;
        } else if p.ty == "..." {
            out.push(format!("${{{}}}", n));
            n += 1;
        } else {
            let _ = recv;
            out.push(format!("${{{}:{}}}", n, p.name));
            n += 1;
        }
    }
    out.join(", ")
}

fn module_items(m: &str) -> Vec<Item> {
    stdlib::STDLIB
        .iter()
        .filter(|f| f.module == m && f.v1)
        .map(|f| Item {
            label: f.name.to_string(),
            kind: "function",
            detail: std_signature(f),
            doc: std_doc(f),
            insert: Some(format!("{}({})", f.name, snippet_args(f.params, None))),
        })
        .collect()
}

fn std_doc(f: &stdlib::StdFn) -> String {
    let mut d = f.doc.to_string();
    if f.fallible {
        d.push_str(&format!("\n\n{}", CAN_FAIL));
    }
    if f.writes {
        d.push_str(&format!("\n\n{}", WRITES));
    }
    d
}

fn enum_items(e: &check::EnumInfo) -> Vec<Item> {
    let mut v: Vec<Item> = e
        .cases
        .iter()
        .map(|c| Item { label: c.clone(), kind: "case", detail: format!("{}.{}", e.name, c), doc: format!("A case of `{}`.", e.name), insert: None })
        .collect();
    if let Some(m) = stdlib::find_method("enum", "parse") {
        let recv = Ty::Enum(e.name.clone());
        v.push(Item {
            label: "parse".into(),
            kind: "method",
            detail: method_signature(m, &recv),
            doc: format!("{}\n\n{}", m.doc, CAN_FAIL),
            insert: Some("parse(${1:text})".into()),
        });
    }
    v
}

fn type_members(a: &Analysis, t: &Ty) -> Vec<Item> {
    let mut v = Vec::new();
    match t {
        Ty::Struct(n) => {
            if let Some(s) = a.prog.structs.iter().find(|s| &s.name == n) {
                for f in &s.fields {
                    v.push(Item { label: f.name.clone(), kind: "field", detail: format!("{}: {}", f.name, tyname(&f.ty)), doc: format!("Field of `{}`.", n), insert: None });
                }
            }
            for (i, f) in a.prog.fns.iter().enumerate() {
                if f.method_of.as_deref() == Some(n.as_str()) {
                    let short = f.name.rsplit('.').next().unwrap_or("").to_string();
                    let args: Vec<String> = f.params.iter().filter(|p| p.default.is_none()).enumerate().map(|(k, p)| format!("${{{}:{}}}", k + 1, p.name)).collect();
                    v.push(Item {
                        label: short.clone(),
                        kind: "method",
                        detail: fn_signature(a, i),
                        doc: describe(a, &Sym::Fn(i)).split_once("\n```\n").map(|(_, d)| d.trim().to_string()).unwrap_or_default(),
                        insert: Some(format!("{}({})", short, args.join(", "))),
                    });
                }
            }
        }
        Ty::Error => {
            v.push(Item { label: "message".into(), kind: "property", detail: "message: str".into(), doc: "The failure's message (what `{err}` shows).".into(), insert: None });
            v.push(Item { label: "code".into(), kind: "property", detail: "code: int".into(), doc: "The exit code: 1 by default; for commands, the command's own code.".into(), insert: None });
        }
        _ => {}
    }
    if *t == Ty::Path {
        for (n, ty, d) in stdlib::PATH_PROPS {
            let doc = d.to_string();
            v.push(Item { label: n.to_string(), kind: "property", detail: format!("path.{}: {}", n, ty), doc, insert: None });
        }
    }
    if matches!(t, Ty::Str | Ty::Path | Ty::List(_) | Ty::Map(_, _)) {
        let what = if matches!(t, Ty::Str | Ty::Path) { "The number of characters." } else { "The number of items." };
        v.push(Item { label: "len".into(), kind: "method", detail: format!("{}.len() -> int", tyname(t)), doc: what.into(), insert: Some("len()".into()) });
    }
    if let Some(cat) = check::method_cat(t) {
        for m in stdlib::METHODS.iter().filter(|m| m.recv == cat && !(m.recv == "enum" && m.name == "parse")) {
            let mut doc = m.doc.to_string();
            if m.mutates {
                doc.push_str(&format!("\n\n{}", MUTATES));
            }
            if m.fallible {
                doc.push_str(&format!("\n\n{}", CAN_FAIL));
            }
            v.push(Item {
                label: m.name.to_string(),
                kind: "method",
                detail: method_signature(m, t),
                doc,
                insert: Some(format!("{}({})", m.name, snippet_args(m.params, Some(t)))),
            });
        }
    }
    v
}

/// Types written in an annotation: after `param x:`, `let x:`, `fn f(a:`, a struct field, or `->`.
fn in_type_position(src: &str, from: usize) -> bool {
    let line_start = src[..from].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let before = src[line_start..from].trim_end();
    if before.ends_with("->") || before.ends_with("[]") || before.ends_with("map[") || before.ends_with(']') && before.contains("map[") {
        return true;
    }
    let Some(head) = before.strip_suffix(':') else { return false };
    let head = head.trim_end();
    let name_start = head.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map(|i| i + 1).unwrap_or(0);
    if name_start == head.len() {
        return false;
    }
    let pre = head[..name_start].trim_end();
        // `param x:` / `let x:` / `var x:`
    if pre.is_empty() || pre.ends_with("param") || pre.ends_with("let") || pre.ends_with("var") {
        if pre.is_empty() {
                        // `name:` alone on the line: it is only a type inside a `struct`
            return inside_struct_decl(src, line_start);
        }
        return true;
    }
        // `fn f(a: int, b:` (no `=` before: named call arguments are not types)
    if let Some(fpos) = head.rfind("fn ") {
        let after = &head[fpos..];
        return after.contains('(') && !after.contains(')');
    }
    false
}

/// Is the line at `at` directly inside `struct Name { … }`?
fn inside_struct_decl(src: &str, at: usize) -> bool {
    let b = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    while i > 0 {
        i -= 1;
        match b[i] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    let head = src[..i].trim_end();
                    let ls = head.rfind('\n').map(|j| j + 1).unwrap_or(0);
                    return head[ls..].trim_start().starts_with("struct ");
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    false
}

fn type_items(a: Option<&Analysis>) -> Vec<Item> {
    let mut v: Vec<Item> = [Ty::Str, Ty::Int, Ty::Bool, Ty::Path, Ty::Duration, Ty::Secret]
        .iter()
        .map(|t| Item { label: t.display(), kind: "type", detail: format!("type {}", t.display()), doc: type_doc(t).into(), insert: None })
        .collect();
    v.push(Item { label: "map".into(), kind: "type", detail: "map[K]V".into(), doc: "A map from `str`, `int` or enum keys to scalars or lists of scalars.".into(), insert: Some("map[${1:str}]${2:int}".into()) });
    if let Some(a) = a {
        for s in &a.prog.structs {
            v.push(Item { label: s.name.clone(), kind: "struct", detail: format!("struct {}", s.name), doc: String::new(), insert: None });
        }
        for e in &a.prog.enums {
            v.push(Item { label: e.name.clone(), kind: "enum", detail: format!("enum {} {{ {} }}", e.name, e.cases.join(", ")), doc: String::new(), insert: None });
        }
    }
    v
}

const SNIPPETS: &[(&str, &str, &str)] = &[
    ("fn", "fn ${1:name}(${2}) {\n\t$0\n}", "function"),
    ("if", "if ${1:cond} {\n\t$0\n}", "if"),
    ("else", "else {\n\t$0\n}", "else"),
    ("for", "for ${1:x} in ${2:list} {\n\t$0\n}", "loop over a list"),
    ("while", "while ${1:cond} {\n\t$0\n}", "while loop"),
    ("match", "match ${1:value} {\n\t${2:_} => $0\n}", "match"),
    ("struct", "struct ${1:Name} {\n\t${2:field}: ${3:str}\n}", "struct"),
    ("enum", "enum ${1:Name} { ${2:a}, ${3:b} }", "enum"),
    ("param", "/// ${3:what it is}\nparam ${1:name}: ${2:str}", "script option"),
    ("test", "test \"${1:name}\" {\n\t$0\n}", "test"),
    ("try", "try {\n\t$0\n} catch ${1:ignore}", "try … catch"),
    ("retry", "retry(${1:3}, every: ${2:5s}) {\n\t$0\n}", "retry"),
    ("parallel", "parallel for ${1:x} in ${2:list} {\n\t$0\n}", "parallel for"),
    ("defer", "defer ${1:fs.rm(tmp)}", "defer"),
    ("let", "let ${1:name} = $0", "let"),
    ("var", "var ${1:name} = $0", "var"),
    ("return", "return $0", "return"),
    ("fail", "fail \"$0\"", "fail"),
];

fn complete_scope(src: &str, root: &Path, from: usize) -> Vec<Item> {
    let a = analyze(src, root);
    if in_type_position(src, from) {
        return type_items(a.as_ref());
    }
    if let Some(items) = a.as_ref().and_then(|a| match_arm_items(src, a, from)) {
        return items;
    }
    let mut v: Vec<Item> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Some(a) = &a {
        let p = &a.prog;
        for id in visible_vars(a, from) {
            let var = &p.vars[id];
            seen.push(var.name.clone());
            let md = describe(a, &Sym::Var(id));
            let (detail, doc) = split_md(&md);
            v.push(Item { label: var.name.clone(), kind: "variable", detail, doc, insert: None });
        }
        if p.fns.iter().any(|f| f.method_of.is_some() && f.span.start <= from && from <= f.span.end) {
            v.push(Item { label: "self".into(), kind: "keyword", detail: "self".into(), doc: stdlib::keyword_doc("self").map(|k| k.1.to_string()).unwrap_or_default(), insert: None });
        }
        for (i, f) in p.fns.iter().enumerate() {
            if f.method_of.is_some() || seen.contains(&f.name) {
                continue;
            }
            seen.push(f.name.clone());
            let args: Vec<String> = f.params.iter().filter(|p| p.default.is_none()).enumerate().map(|(k, p)| format!("${{{}:{}}}", k + 1, p.name)).collect();
            let (detail, doc) = split_md(&describe(a, &Sym::Fn(i)));
            v.push(Item { label: f.name.clone(), kind: "function", detail, doc, insert: Some(format!("{}({})", f.name, args.join(", "))) });
        }
        for s in &p.structs {
            v.push(Item { label: s.name.clone(), kind: "struct", detail: format!("struct {}", s.name), doc: String::new(), insert: Some(format!("{} {{ $0 }}", s.name)) });
        }
        for e in &p.enums {
            v.push(Item { label: e.name.clone(), kind: "enum", detail: format!("enum {} {{ {} }}", e.name, e.cases.join(", ")), doc: String::new(), insert: None });
        }
    }
    for (m, d) in stdlib::MODULES {
        if stdlib::STDLIB.iter().any(|f| f.module == *m && f.v1) {
            v.push(Item { label: m.to_string(), kind: "module", detail: format!("module {}", m), doc: d.to_string(), insert: None });
        }
    }
    for f in stdlib::BUILTINS.iter().filter(|f| !stdlib::internal_builtin(f.name)) {
        v.push(Item { label: f.name.to_string(), kind: "function", detail: std_signature(f), doc: f.doc.to_string(), insert: Some(format!("{}({})", f.name, snippet_args(f.params, None))) });
    }
    for name in ["os", "dry_run", "args"] {
        let (sig, doc) = stdlib::keyword_doc(name).unwrap_or(("", ""));
        v.push(Item { label: name.into(), kind: "constant", detail: sig.into(), doc: doc.into(), insert: None });
    }
    for name in ["exit", "embed"] {
        let (sig, doc) = stdlib::keyword_doc(name).unwrap_or(("", ""));
        v.push(Item { label: name.into(), kind: "function", detail: sig.into(), doc: doc.into(), insert: Some(format!("{}(${{1}})", name)) });
    }
    for (kw, body, what) in SNIPPETS {
        let doc = stdlib::keyword_doc(kw).map(|k| k.1).unwrap_or("");
        v.push(Item { label: kw.to_string(), kind: "snippet", detail: what.to_string(), doc: doc.into(), insert: Some(body.to_string()) });
    }
    for kw in ["catch", "ignore", "in", "break", "continue", "true", "false", "native", "assert", "assert_fails"] {
        let (sig, doc) = stdlib::keyword_doc(kw).unwrap_or(("", ""));
        v.push(Item { label: kw.into(), kind: "keyword", detail: sig.into(), doc: doc.into(), insert: None });
    }
    v
}

/// At the start of a `match` arm over an enum (or `bool`): the cases still missing, and `_`.
fn match_arm_items(src: &str, a: &Analysis, from: usize) -> Option<Vec<Item>> {
    let b = src.as_bytes();
    let line_start = src[..from].rfind('\n').map(|i| i + 1).unwrap_or(0);
    if !src[line_start..from].trim().is_empty() {
        return None;
    }
        // the nearest open `{`
    let mut depth = 0i32;
    let mut i = line_start;
    let brace = loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match b[i] {
            b'}' => depth += 1,
            b'{' if depth == 0 => break i,
            b'{' => depth -= 1,
            _ => {}
        }
    };
    let ls = src[..brace].rfind('\n').map(|j| j + 1).unwrap_or(0);
    let head = &src[ls..brace];
    let m = head.find("match ")?;
    let subj_start = ls + m + "match ".len() + (head[m + 6..].len() - head[m + 6..].trim_start().len());
    let subj_end = ls + head.trim_end().len();
    let ty = a.index.exprs.iter().find(|(sp, _)| sp.start == subj_start && sp.end == subj_end).map(|(_, t)| t.clone())?;
        // cases already written in this match (between the `{` and the cursor)
    let written = &src[brace..from];
    let used = |c: &str| written.lines().any(|l| l.trim_start().split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).next() == Some(c));
    let mut v: Vec<Item> = match &ty {
        Ty::Enum(e) => {
            let en = a.prog.enums.iter().find(|x| &x.name == e)?;
            en.cases.iter().filter(|c| !used(c)).map(|c| Item { label: c.clone(), kind: "case", detail: format!("{}.{}", e, c), doc: format!("A case of `{}`.", e), insert: Some(format!("{} => $0", c)) }).collect()
        }
        Ty::Bool => ["true", "false"].iter().filter(|c| !used(c)).map(|c| Item { label: c.to_string(), kind: "keyword", detail: "bool".into(), doc: String::new(), insert: Some(format!("{} => $0", c)) }).collect(),
        _ => return None,
    };
    let (sig, doc) = stdlib::keyword_doc("_").unwrap_or(("", ""));
    v.push(Item { label: "_".into(), kind: "keyword", detail: sig.into(), doc: doc.into(), insert: Some("_ => $0".into()) });
    Some(v)
}

/// The variables visible at `from` (indices into `prog.vars`), innermost first and without repeated names:
/// whoever declares later hides whoever declared earlier. Globals are visible inside functions and tests.
fn visible_vars(a: &Analysis, from: usize) -> Vec<usize> {
    let p = &a.prog;
    let in_fn_or_test = p.fns.iter().any(|f| f.span.start <= from && from <= f.span.end) || p.tests.iter().any(|t| t.span.start <= from && from <= t.span.end);
    let mut ids: Vec<usize> = (0..p.vars.len())
        .filter(|&i| {
            let v = &p.vars[i];
            !v.name.is_empty()
                && match v.kind {
                    VarKind::SelfParam => false,
                    VarKind::ScriptParam => true,
                    VarKind::Global => from >= v.vis.0 || in_fn_or_test,
                    _ => v.vis.0 <= from && from <= v.vis.1,
                }
        })
        .collect();
    ids.sort_by_key(|&i| std::cmp::Reverse(p.vars[i].vis.0));
    let mut seen: Vec<&str> = Vec::new();
    ids.retain(|&i| {
        let n = p.vars[i].name.as_str();
        if seen.contains(&n) {
            false
        } else {
            seen.push(n);
            true
        }
    });
    ids
}

/// The first code block of a hover markdown becomes the `detail`; the rest, the documentation.
fn split_md(md: &str) -> (String, String) {
    if let Some(rest) = md.strip_prefix("```nalu\n") {
        if let Some((sig, doc)) = rest.split_once("\n```") {
            return (sig.to_string(), doc.trim().to_string());
        }
    }
    (String::new(), md.to_string())
}

// =================================================================================================
// Documentation (`nalu doc`)
// =================================================================================================

/// The first sentence of a description (up to the first ". ").
fn first_sentence(d: &str) -> String {
    let s = match d.find(". ") {
        Some(i) => &d[..=i],
        None => d,
    };
    s.replace('\n', " ")
}

fn flags_md(fallible: bool, writes: bool) -> String {
    let mut s = String::new();
    if fallible {
        s.push_str(&format!("\n\n{}", CAN_FAIL));
    }
    if writes {
        s.push_str(&format!("\n\n{}", WRITES));
    }
    s
}

/// The built-in types that have a page in `nalu doc`, with the receiver used for the methods.
fn doc_type(name: &str) -> Option<Ty> {
    Some(match name {
        "str" => Ty::Str,
        "int" => Ty::Int,
        "bool" => Ty::Bool,
        "path" => Ty::Path,
        "duration" => Ty::Duration,
        "secret" => Ty::Secret,
        "list" | "[]T" | "[]" => Ty::list(Ty::Struct("T".into())),
        "map" => Ty::map(Ty::Struct("K".into()), Ty::Struct("V".into())),
        "enum" => Ty::Enum("Enum".into()),
        "error" | "err" => Ty::Error,
        _ => return None,
    })
}

/// A type's page: what it is and all its members, one line each.
fn type_page(name: &str, t: &Ty) -> String {
    let mut md = format!("## type {}\n\n", name);
    let intro = match t {
        Ty::List(_) => "A list: `[1, 2, 3]`, typed `[]T` (`[]str`, `[]int`, `[][]str`). Indexing `xs[i]` fails when out of range.",
        Ty::Map(_, _) => "A map: `{ \"a\": 1 }`, typed `map[K]V`; keys are `str`, `int` or an enum, values are scalars or lists of scalars (`map[str][]str`). `m[k]` fails when the key does not exist; `m[k] = v` writes.",
        Ty::Enum(_) => "Declared with `enum Name { a, b }`. Cases are written `Name.a` (just `a` inside `match`), interpolation gives the case's name (`\"{x}\"`), and every enum has `Name.parse(text)`.",
        Ty::Error => "The failure being handled, as `err` inside `catch { … }` and in `catch fail \"…{err}\"`.",
        _ => type_doc(t),
    };
    md.push_str(intro);
    md.push_str("\n\n");
    let mut lines = Vec::new();
    if *t == Ty::Error {
        lines.push("- `err.message: str`: the failure's message (what `{err}` shows).".to_string());
        lines.push("- `err.code: int`: the exit code; 1 by default, the command's own code for commands.".to_string());
    }
    if *t == Ty::Path {
        for (n, ty, d) in stdlib::PATH_PROPS {
            lines.push(format!("- `path.{}: {}`: {}", n, ty, d));
        }
    }
    if matches!(t, Ty::Str | Ty::Path | Ty::List(_) | Ty::Map(_, _)) {
        let what = if matches!(t, Ty::Str | Ty::Path) { "The number of characters." } else { "The number of items." };
        lines.push(format!("- `{}.len() -> int`: {}", tyname(t), what));
    }
    if let Some(cat) = check::method_cat(t) {
        for m in stdlib::METHODS.iter().filter(|m| m.recv == cat) {
            let f = if m.fallible { " (can fail)" } else { "" };
            lines.push(format!("- `{}`{}: {}", method_signature(m, t), f, m.doc));
        }
    }
    let notes = match t {
        Ty::Int => "- Arithmetic: `+ - * / %` (`/` truncates toward zero); comparison: `== != < <= > >=`.\n- From text: `s.to_int()` (can fail); to text: `\"{n}\"`.",
        Ty::Bool => "- `&&`, `||` and `!`; `if` takes only `bool` (compare explicitly: `n != 0`, `s != \"\"`).",
        Ty::Duration => "- `+`, `-` and comparison between durations; `d / 1s` gives an `int`; `d * 2` scales it.\n- Interpolation shows the shortest form: `\"{d}\"` → `1m30s`.",
        Ty::Secret => "- Made with `secret(text)`, `ui.password(q)` or a `param` of type `secret` (also read from `NALU_<NAME>`).\n- It cannot be interpolated, printed or compared; pass it to a command with `token | $ cmd` or to a function that takes `secret`.",
        _ => "",
    };
    if !notes.is_empty() {
        lines.push(notes.to_string());
    }
    md.push_str(&lines.join("\n"));
    md.trim_end().to_string()
}

fn module_page(m: &str) -> String {
    let mut md = format!("## module {}\n\n{}\n\n", m, stdlib::module_doc(m).unwrap_or(""));
    let lines: Vec<String> = stdlib::STDLIB
        .iter()
        .filter(|f| f.module == m)
        .map(|f| {
            let mut tags = Vec::new();
            if f.fallible {
                tags.push("can fail");
            }
            if f.writes {
                tags.push("writes");
            }
            if !f.v1 {
                tags.push("planned");
            }
            let tags = if tags.is_empty() { String::new() } else { format!(" ({})", tags.join(", ")) };
            format!("- `{}`{}: {}", std_signature(f), tags, f.doc)
        })
        .collect();
    md.push_str(&lines.join("\n"));
    md
}

fn overview() -> String {
    let mut md = String::from("# Nalu standard library\n\nNo imports: every module is always available. Run `nalu doc <name>` for one module, function, type or keyword (`nalu doc fs`, `nalu doc fs.copy`, `nalu doc str`, `nalu doc catch`). The whole reference, as markdown: `nalu doc --all`.\n\n## Modules\n\n");
    for (m, d) in stdlib::MODULES {
        md.push_str(&format!("- `{}`: {}\n", m, d));
    }
    md.push_str("\n## Types\n\n`str`, `int`, `bool`, `path`, `duration`, `secret`, `list` (`[]T`), `map` (`map[K]V`), `enum`, `error` (`err`).\n\n## Built-ins\n\n");
    for f in stdlib::BUILTINS.iter().filter(|f| !stdlib::internal_builtin(f.name)) {
        md.push_str(&format!("- `{}`: {}\n", std_signature(f), f.doc));
    }
    md.push_str("\n## Keywords and constants\n\n");
    let names: Vec<String> = stdlib::KEYWORDS.iter().map(|(n, _, _)| format!("`{}`", n)).collect();
    md.push_str(&names.join(" "));
    md
}

/// The complete stdlib reference in markdown (`nalu doc --all` → `docs/stdlib.md`), generated from the same
/// tables the compiler uses: the documentation never drifts from the code.
pub fn stdlib_reference() -> String {
    let mut md = String::from("# Standard library\n\n<!-- Generated by `nalu doc --all` from the compiler's own tables; do not edit by hand. -->\n\nEvery module is always available: there are no imports. Each entry says whether it **can fail** (handle it with `catch`, or let the failure propagate) and whether it **writes** (with `--dry-run` it is only shown, not run). `nalu doc <name>` prints any entry in the terminal.\n\n");
    md.push_str("**Modules:** ");
    let mods: Vec<String> = stdlib::MODULES.iter().map(|(m, _)| format!("[`{}`](#module-{})", m, m)).collect();
    md.push_str(&mods.join(" · "));
    md.push_str("  \n**Types:** ");
    let tys = ["str", "path", "list", "map", "int", "bool", "duration", "secret", "enum", "error"];
    md.push_str(&tys.iter().map(|t| format!("[`{}`](#type-{})", t, t)).collect::<Vec<_>>().join(" · "));
    md.push_str("  \n**Also:** [built-ins](#built-ins) · [keywords and constants](#keywords-and-constants)\n\n");
    for (m, _) in stdlib::MODULES {
        md.push_str(&module_page(m).replacen("## module ", "## Module ", 1));
        md.push_str("\n\n");
    }
    for t in tys {
        let ty = doc_type(t).expect("every type has a page");
        md.push_str(&type_page(t, &ty).replacen("## type ", "## Type ", 1));
        md.push_str("\n\n");
    }
    md.push_str("## Built-ins\n\n");
    for f in stdlib::BUILTINS.iter().filter(|f| !stdlib::internal_builtin(f.name)) {
        md.push_str(&format!("- `{}`: {}\n", std_signature(f), f.doc));
    }
    md.push_str("\n## Keywords and constants\n\n| | |\n|---|---|\n");
    for (_, sig, d) in stdlib::KEYWORDS {
        md.push_str(&format!("| `{}` | {} |\n", sig.replace('|', "\\|"), d.replace('|', "\\|")));
    }
    md
}

/// All the names `nalu doc` knows (to suggest when it finds none).
fn doc_names() -> Vec<String> {
    let mut v: Vec<String> = stdlib::modules().into_iter().map(String::from).collect();
    v.extend(stdlib::STDLIB.iter().map(|f| format!("{}.{}", f.module, f.name)));
    v.extend(stdlib::METHODS.iter().map(|m| format!("{}.{}", m.recv, m.name)));
    v.extend(stdlib::PATH_PROPS.iter().map(|(n, _, _)| format!("path.{}", n)));
    v.extend(stdlib::KEYWORDS.iter().map(|(n, _, _)| n.to_string()));
    v.extend(["str", "int", "bool", "path", "duration", "secret", "list", "map", "enum", "error"].iter().map(|s| s.to_string()));
    v
}

/// The documentation of a name: module (`fs`), function (`fs.copy`), type (`str`, `list`), member
/// (`str.split`, `path.parent`, `err.code`), builtin (`print`) or keyword (`catch`). Empty: the summary.
/// `Err` carries the message for the user (with a suggestion).
pub fn doc(query: &str) -> Result<String, String> {
    let q = query.trim().trim_end_matches("()");
    if q.is_empty() {
        return Ok(overview());
    }
    if stdlib::module_doc(q).is_some() {
                // `path` is both a module and a type: the two pages
        return Ok(match doc_type(q) {
            Some(t) => format!("{}\n\n{}", module_page(q), type_page(q, &t)),
            None => module_page(q),
        });
    }
    if let Some(t) = doc_type(q) {
        return Ok(type_page(q, &t));
    }
    if let Some(f) = stdlib::find_builtin(q).filter(|f| !stdlib::internal_builtin(f.name)) {
        return Ok(format!("## {}\n\n{}\n\n{}", f.name, code(&std_signature(f)), f.doc));
    }
    if let Some((owner, member)) = q.split_once('.') {
        if let Some(f) = stdlib::find_fn(owner, member) {
            let planned = "";
            return Ok(format!("## {}.{}\n\n{}\n\n{}{}{}", f.module, f.name, code(&std_signature(f)), f.doc, flags_md(f.fallible, f.writes), planned));
        }
        if let Some(t) = doc_type(owner) {
            if member == "len" && matches!(t, Ty::Str | Ty::Path | Ty::List(_) | Ty::Map(_, _)) {
                let what = if matches!(t, Ty::Str | Ty::Path) { "The number of characters." } else { "The number of items." };
                return Ok(format!("## {}.len\n\n{}\n\n{}", owner, code(&format!("{}.len() -> int", tyname(&t))), what));
            }
            if t == Ty::Error && (member == "message" || member == "code") {
                let d = if member == "message" { "The failure's message (what `{err}` shows)." } else { "The exit code: 1 by default; for commands, the command's own code (127 if it was not found)." };
                let ty = if member == "message" { "str" } else { "int" };
                return Ok(format!("## err.{}\n\n{}\n\n{}", member, code(&format!("err.{}: {}", member, ty)), d));
            }
            if t == Ty::Path {
                if let Some((n, ty, d)) = stdlib::PATH_PROPS.iter().find(|(n, _, _)| *n == member) {
                    return Ok(format!("## path.{}\n\n{}\n\n{}{}", n, code(&format!("path.{}: {}", n, ty)), d, flags_md(false, false)));
                }
            }
            if let Some(m) = check::method_cat(&t).and_then(|c| stdlib::find_method(c, member)) {
                let extra = if m.mutates { format!("\n\n{}", MUTATES) } else { String::new() };
                return Ok(format!("## {}.{}\n\n{}\n\n{}{}{}", owner, m.name, code(&method_signature(m, &t)), m.doc, extra, flags_md(m.fallible, false)));
            }
        }
    }
    if let Some((sig, d)) = stdlib::keyword_doc(q) {
        return Ok(format!("## {}\n\n{}\n\n{}", q, code(sig), d));
    }
    let hint = match check::suggest(q, doc_names()) {
        Some(s) => format!("; did you mean `{}`?", s),
        None => "; run `nalu doc` for the list of modules, types and keywords".to_string(),
    };
    Err(format!("nothing named `{}` in Nalu{}", q, hint))
}

// =================================================================================================
// Context for AI assistants (`nalu ide context`)
// =================================================================================================

/// Context for an assistant that completes code (Copilot, via the extension): fixed language rules and
/// snippets about the cursor position. `importance` goes from 0 to 100.
pub struct AiContext {
    pub traits: Vec<(&'static str, &'static str)>,
    pub snippets: Vec<(&'static str, String, u8)>,
}

/// The rules a model that has never seen Nalu gets wrong most.
pub const AI_TRAITS: &[(&str, &str)] = &[
    ("Language", "This file is Nalu, a small typed scripting language that compiles to Bash and PowerShell. It is not Rust, Swift or JavaScript: use only Nalu syntax and the Nalu standard library."),
    ("Failures", "Anything that can fail stops the script (or makes the enclosing function fail) unless handled with `catch`: `x catch default`, `x catch fail \"message\"`, `x catch ignore`, `x catch { … err … }`, or `try { … } catch …` for several lines. There is no `?`, no null and no exceptions to declare. Logical or is `||`, and is `&&`."),
    ("Commands", "A line that starts with `$ ` runs a program: `$ git push origin {branch}`. `$(git rev-parse HEAD)` captures stdout as `str`. `{x}` inside a command is exactly one argument; there is no globbing and no `$VAR`."),
    ("Syntax", "`let` is immutable, `var` is mutable; `fn name(a: int, b: str = \"x\") -> str { }`; types like Go: `[]str`, `map[str]int`; `\"text {expr}\"` interpolates and `'raw'` does not; `if` and `match` are expressions; no semicolons, `++` or `? :`; names in snake_case, types in PascalCase; no imports: modules like fs, env, path, http, json, log and time are always available."),
];

/// A short, idiomatic script, for when the file still has too little code to serve as an example.
pub const AI_PRIMER: &str = r#"// Idiomatic Nalu (example; not part of this file)
/// where to install
param dest: path = "~/.local/bin"
/// version to install; empty means the latest release
param version: str = ""

enum Channel { stable, beta }

struct Release {
    tag: str
    url: str
}

fn latest(channel: Channel) -> Release {
    let body = http.get("https://api.example.com/releases/{channel}")
    return Release { tag: json.get(body, "tag"), url: json.get(body, "assets[0].url") }
}

let rel = if version != "" { Release { tag: version, url: "https://example.com/{version}.tar.gz" } } else { latest(Channel.stable) }
let tmp = fs.temp_dir()
defer fs.rm(tmp)
http.download(rel.url, tmp / "app.tar.gz")
archive.tar_extract(tmp / "app.tar.gz", tmp)
fs.mkdir(dest)
fs.copy(tmp / "app", dest / "app")
fs.chmod(dest / "app", "+x")
let shell = env.get("SHELL") catch "sh"
$ git -C {dest} log -1 --format=%h catch ignore
log.info("installed {rel.tag} in {dest} ({shell})")
"#;

/// Short declaration of a visible variable, as a Nalu stub.
fn var_stub(a: &Analysis, id: usize) -> String {
    let v = &a.prog.vars[id];
    let t = tyname(&v.ty);
    match v.kind {
        VarKind::ScriptParam => {
            let d = a.prog.params.iter().find(|tp| tp.var == id).and_then(|tp| tp.default.as_ref()).map(|d| format!(" = {}", src_of(&a.src, d.span))).unwrap_or_default();
            format!("param {}: {}{}", v.name, t, d)
        }
        VarKind::Global | VarKind::Local => format!("{} {}: {}", if v.mutable { "var" } else { "let" }, v.name, t),
        VarKind::FnParam => format!("let {}: {}  // parameter", v.name, t),
        VarKind::Loop => format!("let {}: {}  // loop variable", v.name, t),
        VarKind::LambdaParam => format!("let {}: {}  // lambda parameter", v.name, t),
        VarKind::Err => "let err: error  // the failure being handled: err.message, err.code".to_string(),
        VarKind::SelfParam => format!("let self: {}", t),
    }
}

pub fn ai_context(src: &str, root: &Path, offset: usize) -> AiContext {
    let offset = offset.min(src.len());
    let mut snippets = Vec::new();
    if let Some(a) = analyze(src, root) {
        let p = &a.prog;
                // visible names, functions, structs and enums (from the file and the package)
        let mut scope: Vec<String> = visible_vars(&a, offset).into_iter().take(40).map(|id| var_stub(&a, id)).collect();
        for (i, _) in p.fns.iter().enumerate().filter(|(_, f)| f.method_of.is_none()).take(30) {
            let fails = if p.fallible.get(i).copied().unwrap_or(false) { "  // can fail" } else { "" };
            scope.push(format!("{}{}", fn_signature(&a, i), fails));
        }
        for s in &p.structs {
            let fields: Vec<String> = s.fields.iter().map(|f| format!("{}: {}", f.name, tyname(&f.ty))).collect();
            let methods: Vec<String> = p
                .fns
                .iter()
                .enumerate()
                .filter(|(_, f)| f.method_of.as_deref() == Some(s.name.as_str()))
                .map(|(i, _)| fn_signature(&a, i).replacen(&format!("fn {}.", s.name), "", 1))
                .collect();
            let m = if methods.is_empty() { String::new() } else { format!("  // methods: {}", methods.join(", ")) };
            scope.push(format!("struct {} {{ {} }}{}", s.name, fields.join(", "), m));
        }
        for e in &p.enums {
            scope.push(format!("enum {} {{ {} }}", e.name, e.cases.join(", ")));
        }
        if !scope.is_empty() {
            snippets.push(("scope", format!("// Nalu: names visible at the cursor\n{}\n", scope.join("\n")), 80));
        }
                // the part of the stdlib that matters here: modules used in the file (and the basics) and the types' methods for visible types
        let mut modules: Vec<String> = vec!["fs".into(), "env".into(), "log".into()];
        for (_, s) in &a.index.syms {
            let m = match s {
                Sym::Module(m) => m.clone(),
                Sym::Std(f) => f.module.to_string(),
                _ => continue,
            };
            if !modules.contains(&m) {
                modules.push(m);
            }
        }
        let mut lines = vec!["// Nalu standard library (always available, no imports). \"can fail\" = handle it with `catch`.".to_string()];
        for m in &modules {
            for f in stdlib::STDLIB.iter().filter(|f| &f.module == m && f.v1) {
                let tag = if f.fallible { " (can fail)" } else { "" };
                lines.push(format!("// {}{}: {}", std_signature(f), tag, first_sentence(f.doc)));
            }
        }
        let mut cats: Vec<&'static str> = Vec::new();
        for id in visible_vars(&a, offset) {
            let t = &p.vars[id].ty;
            if let Some(c) = check::method_cat(t) {
                if matches!(c, "str" | "list" | "map") && !cats.contains(&c) {
                    cats.push(c);
                }
            }
            if *t == Ty::Path && !cats.contains(&"path") {
                cats.push("path");
            }
        }
        for c in &cats {
            if *c == "path" {
                let props: Vec<String> = stdlib::PATH_PROPS.iter().map(|(n, ty, _)| format!(".{}: {}", n, ty)).collect();
                lines.push(format!("// path properties (no parentheses): {}; `a / \"b\"` joins paths", props.join(", ")));
                continue;
            }
            let recv = match *c {
                "str" => Ty::Str,
                "list" => Ty::list(Ty::Struct("T".into())),
                _ => Ty::map(Ty::Struct("K".into()), Ty::Struct("V".into())),
            };
            let ms: Vec<String> = stdlib::METHODS.iter().filter(|m| m.recv == *c).map(|m| method_signature(m, &recv).splitn(2, '.').nth(1).unwrap_or("").to_string()).collect();
            lines.push(format!("// {} methods: len() -> int, {}", tyname(&recv), ms.join(", ")));
        }
        snippets.push(("stdlib", lines.join("\n") + "\n", 70));
    }
        // short file: an idiomatic example helps the model pick up the style
    if src.lines().filter(|l| !l.trim().is_empty()).count() < 40 {
        snippets.push(("primer", AI_PRIMER.to_string(), 50));
    }
    AiContext { traits: AI_TRAITS.to_vec(), snippets }
}

// =================================================================================================
// Signature help
// =================================================================================================

pub struct SigHelp {
    pub label: String,
        /// `[start, end)` of each parameter within `label` (in characters).
    pub params: Vec<(usize, usize)>,
    pub active: usize,
    pub doc: String,
}

pub fn signature(src: &str, root: &Path, offset: usize) -> Option<SigHelp> {
    let offset = offset.min(src.len());
    if !in_code(src, offset) {
        return None;
    }
    let (toks, _) = lexer::lex(src, 0);
    let before: Vec<&lexer::Token> = toks.iter().filter(|t| t.span.end <= offset && !matches!(t.tok, Tok::Eof)).collect();
        // the nearest open `(`, counting commas at its level
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut named: Option<String> = None;
    let mut open = None;
    for (i, t) in before.iter().enumerate().rev() {
        match t.tok {
            Tok::RParen | Tok::RBracket | Tok::RBrace => depth += 1,
            Tok::LBracket | Tok::LBrace => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            Tok::LParen => {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
            Tok::Comma if depth == 0 => commas += 1,
            Tok::Colon if depth == 0 && commas == 0 && named.is_none() => {
                if let Some(Tok::Ident(n)) = before.get(i.wrapping_sub(1)).map(|t| &t.tok) {
                    named = Some(n.clone());
                }
            }
            _ => {}
        }
    }
        // named argument after the last comma: `f(a, every: |`
    if named.is_none() {
        let start = open? + 1;
        let mut d = 0i32;
        let mut last_comma = start;
        for (i, t) in before.iter().enumerate().skip(start) {
            match t.tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => d += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => d -= 1,
                Tok::Comma if d == 0 => last_comma = i + 1,
                _ => {}
            }
        }
        if let (Some(Tok::Ident(n)), Some(Tok::Colon)) = (before.get(last_comma).map(|t| &t.tok), before.get(last_comma + 1).map(|t| &t.tok)) {
            named = Some(n.clone());
        }
    }
    let open = open?;
    let callee = before.get(open.checked_sub(1)?)?;
    let dot_before = open >= 2 && matches!(before[open - 2].tok, Tok::Dot);
    let (label_name, params, ret, doc): (String, Vec<String>, String, String) = match &callee.tok {
        Tok::Kw(lexer::Kw::Retry) => ("retry".into(), vec!["times: int".into(), "every: duration = 1s".into()], String::new(), stdlib::keyword_doc("retry").map(|k| k.1.to_string()).unwrap_or_default()),
        Tok::Kw(lexer::Kw::Parallel) => ("parallel".into(), vec!["max: int".into()], String::new(), stdlib::keyword_doc("parallel").map(|k| k.1.to_string()).unwrap_or_default()),
        Tok::Ident(name) => {
            let module = if dot_before { before.get(open.wrapping_sub(3)).and_then(|t| if let Tok::Ident(m) = &t.tok { Some(m.clone()) } else { None }) } else { None };
            let module_ok = module.as_deref().map(|m| stdlib::module_doc(m).is_some()).unwrap_or(false);
            if let (true, Some(m)) = (module_ok, module.as_deref()) {
                let f = stdlib::find_fn(m, name)?;
                (format!("{}.{}", m, f.name), params_text(f.params, None), ret_text(f.ret, None), std_doc(f))
            } else {
                                // user function, method or builtin: by the symbol at the name (closing the `(` if needed)
                let a = analyze(src, root);
                let found = a.as_ref().and_then(|a| sym_at(&a.index, callee.span.start).map(|(_, s)| s.clone()));
                let (a, sym) = match found {
                    Some(s) => (a?, s),
                    None => {
                        let mut patched = src.to_string();
                        patched.insert(offset, ')');
                        let a2 = analyze(&patched, root)?;
                        let s = sym_at(&a2.index, callee.span.start).map(|(_, s)| s.clone())?;
                        (a2, s)
                    }
                };
                match sym {
                    Sym::Fn(i) => {
                        let f = &a.prog.fns[i];
                        let ps = f
                            .params
                            .iter()
                            .map(|p| match &p.default {
                                Some(d) => format!("{}: {} = {}", p.name, tyname(&p.ty), src_of(&a.src, d.span)),
                                None => format!("{}: {}", p.name, tyname(&p.ty)),
                            })
                            .collect();
                        let ret = if f.ret == Ty::Unit { String::new() } else { format!(" -> {}", tyname(&f.ret)) };
                        let (_, doc) = split_md(&describe(&a, &Sym::Fn(i)));
                        (f.name.clone(), ps, ret, doc)
                    }
                    Sym::Builtin(b) => {
                        let f = stdlib::find_builtin(b)?;
                        (f.name.to_string(), params_text(f.params, None), ret_text(f.ret, None), f.doc.to_string())
                    }
                    Sym::Method { recv, name } => {
                        if name == "len" {
                            return None;
                        }
                        let m = stdlib::find_method(check::method_cat(&recv)?, &name)?;
                        (format!("{}.{}", tyname(&recv), m.name), params_text(m.params, Some(&recv)), ret_text(m.ret, Some(&recv)), m.doc.to_string())
                    }
                    _ => return None,
                }
            }
        }
        _ => return None,
    };
    let mut label = format!("{}(", label_name);
    let mut spans = Vec::new();
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let s = label.chars().count();
        label.push_str(p);
        spans.push((s, label.chars().count()));
    }
    label.push(')');
    label.push_str(&ret);
    let mut active = match &named {
        Some(n) => params.iter().position(|p| p.split(':').next() == Some(n.as_str())).unwrap_or(commas),
        None => commas,
    };
    if !params.is_empty() {
        let variadic = params.last().map(|p| p.contains("any...")).unwrap_or(false);
        if active >= params.len() {
            active = if variadic { params.len() - 1 } else { active };
        }
    }
    Some(SigHelp { label, params: spans, active, doc })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new(".")
    }

        /// `|` marks the cursor.
    fn at(src: &str) -> (String, usize) {
        let i = src.find('|').expect("cursor");
        (src.replacen('|', "", 1), i)
    }

    fn hover_md(src: &str) -> String {
        let (s, i) = at(src);
        hover(&s, root(), i).map(|h| h.markdown).unwrap_or_default()
    }

    fn labels(src: &str) -> Vec<String> {
        let (s, i) = at(src);
        complete(&s, root(), i).items.into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn hover_shows_types() {
        assert!(hover_md("let nomes = [\"a\"]\nprint(no|mes.len())").contains("let nomes: []str"));
        assert!(hover_md("var n = 1\nn += 1\nprint(|n)").contains("var n: int"));
        assert!(hover_md("let t = fs.re|ad(\"x\") catch \"\"\nprint(t)").contains("fs.read(p: path) -> str"));
        assert!(hover_md("let t = fs.re|ad(\"x\") catch \"\"\nprint(t)").contains("Can fail"));
        assert!(hover_md("fs.wr|ite(\"x\", \"y\")").contains("Writes"));
        assert!(hover_md("let p: path = \"a/b.txt\"\nprint(p.st|em)").contains("path.stem: str"));
        assert!(hover_md("let xs = [\"b\"]\nprint(xs.fi|rst() catch \"\")").contains("[]str.first() -> str"));
        assert!(hover_md("let m: map[str]int = {}\nprint(m.ke|ys().len())").contains("-> []str"));
    }

    #[test]
    fn hover_of_functions_structs_and_params() {
        let src = "/// onde instalar\nparam destino: path = \"~/x\"\n\n/// soma dois\nfn soma(a: int, b: int = 2) -> int {\n    return a + b\n}\nstruct P {\n    nome: str\n    fn oi() -> str {\n        return self.nome\n    }\n}\nenum Cor { verde, azul }\nlet c = Cor.verde\nlet p = P { nome: \"x\" }\nprint(soma(1), destino, p.nome, p.oi(), c)\nfn falha() {\n    $ git push\n}\nfalha()\n";
        let h = |needle: &str, k: usize| {
            let i = src.find(needle).unwrap() + k;
            hover(src, root(), i).map(|h| h.markdown).unwrap_or_default()
        };
        assert!(h("soma(1)", 1).contains("fn soma(a: int, b: int = 2) -> int"));
        assert!(h("soma(1)", 1).contains("soma dois"));
        assert!(h("destino, p", 1).contains("param destino: path = \"~/x\""));
        assert!(h("destino, p", 1).contains("--destino"));
        assert!(h("p.nome,", 3).contains("nome: str"));
        assert!(h("p.oi()", 3).contains("fn P.oi() -> str"));
        assert!(h("Cor.verde", 1).contains("enum Cor { verde, azul }"));
        assert!(h("Cor.verde", 5).contains("Cor.verde"));
        assert!(h("P { nome", 0).contains("struct P {"));
        assert!(h("falha()\n", 1).contains("Can fail"));
        assert!(h("falha()\n", 1).contains("`$ git`"));
    }

    #[test]
    fn hover_of_keywords_and_modules() {
        assert!(hover_md("let x = env.get(\"A\") cat|ch \"b\"\nprint(x)").contains("Handles a failure"));
        assert!(hover_md("de|fer fs.rm(\"x\")").contains("when the function or script ends"));
        assert!(hover_md("let h = di|rs.home()\nprint(h)").contains("module dirs"));
        assert!(hover_md("let x: i|nt = 1\nprint(x)").contains("64-bit"));
    }

    #[test]
    fn hover_works_with_syntax_error() {
                // line 2 is broken; line 3 still has types
        assert!(hover_md("let nomes = [\"a\"]\nlet x = )\nprint(no|mes)").contains("[]str"));
    }

    #[test]
    fn completes_members_by_type() {
        let l = labels("let s = \"a,b\"\nlet partes = s.|\n");
        assert!(l.contains(&"split".to_string()) && l.contains(&"len".to_string()) && !l.contains(&"push".to_string()), "{:?}", l);
        let l = labels("var xs = [1, 2]\nxs.p|");
        assert!(l.contains(&"push".to_string()) && l.contains(&"pop".to_string()), "{:?}", l);
        let l = labels("let p: path = \"a\"\nprint(p.|)");
        assert!(l.contains(&"parent".to_string()) && !l.contains(&"exists".to_string()) && l.contains(&"trim".to_string()), "{:?}", l);
        let l = labels("let x = fs.|");
        assert!(l.contains(&"read".to_string()) && l.contains(&"mkdir".to_string()), "{:?}", l);
        let l = labels("enum Cor { verde, azul }\nlet c = Cor.|");
        assert!(l == vec!["verde", "azul", "parse"], "{:?}", l);
        let l = labels("struct P {\n    nome: str\n    fn oi() -> str {\n        return self.|\n    }\n}");
        assert!(l.contains(&"nome".to_string()) && l.contains(&"oi".to_string()), "{:?}", l);
                // chain: the type of the split result
        let l = labels("let s = \"a\"\nlet n = s.split(\",\").|\n");
        assert!(l.contains(&"join".to_string()) && !l.contains(&"split".to_string()), "{:?}", l);
                // `a + b.` completes `b`
        let l = labels("let a = 1\nlet b = \"x\"\nprint(a, a + b.|)");
        assert!(l.contains(&"upper".to_string()), "{:?}", l);
                // `err.` inside the handler
        let l = labels("let x = fs.read(\"a\") catch {\n    print(err.|)\n    \"\"\n}\nprint(x)");
        assert_eq!(l, vec!["message", "code"]);
                // method on a following line: `xs\n    .`
        let l = labels("let xs = [\"a\"]\nlet ys = xs\n    .|\nprint(ys)");
        assert!(l.contains(&"filter".to_string()), "{:?}", l);
    }

    #[test]
    fn completes_visible_names() {
        let l = labels("let antes = 1\nfn f(a: int) {\n    let dentro = a\n    print(|)\n}\nlet depois = 2\nprint(antes, depois)");
        assert!(l.contains(&"a".to_string()) && l.contains(&"dentro".to_string()) && l.contains(&"antes".to_string()), "{:?}", l);
        assert!(l.contains(&"f".to_string()) && l.contains(&"fs".to_string()) && l.contains(&"print".to_string()));
        let l = labels("for item in [\"a\"] {\n    print(it|)\n}\nprint(|)");
        assert!(l.contains(&"item".to_string()), "{:?}", l);
        let (s, i) = at("for item in [\"a\"] {\n    print(item)\n}\nprint(|)");
        let l: Vec<String> = complete(&s, root(), i).items.into_iter().map(|i| i.label).collect();
        assert!(!l.contains(&"item".to_string()), "outside the for: {:?}", l);
        let l = labels("let x = 1\nlet y = |\nprint(x, y)");
        assert!(l.contains(&"x".to_string()) && !l.contains(&"y".to_string()), "{:?}", l);
    }

    #[test]
    fn completes_cases_in_match() {
        let l = labels("enum Cor { verde, azul, roxo }\nlet c = Cor.verde\nlet n = match c {\n    verde => 1\n    |\n}\nprint(n)");
        assert_eq!(l, vec!["azul", "roxo", "_"]);
        let l = labels("let ok = true\nmatch ok {\n    |\n}");
        assert_eq!(l, vec!["true", "false", "_"]);
    }

    #[test]
    fn completes_types_in_annotations() {
        for src in ["param nome: |", "let x: |", "fn f(a: int, b: |) {\n}", "struct P {\n    nome: |\n}", "fn f() -> |"] {
            let l = labels(src);
            assert!(l.contains(&"str".to_string()) && l.contains(&"duration".to_string()), "{} → {:?}", src, l);
        }
        let l = labels("retry(3, every: |) {\n}");
        assert!(!l.contains(&"duration".to_string()), "named argument is not a type: {:?}", l);
    }

    #[test]
    fn does_not_complete_in_text_and_comments() {
        assert!(labels("let s = \"abc |\"").is_empty());
        assert!(labels("// comentário |").is_empty());
        assert!(labels("$ git push |").is_empty());
        assert!(!labels("let n = 1\nlet s = \"{|}\"\nprint(s)").is_empty());
        assert!(!labels("let n = 1\n$ echo {|}").is_empty());
    }

    #[test]
    fn signatures() {
        let (s, i) = at("fs.copy(\"a\", |)");
        let h = signature(&s, root(), i).unwrap();
        assert_eq!(h.label, "fs.copy(from: path, to: path)");
        assert_eq!(h.active, 1);
        let (s, i) = at("let n = http.get(\"u\", timeout: 5s, bearer: |)\nprint(n)");
        let h = signature(&s, root(), i).unwrap();
        assert_eq!(&h.label[h.params[h.active].0..h.params[h.active].1], "bearer: secret = \"\"");
        let (s, i) = at("fn soma(a: int, b: int) -> int {\n    return a + b\n}\nprint(soma(1, |))");
        let h = signature(&s, root(), i).unwrap();
        assert_eq!(h.label, "soma(a: int, b: int) -> int");
        assert_eq!(h.active, 1);
        let (s, i) = at("let xs = [\"a\"]\nlet j = xs.join(|)\nprint(j)");
        assert_eq!(signature(&s, root(), i).unwrap().label, "[]str.join(sep: str) -> str");
        let (s, i) = at("print(1, 2, |)");
        let h = signature(&s, root(), i).unwrap();
        assert_eq!(h.active, 0, "variadic stays on the last parameter");
        let (s, i) = at("retry(3, |) {\n}");
        assert_eq!(signature(&s, root(), i).unwrap().active, 1);
    }

    #[test]
    fn documentation() {
        assert!(doc("fs").unwrap().contains("`fs.copy(from: path, to: path)` (can fail, writes)"));
        assert!(doc("fs.read").unwrap().contains("Can fail"));
        assert!(doc("str.split").unwrap().contains("str.split(sep: str) -> []str"));
        assert!(doc("list").unwrap().contains("[]T.push(x: T)"));
        assert!(doc("path.parent").unwrap().contains("path.parent: path"));
        assert!(doc("path").unwrap().contains("path.trim() -> str"), "str methods apply to path");
        assert!(doc("err.code").unwrap().contains("err.code: int"));
        assert!(doc("catch").unwrap().contains("Handles a failure"));
        assert!(doc("print").unwrap().contains("print(items: any...)"));
        assert!(doc("pkg.install").unwrap().contains("Planned"));
        assert!(doc("regex.find").unwrap().contains("single way to match"));
        assert!(doc("").unwrap().contains("## Modules"));
        assert_eq!(doc("fs.reed").unwrap_err(), "nothing named `fs.reed` in Nalu; did you mean `fs.read`?");
                // every known name has a page
        for n in doc_names() {
            assert!(doc(&n).is_ok(), "{}", n);
        }
    }

    #[test]
    fn context_for_ai() {
        let (s, i) = at("param destino: path = \"~/x\"\nlet nomes = [\"a\"]\nfn publicar(n: str) {\n    $ git push {n}\n}\nfor n in nomes {\n    |\n}\nlet depois = 1\nprint(depois)");
        let c = ai_context(&s, root(), i);
        assert!(c.traits.iter().any(|(n, _)| *n == "Failures"));
        let scope = &c.snippets.iter().find(|s| s.0 == "scope").unwrap().1;
        assert!(scope.contains("param destino: path = \"~/x\""), "{}", scope);
        assert!(scope.contains("let nomes: []str") && scope.contains("let n: str  // loop variable"), "{}", scope);
        assert!(scope.contains("fn publicar(n: str)  // can fail"), "{}", scope);
        assert!(!scope.contains("depois"), "not declared yet: {}", scope);
        let lib = &c.snippets.iter().find(|s| s.0 == "stdlib").unwrap().1;
        assert!(lib.contains("fs.read(p: path) -> str (can fail)") && lib.contains("[]T methods: len() -> int, push(x: T)"), "{}", lib);
        assert!(lib.contains("path properties"), "{}", lib);
        assert!(c.snippets.iter().any(|s| s.0 == "primer"));
                // the context example is valid, formatted Nalu
        assert!(crate::fmt::format(AI_PRIMER).map(|f| f == AI_PRIMER).unwrap_or(false), "the primer must be formatted");
        let a = analyze(AI_PRIMER, root()).unwrap();
        let _ = a;
        let (_, diags) = crate::parser::parse(AI_PRIMER);
        assert!(diags.is_empty(), "{:?}", diags);
        let c = crate::compile(AI_PRIMER, "primer.nalu", root(), crate::Target::Linux);
        assert!(c.diags.iter().all(|d| !d.is_error()), "{:?}", c.diags);
    }

    #[test]
    fn definitions() {
        let src = "struct P {\n    nome: str\n}\nfn f(x: int) -> int {\n    return x\n}\nlet p = P { nome: \"a\" }\nprint(f(1), p.nome)\n";
        let def = |needle: &str, k: usize| {
            let i = src.rfind(needle).unwrap() + k;
            definition(src, root(), i).map(|s| &src[s.start..s.end]).unwrap_or("")
        };
        assert_eq!(def("f(1)", 0), "f");
        assert_eq!(definition(src, root(), src.rfind("f(1)").unwrap()).unwrap().start, src.find("f(x").unwrap());
        assert_eq!(definition(src, root(), src.rfind("p.nome").unwrap()).unwrap().start, src.find("p = P").unwrap());
        assert_eq!(definition(src, root(), src.rfind("nome)").unwrap()).unwrap().start, src.find("nome: str").unwrap());
        assert_eq!(definition(src, root(), src.rfind("P {").unwrap()).unwrap().start, src.find("P {\n").unwrap());
        assert_eq!(definition(src, root(), src.find("return x").unwrap() + 7).unwrap().start, src.find("x: int").unwrap());
    }

    #[test]
    fn examples_have_hover_on_every_name() {
                // each identifier in the examples has a symbol or is a known keyword/constant
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut checked = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
                        // `x_test.nalu` only makes sense together with `x.nalu` (the package)
            if p.extension().map(|x| x != "nalu").unwrap_or(true) || p.to_string_lossy().ends_with("_test.nalu") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            let a = analyze(&src, &dir).unwrap();
            let (toks, _) = lexer::lex(&src, 0);
            for t in &toks {
                if let Tok::Ident(n) = &t.tok {
                    let has = sym_at(&a.index, t.span.start).is_some() || stdlib::keyword_doc(n).is_some();
                                // named argument names (`every:`, `timeout:`) are not symbols
                    let named_arg = src[t.span.end..].trim_start().starts_with(':') && !src[..t.span.start].trim_end().ends_with("param") && !src[..t.span.start].trim_end().ends_with("let");
                    assert!(has || named_arg, "{}: no hover on `{}` (byte {})", p.display(), n, t.span.start);
                    checked += 1;
                }
            }
        }
        assert!(checked > 200, "{}", checked);
    }
}
