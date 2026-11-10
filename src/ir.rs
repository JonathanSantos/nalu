//! Nalu IR: the **typed, flattened and unambiguous** tree that both backends consume.
//! FROZEN CONTRACT (v0.1). Produced by `lower.rs`; read by `bash/` and `ps/`.
//!
//! # The rules that keep the backends thin
//!
//! 1. **Expressions are pure, infallible and cheap.** [`Expr`] only has literals, variables, fields, index,
//!    arithmetic, comparison, logic, interpolation and `len`. Every call, capture, command and collection
//!    construction is an [`Rvalue`], which only appears in `Let`, `Assign` and `Call` (result discarded).
//!    The lowering creates temporary variables (`__t1`, `__t2`…) to flatten expressions (A-normal form).
//! 2. **Fallibility is explicit.** Every `Rvalue::Call`, `Rvalue::Capture`, `Stmt::Command`, `Stmt::Try`,
//!    `Stmt::Retry` and `Stmt::Parallel` carries an [`OnFail`]. `OnFail::Propagate` means: in a function, the function
//!    fails; inside `try`, jump to the handler; inside `retry`, try again; inside `parallel`,
//!    collect the error; at the top of the script, abort with `error: message` on stderr and the error's code.
//! 3. **Variable names are unique** within each function and at the top (the lowering renames shadows: `x`, `x_2`).
//!    The backend only needs a syntactic mangle (prefix or character replacement), never scope resolution.
//!    Top-level variables are visible (read and, if `mutable`, write) inside functions.
//! 4. **`os` is already resolved**: the dead branch for the other system does not exist in the IR. `embed` is already a literal.
//! 5. **Sugar is already rewritten**: `+=` became `Assign` with `Binary`; `else if` became an `If` nested in `els`;
//!    named arguments and defaults are already filled in and ordered (every `Call.args` is positional and complete);
//!    `if`/`match` as an expression became `Let` + `If`/`Match` that assign the variable;
//!    `a..b` in `for` became `ForRange`; conditions with calls in `while` became `While { cond: true }` + `If !c { Break }`.
//! 6. **Collections only live in variables and fields.** An `Expr` of list/map/struct type is always `Var` or `Field`.
//!    Assigning a collection to another variable **copies** it (by value), on both targets.
//! 7. **Types are always present.** Every `Expr` carries `ty`; every `Let` carries `ty`; every `Call` carries `ret`.
//!
//! Any change here must be **additive** (a new variant with `// v0.2`) and noted in
//! `docs/history/specs/CHANGELOG-ir.md`; the three module owners depend on this file.

use crate::target::Target;

pub type Name = String;

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Bool,
    Path,
    /// Stored in milliseconds (integer). As text: `1m30s`, `250ms`, `0s`.
    Duration,
    /// Never becomes text; it only comes from `param`, `secret(x)`, `env`, `ui.password` and leaves through a command's stdin or to another function.
    Secret,
    /// A handler's error value: fields `message: Str` and `code: Int`.
    Error,
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Struct(Name),
    Enum(Name),
    /// No value (functions that do not return).
    Unit,
}

