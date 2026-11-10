//! Checker (SPEC-001): AST → typed, resolved tree (`TProgram`) + diagnostics.
//!
//! - Resolves names (variables with unique `VarId`s, functions, methods, stdlib, types), checks types and units,
//!   fills in named arguments and defaults (every `TCall.args` comes out positional and complete), checks
//!   `match` exhaustiveness, `let` × `var`, `secret`, `parallel`, and misplaced `err`/`self`/`break`.
//! - **Fallibility**: each node knows whether it can fail; `TProgram.fallible` holds the per-function inference
//!   (fixed point, starting from `false`). `catch` after something infallible is `E040` (checked after the fixed point).
//! - Unknown types (after an already reported error) use an internal marker (`unk()`) that is
//!   compatible with everything, to avoid cascades of errors.
//! - Frequent error messages live in the [`msg`] module, in one place.

use crate::ast::{self, ExprKind as EK, StmtKind as SK};
use crate::diag::{Diag, Span};
use crate::ir::{BinOp, Ty, UnOp};
use crate::stdlib::{self, StdFn, StdParam};
use std::collections::{HashMap, HashSet};
use std::path::Path;

// =================================================================================================
// Typed tree
// =================================================================================================

pub type VarId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    /// `let`/`var` right at the top level (visible in functions).
    Global,
    ScriptParam,
    Local,
    FnParam,
    SelfParam,
    Loop,
    Err,
    LambdaParam,
}

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
    pub kind: VarKind,
    pub span: Span,
    pub used: bool,
    /// Span of the source where the name is visible, `[start, end)` (for editor completion).
    pub vis: (usize, usize),
}

#[derive(Clone, Debug)]
pub struct StructInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Ty,
    pub default: Option<TExpr>,
    /// The field name in the declaration.
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct EnumInfo {
    pub name: String,
    pub cases: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    /// Name in the IR: `ler_versao` or `Versao.proxima`.
    pub name: String,
    /// Declared parameters (without `self`).
    pub params: Vec<PInfo>,
    pub ret: Ty,
    pub method_of: Option<String>,
    pub span: Span,
    /// The name in the declaration (`fn name`).
    pub name_span: Span,
    pub doc: String,
}

