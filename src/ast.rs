//! Nalu AST (SPEC-001): the syntax tree the parser produces and the checker consumes.
//!
//! The AST is **untyped** and close to the text; the checker (`check.rs`) turns it into a typed,
//! resolved tree (`check::TProgram`), which the lowering (`lower.rs`) turns into the IR.
//! `if` and `match` are always expressions here; the checker decides whether they are in value context.

use crate::diag::Span;

#[derive(Clone, Debug)]
pub struct File {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Param(ParamDecl),
    Fn(FnDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Test(TestDecl),
    Stmt(Stmt),
    /// `use "lib.nalu"` (D37): the path as written. The CLI loads the file; the frontend only checks the place.
    Use(String, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ParamDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub doc: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<FnParam>,
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub doc: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnParam {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub name: Ident,
    pub cases: Vec<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: String,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    Named(Ident),
    List(Box<TypeExpr>, Span),
    Map(Box<TypeExpr>, Box<TypeExpr>, Span),
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(i) => i.span,
            TypeExpr::List(_, s) | TypeExpr::Map(_, _, s) => *s,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Let { name: Ident, ty: Option<TypeExpr>, value: Expr, mutable: bool },
    Assign { target: Expr, op: AssignOp, value: Expr },
    Expr(Expr),
    /// `for a in x` / `for a, b in x`.
    For { first: Ident, second: Option<Ident>, iter: Expr, body: Block },
    While { cond: Expr, body: Block },
    Fail { message: Expr, code: Option<Expr> },
    Return(Option<Expr>),
    Break,
    Continue,
    Exit(Option<Expr>),
    Defer(Expr),
    Native { bash: Option<String>, powershell: Option<String> },
    /// `text` is the original text of the condition.
    Assert { cond: Expr, text: String },
    AssertFails(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
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
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    /// Milliseconds.
    Duration(i64),
    Bool(bool),
    /// `"text {x}"` (or `"""…"""`), with the `{ }` parts already parsed.
    Str(Vec<StrPart>),
    /// `'raw'`.
    RawStr(String),
    Ident(String),
    SelfVal,
    Field(Box<Expr>, Ident),
    Call { callee: Box<Expr>, args: Vec<Arg> },
    Index(Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Range { from: Box<Expr>, to: Box<Expr>, inclusive: bool },
    List(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    /// `{}`: empty map or struct with only defaults; decided by the expected type.
    EmptyBraces,
    StructLit { name: Option<Ident>, spread: Option<Box<Expr>>, fields: Vec<(Ident, Expr)> },
    Lambda { params: Vec<Ident>, body: LambdaBody },
    If { cond: Box<Expr>, then: Block, els: Option<Block> },
    Match { subject: Box<Expr>, arms: Vec<MatchArm> },
    Or(Box<Expr>, Box<Handler>),
    /// `$ cmd` (with `value | $ cmd` in `stdin`).
    Command(Box<CmdAst>),
    /// `$(cmd)`.
    Capture(Box<CmdAst>),
    /// `$ cmd` as an `if`/`while` condition.
    CmdTest(Box<CmdAst>),
    Try(Block),
    Retry { times: Box<Expr>, every: Option<Box<Expr>>, body: Block },
    Parallel { max: Option<Box<Expr>>, body: ParBody },
    /// Marker after a syntax error that was already reported.
    Error,
}

#[derive(Clone, Debug)]
pub enum StrPart {
    Lit(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub enum LambdaBody {
    Expr(Box<Expr>),
    Block(Block),
}

#[derive(Clone, Debug)]
pub enum ParBody {
    For { var: Ident, iter: Box<Expr>, body: Block },
    Block(Block),
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub patterns: Vec<Pattern>,
    /// `pattern if cond => …`
    pub guard: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    /// `str`, `int`, `duration` or `bool` literal.
    Lit(Expr),
    Range { from: i64, to: i64, inclusive: bool, span: Span },
    /// An enum case without the type name, or (for other types) a name that binds the value.
    Case(Ident),
    Wildcard(Span),
    /// `Service { port: 443, name: n }`: the fields not written can be anything.
    Struct { name: Ident, fields: Vec<(Ident, Pattern)>, span: Span },
}

#[derive(Clone, Debug)]
pub enum Handler {
    Ignore(Span),
    Fail { message: Expr, code: Option<Expr>, span: Span },
    Block(Block),
    Value(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedirKind {
    /// `>`
    Out,
    /// `>>`
    Append,
    /// `2>`
    Err,
    /// `2>>`
    ErrAppend,
    /// `<`
    In,
    /// `2>&1`
    ErrToOut,
}

#[derive(Clone, Debug)]
pub struct CmdAst {
    pub stages: Vec<Vec<CmdWord>>,
    pub redirects: Vec<CmdRedir>,
    pub stdin: Option<Expr>,
    /// The original text (without the `$`), for `--verbose`/`--dry-run`.
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct CmdWord {
    pub parts: Vec<WordPart>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum WordPart {
    Lit(String),
    /// `{expr}`; `quoted` = came from inside `"…"`.
    Expr { expr: Expr, quoted: bool },
}

#[derive(Clone, Debug)]
pub struct CmdRedir {
    pub kind: RedirKind,
    pub target: Option<CmdWord>,
    pub span: Span,
}