impl Ty {
    pub fn list(t: Ty) -> Ty {
        Ty::List(Box::new(t))
    }
    pub fn map(k: Ty, v: Ty) -> Ty {
        Ty::Map(Box::new(k), Box::new(v))
    }
    /// Scalar = fits in a simple shell variable (text or number).
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Ty::List(_) | Ty::Map(_, _) | Ty::Struct(_) | Ty::Error | Ty::Unit)
    }
    /// Text for messages and `--help`: `str`, `int`, `[]str`, `map[str]int`, `Service`.
    pub fn display(&self) -> String {
        match self {
            Ty::Str => "str".into(),
            Ty::Int => "int".into(),
            Ty::Bool => "bool".into(),
            Ty::Path => "path".into(),
            Ty::Duration => "duration".into(),
            Ty::Secret => "secret".into(),
            Ty::Error => "error".into(),
            Ty::List(t) => format!("[]{}", t.display()),
            Ty::Map(k, v) => format!("map[{}]{}", k.display(), v.display()),
            Ty::Struct(n) | Ty::Enum(n) => n.clone(),
            Ty::Unit => "unit".into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Program
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Program {
    /// Script name (the file stem): `backup`. Used in `--help` and the header.
    pub name: String,
    pub source_file: String,
    pub target: Target,
    /// The full `--help` text (usage, parameters, default flags), already formatted by the frontend,
    /// **without** the final line break. Identical on the three targets.
    pub usage: String,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub params: Vec<Param>,
    /// User functions and methods (a method becomes the function `Type.method`, with `self` as the first parameter).
    pub funcs: Vec<Func>,
    /// Top-level statements, in order.
    pub body: Block,
    pub tests: Vec<TestDef>,
    /// External programs with a fixed name used in commands (`git`, `docker`…), without repeats, in order of
    /// appearance. The script checks at the start that they exist (code 127 if missing); skipped under `--dry-run`.
    /// Shell built-in commands (`cd`, `echo`, `test`, `[`…) are not included.
    pub deps: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Param {
    /// Name inside the script (snake_case): `clear_cache`.
    pub name: Name,
    /// The command-line option (kebab-case, with `--`): `--clear-cache`.
    pub flag: String,
    /// `Str`, `Int`, `Bool`, `Path`, `Duration`, `Secret`, `Enum`, `List(Str)`, `List(Int)`.
    pub ty: Ty,
    /// No default and a type other than `Bool` = required. `Bool` with no default = `false`.
    pub default: Option<Rvalue>,
    /// The `///` above the param (one line; empty if there is none).
    pub doc: String,
    /// For `Secret`: also read from this environment variable (`NALU_TOKEN` for `token`).
    pub env_var: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: Name,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: Name,
    /// Scalar, `List(scalar)` or `Map(scalar, scalar)` in v0.1.
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: Name,
    pub cases: Vec<Name>,
}

#[derive(Clone, Debug)]
pub struct Func {
    /// `read_version`, or `Version.next` for a method.
    pub name: Name,
    /// For a method, the first parameter is `self` (type `Struct`).
    pub params: Vec<FParam>,
    /// `Ty::Unit` when it returns no value.
    pub ret: Ty,
    pub body: Block,
    /// `true` if some path can fail (propagate). Inferred by the checker.
    pub fallible: bool,
    /// `///` documentation (may be empty).
    pub doc: String,
}

#[derive(Clone, Debug)]
pub struct FParam {
    pub name: Name,
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub struct TestDef {
    pub name: String,
    pub body: Block,
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

// ---------------------------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Stmt {
    /// Source line (from 1), for the `# line N` comment and for runtime error messages.
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    /// Declares (and initializes) a variable. `mutable` = `var`; `false` = `let`.
    Let { var: Name, ty: Ty, mutable: bool, value: Rvalue },
    /// Assignment to something already declared (`var`), including fields, list indexes and map entries.
    Assign { target: LValue, value: Rvalue },
    /// Call with its result discarded (`print`, `log.info`, `fs.write`, a user function…).
    Call { call: Call, on_fail: OnFail },
    /// `$ cmd …` as a statement.
    Command { cmd: Command, on_fail: OnFail },
    If { cond: Expr, then: Block, els: Option<Block> },
    While { cond: Expr, body: Block },
    /// `for var in from..to` (`inclusive` = `..=`). `from`/`to` are evaluated once.
    ForRange { var: Name, from: Expr, to: Expr, inclusive: bool, body: Block },
    /// `for x in list` / `for i, x in list`. `list` is `Var` or `Field`; the body sees a **copy** of each item.
    ForList { var: Name, index: Option<Name>, list: Expr, body: Block },
    /// `for k, v in map`, in insertion order.
    ForMap { key: Name, value: Name, map: Expr, body: Block },
    /// `match` as a statement. No fallthrough. `default` = `_ =>`. Exhaustiveness already checked.
    Match { subject: Expr, arms: Vec<Arm>, default: Option<Block> },
    /// `try { body } or …`. Inside `body`, `Propagate` jumps to `on_fail`.
    Try { body: Block, on_fail: OnFail },
    /// `retry(times, every: d) { body }`. Re-runs `body` while it fails, up to `times` times, waiting
    /// `every` between attempts. After the last one, fails with the last error → `on_fail`.
    Retry { times: Expr, every: Expr, body: Block, on_fail: OnFail },
    /// `parallel …`. See [`ParallelBody`]. Waits for **all** the tasks; if any failed, fails with all
    /// the messages together (`parallel: msg1; msg2`) → `on_fail`.
    Parallel { max: Option<Expr>, body: ParallelBody, on_fail: OnFail },
    /// `fail "msg"` / `fail("msg", code: n)` / `fail err`. Default `code`: 1. `message` may use `CurrentErr`.
    Fail { message: Expr, code: Option<Expr> },
    /// `return` (with a value only in functions with `ret != Unit`). The value is an `Expr` (collections: `Var`/`Field`).
    Return(Option<Expr>),
    Break,
    Continue,
    /// `exit(code)` (default 0). Runs the pending `defer`s (from the current function up to the top).
    Exit(Option<Expr>),
    /// `defer <call | command>`. Runs when leaving the function (or the script), in reverse order, including on error,
    /// `exit` and Ctrl+C. Arguments are evaluated at the moment of the `defer`. A failure inside the defer is ignored
    /// (the inner `on_fail` is always `Ignore`).
    Defer(Box<Stmt>),
    /// The shell's own code, fixed text, not analyzed. Both must be present.
    Native { bash: String, powershell: String },
    /// Only in `test`. `text` is the original piece of the condition, for the message `assert failed: …`.
    Assert { cond: Expr, text: String },
    /// Only in `test`: passes if the inner statement (`Call` or `Command` with `Propagate`) **fails**.
    AssertFails(Box<Stmt>),
}

#[derive(Clone, Debug)]
pub enum LValue {
    Var(Name),
    /// `s.field = v` (struct in a `var`).
    Field { base: Name, field: Name },
    /// `xs[i] = v`.
    Index { list: Name, index: Expr },
    /// `m[k] = v` (inserts or replaces).
    MapEntry { map: Name, key: Expr },
}

#[derive(Clone, Debug)]
pub struct Arm {
    /// Several patterns = `a | b`. Never empty.
    pub patterns: Vec<Pattern>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    /// A literal: `Str`, `Int`, `Duration`, `Bool` or `EnumCase`.
    Lit(Expr),
    /// `from..to` (`inclusive` = `..=`), only for `Int`.
    Range { from: Expr, to: Expr, inclusive: bool },
}

/// What to do when the operation fails.
#[derive(Clone, Debug)]
pub enum OnFail {
    /// Hand it to the context (see rule 2 at the top of the file).
    Propagate,
    /// Carry on. Only on a `Unit` result.
    Ignore,
    /// Use this value instead (same type). May itself be fallible (`a or b or c` is `Default(b or c)`).
    Default(Box<Rvalue>),
    /// Fail with another message/code. `message` and `code` may use `CurrentErr`.
    Fail { message: Expr, code: Option<Expr> },
    /// Runs `body` with the error in `err` (when `Some`). In a value context, `value` is the result of the block
    /// (evaluated after `body`); `body` may instead end with `Return`, `Fail`, `Exit`, `Break`
    /// or `Continue`, and then `value` is `None`.
    Block { err: Option<Name>, body: Block, value: Option<Expr> },
}

#[derive(Clone, Debug)]
pub enum ParallelBody {
    /// `parallel for var in list { body }`. If `collect` is `Some((name, expr))`, each task produces `expr`
    /// (evaluated at the end of the body) and `name` (a list variable **already declared** earlier with `Let`) receives the
    /// results **in list order**. The body does not change outside variables (guaranteed by the checker).
    For { var: Name, list: Expr, body: Block, collect: Option<(Name, Expr)> },
    /// `parallel { a(); b() }`: each block is a task.
    Tasks(Vec<Block>),
}

// ---------------------------------------------------------------------------------------------
// Right-hand values (can fail, can be collections)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Rvalue {
    /// A pure expression (including a collection `Var`/`Field`, which the backend **copies**).
    Expr(Expr),
    /// `[a, b, c]`. Items are `Expr`s (nested collections: `Var`).
    List { elem_ty: Ty, items: Vec<Expr> },
    /// `{ "a": 1 }`. Keys are `Str`, `Int` or `Enum`.
    Map { key_ty: Ty, val_ty: Ty, entries: Vec<(Expr, Expr)> },
    /// `Type { field: value, ..base }`. Without `spread`, **all** the fields are in `fields` (the lowering
    /// fills in the defaults). With `spread` (a `Var` of the same struct), the missing ones come from it.
    Struct { ty: Name, spread: Option<Expr>, fields: Vec<(Name, Expr)> },
    /// A call that produces a value.
    Call { call: Call, on_fail: OnFail },
    /// `$(cmd)`: stdout without the trailing line breaks. Type `Str`. Runs **also** under `--dry-run`.
    Capture { cmd: Command, on_fail: OnFail },
    /// `$ cmd` used as a condition: `true` if the exit code is 0. Never fails. Also runs under `--dry-run`.
    CommandOk(Command),
}

#[derive(Clone, Debug)]
pub struct Call {
    pub target: CallTarget,
    /// Positional and complete (defaults already filled in). For `Method` and `Type.method` functions, the receiver is `args[0]`.
    pub args: Vec<Arg>,
    pub ret: Ty,
    pub fallible: bool,
}

#[derive(Clone, Debug)]
pub enum CallTarget {
    /// User function (or method `Type.method`, with `self` in `args[0]`).
    User(Name),
    /// Standard library function: `module` + `name`, exactly as in `stdlib::STDLIB` (`fs`, `read`).
    Std { module: String, name: String },
    /// Builtin without a module: `print`, `secret`, `fmt`, `duration_str` (duration → text), `enum_parse`
    /// (`Type.parse(text)`: `args[0]` is the text; `ret` is the enum). Full list in `stdlib::BUILTINS`.
    Builtin(String),
    /// Method of a built-in type (`str`, `path`, `list`, `map`), exactly as in `stdlib::METHODS`.
    /// `recv_ty` is the receiver's concrete type (`List(Int)`, `Str`…); the receiver is `args[0]`.
    Method { recv_ty: Ty, name: String },
}

#[derive(Clone, Debug)]
pub enum Arg {
    Expr(Expr),
    Lambda(Lambda),
}

/// Only appears as an argument of list methods (`filter`, `map`, `any`, `all`, `find`, `sort_by`).
/// Reads outside variables, does not change them. `body` ends with `Return(Some(expr))` on every path.
#[derive(Clone, Debug)]
pub struct Lambda {
    pub params: Vec<FParam>,
    pub ret: Ty,
    pub body: Block,
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Command {
    /// `a | b | c`: at least one. Fails if any stage fails (`pipefail`).
    pub pipeline: Vec<SimpleCmd>,
    pub redirects: Vec<Redirect>,
    /// `value | $ cmd`: text (or `Secret`) sent to the first command's input, without an extra `\n`.
    pub stdin: Option<Expr>,
    /// The original text of the line (`git add {file}`), for `--verbose` and `--dry-run`. Secrets appear
    /// as `******`; the backend replaces `{…}` with the values when showing it.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct SimpleCmd {
    /// Never empty. `words[0]` is the program.
    pub words: Vec<Word>,
}

#[derive(Clone, Debug)]
pub enum Word {
    /// Literal text (already without quotes). The backend quotes it to become **one** argument, without expansion.
    Lit(String),
    /// Word with interpolation (`api={image}`, `{output}`): always **one** argument.
    Interp(Vec<Part>),
    /// `{list}` alone in the word: becomes **several** arguments (one per item). `Expr` is a list `Var`/`Field`.
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub enum Redirect {
    /// `> p` / `>> p` (stdout). Writes UTF-8 without a BOM and `\n` on all three targets.
    Out { path: Expr, append: bool },
    /// `2> p` / `2>> p`.
    Err { path: Expr, append: bool },
    /// `< p`.
    In(Expr),
    /// `2>&1`.
    ErrToOut,
}

// ---------------------------------------------------------------------------------------------
// Pure expressions
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Ty,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Str(String),
    Int(i64),
    Bool(bool),
    /// Milliseconds.
    Duration(i64),
    /// `Environment.prod`. Represented as the case's text (`prod`) on both targets.
    EnumCase { enum_: Name, case: Name },
    Var(Name),
    /// The `dry_run` constant (`Bool`).
    DryRun,
    /// The `args` list (`List(Str)`): the positional arguments left over on the command line.
    Args,
    /// The error of the handler in effect (`Ty::Error`). Only inside `OnFail::Fail` and `OnFail::Block` (before the `Let err`).
    CurrentErr,
    /// `base.field` (struct or `Error`). `base` is `Var`, `Field` or `Index`.
    Field(Box<Expr>, Name),
    /// `list[i]` (index from 0). The lowering places before it the range check that fails (D13); here the index is already valid.
    Index(Box<Expr>, Box<Expr>),
    /// `.len()` of `Str` (characters), `Path`, list or map. Type `Int`.
    Len(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `"text {x}"`. The `Expr` parts are of type `Str`, `Path`, `Int`, `Bool` (`true`/`false`) or `Enum`
    /// (the case name). Duration and error were already converted by the lowering (`duration_str`, `err.message`).
    Interp(Vec<Part>),
}

#[derive(Clone, Debug)]
pub enum Part {
    Lit(String),
    Expr(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    /// `Int`/`Duration` (ms). `Div` truncates toward zero; `Mod` has the sign of the dividend (like C).
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// **Strict** comparison: both sides have the same type (the operands' `ty` says whether it is text or number).
    /// Text is compared **ordinally** (bytes/code points), never by culture.
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `&&` / `||` with short-circuiting (the operands are pure, so the backend may evaluate both).
    And,
    Or,
    /// `str + str` (the lowering already converted the non-text side with `Interp`).
    Concat,
    /// `path / str`: joins paths with the system separator.
    PathJoin,
}

// ---------------------------------------------------------------------------------------------
// Construction helpers (for backend tests with hand-made IR)
// ---------------------------------------------------------------------------------------------

impl Expr {
    pub fn str(s: &str) -> Expr {
        Expr { kind: ExprKind::Str(s.to_string()), ty: Ty::Str }
    }
    pub fn int(n: i64) -> Expr {
        Expr { kind: ExprKind::Int(n), ty: Ty::Int }
    }
    pub fn bool(b: bool) -> Expr {
        Expr { kind: ExprKind::Bool(b), ty: Ty::Bool }
    }
    pub fn var(name: &str, ty: Ty) -> Expr {
        Expr { kind: ExprKind::Var(name.to_string()), ty }
    }
}

impl Stmt {
    pub fn new(line: u32, kind: StmtKind) -> Stmt {
        Stmt { line, kind }
    }
}

impl Program {
    /// An empty program, useful for tests.
    pub fn empty(name: &str, target: Target) -> Program {
        Program {
            name: name.to_string(),
            source_file: format!("{}.nalu", name),
            target,
            usage: format!("usage: {} [options] [args…]\n\n  --dry-run   show what would happen, without running commands or changing files\n  --verbose   print each command before running it\n  -h, --help  show this help", name),
            structs: vec![],
            enums: vec![],
            params: vec![],
            funcs: vec![],
            body: Block::default(),
            tests: vec![],
            deps: vec![],
        }
    }
}