#[derive(Clone, Debug)]
pub struct PInfo {
    pub name: String,
    pub ty: Ty,
    pub default: Option<TExpr>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TFunc {
    /// For methods, the first one is `self`.
    pub params: Vec<VarId>,
    pub body: Vec<TStmt>,
}

#[derive(Clone, Debug)]
pub struct TParam {
    pub var: VarId,
    pub ty: Ty,
    pub default: Option<TExpr>,
    pub doc: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TTest {
    pub name: String,
    pub body: Vec<TStmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TProgram {
    pub structs: Vec<StructInfo>,
    pub enums: Vec<EnumInfo>,
    pub fns: Vec<FnInfo>,
    /// Aligned with `fns`.
    pub funcs: Vec<TFunc>,
    pub params: Vec<TParam>,
    pub body: Vec<TStmt>,
    pub tests: Vec<TTest>,
    pub vars: Vec<VarInfo>,
    /// Fallibility inference per function (aligned with `fns`).
    pub fallible: Vec<bool>,
    /// What can fail in each function (for `--explain`).
    pub reasons: Vec<Vec<(String, Span)>>,
    /// Does the script use `args`?
    pub uses_args: bool,
}

/// What a source name is. Editors use it for hover, completion and go to definition.
#[derive(Clone, Debug)]
pub enum Sym {
    Var(VarId),
    /// User function or method (index into `TProgram.fns`).
    Fn(usize),
    Std(&'static StdFn),
    /// `print`, `fmt`, `secret`, `embed`, `exit`, `os`, `dry_run`, `args`, `err`.
    Builtin(&'static str),
    /// Built-in method (`str`, lists, maps, enums, scalars), including `len`.
    Method { recv: Ty, name: String },
    Field { owner: String, name: String, ty: Ty },
    /// Property without parentheses of a built-in type: `p.name`, `err.message`.
    Prop { recv: Ty, name: String, ty: Ty },
    Struct(String),
    Enum(String),
    Case(String, String),
    Module(String),
    /// Built-in type name in an annotation (`str`, `int`…).
    Type(Ty),
}

/// Index for editors, built during checking: each name with what it is, and the type of each expression.
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub syms: Vec<(Span, Sym)>,
    pub exprs: Vec<(Span, Ty)>,
}

#[derive(Clone, Debug)]
pub struct TExpr {
    pub kind: TK,
    pub ty: Ty,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TK {
    Int(i64),
    Str(String),
    Bool(bool),
    Duration(i64),
    EnumCase(String, String),
    Interp(Vec<TPart>),
    Var(VarId),
    DryRun,
    Args,
    Os,
    CurrentErr,
    Field(Box<TExpr>, String),
    Index(Box<TExpr>, Box<TExpr>),
    Len(Box<TExpr>),
    Unary(UnOp, Box<TExpr>),
    Binary(BinOp, Box<TExpr>, Box<TExpr>),
    Call(Box<TCall>),
    List(Vec<TExpr>),
    Map(Vec<(TExpr, TExpr)>),
    /// Only the written fields; the lowering fills in the defaults (no `spread`).
    Struct { name: String, spread: Option<Box<TExpr>>, fields: Vec<(String, TExpr)> },
    If { cond: Box<TExpr>, then: TBlock, els: Option<TBlock> },
    Match(Box<TMatch>),
    Or(Box<TExpr>, Box<THandler>),
    Command(Box<TCmd>),
    Capture(Box<TCmd>),
    CmdTest(Box<TCmd>),
    Try(TBlock),
    Retry { times: Box<TExpr>, every: Box<TExpr>, body: TBlock },
    Parallel { max: Option<Box<TExpr>>, body: Box<TPar> },
    /// After an already reported error.
    Bad,
}

#[derive(Clone, Debug)]
pub enum TPart {
    Lit(String),
    Expr(TExpr),
}

#[derive(Clone, Debug)]
pub struct TCall {
    pub target: TTarget,
    /// Positional and complete; for methods, the receiver is `args[0]`.
    pub args: Vec<TArg>,
    pub ret: Ty,
    /// Static fallibility (stdlib/methods); for user functions, see `TProgram.fallible`.
    pub fallible: bool,
    /// For messages and `--explain`: "`fs.read`".
    pub desc: String,
}

#[derive(Clone, Debug)]
pub enum TTarget {
    User(usize),
    Std(&'static str, &'static str),
    Builtin(&'static str),
    Method { recv_ty: Ty, name: String },
}

#[derive(Clone, Debug)]
pub enum TArg {
    Expr(TExpr),
    Lambda(TLambda),
}

#[derive(Clone, Debug)]
pub struct TLambda {
    pub params: Vec<VarId>,
    pub ret: Ty,
    pub body: TLambdaBody,
}

#[derive(Clone, Debug)]
pub enum TLambdaBody {
    Expr(Box<TExpr>),
    Block(Vec<TStmt>),
}

#[derive(Clone, Debug, Default)]
pub struct TBlock {
    pub stmts: Vec<TStmt>,
    /// In value context: the last line. `None` when the block diverges (or is not in value context).
    pub value: Option<Box<TExpr>>,
}

#[derive(Clone, Debug)]
pub struct TMatch {
    pub subject: TExpr,
    pub arms: Vec<TArm>,
    pub default: Option<TBlock>,
    pub is_os: bool,
    pub exhaustive: bool,
    /// With guards, bindings or struct patterns: `arms` hold every arm in order (`_` as `TPattern::Any`)
    /// and `default` is `None`; the lowering tests them one by one (D33).
    pub ordered: bool,
}

#[derive(Clone, Debug)]
pub struct TArm {
    pub patterns: Vec<TPattern>,
    pub guard: Option<TExpr>,
    pub body: TBlock,
}

#[derive(Clone, Debug)]
pub enum TPattern {
    Lit(TExpr),
    Range { from: i64, to: i64, inclusive: bool },
    /// `_` (only in ordered matches and in struct fields).
    Any,
    /// A name that takes the value (the whole subject, or a struct field).
    Bind(VarId),
    /// `Type { field: pattern, … }`
    Struct(Vec<(String, TPattern)>),
}

impl TPattern {
    /// Matches every value.
    pub fn irrefutable(&self) -> bool {
        match self {
            TPattern::Any | TPattern::Bind(_) => true,
            TPattern::Struct(fs) => fs.iter().all(|(_, p)| p.irrefutable()),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum THandler {
    Ignore,
    Default(TExpr),
    Fail { message: TExpr, code: Option<TExpr> },
    Block { err: Option<VarId>, body: TBlock },
}

#[derive(Clone, Debug)]
pub struct TCmd {
    pub stages: Vec<Vec<TWord>>,
    /// `NAME=value` before the program of each stage (same length as `stages`).
    pub envs: Vec<Vec<(String, TWord)>>,
    /// Program position (first word) of each stage.
    pub prog_spans: Vec<Span>,
    pub redirects: Vec<TRedir>,
    pub stdin: Option<TExpr>,
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TWord {
    Lit(String),
    Interp(Vec<TPart>),
    Spread(TExpr),
}

#[derive(Clone, Debug)]
pub enum TRedir {
    Out { path: TExpr, append: bool },
    Err { path: TExpr, append: bool },
    In(TExpr),
    ErrToOut,
}

#[derive(Clone, Debug)]
pub enum TPar {
    For { var: VarId, list: TExpr, body: TBlock },
    Tasks(Vec<TStmt>),
}

#[derive(Clone, Debug)]
pub struct TStmt {
    pub kind: TS,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TS {
    Let { var: VarId, value: TExpr },
    Assign { target: TLValue, value: TExpr },
    Expr(TExpr),
    ForRange { var: VarId, from: TExpr, to: TExpr, inclusive: bool, body: Vec<TStmt> },
    ForList { var: VarId, index: Option<VarId>, list: TExpr, body: Vec<TStmt> },
    ForMap { key: VarId, value: VarId, map: TExpr, body: Vec<TStmt> },
    While { cond: TExpr, body: Vec<TStmt> },
    Fail { message: TExpr, code: Option<TExpr> },
    Return(Option<TExpr>),
    Break,
    Continue,
    Exit(Option<TExpr>),
    /// A `TK::Call` or `TK::Command`.
    Defer(TExpr),
    Native { bash: String, powershell: String },
    Assert { cond: TExpr, text: String },
    /// A `TK::Call` or `TK::Command`.
    AssertFails(TExpr),
}

#[derive(Clone, Debug)]
pub enum TLValue {
    Var(VarId),
    Field(VarId, String),
    Index(VarId, TExpr),
    MapEntry(VarId, TExpr),
}

// =================================================================================================
// Types: utilities
// =================================================================================================

/// Unknown type (after an error): compatible with everything.
pub fn unk() -> Ty {
    Ty::Struct(String::new())
}

pub fn is_unk(t: &Ty) -> bool {
    match t {
        Ty::Struct(n) => n.is_empty(),
        Ty::List(e) => is_unk(e),
        Ty::Map(k, v) => is_unk(k) || is_unk(v),
        _ => false,
    }
}

fn texty(t: &Ty) -> bool {
    matches!(t, Ty::Str | Ty::Path)
}

/// Does `got` fit where `want` is expected? (`str` ↔ `path` convert automatically.)
pub fn compat(want: &Ty, got: &Ty) -> bool {
    if is_unk(want) || is_unk(got) {
        return true;
    }
    match (want, got) {
        (a, b) if a == b => true,
        (Ty::Str, Ty::Path) | (Ty::Path, Ty::Str) => true,
        (Ty::List(a), Ty::List(b)) => compat(a, b),
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) => compat(k1, k2) && compat(v1, v2),
        _ => false,
    }
}

/// Can it become text in interpolation, `print`, `fmt`, `+`?
fn stringable(t: &Ty) -> bool {
    matches!(t, Ty::Str | Ty::Path | Ty::Int | Ty::Bool | Ty::Enum(_) | Ty::Duration | Ty::Error)
}

fn tdisp(t: &Ty) -> String {
    if is_unk(t) {
        "?".into()
    } else {
        t.display()
    }
}

/// Edit distance (Levenshtein).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The closest candidate (distance ≤ 2 and smaller than the name itself).
pub fn suggest<I: IntoIterator<Item = String>>(name: &str, cands: I) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for c in cands {
        if c == name || c.is_empty() {
            continue;
        }
        let d = edit_distance(name, &c);
        if d <= 2 && d < name.chars().count().max(2) && best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}


/// Frequent messages and help texts, in one place.
pub mod msg {
    use crate::ir::Ty;

    pub fn not_found(name: &str) -> String {
        format!("name `{}` does not exist", name)
    }
    pub fn did_you_mean(s: &str) -> String {
        format!("did you mean `{}`?", s)
    }
    pub fn let_immutable(name: &str) -> String {
        format!("`{}` is declared with `let` and cannot change", name)
    }
    pub fn use_var(name: &str) -> String {
        format!("declare it with `var {} = …` if it needs to change", name)
    }
    pub fn redeclared(name: &str) -> String {
        format!("`{}` is already declared in this block", name)
    }
    pub const REDECLARED_HELP: &str = "to change the value, declare it with `var` and assign it (`x = …`), or choose another name";
    pub fn reserved(name: &str) -> String {
        format!("`{}` is a reserved name in Nalu", name)
    }
    pub fn mismatch(what: &str, want: &Ty, got: &Ty) -> String {
        format!("{} expects `{}`, but got `{}`", what, super::tdisp(want), super::tdisp(got))
    }
    pub const SECRET_TEXT: &str = "a `secret` never becomes text";
    pub const SECRET_HELP: &str = "secrets only leave through a command's input (`token | $ cmd --password-stdin`) or to another function that takes a `secret`";
    pub const DURATION_UNIT_HELP: &str = "write the unit (`500ms`, `5s`, `2m`); for a number, divide: `d / 1s`";
    pub const LAMBDA_HELP: &str = "lambdas only appear as an argument to `filter`, `map`, `any`, `all`, `find` and `sort_by`";
}

// =================================================================================================
// Signatures
// =================================================================================================

#[derive(Clone, Debug)]
enum PT {
    Ty(Ty),
    /// Scalar that becomes text (`print`, `log`).
    Any,
    /// `(T) -> bool` (ret `Some`) / `(T) -> U` (ret `None`: inferred).
    Lambda { arg: Ty, ret: Option<Ty> },
}

#[derive(Clone, Debug)]
struct SP {
    name: String,
    ty: PT,
    default: Option<TExpr>,
}

#[derive(Clone, Debug)]
struct Sig {
    params: Vec<SP>,
    /// The last parameter is `...`.
    variadic: bool,
}

#[derive(Clone, Debug, Default)]
struct Bind {
    t: Option<Ty>,
    k: Option<Ty>,
    v: Option<Ty>,
    u: Option<Ty>,
    en: Option<Ty>,
}

fn table_ty(s: &str, b: &Bind) -> Ty {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("[]") {
        return Ty::list(table_ty(inner, b));
    }
    // `map[K]V`
    if let Some(rest) = s.strip_prefix("map[") {
        if let Some((k, v)) = rest.split_once(']') {
            return Ty::map(table_ty(k, b), table_ty(v, b));
        }
    }
    match s {
        "str" => Ty::Str,
        "int" => Ty::Int,
        "bool" => Ty::Bool,
        "path" => Ty::Path,
        "duration" => Ty::Duration,
        "secret" => Ty::Secret,
        "unit" => Ty::Unit,
        "T" => b.t.clone().unwrap_or_else(unk),
        "K" => b.k.clone().unwrap_or_else(unk),
        "V" => b.v.clone().unwrap_or_else(unk),
        "U" => b.u.clone().unwrap_or_else(unk),
        "Enum" => b.en.clone().unwrap_or_else(unk),
        _ => unk(),
    }
}

fn table_pt(s: &str, b: &Bind) -> PT {
    match s {
        "..." | "any" => PT::Any,
        "(T) -> bool" => PT::Lambda { arg: b.t.clone().unwrap_or_else(unk), ret: Some(Ty::Bool) },
        "(T) -> U" => PT::Lambda { arg: b.t.clone().unwrap_or_else(unk), ret: None },
        _ => PT::Ty(table_ty(s, b)),
    }
}

/// Converts a pattern from the table (`"30s"`, `"-1"`, `"\"\""`) into a `TExpr`.
fn table_default(text: &str, ty: &Ty, span: Span) -> Option<TExpr> {
    let (toks, diags) = crate::lexer::lex(text, 0);
    if !diags.is_empty() {
        return None;
    }
    use crate::lexer::Tok;
    let kind = match (&toks[0].tok, toks.get(1).map(|t| &t.tok)) {
        (Tok::Minus, Some(Tok::Int(n))) => TK::Int(-n),
        (Tok::Int(n), _) => TK::Int(*n),
        (Tok::Dur(d), _) => TK::Duration(*d),
        (Tok::Str(s), _) => TK::Str(s.plain()?),
        (Tok::Kw(crate::lexer::Kw::True), _) => TK::Bool(true),
        (Tok::Kw(crate::lexer::Kw::False), _) => TK::Bool(false),
        (Tok::LBracket, Some(Tok::RBracket)) => TK::List(vec![]),
        _ => return None,
    };
    Some(TExpr { kind, ty: ty.clone(), span })
}

fn std_sig(params: &[StdParam], b: &Bind, span: Span) -> Sig {
    let mut out = Vec::new();
    let mut variadic = false;
    for p in params {
        let ty = table_pt(p.ty, b);
        if p.ty == "..." {
            variadic = true;
        }
        let default = match (&ty, p.default) {
            (PT::Ty(t), Some(d)) => table_default(d, t, span),
            _ => None,
        };
        out.push(SP { name: p.name.to_string(), ty, default });
    }
    Sig { params: out, variadic }
}

// =================================================================================================
// Checker
// =================================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtxKind {
    Top,
    Func,
    Test,
    Lambda,
}

#[derive(Clone, Copy, Debug)]
enum ErrBind {
    Var(VarId),
    Current,
}

#[derive(Clone, Debug)]
struct Ctx {
    kind: CtxKind,
    /// Declared return (function) or expected return (lambda, if known).
    ret: Option<Ty>,
    /// Inferred return of a lambda.
    lambda_ret: Option<Ty>,
    self_var: Option<VarId>,
    loop_depth: u32,
    in_parallel: bool,
    /// Variables in scopes with an index lower than this are "outside" the `parallel`.
    par_base: Option<usize>,
    /// … and outside the lambda.
    lambda_base: Option<usize>,
    err_stack: Vec<Option<ErrBind>>,
    in_test: bool,
}

impl Ctx {
    fn new(kind: CtxKind) -> Ctx {
        Ctx {
            kind,
            ret: None,
            lambda_ret: None,
            self_var: None,
            loop_depth: 0,
            in_parallel: false,
            par_base: None,
            lambda_base: None,
            err_stack: Vec::new(),
            in_test: kind == CtxKind::Test,
        }
    }
}

pub struct Checker<'a> {
    #[allow(dead_code)]
    src: &'a str,
    embed_root: &'a Path,
    diags: Vec<Diag>,
    structs: Vec<StructInfo>,
    struct_idx: HashMap<String, usize>,
    enums: Vec<EnumInfo>,
    enum_idx: HashMap<String, usize>,
    fns: Vec<FnInfo>,
    fn_idx: HashMap<String, usize>,
    vars: Vec<VarInfo>,
    scopes: Vec<HashMap<String, VarId>>,
    ctx: Ctx,
    uses_args: bool,
    idx: Index,
    /// End of the current block (the visibility of a name declared in it ends there).
    block_end: Vec<usize>,
}

/// Runs the checker on an already parsed file.
pub fn check(file: &ast::File, src: &str, embed_root: &Path) -> (TProgram, Vec<Diag>) {
    let (p, d, _) = check_indexed(file, src, embed_root);
    (p, d)
}

/// [`check`] and the editor index ([`Index`]). It also runs on an AST with syntax errors (the parser
/// recovers), so the editor has types while the code is still incomplete.
pub fn check_indexed(file: &ast::File, src: &str, embed_root: &Path) -> (TProgram, Vec<Diag>, Index) {
    let mut c = Checker {
        src,
        embed_root,
        diags: Vec::new(),
        structs: Vec::new(),
        struct_idx: HashMap::new(),
        enums: Vec::new(),
        enum_idx: HashMap::new(),
        fns: Vec::new(),
        fn_idx: HashMap::new(),
        vars: Vec::new(),
        scopes: vec![HashMap::new()],
        ctx: Ctx::new(CtxKind::Top),
        uses_args: false,
        idx: Index::default(),
        block_end: Vec::new(),
    };
    let (p, d) = c.run(file);
    (p, d, c.idx)
}

impl<'a> Checker<'a> {
    // ---------------------------------------------------------------------------------------------
    // Diagnostics
    // ---------------------------------------------------------------------------------------------

    fn err(&mut self, code: &'static str, span: Span, m: impl Into<String>) -> &mut Diag {
        self.diags.push(Diag::error(code, span, m));
        self.diags.last_mut().unwrap()
    }
    fn errh(&mut self, code: &'static str, span: Span, m: impl Into<String>, h: impl Into<String>) {
        self.diags.push(Diag::error(code, span, m).with_help(h));
    }
    fn warn(&mut self, code: &'static str, span: Span, m: impl Into<String>, h: Option<String>) {
        let mut d = Diag::warning(code, span, m);
        d.help = h;
        self.diags.push(d);
    }

    fn bad(span: Span) -> TExpr {
        TExpr { kind: TK::Bad, ty: unk(), span }
    }

    // ---------------------------------------------------------------------------------------------
    // Scopes
    // ---------------------------------------------------------------------------------------------

    fn lookup(&self, name: &str) -> Option<(VarId, usize)> {
        for (i, s) in self.scopes.iter().enumerate().rev() {
            if let Some(id) = s.get(name) {
                return Some((*id, i));
            }
        }
        None
    }

    fn is_module(name: &str) -> bool {
        stdlib::STDLIB.iter().any(|f| f.module == name)
    }

    fn is_reserved(name: &str) -> bool {
        stdlib::RESERVED.contains(&name) || Self::is_module(name)
    }

    fn new_var(&mut self, name: &str, ty: Ty, mutable: bool, kind: VarKind, span: Span) -> VarId {
        let id = self.vars.len();
        let end = self.block_end.last().copied().unwrap_or(usize::MAX);
        self.vars.push(VarInfo { name: name.to_string(), ty, mutable, kind, span, used: false, vis: (span.end, end) });
        id
    }

    fn sym(&mut self, span: Span, s: Sym) {
        self.idx.syms.push((span, s));
    }

    /// `push_scope` for a block: the names declared in it are valid until `end`.
    fn enter_block(&mut self, end: usize) {
        self.block_end.push(end);
    }
    fn leave_block(&mut self) {
        self.block_end.pop();
    }

    /// Declares in the current scope (with `E003`/`E004`).
    fn declare(&mut self, name: &ast::Ident, ty: Ty, mutable: bool, kind: VarKind) -> VarId {
        if Self::is_reserved(&name.name) {
            let help = if Self::is_module(&name.name) {
                format!("`{}` is a standard library module; choose another name", name.name)
            } else {
                "choose another name".to_string()
            };
            self.errh("E004", name.span, msg::reserved(&name.name), help);
        } else if self.scopes.last().map(|s| s.contains_key(&name.name)).unwrap_or(false) {
            self.errh("E003", name.span, msg::redeclared(&name.name), msg::REDECLARED_HELP);
        }
        let id = self.new_var(&name.name, ty, mutable, kind, name.span);
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.name.clone(), id);
        }
        self.sym(name.span, Sym::Var(id));
        id
    }

    /// The flattened fields of a struct with their types as stored in a list (D35): inner structs spelled
    /// `inner__field`, and a list of structs as lists of its fields.
    fn struct_leaf_types(&self, s: &str, depth: usize) -> Vec<(String, Ty)> {
        let Some(&i) = self.struct_idx.get(s) else { return Vec::new() };
        let mut out = Vec::new();
        for f in self.structs[i].fields.clone() {
            match &f.ty {
                Ty::Struct(n) if depth < 16 => out.extend(self.struct_leaf_types(n, depth + 1).into_iter().map(|(l, t)| (format!("{}__{}", f.name, l), t))),
                Ty::List(e) if depth < 16 && matches!(**e, Ty::Struct(_)) => {
                    let Ty::Struct(n) = &**e else { continue };
                    out.extend(self.struct_leaf_types(n, depth + 1).into_iter().map(|(l, t)| (format!("{}__{}", f.name, l), Ty::list(t))));
                }
                t => out.push((f.name.clone(), t.clone())),
            }
        }
        out
    }

    /// Whether struct `from` reaches `target` through struct-typed fields (`path` gets the way).
    fn struct_cycle(&self, from: &str, target: &str, path: &mut Vec<String>) -> bool {
        if from == target {
            return true;
        }
        if path.len() > 32 {
            return false;
        }
        let Some(&i) = self.struct_idx.get(from) else { return false };
        for f in self.structs[i].fields.clone() {
            if let Some(n) = struct_in(&f.ty) {
                let n = &n;
                if path.iter().skip(1).any(|p| p == n) && n != target {
                    continue;
                }
                path.push(n.clone());
                if self.struct_cycle(n, target, path) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }

    /// The scalar and collection fields of a struct, with nested structs spelled `inner__field`.
    fn struct_leaves(&self, s: &str) -> Vec<String> {
        self.struct_leaves_at(s, 0)
    }

    fn struct_leaves_at(&self, s: &str, depth: usize) -> Vec<String> {
        let Some(&i) = self.struct_idx.get(s) else { return Vec::new() };
        let mut out = Vec::new();
        for f in self.structs[i].fields.clone() {
            match struct_in(&f.ty) {
                // a cycle is reported elsewhere; stop instead of following it
                Some(n) if depth < 16 => out.extend(self.struct_leaves_at(&n, depth + 1).into_iter().map(|l| format!("{}__{}", f.name, l))),
                _ => out.push(f.name.clone()),
            }
        }
        out
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }
    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn visible_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.scopes.iter().flat_map(|s| s.keys().cloned()).collect();
        v.extend(self.fns.iter().filter(|f| f.method_of.is_none()).map(|f| f.name.clone()));
        v.extend(stdlib::BUILTINS.iter().filter(|b| !stdlib::internal_builtin(b.name)).map(|b| b.name.to_string()));
        v.extend(stdlib::modules().iter().map(|s| s.to_string()));
        v.extend(self.structs.iter().map(|s| s.name.clone()));
        v.extend(self.enums.iter().map(|s| s.name.clone()));
        v.extend(["os", "dry_run", "args", "embed", "exit"].iter().map(|s| s.to_string()));
        v
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        let cands = self.visible_names();
        let d = self.err("E001", span, msg::not_found(name));
        if let Some(s) = suggest(name, cands) {
            d.help = Some(msg::did_you_mean(&s));
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Written types
    // ---------------------------------------------------------------------------------------------

    fn resolve_ty(&mut self, t: &ast::TypeExpr) -> Ty {
        match t {
            ast::TypeExpr::Named(id) => match id.name.as_str() {
                "str" | "int" | "bool" | "path" | "duration" | "secret" => {
                    let t = match id.name.as_str() {
                        "str" => Ty::Str,
                        "int" => Ty::Int,
                        "bool" => Ty::Bool,
                        "path" => Ty::Path,
                        "duration" => Ty::Duration,
                        _ => Ty::Secret,
                    };
                    self.sym(id.span, Sym::Type(t.clone()));
                    t
                }
                n if self.struct_idx.contains_key(n) => {
                    self.sym(id.span, Sym::Struct(n.to_string()));
                    Ty::Struct(n.to_string())
                }
                n if self.enum_idx.contains_key(n) => {
                    self.sym(id.span, Sym::Enum(n.to_string()));
                    Ty::Enum(n.to_string())
                }
                n => {
                    let help = match n {
                        "string" | "String" | "text" => Some("the text type is `str`".to_string()),
                        "float" | "double" | "f64" | "number" => Some("there is no `float` in Nalu; use `int`".to_string()),
                        "i64" | "integer" | "long" => Some("the integer type is `int`".to_string()),
                        "boolean" => Some("the type is `bool`".to_string()),
                        "void" | "unit" | "None" => Some("functions without a return value simply have no `->`".to_string()),
                        _ => {
                            let mut c: Vec<String> = ["str", "int", "bool", "path", "duration", "secret", "map"].iter().map(|s| s.to_string()).collect();
                            c.extend(self.structs.iter().map(|s| s.name.clone()));
                            c.extend(self.enums.iter().map(|s| s.name.clone()));
                            suggest(n, c).map(|s| msg::did_you_mean(&s))
                        }
                    };
                    let d = self.err("E001", id.span, format!("type `{}` does not exist", n));
                    d.help = help;
                    unk()
                }
            },
            ast::TypeExpr::List(inner, sp) => {
                let it = self.resolve_ty(inner);
                if matches!(it, Ty::List(_) | Ty::Map(_, _)) && !inner_list_ok(&it) {
                    self.errh("E090", *sp, format!("`[]{}` is not supported yet", tdisp(&it)), "a list can hold lists of scalars (`[][]str`); lists of maps and deeper nesting are planned");
                    return Ty::list(unk());
                }
                Ty::list(it)
            }
            ast::TypeExpr::Map(k, v, sp) => {
                let kt = self.resolve_ty(k);
                let vt = self.resolve_ty(v);
                if !is_unk(&kt) && !matches!(kt, Ty::Str | Ty::Int | Ty::Enum(_)) {
                    self.errh("E010", k.span(), format!("map keys are `str`, `int` or an enum, not `{}`", tdisp(&kt)), "use `map[str]…`");
                }
                if !is_unk(&vt) && (!vt.is_scalar() || vt == Ty::Secret) && !inner_list_ok(&vt) {
                    self.errh("E090", *sp, format!("maps with `{}` values are not supported yet", tdisp(&vt)), "map values are scalars or lists of scalars (`map[str][]str`)");
                }
                Ty::map(kt, vt)
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Program
    // ---------------------------------------------------------------------------------------------

    fn run(&mut self, file: &ast::File) -> (TProgram, Vec<Diag>) {
        // 1. type names
        for item in &file.items {
            match item {
                ast::Item::Enum(e) => {
                    if self.struct_idx.contains_key(&e.name.name) || self.enum_idx.contains_key(&e.name.name) {
                        self.errh("E003", e.name.span, format!("type `{}` is already declared", e.name.name), "choose another name");
                        continue;
                    }
                    if Self::is_reserved(&e.name.name) {
                        self.errh("E004", e.name.span, msg::reserved(&e.name.name), "choose another name");
                    }
                    let mut cases: Vec<String> = Vec::new();
                    for c in &e.cases {
                        if cases.contains(&c.name) {
                            self.errh("E003", c.span, format!("case `{}` appears twice", c.name), "remove the duplicate");
                        } else {
                            cases.push(c.name.clone());
                        }
                    }
                    if cases.is_empty() {
                        self.errh("E010", e.span, "an enum needs at least one case", "`enum Color { green, blue }`");
                    }
                    if !e.name.name.starts_with(|c: char| c.is_ascii_uppercase()) {
                        let mut fixed = e.name.name.clone();
                        fixed[..1].make_ascii_uppercase();
                        self.errh("E010", e.name.span, "type names start with an uppercase letter", format!("write `enum {}`", fixed));
                    }
                    self.enum_idx.insert(e.name.name.clone(), self.enums.len());
                    self.enums.push(EnumInfo { name: e.name.name.clone(), cases, span: e.span });
                    self.sym(e.name.span, Sym::Enum(e.name.name.clone()));
                    for c in &e.cases {
                        self.sym(c.span, Sym::Case(e.name.name.clone(), c.name.clone()));
                    }
                }
                ast::Item::Struct(s) => {
                    if self.struct_idx.contains_key(&s.name.name) || self.enum_idx.contains_key(&s.name.name) {
                        self.errh("E003", s.name.span, format!("type `{}` is already declared", s.name.name), "choose another name");
                        continue;
                    }
                    if Self::is_reserved(&s.name.name) || matches!(s.name.name.as_str(), "str" | "int" | "bool" | "path" | "duration" | "secret" | "map") {
                        self.errh("E004", s.name.span, msg::reserved(&s.name.name), "choose another name");
                    }
                    if !s.name.name.starts_with(|c: char| c.is_ascii_uppercase()) {
                        let mut fixed = s.name.name.clone();
                        fixed[..1].make_ascii_uppercase();
                        self.errh("E010", s.name.span, "type names start with an uppercase letter", format!("write `struct {}` (literals are `{} {{ … }}`)", fixed, fixed));
                    }
                    self.struct_idx.insert(s.name.name.clone(), self.structs.len());
                    self.structs.push(StructInfo { name: s.name.name.clone(), fields: vec![], span: s.span });
                    self.sym(s.name.span, Sym::Struct(s.name.name.clone()));
                }
                _ => {}
            }
        }
        // 2. struct fields
        let mut struct_decls: Vec<&ast::StructDecl> = Vec::new();
        for item in &file.items {
            if let ast::Item::Struct(s) = item {
                if let Some(&idx) = self.struct_idx.get(&s.name.name) {
                    if self.structs[idx].span != s.span {
                        continue;
                    }
                    struct_decls.push(s);
                    let mut fields: Vec<FieldInfo> = Vec::new();
                    for f in &s.fields {
                        let ty = self.resolve_ty(&f.ty);
                        if fields.iter().any(|x| x.name == f.name.name) {
                            self.errh("E003", f.name.span, format!("field `{}` appears twice", f.name.name), "remove the duplicate");
                            continue;
                        }
                        self.sym(f.name.span, Sym::Field { owner: s.name.name.clone(), name: f.name.name.clone(), ty: ty.clone() });
                        fields.push(FieldInfo { name: f.name.name.clone(), ty, default: None, span: f.name.span });
                    }
                    self.structs[idx].fields = fields;
                }
            }
        }
        // structs inside structs (D35): no cycles, and the flattened names (`svc__port`) don't clash
        for si in 0..self.structs.len() {
            let name = self.structs[si].name.clone();
            let fields = self.structs[si].fields.clone();
            for f in &fields {
                let Some(inner) = struct_in(&f.ty) else { continue };
                let inner = &inner;
                let mut path = vec![name.clone(), inner.clone()];
                if self.struct_cycle(inner, &name, &mut path) {
                    self.errh("E010", f.span, format!("`{}` contains itself ({})", name, path.join(" → ")), "a struct can't hold itself; keep something that identifies it instead, like a name");
                    continue;
                }
                // a list of structs is kept as one list per field of the item: those must be scalars or lists of scalars
                if matches!(f.ty, Ty::List(_)) {
                    if let Some((leaf, lt)) = self.struct_leaf_types(inner, 0).into_iter().find(|(_, t)| !(t.is_scalar() || inner_list_ok(t))) {
                        self.errh(
                            "E090",
                            f.span,
                            format!("`{}: []{}` is not supported yet: `{}.{}` is `{}`", f.name, inner, inner, leaf.replace("__", "."), tdisp(&lt)),
                            "the items of a list field can have scalars, lists of scalars and structs of those as fields",
                        );
                        continue;
                    }
                }
                for leaf in self.struct_leaves(inner) {
                    let flat = format!("{}__{}", f.name, leaf);
                    if fields.iter().any(|x| x.name == flat) {
                        self.errh("E003", f.span, format!("field `{}` clashes with `{}.{}`", flat, f.name, leaf.replace("__", ".")), "rename one of them");
                    }
                }
            }
        }
        // 3. function and method signatures
        let mut fn_decls: Vec<(&ast::FnDecl, Option<String>)> = Vec::new();
        for item in &file.items {
            match item {
                ast::Item::Fn(f) => fn_decls.push((f, None)),
                ast::Item::Struct(s) => {
                    if struct_decls.iter().any(|d| std::ptr::eq(*d, s)) {
                        for m in &s.methods {
                            fn_decls.push((m, Some(s.name.name.clone())));
                        }
                    }
                }
                _ => {}
            }
        }
        let mut fn_ids: Vec<Option<usize>> = Vec::new();
        for (f, owner) in &fn_decls {
            let key = match owner {
                Some(o) => format!("{}.{}", o, f.name.name),
                None => f.name.name.clone(),
            };
            if owner.is_none() && (Self::is_reserved(&f.name.name) || stdlib::find_builtin(&f.name.name).is_some()) {
                self.errh("E004", f.name.span, msg::reserved(&f.name.name), "choose another name for the function");
            }
            if let Some(o) = owner {
                let has_field = self.struct_idx.get(o).map(|&i| self.structs[i].fields.iter().any(|x| x.name == f.name.name)).unwrap_or(false);
                if has_field {
                    self.errh("E003", f.name.span, format!("`{}` is already a field of `{}`", f.name.name, o), "choose another name for the method");
                }
            }
            if self.fn_idx.contains_key(&key) {
                self.errh("E003", f.name.span, format!("function `{}` is already declared", key), "choose another name");
                fn_ids.push(None);
                continue;
            }
            let mut params = Vec::new();
            for p in &f.params {
                if params.iter().any(|x: &PInfo| x.name == p.name.name) {
                    self.errh("E003", p.name.span, format!("parameter `{}` appears twice", p.name.name), "choose another name");
                }
                if Self::is_reserved(&p.name.name) {
                    self.errh("E004", p.name.span, msg::reserved(&p.name.name), "choose another name for the parameter");
                }
                let ty = self.resolve_ty(&p.ty);
                params.push(PInfo { name: p.name.name.clone(), ty, default: None, span: p.name.span });
            }
            let ret = match &f.ret {
                Some(t) => self.resolve_ty(t),
                None => Ty::Unit,
            };
            let idx = self.fns.len();
            self.fn_idx.insert(key.clone(), idx);
            self.fns.push(FnInfo { name: key, params, ret, method_of: owner.clone(), span: f.span, name_span: f.name.span, doc: f.doc.clone() });
            self.sym(f.name.span, Sym::Fn(idx));
            fn_ids.push(Some(idx));
        }
        // 4. defaults (fields and parameters), in an empty scope
        let saved_scopes = std::mem::replace(&mut self.scopes, vec![HashMap::new()]);
        for s in &struct_decls {
            let idx = self.struct_idx[&s.name.name];
            for f in &s.fields {
                if let Some(d) = &f.default {
                    let Some(fi) = self.structs[idx].fields.iter().position(|x| x.name == f.name.name) else { continue };
                    let fty = self.structs[idx].fields[fi].ty.clone();
                    let te = self.ex(d, Some(&fty), true);
                    self.expect_ty(&te, &fty, &format!("the default of field `{}`", f.name.name));
                    self.structs[idx].fields[fi].default = Some(te);
                }
            }
        }
        for ((f, _), id) in fn_decls.iter().zip(fn_ids.iter()) {
            let Some(id) = id else { continue };
            let mut seen_default = false;
            for (i, p) in f.params.iter().enumerate() {
                if let Some(d) = &p.default {
                    seen_default = true;
                    let pty = self.fns[*id].params[i].ty.clone();
                    let te = self.ex(d, Some(&pty), true);
                    self.expect_ty(&te, &pty, &format!("the default of `{}`", p.name.name));
                    self.fns[*id].params[i].default = Some(te);
                } else if seen_default {
                    self.errh("E012", p.name.span, "parameters without a default come before those with one", "move this parameter earlier");
                }
            }
        }
        self.scopes = saved_scopes;

        // 5. script params
        let mut tparams = Vec::new();
        for item in &file.items {
            if let ast::Item::Param(p) = item {
                tparams.push(self.param(p));
            }
        }
        // 6. top-level statements
        self.ctx = Ctx::new(CtxKind::Top);
        let mut body = Vec::new();
        for item in &file.items {
            if let ast::Item::Stmt(s) = item {
                self.stmt(s, &mut body);
            }
        }
        let globals = self.scopes[0].clone();
        // 7. function bodies
        let mut funcs: Vec<TFunc> = (0..self.fns.len()).map(|_| TFunc { params: vec![], body: vec![] }).collect();
        for ((f, owner), id) in fn_decls.iter().zip(fn_ids.iter()) {
            let Some(id) = *id else { continue };
            self.scopes = vec![globals.clone(), HashMap::new()];
            self.ctx = Ctx::new(CtxKind::Func);
            self.ctx.ret = Some(self.fns[id].ret.clone());
            let mut pids = Vec::new();
            if let Some(o) = owner {
                let sid = self.new_var("self", Ty::Struct(o.clone()), false, VarKind::SelfParam, f.name.span);
                self.vars[sid].used = true;
                self.ctx.self_var = Some(sid);
                pids.push(sid);
            }
            for (i, p) in f.params.iter().enumerate() {
                let ty = self.fns[id].params[i].ty.clone();
                let pid = self.new_var(&p.name.name, ty, false, VarKind::FnParam, p.name.span);
                self.vars[pid].vis = (f.body.span.start, f.body.span.end);
                self.sym(p.name.span, Sym::Var(pid));
                self.scopes[1].insert(p.name.name.clone(), pid);
                pids.push(pid);
            }
            let mut stmts = Vec::new();
            self.push_scope();
            self.enter_block(f.body.span.end);
            for s in &f.body.stmts {
                self.stmt(s, &mut stmts);
            }
            self.leave_block();
            self.pop_scope();
            let ret = self.fns[id].ret.clone();
            if ret != Ty::Unit && !is_unk(&ret) && !terminates(&stmts) {
                let end = Span::new(f.body.span.end.saturating_sub(1), f.body.span.end);
                self.errh(
                    "E032",
                    end,
                    format!("function `{}` returns `{}`, but some path reaches the end without `return`", self.fns[id].name, tdisp(&ret)),
                    "end every path with `return value` or `fail \"…\"`",
                );
            }
            funcs[id] = TFunc { params: pids, body: stmts };
        }
        // 8. tests
        let mut tests = Vec::new();
        for item in &file.items {
            if let ast::Item::Test(t) = item {
                self.scopes = vec![globals.clone(), HashMap::new()];
                self.ctx = Ctx::new(CtxKind::Test);
                let mut stmts = Vec::new();
                self.enter_block(t.body.span.end);
                for s in &t.body.stmts {
                    self.stmt(s, &mut stmts);
                }
                self.leave_block();
                tests.push(TTest { name: t.name.clone(), body: stmts, span: t.span });
            }
        }
        self.scopes = vec![globals];

        let mut prog = TProgram {
            structs: std::mem::take(&mut self.structs),
            enums: std::mem::take(&mut self.enums),
            fns: std::mem::take(&mut self.fns),
            funcs,
            params: tparams,
            body,
            tests,
            vars: std::mem::take(&mut self.vars),
            fallible: vec![],
            reasons: vec![],
            uses_args: self.uses_args,
        };
        // 9. fallibility (fixed point) and the checks that depend on it
        infer_fallible(&mut prog);
        post_checks(&prog, &mut self.diags);
        // 10. unused warnings
        for v in &prog.vars {
            if v.used || v.name.starts_with('_') {
                continue;
            }
            match v.kind {
                VarKind::Global | VarKind::Local | VarKind::Loop => self.warn(
                    "W001",
                    v.span,
                    format!("variable `{}` is never used", v.name),
                    Some(format!("remove it, or rename it to `_{}` if this is intentional", v.name)),
                ),
                VarKind::ScriptParam => self.warn("W002", v.span, format!("parameter `{}` is never used", v.name), Some("remove the `param` or use its value".into())),
                _ => {}
            }
        }
        let diags = std::mem::take(&mut self.diags);
        (prog, diags)
    }

    fn param(&mut self, p: &ast::ParamDecl) -> TParam {
        let ty = self.resolve_ty(&p.ty);
        let allowed = matches!(ty, Ty::Str | Ty::Int | Ty::Bool | Ty::Path | Ty::Duration | Ty::Secret | Ty::Enum(_))
            || matches!(&ty, Ty::List(t) if matches!(**t, Ty::Str | Ty::Int | Ty::Path))
            || is_unk(&ty);
        if !allowed {
            self.errh(
                "E010",
                p.ty.span(),
                format!("`param` does not accept type `{}`", tdisp(&ty)),
                "`param` types: `str`, `int`, `bool`, `path`, `duration`, `secret`, enum, `[]str`, `[]int`, `[]path`",
            );
        }
        if matches!(p.name.name.as_str(), "verbose" | "help" | "dry_run") {
            self.errh("E004", p.name.span, msg::reserved(&p.name.name), "every script already has `--dry-run`, `--verbose` and `--help`; choose another name");
        }
        let default = p.default.as_ref().map(|d| {
            let te = self.ex(d, Some(&ty), true);
            self.expect_ty(&te, &ty, &format!("the default of `{}`", p.name.name));
            if !is_const(&te) {
                self.errh("E010", d.span, "the default of a `param` must be a fixed value", "use a literal: `\"text\"`, `8080`, `30s`, `Type.case`, `[\"a\", \"b\"]`");
            }
            te
        });
        if ty == Ty::Secret && default.is_some() {
            self.errh("E050", p.span, "a `param` of type `secret` cannot have a default", "the value comes from `--name` or from the environment variable `NALU_NAME`");
        }
        let id = if matches!(p.name.name.as_str(), "verbose" | "help" | "dry_run") {
            // already reported (E004); declare anyway to avoid cascading errors
            let id = self.new_var(&p.name.name, ty.clone(), false, VarKind::ScriptParam, p.name.span);
            self.vars[id].used = true;
            self.sym(p.name.span, Sym::Var(id));
            self.scopes[0].insert(p.name.name.clone(), id);
            id
        } else {
            self.declare(&p.name, ty.clone(), false, VarKind::ScriptParam)
        };
        TParam { var: id, ty, default, doc: p.doc.clone(), span: p.span }
    }

    // ---------------------------------------------------------------------------------------------
    // Type checking
    // ---------------------------------------------------------------------------------------------

    /// Checks `te` against `want`, emitting the proper error. Returns `true` if it matched.
    fn expect_ty(&mut self, te: &TExpr, want: &Ty, what: &str) -> bool {
        if compat(want, &te.ty) {
            return true;
        }
        let got = te.ty.clone();
        match (want, &got) {
            (Ty::Str | Ty::Path, Ty::Secret) => self.errh("E050", te.span, format!("{}: {}", what, msg::SECRET_TEXT), msg::SECRET_HELP),
            (Ty::Duration, Ty::Int) => {
                let help = match te.kind {
                    TK::Int(n) => format!("write the unit: `{}s` or `{}ms`", n, n),
                    _ => msg::DURATION_UNIT_HELP.to_string(),
                };
                self.errh("E020", te.span, msg::mismatch(what, want, &got), help)
            }
            (Ty::Int, Ty::Duration) => self.errh("E020", te.span, msg::mismatch(what, want, &got), "for a number, divide: `d / 1s` (seconds) or `d / 1ms`"),
            (Ty::Str, Ty::Int | Ty::Bool | Ty::Enum(_)) => {
                self.errh("E010", te.span, msg::mismatch(what, want, &got), "convert to text with interpolation: `\"{x}\"`")
            }
            (Ty::Int, Ty::Str) => self.errh("E010", te.span, msg::mismatch(what, want, &got), "convert with `.to_int()` (can fail: `text.to_int() catch 0`)"),
            (Ty::Enum(e), Ty::Str) => self.errh("E010", te.span, msg::mismatch(what, want, &got), format!("convert with `{}.parse(text)` (can fail)", e)),
            (Ty::Unit, _) => self.errh("E010", te.span, format!("{} expects no value, but got `{}`", what, tdisp(&got)), "remove the value"),
            (_, Ty::Unit) => self.errh("E010", te.span, format!("{} expects `{}`, but this produces no value", what, tdisp(want)), "use something that returns a value"),
            _ => {
                self.err("E010", te.span, msg::mismatch(what, want, &got));
            }
        }
        false
    }

    /// For `print`, `log`, `fmt`, interpolation: the value must become text.
    fn expect_text(&mut self, te: &TExpr, ctx_interp: bool) -> bool {
        if is_unk(&te.ty) || stringable(&te.ty) {
            return true;
        }
        match &te.ty {
            Ty::Secret => self.errh("E050", te.span, msg::SECRET_TEXT, msg::SECRET_HELP),
            Ty::List(inner) => {
                let help = if inner.is_scalar() { "join the items: `list.join(\", \")`" } else { "interpolate fields of the items, for example with `.map(x => x.field).join(\", \")`" };
                self.errh("E022", te.span, format!("a list (`{}`) cannot become text", tdisp(&te.ty)), help)
            }
            Ty::Map(_, _) => self.errh("E022", te.span, format!("a map (`{}`) cannot become text", tdisp(&te.ty)), "use `m.keys().join(\", \")` or interpolate `m[key]`"),
            Ty::Struct(n) => self.errh("E022", te.span, format!("a struct (`{}`) cannot become text", n), "interpolate the fields: `{s.field}`"),
            Ty::Unit => {
                let w = if ctx_interp { "in the interpolation" } else { "here" };
                self.errh("E010", te.span, format!("this produces no value to use {}", w), "use something that returns a value")
            }
            _ => {
                self.err("E010", te.span, format!("`{}` cannot become text", tdisp(&te.ty)));
            }
        }
        false
    }

    fn unify(&mut self, acc: &mut Option<Ty>, t: Option<Ty>, span: Span) {
        let Some(t) = t else { return };
        match acc {
            None => *acc = Some(t),
            Some(a) => {
                if !compat(a, &t) {
                    let m = format!("the branches return different types: `{}` and `{}`", tdisp(a), tdisp(&t));
                    self.errh("E010", span, m, "every branch of an `if`/`match` used as a value must return the same type");
                } else if is_unk(a) {
                    *acc = Some(t);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Statements
    // ---------------------------------------------------------------------------------------------

    fn block_stmts(&mut self, b: &ast::Block) -> Vec<TStmt> {
        self.push_scope();
        self.enter_block(b.span.end);
        let mut out = Vec::new();
        for s in &b.stmts {
            self.stmt(s, &mut out);
        }
        self.leave_block();
        self.pop_scope();
        out
    }

    /// Block in value context: the last line is the value. Returns the type (`None` if the block diverges).
    fn value_block(&mut self, b: &ast::Block, exp: Option<&Ty>) -> (TBlock, Option<Ty>) {
        self.push_scope();
        self.enter_block(b.span.end);
        let mut stmts = Vec::new();
        let n = b.stmts.len();
        let mut value = None;
        let mut ty = None;
        for (i, s) in b.stmts.iter().enumerate() {
            if i + 1 == n {
                match &s.kind {
                    SK::Expr(e) => {
                        let te = self.ex(e, exp, true);
                        ty = Some(te.ty.clone());
                        value = Some(Box::new(te));
                    }
                    SK::Fail { .. } | SK::Return(_) | SK::Exit(_) | SK::Break | SK::Continue => {
                        self.stmt(s, &mut stmts);
                    }
                    _ => {
                        self.stmt(s, &mut stmts);
                        self.errh("E010", s.span, "the block must end with a value", "the last line of a block used as a value is the value (or `fail`/`return`)");
                        ty = Some(unk());
                    }
                }
            } else {
                self.stmt(s, &mut stmts);
            }
        }
        if n == 0 {
            self.errh("E010", b.span, "an empty block has no value", "write the value on the last line of the block");
            ty = Some(unk());
        }
        self.leave_block();
        self.pop_scope();
        (TBlock { stmts, value }, ty)
    }

    fn stmt(&mut self, s: &ast::Stmt, out: &mut Vec<TStmt>) {
        if self.nested_call_stmt(s, out) {
            return;
        }
        let span = s.span;
        let kind = match &s.kind {
            SK::Let { name, ty, value, mutable } => {
                let (te, vty) = match ty {
                    Some(t) => {
                        let t = self.resolve_ty(t);
                        let te = self.ex(value, Some(&t), true);
                        self.expect_ty(&te, &t, &format!("`{}`", name.name));
                        (te, t)
                    }
                    None => {
                        let te = self.ex(value, None, true);
                        if te.ty == Ty::Unit {
                            self.errh("E010", value.span, format!("this produces no value to store in `{}`", name.name), "call it without the `let`, or use something that returns a value");
                        }
                        let t = te.ty.clone();
                        (te, t)
                    }
                };
                let kind = if self.ctx.kind == CtxKind::Top && self.scopes.len() == 1 { VarKind::Global } else { VarKind::Local };
                let id = self.declare(name, vty, *mutable, kind);
                self.vars[id].vis.0 = span.end;
                TS::Let { var: id, value: te }
            }
            SK::Assign { target, op, value } => {
                // a nested target (`grid[i][j] = v`) copies the inner value out before and writes it back after
                let (mut pre, mut post) = (Vec::new(), Vec::new());
                let Some(k) = self.assign(target, *op, value, &mut pre, &mut post) else { return };
                out.extend(pre.into_iter().map(|kind| TStmt { kind, span }));
                out.push(TStmt { kind: k, span });
                out.extend(post.into_iter().map(|kind| TStmt { kind, span }));
                return;
            }
            SK::Expr(e) => {
                let effect = matches!(
                    e.kind,
                    EK::Call { .. } | EK::Command(_) | EK::Capture(_) | EK::If { .. } | EK::Match { .. } | EK::Or(_, _) | EK::Try(_) | EK::Retry { .. } | EK::Parallel { .. } | EK::Error
                );
                if !effect {
                    let help = match &e.kind {
                        EK::Binary(ast::BinOp::Eq, _, _) => "to assign use `=`; to test use `if`",
                        EK::Binary(_, _, _) => "to change a variable use `x += 1`; to show it, `print(…)`",
                        EK::Lambda { .. } => msg::LAMBDA_HELP,
                        EK::Ident(_) | EK::Field(_, _) => "to show the value use `print(…)`; to call it, `f()`",
                        _ => "use the value (`let x = …`, `print(…)`) or remove the line",
                    };
                    let n = self.diags.len();
                    let _ = self.ex(e, None, true);
                    // only E030 matters here (the rest would be noise about a line that does nothing)
                    self.diags.truncate(n);
                    self.errh("E030", e.span, "this expression does nothing", help);
                    return;
                }
                let te = self.ex(e, None, false);
                TS::Expr(te)
            }
            SK::For { first, second, iter, body } => match self.for_stmt(first, second.as_ref(), iter, body) {
                Some(k) => k,
                None => return,
            },
            SK::While { cond, body } => {
                let tc = self.cond(cond);
                self.ctx.loop_depth += 1;
                let b = self.block_stmts(body);
                self.ctx.loop_depth -= 1;
                TS::While { cond: tc, body: b }
            }
            SK::Fail { message, code } => {
                let (m, c) = self.fail_args(message, code.as_ref());
                TS::Fail { message: m, code: c }
            }
            SK::Return(v) => self.return_stmt(v.as_ref(), span),
            SK::Break | SK::Continue => {
                let w = if matches!(s.kind, SK::Break) { "break" } else { "continue" };
                if self.ctx.loop_depth == 0 {
                    let help = if self.ctx.in_parallel {
                        "inside `parallel` each task is independent: use `if` to skip the work".to_string()
                    } else {
                        format!("`{}` only works inside `for` or `while` (and does not cross `retry`, `parallel` or lambdas)", w)
                    };
                    self.errh("E046", span, format!("`{}` outside a loop", w), help);
                }
                if matches!(s.kind, SK::Break) {
                    TS::Break
                } else {
                    TS::Continue
                }
            }
            SK::Exit(code) => {
                let c = code.as_ref().map(|c| {
                    let te = self.ex(c, Some(&Ty::Int), true);
                    self.expect_ty(&te, &Ty::Int, "`exit`");
                    te
                });
                TS::Exit(c)
            }
            SK::Defer(e) => {
                let ok_shape = match &e.kind {
                    EK::Call { callee, .. } => !matches!(callee.kind, EK::Ident(ref n) if n == "embed"),
                    EK::Command(c) => c.stages.len() == 1 && c.redirects.is_empty() && c.stdin.is_none(),
                    _ => false,
                };
                if !ok_shape {
                    self.errh(
                        "E010",
                        e.span,
                        "`defer` takes a call or a simple command (no `|`, redirection or `catch`)",
                        "write `defer fs.rm(tmp)` or `defer $ docker rm -f {id}`",
                    );
                    return;
                }
                let te = self.ex(e, None, false);
                TS::Defer(te)
            }
            SK::Native { bash, powershell } => {
                if bash.is_none() || powershell.is_none() {
                    let falta = if bash.is_none() { "bash" } else { "powershell" };
                    self.errh("E052", span, format!("`native` without the `{}` code", falta), "`native` needs both `bash:` and `powershell:`");
                }
                TS::Native { bash: bash.clone().unwrap_or_default(), powershell: powershell.clone().unwrap_or_default() }
            }
            SK::Assert { cond, text } => {
                if !self.ctx.in_test {
                    self.errh("E060", span, "`assert` can only be used inside `test`", "to fail with a message use `if !cond { fail \"…\" }`");
                }
                let tc = self.ex(cond, Some(&Ty::Bool), true);
                self.expect_ty(&tc, &Ty::Bool, "`assert`");
                TS::Assert { cond: tc, text: text.clone() }
            }
            SK::AssertFails(e) => {
                if !self.ctx.in_test {
                    self.errh("E060", span, "`assert_fails` can only be used inside `test`", "use it inside `test \"name\" { … }`");
                }
                if !matches!(e.kind, EK::Call { .. } | EK::Command(_)) {
                    self.errh("E060", e.span, "`assert_fails` takes a call or a command", "write `assert_fails(f(x))`");
                    return;
                }
                let te = self.ex(e, None, false);
                TS::AssertFails(te)
            }
        };
        out.push(TStmt { kind, span });
    }

    fn fail_args(&mut self, message: &ast::Expr, code: Option<&ast::Expr>) -> (TExpr, Option<TExpr>) {
        let m = self.ex(message, Some(&Ty::Str), true);
        let mut c = code.map(|c| {
            let te = self.ex(c, Some(&Ty::Int), true);
            self.expect_ty(&te, &Ty::Int, "the `fail` code");
            te
        });
        if m.ty == Ty::Error {
            // fail err → error message and code
            let msg_e = TExpr { kind: TK::Field(Box::new(m.clone()), "message".into()), ty: Ty::Str, span: m.span };
            if c.is_none() {
                c = Some(TExpr { kind: TK::Field(Box::new(m.clone()), "code".into()), ty: Ty::Int, span: m.span });
            }
            return (msg_e, c);
        }
        if !texty(&m.ty) && !is_unk(&m.ty) {
            if m.ty == Ty::Secret {
                self.errh("E050", m.span, msg::SECRET_TEXT, msg::SECRET_HELP);
            } else {
                self.errh("E010", m.span, format!("the `fail` message is text, not `{}`", tdisp(&m.ty)), "use interpolation: `fail \"invalid value: {x}\"`");
            }
        }
        (m, c)
    }

    fn return_stmt(&mut self, v: Option<&ast::Expr>, span: Span) -> TS {
        match self.ctx.kind {
            CtxKind::Top | CtxKind::Test => {
                let help = if self.ctx.kind == CtxKind::Top { "to end the script use `exit(0)`" } else { "a test ends on its own at the end of the block" };
                self.errh("E046", span, "`return` outside a function", help);
                if let Some(v) = v {
                    self.ex(v, None, true);
                }
                return TS::Return(None);
            }
            _ => {}
        }
        if self.ctx.in_parallel {
            self.errh("E045", span, "`return` inside `parallel`", "the body of `parallel` cannot leave the function; compute a value on the last line");
        }
        if self.ctx.kind == CtxKind::Lambda {
            let Some(v) = v else {
                self.errh("E010", span, "the lambda must return a value", "write `return value`");
                return TS::Return(None);
            };
            let exp = self.ctx.ret.clone().or_else(|| self.ctx.lambda_ret.clone());
            let te = self.ex(v, exp.as_ref(), true);
            if let Some(e) = &exp {
                self.expect_ty(&te, e, "the lambda's `return`");
            } else {
                self.ctx.lambda_ret = Some(te.ty.clone());
            }
            return TS::Return(Some(te));
        }
        let ret = self.ctx.ret.clone().unwrap_or(Ty::Unit);
        match (v, &ret) {
            (None, Ty::Unit) => TS::Return(None),
            (None, r) => {
                if !is_unk(r) {
                    self.errh("E010", span, format!("the function returns `{}`: the `return` is missing its value", tdisp(r)), "write `return value`");
                }
                TS::Return(None)
            }
            (Some(v), Ty::Unit) => {
                let te = self.ex(v, None, true);
                self.errh("E010", te.span, "this function declares no return type", format!("declare the type: `fn name(…) -> {} {{`", tdisp(&te.ty)));
                TS::Return(None)
            }
            (Some(v), r) => {
                let te = self.ex(v, Some(r), true);
                self.expect_ty(&te, r, "the `return`");
                TS::Return(Some(te))
            }
        }
    }

    fn for_stmt(&mut self, first: &ast::Ident, second: Option<&ast::Ident>, iter: &ast::Expr, body: &ast::Block) -> Option<TS> {
        if let EK::Range { from, to, inclusive } = &iter.kind {
            let tf = self.ex(from, Some(&Ty::Int), true);
            let tt = self.ex(to, Some(&Ty::Int), true);
            self.expect_ty(&tf, &Ty::Int, "the start of the range");
            self.expect_ty(&tt, &Ty::Int, "the end of the range");
            if let Some(s) = second {
                self.errh("E010", s.span, "a range gives only one name", "write `for i in 0..10`");
            }
            self.push_scope();
            self.enter_block(body.span.end);
            let v = self.declare(first, Ty::Int, false, VarKind::Loop);
            self.ctx.loop_depth += 1;
            let b = self.block_stmts(body);
            self.ctx.loop_depth -= 1;
            self.leave_block();
            self.pop_scope();
            return Some(TS::ForRange { var: v, from: tf, to: tt, inclusive: *inclusive, body: b });
        }
        let ti = self.ex(iter, None, true);
        let ty = ti.ty.clone();
        self.push_scope();
        self.enter_block(body.span.end);
        let r = match &ty {
            Ty::List(elem) => {
                let (idx, item) = match second {
                    Some(s) => (Some(self.declare(first, Ty::Int, false, VarKind::Loop)), self.declare(s, (**elem).clone(), false, VarKind::Loop)),
                    None => (None, self.declare(first, (**elem).clone(), false, VarKind::Loop)),
                };
                self.ctx.loop_depth += 1;
                let b = self.block_stmts(body);
                self.ctx.loop_depth -= 1;
                Some(TS::ForList { var: item, index: idx, list: ti, body: b })
            }
            Ty::Map(k, v) => {
                let Some(s) = second else {
                    self.errh("E010", first.span, "a map is iterated with two names", "write `for key, value in map`");
                    self.leave_block();
                    self.pop_scope();
                    return None;
                };
                let kid = self.declare(first, (**k).clone(), false, VarKind::Loop);
                let vid = self.declare(s, (**v).clone(), false, VarKind::Loop);
                self.ctx.loop_depth += 1;
                let b = self.block_stmts(body);
                self.ctx.loop_depth -= 1;
                Some(TS::ForMap { key: kid, value: vid, map: ti, body: b })
            }
            t if is_unk(t) => {
                // keep checking the body
                self.declare(first, unk(), false, VarKind::Loop);
                if let Some(s) = second {
                    self.declare(s, unk(), false, VarKind::Loop);
                }
                self.ctx.loop_depth += 1;
                self.block_stmts(body);
                self.ctx.loop_depth -= 1;
                for id in self.scopes.last().map(|s| s.values().cloned().collect::<Vec<_>>()).unwrap_or_default() {
                    self.vars[id].used = true;
                }
                None
            }
            t => {
                let help = if texty(t) { "to iterate over characters use `s.chars()`; over lines, `s.lines()`" } else { "`for` iterates over lists, maps and ranges (`0..10`)" };
                self.errh("E010", iter.span, format!("cannot iterate over `{}` with `for`", tdisp(t)), help);
                None
            }
        };
        self.leave_block();
        self.pop_scope();
        r
    }

    fn cond(&mut self, e: &ast::Expr) -> TExpr {
        let te = self.ex(e, Some(&Ty::Bool), true);
        if !compat(&Ty::Bool, &te.ty) {
            let help = match &te.ty {
                Ty::Int => "compare explicitly: `n != 0`",
                Ty::Str | Ty::Path => "compare explicitly: `s != \"\"`",
                Ty::List(_) => "use `!list.is_empty()`",
                _ => "the condition must be `bool`",
            };
            self.errh("E010", te.span, format!("the condition must be `bool`, but it is `{}`", tdisp(&te.ty)), help);
        }
        te
    }

    /// Target variable of a mutation: checks `let`, `parallel` and lambda.
    fn check_mutable(&mut self, name: &str, span: Span, what: &str) -> Option<VarId> {
        let Some((id, scope)) = self.lookup(name) else {
            self.unknown_name(name, span);
            return None;
        };
        self.sym(span, Sym::Var(id));
        let v = self.vars[id].clone();
        if let Some(base) = self.ctx.lambda_base {
            if scope < base {
                self.errh("E033", span, format!("a lambda cannot change outside variables (`{}`)", name), "lambdas only read; make the change outside, with a `for`");
                return Some(id);
            }
        }
        if let Some(base) = self.ctx.par_base {
            if scope < base {
                self.errh(
                    "E045",
                    span,
                    format!("the body of `parallel` cannot change `{}`, which comes from outside", name),
                    "each task runs separately; return a value on the last line (`let r = parallel for … { value }`)",
                );
                return Some(id);
            }
        }
        if !v.mutable {
            match v.kind {
                VarKind::ScriptParam => self.errh("E002", span, format!("parameter `{}` cannot change", name), format!("copy it to a variable: `var {}_ = {}`", name, name)),
                VarKind::FnParam => self.errh("E002", span, format!("parameter `{}` cannot change", name), format!("copy it to a local variable: `var {}_ = {}`", name, name)),
                VarKind::Loop => self.errh("E002", span, format!("loop variable `{}` cannot change", name), "copy it to a `var` inside the loop"),
                _ => {
                    let m = format!("{} ({})", msg::let_immutable(name), what);
                    self.errh("E002", span, m, msg::use_var(name))
                }
            }
        }
        Some(id)
    }

    fn assign(&mut self, target: &ast::Expr, op: ast::AssignOp, value: &ast::Expr, pre: &mut Vec<TS>, post: &mut Vec<TS>) -> Option<TS> {
        let (lv, read, tty) = self.lvalue(target, pre, post, false)?;
        self.assign_value(target, lv, read, tty, op, value)
    }

    /// A statement whose value is a mutating method on a nested value (`rows[0].pop()`, `groups[k].push(x)`,
    /// also with `catch`), as a statement, the value of `let`/`var`, the right side of `=` or `return`: the
    /// call changes a hidden copy, which is written back right after (before the `return`). Returns whether
    /// it handled `s`.
    fn nested_call_stmt(&mut self, s: &ast::Stmt, out: &mut Vec<TStmt>) -> bool {
        const RECV: &str = "__nested_recv";
        const RESULT: &str = "__nested_result";
        let value = match &s.kind {
            SK::Expr(e) | SK::Let { value: e, .. } | SK::Assign { value: e, .. } | SK::Return(Some(e)) => e,
            _ => return false,
        };
        // the call, possibly as the left side of `catch`
        let (call, handler) = match &value.kind {
            EK::Or(lhs, h) => (&**lhs, Some(h)),
            _ => (value, None),
        };
        let Some((base, m, args, cspan)) = nested_mutation(call) else { return false };
        let span = s.span;
        let (mut pre, mut post) = (Vec::new(), Vec::new());
        let Some(id) = self.nested_base(base, &mut pre, &mut post) else { return true };
        if let Some(sc) = self.scopes.last_mut() {
            sc.insert(RECV.to_string(), id);
        }
        let recv = ast::Expr { kind: EK::Ident(RECV.to_string()), span: base.span };
        let callee = ast::Expr { kind: EK::Field(Box::new(recv), m.clone()), span: cspan };
        let new_call = ast::Expr { kind: EK::Call { callee: Box::new(callee), args: args.to_vec() }, span: call.span };
        let new_value = match handler {
            Some(h) => ast::Expr { kind: EK::Or(Box::new(new_call), h.clone()), span: value.span },
            None => new_call,
        };
        let mut inner = Vec::new();
        let mut after = Vec::new();
        match &s.kind {
            SK::Return(_) => {
                // `return rows[0].pop()`: the value goes to a hidden variable, the copy is written back, then `return`
                let name = ast::Ident { name: RESULT.to_string(), span: value.span };
                let let_stmt = ast::Stmt { kind: SK::Let { name, ty: None, value: new_value, mutable: false }, span };
                self.stmt(&let_stmt, &mut inner);
                let ret = ast::Stmt { kind: SK::Return(Some(ast::Expr { kind: EK::Ident(RESULT.to_string()), span: value.span })), span };
                self.stmt(&ret, &mut after);
                if let Some(sc) = self.scopes.last_mut() {
                    if let Some(r) = sc.remove(RESULT) {
                        self.vars[r].used = true;
                        self.idx.syms.retain(|(_, s)| !matches!(s, Sym::Var(v) if *v == r));
                    }
                }
            }
            k => {
                let kind = match k {
                    SK::Expr(_) => SK::Expr(new_value),
                    SK::Let { name, ty, mutable, .. } => SK::Let { name: name.clone(), ty: ty.clone(), value: new_value, mutable: *mutable },
                    SK::Assign { target, op, .. } => SK::Assign { target: target.clone(), op: *op, value: new_value },
                    _ => unreachable!("matched above"),
                };
                self.stmt(&ast::Stmt { kind, span }, &mut inner);
            }
        }
        if let Some(sc) = self.scopes.last_mut() {
            sc.remove(RECV);
        }
        // the hidden copy is not something to hover or jump to
        self.idx.syms.retain(|(_, s)| !matches!(s, Sym::Var(v) if *v == id));
        out.extend(pre.into_iter().map(|kind| TStmt { kind, span }));
        out.extend(inner);
        out.extend(post.into_iter().map(|kind| TStmt { kind, span }));
        out.extend(after);
        true
    }

    /// The variable that stands for `base` in a nested assignment: a hidden copy of it, made before (`pre`)
    /// and written back after (`post`, innermost first).
    fn nested_base(&mut self, base: &ast::Expr, pre: &mut Vec<TS>, post: &mut Vec<TS>) -> Option<VarId> {
        let (lv, read, ty) = self.lvalue(base, pre, post, true)?;
        // copying the inner value out reads the variable
        let root = match &lv {
            TLValue::Var(v) | TLValue::Field(v, _) | TLValue::Index(v, _) | TLValue::MapEntry(v, _) => *v,
        };
        self.vars[root].used = true;
        let id = self.new_var("_nested", ty.clone(), true, VarKind::Local, base.span);
        self.vars[id].used = true;
        pre.push(TS::Let { var: id, value: read });
        post.insert(0, TS::Assign { target: lv, value: TExpr { kind: TK::Var(id), ty, span: base.span } });
        Some(id)
    }

    /// What can be assigned: the lvalue, its current value (for `+=` and nested copies) and its type.
    /// `hoist`: an index computed once into a hidden variable, because it is used twice (read and write back).
    fn lvalue(&mut self, target: &ast::Expr, pre: &mut Vec<TS>, post: &mut Vec<TS>, hoist: bool) -> Option<(TLValue, TExpr, Ty)> {
        Some(match &target.kind {
            EK::Ident(n) => {
                let id = self.check_mutable(n, target.span, "assignment")?;
                let ty = self.vars[id].ty.clone();
                (TLValue::Var(id), TExpr { kind: TK::Var(id), ty: ty.clone(), span: target.span }, ty)
            }
            EK::SelfVal | EK::Field(_, _) | EK::Index(_, _) if root_is_self(target) => {
                self.errh("E002", target.span, "`self` is read-only", "return a new value: `Type { ..self, field: value }`");
                return None;
            }
            EK::Field(base, f) => {
                let id = match &base.kind {
                    EK::Ident(n) => self.check_mutable(n, base.span, "changing a field")?,
                    _ => self.nested_base(base, pre, post)?,
                };
                let vty = self.vars[id].ty.clone();
                match &vty {
                    Ty::Struct(s) if !s.is_empty() => {
                        let fty = self.structs.iter().find(|x| &x.name == s).and_then(|st| st.fields.iter().find(|x| x.name == f.name)).map(|x| x.ty.clone());
                        let Some(fty) = fty else {
                            self.errh("E001", f.span, format!("struct `{}` has no field `{}`", s, f.name), "check the field name");
                            return None;
                        };
                        let base_e = TExpr { kind: TK::Var(id), ty: vty.clone(), span: base.span };
                        (TLValue::Field(id, f.name.clone()), TExpr { kind: TK::Field(Box::new(base_e), f.name.clone()), ty: fty.clone(), span: target.span }, fty)
                    }
                    t if is_unk(t) => return None,
                    t => {
                        self.err("E010", target.span, format!("`{}` has no fields to assign", tdisp(t)));
                        return None;
                    }
                }
            }
            EK::Index(base, idx) => {
                let id = match &base.kind {
                    EK::Ident(n) => self.check_mutable(n, base.span, "changing an item")?,
                    _ => self.nested_base(base, pre, post)?,
                };
                let vty = self.vars[id].ty.clone();
                let base_e = TExpr { kind: TK::Var(id), ty: vty.clone(), span: base.span };
                match &vty {
                    Ty::List(elem) => {
                        let ti = self.ex(idx, Some(&Ty::Int), true);
                        self.expect_ty(&ti, &Ty::Int, "the index");
                        let ti = self.hoisted(ti, hoist, pre);
                        let read = TExpr { kind: TK::Index(Box::new(base_e), Box::new(ti.clone())), ty: (**elem).clone(), span: target.span };
                        (TLValue::Index(id, ti), read, (**elem).clone())
                    }
                    Ty::Map(k, v) => {
                        let tk = self.ex(idx, Some(k), true);
                        self.expect_ty(&tk, k, "the key");
                        let tk = self.hoisted(tk, hoist, pre);
                        let read = self.map_get(base_e, tk.clone(), (**v).clone(), target.span);
                        (TLValue::MapEntry(id, tk), read, (**v).clone())
                    }
                    t if is_unk(t) => return None,
                    t => {
                        self.err("E010", target.span, format!("cannot assign by index to `{}`", tdisp(t)));
                        return None;
                    }
                }
            }
            _ => {
                self.errh("E010", target.span, "cannot assign to this", "the left side of `=` must be a variable, `x.field`, `xs[i]` or `m[k]`");
                return None;
            }
        })
    }

    /// An index or key used twice (read and write back) goes to a hidden variable first, unless it is a
    /// literal or a variable.
    fn hoisted(&mut self, e: TExpr, hoist: bool, pre: &mut Vec<TS>) -> TExpr {
        if !hoist || matches!(e.kind, TK::Int(_) | TK::Str(_) | TK::Var(_) | TK::EnumCase(_, _) | TK::Bad) {
            return e;
        }
        let id = self.new_var("_nested_key", e.ty.clone(), false, VarKind::Local, e.span);
        self.vars[id].used = true;
        let (ty, span) = (e.ty.clone(), e.span);
        pre.push(TS::Let { var: id, value: e });
        TExpr { kind: TK::Var(id), ty, span }
    }

    fn assign_value(&mut self, target: &ast::Expr, lv: TLValue, read: TExpr, tty: Ty, op: ast::AssignOp, value: &ast::Expr) -> Option<TS> {
        let tv = if op == ast::AssignOp::Set {
            let tv = self.ex(value, Some(&tty), true);
            self.expect_ty(&tv, &tty, "the assignment");
            tv
        } else {
            if let TLValue::Var(id) = &lv {
                self.vars[*id].used = true;
            }
            let bop = match op {
                ast::AssignOp::Add => ast::BinOp::Add,
                ast::AssignOp::Sub => ast::BinOp::Sub,
                ast::AssignOp::Mul => ast::BinOp::Mul,
                ast::AssignOp::Div => ast::BinOp::Div,
                _ => ast::BinOp::Mod,
            };
            if let Ty::List(_) = tty {
                self.errh("E010", target.span, "lists cannot be added", "to append an item use `xs.push(x)`");
                return None;
            }
            let rhs = self.ex(value, Some(&tty), true);
            let span = target.span.to(value.span);
            let te = self.binary(bop, read, rhs, span);
            self.expect_ty(&te, &tty, "the assignment");
            te
        };
        Some(TS::Assign { target: lv, value: tv })
    }

    // ---------------------------------------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------------------------------------

    /// Checks an expression. `exp` is the expected type (if known); `value` says whether the result is used.
    /// Checks an expression and stores its type in the index (the editor completes `x.` with the type of `x`).
    fn ex(&mut self, e: &ast::Expr, exp: Option<&Ty>, value: bool) -> TExpr {
        let te = self.ex_kind(e, exp, value);
        self.idx.exprs.push((e.span, te.ty.clone()));
        te
    }

    fn ex_kind(&mut self, e: &ast::Expr, exp: Option<&Ty>, value: bool) -> TExpr {
        let span = e.span;
        let mk = |kind: TK, ty: Ty| TExpr { kind, ty, span };
        match &e.kind {
            EK::Error => Self::bad(span),
            EK::Int(n) => {
                if exp == Some(&Ty::Duration) {
                    self.errh("E020", span, "expected a duration, but got a number without a unit", format!("write the unit: `{}s` or `{}ms`", n, n));
                    return Self::bad(span);
                }
                mk(TK::Int(*n), Ty::Int)
            }
            EK::Duration(d) => mk(TK::Duration(*d), Ty::Duration),
            EK::Bool(b) => mk(TK::Bool(*b), Ty::Bool),
            EK::RawStr(s) => mk(TK::Str(s.clone()), if exp == Some(&Ty::Path) { Ty::Path } else { Ty::Str }),
            EK::Str(parts) => {
                let lit_ty = if exp == Some(&Ty::Path) { Ty::Path } else { Ty::Str };
                if parts.iter().all(|p| matches!(p, ast::StrPart::Lit(_))) {
                    let s: String = parts.iter().map(|p| if let ast::StrPart::Lit(t) = p { t.as_str() } else { "" }).collect();
                    return mk(TK::Str(s), lit_ty);
                }
                let mut tparts = Vec::new();
                for p in parts {
                    match p {
                        ast::StrPart::Lit(t) => tparts.push(TPart::Lit(t.clone())),
                        ast::StrPart::Expr(pe) => {
                            let te = self.ex(pe, None, true);
                            self.expect_text(&te, true);
                            tparts.push(TPart::Expr(te));
                        }
                    }
                }
                mk(TK::Interp(tparts), lit_ty)
            }
            EK::Ident(name) => self.ident(name, span),
            EK::SelfVal => match self.ctx.self_var {
                Some(id) => mk(TK::Var(id), self.vars[id].ty.clone()),
                None => {
                    self.errh("E046", span, "`self` outside a method", "`self` only exists in functions declared inside a `struct`");
                    Self::bad(span)
                }
            },
            EK::Field(base, f) => self.field(base, f, span),
            EK::Call { callee, args } => self.call(callee, args, span, exp),
            EK::Index(base, idx) => {
                let tb = self.ex(base, None, true);
                match tb.ty.clone() {
                    Ty::List(elem) => {
                        let ti = self.ex(idx, Some(&Ty::Int), true);
                        self.expect_ty(&ti, &Ty::Int, "the index");
                        mk(TK::Index(Box::new(tb), Box::new(ti)), *elem)
                    }
                    Ty::Map(k, v) => {
                        let tk = self.ex(idx, Some(&k), true);
                        self.expect_ty(&tk, &k, "the key");
                        self.map_get(tb, tk, *v, span)
                    }
                    t if is_unk(&t) => {
                        self.ex(idx, None, true);
                        Self::bad(span)
                    }
                    Ty::Str | Ty::Path => {
                        self.errh("E010", span, "text cannot be indexed", "use `s.slice(i, i + 1)` or `s.chars()[i]`");
                        Self::bad(span)
                    }
                    t => {
                        self.err("E010", span, format!("`{}` cannot be indexed", tdisp(&t)));
                        Self::bad(span)
                    }
                }
            }
            EK::Unary(op, inner) => {
                let ti = self.ex(inner, None, true);
                match op {
                    ast::UnOp::Neg => {
                        if !matches!(ti.ty, Ty::Int | Ty::Duration) && !is_unk(&ti.ty) {
                            self.err("E010", span, format!("`-` needs `int` or `duration`, not `{}`", tdisp(&ti.ty)));
                            return Self::bad(span);
                        }
                        let ty = ti.ty.clone();
                        mk(TK::Unary(UnOp::Neg, Box::new(ti)), ty)
                    }
                    ast::UnOp::Not => {
                        if !compat(&Ty::Bool, &ti.ty) {
                            self.errh("E010", span, format!("`!` needs `bool`, not `{}`", tdisp(&ti.ty)), "compare explicitly (`x == 0`, `s == \"\"`)");
                            return Self::bad(span);
                        }
                        mk(TK::Unary(UnOp::Not, Box::new(ti)), Ty::Bool)
                    }
                }
            }
            EK::Binary(op, l, r) => {
                let tl = self.ex(l, None, true);
                let rexp = if matches!(op, ast::BinOp::Eq | ast::BinOp::Ne | ast::BinOp::Lt | ast::BinOp::Le | ast::BinOp::Gt | ast::BinOp::Ge) {
                    Some(tl.ty.clone()).filter(|t| matches!(t, Ty::Enum(_)))
                } else {
                    None
                };
                let tr = self.ex(r, rexp.as_ref(), true);
                self.binary(*op, tl, tr, span)
            }
            EK::Range { .. } => {
                self.errh("E010", span, "ranges only appear in `for` and in `match` arms", "use `for i in 0..10 { … }`");
                Self::bad(span)
            }
            EK::List(items) => self.list(items, exp, span),
            EK::Map(entries) => self.map_lit(entries, exp, span),
            EK::EmptyBraces => match exp {
                Some(Ty::Map(k, v)) => {
                    let ty = Ty::Map(k.clone(), v.clone());
                    mk(TK::Map(vec![]), ty)
                }
                Some(Ty::Struct(s)) if !s.is_empty() => self.struct_lit(s.clone(), None, &[], span),
                Some(t) if is_unk(t) => Self::bad(span),
                _ => {
                    self.errh("E044", span, "`{}` without a known type", "annotate the type: `let m: map[str]int = {}`");
                    Self::bad(span)
                }
            },
            EK::StructLit { name, spread, fields } => {
                let sname = match name {
                    Some(n) => {
                        if !self.struct_idx.contains_key(&n.name) {
                            let mut c: Vec<String> = self.structs.iter().map(|s| s.name.clone()).collect();
                            c.extend(self.enums.iter().map(|s| s.name.clone()));
                            let d = self.err("E001", n.span, format!("type `{}` does not exist", n.name));
                            if let Some(s) = suggest(&n.name, c) {
                                d.help = Some(msg::did_you_mean(&s));
                            }
                            for (_, v) in fields {
                                self.ex(v, None, true);
                            }
                            return Self::bad(span);
                        }
                        self.sym(n.span, Sym::Struct(n.name.clone()));
                        n.name.clone()
                    }
                    None => match exp {
                        Some(Ty::Struct(s)) if !s.is_empty() => s.clone(),
                        Some(t) if is_unk(t) => return Self::bad(span),
                        _ => {
                            self.errh("E044", span, "literal `{ … }` without a known type", "write the type name: `Type { field: value }`");
                            for (_, v) in fields {
                                self.ex(v, None, true);
                            }
                            return Self::bad(span);
                        }
                    },
                };
                self.struct_lit(sname, spread.as_deref(), fields, span)
            }
            EK::Lambda { .. } => {
                self.errh("E033", span, "lambda outside an argument", msg::LAMBDA_HELP);
                Self::bad(span)
            }
            EK::If { cond, then, els } => {
                let tc = self.cond(cond);
                if value {
                    let Some(els) = els else {
                        self.errh("E010", span, "`if` used as a value needs `else`", "write `if cond { a } else { b }`");
                        self.value_block(then, exp);
                        return Self::bad(span);
                    };
                    let (tb, t1) = self.value_block(then, exp);
                    let exp2 = exp.cloned().or_else(|| t1.clone());
                    let (eb, t2) = self.else_value_block(els, exp2.as_ref());
                    let mut acc = None;
                    self.unify(&mut acc, t1, span);
                    self.unify(&mut acc, t2, span);
                    let ty = acc.or_else(|| exp.cloned()).unwrap_or_else(unk);
                    mk(TK::If { cond: Box::new(tc), then: tb, els: Some(eb) }, ty)
                } else {
                    let tb = TBlock { stmts: self.block_stmts(then), value: None };
                    let eb = els.as_ref().map(|b| TBlock { stmts: self.block_stmts(b), value: None });
                    mk(TK::If { cond: Box::new(tc), then: tb, els: eb }, Ty::Unit)
                }
            }
            EK::Match { subject, arms } => self.match_expr(subject, arms, span, exp, value),
            EK::Or(lhs, h) => self.or_expr(lhs, h, span, exp, value),
            EK::Command(c) => {
                let tc = self.cmd(c);
                if value {
                    self.errh("E010", span, "a `$` command returns no value", "to keep the output use the `$( … )` capture");
                    return Self::bad(span);
                }
                mk(TK::Command(Box::new(tc)), Ty::Unit)
            }
            EK::Capture(c) => {
                let tc = self.cmd(c);
                if let Some(si) = &c.stdin {
                    self.err("E010", si.span, "a capture takes no input");
                }
                mk(TK::Capture(Box::new(tc)), Ty::Str)
            }
            EK::CmdTest(c) => {
                let tc = self.cmd(c);
                mk(TK::CmdTest(Box::new(tc)), Ty::Bool)
            }
            EK::Try(b) => {
                self.errh("E041", span, "`try` without `catch`", "failures already propagate on their own; remove the `try` or handle the failure with `try { … } catch …`");
                let stmts = self.block_stmts(b);
                mk(TK::Try(TBlock { stmts, value: None }), Ty::Unit)
            }
            EK::Retry { times, every, body } => {
                let tt = self.ex(times, Some(&Ty::Int), true);
                self.expect_ty(&tt, &Ty::Int, "the number of attempts for `retry`");
                let te = match every {
                    Some(ev) => {
                        let te = self.ex(ev, Some(&Ty::Duration), true);
                        self.expect_ty(&te, &Ty::Duration, "`every`");
                        te
                    }
                    None => TExpr { kind: TK::Duration(1000), ty: Ty::Duration, span },
                };
                let saved = self.ctx.loop_depth;
                self.ctx.loop_depth = 0;
                let stmts = self.block_stmts(body);
                self.ctx.loop_depth = saved;
                if value {
                    self.errh("E010", span, "`retry` returns no value", "declare the variable beforehand with `var` and assign it inside the block");
                }
                mk(TK::Retry { times: Box::new(tt), every: Box::new(te), body: TBlock { stmts, value: None } }, Ty::Unit)
            }
            EK::Parallel { max, body } => self.parallel(max.as_deref(), body, span, exp, value),
        }
    }

    fn else_value_block(&mut self, b: &ast::Block, exp: Option<&Ty>) -> (TBlock, Option<Ty>) {
        self.value_block(b, exp)
    }

    fn ident(&mut self, name: &str, span: Span) -> TExpr {
        if let Some((id, _)) = self.lookup(name) {
            self.vars[id].used = true;
            self.sym(span, Sym::Var(id));
            return TExpr { kind: TK::Var(id), ty: self.vars[id].ty.clone(), span };
        }
        let mk = |kind: TK, ty: Ty| TExpr { kind, ty, span };
        match name {
            "os" => self.sym(span, Sym::Builtin("os")),
            "dry_run" => self.sym(span, Sym::Builtin("dry_run")),
            "args" => self.sym(span, Sym::Builtin("args")),
            "err" => self.sym(span, Sym::Builtin("err")),
            n if self.enum_idx.contains_key(n) => self.sym(span, Sym::Enum(n.to_string())),
            n if self.struct_idx.contains_key(n) => self.sym(span, Sym::Struct(n.to_string())),
            n if Self::is_module(n) => self.sym(span, Sym::Module(n.to_string())),
            n => {
                if let Some(&fi) = self.fn_idx.get(n) {
                    self.sym(span, Sym::Fn(fi));
                }
            }
        }
        match name {
            "os" => mk(TK::Os, Ty::Str),
            "dry_run" => mk(TK::DryRun, Ty::Bool),
            "args" => {
                self.uses_args = true;
                mk(TK::Args, Ty::list(Ty::Str))
            }
            "err" => match self.ctx.err_stack.last().cloned().flatten() {
                Some(ErrBind::Var(id)) => {
                    self.vars[id].used = true;
                    mk(TK::Var(id), Ty::Error)
                }
                Some(ErrBind::Current) => mk(TK::CurrentErr, Ty::Error),
                None => {
                    self.errh("E046", span, "`err` outside a handler", "`err` exists inside `catch { … }` and in `catch fail \"…{err}\"`");
                    Self::bad(span)
                }
            },
            n if self.enum_idx.contains_key(n) => {
                let cases = self.enums[self.enum_idx[n]].cases.clone();
                self.errh("E010", span, format!("`{}` is a type, not a value", n), format!("the cases are {}", cases.iter().map(|c| format!("`{}.{}`", n, c)).collect::<Vec<_>>().join(", ")));
                Self::bad(span)
            }
            n if self.struct_idx.contains_key(n) => {
                self.errh("E010", span, format!("`{}` is a type, not a value", n), format!("create a value with `{} {{ field: value }}`", n));
                Self::bad(span)
            }
            n if Self::is_module(n) => {
                self.errh("E010", span, format!("`{}` is a standard library module", n), format!("call one of its functions: `{}.…(…)`", n));
                Self::bad(span)
            }
            n if self.fn_idx.contains_key(n) || stdlib::find_builtin(n).is_some() || matches!(n, "exit" | "embed") => {
                self.errh("E033", span, format!("`{}` is a function; functions are not values", n), format!("call it with `{}(…)`", n));
                Self::bad(span)
            }
            n => {
                self.unknown_name(n, span);
                Self::bad(span)
            }
        }
    }

    fn field(&mut self, base: &ast::Expr, f: &ast::Ident, span: Span) -> TExpr {
        if let EK::Ident(bn) = &base.kind {
            if self.lookup(bn).is_none() {
                if let Some(&ei) = self.enum_idx.get(bn) {
                    let cases = self.enums[ei].cases.clone();
                    self.sym(base.span, Sym::Enum(bn.clone()));
                    if cases.contains(&f.name) {
                        self.sym(f.span, Sym::Case(bn.clone(), f.name.clone()));
                        return TExpr { kind: TK::EnumCase(bn.clone(), f.name.clone()), ty: Ty::Enum(bn.clone()), span };
                    }
                    if f.name == "parse" {
                        self.errh("E010", span, "`parse` is a function", format!("write `{}.parse(text)`", bn));
                        return Self::bad(span);
                    }
                    let d = self.err("E001", f.span, format!("enum `{}` has no case `{}`", bn, f.name));
                    d.help = Some(match suggest(&f.name, cases.clone()) {
                        Some(s) => msg::did_you_mean(&format!("{}.{}", bn, s)),
                        None => format!("the cases are: {}", cases.join(", ")),
                    });
                    return Self::bad(span);
                }
                if Self::is_module(bn) {
                    self.sym(base.span, Sym::Module(bn.clone()));
                    if let Some(sf) = stdlib::find_fn(bn, &f.name) {
                        self.sym(f.span, Sym::Std(sf));
                        self.errh("E010", span, format!("`{}.{}` is a function", bn, f.name), format!("call it with parentheses: `{}.{}(…)`", bn, f.name));
                    } else {
                        self.unknown_std(bn, &f.name, span);
                    }
                    return Self::bad(span);
                }
                if self.struct_idx.contains_key(bn) {
                    self.errh("E010", span, format!("`{}` is a type; fields are read from a value", bn), "create a value: `let s = Type { … }` and read `s.field`");
                    return Self::bad(span);
                }
            }
        }
        let tb = self.ex(base, None, true);
        let bty = tb.ty.clone();
        let mk = |kind: TK, ty: Ty| TExpr { kind, ty, span };
        match &bty {
            t if is_unk(t) => Self::bad(span),
            Ty::Struct(s) => {
                let st = self.structs.iter().find(|x| &x.name == s).cloned();
                let Some(st) = st else { return Self::bad(span) };
                if let Some(fi) = st.fields.iter().find(|x| x.name == f.name) {
                    self.sym(f.span, Sym::Field { owner: s.clone(), name: f.name.clone(), ty: fi.ty.clone() });
                    return mk(TK::Field(Box::new(tb), f.name.clone()), fi.ty.clone());
                }
                if self.fn_idx.contains_key(&format!("{}.{}", s, f.name)) {
                    self.errh("E010", span, format!("`{}` is a method", f.name), format!("call it with parentheses: `.{}()`", f.name));
                    return Self::bad(span);
                }
                let mut c: Vec<String> = st.fields.iter().map(|x| x.name.clone()).collect();
                c.extend(self.fns.iter().filter(|x| x.method_of.as_deref() == Some(s)).map(|x| x.name.rsplit('.').next().unwrap_or("").to_string()));
                let d = self.err("E001", f.span, format!("struct `{}` has no field `{}`", s, f.name));
                if let Some(sg) = suggest(&f.name, c) {
                    d.help = Some(msg::did_you_mean(&sg));
                }
                Self::bad(span)
            }
            Ty::Error => match f.name.as_str() {
                "message" => {
                    self.sym(f.span, Sym::Prop { recv: Ty::Error, name: "message".into(), ty: Ty::Str });
                    mk(TK::Field(Box::new(tb), "message".into()), Ty::Str)
                }
                "code" => {
                    self.sym(f.span, Sym::Prop { recv: Ty::Error, name: "code".into(), ty: Ty::Int });
                    mk(TK::Field(Box::new(tb), "code".into()), Ty::Int)
                }
                _ => {
                    self.errh("E001", f.span, format!("the error has no field `{}`", f.name), "the fields are `err.message` and `err.code`");
                    Self::bad(span)
                }
            },
            Ty::Path => {
                if let Some((_, ty, _)) = stdlib::PATH_PROPS.iter().find(|(n, _, _)| *n == f.name) {
                    let ret = table_ty(ty, &Bind::default());
                    self.sym(f.span, Sym::Prop { recv: Ty::Path, name: f.name.clone(), ty: ret.clone() });
                    let call = TCall {
                        target: TTarget::Method { recv_ty: Ty::Path, name: f.name.clone() },
                        args: vec![TArg::Expr(tb)],
                        ret: ret.clone(),
                        fallible: false,
                        desc: format!("`.{}`", f.name),
                    };
                    return mk(TK::Call(Box::new(call)), ret);
                }
                self.no_prop(&bty, f, span)
            }
            Ty::Str => {
                if stdlib::PATH_PROPS.iter().any(|(n, _, _)| *n == f.name) {
                    self.errh(
                        "E010",
                        span,
                        format!("`.{}` is a property of `path`, and this is `str`", f.name),
                        "annotate the type as a path: `let p: path = …` (or take `p: path` as the parameter)",
                    );
                    return Self::bad(span);
                }
                self.no_prop(&bty, f, span)
            }
            _ => self.no_prop(&bty, f, span),
        }
    }

    /// The help for something removed in v1 (`m.get`, `.is_empty()`, `.to_str()`, `p.exists`…), for type `ty`.
    fn removed_help(ty: &Ty, name: &str) -> Option<&'static str> {
        let path = if *ty == Ty::Path { stdlib::removed("path", name) } else { None };
        path.or_else(|| method_cat(ty).and_then(|c| stdlib::removed(c, name)))
    }

    fn no_prop(&mut self, ty: &Ty, f: &ast::Ident, span: Span) -> TExpr {
        if let Some(h) = Self::removed_help(ty, &f.name) {
            self.errh("E001", f.span, format!("`{}` has no `.{}`", tdisp(ty), f.name), h);
            return Self::bad(span);
        }
        let cat = method_cat(ty);
        if f.name == "len" || cat.map(|c| stdlib::find_method(c, &f.name).is_some()).unwrap_or(false) {
            self.errh("E010", span, format!("`{}` is a method", f.name), format!("call it with parentheses: `.{}()`", f.name));
        } else {
            let mut c: Vec<String> = cat.map(|c| stdlib::METHODS.iter().filter(|m| m.recv == c).map(|m| m.name.to_string()).collect()).unwrap_or_default();
            if *ty == Ty::Path {
                c.extend(stdlib::PATH_PROPS.iter().map(|(n, _, _)| n.to_string()));
            }
            let d = self.err("E001", f.span, format!("`{}` has no `.{}`", tdisp(ty), f.name));
            if let Some(s) = suggest(&f.name, c) {
                d.help = Some(msg::did_you_mean(&s));
            }
        }
        Self::bad(span)
    }

    fn unknown_std(&mut self, module: &str, name: &str, span: Span) {
        if let Some(h) = stdlib::removed(&format!("module:{}", module), name) {
            self.errh("E001", span, format!("`{}.{}` does not exist", module, name), h);
            return;
        }
        let cands: Vec<String> = stdlib::STDLIB.iter().filter(|f| f.module == module).map(|f| f.name.to_string()).collect();
        let d = self.err("E001", span, format!("`{}.{}` does not exist", module, name));
        d.help = Some(match suggest(name, cands.clone()) {
            Some(s) => msg::did_you_mean(&format!("{}.{}", module, s)),
            None => format!("functions in `{}`: {}", module, cands.join(", ")),
        });
    }

    fn map_get(&mut self, m: TExpr, k: TExpr, v: Ty, span: Span) -> TExpr {
        let call = TCall {
            target: TTarget::Method { recv_ty: m.ty.clone(), name: "get".into() },
            args: vec![TArg::Expr(m), TArg::Expr(k)],
            ret: v.clone(),
            fallible: true,
            desc: "`m[k]`".into(),
        };
        TExpr { kind: TK::Call(Box::new(call)), ty: v, span }
    }

    fn binary(&mut self, op: ast::BinOp, l: TExpr, r: TExpr, span: Span) -> TExpr {
        use ast::BinOp as A;
        if is_unk(&l.ty) || is_unk(&r.ty) {
            return Self::bad(span);
        }
        let lt = l.ty.clone();
        let rt = r.ty.clone();
        let mk = |op: BinOp, ty: Ty, l: TExpr, r: TExpr| TExpr { kind: TK::Binary(op, Box::new(l), Box::new(r)), ty, span };
        let sym = op.text();
        match op {
            A::And | A::Or => {
                if lt != Ty::Bool || rt != Ty::Bool {
                    let bad = if lt != Ty::Bool { &lt } else { &rt };
                    self.errh("E010", span, format!("`{}` needs `bool` on both sides, but one side is `{}`", sym, tdisp(bad)), "compare explicitly (`x != 0`, `s != \"\"`)");
                    return Self::bad(span);
                }
                mk(if op == A::And { BinOp::And } else { BinOp::Or }, Ty::Bool, l, r)
            }
            A::Add | A::Sub | A::Mul | A::Div | A::Mod => {
                if lt == Ty::Secret || rt == Ty::Secret {
                    self.errh("E050", span, msg::SECRET_TEXT, msg::SECRET_HELP);
                    return Self::bad(span);
                }
                let iop = match op {
                    A::Add => BinOp::Add,
                    A::Sub => BinOp::Sub,
                    A::Mul => BinOp::Mul,
                    A::Div => BinOp::Div,
                    _ => BinOp::Mod,
                };
                match (op, &lt, &rt) {
                    (_, Ty::Int, Ty::Int) => mk(iop, Ty::Int, l, r),
                    (A::Add | A::Sub | A::Mod, Ty::Duration, Ty::Duration) => mk(iop, Ty::Duration, l, r),
                    (A::Mul, Ty::Duration, Ty::Int) | (A::Mul, Ty::Int, Ty::Duration) | (A::Div, Ty::Duration, Ty::Int) => mk(iop, Ty::Duration, l, r),
                    (A::Div, Ty::Duration, Ty::Duration) => mk(iop, Ty::Int, l, r),
                    (A::Div, Ty::Path, _) => {
                        if texty(&rt) {
                            mk(BinOp::PathJoin, Ty::Path, l, r)
                        } else {
                            self.errh("E010", span, format!("`path / {}` does not exist", tdisp(&rt)), "the right side of `/` on a path is text: `dir / \"{n}.txt\"`");
                            Self::bad(span)
                        }
                    }
                    (A::Div, Ty::Str, _) => {
                        self.errh(
                            "E043",
                            span,
                            "`/` joins paths and the left side must be `path`, but it is `str`",
                            "annotate the type (`let base: path = \"dist\"`) or start from a function that returns `path` (`dirs.home() / …`)",
                        );
                        Self::bad(span)
                    }
                    (A::Add, a, b) if (*a == Ty::Str || *b == Ty::Str) && stringable(a) && stringable(b) => mk(BinOp::Concat, Ty::Str, l, r),
                    (A::Add, Ty::Path, b) if stringable(b) => mk(BinOp::Concat, Ty::Str, l, r),
                    (_, Ty::Duration, Ty::Int) | (_, Ty::Int, Ty::Duration) => {
                        let help = if matches!(r.kind, TK::Int(_)) || matches!(l.kind, TK::Int(_)) { msg::DURATION_UNIT_HELP } else { "convert it: `d / 1s` gives the seconds (`int`)" };
                        self.errh("E020", span, format!("`duration {} int` mixes units", sym), help);
                        Self::bad(span)
                    }
                    (_, Ty::Duration, Ty::Duration) => {
                        self.errh("E020", span, format!("`duration {} duration` makes no sense", sym), "durations can be added, subtracted and divided by each other; multiply by an `int`");
                        Self::bad(span)
                    }
                    (A::Add, Ty::List(_), _) | (A::Add, _, Ty::List(_)) => {
                        self.errh("E010", span, "lists cannot be added with `+`", "to join two lists use `xs.concat(ys)`; to append an item use `xs.push(x)` (on a `var`)");
                        Self::bad(span)
                    }
                    _ => {
                        self.err("E010", span, format!("`{}` does not work between `{}` and `{}`", sym, tdisp(&lt), tdisp(&rt)));
                        Self::bad(span)
                    }
                }
            }
            A::Eq | A::Ne | A::Lt | A::Le | A::Gt | A::Ge => {
                let iop = match op {
                    A::Eq => BinOp::Eq,
                    A::Ne => BinOp::Ne,
                    A::Lt => BinOp::Lt,
                    A::Le => BinOp::Le,
                    A::Gt => BinOp::Gt,
                    _ => BinOp::Ge,
                };
                // os == "…"
                for (a, b) in [(&l, &r), (&r, &l)] {
                    if matches!(a.kind, TK::Os) {
                        if let TK::Str(s) = &b.kind {
                            self.check_os_name(s, b.span);
                        }
                    }
                }
                let ordering = !matches!(op, A::Eq | A::Ne);
                match (&lt, &rt) {
                    (Ty::Duration, Ty::Int) | (Ty::Int, Ty::Duration) => {
                        let lit = if let TK::Int(n) = r.kind { Some(n) } else if let TK::Int(n) = l.kind { Some(n) } else { None };
                        let help = match lit {
                            Some(n) => format!("write the unit: `{}ms` or `{}s`", n, n),
                            None => "convert it: `d / 1s` gives the seconds (`int`)".into(),
                        };
                        self.errh("E020", span, format!("`{}` between `duration` and `int` mixes units", sym), help);
                        Self::bad(span)
                    }
                    (Ty::List(_) | Ty::Map(_, _) | Ty::Struct(_), _) | (_, Ty::List(_) | Ty::Map(_, _) | Ty::Struct(_)) => {
                        self.errh("E011", span, format!("`{}` does not compare collections or structs", sym), "compare fields or sizes (`xs.len() == 0`, `a.name == b.name`)");
                        Self::bad(span)
                    }
                    (Ty::Secret, _) | (_, Ty::Secret) => {
                        self.errh("E011", span, "secrets cannot be compared", msg::SECRET_HELP);
                        Self::bad(span)
                    }
                    (a, b) if ordering => {
                        let ok = (a == b && matches!(a, Ty::Int | Ty::Duration | Ty::Str | Ty::Path)) || (texty(a) && texty(b));
                        if !ok {
                            self.errh("E011", span, format!("`{}` does not compare `{}` with `{}`", sym, tdisp(a), tdisp(b)), "`<` and `>` compare numbers, durations or text (of the same type)");
                            return Self::bad(span);
                        }
                        mk(iop, Ty::Bool, l, r)
                    }
                    (a, b) => {
                        if a == b || (texty(a) && texty(b)) {
                            if matches!(a, Ty::Unit | Ty::Error) {
                                self.err("E011", span, format!("`{}` does not compare `{}`", sym, tdisp(a)));
                                return Self::bad(span);
                            }
                            return mk(iop, Ty::Bool, l, r);
                        }
                        let help = match (a, b) {
                            (Ty::Enum(e), Ty::Str) | (Ty::Str, Ty::Enum(e)) => format!("compare with a case (`{}.x`) or convert the text with `{}.parse(text)`", e, e),
                            (Ty::Int, Ty::Str) | (Ty::Str, Ty::Int) => "convert it: `text.to_int()` or `\"{n}\"`".into(),
                            _ => "both sides of `==` must have the same type".into(),
                        };
                        self.errh("E011", span, format!("`{}` between different types: `{}` and `{}`", sym, tdisp(a), tdisp(b)), help);
                        Self::bad(span)
                    }
                }
            }
        }
    }

    fn check_os_name(&mut self, s: &str, span: Span) {
        if !matches!(s, "linux" | "macos" | "windows") {
            let h = match suggest(s, ["linux".to_string(), "macos".to_string(), "windows".to_string()]) {
                Some(sg) => format!("did you mean `\"{}\"`? (the values are `\"linux\"`, `\"macos\"` and `\"windows\"`)", sg),
                None => "the values of `os` are `\"linux\"`, `\"macos\"` and `\"windows\"`".into(),
            };
            self.errh("E051", span, format!("`os` is never `\"{}\"`", s), h);
        }
    }

    fn list(&mut self, items: &[ast::Expr], exp: Option<&Ty>, span: Span) -> TExpr {
        let elem_exp = match exp {
            Some(Ty::List(t)) => Some((**t).clone()),
            _ => None,
        };
        if items.is_empty() {
            return match elem_exp {
                Some(t) => TExpr { kind: TK::List(vec![]), ty: Ty::list(t), span },
                None => {
                    if !exp.map(is_unk).unwrap_or(false) {
                        self.errh("E010", span, "an empty list needs a type", "annotate the type: `var xs: []str = []`");
                    }
                    Self::bad(span)
                }
            };
        }
        let mut out = Vec::new();
        let mut elem = elem_exp.clone();
        for it in items {
            let te = self.ex(it, elem.as_ref(), true);
            match &elem {
                Some(t) => {
                    if !compat(t, &te.ty) {
                        let m = format!("the list items are `{}`, but this one is `{}`", tdisp(t), tdisp(&te.ty));
                        self.errh("E010", te.span, m, "all items of a list must have the same type");
                    } else if is_unk(t) {
                        elem = Some(te.ty.clone());
                    }
                }
                None => elem = Some(te.ty.clone()),
            }
            out.push(te);
        }
        let elem = elem.unwrap_or_else(unk);
        if matches!(elem, Ty::List(_) | Ty::Map(_, _)) && !inner_list_ok(&elem) {
            self.errh("E090", span, format!("a list of `{}` is not supported yet", tdisp(&elem)), "a list can hold lists of scalars (`[][]str`); lists of maps and deeper nesting are planned");
            return Self::bad(span);
        }
        if matches!(elem, Ty::Unit | Ty::Error) {
            self.err("E010", span, format!("a list of `{}` does not exist", tdisp(&elem)));
            return Self::bad(span);
        }
        TExpr { kind: TK::List(out), ty: Ty::list(elem), span }
    }

    fn map_lit(&mut self, entries: &[(ast::Expr, ast::Expr)], exp: Option<&Ty>, span: Span) -> TExpr {
        let (mut kt, mut vt) = match exp {
            Some(Ty::Map(k, v)) => (Some((**k).clone()), Some((**v).clone())),
            _ => (None, None),
        };
        let mut out = Vec::new();
        for (k, v) in entries {
            let tk = self.ex(k, kt.as_ref(), true);
            let tv = self.ex(v, vt.as_ref(), true);
            match &kt {
                Some(t) => {
                    self.expect_ty(&tk, &t.clone(), "the map key");
                }
                None => kt = Some(tk.ty.clone()),
            }
            match &vt {
                Some(t) => {
                    self.expect_ty(&tv, &t.clone(), "the map value");
                }
                None => vt = Some(tv.ty.clone()),
            }
            out.push((tk, tv));
        }
        let kt = kt.unwrap_or_else(unk);
        let vt = vt.unwrap_or_else(unk);
        if !is_unk(&kt) && !matches!(kt, Ty::Str | Ty::Int | Ty::Enum(_)) {
            self.errh("E010", span, format!("map keys are `str`, `int` or an enum, not `{}`", tdisp(&kt)), "use quoted text as the key: `{ \"api\": 1 }`");
            return Self::bad(span);
        }
        if !is_unk(&vt) && (!vt.is_scalar() || vt == Ty::Secret) && !inner_list_ok(&vt) {
            self.errh("E090", span, format!("maps with `{}` values are not supported yet", tdisp(&vt)), "map values are scalars or lists of scalars (`map[str][]str`)");
            return Self::bad(span);
        }
        TExpr { kind: TK::Map(out), ty: Ty::map(kt, vt), span }
    }

    fn struct_lit(&mut self, sname: String, spread: Option<&ast::Expr>, fields: &[(ast::Ident, ast::Expr)], span: Span) -> TExpr {
        let Some(st) = self.struct_idx.get(&sname).map(|&i| self.structs[i].clone()) else { return Self::bad(span) };
        let sty = Ty::Struct(sname.clone());
        let tspread = spread.map(|s| {
            let te = self.ex(s, Some(&sty), true);
            self.expect_ty(&te, &sty, "the `..base`");
            Box::new(te)
        });
        let mut out: Vec<(String, TExpr)> = Vec::new();
        for (f, v) in fields {
            let Some(fi) = st.fields.iter().find(|x| x.name == f.name) else {
                let c: Vec<String> = st.fields.iter().map(|x| x.name.clone()).collect();
                let d = self.err("E012", f.span, format!("struct `{}` has no field `{}`", sname, f.name));
                d.help = Some(match suggest(&f.name, c.clone()) {
                    Some(s) => msg::did_you_mean(&s),
                    None => format!("the fields are: {}", c.join(", ")),
                });
                self.ex(v, None, true);
                continue;
            };
            self.sym(f.span, Sym::Field { owner: sname.clone(), name: f.name.clone(), ty: fi.ty.clone() });
            if out.iter().any(|(n, _)| *n == f.name) {
                self.errh("E012", f.span, format!("field `{}` appears twice", f.name), "remove the duplicate");
                continue;
            }
            let te = self.ex(v, Some(&fi.ty), true);
            self.expect_ty(&te, &fi.ty, &format!("field `{}`", f.name));
            out.push((f.name.clone(), te));
        }
        if tspread.is_none() {
            let missing: Vec<String> = st.fields.iter().filter(|fi| fi.default.is_none() && !out.iter().any(|(n, _)| *n == fi.name)).map(|fi| fi.name.clone()).collect();
            if !missing.is_empty() {
                let lista = missing.iter().map(|m| format!("`{}`", m)).collect::<Vec<_>>().join(", ");
                self.errh(
                    "E012",
                    span,
                    format!("missing field{} {} in `{}`", if missing.len() > 1 { "s" } else { "" }, lista, sname),
                    format!("add {} or start from another value with `..base`", missing.iter().map(|m| format!("`{}: …`", m)).collect::<Vec<_>>().join(", ")),
                );
            }
        }
        TExpr { kind: TK::Struct { name: sname, spread: tspread, fields: out }, ty: sty, span }
    }

    fn match_expr(&mut self, subject: &ast::Expr, arms: &[ast::MatchArm], span: Span, exp: Option<&Ty>, value: bool) -> TExpr {
        let ts = self.ex(subject, None, true);
        let sty = ts.ty.clone();
        let is_os = matches!(ts.kind, TK::Os);
        let enum_cases: Option<(String, Vec<String>)> = match &sty {
            Ty::Enum(e) => self.enum_idx.get(e).map(|&i| (e.clone(), self.enums[i].cases.clone())),
            _ => None,
        };
        if matches!(sty, Ty::List(_) | Ty::Map(_, _) | Ty::Secret | Ty::Unit | Ty::Error) && !is_unk(&sty) {
            self.errh("E010", ts.span, format!("`match` does not work with `{}`", tdisp(&sty)), "`match` compares text, numbers, durations, `bool`, enums and structs");
        }
        let mut covered: HashSet<String> = HashSet::new();
        let mut has_default = false;
        let mut all_arms: Vec<(TArm, bool)> = Vec::new();
        let mut acc: Option<Ty> = None;
        let mut reached_all = false;
        for arm in arms {
            if reached_all {
                self.errh("E031", arm.span, "this arm is never reached: an arm above already matches every value", "move it above that arm, or remove it");
            }
            self.push_scope();
            self.enter_block(arm.span.end);
            let mut pats = Vec::new();
            let mut arm_cov: Vec<String> = Vec::new();
            for p in &arm.patterns {
                match p {
                    ast::Pattern::Wildcard(_) => pats.push(TPattern::Any),
                    ast::Pattern::Case(id) if enum_cases.is_none() && !is_os && !is_unk(&sty) => {
                        if arm.patterns.len() > 1 {
                            self.errh("E010", id.span, format!("`{}` takes the value, so it can't be combined with `|`", id.name), "give the name its own arm: `n => …`");
                        }
                        let v = self.declare(id, sty.clone(), false, VarKind::Loop);
                        pats.push(TPattern::Bind(v));
                    }
                    ast::Pattern::Struct { name, fields, span: ps } => {
                        if is_os && !is_unk(&sty) || (!matches!(&sty, Ty::Struct(s) if *s == name.name) && !is_unk(&sty)) {
                            self.errh("E010", *ps, format!("the pattern is `{}`, but the value is `{}`", name.name, tdisp(&sty)), "the pattern must have the type of the `match` value");
                            continue;
                        }
                        if let Ty::Struct(sn) = &sty {
                            let fs = self.struct_fields_pattern(sn, fields, arm.patterns.len() > 1);
                            pats.push(TPattern::Struct(fs));
                        }
                    }
                    _ => {
                        if let Some(tp) = self.scalar_pattern(p, &sty, enum_cases.as_ref(), is_os, &mut arm_cov) {
                            pats.push(tp);
                        }
                    }
                }
            }
            let guard = arm.guard.as_ref().map(|g| {
                let tg = self.ex(g, Some(&Ty::Bool), true);
                self.expect_ty(&tg, &Ty::Bool, "the `if` of a `match` arm");
                if is_os {
                    self.errh("E010", tg.span, "`match os` arms can't have `if`", "use `if os == \"linux\" && …`");
                }
                tg
            });
            let (body, bty) = if value { self.value_block(&arm.body, exp.or(acc.as_ref())) } else { (TBlock { stmts: self.block_stmts(&arm.body), value: None }, None) };
            if value {
                self.unify(&mut acc, bty, arm.span);
            }
            self.leave_block();
            self.pop_scope();
            // an arm with `if` covers nothing for sure
            let all = guard.is_none() && pats.iter().any(TPattern::irrefutable);
            if guard.is_none() {
                covered.extend(arm_cov);
            }
            if all {
                has_default = true;
                reached_all = true;
            }
            if !pats.is_empty() {
                all_arms.push((TArm { patterns: pats, guard, body }, all));
            }
        }
        // a plain `match` (literals, ranges, cases and `_`) keeps the backends' `case`; the others are tested in order
        let ordered = all_arms.iter().any(|(a, _)| a.guard.is_some() || a.patterns.iter().any(|p| matches!(p, TPattern::Bind(_) | TPattern::Struct(_))));
        let mut tarms = Vec::new();
        let mut default = None;
        for (a, all) in all_arms {
            if ordered {
                tarms.push(a);
            } else if all {
                if default.is_none() {
                    default = Some(a.body);
                }
            } else {
                tarms.push(a);
            }
        }
        let mut exhaustive = has_default;
        if let Some((en, cases)) = &enum_cases {
            let missing: Vec<&String> = cases.iter().filter(|c| !covered.contains(*c)).collect();
            if missing.is_empty() {
                exhaustive = true;
            } else if !has_default {
                let lista = missing.iter().map(|m| format!("`{}`", m)).collect::<Vec<_>>().join(", ");
                self.errh("E031", span, format!("`match` does not cover every case of `{}`: missing case{} {}", en, if missing.len() > 1 { "s" } else { "" }, lista), "add the missing arms or `_ => …`");
            }
        }
        if is_os && ["linux", "macos", "windows"].iter().all(|o| covered.contains(*o)) {
            exhaustive = true;
        }
        if sty == Ty::Bool && covered.contains("true") && covered.contains("false") {
            exhaustive = true;
        }
        if value && !exhaustive && enum_cases.is_none() && !is_unk(&sty) {
            let m = if is_os {
                let falta: Vec<&str> = ["linux", "macos", "windows"].into_iter().filter(|o| !covered.contains(*o)).collect();
                format!("`match os` used as a value does not cover {}", falta.iter().map(|f| format!("`\"{}\"`", f)).collect::<Vec<_>>().join(", "))
            } else {
                "`match` used as a value needs a `_` arm".to_string()
            };
            self.errh("E031", span, m, "add `_ => value`");
        }
        let ty = if value { acc.or_else(|| exp.cloned()).unwrap_or_else(unk) } else { Ty::Unit };
        TExpr { kind: TK::Match(Box::new(TMatch { subject: ts, arms: tarms, default, is_os, exhaustive, ordered })), ty, span }
    }

    /// A literal, range or enum case pattern for a value of type `sty` (also used for struct fields).
    fn scalar_pattern(&mut self, p: &ast::Pattern, sty: &Ty, enum_cases: Option<&(String, Vec<String>)>, is_os: bool, cov: &mut Vec<String>) -> Option<TPattern> {
        match p {
            ast::Pattern::Case(id) => match enum_cases {
                Some((en, cases)) => {
                    if cases.contains(&id.name) {
                        self.sym(id.span, Sym::Case(en.clone(), id.name.clone()));
                        cov.push(id.name.clone());
                        Some(TPattern::Lit(TExpr { kind: TK::EnumCase(en.clone(), id.name.clone()), ty: sty.clone(), span: id.span }))
                    } else {
                        let d = self.err("E001", id.span, format!("enum `{}` has no case `{}`", en, id.name));
                        d.help = Some(match suggest(&id.name, cases.clone()) {
                            Some(s) => msg::did_you_mean(&s),
                            None => format!("the cases are: {}", cases.join(", ")),
                        });
                        None
                    }
                }
                None => {
                    if !is_unk(sty) {
                        self.errh("E010", id.span, format!("`{}` is not a pattern for `{}`", id.name, tdisp(sty)), "`match os` arms are `\"linux\"`, `\"macos\"`, `\"windows\"` or `_`");
                    }
                    None
                }
            },
            ast::Pattern::Lit(le) => {
                let te = self.ex(le, None, true);
                if !compat(sty, &te.ty) || matches!(sty, Ty::Enum(_)) {
                    if !is_unk(sty) {
                        let help = if matches!(sty, Ty::Enum(_)) { "use the cases without the type name: `prod => …`" } else { "the pattern must have the type of the `match` value" };
                        self.errh("E010", te.span, format!("the pattern is `{}`, but the value is `{}`", tdisp(&te.ty), tdisp(sty)), help);
                    }
                    return None;
                }
                match &te.kind {
                    TK::Str(s) => {
                        if is_os {
                            self.check_os_name(s, te.span);
                        }
                        cov.push(s.clone());
                    }
                    TK::Bool(b) => cov.push(b.to_string()),
                    _ => {}
                }
                Some(TPattern::Lit(te))
            }
            ast::Pattern::Range { from, to, inclusive, span: ps } => {
                if *sty != Ty::Int && !is_unk(sty) {
                    self.errh("E010", *ps, format!("ranges only work for `int`, and the value is `{}`", tdisp(sty)), "use literals of the right type");
                    return None;
                }
                if (*inclusive && from > to) || (!*inclusive && from >= to) {
                    self.errh("E010", *ps, "empty range", "the start comes before the end: `0..10` (excludes 10), `0..=10` (includes it)");
                }
                Some(TPattern::Range { from: *from, to: *to, inclusive: *inclusive })
            }
            ast::Pattern::Wildcard(_) => Some(TPattern::Any),
            ast::Pattern::Struct { span, .. } => {
                self.errh("E090", *span, "a struct pattern inside a struct pattern", "match the outer struct, then `match` the field in the arm");
                None
            }
        }
    }

    /// The fields of `Type { field: pattern, … }`: literals, ranges and cases on scalar fields; a name takes the field.
    fn struct_fields_pattern(&mut self, sname: &str, fields: &[(ast::Ident, ast::Pattern)], in_alt: bool) -> Vec<(String, TPattern)> {
        let Some(&si) = self.struct_idx.get(sname) else { return Vec::new() };
        let mut out: Vec<(String, TPattern)> = Vec::new();
        for (f, p) in fields {
            let Some(fi) = self.structs[si].fields.iter().find(|x| x.name == f.name).cloned() else {
                let names: Vec<String> = self.structs[si].fields.iter().map(|x| x.name.clone()).collect();
                let d = self.err("E001", f.span, format!("struct `{}` has no field `{}`", sname, f.name));
                d.help = Some(match suggest(&f.name, names.clone()) {
                    Some(s) => msg::did_you_mean(&s),
                    None => format!("the fields are: {}", names.join(", ")),
                });
                continue;
            };
            self.sym(f.span, Sym::Field { owner: sname.to_string(), name: f.name.clone(), ty: fi.ty.clone() });
            if out.iter().any(|(n, _)| *n == f.name) {
                self.errh("E012", f.span, format!("field `{}` appears twice", f.name), "remove the duplicate");
                continue;
            }
            let fty = fi.ty.clone();
            let tp = match p {
                ast::Pattern::Wildcard(_) => Some(TPattern::Any),
                ast::Pattern::Case(id) if !matches!(fty, Ty::Enum(_)) => {
                    if in_alt {
                        self.errh("E010", id.span, format!("`{}` takes the field, so the pattern can't be combined with `|`", id.name), "give it its own arm");
                    }
                    let v = self.declare(id, fty.clone(), false, VarKind::Loop);
                    Some(TPattern::Bind(v))
                }
                _ => {
                    if !fty.is_scalar() {
                        self.errh("E010", f.span, format!("field `{}` is `{}`: only `_` or a name can match it", f.name, tdisp(&fty)), "write `_`, or a name to use it in the arm");
                        continue;
                    }
                    let cases = match &fty {
                        Ty::Enum(e) => self.enum_idx.get(e).map(|&i| (e.clone(), self.enums[i].cases.clone())),
                        _ => None,
                    };
                    let mut cov = Vec::new();
                    self.scalar_pattern(p, &fty, cases.as_ref(), false, &mut cov)
                }
            };
            if let Some(tp) = tp {
                out.push((f.name.clone(), tp));
            }
        }
        out
    }

    fn or_expr(&mut self, lhs: &ast::Expr, h: &ast::Handler, span: Span, exp: Option<&Ty>, value: bool) -> TExpr {
        let tl = match &lhs.kind {
            EK::Try(b) => {
                let stmts = self.block_stmts(b);
                if value {
                    self.errh("E010", lhs.span, "`try` returns no value", "declare the variable beforehand with `var` and assign it inside the block");
                }
                TExpr { kind: TK::Try(TBlock { stmts, value: None }), ty: Ty::Unit, span: lhs.span }
            }
            _ => self.ex(lhs, exp, value),
        };
        let t = tl.ty.clone();
        let th = match h {
            ast::Handler::Ignore(hs) => {
                if t != Ty::Unit && !is_unk(&t) {
                    self.errh("E010", *hs, format!("`catch ignore` only works on something that returns no value, and this returns `{}`", tdisp(&t)), "use a default value: `… catch value`");
                }
                if let TK::Command(c) = &tl.kind {
                    if let Some(TWord::Lit(prog)) = c.stages.first().and_then(|s| s.first()) {
                        if matches!(prog.as_str(), "rm" | "mv" | "del" | "rmdir" | "Remove-Item") {
                            self.warn(
                                "W012",
                                *hs,
                                format!("`catch ignore` hides failures of `{}`, which changes files", prog),
                                Some("handle the failure with `catch { log.warn(\"…: {err}\") }` or let it fail".into()),
                            );
                        }
                    }
                }
                THandler::Ignore
            }
            ast::Handler::Value(v) => {
                if t == Ty::Unit {
                    self.errh("E010", v.span, "this returns no value, so there is nothing to replace", "use `catch ignore` or `catch { … }`");
                    self.ex(v, None, true);
                    THandler::Ignore
                } else {
                    let tv = self.ex(v, Some(&t), true);
                    self.expect_ty(&tv, &t, "the value after `catch`");
                    THandler::Default(tv)
                }
            }
            ast::Handler::Fail { message, code, .. } => {
                self.ctx.err_stack.push(Some(ErrBind::Current));
                let (m, c) = self.fail_args(message, code.as_ref());
                self.ctx.err_stack.pop();
                THandler::Fail { message: m, code: c }
            }
            ast::Handler::Block(b) => {
                let eid = self.new_var("err", Ty::Error, false, VarKind::Err, b.span);
                self.vars[eid].vis = (b.span.start, b.span.end);
                self.ctx.err_stack.push(Some(ErrBind::Var(eid)));
                let body = if value && t != Ty::Unit {
                    let (tb, bt) = self.value_block(b, Some(&t));
                    if let Some(bt) = bt {
                        if !compat(&t, &bt) {
                            let sp = tb.value.as_ref().map(|v| v.span).unwrap_or(b.span);
                            self.errh("E010", sp, format!("the `catch` block returns `{}`, but the value is `{}`", tdisp(&bt), tdisp(&t)), "the last line of the block is the value used instead");
                        }
                    }
                    tb
                } else {
                    TBlock { stmts: self.block_stmts(b), value: None }
                };
                self.ctx.err_stack.pop();
                let used = self.vars[eid].used;
                THandler::Block { err: if used { Some(eid) } else { None }, body }
            }
        };
        TExpr { kind: TK::Or(Box::new(tl), Box::new(th)), ty: t, span }
    }

    fn parallel(&mut self, max: Option<&ast::Expr>, body: &ast::ParBody, span: Span, exp: Option<&Ty>, value: bool) -> TExpr {
        let tmax = max.map(|m| {
            let te = self.ex(m, Some(&Ty::Int), true);
            self.expect_ty(&te, &Ty::Int, "`max`");
            Box::new(te)
        });
        let saved = self.ctx.clone();
        self.ctx.loop_depth = 0;
        self.ctx.in_parallel = true;
        let (tbody, ty) = match body {
            ast::ParBody::For { var, iter, body } => {
                let ti = self.ex(iter, None, true);
                let elem = match &ti.ty {
                    Ty::List(e) => (**e).clone(),
                    t if is_unk(t) => unk(),
                    t => {
                        let help = if matches!(iter.kind, EK::Range { .. }) { "build a list first; `parallel for` iterates over lists" } else { "`parallel for` iterates over lists" };
                        self.errh("E010", iter.span, format!("`parallel for` does not iterate over `{}`", tdisp(t)), help);
                        unk()
                    }
                };
                self.ctx.par_base = Some(self.scopes.len());
                self.push_scope();
                self.enter_block(body.span.end);
                let vid = self.declare(var, elem, false, VarKind::Loop);
                let (tb, ty) = if value {
                    let item_exp = match exp {
                        Some(Ty::List(t)) => Some((**t).clone()),
                        _ => None,
                    };
                    let (tb, t) = self.value_block(body, item_exp.as_ref());
                    let t = t.unwrap_or_else(unk);
                    if t == Ty::Unit {
                        self.errh("E010", span, "the last line of the `parallel for` produces no value", "to just run it, do not store the result: `parallel for x in xs { … }`");
                    }
                    if matches!(t, Ty::List(_) | Ty::Map(_, _)) {
                        self.errh("E090", span, "`parallel for` that returns lists or maps is not supported yet", "return a scalar or a struct per task");
                    }
                    (tb, Ty::list(t))
                } else {
                    (TBlock { stmts: self.block_stmts(body), value: None }, Ty::Unit)
                };
                self.leave_block();
                self.pop_scope();
                (TPar::For { var: vid, list: ti, body: tb }, ty)
            }
            ast::ParBody::Block(b) => {
                self.ctx.par_base = Some(self.scopes.len());
                let stmts = self.block_stmts(b);
                if value {
                    self.errh("E010", span, "`parallel { … }` returns no value", "to collect results use `let r = parallel for x in xs { value }`");
                }
                (TPar::Tasks(stmts), Ty::Unit)
            }
        };
        let err_stack = std::mem::take(&mut self.ctx.err_stack);
        self.ctx = saved;
        self.ctx.err_stack = err_stack;
        TExpr { kind: TK::Parallel { max: tmax, body: Box::new(tbody) }, ty, span }
    }

    // ---------------------------------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------------------------------

    fn cmd(&mut self, c: &ast::CmdAst) -> TCmd {
        let mut stages = Vec::new();
        let mut prog_spans = Vec::new();
        let mut envs = Vec::new();
        for st in &c.stages {
            // `NAME=value` words before the program: variables for that program only
            let mut env = Vec::new();
            let mut st: &[ast::CmdWord] = st;
            while let Some((name, value)) = st.first().and_then(env_assignment) {
                if st.len() == 1 {
                    self.errh(
                        "E005",
                        st[0].span,
                        format!("`{}=…` needs a program after it", name),
                        format!("`NAME=value program` sets a variable for that program; for the rest of the script use `env.set(\"{}\", …)`", name),
                    );
                    break;
                }
                if let Some(tw) = self.word_in(&value, false, true) {
                    env.push((name, tw));
                }
                st = &st[1..];
            }
            envs.push(env);
            prog_spans.push(st.first().map(|w| w.span).unwrap_or(c.span));
            let mut words = Vec::new();
            for w in st {
                if let Some(tw) = self.word(w, true) {
                    words.push(tw);
                }
            }
            stages.push(words);
        }
        let mut redirects = Vec::new();
        for r in &c.redirects {
            let target = r.target.as_ref().map(|w| self.word_expr(w));
            let t = |x: Option<TExpr>| x.unwrap_or_else(|| Self::bad(r.span));
            redirects.push(match r.kind {
                ast::RedirKind::Out => TRedir::Out { path: t(target), append: false },
                ast::RedirKind::Append => TRedir::Out { path: t(target), append: true },
                ast::RedirKind::Err => TRedir::Err { path: t(target), append: false },
                ast::RedirKind::ErrAppend => TRedir::Err { path: t(target), append: true },
                ast::RedirKind::In => TRedir::In(t(target)),
                ast::RedirKind::ErrToOut => TRedir::ErrToOut,
            });
        }
        let stdin = c.stdin.as_ref().map(|s| {
            let te = self.ex(s, None, true);
            if !(stringable(&te.ty) || te.ty == Ty::Secret || is_unk(&te.ty)) || te.ty == Ty::Error {
                self.expect_text(&te, false);
            }
            if te.ty == Ty::Duration {
                self.errh("E021", te.span, "a bare duration cannot go into a command", "write the number in the unit the program expects: `d / 1s`");
            }
            te
        });
        TCmd { stages, envs, prog_spans, redirects, stdin, text: c.text.clone(), span: c.span }
    }

    /// A command word. `allow_spread`: a lone `{list}` becomes several arguments.
    fn word(&mut self, w: &ast::CmdWord, allow_spread: bool) -> Option<TWord> {
        self.word_in(w, allow_spread, false)
    }

    /// `env`: the word is the value of a `NAME=value` before the program. A secret is fine there:
    /// the environment of a program, unlike its arguments, is not in the process list.
    fn word_in(&mut self, w: &ast::CmdWord, allow_spread: bool, env: bool) -> Option<TWord> {
        if w.parts.iter().all(|p| matches!(p, ast::WordPart::Lit(_))) {
            let s: String = w.parts.iter().map(|p| if let ast::WordPart::Lit(t) = p { t.as_str() } else { "" }).collect();
            return Some(TWord::Lit(s));
        }
        let single = w.parts.len() == 1;
        let mut parts = Vec::new();
        for p in &w.parts {
            match p {
                ast::WordPart::Lit(t) => parts.push(TPart::Lit(t.clone())),
                ast::WordPart::Expr { expr, quoted } => {
                    let te = self.ex(expr, None, true);
                    match &te.ty {
                        Ty::List(inner) => {
                            if single && !quoted && allow_spread {
                                if !inner.is_scalar() || matches!(**inner, Ty::Duration | Ty::Secret) {
                                    self.errh("E042", te.span, format!("a list of `{}` cannot become arguments", tdisp(inner)), "pass a list of text or numbers");
                                }
                                return Some(TWord::Spread(te));
                            }
                            self.errh(
                                "E042",
                                te.span,
                                "a list mixed with text in a command word",
                                "a `{list}` on its own (separated by spaces) becomes several arguments; for a single argument, `{list.join(\",\")}`",
                            );
                        }
                        Ty::Duration => {
                            self.errh("E021", te.span, "a bare duration as a command argument", "write the number in the unit the program expects: `{d / 1s}`");
                        }
                        Ty::Secret if env => {}
                        Ty::Secret => {
                            self.warn(
                                "W011",
                                te.span,
                                "a `secret` as a command argument shows up in the process list (`ps`)",
                                Some("prefer sending it through the input: `token | $ cmd --password-stdin`".into()),
                            );
                        }
                        _ => {
                            self.expect_text(&te, true);
                        }
                    }
                    parts.push(TPart::Expr(te));
                }
            }
        }
        Some(TWord::Interp(parts))
    }

    /// Redirection word as a text expression.
    fn word_expr(&mut self, w: &ast::CmdWord) -> TExpr {
        let tw = self.word(w, false);
        match tw {
            Some(TWord::Lit(s)) => TExpr { kind: TK::Str(s), ty: Ty::Path, span: w.span },
            Some(TWord::Interp(mut parts)) => {
                if parts.len() == 1 {
                    if let TPart::Expr(e) = &parts[0] {
                        if texty(&e.ty) {
                            if let TPart::Expr(e) = parts.pop().unwrap() {
                                return e;
                            }
                        }
                    }
                }
                TExpr { kind: TK::Interp(parts), ty: Ty::Str, span: w.span }
            }
            _ => Self::bad(w.span),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // Calls
    // ---------------------------------------------------------------------------------------------

    fn call(&mut self, callee: &ast::Expr, args: &[ast::Arg], span: Span, exp: Option<&Ty>) -> TExpr {
        match &callee.kind {
            EK::Ident(name) if self.lookup(name).is_none() => self.call_named(name, callee.span, args, span, exp),
            EK::Ident(name) => {
                self.errh("E010", callee.span, format!("`{}` is a variable, not a function", name), "functions are called by the name they were declared with");
                self.check_args_loose(args);
                Self::bad(span)
            }
            EK::Field(base, m) => {
                if let EK::Ident(bn) = &base.kind {
                    if self.lookup(bn).is_none() {
                        if Self::is_module(bn) {
                            self.sym(base.span, Sym::Module(bn.clone()));
                            return match stdlib::find_fn(bn, &m.name) {
                                Some(f) => {
                                    self.sym(m.span, Sym::Std(f));
                                    self.std_call(f, args, span)
                                }
                                None => {
                                    self.unknown_std(bn, &m.name, callee.span);
                                    self.check_args_loose(args);
                                    Self::bad(span)
                                }
                            };
                        }
                        if let Some(&ei) = self.enum_idx.get(bn) {
                            self.sym(base.span, Sym::Enum(bn.clone()));
                            if m.name == "parse" {
                                let en = Ty::Enum(bn.clone());
                                self.sym(m.span, Sym::Method { recv: en.clone(), name: "parse".into() });
                                let sig = Sig { params: vec![SP { name: "text".into(), ty: PT::Ty(Ty::Str), default: None }], variadic: false };
                                let (targs, _) = self.match_args(&format!("{}.parse", bn), &sig, args, span);
                                let call = TCall { target: TTarget::Builtin("enum_parse"), args: targs, ret: en.clone(), fallible: true, desc: format!("`{}.parse`", bn) };
                                return TExpr { kind: TK::Call(Box::new(call)), ty: en, span };
                            }
                            let cases = self.enums[ei].cases.clone();
                            if cases.contains(&m.name) {
                                self.errh("E010", span, format!("`{}.{}` is a case, not a function", bn, m.name), "remove the parentheses");
                            } else {
                                self.errh("E001", m.span, format!("enum `{}` has no `{}`", bn, m.name), format!("enums have the cases ({}) and `{}.parse(text)`", cases.join(", "), bn));
                            }
                            self.check_args_loose(args);
                            return Self::bad(span);
                        }
                        if self.struct_idx.contains_key(bn) {
                            self.errh("E010", span, format!("`{}` is a type; methods are called on a value", bn), format!("create a value (`let s = {} {{ … }}`) and call `s.{}()`", bn, m.name));
                            self.check_args_loose(args);
                            return Self::bad(span);
                        }
                    }
                }
                self.method_call(base, m, args, span)
            }
            _ => {
                self.err("E010", callee.span, "this is not a function");
                self.check_args_loose(args);
                Self::bad(span)
            }
        }
    }

    fn check_args_loose(&mut self, args: &[ast::Arg]) {
        for a in args {
            if !matches!(a.value.kind, EK::Lambda { .. }) {
                self.ex(&a.value, None, true);
            }
        }
    }

    fn call_named(&mut self, name: &str, nspan: Span, args: &[ast::Arg], span: Span, _exp: Option<&Ty>) -> TExpr {
        match name {
            "print" => self.sym(nspan, Sym::Builtin("print")),
            "fmt" => self.sym(nspan, Sym::Builtin("fmt")),
            "secret" => self.sym(nspan, Sym::Builtin("secret")),
            "embed" => self.sym(nspan, Sym::Builtin("embed")),
            _ => {}
        }
        if let Some(&fi) = self.fn_idx.get(name) {
            self.sym(nspan, Sym::Fn(fi));
            let f = self.fns[fi].clone();
            let sig = Sig { params: f.params.iter().map(|p| SP { name: p.name.clone(), ty: PT::Ty(p.ty.clone()), default: p.default.clone() }).collect(), variadic: false };
            let (targs, _) = self.match_args(&format!("`{}`", name), &sig, args, span);
            let call = TCall { target: TTarget::User(fi), args: targs, ret: f.ret.clone(), fallible: false, desc: format!("`{}`", name) };
            return TExpr { kind: TK::Call(Box::new(call)), ty: f.ret, span };
        }
        match name {
            "print" | "fmt" | "secret" => {
                let f = stdlib::find_builtin(name).expect("builtin is in the table");
                let sig = std_sig(f.params, &Bind::default(), span);
                let (targs, _) = self.match_args(&format!("`{}`", name), &sig, args, span);
                if name == "fmt" {
                    if let Some(TArg::Expr(TExpr { kind: TK::Str(t), .. })) = targs.first() {
                        let holes = t.matches("{}").count();
                        if holes != targs.len() - 1 {
                            self.errh(
                                "E012",
                                span,
                                format!("the `fmt` template has {} `{{}}`, but got {} value(s)", holes, targs.len() - 1),
                                "each `{}` in the template takes the next argument",
                            );
                        }
                    }
                }
                let ret = table_ty(f.ret, &Bind::default());
                let bname: &'static str = f.name;
                let call = TCall { target: TTarget::Builtin(bname), args: targs, ret: ret.clone(), fallible: f.fallible, desc: format!("`{}`", name) };
                TExpr { kind: TK::Call(Box::new(call)), ty: ret, span }
            }
            "embed" => self.embed(args, span),
            "exit" => {
                self.errh("E010", span, "`exit(…)` is a statement", "use `exit(code)` on its own line (or inside `catch { exit(1) }`)");
                self.check_args_loose(args);
                Self::bad(span)
            }
            "assert" | "assert_fails" => {
                self.errh("E060", span, format!("`{}` is a test statement", name), "use it on its own line inside `test \"…\" { }`");
                Self::bad(span)
            }
            "duration_str" | "enum_parse" => {
                self.unknown_name(name, nspan);
                Self::bad(span)
            }
            _ => {
                if Self::is_module(name) {
                    self.errh("E010", nspan, format!("`{}` is a module, not a function", name), format!("call one of its functions: `{}.…(…)`", name));
                } else if self.struct_idx.contains_key(name) {
                    self.errh("E010", nspan, format!("`{}` is a struct", name), format!("create a value with `{} {{ field: value }}`", name));
                } else {
                    self.unknown_name(name, nspan);
                }
                self.check_args_loose(args);
                Self::bad(span)
            }
        }
    }

    fn embed(&mut self, args: &[ast::Arg], span: Span) -> TExpr {
        let lit = match args {
            [a] if a.name.is_none() => match &a.value.kind {
                EK::Str(parts) if parts.iter().all(|p| matches!(p, ast::StrPart::Lit(_))) => {
                    Some(parts.iter().map(|p| if let ast::StrPart::Lit(t) = p { t.as_str() } else { "" }).collect::<String>())
                }
                EK::RawStr(s) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(rel) = lit else {
            self.check_args_loose(args);
            self.errh("E053", span, "`embed` takes the path as fixed text", "write `embed(\"dir/file.txt\")`, relative to the `.nalu` file");
            return Self::bad(span);
        };
        let path = self.embed_root.join(&rel);
        match std::fs::read(&path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(mut text) => {
                    if text.starts_with('\u{feff}') {
                        text.remove(0);
                    }
                    TExpr { kind: TK::Str(text), ty: Ty::Str, span }
                }
                Err(_) => {
                    self.errh("E053", span, format!("`{}` is not UTF-8 text", rel), "`embed` only includes text files");
                    Self::bad(span)
                }
            },
            Err(_) => {
                self.errh("E053", span, format!("file `{}` does not exist (looked for at `{}`)", rel, path.display()), "the path is relative to the folder of the `.nalu` file");
                Self::bad(span)
            }
        }
    }

    fn std_call(&mut self, f: &'static StdFn, args: &[ast::Arg], span: Span) -> TExpr {
        let sig = std_sig(f.params, &Bind::default(), span);
        let what = format!("`{}.{}`", f.module, f.name);
        let (targs, _) = self.match_args(&what, &sig, args, span);
        if !f.v1 {
            // Planned (v0.2): not implemented in any backend. Refuse here, not at run time.
            self.errh("E090", span, format!("{} is not implemented yet", what), "for now, use an external command with `$` or a `native` block");
        }
        if f.module == "regex" {
            if let Some(TArg::Expr(p)) = targs.get(1) {
                self.check_regex(f.name, p);
            }
        }
        let ret = table_ty(f.ret, &Bind::default());
        let call = TCall { target: TTarget::Std(f.module, f.name), args: targs, ret: ret.clone(), fallible: f.fallible, desc: what };
        TExpr { kind: TK::Call(Box::new(call)), ty: ret, span }
    }

    /// A `regex` pattern: fixed text, valid, and for functions that return matched text, a single way to match (D32).
    fn check_regex(&mut self, name: &str, p: &TExpr) {
        let TK::Str(src) = &p.kind else {
            if !is_unk(&p.ty) {
                self.errh(
                    "E054",
                    p.span,
                    "the pattern must be fixed text written in the call",
                    "write it in single quotes, like `regex.find(s, '\\d+')`: the compiler checks it and translates it for each target",
                );
            }
            return;
        };
        let r = match crate::regex::Regex::parse(src) {
            Ok(r) => r,
            Err(e) => {
                self.errh("E054", p.span, format!("invalid pattern: {}", e.message), e.help);
                return;
            }
        };
        if name != "test" {
            if let Err(e) = r.check_unambiguous() {
                self.errh("E054", p.span, e.message, format!("{}; `regex.test` accepts any pattern", e.help));
                return;
            }
        }
        if matches!(name, "find_all" | "replace") && r.can_match_empty() {
            self.errh("E054", p.span, format!("`regex.{}` needs a pattern that can't match empty text", name), "make it match at least one character, like `\\d+` instead of `\\d*`");
        }
        if name == "groups" && r.groups == 0 {
            self.errh("E054", p.span, "the pattern has no groups `( … )`", "put the parts you want in parentheses, or use `regex.find`");
        }
    }

    fn method_call(&mut self, base: &ast::Expr, m: &ast::Ident, args: &[ast::Arg], span: Span) -> TExpr {
        let recv = self.ex(base, None, true);
        let rty = recv.ty.clone();
        if is_unk(&rty) {
            self.check_args_loose(args);
            return Self::bad(span);
        }
        // struct methods
        if let Ty::Struct(s) = &rty {
            let key = format!("{}.{}", s, m.name);
            if let Some(&fi) = self.fn_idx.get(&key) {
                self.sym(m.span, Sym::Fn(fi));
                let f = self.fns[fi].clone();
                let sig = Sig { params: f.params.iter().map(|p| SP { name: p.name.clone(), ty: PT::Ty(p.ty.clone()), default: p.default.clone() }).collect(), variadic: false };
                let (mut targs, _) = self.match_args(&format!("`{}`", key), &sig, args, span);
                targs.insert(0, TArg::Expr(recv));
                let call = TCall { target: TTarget::User(fi), args: targs, ret: f.ret.clone(), fallible: false, desc: format!("`{}`", key) };
                return TExpr { kind: TK::Call(Box::new(call)), ty: f.ret, span };
            }
            let st = self.structs.iter().find(|x| &x.name == s).cloned();
            let is_field = st.as_ref().map(|st| st.fields.iter().any(|x| x.name == m.name)).unwrap_or(false);
            if is_field {
                self.errh("E010", m.span, format!("`{}` is a field, not a method", m.name), format!("read it without parentheses: `.{}`", m.name));
            } else {
                let c: Vec<String> = self.fns.iter().filter(|x| x.method_of.as_deref() == Some(s)).map(|x| x.name.rsplit('.').next().unwrap_or("").to_string()).collect();
                let d = self.err("E001", m.span, format!("struct `{}` has no method `{}`", s, m.name));
                if let Some(sg) = suggest(&m.name, c) {
                    d.help = Some(msg::did_you_mean(&sg));
                }
            }
            self.check_args_loose(args);
            return Self::bad(span);
        }
        if m.name == "len" && matches!(rty, Ty::Str | Ty::Path | Ty::List(_) | Ty::Map(_, _)) {
            self.sym(m.span, Sym::Method { recv: rty.clone(), name: "len".into() });
            if !args.is_empty() {
                self.errh("E012", span, "`.len()` takes no arguments", "write `.len()`");
            }
            return TExpr { kind: TK::Len(Box::new(recv)), ty: Ty::Int, span };
        }
        let Some(cat) = method_cat(&rty) else {
            self.errh("E010", m.span, format!("`{}` has no methods", tdisp(&rty)), if rty == Ty::Secret { msg::SECRET_HELP } else { "check the type of the value" });
            self.check_args_loose(args);
            return Self::bad(span);
        };
        let Some(md) = stdlib::find_method(cat, &m.name) else {
            if let Some(h) = Self::removed_help(&rty, &m.name) {
                self.errh("E001", m.span, format!("`{}` has no `.{}()`", tdisp(&rty), m.name), h);
                self.check_args_loose(args);
                return Self::bad(span);
            }
            if rty == Ty::Path && stdlib::PATH_PROPS.iter().any(|(n, _, _)| *n == m.name) {
                self.errh("E010", m.span, format!("`.{}` is a property", m.name), format!("read it without parentheses: `.{}`", m.name));
            } else {
                let c: Vec<String> = stdlib::METHODS.iter().filter(|x| x.recv == cat).map(|x| x.name.to_string()).chain(std::iter::once("len".to_string())).collect();
                let d = self.err("E001", m.span, format!("`{}` has no method `{}`", tdisp(&rty), m.name));
                if let Some(sg) = suggest(&m.name, c) {
                    d.help = Some(msg::did_you_mean(&sg));
                }
            }
            self.check_args_loose(args);
            return Self::bad(span);
        };
        if cat == "enum" && md.name == "parse" {
            self.errh("E010", m.span, "`parse` is called on the type, not on a value", "write `Type.parse(text)`");
            return Self::bad(span);
        }
        self.sym(m.span, Sym::Method { recv: rty.clone(), name: md.name.to_string() });
        let mut b = Bind::default();
        match &rty {
            Ty::List(t) => b.t = Some((**t).clone()),
            Ty::Map(k, v) => {
                b.k = Some((**k).clone());
                b.v = Some((**v).clone());
            }
            Ty::Enum(_) => b.en = Some(rty.clone()),
            _ => {}
        }
        // per-item restrictions
        if let Ty::List(t) = &rty {
            match md.name {
                "sum" if !matches!(**t, Ty::Int | Ty::Duration) => {
                    self.errh("E010", m.span, format!("`.sum()` adds up `[]int` or `[]duration`, not `{}`", tdisp(&rty)), "convert first with `.map(…)`");
                }
                "join" | "sort" if !t.is_scalar() || **t == Ty::Secret => {
                    self.errh("E010", m.span, format!("`.{}()` only works on lists of scalars, not `{}`", md.name, tdisp(&rty)), "use `.map(x => x.field)` first");
                }
                _ => {}
            }
        }
        if md.mutates {
            self.check_receiver_mutable(base);
        }
        let sig = std_sig(md.params, &b, span);
        let (targs, u) = self.match_args(&format!("`.{}()`", md.name), &sig, args, span);
        b.u = u;
        if md.name == "map" {
            if let Some(ut) = &b.u {
                if !ut.is_scalar() && !is_unk(ut) && !inner_list_ok(ut) {
                    self.errh("E010", span, format!("`.map()` returns scalars or lists of scalars, and the lambda returns `{}`", tdisp(ut)), "return a field or a text value");
                }
            }
        }
        if md.name == "sort_by" {
            if let Some(ut) = &b.u {
                if !matches!(ut, Ty::Int | Ty::Str | Ty::Path) && !is_unk(ut) {
                    self.errh("E010", span, format!("`.sort_by()` sorts by `int` or `str`, not `{}`", tdisp(ut)), "return a number or text from the lambda");
                }
            }
        }
        let ret = table_ty(md.ret, &b);
        let mut all = vec![TArg::Expr(recv)];
        all.extend(targs);
        let call = TCall { target: TTarget::Method { recv_ty: rty, name: md.name.to_string() }, args: all, ret: ret.clone(), fallible: md.fallible, desc: format!("`.{}()`", md.name) };
        TExpr { kind: TK::Call(Box::new(call)), ty: ret, span }
    }

    fn check_receiver_mutable(&mut self, base: &ast::Expr) {
        let root = match &base.kind {
            EK::Ident(n) => Some((n.clone(), base.span)),
            EK::Field(b, _) => match &b.kind {
                EK::Ident(n) => Some((n.clone(), b.span)),
                EK::SelfVal => {
                    self.errh("E002", b.span, "`self` is read-only", "return a new value: `Type { ..self, field: value }`");
                    return;
                }
                _ => None,
            },
            EK::SelfVal => {
                self.errh("E002", base.span, "`self` is read-only", "return a new value");
                return;
            }
            _ => None,
        };
        match root {
            Some((n, sp)) => {
                self.check_mutable(&n, sp, "mutating method");
            }
            None if matches!(base.kind, EK::Index(_, _) | EK::Field(_, _)) => self.errh(
                "E002",
                base.span,
                "this method changes a value inside another one, which only works on its own line",
                "use it as a statement or as the whole value of `let`, `=` or `return`: `let x = rows[0].pop()`",
            ),
            None => self.errh("E002", base.span, "this method changes the value, and this is not a variable", "store it in a `var` variable first"),
        }
    }

    /// Matches arguments to the signature (positional, then named; defaults filled in).
    /// Returns the complete positional arguments and the type `U` inferred from a lambda.
    fn match_args(&mut self, what: &str, sig: &Sig, args: &[ast::Arg], span: Span) -> (Vec<TArg>, Option<Ty>) {
        let nfixed = if sig.variadic { sig.params.len() - 1 } else { sig.params.len() };
        let mut slots: Vec<Option<TArg>> = vec![None; nfixed];
        let mut extra: Vec<TArg> = Vec::new();
        let mut u: Option<Ty> = None;
        let mut seen_named = false;
        let mut pos = 0;
        let mut too_many = false;
        for a in args {
            match &a.name {
                None => {
                    if seen_named {
                        self.errh("E012", a.value.span, "positional arguments come before named ones", "move this argument before the `name: value` ones");
                    }
                    if pos < nfixed {
                        let arg = self.check_arg(what, &sig.params[pos], &a.value, &mut u);
                        slots[pos] = Some(arg);
                        pos += 1;
                    } else if sig.variadic {
                        let te = self.ex(&a.value, None, true);
                        self.expect_text(&te, false);
                        extra.push(TArg::Expr(te));
                    } else {
                        if !too_many {
                            too_many = true;
                            let assin = sig_text(sig);
                            self.errh("E012", a.value.span, format!("extra argument: {} takes {} argument(s)", what, nfixed), format!("the signature is ({})", assin));
                        }
                        self.check_args_loose(std::slice::from_ref(a));
                    }
                }
                Some(n) => {
                    seen_named = true;
                    match sig.params[..nfixed].iter().position(|p| p.name == n.name) {
                        Some(i) => {
                            if slots[i].is_some() {
                                self.errh("E012", n.span, format!("argument `{}` was passed twice", n.name), "remove the duplicate");
                                continue;
                            }
                            let arg = self.check_arg(what, &sig.params[i], &a.value, &mut u);
                            slots[i] = Some(arg);
                        }
                        None => {
                            let c: Vec<String> = sig.params[..nfixed].iter().map(|p| p.name.clone()).collect();
                            let d = self.err("E012", n.span, format!("{} has no parameter `{}`", what, n.name));
                            d.help = Some(match suggest(&n.name, c.clone()) {
                                Some(s) => msg::did_you_mean(&s),
                                None => format!("the signature is ({})", sig_text(sig)),
                            });
                            self.check_args_loose(std::slice::from_ref(a));
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (i, s) in slots.into_iter().enumerate() {
            match s {
                Some(a) => out.push(a),
                None => match &sig.params[i].default {
                    Some(d) => out.push(TArg::Expr(TExpr { span, ..d.clone() })),
                    None => {
                        self.errh("E012", span, format!("missing argument `{}` for {}", sig.params[i].name, what), format!("the signature is ({})", sig_text(sig)));
                        out.push(TArg::Expr(Self::bad(span)));
                    }
                },
            }
        }
        out.extend(extra);
        (out, u)
    }

    fn check_arg(&mut self, what: &str, p: &SP, v: &ast::Expr, u: &mut Option<Ty>) -> TArg {
        match &p.ty {
            PT::Ty(t) => {
                let te = self.ex(v, Some(t), true);
                self.expect_ty(&te, t, &format!("argument `{}` of {}", p.name, what));
                TArg::Expr(te)
            }
            PT::Any => {
                let te = self.ex(v, None, true);
                self.expect_text(&te, false);
                TArg::Expr(te)
            }
            PT::Lambda { arg, ret } => match &v.kind {
                EK::Lambda { params, body } => {
                    let (lam, lt) = self.lambda(params, body, arg, ret.as_ref(), v.span);
                    if ret.is_none() {
                        *u = Some(lt);
                    }
                    TArg::Lambda(lam)
                }
                _ => {
                    self.errh("E010", v.span, format!("{} expects a lambda", what), "write `x => expression`, for example `x => x.len() > 3`");
                    if ret.is_none() {
                        *u = Some(unk());
                    }
                    self.ex(v, None, true);
                    TArg::Expr(Self::bad(v.span))
                }
            },
        }
    }

    fn lambda(&mut self, params: &[ast::Ident], body: &ast::LambdaBody, arg: &Ty, ret: Option<&Ty>, span: Span) -> (TLambda, Ty) {
        if params.len() != 1 {
            self.errh("E012", span, format!("this lambda takes 1 parameter, not {}", params.len()), "write `x => …`");
        }
        let saved = std::mem::replace(&mut self.ctx, Ctx::new(CtxKind::Lambda));
        self.ctx.ret = ret.cloned();
        self.ctx.err_stack = saved.err_stack.clone();
        self.ctx.in_test = saved.in_test;
        self.ctx.self_var = saved.self_var;
        self.ctx.lambda_base = Some(self.scopes.len());
        self.push_scope();
        self.enter_block(span.end);
        let mut pids = Vec::new();
        for p in params {
            let id = self.declare(p, arg.clone(), false, VarKind::LambdaParam);
            pids.push(id);
        }
        let (tbody, ty) = match body {
            ast::LambdaBody::Expr(e) => {
                let te = self.ex(e, ret, true);
                if let Some(r) = ret {
                    self.expect_ty(&te, r, "the lambda");
                }
                let t = te.ty.clone();
                (TLambdaBody::Expr(Box::new(te)), t)
            }
            ast::LambdaBody::Block(b) => {
                let stmts = self.block_stmts(b);
                if !terminates(&stmts) {
                    self.errh("E032", b.span, "the lambda reaches the end without `return`", "end every path with `return value`");
                }
                let t = ret.cloned().or_else(|| self.ctx.lambda_ret.clone()).unwrap_or_else(unk);
                (TLambdaBody::Block(stmts), t)
            }
        };
        if ty == Ty::Unit {
            self.errh("E010", span, "the lambda must return a value", "a lambda's body is an expression");
        }
        self.leave_block();
        self.pop_scope();
        self.ctx = saved;
        (TLambda { params: pids, ret: ty.clone(), body: tbody }, ty)
    }
}

fn sig_text(sig: &Sig) -> String {
    sig.params
        .iter()
        .map(|p| {
            let t = match &p.ty {
                PT::Ty(t) => tdisp(t),
                PT::Any => "…".into(),
                PT::Lambda { .. } => "lambda".into(),
            };
            if p.default.is_some() {
                format!("{}: {} = …", p.name, t)
            } else if matches!(p.ty, PT::Any) {
                "…".to_string()
            } else {
                format!("{}: {}", p.name, t)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `a / b` or `a % b` with a divisor that may be zero (anything that is not a nonzero literal).
/// A statement like `rows[i].push(x)` or `groups[k].remove(y)`: a mutating method on something that is
/// not a variable or a field of one. Returns the receiver, the method, the arguments and the callee's span.
fn nested_mutation(e: &ast::Expr) -> Option<(&ast::Expr, &ast::Ident, &[ast::Arg], Span)> {
    let EK::Call { callee, args } = &e.kind else { return None };
    let EK::Field(base, m) = &callee.kind else { return None };
    if !matches!(m.name.as_str(), "push" | "pop" | "remove") {
        return None;
    }
    let nested = match &base.kind {
        EK::Index(_, _) => true,
        EK::Field(b, _) => !matches!(b.kind, EK::Ident(_) | EK::SelfVal),
        _ => false,
    };
    nested.then_some((&**base, m, &args[..], callee.span))
}

/// The struct a field holds, directly or as a list of them (D35).
fn struct_in(t: &Ty) -> Option<String> {
    match t {
        Ty::Struct(n) if !n.is_empty() => Some(n.clone()),
        Ty::List(e) => match &**e {
            Ty::Struct(n) if !n.is_empty() => Some(n.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A collection that can sit inside a list or be a map's value (D34): a list of scalars (no secrets).
pub fn inner_list_ok(t: &Ty) -> bool {
    matches!(t, Ty::List(e) if e.is_scalar() && **e != Ty::Secret)
}

pub fn may_divide_by_zero(op: BinOp, divisor: &TExpr) -> bool {
    matches!(op, BinOp::Div | BinOp::Mod) && !matches!(divisor.kind, TK::Int(n) | TK::Duration(n) if n != 0)
}

/// The method category of a type (`str` also covers `path`).
pub fn method_cat(t: &Ty) -> Option<&'static str> {
    Some(match t {
        Ty::Str | Ty::Path => "str",
        Ty::List(_) => "list",
        Ty::Map(_, _) => "map",
        Ty::Enum(_) => "enum",
        Ty::Int => "int",
        Ty::Bool => "bool",
        Ty::Duration => "duration",
        _ => return None,
    })
}

fn root_is_self(e: &ast::Expr) -> bool {
    match &e.kind {
        EK::SelfVal => true,
        EK::Field(b, _) | EK::Index(b, _) => root_is_self(b),
        _ => false,
    }
}

/// Fixed value (for `param` patterns).
fn is_const(e: &TExpr) -> bool {
    match &e.kind {
        TK::Int(_) | TK::Str(_) | TK::Bool(_) | TK::Duration(_) | TK::EnumCase(_, _) | TK::Bad => true,
        TK::List(items) => items.iter().all(is_const),
        TK::Unary(UnOp::Neg, x) => is_const(x),
        _ => false,
    }
}

/// Do all paths end in `return`/`fail`/`exit`?
pub fn terminates(stmts: &[TStmt]) -> bool {
    match stmts.last().map(|s| &s.kind) {
        Some(TS::Return(_)) | Some(TS::Fail { .. }) | Some(TS::Exit(_)) => true,
        Some(TS::Expr(e)) => expr_terminates(e),
        _ => false,
    }
}

/// `try { … return … } catch { … return … }` also ends: both sides end (or the handler fails).
fn handler_terminates(h: &THandler) -> bool {
    match h {
        THandler::Fail { .. } => true,
        THandler::Block { body, .. } => terminates(&body.stmts),
        THandler::Ignore | THandler::Default(_) => false,
    }
}

fn expr_terminates(e: &TExpr) -> bool {
    match &e.kind {
        TK::If { then, els: Some(els), .. } => terminates(&then.stmts) && terminates(&els.stmts),
        TK::Match(m) => {
            m.exhaustive && m.arms.iter().all(|a| terminates(&a.body.stmts)) && m.default.as_ref().map(|d| terminates(&d.stmts)).unwrap_or(true)
        }
        TK::Or(lhs, h) => matches!(&lhs.kind, TK::Try(b) if terminates(&b.stmts)) && handler_terminates(h),
        _ => false,
    }
}

// =================================================================================================
// Fallibility
// =================================================================================================

/// Can the call fail (with the current table of fallible functions)?
pub fn call_fallible(c: &TCall, fnf: &[bool]) -> bool {
    if c.fallible {
        return true;
    }
    if let TTarget::User(i) = c.target {
        if fnf.get(i).copied().unwrap_or(false) {
            return true;
        }
    }
    c.args.iter().any(|a| match a {
        TArg::Lambda(l) => lambda_fallible(l, fnf),
        _ => false,
    })
}

pub fn lambda_fallible(l: &TLambda, fnf: &[bool]) -> bool {
    let mut r = Vec::new();
    match &l.body {
        TLambdaBody::Expr(e) => reasons_expr(e, fnf, &mut r),
        TLambdaBody::Block(s) => reasons_stmts(s, fnf, &mut r),
    }
    !r.is_empty()
}

fn cmd_desc(c: &TCmd, capture: bool) -> String {
    let prog = match c.stages.first().and_then(|s| s.first()) {
        Some(TWord::Lit(p)) => p.clone(),
        _ => "…".into(),
    };
    if capture {
        format!("`$({} …)`", prog)
    } else {
        format!("`$ {}`", prog)
    }
}

pub fn reasons_cmd(c: &TCmd, fnf: &[bool], out: &mut Vec<(String, Span)>) {
    for st in &c.stages {
        for w in st {
            match w {
                TWord::Lit(_) => {}
                TWord::Interp(parts) => {
                    for p in parts {
                        if let TPart::Expr(e) = p {
                            reasons_expr(e, fnf, out);
                        }
                    }
                }
                TWord::Spread(e) => reasons_expr(e, fnf, out),
            }
        }
    }
    for r in &c.redirects {
        match r {
            TRedir::Out { path, .. } | TRedir::Err { path, .. } | TRedir::In(path) => reasons_expr(path, fnf, out),
            TRedir::ErrToOut => {}
        }
    }
    if let Some(s) = &c.stdin {
        reasons_expr(s, fnf, out);
    }
}

fn reasons_block(b: &TBlock, fnf: &[bool], out: &mut Vec<(String, Span)>) {
    reasons_stmts(&b.stmts, fnf, out);
    if let Some(v) = &b.value {
        reasons_expr(v, fnf, out);
    }
}

/// What, inside `e`, propagates failure to the context (not handled by `catch`/`try`).
pub fn reasons_expr(e: &TExpr, fnf: &[bool], out: &mut Vec<(String, Span)>) {
    match &e.kind {
        TK::Int(_) | TK::Str(_) | TK::Bool(_) | TK::Duration(_) | TK::EnumCase(_, _) | TK::Var(_) | TK::DryRun | TK::Args | TK::Os | TK::CurrentErr | TK::Bad => {}
        TK::Interp(parts) => {
            for p in parts {
                if let TPart::Expr(x) = p {
                    reasons_expr(x, fnf, out);
                }
            }
        }
        TK::Field(b, _) | TK::Len(b) | TK::Unary(_, b) => reasons_expr(b, fnf, out),
        TK::Index(a, b) => {
            // `xs[i]` out of range fails (`m[k]` already became a fallible call in the checker).
            reasons_expr(a, fnf, out);
            reasons_expr(b, fnf, out);
            out.push(("`list[i]`".into(), e.span));
        }
        TK::Binary(op, a, b) => {
            reasons_expr(a, fnf, out);
            reasons_expr(b, fnf, out);
            // dividing by zero fails on all three targets (the lowering adds the check)
            if may_divide_by_zero(*op, b) {
                out.push((if *op == BinOp::Div { "`/`" } else { "`%`" }.into(), e.span));
            }
        }
        TK::Call(c) => {
            for a in &c.args {
                if let TArg::Expr(x) = a {
                    reasons_expr(x, fnf, out);
                }
            }
            if call_fallible(c, fnf) {
                out.push((c.desc.clone(), e.span));
            }
        }
        TK::List(items) => items.iter().for_each(|x| reasons_expr(x, fnf, out)),
        TK::Map(entries) => entries.iter().for_each(|(k, v)| {
            reasons_expr(k, fnf, out);
            reasons_expr(v, fnf, out);
        }),
        TK::Struct { spread, fields, .. } => {
            if let Some(s) = spread {
                reasons_expr(s, fnf, out);
            }
            fields.iter().for_each(|(_, v)| reasons_expr(v, fnf, out));
        }
        TK::If { cond, then, els } => {
            reasons_expr(cond, fnf, out);
            reasons_block(then, fnf, out);
            if let Some(b) = els {
                reasons_block(b, fnf, out);
            }
        }
        TK::Match(m) => {
            reasons_expr(&m.subject, fnf, out);
            for a in &m.arms {
                if let Some(g) = &a.guard {
                    reasons_expr(g, fnf, out);
                }
                reasons_block(&a.body, fnf, out);
            }
            if let Some(d) = &m.default {
                reasons_block(d, fnf, out);
            }
        }
        TK::Or(_, h) => match &**h {
            THandler::Ignore => {}
            THandler::Default(v) => reasons_expr(v, fnf, out),
            THandler::Fail { message, code } => {
                reasons_expr(message, fnf, out);
                if let Some(c) = code {
                    reasons_expr(c, fnf, out);
                }
                out.push(("`catch fail`".into(), e.span));
            }
            THandler::Block { body, .. } => reasons_block(body, fnf, out),
        },
        TK::Command(c) => {
            reasons_cmd(c, fnf, out);
            out.push((cmd_desc(c, false), e.span));
        }
        TK::Capture(c) => {
            reasons_cmd(c, fnf, out);
            out.push((cmd_desc(c, true), e.span));
        }
        TK::CmdTest(c) => reasons_cmd(c, fnf, out),
        TK::Try(_) => {}
        TK::Retry { times, every, body } => {
            reasons_expr(times, fnf, out);
            reasons_expr(every, fnf, out);
            let mut inner = Vec::new();
            reasons_block(body, fnf, &mut inner);
            if !inner.is_empty() {
                out.push(("`retry`".into(), e.span));
            }
        }
        TK::Parallel { max, body } => {
            if let Some(m) = max {
                reasons_expr(m, fnf, out);
            }
            let mut inner = Vec::new();
            match &**body {
                TPar::For { list, body, .. } => {
                    reasons_expr(list, fnf, out);
                    reasons_block(body, fnf, &mut inner);
                }
                TPar::Tasks(stmts) => reasons_stmts(stmts, fnf, &mut inner),
            }
            if !inner.is_empty() {
                out.push(("`parallel`".into(), e.span));
            }
        }
    }
}

pub fn reasons_stmts(stmts: &[TStmt], fnf: &[bool], out: &mut Vec<(String, Span)>) {
    for s in stmts {
        match &s.kind {
            TS::Let { value, .. } => reasons_expr(value, fnf, out),
            TS::Assign { target, value } => {
                match target {
                    TLValue::Index(_, e) => {
                        // `xs[i] = v` with `i` out of range fails (to grow, use `push`).
                        reasons_expr(e, fnf, out);
                        out.push(("`list[i] = …`".into(), s.span));
                    }
                    TLValue::MapEntry(_, e) => reasons_expr(e, fnf, out),
                    _ => {}
                }
                reasons_expr(value, fnf, out);
            }
            TS::Expr(e) => reasons_expr(e, fnf, out),
            TS::ForRange { from, to, body, .. } => {
                reasons_expr(from, fnf, out);
                reasons_expr(to, fnf, out);
                reasons_stmts(body, fnf, out);
            }
            TS::ForList { list, body, .. } => {
                reasons_expr(list, fnf, out);
                reasons_stmts(body, fnf, out);
            }
            TS::ForMap { map, body, .. } => {
                reasons_expr(map, fnf, out);
                reasons_stmts(body, fnf, out);
            }
            TS::While { cond, body } => {
                reasons_expr(cond, fnf, out);
                reasons_stmts(body, fnf, out);
            }
            TS::Fail { message, code } => {
                reasons_expr(message, fnf, out);
                if let Some(c) = code {
                    reasons_expr(c, fnf, out);
                }
                out.push(("`fail`".into(), s.span));
            }
            TS::Return(Some(e)) | TS::Exit(Some(e)) => reasons_expr(e, fnf, out),
            TS::Return(None) | TS::Exit(None) | TS::Break | TS::Continue | TS::Native { .. } => {}
            TS::Defer(e) | TS::AssertFails(e) => match &e.kind {
                TK::Call(c) => {
                    for a in &c.args {
                        if let TArg::Expr(x) = a {
                            reasons_expr(x, fnf, out);
                        }
                    }
                }
                TK::Command(c) => reasons_cmd(c, fnf, out),
                _ => {}
            },
            TS::Assert { cond, .. } => reasons_expr(cond, fnf, out),
        }
    }
}

/// Fixed point: starts with all functions infallible and propagates until nothing changes.
fn infer_fallible(p: &mut TProgram) {
    let n = p.fns.len();
    let mut fnf = vec![false; n];
    loop {
        let mut changed = false;
        for i in 0..n {
            if fnf[i] {
                continue;
            }
            let mut r = Vec::new();
            reasons_stmts(&p.funcs[i].body, &fnf, &mut r);
            if !r.is_empty() {
                fnf[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    p.reasons = (0..n)
        .map(|i| {
            let mut r = Vec::new();
            reasons_stmts(&p.funcs[i].body, &fnf, &mut r);
            r
        })
        .collect();
    p.fallible = fnf;
}

// ---------------------------------------------------------------------------------------------
// Generic visit (for checks after inference)
// ---------------------------------------------------------------------------------------------

pub fn walk_stmts(stmts: &[TStmt], f: &mut dyn FnMut(&TExpr), fs: &mut dyn FnMut(&TStmt)) {
    for s in stmts {
        fs(s);
        match &s.kind {
            TS::Let { value, .. } => walk_expr(value, f, fs),
            TS::Assign { target, value } => {
                if let TLValue::Index(_, e) | TLValue::MapEntry(_, e) = target {
                    walk_expr(e, f, fs);
                }
                walk_expr(value, f, fs);
            }
            TS::Expr(e) | TS::Defer(e) | TS::AssertFails(e) => walk_expr(e, f, fs),
            TS::ForRange { from, to, body, .. } => {
                walk_expr(from, f, fs);
                walk_expr(to, f, fs);
                walk_stmts(body, f, fs);
            }
            TS::ForList { list: e, body, .. } | TS::ForMap { map: e, body, .. } | TS::While { cond: e, body } => {
                walk_expr(e, f, fs);
                walk_stmts(body, f, fs);
            }
            TS::Fail { message, code } => {
                walk_expr(message, f, fs);
                if let Some(c) = code {
                    walk_expr(c, f, fs);
                }
            }
            TS::Return(Some(e)) | TS::Exit(Some(e)) => walk_expr(e, f, fs),
            TS::Assert { cond, .. } => walk_expr(cond, f, fs),
            _ => {}
        }
    }
}

fn walk_block(b: &TBlock, f: &mut dyn FnMut(&TExpr), fs: &mut dyn FnMut(&TStmt)) {
    walk_stmts(&b.stmts, f, fs);
    if let Some(v) = &b.value {
        walk_expr(v, f, fs);
    }
}

fn walk_cmd(c: &TCmd, f: &mut dyn FnMut(&TExpr), fs: &mut dyn FnMut(&TStmt)) {
    for st in &c.stages {
        for w in st {
            match w {
                TWord::Interp(parts) => {
                    for p in parts {
                        if let TPart::Expr(e) = p {
                            walk_expr(e, f, fs);
                        }
                    }
                }
                TWord::Spread(e) => walk_expr(e, f, fs),
                TWord::Lit(_) => {}
            }
        }
    }
    for r in &c.redirects {
        if let TRedir::Out { path, .. } | TRedir::Err { path, .. } | TRedir::In(path) = r {
            walk_expr(path, f, fs);
        }
    }
    if let Some(s) = &c.stdin {
        walk_expr(s, f, fs);
    }
}

pub fn walk_expr(e: &TExpr, f: &mut dyn FnMut(&TExpr), fs: &mut dyn FnMut(&TStmt)) {
    f(e);
    match &e.kind {
        TK::Interp(parts) => {
            for p in parts {
                if let TPart::Expr(x) = p {
                    walk_expr(x, f, fs);
                }
            }
        }
        TK::Field(b, _) | TK::Len(b) | TK::Unary(_, b) => walk_expr(b, f, fs),
        TK::Index(a, b) | TK::Binary(_, a, b) => {
            walk_expr(a, f, fs);
            walk_expr(b, f, fs);
        }
        TK::Call(c) => {
            for a in &c.args {
                match a {
                    TArg::Expr(x) => walk_expr(x, f, fs),
                    TArg::Lambda(l) => match &l.body {
                        TLambdaBody::Expr(x) => walk_expr(x, f, fs),
                        TLambdaBody::Block(s) => walk_stmts(s, f, fs),
                    },
                }
            }
        }
        TK::List(items) => items.iter().for_each(|x| walk_expr(x, f, fs)),
        TK::Map(entries) => entries.iter().for_each(|(k, v)| {
            walk_expr(k, f, fs);
            walk_expr(v, f, fs);
        }),
        TK::Struct { spread, fields, .. } => {
            if let Some(s) = spread {
                walk_expr(s, f, fs);
            }
            fields.iter().for_each(|(_, v)| walk_expr(v, f, fs));
        }
        TK::If { cond, then, els } => {
            walk_expr(cond, f, fs);
            walk_block(then, f, fs);
            if let Some(b) = els {
                walk_block(b, f, fs);
            }
        }
        TK::Match(m) => {
            walk_expr(&m.subject, f, fs);
            for a in &m.arms {
                if let Some(g) = &a.guard {
                    walk_expr(g, f, fs);
                }
                walk_block(&a.body, f, fs);
            }
            if let Some(d) = &m.default {
                walk_block(d, f, fs);
            }
        }
        TK::Or(l, h) => {
            walk_expr(l, f, fs);
            match &**h {
                THandler::Default(v) => walk_expr(v, f, fs),
                THandler::Fail { message, code } => {
                    walk_expr(message, f, fs);
                    if let Some(c) = code {
                        walk_expr(c, f, fs);
                    }
                }
                THandler::Block { body, .. } => walk_block(body, f, fs),
                THandler::Ignore => {}
            }
        }
        TK::Command(c) | TK::Capture(c) | TK::CmdTest(c) => walk_cmd(c, f, fs),
        TK::Try(b) => walk_block(b, f, fs),
        TK::Retry { times, every, body } => {
            walk_expr(times, f, fs);
            walk_expr(every, f, fs);
            walk_block(body, f, fs);
        }
        TK::Parallel { max, body } => {
            if let Some(m) = max {
                walk_expr(m, f, fs);
            }
            match &**body {
                TPar::For { list, body, .. } => {
                    walk_expr(list, f, fs);
                    walk_block(body, f, fs);
                }
                TPar::Tasks(s) => walk_stmts(s, f, fs),
            }
        }
        _ => {}
    }
}

/// Checks that depend on the final fallibility: `E040` (`catch` with nothing fallible) and `E060`.
fn post_checks(p: &TProgram, diags: &mut Vec<Diag>) {
    let fnf = &p.fallible;
    let check = |stmts: &[TStmt], diags: &mut Vec<Diag>| {
        let mut found: Vec<Diag> = Vec::new();
        let mut found2: Vec<Diag> = Vec::new();
        let mut fe = |e: &TExpr| {
            if let TK::Or(lhs, h) = &e.kind {
                if is_unk(&lhs.ty) && matches!(lhs.kind, TK::Bad) {
                    return;
                }
                let mut r = Vec::new();
                match &lhs.kind {
                    TK::Try(b) => reasons_block(b, fnf, &mut r),
                    _ => reasons_expr(lhs, fnf, &mut r),
                }
                if r.is_empty() {
                    let msg = if matches!(lhs.kind, TK::Try(_)) { "`catch` after a `try` in which nothing can fail" } else { "`catch` after something that cannot fail" };
                    let help = match &**h {
                        THandler::Default(v) if lhs.ty == Ty::Bool && v.ty == Ty::Bool => "for a logical \"or\" between conditions, use `||` (`a || b`); `catch` only handles failures",
                        _ => "remove the `catch …`",
                    };
                    found.push(Diag::error("E040", lhs.span, msg).with_help(help));
                }
            }
        };
        let mut fs = |s: &TStmt| {
            if let TS::AssertFails(e) = &s.kind {
                let ok = match &e.kind {
                    TK::Call(c) => call_fallible(c, fnf),
                    TK::Command(_) => true,
                    _ => true,
                };
                if !ok {
                    found2.push(Diag::error("E060", e.span, "`assert_fails` on something that never fails").with_help("the test would only pass if the call failed, and it cannot fail"));
                }
            }
        };
        walk_stmts(stmts, &mut fe, &mut fs);
        diags.append(&mut found);
        diags.append(&mut found2);
    };
    check(&p.body, diags);
    for f in &p.funcs {
        check(&f.body, diags);
    }
    for t in &p.tests {
        check(&t.body, diags);
    }
}

// =================================================================================================
// Tests
// =================================================================================================

/// `NAME=value` at the start of a command word: the name and the value as a word of its own.
/// The name is a shell identifier (`[A-Za-z_][A-Za-z0-9_]*`).
fn env_assignment(w: &ast::CmdWord) -> Option<(String, ast::CmdWord)> {
    let ast::WordPart::Lit(first) = w.parts.first()? else { return None };
    let eq = first.find('=')?;
    let name = &first[..eq];
    let mut cs = name.chars();
    let c0 = cs.next()?;
    if !(c0.is_ascii_alphabetic() || c0 == '_') || !cs.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut parts = Vec::new();
    if eq + 1 < first.len() {
        parts.push(ast::WordPart::Lit(first[eq + 1..].to_string()));
    }
    parts.extend(w.parts[1..].iter().cloned());
    Some((name.to_string(), ast::CmdWord { parts, span: w.span }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (TProgram, Vec<Diag>) {
        let (f, mut d) = crate::parser::parse(src);
        let (p, mut d2) = check(&f, src, Path::new("."));
        d.append(&mut d2);
        (p, d)
    }

    fn codes(src: &str) -> Vec<&'static str> {
        run(src).1.into_iter().map(|d| d.code).collect()
    }

    fn has(src: &str, code: &str) {
        let (_, d) = run(src);
        assert!(d.iter().any(|x| x.code == code), "expected {} in:\n{}\ngot: {:#?}", code, src, d);
    }

    fn help_of(src: &str, code: &str) -> String {
        let (_, d) = run(src);
        d.iter().find(|x| x.code == code).and_then(|x| x.help.clone()).unwrap_or_else(|| panic!("expected {} with help in:\n{}\ngot: {:#?}", code, src, d))
    }

    #[test]
    fn or_between_conditions_suggests_bars() {
        assert!(help_of("let a = 1\nlet ok = a == 1 catch a == 2\nprint(ok)", "E040").contains("`||`"));
        // outside conditions, the hint stays the usual one
        assert!(help_of("let x = 1 catch 2\nprint(x)", "E040").contains("remove"));
    }

    #[test]
    fn map_at_end_of_line() {
        assert!(codes("struct S {\n    m: map[str]int\n    n: int\n}\nlet s = S { m: {}, n: 1 }\nprint(s.n)").is_empty());
        assert!(codes("param m: map[str]int\nprint(m.len())").iter().all(|c| *c != "E005"));
    }

    #[test]
    fn list_index_is_fallible() {
        // `catch` after `xs[i]` is accepted (before: E040) and the function that indexes becomes fallible
        let (p, d) = run("fn pega(xs: []str) -> str {\n return xs[0]\n}\nlet xs = [\"a\"]\nprint(xs[1] catch \"-\", pega(xs) catch \"-\")");
        assert!(!d.iter().any(|x| x.severity == crate::diag::Severity::Error), "{:#?}", d);
        assert_eq!(p.fallible, vec![true]);
        let (p, _) = run("fn poe(i: int) {\n var xs = [1]\n xs[i] = 2\n print(xs.len())\n}\npoe(0)");
        assert_eq!(p.fallible, vec![true]);
    }

    #[test]
    fn match_guards_bindings_and_struct_patterns() {
        let ok = "struct S {\n    a: int\n    b: str\n}\nlet s = S { a: 1, b: \"x\" }\nlet n = 5\nlet r = match n {\n    0 => \"zero\"\n    k if k > 3 => \"big {k}\"\n    k => \"small {k}\"\n}\nlet t = match s {\n    S { a: 1, b: x } => x\n    _ => \"\"\n}\nprint(r, t)";
        let (p, d) = run(ok);
        assert!(d.is_empty(), "{:#?}", d);
        assert!(p.vars.iter().any(|v| v.name == "k"));
        // a name binds the value, alone in its arm
        has("let n = 1\nmatch n {\n    1 | k => print(k)\n    _ => print(0)\n}", "E010");
        // after an arm that takes everything, nothing is reached
        has("let n = 1\nmatch n {\n    k => print(k)\n    2 => print(2)\n}", "E031");
        has("let n = 1\nmatch n {\n    _ => print(1)\n    2 => print(2)\n}", "E031");
        // an arm with `if` doesn't count for coverage
        has("enum E { a, b }\nlet e = E.a\nlet x = match e {\n    a => 1\n    b if true => 2\n}\nprint(x)", "E031");
        has("let n = 1\nlet x = match n {\n    k if k > 0 => 1\n}\nprint(x)", "E031");
        // the guard is a bool
        has("let n = 1\nmatch n {\n    k if k => print(k)\n    _ => print(0)\n}", "E010");
        // struct patterns: the right type and existing fields
        has("struct S {\n    a: int\n}\nlet n = 1\nmatch n {\n    S { a: 1 } => print(1)\n    _ => print(0)\n}", "E010");
        has("struct S {\n    a: int\n}\nlet s = S { a: 1 }\nmatch s {\n    S { z: 1 } => print(1)\n    _ => print(0)\n}", "E001");
        has("struct S {\n    a: int\n}\nlet s = S { a: 1 }\nmatch s {\n    S { a: \"x\" } => print(1)\n    _ => print(0)\n}", "E010");
        // on an enum, a name is a case (a typo isn't silently a binding)
        has("enum E { a, b }\nlet e = E.a\nmatch e {\n    a => print(1)\n    c => print(2)\n}", "E001");
        // the binding lives only in its arm
        has("let n = 1\nmatch n {\n    k => print(k)\n}\nprint(k)", "E001");
    }

    #[test]
    fn regex_patterns_are_checked() {
        has("print(regex.test(\"a\", '(a'))", "E054");
        has("let p = 'a'\nprint(regex.test(\"a\", p))", "E054");
        has("print(regex.find(\"a=b\", '^(.*)=(.*)$'))", "E054");
        has("print(regex.find_all(\"a\", '\\d*'))", "E054");
        has("print(regex.replace(\"a\", 'x?', \"y\"))", "E054");
        has("print(regex.groups(\"a\", 'a'))", "E054");
        // `test` accepts ambiguous patterns: whether there is a match doesn't depend on the engine
        let (_, d) = run("print(regex.test(\"a=b\", '^(.*)=(.*)$'), regex.find(\"a=b\", '^([^=]*)='))");
        assert!(d.is_empty(), "{:#?}", d);
    }

    #[test]
    fn planned_modules_are_compile_errors() {
        for src in ["print(pkg.manager())", "pkg.install([\"git\"])"] {
            has(src, "E090");
        }
        // implemented since D24 (hash, json, archive) and D32 (regex, encoding): no longer E090
        let (_, d) = run("print(hash.sha256(\"a\"), json.quote(\"b\"), regex.test(\"a\", 'a'), encoding.base64_encode(\"a\"))\narchive.tar_extract(\"a.tgz\", \"x\") catch ignore");
        assert!(!d.iter().any(|x| x.code == "E090"), "{:#?}", d);
    }

    #[test]
    fn examples_have_no_errors() {
        for f in ["hello", "backup", "deploy", "installer", "release"] {
            let path = format!("{}/examples/{}.nalu", env!("CARGO_MANIFEST_DIR"), f);
            let src = std::fs::read_to_string(&path).unwrap();
            let (_, d) = run(&src);
            let errs: Vec<_> = d.iter().filter(|x| x.is_error()).collect();
            assert!(errs.is_empty(), "{}: {:#?}", f, errs);
        }
    }

    #[test]
    fn release_test_alone_gives_name_errors() {
        let path = format!("{}/examples/release_test.nalu", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        let (_, d) = run(&src);
        assert!(d.iter().any(|x| x.code == "E001"));
        // with release.nalu alongside, it compiles
        let rel = std::fs::read_to_string(format!("{}/examples/release.nalu", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let both = format!("{}\n{}", rel, src);
        let (_, d) = run(&both);
        let errs: Vec<_> = d.iter().filter(|x| x.is_error()).collect();
        assert!(errs.is_empty(), "{:#?}", errs);
    }

    #[test]
    fn fallibility_fixed_point() {
        let src = "fn a() -> int {\n  return b()\n}\nfn b() -> int {\n  return c()\n}\nfn c() -> int {\n  return \"x\".to_int()\n}\nfn d() -> int {\n  return 1\n}\nfn e(n: int) -> int {\n  if n == 0 { return 0 }\n  return e(n - 1)\n}\nprint(a(), d(), e(3))\n";
        let (p, d) = run(src);
        assert!(d.iter().all(|x| !x.is_error()), "{:#?}", d);
        assert_eq!(p.fallible, vec![true, true, true, false, false]);
    }

    #[test]
    fn codes_are_reported() {
        has("print(nmoe)\nlet nome = 1", "E001");
        has("let x = 1\nx = 2", "E002");
        has("let xs = [1]\nxs.push(2)", "E002");
        has("let x = 1\nlet x = 2\nprint(x)", "E003");
        has("let os = 1", "E004");
        has("let x: int = \"a\"", "E010");
        has("print(1 == \"a\")", "E011");
        has("print([1] == [1])", "E011");
        has("fn f(a: int) {}\nf()", "E012");
        has("fn f(a: int) {}\nf(1, b: 2)", "E012");
        has("let d = 5s\nprint(d > 0)", "E020");
        has("let d = 5s\n$ sleep {d}", "E021");
        has("let xs = [1]\nprint(\"{xs}\")", "E022");
        has("let x = 1\nx + 1", "E030");
        has("enum E { a, b }\nlet e = E.a\nmatch e {\n a => print(1)\n}", "E031");
        has("fn f() -> int {\n print(1)\n}", "E032");
        has("let f = x => x", "E033");
        has("let x = 1 catch 2", "E040");
        has("try {\n $ ls\n}", "E041");
        has("let xs = [\"a\"]\n$ echo a{xs}", "E042");
        has("let a = \"x\"\nprint(a / \"b\")", "E043");
        has("let s = { a: 1 }", "E044");
        has("var n = 0\nparallel for x in [1] {\n n += x\n}", "E045");
        has("break", "E046");
        has("print(err)", "E046");
        has("print(self)", "E046");
        has("param t: secret\nprint(\"{t}\")", "E050");
        has("if os == \"widows\" {\n}", "E051");
        has("native {\n bash: 'x'\n}", "E052");
        has("let x = embed(\"nao-existe.txt\")\nprint(x)", "E053");
        has("assert 1 == 1", "E060");
        has("fn f() -> int { return 1 }\ntest \"t\" {\n assert_fails(f())\n}", "E060");
        has("let x = 1", "W001");
        has("param p: int = 1", "W002");
        has("param t: secret\n$ login {t}", "W011");
        has("$ rm -rf x catch ignore", "W012");
    }

    #[test]
    fn no_false_positives() {
        let ok = [
            "var xs: []str = []\nxs.push(\"a\")\nprint(xs.len())",
            "let m = { \"a\": 1 }\nlet v = m[\"a\"] catch 0\nprint(v)",
            "let d = 1m30s\nprint(\"{d}\", d / 1s, d * 2)",
            "let p = dirs.home() / \".ssh\" / \"id\"\nprint(p.name, fs.exists(p))",
            "fn f(n: int = 1, s: str = \"x\") -> str {\n return s.repeat(n)\n}\nprint(f(s: \"a\"), f(2))",
            "let xs = [3, 1, 2]\nlet ys = xs.filter(x => x > 1).map(x => \"{x}\").sort()\nprint(ys.join(\",\"))",
            "let n = \"42\".to_int() catch {\n log.warn(\"ruim: {err}\")\n 0\n}\nprint(n)",
            "$ git pull catch {\n log.warn(\"offline ({err.code})\")\n}",
            "let r = match os {\n \"linux\" => 1\n \"macos\" => 2\n \"windows\" => 3\n}\nprint(r)",
            "for k, v in { \"a\": 1 } {\n print(k, v)\n}",
            "while !$ nc -z localhost 5432 { time.sleep(1s) }",
            "let t = fmt(\"{} e {}\", 1, \"a\")\nprint(t)",
        ];
        for src in ok {
            let (_, d) = run(src);
            let errs: Vec<_> = d.iter().filter(|x| x.is_error()).collect();
            assert!(errs.is_empty(), "{}\n{:#?}", src, errs);
        }
        assert!(codes("print(1)").is_empty());
    }

    #[test]
    fn suggestion_by_distance() {
        let (_, d) = run("let nome = 1\nprint(nmoe)");
        assert_eq!(d[0].help.as_deref(), Some("did you mean `nome`?"));
        let (_, d) = run("print(fs.reed(\"x\"))");
        assert_eq!(d[0].help.as_deref(), Some("did you mean `fs.read`?"));
    }
}
